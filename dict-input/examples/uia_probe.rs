//! Diagnostic: run the real capture pipeline against one window and print the
//! structured report, including the bounded UI Automation tree sample when UIA
//! is the strategy that fails.
//!
//! ```text
//! cargo run --release --example uia_probe -- <hwnd>
//! ```
//!
//! Get the handle of the window you care about with, e.g.:
//! `(Get-Process Notepad).MainWindowHandle`.
//!
//! The tree sample comes from the *same* UI Automation object the capture used.
//! It is intentionally bounded (depth and node count), so it fits in one log
//! line group instead of dumping the whole provider tree.

use dict_input::capture::TrimSelection;
use dict_input::CaptureTarget;

fn main() {
    let hwnd: isize = match std::env::args().nth(1).and_then(|argument| argument.parse().ok()) {
        Some(hwnd) => hwnd,
        None => {
            eprintln!("usage: uia_probe <hwnd>");
            std::process::exit(2);
        }
    };

    println!(
        "{}",
        dict_input::diagnostics::run(Some(CaptureTarget::new(
            Some(hwnd),
            TrimSelection::Preserve
        )))
    );
}
