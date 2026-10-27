//! [`StoreError`]: every failure this crate can report, each carrying the path
//! or the dictionary id that caused it.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::SourceId;

/// Every failure that [`crate::Library`] or [`crate::manifest`] can report.
///
/// Variants carry context (the path that could not be read, the id and version
/// that were rejected, the field that was empty) so a caller can show the user
/// a message without guessing. The [`fmt::Display`] implementation is written
/// in Chinese because the CLI and the popup show these strings directly.
#[derive(Debug)]
pub enum StoreError {
    /// Reading, creating, renaming or deleting a file or directory failed.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying operating-system error.
        source: io::Error,
    },
    /// `library.json` exists but is not valid JSON or is not a manifest.
    ///
    /// The file is deliberately left untouched: the user can still recover it
    /// by hand, and the library refuses to overwrite it.
    ManifestUnparsable {
        /// Path of the manifest.
        path: PathBuf,
        /// The JSON error that made it unreadable.
        source: serde_json::Error,
    },
    /// `library.json` parsed as JSON but its content is not a usable manifest
    /// (unknown schema version, duplicate `(id, version)`, invalid package).
    ManifestInvalid {
        /// Path of the manifest.
        path: PathBuf,
        /// What is wrong with it, in user-facing Chinese.
        reason: String,
    },
    /// A value could not be serialized back to JSON.
    JsonWrite {
        /// The file that was being produced.
        path: PathBuf,
        /// The JSON error.
        source: serde_json::Error,
    },
    /// A package's own `source.json` could not be parsed.
    SourceJsonUnparsable {
        /// Path of the `source.json` that failed.
        path: PathBuf,
        /// The JSON error.
        source: serde_json::Error,
    },
    /// The dictionary id has no installed version at all.
    PackageNotFound {
        /// The dictionary id that is not installed.
        id: SourceId,
    },
    /// The dictionary id is installed, but not in the requested version.
    VersionNotFound {
        /// The dictionary id.
        id: SourceId,
        /// The version that is missing.
        version: String,
    },
    /// A rollback needs an active version to step back from, and there is none.
    NoActiveVersion {
        /// The dictionary id.
        id: SourceId,
    },
    /// Rollback found no version older than the currently active one.
    NoPreviousVersion {
        /// The dictionary id.
        id: SourceId,
        /// The version that was active (the newest installed one).
        current: String,
    },
    /// `(id, version, sha256)` is already installed, so there is nothing to do.
    DuplicatePackage {
        /// The dictionary id.
        id: SourceId,
        /// The version that is already present.
        version: String,
        /// The content digest that is already recorded.
        sha256: String,
    },
    /// `(id, version)` exists with a different `sha256`; replacing it needs
    /// an explicit `force`.
    SameVersionDifferentContent {
        /// The dictionary id.
        id: SourceId,
        /// The version whose content differs.
        version: String,
    },
    /// An upgrade was handed a version that does not sort after the installed
    /// newest one, and `force` was not set.
    VersionNotNewer {
        /// The dictionary id.
        id: SourceId,
        /// The newest version currently installed.
        current: String,
        /// The version that was offered.
        incoming: String,
    },
    /// A directory named `<id>-<version>` exists but no package record does.
    VersionDirectoryExists {
        /// The occupied directory.
        path: PathBuf,
    },
    /// The path that should hold package files is not a directory.
    NotADirectory {
        /// The offending path.
        path: PathBuf,
    },
    /// A required metadata field was neither passed in nor found in the
    /// package's `source.json`.
    MissingMetadata {
        /// The missing field name (`id`, `version`, `format`, `name`, `license`).
        field: &'static str,
        /// The package directory that was examined.
        source_dir: PathBuf,
    },
    /// A metadata field was present but empty or otherwise unusable.
    InvalidField {
        /// The field name.
        field: &'static str,
        /// The rejected value, quoted in the message.
        value: String,
        /// Why it was rejected, in user-facing Chinese.
        reason: &'static str,
    },
    /// A dictionary id is not usable as a directory-name component.
    InvalidSourceId {
        /// The rejected id.
        raw: String,
        /// Why it was rejected, in user-facing Chinese.
        reason: &'static str,
    },
    /// A version string is not usable as a directory-name component.
    InvalidVersion {
        /// The rejected version.
        raw: String,
        /// Why it was rejected, in user-facing Chinese.
        reason: &'static str,
    },
}

impl StoreError {
    /// Wrap an [`io::Error`] together with the path it happened on.
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        StoreError::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io { path, source } => {
                write!(f, "无法访问 {}：{}", path.display(), source)
            }
            StoreError::ManifestUnparsable { path, source } => write!(
                f,
                "无法解析清单 {}（文件保持原样，请手工修复）：{}",
                path.display(),
                source
            ),
            StoreError::ManifestInvalid { path, reason } => {
                write!(f, "清单 {} 内容无效：{}", path.display(), reason)
            }
            StoreError::JsonWrite { path, source } => {
                write!(f, "无法生成 JSON 写入 {}：{}", path.display(), source)
            }
            StoreError::SourceJsonUnparsable { path, source } => {
                write!(f, "无法解析包元数据 {}：{}", path.display(), source)
            }
            StoreError::PackageNotFound { id } => {
                write!(f, "词典 {} 尚未安装（可以先用 add 添加）", id)
            }
            StoreError::VersionNotFound { id, version } => {
                write!(f, "词典 {} 没有版本 {}", id, version)
            }
            StoreError::NoActiveVersion { id } => {
                write!(f, "词典 {} 没有启用中的版本，无法回退", id)
            }
            StoreError::NoPreviousVersion { id, current } => write!(
                f,
                "词典 {} 的当前版本 {} 已经没有更早的版本可以回退",
                id, current
            ),
            StoreError::DuplicatePackage {
                id,
                version,
                sha256,
            } => write!(
                f,
                "词典 {} {}（sha256 {}）已经安装，无需重复安装",
                id, version, sha256
            ),
            StoreError::SameVersionDifferentContent { id, version } => write!(
                f,
                "词典 {} 的版本 {} 已存在但内容不同（sha256 不一致）；确认要替换请加 --force",
                id, version
            ),
            StoreError::VersionNotNewer {
                id,
                current,
                incoming,
            } => write!(
                f,
                "词典 {} 当前最新版本是 {}，待安装的 {} 并不更新；确认要安装请加 --force",
                id, current, incoming
            ),
            StoreError::VersionDirectoryExists { path } => write!(
                f,
                "目录 {} 已存在但清单中没有对应记录；确认要覆盖请加 --force",
                path.display()
            ),
            StoreError::NotADirectory { path } => {
                write!(f, "{} 不是目录", path.display())
            }
            StoreError::MissingMetadata { field, source_dir } => write!(
                f,
                "缺少 {}：请在命令行提供，或在 {} 的 source.json 里写明",
                field,
                source_dir.display()
            ),
            StoreError::InvalidField {
                field,
                value,
                reason,
            } => write!(f, "{} 的值 \"{}\" 无效：{}", field, value, reason),
            StoreError::InvalidSourceId { raw, reason } => {
                write!(f, "词典 id \"{}\" 无效：{}", raw, reason)
            }
            StoreError::InvalidVersion { raw, reason } => {
                write!(f, "版本 \"{}\" 无效：{}", raw, reason)
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io { source, .. } => Some(source),
            StoreError::ManifestUnparsable { source, .. } => Some(source),
            StoreError::JsonWrite { source, .. } => Some(source),
            StoreError::SourceJsonUnparsable { source, .. } => Some(source),
            _ => None,
        }
    }
}
