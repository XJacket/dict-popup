//! SHA-256 digests of a single file and of a whole package directory.
//!
//! # The directory rule (reproducible)
//!
//! `sha256_dir` (and therefore every `sha256` recorded in `library.json`) is
//! computed like this:
//!
//! 1. Walk the directory recursively. **Every regular file is included**, and
//!    the one exception is a `source.json` sitting directly at the top level of
//!    the directory: it is the library's own metadata copy, written only after
//!    the digest exists, so including it would be circular.
//!    A `source.json` inside a sub-directory is an ordinary package file and is
//!    included.
//! 2. Turn each file's path relative to the directory root into a string with
//!    `/` as the separator (Windows `\` is normalised), encoded as UTF-8.
//! 3. Sort the files by that relative path, in byte order.
//! 4. Feed one SHA-256 hasher, in that order, for each file:
//!    the relative-path bytes, then a single NUL byte (`0x00`), then the file's
//!    bytes exactly as stored, in 64 KiB chunks.
//! 5. The result is the lowercase hex encoding of the 32-byte digest.
//!
//! Because the path and a NUL separator are hashed before the content, renaming
//! a file changes the digest even when its content does not, and two different
//! file layouts cannot produce the same byte stream by concatenation. Because
//! the files are sorted first, the digest does not depend on directory listing
//! order, file creation order, timestamps or the absolute path of the
//! directory: copying a package elsewhere leaves its digest unchanged.
//!
//! An empty directory (or one containing only the skipped top-level
//! `source.json`) hashes to SHA-256 of the empty input,
//! `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
//!
//! This module intentionally knows nothing about dictionary formats: a file is
//! just bytes to it.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::StoreError;

/// Name of the metadata file that is skipped when it sits at the top level of a
/// package directory.
pub const METADATA_FILE: &str = "source.json";

/// Size of the buffer used to read files while hashing.
const READ_BUFFER: usize = 64 * 1024;

/// What a package directory contains, as seen by the checksum walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryDigest {
    /// Lowercase hex SHA-256 of the directory, computed with the rule documented
    /// at the [module level](self).
    pub sha256: String,
    /// Sum of the sizes of the hashed files, in bytes.
    pub bytes: u64,
    /// Number of hashed files.
    pub files: u64,
}

/// Hash one file's contents with SHA-256, returning lowercase hex.
///
/// Unlike [`sha256_dir`] this hashes only the bytes of the file: no path and no
/// separator are mixed in.
pub fn sha256_file(path: &Path) -> Result<String, StoreError> {
    let mut hasher = Sha256::new();
    let mut file = File::open(path).map_err(|error| StoreError::io(path, error))?;
    let mut buffer = vec![0u8; READ_BUFFER];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| StoreError::io(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Hash a package directory with SHA-256, returning lowercase hex.
///
/// The rule is documented at the [module level](self). The path must be a
/// directory; a file path is reported as
/// [`StoreError::NotADirectory`].
pub fn sha256_dir(root: &Path) -> Result<String, StoreError> {
    Ok(summarize_dir(root)?.sha256)
}

/// Walk a package directory and return its digest, total size and file count.
///
/// This is the single walk that both [`sha256_dir`] and [`crate::Library::add`]
/// use, so the recorded `sha256` and `bytes` always describe exactly the same
/// set of files.
pub fn summarize_dir(root: &Path) -> Result<DirectoryDigest, StoreError> {
    let files = package_files(root)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut bytes = 0u64;

    for file in &files {
        hasher.update(file.relative_path.as_bytes());
        hasher.update([0u8]);

        let mut handle =
            File::open(&file.path).map_err(|error| StoreError::io(&file.path, error))?;
        loop {
            let read = handle
                .read(&mut buffer)
                .map_err(|error| StoreError::io(&file.path, error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        bytes += file.size;
    }

    Ok(DirectoryDigest {
        sha256: hex_lower(&hasher.finalize()),
        bytes,
        files: files.len() as u64,
    })
}

/// One regular file found by the package walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageFile {
    /// Path relative to the package root, `/`-separated.
    pub(crate) relative_path: String,
    /// Absolute or root-relative path on disk.
    pub(crate) path: PathBuf,
    /// File size in bytes.
    pub(crate) size: u64,
}

/// Collect the files of a package directory, sorted by relative path.
pub(crate) fn package_files(root: &Path) -> Result<Vec<PackageFile>, StoreError> {
    let metadata = fs::metadata(root).map_err(|error| StoreError::io(root, error))?;
    if !metadata.is_dir() {
        return Err(StoreError::NotADirectory {
            path: root.to_path_buf(),
        });
    }

    let mut files = Vec::new();
    collect_files(root, root, true, &mut files)?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

/// Recursively collect regular files below `directory`.
///
/// `top_level` is true for the package root itself, where `source.json` is
/// skipped. Symbolic links are followed, exactly like
/// [`crate::copy_package_tree`], so the digest always describes the same file
/// set that an install would copy.
fn collect_files(
    root: &Path,
    directory: &Path,
    top_level: bool,
    files: &mut Vec<PackageFile>,
) -> Result<(), StoreError> {
    let entries = fs::read_dir(directory).map_err(|error| StoreError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| StoreError::io(directory, error))?;
        let path = entry.path();
        let metadata = fs::metadata(&path).map_err(|error| StoreError::io(&path, error))?;

        if metadata.is_dir() {
            collect_files(root, &path, false, files)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }

        let relative_path = relative_path_string(root, &path);
        if top_level && relative_path == METADATA_FILE {
            continue;
        }
        files.push(PackageFile {
            relative_path,
            path,
            size: metadata.len(),
        });
    }
    Ok(())
}

/// Render `path` relative to `root` with `/` separators.
fn relative_path_string(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut rendered = String::new();
    for component in relative.components() {
        if !rendered.is_empty() {
            rendered.push('/');
        }
        rendered.push_str(&component.as_os_str().to_string_lossy());
    }
    rendered
}

/// Lowercase hex encoding of a digest.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: [u8; 16] = *b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Scratch;
    use sha2::{Digest, Sha256};

    #[test]
    fn file_hash_matches_known_vector() {
        let scratch = Scratch::new("checksum-file");
        let path = scratch.write("abc.txt", b"abc");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn empty_directory_hashes_to_empty_input() {
        let scratch = Scratch::new("checksum-empty");
        let root = scratch.dir("empty");
        assert_eq!(
            sha256_dir(&root).expect("hash"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let digest = summarize_dir(&root).expect("summarize");
        assert_eq!(digest.bytes, 0);
        assert_eq!(digest.files, 0);
    }

    #[test]
    fn digest_matches_the_documented_byte_stream() {
        let scratch = Scratch::new("checksum-manual");
        let root = scratch.dir("pkg");
        scratch.write("pkg/a.txt", b"hello");
        scratch.write("pkg/nested/b.txt", b"world");

        let mut hasher = Sha256::new();
        hasher.update(b"a.txt");
        hasher.update([0u8]);
        hasher.update(b"hello");
        hasher.update(b"nested/b.txt");
        hasher.update([0u8]);
        hasher.update(b"world");
        let expected = hex_lower(&hasher.finalize());

        assert_eq!(sha256_dir(&root).expect("hash"), expected);
        let digest = summarize_dir(&root).expect("summarize");
        assert_eq!(digest.bytes, 10);
        assert_eq!(digest.files, 2);
    }

    #[test]
    fn digest_does_not_depend_on_creation_order() {
        let scratch = Scratch::new("checksum-order");
        let first = scratch.dir("first");
        let second = scratch.dir("second");

        scratch.write("first/z.txt", b"last");
        scratch.write("first/a.txt", b"first");
        scratch.write("first/m/nested.txt", b"nested");

        scratch.write("second/m/nested.txt", b"nested");
        scratch.write("second/a.txt", b"first");
        scratch.write("second/z.txt", b"last");

        assert_eq!(
            sha256_dir(&first).expect("first hash"),
            sha256_dir(&second).expect("second hash")
        );
    }

    #[test]
    fn digest_changes_with_content_or_path() {
        let scratch = Scratch::new("checksum-changes");
        let base = scratch.dir("base");
        scratch.write("base/a.txt", b"one");
        let digest = sha256_dir(&base).expect("hash");

        scratch.write("base/a.txt", b"two");
        assert_ne!(sha256_dir(&base).expect("hash"), digest);

        scratch.write("base/a.txt", b"one");
        assert_eq!(sha256_dir(&base).expect("hash"), digest);

        fs::rename(base.join("a.txt"), base.join("b.txt")).expect("rename");
        assert_ne!(sha256_dir(&base).expect("hash"), digest);
    }

    #[test]
    fn top_level_metadata_is_skipped_but_nested_is_not() {
        let scratch = Scratch::new("checksum-metadata");
        let root = scratch.dir("pkg");
        scratch.write("pkg/source.json", br#"{"id":"demo"}"#);
        let only_metadata = sha256_dir(&root).expect("hash");
        assert_eq!(
            only_metadata,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        scratch.write("pkg/data.txt", b"payload");
        let with_data = sha256_dir(&root).expect("hash");
        assert_ne!(with_data, only_metadata);

        scratch.write("pkg/nested/source.json", b"{}");
        assert_ne!(sha256_dir(&root).expect("hash"), with_data);
    }

    #[test]
    fn non_directory_is_rejected() {
        let scratch = Scratch::new("checksum-not-dir");
        let path = scratch.write("file.txt", b"x");
        assert!(matches!(
            sha256_dir(&path),
            Err(StoreError::NotADirectory { .. })
        ));
    }
}
