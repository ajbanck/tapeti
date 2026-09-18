//! The design tokens of `src/style.css`, both themes, plus the egui `Visuals`
//! built from them.
//!
//! The web app switches themes by setting `data-theme` on the root element and
//! letting the cascade do the rest; here the frame reads a `Tokens` and the
//! widgets that egui draws itself get a `Visuals` derived from the same values,
//! so a colour is written once.

use egui::{Color32, CornerRadius, Stroke, Visuals};

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8)
}

#[derive(Clone, Copy)]
pub struct Tokens {
    pub dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub surface_2: Color32,
    pub surface_3: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub accent_soft_2: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub ok: Color32,
    pub diff: Color32,
    pub match_: Color32,
    pub ignored: Color32,
    /// `category()` in `src/ui/TapePane.tsx`: data, signal, flow, struct, info, unknown.
    pub cat: [Color32; 6],
    /// Which icon the theme button shows: 0 sun, 1 moon, 2 follow the system.
    /// A token because the bar that draws it has no other route to the setting.
    pub theme_icon: u8,
    /// The wordmark's square. The web fills it with a blue-to-violet gradient;
    /// this is that gradient's midpoint, which is all a 26px square shows.
    pub brand: Color32,
}

pub const LIGHT: Tokens = Tokens {
    dark: false,
    bg: rgb(0xeef0f4),
    surface: rgb(0xffffff),
    surface_2: rgb(0xf6f7f9),
    surface_3: rgb(0xeceef2),
    border: rgb(0xdfe3ea),
    border_strong: rgb(0xc9cfd9),
    text: rgb(0x171b26),
    muted: rgb(0x6b7280),
    faint: rgb(0x9aa3b2),
    accent: rgb(0x2f6fed),
    accent_text: rgb(0xffffff),
    accent_soft: rgb(0xe6eefc),
    accent_soft_2: rgb(0xd3e0fb),
    danger: rgb(0xd23f3f),
    warn: rgb(0xb7791f),
    ok: rgb(0x1f9d55),
    diff: rgb(0xc026d3),
    match_: rgb(0x16a34a),
    ignored: rgb(0xa0a8b6),
    cat: [rgb(0x2f6fed), rgb(0x0e9f9f), rgb(0xd97706), rgb(0x7c3aed), rgb(0x64748b), rgb(0xb91c1c)],
    theme_icon: 2,
    brand: rgb(0x5655ed),
};

pub const DARK: Tokens = Tokens {
    dark: true,
    bg: rgb(0x0f1218),
    surface: rgb(0x171b23),
    surface_2: rgb(0x1d222c),
    surface_3: rgb(0x242a36),
    border: rgb(0x2a3140),
    border_strong: rgb(0x3a4354),
    text: rgb(0xe6e9ef),
    muted: rgb(0x9aa3b2),
    faint: rgb(0x6b7483),
    accent: rgb(0x6b9cff),
    accent_text: rgb(0x0b1220),
    // The web tokens are rgba over the surface; blended here, because a panel
    // fill is painted opaque anyway.
    accent_soft: rgb(0x222a3a),
    accent_soft_2: rgb(0x2f3d58),
    danger: rgb(0xf27171),
    warn: rgb(0xe0a83a),
    ok: rgb(0x4ade80),
    diff: rgb(0xe879f9),
    match_: rgb(0x4ade80),
    ignored: rgb(0x5e6878),
    cat: [rgb(0x6b9cff), rgb(0x2dd4bf), rgb(0xfbbf24), rgb(0xa78bfa), rgb(0x94a3b8), rgb(0xf87171)],
    theme_icon: 2,
    brand: rgb(0x5655ed),
};

impl Tokens {
    /// egui's own widget colours, so buttons, text fields and scrollbars match
    /// the tokens the app paints with.
    pub fn visuals(&self) -> Visuals {
        let mut v = if self.dark { Visuals::dark() } else { Visuals::light() };
        v.panel_fill = self.bg;
        v.window_fill = self.surface;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.extreme_bg_color = self.surface;
        v.faint_bg_color = self.surface_2;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.accent_soft_2;
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.error_fg_color = self.danger;
        v.warn_fg_color = self.warn;
        let r = CornerRadius::same(6);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
            w.bg_stroke = Stroke::new(1.0, self.border);
        }
        v.widgets.noninteractive.bg_fill = self.surface_2;
        v.widgets.noninteractive.weak_bg_fill = self.surface_2;
        v.widgets.inactive.bg_fill = self.surface_2;
        v.widgets.inactive.weak_bg_fill = self.surface_2;
        v.widgets.hovered.bg_fill = self.surface_3;
        v.widgets.hovered.weak_bg_fill = self.surface_3;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.border_strong);
        v.widgets.active.bg_fill = self.accent_soft;
        v.widgets.active.weak_bg_fill = self.accent_soft;
        v.widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        v.widgets.open.bg_fill = self.surface_3;
        v.widgets.open.weak_bg_fill = self.surface_3;
        v.window_shadow.color = Color32::from_black_alpha(if self.dark { 160 } else { 60 });
        v.popup_shadow.color = v.window_shadow.color;
        v
    }
}

/// egui's default font list minus the two emoji fonts.
///
/// Nothing the app draws needs them any more — the icons are geometry, and
/// `src/icons.rs` says why — with one exception: on macOS the shortcut labels
/// spell modifiers as ⌘⇧⌥⌃⌫, and those glyphs live in the emoji fonts.
///
/// Kept because `--measure` weighs it, and the answer is worth keeping visible:
/// about 1 ms of a 4–6 ms first frame. egui rasterises glyphs on demand, so the
/// fonts it never draws from cost almost nothing. Whatever cold start turns out
/// to be, it is not this.
pub fn latin_only_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let keep = ["Hack", "Ubuntu-Light"];
    fonts.font_data.retain(|name, _| keep.contains(&name.as_str()));
    for family in fonts.families.values_mut() {
        family.retain(|name| keep.contains(&name.as_str()));
    }
    fonts
}

/// The tokens for the chosen theme; `System` follows what egui was told about
/// the desktop's preference.
pub fn tokens(theme: crate::settings::Theme, system_dark: bool) -> Tokens {
    let mut tok = match theme {
        crate::settings::Theme::Light => LIGHT,
        crate::settings::Theme::Dark => DARK,
        crate::settings::Theme::System => {
            if system_dark {
                DARK
            } else {
                LIGHT
            }
        }
    };
    // The setting, not the colours it resolved to: "follow the system" is a
    // third state the palette cannot say.
    tok.theme_icon = match theme {
        crate::settings::Theme::Light => 0,
        crate::settings::Theme::Dark => 1,
        crate::settings::Theme::System => 2,
    };
    tok
}
