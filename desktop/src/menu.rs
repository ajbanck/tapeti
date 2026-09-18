//! The application menu, built from the table in `menutable.rs`.
//!
//! Two bars, from one table, and every platform gets the in-window one.
//!
//! On macOS `muda` additionally hangs the table on `NSApp`, the same crate the
//! Tauri shell reached through until stage 5, so the accelerators are the
//! platform's there — `CmdOrCtrl+S` meaning ⌘S. The **window** keeps its own bar
//! as well: the app has had one since it was a web page in a window (it is the
//! app in README.md's screenshot), and dropping it on macOS lost the Left and
//! Right menus from where people had been using them.
//!
//! Everywhere else the in-window bar is the only one, and `app.rs` handles the
//! accelerators itself. **Including Windows**:
//! stage 5 tried `init_for_hwnd` there and it does not work with a winit window
//! — the menu never appeared, a black strip took its place, and every click
//! landed one menu-height away from what it hit, because a Win32 menu shrinks
//! the client area and nothing told egui. muda's accelerators would need a
//! `TranslateAccelerator` in the message loop as well, which winit does not
//! have, so the platform bar on Windows is not a small fix and the egui one is
//! not a stopgap.
//!
//! Clicks arrive on muda's own thread, so they land in a queue and wake the UI;
//! `take_activated` drains it at the top of a frame.

use std::sync::{Arc, Mutex};

use crate::menutable::{item, MenuState, WINDOW_MENUS};
use crate::theme::Tokens;

/// What the theme button reports; not a command id, because nothing but this
/// bar has one to offer.
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
    use crate::menutable::{Item, MENUS};
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
            // Slint, does not add it for you.
            {
                let about = muda::AboutMetadata {
                    name: Some("Tapeti".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                    comments: Some("ZX Spectrum TZX/TAP tape editor".into()),
                    ..Default::default()
                };
                let app_menu = Submenu::new("Tapeti", true);
                let _ = app_menu.append_items(&[
                    &PredefinedMenuItem::about(Some("About Tapeti"), Some(about)),
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
                    if item.id.is_empty() {
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
                let _ = bar.append(&sub);
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

/// The menu bar egui draws inside the window, grouped by pane.
mod in_window {
    use super::*;

    pub struct Menu {
        /// Check marks, by command id, pushed each frame from the store.
        checked: std::cell::RefCell<Vec<(&'static str, bool)>>,
        queue: Queue,
        /// A click a test makes, delivered from where a real one comes from.
        /// See `Menu::fire_next_frame`.
        #[cfg(test)]
        injected: std::cell::RefCell<Option<(String, usize)>>,
    }

    /// `.brand` in `style.css`: the cassette in a rounded square, then the
    /// wordmark. This bar is the app's own, so it says whose it is.
    fn brand(ui: &mut egui::Ui, tok: &Tokens) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, egui::CornerRadius::same(7), tok.brand);
        crate::icons::paint(ui.painter(), rect.shrink(5.5), &crate::icons::CASSETTE, tok.accent_text);
        ui.label(egui::RichText::new("Tapeti").size(13.0).strong().color(tok.text));
        ui.add_space(8.0);
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

        /// Returns what was clicked: a command id and the pane it runs on.
        pub fn bar(
            &self,
            ui: &mut egui::Ui,
            states: &[MenuState; 2],
            active: usize,
            tok: &Tokens,
        ) -> Option<(String, usize)> {
            let checked = self.checked.borrow();
            let mut fired = None;
            egui::MenuBar::new().ui(ui, |ui| {
                brand(ui, tok);
                for menu in WINDOW_MENUS {
                    let side = menu.side.unwrap_or(active);
                    let state = &states[side];
                    ui.menu_button(menu.title, |ui| {
                        for id in menu.ids {
                            if id.is_empty() {
                                ui.separator();
                                continue;
                            }
                            let Some(it) = item(id) else { continue };
                            let on = checked.iter().find(|(i, _)| i == id).is_some_and(|(_, c)| *c);
                            let label = if it.check && on {
                                format!("✓ {}", it.label)
                            } else {
                                it.label.to_string()
                            };
                            let button = egui::Button::new(label).shortcut_text(crate::fmt::accel(it.keys));
                            if ui.add_enabled(it.need.met(state), button).clicked() {
                                fired = Some(((*id).to_string(), side));
                                ui.close();
                            }
                        }
                    });
                }
                // The theme cycle sits at the right end of the bar, where
                // `MenuBar.tsx` puts it. `theme` is not a command id — nothing
                // else can reach it — so the caller reads it off the return.
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
    /// The platform's own bar through muda, *and* the one in the window; macOS
    /// has both, the way the app has always looked there.
    #[cfg(target_os = "macos")]
    Platform { native: platform::Menu, bar: in_window::Menu },
    /// Drawn in the window by egui, and nowhere else.
    InWindow(in_window::Menu),
    /// Neither: the frames the tests draw have no menu at all.
    Headless,
}

impl Menu {
    /// The menu this platform gets.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Menu {
        #[cfg(target_os = "macos")]
        return Menu::Platform { native: platform::Menu::new(&cc.egui_ctx), bar: in_window::Menu::new() };
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
    /// Three of the five places that call `commands::run` do it *during* a
    /// frame, after `App::frame` has refreshed its caches and before the panes
    /// are drawn: this bar, the pane toolbar a few lines above `list::show`,
    /// and the context menu. Nothing in the test layer could reach that point —
    /// tests either call a command between frames or draw a frame in which
    /// nothing is clicked — and that is the gap a load in the middle of a frame
    /// slipped through, leaving rows built for the tape that had just been
    /// replaced. The toolbar's own buttons cannot stand in for it, because the
    /// interesting ones open a native file dialog and would block the test.
    ///
    /// Only the queue is test-only; what happens to the value afterwards is the
    /// production path, unchanged.
    #[cfg(test)]
    pub fn fire_next_frame(&self, id: &str, side: usize) {
        match self {
            #[cfg(target_os = "macos")]
            Menu::Platform { bar, .. } => bar.fire_next_frame(id, side),
            Menu::InWindow(m) => m.fire_next_frame(id, side),
            Menu::Headless => panic!("a headless menu draws no bar to click"),
        }
    }

    /// Whether `bar` has anything to draw this frame: everything but a test.
    pub fn draws_in_window(&self) -> bool {
        !matches!(self, Menu::Headless)
    }

    /// Push the enabled and checked state of every item.
    pub fn set_state(&self, enabled: &[bool], checked: &[bool]) {
        match self {
            #[cfg(target_os = "macos")]
            Menu::Platform { native, bar } => {
                native.set_state(enabled, checked);
                bar.set_state(enabled, checked);
            }
            Menu::InWindow(m) => m.set_state(enabled, checked),
            Menu::Headless => {}
        }
    }

    /// Command ids activated since the last call.
    pub fn take_activated(&self) -> Vec<String> {
        match self {
            #[cfg(target_os = "macos")]
            Menu::Platform { native, .. } => native.take_activated(),
            Menu::InWindow(m) => m.take_activated(),
            Menu::Headless => Vec::new(),
        }
    }

    /// Draw the in-window bar and report what was clicked, with the pane it
    /// belongs to: Left and Right run on their own tape.
    pub fn bar(
        &self,
        ui: &mut egui::Ui,
        states: &[MenuState; 2],
        active: usize,
        tok: &Tokens,
    ) -> Option<(String, usize)> {
        match self {
            #[cfg(target_os = "macos")]
            Menu::Platform { bar, .. } => bar.bar(ui, states, active, tok),
            Menu::InWindow(m) => m.bar(ui, states, active, tok),
            Menu::Headless => None,
        }
    }
}
