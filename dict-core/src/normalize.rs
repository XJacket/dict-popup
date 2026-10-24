//! Query normalisation and the fallback chain.
//!
//! Selected text arrives with punctuation, articles and inflections attached
//! ("the apple," "apples"). The chain is deliberately shallow and predictable:
//! each step is tried once, in order, and the winning step is reported so the
//! UI can explain why a result appeared.

/// One attempt in the fallback chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub term: String,
    pub reason: &'static str,
}

impl Candidate {
    fn new(term: impl Into<String>, reason: &'static str) -> Self {
        Self {
            term: term.into(),
            reason,
        }
    }
}

const ARTICLES: [&str; 3] = ["a ", "an ", "the "];

/// Strip surrounding whitespace and common edge punctuation.
pub fn strip_edges(input: &str) -> String {
    let trimmed = input.trim();
    let trimmed = trimmed.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '“' | '”' | '‘' | '’' | '(' | ')' | '[' | ']' | '{' | '}'
                | ',' | '.' | ';' | ':' | '!' | '?' | '。' | '，' | '；' | '：'
                | '！' | '？' | '、' | '…'
        )
    });
    collapse_ws(trimmed)
}

/// Collapse runs of whitespace (including newlines) into single spaces.
pub fn collapse_ws(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_space = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

fn strip_article(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    for article in ARTICLES {
        if lower.starts_with(article) && input.len() > article.len() {
            let rest = &input[article.len()..];
            if !rest.trim().is_empty() {
                return Some(collapse_ws(rest));
            }
        }
    }
    None
}

/// Light inflection reduction. Not a full lemmatiser - just the shapes that
/// actually show up in selected text.
fn deinflect(input: &str) -> Vec<String> {
    let lower = input.to_ascii_lowercase();
    let mut out = Vec::new();
    let base = lower.as_str();

    if let Some(stem) = base.strip_suffix("ies") {
        if !stem.is_empty() {
            out.push(format!("{stem}y"));
        }
    }
    for suffix in ["es", "s"] {
        if let Some(stem) = base.strip_suffix(suffix) {
            if stem.len() >= 2 {
                out.push(stem.to_string());
            }
        }
    }
    if let Some(stem) = base.strip_suffix("ing") {
        if stem.len() >= 2 {
            out.push(stem.to_string());
            out.push(format!("{stem}e"));
        }
    }
    if let Some(stem) = base.strip_suffix("ed") {
        if stem.len() >= 2 {
            out.push(stem.to_string());
            out.push(format!("{stem}e"));
        }
    }
    if let Some(stem) = base.strip_suffix('s') {
        if stem.len() >= 2 && !lower.ends_with("ss") {
            out.push(stem.to_string());
        }
    }

    out.retain(|c| c.len() >= 2 && c.as_str() != base);
    out.sort();
    out.dedup();
    out
}

/// Build the ordered list of things to try for `input`.
pub fn candidates(input: &str) -> Vec<Candidate> {
    let raw = collapse_ws(input.trim());
    if raw.is_empty() {
        return Vec::new();
    }

    let mut out = vec![Candidate::new(raw.clone(), "exact")];

    let stripped = strip_edges(&raw);
    if !stripped.is_empty() && stripped != raw {
        out.push(Candidate::new(stripped.clone(), "去首尾标点"));
    }

    // Lowercase variant (dictionary headwords are mostly lowercase).
    let lower = stripped.to_ascii_lowercase();
    if lower != stripped {
        out.push(Candidate::new(lower, "忽略大小写"));
    }

    let base_for_more = stripped;

    if let Some(no_article) = strip_article(&base_for_more) {
        out.push(Candidate::new(no_article.clone(), "去冠词"));
        if let Some(no_article_2) = strip_article(&no_article) {
            out.push(Candidate::new(no_article_2, "去冠词"));
        }
    }

    let article_target = strip_article(&base_for_more).unwrap_or_else(|| base_for_more.clone());
    for reduced in deinflect(&article_target) {
        out.push(Candidate::new(reduced, "词形还原"));
    }

    // Deduplicate, keeping the first (earliest) reason.
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.term.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_edges_and_collapses_space() {
        assert_eq!(strip_edges("  \"the apple,  \n"), "the apple");
        assert_eq!(strip_edges("apple..."), "apple");
        assert_eq!(strip_edges("「中文」"), "「中文」");
    }

    #[test]
    fn article_removal() {
        let c = candidates("the apple");
        assert!(c.iter().any(|c| c.term == "apple" && c.reason == "去冠词"));
    }

    #[test]
    fn plural_and_ing() {
        let c = candidates("apples");
        assert!(c.iter().any(|c| c.term == "apple" && c.reason == "词形还原"));
        let c = candidates("running");
        assert!(c.iter().any(|c| c.term == "runn" || c.term == "runne"));
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(candidates("   ").is_empty());
    }

    #[test]
    fn deduplicates_terms() {
        let c = candidates("apple");
        let mut terms: Vec<_> = c.iter().map(|c| c.term.as_str()).collect();
        let before = terms.len();
        terms.sort();
        terms.dedup();
        assert_eq!(before, terms.len());
    }
}
