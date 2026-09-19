//! Number and key formatting, the port of the helpers at the top of
//! `src/state/store.ts` and `fmtKey` in `src/state/commands.ts`.
//!
//! Every one of them depends on a switch — the screen's own Dec/Hex button, the
//! "hex bytes" option, the "number blocks from 0" option — so they take the
//! flags explicitly and the call sites read them off the store or off the
//! screen's own state. The Dec/Hex one belongs to a screen: the main window's
//! is `Store::hex`, the data window keeps its own.

/// `fmtNum`: a number in the base the Dec/Hex switch selects.
pub fn num(n: i64, hex: bool) -> String {
    if hex {
        if n < 0 {
            format!("-{:X}", -n)
        } else {
            format!("{n:X}")
        }
    } else {
        n.to_string()
    }
}

/// `fmtByte`: a flag or checksum byte. Hex under the screen's switch or the
/// "flag and checksum bytes in hex" option, else a zero-padded decimal, the way
/// the web editor shows them. The `0x` is part of it either way: these bytes are
/// read out of a sentence ("Checksum byte 0x17"), where a bare 17 would not say
/// which base it is in.
pub fn byte(n: u8, hex: bool, hex_bytes: bool) -> String {
    if hex || hex_bytes {
        format!("0x{n:02X}")
    } else {
        format!("{n:03}")
    }
}

/// `blockNo`: the number shown for the block at 0-based index `i`.
pub fn block_no(i: usize, zero_based: bool) -> usize {
    if zero_based {
        i
    } else {
        i + 1
    }
}

/// `fmtTime`: seconds as `m:ss.s`.
pub fn time(sec: f64) -> String {
    let m = (sec / 60.0).floor();
    let s = sec - m * 60.0;
    format!("{m}:{s:04.1}")
}

/// The editor footer's playing time: `36.11 s`, or `2:05.30` from a minute up.
pub fn duration(s: f64) -> String {
    if s < 60.0 {
        return format!("{s:.2} s");
    }
    let m = (s / 60.0).floor();
    format!("{m}:{:05.2}", s - m * 60.0)
}

/// `parseNum`: `$`/`0x` force hex, `#` forces decimal, everything else follows
/// the Dec/Hex switch. Returns `None` where the TypeScript returns NaN.
pub fn parse_num(s: &str, hex: bool) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let v = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(rest, 16).ok()?
    } else if let Some(rest) = s.strip_prefix('$') {
        i64::from_str_radix(rest, 16).ok()?
    } else if let Some(rest) = s.strip_prefix('#') {
        rest.parse::<i64>().ok()?
    } else {
        i64::from_str_radix(s, if hex { 16 } else { 10 }).ok()?
    };
    Some(if neg { -v } else { v })
}

pub const IS_MAC: bool = cfg!(target_os = "macos");

/// `fmtKey`: `Mod+Shift+Z` becomes `⇧⌘Z` on macOS (modifiers in the Apple order
/// ⌃⌥⇧⌘) and `Ctrl+Shift+Z` elsewhere. The accelerators in `menutable.rs` are
/// muda's spelling; this is the spelling the in-app menus and the shortcut list
/// show.
pub fn key(spec: &str) -> String {
    let mut parts: Vec<&str> = spec.split('+').collect();
    let Some(k) = parts.pop() else { return String::new() };
    if !IS_MAC {
        let mut out: Vec<&str> = parts.iter().map(|p| if *p == "Mod" { "Ctrl" } else { p }).collect();
        out.push(k);
        return out.join("+");
    }
    const ORDER: [&str; 4] = ["Ctrl", "Alt", "Shift", "Mod"];
    parts.sort_by_key(|p| ORDER.iter().position(|o| o == p).unwrap_or(9));
    let mods: String = parts
        .iter()
        .map(|p| match *p {
            "Ctrl" => "⌃",
            "Alt" => "⌥",
            "Shift" => "⇧",
            "Mod" => "⌘",
            _ => "",
        })
        .collect();
    format!("{mods}{k}")
}

/// muda's accelerator spelling (`CmdOrCtrl+Shift+S`) shown the way `fmtKey`
/// would show it, so the egui menu bar and the platform menu agree.
pub fn accel(spec: &str) -> String {
    if spec.is_empty() {
        return String::new();
    }
    key(&spec.replace("CmdOrCtrl", "Mod").replace("ArrowUp", "↑").replace("ArrowDown", "↓"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_the_switch() {
        assert_eq!(num(255, false), "255");
        assert_eq!(num(255, true), "FF");
        assert_eq!(byte(0xff, false, false), "255");
        assert_eq!(byte(0xff, false, true), "0xFF");
        assert_eq!(byte(0xff, true, false), "0xFF", "the screen's switch prefixes them too");
    }

    #[test]
    fn parses_every_prefix() {
        assert_eq!(parse_num("10", false), Some(10));
        assert_eq!(parse_num("10", true), Some(16));
        assert_eq!(parse_num("0x10", false), Some(16));
        assert_eq!(parse_num("$10", false), Some(16));
        assert_eq!(parse_num("#10", true), Some(10));
        assert_eq!(parse_num("-5", false), Some(-5));
        assert_eq!(parse_num("", false), None);
        assert_eq!(parse_num("zz", false), None);
    }

    #[test]
    fn times_and_durations() {
        assert_eq!(time(65.3), "1:05.3");
        assert_eq!(duration(36.111), "36.11 s");
        assert_eq!(duration(125.3), "2:05.30");
    }
}
