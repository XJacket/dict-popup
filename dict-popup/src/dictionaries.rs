//! Dictionary registry: opens the enabled `.ifo` files, fingerprints them,
//! resolves `direction: auto`, filters by the query's script, and returns the
//! hits grouped per dictionary.
//!
//! Everything here degrades instead of panicking: a missing or unsupported
//! dictionary becomes a marked row that the settings window can show.

use std::fmt;
use std::path::{Path, PathBuf};

use dict_core::{fingerprint, Direction, Dictionary, Entry};

use crate::config::{Config, DictionaryConfig, DirectionSpec};

/// Below this the automatic direction guess is shown in yellow ("自动识别不确定").
pub const LOW_CONFIDENCE: f32 = 0.85;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Opened, fingerprinted and queryable.
    Ready {
        direction: Direction,
        confidence: f32,
        low_confidence: bool,
        entries: usize,
    },
    /// `enabled: false` — not opened, so it costs neither time nor an mmap.
    Disabled,
    /// The `.ifo` (or a sibling file) is gone.
    Missing(String),
    /// Present but unreadable / unsupported (e.g. a compressed `.dict.dz`).
    Broken(String),
}

impl Status {
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Missing(message) | Self::Broken(message) => Some(message),
            _ => None,
        }
    }

    pub fn direction(&self) -> Option<Direction> {
        match self {
            Self::Ready { direction, .. } => Some(*direction),
            _ => None,
        }
    }

    pub fn low_confidence(&self) -> bool {
        matches!(self, Self::Ready { low_confidence: true, .. })
    }

    pub fn summarize(&self) -> String {
        match self {
            Self::Ready {
                direction,
                confidence,
                entries,
                low_confidence,
            } => {
                let mut text = format!("{} · 置信度 {:.0}% · {} 词条", direction.label(), confidence * 100.0, entries);
                if *low_confidence {
                    text.push_str(" · 自动识别不确定");
                }
                text
            }
            Self::Disabled => "未启用".to_string(),
            Self::Missing(_) => "文件缺失".to_string(),
            Self::Broken(_) => "无法打开".to_string(),
        }
    }
}

/// One row of the configured dictionary list.
pub struct LoadedDictionary {
    pub config: DictionaryConfig,
    pub path: PathBuf,
    pub label: String,
    pub status: Status,
    dict: Option<Dictionary>,
}

impl LoadedDictionary {
    pub fn is_queryable(&self) -> bool {
        self.config.enabled && self.dict.is_some()
    }

    pub fn dict(&self) -> Option<&Dictionary> {
        self.dict.as_ref()
    }
}

impl fmt::Debug for LoadedDictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedDictionary")
            .field("label", &self.label)
            .field("path", &self.path)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Script detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryScript {
    /// The query contains at least one Han character.
    Han,
    /// Pure ASCII alphanumerics + neutral separators.
    Latin,
    /// Anything else (Cyrillic, accented Latin, emoji, nothing at all ...).
    Other,
}

/// Which script a query is written in (DESIGN §7.5).
pub fn query_script(query: &str) -> QueryScript {
    let mut han = false;
    let mut latin = false;
    let mut other = false;

    for ch in query.chars() {
        if is_han(ch) {
            han = true;
        } else if ch.is_ascii_alphanumeric() {
            latin = true;
        } else if ch.is_ascii_whitespace() || ch.is_ascii_punctuation() {
            // Neutral: does not decide the script.
        } else {
            other = true;
        }
    }

    if han {
        QueryScript::Han
    } else if other {
        QueryScript::Other
    } else if latin {
        QueryScript::Latin
    } else {
        QueryScript::Other
    }
}

/// Can a dictionary of `direction` answer a query in `script`?
pub fn direction_matches(direction: Direction, script: QueryScript) -> bool {
    match direction {
        // Unknown: we cannot tell, so do not hide the dictionary from the user.
        Direction::Unknown => true,
        Direction::ZhEn | Direction::ZhZh => script != QueryScript::Latin,
        Direction::EnZh | Direction::EnEn => script != QueryScript::Han,
    }
}

fn is_han(ch: char) -> bool {
    matches!(ch as u32,
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x20000..=0x2FA1F)
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Registry {
    rows: Vec<LoadedDictionary>,
}

impl Registry {
    /// Open every enabled dictionary. Never fails: problems live in the rows.
    pub fn load(config: &Config) -> Self {
        let rows = config
            .dictionaries
            .iter()
            .map(load_one)
            .collect::<Vec<_>>();
        Self { rows }
    }

    pub fn rows(&self) -> &[LoadedDictionary] {
        &self.rows
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// How many dictionaries can actually answer a lookup.
    pub fn ready_count(&self) -> usize {
        self.rows.iter().filter(|row| row.is_queryable()).count()
    }

    /// Query every enabled, direction-matching dictionary in configured order.
    ///
    /// `Dictionary::lookup` already runs dict-core's fallback chain per
    /// dictionary, so one call each is enough.
    pub fn lookup(&self, query: &str) -> LookupReport {
        let script = query_script(query);
        let mut report = LookupReport {
            query: query.to_string(),
            script,
            candidates: dict_core::normalize::candidates(query)
                .into_iter()
                .take(3)
                .map(|candidate| candidate.term)
                .collect(),
            groups: Vec::new(),
            missed: Vec::new(),
            searched: 0,
            filtered: 0,
        };

        // Public-API guard, not a capture-path decision: this function is also
        // called by tests and `--demo` with arbitrary text. The hotkey path
        // never reaches here empty, because dict-input's CapturedText
        // guarantees non-empty text.
        if query.trim().is_empty() {
            return report;
        }

        for row in &self.rows {
            let Some(dict) = row.dict() else {
                continue;
            };
            if !row.config.enabled {
                continue;
            }
            let direction = row.status.direction().unwrap_or(Direction::Unknown);
            if !direction_matches(direction, script) {
                report.filtered += 1;
                continue;
            }

            report.searched += 1;

            if let Some(hit) = dict.lookup(query) {
                report.groups.push(HitGroup {
                    label: row.label.clone(),
                    direction,
                    low_confidence: row.status.low_confidence(),
                    entry: hit.entry,
                    matched: hit.matched,
                    reason: hit.reason,
                    attempts: hit.attempts,
                });
            } else {
                report.missed.push(row.label.clone());
            }
        }

        report
    }
}

/// Result of one query, shaped for the card UI (DESIGN §5).
#[derive(Debug, Clone)]
pub struct LookupReport {
    /// The text as captured (after trimming), for display only.
    pub query: String,
    pub script: QueryScript,
    /// Up to three normalised candidates, listed when nothing matched.
    pub candidates: Vec<String>,
    /// One group per dictionary that hit, in configured order.
    pub groups: Vec<HitGroup>,
    /// Dictionaries that were queried but had no hit.
    pub missed: Vec<String>,
    /// Dictionaries actually queried.
    pub searched: usize,
    /// Dictionaries skipped because their direction did not fit the script.
    pub filtered: usize,
}

impl LookupReport {
    pub fn hits(&self) -> usize {
        self.groups.len()
    }
}

#[derive(Debug, Clone)]
pub struct HitGroup {
    pub label: String,
    pub direction: Direction,
    pub low_confidence: bool,
    pub entry: Entry,
    pub matched: String,
    pub reason: &'static str,
    pub attempts: usize,
}

// ---------------------------------------------------------------------------
// Opening one dictionary
// ---------------------------------------------------------------------------

fn load_one(config: &DictionaryConfig) -> LoadedDictionary {
    let path = config.path_buf();
    let fallback_label = label_from_path(&path);

    if !config.enabled {
        return LoadedDictionary {
            config: config.clone(),
            path,
            label: if config.label.is_empty() {
                fallback_label
            } else {
                config.label.clone()
            },
            status: Status::Disabled,
            dict: None,
        };
    }

    if path.as_os_str().is_empty() {
        return LoadedDictionary {
            config: config.clone(),
            path,
            label: fallback_label,
            status: Status::Missing("未设置词典路径".to_string()),
            dict: None,
        };
    }

    if !path.exists() {
        return LoadedDictionary {
            config: config.clone(),
            path: path.clone(),
            label: if config.label.is_empty() {
                fallback_label
            } else {
                config.label.clone()
            },
            status: Status::Missing(format!("找不到词典文件：{}", path.display())),
            dict: None,
        };
    }

    match Dictionary::open(&path) {
        Ok(dict) => {
            let fp = fingerprint(&dict);
            let low_confidence = config.direction == DirectionSpec::Auto
                && fp.confidence < LOW_CONFIDENCE;
            let direction = config.direction.explicit().unwrap_or(fp.direction);
            let label = if config.label.is_empty() {
                let bookname = dict.ifo.bookname.trim();
                if bookname.is_empty() {
                    fallback_label
                } else {
                    bookname.to_string()
                }
            } else {
                config.label.clone()
            };

            LoadedDictionary {
                config: config.clone(),
                path,
                label,
                status: Status::Ready {
                    direction,
                    confidence: fp.confidence,
                    low_confidence,
                    entries: dict.entry_count(),
                },
                dict: Some(dict),
            }
        }
        Err(err) => LoadedDictionary {
            config: config.clone(),
            path: path.clone(),
            label: if config.label.is_empty() {
                fallback_label
            } else {
                config.label.clone()
            },
            status: Status::Broken(describe_open_error(&path, &err)),
            dict: None,
        },
    }
}

/// User-facing Chinese reason, with the usual StarDict traps spelled out.
fn describe_open_error(path: &Path, err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::Unsupported => {
            format!("不支持压缩词典（.dict.dz），请先解压为 .dict：{}", path.display())
        }
        std::io::ErrorKind::NotFound => {
            format!("词典缺少配套文件（.idx / .dict）：{}", path.display())
        }
        std::io::ErrorKind::PermissionDenied => {
            format!("没有读取权限：{}", path.display())
        }
        _ => format!("无法打开 {}：{err}", path.display()),
    }
}

fn label_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_string())
        .unwrap_or_else(|| "未命名词典".to_string())
}

/// Read the `bookname=` line of an `.ifo` without opening the whole dictionary.
///
/// Used by the settings "add dictionary" flow, where opening a 190 MB index just
/// to learn its name would freeze the UI.
pub fn read_bookname(ifo_path: impl AsRef<Path>) -> Option<String> {
    let text = std::fs::read_to_string(ifo_path).ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("bookname="))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

// ---------------------------------------------------------------------------
// Tests: synthetic StarDict files in a temp dir
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DictionaryConfig};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// `%TEMP%` is not writable from cargo child processes on this machine, so
    /// scratch dictionaries live under the (gitignored) crate `target/` dir.
    fn test_tmp_base() -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        fs::create_dir_all(&base).expect("create test-tmp");
        base
    }

    struct TempDict {
        dir: PathBuf,
        stem: PathBuf,
    }

    impl TempDict {
        /// Build `<dir>/<name>.ifo|.idx|.dict` sorted by lowercase headword.
        fn build(name: &str, entries: &[(&str, &str)]) -> Self {
            let mut sorted = entries.to_vec();
            sorted.sort_by_key(|(word, _)| word.to_ascii_lowercase());

            let mut idx = Vec::new();
            let mut dict = Vec::new();
            for (word, definition) in &sorted {
                let offset = dict.len() as u32;
                dict.extend_from_slice(definition.as_bytes());
                idx.extend_from_slice(word.as_bytes());
                idx.push(0);
                idx.extend_from_slice(&offset.to_be_bytes());
                idx.extend_from_slice(&(definition.len() as u32).to_be_bytes());
            }

            let unique = format!(
                "registry-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let dir = test_tmp_base().join(unique);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");

            let stem = dir.join(name);
            fs::write(
                stem.with_extension("ifo"),
                format!(
                    "StarDict's dict ifo file\nversion=2.4.2\nwordcount={}\nidxfilesize={}\nbookname={name}\nsametypesequence=m\n",
                    sorted.len(),
                    idx.len()
                ),
            )
            .expect("write ifo");
            fs::write(stem.with_extension("idx"), &idx).expect("write idx");
            fs::write(stem.with_extension("dict"), &dict).expect("write dict");

            Self { dir, stem }
        }

        fn ifo(&self) -> PathBuf {
            self.stem.with_extension("ifo")
        }

        fn row(&self, label: &str) -> DictionaryConfig {
            DictionaryConfig::new(self.ifo().to_string_lossy(), label)
        }
    }

    impl Drop for TempDict {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn en_zh() -> TempDict {
        TempDict::build(
            "en-zh",
            &[
                ("apple", "n. 苹果"),
                ("banana", "n. 香蕉"),
                ("computer", "n. 计算机"),
                ("run", "v. 跑"),
                ("water", "n. 水"),
            ],
        )
    }

    fn zh_en() -> TempDict {
        TempDict::build(
            "zh-en",
            &[
                ("苹果", "apple"),
                ("香蕉", "banana"),
                ("水", "water"),
            ],
        )
    }

    fn config_with(rows: Vec<DictionaryConfig>) -> Config {
        let mut config = Config::default();
        config.dictionaries = rows;
        config
    }

    #[test]
    fn opens_enabled_and_reports_direction() {
        let dict = en_zh();
        let config = config_with(vec![dict.row("英汉")]);
        let registry = Registry::load(&config);

        assert_eq!(registry.rows().len(), 1);
        assert_eq!(registry.ready_count(), 1);

        let row = &registry.rows()[0];
        assert_eq!(row.label, "英汉");
        let Status::Ready {
            direction,
            confidence,
            low_confidence,
            entries,
        } = &row.status
        else {
            panic!("expected a ready row, got {:?}", row.status);
        };
        assert_eq!(*direction, Direction::EnZh);
        assert!(*confidence >= LOW_CONFIDENCE, "confidence was {confidence}");
        assert!(!*low_confidence);
        assert_eq!(*entries, 5);
    }

    #[test]
    fn disabled_rows_are_not_opened() {
        let dict = en_zh();
        let mut row = dict.row("英汉");
        row.enabled = false;
        let registry = Registry::load(&config_with(vec![row]));

        assert_eq!(registry.rows()[0].status, Status::Disabled);
        assert_eq!(registry.ready_count(), 0);
        assert!(registry.rows()[0].dict().is_none());
    }

    #[test]
    fn ascii_query_only_searches_en_dictionaries() {
        let en = en_zh();
        let zh = zh_en();
        let registry = Registry::load(&config_with(vec![en.row("英汉"), zh.row("汉英")]));

        let report = registry.lookup("apple");
        assert_eq!(report.script, QueryScript::Latin);
        assert_eq!(report.searched, 1, "only the en->zh dictionary should run");
        assert_eq!(report.filtered, 1);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].label, "英汉");
        assert_eq!(report.groups[0].entry.definition, "n. 苹果");
    }

    #[test]
    fn chinese_query_only_searches_zh_dictionaries() {
        let en = en_zh();
        let zh = zh_en();
        let registry = Registry::load(&config_with(vec![en.row("英汉"), zh.row("汉英")]));

        let report = registry.lookup("苹果");
        assert_eq!(report.script, QueryScript::Han);
        assert_eq!(report.searched, 1);
        assert_eq!(report.filtered, 1);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].label, "汉英");
        assert_eq!(report.groups[0].entry.definition, "apple");
    }

    #[test]
    fn groups_follow_configured_order() {
        let first = en_zh();
        let second = TempDict::build(
            "en-zh-2",
            &[("apple", "n. 苹果（第二本）"), ("pear", "n. 梨")],
        );
        let registry = Registry::load(&config_with(vec![first.row("第一本"), second.row("第二本")]));

        let report = registry.lookup("apple");
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].label, "第一本");
        assert_eq!(report.groups[1].label, "第二本");
        assert_eq!(report.groups[1].entry.definition, "n. 苹果（第二本）");
    }

    #[test]
    fn miss_lists_candidates_and_queried_dictionaries() {
        let en = en_zh();
        let registry = Registry::load(&config_with(vec![en.row("英汉")]));

        let report = registry.lookup("the zebras,");
        assert!(report.groups.is_empty());
        assert_eq!(report.missed, vec!["英汉".to_string()]);
        assert!(!report.candidates.is_empty());
        assert!(report.candidates.len() <= 3);
        assert_eq!(report.candidates[0], "the zebras,");
    }

    #[test]
    fn broken_paths_are_marked_and_do_not_break_other_rows() {
        let good = en_zh();
        let missing = DictionaryConfig::new(
            PathBuf::from("Z:\\nope\\missing.ifo").to_string_lossy(),
            "缺失",
        );
        let registry = Registry::load(&config_with(vec![missing, good.row("英汉")]));

        assert_eq!(registry.rows().len(), 2);
        assert!(matches!(registry.rows()[0].status, Status::Missing(_)));
        assert!(registry.rows()[0]
            .status
            .error()
            .is_some_and(|message| message.contains("找不到")));
        assert_eq!(registry.rows()[1].status.direction(), Some(Direction::EnZh));

        // Still queryable through the healthy row.
        let report = registry.lookup("apple");
        assert_eq!(report.groups.len(), 1);
    }

    #[test]
    fn ifo_without_dict_is_broken_not_a_panic() {
        let dict = en_zh();
        fs::remove_file(dict.stem.with_extension("dict")).expect("remove .dict");
        let registry = Registry::load(&config_with(vec![dict.row("英汉")]));

        let row = &registry.rows()[0];
        assert!(matches!(row.status, Status::Broken(_)));
        assert!(row.status.error().is_some_and(|e| e.contains("配套文件")));
        assert!(registry.lookup("apple").groups.is_empty());
    }

    #[test]
    fn compressed_dict_is_reported_as_unsupported() {
        let dict = en_zh();
        fs::remove_file(dict.stem.with_extension("dict")).expect("remove .dict");
        let mut dz = dict.stem.clone();
        dz.set_extension("dict.dz");
        fs::write(dz, b"not really compressed").expect("write .dz");

        let registry = Registry::load(&config_with(vec![dict.row("英汉")]));
        let row = &registry.rows()[0];
        assert!(
            row.status
                .error()
                .is_some_and(|message| message.contains(".dict.dz")),
            "unexpected status: {:?}",
            row.status
        );
    }

    #[test]
    fn low_confidence_auto_direction_is_flagged() {
        // 10 latin headwords, 7 Chinese + 3 latin definitions:
        // fingerprint says EnZh, but only with ~0.7 confidence.
        let entries: Vec<(&str, &str)> = vec![
            ("aaa", "中文一"),
            ("bbb", "中文二"),
            ("ccc", "中文三"),
            ("ddd", "中文四"),
            ("eee", "中文五"),
            ("fff", "中文六"),
            ("ggg", "中文七"),
            ("hhh", "english one"),
            ("iii", "english two"),
            ("jjj", "english three"),
        ];
        let dict = TempDict::build("mixed", &entries);
        let registry = Registry::load(&config_with(vec![dict.row("混合")]));

        let row = &registry.rows()[0];
        assert_eq!(row.status.direction(), Some(Direction::EnZh));
        assert!(row.status.low_confidence(), "0.7 confidence must be flagged");

        let report = registry.lookup("aaa");
        assert_eq!(report.groups.len(), 1);
        assert!(report.groups[0].low_confidence);
    }

    #[test]
    fn explicit_direction_overrides_the_fingerprint() {
        let dict = en_zh(); // fingerprint: EnZh
        let mut row = dict.row("强制汉英");
        row.direction = DirectionSpec::ZhEn;
        let registry = Registry::load(&config_with(vec![row]));

        assert_eq!(registry.rows()[0].status.direction(), Some(Direction::ZhEn));
        // A Chinese query now matches, an ASCII one does not.
        assert_eq!(registry.lookup("苹果").filtered, 0);
        assert_eq!(registry.lookup("apple").searched, 0);
    }

    #[test]
    fn empty_query_matches_nothing() {
        let dict = en_zh();
        let registry = Registry::load(&config_with(vec![dict.row("英汉")]));
        let report = registry.lookup("   ");
        assert!(report.groups.is_empty());
        assert_eq!(report.searched, 0);
        assert!(report.candidates.is_empty());
    }

    #[test]
    fn script_detection() {
        assert_eq!(query_script("apple"), QueryScript::Latin);
        assert_eq!(query_script("apple juice, 2"), QueryScript::Latin);
        assert_eq!(query_script("苹果"), QueryScript::Han);
        assert_eq!(query_script("苹果 apple"), QueryScript::Han);
        assert_eq!(query_script("привет"), QueryScript::Other);
        assert_eq!(query_script(""), QueryScript::Other);
        assert_eq!(query_script("..."), QueryScript::Other);
    }

    #[test]
    fn direction_filter_table() {
        use QueryScript::{Han, Latin, Other};
        for script in [Han, Latin, Other] {
            assert!(direction_matches(Direction::Unknown, script));
        }
        assert!(direction_matches(Direction::EnZh, Latin));
        assert!(direction_matches(Direction::EnEn, Latin));
        assert!(!direction_matches(Direction::EnZh, Han));
        assert!(!direction_matches(Direction::EnEn, Han));
        assert!(direction_matches(Direction::ZhEn, Han));
        assert!(direction_matches(Direction::ZhZh, Han));
        assert!(!direction_matches(Direction::ZhEn, Latin));
        assert!(!direction_matches(Direction::ZhZh, Latin));
        assert!(direction_matches(Direction::EnZh, Other));
        assert!(direction_matches(Direction::ZhEn, Other));
    }

    #[test]
    fn read_bookname_does_not_open_the_index() {
        let dict = en_zh();
        assert_eq!(read_bookname(dict.ifo()).as_deref(), Some("en-zh"));
        assert_eq!(read_bookname(dict.dir.join("nope.ifo")), None);
    }
}
