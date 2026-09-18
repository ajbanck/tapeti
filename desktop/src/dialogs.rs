//! The dialogs, the port of `src/ui/Dialogs.tsx`.
//!
//! Each dialog owns its own widget state here, where the web version keeps it in
//! `useState` hooks; the store holds one `Option<Dialog>` exactly as it holds one
//! `dialog` signal. A dialog is taken out of the store while it draws and put
//! back unless it closed, which is how a dialog gets to run a command without
//! borrowing the store twice.

use egui::{Align, Layout, RichText, Ui};

use tapeti_core::audio::{playback_order, FlowOptions, RenderOptions, TSTATES_PER_SEC};
use tapeti_core::consistency::{check_consistency, Severity};
use tapeti_core::describe::describe_block;
use tapeti_core::programs::detect_programs;
use tapeti_core::types::{block_name, create_body, Block, CREATABLE_IDS};
use tapeti_core::writer::{save_version, serialize_tzx};

use crate::actions::{self, Then};
use crate::app::App;
use crate::files;
use crate::fmt;
use crate::settings::Settings;
use crate::state::{SelectMode, Side};
use crate::theme::Tokens;
use crate::widgets as w;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Where {
    Before,
    After,
    End,
}

pub struct InsertState {
    pub side: Side,
    pub id: u8,
    pub where_: Where,
}

pub struct WavState {
    pub side: Side,
    pub rate: u32,
    pub bits: u16,
    /// false = the whole tape following loops and jumps.
    pub selection: bool,
}

pub struct ProgramsState {
    pub side: Side,
    pub filter: String,
    pub hi: usize,
    pub focus: bool,
}

pub struct EmulatorState {
    pub not_found: bool,
    pub then: Option<Then>,
    pub auto: bool,
    pub program: String,
    pub args: String,
    /// `None` = looked and found nothing.
    pub detected: Option<String>,
}

pub enum Dialog {
    Message { title: String, lines: Vec<String> },
    Confirm { title: String, lines: Vec<String>, then: Then },
    About,
    Insert(InsertState),
    TapeInfo(Side),
    Consistency(Side),
    Wav(WavState),
    Programs(ProgramsState),
    Emulator(EmulatorState),
}

impl Dialog {
    pub fn message(title: &str, lines: Vec<String>) -> Dialog {
        Dialog::Message { title: title.to_string(), lines }
    }
    pub fn confirm(title: &str, lines: Vec<String>, then: Then) -> Dialog {
        Dialog::Confirm { title: title.to_string(), lines, then }
    }
    pub fn about() -> Dialog {
        Dialog::About
    }
    pub fn insert(side: Side) -> Dialog {
        Dialog::Insert(InsertState { side, id: 0x10, where_: Where::After })
    }
    pub fn tape_info(side: Side) -> Dialog {
        Dialog::TapeInfo(side)
    }
    pub fn consistency(side: Side) -> Dialog {
        Dialog::Consistency(side)
    }
    pub fn wav(side: Side) -> Dialog {
        Dialog::Wav(WavState { side, rate: 44_100, bits: 16, selection: false })
    }
    pub fn programs(side: Side) -> Dialog {
        Dialog::Programs(ProgramsState { side, filter: String::new(), hi: 0, focus: true })
    }
    pub fn emulator(not_found: bool, then: Option<Then>, settings: &Settings) -> Dialog {
        Dialog::Emulator(EmulatorState {
            not_found,
            then,
            auto: settings.emulator_program.is_empty(),
            program: settings.emulator_program.clone(),
            args: settings.emulator_args.clone(),
            detected: crate::emulator::detect_emulator(),
        })
    }
}

/// What the frame should do with the dialog afterwards.
enum Outcome {
    Keep,
    Close,
}

/// The ✕ in the title row, under an id of its own so a test can click it.
pub fn close_button_id() -> egui::Id {
    egui::Id::new("tapeti-dialog-close")
}

/// Draw the open dialog, if any.
pub fn draw(app: &mut App, ctx: &egui::Context) {
    let Some(mut dialog) = app.store.dialog.take() else { return };
    let tok = app.tokens;
    let (title, width) = match &dialog {
        Dialog::Message { title, .. } | Dialog::Confirm { title, .. } => (title.clone(), 460.0),
        Dialog::About => ("About Tapeti".into(), 460.0),
        Dialog::Insert(_) => ("Insert block".into(), 420.0),
        Dialog::TapeInfo(side) => (format!("{} tape info", if *side == 0 { "Left" } else { "Right" }), 460.0),
        Dialog::Consistency(side) => {
            (format!("Consistency check — {} tape", if *side == 0 { "left" } else { "right" }), 560.0)
        }
        Dialog::Wav(_) => ("Export WAV".into(), 440.0),
        Dialog::Programs(_) => ("Programs".into(), 460.0),
        Dialog::Emulator(_) => ("Emulator".into(), 560.0),
    };

    let response = egui::Modal::new(egui::Id::new("tapeti-dialog"))
        .frame(
            egui::Frame::window(&ctx.global_style()).fill(tok.surface).inner_margin(egui::Margin::same(14)),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            let closed = ui
                .horizontal(|ui| {
                    ui.label(RichText::new(&title).size(14.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let x = &crate::icons::X;
                        crate::icons::button_with_id(ui, close_button_id(), x, "Close", true).clicked()
                    })
                    .inner
                })
                .inner;
            ui.add_space(6.0);
            let body = match &mut dialog {
                Dialog::Message { lines, .. } => lines_with_ok(ui, lines),
                Dialog::Confirm { lines, then, .. } => confirm_body(ui, app, lines, then.clone()),
                Dialog::About => about_body(ui, &tok),
                Dialog::Insert(s) => insert_body(ui, app, s),
                Dialog::TapeInfo(side) => tape_info_body(ui, app, *side, &tok),
                Dialog::Consistency(side) => consistency_body(ui, app, *side, &tok),
                Dialog::Wav(s) => wav_body(ui, app, s, &tok),
                Dialog::Programs(s) => programs_body(ui, app, s, &tok),
                Dialog::Emulator(s) => emulator_body(ui, app, s, &tok),
            };
            // The ✕ wins over the body, which reports `Keep` on every frame in
            // which nothing was clicked in it. Assigning both to one variable is
            // what made the ✕ do nothing at all.
            if closed {
                Outcome::Close
            } else {
                body
            }
        });

    let dismissed = response.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let outcome = if dismissed { Outcome::Close } else { response.inner };
    if matches!(outcome, Outcome::Keep) {
        app.store.dialog = Some(dialog);
    }
}

fn footer<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
}

fn lines_with_ok(ui: &mut Ui, lines: &[String]) -> Outcome {
    for l in lines {
        ui.label(l);
    }
    footer(ui, |ui| if ui.button("OK").clicked() { Outcome::Close } else { Outcome::Keep })
}

fn confirm_body(ui: &mut Ui, app: &mut App, lines: &[String], then: Then) -> Outcome {
    for l in lines {
        ui.label(l);
    }
    footer(ui, |ui| {
        let cancel = ui.button("Cancel").clicked();
        let ok = ui.button("OK").clicked();
        if ok {
            app.pending = Some(then);
            return Outcome::Close;
        }
        if cancel {
            return Outcome::Close;
        }
        Outcome::Keep
    })
}

fn about_body(ui: &mut Ui, tok: &Tokens) -> Outcome {
    ui.label(RichText::new("Tapeti").strong());
    ui.label("An editor for ZX Spectrum TZX and TAP tape images, for the desktop and the browser.");
    ui.add_space(4.0);
    ui.label("Everything runs locally; files never leave your machine.");
    ui.add_space(4.0);
    w::note(
        ui,
        tok,
        "Supports TZX 1.20 blocks 10–19, 20–28, 2A, 2B, 30–33, 35 and 5A; unknown and deprecated blocks are preserved untouched.",
    );
    ui.add_space(4.0);
    // "Native shell" until stage 6: the wording came from the Tauri build, where a
    // shell wrapped the web app and had a version of its own to distinguish.
    w::note(ui, tok, format!("Version {}", env!("CARGO_PKG_VERSION")));
    // Only once there is one to name: a path here is an answer to "it quit and
    // I do not know why", not a line of small print for everyone else.
    if let Some(path) = crate::crashlog::path().filter(|p| p.exists()) {
        ui.add_space(4.0);
        let text = RichText::new(format!("Crash log: {}", path.display())).size(11.0).color(tok.muted);
        ui.add(egui::Label::new(text).selectable(true));
    }
    footer(ui, |ui| if ui.button("OK").clicked() { Outcome::Close } else { Outcome::Keep })
}

fn insert_body(ui: &mut Ui, app: &mut App, s: &mut InsertState) -> Outcome {
    let mut go = false;
    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
        for id in CREATABLE_IDS {
            let label = format!("{id:02X}  {}", block_name(id).unwrap_or("Unknown"));
            let r = ui.selectable_label(s.id == id, RichText::new(label).monospace());
            if r.clicked() {
                s.id = id;
            }
            if r.double_clicked() {
                s.id = id;
                go = true;
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.radio_value(&mut s.where_, Where::Before, "Before cursor");
        ui.radio_value(&mut s.where_, Where::After, "After cursor");
        ui.radio_value(&mut s.where_, Where::End, "At end");
    });
    let outcome = footer(ui, |ui| {
        let cancel = ui.button("Cancel").clicked();
        go |= ui.button("Insert").clicked();
        if cancel {
            return Outcome::Close;
        }
        Outcome::Keep
    });
    if go {
        let t = app.store.tape(s.side);
        let at = match s.where_ {
            _ if t.cursor < 0 => t.blocks.len(),
            Where::End => t.blocks.len(),
            Where::After => t.cursor as usize + 1,
            Where::Before => t.cursor as usize,
        };
        app.store.insert_blocks(s.side, at, vec![Block::new(create_body(s.id))]);
        return Outcome::Close;
    }
    outcome
}

fn tape_info_body(ui: &mut Ui, app: &mut App, side: Side, tok: &Tokens) -> Outcome {
    let hex = app.store.hex;
    let t = app.store.tape(side);
    let errors =
        check_consistency(&t.blocks, 1).into_iter().filter(|i| i.severity == Severity::Error).count();
    let dur = (errors == 0).then(|| crate::tape::tape_duration(&t.blocks));
    let v = save_version(&t.blocks, t.loaded_version);
    let size = serialize_tzx(&t.blocks, Some(v)).len();
    let data_bytes: usize = t.blocks.iter().filter_map(|b| b.body.data()).map(<[u8]>::len).sum();
    let mut counts: Vec<(u8, usize)> = Vec::new();
    for b in &t.blocks {
        match counts.iter_mut().find(|(id, _)| *id == b.id()) {
            Some((_, n)) => *n += 1,
            None => counts.push((b.id(), 1)),
        }
    }
    counts.sort_unstable();

    let row = |ui: &mut Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.add_sized(
                [190.0, 18.0],
                egui::Label::new(RichText::new(k).color(tok.muted)).halign(Align::LEFT),
            );
            ui.label(v);
        });
    };
    row(ui, "File", t.name.clone());
    row(ui, "Blocks", fmt::num(t.blocks.len() as i64, hex));
    let loaded = t
        .loaded_version
        .filter(|l| l.major != v.major || l.minor != v.minor)
        .map(|l| format!(" (loaded as {}.{:02})", l.major, l.minor))
        .unwrap_or_default();
    row(ui, "TZX version when saved", format!("{}.{:02}{loaded}", v.major, v.minor));
    row(ui, "File size", format!("{} bytes", fmt::num(size as i64, hex)));
    row(ui, "Data payload", format!("{} bytes", fmt::num(data_bytes as i64, hex)));
    row(
        ui,
        "Estimated length",
        match &dur {
            Some((secs, order)) => format!("{} ({} blocks played)", fmt::time(*secs), order.len()),
            None => "n/a — fix consistency errors first".into(),
        },
    );
    ui.add_space(8.0);
    ui.label(RichText::new("Blocks by type").strong());
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for (id, n) in &counts {
            row(ui, &format!("{id:02X} {}", block_name(*id).unwrap_or("Unknown")), fmt::num(*n as i64, hex));
        }
    });
    if errors > 0 {
        ui.colored_label(tok.danger, format!("{errors} consistency error(s) — see Check consistency."));
    }
    w::note(ui, tok, "Durations are computed from block timings and pauses at 3.5 MHz.");
    footer(ui, |ui| if ui.button("OK").clicked() { Outcome::Close } else { Outcome::Keep })
}

fn consistency_body(ui: &mut Ui, app: &mut App, side: Side, tok: &Tokens) -> Outcome {
    let hex = app.store.hex;
    let zero = app.store.settings.zero_based;
    let base = fmt::block_no(0, zero) as i32;
    let issues = check_consistency(&app.store.tape(side).blocks, base);
    let mut goto = None;
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        if issues.is_empty() {
            ui.label("No problems found.");
        }
        for is in &issues {
            let colour = match is.severity {
                Severity::Error => tok.danger,
                Severity::Warning => tok.warn,
                Severity::Info => tok.muted,
            };
            let prefix = if is.block >= 0 {
                let b = &app.store.tape(side).blocks[is.block as usize];
                format!("#{} {}: ", fmt::block_no(is.block as usize, zero), describe_block(b, hex))
            } else {
                String::new()
            };
            let r = ui.add(
                egui::Label::new(RichText::new(format!("{prefix}{}", is.message)).color(colour))
                    .sense(egui::Sense::click()),
            );
            if r.clicked() && is.block >= 0 {
                goto = Some(is.block);
            }
        }
    });
    if let Some(block) = goto {
        app.store.set_cursor(side, block, SelectMode::Single);
        app.scroll_to_cursor(side);
    }
    footer(ui, |ui| if ui.button("OK").clicked() { Outcome::Close } else { Outcome::Keep })
}

fn wav_body(ui: &mut Ui, app: &mut App, s: &mut WavState, tok: &Tokens) -> Outcome {
    let hex = app.store.hex;
    let t = app.store.tape(s.side);
    let sel = t.unit_indices(t.cursor);
    let order: Vec<u32> = if s.selection {
        sel.iter().map(|i| *i as u32).collect()
    } else {
        playback_order(&t.blocks, FlowOptions::default())
    };
    let secs = order.iter().map(|i| crate::tape::block_duration(&t.blocks[*i as usize])).sum::<u64>() as f64
        / f64::from(TSTATES_PER_SEC)
        + 1.0;

    let rates = [48000u32, 44100, 22050, 11025, 8000];
    ui.horizontal(|ui| {
        ui.label("Sample rate");
        let opts: Vec<(u32, String)> = rates.iter().map(|r| (*r, format!("{r} Hz"))).collect();
        w::combo(ui, "wav-rate", &mut s.rate, &opts, 110.0, true);
        ui.label("Resolution");
        let bits = [(16u16, "16 bit".to_string()), (8, "8 bit".to_string())];
        w::combo(ui, "wav-bits", &mut s.bits, &bits, 90.0, true);
    });
    ui.horizontal(|ui| {
        ui.label("Waveform");
        let modes = [(true, "MIC emulation".to_string()), (false, "Square wave".to_string())];
        let mut mic = app.store.audio_mic;
        if w::combo(ui, "wav-mode", &mut mic, &modes, 150.0, true) {
            app.store.audio_mic = mic;
        }
    });
    ui.radio_value(&mut s.selection, false, "Whole tape (following loops/jumps)");
    ui.add_enabled_ui(!sel.is_empty(), |ui| {
        ui.radio_value(&mut s.selection, true, format!("Selection ({} block(s))", sel.len()));
    });
    w::note(
        ui,
        tok,
        format!(
            "About {} of audio, {} KB.",
            fmt::time(secs),
            fmt::num((secs * f64::from(s.rate) * f64::from(s.bits) / 8.0 / 1024.0).round() as i64, hex)
        ),
    );

    let mut go = false;
    let outcome = footer(ui, |ui| {
        let cancel = ui.button("Cancel").clicked();
        go = ui.add_enabled(!order.is_empty(), egui::Button::new("Export")).clicked();
        if cancel {
            return Outcome::Close;
        }
        Outcome::Keep
    });
    if go {
        export_wav(app, s, &order);
        return Outcome::Close;
    }
    outcome
}

fn export_wav(app: &mut App, s: &WavState, order: &[u32]) {
    let t = app.store.tape(s.side);
    let opts = RenderOptions { sample_rate: s.rate, mic: app.store.audio_mic, amplitude: 1.0 };
    let samples = crate::tape::render_tape(&t.blocks, opts, order);
    let wav = tapeti_core::audio::encode_wav(&samples, s.rate, s.bits);
    let name = files::stem(&t.name) + ".wav";
    if let Some(path) = files::save_bytes(&name, ("WAV audio", &["wav"]), &wav) {
        let saved = files::file_name(&path);
        app.store.set_status(format!("Saved {saved}"));
    }
}

fn programs_body(ui: &mut Ui, app: &mut App, s: &mut ProgramsState, tok: &Tokens) -> Outcome {
    let zero = app.store.settings.zero_based;
    let programs = detect_programs(&app.store.tape(s.side).blocks);
    let q = s.filter.trim().to_lowercase();
    let shown: Vec<_> =
        programs.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q)).collect();

    let edit = ui.add(
        egui::TextEdit::singleline(&mut s.filter).hint_text("Filter by name…").desired_width(f32::INFINITY),
    );
    if s.focus {
        edit.request_focus();
        s.focus = false;
    }
    if edit.changed() {
        s.hi = 0;
    }
    let mut pick = None;
    ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) {
            s.hi = (s.hi + 1).min(shown.len().saturating_sub(1));
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            s.hi = s.hi.saturating_sub(1);
        }
        if i.key_pressed(egui::Key::Enter) {
            pick = shown.get(s.hi).map(|p| (*p).clone());
        }
    });
    let cur = s.hi.min(shown.len().saturating_sub(1));
    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
        if shown.is_empty() {
            ui.label("No program matches");
        }
        for (i, p) in shown.iter().enumerate() {
            let text = format!(
                "{}    #{}–#{} · {} block{}",
                p.name,
                fmt::block_no(p.start as usize, zero),
                fmt::block_no(p.end as usize, zero),
                p.end - p.start + 1,
                if p.end == p.start { "" } else { "s" }
            );
            let r = ui.selectable_label(i == cur, text);
            if r.clicked() {
                s.hi = i;
            }
            if r.double_clicked() {
                pick = Some((*p).clone());
            }
        }
    });
    w::note(
        ui,
        tok,
        "Programs start at BASIC Program headers, at groups that contain one, and at Select block entries. Group blocks manually to fix a wrong split.",
    );
    let side = s.side;
    let outcome = footer(ui, |ui| {
        let cancel = ui.button("Cancel").clicked();
        if ui.add_enabled(!shown.is_empty(), egui::Button::new("Select")).clicked() {
            pick = shown.get(cur).map(|p| (*p).clone());
        }
        if cancel {
            return Outcome::Close;
        }
        Outcome::Keep
    });
    if let Some(p) = pick {
        actions::jump_to_program(app, side, &p);
        return Outcome::Close;
    }
    outcome
}

fn emulator_body(ui: &mut Ui, app: &mut App, s: &mut EmulatorState, tok: &Tokens) -> Outcome {
    if s.not_found {
        ui.label(RichText::new("Fuse was not found.").strong());
        ui.label("Choose the emulator program to use.");
        ui.add_space(4.0);
    }
    ui.radio_value(&mut s.auto, true, "Auto-detect Fuse");
    w::note(ui, tok, s.detected.clone().unwrap_or_else(|| "Not found".into()));
    ui.radio_value(&mut s.auto, false, "Program");
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::TextEdit::singleline(&mut s.program)
                    .hint_text("Path or program name")
                    .desired_width(360.0),
            )
            .changed()
        {
            s.auto = false;
        }
        if ui.button("Browse…").clicked() {
            if let Some(p) = rfd::FileDialog::new().pick_file() {
                s.program = p.to_string_lossy().into_owned();
                s.auto = false;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Extra arguments");
        ui.add(
            egui::TextEdit::singleline(&mut s.args).hint_text("e.g. --machine plus2a").desired_width(300.0),
        );
    });
    w::note(
        ui,
        tok,
        "The tape file is passed as the last argument. On macOS an app (.app) is opened with `open -a`; extra arguments only reach it when it is not already running.",
    );
    let can_save = if s.auto { !s.not_found || s.detected.is_some() } else { !s.program.trim().is_empty() };
    let mut save = false;
    let outcome = footer(ui, |ui| {
        let cancel = ui.button("Cancel").clicked();
        let label = if s.then.is_some() { "Save and open" } else { "Save" };
        save = ui.add_enabled(can_save, egui::Button::new(label)).clicked();
        if cancel {
            return Outcome::Close;
        }
        Outcome::Keep
    });
    if save {
        app.store.settings.emulator_program =
            if s.auto { String::new() } else { s.program.trim().to_string() };
        app.store.settings.emulator_args = s.args.trim().to_string();
        app.store.settings.save();
        app.pending = s.then.clone();
        return Outcome::Close;
    }
    outcome
}
