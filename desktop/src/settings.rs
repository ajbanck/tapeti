//! The options the web build keeps in `localStorage`, in a plain `key=value`
//! file next to the platform's other application data.
//!
//! `src/state/store.ts` persists `tapeti.theme`, `tapeti.hexBytes`,
//! `tapeti.zeroBased` and `tapeti.emulator`, and `TapePane.tsx` the two
//! splitter positions. The keys here are the same names, so the two builds are
//! at least readable against each other.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
    System,
}

impl Theme {
    pub fn name(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
        }
    }
    fn from_name(s: &str) -> Theme {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
    /// The three-way cycle the title bar's theme button walks.
    pub fn next(self) -> Theme {
        match self {
            Theme::System => Theme::Light,
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::System,
        }
    }
}

/// Everything that survives a restart.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where [`Settings::save`] writes. `None` in a default (test) settings
    /// value, so nothing a test toggles can reach the real config file.
    path: Option<PathBuf>,
    pub theme: Theme,
    pub hex_bytes: bool,
    pub zero_based: bool,
    pub emulator_program: String,
    pub emulator_args: String,
    pub editor_height: f32,
    pub pane_split: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            path: None,
            theme: Theme::System,
            hex_bytes: false,
            // Blocks are numbered from 0 unless the option is turned off: the
            // block number is an index into the tape, and that is where the
            // file format, the jump targets and every other tool start.
            zero_based: true,
            emulator_program: String::new(),
            emulator_args: String::new(),
            editor_height: 340.0,
            pane_split: 0.5,
        }
    }
}

/// Where the app keeps what it remembers between runs: the settings, and the
/// crash log `crashlog.rs` writes beside them.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let dir = home.map(|h| h.join("Library/Application Support/Tapeti"));
    #[cfg(target_os = "windows")]
    let dir = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Tapeti"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".config")))
        .map(|c| c.join("tapeti"));
    dir
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("native.conf"))
}

fn read_map() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(path) = config_path() else { return map };
    let Ok(text) = std::fs::read_to_string(path) else { return map };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

impl Settings {
    pub fn load() -> Settings {
        let m = read_map();
        let mut s = Settings { path: config_path(), ..Settings::default() };
        let get = |k: &str| m.get(k).map(String::as_str);
        if let Some(v) = get("tapeti.theme") {
            s.theme = Theme::from_name(v);
        }
        s.hex_bytes = get("tapeti.hexBytes") == Some("1");
        // On by default, so an absent key is not "off".
        s.zero_based = get("tapeti.zeroBased") != Some("0");
        if let Some(v) = get("tapeti.emulator.program") {
            s.emulator_program = v.to_string();
        }
        if let Some(v) = get("tapeti.emulator.args") {
            s.emulator_args = v.to_string();
        }
        if let Some(v) = get("tapeti.editorHeight").and_then(|v| v.parse::<f32>().ok()) {
            if v >= 140.0 {
                s.editor_height = v;
            }
        }
        if let Some(v) = get("tapeti.paneSplit").and_then(|v| v.parse::<f32>().ok()) {
            if v > 0.0 && v < 1.0 {
                s.pane_split = v;
            }
        }
        s
    }

    /// Best effort: an unwritable config directory is not worth an error dialog,
    /// exactly as the `try { localStorage… } catch {}` on the web side decides.
    pub fn save(&self) {
        let Some(path) = self.path.clone() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!(
            "tapeti.theme={}\n\
             tapeti.hexBytes={}\n\
             tapeti.zeroBased={}\n\
             tapeti.emulator.program={}\n\
             tapeti.emulator.args={}\n\
             tapeti.editorHeight={:.0}\n\
             tapeti.paneSplit={:.3}\n",
            self.theme.name(),
            u8::from(self.hex_bytes),
            u8::from(self.zero_based),
            self.emulator_program,
            self.emulator_args,
            self.editor_height,
            self.pane_split,
        );
        let _ = std::fs::write(path, text);
    }
}
