//! The native menu against `src/state/commands.ts`.
//!
//! The web app and this one address commands by the same ids and grey them out
//! under the same conditions, so the thing worth testing about the table is that
//! neither has drifted. The table is included rather than imported because it
//! belongs to a binary crate.

include!("../src/menutable.rs");

fn commands_ts() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/state/commands.ts");
    std::fs::read_to_string(path).expect("src/state/commands.ts")
}

fn command_ids() -> Vec<String> {
    // The table's keys are the only single-quoted strings at two spaces of indent
    // followed by a colon.
    commands_ts()
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("  '")?;
            let (id, tail) = rest.split_once('\'')?;
            tail.starts_with(':').then(|| id.to_string())
        })
        .collect()
}

/// The `enabled:` expression of each command, as written, or `None` where the
/// entry has none (which means always enabled).
fn enabled_expressions() -> Vec<(String, Option<String>)> {
    let source = commands_ts();
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in source.lines() {
        if let Some(rest) = line.strip_prefix("  '") {
            if let Some((id, tail)) = rest.split_once('\'') {
                if tail.starts_with(':') {
                    if let Some((id, text)) = current.take() {
                        out.push((id, enabled_of(&text)));
                    }
                    current = Some((id.to_string(), String::new()));
                }
            }
        }
        if let Some((_, text)) = current.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some((id, text)) = current.take() {
        out.push((id, enabled_of(&text)));
    }
    out
}

/// The text after `enabled:` up to the next top-level comma, roughly: enough to
/// recognise which predicate it is.
fn enabled_of(entry: &str) -> Option<String> {
    let at = entry.find("enabled:")? + "enabled:".len();
    let rest = &entry[at..];
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    end = i;
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    Some(rest[..end].split_whitespace().collect::<Vec<_>>().join(" "))
}

/// What the web predicate means in this table's vocabulary.
fn need_of(expression: Option<&str>) -> Option<Need> {
    let Some(e) = expression else { return Some(Need::Always) };
    Some(match e {
        "hasBlocks" => Need::Blocks,
        "hasCursor" => Need::Cursor,
        _ if e.contains("undo.length") => Need::Undo,
        _ if e.contains("redo.length") => Need::Redo,
        _ if e.contains("clipboard.value.length") => Need::Clipboard,
        _ if e.contains("playing.value") => Need::Playing,
        _ if e.contains("groupRanges") => Need::Collapsible,
        _ => return None,
    })
}

#[test]
fn every_menu_item_is_a_command() {
    let ids = command_ids();
    assert!(ids.len() > 40, "commands.ts parsed as {} ids", ids.len());
    for item in flat() {
        if item.id.is_empty() {
            continue;
        }
        assert!(ids.iter().any(|c| c == item.id), "menu item {:?} is not a command id", item.id);
    }
}

#[test]
fn every_command_is_in_the_menu() {
    let menu: Vec<&str> = flat().iter().map(|i| i.id).collect();
    for id in command_ids() {
        assert!(menu.contains(&id.as_str()), "command {id:?} has no menu item");
    }
}

/// `MENUS` in `src/ui/MenuBar.tsx`, as (title, ids) with `""` for a separator:
/// the entries between `export const MENUS` and the `];` that closes it, where
/// a line opening with `['Title', [` starts a menu and every other quoted word
/// is a command id or `sep`.
fn web_menus() -> Vec<(String, Vec<String>)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/ui/MenuBar.tsx");
    let source = std::fs::read_to_string(path).expect("src/ui/MenuBar.tsx");
    let start = source.find("export const MENUS").expect("MenuBar.tsx has no MENUS table");
    let table = &source[start..];
    let table = &table[..table.find("\n];").expect("MENUS is not closed")];
    let mut menus: Vec<(String, Vec<String>)> = Vec::new();
    for line in table.lines().skip(1) {
        let mut words = line.split('\'').skip(1).step_by(2).map(str::to_string);
        if line.trim_start().starts_with("['") {
            menus.push((words.next().unwrap(), Vec::new()));
        }
        let Some((_, ids)) = menus.last_mut() else { continue };
        ids.extend(words.map(|w| if w == "sep" { String::new() } else { w }));
    }
    menus
}

/// One grouping, in both builds: the same titles, holding the same commands in
/// the same order, separators included.
#[test]
fn the_menus_are_grouped_as_on_the_web() {
    let web = web_menus();
    assert_eq!(web.len(), MENUS.len(), "MenuBar.tsx has {} menus", web.len());
    for (menu, (title, ids)) in MENUS.iter().zip(&web) {
        assert_eq!(menu.title, title);
        let here: Vec<&str> = menu.items.iter().map(|i| i.id).collect();
        assert_eq!(&here, ids, "the {title} menu differs from MenuBar.tsx");
    }
}

/// No menu is long enough to need reading: the point of the grouping.
#[test]
fn no_menu_is_long() {
    for menu in MENUS {
        assert!(menu.items.len() <= 15, "the {} menu has {} rows", menu.title, menu.items.len());
    }
}

#[test]
fn ids_are_unique_and_flat_order_is_stable() {
    let items = flat();
    assert_eq!(items.len(), item_count());
    let mut ids: Vec<&str> = items.iter().map(|i| i.id).filter(|i| !i.is_empty()).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(before, ids.len(), "duplicate command id in the menu");
}

/// The rule that actually greys an item out, against the web's own predicate.
#[test]
fn enabled_rules_match_the_web_table() {
    let expressions = enabled_expressions();
    assert!(expressions.len() > 40, "commands.ts parsed as {} entries", expressions.len());
    let mut checked = 0;
    for (id, expression) in &expressions {
        let Some(item) = item(id) else { continue }; // the context menu's own items
        let want = need_of(expression.as_deref()).unwrap_or_else(|| {
            panic!("command {id:?} has an enabled rule this test cannot read: {expression:?}")
        });
        assert!(
            item.need == want,
            "command {id:?} is enabled differently here than in commands.ts ({expression:?})"
        );
        checked += 1;
    }
    assert!(checked > 40, "only {checked} commands compared");
}

#[test]
fn enabled_flags_follow_the_state() {
    let empty = enabled_flags(&MenuState::default());
    let loaded = enabled_flags(&MenuState { blocks: 19, has_cursor: true, ..Default::default() });
    let undoable =
        enabled_flags(&MenuState { blocks: 19, has_cursor: true, can_undo: true, ..Default::default() });
    assert_eq!(empty.len(), item_count());
    let index = |id: &str| flat().iter().position(|i| i.id == id).unwrap();
    assert!(!empty[index("save")], "Save is enabled without blocks");
    assert!(loaded[index("save")], "Save is disabled with a tape loaded");
    assert!(!loaded[index("undo")], "Undo is enabled with an empty undo stack");
    assert!(undoable[index("undo")], "Undo is disabled with something to undo");
    assert!(!loaded[index("paste")], "Paste is enabled with an empty clipboard");
    assert!(!loaded[index("stop")], "Stop is enabled while nothing plays");
    assert!(loaded[index("new")] && empty[index("new")], "New is always available");
}
