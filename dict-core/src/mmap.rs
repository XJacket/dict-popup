//! Fast read-only memory mapping for large dictionary files.
//!
//! The `.idx` (82 MB) and `.dict` (118 MB) files must not be loaded into
//! process memory: the app's budget is 100 MB total. A read-only file mapping
//! keeps them in the OS file cache where they are shared and evictable.

use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS,
    PAGE_READONLY,
};

pub struct MappedFile {
    // Kept alive for the lifetime of the mapping.
    _file: File,
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

impl MappedFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is empty", path.display()),
            ));
        }

        let file_handle = HANDLE(file.as_raw_handle());
        // SAFETY: the handle is valid for as long as `file` lives, and we ask
        // only for a read-only view.
        let mapping = unsafe { CreateFileMappingW(file_handle, None, PAGE_READONLY, 0, 0, None) }
            .map_err(|e| io::Error::other(format!("CreateFileMappingW: {e}")))?;

        if mapping.is_invalid() {
            unsafe {
                let _ = CloseHandle(mapping);
            }
            return Err(io::Error::last_os_error());
        }

        // SAFETY: read-only mapping of `len` bytes from an open handle.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            let err = io::Error::last_os_error();
            unsafe {
                let _ = CloseHandle(mapping);
            }
            return Err(err);
        }

        Ok(Self {
            _file: file,
            mapping,
            view,
            len,
        })
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `view` points at `len` readable bytes owned by the mapping,
        // which outlives every borrow of `self`.
        unsafe { std::slice::from_raw_parts(self.view.Value as *const u8, self.len) }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for MappedFile {
    fn drop(&mut self) {
        // SAFETY: both handles came from the Win32 calls above and are
        // released exactly once, in reverse order of acquisition.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.mapping);
        }
    }
}

// SAFETY: the mapping is read-only, so sharing the bytes across threads cannot
// introduce data races. Callers only ever get `&[u8]`.
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}
