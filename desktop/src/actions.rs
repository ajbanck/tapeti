//! The higher-level actions, the port of `src/state/actions.ts`.
//!
//! One shape changes: `confirmDiscard(side, () => …)` takes a closure on the web
//! and a [`Then`] here. A closure that outlives the frame would have to capture
//! the store it is going to mutate; naming the follow-up instead keeps the
//! borrow checker out of it and makes a queued action something the app can show
//! and test.

use tapeti_core::audio::FlowOptions;
use tapeti_core::consistency::{check_consistency, Severity};
use tapeti_core::programs::{detect_programs, Program};
use tapeti_core::types::{Block, Body};
use tapeti_core::writer::{required_version, serialize_tzx};

use crate::app::App;
use crate::dialogs::Dialog;
use crate::files;
use crate::fmt;
use crate::state::{other, Edit, SelectMode, Side, Store};

/// What "Open in emulator" sends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Tape,
    Cursor,
    Selection,
}

impl Scope {
    fn suffix(self) -> &'static str {
        match self {
            Scope::Tape => "",
            Scope::Cursor => "from",
            Scope::Selection => "sel",
        }
    }
}

/// An action waiting for the user to agree to it.
#[derive(Clone, PartialEq, Eq)]
pub enum Then {
    NewTape(Side),
    Open(Side),
    /// Check the extracted blocks, then extract (or ask again).
    ExtractStart(Side),
    ExtractGo(Side),
    EmulatorGo(Side, Scope),
}

pub fn run_then(app: &mut App, then: Then) {
    match then {
        Then::NewTape(side) => files::new_tape(&mut app.store, side),
        Then::Open(side) => files::pick_and_open(&mut app.store, side, false),
        Then::ExtractStart(side) => extract_start(app, side),
        Then::ExtractGo(side) => extract_go(app, side),
        Then::EmulatorGo(side, scope) => launch_emulator(app, side, scope),
    }
}

/// Run `then` now, or after the user agrees to drop the tape's unsaved changes.
pub fn confirm_discard(app: &mut App, side: Side, then: Then) {
    let t = app.store.tape(side);
    if !t.dirty() {
        run_then(app, then);
        return;
    }
    let lines = vec![format!(
        "The {} tape \"{}\" has unsaved changes.",
        if side == 0 { "left" } else { "right" },
        t.name
    )];
    app.store.dialog = Some(Dialog::confirm("Discard changes?", lines, then));
}

// ---- timings -----------------------------------------------------------------

/// "Set selection timings to current" copies all timings (not the pause) to the
/// selected data blocks.
pub fn set_selection_timings(app: &mut App, side: Side) {
    let t = app.store.tape(side);
    let Some(cur) = t.cursor_block().cloned() else { return };
    if !cur.body.is_data_block() {
        app.store.set_status("Current block is not a data block");
        return;
    }
    let selected = t.selected.clone();
    let cur_uid = cur.uid;
    let mut n = 0usize;
    let cursor = t.cursor;
    app.store.commit(side, |bl| {
        for b in bl.iter_mut() {
            if !selected.contains(&b.uid) || b.uid == cur_uid {
                continue;
            }
            let mut copied = true;
            match (&mut b.body, &cur.body) {
                (
                    Body::Turbo { pilot, sync1, sync2, zero, one, pilot_len, .. },
                    Body::Turbo {
                        pilot: cp, sync1: cs1, sync2: cs2, zero: cz, one: co, pilot_len: cpl, ..
                    },
                ) => {
                    (*pilot, *sync1, *sync2, *zero, *one, *pilot_len) = (*cp, *cs1, *cs2, *cz, *co, *cpl);
                }
                (Body::Turbo { zero, one, .. }, Body::PureData { zero: cz, one: co, .. })
                | (Body::PureData { zero, one, .. }, Body::Turbo { zero: cz, one: co, .. })
                | (Body::PureData { zero, one, .. }, Body::PureData { zero: cz, one: co, .. }) => {
                    (*zero, *one) = (*cz, *co);
                }
                (
                    Body::Generalized { npp, npd, pilot_symbols, data_symbols, .. },
                    Body::Generalized { npp: cnpp, npd: cnpd, pilot_symbols: cps, data_symbols: cds, .. },
                ) => {
                    (*npp, *npd) = (*cnpp, *cnpd);
                    *pilot_symbols = cps.clone();
                    *data_symbols = cds.clone();
                }
                (Body::Direct { tstates, .. }, Body::Direct { tstates: ct, .. }) => *tstates = *ct,
                _ => copied = false,
            }
            if copied {
                n += 1;
            }
        }
        Edit { cursor: Some(cursor), selected: Some(selected.clone()) }
    });
    app.store.set_status(format!("Timings copied to {n} block(s)"));
}

// ---- the data window ---------------------------------------------------------

pub fn view_data(app: &mut App, side: Side, as_one: bool) {
    let t = app.store.tape(side);
    if t.cursor < 0 {
        return;
    }
    let idx = if as_one { t.unit_indices(t.cursor) } else { vec![t.cursor as usize] };
    let uids: Vec<_> =
        idx.iter().map(|i| &t.blocks[*i]).filter(|b| b.body.has_data()).map(|b| b.uid).collect();
    if uids.is_empty() {
        app.store.set_status("This block has no data to view");
        return;
    }
    app.store.active = side;
    app.open_data_window(side, uids);
}

// ---- playback ----------------------------------------------------------------

pub fn play_tape(app: &mut App, side: Side, from_cursor: bool) {
    let t = app.store.tape(side);
    if t.blocks.is_empty() {
        return;
    }
    let mut order = tapeti_core::audio::playback_order(&t.blocks, FlowOptions::default());
    if from_cursor && t.cursor >= 0 {
        let cursor = t.cursor as u32;
        order = match order.iter().position(|i| *i == cursor) {
            Some(at) => order[at..].to_vec(),
            None => vec![cursor],
        };
    }
    play(app, side, order);
}

pub fn play_selection(app: &mut App, side: Side) {
    let t = app.store.tape(side);
    let idx: Vec<u32> = t.unit_indices(t.cursor).into_iter().map(|i| i as u32).collect();
    if idx.is_empty() {
        return;
    }
    play(app, side, idx);
}

fn play(app: &mut App, side: Side, order: Vec<u32>) {
    let mic = app.store.audio_mic;
    let blocks = app.store.tape(side).blocks.clone();
    if let Some(err) = app.player.play(&blocks, order, Some(side), mic) {
        app.store.set_status(err);
    } else {
        app.store.set_status("");
    }
}

// ---- programs ----------------------------------------------------------------

fn select_range(store: &mut Store, side: Side, p: &Program) {
    let uids: Vec<_> =
        store.tape(side).blocks[p.start as usize..=p.end as usize].iter().map(|b| b.uid).collect();
    store.select_uids(side, uids);
    let zero = store.settings.zero_based;
    store.set_status(format!(
        "{}: blocks #{}–#{} selected",
        p.name,
        fmt::block_no(p.start as usize, zero),
        fmt::block_no(p.end as usize, zero)
    ));
}

fn program_at(programs: &[Program], index: i32) -> Option<Program> {
    let i = u32::try_from(index).ok()?;
    programs.iter().find(|p| p.start <= i && i <= p.end).cloned()
}

/// Select the whole program (game) the cursor is in.
pub fn select_program(app: &mut App, side: Side) {
    let t = app.store.tape(side);
    let Some(p) = program_at(&detect_programs(&t.blocks), t.cursor) else { return };
    let cursor = t.cursor;
    app.store.set_cursor(side, cursor, SelectMode::Keep);
    select_range(&mut app.store, side, &p);
}

/// Move the cursor to a program's first block and select the program.
pub fn jump_to_program(app: &mut App, side: Side, p: &Program) {
    app.store.set_cursor(side, p.start as i32, SelectMode::Single);
    select_range(&mut app.store, side, p);
    app.scroll_to_cursor(side);
}

pub fn open_program_picker(app: &mut App, side: Side) {
    if !app.store.tape(side).blocks.is_empty() {
        app.store.dialog = Some(Dialog::programs(side));
    }
}

// ---- extract -----------------------------------------------------------------

/// Copy the unit at the cursor into the other pane as a new tape.
pub fn extract_to_other_pane(app: &mut App, side: Side) {
    if app.store.tape(side).unit_indices(app.store.tape(side).cursor).is_empty() {
        return;
    }
    let to = other(side);
    confirm_discard(app, to, Then::ExtractStart(side));
}

fn extract_start(app: &mut App, side: Side) {
    let t = app.store.tape(side);
    let idx = t.unit_indices(t.cursor);
    let problems = part_problems(&app.store, side, &idx);
    if problems.is_empty() {
        extract_go(app, side);
        return;
    }
    let mut lines = vec!["The extracted blocks may not load or play correctly on their own:".to_string()];
    lines.extend(problems);
    lines.push("Extract anyway?".into());
    app.store.dialog = Some(Dialog::confirm("Extract to other pane", lines, Then::ExtractGo(side)));
}

fn extract_go(app: &mut App, side: Side) {
    let t = app.store.tape(side);
    let idx = t.unit_indices(t.cursor);
    let Some(first) = idx.first().copied() else { return };
    let last = *idx.last().unwrap();
    let to = other(side);
    let p = program_at(&detect_programs(&t.blocks), first as i32);
    let whole = p.as_ref().is_some_and(|p| {
        p.start as usize == first && p.end as usize == last && idx.len() == (p.end - p.start + 1) as usize
    });
    let zero = app.store.settings.zero_based;
    let name = if whole {
        let n: String =
            p.unwrap().name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
        n + ".tzx"
    } else {
        format!("{}-sel{}.tzx", files::stem(&t.name), fmt::block_no(first, zero))
    };
    let blocks: Vec<Block> = idx.iter().map(|i| t.blocks[*i].clone_fresh()).collect();
    let n = blocks.len();
    files::new_tape(&mut app.store, to);
    app.store.tape_mut(to).name = name.clone();
    app.store.insert_blocks(to, 0, blocks);
    app.store.set_status(format!(
        "Extracted {n} block(s) to the {} pane as {name}",
        if to == 0 { "left" } else { "right" }
    ));
}

// ---- the emulator ------------------------------------------------------------

/// Indices of the blocks "Open in emulator" sends for `scope`.
pub fn emulator_indices(store: &Store, side: Side, scope: Scope) -> Vec<usize> {
    let t = store.tape(side);
    match scope {
        Scope::Tape => (0..t.blocks.len()).collect(),
        _ if t.cursor < 0 => Vec::new(),
        Scope::Cursor => (t.cursor as usize..t.blocks.len()).collect(),
        Scope::Selection => t.unit_indices(t.cursor),
    }
}

/// `target 12` and `target 40` are the same problem seen from two numberings.
fn without_target_number(msg: &str) -> String {
    match msg.find("target ") {
        Some(at) => {
            let rest = &msg[at + 7..];
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            format!("{}target{}", &msg[..at], &rest[digits..])
        }
        None => msg.to_string(),
    }
}

/// Problems the exported part would have on its own, numbered as in the full tape.
pub fn part_problems(store: &Store, side: Side, idx: &[usize]) -> Vec<String> {
    let t = store.tape(side);
    if idx.len() == t.blocks.len() {
        return Vec::new();
    }
    let part: Vec<Block> = idx.iter().map(|i| t.blocks[*i].clone()).collect();
    let key = |block: i32, message: &str| format!("{block}:{}", without_target_number(message));
    let already: std::collections::HashSet<String> =
        check_consistency(&t.blocks, 1).iter().map(|is| key(is.block, &is.message)).collect();
    let zero = store.settings.zero_based;
    let mut out: Vec<String> = check_consistency(&part, 1)
        .iter()
        .filter(|is| is.severity == Severity::Error && is.block >= 0)
        .filter(|is| !already.contains(&key(idx[is.block as usize] as i32, &is.message)))
        .map(|is| {
            format!(
                "#{}: {}",
                fmt::block_no(idx[is.block as usize], zero),
                without_target_number(&is.message)
            )
        })
        .collect();
    let gaps = idx.windows(2).any(|w| w[1] != w[0] + 1);
    let jumps =
        part.iter().any(|b| matches!(b.body, Body::Jump { .. } | Body::Call { .. } | Body::Select { .. }));
    if gaps && jumps {
        out.push(
            "The selection has gaps, so relative jump/call/select offsets point to different blocks.".into(),
        );
    }
    out
}

/// Open the tape, the part from the cursor, or the selection in the emulator.
pub fn open_in_emulator(app: &mut App, side: Side, scope: Scope) {
    let idx = emulator_indices(&app.store, side, scope);
    if idx.is_empty() {
        let empty = app.store.tape(side).blocks.is_empty();
        app.store.set_status(if empty { "The tape is empty" } else { "No current block" });
        return;
    }
    let problems = part_problems(&app.store, side, &idx);
    if problems.is_empty() {
        launch_emulator(app, side, scope);
        return;
    }
    let mut lines = vec!["The exported blocks may not load or play correctly:".to_string()];
    lines.extend(problems);
    lines.push("Open anyway?".into());
    app.store.dialog = Some(Dialog::confirm("Open in emulator", lines, Then::EmulatorGo(side, scope)));
}

fn launch_emulator(app: &mut App, side: Side, scope: Scope) {
    let idx = emulator_indices(&app.store, side, scope);
    let t = app.store.tape(side);
    let blocks: Vec<Block> = idx.iter().map(|i| t.blocks[*i].clone()).collect();
    let bytes = serialize_tzx(&blocks, Some(required_version(&blocks)));
    let stem = files::stem(&t.name);
    let zero = app.store.settings.zero_based;
    let name = if scope == Scope::Tape {
        stem
    } else {
        format!("{stem}-{}{}", scope.suffix(), fmt::block_no(idx[0], zero))
    };
    let program = app.store.settings.emulator_program.clone();
    let args = app.store.settings.emulator_args.clone();
    match crate::emulator::open_in_emulator(bytes, name, program, args) {
        Ok(used) => {
            let shown = used.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or(&used).to_string();
            let shown = shown.strip_suffix(".app").unwrap_or(&shown).to_string();
            app.store.set_status(format!("Opened {} block(s) in {shown}", blocks.len()));
        }
        Err(msg) if msg == "NOT_FOUND" => {
            app.store.dialog =
                Some(Dialog::emulator(true, Some(Then::EmulatorGo(side, scope)), &app.store.settings));
        }
        Err(msg) => {
            app.store
                .message("Open in emulator", vec![msg, "Check the emulator in Options → Emulator…".into()]);
        }
    }
}
