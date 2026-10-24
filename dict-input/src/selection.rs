//! Selected-text capture through UI Automation, plus the event types the UI
//! receives.

use std::fmt;

/// Text captured from the focused control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    /// The focused element exposes no text pattern (game, Java app, canvas).
    NoTextPattern,
    /// The element has a text pattern but nothing is selected.
    EmptySelection,
    /// UIA itself failed to initialise or the call timed out.
    Automation(String),
    /// Capture took longer than the budget; the provider is probably hung.
    TimedOut,
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectionError::NoTextPattern => write!(
                f,
                "当前程序不支持取词（需要能用鼠标选中文字的应用，例如浏览器、记事本、Office）"
            ),
            SelectionError::EmptySelection => write!(f, "没有选中文字"),
            SelectionError::Automation(m) => write!(f, "取词失败：{m}"),
            SelectionError::TimedOut => write!(f, "取词超时"),
        }
    }
}

impl std::error::Error for SelectionError {}
