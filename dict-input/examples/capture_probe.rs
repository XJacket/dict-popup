//! Diagnostic: run the real capture pipeline (clipboard, then UIA) and print
//! what happened.
//!
//! ```text
//! cargo run --release --example capture_probe            # foreground window
//! cargo run --release --example capture_probe -- <hwnd>  # a specific window
//! ```
//!
//! With `DICT_PROBE_TYPE=<text>` it first types that text with
//! `KEYEVENTF_UNICODE`, which isolates "synthetic input never reaches the
//! desktop" from "the copy shortcut was ignored".

fn main() {
    if let Ok(text) = std::env::var("DICT_PROBE_TYPE") {
        let accepted = dict_input::debug_type_text(&text);
        println!("typed (unicode): accepted {accepted} input events");
    }

    let hwnd = std::env::args().nth(1).and_then(|a| a.parse::<isize>().ok());
    println!("--- target window ---");
    println!("{}", dict_input::debug_capture_once(hwnd));
}
