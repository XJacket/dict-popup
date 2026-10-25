//! Why a capture did not produce text.
//!
//! This is the **only** place in the crate that turns a capture problem into a
//! user-facing sentence. Strategy code returns a variant; the UI calls
//! [`Display`](std::fmt::Display). No strategy builds Chinese prose at the throw
//! site, and there is no catch-all `Automation(String)` on the happy path.

use std::fmt;

/// A capture attempt that produced no usable text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
    /// The focused control has a text surface but nothing is selected (or the
    /// text that came back was whitespace only / the clipboard did not change).
    NoSelection,
    /// The focused control exposes no text pattern at all (a game, a canvas, a
    /// Java UI, some Electron windows).
    NoTextPattern,
    /// Synthetic input could not be delivered (`SendInput` was rejected).
    InputBlocked,
    /// Another process kept the clipboard open for the whole retry window.
    ClipboardBusy,
    /// A strategy ran out of its budget without producing text, or the whole
    /// request exceeded [`super::pipeline::CAPTURE_TIMEOUT`].
    TimedOut,
    /// A capture request is already in flight; the new hotkey was dropped.
    Busy,
    /// Something genuinely unexpected (COM/UIA/thread start-up failure). The
    /// payload is technical detail, not user-facing prose.
    Platform(String),
}

impl CaptureFailure {
    /// Pick the more informative of two failed strategies.
    ///
    /// The last strategy is not automatically the best explanation: UI
    /// Automation knows "there is no text surface at all", while the clipboard
    /// only knows "nothing usable appeared". Prefer the more specific failure so
    /// the user sees "没有选中文字" only when that is really the best answer.
    pub(crate) fn prefer(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }

    fn rank(&self) -> u8 {
        match self {
            CaptureFailure::Platform(_) => 6,
            CaptureFailure::InputBlocked => 5,
            CaptureFailure::ClipboardBusy => 4,
            CaptureFailure::NoTextPattern => 3,
            CaptureFailure::TimedOut => 2,
            CaptureFailure::NoSelection => 1,
            CaptureFailure::Busy => 0,
        }
    }
}

impl fmt::Display for CaptureFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureFailure::NoSelection => write!(f, "没有选中文字"),
            CaptureFailure::NoTextPattern => write!(
                f,
                "当前程序不支持取词（需要能用鼠标选中文字的应用，例如浏览器、记事本、Office）"
            ),
            CaptureFailure::InputBlocked => {
                write!(f, "无法模拟 Ctrl+C（目标程序不接受模拟按键）")
            }
            CaptureFailure::ClipboardBusy => {
                write!(f, "剪贴板正被其他程序占用，请稍后再试")
            }
            CaptureFailure::TimedOut => write!(f, "取词超时"),
            CaptureFailure::Busy => write!(f, "上一次取词还没有结束，请稍后再试"),
            CaptureFailure::Platform(detail) => write!(f, "取词失败：{detail}"),
        }
    }
}

impl std::error::Error for CaptureFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_keeps_the_existing_user_wording() {
        assert_eq!(CaptureFailure::NoSelection.to_string(), "没有选中文字");
        assert_eq!(CaptureFailure::TimedOut.to_string(), "取词超时");
        assert!(CaptureFailure::NoTextPattern
            .to_string()
            .contains("当前程序不支持取词"));
        assert!(CaptureFailure::Platform("boom".to_string())
            .to_string()
            .contains("取词失败：boom"));
    }

    #[test]
    fn prefer_keeps_the_more_specific_failure() {
        assert_eq!(
            CaptureFailure::NoSelection.prefer(CaptureFailure::NoTextPattern),
            CaptureFailure::NoTextPattern
        );
        assert_eq!(
            CaptureFailure::TimedOut.prefer(CaptureFailure::NoSelection),
            CaptureFailure::TimedOut
        );
        assert_eq!(
            CaptureFailure::NoTextPattern.prefer(CaptureFailure::InputBlocked),
            CaptureFailure::InputBlocked
        );
    }
}
