//! Diagnostic: run the real capture pipeline (Pot, clipboard, `WM_COPY`, UIA)
//! and print the structured report.
//!
//! ```text
//! cargo run --release --example capture_probe            # foreground window
//! cargo run --release --example capture_probe -- <hwnd>  # a specific window
//! ```
//!
//! With `DICT_PROBE_TYPE=<text>` it first types that text with
//! `KEYEVENTF_UNICODE`, which isolates "synthetic input never reaches the
//! desktop" from "the copy shortcut was ignored".

use dict_input::capture::TrimSelection;
use dict_input::CaptureTarget;

fn main() {
    if let Ok(text) = std::env::var("DICT_PROBE_TYPE") {
        let accepted = dict_input::diagnostics::type_text(&text);
        println!("typed (unicode): accepted {accepted} input events");
    }

    let target = std::env::args()
        .nth(1)
        .and_then(|argument| argument.parse::<isize>().ok())
        .map(|hwnd| CaptureTarget::new(Some(hwnd), TrimSelection::Preserve));

    println!("--- capture diagnostics ---");
    println!("{}", dict_input::diagnostics::run(target));
}
