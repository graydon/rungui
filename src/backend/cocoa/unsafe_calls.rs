//! Every `unsafe` of the Cocoa backend lives here (the rest of the backend is
//! `#![forbid(unsafe_code)]`). Each function is a thin typed wrapper; the justification is next to it.
//!
//! Recurring reasons:
//! * "weak reference": AppKit stores targets, delegates and data sources unretained, which
//!   objc2-app-kit models as `unsafe` setters. The only object ever passed is the process-global
//!   `RunguiTarget` held in the backend's state for the whole life of the app; before a native
//!   object is dropped its delegate/data source/target is reset by `Cocoa::destroy`.
//! * "unsafe by header translation": methods objc2-app-kit marks `unsafe` although they are
//!   ordinary main-thread messages for the argument types used here (nullable returns, selectors).
#![allow(unsafe_code)]

use super::imp::{RunguiSash, RunguiTarget, app_event, dispatch_action};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject, Sel};
use objc2::{AllocAnyThread, ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::*;
use std::sync::atomic::{AtomicPtr, Ordering};

// ---------------------------------------------------------------- RunguiApp

define_class!(
    /// NSApplication subclass: `-sendEvent:` sees every event, which is where right clicks are turned
    /// into `Event::ContextMenu`.
    #[unsafe(super(NSApplication))]
    #[thread_kind = MainThreadOnly]
    #[name = "RunguiApp"]
    pub(super) struct RunguiApp;

    impl RunguiApp {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, ev: &NSEvent) {
            // SAFETY: plain super call of the same selector with the same argument.
            let _: () = unsafe { msg_send![super(self), sendEvent: ev] };
            app_event(ev);
        }
    }
);

impl RunguiApp {
    objc2::extern_methods!(
        #[unsafe(method(sharedApplication))]
        fn shared_application_raw(mtm: MainThreadMarker) -> Retained<NSApplication>;
    );
}

/// `[RunguiApp sharedApplication]`: creates the application object as an instance of the subclass
/// (calling it through `NSApplication` would create a plain NSApplication).
pub(super) fn shared_application(mtm: MainThreadMarker) -> Retained<NSApplication> {
    RunguiApp::shared_application_raw(mtm)
}

#[cfg(rungui_gnustep)]
pub(super) fn set_app_delegate(app: &NSApplication, t: &RunguiTarget) {
    app.setDelegate(Some(ProtocolObject::from_ref(t)));
}

// ---------------------------------------------------------------- creation

/// `NSWindow` creation is `unsafe` because a window outside a window controller must have
/// `releasedWhenClosed` turned off (done right here), else it is over-released on close.
pub(super) fn new_window(
    mtm: MainThreadMarker,
    frame: NSRect,
    style: NSWindowStyleMask,
) -> Retained<NSWindow> {
    // SAFETY: standard designated initializer, called on the main thread (mtm).
    let w = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: Rust owns the window through `Retained`; AppKit must not release it on close.
    unsafe { w.setReleasedWhenClosed(false) };
    w
}

/// `NSMenuItem` initializer taking a selector (unsafe: arbitrary selector).
pub(super) fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    // SAFETY: `action` is nil or a selector the target implements (or a first-responder action).
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

/// An NSTimer scheduled in the current run loop, firing `action` on `target` (weak reference).
pub(super) fn scheduled_timer(
    secs: f64,
    target: &RunguiTarget,
    action: Sel,
    repeats: bool,
) -> Retained<NSTimer> {
    // SAFETY: `target` implements `action` (runguiTimer:) and lives for the whole process.
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            secs,
            target.as_ref(),
            action,
            None,
            repeats,
        )
    }
}

// ---------------------------------------------------------------- target / delegate wiring

pub(super) fn control_target_action(ctl: &NSControl, t: &RunguiTarget, action: Option<Sel>) {
    // SAFETY: weak reference to the process-global target; `action` is implemented by it.
    unsafe {
        ctl.setTarget(Some(t.as_ref()));
        ctl.setAction(action);
    }
}

pub(super) fn item_target_action(item: &NSMenuItem, t: &RunguiTarget, action: Option<Sel>) {
    // SAFETY: as in `control_target_action`.
    unsafe {
        item.setTarget(Some(t.as_ref()));
        item.setAction(action);
    }
}

pub(super) fn text_field_delegate(f: &NSTextField, t: Option<&RunguiTarget>) {
    // SAFETY: weak reference to the process-global target (reset to nil by `destroy`).
    unsafe { f.setDelegate(t.map(ProtocolObject::from_ref)) }
}

pub(super) fn text_view_delegate(tv: &NSTextView, t: Option<&RunguiTarget>) {
    tv.setDelegate(t.map(ProtocolObject::from_ref))
}

/// Table / list view: data source, delegate, target and double-click action.
pub(super) fn wire_table(tv: &NSTableView, t: Option<&RunguiTarget>) {
    // SAFETY: as above; `runguiDouble:` is implemented by RunguiTarget.
    unsafe {
        tv.setDataSource(t.map(ProtocolObject::from_ref));
        tv.setDelegate(t.map(ProtocolObject::from_ref));
        tv.setTarget(t.map(|t| t.as_ref()));
        tv.setDoubleAction(t.map(|_| objc2::sel!(runguiDouble:)));
    }
}

/// Outline view: NSOutlineView has its own data source/delegate setters.
pub(super) fn wire_outline(ov: &NSOutlineView, t: Option<&RunguiTarget>) {
    // SAFETY: as above.
    unsafe {
        ov.setDataSource(t.map(ProtocolObject::from_ref));
        ov.setDelegate(t.map(ProtocolObject::from_ref));
        ov.setTarget(t.map(|t| t.as_ref()));
        ov.setDoubleAction(t.map(|_| objc2::sel!(runguiDouble:)));
    }
}

pub(super) fn set_outline_column(ov: &NSOutlineView, col: &NSTableColumn) {
    // SAFETY: `col` was added to `ov` just before.
    unsafe { ov.setOutlineTableColumn(Some(col)) }
}

// ---------------------------------------------------------------- queries marked unsafe

pub(super) fn superview(v: &NSView) -> Option<Retained<NSView>> {
    // SAFETY: plain getter (nullable).
    unsafe { v.superview() }
}

pub(super) fn menu_of(item: &NSMenuItem) -> Option<Retained<NSMenu>> {
    // SAFETY: plain getter (nullable).
    unsafe { item.menu() }
}

pub(super) fn text_container(tv: &NSTextView) -> Option<Retained<NSTextContainer>> {
    // SAFETY: plain getter (nullable).
    unsafe { tv.textContainer() }
}

pub(super) fn deselect_all(tv: &NSTableView) {
    // SAFETY: the sender may be nil.
    unsafe { tv.deselectAll(None) }
}

pub(super) fn start_animation(p: &NSProgressIndicator) {
    // SAFETY: the sender may be nil.
    unsafe { p.startAnimation(None) }
}

pub(super) fn stop_animation(p: &NSProgressIndicator) {
    // SAFETY: the sender may be nil.
    unsafe { p.stopAnimation(None) }
}

pub(super) fn row_for_item(ov: &NSOutlineView, item: &NSNumber) -> isize {
    // SAFETY: `item` is one of the NSNumber items the data source handed out (or any object: AppKit returns -1).
    unsafe { ov.rowForItem(Some(item.as_ref())) }
}

pub(super) fn expand_item(ov: &NSOutlineView, item: &NSNumber) {
    // SAFETY: as in `row_for_item`.
    unsafe { ov.expandItem(Some(item.as_ref())) }
}

/// Send the event to the next responder (the sash only handles a few keys).
pub(super) fn forward_key_down(this: &RunguiSash, ev: &NSEvent) {
    // SAFETY: plain getter; `keyDown:` takes an NSEvent.
    if let Some(next) = unsafe { this.nextResponder() } {
        next.keyDown(ev);
    }
}

// ---------------------------------------------------------------- images

/// A 32 bit RGBA (non-premultiplied, 8 bits per sample) bitmap as an NSImage of `w` x `h` points.
pub(super) fn rgba_image(w: isize, h: isize, rgba: &[u8]) -> Option<Retained<NSImage>> {
    // SAFETY: nil planes make the rep allocate its own buffer of `bytesPerRow * h` bytes.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w,
            h,
            8,
            4,
            true,
            false,
            &NSString::from_str("NSCalibratedRGBColorSpace"),
            w * 4,
            32,
        )
    }?;
    let data = rep.bitmapData();
    if data.is_null() || rgba.len() != (w * h * 4) as usize {
        return None;
    }
    // SAFETY: the buffer is `w * 4 * h` bytes (checked above) and does not overlap `rgba`.
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, rgba.len()) };
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(w as f64, h as f64));
    image.addRepresentation(&rep);
    Some(image)
}

// ---------------------------------------------------------------- per-radio action selectors

/// Add an instance method `sel` (signature `v@:@`) to `RunguiTarget` that behaves like
/// `runguiAction:`. Radio buttons need distinct selectors: AppKit auto-groups radios that share
/// target+action in one superview, and the core owns exclusivity itself.
pub(super) fn add_action_selector(sel: Sel) {
    unsafe extern "C-unwind" fn radio_action(
        _this: *mut AnyObject,
        _cmd: Sel,
        sender: *mut AnyObject,
    ) {
        // SAFETY: AppKit passes the sending control (or nil); it outlives this call.
        if let Some(sender) = unsafe { sender.as_ref() } {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch_action(sender)));
        }
    }
    let cls: &AnyClass = RunguiTarget::class();
    // SAFETY: `radio_action` matches the type encoding `v@:@`; the class is registered; adding a
    // method that already exists just fails (returns NO), which is fine.
    unsafe {
        objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel,
            std::mem::transmute::<
                unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject),
                unsafe extern "C-unwind" fn(),
            >(radio_action),
            c"v@:@".as_ptr(),
        );
    }
}

/// Add `timer` to the run loop for one more mode (unsafe: the mode name must be a valid run-loop mode string).
pub(super) fn add_timer_mode(rl: &NSRunLoop, timer: &NSTimer, mode: &NSString) {
    // SAFETY: any string is accepted as a mode name by NSRunLoop; the timer is valid.
    unsafe { rl.addTimer_forMode(timer, mode) }
}

// ---------------------------------------------------------------- waking the main thread

/// The process-global target, for `perform_on_main` (callable from any thread).
static TARGET: AtomicPtr<RunguiTarget> = AtomicPtr::new(std::ptr::null_mut());

/// Remember `t` as the object `perform_on_main` messages. The target is leaked on purpose: it is the
/// weakly-referenced delegate of every native object and must outlive them all.
pub(super) fn publish_target(t: &Retained<RunguiTarget>) {
    std::mem::forget(t.clone());
    TARGET.store(Retained::as_ptr(t).cast_mut(), Ordering::SeqCst);
}

/// THREAD-SAFE: run `action` (`v@:@`, argument nil) of the published target on the main thread, in the
/// default, modal-panel and event-tracking run-loop modes (so it also fires during menus, modal
/// dialogs and live resizes).
pub(super) fn perform_on_main(action: Sel) {
    let p = TARGET.load(Ordering::SeqCst);
    if p.is_null() {
        return;
    }
    autoreleasepool(|_| {
        let modes = NSArray::from_retained_slice(&[
            NSString::from_str(MODE_DEFAULT),
            NSString::from_str("NSModalPanelRunLoopMode"),
            NSString::from_str("NSEventTrackingRunLoopMode"),
        ]);
        // SAFETY: `p` is the leaked, never-freed target (see `publish_target`), valid on every
        // thread as a plain object pointer; `performSelectorOnMainThread:...` is documented as
        // callable from any thread and `action` is implemented by RunguiTarget.
        unsafe {
            (*p).performSelectorOnMainThread_withObject_waitUntilDone_modes(
                action,
                None,
                false,
                Some(&modes),
            )
        }
    });
}

/// NSDefaultRunLoopMode is `kCFRunLoopDefaultMode` on Apple platforms.
#[cfg(rungui_gnustep)]
const MODE_DEFAULT: &str = "NSDefaultRunLoopMode";
#[cfg(not(rungui_gnustep))]
const MODE_DEFAULT: &str = "kCFRunLoopDefaultMode";
