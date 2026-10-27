//! The provider seam: where packages can come from.
//!
//! [`LocalDirectoryProvider`] is the working implementation: it scans a
//! directory of unpacked packages and hands back the newest version of a given
//! dictionary. `HttpProvider` is the **reserved download interface** — a
//! skeleton that reports `available: false` and returns
//! [`ProviderError::Unavailable`] from every method. It performs no network I/O
//! and this crate has no networking dependency; the type exists so the rest of
//! the code can already talk about "where a package comes from" without a
//! half-finished downloader.
//!
//! A caller (the future UI flow, or the CLI) drives a provider in two steps:
//!
//! 1. [`Provider::latest`] asks what the newest release of an id is;
//! 2. [`Provider::fetch`] materialises that release into a directory, which is
//!    then installed with [`crate::Library::add`] or
//!    [`crate::Library::upgrade`].
//!
//! Keeping the two steps separate is what lets a provider answer "is there an
//! update?" without downloading anything.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::checksum;
use crate::error::StoreError;
use crate::manifest::PackageMeta;
use crate::{copy_package_tree, Format, NewPackage, SourceId};

/// Message returned by every [`HttpProvider`] method.
const NETWORK_UNAVAILABLE: &str = "network provider not implemented";

/// What a provider is and whether it can be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    /// Short provider name, suitable for a status line.
    pub name: String,
    /// Whether calling the provider can succeed at all.
    pub available: bool,
    /// Whether using it requires network access.
    pub requires_network: bool,
}

/// A package a provider knows about but has not downloaded yet.
///
/// `location` is opaque to this crate: an HTTP provider would put a URL there,
/// [`LocalDirectoryProvider`] puts the package directory. It is only meaningful
/// together with the provider that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePackage {
    /// Dictionary id.
    pub id: SourceId,
    /// Version string, comparable with [`crate::version::compare`].
    pub version: String,
    /// File layout tag.
    pub format: Format,
    /// Display name.
    pub name: String,
    /// Licence (SPDX identifier or free text).
    pub license: String,
    /// Source homepage, when known.
    pub homepage: Option<String>,
    /// Digest the provider promises, when it knows it.
    pub sha256: Option<String>,
    /// Number of headwords, when known.
    pub entries: Option<u64>,
    /// Provider-specific locator handed back to [`Provider::fetch`].
    pub location: String,
}

impl RemotePackage {
    /// Convert this remote description into the input form of
    /// [`crate::Library::add`] / [`crate::Library::upgrade`].
    pub fn to_new_package(&self) -> NewPackage {
        NewPackage {
            id: self.id.clone(),
            version: self.version.clone(),
            format: self.format.clone(),
            name: self.name.clone(),
            license: self.license.clone(),
            homepage: self.homepage.clone(),
            entries: self.entries,
        }
    }
}

/// Why a provider could not answer or deliver.
#[derive(Debug)]
pub enum ProviderError {
    /// The provider exists as an interface but is not implemented or not
    /// configured.
    Unavailable(String),
    /// The requested dictionary id is not offered by this provider.
    NotFound {
        /// The id that was requested.
        id: SourceId,
    },
    /// A candidate package is present but its metadata cannot be used.
    InvalidPackage {
        /// The metadata file or directory that is wrong.
        path: PathBuf,
        /// What is wrong, in user-facing Chinese.
        reason: String,
    },
    /// Reading or writing files failed.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying operating-system error.
        source: io::Error,
    },
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Unavailable(reason) => write!(f, "下载接口不可用：{reason}"),
            ProviderError::NotFound { id } => write!(f, "来源中没有词典 {id}"),
            ProviderError::InvalidPackage { path, reason } => {
                write!(f, "包 {} 不可用：{reason}", path.display())
            }
            ProviderError::Io { path, source } => {
                write!(f, "无法访问 {}：{}", path.display(), source)
            }
        }
    }
}

impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProviderError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A place dictionary packages can be obtained from.
///
/// Implementations are expected to be cheap to construct and to do no I/O until
/// one of the methods is called. `latest` must not download the package; that is
/// what `fetch` is for.
pub trait Provider {
    /// Describe the provider: name, availability, network requirement.
    fn describe(&self) -> ProviderInfo;

    /// Report the newest version of `id` the provider offers.
    ///
    /// # Errors
    ///
    /// [`ProviderError::NotFound`] when the id is unknown, and whatever the
    /// implementation needs to report for broken metadata or I/O.
    fn latest(&self, id: &SourceId) -> Result<RemotePackage, ProviderError>;

    /// Materialise `remote` into `dest`.
    ///
    /// `dest` is created if needed; existing files may be overwritten. Installing
    /// the result is the caller's next step.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unavailable`] for the reserved HTTP provider, and the
    /// implementation's own failure modes.
    fn fetch(&self, remote: &RemotePackage, dest: &Path) -> Result<(), ProviderError>;
}

/// A provider that reads unpacked packages from a local directory.
///
/// Every immediate sub-directory that contains a `source.json` is a candidate.
/// A candidate is considered for an id when its `source.json` names that id;
/// candidates without an `id` are skipped (there is no way to tell whose they
/// are), and candidates for other ids are skipped silently.
#[derive(Debug, Clone)]
pub struct LocalDirectoryProvider {
    root: PathBuf,
}

impl LocalDirectoryProvider {
    /// Watch `root` for unpacked packages.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalDirectoryProvider { root: root.into() }
    }

    /// The directory being scanned.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read one candidate's metadata.
    fn candidate_meta(&self, directory: &Path) -> Result<Option<PackageMeta>, ProviderError> {
        let metadata_path = directory.join(checksum::METADATA_FILE);
        if !metadata_path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&metadata_path).map_err(|source| ProviderError::Io {
            path: metadata_path.clone(),
            source,
        })?;
        let meta: PackageMeta =
            serde_json::from_str(&text).map_err(|source| ProviderError::InvalidPackage {
                path: metadata_path.clone(),
                reason: format!("source.json 不是合法 JSON：{source}"),
            })?;
        Ok(Some(meta))
    }
}

impl Provider for LocalDirectoryProvider {
    fn describe(&self) -> ProviderInfo {
        ProviderInfo {
            name: format!("本地目录 {}", self.root.display()),
            available: self.root.is_dir(),
            requires_network: false,
        }
    }

    fn latest(&self, id: &SourceId) -> Result<RemotePackage, ProviderError> {
        let entries = fs::read_dir(&self.root).map_err(|source| ProviderError::Io {
            path: self.root.clone(),
            source,
        })?;

        let mut candidates: Vec<RemotePackage> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| ProviderError::Io {
                path: self.root.clone(),
                source,
            })?;
            let directory = entry.path();
            let is_directory = fs::metadata(&directory)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            if !is_directory {
                continue;
            }
            let meta = match self.candidate_meta(&directory)? {
                Some(meta) => meta,
                None => continue,
            };
            // Without an id we cannot tell whose package this is.
            let candidate_id = match &meta.id {
                Some(candidate_id) => candidate_id,
                None => continue,
            };
            if candidate_id != id {
                continue;
            }

            let metadata_path = directory.join(checksum::METADATA_FILE);
            let missing = |field: &'static str| ProviderError::InvalidPackage {
                path: metadata_path.clone(),
                reason: format!("缺少 {field}"),
            };
            let version = meta.version.filter(|value| !value.trim().is_empty()).ok_or_else(|| missing("version"))?;
            let format = meta.format.ok_or_else(|| missing("format"))?;
            let name = meta
                .name
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing("name"))?;
            let license = meta
                .license
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing("license"))?;

            candidates.push(RemotePackage {
                id: id.clone(),
                version,
                format,
                name: name.trim().to_string(),
                license: license.trim().to_string(),
                homepage: meta.homepage,
                sha256: None,
                entries: meta.entries,
                location: directory.to_string_lossy().into_owned(),
            });
        }

        candidates.sort_by(|left, right| {
            crate::version::compare(&left.version, &right.version)
                .then_with(|| left.location.cmp(&right.location))
        });
        candidates.pop().ok_or_else(|| ProviderError::NotFound { id: id.clone() })
    }

    fn fetch(&self, remote: &RemotePackage, dest: &Path) -> Result<(), ProviderError> {
        let source = PathBuf::from(&remote.location);
        if !source.is_dir() {
            return Err(ProviderError::InvalidPackage {
                path: source,
                reason: "包目录不存在".to_string(),
            });
        }
        copy_package_tree(&source, dest, false).map_err(|error| provider_error(&source, error))
    }
}

/// Reserved HTTP download provider.
///
/// This type is the placeholder for the future download interface. It is
/// deliberately inert: [`Provider::describe`] reports `available: false`,
/// `requires_network: true`, and both [`Provider::latest`] and
/// [`Provider::fetch`] return
/// `ProviderError::Unavailable("network provider not implemented")` without
/// touching the network. It exists so that call sites can be written against
/// [`Provider`] today and start working when a real implementation lands.
#[derive(Debug, Clone)]
pub struct HttpProvider {
    base_url: String,
}

impl HttpProvider {
    /// Remember the base URL a future implementation would download from.
    ///
    /// No URL is validated or contacted here.
    pub fn new(base_url: impl Into<String>) -> Self {
        HttpProvider {
            base_url: base_url.into(),
        }
    }

    /// The configured base URL, for diagnostics.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Provider for HttpProvider {
    fn describe(&self) -> ProviderInfo {
        ProviderInfo {
            name: format!("HTTP {}", self.base_url),
            available: false,
            requires_network: true,
        }
    }

    fn latest(&self, _id: &SourceId) -> Result<RemotePackage, ProviderError> {
        Err(ProviderError::Unavailable(NETWORK_UNAVAILABLE.to_string()))
    }

    fn fetch(&self, _remote: &RemotePackage, _dest: &Path) -> Result<(), ProviderError> {
        Err(ProviderError::Unavailable(NETWORK_UNAVAILABLE.to_string()))
    }
}

/// Re-wrap a store error as a provider error, keeping the path that failed.
fn provider_error(path: &Path, error: StoreError) -> ProviderError {
    match error {
        StoreError::Io { source, .. } => ProviderError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => ProviderError::InvalidPackage {
            path: path.to_path_buf(),
            reason: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Scratch;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn id(raw: &str) -> SourceId {
        SourceId::new(raw).expect("valid id")
    }

    /// Write a package directory with a `source.json` describing it.
    fn package_dir(
        scratch: &Scratch,
        directory: &str,
        id: &str,
        version: &str,
        payload: &[u8],
    ) -> PathBuf {
        let path = scratch.dir(directory);
        fs::write(
            path.join("source.json"),
            format!(
                "{{\"id\":\"{id}\",\"version\":\"{version}\",\"format\":\"stardict\",\"name\":\"Demo {version}\",\"license\":\"MIT\"}}"
            ),
        )
        .expect("write source.json");
        fs::write(path.join("data.bin"), payload).expect("write data");
        path
    }

    #[test]
    fn local_provider_picks_the_newest_version() {
        let scratch = Scratch::new("provider-newest");
        let root = scratch.dir("providers");
        package_dir(&scratch, "providers/ecdict-2.4.1", "ecdict", "2.4.1", b"old");
        package_dir(&scratch, "providers/ecdict-2024.03", "ecdict", "2024.03", b"new");
        package_dir(&scratch, "providers/ecdict-2.4.2", "ecdict", "2.4.2", b"middle");
        // No source.json: not a candidate.
        scratch.write("providers/not-a-package/readme.txt", b"ignore me");
        // Another dictionary: must not be picked for `ecdict`.
        package_dir(&scratch, "providers/other-9.9", "other", "9.9", b"other");

        let provider = LocalDirectoryProvider::new(&root);
        let latest = provider.latest(&id("ecdict")).expect("latest");
        assert_eq!(latest.version, "2024.03");
        assert_eq!(latest.name, "Demo 2024.03");
        assert_eq!(
            latest.location,
            root.join("ecdict-2024.03").to_string_lossy().into_owned()
        );
        assert!(!provider.describe().requires_network);
        assert!(provider.describe().available);

        let new_package = latest.to_new_package();
        assert_eq!(new_package.id.as_str(), "ecdict");
        assert_eq!(new_package.version, "2024.03");
        assert_eq!(new_package.license, "MIT");
    }

    #[test]
    fn local_provider_reports_missing_and_broken_packages() {
        let scratch = Scratch::new("provider-errors");
        let root = scratch.dir("providers");
        package_dir(&scratch, "providers/ecdict-1.0", "ecdict", "1.0", b"x");
        let provider = LocalDirectoryProvider::new(&root);

        assert!(matches!(
            provider.latest(&id("missing")),
            Err(ProviderError::NotFound { .. })
        ));

        // A matching id with incomplete metadata is an error, not a skip.
        let broken = scratch.dir("providers/ecdict-broken");
        fs::write(
            broken.join("source.json"),
            "{\"id\":\"ecdict\",\"format\":\"stardict\",\"name\":\"x\",\"license\":\"MIT\"}",
        )
        .expect("write");
        assert!(matches!(
            provider.latest(&id("ecdict")),
            Err(ProviderError::InvalidPackage { .. })
        ));

        // A directory that does not exist at all.
        let missing_root = LocalDirectoryProvider::new(scratch.path().join("nope"));
        assert!(!missing_root.describe().available);
        assert!(matches!(
            missing_root.latest(&id("ecdict")),
            Err(ProviderError::Io { .. })
        ));
    }

    #[test]
    fn local_provider_fetch_copies_the_package() {
        let scratch = Scratch::new("provider-fetch");
        let root = scratch.dir("providers");
        let source = package_dir(&scratch, "providers/ecdict-1.0", "ecdict", "1.0", b"bytes");
        let provider = LocalDirectoryProvider::new(&root);
        let remote = provider.latest(&id("ecdict")).expect("latest");

        let dest = scratch.path().join("download/ecdict-1.0");
        provider.fetch(&remote, &dest).expect("fetch");
        assert_eq!(
            fs::read(dest.join("data.bin")).expect("read"),
            b"bytes".to_vec()
        );
        assert!(dest.join("source.json").is_file());
        assert_eq!(
            checksum::sha256_dir(&source).expect("source hash"),
            checksum::sha256_dir(&dest).expect("dest hash"),
            "a fetched package must keep the source digest"
        );
    }

    #[test]
    fn http_provider_is_offline_and_unavailable() {
        let provider = HttpProvider::new("https://example.invalid/dictionaries");
        let info = provider.describe();
        assert!(!info.available);
        assert!(info.requires_network);
        assert_eq!(provider.base_url(), "https://example.invalid/dictionaries");

        let remote = RemotePackage {
            id: id("ecdict"),
            version: "1.0".to_string(),
            format: Format::Stardict,
            name: "Demo".to_string(),
            license: "MIT".to_string(),
            homepage: None,
            sha256: None,
            entries: None,
            location: "https://example.invalid/ecdict-1.0.zip".to_string(),
        };
        match provider.latest(&id("ecdict")) {
            Err(ProviderError::Unavailable(reason)) => {
                assert_eq!(reason, "network provider not implemented");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        assert!(matches!(
            provider.fetch(&remote, Path::new("unused")),
            Err(ProviderError::Unavailable(_))
        ));
    }

    /// A provider that records the calls it receives, to pin down the
    /// `latest`-then-`fetch` contract and failure propagation.
    struct RecordingProvider {
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail_fetch: bool,
    }

    impl Provider for RecordingProvider {
        fn describe(&self) -> ProviderInfo {
            ProviderInfo {
                name: "recording".to_string(),
                available: true,
                requires_network: false,
            }
        }

        fn latest(&self, id: &SourceId) -> Result<RemotePackage, ProviderError> {
            self.calls.borrow_mut().push("latest");
            Ok(RemotePackage {
                id: id.clone(),
                version: "3.0".to_string(),
                format: Format::Jsonl,
                name: "Recorded".to_string(),
                license: "MIT".to_string(),
                homepage: None,
                sha256: None,
                entries: Some(7),
                location: "recorded:3.0".to_string(),
            })
        }

        fn fetch(&self, remote: &RemotePackage, dest: &Path) -> Result<(), ProviderError> {
            self.calls.borrow_mut().push("fetch");
            if self.fail_fetch {
                return Err(ProviderError::Unavailable("fetch failed".to_string()));
            }
            fs::create_dir_all(dest).map_err(|source| ProviderError::Io {
                path: dest.to_path_buf(),
                source,
            })?;
            fs::write(dest.join("data.jsonl"), remote.version.as_bytes()).map_err(|source| {
                ProviderError::Io {
                    path: dest.to_path_buf(),
                    source,
                }
            })
        }
    }

    #[test]
    fn provider_calls_happen_latest_then_fetch() {
        let scratch = Scratch::new("provider-order");
        let calls = Rc::new(RefCell::new(Vec::new()));
        let provider = RecordingProvider {
            calls: Rc::clone(&calls),
            fail_fetch: false,
        };
        let remote = provider.latest(&id("demo")).expect("latest");
        let dest = scratch.path().join("fetched");
        provider.fetch(&remote, &dest).expect("fetch");
        assert_eq!(&*calls.borrow(), &["latest", "fetch"]);
        assert_eq!(
            fs::read_to_string(dest.join("data.jsonl")).expect("read"),
            "3.0"
        );
    }

    #[test]
    fn provider_fetch_failure_is_reported() {
        let scratch = Scratch::new("provider-failure");
        let calls = Rc::new(RefCell::new(Vec::new()));
        let provider = RecordingProvider {
            calls: Rc::clone(&calls),
            fail_fetch: true,
        };
        let remote = provider.latest(&id("demo")).expect("latest");
        assert!(matches!(
            provider.fetch(&remote, &scratch.path().join("nope")),
            Err(ProviderError::Unavailable(_))
        ));
        assert_eq!(&*calls.borrow(), &["latest", "fetch"]);
    }
}
