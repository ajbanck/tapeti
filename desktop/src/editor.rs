//! The per-type block editor, the port of `src/ui/BlockEditor.tsx`.
//!
//! This is the area that decided the toolkit. The form edits a *draft* `Body` —
//! a plain Rust value — and Commit replaces the block with it; in a markup
//! toolkit every one of these fields would have needed a property, a setter and
//! a callback to cross the boundary. Here the field is the data.

use std::collections::HashMap;

use egui::{RichText, Ui};

use tapeti_core::content::{block_body, detect_content, ContentInfo, ContentKind};
use tapeti_core::convert::convert_block;
use tapeti_core::describe::{checksum, decode_header, encode_header, HeaderInfo, HEADER_TYPE_NAMES};
use tapeti_core::pokes::{decode_pokes, encode_pokes, pokes_to_text, text_to_pokes};
use tapeti_core::spectrum::basic::{basic_to_text, list_basic, BasicOptions};
use tapeti_core::spectrum::screen::{render_screen, ScreenOptions};
use tapeti_core::types::{
    block_name, ArchiveEntry, Block, Body, HardwareEntry, PilotRun, RomTimings, SelectEntry, SymDef, Uid,
    CREATABLE_IDS,
};

use crate::app::App;
use crate::fmt;
use crate::state::Side;
use crate::tables;
use crate::theme::Tokens;
use crate::widgets as w;

const TSTATES_PER_SEC: f64 = 3_500_000.0;

/// Where a pane's Commit button was drawn, for the headless tests.
pub fn commit_button_id(side: Side) -> egui::Id {
    egui::Id::new(("tapeti-editor-commit", side))
}

/// What a row's trailing − and ↑ take, their spacing and the scroll area's own
/// bar included. A row that overspends this widens the `Ui` around it, and egui
/// will not let the footer shrink back afterwards.
const BUTTONS_W: f32 = 78.0;

/// A row that puts what does not fit on the next line instead of clipping it —
/// what `flex-wrap` does in the web editor. egui clips overflow and shows no
/// scrollbar, so in a narrow pane the screen thumbnail was simply swallowed:
/// half an image against the pane edge, or nothing at all.
fn wrapping_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true), add).inner
}

/// A rendered screen preview, kept until the bytes behind it change.
struct Preview {
    key: u64,
    tex: egui::TextureHandle,
}

#[derive(Default)]
pub struct EditorState {
    /// Which block the draft belongs to; cleared to force a reload.
    pub uid: Option<Uid>,
    draft: Option<Body>,
    errors: Vec<String>,
    /// Text the user is typing into a field that parses into a list.
    bufs: HashMap<&'static str, String>,
    buf_hex: bool,
    preview: Option<Preview>,
}

impl EditorState {
    fn reset(&mut self, uid: Uid, body: &Body, hex: bool) {
        self.uid = Some(uid);
        self.draft = Some(body.clone());
        self.errors.clear();
        self.bufs.clear();
        self.buf_hex = hex;
        self.preview = None;
    }
}

/// What the form asked the app to do once the borrows are over.
enum Action {
    Commit(Body),
    ViewData,
}

pub fn show(app: &mut App, ui: &mut Ui, side: Side, height: f32) {
    let tok = app.tokens;
    let hex = app.store.hex;
    let hex_bytes = app.store.settings.hex_bytes;
    let zero_based = app.store.settings.zero_based;
    let locked = app.store.locked;

    let Some((uid, body, index)) = app
        .store
        .tape(side)
        .cursor_block()
        .map(|b| (b.uid, b.body.clone(), app.store.tape(side).cursor as usize))
    else {
        ui.allocate_space(egui::vec2(ui.available_width(), height.min(40.0)));
        ui.vertical_centered(|ui| w::note(ui, &tok, "No block selected"));
        return;
    };

    let content = detect_content(&app.store.tape(side).blocks, index);
    let blocks_len = app.store.tape(side).blocks.len();
    let cursor = index;

    let ed = &mut app.editor[side];
    if ed.uid != Some(uid) {
        ed.reset(uid, &body, hex);
    }
    if ed.buf_hex != hex {
        ed.bufs.clear();
        ed.buf_hex = hex;
    }
    // The draft and its scratch state come out of the editor for the frame: the
    // form needs them mutably while it also reads the store.
    let mut draft = ed.draft.take().unwrap_or_else(|| body.clone());
    let mut errors = std::mem::take(&mut ed.errors);
    let mut bufs = std::mem::take(&mut ed.bufs);
    let mut preview = ed.preview.take();
    let dirty = draft != body;
    let mut action = None;
    let mut revert = false;
    let mut cannot_commit: Option<Vec<String>> = None;

    // The rect the pane gave us, kept aside: a form row that overflows widens
    // the `Ui` around it and `set_max_width` will not shrink it back (egui
    // unions the new max rect with the content's), so the footer is laid out
    // against this instead — never off the pane and under its neighbour.
    let full_rect = ui.max_rect();
    ui.set_min_height(height);
    ui.vertical(|ui| {
        // ---- type row. Not a wrapping row: a row of small inline widgets wraps
        // into a scattered column, and the note at the end is a hint, not content.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Block type").size(12.0).color(tok.muted));
            let unknown = draft.is_unknown();
            let mut id = draft.id();
            let options: Vec<(u8, String)> = if unknown {
                vec![(id, format!("{} (ID {id:02X})", block_name(id).unwrap_or("Unknown")))]
            } else {
                CREATABLE_IDS.iter().map(|i| (*i, block_name(*i).unwrap_or("Unknown").to_string())).collect()
            };
            if w::combo(ui, ("blocktype", side), &mut id, &options, 220.0, !unknown && !locked) {
                draft = convert_block(&Block { uid, body: draft.clone() }, id).body;
            }
            if draft.is_data_block() {
                w::note(ui, &tok, "Data blocks: edit the bytes with View data");
            }
        });
        ui.separator();

        // ---- the type's own fields
        let body_height = (height - 92.0).max(60.0);
        egui::ScrollArea::vertical().id_salt(("editor", side)).max_height(body_height).show(ui, |ui| {
            let mut f = Form {
                draft: &mut draft,
                errors: &mut errors,
                bufs: &mut bufs,
                preview: &mut preview,
                hex,
                hex_bytes,
                zero_based,
                disabled: locked,
                tok,
                cursor,
                blocks_len,
                content: &content,
                action: &mut action,
            };
            f.fields(ui);
            for e in f.errors.clone() {
                ui.colored_label(tok.danger, e);
            }
        });

        // ---- footer, pinned to the bottom of the panel: `.editor .body` is
        // `flex: 1`, so Commit and Revert are in the same place whatever the
        // form above them is.
        let foot_h = 32.0;
        let top = ui.cursor().top().max(full_rect.bottom() - foot_h);
        let foot = egui::Rect::from_min_size(
            egui::pos2(full_rect.left(), top),
            egui::vec2(full_rect.width(), (full_rect.bottom() - top).max(foot_h)),
        );
        let builder = egui::UiBuilder::new().max_rect(foot).layout(egui::Layout::top_down(egui::Align::Min));
        let mut ui = ui.new_child(builder);
        let ui = &mut ui;
        ui.separator();
        ui.horizontal(|ui| {
            if let Some(pause) = pause_of(&mut draft) {
                ui.label(RichText::new("Pause").size(12.0).color(tok.muted));
                let mut v = *pause;
                if w::num_u16(ui, ("pause", side), &mut v, hex, !locked) {
                    *pause = v;
                }
                w::note(ui, &tok, "ms after this block");
            }
            let seconds = if draft.is_unknown() {
                0.0
            } else {
                crate::tape::block_duration(&Block { uid, body: draft.clone() }) as f64 / TSTATES_PER_SEC
            };
            if seconds > 0.0 {
                ui.label(RichText::new(format!("Duration {}", fmt::duration(seconds))).size(12.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                revert = ui.add_enabled(dirty, egui::Button::new("Revert")).clicked();
                let commit = ui.add_enabled(dirty && !locked, egui::Button::new("Commit"));
                // Under an id of its own as well, so a test can ask where the
                // footer ended up without a pointer.
                ui.interact(commit.rect, commit_button_id(side), egui::Sense::hover());
                if commit.clicked() {
                    if errors.is_empty() {
                        action = Some(Action::Commit(draft.clone()));
                    } else {
                        cannot_commit = Some(errors.clone());
                    }
                }
            });
        });
    });

    if revert {
        draft = body.clone();
        errors.clear();
        bufs.clear();
    }
    let ed = &mut app.editor[side];
    ed.draft = Some(draft);
    ed.errors = errors;
    ed.bufs = bufs;
    ed.preview = preview;

    if let Some(errors) = cannot_commit {
        app.store.message("Cannot commit", errors);
    }
    match action {
        Some(Action::Commit(body)) => {
            app.store.replace_block(side, uid, body);
            app.editor[side].uid = None;
        }
        Some(Action::ViewData) => crate::actions::view_data(app, side, false),
        None => {}
    }
}

/// The pause field every timed block has, as a place to write into.
fn pause_of(body: &mut Body) -> Option<&mut u16> {
    match body {
        Body::Standard { pause, .. }
        | Body::Turbo { pause, .. }
        | Body::PureData { pause, .. }
        | Body::Direct { pause, .. }
        | Body::Csw { pause, .. }
        | Body::Generalized { pause, .. }
        | Body::Pause { pause } => Some(pause),
        _ => None,
    }
}

struct Form<'a> {
    draft: &'a mut Body,
    errors: &'a mut Vec<String>,
    bufs: &'a mut HashMap<&'static str, String>,
    preview: &'a mut Option<Preview>,
    hex: bool,
    hex_bytes: bool,
    zero_based: bool,
    disabled: bool,
    tok: Tokens,
    cursor: usize,
    blocks_len: usize,
    content: &'a ContentInfo,
    action: &'a mut Option<Action>,
}

impl Form<'_> {
    fn fields(&mut self, ui: &mut Ui) {
        let id = self.draft.id();
        if self.draft.is_unknown() {
            self.unknown(ui);
            return;
        }
        match id {
            0x10 => {
                wrapping_row(ui, |ui| {
                    let header = matches!(self.draft.data(), Some(d) if d.first().is_some_and(|f| *f < 128));
                    let pilot = if header { 8063 } else { 3223 };
                    ui.vertical(|ui| {
                        w::note(ui, &self.tok, "ROM timings");
                        w::note(ui, &self.tok, format!("pilot 2168 × {pilot}"));
                        w::note(ui, &self.tok, "sync 667 / 735");
                        w::note(ui, &self.tok, "bits 855 / 1710");
                    });
                    ui.add_space(24.0);
                    self.data_info(ui);
                    ui.add_space(24.0);
                    self.preview(ui, false);
                });
                self.header_editor(ui);
            }
            0x11 => {
                wrapping_row(ui, |ui| {
                    ui.vertical(|ui| {
                        let hex = self.hex;
                        let dis = self.disabled;
                        if let Body::Turbo {
                            pilot,
                            sync1,
                            sync2,
                            zero,
                            one,
                            pilot_len,
                            used_bits,
                            data,
                            ..
                        } = self.draft
                        {
                            w::field(ui, "Pilot pulse", &self.tok, |ui| {
                                w::num_u16(ui, "pilot", pilot, hex, !dis)
                            });
                            w::field(ui, "Pilot length", &self.tok, |ui| {
                                w::num_u16(ui, "pilotlen", pilot_len, hex, !dis)
                            });
                            w::field(ui, "Sync 1 pulse", &self.tok, |ui| {
                                w::num_u16(ui, "sync1", sync1, hex, !dis)
                            });
                            w::field(ui, "Sync 2 pulse", &self.tok, |ui| {
                                w::num_u16(ui, "sync2", sync2, hex, !dis)
                            });
                            w::field(ui, "Zero pulse", &self.tok, |ui| {
                                w::num_u16(ui, "zero", zero, hex, !dis)
                            });
                            w::field(ui, "One pulse", &self.tok, |ui| w::num_u16(ui, "one", one, hex, !dis));
                            w::field(ui, "Used bits (last byte)", &self.tok, |ui| {
                                w::num_u8(ui, "usedbits", used_bits, 1, 8, hex, !dis)
                            });
                            if ui.add_enabled(!dis, egui::Button::new("ROM timings").small()).clicked() {
                                let header = data.first().is_some_and(|f| *f < 128);
                                *pilot = RomTimings::PILOT;
                                *sync1 = RomTimings::SYNC1;
                                *sync2 = RomTimings::SYNC2;
                                *zero = RomTimings::ZERO;
                                *one = RomTimings::ONE;
                                *pilot_len =
                                    if header { RomTimings::PILOT_HEADER } else { RomTimings::PILOT_DATA };
                            }
                        }
                    });
                    ui.add_space(24.0);
                    self.data_info(ui);
                    ui.add_space(24.0);
                    self.preview(ui, false);
                });
                self.header_editor(ui);
            }
            0x14 => {
                wrapping_row(ui, |ui| {
                    ui.vertical(|ui| {
                        let (hex, dis) = (self.hex, self.disabled);
                        if let Body::PureData { zero, one, used_bits, .. } = self.draft {
                            w::field(ui, "Zero pulse", &self.tok, |ui| {
                                w::num_u16(ui, "zero", zero, hex, !dis)
                            });
                            w::field(ui, "One pulse", &self.tok, |ui| w::num_u16(ui, "one", one, hex, !dis));
                            w::field(ui, "Used bits (last byte)", &self.tok, |ui| {
                                w::num_u8(ui, "usedbits", used_bits, 1, 8, hex, !dis)
                            });
                        }
                    });
                    ui.add_space(24.0);
                    self.data_info(ui);
                    ui.add_space(24.0);
                    self.preview(ui, false);
                });
                self.header_editor(ui);
            }
            0x12 => {
                let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                if let Body::PureTone { pulse_len, count } = self.draft {
                    w::field(ui, "Pulse length", &tok, |ui| w::num_u16(ui, "pulselen", pulse_len, hex, !dis));
                    w::field(ui, "Number of pulses", &tok, |ui| w::num_u16(ui, "count", count, hex, !dis));
                    let ms = f64::from(*pulse_len) * f64::from(*count) / TSTATES_PER_SEC * 1000.0;
                    w::note(ui, &tok, format!("Duration {ms:.1} ms"));
                }
            }
            0x13 => self.pulse_list(ui),
            0x15 => {
                wrapping_row(ui, |ui| {
                    ui.vertical(|ui| {
                        let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                        if let Body::Direct { tstates, used_bits, data, .. } = self.draft {
                            w::field(ui, "T-states per sample", &tok, |ui| {
                                let mut v = i64::from(*tstates);
                                let ch = w::num(ui, "tstates", &mut v, 1, 0xffff, hex, 70.0, !dis);
                                if ch {
                                    *tstates = v as u16;
                                }
                                ch
                            });
                            w::field(ui, "Sample rate", &tok, |ui| {
                                // Always decimal, whatever the Dec/Hex switch says.
                                let mut rate =
                                    (TSTATES_PER_SEC / f64::from((*tstates).max(1))).round() as i64;
                                if w::num(ui, "samplerate", &mut rate, 1, 1_000_000, false, 70.0, !dis) {
                                    *tstates = (TSTATES_PER_SEC / rate as f64).round().max(1.0) as u16;
                                }
                            });
                            w::field(ui, "Used bits (last byte)", &tok, |ui| {
                                w::num_u8(ui, "usedbits", used_bits, 1, 8, hex, !dis)
                            });
                            let secs = ((data.len().max(1) - 1) as f64 * 8.0 + f64::from(*used_bits))
                                * f64::from(*tstates)
                                / TSTATES_PER_SEC;
                            w::note(ui, &tok, format!("{secs:.2} s"));
                        }
                    });
                    ui.add_space(24.0);
                    self.data_info(ui);
                    ui.add_space(24.0);
                    self.preview(ui, false);
                });
            }
            0x18 => {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                        if let Body::Csw { sample_rate, compression, pulse_count, .. } = self.draft {
                            w::field(ui, "Sample rate", &tok, |ui| {
                                let mut v = i64::from(*sample_rate);
                                if w::num(ui, "cswrate", &mut v, 1, 0xff_ffff, hex, 90.0, !dis) {
                                    *sample_rate = v as u32;
                                }
                            });
                            w::field(ui, "Compression", &tok, |ui| {
                                let opts = [(1u8, "RLE".to_string()), (2, "Z-RLE".to_string())];
                                w::combo(ui, "cswcomp", compression, &opts, 100.0, !dis)
                            });
                            w::field(ui, "Stored pulses", &tok, |ui| {
                                w::num_u32(ui, "cswpulses", pulse_count, 0xffff_ffff, hex, !dis)
                            });
                        }
                    });
                    ui.add_space(24.0);
                    self.data_info(ui);
                });
            }
            0x19 => self.generalized(ui),
            0x20 => w::note(
                ui,
                &self.tok,
                "A pause of 0 ms means \"stop the tape\". Set the value in the Pause field below.",
            ),
            0x21 => {
                let (dis, tok) = (self.disabled, self.tok);
                if let Body::GroupStart { name } = self.draft {
                    w::field(ui, "Group name", &tok, |ui| w::text(ui, name, 255, 300.0, !dis));
                }
            }
            0x22 | 0x25 | 0x27 | 0x2a => w::note(ui, &self.tok, "This block has no parameters."),
            0x23 => {
                let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                let (cursor, len, zero) = (self.cursor, self.blocks_len, self.zero_based);
                if let Body::Jump { offset } = self.draft {
                    w::field(ui, "Relative offset", &tok, |ui| w::num_i16(ui, "offset", offset, hex, !dis));
                    w::note(ui, &tok, target_note(cursor, i64::from(*offset), len, zero));
                }
            }
            0x24 => {
                let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                if let Body::LoopStart { count } = self.draft {
                    w::field(ui, "Repetitions", &tok, |ui| w::num_u16(ui, "loopcount", count, hex, !dis));
                }
            }
            0x26 => self.offset_list(ui),
            0x28 => self.select_entries(ui),
            0x2b => {
                let (dis, tok) = (self.disabled, self.tok);
                if let Body::SignalLevel { level } = self.draft {
                    w::field(ui, "Signal level", &tok, |ui| {
                        let opts = [(0u8, "Low".to_string()), (1, "High".to_string())];
                        w::combo(ui, "level", level, &opts, 100.0, !dis)
                    });
                }
            }
            0x30 => {
                let dis = self.disabled;
                if let Body::Text { text } = self.draft {
                    w::multiline(ui, text, 5, !dis);
                    truncate(text, 255);
                }
            }
            0x31 => {
                let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                if let Body::Message { time, text } = self.draft {
                    w::field(ui, "Display time (s)", &tok, |ui| {
                        w::num_u8(ui, "msgtime", time, 0, 255, hex, !dis)
                    });
                    w::multiline(ui, text, 4, !dis);
                    truncate(text, 255);
                }
            }
            0x32 => self.archive_entries(ui),
            0x33 => self.hardware_entries(ui),
            0x35 => self.custom(ui),
            0x5a => {
                let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
                if let Body::Glue { raw } = self.draft {
                    raw.resize(9, 0);
                    let (mut major, mut minor) = (raw[7], raw[8]);
                    w::field(ui, "Major version", &tok, |ui| {
                        if w::num_u8(ui, "gluemaj", &mut major, 0, 255, hex, !dis) {
                            raw[7] = major;
                        }
                    });
                    w::field(ui, "Minor version", &tok, |ui| {
                        if w::num_u8(ui, "gluemin", &mut minor, 0, 255, hex, !dis) {
                            raw[8] = minor;
                        }
                    });
                }
            }
            _ => {}
        }
    }

    fn unknown(&mut self, ui: &mut Ui) {
        let Body::Unknown { raw, .. } = self.draft else { return };
        ui.label(format!(
            "Raw body {} bytes. This block type is not editable; it will be written back unchanged.",
            fmt::num(raw.len() as i64, self.hex)
        ));
        let shown: String = raw.iter().take(64).map(|b| format!("{b:02x} ")).collect();
        let more = if raw.len() > 64 { "…" } else { "" };
        ui.label(RichText::new(format!("{shown}{more}")).monospace().size(11.0));
    }

    /// The info column: lengths, flag and checksum bytes, the detected content.
    fn data_info(&mut self, ui: &mut Ui) {
        let Some(data) = self.draft.data().map(<[u8]>::to_vec) else { return };
        let tok = self.tok;
        ui.vertical(|ui| {
            let id = self.draft.id();
            let spells_out = self.content.skip_flag
                && self.content.skip_checksum
                && data.len() >= 2
                && matches!(id, 0x10 | 0x11 | 0x14 | 0x19);
            if spells_out {
                ui.label(format!(
                    "Block length {} bytes: flag + {} data + checksum",
                    fmt::num(data.len() as i64, self.hex),
                    fmt::num(data.len() as i64 - 2, self.hex)
                ));
            } else {
                ui.label(format!("Data length {} bytes", fmt::num(data.len() as i64, self.hex)));
            }
            if let Some(flag) = data.first() {
                let what = match flag {
                    0 => " (header)",
                    255 => " (data)",
                    _ => "",
                };
                ui.label(format!("Flag byte {}{what}", fmt::byte(*flag, self.hex, self.hex_bytes)));
            }
            if data.len() > 1 {
                let cs = data[data.len() - 1];
                let expected = checksum(&data[..data.len() - 1]);
                ui.horizontal(|ui| {
                    ui.label(format!("Checksum byte {}", fmt::byte(cs, self.hex, self.hex_bytes)));
                    if cs == expected {
                        w::chip(ui, "valid", tok.ok);
                    } else {
                        w::chip(
                            ui,
                            &format!("expected {}", fmt::byte(expected, self.hex, self.hex_bytes)),
                            tok.danger,
                        );
                    }
                });
            }
            if !self.content.label.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Content");
                    w::chip(ui, &self.content.label, tok.accent);
                    w::note(
                        ui,
                        &tok,
                        match self.content.source {
                            tapeti_core::content::Source::Header => "from header",
                            tapeti_core::content::Source::Heuristic => "guessed",
                            tapeti_core::content::Source::None => "",
                        },
                    );
                });
            }
            if ui.button("View data").clicked() {
                *self.action = Some(Action::ViewData);
            }
        });
    }

    /// The screen or BASIC thumbnail next to the fields.
    fn preview(&mut self, ui: &mut Ui, compact: bool) {
        let Some(data) = self.draft.data() else { return };
        let body = block_body(data, self.content.skip_flag, self.content.skip_checksum);
        match self.content.kind {
            ContentKind::Screen => {
                let key = fingerprint(body);
                if self.preview.as_ref().is_none_or(|p| p.key != key) {
                    let px = render_screen(body, 0, ScreenOptions::default());
                    let image = egui::ColorImage::from_rgba_unmultiplied([256, 192], &px);
                    let tex = ui.ctx().load_texture("preview", image, egui::TextureOptions::NEAREST);
                    *self.preview = Some(Preview { key, tex });
                }
                if let Some(p) = &self.preview {
                    let scale = if compact { 0.5 } else { 0.75 };
                    ui.image((p.tex.id(), egui::vec2(256.0 * scale, 192.0 * scale)));
                }
            }
            ContentKind::Basic => {
                let opts = BasicOptions::default();
                let end = self.content.prog_len.map_or(body.len(), |l| usize::from(l).min(body.len()));
                let lines = list_basic(body, 0, end, opts);
                let shown: Vec<_> = lines.iter().take(40).cloned().collect();
                let mut text = basic_to_text(&shown, opts);
                if lines.len() > 40 {
                    text.push_str(&format!("\n… {} more line(s)", lines.len() - 40));
                }
                egui::ScrollArea::vertical()
                    .id_salt("preview")
                    .max_height(if compact { 90.0 } else { 140.0 })
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(if text.is_empty() { "(no lines)".into() } else { text })
                                .monospace()
                                .size(10.0),
                        );
                    });
            }
            _ => {}
        }
    }

    fn header_editor(&mut self, ui: &mut Ui) {
        let Some(data) = self.draft.data() else { return };
        let Some(hdr) = decode_header(data) else { return };
        let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
        let mut h = hdr.clone();
        // A header name is padded to 10 bytes; the padding is not the name, and
        // an edit has no room under the 10-character limit while it is there.
        // `encode_header` pads it again on the way back.
        h.name = h.name.trim_end().to_string();
        ui.add_space(6.0);
        w::note(ui, &tok, "Header");
        let mut changed = false;
        w::field(ui, "Type", &tok, |ui| {
            let opts: Vec<(u8, String)> =
                HEADER_TYPE_NAMES.iter().enumerate().map(|(i, n)| (i as u8, (*n).to_string())).collect();
            changed |= w::combo(ui, "hdrtype", &mut h.kind, &opts, 150.0, !dis);
        });
        w::field(ui, "Name", &tok, |ui| {
            let before = h.name.clone();
            w::text(ui, &mut h.name, 10, 110.0, !dis);
            changed |= h.name != before;
        });
        w::field(ui, "Length", &tok, |ui| changed |= w::num_u16(ui, "hdrlen", &mut h.length, hex, !dis));
        let p1 = match h.kind {
            0 => "Autostart line",
            3 => "Start address",
            _ => "Variable name",
        };
        w::field(ui, p1, &tok, |ui| changed |= w::num_u16(ui, "hdrp1", &mut h.param1, hex, !dis));
        let p2 = if h.kind == 0 { "Program length" } else { "Param 2" };
        w::field(ui, p2, &tok, |ui| changed |= w::num_u16(ui, "hdrp2", &mut h.param2, hex, !dis));
        if h.kind == 0 && h.param1 >= 32768 {
            w::note(ui, &tok, "no autostart");
        }
        if changed {
            h.type_name = HEADER_TYPE_NAMES[usize::from(h.kind.min(3))].to_string();
            set_data(self.draft, encode_header(&HeaderInfo { ..h }));
        }
    }

    // ---- list-shaped fields -------------------------------------------------

    fn pulse_list(&mut self, ui: &mut Ui) {
        let hex = self.hex;
        let seed = || match self.draft {
            Body::PulseSeq { pulses } => {
                pulses.iter().map(|p| fmt::num(i64::from(*p), hex)).collect::<Vec<_>>().join(", ")
            }
            _ => String::new(),
        };
        let tok = self.tok;
        w::note(ui, &tok, "Pulse lengths (max 255, separated by commas, semicolons or new lines)");
        let text = self.bufs.entry("pulses").or_insert_with(seed);
        let changed = w::multiline(ui, text, 5, !self.disabled).changed();
        let text = text.clone();
        if changed {
            match parse_numbers(&text, 0, 255, hex) {
                Ok(v) => {
                    self.errors.clear();
                    if let Body::PulseSeq { pulses } = self.draft {
                        *pulses = v.into_iter().map(|n| n as u16).collect();
                    }
                }
                Err(e) => *self.errors = vec![e],
            }
        }
        if let Body::PulseSeq { pulses } = self.draft {
            let n = pulses.len();
            w::note(ui, &tok, format!("{n} value(s)"));
        }
    }

    fn offset_list(&mut self, ui: &mut Ui) {
        let (hex, tok, cursor, len, zero) =
            (self.hex, self.tok, self.cursor, self.blocks_len, self.zero_based);
        let seed = || match self.draft {
            Body::Call { offsets } => {
                offsets.iter().map(|o| fmt::num(i64::from(*o), hex)).collect::<Vec<_>>().join(", ")
            }
            _ => String::new(),
        };
        w::note(ui, &tok, "Relative offsets of the called blocks, in order");
        let text = self.bufs.entry("offsets").or_insert_with(seed);
        let changed = w::multiline(ui, text, 5, !self.disabled).changed();
        let text = text.clone();
        if changed {
            match parse_numbers(&text, -0x8000, 0x7fff, hex) {
                Ok(v) => {
                    self.errors.clear();
                    if let Body::Call { offsets } = self.draft {
                        *offsets = v.into_iter().map(|n| n as i16).collect();
                    }
                }
                Err(e) => *self.errors = vec![e],
            }
        }
        if let Body::Call { offsets } = self.draft {
            let targets: Vec<String> = offsets
                .iter()
                .map(|o| {
                    let t = cursor as i64 + i64::from(*o);
                    if t >= 0 && (t as usize) < len {
                        format!("#{}", fmt::block_no(t as usize, zero))
                    } else {
                        "?".into()
                    }
                })
                .collect();
            w::note(ui, &tok, format!("{} value(s) → {}", offsets.len(), targets.join(", ")));
        }
    }

    fn select_entries(&mut self, ui: &mut Ui) {
        let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
        let Body::Select { entries } = self.draft else { return };
        let mut remove = None;
        let mut swap = None;
        for (i, e) in entries.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                w::num_i16(ui, ("seloff", i), &mut e.offset, hex, !dis);
                w::text(ui, &mut e.text, 255, 260.0, !dis);
                if w::icon_button(ui, &crate::icons::MINUS, "Remove", !dis).clicked() {
                    remove = Some(i);
                }
                if w::icon_button(ui, &crate::icons::ARROW_UP, "Move up", !dis && i > 0).clicked() {
                    swap = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            entries.remove(i);
        }
        if let Some(i) = swap {
            entries.swap(i - 1, i);
        }
        if ui.add_enabled(!dis && entries.len() < 255, egui::Button::new("+ Add").small()).clicked() {
            entries.push(SelectEntry { offset: 1, text: String::new() });
        }
        let _ = tok;
    }

    fn archive_entries(&mut self, ui: &mut Ui) {
        let (dis, tok) = (self.disabled, self.tok);
        let Body::Archive { entries } = self.draft else { return };
        let opts: Vec<(u8, String)> =
            tables::ARCHIVE_TYPES.iter().map(|(k, n)| (*k, (*n).to_string())).collect();
        let mut remove = None;
        let mut swap = None;
        for (i, e) in entries.iter_mut().enumerate() {
            ui.horizontal_top(|ui| {
                w::combo(ui, ("archtype", i), &mut e.kind, &opts, 180.0, !dis);
                let rows = if e.text.contains('\n') { 3 } else { 1 };
                // Room for the − and ↑ that follow, as the web's flex row leaves.
                let width = (ui.available_width() - BUTTONS_W).max(80.0);
                w::multiline_w(ui, &mut e.text, rows, width, !dis);
                truncate(&mut e.text, 255);
                if w::icon_button(ui, &crate::icons::MINUS, "Remove", !dis).clicked() {
                    remove = Some(i);
                }
                if w::icon_button(ui, &crate::icons::ARROW_UP, "Move up", !dis && i > 0).clicked() {
                    swap = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            entries.remove(i);
        }
        if let Some(i) = swap {
            entries.swap(i - 1, i);
        }
        if ui.add_enabled(!dis && entries.len() < 255, egui::Button::new("+ Add").small()).clicked() {
            entries.push(ArchiveEntry { kind: 0xff, text: String::new() });
        }
        let _ = tok;
    }

    fn hardware_entries(&mut self, ui: &mut Ui) {
        let dis = self.disabled;
        let Body::Hardware { entries } = self.draft else { return };
        let kinds: Vec<(u8, String)> = tables::HARDWARE_TYPES
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (i as u8, (*name).to_string()))
            .collect();
        let infos: Vec<(u8, String)> =
            tables::HARDWARE_INFO.iter().enumerate().map(|(i, n)| (i as u8, (*n).to_string())).collect();
        let mut remove = None;
        let mut swap = None;
        for (i, e) in entries.iter_mut().enumerate() {
            wrapping_row(ui, |ui| {
                if w::combo(ui, ("hwtype", i), &mut e.kind, &kinds, 170.0, !dis) {
                    e.id = 0;
                }
                let mut ids: Vec<(u8, String)> = tables::hardware_ids(e.kind)
                    .iter()
                    .enumerate()
                    .map(|(k, n)| (k as u8, (*n).to_string()))
                    .collect();
                if !ids.iter().any(|(k, _)| *k == e.id) {
                    ids.push((e.id, format!("Unknown ({})", e.id)));
                }
                w::combo(ui, ("hwid", i), &mut e.id, &ids, 230.0, !dis);
                w::combo(ui, ("hwinfo", i), &mut e.info, &infos, 210.0, !dis);
                if w::icon_button(ui, &crate::icons::MINUS, "Remove", !dis).clicked() {
                    remove = Some(i);
                }
                if w::icon_button(ui, &crate::icons::ARROW_UP, "Move up", !dis && i > 0).clicked() {
                    swap = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            entries.remove(i);
        }
        if let Some(i) = swap {
            entries.swap(i - 1, i);
        }
        if ui.add_enabled(!dis && entries.len() < 255, egui::Button::new("+ Add").small()).clicked() {
            entries.push(HardwareEntry { kind: 0, id: 1, info: 0 });
        }
    }

    // ---- generalized data block ---------------------------------------------

    fn generalized(&mut self, ui: &mut Ui) {
        let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
        let (pilot_seed, stream_seed, data_seed) = match self.draft {
            Body::Generalized { pilot_symbols, pilot_stream, data_symbols, .. } => (
                symbols_to_text(pilot_symbols, hex),
                stream_to_text(pilot_stream, hex),
                symbols_to_text(data_symbols, hex),
            ),
            _ => (String::new(), String::new(), String::new()),
        };
        wrapping_row(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width() * 0.55);
                w::note(ui, &tok, "Pilot/sync symbol table — [code]: flags; pulse1, pulse2, …");
                let text = self.bufs.entry("pilotsym").or_insert(pilot_seed);
                let changed = w::multiline(ui, text, 4, !dis).changed();
                let text = text.clone();
                if changed {
                    match text_to_symbols(&text, hex) {
                        Ok(syms) => {
                            self.errors.clear();
                            if let Body::Generalized { pilot_symbols, npp, .. } = self.draft {
                                *npp = syms.iter().map(|s| s.pulses.len()).max().unwrap_or(1).max(1) as u8;
                                *pilot_symbols = syms;
                            }
                        }
                        Err(e) => *self.errors = vec![e],
                    }
                }
                w::note(ui, &tok, "Pilot/sync stream — code, repetitions; …");
                let text = self.bufs.entry("pilotstream").or_insert(stream_seed);
                let changed = w::multiline(ui, text, 2, !dis).changed();
                let text = text.clone();
                if changed {
                    match text_to_stream(&text, hex) {
                        Ok(runs) => {
                            self.errors.clear();
                            if let Body::Generalized { pilot_stream, totp, .. } = self.draft {
                                *totp = runs.len() as u32;
                                *pilot_stream = runs;
                            }
                        }
                        Err(e) => *self.errors = vec![e],
                    }
                }
                w::note(ui, &tok, "Data symbol table — [code]: flags; pulse1, pulse2, …");
                let text = self.bufs.entry("datasym").or_insert(data_seed);
                let changed = w::multiline(ui, text, 4, !dis).changed();
                let text = text.clone();
                if changed {
                    match text_to_symbols(&text, hex) {
                        Ok(syms) => {
                            self.errors.clear();
                            if let Body::Generalized { data_symbols, npd, .. } = self.draft {
                                *npd = syms.iter().map(|s| s.pulses.len()).max().unwrap_or(1).max(1) as u8;
                                *data_symbols = syms;
                            }
                        }
                        Err(e) => *self.errors = vec![e],
                    }
                }
            });
            ui.add_space(20.0);
            ui.vertical(|ui| {
                if let Body::Generalized { pilot_symbols, data_symbols, totp, totd, .. } = self.draft {
                    let npp = pilot_symbols.iter().map(|s| s.pulses.len()).max().unwrap_or(1).max(1);
                    let npd = data_symbols.iter().map(|s| s.pulses.len()).max().unwrap_or(1).max(1);
                    ui.label(format!(
                        "NPP {}, ASP {}, TOTP {}",
                        fmt::num(npp as i64, hex),
                        fmt::num(pilot_symbols.len() as i64, hex),
                        fmt::num(i64::from(*totp), hex)
                    ));
                    ui.label(format!(
                        "NPD {}, ASD {}",
                        fmt::num(npd as i64, hex),
                        fmt::num(data_symbols.len() as i64, hex)
                    ));
                    w::field(ui, "TOTD (symbols)", &tok, |ui| {
                        w::num_u32(ui, "totd", totd, 0xffff_ffff, hex, !dis)
                    });
                }
                self.data_info(ui);
                w::note(ui, &tok, "Flags: 0 = edge, 1 = no edge, 2 = force low, 3 = force high");
            });
            ui.add_space(12.0);
            self.preview(ui, true);
        });
    }

    // ---- custom info ---------------------------------------------------------

    fn custom(&mut self, ui: &mut Ui) {
        let (hex, dis, tok) = (self.hex, self.disabled, self.tok);
        let is_pokes = matches!(self.draft, Body::Custom { ident, .. } if ident.starts_with("POKEs"));
        ui.horizontal(|ui| {
            ui.label("Identification");
            if let Body::Custom { ident, .. } = self.draft {
                let mut s = ident.trim_end().to_string();
                let before = s.clone();
                w::text(ui, &mut s, 16, 160.0, !dis);
                if s != before {
                    *ident = pad16(&s);
                }
                let mut pick = String::new();
                let opts: Vec<(&str, String)> =
                    ["", "POKEs", "Instructions", "Screen", "ZX-Edit document", "Picture", "Custom"]
                        .iter()
                        .map(|n| (*n, if n.is_empty() { "standard…".to_string() } else { (*n).to_string() }))
                        .collect();
                let mut current: &str = "";
                if w::combo(ui, "identpick", &mut current, &opts, 150.0, !dis) {
                    pick = current.to_string();
                }
                if !pick.is_empty() {
                    *ident = pad16(&pick);
                }
            }
        });
        let len = self.draft.data().map_or(0, <[u8]>::len);
        ui.horizontal(|ui| {
            ui.label(format!("Data length {} bytes", fmt::num(len as i64, hex)));
            if ui.button("View data").clicked() {
                *self.action = Some(Action::ViewData);
            }
        });
        if is_pokes {
            w::note(
                ui,
                &tok,
                "POKEs — one per line: [POKE] [page:]adr,val[/orgval]; use ? as value to ask the user. Lines starting with ; are descriptions, [name] starts a trainer.",
            );
            let decoded =
                self.draft.data().and_then(|d| decode_pokes(d).ok()).map(|info| pokes_to_text(&info, hex));
            if decoded.is_none() {
                ui.colored_label(
                    tok.danger,
                    "Existing POKEs data could not be decoded; editing will replace it.",
                );
            }
            let seed = decoded.unwrap_or_default();
            let text = self.bufs.entry("pokes").or_insert(seed);
            let changed = w::multiline(ui, text, 6, !dis).changed();
            let text = text.clone();
            if changed {
                match text_to_pokes(&text, hex) {
                    Ok(info) => {
                        self.errors.clear();
                        set_data(self.draft, encode_pokes(&info));
                    }
                    Err(e) => *self.errors = vec![e],
                }
            }
        } else if matches!(self.draft, Body::Custom { ident, .. }
            if ["Instructions", "Screen", "ZX-Edit", "Picture"].iter().any(|p| ident.starts_with(p)))
        {
            w::note(ui, &tok, "Standardized custom block; edit the bytes with View data.");
        }
    }
}

// ---- helpers -------------------------------------------------------------------

fn truncate(s: &mut String, max: usize) {
    if s.chars().count() > max {
        *s = s.chars().take(max).collect();
    }
}

fn pad16(s: &str) -> String {
    let mut out = s.to_string();
    while out.chars().count() < 16 {
        out.push(' ');
    }
    out.chars().take(16).collect()
}

fn fingerprint(d: &[u8]) -> u64 {
    // Enough to notice an edit: length plus a sparse sample of the bytes.
    let mut h = d.len() as u64;
    for (i, b) in d.iter().enumerate().step_by(97) {
        h = h.wrapping_mul(31).wrapping_add((u64::from(*b) << 8) ^ i as u64);
    }
    h
}

fn set_data(body: &mut Body, data: Vec<u8>) {
    match body {
        Body::Standard { data: d, .. }
        | Body::Turbo { data: d, .. }
        | Body::PureData { data: d, .. }
        | Body::Direct { data: d, .. }
        | Body::Csw { data: d, .. }
        | Body::Generalized { data: d, .. }
        | Body::Custom { data: d, .. } => *d = data,
        _ => {}
    }
}

fn target_note(cursor: usize, offset: i64, len: usize, zero: bool) -> String {
    let target = cursor as i64 + offset;
    if target >= 0 && (target as usize) < len {
        format!("→ block #{}", fmt::block_no(target as usize, zero))
    } else if target == len as i64 {
        "→ end of tape".into()
    } else {
        "outside the tape!".into()
    }
}

/// `NumberList`'s parser: whitespace, commas and semicolons all separate.
fn parse_numbers(text: &str, min: i64, max: i64, hex: bool) -> Result<Vec<i64>, String> {
    let parts: Vec<&str> = text.split([' ', '\t', '\r', '\n', ',', ';']).filter(|s| !s.is_empty()).collect();
    let mut out = Vec::with_capacity(parts.len());
    let mut bad = Vec::new();
    for p in parts {
        match fmt::parse_num(p, hex) {
            Some(v) if v >= min && v <= max => out.push(v),
            _ => bad.push(p),
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(format!("Invalid values: {}", bad.join(", ")))
    }
}

fn symbols_to_text(syms: &[SymDef], hex: bool) -> String {
    syms.iter()
        .enumerate()
        .map(|(i, s)| {
            let pulses: Vec<String> = s.pulses.iter().map(|p| fmt::num(i64::from(*p), hex)).collect();
            format!(
                "{}: {}; {}",
                fmt::num(i as i64, hex),
                fmt::num(i64::from(s.flags), hex),
                pulses.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_to_symbols(text: &str, hex: bool) -> Result<Vec<SymDef>, String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let body = match line.find(':') {
            Some(at) => &line[at + 1..],
            None => line,
        };
        let parts: Vec<&str> = body.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err(format!("Symbol line \"{raw}\" has no flags"));
        }
        let nums: Option<Vec<i64>> = parts.iter().map(|p| fmt::parse_num(p, hex)).collect();
        let nums = nums.ok_or_else(|| format!("Symbol line \"{raw}\" contains a bad number"))?;
        if nums[0] < 0 || nums[0] > 3 {
            return Err(format!("Symbol flags must be 0-3 in \"{raw}\""));
        }
        out.push(SymDef { flags: nums[0] as u8, pulses: nums[1..].iter().map(|n| *n as u16).collect() });
    }
    if out.len() > 256 {
        return Err("At most 256 symbols are allowed".into());
    }
    Ok(out)
}

fn stream_to_text(runs: &[PilotRun], hex: bool) -> String {
    runs.iter()
        .map(|r| format!("{}, {}", fmt::num(i64::from(r.symbol), hex), fmt::num(i64::from(r.reps), hex)))
        .collect::<Vec<_>>()
        .join("; ")
}

fn text_to_stream(text: &str, hex: bool) -> Result<Vec<PilotRun>, String> {
    let nums =
        parse_numbers(text, 0, 0xffff, hex).map_err(|_| "Pilot stream contains a bad number".to_string())?;
    if nums.len() % 2 != 0 {
        return Err("Pilot stream must be pairs of symbol, repetitions".into());
    }
    Ok(nums.chunks(2).map(|c| PilotRun { symbol: c[0] as u8, reps: c[1] as u16 }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_tables_round_trip() {
        let syms = vec![SymDef { flags: 0, pulses: vec![855, 855] }, SymDef { flags: 2, pulses: vec![1710] }];
        let text = symbols_to_text(&syms, false);
        assert_eq!(text_to_symbols(&text, false).unwrap(), syms);
    }

    #[test]
    fn pilot_streams_round_trip() {
        let runs = vec![PilotRun { symbol: 0, reps: 8063 }, PilotRun { symbol: 1, reps: 1 }];
        let text = stream_to_text(&runs, false);
        assert_eq!(text_to_stream(&text, false).unwrap(), runs);
    }

    #[test]
    fn rejects_bad_symbol_lines() {
        assert!(text_to_symbols("0: 9; 855", false).is_err());
        assert!(text_to_symbols("0: zz", false).is_err());
    }

    #[test]
    fn identification_is_padded_to_sixteen() {
        assert_eq!(pad16("POKEs").len(), 16);
        assert_eq!(pad16("0123456789abcdefGH").len(), 16);
    }
}
