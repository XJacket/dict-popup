//! Application state, the host-event pump, and the three native windows.
//!
//! Window layout (eframe/egui 0.37 viewports):
//!
//! - **root** — a 1×1, transparent, off-screen keep-alive window. eframe only
//!   runs `App::ui` while the root (or one of its descendants) is visible, so
//!   this invisible window is what lets the app open the card from an idle
//!   tray state. It paints nothing.
//! - **card** — deferred viewport `ui::card`, frameless / always-on-top /
//!   translucent; hidden rather than destroyed when unused so the next hotkey
//!   does not pay for window creation.
//! - **settings** — deferred viewport `ui::settings`, an ordinary window.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{ViewportBuilder, ViewportCommand, ViewportId};

use dict_input::host::{Command, Event, Host};
use dict_input::TrayAction;

use crate::config::Config;
use crate::dictionaries::Registry;
use crate::logging;
use crate::theme::{self, FontInstall};
use crate::ui::card::{self, CardState};
use crate::ui::settings::{self, SettingsState};

/// How long the settings autosave waits after the last edit.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// The invisible keep-alive window.
pub const ROOT_SIZE: f32 = 1.0;
/// Where the keep-alive window lives (off every screen).
pub const ROOT_POS: [f32; 2] = [-30_000.0, -30_000.0];

pub fn card_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("dict-popup-card")
}

pub fn settings_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("dict-popup-settings")
}

// ---------------------------------------------------------------------------
// Notices: every error must be visible somewhere (DESIGN §9.6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub level: NoticeLevel,
    pub text: String,
}

// ---------------------------------------------------------------------------
// Shared state (touched from the root, card and settings passes)
// ---------------------------------------------------------------------------

pub struct Shared {
    pub ctx: egui::Context,
    pub config: Config,
    pub config_path: PathBuf,
    pub config_error: Option<String>,
    pub font: FontInstall,
    pub notices: Vec<Notice>,
    pub registry: Registry,
    pub registry_loading: bool,
    loader: Option<Receiver<Registry>>,
    pub card: CardState,
    pub settings: SettingsState,
    pub host: Option<Host>,
    pub host_error: Option<String>,
    /// A hotkey fired and the UIA capture is still in flight. The card is shown
    /// only when the result arrives (see `on_hotkey`), or by the watchdog.
    pending_hotkey: Option<Instant>,
    /// The user dismissed the card while a capture was still running; its
    /// result must not reopen the card (the host cannot cancel a capture).
    ignore_result: bool,
}

impl Shared {
    pub fn notify(&mut self, level: NoticeLevel, text: impl Into<String>) {
        let text = text.into();
        if self.notices.iter().any(|notice| notice.text == text) {
            return;
        }
        self.notices.push(Notice { level, text });
        if self.notices.len() > 8 {
            self.notices.remove(0);
        }
    }

    /// Wake the root pass, and repaint the viewport we are currently in.
    pub fn wake(&self) {
        self.ctx.request_repaint_of(ViewportId::ROOT);
        self.ctx.request_repaint();
    }

    pub fn mark_settings_dirty(&mut self) {
        self.settings.dirty_since = Some(Instant::now());
        self.wake();
    }

    pub fn open_settings(&mut self) {
        if !self.settings.open {
            self.settings.draft = self.config.clone();
            self.settings.capture = settings::HotkeyCapture::default();
            self.settings.dirty_since = None;
        }
        self.settings.open = true;
        // Bumped on every request (tray double-click, second launch, startup) so
        // the window is brought to the front even when it is already open.
        self.settings.generation = self.settings.generation.wrapping_add(1);
        self.wake();
    }

    pub fn quit(&mut self) {
        if let Some(host) = &self.host {
            host.send(Command::Shutdown);
        }
        self.ctx
            .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    pub fn set_tooltip(&self) {
        if let Some(host) = &self.host {
            host.send(Command::SetTooltip(format!(
                "dict-popup · {} 本词典",
                self.registry.ready_count()
            )));
        }
    }

    // ---- host events ------------------------------------------------------

    pub fn handle_event(&mut self, event: Event) {
        match event {
            Event::Hotkey => self.on_hotkey(),
            Event::Selection(selection) => self.on_selection(selection.text),
            Event::SelectionFailed(err) => {
                self.on_selection_failed(err.to_string());
            }
            Event::HotkeyFailed(message) => {
                // The old hotkey (if any) is still active; surface the reason in
                // the settings window and as a tray balloon.
                logging::warn(format!("hotkey failed: {message}"));
                self.notify(NoticeLevel::Error, message.clone());
                if let Some(host) = &self.host {
                    host.send(Command::Notify {
                        title: "热键注册失败".to_string(),
                        body: message,
                    });
                }
                self.wake();
            }
            Event::Tray(action) => match action {
                TrayAction::OpenSettings => self.open_settings(),
                TrayAction::ReloadDictionaries => {
                    self.reload_registry();
                    self.wake();
                }
                TrayAction::Quit => self.quit(),
            },
            Event::SecondLaunch => self.open_settings(),
        }
    }

    fn on_hotkey(&mut self) {
        // While the card is up it holds focus, so UIA could only read our own
        // window. Treat a second press as "dismiss" (the footer says the same).
        if self.card.visible {
            self.card.visible = false;
            // The host captures on every hotkey and cannot cancel it, so this
            // press always has a result in flight; drop it so the card does not
            // immediately reopen.
            self.pending_hotkey = None;
            self.ignore_result = true;
            logging::info("hotkey (dismiss)");
            self.wake();
            return;
        }

        // Do NOT show the card yet. A visible card takes focus, and the UIA
        // worker runs ~10-200 ms later: it would then read *our* window instead
        // of the user's, which is exactly the "当前程序不支持取词" report.
        // The card is shown when the selection (or the failure) arrives, with a
        // watchdog in `logic` that shows "查询中" if capture is slow.
        self.pending_hotkey = Some(Instant::now());
        logging::info("hotkey");
        self.wake();
    }

    /// Show the card for this pass: position it, then reveal `phase`.
    fn show_card(&mut self, phase: impl FnOnce(&mut CardState)) {
        self.pending_hotkey = None;
        let ui_config = self.config.ui.clone();
        self.card.begin_query(&ui_config);
        phase(&mut self.card);
        self.wake();
    }

    fn on_selection_failed(&mut self, message: String) {
        logging::warn(format!("selection failed: {message}"));
        if self.ignore_result {
            self.ignore_result = false;
            self.pending_hotkey = None;
            return;
        }
        self.show_card(|card| card.show_error(message));
    }

    fn on_selection(&mut self, text: String) {
        if self.ignore_result {
            self.ignore_result = false;
            return;
        }
        self.pending_hotkey = None;
        let ui_config = self.config.ui.clone();
        self.card.begin_query(&ui_config);

        let query = if self.config.behavior.trim_selection {
            text.trim().to_string()
        } else {
            text
        };

        if query.trim().is_empty() {
            self.card.show_error("没有选中文字");
            self.wake();
            return;
        }

        // Never log the text, only its length (DESIGN §9.2).
        logging::info(format!("selection chars={}", query.chars().count()));

        if self.registry_loading && self.registry.ready_count() == 0 {
            self.card.pending = Some(query);
            self.card.show_info("词典加载中，请稍候…");
            self.wake();
            return;
        }

        if self.registry.ready_count() == 0 {
            let message = if self.registry.rows().is_empty() {
                "还没有配置词典，请打开设置添加 .ifo 文件"
            } else {
                "没有可用的词典，请检查设置中的词典路径"
            };
            self.card.show_info(message);
            self.wake();
            return;
        }

        self.run_lookup(&query);
    }

    fn run_lookup(&mut self, query: &str) {
        let started = Instant::now();
        let report = self.registry.lookup(query);
        logging::info(format!(
            "lookup chars={} searched={} hits={} filtered={} us={}",
            query.chars().count(),
            report.searched,
            report.hits(),
            report.filtered,
            started.elapsed().as_micros()
        ));
        self.card.show_report(report);
        self.wake();
    }

    // ---- dictionaries -----------------------------------------------------

    pub fn reload_registry(&mut self) {
        if self.loader.is_some() {
            return;
        }

        let config = self.config.clone();
        let ctx = self.ctx.clone();
        let (sender, receiver) = mpsc::channel();

        let spawned = std::thread::Builder::new()
            .name("dict-registry".to_string())
            .spawn(move || {
                let registry = Registry::load(&config);
                if sender.send(registry).is_ok() {
                    ctx.request_repaint_of(ViewportId::ROOT);
                }
            });

        match spawned {
            Ok(_handle) => {
                self.loader = Some(receiver);
                self.registry_loading = true;
            }
            Err(err) => {
                self.registry_loading = false;
                self.notify(
                    NoticeLevel::Error,
                    format!("无法启动词典加载线程：{err}"),
                );
            }
        }
    }

    pub fn poll_registry(&mut self) {
        let Some(receiver) = self.loader.as_ref() else {
            return;
        };

        match receiver.try_recv() {
            Ok(registry) => {
                self.loader = None;
                self.registry = registry;
                self.registry_loading = false;

                let ready = self.registry.ready_count();
                let total = self.registry.rows().len();
                self.set_tooltip();
                if total > 0 && ready == 0 {
                    self.notify(
                        NoticeLevel::Error,
                        "没有可用的词典，请在设置中检查词典路径",
                    );
                } else {
                    self.notify(
                        NoticeLevel::Info,
                        format!("已加载 {ready}/{total} 本词典"),
                    );
                }

                if let Some(query) = self.card.pending.take() {
                    self.run_lookup(&query);
                }
                self.wake();
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.loader = None;
                self.registry_loading = false;
                self.notify(NoticeLevel::Error, "词典加载线程异常退出");
                self.wake();
            }
        }
    }

    // ---- config -----------------------------------------------------------

    /// Debounced autosave: "保存即生效" (DESIGN §6) without writing on every
    /// slider frame.
    pub fn maybe_commit_config(&mut self, now: Instant) {
        if self.settings.capture.active {
            return;
        }
        let Some(since) = self.settings.dirty_since else {
            return;
        };
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= AUTOSAVE_DEBOUNCE {
            self.commit_config();
        } else {
            self.ctx.request_repaint_after(AUTOSAVE_DEBOUNCE - elapsed);
        }
    }

    pub fn commit_config(&mut self) {
        self.settings.dirty_since = None;

        let mut new = self.settings.draft.clone();
        new.sanitize();

        let theme_changed = new.ui.theme != self.config.ui.theme
            || (new.ui.font_size - self.config.ui.font_size).abs() > f32::EPSILON;
        let hotkey_changed = new.hotkey != self.config.hotkey;
        let autostart_changed = new.behavior.autostart != self.config.behavior.autostart;
        let dictionaries_changed = new.dictionaries != self.config.dictionaries;

        self.config = new;

        // Side effects first: `apply_autostart` may revert the flag when Windows
        // refuses the registry write, and the file must reflect the real state.
        if theme_changed {
            theme::apply(&self.ctx, self.config.ui.theme, self.config.ui.font_size);
        }
        if hotkey_changed {
            self.apply_hotkey();
        }
        if autostart_changed {
            self.apply_autostart();
        }
        if dictionaries_changed {
            self.reload_registry();
        }

        if let Err(err) = self.config.save_to(&self.config_path) {
            self.notify(NoticeLevel::Error, format!("保存配置失败：{err}"));
        } else {
            self.settings.saved_at = Some(Instant::now());
        }

        self.settings.draft = self.config.clone();
        self.set_tooltip();
        self.wake();
    }

    fn apply_hotkey(&mut self) {
        match dict_input::hotkey::parse(&self.config.hotkey) {
            Ok(hotkey) => {
                if let Some(host) = &self.host {
                    host.send(Command::SetHotkey(Some(hotkey)));
                }
                logging::info(format!("hotkey registered: {}", hotkey));
            }
            Err(err) => {
                self.notify(
                    NoticeLevel::Error,
                    format!("热键「{}」无效：{err}", self.config.hotkey),
                );
            }
        }
    }

    fn apply_autostart(&mut self) {
        match dict_input::autostart::set_enabled(self.config.behavior.autostart) {
            Ok(()) => {
                logging::info(format!(
                    "autostart={}",
                    self.config.behavior.autostart
                ));
            }
            Err(err) => {
                // The registry value did not change: flip the checkbox back so
                // the UI cannot lie to the user.
                self.config.behavior.autostart = !self.config.behavior.autostart;
                self.notify(
                    NoticeLevel::Error,
                    format!("开机自启设置失败：{err}"),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    shared: Arc<Mutex<Shared>>,
    events: Receiver<Event>,
    /// `--demo <word>`: look the word up and show the card without the hotkey
    /// or UIA capture. Used by `scripts/e2e-smoke.ps1` and manual visual checks.
    demo: Option<String>,
    /// Working-set trimming bookkeeping (see `logic`).
    card_was_visible: bool,
    /// Last visibility we explicitly pushed to each child viewport.
    ///
    /// `ViewportBuilder::with_visible` is applied **only when the viewport is
    /// created**. The card viewport is created hidden at startup, so a later
    /// `visible = true` in the builder does nothing: the transition has to be
    /// sent as `ViewportCommand::Visible`. Without this the card never appears
    /// after the first hotkey (the window exists, 500x202, but is invisible).
    card_visible_sent: bool,
    settings_visible_sent: bool,
}

/// Everything `main` prepared before eframe existed: the config (loaded early so
/// the hotkey can be registered before the DPI context is locked) and the
/// already-running input host.
pub struct Startup {
    pub config: Config,
    pub config_path: PathBuf,
    pub config_error: Option<String>,
    pub host: Option<Host>,
    pub host_error: Option<String>,
    /// E.g. "the configured hotkey was invalid, the default is in use".
    pub hotkey_notice: Option<String>,
    pub events: Receiver<Event>,
    /// `--demo <word>`: render the card for this word without the input layer.
    pub demo: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        let ctx = cc.egui_ctx.clone();
        let font = theme::install_fonts(&ctx);

        theme::apply(&ctx, startup.config.ui.theme, startup.config.ui.font_size);

        let mut shared = Shared {
            ctx,
            config: startup.config,
            config_path: startup.config_path,
            config_error: startup.config_error.clone(),
            font,
            notices: Vec::new(),
            registry: Registry::empty(),
            registry_loading: false,
            loader: None,
            card: CardState::default(),
            settings: SettingsState::default(),
            host: startup.host,
            host_error: startup.host_error.clone(),
            pending_hotkey: None,
            ignore_result: false,
        };

        if let Some(message) = &shared.config_error {
            shared.notify(NoticeLevel::Error, format!("配置文件问题：{message}"));
        }
        if let Some(message) = &shared.host_error {
            shared.notify(
                NoticeLevel::Error,
                format!("输入层初始化失败：{message}"),
            );
        }
        if let Some(message) = &startup.hotkey_notice {
            shared.notify(NoticeLevel::Warn, message.clone());
        }
        if let Some(message) = &shared.font.error {
            shared.notify(NoticeLevel::Warn, message.clone());
        }

        shared.reload_registry();

        // Show the settings window when the user must act (first run, broken
        // config, no input layer). Debug builds always show it, so a smoke test
        // never depends on a tray icon that dict-input does not implement yet.
        let needs_attention = shared.config.dictionaries.is_empty()
            || shared.config_error.is_some()
            || shared.host_error.is_some()
            || cfg!(debug_assertions);
        if needs_attention {
            shared.open_settings();
        }

        // Trim once shortly after startup. `logic()` only runs when something
        // repaints, so an idle tray app would never reach a timed branch there.
        std::thread::Builder::new()
            .name("ws-trim".to_string())
            .spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(4));
                crate::platform::trim_working_set();
            })
            .ok();

        Self {
            shared: Arc::new(Mutex::new(shared)),
            events: startup.events,
            demo: startup.demo,
            card_was_visible: false,
            card_visible_sent: false,
            settings_visible_sent: false,
        }
    }

    fn drain_events(&self, shared: &mut Shared) {
        while let Ok(event) = self.events.try_recv() {
            shared.handle_event(event);
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut shared = lock(&self.shared);
        self.drain_events(&mut shared);
        shared.poll_registry();
        shared.maybe_commit_config(Instant::now());

        // Watchdog: only if the capture is genuinely slow. Opening the card
        // earlier would steal focus *while UIA is still reading*, which is the
        // exact failure we removed: a typical capture is tens of milliseconds,
        // so in practice the card appears with the result and never mid-capture.
        if let Some(started) = shared.pending_hotkey {
            if started.elapsed() >= std::time::Duration::from_millis(1200) {
                shared.pending_hotkey = None;
                let ui_config = shared.config.ui.clone();
                shared.card.begin_query(&ui_config);
                shared.wake();
            }
        }

        // `--demo`: wait for the registry, then show the card as if the hotkey
        // had fired. Deliberately one-shot.
        if let Some(word) = self.demo.take() {
            if shared.registry_loading {
                self.demo = Some(word);
            } else {
                logging::info(format!("demo lookup chars={}", word.chars().count()));
                shared.run_lookup(&word);
            }
        }

        // Give the working set back when nothing is on screen. This is a tray
        // app that idles for hours; the dictionary stays memory-mapped, so the
        // pages fault back in on the next lookup.
        if shared.card.visible {
            self.card_was_visible = true;
        } else if self.card_was_visible {
            self.card_was_visible = false;
            crate::platform::trim_working_set();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        let (card_id, settings_id) = (card_viewport_id(), settings_viewport_id());

        let (card_visible, card_pos, card_size, focus_card, settings_visible, focus_settings) = {
            let mut shared = lock(&self.shared);

            let card_visible = shared.card.visible;
            let focus_card =
                card_visible && shared.card.focused_generation != shared.card.open_generation;
            if focus_card {
                shared.card.focused_generation = shared.card.open_generation;
            }

            let settings_visible = shared.settings.open;
            let focus_settings = settings_visible
                && shared.settings.focused_generation != shared.settings.generation;
            if focus_settings {
                shared.settings.focused_generation = shared.settings.generation;
            }

            (
                card_visible,
                shared.card.window_pos,
                shared.card.window_size,
                focus_card,
                settings_visible,
                focus_settings,
            )
        };

        // The builder's `with_visible` only takes effect at viewport creation;
        // push every later transition as an explicit command. A hidden window is
        // only painted when it has a pending repaint (eframe's
        // check_redraw_requests skips idle invisible windows), so the wake is
        // what actually lets `Visible(true)` be processed.
        if card_visible != self.card_visible_sent {
            ctx.send_viewport_cmd_to(card_id, ViewportCommand::Visible(card_visible));
            ctx.request_repaint_of(card_id);
            self.card_visible_sent = card_visible;
        }
        if settings_visible != self.settings_visible_sent {
            ctx.send_viewport_cmd_to(settings_id, ViewportCommand::Visible(settings_visible));
            ctx.request_repaint_of(settings_id);
            self.settings_visible_sent = settings_visible;
        }

        // The card: frameless, translucent, always on top, never in the taskbar.
        let shared_for_card = Arc::clone(&self.shared);
        ctx.show_viewport_deferred(
            card_id,
            card_builder(card_visible, card_pos, card_size),
            move |ui, _class| {
                let mut shared = lock(&shared_for_card);
                card::show(ui, &mut shared);
            },
        );

        // The settings window: an ordinary decorated window.
        let shared_for_settings = Arc::clone(&self.shared);
        ctx.show_viewport_deferred(
            settings_id,
            settings_builder(settings_visible),
            move |ui, _class| {
                let mut shared = lock(&shared_for_settings);
                settings::show(ui, &mut shared);
            },
        );

        if focus_card {
            ctx.send_viewport_cmd_to(card_id, ViewportCommand::Focus);
        }
        if focus_settings {
            ctx.send_viewport_cmd_to(settings_id, ViewportCommand::Focus);
        }
    }

    /// The clear colour is fully transparent: the card window is translucent and
    /// must not have an opaque backdrop. The opaque settings window paints its
    /// own `CentralPanel` background over the whole client area.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn lock(shared: &Arc<Mutex<Shared>>) -> MutexGuard<'_, Shared> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn card_builder(visible: bool, pos: Option<egui::Pos2>, size: egui::Vec2) -> ViewportBuilder {
    let mut builder = ViewportBuilder::default()
        .with_title("dict-popup")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_resizable(false)
        .with_close_button(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_inner_size(size)
        .with_visible(visible);

    if let Some(pos) = pos {
        builder = builder.with_position(pos);
    }

    builder
}

fn settings_builder(visible: bool) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title("dict-popup 设置")
        .with_inner_size([760.0, 660.0])
        .with_min_inner_size([560.0, 420.0])
        // Child viewports inherit the root's transparency unless overridden.
        .with_transparent(false)
        .with_visible(visible)
}
