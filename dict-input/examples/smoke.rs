//! Integration smoke test for the Win32 host.
//!
//! Starts the real host (hidden window, tray icon, hotkey), exercises the
//! command channel, checks single-instance behaviour **across processes**, and
//! shuts down. Run with: cargo run --release --example smoke
//!
//! It puts a tray icon on the desktop for about a second and registers
//! Ctrl+Alt+F13; it never touches the clipboard or other applications.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use dict_input::{hotkey, Command, Event, Host, HostError, HostOptions};

fn options(tooltip: &str) -> HostOptions {
    HostOptions {
        hotkey: hotkey::parse("Ctrl+Alt+F13"),
        tooltip: tooltip.to_string(),
        trim_selection: true,
    }
}

fn main() {
    // Child mode: expect the mutex to be owned by the parent.
    if std::env::args().any(|arg| arg == "--child") {
        match Host::start(options("child"), Box::new(|_| {})) {
            Err(HostError::AlreadyRunning) => {
                println!("CHILD AlreadyRunning");
                std::process::exit(0);
            }
            Ok(_) => {
                println!("CHILD STARTED (single instance NOT enforced)");
                std::process::exit(2);
            }
            Err(error) => {
                println!("CHILD ERROR {error}");
                std::process::exit(3);
            }
        }
    }

    println!("[1] starting host");
    let (events_tx, events_rx) = mpsc::channel::<Event>();
    let started = Instant::now();
    let host = match Host::start(
        options("dict-popup smoke"),
        Box::new(move |event| {
            let _ = events_tx.send(event);
        }),
    ) {
        Ok(host) => host,
        Err(error) => {
            println!("    start failed: {error}");
            std::process::exit(1);
        }
    };
    println!("    started in {:?}", started.elapsed());

    println!("[2] second instance in a separate process");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .arg("--child")
        .output()
        .expect("spawn child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or("<no output>");
    println!("    child said: {line} (exit {:?})", output.status.code());
    if !line.contains("AlreadyRunning") {
        println!("    FAIL: single instance is not enforced across processes");
        host.shutdown();
        std::process::exit(1);
    }

    println!("[3] updating tooltip + hotkey");
    host.send(Command::SetTooltip("dict-popup smoke (2)".to_string()));
    host.send(Command::SetHotkey(hotkey::parse("Ctrl+Alt+F13").ok()));
    host.send(Command::Notify {
        title: "dict-popup".to_string(),
        body: "smoke test".to_string(),
    });

    println!("[4] waiting for events");
    let deadline = Instant::now() + Duration::from_millis(700);
    let mut seen = 0usize;
    while Instant::now() < deadline {
        if let Ok(event) = events_rx.recv_timeout(Duration::from_millis(150)) {
            seen += 1;
            println!("    event: {event:?}");
        }
    }

    println!("[5] shutting down");
    host.shutdown();
    println!("    OK; events seen: {seen}");
}
