// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 AJ Banck

//! macOS glue eframe does not provide: Finder document opens, a Quit that can be turned
//! down, and a drag-drop position.
//!
//! Double-clicking a tape does not put a path in `argv`. Launch Services sends a
//! `kAEOpenDocuments` Apple Event, which AppKit turns into `application:openURLs:` on the app
//! delegate, at launch and again on every later open. winit's own delegate implements only its
//! two lifecycle methods, so the message goes nowhere: `install` adds the method from an
//! `NSApplicationWillFinishLaunchingNotification` observer, the first moment the delegate exists
//! and the last before AppKit delivers the launch open, and resets the delegate afterward
//! because `NSApplication` caches which selectors it answers.
//! Opened paths queue in `PENDING` rather than the store because they can arrive on AppKit's
//! thread mid-frame; `App::frame` drains it. `pointer_at_drop` supplies the drop position winit
//! does not report.
//!
//! Quit from the app menu, Cmd+Q, the Dock or a log-out is `-[NSApplication terminate:]`, which
//! never asks the window, so the unsaved-changes question would be skipped. The same install
//! adds `applicationShouldTerminate:`, which lets it through only while `App::frame` says there
//! is nothing to lose (`set_may_quit`), and otherwise cancels it and leaves a request that the
//! next frame answers with the question.

use std::ffi::CStr;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};
use objc2_foundation::{
    NSArray, NSNotification, NSNotificationCenter, NSNotificationName, NSPoint, NSRect, NSString, NSURL,
};

/// Tapes the OS asked for, not yet opened.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// A context to wake, because the event arrives outside egui's event loop.
static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Whether a quit would lose nothing, as of the last frame.
static MAY_QUIT: AtomicBool = AtomicBool::new(true);
/// A quit AppKit was told to cancel, for the next frame to ask about.
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Tell AppKit's next quit whether it may go ahead.
pub fn set_may_quit(may: bool) {
    MAY_QUIT.store(may, Ordering::Relaxed);
}

/// Whether a quit was turned down since the last call.
pub fn take_quit_request() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::Relaxed)
}

/// `-[NSObject applicationShouldTerminate:]`: `NSTerminateNow` (1) when nothing is unsaved,
/// otherwise `NSTerminateCancel` (0) and a request for the app to ask.
unsafe extern "C-unwind" fn should_terminate(_this: &AnyObject, _cmd: Sel, _app: *mut AnyObject) -> usize {
    if MAY_QUIT.load(Ordering::Relaxed) {
        return 1;
    }
    QUIT_REQUESTED.store(true, Ordering::Relaxed);
    if let Some(ctx) = WAKE.lock().unwrap().as_ref() {
        ctx.request_repaint();
    }
    0
}

/// Paths the OS asked us to open since the last call.
pub fn take_pending() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// Ask for a frame when the next document arrives.
pub fn wake_with(ctx: &egui::Context) {
    *WAKE.lock().unwrap() = Some(ctx.clone());
}

fn queue(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    PENDING.lock().unwrap().extend(paths);
    if let Some(ctx) = WAKE.lock().unwrap().as_ref() {
        ctx.request_repaint();
    }
}

/// `-[NSObject application:openURLs:]`, as AppKit will call it: the receiver is
/// winit's delegate, which knows nothing about this method.
unsafe extern "C-unwind" fn open_urls(
    _this: &AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    urls: *mut AnyObject,
) {
    let Some(urls) = NonNull::new(urls) else { return };
    // Safety: AppKit passes an NSArray<NSURL> in this argument.
    let urls: &NSArray<NSURL> = unsafe { urls.cast().as_ref() };
    queue(paths_of(urls));
}

/// The file paths in an array of URLs, skipping any that are not files.
fn paths_of(urls: &NSArray<NSURL>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for i in 0..urls.count() {
        let url = urls.objectAtIndex(i);
        if let Some(path) = url.path() {
            paths.push(PathBuf::from(path.to_string()));
        }
    }
    paths
}

/// Teach the application delegate `application:openURLs:` and
/// `applicationShouldTerminate:`, unless it already knows them (a later winit may
/// implement one, and then it owns the message).
fn add_delegate_methods() {
    let app: *mut AnyObject = unsafe { msg_send![class!(NSApplication), sharedApplication] };
    if app.is_null() {
        return;
    }
    let delegate: *mut AnyObject = unsafe { msg_send![app, delegate] };
    let class: &AnyClass = match NonNull::new(delegate) {
        Some(d) => unsafe { d.as_ref() }.class(),
        // No delegate yet: winit's class is registered all the same.
        None => match AnyClass::get(c"WinitApplicationDelegate") {
            Some(c) => c,
            None => return,
        },
    };
    let add = |selector: Sel, imp: Imp, types: &CStr| {
        !class.responds_to(selector)
            && unsafe {
                ffi::class_addMethod((class as *const AnyClass).cast_mut(), selector, imp, types.as_ptr())
            }
            .as_bool()
    };
    // "v@:@@": returns void, takes the receiver, the selector and two objects.
    let open = add(
        sel!(application:openURLs:),
        unsafe {
            std::mem::transmute::<
                unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *mut AnyObject),
                Imp,
            >(open_urls)
        },
        c"v@:@@",
    );
    // "Q@:@": returns an NSUInteger, takes the receiver, the selector and one object.
    let quit = add(
        sel!(applicationShouldTerminate:),
        unsafe {
            std::mem::transmute::<unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize, Imp>(
                should_terminate,
            )
        },
        c"Q@:@",
    );
    if !(open || quit) || delegate.is_null() {
        return;
    }
    // NSApplication remembers what its delegate answered to when it was set, so
    // set it again now that the answer has changed.
    unsafe {
        let _: () = msg_send![app, setDelegate: std::ptr::null_mut::<AnyObject>()];
        let _: () = msg_send![app, setDelegate: delegate];
    }
}

/// Call once, before the event loop starts. The observer outlives the process on
/// purpose: it is the app's own launch it is waiting for.
pub fn install() {
    let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
    let block = block2::RcBlock::new(|_: NonNull<NSNotification>| add_delegate_methods());
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(&*(&*name as *const NSString as *const NSNotificationName)),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
}

/// Where the pointer is in the window, in points, and whether Shift is down.
///
/// Asked of AppKit rather than egui: winit's `DroppedFile` carries no position, and AppKit sends
/// no mouse moves during a drag, so egui's last-known pointer is often stale, outside the window
/// where the Finder was.
pub fn pointer_at_drop(ctx: &egui::Context) -> (Option<egui::Pos2>, bool) {
    let inner = ctx.input(|i| i.viewport().inner_rect);
    let (mouse, flags, primary) = unsafe {
        let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let flags: usize = msg_send![class!(NSEvent), modifierFlags];
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let first: *mut AnyObject =
            if screens.is_null() { std::ptr::null_mut() } else { msg_send![screens, firstObject] };
        let primary: Option<NSRect> = (!first.is_null()).then(|| msg_send![first, frame]);
        (mouse, flags, primary)
    };
    const SHIFT: usize = 1 << 17; // NSEventModifierFlagShift
    let at = inner.zip(primary).map(|(inner, primary)| window_point(mouse, primary.size.height, inner.min));
    (at, flags & SHIFT != 0)
}

/// Converts an AppKit screen point, counted up from the bottom of the primary screen, to a point
/// in a window whose top left is at `window`, counted down from the top of that same screen.
fn window_point(screen: NSPoint, primary_height: f64, window: egui::Pos2) -> egui::Pos2 {
    egui::pos2(screen.x as f32 - window.x, (primary_height - screen.y) as f32 - window.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls the handler directly: the URLs it is handed and the queue they land in are what a
    /// test can reach without a window or double-click. One test, since `PENDING` is a shared,
    /// process-wide static.
    #[test]
    fn open_urls_queues_the_paths_it_is_given() {
        let urls = NSArray::from_retained_slice(&[
            NSURL::fileURLWithPath(&NSString::from_str("/tmp/one.tzx")),
            NSURL::fileURLWithPath(&NSString::from_str("/tmp/two.tap")),
        ]);
        assert_eq!(paths_of(&urls), vec![PathBuf::from("/tmp/one.tzx"), PathBuf::from("/tmp/two.tap")]);

        queue(paths_of(&urls));
        assert_eq!(take_pending().len(), 2, "the queue holds what arrived");
        assert!(take_pending().is_empty(), "and is drained by reading it");

        queue(paths_of(&NSArray::from_retained_slice(&[])));
        assert!(take_pending().is_empty(), "an empty event leaves nothing behind");
    }

    /// objc2 checks each `msg_send!` reply against the type it is read as, so a wrong AppKit
    /// signature fails here rather than in the app. With no window, there is nowhere to place
    /// the pointer.
    #[test]
    fn asking_appkit_where_the_pointer_is_answers_without_a_window() {
        let (at, _shift) = pointer_at_drop(&egui::Context::default());
        assert_eq!(at, None);
    }

    /// Calls the delegate method directly, as AppKit would on Cmd+Q. One test, since the
    /// flags are process-wide.
    #[test]
    fn a_quit_goes_through_only_when_nothing_is_unsaved() {
        let app = std::ptr::null_mut();
        let this: &AnyObject = unsafe { &*(&*NSString::from_str("x") as *const NSString).cast() };
        let quit = || unsafe { should_terminate(this, sel!(applicationShouldTerminate:), app) };

        set_may_quit(true);
        assert_eq!(quit(), 1, "NSTerminateNow");
        assert!(!take_quit_request(), "a quit that went through asks nothing");

        set_may_quit(false);
        assert_eq!(quit(), 0, "NSTerminateCancel");
        assert!(take_quit_request(), "the cancelled quit is left for the app to ask about");
        assert!(!take_quit_request(), "and is taken once");
        set_may_quit(true);
    }

    #[test]
    fn a_screen_point_lands_in_the_window_counted_from_its_top_left() {
        // A 1080-point screen, the window's content 40 points in and 100 down.
        let window = egui::pos2(40.0, 100.0);
        assert_eq!(window_point(NSPoint::new(40.0, 980.0), 1080.0, window), egui::pos2(0.0, 0.0));
        assert_eq!(window_point(NSPoint::new(540.0, 480.0), 1080.0, window), egui::pos2(500.0, 500.0));
    }
}
