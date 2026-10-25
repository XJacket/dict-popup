//! dict-popup — offline StarDict lookup with a floating card.
//!
//! `docs/DESIGN.md` is the interface contract. This crate is the user-facing
//! layer: it owns the card, the settings window, configuration and the query
//! orchestration; `dict-core` reads dictionaries and `dict-input` owns Win32.
//!
//! Startup order matters: the input host must be started **before** eframe
//! creates any window, because `Host::start` sets
//! `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` and Windows rejects
//! that once a window exists.

// No console window in release builds; debug builds keep one for `cargo run`
// and test output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod dictionaries;
mod logging;
mod platform;
mod request;
mod theme;
mod ui;

use std::sync::{mpsc, Arc, Mutex};

use eframe::egui;

use dict_input::host::{Event, EventSink, Host, HostError, HostOptions};
use dict_input::Hotkey;

fn main() -> eframe::Result<()> {
    logging::init(config::log_path());
    install_panic_hook();
    logging::info(format!(
        "dict-popup {} starting",
        env!("CARGO_PKG_VERSION")
    ));

    // One informational line: UIAccess is opt-in and never required. Capture
    // keeps working without it; the diagnostic probe reports the same fields.
    let privilege = dict_input::privilege::current();
    logging::info(format!("privilege: {privilege}"));

    // ---- config (needed before the host, for the hotkey) ------------------
    let (config, config_path, config_error) = match config::Config::load() {
        Ok(config) => (config, config::config_path(), None),
        Err(err) => {
            let message = err.to_string();
            logging::error(format!("config: {message}"));
            (err.fallback, err.path, Some(message))
        }
    };

    // ---- input host, before any window exists -----------------------------
    let (event_sender, events) = mpsc::channel::<Event>();
    let waker: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    let sink_waker = Arc::clone(&waker);
    let sink: EventSink = Box::new(move |event: Event| {
        if event_sender.send(event).is_err() {
            return;
        }
        // Wake the UI thread once eframe has published its context.
        let guard = match sink_waker.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ctx) = guard.as_ref() {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    });

    // The config file is the only textual hotkey source: convert it once here.
    let (hotkey, hotkey_notice) = match dict_input::hotkey::parse(&config.hotkey) {
        Ok(hotkey) => (hotkey, None),
        Err(err) => {
            // An unreadable hotkey must not leave the user without a working
            // hotkey at all: fall back to the shipped default and say so.
            logging::error(format!("hotkey: {err}"));
            let hotkey = Hotkey::default();
            let notice = format!(
                "配置里的热键「{}」无效（{err}），本次使用默认的 {}，请在设置里重新捕获",
                config.hotkey,
                hotkey.canonical()
            );
            (hotkey, Some(notice))
        }
    };

    let tooltip = "dict-popup".to_string();
    let trim_selection = config.behavior.trim_selection;
    let (host, host_error) = match Host::start(
        HostOptions {
            hotkey: Ok(hotkey),
            tooltip,
            trim_selection,
        },
        sink,
    ) {
        Ok(host) => (Some(host), None),
        Err(HostError::AlreadyRunning) => {
            // The first instance received the "show settings" message.
            logging::info("another instance is already running; exiting");
            return Ok(());
        }
        Err(err) => {
            logging::error(format!("host: {err}"));
            (None, Some(err.to_string()))
        }
    };

    let startup = app::Startup {
        config,
        hotkey,
        config_path,
        config_error,
        privilege,
        host,
        host_error,
        hotkey_notice,
        events,
        demo: demo_query(),
    };

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        // The root window is an invisible keep-alive surface: eframe only runs
        // `App::ui` while the root (or a child viewport) is visible, and the app
        // must be able to open the card from an idle tray state.
        viewport: egui::ViewportBuilder::default()
            .with_title("dict-popup")
            .with_inner_size([app::ROOT_SIZE, app::ROOT_SIZE])
            .with_position(app::ROOT_POS)
            .with_decorations(false)
            .with_transparent(true)
            .with_taskbar(false)
            .with_resizable(false)
            .with_mouse_passthrough(true)
            .with_window_level(egui::WindowLevel::AlwaysOnBottom),
        persist_window: false,
        ..Default::default()
    };

    let result = eframe::run_native(
        "dict-popup",
        options,
        Box::new(move |cc| {
            publish_context(&waker, &cc.egui_ctx);
            Ok(Box::new(app::App::new(cc, startup)) as Box<dyn eframe::App>)
        }),
    );

    if let Err(err) = &result {
        logging::error(format!("eframe failed: {err}"));
        // Startup failure must be visible even without a console (DESIGN §9.5).
        platform::message_box("dict-popup 启动失败", &format!("无法创建窗口：{err}"));
    }

    result
}

/// `--demo <word>` (or `--demo=<word>`): render the card for a word without the
/// global hotkey or UIA capture. Used by `scripts/e2e-smoke.ps1` and for manual
/// visual checks; it never writes config.
fn demo_query() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--demo" {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix("--demo=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn publish_context(waker: &Arc<Mutex<Option<egui::Context>>>, ctx: &egui::Context) {    let mut guard = match waker.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Some(ctx.clone());
}

/// Panics are bugs, but a user-facing tool must not vanish silently: log it,
/// tell the user, and abort rather than continue in a corrupted state.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = info.to_string();
        logging::error(format!("panic: {message}"));
        platform::message_box(
            "dict-popup 发生错误",
            &format!("程序遇到未处理的错误，将退出：\n\n{message}"),
        );
        std::process::abort();
    }));
}
