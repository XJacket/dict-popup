#![deny(missing_docs)]
#![forbid(unsafe_code)]

//! `dict-store` — the dictionary *library*.
//!
//! A library is one directory that holds installed dictionary packages and one
//! manifest that records what is installed, which version is active, and what
//! happened when:
//!
//! ```text
//! <root>/
//!   library.json          manifest: package records + append-only history
//!   ecdict-2.4.2/         one directory per installed version
//!     source.json         the package metadata, as recorded in the manifest
//!     stardict-ecdict.ifo
//!     stardict-ecdict.idx
//!     stardict-ecdict.dict
//!   ecdict-2.4.1/         older versions stay installed so rollback works
//! ```
//!
//! The public interface is deliberately small and deep: callers talk about
//! *packages* and *versions*, and the implementation owns the file system, the
//! checksums and the manifest merge.
//!
//! # Version control in one paragraph
//!
//! Installing a new version is [`Library::add`] (or [`Library::upgrade`], which
//! also switches the active version and records an `upgraded` event). Old
//! version directories are kept until [`Library::remove`] deletes them, so
//! [`Library::rollback`] can point `active` back at the previous version.
//! Exactly one version per id is active at a time, and it is the active version
//! that a query layer would load.
//!
//! # Metadata
//!
//! [`DictionaryPackage`] is both the manifest record and the `source.json`
//! written next to an installed version's files. [`NewPackage`] is the input
//! form used by [`Library::add`] and [`Library::upgrade`]; it can be resolved
//! from command-line fields merged over the package's own `source.json`.
//!
//! # What this crate does not know
//!
//! Nothing here interprets dictionary files. [`Format`] is an open tag used to
//! pick a reader later; which formats the popup supports is documented in
//! `docs/DICTIONARIES.md`. See also [`provider`] for the reserved download
//! interface.

use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod checksum;
pub mod error;
pub mod manifest;
pub mod provider;
pub mod version;

pub use error::StoreError;
pub use manifest::{EventKind, HistoryEvent, Manifest, NewPackage, PackageMeta};
pub use provider::{
    HttpProvider, LocalDirectoryProvider, Provider, ProviderError, ProviderInfo, RemotePackage,
};

/// Duration of one day in seconds, used by the RFC 3339 helper.
const SECONDS_PER_DAY: u64 = 86_400;

/// A stable dictionary identifier such as `ecdict`.
///
/// The id is validated once, on construction, so every id that exists is safe
/// to use as part of a directory name: it is 1 to 64 bytes, starts with an
/// ASCII lowercase letter or a digit, and contains only ASCII lowercase
/// letters, digits, `-`, `_` and `.`.
///
/// Deserialization goes through [`SourceId::new`], so an invalid id in
/// `library.json` is rejected instead of silently accepted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(String);

impl SourceId {
    /// Longest accepted id, in bytes.
    pub const MAX_LEN: usize = 64;

    /// Validate and wrap a dictionary id.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidSourceId`] when the id is empty, longer than
    /// [`SourceId::MAX_LEN`], does not start with a lowercase letter or digit,
    /// or contains anything other than lowercase ASCII letters, digits, `-`,
    /// `_` and `.`.
    pub fn new(raw: impl Into<String>) -> Result<Self, StoreError> {
        let raw = raw.into();
        if let Some(reason) = source_id_problem(&raw) {
            return Err(StoreError::InvalidSourceId { raw, reason });
        }
        Ok(SourceId(raw))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Return why `raw` is not a valid id, or `None` when it is valid.
fn source_id_problem(raw: &str) -> Option<&'static str> {
    if raw.is_empty() {
        return Some("不能为空");
    }
    if raw.len() > SourceId::MAX_LEN {
        return Some("最长 64 个字节");
    }
    let valid = raw.bytes().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'-'
            || byte == b'_'
            || byte == b'.'
    });
    if !valid {
        return Some("只能使用小写字母、数字和 '-', '_', '.'");
    }
    if raw.starts_with('.') || raw.starts_with('-') {
        return Some("不能以 '.' 或 '-' 开头");
    }
    None
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SourceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for SourceId {
    type Err = StoreError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        SourceId::new(raw)
    }
}

impl Serialize for SourceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        SourceId::new(raw).map_err(serde::de::Error::custom)
    }
}

/// The file layout of a package, as an open tag.
///
/// `stardict`, `jsonl` and `mdx` are spelled out because the spec names them,
/// but the set is open: any other name is preserved verbatim in
/// [`Format::Other`] and round-trips through `library.json` unchanged. No code
/// in this crate looks at file extensions or file contents; the tag only lets a
/// reader be chosen later (see `docs/DICTIONARIES.md`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    /// StarDict (`.ifo` / `.idx` / `.dict`).
    Stardict,
    /// One JSON object per line.
    Jsonl,
    /// MDict (`.mdx`).
    Mdx,
    /// Any other format name, preserved as written (trimmed).
    Other(String),
}

impl Format {
    /// Parse a format name; never fails, unknown names become [`Format::Other`].
    ///
    /// The three known names are matched case-insensitively and normalised to
    /// lowercase; everything else is kept as given (trimmed).
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("stardict") {
            Format::Stardict
        } else if trimmed.eq_ignore_ascii_case("jsonl") {
            Format::Jsonl
        } else if trimmed.eq_ignore_ascii_case("mdx") {
            Format::Mdx
        } else {
            Format::Other(trimmed.to_string())
        }
    }

    /// The format name as it is stored in `library.json`.
    pub fn as_str(&self) -> &str {
        match self {
            Format::Stardict => "stardict",
            Format::Jsonl => "jsonl",
            Format::Mdx => "mdx",
            Format::Other(name) => name,
        }
    }

    /// Whether this is one of the formats this crate spells out.
    pub fn is_known(&self) -> bool {
        !matches!(self, Format::Other(_))
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Format {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Format {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Format::parse(&raw))
    }
}

/// One installed version of one dictionary, exactly as recorded in
/// `library.json` and as copied into the version directory's `source.json`.
///
/// `license` is mandatory: a package without a licence cannot be stored, so the
/// library never contains data whose redistribution terms are unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryPackage {
    /// Stable dictionary id; unchanged across versions.
    pub id: SourceId,
    /// Version string; compared with [`version::compare`].
    pub version: String,
    /// File layout tag.
    pub format: Format,
    /// Display name.
    pub name: String,
    /// Licence (SPDX identifier or free text). Must not be empty.
    pub license: String,
    /// Source homepage, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Lowercase hex SHA-256 of the package directory, computed by
    /// [`checksum::sha256_dir`].
    pub sha256: String,
    /// Total size of the hashed files, in bytes.
    pub bytes: u64,
    /// Number of headwords, when the source states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<u64>,
    /// Install time, RFC 3339 in UTC with millisecond precision
    /// (`2026-10-27T05:31:12.345Z`).
    pub installed_at: String,
    /// Whether this version is the one a query layer should load.
    pub active: bool,
}

impl DictionaryPackage {
    /// Name of this package's directory inside the library root:
    /// `<id>-<version>`.
    pub fn directory_name(&self) -> String {
        format!("{}-{}", self.id, self.version)
    }

    /// Check that every field is usable and safe to store.
    ///
    /// A package must have a valid id and version, a non-empty name, licence
    /// and format, a 64-character lowercase hex `sha256`, and a non-empty
    /// `installed_at`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidVersion`],
    /// [`StoreError::InvalidField`] or [`StoreError::InvalidSourceId`] naming
    /// the field that is wrong.
    pub fn validate(&self) -> Result<(), StoreError> {
        version::validate(&self.version)?;
        if source_id_problem(self.id.as_str()).is_some() {
            return Err(StoreError::InvalidSourceId {
                raw: self.id.to_string(),
                reason: "格式不合法",
            });
        }
        validate_text_fields(&self.format, &self.name, &self.license)?;
        if !is_sha256_hex(&self.sha256) {
            return Err(StoreError::InvalidField {
                field: "sha256",
                value: self.sha256.clone(),
                reason: "应为 64 位小写十六进制",
            });
        }
        if self.installed_at.trim().is_empty() {
            return Err(StoreError::InvalidField {
                field: "installed_at",
                value: self.installed_at.clone(),
                reason: "不能为空",
            });
        }
        Ok(())
    }
}

/// Shared checks for the display fields of a package.
pub(crate) fn validate_text_fields(
    format: &Format,
    name: &str,
    license: &str,
) -> Result<(), StoreError> {
    if format.as_str().trim().is_empty() {
        return Err(StoreError::InvalidField {
            field: "format",
            value: format.to_string(),
            reason: "不能为空",
        });
    }
    if name.trim().is_empty() {
        return Err(StoreError::InvalidField {
            field: "name",
            value: name.to_string(),
            reason: "不能为空",
        });
    }
    if license.trim().is_empty() {
        return Err(StoreError::InvalidField {
            field: "license",
            value: license.to_string(),
            reason: "许可证必填，不能为空",
        });
    }
    Ok(())
}

/// `true` when the string is exactly 64 lowercase hexadecimal characters.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An open dictionary library rooted at one directory.
///
/// Opening a library reads and validates `<root>/library.json` but creates
/// nothing: the root directory and the manifest appear with the first write.
#[derive(Debug)]
pub struct Library {
    root: PathBuf,
    manifest: Manifest,
}

impl Library {
    /// Open the library rooted at `root`.
    ///
    /// A missing `library.json` is an empty library. A manifest that cannot be
    /// parsed or is semantically invalid is an error and is left untouched.
    /// The root directory itself is not created here.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ManifestUnparsable`] for unreadable JSON,
    /// [`StoreError::ManifestInvalid`] for a manifest this version cannot use,
    /// and [`StoreError::Io`] when the file exists but cannot be read.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let manifest = Manifest::load(&root.join(manifest::MANIFEST_FILE))?;
        Ok(Library { root, manifest })
    }

    /// The library root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of this library's `library.json`.
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(manifest::MANIFEST_FILE)
    }

    /// Every installed package, sorted by id and then by version.
    pub fn list(&self) -> &[DictionaryPackage] {
        &self.manifest.packages
    }

    /// The active version of `id`, if any.
    pub fn active(&self, id: &SourceId) -> Option<&DictionaryPackage> {
        self.manifest.active_of(id)
    }

    /// History events, newest last; filtered to `id` when one is given.
    pub fn history(&self, id: Option<&SourceId>) -> Vec<&HistoryEvent> {
        self.manifest
            .history
            .iter()
            .filter(|event| match id {
                Some(id) => &event.id == id,
                None => true,
            })
            .collect()
    }

    /// Install a package version from a directory.
    ///
    /// The directory is copied to `<root>/<id>-<version>/`, its digest and size
    /// are recorded, and an `installed` event is appended. The copy skips a
    /// `source.json` at the top of `source_dir` because the library writes its
    /// own metadata copy.
    ///
    /// The version becomes active when it is the first version of that id;
    /// otherwise it is installed inactive (use [`Library::upgrade`] to switch,
    /// or [`Library::activate`]).
    ///
    /// # Refusals
    ///
    /// - identical `(id, version, sha256)` — always a [`StoreError::DuplicatePackage`];
    /// - same `(id, version)` but a different digest — requires `force`;
    /// - a `<id>-<version>` directory that no record points at — requires
    ///   `force`.
    ///
    /// # Errors
    ///
    /// Any of the above, plus [`StoreError::MissingMetadata`] /
    /// [`StoreError::InvalidField`] for unusable metadata and
    /// [`StoreError::Io`] for file-system failures. When writing the manifest
    /// fails, the installed directory is rolled back.
    pub fn add(
        &mut self,
        source_dir: &Path,
        package: &NewPackage,
        force: bool,
    ) -> Result<DictionaryPackage, StoreError> {
        package.validate()?;
        let digest = checksum::summarize_dir(source_dir)?;
        let existing = self
            .manifest
            .find(&package.id, &package.version)
            .cloned();

        if let Some(previous) = &existing {
            if previous.sha256 == digest.sha256 {
                return Err(StoreError::DuplicatePackage {
                    id: package.id.clone(),
                    version: package.version.clone(),
                    sha256: digest.sha256,
                });
            }
            if !force {
                return Err(StoreError::SameVersionDifferentContent {
                    id: package.id.clone(),
                    version: package.version.clone(),
                });
            }
        }

        let directory = self.root.join(format!("{}-{}", package.id, package.version));
        if directory.exists() && existing.is_none() && !force {
            return Err(StoreError::VersionDirectoryExists { path: directory });
        }

        let active = match &existing {
            Some(previous) => previous.active,
            None => self.manifest.active_of(&package.id).is_none(),
        };
        let installed = package.install(digest.sha256, digest.bytes, active);

        let detail = existing
            .as_ref()
            .map(|_| "同一版本内容被 --force 替换".to_string());
        let mut next = self.manifest.clone();
        next.upsert(installed.clone());
        next.push_event(self.event(
            EventKind::Installed,
            &installed.id,
            Some(&installed.version),
            detail,
        ));
        next.sort_packages();

        let guard = install_package_dir(source_dir, &self.root, &installed)?;
        self.commit_install(next, guard)?;
        Ok(installed)
    }

    /// Install a newer version and make it the active one.
    ///
    /// The new version directory is added, every other version of the id is
    /// marked inactive, and an `upgraded` event records the transition from the
    /// previous newest version. Old directories are kept for rollback.
    ///
    /// # Refusals
    ///
    /// - unknown id — [`StoreError::PackageNotFound`];
    /// - an identical `(id, version, sha256)` — [`StoreError::DuplicatePackage`];
    /// - the same version with a different digest, or a version that does not
    ///   sort strictly after the installed newest one — requires `force`.
    ///
    /// # Errors
    ///
    /// Any of the above, plus the metadata and I/O errors documented on
    /// [`Library::add`].
    pub fn upgrade(
        &mut self,
        source_dir: &Path,
        package: &NewPackage,
        force: bool,
    ) -> Result<DictionaryPackage, StoreError> {
        package.validate()?;
        let newest = self
            .manifest
            .newest_of(&package.id)
            .cloned()
            .ok_or_else(|| StoreError::PackageNotFound {
                id: package.id.clone(),
            })?;
        let digest = checksum::summarize_dir(source_dir)?;

        if let Some(previous) = self.manifest.find(&package.id, &package.version) {
            if previous.sha256 == digest.sha256 {
                return Err(StoreError::DuplicatePackage {
                    id: package.id.clone(),
                    version: package.version.clone(),
                    sha256: digest.sha256,
                });
            }
            if !force {
                return Err(StoreError::SameVersionDifferentContent {
                    id: package.id.clone(),
                    version: package.version.clone(),
                });
            }
        }
        if !version::is_newer(&package.version, &newest.version) && !force {
            return Err(StoreError::VersionNotNewer {
                id: package.id.clone(),
                current: newest.version,
                incoming: package.version.clone(),
            });
        }

        let installed = package.install(digest.sha256, digest.bytes, true);
        let detail = if newest.version == installed.version {
            format!("同版本内容替换（{}）", installed.version)
        } else {
            format!("{} -> {}", newest.version, installed.version)
        };

        let mut next = self.manifest.clone();
        for existing in next.packages.iter_mut() {
            if existing.id == installed.id {
                existing.active = false;
            }
        }
        next.upsert(installed.clone());
        next.push_event(self.event(
            EventKind::Upgraded,
            &installed.id,
            Some(&installed.version),
            Some(detail),
        ));
        next.sort_packages();

        let guard = install_package_dir(source_dir, &self.root, &installed)?;
        self.commit_install(next, guard)?;
        Ok(installed)
    }

    /// Point `active` back at the newest installed version older than the
    /// currently active one, appending a `rolled_back` event.
    ///
    /// # Errors
    ///
    /// [`StoreError::NoActiveVersion`] when nothing is active, and
    /// [`StoreError::NoPreviousVersion`] when the active version is already the
    /// oldest installed one.
    pub fn rollback(&mut self, id: &SourceId) -> Result<DictionaryPackage, StoreError> {
        let active = self
            .manifest
            .active_of(id)
            .cloned()
            .ok_or_else(|| StoreError::NoActiveVersion { id: id.clone() })?;
        let previous = self
            .manifest
            .versions_of(id)
            .into_iter()
            .rev()
            .find(|candidate| version::compare(&candidate.version, &active.version) == Ordering::Less)
            .cloned()
            .ok_or_else(|| StoreError::NoPreviousVersion {
                id: id.clone(),
                current: active.version.clone(),
            })?;

        let mut next = self.manifest.clone();
        set_active(&mut next.packages, id, Some(&previous.version));
        next.push_event(self.event(
            EventKind::RolledBack,
            id,
            Some(&previous.version),
            Some(format!("{} -> {}", active.version, previous.version)),
        ));
        next.sort_packages();
        self.persist(next)?;

        let mut rolled_back = previous;
        rolled_back.active = true;
        Ok(rolled_back)
    }

    /// Make `version` of `id` the active version, appending an `activated`
    /// event.
    ///
    /// Activating the already active version is allowed and still recorded,
    /// because the history is a log of user actions rather than a diff.
    ///
    /// # Errors
    ///
    /// [`StoreError::VersionNotFound`] when that version is not installed.
    pub fn activate(
        &mut self,
        id: &SourceId,
        version: &str,
    ) -> Result<DictionaryPackage, StoreError> {
        let found = self
            .manifest
            .find(id, version)
            .cloned()
            .ok_or_else(|| StoreError::VersionNotFound {
                id: id.clone(),
                version: version.to_string(),
            })?;

        let mut next = self.manifest.clone();
        set_active(&mut next.packages, id, Some(version));
        next.push_event(self.event(EventKind::Activated, id, Some(version), None));
        next.sort_packages();
        self.persist(next)?;

        match self.manifest.find(id, version) {
            Some(package) => Ok(package.clone()),
            None => Ok(found),
        }
    }

    /// Mark every version of `id` inactive, appending a `deactivated` event.
    ///
    /// Returns the packages that were deactivated; an id with no active version
    /// returns an empty vector and records nothing.
    ///
    /// # Errors
    ///
    /// [`StoreError::PackageNotFound`] when the id is not installed at all.
    pub fn deactivate(&mut self, id: &SourceId) -> Result<Vec<DictionaryPackage>, StoreError> {
        if self.manifest.versions_of(id).is_empty() {
            return Err(StoreError::PackageNotFound { id: id.clone() });
        }
        let active: Vec<DictionaryPackage> = self
            .manifest
            .versions_of(id)
            .into_iter()
            .filter(|package| package.active)
            .cloned()
            .collect();
        if active.is_empty() {
            return Ok(Vec::new());
        }

        let version = active.first().map(|package| package.version.clone());
        let mut next = self.manifest.clone();
        set_active(&mut next.packages, id, None);
        next.push_event(self.event(
            EventKind::Deactivated,
            id,
            version.as_deref(),
            None,
        ));
        next.sort_packages();
        self.persist(next)?;
        Ok(active)
    }

    /// Delete one installed version, or every version of an id.
    ///
    /// Each deleted version directory is removed and one `removed` event is
    /// appended per version. When an active version is deleted, the newest
    /// remaining version of that id is automatically activated (and an
    /// `activated` event is appended), or the id is left with no versions.
    ///
    /// The directories are moved aside first and only deleted after the
    /// manifest has been written, so a failed manifest write puts them back.
    ///
    /// # Errors
    ///
    /// [`StoreError::VersionNotFound`] for a missing requested version,
    /// [`StoreError::PackageNotFound`] when the id has no versions, and
    /// [`StoreError::Io`] for file-system failures.
    pub fn remove(
        &mut self,
        id: &SourceId,
        version: Option<&str>,
    ) -> Result<Vec<DictionaryPackage>, StoreError> {
        let targets: Vec<DictionaryPackage> = match version {
            Some(requested) => vec![self
                .manifest
                .find(id, requested)
                .cloned()
                .ok_or_else(|| StoreError::VersionNotFound {
                    id: id.clone(),
                    version: requested.to_string(),
                })?],
            None => {
                let all: Vec<DictionaryPackage> = self
                    .manifest
                    .versions_of(id)
                    .into_iter()
                    .cloned()
                    .collect();
                if all.is_empty() {
                    return Err(StoreError::PackageNotFound { id: id.clone() });
                }
                all
            }
        };
        let removed_active = targets.iter().any(|package| package.active);

        let mut next = self.manifest.clone();
        next.packages.retain(|package| {
            !targets
                .iter()
                .any(|target| target.id == package.id && target.version == package.version)
        });
        for target in &targets {
            next.push_event(self.event(
                EventKind::Removed,
                &target.id,
                Some(&target.version),
                None,
            ));
        }
        if removed_active {
            if let Some(newest) = next.newest_of(id).map(|package| package.version.clone()) {
                set_active(&mut next.packages, id, Some(&newest));
                next.push_event(self.event(
                    EventKind::Activated,
                    id,
                    Some(&newest),
                    Some("删除启用版本后自动启用最新剩余版本".to_string()),
                ));
            }
        }
        next.sort_packages();

        // Move directories aside; they are deleted only after the manifest is
        // durable, and moved back if the write fails.
        let mut trashed: Vec<(PathBuf, PathBuf)> = Vec::new();
        for target in &targets {
            let directory = self.root.join(target.directory_name());
            if !directory.exists() {
                continue;
            }
            let trash = self.root.join(format!(
                ".trash-{}-{}",
                target.directory_name(),
                process::id()
            ));
            if trash.exists() {
                fs::remove_dir_all(&trash).map_err(|error| StoreError::io(&trash, error))?;
            }
            match fs::rename(&directory, &trash) {
                Ok(()) => trashed.push((directory, trash)),
                Err(error) => {
                    restore_trash(&trashed);
                    return Err(StoreError::io(&directory, error));
                }
            }
        }

        match manifest::save(&self.manifest_path(), &next) {
            Ok(()) => {
                for (_, trash) in &trashed {
                    let _ = fs::remove_dir_all(trash);
                }
                self.manifest = next;
                Ok(targets)
            }
            Err(error) => {
                restore_trash(&trashed);
                Err(error)
            }
        }
    }

    /// Write `next` as the manifest and adopt it as the in-memory state.
    fn persist(&mut self, next: Manifest) -> Result<(), StoreError> {
        manifest::save(&self.manifest_path(), &next)?;
        self.manifest = next;
        Ok(())
    }

    /// Write the manifest for an install and delete the backup directory on
    /// success; on failure the guard's `Drop` puts the previous directory back.
    fn commit_install(
        &mut self,
        next: Manifest,
        guard: InstallGuard,
    ) -> Result<(), StoreError> {
        manifest::save(&self.manifest_path(), &next)?;
        guard.commit();
        self.manifest = next;
        Ok(())
    }

    /// Append one history event with the current time.
    fn event(
        &self,
        kind: EventKind,
        id: &SourceId,
        version: Option<&str>,
        detail: Option<String>,
    ) -> HistoryEvent {
        HistoryEvent {
            at: now_rfc3339(),
            event: kind,
            id: id.clone(),
            version: version.map(str::to_string),
            detail,
        }
    }
}

/// Make `version` (or nothing) the only active version of `id`.
fn set_active(packages: &mut [DictionaryPackage], id: &SourceId, version: Option<&str>) {
    for package in packages.iter_mut() {
        if &package.id == id {
            package.active = Some(package.version.as_str()) == version;
        }
    }
}

/// Put directories moved aside by [`Library::remove`] back where they were.
fn restore_trash(trashed: &[(PathBuf, PathBuf)]) {
    for (directory, trash) in trashed.iter().rev() {
        let _ = fs::rename(trash, directory);
    }
}

/// A freshly committed install whose backup (if any) must be kept until the
/// manifest write succeeds.
#[derive(Debug)]
struct InstallGuard {
    final_dir: PathBuf,
    backup_dir: Option<PathBuf>,
    committed: bool,
}

impl InstallGuard {
    /// Accept the install: the new directory stays and the backup is deleted.
    fn commit(mut self) {
        self.committed = true;
        if let Some(backup) = self.backup_dir.take() {
            let _ = fs::remove_dir_all(backup);
        }
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Roll back: drop the new directory and restore the previous one.
        let _ = fs::remove_dir_all(&self.final_dir);
        if let Some(backup) = self.backup_dir.take() {
            let _ = fs::rename(backup, &self.final_dir);
        }
    }
}

/// Copy `source_dir` into `<root>/<id>-<version>/` through a staging directory,
/// writing the package's `source.json`.
///
/// Staging means an interrupted copy never leaves a half-written version
/// directory in place, and an existing same-version directory is only replaced
/// after the new content is complete.
fn install_package_dir(
    source_dir: &Path,
    root: &Path,
    package: &DictionaryPackage,
) -> Result<InstallGuard, StoreError> {
    let directory_name = package.directory_name();
    let final_dir = root.join(&directory_name);
    let staged_dir = root.join(format!(".staging-{}-{}", directory_name, process::id()));

    if staged_dir.exists() {
        fs::remove_dir_all(&staged_dir).map_err(|error| StoreError::io(&staged_dir, error))?;
    }
    if let Err(error) = copy_package_tree(source_dir, &staged_dir, true) {
        let _ = fs::remove_dir_all(&staged_dir);
        return Err(error);
    }
    let metadata_path = staged_dir.join(checksum::METADATA_FILE);
    let encoded = serde_json::to_vec_pretty(package).map_err(|source| StoreError::JsonWrite {
        path: metadata_path.clone(),
        source,
    });
    let encoded = match encoded {
        Ok(encoded) => encoded,
        Err(error) => {
            let _ = fs::remove_dir_all(&staged_dir);
            return Err(error);
        }
    };
    if let Err(error) = fs::write(&metadata_path, encoded) {
        let _ = fs::remove_dir_all(&staged_dir);
        return Err(StoreError::io(&metadata_path, error));
    }

    let backup_dir = if final_dir.exists() {
        let backup = root.join(format!(".backup-{}-{}", directory_name, process::id()));
        if backup.exists() {
            fs::remove_dir_all(&backup).map_err(|error| StoreError::io(&backup, error))?;
        }
        if let Err(error) = fs::rename(&final_dir, &backup) {
            let _ = fs::remove_dir_all(&staged_dir);
            return Err(StoreError::io(&final_dir, error));
        }
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(&staged_dir, &final_dir) {
        if let Some(backup) = &backup_dir {
            let _ = fs::rename(backup, &final_dir);
        }
        let _ = fs::remove_dir_all(&staged_dir);
        return Err(StoreError::io(&final_dir, error));
    }

    Ok(InstallGuard {
        final_dir,
        backup_dir,
        committed: false,
    })
}

/// Recursively copy `source` into `dest`.
///
/// `dest` is created if needed and existing files are overwritten. Directories
/// are recreated; regular files are copied byte for byte. When
/// `skip_top_metadata` is true, a `source.json` directly at the top of
/// `source` is not copied, because the library writes its own metadata copy
/// (this is the same file [`checksum::sha256_dir`] skips).
pub(crate) fn copy_package_tree(
    source: &Path,
    dest: &Path,
    skip_top_metadata: bool,
) -> Result<(), StoreError> {
    let metadata = fs::metadata(source).map_err(|error| StoreError::io(source, error))?;
    if !metadata.is_dir() {
        return Err(StoreError::NotADirectory {
            path: source.to_path_buf(),
        });
    }
    fs::create_dir_all(dest).map_err(|error| StoreError::io(dest, error))?;
    copy_directory_contents(source, dest, skip_top_metadata)
}

/// One level of [`copy_package_tree`]'s recursion.
fn copy_directory_contents(
    source: &Path,
    dest: &Path,
    skip_top_metadata: bool,
) -> Result<(), StoreError> {
    let entries = fs::read_dir(source).map_err(|error| StoreError::io(source, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| StoreError::io(source, error))?;
        let source_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest.join(&file_name);

        let entry_metadata =
            fs::metadata(&source_path).map_err(|error| StoreError::io(&source_path, error))?;
        if entry_metadata.is_dir() {
            fs::create_dir_all(&dest_path)
                .map_err(|error| StoreError::io(&dest_path, error))?;
            copy_directory_contents(&source_path, &dest_path, false)?;
        } else if entry_metadata.is_file() {
            if skip_top_metadata && file_name == checksum::METADATA_FILE {
                continue;
            }
            fs::copy(&source_path, &dest_path)
                .map_err(|error| StoreError::io(&dest_path, error))?;
        }
    }
    Ok(())
}

/// Current wall-clock time as RFC 3339 UTC with millisecond precision.
fn now_rfc3339() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    rfc3339_from_millis(millis)
}

/// Render milliseconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn rfc3339_from_millis(millis: u64) -> String {
    let seconds = millis / 1000;
    let millis_part = millis % 1000;
    let days = (seconds / SECONDS_PER_DAY) as i64;
    let seconds_of_day = seconds % SECONDS_PER_DAY;

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis_part:03}Z"
    )
}

/// Convert a count of days since 1970-01-01 into `(year, month, day)`.
///
/// This is Howard Hinnant's `civil_from_days` algorithm, which is exact for the
/// whole proleptic Gregorian calendar and needs no dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Scratch directories for tests: always under `target/test-tmp`, never
    //! `%TEMP%` (spawned processes cannot write there on this machine), and
    //! removed again by `Drop`.

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A unique directory under `target/test-tmp`, deleted on drop.
    pub(crate) struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        /// Create a fresh scratch directory labelled `label`.
        pub(crate) fn new(label: &str) -> Self {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-tmp");
            let root = base.join(format!("{label}-{}-{unique}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("create scratch directory");
            Scratch { root }
        }

        /// The scratch root.
        pub(crate) fn path(&self) -> &Path {
            &self.root
        }

        /// Create (if needed) and return a directory below the scratch root.
        pub(crate) fn dir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(&path).expect("create scratch sub-directory");
            path
        }

        /// Write `bytes` to `relative` below the scratch root and return the path.
        pub(crate) fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create file parent");
            }
            fs::write(&path, bytes).expect("write scratch file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Scratch;

    /// A minimal package description for tests.
    fn spec(id: &str, version: &str) -> NewPackage {
        NewPackage {
            id: SourceId::new(id).expect("valid id"),
            version: version.to_string(),
            format: Format::Stardict,
            name: "Demo".to_string(),
            license: "MIT".to_string(),
            homepage: None,
            entries: None,
        }
    }

    /// A package source directory with two files, unique per version.
    fn source_dir(scratch: &Scratch, label: &str, payload: &[u8]) -> PathBuf {
        let directory = scratch.dir(label);
        scratch.write(&format!("{label}/a.txt"), payload);
        scratch.write(&format!("{label}/nested/b.txt"), b"nested");
        directory
    }

    #[test]
    fn open_creates_nothing() {
        let scratch = Scratch::new("open-nothing");
        let root = scratch.path().join("library");
        let library = Library::open(&root).expect("open");
        assert!(library.list().is_empty());
        assert!(library.history(None).is_empty());
        assert!(!root.exists());
    }

    #[test]
    fn add_installs_directory_metadata_and_event() {
        let scratch = Scratch::new("add-basic");
        let source = source_dir(&scratch, "src", b"hello");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");

        let installed = library
            .add(&source, &spec("ecdict", "2.4.2"), false)
            .expect("add");

        assert_eq!(installed.sha256, checksum::sha256_dir(&source).expect("hash"));
        assert_eq!(installed.bytes, 11);
        assert!(installed.active);
        assert!(root.join("library.json").is_file());
        assert!(root.join("ecdict-2.4.2").is_dir());
        assert!(root.join("ecdict-2.4.2/a.txt").is_file());
        assert!(root.join("ecdict-2.4.2/source.json").is_file());

        let events = library.history(None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, EventKind::Installed);
        assert_eq!(events[0].version.as_deref(), Some("2.4.2"));

        // The installed directory hashes to the recorded digest.
        assert_eq!(
            checksum::sha256_dir(&root.join("ecdict-2.4.2")).expect("hash"),
            installed.sha256
        );
    }

    #[test]
    fn add_rejects_duplicates_and_content_changes() {
        let scratch = Scratch::new("add-duplicate");
        let source = source_dir(&scratch, "src", b"hello");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        library
            .add(&source, &spec("ecdict", "2.4.2"), false)
            .expect("first add");

        // Identical content: refused even with force.
        assert!(matches!(
            library.add(&source, &spec("ecdict", "2.4.2"), false),
            Err(StoreError::DuplicatePackage { .. })
        ));
        assert!(matches!(
            library.add(&source, &spec("ecdict", "2.4.2"), true),
            Err(StoreError::DuplicatePackage { .. })
        ));

        // Same version, different content: needs force.
        let changed = source_dir(&scratch, "changed", b"hello again");
        assert!(matches!(
            library.add(&changed, &spec("ecdict", "2.4.2"), false),
            Err(StoreError::SameVersionDifferentContent { .. })
        ));
        let replaced = library
            .add(&changed, &spec("ecdict", "2.4.2"), true)
            .expect("forced replace");
        assert!(replaced.active);
        assert_eq!(
            checksum::sha256_dir(&changed).expect("hash"),
            replaced.sha256
        );
    }

    #[test]
    fn add_second_version_is_inactive() {
        let scratch = Scratch::new("add-second");
        let first = source_dir(&scratch, "first", b"one");
        let second = source_dir(&scratch, "second", b"two");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");

        library.add(&first, &spec("ecdict", "1.0"), false).expect("add 1");
        let added = library.add(&second, &spec("ecdict", "2.0"), false).expect("add 2");
        assert!(!added.active);
        assert!(library
            .active(&SourceId::new("ecdict").expect("id"))
            .is_some_and(|package| package.version == "1.0"));
    }

    #[test]
    fn upgrade_switches_active_and_keeps_old_directory() {
        let scratch = Scratch::new("upgrade-basic");
        let first = source_dir(&scratch, "first", b"one");
        let second = source_dir(&scratch, "second", b"two");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let id = SourceId::new("ecdict").expect("id");

        library.add(&first, &spec("ecdict", "1.0"), false).expect("add");
        let upgraded = library
            .upgrade(&second, &spec("ecdict", "2.0"), false)
            .expect("upgrade");

        assert!(upgraded.active);
        assert!(root.join("ecdict-1.0").is_dir());
        assert!(root.join("ecdict-2.0").is_dir());
        assert_eq!(
            library.active(&id).map(|package| package.version.as_str()),
            Some("2.0")
        );
        assert_eq!(
            library.list().iter().filter(|package| package.active).count(),
            1
        );
        let events = library.history(Some(&id));
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, EventKind::Upgraded);
        assert_eq!(events[1].version.as_deref(), Some("2.0"));
        assert_eq!(events[1].detail.as_deref(), Some("1.0 -> 2.0"));
    }

    #[test]
    fn upgrade_rejects_older_versions_unless_forced() {
        let scratch = Scratch::new("upgrade-older");
        let current = source_dir(&scratch, "current", b"current");
        let older = source_dir(&scratch, "older", b"older");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");

        library
            .add(&current, &spec("ecdict", "2.0"), false)
            .expect("add");
        assert!(matches!(
            library.upgrade(&older, &spec("ecdict", "1.0"), false),
            Err(StoreError::VersionNotNewer { .. })
        ));
        library
            .upgrade(&older, &spec("ecdict", "1.0"), true)
            .expect("forced downgrade");
        assert!(library
            .active(&SourceId::new("ecdict").expect("id"))
            .is_some_and(|package| package.version == "1.0"));
    }

    #[test]
    fn upgrade_requires_an_installed_version() {
        let scratch = Scratch::new("upgrade-unknown");
        let source = source_dir(&scratch, "src", b"data");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        assert!(matches!(
            library.upgrade(&source, &spec("ecdict", "2.0"), false),
            Err(StoreError::PackageNotFound { .. })
        ));
        assert!(!root.exists());
    }

    #[test]
    fn rollback_steps_back_and_stops_at_the_oldest_version() {
        let scratch = Scratch::new("rollback");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let id = SourceId::new("ecdict").expect("id");

        library
            .add(&source_dir(&scratch, "one", b"one"), &spec("ecdict", "1.0"), false)
            .expect("add 1");
        library
            .upgrade(&source_dir(&scratch, "two", b"two"), &spec("ecdict", "2.0"), false)
            .expect("upgrade 2");
        library
            .upgrade(&source_dir(&scratch, "three", b"three"), &spec("ecdict", "3.0"), false)
            .expect("upgrade 3");

        let rolled = library.rollback(&id).expect("rollback to 2");
        assert_eq!(rolled.version, "2.0");
        assert!(rolled.active);
        assert_eq!(
            library.active(&id).map(|package| package.version.as_str()),
            Some("2.0")
        );
        let rolled = library.rollback(&id).expect("rollback to 1");
        assert_eq!(rolled.version, "1.0");
        assert!(matches!(
            library.rollback(&id),
            Err(StoreError::NoPreviousVersion { .. })
        ));

        let events = library.history(Some(&id));
        let kinds: Vec<EventKind> = events.iter().map(|event| event.event.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                EventKind::Installed,
                EventKind::Upgraded,
                EventKind::Upgraded,
                EventKind::RolledBack,
                EventKind::RolledBack,
            ]
        );
    }

    #[test]
    fn rollback_without_an_active_version_errors() {
        let scratch = Scratch::new("rollback-inactive");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let id = SourceId::new("ecdict").expect("id");
        library
            .add(
                &source_dir(&scratch, "src", b"data"),
                &spec("ecdict", "1.0"),
                false,
            )
            .expect("add");
        library.deactivate(&id).expect("deactivate");
        assert!(matches!(
            library.rollback(&id),
            Err(StoreError::NoActiveVersion { .. })
        ));
    }

    #[test]
    fn remove_version_activates_the_newest_remaining() {
        let scratch = Scratch::new("remove-version");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let id = SourceId::new("ecdict").expect("id");

        library
            .add(&source_dir(&scratch, "one", b"one"), &spec("ecdict", "1.0"), false)
            .expect("add 1");
        library
            .upgrade(&source_dir(&scratch, "two", b"two"), &spec("ecdict", "2.0"), false)
            .expect("upgrade 2");
        library
            .add(&source_dir(&scratch, "three", b"three"), &spec("ecdict", "3.0"), false)
            .expect("add 3");

        let removed = library.remove(&id, Some("2.0")).expect("remove 2");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].version, "2.0");
        assert!(!root.join("ecdict-2.0").exists());
        assert_eq!(
            library.active(&id).map(|package| package.version.as_str()),
            Some("3.0"),
            "removing the active version should activate the newest remaining one"
        );

        let removed = library.remove(&id, None).expect("remove rest");
        assert_eq!(removed.len(), 2);
        assert!(library.list().is_empty());
        assert!(!root.join("ecdict-1.0").exists());
        assert!(!root.join("ecdict-2.0").exists());
        assert!(!root.join("ecdict-3.0").exists());
        assert!(root.join("library.json").is_file());
    }

    #[test]
    fn remove_missing_versions_error() {
        let scratch = Scratch::new("remove-missing");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let id = SourceId::new("ecdict").expect("id");
        // Nothing installed: the requested version is what is missing.
        assert!(matches!(
            library.remove(&id, Some("1.0")),
            Err(StoreError::VersionNotFound { .. })
        ));
        library
            .add(&source_dir(&scratch, "src", b"data"), &spec("ecdict", "1.0"), false)
            .expect("add");
        assert!(matches!(
            library.remove(&id, Some("9.9")),
            Err(StoreError::VersionNotFound { .. })
        ));
    }

    #[test]
    fn activate_and_deactivate_round_trip() {
        let scratch = Scratch::new("activate");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let id = SourceId::new("ecdict").expect("id");

        library
            .add(&source_dir(&scratch, "one", b"one"), &spec("ecdict", "1.0"), false)
            .expect("add 1");
        library
            .add(&source_dir(&scratch, "two", b"two"), &spec("ecdict", "2.0"), false)
            .expect("add 2");

        let activated = library.activate(&id, "2.0").expect("activate");
        assert!(activated.active);
        assert_eq!(library.list().iter().filter(|p| p.active).count(), 1);

        let deactivated = library.deactivate(&id).expect("deactivate");
        assert_eq!(deactivated.len(), 1);
        assert_eq!(deactivated[0].version, "2.0");
        assert!(library.list().iter().all(|package| !package.active));

        // Deactivating again changes nothing and records nothing.
        assert!(library.deactivate(&id).expect("noop").is_empty());
        assert_eq!(
            library
                .history(Some(&id))
                .iter()
                .filter(|event| event.event == EventKind::Deactivated)
                .count(),
            1
        );

        assert!(matches!(
            library.activate(&id, "9.9"),
            Err(StoreError::VersionNotFound { .. })
        ));
    }

    #[test]
    fn library_state_survives_reopen() {
        let scratch = Scratch::new("reopen");
        let root = scratch.path().join("library");
        let id = SourceId::new("ecdict").expect("id");
        {
            let mut library = Library::open(&root).expect("open");
            library
                .add(&source_dir(&scratch, "one", b"one"), &spec("ecdict", "1.0"), false)
                .expect("add 1");
            library
                .upgrade(&source_dir(&scratch, "two", b"two"), &spec("ecdict", "2.0"), false)
                .expect("upgrade");
        }
        let reopened = Library::open(&root).expect("reopen");
        assert_eq!(reopened.list().len(), 2);
        assert_eq!(
            reopened.active(&id).map(|package| package.version.as_str()),
            Some("2.0")
        );
        assert_eq!(reopened.history(Some(&id)).len(), 2);
    }

    #[test]
    fn unparsable_manifest_is_never_overwritten() {
        let scratch = Scratch::new("corrupt-manifest");
        let root = scratch.dir("library");
        let manifest_path = root.join("library.json");
        let garbage = b"this is not json\n";
        fs::write(&manifest_path, garbage).expect("write garbage");

        assert!(matches!(
            Library::open(&root),
            Err(StoreError::ManifestUnparsable { .. })
        ));
        assert_eq!(fs::read(&manifest_path).expect("read"), garbage);

        // Even a direct save refuses and leaves the file alone.
        assert!(matches!(
            manifest::save(&manifest_path, &Manifest::new()),
            Err(StoreError::ManifestUnparsable { .. })
        ));
        assert_eq!(fs::read(&manifest_path).expect("read"), garbage);
    }

    #[test]
    fn missing_license_is_rejected_before_any_write() {
        let scratch = Scratch::new("license");
        let source = source_dir(&scratch, "src", b"data");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let mut no_license = spec("ecdict", "1.0");
        no_license.license = "   ".to_string();

        assert!(matches!(
            library.add(&source, &no_license, false),
            Err(StoreError::InvalidField { field: "license", .. })
        ));
        assert!(!root.exists());
    }

    #[test]
    fn existing_version_directory_without_record_needs_force() {
        let scratch = Scratch::new("orphan-dir");
        let source = source_dir(&scratch, "src", b"data");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        fs::create_dir_all(root.join("ecdict-1.0")).expect("orphan dir");

        assert!(matches!(
            library.add(&source, &spec("ecdict", "1.0"), false),
            Err(StoreError::VersionDirectoryExists { .. })
        ));
        let installed = library
            .add(&source, &spec("ecdict", "1.0"), true)
            .expect("forced add over orphan directory");
        assert!(installed.active);
        assert!(root.join("ecdict-1.0/source.json").is_file());
    }

    #[test]
    fn unknown_formats_round_trip() {
        let scratch = Scratch::new("format-other");
        let root = scratch.path().join("library");
        let mut library = Library::open(&root).expect("open");
        let mut package = spec("custom", "2024.03");
        package.format = Format::Other("my-format".to_string());
        library
            .add(&source_dir(&scratch, "src", b"data"), &package, false)
            .expect("add");

        let reopened = Library::open(&root).expect("reopen");
        assert_eq!(
            reopened.list()[0].format,
            Format::Other("my-format".to_string())
        );
    }

    #[test]
    fn package_validation_catches_bad_fields() {
        let installed = spec("ecdict", "1.0").install(
            "0".repeat(64),
            1,
            true,
        );
        assert!(installed.validate().is_ok());

        let mut bad_sha = installed.clone();
        bad_sha.sha256 = "ABC".to_string();
        assert!(matches!(
            bad_sha.validate(),
            Err(StoreError::InvalidField { field: "sha256", .. })
        ));

        let mut bad_version = installed.clone();
        bad_version.version = "1/0".to_string();
        assert!(matches!(
            bad_version.validate(),
            Err(StoreError::InvalidVersion { .. })
        ));

        let mut bad_installed_at = installed;
        bad_installed_at.installed_at.clear();
        assert!(matches!(
            bad_installed_at.validate(),
            Err(StoreError::InvalidField {
                field: "installed_at",
                ..
            })
        ));
    }

    #[test]
    fn source_ids_are_validated() {
        assert!(SourceId::new("ecdict").is_ok());
        assert!(SourceId::new("ecdict_cn-2.0").is_ok());
        assert!(SourceId::new("2term").is_ok());
        for invalid in ["", "ECDICT", "ec dict", "../evil", ".hidden", "-dash", "字典"] {
            assert!(
                matches!(SourceId::new(invalid), Err(StoreError::InvalidSourceId { .. })),
                "{invalid} should be rejected"
            );
        }
    }

    #[test]
    fn rfc3339_conversion_matches_known_instants() {
        assert_eq!(rfc3339_from_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_from_millis(1_000), "1970-01-01T00:00:01.000Z");
        assert_eq!(
            rfc3339_from_millis(1_700_000_000_123),
            "2023-11-14T22:13:20.123Z"
        );
        assert_eq!(
            rfc3339_from_millis(1_709_164_800_000),
            "2024-02-29T00:00:00.000Z"
        );
        let now = now_rfc3339();
        assert_eq!(now.len(), 24, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(&now[10..11], "T");
    }
}
