//! Look & feel: system CJK font, theme selection, and the card palette.
//!
//! No font is bundled (DESIGN §"Theme" in the task): the Microsoft YaHei face is
//! memory-mapped straight out of `C:\Windows\Fonts` and handed to egui, so the
//! 20 MB font file never becomes 20 MB of private heap.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, ThemePreference,
};

use crate::config::ThemeMode;

/// Faces to try, in order. `(file name, face index inside the collection)`.
const CJK_CANDIDATES: &[(&str, u32)] = &[
    ("msyh.ttc", 0),   // Microsoft YaHei
    ("msyh.ttf", 0),
    ("msyhl.ttc", 1),  // Microsoft YaHei Light
    ("msjh.ttc", 0),   // Microsoft JhengHei (traditional)
    ("simsun.ttc", 0), // SimSun (last-ditch)
    ("simhei.ttf", 0),
];

pub const CJK_FAMILY: &str = "dict-popup-cjk";

/// Outcome of the font installation, surfaced in the settings window.
#[derive(Debug, Clone, Default)]
pub struct FontInstall {
    pub source: Option<String>,
    pub error: Option<String>,
}

/// Wrap a read-only file mapping so epaint can borrow it as font bytes.
struct MappedBlob(dict_core::mmap::MappedFile);

impl AsRef<[u8]> for MappedBlob {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// Load the system Chinese font and install it as the fallback for both
/// families (Latin text keeps the default egui font).
pub fn install_fonts(ctx: &egui::Context) -> FontInstall {
    let mut definitions = FontDefinitions::default();

    let Some(fonts_dir) = system_fonts_dir() else {
        return FontInstall {
            source: None,
            error: Some("找不到系统字体目录，中文可能显示为方块".to_string()),
        };
    };

    let mut last_error: Option<String> = None;
    for (name, index) in CJK_CANDIDATES {
        let path = fonts_dir.join(name);
        if !path.exists() {
            continue;
        }

        let mapped = match dict_core::mmap::MappedFile::open(&path) {
            Ok(mapped) => mapped,
            Err(err) => {
                last_error = Some(format!("无法读取中文字体 {}：{err}", path.display()));
                continue;
            }
        };

        // A `.ttc` is a collection: make sure the face index we are about to use
        // actually exists, otherwise epaint would silently drop the font and the
        // UI would show tofu without any error.
        let faces = face_count(mapped.as_slice());
        if *index >= faces {
            last_error = Some(format!(
                "字体 {} 只有 {faces} 个字面，缺少索引 {index}",
                path.display()
            ));
            continue;
        }

        let data = FontData::from_blob(Arc::new(MappedBlob(mapped)), *index);
        definitions
            .font_data
            .insert(CJK_FAMILY.to_owned(), Arc::new(data));

        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = definitions.families.get_mut(&family) {
                list.push(CJK_FAMILY.to_owned());
            }
        }

        ctx.set_fonts(definitions);
        return FontInstall {
            source: Some(path.display().to_string()),
            error: None,
        };
    }

    FontInstall {
        source: None,
        error: Some(last_error.unwrap_or_else(|| {
            format!(
                "未找到系统中文（微软雅黑），中文可能显示为方块：{}",
                fonts_dir.join("msyh.ttc").display()
            )
        })),
    }
}

fn system_fonts_dir() -> Option<PathBuf> {
    let windir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let dir = windir.join("Fonts");
    dir.is_dir().then_some(dir)
}

/// Number of faces in a font file: `n` for a `.ttc` collection header, `1` for a
/// plain `.ttf`/`.otf`.
fn face_count(bytes: &[u8]) -> u32 {
    if bytes.len() >= 12 && &bytes[0..4] == b"ttcf" {
        let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        count.max(1)
    } else {
        1
    }
}

/// Apply theme + typography. Cheap enough to run whenever the config changes.
pub fn apply(ctx: &egui::Context, mode: ThemeMode, font_size: f32) {
    ctx.set_theme(match mode {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Light => ThemePreference::Light,
        ThemeMode::Dark => ThemePreference::Dark,
    });

    let font_size = font_size.clamp(9.0, 32.0);

    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            TextStyle::Body,
            FontId::proportional(font_size),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::proportional(font_size),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::proportional((font_size - 2.0).max(9.0)),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::proportional(font_size + 6.0),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::monospace((font_size - 1.0).max(8.0)),
        );

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.menu_margin = egui::Margin::same(8);
        style.interaction.selectable_labels = true;

        style.visuals.window_corner_radius = CornerRadius::same(12);
        style.visuals.menu_corner_radius = CornerRadius::same(10);
        style.visuals.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(80),
        };
        style.visuals.popup_shadow = style.visuals.window_shadow;
        style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(8);
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
        style.visuals.widgets.active.corner_radius = CornerRadius::same(8);
        style.visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(120, 160, 255));
    });
}

/// Colours for the floating card. Deliberately independent of the settings
/// window's style: the card is always a translucent "floating" surface.
#[derive(Debug, Clone, Copy)]
pub struct CardPalette {
    pub bg: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub divider: Color32,
    /// Header word colour.
    pub headword: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub shadow: Shadow,
}

pub fn card_palette(dark: bool) -> CardPalette {
    if dark {
        CardPalette {
            bg: Color32::from_rgba_unmultiplied(26, 27, 32, 243),
            border: Color32::from_rgba_unmultiplied(72, 75, 88, 180),
            text: Color32::from_rgb(233, 235, 242),
            muted: Color32::from_rgb(150, 154, 168),
            accent: Color32::from_rgba_unmultiplied(92, 132, 255, 46),
            accent_text: Color32::from_rgb(158, 186, 255),
            divider: Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            headword: Color32::from_rgb(248, 249, 253),
            danger: Color32::from_rgb(255, 138, 128),
            warn: Color32::from_rgb(240, 196, 96),
            shadow: Shadow {
                offset: [0, 8],
                blur: 22,
                spread: 1,
                color: Color32::from_black_alpha(130),
            },
        }
    } else {
        CardPalette {
            bg: Color32::from_rgba_unmultiplied(252, 252, 253, 247),
            border: Color32::from_rgba_unmultiplied(0, 0, 0, 28),
            text: Color32::from_rgb(31, 32, 38),
            muted: Color32::from_rgb(108, 112, 124),
            accent: Color32::from_rgba_unmultiplied(72, 112, 235, 30),
            accent_text: Color32::from_rgb(46, 82, 210),
            divider: Color32::from_rgba_unmultiplied(0, 0, 0, 20),
            headword: Color32::from_rgb(16, 17, 22),
            danger: Color32::from_rgb(196, 48, 42),
            warn: Color32::from_rgb(158, 110, 12),
            shadow: Shadow {
                offset: [0, 8],
                blur: 20,
                spread: 1,
                color: Color32::from_black_alpha(90),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_collection_faces() {
        let mut ttc = vec![b't', b't', b'c', b'f', 0, 1, 0, 0, 0, 0, 0, 3];
        ttc.extend_from_slice(&[0; 8]);
        assert_eq!(face_count(&ttc), 3);

        let ttf = vec![0, 1, 0, 0, 0, 0, 0, 0];
        assert_eq!(face_count(&ttf), 1);

        assert_eq!(face_count(&[]), 1);
    }

    #[test]
    fn card_palettes_are_translucent_and_distinct() {
        let dark = card_palette(true);
        let light = card_palette(false);
        assert!(dark.bg.a() < 255, "the card must be translucent");
        assert!(light.bg.a() < 255);
        assert_ne!(dark.text, light.text);
        assert_ne!(dark.bg, light.bg);
    }
}
