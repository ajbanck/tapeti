//! The application state, the port of `src/state/store.ts`.
//!
//! Preact signals become plain fields: egui redraws from this struct every
//! frame, so there is nothing to subscribe to. Blocks stay immutable — an edit
//! builds a new `Vec<Block>` — which is what makes the undo snapshot a cheap
//! clone of a vector of `Rc`-free values whose payloads the compiler moves
//! rather than copies.
//!
//! `dirty` is identity in TypeScript (`snap.blocks !== t.saved`). Here every
//! version of the blocks array carries a generation number, so undoing back to
//! the saved version clears `dirty` exactly as it does on the web.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tapeti_core::compare::{compare_tapes, find_matches, BlockCompareMode, CompareResult, TapeCompareMode};
use tapeti_core::programs::group_ranges;
use tapeti_core::types::{Block, Body, Uid};
use tapeti_core::writer::Version;

use crate::dialogs::Dialog;
use crate::settings::Settings;

pub type Side = usize;

pub fn other(side: Side) -> Side {
    1 - side
}

static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

fn next_gen() -> u64 {
    NEXT_GEN.fetch_add(1, Ordering::Relaxed)
}

/// What a block's row is tinted with after Compare tapes or Find match. The
/// core's `CompareResult` has no `Match`: find-match is a different question
/// asked of the same comparison, and the web store spells its answer the same
/// way, as a third colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Diff,
    Match,
    Ignored,
}

#[derive(Clone)]
struct Snapshot {
    blocks: Vec<Block>,
    gen: u64,
    cursor: i32,
    selected: HashSet<Uid>,
    /// What the tape was called and saved against when the snapshot was taken:
    /// emptying a tape resets its identity, and undo brings that back too.
    name: String,
    path: Option<PathBuf>,
    loaded_version: Option<Version>,
    saved_gen: u64,
}

pub struct TapeState {
    pub name: String,
    /// Absolute path when opened or saved through a dialog.
    pub path: Option<PathBuf>,
    pub blocks: Vec<Block>,
    /// Version of `blocks`; compared with `saved_gen` for `dirty`.
    gen: u64,
    saved_gen: u64,
    /// Index of the current block, -1 for none.
    pub cursor: i32,
    pub selected: HashSet<Uid>,
    /// Uids of collapsed group/loop start blocks.
    pub collapsed: HashSet<Uid>,
    pub loaded_version: Option<Version>,
    /// Compare/find-match colours, by uid. Empty when there is nothing to show.
    pub compare: HashMap<Uid, Mark>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl TapeState {
    pub fn empty(name: &str) -> TapeState {
        let gen = next_gen();
        TapeState {
            name: name.to_string(),
            path: None,
            blocks: Vec::new(),
            gen,
            saved_gen: gen,
            cursor: -1,
            selected: HashSet::new(),
            collapsed: HashSet::new(),
            loaded_version: None,
            compare: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.gen != self.saved_gen
    }

    /// Version of the blocks array; the row cache rebuilds when it changes.
    pub fn generation(&self) -> u64 {
        self.gen
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn cursor_block(&self) -> Option<&Block> {
        usize::try_from(self.cursor).ok().and_then(|i| self.blocks.get(i))
    }

    pub fn has_cursor(&self) -> bool {
        self.cursor_block().is_some()
    }

    /// Record that the current blocks are what is on disk. A snapshot remembers
    /// the tape's identity, so that emptying it and undoing that puts the whole
    /// thing back; a save re-bases that identity over the history too, because
    /// the file on disk is these blocks under this name. Undoing past a save is
    /// dirty again, and does not take the name back with it.
    pub fn mark_saved(&mut self) {
        self.saved_gen = self.gen;
        for snap in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            snap.saved_gen = self.gen;
            snap.name.clone_from(&self.name);
            snap.path.clone_from(&self.path);
            snap.loaded_version = self.loaded_version;
        }
    }

    /// The tape's identity and history position, to put back on undo.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            blocks: self.blocks.clone(),
            gen: self.gen,
            cursor: self.cursor,
            selected: self.selected.clone(),
            name: self.name.clone(),
            path: self.path.clone(),
            loaded_version: self.loaded_version,
            saved_gen: self.saved_gen,
        }
    }

    /// Put a snapshot back, handing over what it replaced for the other stack.
    fn restore(&mut self, snap: Snapshot) -> Snapshot {
        let current = self.snapshot();
        self.blocks = snap.blocks;
        self.gen = snap.gen;
        self.cursor = snap.cursor;
        self.selected = snap.selected;
        self.name = snap.name;
        self.path = snap.path;
        self.loaded_version = snap.loaded_version;
        self.saved_gen = snap.saved_gen;
        current
    }

    /// A tape with nothing left on it is a new tape, not a file with no blocks:
    /// the name on the pane would go on promising the one that was loaded, and
    /// saving it would write an empty file. Undo puts the name back.
    fn reset_to_new(&mut self) {
        self.name = "new".to_string();
        self.path = None;
        self.loaded_version = None;
        self.collapsed.clear();
        self.saved_gen = self.gen;
    }

    /// Replace the tape wholesale (a load), dropping the history with it.
    pub fn load(
        &mut self,
        name: String,
        path: Option<PathBuf>,
        blocks: Vec<Block>,
        version: Option<Version>,
    ) {
        let cursor = if blocks.is_empty() { -1 } else { 0 };
        let gen = next_gen();
        *self = TapeState {
            name,
            path,
            blocks,
            gen,
            saved_gen: gen,
            cursor,
            selected: HashSet::new(),
            collapsed: HashSet::new(),
            loaded_version: version,
            compare: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        };
    }

    /// Start index -> end index of every group and loop, the map shape
    /// `groupRanges` has on the TypeScript side.
    pub fn ranges(&self) -> HashMap<usize, usize> {
        group_ranges(&self.blocks).into_iter().map(|(s, e)| (s as usize, e as usize)).collect()
    }

    /// `unitIndices`: what acts as one unit for drag, delete and copy when
    /// `index` is the grabbed block — the selection if the block is selected,
    /// else the block itself, always expanded to whole collapsed groups.
    pub fn unit_indices(&self, index: i32) -> Vec<usize> {
        let ranges = self.ranges();
        let mut set: HashSet<usize> = HashSet::new();
        let add = |i: usize, set: &mut HashSet<usize>| {
            set.insert(i);
            if let Some(end) = ranges.get(&i) {
                if self.collapsed.contains(&self.blocks[i].uid) {
                    for k in i..=*end {
                        set.insert(k);
                    }
                }
            }
        };
        let index = usize::try_from(index).ok().filter(|i| *i < self.blocks.len());
        match index {
            Some(i) if self.selected.contains(&self.blocks[i].uid) => {
                for k in 0..self.blocks.len() {
                    if self.selected.contains(&self.blocks[k].uid) {
                        add(k, &mut set);
                    }
                }
            }
            Some(i) => add(i, &mut set),
            None => {}
        }
        let mut out: Vec<usize> = set.into_iter().collect();
        out.sort_unstable();
        out
    }
}

/// What a `commit` closure may override; `None` keeps the store's own rule
/// (cursor clamped, selection filtered to surviving uids).
#[derive(Default)]
pub struct Edit {
    pub cursor: Option<i32>,
    pub selected: Option<HashSet<Uid>>,
}

impl Edit {
    pub fn cursor(c: i32) -> Edit {
        Edit { cursor: Some(c), selected: None }
    }
    pub fn at(c: i32, selected: HashSet<Uid>) -> Edit {
        Edit { cursor: Some(c), selected: Some(selected) }
    }
}

/// How a click changes the selection, the `setCursor` modes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    Single,
    Toggle,
    Range,
    /// Right-click inside a selection: leave it alone.
    Keep,
}

pub struct Store {
    pub tapes: [TapeState; 2],
    pub active: Side,
    pub hex: bool,
    pub locked: bool,
    pub block_compare: BlockCompareMode,
    pub tape_compare: TapeCompareMode,
    /// false = plain square wave, true = the Spectrum MIC response.
    pub audio_mic: bool,
    pub clipboard: Vec<Block>,
    pub settings: Settings,
    pub dialog: Option<Dialog>,
    status: String,
    status_at: Option<Instant>,
    /// Bumped whenever something outside the blocks changes what a row shows.
    pub view_gen: u64,
}

impl Store {
    pub fn new(settings: Settings) -> Store {
        Store {
            tapes: [TapeState::empty("new"), TapeState::empty("new")],
            active: 0,
            hex: false,
            locked: false,
            block_compare: BlockCompareMode::Data,
            tape_compare: TapeCompareMode::DataBlocks,
            audio_mic: true,
            clipboard: Vec::new(),
            settings,
            dialog: None,
            status: String::new(),
            status_at: None,
            view_gen: 0,
        }
    }

    pub fn tape(&self, side: Side) -> &TapeState {
        &self.tapes[side]
    }

    pub fn tape_mut(&mut self, side: Side) -> &mut TapeState {
        &mut self.tapes[side]
    }

    pub fn active_tape(&self) -> &TapeState {
        &self.tapes[self.active]
    }

    // ---- status ------------------------------------------------------------

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_at = Some(Instant::now());
    }

    /// The status line, cleared six seconds after it was set, as `setStatus` does.
    pub fn status(&mut self) -> &str {
        if let Some(at) = self.status_at {
            if at.elapsed().as_secs_f32() > 6.0 {
                self.status.clear();
                self.status_at = None;
            }
        }
        &self.status
    }

    pub fn message(&mut self, title: &str, lines: Vec<String>) {
        self.dialog = Some(Dialog::message(title, lines));
    }

    pub fn toggle_lock(&mut self) {
        self.locked = !self.locked;
        let msg = if self.locked { "Tapes locked" } else { "Tapes unlocked" };
        self.set_status(msg);
    }

    /// Something that is not a block changed what the rows show.
    pub fn touch_view(&mut self) {
        self.view_gen += 1;
    }

    // ---- editing -----------------------------------------------------------

    /// Apply a structural change with undo support. The closure edits a copy of
    /// the blocks; returning an [`Edit`] overrides the cursor and selection.
    pub fn commit<F>(&mut self, side: Side, f: F) -> bool
    where
        F: FnOnce(&mut Vec<Block>) -> Edit,
    {
        if self.locked {
            self.set_status("Tape is locked. Unlock it in the status bar to edit.");
            return false;
        }
        let t = &mut self.tapes[side];
        let snap = t.snapshot();
        let was_empty = t.blocks.is_empty();
        let mut blocks = t.blocks.clone();
        let edit = f(&mut blocks);
        let last = blocks.len() as i32 - 1;
        let cursor = edit.cursor.unwrap_or_else(|| t.cursor.min(last)).min(last).max(-1);
        let selected = edit.selected.unwrap_or_else(|| {
            let alive: HashSet<Uid> = blocks.iter().map(|b| b.uid).collect();
            t.selected.intersection(&alive).copied().collect()
        });
        t.blocks = blocks;
        t.gen = next_gen();
        t.cursor = cursor;
        t.selected = selected;
        t.compare.clear();
        t.undo.push(snap);
        if t.undo.len() > 100 {
            t.undo.remove(0);
        }
        t.redo.clear();
        if t.blocks.is_empty() && !was_empty {
            t.reset_to_new();
            self.set_status("Nothing left on the tape: the pane is a new tape again");
        }
        true
    }

    pub fn undo(&mut self, side: Side) {
        let t = &mut self.tapes[side];
        let Some(snap) = t.undo.pop() else { return };
        let current = t.restore(snap);
        t.redo.push(current);
    }

    pub fn redo(&mut self, side: Side) {
        let t = &mut self.tapes[side];
        let Some(snap) = t.redo.pop() else { return };
        let current = t.restore(snap);
        t.undo.push(current);
    }

    // ---- selection ---------------------------------------------------------

    pub fn set_cursor(&mut self, side: Side, index: i32, mode: SelectMode) {
        self.active = side;
        let t = &mut self.tapes[side];
        let Ok(i) = usize::try_from(index) else {
            t.cursor = -1;
            t.selected.clear();
            return;
        };
        if i >= t.blocks.len() {
            t.cursor = -1;
            t.selected.clear();
            return;
        }
        let uid = t.blocks[i].uid;
        match mode {
            SelectMode::Single => {
                t.selected.clear();
                t.selected.insert(uid);
            }
            SelectMode::Keep => {}
            SelectMode::Toggle => {
                if !t.selected.remove(&uid) {
                    t.selected.insert(uid);
                }
            }
            SelectMode::Range => {
                let from = if t.cursor < 0 { i } else { t.cursor as usize };
                let (a, b) = if from < i { (from, i) } else { (i, from) };
                for k in a..=b {
                    t.selected.insert(t.blocks[k].uid);
                }
            }
        }
        t.cursor = index;
    }

    pub fn select_all(&mut self, side: Side) {
        let t = &mut self.tapes[side];
        t.selected = t.blocks.iter().map(|b| b.uid).collect();
    }

    pub fn select_uids(&mut self, side: Side, uids: Vec<Uid>) {
        self.tapes[side].selected = uids.into_iter().collect();
    }

    pub fn toggle_collapse(&mut self, side: Side, uid: Uid) {
        let c = &mut self.tapes[side].collapsed;
        if !c.remove(&uid) {
            c.insert(uid);
        }
    }

    pub fn collapse_all(&mut self, side: Side, collapse: bool) {
        let t = &mut self.tapes[side];
        let mut set = HashSet::new();
        if collapse {
            for (s, _) in group_ranges(&t.blocks) {
                set.insert(t.blocks[s as usize].uid);
            }
        }
        t.collapsed = set;
    }

    // ---- block operations --------------------------------------------------

    pub fn insert_blocks(&mut self, side: Side, at: usize, blocks: Vec<Block>) {
        let uids: HashSet<Uid> = blocks.iter().map(|b| b.uid).collect();
        self.commit(side, move |bl| {
            let idx = at.min(bl.len());
            for (k, b) in blocks.into_iter().enumerate() {
                bl.insert(idx + k, b);
            }
            Edit::at(idx as i32, uids)
        });
    }

    pub fn replace_block(&mut self, side: Side, uid: Uid, body: Body) {
        let cursor = self.tapes[side].cursor;
        self.commit(side, move |bl| {
            if let Some(b) = bl.iter_mut().find(|b| b.uid == uid) {
                b.body = body;
            }
            Edit::cursor(cursor)
        });
    }

    pub fn delete_indices(&mut self, side: Side, indices: Vec<usize>) {
        if indices.is_empty() {
            return;
        }
        let set: HashSet<usize> = indices.iter().copied().collect();
        let first = *indices.iter().min().unwrap();
        self.commit(side, move |bl| {
            let mut i = 0;
            bl.retain(|_| {
                let keep = !set.contains(&i);
                i += 1;
                keep
            });
            Edit::at(first.min(bl.len().saturating_sub(1)) as i32, HashSet::new())
        });
    }

    pub fn delete_unit(&mut self, side: Side) {
        let idx = self.tapes[side].unit_indices(self.tapes[side].cursor);
        self.delete_indices(side, idx);
    }

    pub fn copy_unit(&mut self, side: Side) {
        let t = &self.tapes[side];
        let idx = t.unit_indices(t.cursor);
        self.clipboard = idx.iter().map(|i| self.tapes[side].blocks[*i].clone_fresh()).collect();
        let n = idx.len();
        self.set_status(format!("{n} block(s) copied"));
    }

    pub fn cut_unit(&mut self, side: Side) {
        let t = &self.tapes[side];
        let idx = t.unit_indices(t.cursor);
        if idx.is_empty() {
            return;
        }
        self.clipboard = idx.iter().map(|i| self.tapes[side].blocks[*i].clone_fresh()).collect();
        self.delete_indices(side, idx);
    }

    pub fn paste(&mut self, side: Side) {
        if self.clipboard.is_empty() {
            return;
        }
        let t = &self.tapes[side];
        let at = if t.cursor < 0 { t.blocks.len() } else { t.cursor as usize + 1 };
        let blocks: Vec<Block> = self.clipboard.iter().map(Block::clone_fresh).collect();
        self.insert_blocks(side, at, blocks);
    }

    pub fn duplicate_unit(&mut self, side: Side) {
        let t = &self.tapes[side];
        let idx = t.unit_indices(t.cursor);
        let Some(last) = idx.last().copied() else { return };
        let blocks: Vec<Block> = idx.iter().map(|i| t.blocks[*i].clone_fresh()).collect();
        self.insert_blocks(side, last + 1, blocks);
    }

    /// Move a unit of blocks so they land before index `to` in `to_side`.
    pub fn move_blocks(&mut self, from: Side, indices: Vec<usize>, to_side: Side, to: usize, copy: bool) {
        if indices.is_empty() {
            return;
        }
        if from == to_side && !copy {
            let set: HashSet<usize> = indices.iter().copied().collect();
            let moving: Vec<Block> = indices.iter().map(|i| self.tapes[from].blocks[*i].clone()).collect();
            let uids: HashSet<Uid> = moving.iter().map(|b| b.uid).collect();
            self.commit(from, move |bl| {
                let before: Vec<Block> = bl[..to]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !set.contains(i))
                    .map(|(_, b)| b.clone())
                    .collect();
                let after: Vec<Block> = bl[to..]
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !set.contains(&(i + to)))
                    .map(|(_, b)| b.clone())
                    .collect();
                let cursor = before.len() as i32;
                *bl = before;
                bl.extend(moving);
                bl.extend(after);
                Edit::at(cursor, uids)
            });
            return;
        }
        let clones: Vec<Block> = indices.iter().map(|i| self.tapes[from].blocks[*i].clone_fresh()).collect();
        if !copy {
            self.delete_indices(from, indices);
        }
        self.insert_blocks(to_side, to, clones);
        self.active = to_side;
    }

    pub fn move_unit(&mut self, side: Side, delta: i32) {
        let t = &self.tapes[side];
        let idx = t.unit_indices(t.cursor);
        let (Some(first), Some(last)) = (idx.first().copied(), idx.last().copied()) else { return };
        if delta < 0 && first == 0 {
            return;
        }
        if delta > 0 && last + 1 >= t.blocks.len() {
            return;
        }
        // Skip over a whole collapsed group when moving past its start or end.
        let ranges = t.ranges();
        let to = if delta < 0 {
            let mut to = first - 1;
            for (s, e) in &ranges {
                if *e == first - 1 && t.collapsed.contains(&t.blocks[*s].uid) {
                    to = *s;
                }
            }
            to
        } else {
            let mut to = last + 2;
            if let Some(end) = ranges.get(&(last + 1)) {
                if t.collapsed.contains(&t.blocks[last + 1].uid) {
                    to = end + 1;
                }
            }
            to
        };
        self.move_blocks(side, idx, side, to, false);
    }

    /// Wrap the selection (or the cursor block) in a group.
    pub fn group_selection(&mut self, side: Side, name: &str) {
        let t = &self.tapes[side];
        let idx = t.unit_indices(t.cursor);
        let (Some(first), Some(last)) = (idx.first().copied(), idx.last().copied()) else { return };
        let start = Block::new(Body::GroupStart { name: name.to_string() });
        let end = Block::new(Body::GroupEnd);
        let uid = start.uid;
        self.commit(side, move |bl| {
            bl.insert(last + 1, end);
            bl.insert(first, start);
            Edit::at(first as i32, HashSet::from([uid]))
        });
    }

    // ---- compare -----------------------------------------------------------

    pub fn run_compare_tapes(&mut self) {
        let (l, r, identical) = compare_tapes(
            &self.tapes[0].blocks,
            &self.tapes[1].blocks,
            self.block_compare,
            self.tape_compare,
        );
        for (side, res) in [(0usize, l), (1usize, r)] {
            let t = &mut self.tapes[side];
            t.compare = t
                .blocks
                .iter()
                .zip(res)
                .filter_map(|(b, c)| match c {
                    CompareResult::Diff => Some((b.uid, Mark::Diff)),
                    CompareResult::Ignored => Some((b.uid, Mark::Ignored)),
                    CompareResult::Same => None,
                })
                .collect();
        }
        let msg = if identical {
            "Tapes are identical (with the current compare settings)"
        } else {
            "Tapes differ: differing blocks shown in magenta"
        };
        self.set_status(msg);
    }

    pub fn run_find_match(&mut self, side: Side) {
        let Some(needle) = self.tapes[side].cursor_block().cloned() else { return };
        let cursor = self.tapes[side].cursor as usize;
        let mut total = 0;
        for s in [0usize, 1] {
            let skip = if s == side { Some(cursor) } else { None };
            let m = find_matches(&needle, &self.tapes[s].blocks, self.block_compare, skip);
            total += m.len();
            let t = &mut self.tapes[s];
            t.compare = m.iter().map(|i| (t.blocks[*i as usize].uid, Mark::Match)).collect();
        }
        let msg = if total > 0 {
            format!("{total} matching block(s) shown in green")
        } else {
            "No matching blocks found".to_string()
        };
        self.set_status(msg);
    }

    pub fn clear_compare(&mut self) {
        for t in &mut self.tapes {
            t.compare.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapeti_core::types::create_body;

    fn store_with(ids: &[u8]) -> Store {
        let mut store = Store::new(Settings::default());
        let blocks: Vec<Block> = ids.iter().map(|id| Block::new(create_body(*id))).collect();
        store.tape_mut(0).load("t.tzx".into(), None, blocks, None);
        store
    }

    fn ids(store: &Store, side: Side) -> Vec<u8> {
        store.tape(side).blocks.iter().map(Block::id).collect()
    }

    #[test]
    fn a_fresh_tape_is_clean_and_an_edit_makes_it_dirty() {
        let mut store = store_with(&[0x10, 0x20]);
        assert!(!store.tape(0).dirty());
        store.delete_indices(0, vec![1]);
        assert!(store.tape(0).dirty());
        assert_eq!(ids(&store, 0), vec![0x10]);
    }

    /// The point of the generation counter: undoing back to the saved version
    /// clears `dirty`, and redoing sets it again.
    /// Deleting the last block leaves a pane with a file name on it and nothing
    /// under it, which promises a tape that is not there and would save as an
    /// empty file. It becomes a new tape instead — and undo brings the old one
    /// back whole, name and all.
    #[test]
    fn emptying_a_tape_makes_it_a_new_one() {
        let mut store = store_with(&[0x10, 0x20]);
        store.tape_mut(0).path = Some(PathBuf::from("/tapes/t.tzx"));
        store.tape_mut(0).loaded_version = Some(Version { major: 1, minor: 20 });
        store.delete_indices(0, vec![0, 1]);

        let t = store.tape(0);
        assert_eq!(t.name, "new");
        assert_eq!(t.path, None);
        assert_eq!(t.loaded_version, None);
        assert_eq!(t.cursor, -1);
        assert!(!t.dirty(), "an empty tape has nothing to save, so nothing to ask about");

        store.undo(0);
        let t = store.tape(0);
        assert_eq!(ids(&store, 0), vec![0x10, 0x20]);
        assert_eq!(t.name, "t.tzx");
        assert_eq!(t.path, Some(PathBuf::from("/tapes/t.tzx")));
        assert_eq!(t.loaded_version, Some(Version { major: 1, minor: 20 }));
        assert!(!t.dirty(), "back to what was loaded");

        store.redo(0);
        assert_eq!(store.tape(0).name, "new");
        assert!(store.tape(0).blocks.is_empty());
        assert!(!store.tape(0).dirty());
    }

    /// A snapshot remembers what the tape was saved against, so saving has to
    /// reach the ones already on the stack: undoing past a save is dirty again.
    #[test]
    fn undoing_past_a_save_is_dirty() {
        let mut store = store_with(&[0x10, 0x20]);
        store.delete_indices(0, vec![0]);
        store.tape_mut(0).mark_saved();
        assert!(!store.tape(0).dirty());
        store.undo(0);
        assert_eq!(ids(&store, 0), vec![0x10, 0x20]);
        assert!(store.tape(0).dirty(), "the file on disk has one block, the tape has two");
    }

    #[test]
    fn undo_back_to_the_saved_version_is_clean_again() {
        let mut store = store_with(&[0x10, 0x20]);
        store.delete_indices(0, vec![0]);
        assert!(store.tape(0).dirty());
        store.undo(0);
        assert!(!store.tape(0).dirty());
        assert_eq!(ids(&store, 0), vec![0x10, 0x20]);
        store.redo(0);
        assert!(store.tape(0).dirty());
        assert_eq!(ids(&store, 0), vec![0x20]);
    }

    #[test]
    fn saving_rebases_dirty_on_the_current_version() {
        let mut store = store_with(&[0x10]);
        store.insert_blocks(0, 1, vec![Block::new(create_body(0x20))]);
        store.tape_mut(0).mark_saved();
        assert!(!store.tape(0).dirty());
        store.undo(0);
        assert!(store.tape(0).dirty(), "undoing past the save point is a change again");
    }

    #[test]
    fn a_locked_tape_refuses_structural_edits() {
        let mut store = store_with(&[0x10, 0x20]);
        store.locked = true;
        store.delete_indices(0, vec![0]);
        assert_eq!(ids(&store, 0), vec![0x10, 0x20]);
        assert!(!store.tape(0).dirty());
    }

    #[test]
    fn a_collapsed_group_acts_as_one_unit() {
        // group start, data, group end, pause
        let mut store = store_with(&[0x21, 0x10, 0x22, 0x20]);
        let start_uid = store.tape(0).blocks[0].uid;
        store.set_cursor(0, 0, SelectMode::Single);
        assert_eq!(store.tape(0).unit_indices(0), vec![0], "expanded: the start block alone");
        store.toggle_collapse(0, start_uid);
        assert_eq!(store.tape(0).unit_indices(0), vec![0, 1, 2], "collapsed: the whole group");
        store.delete_unit(0);
        assert_eq!(ids(&store, 0), vec![0x20]);
    }

    #[test]
    fn the_selection_is_what_moves_when_a_selected_block_is_grabbed() {
        let mut store = store_with(&[0x10, 0x11, 0x12, 0x13]);
        store.set_cursor(0, 1, SelectMode::Single);
        store.set_cursor(0, 2, SelectMode::Range);
        assert_eq!(store.tape(0).unit_indices(2), vec![1, 2]);
        store.move_blocks(0, vec![1, 2], 0, 0, false);
        assert_eq!(ids(&store, 0), vec![0x11, 0x12, 0x10, 0x13]);
        assert_eq!(store.tape(0).cursor, 0);
    }

    #[test]
    fn move_up_and_down_step_over_neighbours() {
        let mut store = store_with(&[0x10, 0x11, 0x12]);
        store.set_cursor(0, 2, SelectMode::Single);
        store.move_unit(0, -1);
        assert_eq!(ids(&store, 0), vec![0x10, 0x12, 0x11]);
        store.move_unit(0, 1);
        assert_eq!(ids(&store, 0), vec![0x10, 0x11, 0x12]);
        store.set_cursor(0, 0, SelectMode::Single);
        store.move_unit(0, -1);
        assert_eq!(ids(&store, 0), vec![0x10, 0x11, 0x12], "the first block cannot move up");
    }

    #[test]
    fn move_down_jumps_a_whole_collapsed_group() {
        let mut store = store_with(&[0x10, 0x21, 0x11, 0x22]);
        let group = store.tape(0).blocks[1].uid;
        store.toggle_collapse(0, group);
        store.set_cursor(0, 0, SelectMode::Single);
        store.move_unit(0, 1);
        assert_eq!(ids(&store, 0), vec![0x21, 0x11, 0x22, 0x10]);
    }

    #[test]
    fn grouping_wraps_the_selection() {
        let mut store = store_with(&[0x10, 0x11, 0x12]);
        store.set_cursor(0, 0, SelectMode::Single);
        store.set_cursor(0, 1, SelectMode::Range);
        store.group_selection(0, "Loader");
        assert_eq!(ids(&store, 0), vec![0x21, 0x10, 0x11, 0x22, 0x12]);
        assert_eq!(store.tape(0).cursor, 0);
    }

    #[test]
    fn cut_and_paste_move_blocks_through_the_clipboard() {
        let mut store = store_with(&[0x10, 0x11, 0x12]);
        store.set_cursor(0, 0, SelectMode::Single);
        store.cut_unit(0);
        assert_eq!(ids(&store, 0), vec![0x11, 0x12]);
        store.set_cursor(0, 1, SelectMode::Single);
        store.paste(0);
        assert_eq!(ids(&store, 0), vec![0x11, 0x12, 0x10]);
    }

    /// Pasted blocks are copies: editing one must not touch the original.
    #[test]
    fn pasted_blocks_get_fresh_uids() {
        let mut store = store_with(&[0x10]);
        store.set_cursor(0, 0, SelectMode::Single);
        store.copy_unit(0);
        store.paste(0);
        let uids: Vec<Uid> = store.tape(0).blocks.iter().map(|b| b.uid).collect();
        assert_eq!(uids.len(), 2);
        assert_ne!(uids[0], uids[1]);
    }

    #[test]
    fn duplicate_puts_the_copy_after_the_unit() {
        let mut store = store_with(&[0x10, 0x11]);
        store.set_cursor(0, 0, SelectMode::Single);
        store.duplicate_unit(0);
        assert_eq!(ids(&store, 0), vec![0x10, 0x10, 0x11]);
    }

    #[test]
    fn dragging_between_panes_copies_or_moves() {
        let mut store = store_with(&[0x10, 0x11]);
        store.move_blocks(0, vec![0], 1, 0, true);
        assert_eq!(ids(&store, 0), vec![0x10, 0x11], "a copy leaves the source alone");
        assert_eq!(ids(&store, 1), vec![0x10]);
        store.move_blocks(0, vec![1], 1, 1, false);
        assert_eq!(ids(&store, 0), vec![0x10]);
        assert_eq!(ids(&store, 1), vec![0x10, 0x11]);
        assert_eq!(store.active, 1, "the pane dropped into becomes active");
    }

    #[test]
    fn click_modes_follow_the_web_selection_rules() {
        let mut store = store_with(&[0x10, 0x11, 0x12]);
        store.set_cursor(0, 0, SelectMode::Single);
        assert_eq!(store.tape(0).selected.len(), 1);
        store.set_cursor(0, 2, SelectMode::Range);
        assert_eq!(store.tape(0).selected.len(), 3);
        store.set_cursor(0, 1, SelectMode::Toggle);
        assert_eq!(store.tape(0).selected.len(), 2, "toggle removes a selected block");
        store.set_cursor(0, 1, SelectMode::Keep);
        assert_eq!(store.tape(0).selected.len(), 2, "keep leaves the selection alone");
        store.set_cursor(0, 1, SelectMode::Single);
        assert_eq!(store.tape(0).selected.len(), 1);
    }

    #[test]
    fn deleting_drops_the_deleted_uids_from_the_selection() {
        let mut store = store_with(&[0x10, 0x11, 0x12]);
        store.select_all(0);
        store.delete_indices(0, vec![1]);
        assert_eq!(store.tape(0).selected.len(), 0, "a delete clears the selection");
        store.select_all(0);
        assert_eq!(store.tape(0).selected.len(), 2);
    }

    #[test]
    fn collapse_all_collapses_every_group_and_loop() {
        let mut store = store_with(&[0x21, 0x10, 0x22, 0x24, 0x11, 0x25]);
        store.collapse_all(0, true);
        assert_eq!(store.tape(0).collapsed.len(), 2);
        store.collapse_all(0, false);
        assert!(store.tape(0).collapsed.is_empty());
    }

    #[test]
    fn comparing_two_tapes_marks_what_differs() {
        let mut store = store_with(&[0x10, 0x11]);
        let copy: Vec<Block> = store.tape(0).blocks.iter().map(Block::clone_fresh).collect();
        store.tape_mut(1).load("r.tzx".into(), None, copy, None);
        store.run_compare_tapes();
        assert!(store.tape(0).compare.values().all(|m| *m != Mark::Diff));
        store.delete_indices(1, vec![1]);
        store.run_compare_tapes();
        assert!(store.tape(0).compare.values().any(|m| *m == Mark::Diff));
        store.clear_compare();
        assert!(store.tape(0).compare.is_empty());
    }

    #[test]
    fn find_match_marks_the_blocks_that_match_the_cursor() {
        // A pause is not a data block, so "data" compare never calls it a match.
        let mut store = store_with(&[0x10, 0x20, 0x10]);
        store.set_cursor(0, 0, SelectMode::Single);
        store.run_find_match(0);
        // The needle itself is skipped; the other data block matches.
        assert_eq!(store.tape(0).compare.len(), 1);
        assert_eq!(store.tape(0).compare.values().next(), Some(&Mark::Match));
    }
}
