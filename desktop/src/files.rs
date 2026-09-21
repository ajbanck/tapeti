//! Getting tapes in and out of the store: the port of `src/state/files.ts`, with
//! `rfd` where the web build has `src/platform/`.
//!
//! There is no platform adapter here. The web app needs one because a browser
//! download and a native save dialog have nothing in common; a native binary has
//! only the second, so the boundary the adapter existed to hide is gone.

use std::path::{Path, PathBuf};

use tapeti_core::describe::file_blocks;
use tapeti_core::hash::file_hashes;
use tapeti_core::parser::{is_tzx, parse_tape};
use tapeti_core::snapshot::{parse_snapshot, snapshot_to_blocks, LoaderOptions, SnapshotKind, SPEED_BPS};
use tapeti_core::writer::{save_version, serialize_tap, serialize_tzx, Version};

use crate::fmt;
use crate::state::{Side, Store};

pub const TAPE_EXTENSIONS: [&str; 4] = ["tzx", "tap", "TZX", "TAP"];
/// Snapshots open too: as the tape that loads them, once the dialog has been answered.
pub const SNAPSHOT_EXTENSIONS: [&str; 4] = ["z80", "sna", "Z80", "SNA"];

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The tape's name without a `.tzx`/`.tap` extension (or a snapshot's), or `tape`.
pub fn stem(name: &str) -> String {
    let known = TAPE_EXTENSIONS.iter().chain(&SNAPSHOT_EXTENSIONS);
    let s = known.filter_map(|e| name.strip_suffix(e)?.strip_suffix('.')).next().unwrap_or(name);
    if s.is_empty() {
        "tape".to_string()
    } else {
        s.to_string()
    }
}

pub fn new_tape(store: &mut Store, side: Side) {
    *store.tape_mut(side) = crate::state::TapeState::empty("new");
    store.active = side;
}

/// Parse `bytes` into the tape, or insert them at the cursor.
pub fn load_bytes(
    store: &mut Store,
    side: Side,
    name: &str,
    bytes: &[u8],
    insert_at_cursor: bool,
    path: Option<PathBuf>,
) {
    // Inserting something that is no tape: it goes in as a data block, once the
    // dialog has said where it loads. (Opening still reads anything as a TAP,
    // which is what a tape with an odd extension needs.)
    let is_tap = name.rsplit('.').next().is_some_and(|e| e.eq_ignore_ascii_case("tap"));
    if insert_at_cursor && !is_tzx(bytes) && !is_tap && SnapshotKind::from_name(name).is_none() {
        store.dialog = Some(crate::dialogs::Dialog::data_file(side, name, bytes.to_vec()));
        return;
    }
    if let Some(kind) = SnapshotKind::from_name(name) {
        // A snapshot becomes a tape only once the dialog has its answers.
        match parse_snapshot(bytes, kind) {
            Ok(snap) => {
                store.dialog = Some(crate::dialogs::Dialog::snapshot(side, name, snap, insert_at_cursor))
            }
            Err(e) => store.message("Cannot load file", vec![e]),
        }
        return;
    }
    let parsed = match parse_tape(bytes) {
        Ok(p) => p,
        Err(e) => {
            store.message("Cannot load file", vec![e]);
            return;
        }
    };
    if !parsed.warnings.is_empty() {
        store.message(&format!("Warnings while loading {name}"), parsed.warnings.clone());
    }
    let count = parsed.blocks.len();
    let (major, minor) = (parsed.major, parsed.minor);
    if insert_at_cursor {
        let t = store.tape(side);
        let at = if t.cursor < 0 { t.blocks.len() } else { t.cursor as usize };
        store.insert_blocks(side, at, parsed.blocks);
    } else {
        // TAP files carry no version; only a real TZX header is worth remembering.
        let version = is_tzx(bytes).then_some(Version { major, minor });
        let t = store.tape_mut(side);
        t.load(name.to_string(), path, parsed.blocks, version);
        t.file_hashes = Some(file_hashes(bytes));
    }
    store.active = side;
    store.set_status(format!("Loaded {name}: {count} blocks, TZX v{major}.{minor:02}"));
}

/// The data file dialog's OK: the file as a header and its data, at the cursor.
pub fn insert_data_file(
    store: &mut Store,
    side: Side,
    name: &str,
    bytes: &[u8],
    address: u16,
    with_header: bool,
) {
    let bodies = match file_blocks(name, bytes, address, with_header) {
        Ok(b) => b,
        Err(e) => {
            store.message("Cannot insert file", vec![e]);
            return;
        }
    };
    let t = store.tape(side);
    let at = if t.cursor < 0 { t.blocks.len() } else { t.cursor as usize };
    store.insert_blocks(side, at, bodies.into_iter().map(tapeti_core::types::Block::new).collect());
    store.active = side;
    store.set_status(format!("Inserted {} bytes as \"{}\"", bytes.len(), name.trim_end()));
}

/// The snapshot import dialog's OK: build the tape that loads the snapshot.
pub fn import_snapshot(
    store: &mut Store,
    side: Side,
    snap: &tapeti_core::snapshot::Snapshot,
    opts: &LoaderOptions,
    insert_at_cursor: bool,
) {
    let blocks = match snapshot_to_blocks(snap, opts) {
        Ok(b) => b,
        Err(e) => {
            store.message("Cannot import snapshot", vec![e]);
            return;
        }
    };
    let count = blocks.len();
    if insert_at_cursor {
        let t = store.tape(side);
        let at = if t.cursor < 0 { t.blocks.len() } else { t.cursor as usize };
        store.insert_blocks(side, at, blocks);
    } else {
        // No path: saving must ask where, not write over the snapshot.
        store.tape_mut(side).load(opts.name.to_string(), None, blocks, None);
    }
    store.active = side;
    let bps = SPEED_BPS[usize::from(opts.speed)];
    store.set_status(format!("Imported {}: {count} blocks, loading at {bps} bps", opts.name));
}

fn tape_dialog() -> rfd::FileDialog {
    let all: Vec<&str> = TAPE_EXTENSIONS.iter().chain(&SNAPSHOT_EXTENSIONS).copied().collect();
    rfd::FileDialog::new()
        .add_filter("Tape images and snapshots", &all)
        .add_filter("Tape images", &TAPE_EXTENSIONS)
        .add_filter("Snapshots (imported as a tape)", &SNAPSHOT_EXTENSIONS)
        .add_filter("All files", &["*"])
}

/// Show the open dialog and load what was chosen.
pub fn pick_and_open(store: &mut Store, side: Side, insert_at_cursor: bool) {
    let picked = if insert_at_cursor {
        tape_dialog().pick_files()
    } else {
        tape_dialog().pick_file().map(|p| vec![p])
    };
    let Some(paths) = picked else { return };
    open_paths(store, side, &paths, insert_at_cursor);
}

pub fn open_paths(store: &mut Store, side: Side, paths: &[PathBuf], insert_at_cursor: bool) {
    for path in paths {
        match std::fs::read(path) {
            Ok(bytes) => {
                load_bytes(store, side, &file_name(path), &bytes, insert_at_cursor, Some(path.clone()))
            }
            Err(e) => store.message("Cannot load file", vec![format!("{}: {e}", path.display())]),
        }
    }
}

/// Files the OS asked us to open: the left tape unless it has unsaved work.
pub fn open_with(store: &mut Store, paths: &[PathBuf]) {
    let mut side: Side = usize::from(store.tape(0).dirty());
    for path in paths {
        open_paths(store, side, std::slice::from_ref(path), false);
        side = crate::state::other(side);
    }
}

/// Tapes the panic hook wrote out last time go back into their panes, unsaved as
/// they were, and the files go. A pane that already has a tape in it (one named
/// on the command line) keeps it, and the rescued file stays where it is.
pub fn restore_rescued(store: &mut Store) {
    if let Some(dir) = crate::settings::config_dir() {
        restore_rescued_from(store, &dir);
    }
}

pub fn restore_rescued_from(store: &mut Store, dir: &Path) {
    let mut left_behind = Vec::new();
    let mut restored = false;
    for side in 0..2 {
        let path = dir.join(crate::crashlog::rescue_name(side));
        if !path.exists() {
            continue;
        }
        let parsed = std::fs::read(&path).ok().and_then(|b| parse_tape(&b).ok());
        match parsed {
            Some(p) if store.tape(side).blocks.is_empty() => {
                let t = store.tape_mut(side);
                t.load(
                    format!("rescued-{}.tzx", if side == 0 { "left" } else { "right" }),
                    None,
                    p.blocks,
                    None,
                );
                t.mark_unsaved();
                restored = true;
                let _ = std::fs::remove_file(&path);
            }
            _ => left_behind.push(path.display().to_string()),
        }
    }
    if restored {
        let mut lines = vec![
            "Tapeti stopped unexpectedly last time. The tapes with unsaved changes are back in their panes;"
                .to_string(),
            "save them under a name of your choosing.".to_string(),
        ];
        lines.extend(left_behind.iter().map(|p| format!("Also kept, and not opened: {p}")));
        store.message("Unsaved tapes recovered", lines);
    } else if !left_behind.is_empty() {
        let mut lines =
            vec!["Tapeti stopped unexpectedly last time. Unsaved tapes were kept as:".to_string()];
        lines.extend(left_behind);
        store.message("Unsaved tapes recovered", lines);
    }
}

/// Write `bytes` over the file at `path` so that a failure — a full disk —
/// leaves the old file whole: to a temporary beside it first, then renamed over
/// it. With `backup` the old file is kept as `name.tzx.bak`, which is then
/// always the version before the last save.
fn write_in_place(path: &Path, bytes: &[u8], backup: bool) -> std::io::Result<()> {
    let beside = |suffix: &str| {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        path.with_file_name(name)
    };
    if backup && path.exists() {
        std::fs::copy(path, beside(".bak"))?;
    }
    let tmp = beside(".tmp");
    std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Pick one arbitrary file (the data window's Append / Replace from file).
pub fn pick_any_file() -> Option<(String, Vec<u8>)> {
    let path = rfd::FileDialog::new().add_filter("All files", &["*"]).pick_file()?;
    std::fs::read(&path).ok().map(|b| (file_name(&path), b))
}

/// Save bytes somewhere the user picks. Returns the path written.
pub fn save_bytes(suggested: &str, filter: (&str, &[&str]), bytes: &[u8]) -> Option<PathBuf> {
    let path = rfd::FileDialog::new().set_file_name(suggested).add_filter(filter.0, filter.1).save_file()?;
    match std::fs::write(&path, bytes) {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!("cannot write {}: {e}", path.display());
            None
        }
    }
}

/// Save as TZX. With `save_as` false the file it was opened from is written back.
pub fn save_tzx(store: &mut Store, side: Side, save_as: bool) {
    let t = store.tape(side);
    let v = save_version(&t.blocks, t.loaded_version);
    let bytes = serialize_tzx(&t.blocks, Some(v));
    let in_place = (!save_as)
        .then(|| t.path.clone())
        .flatten()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("tzx")));
    let written = match in_place {
        Some(path) => match write_in_place(&path, &bytes, store.settings.backup) {
            Ok(()) => Some(path),
            Err(e) => {
                store.message("Cannot save", vec![format!("{}: {e}", path.display())]);
                return;
            }
        },
        None => save_bytes(&(stem(&t.name) + ".tzx"), ("TZX tape image", &["tzx"]), &bytes),
    };
    let Some(path) = written else { return };
    let name = file_name(&path);
    let t = store.tape_mut(side);
    t.loaded_version = Some(v);
    t.file_hashes = Some(file_hashes(&bytes));
    t.name = name.clone();
    t.path = Some(path);
    t.mark_saved();
    store.set_status(format!("Saved {name} as TZX v{}.{:02}", v.major, v.minor));
}

pub fn save_tap(store: &mut Store, side: Side) {
    let t = store.tape(side);
    let (bytes, skipped) = serialize_tap(&t.blocks);
    let Some(path) = save_bytes(&(stem(&t.name) + ".tap"), ("TAP tape image", &["tap"]), &bytes) else {
        return;
    };
    let name = file_name(&path);
    if skipped.is_empty() {
        let t = store.tape_mut(side);
        t.name = name.clone();
        t.path = Some(path);
        t.file_hashes = Some(file_hashes(&bytes));
        t.mark_saved();
        store.set_status(format!("Saved {name}"));
    } else {
        let zero_based = store.settings.zero_based;
        let mut lines = vec!["TAP files can only hold data blocks. These blocks were skipped:".to_string()];
        lines.extend(skipped.iter().map(|i| format!("#{}", fmt::block_no(*i as usize, zero_based))));
        lines.push("Turbo/pure data blocks were written as standard blocks (their timings are lost).".into());
        store.message("TAP export", lines);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::dialogs::Dialog;
    use crate::settings::Settings;
    use tapeti_core::snapshot::Snapshot;

    /// A 48K .sna of synthetic memory, with the program counter on its stack.
    fn sna() -> Vec<u8> {
        let mut file = vec![0u8; 27 + 49152];
        file[23..25].copy_from_slice(&0x8000u16.to_le_bytes());
        file[26] = 3;
        file[27..27 + 6144].fill(0x55);
        file[27 + 0x4000..27 + 0x4002].copy_from_slice(&0x9123u16.to_le_bytes());
        file
    }

    pub fn snapshot() -> Snapshot {
        parse_snapshot(&sna(), SnapshotKind::Sna).unwrap()
    }

    #[test]
    fn a_snapshot_asks_first_and_loads_on_ok() {
        let mut store = Store::new(Settings::default());
        let path = PathBuf::from("/somewhere/game.sna");
        load_bytes(&mut store, 0, "game.sna", &sna(), false, Some(path));
        assert!(store.tape(0).blocks.is_empty(), "nothing loads before the dialog is answered");
        let Some(Dialog::Snapshot(s)) = store.dialog.take() else { panic!("no import dialog") };
        assert_eq!((s.border, s.speed, s.snap.is_128k), (3, 2, false));

        let opts = LoaderOptions {
            name: &s.name,
            speed: s.speed,
            border: s.border,
            compress_all: false,
            screen: None,
        };
        import_snapshot(&mut store, 0, &s.snap, &opts, false);
        let t = store.tape(0);
        // text, header, program and three pages; the loader sat on zeroes
        assert_eq!(t.blocks.iter().map(|b| b.id()).collect::<Vec<_>>(), [0x30, 0x10, 0x10, 0x11, 0x11, 0x11]);
        assert_eq!(t.name, "game.sna");
        // Saving must ask where: the snapshot is not the tape's file.
        assert!(t.path.is_none() && t.loaded_version.is_none() && !t.dirty());
        assert!(t.file_hashes.is_none(), "a snapshot's tape is no file, so it has no checksums");
        assert_eq!(stem(&t.name), "game");

        // Inserting goes to the cursor of the tape that is there.
        import_snapshot(&mut store, 0, &s.snap, &opts, true);
        assert_eq!(store.tape(0).blocks.len(), 12);
    }

    #[test]
    fn inserting_a_file_that_is_no_tape_asks_where_it_loads() {
        let mut store = Store::new(Settings::default());
        let screen = vec![0x38u8; 6912];
        load_bytes(&mut store, 0, "Title.scr", &screen, true, None);
        let Some(Dialog::DataFile(s)) = store.dialog.take() else { panic!("no data file dialog") };
        assert_eq!((s.name.as_str(), s.address, s.with_header), ("Title", 16384, true));
        insert_data_file(&mut store, 0, &s.name, &s.bytes, s.address as u16, s.with_header);
        let t = store.tape(0);
        assert_eq!(t.blocks.iter().map(|b| b.body.data().unwrap().len()).collect::<Vec<_>>(), [19, 6914]);
        assert!(t.dirty());

        // A TAP by name and a TZX by signature are still tapes, and opening (not
        // inserting) reads anything as one, as it always did.
        load_bytes(&mut store, 0, "x.tap", &[2, 0, 0xff, 0xff], true, None);
        assert!(store.dialog.is_none());
        load_bytes(&mut store, 1, "odd.bin", &[2, 0, 0xff, 0xff], false, None);
        assert!(store.dialog.is_none());
        assert_eq!(store.tape(1).blocks.len(), 1);
    }

    #[test]
    fn saving_in_place_keeps_the_old_file_when_asked() {
        let dir = std::env::temp_dir().join(format!("tapeti-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.tzx");
        std::fs::write(&path, b"first").unwrap();

        write_in_place(&path, b"second", false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(!dir.join("game.tzx.bak").exists() && !dir.join("game.tzx.tmp").exists());

        write_in_place(&path, b"third", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"third");
        assert_eq!(std::fs::read(dir.join("game.tzx.bak")).unwrap(), b"second");

        // Nowhere to write: the error comes back and nothing is left lying about.
        assert!(write_in_place(&dir.join("missing").join("x.tzx"), b"x", true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The checksums are of the file: read at load, kept through edits (the note in
    /// tape info says they no longer describe the pane), gone with an emptied tape
    /// and back on undo, and after a save those of what was written.
    #[test]
    fn the_checksums_follow_the_file() {
        use tapeti_core::hash::file_hashes;
        use tapeti_core::types::{create_body, Block};
        let dir = std::env::temp_dir().join(format!("tapeti-hashes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.tzx");
        let file = serialize_tzx(&[Block::new(create_body(0x10))], Some(Version { major: 1, minor: 20 }));
        std::fs::write(&path, &file).unwrap();

        let mut store = Store::new(Settings::default());
        load_bytes(&mut store, 0, "game.tzx", &file, false, Some(path.clone()));
        let read = Some(file_hashes(&file));
        assert_eq!(store.tape(0).file_hashes, read);

        store.insert_blocks(0, 1, vec![Block::new(create_body(0x20))]);
        assert!(store.tape(0).dirty());
        assert_eq!(store.tape(0).file_hashes, read, "an edit does not change the file");
        store.delete_indices(0, vec![0, 1]);
        assert_eq!(store.tape(0).file_hashes, None, "an emptied tape is a new one");
        store.undo(0);
        assert_eq!(store.tape(0).file_hashes, read);

        save_tzx(&mut store, 0, false);
        let written = std::fs::read(&path).unwrap();
        assert_ne!(written, file);
        let saved = Some(file_hashes(&written));
        assert_eq!(store.tape(0).file_hashes, saved);
        store.undo(0);
        assert_eq!(store.tape(0).file_hashes, saved, "undo past a save leaves the file as saved");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_snapshot_says_so() {
        let mut store = Store::new(Settings::default());
        load_bytes(&mut store, 0, "broken.z80", &[0; 5], false, None);
        assert!(matches!(&store.dialog, Some(Dialog::Message { title, .. }) if title == "Cannot load file"));
    }

    #[test]
    fn stems_drop_tape_and_snapshot_extensions() {
        assert_eq!(stem("a.tzx"), "a");
        assert_eq!(stem("a.TAP"), "a");
        assert_eq!(stem("a.z80"), "a");
        assert_eq!(stem("a.wav"), "a.wav");
        assert_eq!(stem(".tzx"), "tape");
    }
}
