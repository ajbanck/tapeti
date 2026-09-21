//! The one thing macOS asks of a document app that eframe does not surface:
//! `application:openURLs:`.
//!
//! Double-clicking a tape in the Finder does not put a path in `argv`. Launch
//! Services sends the application a `kAEOpenDocuments` Apple Event, which AppKit
//! turns into `application:openURLs:` on the application delegate — at launch,
//! and again every time afterwards. winit registers that delegate itself
//! (`WinitApplicationDelegate`) and implements only the two lifecycle methods it
//! needs, so the message goes nowhere and the double-click does nothing.
//!
//! So this module adds the method to whatever class winit registered, from an
//! observer of `NSApplicationWillFinishLaunchingNotification` — the last moment
//! before AppKit delivers the launch event, and the first at which the delegate
//! exists. `NSApplication` caches which selectors its delegate answers, so the
//! delegate is set again afterwards to make it look anew.
//!
//! Paths land in a queue rather than in the store, because they arrive on
//! AppKit's thread while a frame may be running; `App::frame` drains it, the way
//! the Tauri shell had the front end drain its `take_pending_files`.
//!
//! The second thing is where a file dragged in from the Finder is let go
//! (`pointer_at_drop`): winit reports the drop but not its position, and AppKit
//! sends the window no mouse moves while a drag is over it.

use std::ffi::CStr;
use std::path::PathBuf;
use std::ptr::NonNull;
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

/// Teach the application delegate `application:openURLs:`, unless it already
/// knows it (a later winit may implement it, and then it owns the message).
fn add_open_urls() {
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
    let selector = sel!(application:openURLs:);
    if class.responds_to(selector) {
        return;
    }
    // "v@:@@": returns void, takes the receiver, the selector and two objects.
    let types: &CStr = c"v@:@@";
    let imp: Imp = unsafe { std::mem::transmute(open_urls as unsafe extern "C-unwind" fn(_, _, _, _)) };
    let added =
        unsafe { ffi::class_addMethod((class as *const AnyClass).cast_mut(), selector, imp, types.as_ptr()) };
    if !added.as_bool() || delegate.is_null() {
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
    let block = block2::RcBlock::new(|_: NonNull<NSNotification>| add_open_urls());
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

/// Where the pointer is in the window, in points, and whether Shift is down —
/// asked of AppKit, because egui's idea of both is from before the drag began.
/// A drop carries no position (winit's `DroppedFile` is a path alone) and AppKit
/// sends no mouse moves during a drag session, so egui's pointer is wherever it
/// was last seen — outside the window, as often as not, where the Finder is.
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

/// A point in AppKit's screen space — points up from the bottom of the primary
/// screen — in a window whose top left sits at `window` in winit's, which counts
/// down from the top of that same screen.
fn window_point(screen: NSPoint, primary_height: f64, window: egui::Pos2) -> egui::Pos2 {
    egui::pos2(screen.x as f32 - window.x, (primary_height - screen.y) as f32 - window.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handler AppKit calls, called directly: the URLs it is handed and the
    /// queue they land in are the part of this that a test can reach without a
    /// window and a double-click. One test, because the queue is process-wide.
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

    /// Every AppKit message the drop asks, sent for real: objc2 checks each reply
    /// against the type it is read as, so a wrong signature fails here rather
    /// than in the app. With no window there is nowhere to place the pointer.
    #[test]
    fn asking_appkit_where_the_pointer_is_answers_without_a_window() {
        let (at, _shift) = pointer_at_drop(&egui::Context::default());
        assert_eq!(at, None);
    }

    #[test]
    fn a_screen_point_lands_in_the_window_counted_from_its_top_left() {
        // A 1080-point screen, the window's content 40 points in and 100 down.
        let window = egui::pos2(40.0, 100.0);
        assert_eq!(window_point(NSPoint::new(40.0, 980.0), 1080.0, window), egui::pos2(0.0, 0.0));
        assert_eq!(window_point(NSPoint::new(540.0, 480.0), 1080.0, window), egui::pos2(500.0, 500.0));
    }
}
