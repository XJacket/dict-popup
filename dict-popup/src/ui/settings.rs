//! Settings window (DESIGN §6): hotkey, dictionaries, appearance, behaviour,
//! about. Every edit autosaves (debounced) and takes effect immediately —
//! no "restart required".

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{RichText, ViewportCommand};

use crate::app::{NoticeLevel, Shared};
use crate::config::{Config, DictionaryConfig, DirectionSpec, ThemeMode, DEFAULT_HOTKEY};
use crate::dictionaries::{read_bookname, LoadedDictionary, Status};
use crate::platform;

const LOG_OPEN_FAILED: &str = "无法打开目录";

#[derive(Debug, Default)]
pub struct HotkeyCapture {
    pub active: bool,
    pub error: Option<String>,
}

pub struct SettingsState {
    pub open: bool,
    /// Edited copy; copied back into `Shared::config` by `commit_config`.
    pub draft: Config,
    pub dirty_since: Option<Instant>,
    pub saved_at: Option<Instant>,
    pub capture: HotkeyCapture,
    pub new_path: String,
    pub add_error: Option<String>,
    /// Bumped on every open so the viewport can be focused.
    pub generation: u64,
    pub focused_generation: u64,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            open: false,
            draft: Config::default(),
            dirty_since: None,
            saved_at: None,
            capture: HotkeyCapture::default(),
            new_path: String::new(),
            add_error: None,
            generation: 0,
            focused_generation: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn show(ui: &mut egui::Ui, shared: &mut Shared) {
    if !shared.settings.open {
        return;
    }

    if ui.ctx().input(|input| input.viewport().close_requested()) {
        close_settings(ui, shared);
        return;
    }

    egui::CentralPanel::default().show(ui, |ui| {
        header(ui, shared);
        notices(ui, shared);

        egui::Panel::bottom("dict-popup-settings-footer")
            .resizable(false)
            .show(ui, |ui| {
                footer(ui, shared);
            });

        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                hotkey_section(ui, shared);
                ui.add_space(10.0);
                dictionaries_section(ui, shared);
                ui.add_space(10.0);
                appearance_section(ui, shared);
                ui.add_space(10.0);
                behavior_section(ui, shared);
                ui.add_space(10.0);
                about_section(ui, shared);
                ui.add_space(12.0);
            });
    });
}

fn close_settings(ui: &egui::Ui, shared: &mut Shared) {
    // Never lose a pending edit because the window was closed inside the
    // autosave debounce window.
    if shared.settings.dirty_since.is_some() {
        shared.commit_config();
    }
    shared.settings.open = false;
    ui.ctx().send_viewport_cmd(ViewportCommand::Visible(false));
}

fn header(ui: &mut egui::Ui, shared: &Shared) {
    ui.horizontal(|ui| {
        ui.heading("dict-popup 设置");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .small()
                    .weak(),
            );
        });
    });

    let hotkey = shared.settings.draft.hotkey.clone();
    ui.label(
        RichText::new(format!("按 {hotkey} 读取选中文字，在鼠标旁弹出卡片。"))
            .small()
            .weak(),
    );
}

fn notices(ui: &mut egui::Ui, shared: &mut Shared) {
    if shared.notices.is_empty() {
        return;
    }

    let error_color = ui.visuals().error_fg_color;
    let warn_color = ui.visuals().warn_fg_color;
    let info_color = ui.visuals().weak_text_color();

    ui.add_space(4.0);
    for notice in &shared.notices {
        let color = match notice.level {
            NoticeLevel::Error => error_color,
            NoticeLevel::Warn => warn_color,
            NoticeLevel::Info => info_color,
        };
        ui.label(RichText::new(format!("• {}", notice.text)).color(color));
    }

    if ui.small_button("清除提示").clicked() {
        shared.notices.clear();
    }
}

fn footer(ui: &mut egui::Ui, shared: &mut Shared) {    ui.add_space(4.0);
    let now = Instant::now();
    let weak = ui.visuals().weak_text_color();
    let ok = ui.visuals().hyperlink_color;

    let status = if shared.settings.dirty_since.is_some() {
        ("正在保存…".to_string(), weak)
    } else if shared
        .settings
        .saved_at
        .is_some_and(|saved| now.saturating_duration_since(saved) < Duration::from_secs(4))
    {
        ("已保存，立即生效".to_string(), ok)
    } else {
        ("修改后自动保存并立即生效".to_string(), weak)
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(status.0).small().color(status.1));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("退出").on_hover_text("完全退出 dict-popup").clicked() {
                shared.quit();
            }
            if ui.button("关闭").clicked() {
                close_settings(ui, shared);
            }
        });
    });
}

// ---------------------------------------------------------------------------
// 1. 热键
// ---------------------------------------------------------------------------

fn hotkey_section(ui: &mut egui::Ui, shared: &mut Shared) {
    ui.group(|ui| {
        ui.strong("热键");
        ui.add_space(4.0);

        let capturing = shared.settings.capture.active;
        let accent = ui.visuals().hyperlink_color;

        ui.horizontal(|ui| {
            ui.label("当前：");
            ui.monospace(RichText::new(&shared.settings.draft.hotkey).strong());

            if capturing {
                ui.label(RichText::new("请按下新的组合键…（Esc 取消）").color(accent));
                if ui.button("取消").clicked() {
                    shared.settings.capture = HotkeyCapture::default();
                }
            } else {
                if ui.button("捕获新热键").clicked() {
                    shared.settings.capture = HotkeyCapture {
                        active: true,
                        error: None,
                    };
                }
                if ui.button("恢复默认").clicked() {
                    shared.settings.draft.hotkey = DEFAULT_HOTKEY.to_string();
                    shared.settings.capture = HotkeyCapture::default();
                    shared.mark_settings_dirty();
                }
            }
        });

        if capturing {
            let captured = ui.ctx().input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*modifiers, *key)),
                    _ => None,
                })
            });

            if let Some((modifiers, key)) = captured {
                apply_capture(shared, modifiers, key);
            }
        }

        if let Some(error) = shared.settings.capture.error.clone() {
            let color = ui.visuals().error_fg_color;
            ui.colored_label(color, error);
        }

        ui.label(
            RichText::new("全局热键至少需要 Ctrl / Alt / Shift 之一；Win 键暂时只能手动填写。")
                .small()
                .weak(),
        );
    });
}

fn apply_capture(shared: &mut Shared, modifiers: egui::Modifiers, key: egui::Key) {
    if key == egui::Key::Escape && !has_modifier(modifiers) {
        shared.settings.capture = HotkeyCapture::default();
        return;
    }

    match combo_from(modifiers, key) {
        Ok(combo) => match dict_input::hotkey::parse(&combo) {
            Ok(hotkey) => {
                shared.settings.draft.hotkey = hotkey.to_string();
                shared.settings.capture = HotkeyCapture::default();
                shared.mark_settings_dirty();
            }
            Err(err) => {
                shared.settings.capture.error = Some(err.to_string());
            }
        },
        Err(reason) => {
            shared.settings.capture.error = Some(reason);
        }
    }
}

fn has_modifier(modifiers: egui::Modifiers) -> bool {
    modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.command || modifiers.mac_cmd
}

/// Turn an egui key event into a canonical hotkey string that
/// `dict_input::hotkey::parse` accepts.
fn combo_from(modifiers: egui::Modifiers, key: egui::Key) -> Result<String, String> {
    let token = key_token(key).ok_or_else(|| "不支持这个按键，请换一个".to_string())?;
    if !has_modifier(modifiers) {
        return Err("全局热键至少需要 Ctrl / Alt / Shift 之一".to_string());
    }

    let mut parts: Vec<String> = Vec::new();
    if modifiers.ctrl {
        parts.push("Ctrl".to_string());
    }
    if modifiers.alt {
        parts.push("Alt".to_string());
    }
    if modifiers.shift {
        parts.push("Shift".to_string());
    }
    // egui only reports the Windows/Super key through `command` when it is not
    // already used as the platform command modifier.
    if modifiers.command && !modifiers.ctrl {
        parts.push("Win".to_string());
    }
    parts.push(token);
    Ok(parts.join("+"))
}

fn key_token(key: egui::Key) -> Option<String> {
    use egui::Key;

    let token = match key {
        Key::ArrowUp => "Up".to_string(),
        Key::ArrowDown => "Down".to_string(),
        Key::ArrowLeft => "Left".to_string(),
        Key::ArrowRight => "Right".to_string(),
        Key::Escape => "Esc".to_string(),
        Key::Enter => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Space => "Space".to_string(),
        Key::Backspace => "Backspace".to_string(),
        Key::Delete => "Delete".to_string(),
        Key::Insert => "Insert".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "PageUp".to_string(),
        Key::PageDown => "PageDown".to_string(),
        Key::Minus => "-".to_string(),
        Key::Equals => "=".to_string(),
        Key::OpenBracket => "[".to_string(),
        Key::CloseBracket => "]".to_string(),
        Key::Backslash => "\\".to_string(),
        Key::Semicolon => ";".to_string(),
        Key::Quote => "'".to_string(),
        Key::Comma => ",".to_string(),
        Key::Period => ".".to_string(),
        Key::Slash => "/".to_string(),
        Key::Backtick => "`".to_string(),
        _ => {
            // Letters (A-Z), digits (Num0-Num9) and F1-F24 print as their own
            // names; everything else (media keys, browser keys, …) cannot be
            // registered by dict-input.
            let name = format!("{key:?}");
            if name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase() {
                name
            } else if let Some(digit) = name.strip_prefix("Num") {
                match digit.parse::<u32>() {
                    Ok(number) if (0..=9).contains(&number) => number.to_string(),
                    _ => return None,
                }
            } else if let Some(digits) = name.strip_prefix('F') {
                match digits.parse::<u32>() {
                    Ok(number) if (1..=24).contains(&number) => name,
                    _ => return None,
                }
            } else {
                return None;
            }
        }
    };

    Some(token)
}

// ---------------------------------------------------------------------------
// 2. 词典
// ---------------------------------------------------------------------------

fn dictionaries_section(ui: &mut egui::Ui, shared: &mut Shared) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.strong("词典");
            ui.label(
                RichText::new(format!(
                    "{} 本已配置 · {} 本可用",
                    shared.settings.draft.dictionaries.len(),
                    shared.registry.ready_count()
                ))
                .small()
                .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("重新加载").clicked() {
                    shared.reload_registry();
                }
            });
        });
        ui.label(
            RichText::new("从上到下依次查询；方向不匹配的词典会自动跳过。")
                .small()
                .weak(),
        );

        let mut dirty = false;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let mut remove: Option<usize> = None;

        let count = shared.settings.draft.dictionaries.len();
        for index in 0..count {
            let (mut enabled, mut label, mut direction, path_text) = {
                let row = &shared.settings.draft.dictionaries[index];
                (
                    row.enabled,
                    row.label.clone(),
                    row.direction,
                    row.path.clone(),
                )
            };

            ui.separator();

            let mut row_dirty = false;
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut enabled, "")
                    .on_hover_text("启用这本词典")
                    .changed()
                {
                    row_dirty = true;
                }

                if ui
                    .add(
                        egui::TextEdit::singleline(&mut label)
                            .desired_width(150.0)
                            .hint_text("显示名"),
                    )
                    .on_hover_text("卡片上显示的词典名")
                    .changed()
                {
                    row_dirty = true;
                }

                egui::ComboBox::from_id_salt(("dict-direction", index))
                    .width(88.0)
                    .selected_text(direction.label())
                    .show_ui(ui, |ui| {
                        for spec in DirectionSpec::ALL {
                            if ui
                                .selectable_value(&mut direction, spec, spec.label())
                                .changed()
                            {
                                row_dirty = true;
                            }
                        }
                    });

                if ui
                    .add_enabled(index > 0, egui::Button::new("↑"))
                    .on_hover_text("上移（优先查询）")
                    .clicked()
                {
                    move_up = Some(index);
                }
                if ui
                    .add_enabled(index + 1 < count, egui::Button::new("↓"))
                    .on_hover_text("下移")
                    .clicked()
                {
                    move_down = Some(index);
                }
                if ui.button("移除").on_hover_text("从列表移除（不删除文件）").clicked() {
                    remove = Some(index);
                }
            });

            let (status_text, status_color) = row_status(ui, shared, &path_text);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&path_text).small().weak());
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(status_text).small().color(status_color));
            });

            if row_dirty {
                dirty = true;
            }

            let row = &mut shared.settings.draft.dictionaries[index];
            if row.enabled != enabled {
                row.enabled = enabled;
            }
            if row.label != label {
                row.label = label;
            }
            if row.direction != direction {
                row.direction = direction;
            }
        }

        if count == 0 {
            ui.add_space(4.0);
            ui.label(RichText::new("还没有词典。在下面粘贴 .ifo 路径后点「添加」。").weak());
        }

        if let Some(index) = move_up {
            if index > 0 {
                shared.settings.draft.dictionaries.swap(index, index - 1);
                dirty = true;
            }
        }
        if let Some(index) = move_down {
            if index + 1 < shared.settings.draft.dictionaries.len() {
                shared.settings.draft.dictionaries.swap(index, index + 1);
                dirty = true;
            }
        }
        if let Some(index) = remove {
            if index < shared.settings.draft.dictionaries.len() {
                shared.settings.draft.dictionaries.remove(index);
                dirty = true;
            }
        }

        // ---- add by path (no file-dialog dependency, see report) ----------
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("添加：");
            ui.add(
                egui::TextEdit::singleline(&mut shared.settings.new_path)
                    .desired_width(360.0)
                    .hint_text(r"D:\dicts\stardict-ecdict-2.4.2.ifo"),
            );
            if ui.button("添加").clicked() {
                add_dictionary(shared);
                dirty = true;
            }
        });
        ui.label(
            RichText::new("文件对话框未接入（不新增依赖），请粘贴 .ifo 的完整路径；配套的 .idx/.dict 必须在同一目录。")
                .small()
                .weak(),
        );

        if let Some(error) = shared.settings.add_error.clone() {
            let color = ui.visuals().error_fg_color;
            ui.colored_label(color, error);
        }

        if dirty {
            shared.mark_settings_dirty();
        }
    });
}

/// Status text + colour for one configured dictionary row.
fn row_status(ui: &egui::Ui, shared: &Shared, path_text: &str) -> (String, egui::Color32) {
    let error_color = ui.visuals().error_fg_color;
    let warn_color = ui.visuals().warn_fg_color;
    let weak = ui.visuals().weak_text_color();

    let path = PathBuf::from(path_text.trim());
    match loaded_row(&shared.registry, &path) {
        Some(loaded) => {
            let mut text = loaded.status.summarize();
            if let Some(detail) = loaded.status.error() {
                text = truncate(detail, 120);
            }
            let color = match &loaded.status {
                Status::Ready {
                    low_confidence: true,
                    ..
                } => warn_color,
                Status::Ready { .. } | Status::Disabled => weak,
                Status::Missing(_) | Status::Broken(_) => error_color,
            };
            (text, color)
        }
        None => ("尚未应用（保存后生效）".to_string(), warn_color),
    }
}

fn loaded_row<'a>(
    registry: &'a crate::dictionaries::Registry,
    path: &PathBuf,
) -> Option<&'a LoadedDictionary> {
    registry.rows().iter().find(|row| row.path == *path)
}

fn add_dictionary(shared: &mut Shared) {
    let path = shared.settings.new_path.trim().to_string();

    if path.is_empty() {
        shared.settings.add_error = Some("请粘贴 .ifo 文件的完整路径".to_string());
        return;
    }

    let path_buf = PathBuf::from(&path);
    if !path_buf.is_file() {
        shared.settings.add_error = Some(format!("文件不存在：{path}"));
        return;
    }
    if !path.to_ascii_lowercase().ends_with(".ifo") {
        shared.settings.add_error = Some("只能添加 .ifo 文件（StarDict 词典）".to_string());
        return;
    }
    if shared
        .settings
        .draft
        .dictionaries
        .iter()
        .any(|row| row.path.eq_ignore_ascii_case(&path))
    {
        shared.settings.add_error = Some("这本词典已经在列表里了".to_string());
        return;
    }

    let label = read_bookname(&path_buf).unwrap_or_else(|| {
        path_buf
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("未命名词典")
            .to_string()
    });

    shared
        .settings
        .draft
        .dictionaries
        .push(DictionaryConfig::new(path, label));
    shared.settings.new_path.clear();
    shared.settings.add_error = None;
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------------------
// 3. 外观
// ---------------------------------------------------------------------------

fn appearance_section(ui: &mut egui::Ui, shared: &mut Shared) {
    ui.group(|ui| {
        ui.strong("外观");
        ui.add_space(4.0);

        let mut dirty = false;

        ui.horizontal(|ui| {
            ui.label("主题");
            egui::ComboBox::from_id_salt("appearance-theme")
                .width(110.0)
                .selected_text(shared.settings.draft.ui.theme.label())
                .show_ui(ui, |ui| {
                    for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
                        if ui
                            .selectable_value(
                                &mut shared.settings.draft.ui.theme,
                                mode,
                                mode.label(),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                    }
                });

            ui.add_space(12.0);
            ui.label("字号");
            if ui
                .add(
                    egui::DragValue::new(&mut shared.settings.draft.ui.font_size)
                        .range(9.0..=32.0)
                        .speed(0.2),
                )
                .changed()
            {
                dirty = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label("卡片宽度");
            if ui
                .add(
                    egui::DragValue::new(&mut shared.settings.draft.ui.card_width)
                        .range(280.0..=900.0)
                        .speed(2.0)
                        .suffix(" px"),
                )
                .changed()
            {
                dirty = true;
            }

            ui.add_space(12.0);
            ui.label("最大高度");
            if ui
                .add(
                    egui::DragValue::new(&mut shared.settings.draft.ui.max_card_height)
                        .range(160.0..=1400.0)
                        .speed(2.0)
                        .suffix(" px"),
                )
                .changed()
            {
                dirty = true;
            }
        });

        ui.label(
            RichText::new("圆角 12 px、投影、深色半透明卡片；中文使用系统微软雅黑。")
                .small()
                .weak(),
        );

        if dirty {
            shared.mark_settings_dirty();
        }
    });
}

// ---------------------------------------------------------------------------
// 4. 行为
// ---------------------------------------------------------------------------

fn behavior_section(ui: &mut egui::Ui, shared: &mut Shared) {
    ui.group(|ui| {
        ui.strong("行为");
        ui.add_space(4.0);

        let mut dirty = false;

        if ui
            .checkbox(
                &mut shared.settings.draft.behavior.autostart,
                "开机自动启动",
            )
            .on_hover_text("写入 HKCU\\...\\CurrentVersion\\Run")
            .changed()
        {
            dirty = true;
        }

        if ui
            .checkbox(
                &mut shared.settings.draft.behavior.trim_selection,
                "去掉选中文字的首尾空白",
            )
            .changed()
        {
            dirty = true;
        }

        ui.horizontal(|ui| {
            if ui
                .checkbox(
                    &mut shared.settings.draft.ui.close_on_focus_lost,
                    "失去焦点时关闭卡片",
                )
                .changed()
            {
                dirty = true;
            }

            ui.add_space(12.0);
            ui.label("自动隐藏");
            if ui
                .add(
                    egui::DragValue::new(&mut shared.settings.draft.ui.auto_hide_seconds)
                        .range(0..=600)
                        .suffix(" 秒"),
                )
                .changed()
            {
                dirty = true;
            }
            ui.label(RichText::new("（0 = 不自动隐藏）").small().weak());
        });

        if ui
            .checkbox(
                &mut shared.settings.draft.ui.expand_first_dictionary,
                "卡片中默认展开第一组释义",
            )
            .changed()
        {
            dirty = true;
        }

        if dirty {
            shared.mark_settings_dirty();
        }
    });
}

// ---------------------------------------------------------------------------
// 5. 关于
// ---------------------------------------------------------------------------

fn about_section(ui: &mut egui::Ui, shared: &mut Shared) {
    ui.group(|ui| {
        ui.strong("关于");
        ui.add_space(4.0);
        ui.label(format!(
            "dict-popup {} · 完全离线 · 不读写剪贴板 · 日志只记录选中文字长度",
            env!("CARGO_PKG_VERSION")
        ));

        let config_path = shared.config_path.clone();
        ui.horizontal(|ui| {
            ui.label("配置：");
            ui.monospace(config_path.display().to_string());
            if ui.button("打开目录").clicked() {
                open_parent(&config_path, shared);
            }
        });

        let log_path = crate::config::log_path();
        ui.horizontal(|ui| {
            ui.label("日志：");
            ui.monospace(log_path.display().to_string());
            if ui.button("打开目录").clicked() {
                open_parent(&log_path, shared);
            }
        });

        if let Some(source) = &shared.font.source {
            ui.label(
                RichText::new(format!("中文字体：{source}"))
                    .small()
                    .weak(),
            );
        }

        ui.label(
            RichText::new("许可证：MIT OR Apache-2.0（详见发行包内的 LICENSE）")
                .small()
                .weak(),
        );
    });
}

fn open_parent(path: &std::path::Path, shared: &mut Shared) {
    let opened = path
        .parent()
        .map(platform::open_path)
        .unwrap_or(false);
    if !opened {
        shared.notify(
            NoticeLevel::Warn,
            format!("{LOG_OPEN_FAILED}：{}", path.display()),
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dict_input::hotkey;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            ctrl,
            alt,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn builds_a_canonical_combo() {
        let combo = combo_from(mods(true, true, false), egui::Key::D).expect("combo");
        assert_eq!(combo, "Ctrl+Alt+D");
        // and dict-input accepts it
        let parsed = hotkey::parse(&combo).expect("parse");
        assert_eq!(parsed.to_string(), "Ctrl+Alt+D");
    }

    #[test]
    fn rejects_a_bare_key() {
        let err = combo_from(mods(false, false, false), egui::Key::D).unwrap_err();
        assert!(err.contains("至少需要"));
    }

    #[test]
    fn rejects_keys_dict_input_cannot_register() {
        // egui reports it, but dict-input has no token for it (and "+" is the
        // separator of the hotkey syntax anyway).
        assert!(combo_from(mods(true, false, false), egui::Key::Plus).is_err());
        assert!(combo_from(mods(true, false, false), egui::Key::MediaPlayPause).is_err());
    }

    #[test]
    fn maps_the_keys_the_design_lists() {
        assert_eq!(key_token(egui::Key::F9).as_deref(), Some("F9"));
        assert_eq!(key_token(egui::Key::Num5).as_deref(), Some("5"));
        assert_eq!(key_token(egui::Key::ArrowUp).as_deref(), Some("Up"));
        assert_eq!(key_token(egui::Key::Escape).as_deref(), Some("Esc"));
        assert_eq!(key_token(egui::Key::Backtick).as_deref(), Some("`"));
        assert_eq!(key_token(egui::Key::Slash).as_deref(), Some("/"));

        // Everything we map must be parseable by dict-input.
        for key in [
            egui::Key::A,
            egui::Key::Z,
            egui::Key::F1,
            egui::Key::F24,
            egui::Key::Num0,
            egui::Key::Space,
            egui::Key::PageDown,
            egui::Key::Delete,
            egui::Key::Backslash,
        ] {
            let token = key_token(key).expect("token");
            let combo = format!("Ctrl+{token}");
            assert!(
                hotkey::parse(&combo).is_ok(),
                "dict-input rejects {combo}"
            );
        }
    }

    #[test]
    fn truncation_is_char_safe() {
        assert_eq!(truncate("短", 10), "短");
        let long = "中".repeat(20);
        let cut = truncate(&long, 5);
        assert_eq!(cut.chars().count(), 6); // 5 + ellipsis
        assert!(cut.ends_with('…'));
    }
}
