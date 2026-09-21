//! The window: two tape panes, the editor under each, the status bar, and the
//! keyboard. The port of `src/ui/App.tsx` and the frame of `TapePane.tsx`.
//!
//! egui is immediate mode, so there is no binding graph and no component tree:
//! this struct *is* the app, and the frame reads it. What used to be a signal
//! subscription is a field read; what used to be a `useEffect` is a line in the
//! right order.

use std::time::Instant;

use egui::{vec2, Align, Key, KeyboardShortcut, Layout, Modifiers, Rect, RichText, Sense, Ui};

use tapeti_core::types::Uid;
use tapeti_core::writer::required_version;

use crate::actions::Then;
use crate::commands;
use crate::datawin::DataWin;
use crate::editor::EditorState;
use crate::fmt;
use crate::icons;
use crate::list::{self, RowCache};
use crate::menu::Menu;
use crate::menutable;
use crate::player::{Player, Progress};
use crate::settings::Theme;
use crate::state::{SelectMode, Side, Store};
use crate::statusbar;
use crate::theme::{self, Tokens};

const EDITOR_MIN: f32 = 140.0;
const EDITOR_DEFAULT: f32 = 340.0;
const PANE_MIN: f32 = 300.0;
/// `.vsplitter`, the gap between the two pane cards.
const SPLITTER_W: f32 = 10.0;
/// `.pane-head`: 22px of side tag inside 8px of padding, plus its rule.
const HEAD_H: f32 = 37.0;

/// Blocks being dragged, and where they came from.
pub struct Drag {
    pub from: Side,
    pub indices: Vec<usize>,
}

/// What `--bench` collects, and what the status bar shows while it runs.
#[derive(Default)]
struct Perf {
    update: Vec<f64>,
    frame: Vec<f64>,
    last_update: f64,
}

/// Where the pointer is at a file drop, and whether Shift (insert rather than
/// open) is held. On macOS egui's own answer is stale, so AppKit is asked
/// (`macos::pointer_at_drop`); the tests draw no window, and keep egui's.
fn pointer_at_drop(ctx: &egui::Context) -> (Option<egui::Pos2>, bool) {
    #[cfg(all(target_os = "macos", not(test)))]
    return crate::macos::pointer_at_drop(ctx);
    #[cfg(not(all(target_os = "macos", not(test))))]
    ctx.input(|i| (i.pointer.hover_pos(), i.modifiers.shift))
}

/// The pane a file dropped at `at` goes to: the column either side of the
/// splitter, editor included, not only the block list. If where it fell is not
/// known at all, the active pane has it rather than nobody.
fn drop_side(at: Option<egui::Pos2>, splitter_x: f32, active: Side) -> Side {
    match at {
        Some(p) if p.x < splitter_x => 0,
        Some(_) => 1,
        None => active,
    }
}

fn stats(v: &[f64]) -> (f64, f64, f64) {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (s[0], s[s.len() / 2], s[s.len() - 1])
}

pub struct App {
    /// A handle on the context, so a command can answer a menu click with an
    /// input event (see `commands::text_field_edit`).
    pub ctx: egui::Context,
    /// Whether a text field has the keyboard, read once a frame.
    pub text_focus: bool,
    pub store: Store,
    pub player: Player,
    pub progress: Progress,
    pub menu: Menu,
    pub rows: [RowCache; 2],
    pub editor: [EditorState; 2],
    pub datawin: Option<DataWin>,
    pub context_menu: Option<(Side, egui::Pos2)>,
    /// The frame the context menu opened in, whose click is the one that opened it.
    pub context_menu_frame: u64,
    pub drag: Option<Drag>,
    pub drop_target: Option<(Side, (usize, bool))>,
    pub tokens: Tokens,
    /// A row to bring into view on the next frame, per pane.
    pub scroll_to: [Option<usize>; 2],
    pub scroll_offset: [f32; 2],
    pub view_h: [f32; 2],
    /// An action a dialog agreed to, run once the dialog is gone.
    pub pending: Option<Then>,
    theme_applied: Option<(Theme, bool)>,

    start: Instant,
    first_frame: bool,
    exit_on_draw: bool,
    bench_left: usize,
    benching: bool,
    perf: Perf,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        store: Store,
        start: Instant,
        bench: usize,
        exit_on_draw: bool,
    ) -> App {
        // The documents macOS asks for arrive outside the event loop; the queue
        // they land in wakes this context (see `macos.rs`).
        #[cfg(target_os = "macos")]
        crate::macos::wake_with(&cc.egui_ctx);
        App::build(&cc.egui_ctx, Menu::new(cc), store, start, bench, exit_on_draw)
    }

    /// The part of start-up that needs no window, so a test can draw frames.
    pub fn build(
        ctx: &egui::Context,
        menu: Menu,
        store: Store,
        start: Instant,
        bench: usize,
        exit_on_draw: bool,
    ) -> App {
        let tokens = theme::tokens(store.settings.theme, system_dark(ctx));
        ctx.set_visuals(tokens.visuals());
        ctx.set_fonts(theme::fonts());
        App {
            ctx: ctx.clone(),
            text_focus: false,
            store,
            player: Player::default(),
            progress: Progress::default(),
            menu,
            rows: Default::default(),
            editor: Default::default(),
            datawin: None,
            context_menu: None,
            context_menu_frame: 0,
            drag: None,
            drop_target: None,
            tokens,
            scroll_to: [None, None],
            scroll_offset: [0.0, 0.0],
            view_h: [400.0, 400.0],
            pending: None,
            theme_applied: None,
            start,
            first_frame: true,
            exit_on_draw,
            bench_left: bench,
            benching: bench > 0,
            perf: Perf::default(),
        }
    }

    pub fn scroll_to_cursor(&mut self, side: Side) {
        let cursor = self.store.tape(side).cursor;
        if cursor >= 0 {
            self.scroll_to[side] = Some(cursor as usize);
        }
    }

    pub fn open_data_window(&mut self, side: Side, uids: Vec<Uid>) {
        self.datawin = Some(DataWin::new(&self.store.tape(side).blocks, side, uids));
    }

    /// The row the playing marker sits on: a block inside a collapsed range
    /// marks the range's header, as the web list does.
    pub fn playing_row(&self, side: Side) -> Option<usize> {
        if self.progress.side != Some(side) || self.progress.block < 0 {
            return None;
        }
        let mut row = self.progress.block as usize;
        let t = self.store.tape(side);
        let ranges = t.ranges();
        for (s, e) in ranges {
            if s < row && row <= e && t.collapsed.contains(&t.blocks[s].uid) {
                row = s;
            }
        }
        Some(row)
    }

    fn modal_open(&self) -> bool {
        self.store.dialog.is_some() || self.datawin.is_some()
    }

    // ---- keyboard ---------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.store.dialog.is_some() {
                self.store.dialog = None;
            } else if self.datawin.is_some() {
                self.datawin = None;
            } else {
                self.context_menu = None;
            }
            return;
        }
        if self.modal_open() {
            return; // the modal handles its own keys
        }
        let side = self.store.active;

        // Accelerators. On macOS the platform menu owns every one it declares,
        // so these never arrive; elsewhere this is what makes them work.
        if !fmt::IS_MAC {
            let mut fired = None;
            for item in menutable::flat() {
                if let Some((mods, key)) = commands::accelerator(item) {
                    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key))) {
                        fired = Some(item.id);
                        break;
                    }
                }
            }
            if let Some(id) = fired {
                commands::run(self, id, side);
                self.scroll_to_cursor(side);
                return;
            }
        }

        // Keys that belong to a focused text field, not to the tape.
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }

        for (key, id) in commands::PLAIN_KEYS {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, *key)) {
                commands::run(self, id, side);
                self.scroll_to_cursor(side);
                return;
            }
        }

        // Cursor movement and play/stop are not menu commands: they need the
        // modifier state, which an accelerator cannot express.
        let t = self.store.tape(side);
        let (cursor, len) = (t.cursor, t.blocks.len() as i32);
        let shift = ctx.input(|i| i.modifiers.shift);
        let mode = if shift { SelectMode::Range } else { SelectMode::Single };
        let mut moved = None;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::SHIFT, Key::ArrowUp) {
                moved = Some((cursor - 1).max(0));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown)
                || i.consume_key(Modifiers::SHIFT, Key::ArrowDown)
            {
                moved = Some((cursor + 1).min(len - 1));
            }
            if i.consume_key(Modifiers::NONE, Key::Home) {
                moved = Some(0);
            }
            if i.consume_key(Modifiers::NONE, Key::End) {
                moved = Some(len - 1);
            }
            if i.consume_key(Modifiers::NONE, Key::PageUp) {
                moved = Some((cursor - 20).max(0));
            }
            if i.consume_key(Modifiers::NONE, Key::PageDown) {
                moved = Some((cursor + 20).min(len - 1));
            }
        });
        if let Some(index) = moved {
            if len > 0 {
                self.store.set_cursor(side, index, mode);
                self.scroll_to_cursor(side);
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space)) {
            if self.player.playing() {
                self.player.stop();
            } else {
                crate::actions::play_tape(self, side, true);
            }
        }
    }

    fn handle_menu(&mut self) {
        for id in self.menu.take_activated() {
            let side = self.store.active;
            commands::run(self, &id, side);
            self.scroll_to_cursor(side);
        }
    }

    /// Push the enabled and checked state of every item to the platform menu.
    fn sync_menu(&mut self) {
        let side = self.store.active;
        let state = commands::menu_state(self, side);
        let flags = menutable::enabled_flags(&state);
        let checks: Vec<bool> = menutable::flat().iter().map(|i| commands::checked(self, i.id)).collect();
        self.menu.set_state(&flags, &checks);
    }

    // ---- panes ------------------------------------------------------------

    fn panes(&mut self, ui: &mut Ui) {
        let full = ui.available_rect_before_wrap();
        let avail = full.width() - SPLITTER_W;
        let min_frac = if avail > 2.0 * PANE_MIN { PANE_MIN / avail } else { 0.5 };
        let frac = self.store.settings.pane_split.clamp(min_frac, 1.0 - min_frac);
        let left_w = avail * frac;

        let left = Rect::from_min_size(full.min, vec2(left_w, full.height()));
        let bar = Rect::from_min_size(egui::pos2(left.right(), full.top()), vec2(SPLITTER_W, full.height()));
        let right =
            Rect::from_min_size(egui::pos2(bar.right(), full.top()), vec2(avail - left_w, full.height()));

        for (side, rect) in [(0usize, left), (1usize, right)] {
            // Salted per side: the two panes draw the same widgets, and their
            // ids have to stay apart.
            let builder = egui::UiBuilder::new()
                .id_salt(("pane", side))
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min));
            let mut child = ui.new_child(builder);
            self.pane(&mut child, side, rect);
        }
        // Files dropped from outside, once both panes are drawn: loading one
        // replaces a tape whose rows this frame has already laid out.
        let dropped: Vec<std::path::PathBuf> =
            ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            let (at, shift) = pointer_at_drop(ui.ctx());
            let side = drop_side(at, bar.center().x, self.store.active);
            crate::files::open_paths(&mut self.store, side, &dropped, shift);
        }

        let r = ui.interact(bar, ui.id().with("vsplitter"), Sense::click_and_drag());
        ui.painter().rect_filled(bar, egui::CornerRadius::ZERO, self.tokens.bg);
        ui.painter().vline(bar.center().x, bar.y_range(), egui::Stroke::new(1.0, self.tokens.border));
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if r.dragged() {
            let dx = r.drag_delta().x / avail;
            self.store.settings.pane_split = (frac + dx).clamp(min_frac, 1.0 - min_frac);
        }
        if r.double_clicked() {
            self.store.settings.pane_split = 0.5;
        }
        if r.drag_stopped() || r.double_clicked() {
            self.store.settings.save();
        }
        ui.advance_cursor_after_rect(full);
    }

    fn pane(&mut self, ui: &mut Ui, side: Side, rect: Rect) {
        let tok = self.tokens;
        let active = self.store.active == side;
        ui.painter().rect_filled(rect, egui::CornerRadius::same(8), tok.surface);
        // `.pane-head` is a band of its own, not the list's background: a tinted
        // strip across the top of the card, ruled off from the rows below it.
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
        ui.painter().rect_filled(head, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, tok.surface_2);
        ui.painter().hline(head.x_range(), head.bottom() - 0.5, egui::Stroke::new(1.0, tok.border));
        if active {
            // `.pane.active` takes the *soft* accent, not the accent: a full
            // strength ring round half the window is a shout, and the web
            // never did it.
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0, tok.accent_soft_2),
                egui::StrokeKind::Inside,
            );
        } else {
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0, tok.border),
                egui::StrokeKind::Inside,
            );
        }
        if ui.interact(rect, ui.id().with(("pane", side)), Sense::click()).clicked() {
            self.store.active = side;
        }
        ui.add_space(7.0);
        self.pane_head(ui, side);
        ui.add_space(8.0);

        let editor_h =
            self.store.settings.editor_height.clamp(EDITOR_MIN, (rect.height() - 160.0).max(EDITOR_MIN));
        let list_h = (rect.height() - HEAD_H - editor_h - 18.0).max(60.0);
        let list_rect = Rect::from_min_size(
            egui::pos2(rect.left() + 1.0, ui.cursor().top()),
            vec2(rect.width() - 2.0, list_h),
        );
        let builder = egui::UiBuilder::new()
            .id_salt(("list", side))
            .max_rect(list_rect)
            .layout(Layout::top_down(Align::Min));
        let mut child = ui.new_child(builder);
        list::show(self, &mut child, side);
        ui.advance_cursor_after_rect(list_rect);

        // The editor splitter: drag to resize, double-click to restore.
        let bar = Rect::from_min_size(egui::pos2(rect.left(), ui.cursor().top()), vec2(rect.width(), 7.0));
        let r = ui.interact(bar, ui.id().with(("hsplitter", side)), Sense::click_and_drag());
        // `.splitter` and its `::after` grab handle: a tinted strip under a
        // rule, with a 36×3 bar in the middle that takes the accent on hover.
        ui.painter().rect_filled(bar, egui::CornerRadius::ZERO, tok.surface_2);
        ui.painter().hline(bar.x_range(), bar.top() + 0.5, egui::Stroke::new(1.0, tok.border));
        let grip = Rect::from_center_size(egui::pos2(bar.center().x, bar.top() + 3.5), vec2(36.0, 3.0));
        let lit = r.hovered() || r.dragged();
        ui.painter().rect_filled(
            grip,
            egui::CornerRadius::same(2),
            if lit { tok.accent } else { tok.border_strong },
        );
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if r.dragged() {
            self.store.settings.editor_height = (editor_h - r.drag_delta().y).max(EDITOR_MIN);
        }
        if r.double_clicked() {
            self.store.settings.editor_height = EDITOR_DEFAULT;
        }
        if r.drag_stopped() || r.double_clicked() {
            self.store.settings.save();
        }
        ui.advance_cursor_after_rect(bar);

        // `.editor` is a panel of its own: tinted, so the white fields on it
        // read as fields, and rounded off at the bottom of the card.
        let band = Rect::from_min_max(
            egui::pos2(rect.left() + 1.0, ui.cursor().top()),
            egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0),
        );
        ui.painter().rect_filled(band, egui::CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }, tok.surface_2);
        let editor_rect = Rect::from_min_size(
            egui::pos2(rect.left() + 12.0, ui.cursor().top() + 6.0),
            vec2(rect.width() - 24.0, editor_h),
        );
        let builder = egui::UiBuilder::new()
            .id_salt(("editor", side))
            .max_rect(editor_rect)
            .layout(Layout::top_down(Align::Min));
        let mut child = ui.new_child(builder);
        crate::editor::show(self, &mut child, side, editor_h);
    }

    fn pane_head(&mut self, ui: &mut Ui, side: Side) {
        let tok = self.tokens;
        let mut run: Option<&'static str> = None;
        let active = self.store.active == side;
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            // The web's .pane-title gap; the toolbar below sets its own.
            ui.spacing_mut().item_spacing.x = 8.0;
            let t = self.store.tape(side);
            crate::widgets::side_tag(ui, if side == 0 { "L" } else { "R" }, active, &tok);
            ui.label(RichText::new(&t.name).size(13.0).strong());
            if t.dirty() {
                icons::inline(ui, &icons::DOT, tok.warn, 8.0).on_hover_text("Unsaved changes");
            }
            if !t.blocks.is_empty() {
                let v = required_version(&t.blocks);
                crate::widgets::pill(ui, &format!("TZX {}.{:02}", v.major, v.minor), &tok)
                    .on_hover_text("TZX version this tape will be saved as");
            }
            // Right to left, so the order here is the reverse of the web
            // toolbar's: folder, save | insert, play, emulator, programs, info | more.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0; // the web's .toolbar gap
                let has = !self.store.tape(side).blocks.is_empty();
                // What is per tape and has no button here: the overflow menu,
                // `paneMenu` on the web.
                let more = icons::button(ui, &icons::MORE, "More for this tape", true);
                egui::Popup::menu(&more).show(|ui| {
                    for id in menutable::PANE_MENU {
                        if id.is_empty() {
                            ui.separator();
                            continue;
                        }
                        let Some(it) = menutable::item(id) else { continue };
                        let button = egui::Button::new(it.label).shortcut_text(crate::fmt::accel(it.keys));
                        if ui.add_enabled(commands::enabled(self, id, side), button).clicked() {
                            run = Some(it.id);
                            ui.close();
                        }
                    }
                });
                crate::widgets::vsep(ui, &tok);
                if icons::button(ui, &icons::INFO, "Tape info…", has).clicked() {
                    run = Some("tape-info");
                }
                if icons::button(ui, &icons::LIST, "Programs…", has).clicked() {
                    run = Some("programs");
                }
                if icons::button(ui, &icons::LAUNCH, "Open tape in emulator", has).clicked() {
                    run = Some("emu-tape");
                }
                let playing = self.player.playing();
                let (icon, hover) = if playing {
                    (&icons::STOP, "Stop playback")
                } else {
                    (&icons::PLAY, "Play from cursor")
                };
                if icons::button(ui, icon, hover, has).clicked() {
                    run = Some(if playing { "stop" } else { "play-cursor" });
                }
                if icons::button(ui, &icons::PLUS, "Insert block…", true).clicked() {
                    run = Some("insert");
                }
                // Right to left: the rule the web draws between save and insert.
                crate::widgets::vsep(ui, &tok);
                let save_hover = if self.store.tape(side).path.is_some() { "Save" } else { "Save as TZX" };
                if icons::button(ui, &icons::SAVE, save_hover, has).clicked() {
                    run = Some("save");
                }
                if icons::button(ui, &icons::FOLDER, "Open tape…", true).clicked() {
                    run = Some("open");
                }
                // `.playing-pill`, in the header of the pane that is playing.
                if self.progress.playing && self.progress.side == Some(side) {
                    ui.add_space(6.0);
                    let time = RichText::new(crate::fmt::time(self.progress.elapsed)).size(12.0).monospace();
                    ui.label(time.color(tok.accent));
                }
            });
        });
        if let Some(id) = run {
            commands::run(self, id, side);
        }
    }

    // ---- measuring --------------------------------------------------------

    /// `--bench`: one cursor move per frame, so every sample is a real frame.
    fn bench_step(&mut self, ctx: &egui::Context) {
        if !self.benching {
            return;
        }
        if self.bench_left == 0 {
            if !self.perf.update.is_empty() {
                let (lo, mid, hi) = stats(&self.perf.update);
                let (flo, fmid, fhi) = stats(&self.perf.frame);
                println!(
                    "cursor move over {} frames on {} rows:\n  \
                     our own frame build  min {lo:.2} ms, median {mid:.2} ms, max {hi:.2} ms\n  \
                     whole frame          min {flo:.2} ms, median {fmid:.2} ms, max {fhi:.2} ms",
                    self.perf.update.len(),
                    self.store.tape(0).blocks.len()
                );
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.bench_left -= 1;
        let down = self.perf.update.len() % 40 < 20;
        let t = self.store.tape(0);
        let last = t.blocks.len() as i32 - 1;
        let next = (t.cursor + if down { 1 } else { -1 }).clamp(0, last.max(0));
        self.store.set_cursor(0, next, SelectMode::Single);
        self.scroll_to_cursor(0);
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.store.settings.save();
    }
}

impl App {
    /// One frame. Separate from the `eframe::App` impl so the tests can run it
    /// against a bare `egui::Context`, with no window and no event loop.
    pub fn frame(&mut self, ui: &mut Ui) {
        let t0 = Instant::now();
        let ctx = ui.ctx().clone();
        self.ctx = ctx.clone();
        let ctx = &ctx;
        // Read before anything draws, so it describes the field that had the
        // keyboard when the menu item was clicked.
        self.text_focus = ctx.text_edit_focused();

        // The theme follows the desktop when it is set to "system".
        let system_dark = system_dark(ctx);
        let want = (self.store.settings.theme, system_dark);
        if self.theme_applied != Some(want) {
            self.tokens = theme::tokens(want.0, want.1);
            ctx.set_visuals(self.tokens.visuals());
            // The WM draws the title bar from `_GTK_THEME_VARIANT`, and winit's
            // X11 fallback for "no preference" is dark: a black bar over a light
            // window. So name the variant rather than leaving it unset.
            //
            // macOS in "system" mode is the exception: naming one pins
            // `NSWindow.appearance`, and winit's observer then stops reporting
            // appearance changes for a window the app has customised, so
            // "system" would freeze at whatever it was at launch. Unpinned the
            // title bar already follows the desktop, and going back to "system"
            // unpins it, which makes the observer emit the theme it missed.
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(
                if cfg!(target_os = "macos") && want.0 == Theme::System {
                    egui::SystemTheme::SystemDefault
                } else if self.tokens.dark {
                    egui::SystemTheme::Dark
                } else {
                    egui::SystemTheme::Light
                },
            ));
            // Frame one has no window yet, so that hint is dropped: leave
            // `theme_applied` unset to send it again once there is one.
            if !self.first_frame {
                self.theme_applied = Some(want);
            }
        }

        self.progress = self.player.poll();
        if self.progress.playing {
            ctx.request_repaint();
            if let Some(side) = self.progress.side {
                if let Some(row) = self.playing_row(side) {
                    self.scroll_to[side] = Some(row);
                }
            }
        }
        // A drop target is only meaningful while the pointer is over a list;
        // each pane sets it again this frame if it is.
        self.drop_target = None;
        // Tapes the OS asked for while the app was already running (macOS sends an
        // Apple Event, not argv).
        #[cfg(target_os = "macos")]
        {
            let pending = crate::macos::take_pending();
            if !pending.is_empty() {
                crate::files::open_with(&mut self.store, &pending);
                self.scroll_to_cursor(self.store.active);
            }
        }
        self.handle_menu();
        self.handle_keys(ctx);
        if let Some(then) = self.pending.take() {
            crate::actions::run_then(self, then);
        }
        list::refresh(self);
        self.bench_step(ctx);
        self.sync_menu();

        // Where the platform takes a menu bar (macOS) it is muda's and the window
        // has none; otherwise it is drawn here, from the same table.
        if self.menu.draws_in_window() {
            let active = self.store.active;
            let state = commands::menu_state(self, active);
            let mut fired = None;
            // `.menubar`: 44px of `--surface` ruled off from the panes.
            let tok = self.tokens;
            let out = egui::Panel::top("menubar")
                .frame(egui::Frame::new().fill(tok.surface).inner_margin(egui::Margin {
                    left: 12,
                    right: 10,
                    top: 9,
                    bottom: 9,
                }))
                .show(ui, |ui| {
                    fired = self.menu.bar(ui, &state, active, &tok);
                });
            let bar = out.response.rect;
            ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, egui::Stroke::new(1.0, tok.border));
            if let Some((id, side)) = fired {
                if id == crate::menu::THEME {
                    self.store.settings.theme = self.store.settings.theme.next();
                    self.store.settings.save();
                } else {
                    commands::run(self, &id, side);
                    self.scroll_to_cursor(side);
                }
            }
        }

        // `.statusbar`: the same surface as the menu bar, ruled off the same way.
        let tok = self.tokens;
        let out = egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(tok.surface).inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 6,
                bottom: 6,
            }))
            .show(ui, |ui| statusbar::show(self, ui));
        let bar = out.response.rect;
        ui.painter().hline(bar.x_range(), bar.top() + 0.5, egui::Stroke::new(1.0, tok.border));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.tokens.bg).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| self.panes(ui));
        list::finish_drag(self, ctx);

        crate::dialogs::draw(self, ctx);
        crate::datawin::draw(self, ctx);
        list::context_menu(self, ctx);
        if let Some(then) = self.pending.take() {
            crate::actions::run_then(self, then);
        }

        // The window title mirrors the active tape.
        let t = self.store.active_tape();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{}{} — Tapeti",
            t.name,
            if t.dirty() { " *" } else { "" }
        )));

        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.perf.last_update = ms;
        if !self.first_frame {
            // The first frame includes font atlas building and window set-up; it
            // is the cold start number, not a cursor move.
            self.perf.update.push(ms);
            self.perf.frame.push(f64::from(ctx.input(|i| i.unstable_dt)) * 1000.0);
        }
        if self.first_frame {
            self.first_frame = false;
            println!(
                "first frame: {:.0} ms after main() started ({} rows)",
                self.start.elapsed().as_secs_f64() * 1000.0,
                self.store.tape(0).blocks.len()
            );
            if self.exit_on_draw {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

/// Whether the desktop asked for a dark theme; what `Theme::System` follows.
fn system_dark(ctx: &egui::Context) -> bool {
    ctx.system_theme().unwrap_or(ctx.theme()) == egui::Theme::Dark
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Scope, Then};
    use crate::dialogs::Dialog;
    use crate::settings::Settings;
    use tapeti_core::types::{create_body, Block, CREATABLE_IDS};

    /// A tape with one block of every type the editor can create, which is what
    /// makes one frame cover all 25 forms.
    fn every_block() -> Vec<Block> {
        CREATABLE_IDS.iter().map(|id| Block::new(create_body(*id))).collect()
    }

    fn app_with(blocks: Vec<Block>) -> (egui::Context, App) {
        let ctx = egui::Context::default();
        let mut store = Store::new(Settings::default());
        store.tape_mut(0).load("demo.tzx".into(), None, blocks, None);
        let app = App::build(&ctx, Menu::headless(), store, Instant::now(), 0, false);
        (ctx, app)
    }

    /// Draw one frame against a bare context: no window, no event loop, but the
    /// same code the window runs — `run_ui` hands over the root `Ui` that eframe
    /// would. A layout panic or an out-of-range index shows up here instead of
    /// on screen.
    fn draw(ctx: &egui::Context, app: &mut App) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
    }

    #[test]
    fn draws_a_frame_with_the_cursor_on_every_block_type() {
        let (ctx, mut app) = app_with(every_block());
        for i in 0..CREATABLE_IDS.len() {
            app.store.set_cursor(0, i as i32, SelectMode::Single);
            draw(&ctx, &mut app);
        }
    }

    /// The text one frame paints, in painting order.
    fn painted_text(ctx: &egui::Context, app: &mut App) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
            ..Default::default()
        };
        let mut out = String::new();
        for clipped in ctx.run_ui(input, |ui| app.frame(ui)).shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Samples and CSW pulse lengths have no flag byte or checksum; reading their
    /// first and last byte as one reported a bad checksum on a sound block.
    #[test]
    fn the_editor_finds_flag_and_checksum_only_in_loader_bytes() {
        use tapeti_core::types::Body;
        let data = vec![22, 22, 22, 22, 0, 0x10, 0x27, 0, 0];
        let with = |id| {
            let mut body = create_body(id);
            match &mut body {
                Body::Standard { data: d, .. } | Body::Direct { data: d, .. } | Body::Csw { data: d, .. } => {
                    *d = data.clone()
                }
                _ => unreachable!(),
            }
            Block::new(body)
        };
        let (ctx, mut app) = app_with(vec![with(0x10), with(0x15), with(0x18)]);
        for (i, rom) in [(0, true), (1, false), (2, false)] {
            app.store.set_cursor(0, i, SelectMode::Single);
            draw(&ctx, &mut app);
            let text = painted_text(&ctx, &mut app);
            assert!(text.contains("Data length 9 bytes") || rom, "block {i}:\n{text}");
            assert_eq!(text.contains("Flag byte"), rom, "block {i}:\n{text}");
            assert_eq!(text.contains("Checksum byte"), rom, "block {i}:\n{text}");
        }
    }

    #[test]
    fn draws_an_empty_tape() {
        let (ctx, mut app) = app_with(Vec::new());
        draw(&ctx, &mut app);
    }

    #[test]
    fn draws_every_dialog() {
        let (ctx, mut app) = app_with(every_block());
        app.store.set_cursor(0, 0, SelectMode::Single);
        let dialogs = [
            Dialog::message("Hello", vec!["one".into(), "two".into()]),
            Dialog::confirm("Sure?", vec!["really".into()], Then::NewTape(0)),
            Dialog::about(),
            Dialog::insert(0),
            Dialog::tape_info(0),
            Dialog::consistency(0),
            Dialog::wav(0),
            Dialog::programs(0),
            Dialog::emulator(true, Some(Then::EmulatorGo(0, Scope::Tape)), &Settings::default()),
            Dialog::snapshot(0, "game.sna", crate::files::tests::snapshot(), false),
            Dialog::data_file(0, "title.scr", vec![0; 6912]),
            Dialog::data_file(0, "huge.bin", vec![0; 70_000]),
        ];
        for d in dialogs {
            app.store.dialog = Some(d);
            draw(&ctx, &mut app);
            app.store.dialog = None;
        }
    }

    #[test]
    fn draws_every_data_window_view() {
        use crate::datawin::ViewAs;
        let (ctx, mut app) = app_with(every_block());
        // The standard-speed data block, filled with something to look at.
        let uid = app.store.tape(0).blocks[0].uid;
        app.store.replace_block(
            0,
            uid,
            tapeti_core::types::Body::Standard { pause: 1000, data: (0..=255u8).collect() },
        );
        app.store.set_cursor(0, 0, SelectMode::Single);
        for view in [
            ViewAs::Dump,
            ViewAs::Header,
            ViewAs::Screen,
            ViewAs::Basic,
            ViewAs::Vars,
            ViewAs::Text,
            ViewAs::Dis,
        ] {
            app.open_data_window(0, vec![uid]);
            app.datawin.as_mut().unwrap().set_view(view);
            draw(&ctx, &mut app);
            // And with the symbol strip open, a table in it (one line of it bad),
            // and the bytes decrypted.
            app.store.symbols = "$8000 START\n32770 NEXT\nnot a symbol".into();
            app.datawin.as_mut().unwrap().show_everything();
            draw(&ctx, &mut app);
            assert!(app.store.symbols.starts_with("$8000"), "the table goes back to the store");
            app.datawin = None;
        }
    }

    /// Typing in the dump, through the frame that the window runs: a standard
    /// block opens with "hide flag byte" and "hide checksum byte" ticked, and
    /// the two keys must still land — on the first body byte, not on the flag.
    /// The window used to turn read-only the moment any modifier was on, which
    /// is every ordinary tape block.
    #[test]
    fn typing_in_the_dump_edits_the_byte_the_view_shows() {
        let data = vec![0xff, 0x11, 0x22, 0x33, 0xaa];
        let blocks = vec![Block::new(tapeti_core::types::Body::Standard { pause: 1000, data })];
        let uid = blocks[0].uid;
        let (ctx, mut app) = app_with(blocks);
        app.store.set_cursor(0, 0, SelectMode::Single);
        app.open_data_window(0, vec![uid]);
        app.datawin.as_mut().unwrap().set_view(crate::datawin::ViewAs::Dump);
        draw(&ctx, &mut app);

        for key in ["9", "9"] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
                events: vec![egui::Event::Text(key.into())],
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
        }
        assert_eq!(
            app.datawin.as_ref().unwrap().work_data(),
            [0xff, 0x99, 0x22, 0x33, 0xaa],
            "the typed byte went to the flag byte, or nowhere at all"
        );
    }

    /// Editing a program as text: the block gets the new lines and a checksum
    /// that is right again, the header in front gets the new length and VARS, and
    /// the two are one step to undo.
    #[test]
    fn editing_basic_as_text_keeps_block_and_header_in_step() {
        use tapeti_core::describe::{checksum, decode_header, encode_header, HeaderInfo};
        use tapeti_core::spectrum::source::{tokenise_line, SourceOptions};
        use tapeti_core::types::Body;

        let program = tokenise_line("10 PRINT \"hello\"", SourceOptions::default()).unwrap();
        let vars = [0x80u8];
        let mut data = vec![0xff];
        data.extend_from_slice(&program);
        data.extend_from_slice(&vars);
        data.push(checksum(&data));
        let header = HeaderInfo {
            kind: 0,
            type_name: "Program".into(),
            name: "demo".into(),
            length: (program.len() + vars.len()) as u16,
            param1: 10,
            param2: program.len() as u16,
        };
        let blocks = vec![
            Block::new(Body::Standard { pause: 1000, data: encode_header(&header) }),
            Block::new(Body::Standard { pause: 1000, data }),
        ];
        let uid = blocks[1].uid;
        let (ctx, mut app) = app_with(blocks);
        app.store.set_cursor(0, 1, SelectMode::Single);
        app.open_data_window(0, vec![uid]);
        draw(&ctx, &mut app);

        let mut dw = app.datawin.take().unwrap();
        // A mistake is reported and changes nothing.
        let errors = dw.retype_program(|text| format!("{text}\nno number"));
        assert_eq!(errors.iter().map(|e| e.line).collect::<Vec<_>>(), [2]);
        assert_eq!(dw.work_data().len(), 1 + program.len() + vars.len() + 1);

        let errors = dw.retype_program(|text| format!("{text}\n20 GO TO 10"));
        assert!(errors.is_empty(), "{errors:?}");
        let added = tokenise_line("20 GO TO 10", SourceOptions::default()).unwrap();
        // The editing frame, with the text box up, draws too.
        let mut dw = dw;
        dw.edit_program("10 PRINT \"hi\"\n20 GO TO 10");
        app.datawin = Some(dw);
        draw(&ctx, &mut app);
        let dw = app.datawin.take().unwrap();
        crate::datawin::apply(&mut app, &dw);

        let t = app.store.tape(0);
        let Body::Standard { data, .. } = &t.blocks[1].body else { panic!() };
        assert_eq!(data[1..data.len() - 2], [program.clone(), added.clone()].concat()[..]);
        assert_eq!(data[data.len() - 2], 0x80, "the variables stay behind the program");
        assert_eq!(checksum(data), 0, "the checksum is right again");
        let Body::Standard { data: head, .. } = &t.blocks[0].body else { panic!() };
        let h = decode_header(head).unwrap();
        assert_eq!(usize::from(h.param2), program.len() + added.len());
        assert_eq!(usize::from(h.length), program.len() + added.len() + vars.len());
        assert_eq!((h.name.trim_end(), h.param1), ("demo", 10), "the rest of the header is left alone");

        app.store.undo(0);
        let t = app.store.tape(0);
        assert_eq!(decode_header(t.blocks[0].body.data().unwrap()).unwrap().param2, header.param2);
        assert_eq!(t.blocks[1].body.data().unwrap().len(), 1 + program.len() + vars.len() + 1);
    }

    #[test]
    fn draws_a_collapsed_group_and_a_context_menu() {
        let ids = [0x21u8, 0x10, 0x22, 0x20];
        let (ctx, mut app) = app_with(ids.iter().map(|id| Block::new(create_body(*id))).collect());
        let group = app.store.tape(0).blocks[0].uid;
        app.store.toggle_collapse(0, group);
        app.store.set_cursor(0, 0, SelectMode::Single);
        app.context_menu = Some((0, egui::pos2(100.0, 100.0)));
        draw(&ctx, &mut app);
    }

    /// Every character of every shortcut label has a glyph in the fonts the app
    /// sets. On macOS egui's own fonts have ⌘ and none of ⌃⌥⇧ or the arrows, and
    /// every menu egui drew showed them as empty boxes (`theme::fonts`).
    #[test]
    fn every_shortcut_label_has_its_glyphs() {
        let (ctx, _app) = app_with(Vec::new());
        let labels: Vec<String> = menutable::flat().iter().map(|it| fmt::accel(it.keys)).collect();
        let font = egui::FontId::proportional(13.0);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for label in &labels {
                for c in label.chars() {
                    assert!(ui.fonts_mut(|f| f.has_glyph(&font, c)), "no glyph for {c:?} in {label:?}");
                }
            }
        })
        .drop_without_applying_deltas();
    }

    /// A file dropped from outside goes to the pane on its side of the splitter —
    /// over the editor as well as over the list — and to the active pane when
    /// nobody knows where it fell. It used to need egui's pointer inside a block
    /// list, and on macOS that pointer is from before the drag: most drops went
    /// nowhere.
    /// What egui-winit hands over for a file let go on the window (its own
    /// `NativeFile` is private to it).
    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn a_dropped_file_goes_to_the_pane_it_fell_on() {
        let dir = std::env::temp_dir().join(format!("tapeti-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dropped.tzx");
        let blocks = [Block::new(create_body(0x10)), Block::new(create_body(0x20))];
        std::fs::write(&path, tapeti_core::writer::serialize_tzx(&blocks, None)).unwrap();

        let drop_at = |at: Option<egui::Pos2>, active: Side| {
            let (ctx, mut app) = app_with(Vec::new());
            app.store.active = active;
            draw(&ctx, &mut app);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
                ..Default::default()
            };
            input.events.extend(at.map(egui::Event::PointerMoved));
            input.dropped_files.push(std::sync::Arc::new(Dropped(path.clone())));
            ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
            [app.store.tape(0).blocks.len(), app.store.tape(1).blocks.len()]
        };
        assert_eq!(drop_at(Some(egui::pos2(900.0, 100.0)), 0), [0, 2], "on the right list");
        assert_eq!(drop_at(Some(egui::pos2(200.0, 600.0)), 1), [2, 0], "on the left editor");
        assert_eq!(drop_at(None, 1), [0, 2], "nowhere known: the active pane");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A right-click on a row opens the menu and it stays open; a click anywhere
    /// else shuts it. The first half did not hold: the release that opened the
    /// menu counted as a click outside it — always, and then, once the menu's rect
    /// was asked instead, whenever the click point had a fraction the menu's
    /// corner, snapped to physical pixels, landed past. On a 2x screen that was
    /// two clicks in three, so this clicks at fractions of a point, as a mouse does.
    #[test]
    fn a_right_click_opens_the_context_menu_and_a_click_elsewhere_shuts_it() {
        let frame = |ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>| {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
                events,
                ..Default::default()
            };
            input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
            ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
        };
        let press = |ctx: &egui::Context, app: &mut App, at: egui::Pos2, button: egui::PointerButton| {
            for pressed in [true, false] {
                let button =
                    egui::Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE };
                frame(ctx, app, vec![egui::Event::PointerMoved(at), button]);
            }
        };
        for (dx, dy) in [(0.0, 0.0), (0.3, 0.7), (0.9, 0.2), (0.6, 0.9)] {
            let ids = [0x10u8, 0x15, 0x10];
            let (ctx, mut app) = app_with(ids.iter().map(|id| Block::new(create_body(*id))).collect());
            frame(&ctx, &mut app, vec![]);
            assert_eq!(ctx.pixels_per_point(), 2.0);
            let at = egui::pos2(200.0 + dx, 88.0 + dy);
            press(&ctx, &mut app, at, egui::PointerButton::Secondary);
            assert_eq!(app.store.tape(0).cursor, 1, "the right-click lands on the second row");
            assert!(app.context_menu.is_some(), "the menu opens at {at:?}");
            for _ in 0..3 {
                frame(&ctx, &mut app, vec![]);
            }
            assert!(app.context_menu.is_some(), "and stays open");
            press(&ctx, &mut app, egui::pos2(900.0, 700.0), egui::PointerButton::Primary);
            assert!(app.context_menu.is_none(), "a click outside it shuts it");
        }
    }

    /// Click where a widget was drawn, over two frames: egui sees the press in
    /// one and the release in the next, which is what makes a click.
    fn click(ctx: &egui::Context, app: &mut App, at: egui::Pos2) {
        for pressed in [true, false] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1200.0, 800.0))),
                events: vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
        }
    }

    /// Where a widget ends up once the layout stops moving: a modal centres
    /// itself on the size it measured the frame before, so the first frame after
    /// a dialog opens draws it somewhere it will not stay.
    fn settled(ctx: &egui::Context, app: &mut App, id: egui::Id) -> Rect {
        // The first frames of a new dialog are drawn where the *previous* one
        // sat, because a modal reuses the position it remembers until it has
        // measured this content, so a couple of frames go by before comparing.
        let mut last = None;
        for _ in 0..3 {
            draw(ctx, app);
        }
        for _ in 0..8 {
            draw(ctx, app);
            let rect = ctx.read_response(id).expect("the widget was drawn").rect;
            if last == Some(rect) {
                return rect;
            }
            last = Some(rect);
        }
        panic!("the layout never settled");
    }

    /// The ✕ in a dialog's title row, clicked. Every dialog is drawn by the same
    /// frame, so About stands for all of them — and About is where the ✕ was
    /// found to do nothing, because the body's answer overwrote it.
    #[test]
    fn the_close_control_closes_a_dialog() {
        let (ctx, mut app) = app_with(every_block());
        for dialog in [Dialog::about(), Dialog::tape_info(0), Dialog::message("Hello", vec!["one".into()])] {
            app.store.dialog = Some(dialog);
            let x = settled(&ctx, &mut app, crate::dialogs::close_button_id());
            click(&ctx, &mut app, x.center());
            assert!(app.store.dialog.is_none(), "the dialog stayed open after its ✕ was clicked");
        }
    }

    /// The editor footer belongs to its own pane. A form row that overflowed
    /// used to widen the `Ui` around it, and egui will not shrink a `Ui` back
    /// below what it has already laid out — so Commit and Revert were laid out
    /// against the wider rect and drawn *past* the pane, under its neighbour,
    /// where the neighbour's background then painted over them. Every block
    /// type, because only some of them have a form wide enough to do it.
    #[test]
    fn the_editor_footer_stays_inside_its_pane() {
        let (ctx, mut app) = app_with(every_block());
        // The right edge of the left pane's editor panel at the size `draw`
        // uses: half the central panel, less the 12 points `pane` insets by.
        let editor_right = 10.0 + (1200.0 - 20.0 - SPLITTER_W) / 2.0 - 12.0;
        for i in 0..CREATABLE_IDS.len() {
            app.store.set_cursor(0, i as i32, SelectMode::Single);
            for _ in 0..3 {
                draw(&ctx, &mut app);
            }
            let rect = ctx
                .read_response(crate::editor::commit_button_id(0))
                .expect("the left pane drew a Commit button")
                .rect;
            assert!(
                rect.right() <= editor_right,
                "with the cursor on block type {i} Commit was drawn at {rect:?}, past \
                 {editor_right} — the right edge of the left pane's editor in a 1200-point window"
            );
        }
    }

    /// Commands that cannot run in a test: the first six open a native file
    /// dialog and would block until someone dismissed it, the rest reach for an
    /// audio device or launch another program. Everything else in the table is
    /// swept below, so a command added later is covered without being listed
    /// here — and if it turns out to need a dialog, this is the list to add it
    /// to.
    const NOT_HEADLESS: &[&str] = &[
        "open",
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
    ];

    /// A name in `NOT_HEADLESS` that no longer matches a command is dead
    /// weight, and worse, silently lets that command back into the sweep under
    /// its new name — where it would open a file dialog and hang.
    #[test]
    fn the_commands_left_out_of_the_sweep_all_exist() {
        for id in NOT_HEADLESS {
            assert!(crate::menutable::item(id).is_some(), "{id:?} is not a command any more");
        }
    }

    /// Every command, run where the menu bar runs one: during the frame, after
    /// the row caches have been rebuilt and before the panes are drawn. That is
    /// the point a tape can be swapped out from under a cache that has already
    /// been built for the old one, and until `Menu::fire_next_frame` there was
    /// no way to reach it from a test at all — the list panic that this guards
    /// against went out in a release because of it.
    #[test]
    fn every_command_survives_being_run_in_the_middle_of_a_frame() {
        for item in crate::menutable::flat() {
            if item.id.is_empty() || NOT_HEADLESS.contains(&item.id) {
                continue;
            }
            let ctx = egui::Context::default();
            let mut store = Store::new(Settings::default());
            // Groups and loops in both panes: `visible_rows` only reaches for a
            // block when a row has a range, so a tape without one cannot show
            // the mismatch this is here to catch.
            store.tape_mut(0).load("left.tzx".into(), None, every_block(), None);
            store.tape_mut(1).load("right.tzx".into(), None, every_block(), None);
            let mut app = App::build(&ctx, Menu::in_window(), store, Instant::now(), 0, false);
            app.store.set_cursor(0, 3, SelectMode::Single);
            draw(&ctx, &mut app);

            app.menu.fire_next_frame(item.id, 0);
            draw(&ctx, &mut app);
            // And again, because a command that opens a dialog does its work on
            // the frame after the one it was picked on.
            draw(&ctx, &mut app);

            for side in 0..2 {
                assert_eq!(
                    app.rows[side].rows.len(),
                    app.store.tape(side).blocks.len(),
                    "after {:?} the rows of pane {side} describe a different tape",
                    item.id
                );
            }
        }
    }

    /// The menu bar egui draws when the platform does not take one — Linux, and
    /// Windows if a window ever refuses muda's. It is drawn here whatever the
    /// platform, so the path is not left to a machine nobody is testing on.
    #[test]
    fn draws_the_in_window_menu_bar() {
        let ctx = egui::Context::default();
        let mut store = Store::new(Settings::default());
        store.tape_mut(0).load("demo.tzx".into(), None, every_block(), None);
        let mut app = App::build(&ctx, Menu::in_window(), store, Instant::now(), 0, false);
        assert!(app.menu.draws_in_window());
        app.store.set_cursor(0, 0, SelectMode::Single);
        draw(&ctx, &mut app);
    }

    #[test]
    fn draws_both_themes() {
        let (ctx, mut app) = app_with(every_block());
        for theme in [Theme::Light, Theme::Dark, Theme::System] {
            app.store.settings.theme = theme;
            draw(&ctx, &mut app);
        }
    }
}
