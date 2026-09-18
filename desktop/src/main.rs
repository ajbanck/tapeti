//! Tapeti, the desktop app.
//!
//! A window, the platform's menu built from one command table, two tape panes
//! with their editors, the data window, the dialogs and playback — all on
//! `tapeti-core` linked directly: no wasm, no wire format, no web view. Stage 3
//! of the Rust migration chose the toolkit and measured the boundary, stage 4
//! filled the app in against a parity checklist, and stage 5 made it the
//! desktop app: the Tauri shell is gone, `src/` is the browser build.
//!
//! The flags are in `HELP` below, which is what `--help` prints — one list, so it
//! cannot drift from the arguments `parse_args` accepts the way it did before
//! `--screenshot` existed. `--screenshot` and `--measure` are the two that open no
//! window, which is what makes them the way to check a change from a terminal.

#![windows_subsystem = "windows"]

mod actions;
mod app;
mod commands;
mod crashlog;
mod datawin;
mod dialogs;
mod editor;
mod emulator;
mod files;
mod fmt;
mod icons;
mod list;
#[cfg(target_os = "macos")]
mod macos;
mod menu;
mod menutable;
mod player;
mod settings;
mod shot;
mod state;
mod statusbar;
mod tables;
mod tape;
mod theme;
mod widgets;

use std::path::PathBuf;
use std::time::Instant;

use tapeti_core::types::Block;
use tapeti_core::{consistency, content, describe, programs, writer};

use settings::Settings;
use state::Store;

/// Where a tape comes from when none is named on the command line.
const DEFAULT_TAPE: &str = "public/samples/Tapeti demo.tzx";

/// The window icon, compiled in: the bundles' own icon, at the size a title bar
/// and a task bar want.
const ICON_PNG: &[u8] = include_bytes!("../../assets/icons/128x128.png");

/// `--help`. The flags below the first group open no window, which is what makes
/// them the way to check a change from a terminal; `--screenshot` is the one the
/// tests use too, through `shot::capture`.
const HELP: &str = "\
tapeti [TAPE…] [OPTIONS]

  A tape named here opens in the left pane, a second one in the right.

  --hex                  start with numbers in hexadecimal
  --screenshot F[,WxH]   draw the app into a PNG and quit — no window, no GPU
  --theme NAME           light, dark or system (default: system)
  --cursor N             put the cursor on row N first
  --measure              time the core in process and the app's own first frame
  --rows N               repeat the tape's blocks until the list is N rows long
  --bench N              move the cursor once per frame N times, report the spread
  --exit-on-draw         quit on the first frame, so `time tapeti …` is the cold start
  -h, --help             this

  --measure and --screenshot write to the terminal or to a file and quit;
  every other way of starting opens a window.";

struct Opts {
    paths: Vec<PathBuf>,
    rows: Option<usize>,
    bench: Option<usize>,
    exit_on_draw: bool,
    measure: bool,
    hex: bool,
    /// `--screenshot FILE[,WxH]`: draw the app into a PNG and quit, no window.
    screenshot: Option<String>,
    /// Which row the screenshot has the cursor on.
    cursor: Option<i32>,
    /// `light`, `dark` or `system`: which theme to draw, for comparing the two.
    theme: Option<String>,
}

fn parse_args() -> Opts {
    let mut o = Opts {
        paths: Vec::new(),
        rows: None,
        bench: None,
        exit_on_draw: false,
        measure: false,
        hex: false,
        screenshot: None,
        cursor: None,
        theme: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rows" => o.rows = args.next().and_then(|v| v.parse().ok()),
            "--bench" => o.bench = args.next().and_then(|v| v.parse().ok()),
            "--exit-on-draw" => o.exit_on_draw = true,
            "--measure" => o.measure = true,
            "--hex" => o.hex = true,
            "--screenshot" => o.screenshot = args.next(),
            "--cursor" => o.cursor = args.next().and_then(|v| v.parse().ok()),
            "--theme" => o.theme = args.next(),
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            // macOS hands a bundled app a process serial number argument.
            _ if a.starts_with('-') => {}
            _ => o.paths.push(PathBuf::from(a)),
        }
    }
    o
}

/// Tapes to open at startup. Naming none opens none: an app started from the Dock
/// or with `open -a` shows an empty tape, the way every other editor does. The
/// measuring flags are the exception — they have to measure *something*, so they
/// fall back to the demo tape.
fn startup_paths(o: &Opts) -> Vec<PathBuf> {
    if !o.paths.is_empty() {
        return o.paths.clone();
    }
    let measuring = o.measure || o.rows.is_some() || o.bench.is_some();
    if measuring {
        default_tape().into_iter().collect()
    } else {
        Vec::new()
    }
}

/// The sample tape, looked for up the tree from the working directory and next to
/// the crate, so both `cargo run` and a bundle built here find it.
fn default_tape() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            roots.push(d.to_path_buf());
            dir = d.parent();
        }
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    roots.into_iter().map(|r| r.join(DEFAULT_TAPE)).find(|p| p.is_file())
}

/// Repeat the tape's blocks until the list is `rows` long, the way the stage 2
/// benchmarks built their 3,000-block tape.
fn grow(blocks: &[Block], rows: usize) -> Vec<Block> {
    if blocks.is_empty() {
        return Vec::new();
    }
    (0..rows).map(|i| blocks[i % blocks.len()].clone_fresh()).collect()
}

/// The stage 2 boundary table, one call per line, with the wire encoding gone:
/// this is the same work the web app pays for through wasm.
fn measure(blocks: &[Block], parse_ms: f64, hex: bool) {
    fn ms(f: impl FnOnce()) -> f64 {
        let t = Instant::now();
        f();
        t.elapsed().as_secs_f64() * 1000.0
    }
    println!("{} blocks, in process, no wire format:", blocks.len());
    println!("  parse                          {parse_ms:7.2} ms");
    let mut sink = 0usize;
    let describe = ms(|| {
        for b in blocks {
            sink += describe::describe_block(b, hex).len() + describe::block_length(b) as usize;
        }
    });
    println!("  describeBlock + blockLength    {describe:7.2} ms");
    println!(
        "  content labels                 {:7.2} ms",
        ms(|| sink += content::content_labels(blocks).len())
    );
    println!(
        "  checkConsistency               {:7.2} ms",
        ms(|| sink += consistency::check_consistency(blocks, 1).len())
    );
    println!(
        "  detectPrograms                 {:7.2} ms",
        ms(|| sink += programs::detect_programs(blocks).len())
    );
    println!(
        "  groupRanges                    {:7.2} ms",
        ms(|| sink += programs::group_ranges(blocks).len())
    );
    println!(
        "  requiredVersion                {:7.2} ms",
        ms(|| sink += writer::required_version(blocks).major as usize)
    );
    println!(
        "  serializeTzx                   {:7.2} ms",
        ms(|| sink += writer::serialize_tzx(blocks, None).len())
    );
    debug_assert!(sink > 0);
}

/// What the app's own first frame costs before a window is involved: laying out
/// and tessellating every panel, and rasterising the glyphs it uses. This is the
/// half of cold start that can be measured from a terminal — the other half is
/// process start, window creation and the GL context, and it needs a screen.
///
/// The two font sets are timed against each other because trimming them was the
/// obvious suspect for a slow start. It is not: the gap is about a millisecond.
fn measure_first_frame(store: Store) {
    let mut store = Some(store);
    println!("\nfirst headless frame (no window, no GL context, warm page cache):");
    for (label, fonts) in
        [("egui's default fonts", None), ("without the emoji fonts", Some(theme::latin_only_fonts()))]
    {
        let ctx = egui::Context::default();
        if let Some(fonts) = fonts {
            ctx.set_fonts(fonts);
        }
        // A fresh store per run: the second frame would find the rows cached.
        let taken = store.take().unwrap();
        let name = taken.tape(0).name.clone();
        let blocks = taken.tape(0).blocks.clone();
        let mut next = Store::new(Settings::default());
        next.tape_mut(0).load(name, None, blocks, None);
        let mut app = app::App::build(&ctx, menu::Menu::headless(), taken, Instant::now(), 0, false);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
            ..Default::default()
        };
        let t = Instant::now();
        ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
        println!("  {label:<24} {:7.2} ms", t.elapsed().as_secs_f64() * 1000.0);
        store = Some(next);
    }
}

/// The app icon, for the task bar and the window: the same PNG the bundles carry.
/// macOS reads it from the bundle instead, but it costs a fraction of a
/// millisecond and it is one code path.
fn window_icon() -> Option<egui::IconData> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(ICON_PNG)).read_info().ok()?;
    let mut rgba = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    rgba.truncate(info.buffer_size());
    Some(egui::IconData { rgba, width: info.width, height: info.height })
}

fn main() -> eframe::Result<()> {
    // Before anything that could panic: a GUI app has nowhere to print one.
    crashlog::install();
    let t0 = Instant::now();
    let opts = parse_args();
    let mut settings = Settings::load();
    if opts.hex {
        settings.hex_bytes = true;
    }
    let mut store = Store::new(settings);
    store.hex = opts.hex;

    // The command line is the "open with" queue the Tauri shell used to drain: a
    // tape named on it goes into the left pane, a second into the right. macOS
    // sends an Apple Event instead of argv, which `macos::install` catches.
    let t_parse = Instant::now();
    files::open_with(&mut store, &startup_paths(&opts));
    let parse_ms = t_parse.elapsed().as_secs_f64() * 1000.0;
    store.active = 0;

    if let Some(n) = opts.rows {
        let blocks = &store.tape(0).blocks;
        let grown = if n > blocks.len() { grow(blocks, n) } else { blocks[..n.min(blocks.len())].to_vec() };
        let name = store.tape(0).name.clone();
        store.tape_mut(0).load(name, None, grown, None);
    }

    // A picture of the app, without a window: what `npm run smoke` gives the
    // browser build. `--screenshot out.png` or `--screenshot out.png,1400x900`.
    if let Some(spec) = opts.screenshot.clone() {
        let (path, size) = match spec.split_once(',') {
            Some((p, wh)) => {
                let (w, h) = wh.split_once('x').unwrap_or(("1100", "720"));
                (p.to_string(), (w.parse().unwrap_or(1100.0), h.parse().unwrap_or(720.0)))
            }
            None => (spec, (1100.0, 720.0)),
        };
        if let Some(name) = opts.theme.as_deref() {
            store.settings.theme = match name {
                "light" => settings::Theme::Light,
                "dark" => settings::Theme::Dark,
                _ => settings::Theme::System,
            };
        }
        if let Some(at) = opts.cursor {
            store.set_cursor(0, at, state::SelectMode::Single);
        }
        let ctx = egui::Context::default();
        // The in-window bar, not the headless menu a test draws with: a
        // screenshot should show the window the way the window looks.
        let mut app = app::App::build(&ctx, menu::Menu::in_window(), store, t0, 0, false);
        let canvas = shot::capture(&ctx, &mut app, size, 3);
        std::fs::write(&path, canvas.to_png()).expect("write the screenshot");
        println!("{path}: {}×{}", canvas.width, canvas.height);
        return Ok(());
    }

    if opts.measure {
        measure(&store.tape(0).blocks, parse_ms, opts.hex);
        measure_first_frame(store);
        return Ok(());
    }

    let blocks = &store.tape(0).blocks;
    if !blocks.is_empty() {
        println!("core: parse {parse_ms:.1} ms, {} blocks", blocks.len());
    }
    if let Some(v) = store.tape(0).loaded_version {
        println!("{}: TZX {}.{:02} as loaded", store.tape(0).name, v.major, v.minor);
    }

    let bench = opts.bench.unwrap_or(0);
    let exit_on_draw = opts.exit_on_draw;
    // Before the event loop: the observer it installs has to be in place by the
    // time AppKit delivers the documents the app was launched with.
    #[cfg(target_os = "macos")]
    macos::install();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Tapeti")
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([680.0, 420.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Tapeti",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, store, t0, bench, exit_on_draw)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Opts {
        Opts {
            paths: Vec::new(),
            rows: None,
            bench: None,
            exit_on_draw: false,
            measure: false,
            hex: false,
            screenshot: None,
            cursor: None,
            theme: None,
        }
    }

    /// Started with nothing to open, the app opens nothing: the demo tape is for
    /// the measuring flags, which have to measure something. Double-clicking the
    /// app used to open whatever tape was next to the binary, which is a
    /// development convenience and not what an editor does.
    #[test]
    fn a_plain_start_opens_no_tape() {
        assert!(startup_paths(&opts()).is_empty());
        assert!(startup_paths(&Opts { exit_on_draw: true, ..opts() }).is_empty());

        assert_eq!(
            startup_paths(&Opts { measure: true, ..opts() }),
            default_tape().into_iter().collect::<Vec<_>>()
        );
        assert_eq!(
            startup_paths(&Opts { bench: Some(200), ..opts() }),
            default_tape().into_iter().collect::<Vec<_>>()
        );

        let named = vec![PathBuf::from("a.tzx"), PathBuf::from("b.tzx")];
        assert_eq!(startup_paths(&Opts { paths: named.clone(), rows: Some(3000), ..opts() }), named);
    }
}
