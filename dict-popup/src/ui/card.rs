//! The floating card: a frameless, always-on-top, translucent viewport that
//! shows one query result (DESIGN §5).

use std::time::{Duration, Instant};

use egui::{
    Color32, CornerRadius, Key, Margin, RichText, Stroke, TextStyle, Vec2, ViewportCommand,
};

use crate::app::Shared;
use crate::config::UiConfig;
use crate::dictionaries::{LookupReport, QueryScript, Registry};
use crate::platform::WorkArea;
use crate::theme::{self, CardPalette};

/// Transparent border around the card, so the painted shadow has room and the
/// rounded corners are not clipped by the window edge. The shadow reaches
/// `offset + blur / 2 + spread` points, which must stay below this.
pub const PAD: f32 = 20.0;
/// Total fade-in time (DESIGN §5: 150 ms).
pub const FADE_SECONDS: f32 = 0.15;
/// Offset from the cursor, in egui points (DESIGN §5).
pub const CURSOR_OFFSET: (f32, f32) = (16.0, 20.0);
/// Ignore focus loss for a moment after opening, while Windows hands focus over.
const FOCUS_GRACE: Duration = Duration::from_millis(350);
/// Safety net for a capture whose `SelectionFailed` never arrived (dict-input's
/// UIA budget is 1.5 s; this is deliberately generous).
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_ESTIMATED_HEIGHT: f32 = 220.0;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Phase {
    /// Nothing has been queried yet.
    Idle,
    /// Waiting for the UIA capture ("查询中" placeholder).
    Querying { since: Instant },
    /// A finished lookup (may still have zero hits).
    Ready(LookupReport),
    /// Informational card (no dictionaries configured, loading, ...).
    Info(String),
    /// Capture or lookup error.
    Failed(String),
}

pub struct CardState {
    pub visible: bool,
    pub phase: Phase,
    /// Bumped whenever the content is replaced; used for widget ids.
    pub generation: u64,
    /// Bumped only when the card goes from hidden to visible; drives the
    /// 150 ms fade-in and the one-time focus request.
    pub open_generation: u64,
    /// Cursor position when the card was opened, in physical pixels.
    pub anchor_phys: Option<(i32, i32)>,
    /// Work area of the monitor the cursor was on, in physical pixels.
    pub work_area: Option<WorkArea>,
    /// Scale factor of that monitor, used before the first frame.
    pub monitor_scale: f32,
    /// Last size/position we asked the OS for, in egui points.
    pub window_size: Vec2,
    pub window_pos: Option<egui::Pos2>,
    /// The window has to be (re)positioned on the next frame.
    pub needs_position: bool,
    /// `open_generation` the card last got focus for.
    pub focused_generation: u64,
    /// Did the card ever actually receive focus? Windows may refuse a focus
    /// steal from a background process; without this a "close on focus lost"
    /// card would close itself immediately after opening.
    pub saw_focus: bool,
    pub shown_at: Option<Instant>,
    /// Height of the card frame measured on the previous frame.
    pub measured_height: Option<f32>,
    /// Query captured while the registry was still loading.
    pub pending: Option<String>,
}

impl Default for CardState {
    fn default() -> Self {
        Self {
            visible: false,
            phase: Phase::Idle,
            generation: 0,
            open_generation: 0,
            anchor_phys: None,
            work_area: None,
            monitor_scale: 1.0,
            window_size: Vec2::new(320.0, DEFAULT_ESTIMATED_HEIGHT),
            window_pos: None,
            needs_position: false,
            focused_generation: 0,
            saw_focus: false,
            shown_at: None,
            measured_height: None,
            pending: None,
        }
    }
}

impl CardState {
    /// Show the "查询中" placeholder at the cursor (called on `Event::Hotkey`,
    /// well inside the ≤ 250 ms feedback budget of DESIGN §7.2).
    pub fn begin_query(&mut self, ui: &UiConfig) {
        self.mark_opened();
        self.phase = Phase::Querying {
            since: Instant::now(),
        };
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;

        let height = self
            .measured_height
            .unwrap_or(DEFAULT_ESTIMATED_HEIGHT)
            .clamp(96.0, ui.max_card_height);
        self.window_size = Vec2::new(ui.card_width + PAD * 2.0, height + PAD * 2.0);
        self.window_pos = Some(window_origin(place_card(
            self.anchor_phys,
            self.work_area,
            Vec2::new(ui.card_width, height),
            self.monitor_scale,
        )));
        self.needs_position = false;
    }

    /// Bump the open-generation (fade-in + focus) only on a hidden→visible
    /// transition, and re-anchor at the cursor.
    fn mark_opened(&mut self) {
        if !self.visible {
            self.open_generation = self.open_generation.wrapping_add(1);
            self.shown_at = Some(Instant::now());
            self.saw_focus = false;
            self.capture_anchor();
        }
        self.visible = true;
    }

    /// Remember where the mouse was and which monitor work area the card must
    /// stay inside of.
    fn capture_anchor(&mut self) {
        self.anchor_phys = crate::platform::cursor_pos();
        if let Some((x, y)) = self.anchor_phys {
            if let Some((area, scale)) = crate::platform::work_area_at(x, y) {
                self.work_area = Some(area);
                self.monitor_scale = scale;
            }
        }
    }

    pub fn show_report(&mut self, report: LookupReport) {
        self.reveal(Phase::Ready(report));
    }

    pub fn show_info(&mut self, text: impl Into<String>) {
        self.reveal(Phase::Info(text.into()));
    }

    pub fn show_error(&mut self, text: impl Into<String>) {
        self.reveal(Phase::Failed(text.into()));
    }

    fn reveal(&mut self, phase: Phase) {
        self.mark_opened();
        self.phase = phase;
        self.generation = self.generation.wrapping_add(1);
        self.needs_position = true;
    }

    /// The text the header shows (headword of the first hit, or the query).
    pub fn headline(&self) -> String {
        match &self.phase {
            Phase::Querying { .. } => "查询中".to_string(),
            Phase::Ready(report) => report
                .groups
                .first()
                .map(|group| group.entry.headword.clone())
                .filter(|headword| !headword.is_empty())
                .unwrap_or_else(|| report.query.clone()),
            Phase::Info(_) => "dict-popup".to_string(),
            Phase::Failed(_) => "取词失败".to_string(),
            Phase::Idle => String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Positioning (pure, unit tested)
// ---------------------------------------------------------------------------

/// Position of the **card frame** (not the window, which is `PAD` larger on
/// every side): cursor + (16, 20), flipped to the other side when the card would
/// leave the monitor's work area, then clamped so it is always fully on screen.
pub fn place_card(
    cursor_phys: Option<(i32, i32)>,
    work_area: Option<WorkArea>,
    card_size: Vec2,
    scale: f32,
) -> egui::Pos2 {
    let scale = if scale.is_finite() && scale > 0.1 {
        scale
    } else {
        1.0
    };

    let Some((cx, cy)) = cursor_phys else {
        return egui::pos2(PAD + 64.0, PAD + 64.0);
    };
    let Some(work) = work_area else {
        return egui::pos2(
            cx as f32 / scale + CURSOR_OFFSET.0,
            cy as f32 / scale + CURSOR_OFFSET.1,
        );
    };

    let cursor = egui::pos2(cx as f32 / scale, cy as f32 / scale);
    let min = egui::pos2(work.left as f32 / scale, work.top as f32 / scale);
    let max = egui::pos2(work.right as f32 / scale, work.bottom as f32 / scale);

    // The window around the card needs `PAD` of margin; keep that on screen too.
    let margin = Vec2::splat(PAD);
    let mut pos = egui::pos2(cursor.x + CURSOR_OFFSET.0, cursor.y + CURSOR_OFFSET.1);

    // Flip horizontally when the right edge would leave the work area.
    if pos.x + card_size.x + margin.x > max.x {
        pos.x = cursor.x - CURSOR_OFFSET.0 - card_size.x;
    }
    // Flip vertically when the bottom edge would leave the work area.
    if pos.y + card_size.y + margin.y > max.y {
        pos.y = cursor.y - CURSOR_OFFSET.1 - card_size.y;
    }

    pos.x = pos.x.clamp(min.x + margin.x, (max.x - card_size.x - margin.x).max(min.x + margin.x));
    pos.y = pos.y.clamp(min.y + margin.y, (max.y - card_size.y - margin.y).max(min.y + margin.y));
    pos
}

/// The window is the card plus the transparent shadow margin.
fn window_origin(card_pos: egui::Pos2) -> egui::Pos2 {
    card_pos - Vec2::splat(PAD)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn show(ui: &mut egui::Ui, shared: &mut Shared) {
    if !shared.card.visible {
        return;
    }

    let ui_config = shared.config.ui.clone();
    let now = Instant::now();

    // ---- close conditions -------------------------------------------------
    let mut close = ui.input(|input| input.key_pressed(Key::Escape));

    if !close && ui_config.close_on_focus_lost {
        let focused = ui.ctx().input(|input| input.viewport().focused);
        if focused == Some(true) {
            shared.card.saw_focus = true;
        }
        let age = shared
            .card
            .shown_at
            .map(|shown| now.saturating_duration_since(shown))
            .unwrap_or_default();
        if shared.card.saw_focus && focused == Some(false) && age > FOCUS_GRACE {
            close = true;
        }
    }

    if !close && ui_config.auto_hide_seconds > 0 {
        if let Some(shown) = shared.card.shown_at {
            let deadline = shown + Duration::from_secs(u64::from(ui_config.auto_hide_seconds));
            if now >= deadline {
                close = true;
            } else {
                ui.ctx().request_repaint_after(deadline - now);
            }
        }
    }

    // ---- query watchdog ---------------------------------------------------
    // The UIA capture is dropped rather than queued after 1.5 s (dict-input
    // contract); this is the belt-and-braces version, so a lost event can never
    // leave the spinner animating for ever.
    if let Phase::Querying { since } = shared.card.phase {
        let waited = now.saturating_duration_since(since);
        if waited > QUERY_TIMEOUT {
            shared
                .card
                .show_error("取词超时，请重试。若一直失败，当前程序可能不支持 UIA 取词。");
        } else {
            ui.ctx().request_repaint_after(QUERY_TIMEOUT - waited);
        }
    }

    // ---- fade-in ----------------------------------------------------------
    // Driven by wall-clock time, not egui's animation manager: opening the card
    // owns the fade, so it always ends after exactly FADE_SECONDS (a handful of
    // repaints) and then the app goes idle again.
    let alpha = match shared.card.shown_at {
        Some(shown) => {
            let t = now.saturating_duration_since(shown).as_secs_f32() / FADE_SECONDS;
            if t < 1.0 {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
            let t = t.clamp(0.0, 1.0);
            // Ease-out, so the card appears quickly and settles softly.
            1.0 - (1.0 - t) * (1.0 - t)
        }
        None => 1.0,
    };
    ui.set_opacity(alpha);

    let palette = theme::card_palette(ui.visuals().dark_mode);
    let inner_width = (ui_config.card_width - 32.0).max(160.0);
    // Room for the header, two separators, the footer and the frame margins,
    // so that a fully expanded body still fits inside `max_card_height`.
    let body_cap = (ui_config.max_card_height - PAD * 2.0 - 130.0).max(120.0);

    let (card_height, close_clicked) = egui::Frame::NONE
        .inner_margin(Margin::same(PAD as i8))
        .show(ui, |ui| {
            let frame = egui::Frame::new()
                .fill(palette.bg)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(CornerRadius::same(12))
                .shadow(palette.shadow)
                .inner_margin(Margin::symmetric(16, 13));

            let response = frame.show(ui, |ui| {
                ui.set_width(inner_width);
                ui.visuals_mut().override_text_color = Some(palette.text);
                card_contents(
                    ui,
                    &palette,
                    &ui_config,
                    &mut shared.card,
                    &shared.registry,
                    body_cap,
                )
            });
            (response.response.rect.height(), response.inner)
        })
        .inner;

    // ---- size + position --------------------------------------------------
    let desired = Vec2::new(
        ui_config.card_width + PAD * 2.0,
        (card_height + PAD * 2.0).min(ui_config.max_card_height + PAD * 2.0),
    );

    if (desired - shared.card.window_size).length() > 0.5 {
        shared.card.window_size = desired;
        shared.card.measured_height = Some(desired.y - PAD * 2.0);
        ui.ctx().send_viewport_cmd(ViewportCommand::InnerSize(desired));
        shared.card.needs_position = true;
    }

    if shared.card.needs_position {
        let ppp = ui.ctx().pixels_per_point();
        let card_pos = place_card(
            shared.card.anchor_phys,
            shared.card.work_area,
            desired - Vec2::splat(PAD * 2.0),
            ppp,
        );
        let pos = window_origin(card_pos);
        shared.card.window_pos = Some(pos);
        ui.ctx().send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        shared.card.needs_position = false;
    }

    if close || close_clicked {
        shared.card.visible = false;
        shared.card.shown_at = None;
        ui.ctx().send_viewport_cmd(ViewportCommand::Visible(false));
    }
}

fn card_contents(
    ui: &mut egui::Ui,
    palette: &CardPalette,
    ui_config: &UiConfig,
    card: &mut CardState,
    registry: &Registry,
    body_cap: f32,
) -> bool {
    card_header(ui, palette, card);
    ui.add_space(2.0);
    separator(ui, palette);

    egui::ScrollArea::vertical()
        .id_salt(("dict-popup-card-body", card.generation))
        .max_height(body_cap)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            card_body(ui, palette, ui_config, card, registry);
        });

    separator(ui, palette);
    let mut close_clicked = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Esc 关闭 · 再按热键重新查询")
                .small()
                .color(palette.muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(RichText::new("关闭").small()).frame(false))
                .clicked()
            {
                close_clicked = true;
            }
        });
    });

    close_clicked
}

fn card_header(ui: &mut egui::Ui, palette: &CardPalette, card: &CardState) {
    let headline = ellipsize(&card.headline(), 80);
    let (phonetic, _) = match &card.phase {
        Phase::Ready(report) => report
            .groups
            .first()
            .map(|group| split_phonetic(&group.entry.definition))
            .unwrap_or((None, String::new())),
        _ => (None, String::new()),
    };

    ui.horizontal(|ui| {
        let size = ui
            .style()
            .text_styles
            .get(&TextStyle::Heading)
            .map(|font| font.size + 2.0)
            .unwrap_or(24.0);
        ui.add(
            egui::Label::new(
                RichText::new(headline)
                    .size(size)
                    .strong()
                    .color(palette.headword),
            )
            .selectable(true),
        );

        if let Some(phonetic) = phonetic {
            ui.add(
                egui::Label::new(
                    RichText::new(phonetic)
                        .monospace()
                        .color(palette.muted),
                )
                .selectable(true),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Phase::Ready(report) = &card.phase {
                if let Some(first) = report.groups.first() {
                    chip(ui, first.direction.label(), palette.accent_text, palette.accent);
                }
            }
        });
    });

    // Second line: dictionary name + match metadata.
    ui.horizontal(|ui| {
        let mut source = String::new();
        if let Phase::Ready(report) = &card.phase {
            if let Some(first) = report.groups.first() {
                source = first.label.clone();
                if first.matched != report.query {
                    source.push_str(&format!(" · 匹配「{}」", ellipsize(&first.matched, 60)));
                }
            }
        }

        if !source.is_empty() {
            ui.label(RichText::new(source).small().color(palette.muted));
        }

        if let Phase::Ready(report) = &card.phase {
            if report.groups.iter().any(|group| group.low_confidence) {
                ui.label(
                    RichText::new("方向自动识别不确定")
                        .small()
                        .color(palette.warn),
                );
            }
        }
    });
}

fn card_body(
    ui: &mut egui::Ui,
    palette: &CardPalette,
    ui_config: &UiConfig,
    card: &CardState,
    registry: &Registry,
) {
    match &card.phase {
        Phase::Idle => {}
        Phase::Querying { .. } => {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(15.0));
                ui.label(RichText::new("正在读取选中文字…").color(palette.muted));
            });
            ui.add_space(6.0);
        }
        Phase::Info(text) => {
            ui.add_space(4.0);
            ui.label(RichText::new(text).color(palette.text));
            ui.add_space(4.0);
        }
        Phase::Failed(message) => {
            ui.add_space(4.0);
            ui.label(RichText::new(message).color(palette.danger));
            ui.add_space(4.0);
        }
        Phase::Ready(report) => render_report(ui, palette, ui_config, report, registry, card),
    }
}

fn render_report(
    ui: &mut egui::Ui,
    palette: &CardPalette,
    ui_config: &UiConfig,
    report: &LookupReport,
    registry: &Registry,
    card: &CardState,
) {
    if report.groups.is_empty() {
        ui.add_space(4.0);
        let query = ellipsize(&report.query, 120);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("未收录：").color(palette.muted));
            ui.add(
                egui::Label::new(RichText::new(&query).strong().color(palette.headword))
                    .selectable(true),
            );
        });

        if !report.candidates.is_empty() {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("试过的候选：{}", report.candidates.join("、")))
                    .small()
                    .color(palette.muted),
            );
        }

        if report.searched == 0 {
            let hint = if registry.is_empty() {
                "还没有配置词典，请打开设置添加 .ifo 文件".to_string()
            } else {
                let script_hint = match report.script {
                    QueryScript::Han => "中文查询只查「中→英 / 中→中」词典",
                    QueryScript::Latin => "英文查询只查「英→中 / 英→英」词典",
                    QueryScript::Other => "无法判断语种，全部词典都会被查询",
                };
                format!(
                    "没有方向匹配的词典：{script_hint}（{} 本被过滤）",
                    report.filtered
                )
            };
            ui.add_space(4.0);
            ui.label(RichText::new(hint).color(palette.warn));
        } else if !report.missed.is_empty() {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("已查询：{}", report.missed.join("、")))
                    .small()
                    .color(palette.muted),
            );
        }

        ui.add_space(4.0);
        return;
    }

    for (index, group) in report.groups.iter().enumerate() {
        let default_open = index == 0 && ui_config.expand_first_dictionary;
        let header = format!(
            "{} · 命中 {} 条",
            group.label,
            if group.entry.definition.is_empty() { 0 } else { 1 }
        );

        egui::CollapsingHeader::new(RichText::new(header).color(palette.text))
            .id_salt(("dict-popup-group", card.generation, index))
            .default_open(default_open)
            .show(ui, |ui| {
                let (_, definition) = split_phonetic(&group.entry.definition);
                if group.matched != report.query {
                    ui.label(
                        RichText::new(format!(
                            "匹配「{}」· {} · 第 {} 次尝试",
                            group.matched, group.reason, group.attempts
                        ))
                        .small()
                        .color(palette.muted),
                    );
                }
                let definition = clip_definition(&definition);
                ui.add(
                    egui::Label::new(RichText::new(definition.as_ref()).color(palette.text))
                        .selectable(true)
                        .wrap(),
                );
            });
    }

    if !report.missed.is_empty() {
        ui.add_space(2.0);
        ui.label(
            RichText::new(format!("未命中：{}", report.missed.join("、")))
                .small()
                .color(palette.muted),
        );
    }
}

fn separator(ui: &mut egui::Ui, palette: &CardPalette) {
    ui.add_space(6.0);
    let rect = ui.available_rect_before_wrap();
    let y = rect.top();
    ui.painter().hline(
        rect.x_range(),
        y,
        Stroke::new(1.0, palette.divider),
    );
    ui.add_space(7.0);
}

fn chip(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(fg));
        });
}

/// Pull a leading `/…/` or `[…]` phonetic off a definition, if the dictionary
/// stored one inline. Pure so it can be unit tested.
pub fn split_phonetic(definition: &str) -> (Option<String>, String) {    let trimmed = definition.trim_start();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return (None, String::new());
    };

    let close = match first {
        '/' => '/',
        '[' => ']',
        _ => return (None, definition.to_string()),
    };

    let body_len = trimmed[first.len_utf8()..]
        .find(close)
        .map(|index| first.len_utf8() + index);
    let Some(body_len) = body_len else {
        return (None, definition.to_string());
    };

    let candidate = trimmed[..body_len + close.len_utf8()].trim();
    // A phonetic is short; this keeps prose that happens to start with "[".
    if candidate.chars().count() > 40 {
        return (None, definition.to_string());
    }

    let rest = trimmed[body_len + close.len_utf8()..].trim_start();
    (Some(candidate.to_string()), rest.to_string())
}

/// Shorten a string for display without splitting a UTF-8 code point.
pub fn ellipsize(text: &str, max_chars: usize) -> String {
    // Fast path: a byte length below the limit implies a char count below it.
    if text.len() <= max_chars || text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// A pathological definition (some dictionaries embed a whole article) must not
/// be laid out every frame; the card shows the first 8000 bytes and says so.
pub fn clip_definition(definition: &str) -> std::borrow::Cow<'_, str> {
    const MAX_BYTES: usize = 8_000;

    if definition.len() <= MAX_BYTES {
        return std::borrow::Cow::Borrowed(definition);
    }

    let mut clipped: String = definition.chars().take(MAX_BYTES / 3).collect();
    clipped.push_str("\n…（释义过长，已截断）");
    std::borrow::Cow::Owned(clipped)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn area(left: i32, top: i32, right: i32, bottom: i32) -> WorkArea {
        WorkArea {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn places_below_and_right_of_the_cursor() {
        let pos = place_card(
            Some((100, 100)),
            Some(area(0, 0, 1920, 1080)),
            Vec2::new(400.0, 300.0),
            1.0,
        );
        assert_eq!(pos, egui::pos2(116.0, 120.0));
    }

    #[test]
    fn flips_left_near_the_right_edge() {
        let pos = place_card(
            Some((1900, 100)),
            Some(area(0, 0, 1920, 1080)),
            Vec2::new(400.0, 300.0),
            1.0,
        );
        // 1900 + 16 + 400 > 1920 -> flip to the left of the cursor.
        assert!(pos.x < 1900.0, "expected a left flip, got {pos:?}");
        assert!(pos.x + 400.0 <= 1920.0);
    }

    #[test]
    fn flips_up_near_the_bottom_edge() {
        let pos = place_card(
            Some((100, 1060)),
            Some(area(0, 0, 1920, 1080)),
            Vec2::new(400.0, 300.0),
            1.0,
        );
        assert!(pos.y < 1060.0, "expected an upward flip, got {pos:?}");
        assert!(pos.y + 300.0 <= 1080.0);
    }

    #[test]
    fn clamps_into_the_work_area() {
        // The card is bigger than the monitor: clamp to the work area plus the
        // shadow margin.
        let pos = place_card(
            Some((5, 5)),
            Some(area(0, 0, 300, 200)),
            Vec2::new(400.0, 300.0),
            1.0,
        );
        assert_eq!(pos, egui::pos2(PAD, PAD));
    }

    #[test]
    fn divides_physical_pixels_by_the_monitor_scale() {
        let pos = place_card(
            Some((200, 200)),
            Some(area(0, 0, 3840, 2160)),
            Vec2::new(400.0, 300.0),
            2.0,
        );
        assert_eq!(pos, egui::pos2(116.0, 120.0));
    }

    #[test]
    fn falls_back_without_a_cursor_or_monitor() {
        let pos = place_card(None, None, Vec2::new(400.0, 300.0), 1.0);
        assert_eq!(pos, egui::pos2(PAD + 64.0, PAD + 64.0));

        let pos = place_card(Some((50, 60)), None, Vec2::new(400.0, 300.0), 2.0);
        assert_eq!(pos, egui::pos2(41.0, 50.0));
    }

    #[test]
    fn the_window_origin_puts_the_card_at_the_cursor_offset() {
        // The window is PAD larger than the card on every side, so the visible
        // card lands exactly at cursor + (16, 20).
        let card_pos = place_card(
            Some((100, 100)),
            Some(area(0, 0, 1920, 1080)),
            Vec2::new(400.0, 300.0),
            1.0,
        );
        assert_eq!(
            window_origin(card_pos),
            egui::pos2(100.0 + CURSOR_OFFSET.0 - PAD, 100.0 + CURSOR_OFFSET.1 - PAD)
        );
    }

    #[test]
    fn splits_a_leading_phonetic() {
        let (phonetic, rest) = split_phonetic("/ˈæp.əl/\nn. 苹果");
        assert_eq!(phonetic.as_deref(), Some("/ˈæp.əl/"));
        assert_eq!(rest, "n. 苹果");

        let (phonetic, rest) = split_phonetic("[ˈæpəl] n. 苹果");
        assert_eq!(phonetic.as_deref(), Some("[ˈæpəl]"));
        assert_eq!(rest, "n. 苹果");
    }

    #[test]
    fn leaves_plain_definitions_alone() {
        let (phonetic, rest) = split_phonetic("n. 苹果\nv. 苹果");
        assert!(phonetic.is_none());
        assert_eq!(rest, "n. 苹果\nv. 苹果");

        let (phonetic, _) = split_phonetic("");
        assert!(phonetic.is_none());

        // Unbalanced bracket: not a phonetic.
        let (phonetic, rest) = split_phonetic("[not closed n. 苹果");
        assert!(phonetic.is_none());
        assert_eq!(rest, "[not closed n. 苹果");
    }

    #[test]
    fn clips_pathological_definitions() {
        let short = "n. 苹果";
        assert_eq!(clip_definition(short), short);

        let long = "中".repeat(20_000);
        let clipped = clip_definition(&long);
        assert!(clipped.len() < long.len());
        assert!(clipped.ends_with("…（释义过长，已截断）"));
        assert!(clipped.chars().count() > 1000);
    }

    #[test]
    fn ellipsizes_on_char_boundaries() {
        assert_eq!(ellipsize("苹果", 10), "苹果");
        assert_eq!(ellipsize("abcdef", 4), "abcd…");
        let long = "苹果".repeat(50);
        let short = ellipsize(&long, 10);
        assert_eq!(short.chars().count(), 11);
        assert!(short.ends_with('…'));
    }
}
