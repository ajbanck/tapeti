//! "Open in emulator": write the tape to a temp file and start an external emulator with it.
//! The program is either chosen by the user or auto-detected (Fuse). It is started directly,
//! never through a shell, so paths with spaces need no quoting.
//!
//! This is the one piece of the Tauri shell that outlived it: stage 5 deleted
//! `src-tauri/`, and the two `#[tauri::command]` attributes its copy carried were
//! the whole difference — the shell reached these through the IPC bridge, this
//! app calls them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
const FUSE_BUNDLE_ID: &str = "net.sourceforge.fuse-for-macosx.Fuse";

/// Executable names of Fuse builds: plain, GTK and SDL front ends (Debian/Ubuntu package names).
const FUSE_NAMES: [&str; 3] = ["fuse", "fuse-gtk", "fuse-sdl"];

/// What will be run: a macOS app bundle (via `open`) or a program.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    App(PathBuf),
    Program(PathBuf),
}

impl Target {
    fn display(&self) -> String {
        match self {
            Target::App(p) | Target::Program(p) => p.to_string_lossy().to_string(),
        }
    }
}

/// Split an argument string on whitespace, honouring "double" and 'single' quotes.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Program and arguments to run for `target` with the tape `file`.
pub fn build_command(target: &Target, args: &[String], file: &Path) -> (String, Vec<String>) {
    let file = file.to_string_lossy().to_string();
    match target {
        // `open -a App file --args …`: extra arguments only reach an app that is not yet running.
        Target::App(app) => {
            let mut v = vec!["-a".to_string(), app.to_string_lossy().to_string(), file];
            if !args.is_empty() {
                v.push("--args".into());
                v.extend(args.iter().cloned());
            }
            ("open".into(), v)
        }
        Target::Program(p) => {
            let mut v = args.to_vec();
            v.push(file);
            (p.to_string_lossy().to_string(), v)
        }
    }
}

/// Safe file name stem: letters, digits, dash and underscore only.
pub fn sanitize(name: &str) -> String {
    let stem = name
        .trim_end_matches(".tzx")
        .trim_end_matches(".TZX")
        .trim_end_matches(".tap")
        .trim_end_matches(".TAP");
    let s: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').chars().take(40).collect::<String>();
    if s.is_empty() {
        "tape".into()
    } else {
        s
    }
}

fn is_executable(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    true
}

/// Directories to search for a program. GUI apps on macOS start with a minimal PATH, so the
/// usual install locations are added explicitly.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    #[cfg(unix)]
    {
        for d in ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/usr/games", "/snap/bin"] {
            dirs.push(PathBuf::from(d));
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/bin"));
            dirs.push(PathBuf::from(&home).join("bin"));
        }
    }
    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            if let Some(pf) = std::env::var_os(var) {
                dirs.push(PathBuf::from(&pf).join("Fuse"));
            }
        }
    }
    dirs
}

fn find_program(names: &[&str]) -> Option<PathBuf> {
    let dirs = search_dirs();
    for name in names {
        for d in &dirs {
            let p = d.join(name);
            if is_executable(&p) {
                return Some(p);
            }
            #[cfg(windows)]
            {
                let exe = d.join(format!("{name}.exe"));
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn find_fuse_app() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications/Fuse.app")];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/Fuse.app"));
    }
    if let Some(p) = candidates.into_iter().find(|p| p.is_dir()) {
        return Some(p);
    }
    // Installed elsewhere: ask Spotlight by bundle id.
    let out = Command::new("mdfind")
        .arg(format!("kMDItemCFBundleIdentifier == '{FUSE_BUNDLE_ID}'"))
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from).find(|p| p.is_dir())
}

/// Fuse, if installed: the macOS app first, then a Fuse program.
pub fn detect() -> Option<Target> {
    #[cfg(target_os = "macos")]
    if let Some(app) = find_fuse_app() {
        return Some(Target::App(app));
    }
    find_program(&FUSE_NAMES).map(Target::Program)
}

/// The user's choice: an app bundle on macOS, an absolute path, or a program name on the PATH.
fn resolve(program: &str) -> Result<Target, String> {
    let p = PathBuf::from(program);
    #[cfg(target_os = "macos")]
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
        return if p.is_dir() { Ok(Target::App(p)) } else { Err(format!("{program} not found")) };
    }
    if p.components().count() > 1 {
        return if p.is_file() { Ok(Target::Program(p)) } else { Err(format!("{program} not found")) };
    }
    find_program(&[program]).map(Target::Program).ok_or_else(|| format!("{program} not found on the PATH"))
}

/// Temp folder for exported tapes; files older than a day are removed on each use.
fn temp_folder() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("tapeti-emulator");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let old = SystemTime::now() - Duration::from_secs(24 * 3600);
        for e in entries.flatten() {
            if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < old) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(dir)
}

/// Emulator that "auto-detect" would use, for display in the settings dialog.
pub fn detect_emulator() -> Option<String> {
    detect().map(|t| t.display())
}

/// Write `bytes` as a TZX file and open it in the emulator. Returns what was started.
/// An empty `program` means auto-detect.
pub fn open_in_emulator(
    bytes: Vec<u8>,
    name: String,
    program: String,
    args: String,
) -> Result<String, String> {
    let target = if program.trim().is_empty() {
        detect().ok_or_else(|| "NOT_FOUND".to_string())?
    } else {
        resolve(program.trim())?
    };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let file = temp_folder()?.join(format!("{}-{stamp}.tzx", sanitize(&name)));
    std::fs::write(&file, &bytes).map_err(|e| format!("Cannot write {}: {e}", file.display()))?;

    let (cmd, argv) = build_command(&target, &split_args(&args), &file);
    let mut child = Command::new(&cmd)
        .args(&argv)
        .spawn()
        .map_err(|e| format!("Cannot start {}: {e}", target.display()))?;
    if matches!(target, Target::App(_)) {
        // `open` returns at once; its exit status tells whether the app could be launched.
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("Cannot open {}", target.display()));
        }
    } else {
        // Reap the emulator when it exits so it does not linger as a zombie process.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    Ok(target.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_arguments_with_quotes() {
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("  --machine  plus2a "), vec!["--machine", "plus2a"]);
        assert_eq!(
            split_args(r#"--rom "/my roms/48.rom" -x ''"#),
            vec!["--rom", "/my roms/48.rom", "-x", ""]
        );
    }

    #[test]
    fn builds_commands() {
        let file = Path::new("/tmp/t.tzx");
        let prog = Target::Program(PathBuf::from("/usr/bin/fuse"));
        assert_eq!(
            build_command(&prog, &["--machine".into(), "128".into()], file),
            ("/usr/bin/fuse".into(), vec!["--machine".into(), "128".into(), "/tmp/t.tzx".into()])
        );
        let app = Target::App(PathBuf::from("/Applications/Fuse.app"));
        assert_eq!(
            build_command(&app, &[], file),
            ("open".into(), vec!["-a".into(), "/Applications/Fuse.app".into(), "/tmp/t.tzx".into()])
        );
        assert_eq!(build_command(&app, &["-x".into()], file).1.last().unwrap(), "-x");
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize("Dan Dare 2 - Mekon's.tzx"), "Dan_Dare_2_-_Mekon_s");
        assert_eq!(sanitize("..."), "tape");
    }
}
