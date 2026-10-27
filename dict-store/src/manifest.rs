//! `library.json`: the manifest, its history log, and the metadata input types.
//!
//! The manifest is the single source of truth for what is installed:
//!
//! ```json
//! {
//!   "schema": 1,
//!   "packages": [
//!     {
//!       "id": "ecdict",
//!       "version": "2.4.2",
//!       "format": "stardict",
//!       "name": "ECDICT",
//!       "license": "CC-BY-SA-4.0",
//!       "homepage": "https://example.invalid/ecdict",
//!       "sha256": "…64 hex characters…",
//!       "bytes": 199229440,
//!       "entries": 770000,
//!       "installed_at": "2026-10-27T05:31:12.345Z",
//!       "active": true
//!     }
//!   ],
//!   "history": [
//!     {
//!       "at": "2026-10-27T05:31:12.345Z",
//!       "event": "installed",
//!       "id": "ecdict",
//!       "version": "2.4.2"
//!     }
//!   ]
//! }
//! ```
//!
//! # Writing rules
//!
//! - The manifest is written to a temporary file in the same directory, flushed
//!   with `sync_all`, and renamed over `library.json`, so a crash leaves either
//!   the old file or the new one, never a half-written one.
//! - If the existing `library.json` cannot be parsed, [`save`] refuses to touch
//!   it: an unreadable manifest is almost always a file the user edited by hand
//!   or a disk problem, and silently replacing it would destroy their data.
//! - History is append-only. No code path edits or drops an event.
//!
//! # Reading rules
//!
//! A missing file is an empty library. A file that parses but is unusable
//! (unknown schema version, duplicate `(id, version)`, empty licence, malformed
//! id) is rejected with an error that names the manifest, so the user can see
//! what is wrong without guessing.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::checksum;
use crate::error::StoreError;
use crate::version;
use crate::{validate_text_fields, DictionaryPackage, Format, SourceId};

/// File name of the manifest inside the library root.
pub const MANIFEST_FILE: &str = "library.json";

/// Manifest schema this build understands. A newer file is refused, not
/// rewritten, so an older program cannot destroy a newer library.
pub const SCHEMA_VERSION: u32 = 1;

/// The manifest: every installed package plus the append-only history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest schema version; [`SCHEMA_VERSION`] for files this build writes.
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Installed package versions, sorted by id and then by version on write.
    #[serde(default)]
    pub packages: Vec<DictionaryPackage>,
    /// Append-only event log, oldest first.
    #[serde(default)]
    pub history: Vec<HistoryEvent>,
}

/// Schema value used when a file predates the `schema` field.
fn default_schema() -> u32 {
    SCHEMA_VERSION
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest::new()
    }
}

impl Manifest {
    /// An empty manifest at the current schema version.
    pub fn new() -> Self {
        Manifest {
            schema: SCHEMA_VERSION,
            packages: Vec::new(),
            history: Vec::new(),
        }
    }

    /// The record for exactly `(id, version)`, if installed.
    pub fn find(&self, id: &SourceId, version: &str) -> Option<&DictionaryPackage> {
        self.packages
            .iter()
            .find(|package| &package.id == id && package.version == version)
    }

    /// All versions of `id`, oldest first.
    pub fn versions_of(&self, id: &SourceId) -> Vec<&DictionaryPackage> {
        let mut versions: Vec<&DictionaryPackage> = self
            .packages
            .iter()
            .filter(|package| &package.id == id)
            .collect();
        versions.sort_by(|left, right| version::compare(&left.version, &right.version));
        versions
    }

    /// The newest installed version of `id`, if any.
    pub fn newest_of(&self, id: &SourceId) -> Option<&DictionaryPackage> {
        self.versions_of(id).last().copied()
    }

    /// The active version of `id`, if any.
    pub fn active_of(&self, id: &SourceId) -> Option<&DictionaryPackage> {
        self.packages
            .iter()
            .find(|package| &package.id == id && package.active)
    }

    /// Insert `package`, replacing an existing record with the same
    /// `(id, version)`.
    pub fn upsert(&mut self, package: DictionaryPackage) {
        match self.packages.iter_mut().find(|existing| {
            existing.id == package.id && existing.version == package.version
        }) {
            Some(existing) => *existing = package,
            None => self.packages.push(package),
        }
    }

    /// Sort package records by id and version, for a stable file and console
    /// table.
    pub fn sort_packages(&mut self) {
        self.packages.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| version::compare(&left.version, &right.version))
        });
    }

    /// Append one history event. This is the only way history grows.
    pub fn push_event(&mut self, event: HistoryEvent) {
        self.history.push(event);
    }

    /// Check that the manifest can be used by this build.
    ///
    /// Rejects an unsupported schema version, a package record that fails
    /// [`DictionaryPackage::validate`], and duplicate `(id, version)` records.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ManifestInvalid`] with the offending record named.
    pub fn validate(&self, path: &Path) -> Result<(), StoreError> {
        if self.schema == 0 || self.schema > SCHEMA_VERSION {
            return Err(StoreError::ManifestInvalid {
                path: path.to_path_buf(),
                reason: format!(
                    "schema 版本 {} 不受支持（本程序支持 1..={}）",
                    self.schema, SCHEMA_VERSION
                ),
            });
        }
        let mut seen: HashSet<(&SourceId, &str)> = HashSet::new();
        for package in &self.packages {
            package
                .validate()
                .map_err(|error| StoreError::ManifestInvalid {
                    path: path.to_path_buf(),
                    reason: format!(
                        "包记录 {} {} 无效：{}",
                        package.id, package.version, error
                    ),
                })?;
            if !seen.insert((&package.id, package.version.as_str())) {
                return Err(StoreError::ManifestInvalid {
                    path: path.to_path_buf(),
                    reason: format!("重复的版本记录 {} {}", package.id, package.version),
                });
            }
        }
        Ok(())
    }

    /// Read a manifest from `path`.
    ///
    /// A missing file is an empty library, not an error.
    ///
    /// # Errors
    ///
    /// [`StoreError::ManifestUnparsable`] for invalid JSON,
    /// [`StoreError::ManifestInvalid`] for unusable content, and
    /// [`StoreError::Io`] when the file cannot be read.
    pub fn load(path: &Path) -> Result<Manifest, StoreError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Manifest::new()),
            Err(error) => return Err(StoreError::io(path, error)),
        };
        let mut manifest: Manifest =
            serde_json::from_str(&text).map_err(|source| StoreError::ManifestUnparsable {
                path: path.to_path_buf(),
                source,
            })?;
        manifest.validate(path)?;
        manifest.sort_packages();
        Ok(manifest)
    }

    /// Write a manifest to `path` atomically.
    ///
    /// The document is written to a temporary file (`library.json.tmp-<pid>`,
    /// hidden in the same directory), synced to disk and renamed over
    /// `library.json`.
    ///
    /// # Refusal
    ///
    /// If `path` exists but this build cannot use it — invalid JSON, or a
    /// manifest that fails [`Manifest::validate`], for example one written by a
    /// newer schema — this returns an error without modifying the file. Such a
    /// file is almost always hand-edited or produced by a newer program, and
    /// silently replacing it would destroy data.
    ///
    /// # Errors
    ///
    /// That refusal, [`StoreError::ManifestInvalid`] when `manifest` itself is
    /// unusable, [`StoreError::JsonWrite`] when serialization fails, and
    /// [`StoreError::Io`] for file-system failures.
    pub fn save(path: &Path, manifest: &Manifest) -> Result<(), StoreError> {
        if path.exists() {
            let text = fs::read_to_string(path).map_err(|error| StoreError::io(path, error))?;
            let existing: Manifest =
                serde_json::from_str(&text).map_err(|source| StoreError::ManifestUnparsable {
                    path: path.to_path_buf(),
                    source,
                })?;
            existing.validate(path)?;
        }

        let mut document = manifest.clone();
        document.sort_packages();
        document.validate(path)?;

        let json = serde_json::to_string_pretty(&document).map_err(|source| {
            StoreError::JsonWrite {
                path: path.to_path_buf(),
                source,
            }
        })?;
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;

        let temp = parent.join(format!(".{}.tmp-{}", MANIFEST_FILE, std::process::id()));
        let written = write_file(&temp, &json);
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);
            return Err(StoreError::io(path, error));
        }
        Ok(())
    }
}

/// Write `json` plus a trailing newline to `path`, syncing it to disk.
fn write_file(path: &Path, json: &str) -> Result<(), StoreError> {
    let mut file = fs::File::create(path).map_err(|error| StoreError::io(path, error))?;
    file.write_all(json.as_bytes())
        .map_err(|error| StoreError::io(path, error))?;
    file.write_all(b"\n")
        .map_err(|error| StoreError::io(path, error))?;
    file.sync_all()
        .map_err(|error| StoreError::io(path, error))?;
    Ok(())
}

/// Save a manifest to `path`.
///
/// Convenience wrapper around [`Manifest::save`].
///
/// # Errors
///
/// See [`Manifest::save`].
pub fn save(path: &Path, manifest: &Manifest) -> Result<(), StoreError> {
    Manifest::save(path, manifest)
}

/// One entry of the append-only history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEvent {
    /// When it happened: RFC 3339 UTC with milliseconds.
    pub at: String,
    /// What happened.
    pub event: EventKind,
    /// The dictionary id it happened to.
    pub id: SourceId,
    /// The version involved, when the event has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Free-text extra context (for example `1.2.0 -> 2.0.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The kind of a [`HistoryEvent`].
///
/// Serialized as `snake_case` strings, which are also what the CLI table shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A version was installed for the first time (or replaced with `--force`).
    Installed,
    /// A newer version replaced the active one.
    Upgraded,
    /// `active` moved back to an older installed version.
    RolledBack,
    /// A version was deleted.
    Removed,
    /// A version became active (explicitly, or automatically after a remove).
    Activated,
    /// A version stopped being active.
    Deactivated,
}

impl EventKind {
    /// The `snake_case` token stored in `library.json`.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Installed => "installed",
            EventKind::Upgraded => "upgraded",
            EventKind::RolledBack => "rolled_back",
            EventKind::Removed => "removed",
            EventKind::Activated => "activated",
            EventKind::Deactivated => "deactivated",
        }
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Package metadata read from a package directory's `source.json`.
///
/// Every field is optional because `source.json` is written by whoever built the
/// package and may contain only some of it. Unknown fields are ignored, so a
/// full [`DictionaryPackage`] record is also accepted here. The fields present
/// in `source.json` are used as defaults and overridden by whatever the caller
/// (usually the CLI) supplies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PackageMeta {
    /// Dictionary id.
    pub id: Option<SourceId>,
    /// Version string.
    pub version: Option<String>,
    /// File layout tag.
    pub format: Option<Format>,
    /// Display name.
    pub name: Option<String>,
    /// Licence (SPDX identifier or free text).
    pub license: Option<String>,
    /// Source homepage.
    pub homepage: Option<String>,
    /// Number of headwords.
    pub entries: Option<u64>,
}

impl PackageMeta {
    /// Read `source.json` from `path`, treating a missing file as empty
    /// metadata.
    ///
    /// # Errors
    ///
    /// [`StoreError::SourceJsonUnparsable`] when the file exists but is not
    /// valid JSON, and [`StoreError::Io`] when it cannot be read.
    pub fn read(path: &Path) -> Result<Option<PackageMeta>, StoreError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StoreError::io(path, error)),
        };
        let meta = serde_json::from_str(&text).map_err(|source| {
            StoreError::SourceJsonUnparsable {
                path: path.to_path_buf(),
                source,
            }
        })?;
        Ok(Some(meta))
    }
}

/// A resolved package description ready for [`crate::Library::add`] or
/// [`crate::Library::upgrade`].
///
/// Unlike [`DictionaryPackage`] this carries no checksum, size, timestamp or
/// active flag: those are produced by the library when the package is actually
/// installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPackage {
    /// Dictionary id.
    pub id: SourceId,
    /// Version string.
    pub version: String,
    /// File layout tag.
    pub format: Format,
    /// Display name.
    pub name: String,
    /// Licence (SPDX identifier or free text). Must not be empty.
    pub license: String,
    /// Source homepage, when known.
    pub homepage: Option<String>,
    /// Number of headwords, when known.
    pub entries: Option<u64>,
}

impl NewPackage {
    /// Build a package description from a package directory plus caller
    /// overrides.
    ///
    /// `source_dir/source.json` is read when present and used as the base;
    /// every field set in `overrides` wins. The result is validated, so the
    /// caller learns about empty names or licences before anything is copied.
    ///
    /// # Errors
    ///
    /// - [`StoreError::SourceJsonUnparsable`] when `source.json` is broken;
    /// - [`StoreError::MissingMetadata`] naming the first field that neither
    ///   source supplied;
    /// - [`StoreError::InvalidField`] / [`StoreError::InvalidVersion`] when a
    ///   supplied field is unusable.
    pub fn resolve(source_dir: &Path, overrides: &PackageMeta) -> Result<NewPackage, StoreError> {
        let base = PackageMeta::read(&source_dir.join(checksum::METADATA_FILE))?.unwrap_or_default();
        let missing = |field: &'static str| StoreError::MissingMetadata {
            field,
            source_dir: source_dir.to_path_buf(),
        };

        let id = overrides
            .id
            .clone()
            .or(base.id)
            .ok_or_else(|| missing("id"))?;
        let version = overrides
            .version
            .clone()
            .or(base.version)
            .ok_or_else(|| missing("version"))?;
        let format = overrides
            .format
            .clone()
            .or(base.format)
            .ok_or_else(|| missing("format"))?;
        let name = overrides
            .name
            .clone()
            .or(base.name)
            .ok_or_else(|| missing("name"))?;
        let license = overrides
            .license
            .clone()
            .or(base.license)
            .ok_or_else(|| missing("license"))?;
        let homepage = overrides.homepage.clone().or(base.homepage);
        let entries = overrides.entries.or(base.entries);

        let package = NewPackage {
            id,
            version,
            format,
            name: name.trim().to_string(),
            license: license.trim().to_string(),
            homepage: homepage
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            entries,
        };
        package.validate()?;
        Ok(package)
    }

    /// Check that the description is complete and safe to store.
    ///
    /// # Errors
    ///
    /// [`StoreError::InvalidVersion`] for a version that cannot be part of a
    /// directory name, and [`StoreError::InvalidField`] for an empty name,
    /// licence or format.
    pub fn validate(&self) -> Result<(), StoreError> {
        version::validate(&self.version)?;
        validate_text_fields(&self.format, &self.name, &self.license)
    }

    /// Turn this description into a manifest record.
    pub(crate) fn install(
        &self,
        sha256: String,
        bytes: u64,
        active: bool,
    ) -> DictionaryPackage {
        DictionaryPackage {
            id: self.id.clone(),
            version: self.version.clone(),
            format: self.format.clone(),
            name: self.name.clone(),
            license: self.license.clone(),
            homepage: self.homepage.clone(),
            sha256,
            bytes,
            entries: self.entries,
            installed_at: crate::now_rfc3339(),
            active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Scratch;
    use crate::SourceId;

    fn package(id: &str, version: &str, sha: &str, active: bool) -> DictionaryPackage {
        DictionaryPackage {
            id: SourceId::new(id).expect("id"),
            version: version.to_string(),
            format: Format::Stardict,
            name: "Demo".to_string(),
            license: "MIT".to_string(),
            homepage: None,
            sha256: sha.to_string(),
            bytes: 12,
            entries: Some(10),
            installed_at: "2026-10-27T05:31:12.345Z".to_string(),
            active,
        }
    }

    fn sample_manifest() -> Manifest {
        let mut manifest = Manifest::new();
        manifest.upsert(package("ecdict", "2.4.2", &"a".repeat(64), true));
        manifest.upsert(package("ecdict", "2.4.1", &"b".repeat(64), false));
        manifest.push_event(HistoryEvent {
            at: "2026-10-27T05:31:12.345Z".to_string(),
            event: EventKind::Installed,
            id: SourceId::new("ecdict").expect("id"),
            version: Some("2.4.1".to_string()),
            detail: None,
        });
        manifest.sort_packages();
        manifest
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let scratch = Scratch::new("manifest-roundtrip");
        let path = scratch.path().join("library.json");
        let manifest = sample_manifest();

        Manifest::save(&path, &manifest).expect("save");
        let loaded = Manifest::load(&path).expect("load");

        assert_eq!(loaded, manifest);
        assert!(loaded.packages[0].version == "2.4.1");
        assert!(loaded.packages[1].version == "2.4.2");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"schema\": 1"));
        assert!(text.contains("\"event\": \"installed\""));
    }

    #[test]
    fn missing_file_is_an_empty_library() {
        let scratch = Scratch::new("manifest-missing");
        let loaded = Manifest::load(&scratch.path().join("library.json")).expect("load");
        assert_eq!(loaded, Manifest::new());
        assert!(loaded.packages.is_empty());
        assert!(loaded.history.is_empty());
    }

    #[test]
    fn save_replaces_atomically_and_leaves_no_temp_file() {
        let scratch = Scratch::new("manifest-atomic");
        let path = scratch.path().join("library.json");
        Manifest::save(&path, &Manifest::new()).expect("first save");
        let first = fs::read_to_string(&path).expect("read first");

        let mut second = Manifest::new();
        second.upsert(package("ecdict", "1.0", &"c".repeat(64), true));
        Manifest::save(&path, &second).expect("second save");
        let third = fs::read_to_string(&path).expect("read second");
        assert_ne!(first, third);
        assert_eq!(Manifest::load(&path).expect("reload"), second);

        let leftovers: Vec<String> = fs::read_dir(scratch.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
    }

    #[test]
    fn save_refuses_to_overwrite_unparsable_content() {
        let scratch = Scratch::new("manifest-refuse");
        let path = scratch.path().join("library.json");
        fs::write(&path, b"{ broken json").expect("write");

        let error = Manifest::save(&path, &Manifest::new()).expect_err("must refuse");
        assert!(matches!(error, StoreError::ManifestUnparsable { .. }));
        assert_eq!(fs::read(&path).expect("read"), b"{ broken json");
        assert!(matches!(
            Manifest::load(&path),
            Err(StoreError::ManifestUnparsable { .. })
        ));
    }

    #[test]
    fn save_refuses_to_overwrite_a_newer_manifest() {
        let scratch = Scratch::new("manifest-newer");
        let path = scratch.path().join("library.json");
        let newer = "{\"schema\":99,\"packages\":[],\"history\":[]}";
        fs::write(&path, newer).expect("write");

        let error = Manifest::save(&path, &Manifest::new()).expect_err("must refuse");
        assert!(matches!(error, StoreError::ManifestInvalid { .. }));
        assert_eq!(fs::read_to_string(&path).expect("read"), newer);
    }

    #[test]
    fn load_rejects_duplicate_records() {
        let scratch = Scratch::new("manifest-duplicate");
        let path = scratch.path().join("library.json");
        // `upsert` replaces, so a duplicate can only come from hand-made JSON.
        fs::write(
            &path,
            format!(
                "{{\"schema\":1,\"packages\":[{record},{record}],\"history\":[]}}",
                record = serde_json::to_string(&package("ecdict", "1.0", &"a".repeat(64), true))
                    .expect("serialize")
            ),
        )
        .expect("write");
        assert!(matches!(
            Manifest::load(&path),
            Err(StoreError::ManifestInvalid { .. })
        ));
    }

    #[test]
    fn load_rejects_unknown_schema_and_invalid_packages() {
        let scratch = Scratch::new("manifest-invalid");
        let path = scratch.path().join("library.json");

        fs::write(&path, "{\"schema\":99,\"packages\":[],\"history\":[]}").expect("write");
        assert!(matches!(
            Manifest::load(&path),
            Err(StoreError::ManifestInvalid { .. })
        ));

        let mut invalid = Manifest::new();
        invalid.upsert(package("ecdict", "1.0", &"a".repeat(64), true));
        invalid.packages[0].license = String::new();
        assert!(matches!(
            invalid.validate(&path),
            Err(StoreError::ManifestInvalid { .. })
        ));

        let mut bad_id = package("ecdict", "1.0", &"a".repeat(64), true);
        bad_id.sha256 = "short".to_string();
        assert!(bad_id.validate().is_err());
    }

    #[test]
    fn package_meta_accepts_full_records_and_rejects_broken_json() {
        let scratch = Scratch::new("manifest-meta");
        let path = scratch.path().join("source.json");
        let record = package("ecdict", "1.0", &"a".repeat(64), true);
        fs::write(&path, serde_json::to_string(&record).expect("serialize")).expect("write");

        let meta = PackageMeta::read(&path).expect("read").expect("present");
        assert_eq!(meta.id.as_ref().map(SourceId::as_str), Some("ecdict"));
        assert_eq!(meta.version.as_deref(), Some("1.0"));
        assert_eq!(meta.license.as_deref(), Some("MIT"));

        fs::write(&path, b"not json").expect("write");
        assert!(matches!(
            PackageMeta::read(&path),
            Err(StoreError::SourceJsonUnparsable { .. })
        ));
    }

    #[test]
    fn resolve_merges_overrides_over_source_json() {
        let scratch = Scratch::new("manifest-resolve");
        let directory = scratch.dir("pkg");
        fs::write(
            directory.join("source.json"),
            "{\"id\":\"ecdict\",\"version\":\"1.0\",\"format\":\"stardict\",\"name\":\"From file\",\"license\":\"MIT\"}",
        )
        .expect("write");

        let resolved = NewPackage::resolve(&directory, &PackageMeta::default()).expect("resolve");
        assert_eq!(resolved.name, "From file");
        assert_eq!(resolved.license, "MIT");

        let overrides = PackageMeta {
            name: Some("  Overridden  ".to_string()),
            homepage: Some("https://example.invalid".to_string()),
            ..PackageMeta::default()
        };
        let resolved = NewPackage::resolve(&directory, &overrides).expect("resolve");
        assert_eq!(resolved.name, "Overridden");
        assert_eq!(resolved.homepage.as_deref(), Some("https://example.invalid"));

        let empty = scratch.dir("empty");
        assert!(matches!(
            NewPackage::resolve(&empty, &PackageMeta::default()),
            Err(StoreError::MissingMetadata { field: "id", .. })
        ));

        let only_license = PackageMeta {
            license: Some(String::new()),
            ..PackageMeta::default()
        };
        assert!(matches!(
            NewPackage::resolve(&directory, &only_license),
            Err(StoreError::InvalidField { field: "license", .. })
        ));
    }

    #[test]
    fn checksum_import_is_the_shared_walk() {
        let scratch = Scratch::new("manifest-checksum");
        scratch.write("pkg/a.txt", b"hello");
        let digest = checksum::summarize_dir(&scratch.dir("pkg")).expect("summarize");
        assert_eq!(digest.files, 1);
        assert_eq!(digest.bytes, 5);
    }
}
