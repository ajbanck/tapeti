// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! The application menu, built from the table in `menutable.rs`.
//!
//! One bar per platform, from one table, in one grouping. On macOS `muda`
//! hangs the table on `NSApp`, so accelerators are the platform's there
//! (`CmdOrCtrl+S` means Cmd+S) and the window draws no bar of its own. There
//! are no Left / Right menus: what is aimed at one pane is in that pane's
//! header (`App::pane_head`).
//!
//! Everywhere else egui draws the bar in the window and `app.rs` handles the
//! accelerators. That includes Windows, on purpose: muda's `init_for_hwnd` on
//! a winit window shows a black strip where the menu should be and shrinks the
//! client area without telling egui, so every click lands a menu height off,
//! and its accelerators would need a `TranslateAccelerator` in winit's message
//! loop, which winit lacks. The egui bar there is not a stopgap.
//!
//! Clicks from muda's own thread land in a queue; `take_activated` drains it
//! at the top of a frame.

use std::sync::{Arc, Mutex};

use crate::menutable::{MenuState, MENUS};
use crate::theme::Tokens;

/// What the theme button reports: the three-way cycle, which is not a command
/// id — the View menu's three items each name a theme instead.
pub const THEME: &str = "\0theme";

#[derive(Default)]
pub struct Queue(Arc<Mutex<Vec<String>>>);

impl Queue {
    /// Command ids activated since the last call.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use crate::menutable::Item;
    use muda::accelerator::Accelerator;
    use muda::{CheckMenuItem, Menu as MudaMenu, MenuItem, PredefinedMenuItem, Submenu};

    enum Handle {
        Item(MenuItem),
        Check(CheckMenuItem),
        Separator,
    }

    pub struct Menu {
        /// One entry per flat index of `menutable::flat()`, so the app can address
        /// items the way the table lists them.
        handles: Vec<Handle>,
        /// Kept alive: dropping the menu takes it off the menu bar.
        _bar: MudaMenu,
        queue: Queue,
        /// What was last pushed, so an unchanged frame touches nothing.
        last: std::cell::RefCell<(Vec<bool>, Vec<bool>)>,
    }

    fn accelerator(item: &Item) -> Option<Accelerator> {
        if item.keys.is_empty() {
            return None;
        }
        match item.keys.parse() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("menu: {:?} has an unusable accelerator {:?}: {e}", item.id, item.keys);
                None
            }
        }
    }

    impl Menu {
        pub fn new(ctx: &egui::Context) -> Menu {
            let bar = MudaMenu::new();

            // macOS expects the first menu to be the application's own; muda, unlike
            // Slint, does not add it for you. Its About is the table's `about`, the
            // app's own dialog, rather than AppKit's standard panel, and it leaves
            // the Help menu, which is dropped once it has nothing else.
            let about = crate::menutable::flat().into_iter().find(|i| i.id == "about").expect("about");
            let about = MenuItem::with_id(about.id, about.label, true, None);
            {
                let app_menu = Submenu::new("Tapeti", true);
                let _ = app_menu.append_items(&[
                    &about,
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::services(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::quit(None),
                ]);
                let _ = bar.append(&app_menu);
            }

            let mut handles = Vec::new();
            for menu in MENUS {
                let sub = Submenu::new(menu.title, true);
                for item in menu.items {
                    if item.id == about.id().0 {
                        handles.push(Handle::Item(about.clone()));
                    } else if item.id.is_empty() {
                        let sep = PredefinedMenuItem::separator();
                        let _ = sub.append(&sep);
                        handles.push(Handle::Separator);
                    } else if item.check {
                        let it = CheckMenuItem::with_id(item.id, item.label, true, false, accelerator(item));
                        let _ = sub.append(&it);
                        handles.push(Handle::Check(it));
                    } else {
                        let it = MenuItem::with_id(item.id, item.label, true, accelerator(item));
                        let _ = sub.append(&it);
                        handles.push(Handle::Item(it));
                    }
                }
                if !sub.items().is_empty() {
                    let _ = bar.append(&sub);
                }
            }

            bar.init_for_nsapp();

            let queue = Queue::default();
            let sink = queue.0.clone();
            let ctx = ctx.clone();
            muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
                sink.lock().unwrap().push(event.id.0.clone());
                // The click happens outside egui's event loop, so ask for a frame.
                ctx.request_repaint();
            }));

            Menu { handles, _bar: bar, queue, last: std::cell::RefCell::new((Vec::new(), Vec::new())) }
        }

        /// Push enabled and checked state, skipping the frames where nothing moved.
        pub fn set_state(&self, enabled: &[bool], checked: &[bool]) {
            let mut last = self.last.borrow_mut();
            if last.0 == enabled && last.1 == checked {
                return;
            }
            for (i, h) in self.handles.iter().enumerate() {
                match h {
                    Handle::Item(it) => it.set_enabled(enabled[i]),
                    Handle::Check(it) => {
                        it.set_enabled(enabled[i]);
                        it.set_checked(checked[i]);
                    }
                    Handle::Separator => {}
                }
            }
            *last = (enabled.to_vec(), checked.to_vec());
        }

        pub fn take_activated(&self) -> Vec<String> {
            self.queue.take()
        }
    }
}

/// The menu bar egui draws inside the window, where the platform has none.
mod in_window {
    use super::*;

    pub struct Menu {
        /// Check marks, by command id, pushed each frame from the store.
        checked: std::cell::RefCell<Vec<(&'static str, bool)>>,
        queue: Queue,
        /// A click a test injects; `bar` returns it the same way as a real one.
        /// See `Menu::fire_next_frame`.
        #[cfg(test)]
        injected: std::cell::RefCell<Option<(String, usize)>>,
    }

    /// The left padding every item of a dropdown gets, and the box the tick is
    /// drawn in.
    const TICK_GUTTER: f32 = 20.0;

    /// The check mark in front of an option that is on, painted rather than
    /// written: the app ships no font with a `✓` in it, and the glyph egui
    /// would otherwise draw is an empty box.
    fn tick(ui: &egui::Ui, response: &egui::Response) {
        let colour = ui.style().interact(response).fg_stroke.color;
        let rect = egui::Rect::from_center_size(
            egui::pos2(response.rect.left() + TICK_GUTTER / 2.0, response.rect.center().y),
            egui::vec2(13.0, 13.0),
        );
        crate::icons::paint(ui.painter(), rect, &crate::icons::CHECK, colour);
    }

    impl Menu {
        pub fn new() -> Menu {
            Menu {
                checked: Vec::new().into(),
                queue: Queue::default(),
                #[cfg(test)]
                injected: None.into(),
            }
        }

        #[cfg(test)]
        pub fn fire_next_frame(&self, id: &str, side: usize) {
            self.injected.replace(Some((id.to_string(), side)));
        }

        pub fn set_state(&self, _enabled: &[bool], checked: &[bool]) {
            let flat = crate::menutable::flat();
            self.checked.replace(flat.iter().zip(checked).map(|(i, c)| (i.id, *c)).collect());
        }

        pub fn take_activated(&self) -> Vec<String> {
            self.queue.take()
        }

        /// Returns what was clicked: a command id and the pane it runs on,
        /// which is the active one for anything a person can click.
        pub fn bar(
            &self,
            ui: &mut egui::Ui,
            state: &MenuState,
            active: usize,
            tok: &Tokens,
        ) -> Option<(String, usize)> {
            let checked = self.checked.borrow();
            let mut fired = None;
            egui::MenuBar::new().ui(ui, |ui| {
                for menu in MENUS {
                    ui.menu_button(menu.title, |ui| {
                        // Reserves room for a tick like `.menu .item`'s 28px `::before`
                        // padding, so labels line up whether or not the item above is checked.
                        ui.spacing_mut().button_padding.x = TICK_GUTTER;
                        for it in menu.items {
                            if it.id.is_empty() {
                                ui.separator();
                                continue;
                            }
                            let on = checked.iter().find(|(i, _)| *i == it.id).is_some_and(|(_, c)| *c);
                            let button =
                                egui::Button::new(it.label).shortcut_text(crate::fmt::accel(it.keys));
                            let response = ui.add_enabled(it.need.met(state), button);
                            if it.check && on {
                                tick(ui, &response);
                            }
                            if response.clicked() {
                                fired = Some((it.id.to_string(), active));
                                ui.close();
                            }
                        }
                    });
                }
                // The theme cycle sits at the right end of the bar. `theme` is not a
                // command id: the caller reads it directly off the return.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icon = match tok.theme_icon {
                        0 => &crate::icons::SUN,
                        1 => &crate::icons::MOON,
                        _ => &crate::icons::MONITOR,
                    };
                    if crate::icons::button(ui, icon, "Theme (click to change)", true).clicked() {
                        fired = Some((THEME.to_string(), active));
                    }
                });
            });
            #[cfg(test)]
            let fired = fired.or_else(|| self.injected.borrow_mut().take());
            fired
        }
    }
}

/// Where this run's menu lives.
pub enum Menu {
    /// The platform's own bar through muda: macOS, where the window draws none.
    #[cfg(target_os = "macos")]
    Platform(platform::Menu),
    /// Drawn in the window by egui, and nowhere else.
    InWindow(in_window::Menu),
    /// Neither: the frames the tests draw have no menu at all.
    Headless,
}

impl Menu {
    /// The menu this platform gets.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Menu {
        #[cfg(target_os = "macos")]
        return Menu::Platform(platform::Menu::new(&cc.egui_ctx));
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cc;
            Menu::InWindow(in_window::Menu::new())
        }
    }

    /// A menu that talks to no platform, for drawing frames in a test.
    pub fn headless() -> Menu {
        Menu::Headless
    }

    /// The in-window bar, whatever the platform: what everything but macOS runs,
    /// and what lets a test on macOS draw it too.
    #[allow(dead_code)]
    pub fn in_window() -> Menu {
        Menu::InWindow(in_window::Menu::new())
    }

    /// Pretend the user picked `id` from the bar on the next frame drawn.
    ///
    /// Three of the five callers of `commands::run` fire *during* a frame, after
    /// `App::frame` has refreshed its caches and before the panes are drawn: this
    /// bar, the pane toolbar and the context menu (`handle_menu` and `handle_keys`
    /// run before it). A test that calls a command between frames, or draws a
    /// frame in which nothing is clicked, never reaches that path, where a command
    /// can leave rows built for the tape it just replaced. The toolbar's own
    /// buttons cannot stand in for it: the interesting ones open a native file
    /// dialog and would block the test.
    #[cfg(test)]
    pub fn fire_next_frame(&self, id: &str, side: usize) {
        match self {
            Menu::InWindow(m) => m.fire_next_frame(id, side),
            _ => panic!("only the in-window bar is drawn in a frame, to be clicked"),
        }
    }

    /// Whether `bar` has anything to draw this frame: not on macOS, where the
    /// bar is the platform's, and not in a headless test.
    pub fn draws_in_window(&self) -> bool {
        matches!(self, Menu::InWindow(_))
    }

    /// Push the enabled and checked state of every item.
    pub fn set_state(&self, enabled: &[bool], checked: &[bool]) {
        match self {
            #[cfg(target_os = "macos")]
            Menu::Platform(m) => m.set_state(enabled, checked),
            Menu::InWindow(m) => m.set_state(enabled, checked),
            Menu::Headless => {}
        }
    }

    /// Command ids activated since the last call.
    pub fn take_activated(&self) -> Vec<String> {
        match self {
            #[cfg(target_os = "macos")]
            Menu::Platform(m) => m.take_activated(),
            Menu::InWindow(m) => m.take_activated(),
            Menu::Headless => Vec::new(),
        }
    }

    /// Draw the in-window bar and report what was clicked, with the pane it
    /// runs on.
    pub fn bar(
        &self,
        ui: &mut egui::Ui,
        state: &MenuState,
        active: usize,
        tok: &Tokens,
    ) -> Option<(String, usize)> {
        match self {
            Menu::InWindow(m) => m.bar(ui, state, active, tok),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::settings::Settings;
    use crate::shot::{capture_with, Canvas};
    use crate::state::Store;

    /// Click the menu title `x` points along the bar open, then draw the
    /// dropdown: a real click, through the real bar, because the tick is
    /// painted over the rect the button ends up with.
    fn dropdown(app: &mut App, ctx: &egui::Context, x: f32) -> Canvas {
        click_at(app, ctx, egui::pos2(x, 22.0))
    }

    /// Four frames with a click at `at` in the second; the last one drawn.
    fn click_at(app: &mut App, ctx: &egui::Context, at: egui::Pos2) -> Canvas {
        let click = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        capture_with(
            ctx,
            app,
            (1100.0, 720.0),
            4,
            |frame| {
                if frame == 1 {
                    click.clone()
                } else {
                    Vec::new()
                }
            },
        )
    }

    /// An empty app whose two options are both on or both off. The tape is
    /// empty, so nothing outside the menu can differ between the two frames.
    fn app_with(options: bool) -> (egui::Context, App) {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        settings.hex_bytes = options;
        settings.zero_based = options;
        let store = Store::new(settings);
        let app = App::build(&ctx, Menu::in_window(), store, std::time::Instant::now(), 0, false);
        (ctx, app)
    }

    /// The check mark of an option that is on (see `tick`). The tape is empty,
    /// so nothing but the check-mark pixels differs between the two frames.
    #[test]
    fn an_option_that_is_on_is_ticked() {
        let (ctx, mut app) = app_with(false);
        let plain = dropdown(&mut app, &ctx, VIEW_X);
        let (ctx, mut app) = app_with(true);
        let ticked = dropdown(&mut app, &ctx, VIEW_X);
        let changed = plain.diff(&ticked);
        assert!(changed > 30, "the View menu draws the same {changed} pixels checked or not");
        assert!(changed < 400, "{changed} pixels changed: more than two check marks moved");
    }

    /// The pane header's overflow button opens a menu of its own, over the
    /// list. Drawn without a menu bar, the way macOS draws the window.
    #[test]
    fn the_pane_header_has_an_overflow_menu() {
        let ctx = egui::Context::default();
        let store = Store::new(Settings::default());
        let mut app = App::build(&ctx, Menu::headless(), store, std::time::Instant::now(), 0, false);
        let closed = click_at(&mut app, &ctx, egui::pos2(300.0, 400.0));
        let open = click_at(&mut app, &ctx, MORE_AT);
        let changed = closed.diff(&open);
        assert!(changed > 2000, "clicking the overflow button changed {changed} pixels: no menu opened");
    }

    /// The left pane's overflow button in an 1100 point window with no menu bar.
    const MORE_AT: egui::Pos2 = egui::pos2(506.0, 28.0);

    /// Where "View" sits in the bar: after File, Edit, Block, Tape, Play.
    const VIEW_X: f32 = 213.0;
}
