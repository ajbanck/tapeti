//! Running a command, the port of `src/state/commands.ts`.
//!
//! The web table carries label, shortcut, enabled rule and behaviour in one
//! object. Here the first three live in `menutable.rs`, because the platform
//! menu is built from a static table before the app exists, and the behaviour
//! lives in [`run`]. The ids are the same in all three menus, which is what
//! `tests/menu.rs` pins.

use egui::{Key, Modifiers};

use crate::actions::{self, Scope, Then};
use crate::app::App;
use crate::dialogs::Dialog;
use crate::files;
use crate::menutable::{self, MenuState, Need};
use crate::state::{other, Side};

/// The state the enabled rules ask about, for `side`.
pub fn menu_state(app: &App, side: Side) -> MenuState {
    let t = app.store.tape(side);
    MenuState {
        blocks: t.blocks.len(),
        has_cursor: t.has_cursor(),
        can_undo: t.can_undo(),
        can_redo: t.can_redo(),
        clipboard: !app.store.clipboard.is_empty(),
        playing: app.player.playing(),
        collapsible: t.cursor >= 0 && t.ranges().contains_key(&(t.cursor as usize)),
    }
}

pub fn enabled(app: &App, id: &str, side: Side) -> bool {
    let state = menu_state(app, side);
    match id {
        // The context menu's own item; it is in no menu bar, as on the web.
        "toggle-collapse" => Need::Collapsible.met(&state),
        _ => menutable::item(id).is_some_and(|i| i.need.met(&state)),
    }
}

/// The label a menu shows for `id`. Only one is dynamic, the way `commands.ts`
/// makes only one a function.
pub fn label(app: &App, id: &str, side: Side) -> String {
    if id == "toggle-collapse" {
        let t = app.store.tape(side);
        let collapsed = t
            .cursor_block()
            .is_some_and(|b| t.collapsed.contains(&b.uid) && t.ranges().contains_key(&(t.cursor as usize)));
        return if collapsed { "Expand Group/Loop".into() } else { "Collapse Group/Loop".into() };
    }
    menutable::item(id).map_or_else(|| id.to_string(), |i| i.label.to_string())
}

pub fn checked(app: &App, id: &str) -> bool {
    match id {
        "toggle-hex" => app.store.hex,
        "opt-hex-bytes" => app.store.settings.hex_bytes,
        "opt-zero-based" => app.store.settings.zero_based,
        "toggle-lock" => app.store.locked,
        _ => false,
    }
}

/// The Edit commands, when a text field has the keyboard: they act on the field
/// rather than on the tape, which is the rule `handleNativeMenu` follows in
/// `src/ui/App.tsx`. On macOS the platform menu owns ⌘X/C/V/A/Z, so without this
/// the keystroke would never reach the field it was aimed at.
///
/// egui's `TextEdit` reads these off the input queue, and the queue is still
/// empty when the menu is drained at the top of the frame, so the field sees
/// them this frame.
fn text_field_edit(app: &App, id: &str) -> bool {
    let key = |key: Key, modifiers: Modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let event = match id {
        "copy" => egui::Event::Copy,
        "cut" => egui::Event::Cut,
        "paste" => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => egui::Event::Paste(text),
            Err(_) => return true, // nothing to paste; the tape must not get it either
        },
        "select-all" => key(Key::A, Modifiers::COMMAND),
        "undo" => key(Key::Z, Modifiers::COMMAND),
        "redo" => key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
        _ => return false,
    };
    app.ctx.input_mut(|i| i.events.push(event));
    true
}

/// Run a command on `side` if it is enabled. Returns whether it ran.
pub fn run(app: &mut App, id: &str, side: Side) -> bool {
    if app.text_focus && text_field_edit(app, id) {
        return true;
    }
    if !enabled(app, id, side) {
        return false;
    }
    match id {
        // ---- file
        "new" => actions::confirm_discard(app, side, Then::NewTape(side)),
        "open" => actions::confirm_discard(app, side, Then::Open(side)),
        "open-other" => {
            let o = other(side);
            actions::confirm_discard(app, o, Then::Open(o));
        }
        "insert-file" => files::pick_and_open(&mut app.store, side, true),
        "save" => {
            let save_as = app.store.tape(side).path.is_none();
            files::save_tzx(&mut app.store, side, save_as);
        }
        "save-as" => files::save_tzx(&mut app.store, side, true),
        "save-tap" => files::save_tap(&mut app.store, side),
        "export-wav" => app.store.dialog = Some(Dialog::wav(side)),
        // ---- edit
        "undo" => app.store.undo(side),
        "redo" => app.store.redo(side),
        "cut" => app.store.cut_unit(side),
        "copy" => app.store.copy_unit(side),
        "paste" => app.store.paste(side),
        "duplicate" => app.store.duplicate_unit(side),
        "delete" => app.store.delete_unit(side),
        "select-all" => app.store.select_all(side),
        // ---- block
        "insert" => app.store.dialog = Some(Dialog::insert(side)),
        "view-data" => actions::view_data(app, side, false),
        "view-as-one" => actions::view_data(app, side, true),
        "move-up" => app.store.move_unit(side, -1),
        "move-down" => app.store.move_unit(side, 1),
        "group" => app.store.group_selection(side, "Group"),
        "toggle-collapse" => {
            if let Some(uid) = app.store.tape(side).cursor_block().map(|b| b.uid) {
                app.store.toggle_collapse(side, uid);
            }
        }
        "collapse-all" => app.store.collapse_all(side, true),
        "expand-all" => app.store.collapse_all(side, false),
        "select-program" => actions::select_program(app, side),
        "extract" => actions::extract_to_other_pane(app, side),
        "find-match" => app.store.run_find_match(side),
        "set-timings" => actions::set_selection_timings(app, side),
        // ---- tape
        "play" => actions::play_tape(app, side, false),
        "play-cursor" => actions::play_tape(app, side, true),
        "play-selection" => actions::play_selection(app, side),
        "stop" => app.player.stop(),
        "emu-tape" => actions::open_in_emulator(app, side, Scope::Tape),
        "emu-cursor" => actions::open_in_emulator(app, side, Scope::Cursor),
        "emu-selection" => actions::open_in_emulator(app, side, Scope::Selection),
        "emu-settings" => {
            app.store.dialog = Some(Dialog::emulator(false, None, &app.store.settings));
        }
        "programs" => actions::open_program_picker(app, side),
        "tape-info" => app.store.dialog = Some(Dialog::tape_info(side)),
        "consistency" => app.store.dialog = Some(Dialog::consistency(side)),
        "compare" => app.store.run_compare_tapes(),
        "clear-compare" => app.store.clear_compare(),
        "switch-pane" => app.store.active = other(side),
        "toggle-lock" => app.store.toggle_lock(),
        // ---- options
        "toggle-hex" => {
            app.store.hex = !app.store.hex;
            app.store.touch_view();
        }
        "opt-hex-bytes" => {
            app.store.settings.hex_bytes = !app.store.settings.hex_bytes;
            app.store.settings.save();
            app.store.touch_view();
        }
        "opt-zero-based" => {
            app.store.settings.zero_based = !app.store.settings.zero_based;
            app.store.settings.save();
            app.store.touch_view();
        }
        // ---- help
        "shortcuts" => {
            app.store.dialog = Some(Dialog::message("Keyboard & mouse", shortcuts()));
        }
        "about" => app.store.dialog = Some(Dialog::about()),
        _ => {
            app.store.set_status(format!("Unknown command {id}"));
            return false;
        }
    }
    true
}

/// The block context menu, `blockMenu` in `src/ui/MenuBar.tsx`. An empty string
/// is a separator.
pub const CONTEXT_MENU: &[&str] = &[
    "insert",
    "view-data",
    "view-as-one",
    "",
    "cut",
    "copy",
    "paste",
    "duplicate",
    "delete",
    "",
    "move-up",
    "move-down",
    "group",
    "toggle-collapse",
    "collapse-all",
    "expand-all",
    "",
    "select-program",
    "extract",
    "",
    "emu-cursor",
    "emu-selection",
    "",
    "find-match",
    "set-timings",
];

/// Keys the window handles itself. On macOS the platform menu owns every
/// accelerator it declares, so those never arrive here; elsewhere the egui menu
/// bar draws the same table but delivers nothing, and this is what makes the
/// shortcuts work.
pub fn accelerator(item: &menutable::Item) -> Option<(Modifiers, Key)> {
    if item.keys.is_empty() {
        return None;
    }
    let mut mods = Modifiers::NONE;
    let mut key = None;
    for part in item.keys.split('+') {
        match part {
            "CmdOrCtrl" => mods |= Modifiers::COMMAND,
            "Shift" => mods |= Modifiers::SHIFT,
            "Alt" => mods |= Modifiers::ALT,
            other => key = Key::from_name(other),
        }
    }
    key.map(|k| (mods, k))
}

/// Keys with no menu accelerator, the tail of `KEY_COMMANDS` in `commands.ts`
/// plus the two the web view keeps for itself.
pub const PLAIN_KEYS: &[(Key, &str)] = &[
    (Key::Delete, "delete"),
    (Key::Backspace, "delete"),
    (Key::Insert, "insert"),
    (Key::Enter, "view-data"),
    (Key::Tab, "switch-pane"),
];

/// The list behind Help → Keyboard shortcuts, `SHORTCUTS` in `commands.ts`.
pub fn shortcuts() -> Vec<String> {
    let k = crate::fmt::key;
    let modk = if crate::fmt::IS_MAC { "⌘" } else { "Ctrl" };
    let alt = if crate::fmt::IS_MAC { "Option" } else { "Alt" };
    let ctrl = if crate::fmt::IS_MAC { "Control" } else { "Ctrl" };
    vec![
        format!("Click: make block current. Shift+click: select range. {modk}+click: toggle selection."),
        "Right click: context menu. Double click: view data, or collapse/expand a group or loop.".into(),
        format!("Drag & drop blocks to move them within or between tapes; hold {alt} or {ctrl} to copy."),
        "Drop a TZX/TAP file on a tape to open it; hold Shift to insert it at the cursor.".into(),
        format!(
            "{}: delete current block or selection. Insert or {}: insert block.",
            if crate::fmt::IS_MAC { "Delete (⌫)" } else { "Delete / Backspace" },
            k("Mod+Shift+N")
        ),
        format!(
            "↑ ↓: move cursor. {} {}: move block. Enter: view data. Escape: close window.",
            k("Mod+↑"),
            k("Mod+↓")
        ),
        format!(
            "{}, {}, {}, {}: cut, copy, paste, duplicate. {}, {}: undo, redo. {}: select all.",
            k("Mod+X"),
            k("Mod+C"),
            k("Mod+V"),
            k("Mod+D"),
            k("Mod+Z"),
            k("Mod+Shift+Z"),
            k("Mod+A")
        ),
        format!(
            "{}, {}: open / save the active tape. {}: open in the other pane. {}: group selection. {}: find match.",
            k("Mod+O"),
            k("Mod+S"),
            k("Mod+Shift+O"),
            k("Mod+G"),
            k("Mod+F")
        ),
        format!(
            "{}: pick a program (game) on a collection tape. {}: select the program at the cursor. {}: extract the selection to the other pane.",
            k("Mod+J"),
            k("Mod+Shift+A"),
            k("Mod+Shift+E")
        ),
        format!("{}: switch active tape. Space: play/stop tape from the cursor.", k("Tab")),
        format!(
            "{}, {}: open the tape / the tape from the cursor in the emulator (Options → Emulator…).",
            k("Mod+R"),
            k("Mod+Shift+R")
        ),
        "Data window: click a byte and type hex digits to edit; Tab switches to ASCII editing; arrows move.".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menu;
    use crate::settings::Settings;
    use crate::state::{SelectMode, Store};
    use tapeti_core::types::{create_body, Block, CREATABLE_IDS};

    /// Commands that would reach outside the process: a file dialog, the audio
    /// device, or the emulator. Everything else is run for real below.
    const REACHES_OUT: &[&str] = &[
        "open",
        "open-other",
        "insert-file",
        "save",
        "save-as",
        "save-tap",
        "play",
        "play-cursor",
        "play-selection",
        "emu-tape",
        "emu-cursor",
        "emu-selection",
        "emu-settings",
    ];

    fn app() -> App {
        let ctx = egui::Context::default();
        let mut store = Store::new(Settings::default());
        let blocks: Vec<Block> = CREATABLE_IDS.iter().map(|id| Block::new(create_body(*id))).collect();
        store.tape_mut(0).load("t.tzx".into(), None, blocks, None);
        let mut app = App::build(&ctx, Menu::headless(), store, std::time::Instant::now(), 0, false);
        app.store.set_cursor(0, 0, SelectMode::Single);
        app
    }

    fn command_ids() -> Vec<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/state/commands.ts");
        std::fs::read_to_string(path)
            .expect("src/state/commands.ts")
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("  '")?;
                let (id, tail) = rest.split_once('\'')?;
                tail.starts_with(':').then(|| id.to_string())
            })
            .collect()
    }

    /// Every id the web table declares is dispatched here — nothing falls
    /// through to the "unknown command" arm.
    #[test]
    fn every_command_id_is_dispatched() {
        let ids = command_ids();
        assert!(ids.len() > 40);
        for id in &ids {
            if REACHES_OUT.contains(&id.as_str()) {
                continue;
            }
            let mut app = app();
            // Enabled or not, the arm has to exist; `run` only returns false for
            // an unknown id once the enabled check has passed, so ask it directly.
            let known = run(&mut app, id, 0) || !enabled(&app, id, 0);
            assert!(known, "command {id:?} has no arm in commands::run");
            assert!(
                !app.store.status().starts_with("Unknown command"),
                "command {id:?} fell through to the unknown arm"
            );
        }
    }

    #[test]
    fn options_toggle_and_report_their_check_marks() {
        let mut app = app();
        for id in ["toggle-hex", "opt-hex-bytes", "opt-zero-based", "toggle-lock"] {
            let before = checked(&app, id);
            assert!(run(&mut app, id, 0), "{id} did not run");
            assert_ne!(checked(&app, id), before, "{id} did not toggle");
        }
    }

    #[test]
    fn a_disabled_command_does_nothing() {
        let mut app = app();
        // Nothing on the clipboard, so Paste is greyed out and must not run.
        assert!(!enabled(&app, "paste", 0));
        assert!(!run(&mut app, "paste", 0));
        assert_eq!(app.store.tape(0).blocks.len(), CREATABLE_IDS.len());
    }

    #[test]
    fn the_collapse_label_follows_the_cursor() {
        let mut app = app();
        let blocks: Vec<Block> = [0x21u8, 0x10, 0x22].iter().map(|id| Block::new(create_body(*id))).collect();
        app.store.tape_mut(0).load("t.tzx".into(), None, blocks, None);
        app.store.set_cursor(0, 0, SelectMode::Single);
        assert_eq!(label(&app, "toggle-collapse", 0), "Collapse Group/Loop");
        assert!(run(&mut app, "toggle-collapse", 0));
        assert_eq!(label(&app, "toggle-collapse", 0), "Expand Group/Loop");
        app.store.set_cursor(0, 1, SelectMode::Single);
        assert!(!enabled(&app, "toggle-collapse", 0), "a plain block cannot be collapsed");
    }

    /// With a text field focused, the Edit commands go to the field, not the
    /// tape: the rule `handleNativeMenu` follows on the web.
    #[test]
    fn edit_commands_reach_a_focused_text_field_first() {
        let mut app = app();
        app.store.copy_unit(0);
        let before = app.store.tape(0).blocks.len();
        app.text_focus = true;
        for id in ["cut", "copy", "paste", "select-all", "undo", "redo"] {
            assert!(run(&mut app, id, 0), "{id} was not routed to the field");
        }
        assert_eq!(app.store.tape(0).blocks.len(), before, "the tape was edited instead of the field");
        // Commands that are not text editing still act on the tape.
        app.text_focus = true;
        assert!(run(&mut app, "duplicate", 0));
        assert_eq!(app.store.tape(0).blocks.len(), before + 1);
    }

    /// The shortcut table the Help dialog shows, and the accelerators the
    /// non-macOS window handles, both come out of the one table.
    #[test]
    fn accelerators_parse_for_every_item_that_declares_one() {
        for item in menutable::flat() {
            if item.keys.is_empty() {
                continue;
            }
            assert!(
                accelerator(item).is_some(),
                "accelerator {:?} of {:?} did not parse",
                item.keys,
                item.id
            );
        }
        assert!(shortcuts().len() > 8);
    }
}
