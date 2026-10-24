//! Direction fingerprint: infer what language pair a dictionary serves by
//! sampling both its headwords and its definitions.
//!
//! Judging only the headwords is not enough - it cannot tell an EN->ZH
//! dictionary from a ZH->EN one that happens to store English glosses. Looking
//! at both sides gives a direction plus a confidence, and lets the caller
//! decide to ask the user instead of guessing.

use crate::stardict::Dictionary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Headwords latin, definitions Chinese.
    EnZh,
    /// Headwords Chinese, definitions latin.
    ZhEn,
    /// Chinese on both sides (a Chinese-Chinese dictionary).
    ZhZh,
    /// Both latin (an English-English dictionary).
    EnEn,
    /// No confident call.
    Unknown,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::EnZh => "英→中",
            Direction::ZhEn => "中→英",
            Direction::ZhZh => "中→中",
            Direction::EnEn => "英→英",
            Direction::Unknown => "未知",
        }
    }

    /// The language of the *query* this dictionary answers, if known.
    pub fn query_is_chinese(self) -> Option<bool> {
        match self {
            Direction::EnZh | Direction::EnEn => Some(false),
            Direction::ZhEn | Direction::ZhZh => Some(true),
            Direction::Unknown => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub direction: Direction,
    /// 0..=1; below ~0.85 the UI should ask the user to confirm.
    pub confidence: f32,
    pub headword_samples: usize,
    pub definition_samples: usize,
    pub headword_han_ratio: f32,
    pub definition_han_ratio: f32,
}

const HEADWORD_SAMPLES: usize = 512;
const DEFINITION_SAMPLES: usize = 64;

pub fn fingerprint(dict: &Dictionary) -> Fingerprint {
    let count = dict.entry_count();
    if count == 0 {
        return Fingerprint {
            direction: Direction::Unknown,
            confidence: 0.0,
            headword_samples: 0,
            definition_samples: 0,
            headword_han_ratio: 0.0,
            definition_han_ratio: 0.0,
        };
    }

    let stride = (count / HEADWORD_SAMPLES).max(1);
    let mut head_total = 0usize;
    let mut head_han = 0usize;
    let mut i = 0usize;
    while i < count && head_total < HEADWORD_SAMPLES {
        if let Some(word) = dict.headword_at(i) {
            head_total += 1;
            if contains_han_bytes(word) {
                head_han += 1;
            }
        }
        i += stride;
    }

    let stride_def = (count / DEFINITION_SAMPLES).max(1);
    let mut def_total = 0usize;
    let mut def_han = 0usize;
    let mut j = 0usize;
    while j < count && def_total < DEFINITION_SAMPLES {
        if let Some((off, size)) = dict.definition_range_at(j) {
            if let Some(text) = dict.definition_at(off, size) {
                def_total += 1;
                if text.chars().any(is_han) {
                    def_han += 1;
                }
            }
        }
        j += stride_def;
    }

    let headword_han_ratio = ratio(head_han, head_total);
    let definition_han_ratio = ratio(def_han, def_total);

    let (direction, confidence) = classify(headword_han_ratio, definition_han_ratio);

    Fingerprint {
        direction,
        confidence,
        headword_samples: head_total,
        definition_samples: def_total,
        headword_han_ratio,
        definition_han_ratio,
    }
}

fn ratio(n: usize, total: usize) -> f32 {
    if total == 0 {
        0.0
    } else {
        n as f32 / total as f32
    }
}

fn classify(head_han: f32, def_han: f32) -> (Direction, f32) {
    const HI: f32 = 0.60;
    const LO: f32 = 0.40;

    match (head_han >= HI, def_han >= HI, head_han <= LO, def_han <= LO) {
        // Chinese headwords, latin definitions.
        (true, false, _, true) => (Direction::ZhEn, head_han.min(1.0 - def_han)),
        // Latin headwords, Chinese definitions.
        (false, true, true, _) => (Direction::EnZh, (1.0 - head_han).min(def_han)),
        // Chinese on both sides.
        (true, true, _, _) => (Direction::ZhZh, head_han.min(def_han)),
        // Latin on both sides.
        (false, false, true, true) => (Direction::EnEn, (1.0 - head_han).min(1.0 - def_han)),
        // Anything in between is a judgement call we refuse to make.
        _ => (Direction::Unknown, 0.0),
    }
}

fn contains_han_bytes(bytes: &[u8]) -> bool {
    // Headwords are UTF-8; decode lossily so a stray byte cannot panic.
    String::from_utf8_lossy(bytes).chars().any(is_han)
}

fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF |     // CJK Extension A
        0x4E00..=0x9FFF |     // CJK Unified Ideographs
        0xF900..=0xFAFF |     // Compatibility Ideographs
        0x20000..=0x2FA1F)    // Extensions B-F
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_covers_the_four_pairs() {
        assert_eq!(classify(0.0, 1.0).0, Direction::EnZh);
        assert_eq!(classify(1.0, 0.0).0, Direction::ZhEn);
        assert_eq!(classify(0.9, 0.9).0, Direction::ZhZh);
        assert_eq!(classify(0.1, 0.1).0, Direction::EnEn);
        assert_eq!(classify(0.5, 0.5).0, Direction::Unknown);
    }

    #[test]
    fn confidence_is_bounded() {
        for (h, d) in [(0.0, 1.0), (1.0, 0.0), (0.1, 0.1), (0.95, 0.95)] {
            let (_, c) = classify(h, d);
            assert!((0.0..=1.0).contains(&c), "confidence out of range: {c}");
        }
    }

    #[test]
    fn direction_reports_query_language() {
        assert_eq!(Direction::EnZh.query_is_chinese(), Some(false));
        assert_eq!(Direction::ZhEn.query_is_chinese(), Some(true));
        assert_eq!(Direction::EnEn.query_is_chinese(), Some(false));
        assert_eq!(Direction::ZhZh.query_is_chinese(), Some(true));
        assert_eq!(Direction::Unknown.query_is_chinese(), None);
    }
}
