//! Persistent configuration (`%APPDATA%\dict-popup\config.json`).
//!
//! Schema: `docs/DESIGN.md` §4. The rules that matter:
//!
//! - a **missing** file is created with defaults;
//! - a file that **fails to parse** is never overwritten (约束 9.3) — the caller
//!   gets an error it can show, plus a fallback so the app still runs;
//! - every field is `#[serde(default)]`, so old/partial files keep working;
//! - saves are atomic (temp file + rename) and keep `config.json.bak`.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use dict_core::Direction;

pub const APP_DIR: &str = "dict-popup";
pub const CONFIG_FILE: &str = "config.json";
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+D";
pub const CONFIG_VERSION: u32 = 1;

/// `%APPDATA%\dict-popup`, with a fallback for portable/odd environments.
pub fn config_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join(APP_DIR);
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join("AppData")
                .join("Roaming")
                .join(APP_DIR);
        }
    }
    PathBuf::from(".").join(APP_DIR)
}

pub fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILE)
}

pub fn log_path() -> PathBuf {
    config_dir().join("dict-popup.log")
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// Canonical string, see `dict_input::hotkey`.
    pub hotkey: String,
    pub dictionaries: Vec<DictionaryConfig>,
    pub ui: UiConfig,
    pub behavior: BehaviorConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            hotkey: DEFAULT_HOTKEY.to_string(),
            dictionaries: Vec::new(),
            ui: UiConfig::default(),
            behavior: BehaviorConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictionaryConfig {
    /// Full path to the `.ifo` file.
    pub path: String,
    /// User-visible name; defaults to the `.ifo` `bookname`.
    pub label: String,
    pub enabled: bool,
    pub direction: DirectionSpec,
}

impl Default for DictionaryConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            label: String::new(),
            enabled: true,
            direction: DirectionSpec::Auto,
        }
    }
}

impl DictionaryConfig {
    pub fn new(path: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            label: label.into(),
            enabled: true,
            direction: DirectionSpec::Auto,
        }
    }

    pub fn path_buf(&self) -> PathBuf {
        PathBuf::from(self.path.trim())
    }
}

/// `direction` in the config file. `Auto` defers to `dict_core::fingerprint`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DirectionSpec {
    #[default]
    Auto,
    EnZh,
    ZhEn,
    EnEn,
    ZhZh,
}

impl DirectionSpec {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::EnZh,
        Self::ZhEn,
        Self::EnEn,
        Self::ZhZh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::EnZh => "英→中",
            Self::ZhEn => "中→英",
            Self::EnEn => "英→英",
            Self::ZhZh => "中→中",
        }
    }

    /// The explicitly configured direction, or `None` for `auto`.
    pub fn explicit(self) -> Option<Direction> {
        match self {
            Self::Auto => None,
            Self::EnZh => Some(Direction::EnZh),
            Self::ZhEn => Some(Direction::ZhEn),
            Self::EnEn => Some(Direction::EnEn),
            Self::ZhZh => Some(Direction::ZhZh),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "浅色",
            Self::Dark => "深色",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: ThemeMode,
    pub font_size: f32,
    pub card_width: f32,
    pub max_card_height: f32,
    pub close_on_focus_lost: bool,
    /// 0 = never auto-hide.
    pub auto_hide_seconds: u32,
    pub expand_first_dictionary: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            font_size: 15.0,
            card_width: 460.0,
            max_card_height: 520.0,
            close_on_focus_lost: true,
            auto_hide_seconds: 0,
            expand_first_dictionary: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub trim_selection: bool,
    pub autostart: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            trim_selection: true,
            autostart: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

impl Config {
    /// Clamp out-of-range values and normalise strings so a hand-edited file
    /// can never make the UI unreadable.
    pub fn sanitize(&mut self) {
        self.version = CONFIG_VERSION;
        self.hotkey = self.hotkey.trim().to_string();
        if self.hotkey.is_empty() {
            self.hotkey = DEFAULT_HOTKEY.to_string();
        }
        for dict in &mut self.dictionaries {
            dict.path = dict.path.trim().to_string();
            dict.label = dict.label.trim().to_string();
        }
        self.ui.font_size = clamp(self.ui.font_size, 9.0, 32.0, UiConfig::default().font_size);
        self.ui.card_width = clamp(
            self.ui.card_width,
            280.0,
            900.0,
            UiConfig::default().card_width,
        );
        self.ui.max_card_height = clamp(
            self.ui.max_card_height,
            160.0,
            1400.0,
            UiConfig::default().max_card_height,
        );
        self.ui.auto_hide_seconds = self.ui.auto_hide_seconds.min(3600);
    }
}

fn clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if !value.is_finite() {
        return fallback;
    }
    value.clamp(min, max)
}

// ---------------------------------------------------------------------------
// Load / save
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ConfigError {
    pub path: PathBuf,
    pub message: String,
    /// Defaults the app can keep running with; the user's file is untouched.
    pub fallback: Config,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.message, self.path.display())
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Load `%APPDATA%\dict-popup\config.json`.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(config_path())
    }

    /// Load from an explicit path (used by tests and portable setups).
    ///
    /// A missing file is created with defaults and returned as `Ok`.
    /// A file that cannot be parsed is returned as `Err` and left untouched.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();

        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let config = Self::default();
                return match config.save_to(path) {
                    Ok(()) => Ok(config),
                    Err(write_err) => Err(ConfigError {
                        path: path.to_path_buf(),
                        message: format!("无法创建默认配置文件：{write_err}"),
                        fallback: config,
                    }),
                };
            }
            Err(err) => {
                return Err(ConfigError {
                    path: path.to_path_buf(),
                    message: format!("无法读取配置文件：{err}"),
                    fallback: Self::default(),
                });
            }
        };

        // Tolerate a UTF-8 BOM. PowerShell 5.1 (`-Encoding UTF8`) and older
        // Notepad write one by default, and serde_json rejects it - a user who
        // hand-edits the config must not lose their settings over that.
        let payload: &[u8] = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]) {
            Some(rest) => rest,
            None => &bytes,
        };

        match serde_json::from_slice::<Self>(payload) {
            Ok(mut config) => {
                config.sanitize();
                Ok(config)
            }
            Err(err) => Err(ConfigError {
                path: path.to_path_buf(),
                message: format!("配置文件解析失败（已保留原文件，请修复或删除后重启）：{err}"),
                fallback: Self::default(),
            }),
        }
    }

    /// Atomic save: write `config.json.tmp`, keep `config.json.bak`, rename.
    pub fn save_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }

        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = sibling(path, ".tmp");
        let bak = sibling(path, ".bak");

        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&json)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }

        // Keep the previous contents before replacing them, so a bad save never
        // loses the user's configuration.
        if path.exists() {
            if let Err(err) = fs::copy(path, &bak) {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
        }

        if let Err(err) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }

        Ok(())
    }
}

/// `config.json` + `suffix`, e.g. `config.json.tmp`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from(CONFIG_FILE));
    name.push(suffix);

    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// `%TEMP%` is not writable from cargo child processes on this machine, so
    /// scratch files live under the (gitignored) crate `target/` dir.
    fn test_tmp_base() -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        fs::create_dir_all(&base).expect("create test-tmp");
        base
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = format!(
                "config-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let dir = test_tmp_base().join(unique);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            self.0.join(CONFIG_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = TempDir::new();
        let path = dir.path();
        assert!(!path.exists());

        let config = Config::load_from(&path).expect("missing file must load");
        assert_eq!(config, Config::default());
        assert!(path.exists(), "defaults must be written to disk");

        // The file just written must parse back to the same thing.
        let reread = Config::load_from(&path).expect("defaults must parse");
        assert_eq!(reread, config);
    }

    #[test]
    fn bad_json_is_not_overwritten() {
        let dir = TempDir::new();
        let path = dir.path();
        let garbage = b"{ this is not json at all ";
        fs::write(&path, garbage).expect("write garbage");

        let err = Config::load_from(&path).expect_err("bad json must be an error");
        assert!(
            err.message.contains("解析失败"),
            "unexpected message: {}",
            err.message
        );
        assert_eq!(err.fallback, Config::default());

        let on_disk = fs::read(&path).expect("read back");
        assert_eq!(
            on_disk, garbage,
            "a parse failure must never overwrite the user's file"
        );
    }

    #[test]
    fn tolerates_utf8_bom() {
        let dir = TempDir::new();
        let path = dir.path();

        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"hotkey":"Alt+Shift+Q"}"#);
        fs::write(&path, &bytes).expect("write bom file");

        let config = Config::load_from(&path).expect("a BOM must not break loading");
        assert_eq!(config.hotkey, "Alt+Shift+Q");
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let dir = TempDir::new();
        let path = dir.path();

        let mut config = Config::default();
        config.hotkey = "Alt+Shift+Q".to_string();
        config.dictionaries = vec![
            DictionaryConfig {
                path: r"D:\dicts\ecdict.ifo".to_string(),
                label: "ECDICT".to_string(),
                enabled: true,
                direction: DirectionSpec::EnZh,
            },
            DictionaryConfig {
                path: r"D:\dicts\han.ifo".to_string(),
                label: "汉典".to_string(),
                enabled: false,
                direction: DirectionSpec::Auto,
            },
        ];
        config.ui.theme = ThemeMode::Dark;
        config.ui.font_size = 17.0;
        config.ui.card_width = 520.0;
        config.ui.max_card_height = 640.0;
        config.ui.close_on_focus_lost = false;
        config.ui.auto_hide_seconds = 5;
        config.ui.expand_first_dictionary = false;
        config.behavior.trim_selection = false;
        config.behavior.autostart = true;

        config.save_to(&path).expect("save");
        let loaded = Config::load_from(&path).expect("load");
        assert_eq!(loaded, config);
    }

    #[test]
    fn missing_fields_get_defaults() {
        let dir = TempDir::new();
        let path = dir.path();
        // Only a subset of the schema, as an old release might have written it.
        fs::write(
            &path,
            br#"{"hotkey":"F9","ui":{"theme":"light"}}"#,
        )
        .expect("write partial");

        let config = Config::load_from(&path).expect("partial file must load");
        assert_eq!(config.hotkey, "F9");
        assert_eq!(config.ui.theme, ThemeMode::Light);
        assert_eq!(config.ui.font_size, UiConfig::default().font_size);
        assert_eq!(config.ui.card_width, UiConfig::default().card_width);
        assert!(config.behavior.trim_selection);
        assert_eq!(config.version, CONFIG_VERSION);
        assert!(config.dictionaries.is_empty());
    }

    #[test]
    fn save_keeps_previous_contents_as_bak() {
        let dir = TempDir::new();
        let path = dir.path();

        let first = Config::default();
        first.save_to(&path).expect("first save");
        let first_bytes = fs::read(&path).expect("read first");

        let mut second = Config::default();
        second.hotkey = "Ctrl+Alt+F".to_string();
        second.save_to(&path).expect("second save");

        let bak = sibling(&path, ".bak");
        assert!(bak.exists(), "backup file must exist");
        assert_eq!(fs::read(&bak).expect("read bak"), first_bytes);
        assert_eq!(
            Config::load_from(&path).expect("load second").hotkey,
            "Ctrl+Alt+F"
        );
    }

    #[test]
    fn sanitize_clamps_hostile_values() {
        let mut config = Config::default();
        config.hotkey = "   ".to_string();
        config.ui.font_size = 400.0;
        config.ui.card_width = f32::NAN;
        config.ui.max_card_height = 1.0;
        config.sanitize();

        assert_eq!(config.hotkey, DEFAULT_HOTKEY);
        assert_eq!(config.ui.font_size, 32.0);
        assert_eq!(config.ui.card_width, UiConfig::default().card_width);
        assert_eq!(config.ui.max_card_height, 160.0);
    }

    #[test]
    fn direction_spec_round_trips_through_json() {
        for spec in DirectionSpec::ALL {
            let json = serde_json::to_string(&spec).expect("serialize");
            let back: DirectionSpec = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, spec, "direction {spec:?} did not survive {json}");
        }
        assert_eq!(
            serde_json::to_string(&DirectionSpec::EnZh).expect("serialize"),
            "\"en-zh\""
        );
        assert_eq!(DirectionSpec::ZhZh.explicit(), Some(Direction::ZhZh));
        assert_eq!(DirectionSpec::Auto.explicit(), None);
    }
}
