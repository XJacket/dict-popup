//! The floating card: a frameless, always-on-top, translucent viewport that
//! shows one query result (DESIGN §5).
//!
//! The card has no lifecycle of its own: [`crate::request::Request`] says
//! whether a request is running, finished, failed or timed out, and the card
//! renders that state. `CardState` keeps only presentation state (visibility,
//! fade, measured size) and the anchor it was placed at.

use std::time::{Duration, Instant};

use egui::{
    Color32, CornerRadius, Key, Margin, RichText, Stroke, TextStyle, Vec2, ViewportCommand,
};

use dict_input::host::Anchor;

use crate::app::Shared;
use crate::config::UiConfig;
use crate::dictionaries::{LookupReport, QueryScript, Registry};
use crate::request::{Outcome, State};
use crate::theme::{self, CardPalette};

/// Transparent margin around the card inside its window.
///
/// This used to be `20.0` so a painted drop shadow had room, but the shadow
/// (offset 8, blur 20-22, spread 1) reached exactly the window edge and was
/// clipped by it. The clip left a hard rectangular edge outside the card's own
/// 1px stroke - two visible borders - and the transparent ring swallowed clicks
/// that should have gone to the window underneath.
///
/// The window is now exactly the card: one border, exact hit area.
pub const PAD: f32 = 0.0;
/// Total fade-in time (DESIGN §5: 150 ms).
pub const FADE_SECONDS: f32 = 0.15;
/// Offset from the anchor cursor, in egui points (DESIGN §5).
pub const CURSOR_OFFSET: (f32, f32) = (16.0, 20.0);
/// Ignore focus loss for a moment after opening, while Windows hands focus over.
const FOCUS_GRACE: Duration = Duration::from_millis(350);
const DEFAULT_ESTIMATED_HEIGHT: f32 = 220.0;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct CardState {
    pub visible: bool,
    /// Bumped only when the card goes from hidden to visible; drives the
    /// 150 ms fade-in and the one-time focus request.
    pub open_generation: u64,
    /// Anchor of the request this card currently shows: cursor, work area and
    /// scale, sampled by the host when the hotkey fired.
    pub anchor: Anchor,
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
}

impl Default for CardState {
    fn default() -> Self {
        Self {
            visible: false,
            open_generation: 0,
            anchor: Anchor::default(),
            window_size: Vec2::new(320.0, DEFAULT_ESTIMATED_HEIGHT),
            window_pos: None,
            needs_position: false,
            focused_generation: 0,
            saw_focus: false,
            shown_at: None,
            measured_height: None,
        }
    }
}

impl CardState {
    /// Reveal the card at `anchor`: estimate its size from the last measured
    /// frame, position it and start the fade-in.
    ///
    /// Called when a request finishes (or when `--demo` opens the card). The
    /// content itself comes from [`crate::request::Request`]; when the card is
    /// already visible (the deferred lookup replaced "正在加载…"), only the
    /// content changes and the window stays where it is.
    pub fn open_at(&mut self, ui: &UiConfig, anchor: Anchor) {
        self.anchor = anchor;
        if self.visible {
            return;
        }

        self.open_generation = self.open_generation.wrapping_add(1);
        self.shown_at = Some(Instant::now());
        self.saw_focus = false;
        self.visible = true;

        let height = self
            .measured_height
            .unwrap_or(DEFAULT_ESTIMATED_HEIGHT)
            .clamp(96.0, ui.max_card_height);
        self.window_size = Vec2::new(ui.card_width + PAD * 2.0, height + PAD * 2.0);
        self.window_pos = Some(window_origin(place_card(
            anchor,
            Vec2::new(ui.card_width, height),
        )));
        // Re-position with the real measured height on the next pass.
        self.needs_position = true;
    }

    /// Hide the card (Esc, focus loss, auto-hide, second hotkey).
    pub fn hide(&mut self) {
        self.visible = false;
        self.shown_at = None;
        self.saw_focus = false;
    }
}

// ---------------------------------------------------------------------------
// Positioning (pure, unit tested)
// ---------------------------------------------------------------------------

/// Position of the **card frame** (not the window, which is `PAD` larger on
/// every side): anchor cursor + (16, 20), flipped to the other side when the
/// card would leave the monitor's work area, then clamped so it is always fully
/// on screen.
///
/// The scale comes from the anchor - the monitor the hotkey was pressed on -
/// and from nowhere else. There is deliberately no `pixels_per_point()` input:
/// on a mixed-DPI setup the two could disagree.
pub fn place_card(anchor: Anchor, card_size: Vec2) -> egui::Pos2 {
    let scale = anchor.effective_scale();

    let Some((cx, cy)) = anchor.cursor else {
        return egui::pos2(PAD + 64.0, PAD + 64.0);
    };
    let Some(work) = anchor.work_area else {
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

    pos.x = pos.x.clamp(
        min.x + margin.x,
        (max.x - card_size.x - margin.x).max(min.x + margin.x),
    );
    pos.y = pos.y.clamp(
        min.y + margin.y,
        (max.y - card_size.y - margin.y).max(min.y + margin.y),
    );
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
    let revision = shared.request.revision();

    let (card_height, close_clicked) = egui::Frame::NONE
        .inner_margin(Margin::same(PAD as i8))
        .show(ui, |ui| {
            let frame = egui::Frame::new()
                .fill(palette.bg)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(16, 13));

            let response = frame.show(ui, |ui| {
                ui.set_width(inner_width);
                ui.visuals_mut().override_text_color = Some(palette.text);
                card_contents(
                    ui,
                    &palette,
                    &ui_config,
                    shared.request.state(),
                    &shared.registry,
                    revision,
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
        let card_pos = place_card(shared.card.anchor, desired - Vec2::splat(PAD * 2.0));
        let pos = window_origin(card_pos);
        shared.card.window_pos = Some(pos);
        ui.ctx().send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        shared.card.needs_position = false;
    }

    if close || close_clicked {
        // Closing the card ends the request: the pending query and any late
        // capture result die here, so the card cannot reopen by itself.
        shared.close_card();
    }
}

fn card_contents(
    ui: &mut egui::Ui,
    palette: &CardPalette,
    ui_config: &UiConfig,
    state: &State,
    registry: &Registry,
    revision: u64,
    body_cap: f32,
) -> bool {
    card_header(ui, palette, state);
    ui.add_space(2.0);
    separator(ui, palette);

    egui::ScrollArea::vertical()
        .id_salt(("dict-popup-card-body", revision))
        .max_height(body_cap)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            card_body(ui, palette, ui_config, state, registry, revision);
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

fn card_header(ui: &mut egui::Ui, palette: &CardPalette, state: &State) {
    let headline = ellipsize(&state.headline(), 80);
    let (phonetic, _) = match state {
        State::Done(Outcome::Report(report)) => report
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
                egui::Label::new(RichText::new(phonetic).monospace().color(palette.muted))
                    .selectable(true),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let State::Done(Outcome::Report(report)) = state {
                if let Some(first) = report.groups.first() {
                    chip(ui, first.direction.label(), palette.accent_text, palette.accent);
                }
            }
        });
    });

    // Second line: dictionary name + match metadata.
    ui.horizontal(|ui| {
        let mut source = String::new();
        if let State::Done(Outcome::Report(report)) = state {
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

        if let State::Done(Outcome::Report(report)) = state {
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
    state: &State,
    registry: &Registry,
    revision: u64,
) {
    match state {
        State::Idle => {}
        State::Running { .. } => {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(15.0));
                ui.label(RichText::new("正在读取选中文字…").color(palette.muted));
            });
            ui.add_space(6.0);
            // Normally the card is hidden while a request runs; keep the
            // spinner alive if it is ever visible in this state.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        State::Done(Outcome::Info(info)) => {
            ui.add_space(4.0);
            ui.label(RichText::new(&info.text).color(palette.text));
            ui.add_space(4.0);
        }
        State::Done(Outcome::Report(report)) => {
            render_report(ui, palette, ui_config, report, registry, revision);
        }
        State::Failed(_) | State::TimedOut => {
            ui.add_space(4.0);
            let message = state.failure_text().unwrap_or_default();
            ui.label(RichText::new(message).color(palette.danger));
            ui.add_space(4.0);
        }
    }
}

fn render_report(
    ui: &mut egui::Ui,
    palette: &CardPalette,
    ui_config: &UiConfig,
    report: &LookupReport,
    registry: &Registry,
    revision: u64,
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
            .id_salt(("dict-popup-group", revision, index))
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
    ui.painter()
        .hline(rect.x_range(), y, Stroke::new(1.0, palette.divider));
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
pub fn split_phonetic(definition: &str) -> (Option<String>, String) {
    let trimmed = definition.trim_start();
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

    use dict_input::host::WorkArea;

    fn area(left: i32, top: i32, right: i32, bottom: i32) -> Option<WorkArea> {
        Some(WorkArea {
            left,
            top,
            right,
            bottom,
        })
    }

    fn anchor(cursor: Option<(i32, i32)>, work_area: Option<WorkArea>, scale: f32) -> Anchor {
        Anchor {
            cursor,
            work_area,
            scale,
        }
    }

    #[test]
    fn places_below_and_right_of_the_cursor() {
        let pos = place_card(
            anchor(Some((100, 100)), area(0, 0, 1920, 1080), 1.0),
            Vec2::new(400.0, 300.0),
        );
        assert_eq!(pos, egui::pos2(116.0, 120.0));
    }

    #[test]
    fn flips_left_near_the_right_edge() {
        let pos = place_card(
            anchor(Some((1900, 100)), area(0, 0, 1920, 1080), 1.0),
            Vec2::new(400.0, 300.0),
        );
        // 1900 + 16 + 400 > 1920 -> flip to the left of the cursor.
        assert!(pos.x < 1900.0, "expected a left flip, got {pos:?}");
        assert!(pos.x + 400.0 <= 1920.0);
    }

    #[test]
    fn flips_up_near_the_bottom_edge() {
        let pos = place_card(
            anchor(Some((100, 1060)), area(0, 0, 1920, 1080), 1.0),
            Vec2::new(400.0, 300.0),
        );
        assert!(pos.y < 1060.0, "expected an upward flip, got {pos:?}");
        assert!(pos.y + 300.0 <= 1080.0);
    }

    #[test]
    fn clamps_into_the_work_area() {
        // The card is bigger than the monitor: clamp to the work area plus the
        // shadow margin.
        let pos = place_card(
            anchor(Some((5, 5)), area(0, 0, 300, 200), 1.0),
            Vec2::new(400.0, 300.0),
        );
        assert_eq!(pos, egui::pos2(PAD, PAD));
    }

    #[test]
    fn divides_physical_pixels_by_the_monitor_scale() {
        let pos = place_card(
            anchor(Some((200, 200)), area(0, 0, 3840, 2160), 2.0),
            Vec2::new(400.0, 300.0),
        );
        assert_eq!(pos, egui::pos2(116.0, 120.0));
    }

    #[test]
    fn the_anchor_scale_is_the_only_scale() {
        // Cursor and work area are physical pixels on a 150% monitor: every
        // coordinate must be divided by the anchor's scale.
        let pos = place_card(
            anchor(Some((300, 300)), area(0, 0, 3840, 2160), 1.5),
            Vec2::new(400.0, 300.0),
        );
        assert_eq!(pos, egui::pos2(216.0, 220.0));
    }

    #[test]
    fn a_broken_scale_falls_back_to_one() {
        let expected = egui::pos2(116.0, 120.0);
        for scale in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let pos = place_card(
                anchor(Some((100, 100)), area(0, 0, 1920, 1080), scale),
                Vec2::new(400.0, 300.0),
            );
            assert_eq!(pos, expected, "scale {scale} must degrade to 1.0");
        }
    }

    #[test]
    fn falls_back_without_a_cursor_or_monitor() {
        let pos = place_card(Anchor::default(), Vec2::new(400.0, 300.0));
        assert_eq!(pos, egui::pos2(PAD + 64.0, PAD + 64.0));

        let pos = place_card(
            anchor(Some((50, 60)), None, 2.0),
            Vec2::new(400.0, 300.0),
        );
        assert_eq!(pos, egui::pos2(41.0, 50.0));
    }

    #[test]
    fn the_window_origin_puts_the_card_at_the_cursor_offset() {
        // The window is PAD larger than the card on every side, so the visible
        // card lands exactly at cursor + (16, 20).
        let card_pos = place_card(
            anchor(Some((100, 100)), area(0, 0, 1920, 1080), 1.0),
            Vec2::new(400.0, 300.0),
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
