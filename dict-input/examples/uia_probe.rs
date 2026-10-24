//! Diagnostic: dump the UI Automation tree of a window and what each element
//! exposes (control type, class, text pattern, current selection).
//!
//! ```text
//! cargo run --release --example uia_probe -- <hwnd>
//! ```
//!
//! Get the handle of the window you care about with, e.g.:
//! `(Get-Process Notepad).MainWindowHandle`.

fn main() {
    let hwnd: isize = match std::env::args().nth(1).and_then(|a| a.parse().ok()) {
        Some(hwnd) => hwnd,
        None => {
            eprintln!("usage: uia_probe <hwnd>");
            std::process::exit(2);
        }
    };

    print!("{}", dict_input::debug_capture_report(hwnd));
}
