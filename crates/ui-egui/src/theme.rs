//! Design tokens (Illustrator's four UI brightness levels), fonts and egui style.

use std::sync::Arc;

use craft_ui::theme::SuitePalette;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Brightness {
    Dark,
    #[default]
    MediumDark,
    MediumLight,
    Light,
}

impl Brightness {
    pub const ALL: [Brightness; 4] = [Brightness::Dark, Brightness::MediumDark, Brightness::MediumLight, Brightness::Light];
    pub fn label(self) -> &'static str {
        match self {
            Brightness::Dark => "Dark",
            Brightness::MediumDark => "Medium Dark",
            Brightness::MediumLight => "Medium Light",
            Brightness::Light => "Light",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Brightness::Dark => "dark",
            Brightness::MediumDark => "mediumDark",
            Brightness::MediumLight => "mediumLight",
            Brightness::Light => "light",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id().eq_ignore_ascii_case(s) || b.label().eq_ignore_ascii_case(s))
    }
}

/// All colours and metrics the UI uses. Never hard-code a colour in a widget.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    pub app_bar: Color32,
    pub panel: Color32,
    pub panel_darker: Color32,
    pub input: Color32,
    pub input_border: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub tool_active: Color32,
    pub accent: Color32,
    pub accent_strong: Color32,
    pub on_accent: Color32,
    pub row_selected: Color32,
    pub pasteboard: Color32,
    pub ruler: Color32,
    pub ruler_tick: Color32,
    pub smart_guide: Color32,
    pub guide: Color32,
    pub measure_bg: Color32,
    pub button: Color32,
    pub radius: u8,
    /// Field values, button labels, link labels.
    pub text_strong: Color32,
    /// Panel tab strips, doc-tab bar, dock header.
    pub tab_strip: Color32,
    /// Outline of secondary (outlined) buttons.
    pub button_border: Color32,
    /// Default Layer 1 selection colour and selected-anchor fill.
    pub selection: Color32,
    pub anchor_selected: Color32,
    /// Active item text in tool flyouts.
    pub flyout_active: Color32,
    /// Hint-bar / tab-bar heights etc. scale with this base font size.
    pub font_size: f32,
    /// Window Close button (custom title bar) when hovered, and its glyph there.
    pub caption_close: Color32,
    pub caption_close_text: Color32,
    /// What preview images mark (Flattener Preview highlights).
    pub highlight: Color32,
    /// The bleed outline around artboards (Document Setup → Bleed).
    pub bleed: Color32,
    /// Error marks (a missing linked file).
    pub error: Color32,
    /// Warning marks (a modified linked file).
    pub warning: Color32,
    /// Rotation gizmo axes, also labelled X, Y and Z.
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
}

impl Tokens {
    /// Canvas editing cues keep their original colour independently of suite UI chrome.
    pub(crate) fn editing_accent(&self) -> Color32 {
        if self.accent == SuitePalette::ARTCRAFT_DARK.accent { Color32::from_rgb(55, 142, 240) } else { self.accent }
    }
    pub fn for_brightness(b: Brightness) -> Self {
        let hex = |s: u32| Color32::from_rgb((s >> 16) as u8, (s >> 8) as u8, s as u8);
        let base = Tokens {
            dark: true,
            app_bar: hex(0x262626),
            panel: hex(0x323232),
            panel_darker: hex(0x282828),
            input: hex(0x1f1f1f),
            input_border: hex(0x464646),
            border: hex(0x1b1b1b),
            divider: hex(0x3f3f3f),
            text: hex(0xd8d8d8),
            text_dim: hex(0xa8a8a8),
            text_disabled: hex(0x6e6e6e),
            icon: hex(0xcfcfcf),
            hover: hex(0x444444),
            tool_active: hex(0x1d1d1d),
            accent: hex(0x378ef0),
            accent_strong: hex(0x1473e6),
            on_accent: Color32::WHITE,
            row_selected: hex(0x484b50),
            pasteboard: hex(0x1e1e1e),
            ruler: hex(0x2f2f2f),
            ruler_tick: hex(0x8a8a8a),
            smart_guide: hex(0xff3dfc),
            guide: hex(0x4affff),
            measure_bg: Color32::from_rgba_unmultiplied(92, 92, 92, 235),
            button: hex(0x444444),
            radius: 2,
            text_strong: hex(0xf5f5f5),
            tab_strip: hex(0x282828),
            button_border: hex(0x6a6a6a),
            selection: hex(0x4f80ff),
            anchor_selected: hex(0x3d82ff),
            flyout_active: hex(0x54b2f7),
            font_size: 13.0,
            caption_close: hex(0xc42b1c),
            caption_close_text: Color32::WHITE,
            highlight: hex(0xff3030),
            bleed: hex(0xf03030),
            error: hex(0xe34850),
            warning: hex(0xf0a330),
            axis_x: hex(0xef6058),
            axis_y: hex(0x45af63),
            axis_z: hex(0x378ef0),
        };
        let suite = SuitePalette::ARTCRAFT_DARK;
        match b {
            // ArtCraft's first-party dark UI roles. Keep the measured brightness presets
            // inheriting the original base below, so their colors stay independent.
            Brightness::Dark => Tokens {
                app_bar: suite.chrome,
                panel: suite.panel,
                panel_darker: suite.window,
                input: suite.input,
                input_border: suite.border,
                border: suite.border,
                divider: suite.border,
                text: suite.text,
                text_dim: suite.text_secondary,
                text_disabled: suite.text_disabled,
                icon: suite.text,
                hover: suite.hover,
                tool_active: suite.pressed,
                accent: suite.accent,
                accent_strong: suite.accent_hover,
                on_accent: suite.on_accent,
                row_selected: suite.selection,
                button: suite.raised,
                text_strong: suite.text_strong,
                tab_strip: suite.chrome,
                button_border: suite.border_strong,
                ..base
            },
            // Measured from Illustrator 2026 (plan/illustrator/10-observed-ui.md §1).
            Brightness::MediumDark => Tokens {
                app_bar: hex(0x535353),
                panel: hex(0x535353),
                panel_darker: hex(0x424242),
                tab_strip: hex(0x424242),
                input: hex(0x454545),
                input_border: hex(0x5f5f5f),
                border: hex(0x383838),
                divider: hex(0x4b4b4b),
                text: hex(0xd1d1d1),
                text_strong: hex(0xffffff),
                text_dim: hex(0xb0b0b0),
                text_disabled: hex(0x7a7a7a),
                icon: hex(0xc2c2c2),
                hover: hex(0x606060),
                tool_active: hex(0x303030),
                row_selected: hex(0x52677c),
                pasteboard: hex(0x606060),
                ruler: hex(0x333333),
                ruler_tick: hex(0x8a8a8a),
                button: hex(0x535353),
                button_border: hex(0x747474),
                ..base
            },
            Brightness::MediumLight => Tokens {
                tab_strip: hex(0xa6a6a6),
                text_strong: hex(0x000000),
                button_border: hex(0x7a7a7a),
                dark: false,
                app_bar: hex(0xa9a9a9),
                panel: hex(0xb8b8b8),
                panel_darker: hex(0xaaaaaa),
                input: hex(0xd6d6d6),
                input_border: hex(0x8c8c8c),
                border: hex(0x969696),
                divider: hex(0x9e9e9e),
                text: hex(0x1f1f1f),
                text_dim: hex(0x3c3c3c),
                text_disabled: hex(0x7c7c7c),
                icon: hex(0x2a2a2a),
                hover: hex(0xc8c8c8),
                tool_active: hex(0x969696),
                row_selected: hex(0x9fb4d6),
                pasteboard: hex(0xa0a0a0),
                ruler: hex(0xc2c2c2),
                ruler_tick: hex(0x4a4a4a),
                button: hex(0xcacaca),
                ..base
            },
            Brightness::Light => Tokens {
                tab_strip: hex(0xdddddd),
                text_strong: hex(0x000000),
                button_border: hex(0x9a9a9a),
                dark: false,
                app_bar: hex(0xe4e4e4),
                panel: hex(0xf0f0f0),
                panel_darker: hex(0xe2e2e2),
                input: hex(0xffffff),
                input_border: hex(0xb4b4b4),
                border: hex(0xcfcfcf),
                divider: hex(0xd6d6d6),
                text: hex(0x1f1f1f),
                text_dim: hex(0x4b4b4b),
                text_disabled: hex(0x9a9a9a),
                icon: hex(0x303030),
                hover: hex(0xdddddd),
                tool_active: hex(0xcdcdcd),
                row_selected: hex(0xc8daf7),
                pasteboard: hex(0xdcdcdc),
                ruler: hex(0xeaeaea),
                ruler_tick: hex(0x5a5a5a),
                button: hex(0xe0e0e0),
                ..base
            },
        }
    }

    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::NULL)).unwrap_or_else(|| Tokens::for_brightness(Brightness::Dark))
    }
    pub fn cr(&self) -> CornerRadius {
        CornerRadius::same(self.radius)
    }
}

/// Selected text in the text fields `ui` draws from here on is in the strong text colour. egui
/// paints it in the selection stroke's colour, the accent, which is hard to read on the selection's
/// accent tint (#1465). For fields that draw their own frame: egui's frame takes its focus ring from
/// the same stroke.
pub fn strong_selected_text(ui: &mut egui::Ui) {
    ui.visuals_mut().selection.stroke.color = Tokens::get(ui.ctx()).text_strong;
}

pub const FONT_UI: &str = "ui";
pub const FONT_UI_SEMIBOLD: &str = "ui-semibold";
pub const FONT_MONO: &str = "mono";

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_UI_SEMIBOLD.into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_MONO.into()))
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |f: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        f.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "SourceSans3", include_bytes!("../../../assets/fonts/SourceSans3-Regular.ttf"));
    add(&mut fonts, "SourceSans3-Semibold", include_bytes!("../../../assets/fonts/SourceSans3-Semibold.ttf"));
    add(&mut fonts, "JetBrainsMono", include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"));
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["SourceSans3".to_string()];
    prop.extend(fallback.clone());
    fonts.families.insert(FontFamily::Proportional, prop);
    let mut semi = vec!["SourceSans3-Semibold".to_string()];
    semi.extend(fallback.clone());
    fonts.families.insert(FontFamily::Name(FONT_UI_SEMIBOLD.into()), semi);
    let mut ui = vec!["SourceSans3".to_string()];
    ui.extend(fallback.clone());
    fonts.families.insert(FontFamily::Name(FONT_UI.into()), ui);
    let mut mono = vec!["JetBrainsMono".to_string()];
    mono.extend(fallback);
    fonts.families.insert(FontFamily::Name(FONT_MONO.into()), mono);
    add_craft_fonts(&mut fonts);
    ctx.set_fonts(fonts);
}

/// The egui name of a craft-fonts face.
pub(crate) fn craft_font_name(f: &vectorcraft_text::CraftFont) -> String {
    format!("craft-fonts {} {}", f.family, f.style)
}

/// The Japanese craft-fonts faces (when built with `CRAFT_FONTS_DIR`) as fallbacks at the end of
/// every family, after the app's own fonts and before the installed fonts `ui_fonts` adds: BIZ
/// UDPGothic first (bold first in the semibold family), then the Mincho faces.
fn add_craft_fonts(fonts: &mut FontDefinitions) {
    for f in vectorcraft_text::CRAFT_FONTS.iter().filter(|f| f.is_japanese()) {
        fonts.font_data.insert(craft_font_name(f), Arc::new(FontData::from_static(f.bytes)));
    }
    for (family, stack) in fonts.families.iter_mut() {
        let bold = *family == FontFamily::Name(FONT_UI_SEMIBOLD.into());
        stack.extend(vectorcraft_text::craft_fonts::japanese_ui_fonts(bold).into_iter().map(craft_font_name));
    }
}

/// Apply tokens to egui's global style.
pub fn apply(ctx: &egui::Context, b: Brightness) {
    let t = Tokens::for_brightness(b);
    let suite = SuitePalette::ARTCRAFT_DARK;
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, t));
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.extreme_bg_color = t.input;
    v.faint_bg_color = t.panel_darker;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = if b == Brightness::Dark { t.row_selected } else { t.accent.gamma_multiply(0.55) };
    v.selection.stroke = Stroke::new(1.0, if b == Brightness::Dark { suite.selection_text } else { t.accent });
    v.hyperlink_color = t.accent;
    v.override_text_color = Some(t.text);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if t.dark { 120 } else { 60 }) };
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if t.dark { 140 } else { 70 }) };
    let w = &mut v.widgets;
    craft_ui::theme::WidgetPalette { noninteractive: t.panel, inactive: t.button, hovered: t.hover, active: t.tool_active, open: t.hover }.apply_to(
        w,
        t.cr(),
        Stroke::new(1.0, t.text),
    );
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::new(1.0, t.input_border);
    w.active.bg_stroke = Stroke::new(1.0, if b == Brightness::Dark { suite.focus } else { t.accent });
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(6.0, 2.0);
        s.spacing.interact_size = egui::vec2(24.0, 24.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.slider_width = 120.0;
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, FontId::new(15.0, FontFamily::Name(FONT_UI_SEMIBOLD.into()))),
            (TextStyle::Monospace, FontId::new(11.5, FontFamily::Name(FONT_MONO.into()))),
        ]
        .into();
        s.animation_time = 0.08;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_dark_maps_shared_ui_roles_and_preserves_artwork() {
        let suite = SuitePalette::ARTCRAFT_DARK;
        let dark = Tokens::for_brightness(Brightness::Dark);
        for (actual, expected) in [
            (dark.app_bar, suite.chrome),
            (dark.panel, suite.panel),
            (dark.panel_darker, suite.window),
            (dark.input, suite.input),
            (dark.input_border, suite.border),
            (dark.border, suite.border),
            (dark.divider, suite.border),
            (dark.text, suite.text),
            (dark.text_dim, suite.text_secondary),
            (dark.text_disabled, suite.text_disabled),
            (dark.icon, suite.text),
            (dark.hover, suite.hover),
            (dark.tool_active, suite.pressed),
            (dark.accent, suite.accent),
            (dark.accent_strong, suite.accent_hover),
            (dark.on_accent, suite.on_accent),
            (dark.row_selected, suite.selection),
            (dark.button, suite.raised),
            (dark.text_strong, suite.text_strong),
            (dark.tab_strip, suite.chrome),
            (dark.button_border, suite.border_strong),
        ] {
            assert_eq!(actual, expected);
        }
        let hex = |s: u32| Color32::from_rgb((s >> 16) as u8, (s >> 8) as u8, s as u8);
        for (actual, expected) in [
            (dark.pasteboard, 0x1e1e1e),
            (dark.ruler, 0x2f2f2f),
            (dark.ruler_tick, 0x8a8a8a),
            (dark.smart_guide, 0xff3dfc),
            (dark.guide, 0x4affff),
            (dark.selection, 0x4f80ff),
            (dark.anchor_selected, 0x3d82ff),
            (dark.highlight, 0xff3030),
            (dark.bleed, 0xf03030),
            (dark.error, 0xe34850),
            (dark.warning, 0xf0a330),
            (dark.axis_x, 0xef6058),
            (dark.axis_y, 0x45af63),
            (dark.axis_z, 0x378ef0),
        ] {
            assert_eq!(actual, hex(expected));
        }
        assert_eq!(dark.measure_bg, Color32::from_rgba_unmultiplied(92, 92, 92, 235));
        assert_eq!(dark.editing_accent(), hex(0x378ef0));
        assert_eq!(crate::state::UiState::default().brightness, Brightness::Dark);
        let ctx = egui::Context::default();
        apply(&ctx, Brightness::Dark);
        let visuals = ctx.style_of(ctx.theme()).visuals.clone();
        assert_eq!(visuals.selection.bg_fill, suite.selection);
        assert_eq!(visuals.selection.stroke.color, suite.selection_text);
        assert_eq!(visuals.widgets.active.bg_stroke, Stroke::new(1.0, suite.focus));
        assert_eq!(visuals.widgets.inactive.bg_stroke, Stroke::NONE);
        assert_eq!(visuals.widgets.inactive.bg_fill, suite.raised);
        assert_eq!(visuals.widgets.hovered.bg_fill, suite.hover);
        assert_eq!(visuals.widgets.active.bg_fill, suite.pressed);
    }

    #[test]
    fn alternate_brightness_preserves_local_palette_and_selection_styles() {
        let hex = |s: u32| Color32::from_rgb((s >> 16) as u8, (s >> 8) as u8, s as u8);
        for (brightness, panel, input, text) in [
            (Brightness::MediumDark, 0x535353, 0x454545, 0xd1d1d1),
            (Brightness::MediumLight, 0xb8b8b8, 0xd6d6d6, 0x1f1f1f),
            (Brightness::Light, 0xf0f0f0, 0xffffff, 0x1f1f1f),
        ] {
            let t = Tokens::for_brightness(brightness);
            assert_eq!(t.panel, hex(panel), "{brightness:?}");
            assert_eq!(t.input, hex(input), "{brightness:?}");
            assert_eq!(t.text, hex(text), "{brightness:?}");
            assert_eq!(t.accent, hex(0x378ef0), "{brightness:?}");
            assert_eq!(t.editing_accent(), t.accent, "{brightness:?}");
            assert_eq!(t.on_accent, Color32::WHITE, "{brightness:?}");
            let ctx = egui::Context::default();
            apply(&ctx, brightness);
            let visuals = ctx.style_of(ctx.theme()).visuals.clone();
            assert_eq!(visuals.panel_fill, t.panel, "{brightness:?}");
            assert_eq!(visuals.selection.bg_fill, t.accent.gamma_multiply(0.55), "{brightness:?}");
            assert_eq!(visuals.selection.stroke.color, t.accent, "{brightness:?}");
            assert_eq!(visuals.widgets.active.bg_stroke, Stroke::new(1.0, t.accent), "{brightness:?}");
            assert_eq!(visuals.widgets.inactive.bg_stroke, Stroke::NONE, "{brightness:?}");
        }
    }

    #[test]
    fn brightness_roundtrip() {
        for b in Brightness::ALL {
            assert_eq!(Brightness::parse(b.id()), Some(b));
            assert_eq!(Brightness::parse(b.label()), Some(b));
        }
        assert!(!Tokens::for_brightness(Brightness::Light).dark);
    }

    /// #1662: a slider's rail (`button_border`, see `widgets::slider`) stands out from the panel
    /// in every brightness.
    #[test]
    fn slider_rails_show_on_every_panel() {
        for b in [Brightness::Dark, Brightness::MediumDark, Brightness::MediumLight, Brightness::Light] {
            let t = Tokens::for_brightness(b);
            let gap = (i32::from(t.button_border.r()) - i32::from(t.panel.r())).abs();
            assert!(gap >= 24, "{b:?}: rail {:?} on panel {:?}", t.button_border, t.panel);
        }
    }
}
