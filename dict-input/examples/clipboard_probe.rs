//! Diagnostic: prove the clipboard read/write helpers round-trip, then put the
//! user's clipboard back.
//!
//! ```text
//! cargo run --release --example clipboard_probe
//! ```

fn main() {
    let before = dict_input::debug_clipboard_read();
    println!("before: {:?}", before);

    dict_input::debug_clipboard_write("dict-popup clipboard probe");
    let after = dict_input::debug_clipboard_read();
    println!("after:  {:?}", after);

    if let Some(previous) = &before {
        dict_input::debug_clipboard_write(previous);
        println!("restored: {:?}", dict_input::debug_clipboard_read());
    }

    let ok = after.as_deref() == Some("dict-popup clipboard probe");
    println!("round-trip: {}", if ok { "OK" } else { "FAILED" });
    std::process::exit(if ok { 0 } else { 1 });
}
