//! The data window, the port of `src/ui/DataWindow.tsx`: hex, screen, BASIC,
//! variables, text and disassembly over the bytes of one block, or of several
//! viewed as one.
//!
//! The views are virtualised lists, the way they are on the web — a 64 kB block
//! is 4,096 dump rows and 20,000 disassembly lines, and egui's `show_rows` asks
//! only for the ones on screen.

use std::collections::HashSet;
use std::time::Instant;

use egui::{vec2, Align2, FontId, Rect, RichText, Sense, Ui};

use tapeti_core::bits::{
    add_bits, drop_bits, flip_bytes, join_bits, shift_left_bits, shift_right_bits, total_bits, BitData,
};
use tapeti_core::content::detect_content;
use tapeti_core::spectrum::basic::{basic_to_text, list_basic, list_variables, BasicOptions};
use tapeti_core::spectrum::charset::{dump_char, zx_char};
use tapeti_core::spectrum::screen::{has_flash, render_screen, ScreenOptions, SCREEN_SIZE};
use tapeti_core::spectrum::z80dis::{disassemble, DisOptions};
use tapeti_core::types::{Block, Body, Uid};

use crate::app::App;
use crate::files;
use crate::fmt;
use crate::state::Side;
use crate::theme::Tokens;
use crate::widgets as w;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewAs {
    Dump,
    Screen,
    Basic,
    Vars,
    Text,
    Dis,
}

const VIEWS: [(ViewAs, &str); 6] = [
    (ViewAs::Dump, "Dump"),
    (ViewAs::Screen, "Screen"),
    (ViewAs::Basic, "BASIC"),
    (ViewAs::Vars, "Variables"),
    (ViewAs::Text, "Text"),
    (ViewAs::Dis, "Disassembly"),
];

pub struct DataWin {
    pub side: Side,
    pub uids: Vec<Uid>,
    work: BitData,
    base: i64,
    view: ViewAs,
    flip: bool,
    reverse: bool,
    hide_flag: bool,
    hide_cs: bool,
    n: i64,
    dirty: bool,

    // dump
    cur: usize,
    ascii: bool,
    nibble: u8,
    pattern: String,
    ascii_pat: String,
    hits: HashSet<usize>,
    search_msg: String,
    scroll_to: Option<usize>,

    // screen
    hide_attr: bool,
    animate: bool,
    phase: bool,
    phase_at: Instant,
    tex: Option<(u64, egui::TextureHandle)>,

    // BASIC
    prog: i64,
    vars_addr: i64,
    basic: BasicOptions,

    // text
    cols: usize,
    expand_tokens: bool,

    // disassembly
    from: i64,
    rom_labels: bool,
}

fn bit_data_of(b: &Block) -> BitData {
    let used_bits = match &b.body {
        Body::Turbo { used_bits, .. } | Body::PureData { used_bits, .. } | Body::Direct { used_bits, .. } => {
            *used_bits
        }
        _ => 8,
    };
    BitData { data: b.body.data().unwrap_or(&[]).to_vec(), used_bits }
}

impl DataWin {
    pub fn new(blocks: &[Block], side: Side, uids: Vec<Uid>) -> DataWin {
        let chosen: Vec<&Block> = uids.iter().filter_map(|u| blocks.iter().find(|b| b.uid == *u)).collect();
        let single = chosen.len() == 1;
        let index = blocks.iter().position(|b| Some(b.uid) == uids.first().copied()).unwrap_or(0);
        let guess = detect_content(blocks, index);
        let work = join_bits(&chosen.iter().map(|b| bit_data_of(b)).collect::<Vec<_>>());
        let base = if single { i64::from(guess.base) } else { 0x8000 };
        let view = match guess.kind {
            tapeti_core::content::ContentKind::Screen if single => ViewAs::Screen,
            tapeti_core::content::ContentKind::Basic if single => ViewAs::Basic,
            _ => ViewAs::Dump,
        };
        DataWin {
            side,
            uids,
            work,
            base,
            view,
            flip: false,
            reverse: false,
            hide_flag: single && guess.skip_flag,
            hide_cs: single && guess.skip_checksum,
            n: 1,
            dirty: false,

            cur: 0,
            ascii: false,
            nibble: 0,
            pattern: String::new(),
            ascii_pat: String::new(),
            hits: HashSet::new(),
            search_msg: String::new(),
            scroll_to: None,
            hide_attr: false,
            animate: true,
            phase: false,
            phase_at: Instant::now(),
            tex: None,
            prog: base,
            vars_addr: guess.prog_len.map_or(-1, |l| base + i64::from(l)),
            basic: BasicOptions::default(),
            cols: 32,
            expand_tokens: true,
            from: base,
            rom_labels: true,
        }
    }

    /// Which view is showing. The tests walk all six; the window itself sets it
    /// from the tab strip.
    #[cfg(test)]
    pub fn set_view(&mut self, view: ViewAs) {
        self.view = view;
    }

    fn single(&self) -> bool {
        self.uids.len() == 1
    }

    fn modifiers_on(&self) -> bool {
        self.flip || self.reverse || self.hide_flag || self.hide_cs
    }

    /// The bytes the views show: the work buffer with the modifiers applied.
    fn view_bytes(&self) -> Vec<u8> {
        let mut d = self.work.data.clone();
        if self.hide_flag && !d.is_empty() {
            d.remove(0);
        }
        if self.hide_cs && !d.is_empty() {
            d.pop();
        }
        if self.flip {
            d = flip_bytes(&d);
        }
        if self.reverse {
            d.reverse();
        }
        d
    }

    fn start_addr(&self, len: usize) -> i64 {
        if self.reverse {
            self.base - len as i64 + 1
        } else {
            self.base
        }
    }
}

/// Draw the data window if one is open.
pub fn draw(app: &mut App, ctx: &egui::Context) {
    if app.datawin.is_none() {
        return;
    }
    let tok = app.tokens;
    let hex = app.store.hex;
    let locked = app.store.locked;
    let zero = app.store.settings.zero_based;
    let mut dw = app.datawin.take().unwrap();
    let mut close = false;
    let mut commit = false;
    let mut pick_file: Option<bool> = None;
    let mut save_file = false;

    let title = if dw.single() {
        format!(
            "Data window — block #{}",
            fmt::block_no(app.store.tape(dw.side).cursor.max(0) as usize, zero)
        )
    } else {
        format!("Data window — {} blocks viewed as one", dw.uids.len())
    };

    let response = egui::Modal::new(egui::Id::new("datawin"))
        .frame(egui::Frame::window(&ctx.global_style()).fill(tok.surface).inner_margin(egui::Margin::same(12)))
        .show(ctx, |ui| {
            let avail = ctx.content_rect().size();
            ui.set_width((avail.x - 80.0).min(1100.0));
            ui.set_max_height(avail.y - 60.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&title).size(14.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::icons::button(ui, &crate::icons::X, "Close", true).clicked() {
                        close = true;
                    }
                });
            });
            ui.separator();

            // ---- controls
            ui.horizontal(|ui| {
                for (v, label) in VIEWS {
                    if ui.selectable_label(dw.view == v, label).clicked() {
                        dw.view = v;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    w::num(ui, "dw-base", &mut dw.base, 0, 0xffff, hex, 70.0, true);
                    ui.label("Base address");
                });
            });
            let view = dw.view_bytes();
            let editable = dw.single() && !locked && !dw.modifiers_on();
            ui.horizontal(|ui| {
                let used = if dw.work.used_bits != 8 {
                    format!(" ({} bits used in last)", fmt::num(i64::from(dw.work.used_bits), hex))
                } else {
                    String::new()
                };
                w::note(ui, &tok, format!("Raw length {} bytes{used}", fmt::num(dw.work.data.len() as i64, hex)));
                w::note(ui, &tok, format!("Length {} bytes", fmt::num(view.len() as i64, hex)));
                if dw.modifiers_on() {
                    w::chip(ui, "read-only while modifiers are on", tok.warn);
                } else if !dw.single() {
                    w::chip(ui, "read-only: multiple blocks", tok.warn);
                } else if locked {
                    w::chip(ui, "locked", tok.warn);
                }
            });
            ui.horizontal(|ui| {
                w::check(ui, "Flip bytes (RR L)", &mut dw.flip, true);
                let screen = dw.view == ViewAs::Screen;
                if w::check(ui, "Reverse order (DEC IX)", &mut dw.reverse, true) && dw.reverse && screen {
                    dw.base = 0x5aff;
                }
                let single = dw.single();
                w::check(ui, "Hide flag byte", &mut dw.hide_flag, single);
                w::check(ui, "Hide checksum byte", &mut dw.hide_cs, single);
            });
            ui.separator();

            let start = dw.start_addr(view.len());
            let body_h = ui.available_height() - 110.0;
            match dw.view {
                ViewAs::Dump => dump(&mut dw, ui, &view, start, editable, hex, &tok, body_h),
                ViewAs::Screen => screen(&mut dw, ui, &view, 16384 - start, &tok, body_h),
                ViewAs::Basic => basic(&mut dw, ui, &view, start, false, &tok, body_h),
                ViewAs::Vars => basic(&mut dw, ui, &view, start, true, &tok, body_h),
                ViewAs::Text => text_view(&mut dw, ui, &view, body_h),
                ViewAs::Dis => disassembly(&mut dw, ui, &view, start, hex, &tok, body_h),
            }

            // ---- bit and byte editing
            ui.separator();
            let n = dw.n.max(1) as usize;
            ui.horizontal(|ui| {
                bit_buttons(&mut dw, ui, n, editable, 1);
                ui.label("bit(s)");
                ui.add_space(16.0);
                ui.label("Last byte mask");
                for i in 0..8u8 {
                    let mut on = i < dw.work.used_bits;
                    if ui
                        .add_enabled(editable, egui::Checkbox::without_text(&mut on))
                        .on_hover_text(format!("bit {}", 7 - i))
                        .changed()
                    {
                        dw.work.used_bits = i + 1;
                        dw.dirty = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                bit_buttons(&mut dw, ui, n, editable, 8);
                ui.label("byte(s)");
                ui.add_space(16.0);
                ui.label("N");
                w::num(ui, "dw-n", &mut dw.n, 1, 0xff_ffff, hex, 70.0, true);
            });
            w::note(
                ui,
                &tok,
                "Drop/Add act on the end of the data; Shift left removes from the start, Shift right inserts zeros at the start.",
            );

            // ---- footer
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(dw.single() && !locked, egui::Button::new("Append file")).clicked() {
                    pick_file = Some(false);
                }
                if ui.add_enabled(dw.single() && !locked, egui::Button::new("Replace from file")).clicked() {
                    pick_file = Some(true);
                }
                if ui.button("Save to file").clicked() {
                    save_file = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(if dw.dirty { "Cancel" } else { "Close" }).clicked() {
                        close = true;
                    }
                    if ui.add_enabled(dw.dirty, egui::Button::new("OK")).clicked() {
                        commit = true;
                    }
                });
            });
        });

    if response.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    if let Some(replace) = pick_file {
        if let Some((name, bytes)) = files::pick_any_file() {
            let n = bytes.len();
            if replace {
                dw.work = BitData { data: bytes, used_bits: 8 };
            } else {
                dw.work.data.extend_from_slice(&bytes);
                dw.work.used_bits = 8;
            }
            dw.dirty = true;
            app.store.set_status(format!(
                "{} {n} bytes from {name}",
                if replace { "Replaced with" } else { "Appended" }
            ));
        }
    }
    if save_file {
        let t = app.store.tape(dw.side);
        let name =
            format!("{}-block{}.bin", files::stem(&t.name), fmt::block_no(t.cursor.max(0) as usize, zero));
        if let Some(path) = files::save_bytes(&name, ("Binary", &["bin"]), &dw.view_bytes()) {
            let saved = files::file_name(&path);
            app.store.set_status(format!("Saved {saved}"));
        }
    }
    if commit {
        apply(app, &dw);
        close = true;
    }
    if !close {
        app.datawin = Some(dw);
    }
}

/// Write the edited bytes back into the block.
fn apply(app: &mut App, dw: &DataWin) {
    if !dw.dirty || !dw.single() {
        return;
    }
    let uid = dw.uids[0];
    let Some(block) = app.store.tape(dw.side).blocks.iter().find(|b| b.uid == uid) else { return };
    let mut body = block.body.clone();
    match &mut body {
        Body::Standard { data, .. } | Body::Csw { data, .. } | Body::Custom { data, .. } => {
            *data = dw.work.data.clone();
        }
        Body::Turbo { data, used_bits, .. }
        | Body::PureData { data, used_bits, .. }
        | Body::Direct { data, used_bits, .. } => {
            *data = dw.work.data.clone();
            *used_bits = dw.work.used_bits;
        }
        Body::Generalized { data, totd, data_symbols, .. } => {
            *data = dw.work.data.clone();
            if data_symbols.len() <= 2 {
                *totd = total_bits(&dw.work) as u32;
            }
        }
        _ => return,
    }
    app.store.replace_block(dw.side, uid, body);
    app.editor[dw.side].uid = None;
}

fn bit_buttons(dw: &mut DataWin, ui: &mut Ui, n: usize, editable: bool, scale: usize) {
    let n = n * scale;
    let apply = |f: fn(&BitData, usize) -> BitData, dw: &mut DataWin| {
        dw.work = f(&dw.work, n);
        dw.dirty = true;
    };
    if ui.add_enabled(editable, egui::Button::new("Drop").small()).clicked() {
        apply(drop_bits, dw);
    }
    if ui.add_enabled(editable, egui::Button::new("Add").small()).clicked() {
        apply(add_bits, dw);
    }
    if ui.add_enabled(editable, egui::Button::new("Shift left").small()).clicked() {
        apply(shift_left_bits, dw);
    }
    if ui.add_enabled(editable, egui::Button::new("Shift right").small()).clicked() {
        apply(shift_right_bits, dw);
    }
}

// ---- dump ---------------------------------------------------------------------

const DUMP_ROW_H: f32 = 18.0;

#[allow(clippy::too_many_arguments)]
fn dump(
    dw: &mut DataWin,
    ui: &mut Ui,
    data: &[u8],
    start: i64,
    editable: bool,
    hex: bool,
    tok: &Tokens,
    h: f32,
) {
    if dw.cur >= data.len() {
        dw.cur = data.len().saturating_sub(1);
    }
    keys(dw, ui, data, editable);

    let rows = data.len().div_ceil(16);
    let mut area =
        egui::ScrollArea::vertical().id_salt("dump").max_height(h - 34.0).auto_shrink([false, false]);
    if let Some(row) = dw.scroll_to.take() {
        area = area.vertical_scroll_offset(row as f32 * DUMP_ROW_H - (h - 34.0) / 2.0);
    }
    let mut click: Option<(usize, bool)> = None;
    area.show_rows(ui, DUMP_ROW_H, rows, |ui, range| {
        let painter = ui.painter().clone();
        for r in range {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), DUMP_ROW_H), Sense::hover());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let o = r * 16;
            let mono = FontId::monospace(12.0);
            let y = rect.center().y;
            painter.text(
                egui::pos2(rect.left() + 4.0, y),
                Align2::LEFT_CENTER,
                format!("{:04X}", (start + o as i64).max(0)),
                mono.clone(),
                tok.faint,
            );
            for k in 0..16usize {
                let i = o + k;
                let x = rect.left() + 50.0 + k as f32 * 21.0 + if k >= 8 { 8.0 } else { 0.0 };
                let cell = Rect::from_min_size(egui::pos2(x, rect.top()), vec2(21.0, DUMP_ROW_H));
                if i < data.len() {
                    paint_cell(
                        &painter,
                        ui,
                        cell,
                        dw,
                        i,
                        &format!("{:02X}", data[i]),
                        tok,
                        false,
                        &mut click,
                    );
                }
                let ax = rect.left() + 400.0 + k as f32 * 9.0;
                let acell = Rect::from_min_size(egui::pos2(ax, rect.top()), vec2(9.0, DUMP_ROW_H));
                if i < data.len() {
                    paint_cell(&painter, ui, acell, dw, i, &dump_char(data[i]), tok, true, &mut click);
                }
            }
        }
    });
    if let Some((i, ascii)) = click {
        dw.cur = i;
        dw.ascii = ascii;
        dw.nibble = 0;
    }
    if data.is_empty() {
        w::note(ui, tok, "No data. Use Add or Append file.");
    }

    ui.horizontal(|ui| {
        let typing = if editable {
            if dw.ascii {
                " — typing ASCII"
            } else {
                " — typing hex"
            }
        } else {
            ""
        };
        w::note(
            ui,
            tok,
            format!(
                "Cursor {} (offset {}){typing}",
                fmt::num(start + dw.cur as i64, hex),
                fmt::num(dw.cur as i64, hex)
            ),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            w::note(ui, tok, dw.search_msg.clone());
            if ui.small_button("Find next").clicked() {
                find_next(dw, data, start, hex);
            }
            let ascii =
                ui.add(egui::TextEdit::singleline(&mut dw.ascii_pat).hint_text("ASCII").desired_width(90.0));
            let pat = ui.add(
                egui::TextEdit::singleline(&mut dw.pattern)
                    .hint_text(if hex { "CD ? 05" } else { "205 ? 5" })
                    .desired_width(120.0),
            );
            ui.label("Search");
            if (ascii.lost_focus() || pat.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                find_next(dw, data, start, hex);
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn paint_cell(
    p: &egui::Painter,
    ui: &Ui,
    rect: Rect,
    dw: &DataWin,
    i: usize,
    text: &str,
    tok: &Tokens,
    ascii_col: bool,
    click: &mut Option<(usize, bool)>,
) {
    if dw.hits.contains(&i) {
        p.rect_filled(rect, egui::CornerRadius::ZERO, tok.warn.gamma_multiply(0.3));
    }
    if i == dw.cur {
        let active = dw.ascii == ascii_col;
        p.rect_filled(rect, egui::CornerRadius::ZERO, if active { tok.accent } else { tok.accent_soft_2 });
    }
    let colour = if i == dw.cur && dw.ascii == ascii_col { tok.accent_text } else { tok.text };
    p.text(rect.center(), Align2::CENTER_CENTER, text, FontId::monospace(12.0), colour);
    if ui.rect_contains_pointer(rect) && ui.input(|inp| inp.pointer.primary_pressed()) {
        *click = Some((i, ascii_col));
    }
}

fn keys(dw: &mut DataWin, ui: &mut Ui, data: &[u8], editable: bool) {
    // The search boxes and the address fields are text fields of their own: while
    // one has focus the keys are theirs, not the dump's.
    if ui.memory(|m| m.focused().is_some()) {
        return;
    }
    let events = ui.input(|i| i.events.clone());
    let mods = ui.input(|i| i.modifiers);
    if mods.command {
        return;
    }
    let move_by = |dw: &mut DataWin, d: i64| {
        let at = (dw.cur as i64 + d).clamp(0, data.len().saturating_sub(1) as i64);
        dw.cur = at as usize;
        dw.nibble = 0;
        dw.scroll_to = Some(dw.cur / 16);
    };
    for e in &events {
        match e {
            egui::Event::Key { key, pressed: true, .. } => match key {
                egui::Key::ArrowLeft => move_by(dw, -1),
                egui::Key::ArrowRight => move_by(dw, 1),
                egui::Key::ArrowUp => move_by(dw, -16),
                egui::Key::ArrowDown => move_by(dw, 16),
                egui::Key::PageUp => move_by(dw, -256),
                egui::Key::PageDown => move_by(dw, 256),
                egui::Key::Home => move_by(dw, -(data.len() as i64)),
                egui::Key::End => move_by(dw, data.len() as i64),
                egui::Key::Tab => {
                    dw.ascii = !dw.ascii;
                    dw.nibble = 0;
                }
                _ => {}
            },
            egui::Event::Text(t) if editable && !data.is_empty() => {
                let Some(c) = t.chars().next() else { continue };
                if dw.ascii {
                    set_byte(dw, dw.cur, c as u8);
                    move_by(dw, 1);
                } else if let Some(d) = c.to_digit(16) {
                    let old = dw.work.data.get(dw.cur).copied().unwrap_or(0);
                    let nv =
                        if dw.nibble == 0 { ((d as u8) << 4) | (old & 0x0f) } else { (old & 0xf0) | d as u8 };
                    set_byte(dw, dw.cur, nv);
                    if dw.nibble == 0 {
                        dw.nibble = 1;
                    } else {
                        dw.nibble = 0;
                        move_by(dw, 1);
                    }
                }
            }
            _ => {}
        }
    }
}

fn set_byte(dw: &mut DataWin, i: usize, v: u8) {
    if let Some(b) = dw.work.data.get_mut(i) {
        *b = v;
        dw.dirty = true;
    }
}

fn find_next(dw: &mut DataWin, data: &[u8], start: i64, hex: bool) {
    let mut pat: Vec<Option<u8>> = Vec::new();
    for part in dw.pattern.split([' ', ',']).filter(|s| !s.is_empty()) {
        if part == "?" || part == "??" {
            pat.push(None);
        } else {
            match fmt::parse_num(part, hex) {
                Some(v) if (0..=255).contains(&v) => pat.push(Some(v as u8)),
                _ => {
                    dw.search_msg = "Enter a search pattern".into();
                    return;
                }
            }
        }
    }
    for c in dw.ascii_pat.chars() {
        pat.push(Some(c as u8));
    }
    if pat.is_empty() {
        dw.search_msg = "Enter a search pattern".into();
        return;
    }
    for i in dw.cur + 1..data.len().saturating_sub(pat.len() - 1) {
        if pat.iter().enumerate().all(|(k, p)| p.is_none_or(|v| data[i + k] == v)) {
            dw.cur = i;
            dw.hits = (i..i + pat.len()).collect();
            dw.scroll_to = Some(i / 16);
            dw.search_msg = format!("Found at {}", fmt::num(start + i as i64, hex));
            return;
        }
    }
    dw.hits.clear();
    dw.search_msg = "Not found (searching from the cursor onward)".into();
}

// ---- screen -------------------------------------------------------------------

/// Screen bytes (0..6912) the block supplies when the screen starts at `offset`.
fn screen_overlap(len: usize, offset: i64) -> usize {
    (offset + SCREEN_SIZE as i64).min(len as i64).saturating_sub(offset.max(0)).max(0) as usize
}

fn screen(dw: &mut DataWin, ui: &mut Ui, data: &[u8], at_base: i64, tok: &Tokens, _h: f32) {
    // A base address that puts the whole block outside screen memory shows it
    // from its first byte instead of an all-black picture.
    let from_start = screen_overlap(data.len(), at_base) == 0;
    let offset = if from_start { 0 } else { at_base };
    let flashing = has_flash(data, offset);
    if flashing && dw.animate && dw.phase_at.elapsed().as_millis() >= 320 {
        dw.phase = !dw.phase;
        dw.phase_at = Instant::now();
        ui.ctx().request_repaint();
    } else if flashing && dw.animate {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(320));
    }
    let key = {
        let mut k = data.len() as u64 ^ (offset as u64) << 20;
        k = k.wrapping_mul(2).wrapping_add(u64::from(dw.hide_attr));
        k = k.wrapping_mul(2).wrapping_add(u64::from(dw.phase && dw.animate));
        for (i, b) in data.iter().enumerate().step_by(53) {
            k = k.wrapping_mul(31).wrapping_add(u64::from(*b) ^ i as u64);
        }
        k
    };
    if dw.tex.as_ref().is_none_or(|(k, _)| *k != key) {
        let px = render_screen(
            data,
            offset,
            ScreenOptions { hide_attributes: dw.hide_attr, flash_phase: dw.phase && dw.animate },
        );
        let image = egui::ColorImage::from_rgba_unmultiplied([256, 192], &px);
        let tex = ui.ctx().load_texture("screen", image, egui::TextureOptions::NEAREST);
        dw.tex = Some((key, tex));
    }
    ui.vertical_centered(|ui| {
        if let Some((_, tex)) = &dw.tex {
            ui.image((tex.id(), vec2(512.0, 384.0)));
        }
    });
    let mut save_scr = false;
    let mut save_png = false;
    ui.horizontal(|ui| {
        w::check(ui, "Hide attributes", &mut dw.hide_attr, true);
        w::check(ui, "Animate FLASH", &mut dw.animate, flashing);
        save_scr = ui.button("Save to SCR").clicked();
        save_png = ui.button("Save PNG").clicked();
    });
    let avail = screen_overlap(data.len(), offset);
    let mut note = String::new();
    if from_start {
        note.push_str(
            "No screen bytes at this base address (16384–23295); showing the block from its first byte. ",
        );
    }
    if avail < SCREEN_SIZE {
        note.push_str(&format!(
            "Only {avail} of 6912 screen bytes are present{}; missing pixels are blank and missing attributes use black ink on white paper.",
            if from_start { "" } else { " at this base address" }
        ));
    } else {
        note.push_str("Full screen present.");
    }
    w::note(ui, tok, note);

    if save_scr {
        let from = offset.max(0) as usize;
        let end = (from + SCREEN_SIZE).min(data.len());
        let bytes = data.get(from..end).unwrap_or(&[]).to_vec();
        files::save_bytes("screen.scr", ("Screen dump", &["scr"]), &bytes);
    }
    if save_png {
        let px = render_screen(
            data,
            offset,
            ScreenOptions { hide_attributes: dw.hide_attr, flash_phase: dw.phase && dw.animate },
        );
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 256, 192);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            if let Ok(mut writer) = enc.write_header() {
                let _ = writer.write_image_data(&px);
            }
        }
        files::save_bytes("screen.png", ("PNG image", &["png"]), &out);
    }
}

// ---- BASIC and variables --------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn basic(dw: &mut DataWin, ui: &mut Ui, data: &[u8], start: i64, vars: bool, tok: &Tokens, h: f32) {
    if dw.prog == 0 {
        dw.prog = start;
    }
    let prog_off = (dw.prog - start).max(0) as usize;
    let lines = list_basic(data, prog_off.min(data.len()), data.len(), dw.basic);
    let auto_vars = if dw.vars_addr >= 0 {
        dw.vars_addr
    } else {
        match lines.last() {
            Some(l) => start + i64::from(l.offset) + 4 + i64::from(l.length),
            None => dw.prog,
        }
    };
    ui.horizontal(|ui| {
        ui.label("PROG");
        w::num(ui, "dw-prog", &mut dw.prog, 0, 0xffff, false, 70.0, true);
        ui.label("VARS");
        let mut v = auto_vars;
        if w::num(ui, "dw-vars", &mut v, 0, 0xffff, false, 70.0, true) {
            dw.vars_addr = v;
        }
        if !vars {
            w::check(ui, "Show numbers", &mut dw.basic.show_numbers, true);
            w::check(ui, "Speccy formatting", &mut dw.basic.speccy_format, true);
            w::check(ui, "128k BASIC", &mut dw.basic.basic128, true);
        }
        let count = if vars {
            format!(
                "{} variable(s)",
                list_variables(data, (auto_vars - start).max(0) as usize, data.len()).len()
            )
        } else {
            format!("{} line(s)", lines.len())
        };
        w::note(ui, tok, count);
    });
    egui::ScrollArea::vertical().id_salt("basic").max_height(h - 30.0).auto_shrink([false, false]).show(
        ui,
        |ui| {
            if vars {
                let entries = list_variables(data, (auto_vars - start).max(0) as usize, data.len());
                egui::Grid::new("vars").striped(true).show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Type").strong());
                    ui.label(RichText::new("Value").strong());
                    ui.end_row();
                    for v in &entries {
                        ui.label(&v.name);
                        ui.label(&v.kind);
                        ui.label(&v.value);
                        ui.end_row();
                    }
                });
            } else {
                let shown: Vec<_> = lines
                    .iter()
                    .filter(|l| {
                        dw.vars_addr < 0 || start + i64::from(l.offset) + 4 + i64::from(l.length) <= auto_vars
                    })
                    .cloned()
                    .collect();
                let text = basic_to_text(&shown, dw.basic);
                ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).selectable(true));
            }
        },
    );
}

// ---- text ------------------------------------------------------------------------

fn text_view(dw: &mut DataWin, ui: &mut Ui, data: &[u8], h: f32) {
    ui.horizontal(|ui| {
        ui.label("Columns");
        let opts: Vec<(usize, String)> = [32usize, 64, 128].iter().map(|c| (*c, c.to_string())).collect();
        w::combo(ui, "dw-cols", &mut dw.cols, &opts, 70.0, true);
        w::check(ui, "Expand tokens", &mut dw.expand_tokens, true);
    });
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut width = 0usize;
    for c in data {
        if *c == 0x0d {
            out.push(std::mem::take(&mut line));
            width = 0;
            continue;
        }
        line.push_str(&if *c < 0x20 { "·".to_string() } else { zx_char(*c, dw.expand_tokens) });
        width += 1;
        if width >= dw.cols {
            out.push(std::mem::take(&mut line));
            width = 0;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    egui::ScrollArea::vertical().id_salt("text").max_height(h - 30.0).auto_shrink([false, false]).show(
        ui,
        |ui| {
            ui.add(egui::Label::new(RichText::new(out.join("\n")).monospace().size(12.0)).selectable(true));
        },
    );
}

// ---- disassembly ------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn disassembly(dw: &mut DataWin, ui: &mut Ui, data: &[u8], start: i64, hex: bool, tok: &Tokens, h: f32) {
    if dw.from == 0 {
        dw.from = start;
    }
    ui.horizontal(|ui| {
        ui.label("From address");
        w::num(ui, "dw-from", &mut dw.from, 0, 0xffff, false, 70.0, true);
        w::check(ui, "ROM labels", &mut dw.rom_labels, true);
    });
    let offset = (dw.from - start).max(0) as usize;
    let lines = disassemble(
        data,
        offset.min(data.len()),
        dw.from.max(0) as u32,
        1_000_000,
        DisOptions { hex, rom_labels: dw.rom_labels },
    );
    w::note(ui, tok, format!("{} instruction(s)", lines.len()));
    egui::ScrollArea::vertical().id_salt("dis").max_height(h - 50.0).auto_shrink([false, false]).show_rows(
        ui,
        DUMP_ROW_H,
        lines.len(),
        |ui, range| {
            let painter = ui.painter().clone();
            for r in range {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), DUMP_ROW_H), Sense::hover());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let l = &lines[r];
                let mono = FontId::monospace(12.0);
                let y = rect.center().y;
                painter.text(
                    egui::pos2(rect.left() + 4.0, y),
                    Align2::LEFT_CENTER,
                    format!("{:04X}", l.addr),
                    mono.clone(),
                    tok.faint,
                );
                let bytes: String = l.bytes.iter().map(|b| format!("{b:02X} ")).collect();
                painter.text(
                    egui::pos2(rect.left() + 56.0, y),
                    Align2::LEFT_CENTER,
                    bytes,
                    mono.clone(),
                    tok.muted,
                );
                let (ins, label) = match l.text.split_once("  ; ") {
                    Some((a, b)) => (a, Some(b)),
                    None => (l.text.as_str(), None),
                };
                painter.text(
                    egui::pos2(rect.left() + 160.0, y),
                    Align2::LEFT_CENTER,
                    ins,
                    mono.clone(),
                    tok.text,
                );
                if let Some(lbl) = label {
                    painter.text(
                        egui::pos2(rect.left() + 320.0, y),
                        Align2::LEFT_CENTER,
                        format!("; {lbl}"),
                        mono,
                        tok.accent,
                    );
                }
            }
        },
    );
}
