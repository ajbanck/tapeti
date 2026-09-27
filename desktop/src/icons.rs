// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The icon set: the SVG paths of `web/src/ui/icons.paths`, the file the web app draws
//! from, flattened into polylines and painted by egui.
//!
//! Icons are geometry rather than glyphs, so the app needs no icon font and no
//! platform can be missing one. The path language the reader knows is what the
//! file uses: M, L, H, V, A and Z, absolute and relative; an arc is sampled into
//! a polyline. The three shapes the web has no icon for (the list's collapse
//! carets and the unsaved dot, which read better solid) are filled polygons
//! written here.
//!
//! Nothing outside the macOS shortcut labels then needs a glyph above the Latin
//! block, so the emoji fonts could go (`theme::latin_only_fonts`); that saves
//! about a millisecond and is not a cold-start lever.

use egui::{vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};
use std::collections::HashMap;
use std::sync::OnceLock;

/// The shared file, one `name path` per line.
const PATHS: &str = include_str!("../../web/src/ui/icons.paths");

pub enum Icon {
    /// A path of the shared file, by name.
    Path(&'static str),
    /// Geometry of this app's own.
    Parts(&'static [Part]),
}

/// A piece of geometry the shared file has no form for, in the same 24×24 box.
pub enum Part {
    /// Filled polygon.
    Fill(&'static [(f32, f32)]),
    Dot {
        c: (f32, f32),
        r: f32,
    },
}

use Part::{Dot, Fill};

pub const FOLDER: Icon = Icon::Path("folder");
pub const SAVE: Icon = Icon::Path("save");
pub const PLAY: Icon = Icon::Path("play");
pub const STOP: Icon = Icon::Path("stop");
pub const PLUS: Icon = Icon::Path("plus");
pub const INFO: Icon = Icon::Path("info");
pub const SUN: Icon = Icon::Path("sun");
pub const MOON: Icon = Icon::Path("moon");
pub const MONITOR: Icon = Icon::Path("monitor");
pub const LOCK: Icon = Icon::Path("lock");
pub const UNLOCK: Icon = Icon::Path("unlock");
/// The tick a menu puts in front of an option that is on, drawn as geometry: the app's
/// font set has no glyph for it and egui would draw a square instead.
pub const CHECK: Icon = Icon::Path("check");
pub const X: Icon = Icon::Path("x");
pub const WAVE: Icon = Icon::Path("wave");
pub const COMPARE: Icon = Icon::Path("compare");
pub const HASH: Icon = Icon::Path("hash");
pub const LIST: Icon = Icon::Path("list");
/// The wordmark's cassette.
pub const CASSETTE: Icon = Icon::Path("cassette");
/// An arrow leaving a box: open in the emulator.
pub const LAUNCH: Icon = Icon::Path("launch");
/// Three dots: the rest of a toolbar.
pub const MORE: Icon = Icon::Path("more");
/// The editor's list-row − and ↑. Paths, not characters: egui's font set has no glyph for
/// U+2191, which shows as a hollow box on the platform that lacks it.
pub const MINUS: Icon = Icon::Path("minus");
pub const ARROW_UP: Icon = Icon::Path("arrow-up");

/// The block list's collapse toggles, where the web list has ▸ and ▾.
pub const CARET_RIGHT: Icon = Icon::Parts(&[Fill(&[(9.0, 6.0), (17.0, 12.0), (9.0, 18.0)])]);
pub const CARET_DOWN: Icon = Icon::Parts(&[Fill(&[(6.0, 9.0), (18.0, 9.0), (12.0, 17.0)])]);

/// The unsaved-changes dot in a pane's title.
pub const DOT: Icon = Icon::Parts(&[Dot { c: (12.0, 12.0), r: 4.0 }]);

/// One subpath of a flattened SVG path.
struct Poly {
    points: Vec<(f32, f32)>,
    closed: bool,
}

/// Every path of the shared file, flattened once.
fn shared() -> &'static HashMap<&'static str, Vec<Poly>> {
    static PARSED: OnceLock<HashMap<&'static str, Vec<Poly>>> = OnceLock::new();
    PARSED.get_or_init(|| {
        PATHS
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once(' '))
            .map(|(name, d)| (name, flatten(d)))
            .collect()
    })
}

/// The numbers and command letters of a path, in order.
fn tokens(d: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() {
            out.push(Token::Cmd(c));
            i += 1;
        } else if c == b'-' || c == b'.' || c.is_ascii_digit() {
            // A number ends at the next sign or at a second decimal point, so
            // `2-2` and `.5.5` are two numbers each.
            let start = i;
            i += 1;
            let mut dot = c == b'.';
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot)) {
                dot |= b[i] == b'.';
                i += 1;
            }
            out.push(Token::Num(d[start..i].parse().unwrap_or(0.0)));
        } else {
            i += 1;
        }
    }
    out
}

enum Token {
    Cmd(u8),
    Num(f32),
}

/// A path as polylines: lines as they are, arcs sampled.
fn flatten(d: &str) -> Vec<Poly> {
    let toks = tokens(d);
    let mut polys = Vec::new();
    let mut points: Vec<(f32, f32)> = Vec::new();
    let (mut cur, mut start) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    let mut cmd = b'M';
    let mut i = 0;
    let num = |i: &mut usize| -> Option<f32> {
        match toks.get(*i) {
            Some(Token::Num(v)) => {
                *i += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    let flush = |points: &mut Vec<(f32, f32)>, polys: &mut Vec<Poly>, closed: bool| {
        if points.len() >= 2 {
            polys.push(Poly { points: std::mem::take(points), closed });
        } else {
            points.clear();
        }
    };
    while i < toks.len() {
        if let Token::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else { break };
                flush(&mut points, &mut polys, false);
                cur = (base.0 + x, base.1 + y);
                start = cur;
                points.push(cur);
                // What follows a move is a line to.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else { break };
                cur = (base.0 + x, base.1 + y);
                points.push(cur);
            }
            b'H' => {
                let Some(x) = num(&mut i) else { break };
                cur = (base.0 + x, cur.1);
                points.push(cur);
            }
            b'V' => {
                let Some(y) = num(&mut i) else { break };
                cur = (cur.0, base.1 + y);
                points.push(cur);
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(_rot), Some(large), Some(sweep), Some(x), Some(y)) = (
                    num(&mut i),
                    num(&mut i),
                    num(&mut i),
                    num(&mut i),
                    num(&mut i),
                    num(&mut i),
                    num(&mut i),
                ) else {
                    break;
                };
                let to = (base.0 + x, base.1 + y);
                points.extend(arc(cur, to, rx, ry, large != 0.0, sweep != 0.0));
                cur = to;
            }
            b'Z' => {
                flush(&mut points, &mut polys, true);
                cur = start;
            }
            _ => break,
        }
    }
    flush(&mut points, &mut polys, false);
    polys
}

/// The points after `from` along an SVG arc to `to` (the endpoint parameterisation
/// of the SVG specification, without rotation), sampled every few degrees.
fn arc(from: (f32, f32), to: (f32, f32), rx: f32, ry: f32, large: bool, sweep: bool) -> Vec<(f32, f32)> {
    if from == to || rx == 0.0 || ry == 0.0 {
        return vec![to];
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let (dx, dy) = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    // Radii too small for the chord are scaled up, as the specification says.
    let lambda = dx * dx / (rx * rx) + dy * dy / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx;
    let den = rx * rx * dy * dy + ry * ry * dx * dx;
    let coef = (num.max(0.0) / den).sqrt() * if large == sweep { -1.0 } else { 1.0 };
    let (cxp, cyp) = (coef * rx * dy / ry, -coef * ry * dx / rx);
    let (cx, cy) = (cxp + (from.0 + to.0) / 2.0, cyp + (from.1 + to.1) / 2.0);
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            -a
        } else {
            a
        }
    };
    let (ux, uy) = ((dx - cxp) / rx, (dy - cyp) / ry);
    let (vx, vy) = ((-dx - cxp) / rx, (-dy - cyp) / ry);
    let theta = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let steps = ((delta.abs().to_degrees() / 12.0).ceil() as usize).max(2);
    (1..=steps)
        .map(|s| {
            let a = theta + delta * s as f32 / steps as f32;
            (cx + rx * a.cos(), cy + ry * a.sin())
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
    match icon {
        Icon::Path(name) => {
            for poly in shared().get(name).map_or(&[][..], |v| v.as_slice()) {
                let mut v: Vec<Pos2> = poly.points.iter().map(|p| at(*p)).collect();
                // A stroke too short to see is the web's round-capped dot (`h.01`).
                if v.iter().all(|p| p.distance(v[0]) < 0.25 * scale) {
                    painter.add(Shape::circle_filled(v[0], stroke.width / 2.0, colour));
                    continue;
                }
                if poly.closed {
                    v.push(v[0]);
                }
                painter.add(Shape::line(v, stroke));
            }
        }
        Icon::Parts(parts) => {
            for part in *parts {
                painter.add(match part {
                    Fill(points) => {
                        Shape::convex_polygon(points.iter().map(|p| at(*p)).collect(), colour, Stroke::NONE)
                    }
                    Dot { c, r } => Shape::circle_filled(at(*c), r * scale, colour),
                });
            }
        }
    }
}

/// A toolbar button: the icon, a hover background, and a tooltip. A disabled one is
/// dimmed and cannot be clicked, but still says what it would have done.
pub fn button(ui: &mut Ui, icon: &Icon, hover: &str, enabled: bool) -> Response {
    button_with_id(ui, ui.next_auto_id(), icon, hover, enabled)
}

/// The same button under its own id, so a headless test, not a pointer, can find and
/// click it.
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

    const ALL: &[(&str, &Icon)] = &[
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
        ("launch", &LAUNCH),
        ("more", &MORE),
        ("minus", &MINUS),
        ("arrow up", &ARROW_UP),
    ];

    /// Every icon this module names is in the shared file, and every path in the
    /// file flattens to something: a name that drifts between the two draws nothing.
    #[test]
    fn every_named_icon_is_in_the_shared_file() {
        for (label, icon) in ALL {
            if let Icon::Path(name) = icon {
                let polys = shared().get(name).unwrap_or_else(|| panic!("{label}: no path named {name}"));
                assert!(!polys.is_empty(), "{name} flattens to nothing");
            }
        }
        for (name, polys) in shared() {
            assert!(polys.iter().all(|p| p.points.len() >= 2), "{name} has a polyline of one point");
        }
    }

    /// Every icon, at the sizes the app draws them, through egui's real painter:
    /// an empty polyline or a degenerate polygon panics in tessellation.
    #[test]
    fn every_icon_paints() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for (_, icon) in ALL {
                for size in [8.0f32, 11.0, 13.0, 24.0] {
                    let rect = Rect::from_min_size(egui::Pos2::ZERO, vec2(size, size));
                    paint(ui.painter(), rect, icon, Color32::WHITE);
                }
            }
        })
        .drop_without_applying_deltas();
    }

    /// The path reader: relative and absolute commands, an implicit line after a
    /// move, a closed subpath, and numbers run together as SVG allows.
    #[test]
    fn the_path_reader_follows_the_specification() {
        let polys = flatten("M3 7h4l2 2v-2.5H3z M8 3v6h8V3");
        assert_eq!(polys.len(), 2);
        assert!(polys[0].closed && !polys[1].closed);
        assert_eq!(polys[0].points, vec![(3.0, 7.0), (7.0, 7.0), (9.0, 9.0), (9.0, 6.5), (3.0, 6.5)]);
        assert_eq!(polys[1].points, vec![(8.0, 3.0), (8.0, 9.0), (16.0, 9.0), (16.0, 3.0)]);
        assert_eq!(tokens(".5.5-1").len(), 3);
        assert_eq!(flatten("M1 1l2-2").last().unwrap().points[1], (3.0, -1.0));
    }

    /// An arc is sampled on the circle it describes, and two half arcs make a
    /// full circle: the info icon's ring, nine units round the centre.
    #[test]
    fn arcs_lie_on_their_circle() {
        let ring = &shared()["info"][0];
        assert!(ring.closed);
        assert!(ring.points.len() > 20, "a full circle is sampled finely");
        for (x, y) in &ring.points {
            let r = ((x - 12.0).powi(2) + (y - 12.0).powi(2)).sqrt();
            assert!((r - 9.0).abs() < 0.01, "({x}, {y}) is {r} from the centre");
        }
        // The sweep flag picks the side: a lock's shackle bulges upwards.
        let shackle = &shared()["lock"][1];
        assert!(shackle.points.iter().all(|(_, y)| *y <= 11.0 + 0.01));
        assert!(shackle.points.iter().any(|(_, y)| *y < 5.0));
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
