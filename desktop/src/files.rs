//! Getting tapes in and out of the store: the port of `src/state/files.ts`, with
//! `rfd` where the web build has `src/platform/`.
//!
//! There is no platform adapter here. The web app needs one because a browser
//! download and a native save dialog have nothing in common; a native binary has
//! only the second, so the boundary the adapter existed to hide is gone.

use std::path::{Path, PathBuf};

use tapeti_core::parser::{is_tzx, parse_tape};
use tapeti_core::writer::{save_version, serialize_tap, serialize_tzx, Version};

use crate::fmt;
use crate::state::{Side, Store};

pub const TAPE_EXTENSIONS: [&str; 4] = ["tzx", "tap", "TZX", "TAP"];

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The tape's name without a `.tzx`/`.tap` extension, or `tape`.
pub fn stem(name: &str) -> String {
    let s = name.strip_suffix(".tzx").or_else(|| name.strip_suffix(".TZX"));
    let s = s.or_else(|| name.strip_suffix(".tap")).or_else(|| name.strip_suffix(".TAP"));
    let s = s.unwrap_or(name);
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
        store.tape_mut(side).load(name.to_string(), path, parsed.blocks, version);
    }
    store.active = side;
    store.set_status(format!("Loaded {name}: {count} blocks, TZX v{major}.{minor:02}"));
}

fn tape_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("Tape images", &TAPE_EXTENSIONS).add_filter("All files", &["*"])
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
        Some(path) => match std::fs::write(&path, &bytes) {
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
