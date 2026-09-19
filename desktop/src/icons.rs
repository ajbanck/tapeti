//! The icon set, the port of `src/ui/icons.tsx`.
//!
//! The web app draws these as inline SVG "so the app has no icon-font
//! dependency"; the native shell had been drawing them as emoji, which is the
//! same dependency by another name — on a font, and on every platform shipping
//! the same glyph for 💾. Here they are geometry: the same 24×24 stroke paths,
//! transcribed into polylines, circles and arcs, painted by egui.
//!
//! It also unpins the font set: nothing outside the macOS shortcut labels needs
//! a glyph above the Latin block any more. That turned out to be worth about a
//! millisecond rather than the tens the cold-start question was after — the
//! measurement is in `--measure` —
//! so the reason to have done this is the icons themselves: one shape per
//! meaning, the same on every platform, at any size.

use egui::{pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

/// One piece of an icon, in the 24×24 box the SVG paths use.
pub enum Part {
    /// Open polyline.
    Line(&'static [(f32, f32)]),
    /// Polyline closed back to its first point.
    Closed(&'static [(f32, f32)]),
    Circle {
        c: (f32, f32),
        r: f32,
    },
    /// Stroked arc; angles in degrees, clockwise from "east", y pointing down.
    Arc {
        c: (f32, f32),
        r: f32,
        from: f32,
        to: f32,
    },
    /// Filled polygon: the carets and the dirty dot, which read better solid.
    Fill(&'static [(f32, f32)]),
    Dot {
        c: (f32, f32),
        r: f32,
    },
}

pub struct Icon(pub &'static [Part]);

use Part::{Arc, Circle, Closed, Dot, Fill, Line};

pub const FOLDER: Icon = Icon(&[Closed(&[
    (3.0, 7.0),
    (5.0, 5.0),
    (9.0, 5.0),
    (11.0, 7.0),
    (19.0, 7.0),
    (21.0, 9.0),
    (21.0, 18.0),
    (19.0, 20.0),
    (5.0, 20.0),
    (3.0, 18.0),
])]);

pub const SAVE: Icon = Icon(&[
    Closed(&[
        (5.0, 3.0),
        (16.0, 3.0),
        (20.0, 7.0),
        (20.0, 20.0),
        (19.0, 21.0),
        (5.0, 21.0),
        (4.0, 20.0),
        (4.0, 4.0),
    ]),
    Line(&[(8.0, 3.0), (8.0, 9.0), (16.0, 9.0), (16.0, 3.0)]),
    Line(&[(8.0, 21.0), (8.0, 14.0), (16.0, 14.0), (16.0, 21.0)]),
]);

pub const PLAY: Icon = Icon(&[Closed(&[(7.0, 4.0), (20.0, 12.0), (7.0, 20.0)])]);

pub const STOP: Icon = Icon(&[Closed(&[(6.0, 6.0), (18.0, 6.0), (18.0, 18.0), (6.0, 18.0)])]);

pub const PLUS: Icon = Icon(&[Line(&[(12.0, 5.0), (12.0, 19.0)]), Line(&[(5.0, 12.0), (19.0, 12.0)])]);

pub const INFO: Icon = Icon(&[
    Circle { c: (12.0, 12.0), r: 9.0 },
    Line(&[(12.0, 11.0), (12.0, 16.0)]),
    Dot { c: (12.0, 8.0), r: 1.0 },
]);

pub const SUN: Icon = Icon(&[
    Circle { c: (12.0, 12.0), r: 4.0 },
    Line(&[(12.0, 3.0), (12.0, 5.5)]),
    Line(&[(12.0, 18.5), (12.0, 21.0)]),
    Line(&[(3.0, 12.0), (5.5, 12.0)]),
    Line(&[(18.5, 12.0), (21.0, 12.0)]),
    Line(&[(5.6, 5.6), (7.4, 7.4)]),
    Line(&[(16.6, 16.6), (18.4, 18.4)]),
    Line(&[(5.6, 18.4), (7.4, 16.6)]),
    Line(&[(16.6, 7.4), (18.4, 5.6)]),
]);

/// A crescent: most of a disc, with a bite taken out of its right-hand side.
pub const MOON: Icon = Icon(&[
    Arc { c: (12.0, 12.0), r: 8.0, from: 45.0, to: 315.0 },
    Arc { c: (24.0, 12.0), r: 8.5, from: 222.0, to: 138.0 },
]);

pub const MONITOR: Icon = Icon(&[
    Closed(&[(3.0, 5.0), (21.0, 5.0), (21.0, 16.0), (3.0, 16.0)]),
    Line(&[(8.0, 20.0), (16.0, 20.0)]),
    Line(&[(12.0, 16.0), (12.0, 20.0)]),
]);

pub const LOCK: Icon = Icon(&[
    Closed(&[(6.0, 11.0), (18.0, 11.0), (18.0, 20.0), (6.0, 20.0)]),
    Line(&[(8.0, 11.0), (8.0, 8.0)]),
    Line(&[(16.0, 11.0), (16.0, 8.0)]),
    Arc { c: (12.0, 8.0), r: 4.0, from: 180.0, to: 360.0 },
]);

pub const UNLOCK: Icon = Icon(&[
    Closed(&[(6.0, 11.0), (18.0, 11.0), (18.0, 20.0), (6.0, 20.0)]),
    Line(&[(8.0, 11.0), (8.0, 8.0)]),
    Arc { c: (12.0, 8.0), r: 4.0, from: 180.0, to: 320.0 },
]);

/// The tick a menu puts in front of an option that is on. The web app draws it
/// with a `✓` in CSS; here it is geometry, because the font set the app ships
/// has no glyph for it and egui would draw a square instead.
pub const CHECK: Icon = Icon(&[Line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 6.5)])]);

pub const X: Icon = Icon(&[Line(&[(6.0, 6.0), (18.0, 18.0)]), Line(&[(18.0, 6.0), (6.0, 18.0)])]);

pub const WAVE: Icon = Icon(&[Line(&[
    (3.0, 12.0),
    (6.0, 12.0),
    (8.0, 6.0),
    (11.0, 18.0),
    (14.0, 9.0),
    (16.0, 12.0),
    (21.0, 12.0),
])]);

pub const COMPARE: Icon = Icon(&[
    Line(&[(10.0, 3.0), (10.0, 21.0)]),
    Line(&[(14.0, 3.0), (14.0, 21.0)]),
    Line(&[(3.0, 8.0), (10.0, 8.0)]),
    Line(&[(14.0, 8.0), (21.0, 8.0)]),
    Line(&[(3.0, 16.0), (10.0, 16.0)]),
    Line(&[(14.0, 16.0), (21.0, 16.0)]),
]);

pub const HASH: Icon = Icon(&[
    Line(&[(5.0, 9.0), (19.0, 9.0)]),
    Line(&[(5.0, 15.0), (19.0, 15.0)]),
    Line(&[(10.0, 3.0), (8.0, 21.0)]),
    Line(&[(16.0, 3.0), (14.0, 21.0)]),
]);

pub const LIST: Icon = Icon(&[
    Line(&[(4.0, 6.0), (6.0, 6.0)]),
    Line(&[(9.0, 6.0), (20.0, 6.0)]),
    Line(&[(4.0, 12.0), (6.0, 12.0)]),
    Line(&[(9.0, 12.0), (20.0, 12.0)]),
    Line(&[(4.0, 18.0), (6.0, 18.0)]),
    Line(&[(9.0, 18.0), (20.0, 18.0)]),
]);

/// The block list's collapse toggles, where the web list has ▸ and ▾.
pub const CARET_RIGHT: Icon = Icon(&[Fill(&[(9.0, 6.0), (17.0, 12.0), (9.0, 18.0)])]);
pub const CARET_DOWN: Icon = Icon(&[Fill(&[(6.0, 9.0), (18.0, 9.0), (12.0, 17.0)])]);

/// The unsaved-changes dot in a pane's title.
pub const DOT: Icon = Icon(&[Dot { c: (12.0, 12.0), r: 4.0 }]);

/// The wordmark's cassette, `PATHS.cassette` in `src/ui/icons.tsx`.
pub const CASSETTE: Icon = Icon(&[
    Closed(&[(2.0, 6.0), (22.0, 6.0), (22.0, 18.0), (2.0, 18.0)]),
    Circle { c: (8.0, 12.0), r: 2.0 },
    Circle { c: (16.0, 12.0), r: 2.0 },
    Line(&[(8.0, 18.0), (9.0, 15.0), (15.0, 15.0), (16.0, 18.0)]),
]);

/// The − and ↑ of the editor's list rows. The web writes them as characters;
/// here they are paths, because egui's font set has no U+2191 and a missing
/// glyph is a hollow box on the one platform that lacks it.
/// An arrow leaving a box: open this somewhere else, which is the emulator.
pub const LAUNCH: Icon = Icon(&[
    Line(&[(14.0, 4.0), (20.0, 4.0), (20.0, 10.0)]),
    Line(&[(20.0, 4.0), (11.0, 13.0)]),
    Line(&[
        (18.0, 14.0),
        (18.0, 19.0),
        (17.0, 20.0),
        (5.0, 20.0),
        (4.0, 19.0),
        (4.0, 7.0),
        (5.0, 6.0),
        (10.0, 6.0),
    ]),
]);

/// Three dots: the rest of a toolbar.
pub const MORE: Icon =
    Icon(&[Dot { c: (5.0, 12.0), r: 1.8 }, Dot { c: (12.0, 12.0), r: 1.8 }, Dot { c: (19.0, 12.0), r: 1.8 }]);

pub const MINUS: Icon = Icon(&[Line(&[(6.0, 12.0), (18.0, 12.0)])]);
pub const ARROW_UP: Icon =
    Icon(&[Line(&[(12.0, 19.0), (12.0, 5.0)]), Line(&[(6.0, 11.0), (12.0, 5.0), (18.0, 11.0)])]);

fn sample_arc(c: (f32, f32), r: f32, from: f32, to: f32) -> Vec<Pos2> {
    let sweep = to - from;
    let steps = ((sweep.abs() / 12.0).ceil() as usize).max(3);
    (0..=steps)
        .map(|i| {
            let a = (from + sweep * i as f32 / steps as f32).to_radians();
            pos2(c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// Paint `icon` into `rect`, scaled from its 24×24 box.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: &Icon, colour: Color32) {
    let size = rect.width().min(rect.height());
    let scale = size / 24.0;
    let origin = rect.center() - vec2(size, size) / 2.0;
    let at = |p: (f32, f32)| origin + vec2(p.0 * scale, p.1 * scale);
    // The web strokes at 1.8 of 24 with round caps and joins.
    let stroke = Stroke::new((1.8 * scale).max(1.0), colour);
    let line = |points: Vec<Pos2>| Shape::line(points, stroke);
    for part in icon.0 {
        let shape = match part {
            Line(points) => line(points.iter().map(|p| at(*p)).collect()),
            Closed(points) => {
                let mut v: Vec<Pos2> = points.iter().map(|p| at(*p)).collect();
                v.push(v[0]);
                line(v)
            }
            Circle { c, r } => Shape::circle_stroke(at(*c), r * scale, stroke),
            Arc { c, r, from, to } => line(
                sample_arc(*c, *r, *from, *to)
                    .into_iter()
                    .map(|p| origin + vec2(p.x * scale, p.y * scale))
                    .collect(),
            ),
            Fill(points) => {
                Shape::convex_polygon(points.iter().map(|p| at(*p)).collect(), colour, Stroke::NONE)
            }
            Dot { c, r } => Shape::circle_filled(at(*c), r * scale, colour),
        };
        painter.add(shape);
    }
}

/// A toolbar button: the icon, a hover background, and the tooltip the web
/// button carries as its `title`. A disabled one is dimmed and cannot be
/// clicked, but still says what it would have done.
pub fn button(ui: &mut Ui, icon: &Icon, hover: &str, enabled: bool) -> Response {
    button_with_id(ui, ui.next_auto_id(), icon, hover, enabled)
}

/// The same button under an id of its own, so something that is not a pointer —
/// a headless test — can find it and click it.
pub fn button_with_id(ui: &mut Ui, id: egui::Id, icon: &Icon, hover: &str, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let rect = ui.allocate_exact_size(vec2(24.0, 20.0), Sense::hover()).0;
    let response = ui.interact(rect, id, sense);
    let visuals = ui.visuals();
    let colour = if !enabled {
        visuals.weak_text_color()
    } else if response.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    if enabled && response.hovered() {
        let fill = visuals.widgets.hovered.bg_fill;
        ui.painter().rect_filled(rect, egui::CornerRadius::same(4), fill);
    }
    paint(ui.painter(), rect.shrink(3.0), icon, colour);
    response.on_hover_text(hover)
}

/// An icon drawn inline with the text of a status-bar cell or a pane title.
pub fn inline(ui: &mut Ui, icon: &Icon, colour: Color32, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui.painter(), rect, icon, colour);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon, at the sizes the app draws them, through egui's real painter:
    /// an empty polyline or a degenerate polygon panics in tessellation, and a
    /// transcription slip is easiest to make in exactly those.
    #[test]
    fn every_icon_paints() {
        let all: &[(&str, &Icon)] = &[
            ("folder", &FOLDER),
            ("save", &SAVE),
            ("play", &PLAY),
            ("stop", &STOP),
            ("plus", &PLUS),
            ("info", &INFO),
            ("sun", &SUN),
            ("moon", &MOON),
            ("monitor", &MONITOR),
            ("lock", &LOCK),
            ("unlock", &UNLOCK),
            ("check", &CHECK),
            ("hash", &HASH),
            ("x", &X),
            ("wave", &WAVE),
            ("compare", &COMPARE),
            ("list", &LIST),
            ("caret right", &CARET_RIGHT),
            ("caret down", &CARET_DOWN),
            ("dot", &DOT),
            ("cassette", &CASSETTE),
            ("minus", &MINUS),
            ("arrow up", &ARROW_UP),
        ];
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for (name, icon) in all {
                for size in [8.0f32, 11.0, 13.0, 24.0] {
                    let rect = Rect::from_min_size(egui::Pos2::ZERO, vec2(size, size));
                    paint(ui.painter(), rect, icon, Color32::WHITE);
                    assert!(!icon.0.is_empty(), "{name} has no parts");
                }
            }
        })
        .drop_without_applying_deltas();
    }

    /// A polyline needs two points and a filled polygon three, whatever the
    /// shape is meant to look like.
    #[test]
    fn the_geometry_is_well_formed() {
        for icon in [&FOLDER, &SAVE, &PLAY, &STOP, &INFO, &SUN, &MOON, &LOCK, &UNLOCK, &WAVE, &LIST] {
            for part in icon.0 {
                match part {
                    Line(p) | Closed(p) => assert!(p.len() >= 2, "a polyline needs two points"),
                    Fill(p) => assert!(p.len() >= 3, "a polygon needs three points"),
                    Circle { r, .. } | Dot { r, .. } => assert!(*r > 0.0),
                    Arc { r, from, to, .. } => {
                        assert!(*r > 0.0);
                        assert!((to - from).abs() > 1.0, "an arc needs a sweep");
                    }
                }
            }
        }
    }

    /// Dropping the emoji fonts must not drop the families the app draws with.
    #[test]
    fn the_trimmed_font_set_still_has_both_families() {
        let fonts = crate::theme::latin_only_fonts();
        assert_eq!(fonts.font_data.len(), 2, "only Hack and Ubuntu-Light survive");
        for (family, names) in &fonts.families {
            assert!(!names.is_empty(), "{family:?} was left with no font");
        }
    }
}
