//! The block list, the port of `src/ui/TapePane.tsx`.
//!
//! The rows are built from the core once per version of the tape, not once per
//! frame: `describe_block` over 3,000 blocks is 0.9 ms in process and a frame is
//! 16. What the frame does is decide which of them are visible (a collapsed
//! group hides its body) and paint the ones on screen.
//!
//! Drag and drop is the one place where immediate mode shows: there is no
//! `dataTransfer`, so the drag lives in the app struct and every pane looks at
//! it, which is also what makes dragging *between* the two panes fall out for
//! free.

use std::collections::HashMap;

use egui::{pos2, vec2, Align2, CornerRadius, FontId, Id, Rect, Sense, Stroke, Ui};

use tapeti_core::consistency::{check_consistency, Issue, Severity};
use tapeti_core::content::content_labels;
use tapeti_core::describe::{block_length, describe_block, fmt as core_fmt, is_metadata};
use tapeti_core::types::{block_name, Block, Body};

use crate::app::{App, Drag};
use crate::commands;
use crate::fmt;
use crate::icons;
use crate::state::{Mark, SelectMode, Side};
use crate::theme::Tokens;

/// `.blocklist .row { height: 26px }` in `style.css`.
pub const ROW_H: f32 = 26.0;

pub struct Row {
    pub no: String,
    pub id: String,
    pub desc: String,
    pub kind: String,
    pub len: String,
    pub depth: i32,
    pub cat: usize,
    /// Last index of the group or loop starting here.
    pub range_end: Option<usize>,
    /// Metadata and flow blocks, drawn dimmer as `.row.info` is on the web.
    pub info: bool,
}

/// Rows and consistency marks for one tape, rebuilt when the tape or a display
/// option changes.
#[derive(Default)]
pub struct RowCache {
    /// The tape version and display-option version the rows were built from;
    /// `None` until the first build, so an empty tape is built once too.
    built: Option<(u64, u64)>,
    pub rows: Vec<Row>,
    pub issues: HashMap<usize, Vec<Issue>>,
}

/// `category()` in `src/ui/TapePane.tsx`, as an index into `Tokens::cat`.
fn category(b: &Block) -> usize {
    if b.body.is_unknown() {
        return 5;
    }
    match b.id() {
        0x10 | 0x11 | 0x14 | 0x15 | 0x18 | 0x19 => 0,
        0x12 | 0x13 | 0x2b => 1,
        0x21 | 0x22 => 3,
        0x20 | 0x23..=0x2a => 2,
        _ => 4,
    }
}

fn is_info_row(b: &Block) -> bool {
    !b.body.is_data_block()
        && (is_metadata(b)
            || matches!(
                b.body,
                Body::Pause { .. }
                    | Body::Jump { .. }
                    | Body::LoopStart { .. }
                    | Body::LoopEnd
                    | Body::Call { .. }
                    | Body::Return
                    | Body::Select { .. }
                    | Body::Stop48
                    | Body::SignalLevel { .. }
            ))
}

impl RowCache {
    fn rebuild(&mut self, blocks: &[Block], hex: bool, zero_based: bool) {
        let kinds = content_labels(blocks);
        let ranges: HashMap<usize, usize> = tapeti_core::programs::group_ranges(blocks)
            .into_iter()
            .map(|(s, e)| (s as usize, e as usize))
            .collect();
        let mut depth = 0i32;
        self.rows = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if matches!(b.body, Body::GroupEnd | Body::LoopEnd) {
                    depth = (depth - 1).max(0);
                }
                let d = depth;
                if ranges.contains_key(&i) {
                    depth += 1;
                }
                Row {
                    no: fmt::block_no(i, zero_based).to_string(),
                    id: format!("{:02X}", b.id()),
                    desc: describe_block(b, hex),
                    kind: kinds[i].clone(),
                    len: core_fmt(block_length(b), hex, 0),
                    depth: d,
                    cat: category(b),
                    range_end: ranges.get(&i).copied(),
                    info: is_info_row(b),
                }
            })
            .collect();
        self.issues.clear();
        for is in check_consistency(blocks, fmt::block_no(0, zero_based) as i32) {
            if is.block < 0 || is.severity == Severity::Info {
                continue;
            }
            self.issues.entry(is.block as usize).or_default().push(is);
        }
    }
}

/// Rebuild the caches of both panes if anything they read has changed.
pub fn refresh(app: &mut App) {
    for side in 0..2 {
        let want = (app.store.tape(side).generation(), app.store.view_gen);
        if app.rows[side].built == Some(want) {
            continue;
        }
        let hex = app.store.hex;
        let zero = app.store.settings.zero_based;
        // Taken out and put back, so the cache can borrow the blocks while the
        // store stays mutable for the rest of the frame.
        let blocks = std::mem::take(&mut app.store.tape_mut(side).blocks);
        app.rows[side].rebuild(&blocks, hex, zero);
        app.store.tape_mut(side).blocks = blocks;
        app.rows[side].built = Some(want);
    }
}

/// Indices of the rows a collapsed group does not hide.
pub fn visible_rows(app: &App, side: Side) -> Vec<usize> {
    let t = app.store.tape(side);
    let rows = &app.rows[side].rows;
    let mut out = Vec::with_capacity(rows.len());
    let mut hide_until: i64 = -1;
    for (i, row) in rows.iter().enumerate() {
        if (i as i64) > hide_until {
            out.push(i);
        }
        if let Some(end) = row.range_end {
            if t.collapsed.contains(&t.blocks[i].uid) && end as i64 > hide_until {
                hide_until = end as i64;
            }
        }
    }
    out
}

/// Issues of blocks start..=end, so a collapsed group shows what it hides.
fn range_issues(cache: &RowCache, start: usize, end: usize, zero: bool) -> Vec<Issue> {
    let mut out = Vec::new();
    for i in start..=end {
        for is in cache.issues.get(&i).into_iter().flatten() {
            let mut is = is.clone();
            if i != start {
                is.message = format!("#{}: {}", fmt::block_no(i, zero), is.message);
            }
            out.push(is);
        }
    }
    out
}

/// The whole list of one pane.
pub fn show(app: &mut App, ui: &mut Ui, side: Side) {
    // `App::frame` refreshes the caches too, but a command can run between
    // there and here — the pane's own Open button, a few lines above this in
    // `App::pane`, or the in-window menu bar. Loading a tape then leaves rows
    // built for the tape that *was* open to be indexed against the one that is
    // open now, and a shorter tape panics on the first group or loop the old
    // rows remember. `refresh` compares generations, so asking twice in a frame
    // costs a comparison when nothing has moved.
    refresh(app);
    let tok = app.tokens;
    let visible = visible_rows(app, side);
    let dragging = app.drag.is_some();
    // Rows butt up against each other, so a row is exactly ROW_H tall wherever
    // the code measures one: the scroll target, the drop line, `show_rows`.
    ui.spacing_mut().item_spacing.y = 0.0;

    let mut area = egui::ScrollArea::vertical().id_salt(("blocklist", side)).auto_shrink([false, false]);
    if let Some(index) = app.scroll_to[side].take() {
        if let Some(pos) = visible.iter().position(|i| *i == index) {
            let top = pos as f32 * ROW_H;
            let view_h = app.view_h[side];
            let offset = if top < app.scroll_offset[side] {
                top
            } else if top + ROW_H > app.scroll_offset[side] + view_h {
                top + ROW_H - view_h
            } else {
                app.scroll_offset[side]
            };
            area = area.vertical_scroll_offset(offset);
        }
    }

    let mut clicked: Option<(usize, SelectMode)> = None;
    let mut double: Option<usize> = None;
    let mut context: Option<usize> = None;
    let mut drag_start: Option<usize> = None;
    let mut hovered_row: Option<(usize, bool)> = None;

    let out = area.show_rows(ui, ROW_H, visible.len(), |ui, range| {
        let painter = ui.painter().clone();
        for pos in range {
            let i = visible[pos];
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
            if response.clicked() {
                let mods = ui.input(|inp| inp.modifiers);
                let mode = if mods.shift {
                    SelectMode::Range
                } else if mods.command {
                    SelectMode::Toggle
                } else {
                    SelectMode::Single
                };
                clicked = Some((i, mode));
            }
            if response.double_clicked() {
                double = Some(i);
            }
            if response.secondary_clicked() {
                context = Some(i);
            }
            if response.drag_started() {
                drag_start = Some(i);
            }
            if let Some(p) = ui.input(|inp| inp.pointer.hover_pos()) {
                if rect.contains(p) {
                    hovered_row = Some((i, p.y > rect.center().y));
                }
            }
            if ui.is_rect_visible(rect) {
                draw_row(app, &painter, ui, rect, side, i, &tok);
            }
        }
        // Clicking the empty space below the last row clears the cursor, the way
        // the "No tape loaded" placeholder counts as list background on the web.
        let rest = ui.available_rect_before_wrap();
        if rest.height() > 1.0 {
            let r = ui.allocate_rect(rest, Sense::click());
            if r.clicked() {
                clicked = Some((usize::MAX, SelectMode::Single));
            }
            if r.secondary_clicked() {
                context = Some(usize::MAX);
            }
        }
    });
    app.scroll_offset[side] = out.state.offset.y;
    app.view_h[side] = out.inner_rect.height();
    let list_rect = out.inner_rect;

    if app.store.tape(side).blocks.is_empty() {
        let p = ui.painter();
        p.text(
            list_rect.center() - vec2(0.0, 10.0),
            Align2::CENTER_CENTER,
            "No tape loaded",
            FontId::proportional(15.0),
            tok.muted,
        );
        p.text(
            list_rect.center() + vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            "Drop a TZX or TAP file here, or use the folder button above.",
            FontId::proportional(12.0),
            tok.faint,
        );
    }

    // ---- drag and drop ----------------------------------------------------
    if let Some(i) = drag_start {
        if !app.store.tape(side).selected.contains(&app.store.tape(side).blocks[i].uid) {
            app.store.set_cursor(side, i as i32, SelectMode::Single);
        }
        let indices = app.store.tape(side).unit_indices(i as i32);
        app.drag = Some(Drag { from: side, indices });
    }
    let pointer_here = ui.input(|inp| inp.pointer.hover_pos()).is_some_and(|p| list_rect.contains(p));
    if dragging && pointer_here {
        let target = drop_target(app, side, &visible, hovered_row);
        let y = drop_line_y(&visible, list_rect, out.state.offset.y, target);
        ui.painter().hline(list_rect.x_range(), y, Stroke::new(2.0, tok.accent));
        app.drop_target = Some((side, target));
    }

    // ---- clicks -----------------------------------------------------------
    if let Some((i, mode)) = clicked {
        if i == usize::MAX {
            app.store.set_cursor(side, -1, SelectMode::Single);
        } else {
            app.store.set_cursor(side, i as i32, mode);
            app.scroll_to_cursor(side);
        }
    }
    if let Some(i) = context {
        app.store.active = side;
        if i != usize::MAX {
            let keep = app.store.tape(side).selected.contains(&app.store.tape(side).blocks[i].uid);
            app.store.set_cursor(side, i as i32, if keep { SelectMode::Keep } else { SelectMode::Single });
        }
        if let Some(p) = ui.input(|inp| inp.pointer.interact_pos()) {
            app.context_menu = Some((side, p));
        }
    }
    if let Some(i) = double {
        let uid = app.store.tape(side).blocks[i].uid;
        if app.rows[side].rows[i].range_end.is_some() {
            app.store.toggle_collapse(side, uid);
        } else {
            crate::actions::view_data(app, side, false);
        }
    }

    // Files dropped on the pane.
    let dropped: Vec<std::path::PathBuf> =
        ui.input(|inp| inp.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
    if !dropped.is_empty() && pointer_here {
        let insert = ui.input(|inp| inp.modifiers.shift);
        crate::files::open_paths(&mut app.store, side, &dropped, insert);
    }
}

/// Where a drop would land: an index in the tape and whether it goes after it.
/// Below the last row it lands at the end, as it does on the web.
fn drop_target(app: &App, side: Side, visible: &[usize], hovered: Option<(usize, bool)>) -> (usize, bool) {
    let len = app.store.tape(side).blocks.len();
    let (index, after) = hovered.unwrap_or((visible.last().copied().unwrap_or(0), true));
    (index.min(len.saturating_sub(1)), after)
}

/// The y of the insertion line for `target`, in screen coordinates.
fn drop_line_y(visible: &[usize], list_rect: Rect, offset: f32, target: (usize, bool)) -> f32 {
    let pos = visible.iter().position(|i| *i == target.0).unwrap_or(0) as f32;
    let y = list_rect.top() - offset + (pos + f32::from(u8::from(target.1))) * ROW_H;
    y.clamp(list_rect.top(), list_rect.bottom())
}

/// Called once both panes have drawn: only then is it known which of them the
/// pointer was over, which is what makes a drag from one pane to the other work.
pub fn finish_drag(app: &mut App, ctx: &egui::Context) {
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    let Some(drag) = app.drag.take() else { return };
    let Some((to_side, target)) = app.drop_target.take() else { return };
    let copy = ctx.input(|i| i.modifiers.alt || i.modifiers.ctrl);
    finish_drop(app, &drag, to_side, target, copy);
}

fn finish_drop(app: &mut App, drag: &Drag, to_side: Side, target: (usize, bool), copy: bool) {
    let (index, after) = target;
    let t = app.store.tape(to_side);
    let mut to = if after { index + 1 } else { index };
    // Dropping after a collapsed group start lands after the whole group.
    if after {
        if let Some(end) = t.ranges().get(&index) {
            if t.collapsed.contains(&t.blocks[index].uid) {
                to = end + 1;
            }
        }
    }
    if drag.from == to_side
        && !copy
        && drag.indices.contains(&to)
        && to > 0
        && drag.indices.contains(&(to - 1))
    {
        return; // dropped on itself
    }
    app.store.move_blocks(drag.from, drag.indices.clone(), to_side, to, copy);
}

#[allow(clippy::too_many_arguments)]
fn draw_row(app: &App, p: &egui::Painter, ui: &mut Ui, rect: Rect, side: Side, i: usize, tok: &Tokens) {
    let t = app.store.tape(side);
    let row = &app.rows[side].rows[i];
    let b = &t.blocks[i];
    let cursor = t.cursor == i as i32;
    let selected = t.selected.contains(&b.uid);
    let playing = app.progress.side == Some(side) && app.playing_row(side) == Some(i);

    // `.row`, `.row.selected`, `.row.cursor`, `.row:hover` — and nothing else:
    // the web list has no zebra stripe, so neither has this one.
    if cursor {
        p.rect_filled(rect, CornerRadius::ZERO, tok.accent_soft);
        p.rect_filled(
            Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())),
            CornerRadius::ZERO,
            tok.accent,
        );
    } else if selected {
        p.rect_filled(rect, CornerRadius::ZERO, tok.accent_soft.gamma_multiply(0.6));
    } else if ui.rect_contains_pointer(rect) {
        p.rect_filled(rect, CornerRadius::ZERO, tok.surface_2);
    }
    if playing {
        p.rect_filled(
            Rect::from_min_size(rect.left_top(), vec2(3.0, rect.height())),
            CornerRadius::ZERO,
            tok.ok,
        );
    }

    // The column stops are `style.css`'s: a 3px cursor rule and 6px of padding,
    // then `.num` (30px, 8px of it padding), `.badge` (24px + 8px), `.desc`.
    let y = rect.center().y;
    let mono = FontId::monospace(12.0);
    p.text(pos2(rect.left() + 31.0, y), Align2::RIGHT_CENTER, &row.no, FontId::monospace(11.0), tok.faint);

    let badge = Rect::from_min_size(pos2(rect.left() + 39.0, y - 8.5), vec2(24.0, 17.0));
    p.rect_filled(badge, CornerRadius::same(4), tok.cat[row.cat]);
    // `:root[data-theme="dark"] .blocklist .row .badge` inks the id dark: the
    // category colours lighten in the dark theme, and white on them is unread.
    p.text(badge.center(), Align2::CENTER_CENTER, &row.id, FontId::monospace(10.0), tok.accent_text);

    let mut left = rect.left() + 71.0 + row.depth as f32 * 12.0;
    let collapsed = t.collapsed.contains(&b.uid) && row.range_end.is_some();
    if row.range_end.is_some() {
        let caret = if collapsed { &icons::CARET_RIGHT } else { &icons::CARET_DOWN };
        let box_ = Rect::from_center_size(pos2(left + 6.0, y), vec2(11.0, 11.0));
        icons::paint(p, box_, caret, tok.muted);
    }
    left += 16.0; // `.tog`, drawn or not

    // `.row.cursor .desc` takes the full text colour back off `.row.info`.
    let colour = match t.compare.get(&b.uid) {
        Some(Mark::Diff) => tok.diff,
        Some(Mark::Match) => tok.match_,
        Some(Mark::Ignored) => tok.ignored,
        None if row.info && !cursor => tok.muted,
        None => tok.text,
    };
    // From the right: 10px of padding, `.len` (74), `.kindcol` (96 + 10 of
    // margin) and, when there is one, the issue mark (16 + 8).
    let zero = app.store.settings.zero_based;
    let issues = if collapsed {
        range_issues(&app.rows[side], i, row.range_end.unwrap(), zero)
    } else {
        app.rows[side].issues.get(&i).cloned().unwrap_or_default()
    };
    let right = rect.right() - 190.0 - if issues.is_empty() { 0.0 } else { 24.0 };
    if right > left {
        let mut text = row.desc.clone();
        if collapsed {
            let hidden = row.range_end.unwrap().saturating_sub(i + 1);
            text.push_str(&format!(" … {hidden} block(s)"));
        }
        let clip = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
        p.with_clip_rect(clip).text(pos2(left, y), Align2::LEFT_CENTER, text, mono.clone(), colour);
    }

    // The consistency mark, and the tooltip it carries.
    if !issues.is_empty() {
        let error = issues.iter().any(|is| is.severity == Severity::Error);
        let at = pos2(rect.right() - 198.0, y);
        p.text(
            at,
            Align2::CENTER_CENTER,
            "!",
            FontId::proportional(12.0),
            if error { tok.danger } else { tok.warn },
        );
        let mark = Rect::from_center_size(at, vec2(14.0, ROW_H));
        if ui.rect_contains_pointer(mark) {
            let text = issues.iter().map(|is| is.message.clone()).collect::<Vec<_>>().join("\n");
            ui.interact(mark, Id::new(("issue", side, i)), Sense::hover()).on_hover_text(text);
        }
    }

    // `.kind` is a bordered pill, not bare text — and it takes the accent on
    // the cursor row, the way `.row.cursor .kind` does.
    if !row.kind.is_empty() {
        let (fg, edge) = if cursor { (tok.accent, tok.accent) } else { (tok.muted, tok.border_strong) };
        // `.kind`'s own `font:` shorthand nests `var(--font)`, which is itself a
        // shorthand, so the declaration never applied: the pill has always been
        // set in the list's monospace, at the list's size.
        let galley = p.layout_no_wrap(row.kind.clone(), mono.clone(), fg);
        let size = galley.size() + vec2(12.0, 6.0);
        let pill = Rect::from_min_size(pos2(rect.right() - 88.0 - size.x, y - size.y / 2.0), size);
        p.rect_stroke(pill, CornerRadius::same(4), Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        p.galley(pill.min + vec2(6.0, 3.0), galley, fg);
    }
    p.text(pos2(rect.right() - 10.0, y), Align2::RIGHT_CENTER, &row.len, mono, tok.muted);

    // The badge's tooltip names the block type, as its `title` does on the web.
    if ui.rect_contains_pointer(badge) {
        let name = block_name(b.id()).unwrap_or("Unknown block");
        ui.interact(badge, Id::new(("badge", side, i)), Sense::hover()).on_hover_text(name);
    }
}

/// The context menu the right button opens, `contextMenu` in `MenuBar.tsx`.
pub fn context_menu(app: &mut App, ctx: &egui::Context) {
    let Some((side, pos)) = app.context_menu else { return };
    let tok = app.tokens;
    let mut chosen: Option<String> = None;
    let mut close = false;
    let area =
        egui::Area::new(Id::new("ctxmenu")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::popup(&ctx.global_style()).fill(tok.surface).show(ui, |ui| {
                ui.set_width(250.0);
                for id in commands::CONTEXT_MENU {
                    if id.is_empty() {
                        ui.separator();
                        continue;
                    }
                    let enabled = commands::enabled(app, id, side);
                    let label = commands::context_label(app, id, side);
                    let keys = crate::menutable::item(id).map(|i| fmt::accel(i.keys)).unwrap_or_default();
                    let button = egui::Button::new(label).shortcut_text(keys).min_size(vec2(240.0, 0.0));
                    if ui.add_enabled(enabled, button).clicked() {
                        chosen = Some((*id).to_string());
                    }
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if ctx.input(|i| i.pointer.any_click()) && !area.response.contains_pointer() {
        close = true;
    }
    if let Some(id) = chosen {
        commands::run(app, &id, side);
        close = true;
    }
    if close {
        app.context_menu = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menu;
    use crate::settings::Settings;
    use crate::state::Store;
    use tapeti_core::types::create_body;

    fn app_with(ids: &[u8]) -> App {
        let ctx = egui::Context::default();
        let mut store = Store::new(Settings::default());
        let blocks: Vec<Block> = ids.iter().map(|id| Block::new(create_body(*id))).collect();
        store.tape_mut(0).load("t.tzx".into(), None, blocks, None);
        let mut app = App::build(&ctx, Menu::headless(), store, std::time::Instant::now(), 0, false);
        refresh(&mut app);
        app
    }

    fn ids(app: &App, side: Side) -> Vec<u8> {
        app.store.tape(side).blocks.iter().map(Block::id).collect()
    }

    #[test]
    fn a_collapsed_group_hides_its_body() {
        // group start, data, group end, pause
        let mut app = app_with(&[0x21, 0x10, 0x22, 0x20]);
        assert_eq!(visible_rows(&app, 0), vec![0, 1, 2, 3]);
        let group = app.store.tape(0).blocks[0].uid;
        app.store.toggle_collapse(0, group);
        assert_eq!(visible_rows(&app, 0), vec![0, 3], "only the header and what follows the group");
    }

    /// A command can run after `App::frame` has refreshed the row caches and
    /// before the list is drawn: the pane's Open button is a few lines above
    /// `show` in `App::pane`, and the in-window menu bar fires earlier still.
    /// Loading a *shorter* tape there left `visible_rows` walking rows the old
    /// tape had and indexing `blocks` with them — a panic on the first group or
    /// loop past the end of the new tape, which is every tape with a group in
    /// it followed by a small one.
    #[test]
    fn the_list_survives_a_tape_replaced_mid_frame() {
        let ctx = egui::Context::default();
        let mut store = Store::new(Settings::default());
        // A group starting at index 2, so a two-block tape cannot reach it.
        let ids = [0x10u8, 0x10, 0x21, 0x10, 0x22, 0x20];
        let blocks: Vec<Block> = ids.iter().map(|id| Block::new(create_body(*id))).collect();
        store.tape_mut(0).load("long.tzx".into(), None, blocks, None);
        let mut app = App::build(&ctx, Menu::headless(), store, std::time::Instant::now(), 0, false);
        refresh(&mut app);

        // What opening a tape from the pane's own toolbar does, at the point in
        // the frame where it does it: the caches are already built for the tape
        // that is being replaced.
        let short: Vec<Block> = [0x10u8, 0x20].iter().map(|id| Block::new(create_body(*id))).collect();
        app.store.tape_mut(0).load("short.tzx".into(), None, short, None);

        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| show(&mut app, ui, 0)).drop_without_applying_deltas();
        assert_eq!(app.rows[0].rows.len(), 2, "the list drew the tape that is open now");
    }

    #[test]
    fn dropping_after_a_collapsed_group_lands_after_the_whole_group() {
        let mut app = app_with(&[0x21, 0x10, 0x22, 0x20]);
        let group = app.store.tape(0).blocks[0].uid;
        app.store.toggle_collapse(0, group);
        // Drag the pause and drop it just after the collapsed group's header row.
        let drag = Drag { from: 0, indices: vec![3] };
        finish_drop(&mut app, &drag, 0, (0, true), false);
        assert_eq!(ids(&app, 0), vec![0x21, 0x10, 0x22, 0x20]);
    }

    /// Dropping inside the run being dragged is the one drop the web ignores
    /// outright; every other drop commits, even when it puts the blocks back
    /// where they were.
    #[test]
    fn dropping_inside_the_dragged_run_is_ignored() {
        let mut app = app_with(&[0x10, 0x11, 0x12]);
        let before = app.store.tape(0).generation();
        let drag = Drag { from: 0, indices: vec![1, 2] };
        finish_drop(&mut app, &drag, 0, (1, true), false);
        assert_eq!(app.store.tape(0).generation(), before, "no edit, so no undo step either");
        assert_eq!(ids(&app, 0), vec![0x10, 0x11, 0x12]);
    }

    #[test]
    fn dropping_a_block_just_after_itself_leaves_the_order_alone() {
        let mut app = app_with(&[0x10, 0x11, 0x12]);
        let drag = Drag { from: 0, indices: vec![1] };
        finish_drop(&mut app, &drag, 0, (1, true), false);
        assert_eq!(ids(&app, 0), vec![0x10, 0x11, 0x12]);
    }

    #[test]
    fn dragging_to_the_other_pane_copies_with_the_modifier() {
        let mut app = app_with(&[0x10, 0x11]);
        let drag = Drag { from: 0, indices: vec![0] };
        finish_drop(&mut app, &drag, 1, (0, false), true);
        assert_eq!(ids(&app, 0), vec![0x10, 0x11]);
        assert_eq!(ids(&app, 1), vec![0x10]);
    }

    #[test]
    fn the_rows_carry_what_the_list_shows() {
        let app = app_with(&[0x21, 0x10, 0x22]);
        let rows = &app.rows[0].rows;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "21");
        assert_eq!(rows[0].range_end, Some(2), "the group start knows where it ends");
        assert_eq!(rows[1].depth, 1, "the block inside the group is indented");
        assert_eq!(rows[2].depth, 0);
        assert!(rows[0].info, "a group start is a metadata row");
        assert!(!rows[1].info, "a data block is not");
    }

    /// Block numbers follow the "number blocks from 0" option, everywhere. It is
    /// on by default, so this starts from 0 and turns it off.
    #[test]
    fn the_zero_based_option_renumbers_the_rows() {
        let mut app = app_with(&[0x10, 0x11]);
        assert!(app.store.settings.zero_based, "blocks are numbered from 0 by default");
        assert_eq!(app.rows[0].rows[0].no, "0");
        app.store.settings.zero_based = false;
        app.store.touch_view();
        refresh(&mut app);
        assert_eq!(app.rows[0].rows[0].no, "1");
    }
}
