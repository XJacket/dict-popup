//! A tiny append-only log file (`%APPDATA%\dict-popup\dict-popup.log`).
//!
//! DESIGN §9.2: the selected text is **never** logged, only its length. Nothing
//! in this module accepts a selection, so that rule cannot be broken by a
//! careless call site.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERR ",
        }
    }
}

static SINK: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// Open (and rotate) the log file. Safe to call once; later calls are ignored.
pub fn init(path: PathBuf) {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = fs::create_dir_all(dir);
        }
    }

    rotate_if_needed(&path);

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let _ = SINK.set(Mutex::new(file));
}

pub fn info(message: impl AsRef<str>) {
    write(Level::Info, message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write(Level::Warn, message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    write(Level::Error, message.as_ref());
}

fn write(level: Level, message: &str) {
    let Some(sink) = SINK.get() else {
        return;
    };
    let Ok(mut guard) = sink.lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    let _ = writeln!(file, "[{}] {} {}", timestamp(), level.tag(), message);
}

fn rotate_if_needed(path: &Path) {
    let too_big = fs::metadata(path)
        .map(|meta| meta.len() > MAX_BYTES)
        .unwrap_or(false);
    if !too_big {
        return;
    }

    let mut old_name = path.as_os_str().to_owned();
    old_name.push(".old");
    let old = PathBuf::from(old_name);
    let _ = fs::remove_file(&old);
    let _ = fs::rename(path, old);
}

/// `YYYY-MM-DD HH:MM:SSZ` in UTC, without pulling in `chrono`.
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);

    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn writes_lines_without_panicking_when_uninitialised() {
        // No `init` called: must be a silent no-op rather than a panic.
        info("test line");
    }

    #[test]
    fn log_file_is_appended_to() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("log-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.log");
        let _ = fs::remove_file(&path);

        // The global sink can only be initialised once per process; if another
        // test got there first this still just checks the file path helper.
        init(path.clone());
        info("hello");
        let contents = fs::read_to_string(&path).unwrap_or_default();
        assert!(contents.contains("hello"), "contents: {contents}");
        assert!(contents.contains("INFO"));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&dir);
    }
}
