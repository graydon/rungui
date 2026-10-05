//! Cocoa/AppKit backend (macOS; also GNUstep when emulated on linux, cfg `rungui_gnustep`).
//!
//! Pure Rust over the Objective-C runtime C API (hand-written externs, no binding crates).
//! * Messaging goes through the `send!` macro: `objc_msgSend` on macOS (plus `objc_msgSend_stret`
//!   for `NSRect` returns on x86_64), `objc_msg_lookup` on GNUstep's GCC libobjc.
//! * One runtime-defined class `RunguiTarget` (a single shared instance) is target, delegate and
//!   data source for every native object; it maps the sender back to a [`WidgetId`] through a
//!   pointer -> id table. `RunguiFlipView` is a flipped `NSView` used for every container so that
//!   all coordinates are top-left based, as the core expects.
//! * Containers: Window (flipped content view), GroupBox (NSBox + flipped content view),
//!   Tabs/Page (NSTabView + flipped page views). Layout comes from the core (absolute frames).
//! * Splitter sash: `RunguiSash`, an NSView subclass (mouseDown/Dragged/Up, cursor rects, a
//!   one point separator line). Dragging emits `SashDragged(bounds-at-press + pointer delta)`.
//!   It accepts first responder (a click or the key view loop focuses it, the handle is then
//!   filled with the focus colour) and turns arrows / Shift+arrows / Home / End into `SashKey`.
//!   Also native here: `Prop::Monospace` (NSFont), `Wrap` (text container tracking), window
//!   `Position` (`setFrameTopLeftPoint:`, flipped against the primary screen), `MinSize`
//!   (`setContentMinSize:`) and `Event::Moved` (`windowDidMove:`).
//! * GNUstep: `run` is a hand-rolled event loop with a 100 ms timeout (`-stop:` is only noticed
//!   after the next X event there), popups go through `popUpContextMenu:withEvent:forView:`.
//! * Accessibility: AppKit controls are natively accessible; `a11y_changed` pushes the core's
//!   computed names/descriptions into `accessibilityLabel`/`accessibilityHelp` of the native views.
//!   AppKit's own accessibility already covers every native control (the sash gets its role/value
//!   only through the label and help text).
//!
//! Unverifiable without macOS (checked by reading + `cargo check` for both apple-darwin targets):
//! `objc_msgSend_stret` for NSRect returns on x86_64 only (aarch64 returns the HFA in registers);
//! every message is sent through a transmute to its exact function type (no variadic casts);
//! BOOL is read as `u8` (signed char on x86_64, bool on arm64); NSInteger/NSUInteger are
//! isize/usize; timers use `kCFRunLoopCommonModes` (fire during menu tracking and modals).

#![allow(clippy::missing_safety_doc)]

use super::*;
use crate::core;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

// ---------------------------------------------------------------- runtime bindings

type Id = *mut c_void;
type Sel = *mut c_void;
const NIL: Id = null_mut();

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct NSPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct NSSize {
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct NSRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

// Layout checks (64-bit only; CGFloat == f64).
const _: () = {
    assert!(std::mem::size_of::<NSPoint>() == 16);
    assert!(std::mem::size_of::<NSSize>() == 16);
    assert!(std::mem::size_of::<NSRect>() == 32);
};

/// Run-loop mode names. NSDefaultRunLoopMode is `kCFRunLoopDefaultMode` on Apple platforms.
#[cfg(rungui_gnustep)]
const MODE_DEFAULT: &str = "NSDefaultRunLoopMode";
#[cfg(not(rungui_gnustep))]
const MODE_DEFAULT: &str = "kCFRunLoopDefaultMode";

unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn class_getMethodImplementation(cls: Id, sel: Sel) -> *const c_void;
    fn class_addMethod(cls: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> u8;
    fn NSRectFill(r: NSRect);
    #[cfg(rungui_gnustep)]
    fn objc_msg_lookup(recv: Id, sel: Sel) -> *const c_void;
    #[cfg(not(rungui_gnustep))]
    fn objc_msgSend();
    #[cfg(all(not(rungui_gnustep), target_arch = "x86_64"))]
    fn objc_msgSend_stret();
}

#[cfg(rungui_gnustep)]
unsafe fn imp_for(recv: Id, sel: Sel) -> *const c_void {
    unsafe { objc_msg_lookup(recv, sel) }
}
#[cfg(not(rungui_gnustep))]
unsafe fn imp_for(_recv: Id, _sel: Sel) -> *const c_void {
    objc_msgSend as *const c_void
}

fn sel_c(p: *const c_char) -> Sel {
    unsafe { sel_registerName(p) }
}

macro_rules! sel {
    ($s:expr) => {
        sel_c(concat!($s, "\0").as_ptr() as *const c_char)
    };
}

/// Typed message send: `send!(RetType, receiver, "selector:", ArgType: value, ...)`.
macro_rules! send {
    ($ret:ty, $recv:expr, $sel:literal $(, $t:ty: $a:expr)* $(,)?) => {{
        let r: Id = $recv;
        let s: Sel = sel!($sel);
        unsafe {
            let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
                std::mem::transmute(imp_for(r, s));
            f(r, s $(, $a)*)
        }
    }};
}
/// Message returning an object.
macro_rules! idm {
    ($recv:expr, $sel:literal $(, $t:ty: $a:expr)* $(,)?) => { send!(Id, $recv, $sel $(, $t: $a)*) };
}
/// Message returning nothing.
macro_rules! vm {
    ($recv:expr, $sel:literal $(, $t:ty: $a:expr)* $(,)?) => { send!((), $recv, $sel $(, $t: $a)*) };
}
/// Message returning BOOL.
macro_rules! bm {
    ($recv:expr, $sel:literal $(, $t:ty: $a:expr)* $(,)?) => { (send!(u8, $recv, $sel $(, $t: $a)*) != 0) };
}

#[cfg(all(not(rungui_gnustep), target_arch = "x86_64"))]
fn msg_rect(recv: Id, sel: Sel) -> NSRect {
    let mut out = NSRect::default();
    if !recv.is_null() {
        unsafe {
            let f: unsafe extern "C" fn(*mut NSRect, Id, Sel) =
                std::mem::transmute(objc_msgSend_stret as *const c_void);
            f(&mut out, recv, sel);
        }
    }
    out
}
#[cfg(not(all(not(rungui_gnustep), target_arch = "x86_64")))]
fn msg_rect(recv: Id, sel: Sel) -> NSRect {
    if recv.is_null() {
        return NSRect::default();
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> NSRect = std::mem::transmute(imp_for(recv, sel));
        f(recv, sel)
    }
}
macro_rules! rect_of {
    ($recv:expr, $sel:literal) => {
        msg_rect($recv, sel!($sel))
    };
}

fn cls(name: &str) -> Id {
    match CString::new(name) {
        Ok(c) => unsafe { objc_getClass(c.as_ptr()) },
        Err(_) => NIL,
    }
}
fn sel_named(name: &str) -> Sel {
    match CString::new(name) {
        Ok(c) => unsafe { sel_registerName(c.as_ptr()) },
        Err(_) => null_mut(),
    }
}
/// Autoreleased NSString from UTF-8 (interior NULs are dropped).
fn ns(s: &str) -> Id {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    idm!(cls("NSString"), "stringWithUTF8String:", *const c_char: c.as_ptr())
}
fn from_ns(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p = send!(*const c_char, s, "UTF8String");
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect { x, y, w, h }
}
fn rect_from(r: Rect) -> NSRect {
    rect(r.x as f64, r.y as f64, r.w.max(0) as f64, r.h.max(0) as f64)
}
fn release(o: Id) {
    if !o.is_null() {
        vm!(o, "release");
    }
}
fn autorelease(o: Id) {
    if !o.is_null() {
        idm!(o, "autorelease");
    }
}
fn responds(o: Id, s: &str) -> bool {
    !o.is_null() && bm!(o, "respondsToSelector:", Sel: sel_named(s))
}
fn alloc_init(class: &str) -> Id {
    idm!(idm!(cls(class), "alloc"), "init")
}
fn view_new(class: &str, f: NSRect) -> Id {
    idm!(idm!(cls(class), "alloc"), "initWithFrame:", NSRect: f)
}
fn flip_view() -> Id {
    view_new("RunguiFlipView", NSRect::default())
}
fn b(v: bool) -> u8 {
    v as u8
}

// ---------------------------------------------------------------- state

#[derive(Copy, Clone)]
struct Entry {
    kind: Kind,
    /// Main native object (NSWindow, NSView, NSMenu, NSMenuItem).
    obj: Id,
    /// Container view that receives children (Window, GroupBox, Page).
    cont: Id,
    /// Inner object (NSTextView, NSTableView, spin text field, NSTabViewItem, Menu's NSMenuItem).
    aux: Id,
    aux2: Id,
}

/// Model of an NSOutlineView: items are retained NSNumbers holding the core's node id.
#[derive(Default)]
struct TreeModel {
    nodes: Vec<TNode>,
    roots: Vec<usize>,
    by_id: HashMap<u64, usize>,
}
struct TNode {
    text: String,
    expanded: bool,
    has_children: bool,
    children: Vec<usize>,
    obj: Id,
}

#[derive(Default)]
struct State {
    rows: HashMap<WidgetId, Vec<Vec<String>>>,
    trees: HashMap<WidgetId, TreeModel>,
    ents: HashMap<WidgetId, Entry>,
    /// native pointer -> widget
    rev: HashMap<usize, WidgetId>,
    items: HashMap<WidgetId, Vec<String>>,
    range: HashMap<WidgetId, (f64, f64, f64)>,
    imgsz: HashMap<WidgetId, Size>,
    readonly: HashSet<WidgetId>,
    disabled: HashSet<WidgetId>,
    /// window -> its NSMenu bar
    menubars: HashMap<WidgetId, Id>,
    /// menubar -> built-in Edit menu item (removed when the app defines its own "Edit" menu)
    std_edit: HashMap<WidgetId, Id>,
    shown: HashSet<WidgetId>,
    timers: HashMap<u64, Id>,
    trev: HashMap<usize, u64>,
    /// sash -> orientation (Horizontal splitter = vertical sash that moves along x)
    sash_orient: HashMap<WidgetId, Orientation>,
    /// active sash drag: (sash, pointer coordinate at press along the drag axis, sash position at press)
    drag: Option<(WidgetId, f64, i32)>,
    /// windows the app positioned explicitly: not centered on first show
    placed: HashSet<WidgetId>,
    /// GNUstep: the content view of the last page removed from a tab view (key: the NSTabView).
    /// GNUstep's NSTabView still points at it after the removal and messages it when the next page
    /// is added, so it must outlive that (see `Kind::Page` in `create` and `destroy`).
    stale_page_views: HashMap<usize, Id>,
    /// GNUstep only: windows made non-resizable by pinning their min and max size together
    /// (its `setStyleMask:` raises on a live window), and the minimum size each was given.
    fixed: HashSet<WidgetId>,
    min_size: HashMap<WidgetId, NSSize>,
    target: Id,
    tclass: Id,
    app: Id,
    app_name: String,
}
impl Default for Entry {
    fn default() -> Self {
        Entry {
            kind: Kind::Spacer,
            obj: NIL,
            cont: NIL,
            aux: NIL,
            aux2: NIL,
        }
    }
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State { target: NIL, tclass: NIL, app: NIL, ..Default::default() });
    static QUIET: Cell<u32> = const { Cell::new(0) };
}
static TARGET: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
/// GNUstep only: set by `quit` to leave the hand-rolled event loop in `run`.
#[cfg(rungui_gnustep)]
static QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn st<R>(f: impl FnOnce(&mut State) -> R) -> R {
    S.with(|s| f(&mut s.borrow_mut()))
}
fn ent(id: WidgetId) -> Option<Entry> {
    st(|s| s.ents.get(&id).copied())
}
fn lookup(p: Id) -> Option<(WidgetId, Entry)> {
    if p.is_null() {
        return None;
    }
    st(|s| {
        let id = *s.rev.get(&(p as usize))?;
        Some((id, *s.ents.get(&id)?))
    })
}
/// While alive, native notifications caused by our own property changes are ignored (rule 2).
struct Quiet;
impl Quiet {
    fn new() -> Quiet {
        QUIET.with(|q| q.set(q.get() + 1));
        Quiet
    }
}
impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.with(|q| q.set(q.get().saturating_sub(1)));
    }
}
fn quiet() -> bool {
    QUIET.try_with(|q| q.get() > 0).unwrap_or(true)
}
/// Run a native callback body: not while we are mutating natives ourselves, never unwinding.
fn guarded(f: impl FnOnce()) {
    if quiet() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(f));
}

// ---------------------------------------------------------------- the target class

extern "C" fn on_wake(_t: Id, _c: Sel, _a: Id) {
    let _ = catch_unwind(core::drain_posted);
}

extern "C" fn on_quit_item(_t: Id, _c: Sel, _a: Id) {
    let _ = catch_unwind(<Cocoa as Backend>::quit);
}

extern "C" fn on_timer(_t: Id, _c: Sel, timer: Id) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(tok) = st(|s| s.trev.get(&(timer as usize)).copied()) {
            core::timer_fired(tok);
        }
    }));
}

fn parse_spin(text: &str, id: WidgetId) -> Option<f64> {
    let v: f64 = text.trim().parse().ok()?;
    let (lo, hi, _) = st(|s| s.range.get(&id).copied()).unwrap_or((0.0, 100.0, 1.0));
    Some(v.clamp(lo.min(hi), hi.max(lo)))
}

fn set_spin(e: &Entry, v: f64) {
    let _q = Quiet::new();
    vm!(e.aux2, "setDoubleValue:", f64: v);
    vm!(e.aux, "setStringValue:", Id: ns(&format!("{v}")));
}

extern "C" fn on_action(_t: Id, _c: Sel, sender: Id) {
    guarded(|| {
        let Some((id, e)) = lookup(sender) else {
            return;
        };
        match e.kind {
            Kind::Button | Kind::MenuItem => core::event(id, Event::Click),
            Kind::CheckBox | Kind::RadioButton => {
                let on = send!(isize, sender, "state") == 1;
                core::event(id, Event::Toggled(on));
            }
            Kind::CheckMenuItem => {
                let on = send!(isize, sender, "state") != 1;
                {
                    let _q = Quiet::new();
                    vm!(sender, "setState:", isize: on as isize);
                }
                core::event(id, Event::Toggled(on));
            }
            Kind::Slider => {
                let mut v = send!(f64, sender, "doubleValue");
                let (lo, _hi, step) =
                    st(|s| s.range.get(&id).copied()).unwrap_or((0.0, 100.0, 0.0));
                if step > 0.0 {
                    let snapped = lo + ((v - lo) / step).round() * step;
                    if snapped != v {
                        v = snapped;
                        let _q = Quiet::new();
                        vm!(sender, "setDoubleValue:", f64: v);
                    }
                }
                core::event(id, Event::Value(v));
            }
            Kind::ComboBox => {
                let i = send!(isize, sender, "indexOfSelectedItem");
                core::event(id, Event::Selected((i >= 0).then_some(i as usize)));
            }
            Kind::SpinBox => {
                let v = if sender == e.aux2 {
                    send!(f64, sender, "doubleValue")
                } else {
                    match parse_spin(&from_ns(idm!(sender, "stringValue")), id) {
                        Some(v) => v,
                        None => {
                            let cur = send!(f64, e.aux2, "doubleValue");
                            set_spin(&e, cur);
                            return;
                        }
                    }
                };
                set_spin(&e, v);
                core::event(id, Event::Value(v));
            }
            _ => {}
        }
    });
}

/// Table double click.
extern "C" fn on_double(_t: Id, _c: Sel, sender: Id) {
    guarded(|| {
        let Some((id, e)) = lookup(sender) else {
            return;
        };
        let row = send!(isize, sender, "clickedRow");
        if row < 0 {
            return;
        }
        match e.kind {
            Kind::ListBox | Kind::Table => core::event(id, Event::Activated(row as usize)),
            Kind::Tree => {
                if let Some(n) = outline_item_node(sender, row) {
                    core::event(id, Event::TreeActivated(n));
                }
            }
            _ => {}
        }
    });
}

fn note_object(note: Id) -> Id {
    idm!(note, "object")
}

/// controlTextDidChange: (NSTextField) and textDidChange: (NSTextView)
extern "C" fn on_text_changed(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        let o = note_object(note);
        let Some((id, e)) = lookup(o) else { return };
        match e.kind {
            Kind::TextInput | Kind::PasswordInput => {
                core::event(id, Event::Text(from_ns(idm!(o, "stringValue"))))
            }
            Kind::TextArea => core::event(id, Event::Text(from_ns(idm!(o, "string")))),
            _ => {}
        }
    });
}

extern "C" fn on_begin_edit(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        if let Some((id, _)) = lookup(note_object(note)) {
            core::event(id, Event::Focus(true));
        }
    });
}

extern "C" fn on_end_edit(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        let o = note_object(note);
        let Some((id, e)) = lookup(o) else { return };
        if e.kind == Kind::SpinBox && o == e.aux {
            if let Some(v) = parse_spin(&from_ns(idm!(o, "stringValue")), id) {
                if v != send!(f64, e.aux2, "doubleValue") {
                    set_spin(&e, v);
                    core::event(id, Event::Value(v));
                }
            }
        }
        core::event(id, Event::Focus(false));
    });
}

extern "C" fn on_should_close(_t: Id, _c: Sel, win: Id) -> u8 {
    guarded(|| {
        if let Some((id, _)) = lookup(win) {
            core::close_requested(id);
        }
    });
    0
}

fn client_size(e: &Entry) -> NSSize {
    let r = rect_of!(e.cont, "frame");
    NSSize { w: r.w, h: r.h }
}

extern "C" fn on_resized(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        let Some((id, e)) = lookup(note_object(note)) else {
            return;
        };
        let s = client_size(&e);
        core::event(
            id,
            Event::Resized {
                w: s.w.round() as i32,
                h: s.h.round() as i32,
            },
        );
    });
}

fn install_menubar(win: WidgetId) {
    if let Some(m) = st(|s| s.menubars.get(&win).copied()) {
        let app = st(|s| s.app);
        vm!(app, "setMainMenu:", Id: m);
    }
}

extern "C" fn on_moved(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        let win = note_object(note);
        let Some((id, e)) = lookup(win) else { return };
        if e.kind != Kind::Window {
            return;
        }
        let f = rect_of!(win, "frame");
        // Cocoa screen coordinates are bottom-left based; the API reports the outer top-left.
        let y = primary_screen_height() - (f.y + f.h);
        core::event(
            id,
            Event::Moved {
                x: f.x.round() as i32,
                y: y.round() as i32,
            },
        );
    });
}

/// Height of the primary screen (the one holding the menu bar, origin of the screen coordinates).
fn primary_screen_height() -> f64 {
    let screens = idm!(cls("NSScreen"), "screens");
    if screens.is_null() || send!(usize, screens, "count") == 0 {
        return 0.0;
    }
    rect_of!(idm!(screens, "objectAtIndex:", usize: 0), "frame").h
}

extern "C" fn on_became_key(_t: Id, _c: Sel, note: Id) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some((id, _)) = lookup(note_object(note)) {
            install_menubar(id);
            if !quiet() {
                core::event(id, Event::Focus(true));
            }
        }
    }));
}

extern "C" fn on_tab_selected(_t: Id, _c: Sel, tv: Id, item: Id) {
    guarded(|| {
        let Some((id, _)) = lookup(tv) else { return };
        let i = send!(isize, tv, "indexOfTabViewItem:", Id: item);
        if i >= 0 {
            core::event(id, Event::Selected(Some(i as usize)));
        }
    });
}

extern "C" fn tv_rows(_t: Id, _c: Sel, tv: Id) -> isize {
    let Some((id, e)) = lookup(tv) else { return 0 };
    st(|s| match e.kind {
        Kind::Table => s.rows.get(&id).map_or(0, |r| r.len()),
        _ => s.items.get(&id).map_or(0, |r| r.len()),
    }) as isize
}

fn column_index(col: Id) -> usize {
    from_ns(idm!(col, "identifier")).parse().unwrap_or(0)
}

extern "C" fn tv_value(_t: Id, _c: Sel, tv: Id, col: Id, row: isize) -> Id {
    let Some((id, e)) = lookup(tv) else {
        return ns("");
    };
    let row = row.max(0) as usize;
    let text = st(|s| match e.kind {
        Kind::Table => s
            .rows
            .get(&id)
            .and_then(|r| r.get(row))
            .and_then(|r| r.get(column_index(col)))
            .cloned(),
        _ => s.items.get(&id).and_then(|v| v.get(row)).cloned(),
    });
    ns(&text.unwrap_or_default())
}

extern "C" fn tv_column_clicked(_t: Id, _c: Sel, tv: Id, col: Id) {
    guarded(|| {
        if let Some((id, e)) = lookup(tv) {
            if e.kind == Kind::Table {
                core::event(id, Event::ColumnClicked(column_index(col)));
            }
        }
    });
}

fn tree_node_of(item: Id) -> u64 {
    send!(u64, item, "unsignedLongLongValue")
}

extern "C" fn ov_count(_t: Id, _c: Sel, ov: Id, item: Id) -> isize {
    let Some((id, _)) = lookup(ov) else { return 0 };
    let key = (!item.is_null()).then(|| tree_node_of(item));
    st(|s| {
        let Some(m) = s.trees.get(&id) else { return 0 };
        match key {
            None => m.roots.len(),
            Some(k) => m
                .by_id
                .get(&k)
                .and_then(|i| m.nodes.get(*i))
                .map_or(0, |n| n.children.len()),
        }
    }) as isize
}

extern "C" fn ov_child(_t: Id, _c: Sel, ov: Id, index: isize, item: Id) -> Id {
    let Some((id, _)) = lookup(ov) else {
        return NIL;
    };
    let key = (!item.is_null()).then(|| tree_node_of(item));
    st(|s| {
        let m = s.trees.get(&id)?;
        let list = match key {
            None => &m.roots,
            Some(k) => &m.nodes.get(*m.by_id.get(&k)?)?.children,
        };
        Some(m.nodes.get(*list.get(index.max(0) as usize)?)?.obj)
    })
    .unwrap_or(NIL)
}

extern "C" fn ov_expandable(_t: Id, _c: Sel, ov: Id, item: Id) -> u8 {
    let Some((id, _)) = lookup(ov) else { return 0 };
    let k = tree_node_of(item);
    st(|s| {
        s.trees
            .get(&id)
            .and_then(|m| {
                m.by_id
                    .get(&k)
                    .and_then(|i| m.nodes.get(*i))
                    .map(|n| n.has_children)
            })
            .unwrap_or(false)
    }) as u8
}

extern "C" fn ov_value(_t: Id, _c: Sel, ov: Id, _col: Id, item: Id) -> Id {
    let Some((id, _)) = lookup(ov) else {
        return ns("");
    };
    let k = tree_node_of(item);
    let t = st(|s| {
        s.trees.get(&id).and_then(|m| {
            m.by_id
                .get(&k)
                .and_then(|i| m.nodes.get(*i))
                .map(|n| n.text.clone())
        })
    });
    ns(&t.unwrap_or_default())
}

fn ov_expansion(note: Id, open: bool) {
    guarded(|| {
        let Some((id, _)) = lookup(note_object(note)) else {
            return;
        };
        let info = idm!(note, "userInfo");
        let item = idm!(info, "objectForKey:", Id: ns("NSObject"));
        if item.is_null() {
            return;
        }
        let node = tree_node_of(item);
        // Deferred: the core may rebuild the whole tree in response, which must not happen
        // inside AppKit's expansion bookkeeping.
        core::post(move || core::event(id, Event::TreeExpanded(node, open)));
        st(|s| {
            if let Some(m) = s.trees.get_mut(&id) {
                if let Some(n) = m.by_id.get(&node).copied().and_then(|i| m.nodes.get_mut(i)) {
                    n.expanded = open;
                }
            }
        });
    });
}
extern "C" fn ov_did_expand(_t: Id, _c: Sel, note: Id) {
    ov_expansion(note, true);
}
extern "C" fn ov_did_collapse(_t: Id, _c: Sel, note: Id) {
    ov_expansion(note, false);
}

fn outline_item_node(ov: Id, row: isize) -> Option<u64> {
    if row < 0 {
        return None;
    }
    let item = idm!(ov, "itemAtRow:", isize: row);
    (!item.is_null()).then(|| tree_node_of(item))
}

extern "C" fn ov_selection(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        let o = note_object(note);
        let Some((id, _)) = lookup(o) else { return };
        let row = send!(isize, o, "selectedRow");
        core::event(id, Event::TreeSelected(outline_item_node(o, row)));
    });
}

/// NSApplication subclass hook: `-sendEvent:` sees every right click.
extern "C" fn app_send_event(this: Id, cmd: Sel, ev: Id) {
    unsafe {
        let sup = class_getMethodImplementation(cls("NSApplication"), cmd);
        if !sup.is_null() {
            let f: unsafe extern "C" fn(Id, Sel, Id) = std::mem::transmute(sup);
            f(this, cmd, ev);
        }
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if ev.is_null() {
            return;
        }
        let ty = send!(usize, ev, "type");
        let flags = send!(usize, ev, "modifierFlags");
        // 3 = NSRightMouseDown; control-click is the Mac way to right click.
        if !(ty == 3 || (ty == 1 && flags & (1 << 18) != 0)) {
            return;
        }
        let win = idm!(ev, "window");
        let Some((wid, we)) = lookup(win) else { return };
        let loc = send!(NSPoint, ev, "locationInWindow");
        let h = client_size(&we).h;
        let mut v = idm!(we.cont, "hitTest:", NSPoint: loc);
        let mut target = wid;
        while !v.is_null() {
            if let Some((id, _)) = lookup(v) {
                target = id;
                break;
            }
            v = idm!(v, "superview");
        }
        core::event(
            target,
            Event::ContextMenu {
                x: loc.x.round() as i32,
                y: (h - loc.y).round() as i32,
            },
        );
    }));
}

extern "C" fn tv_selection(_t: Id, _c: Sel, note: Id) {
    guarded(|| {
        let o = note_object(note);
        let Some((id, _)) = lookup(o) else { return };
        let row = send!(isize, o, "selectedRow");
        core::event(id, Event::Selected((row >= 0).then_some(row as usize)));
    });
}

extern "C" fn is_flipped(_t: Id, _c: Sel) -> u8 {
    1
}

/// NSApplication delegate hook: GNUstep treats command-line paths as files to open and shows an
/// "Alert / No information" panel when the delegate does not claim them. The app handles its own
/// arguments, so claim them and do nothing.
#[cfg(rungui_gnustep)]
extern "C" fn app_open_file(_t: Id, _c: Sel, _app: Id, _file: Id) -> u8 {
    1
}

// ---- RunguiSash: the Splitter drag handle (NSView subclass)

fn sash_axis(id: WidgetId) -> Orientation {
    st(|s| s.sash_orient.get(&id).copied()).unwrap_or_default()
}
/// Pointer coordinate along the sash's drag axis, in window coordinates but oriented like the
/// (flipped) parent: x grows right, y grows down.
fn sash_pointer(ev: Id, o: Orientation) -> f64 {
    let p = send!(NSPoint, ev, "locationInWindow");
    match o {
        Orientation::Horizontal => p.x,
        Orientation::Vertical => -p.y,
    }
}

extern "C" fn sash_mouse_down(this: Id, _c: Sel, ev: Id) {
    guarded(|| {
        let Some((id, _)) = lookup(this) else { return };
        if st(|s| s.disabled.contains(&id)) {
            return;
        }
        let o = sash_axis(id);
        let f = rect_of!(this, "frame");
        let start = match o {
            Orientation::Horizontal => f.x,
            Orientation::Vertical => f.y,
        };
        let ptr = sash_pointer(ev, o);
        st(|s| s.drag = Some((id, ptr, start.round() as i32)));
        // a click focuses the sash so the arrow keys work right after
        let w = idm!(this, "window");
        if !w.is_null() {
            vm!(w, "makeFirstResponder:", Id: this);
        }
    });
}

fn sash_has_focus(this: Id) -> bool {
    let w = idm!(this, "window");
    !w.is_null() && idm!(w, "firstResponder") == this
}

extern "C" fn sash_focus_changed(this: Id, _c: Sel) -> u8 {
    // the focus indicator comes or goes; the redraw happens after the window has updated its first responder
    let _ = catch_unwind(AssertUnwindSafe(|| vm!(this, "setNeedsDisplay:", u8: 1)));
    1
}

/// Arrow keys along the sash's axis (Shift = large step), Home and End; any other key goes on up the
/// responder chain (Tab moves the key view, shortcuts reach the menus).
extern "C" fn sash_key_down(this: Id, _c: Sel, ev: Id) {
    let key = catch_unwind(AssertUnwindSafe(|| {
        let (id, _) = lookup(this)?;
        if ev.is_null() || st(|s| s.disabled.contains(&id)) {
            return None;
        }
        let flags = send!(usize, ev, "modifierFlags");
        // Control, Option, Command: not ours
        if flags & ((1 << 18) | (1 << 19) | (1 << 20)) != 0 {
            return None;
        }
        let big = flags & (1 << 17) != 0;
        let chars = from_ns(idm!(ev, "charactersIgnoringModifiers"));
        // NSUp/Down/Left/RightArrowFunctionKey, NSHomeFunctionKey, NSEndFunctionKey
        let (prev, next) = match sash_axis(id) {
            Orientation::Horizontal => ('\u{F702}', '\u{F703}'),
            Orientation::Vertical => ('\u{F700}', '\u{F701}'),
        };
        let c = chars.chars().next()?;
        let key = match c {
            c if c == prev => {
                if big {
                    SashKey::PrevLarge
                } else {
                    SashKey::Prev
                }
            }
            c if c == next => {
                if big {
                    SashKey::NextLarge
                } else {
                    SashKey::Next
                }
            }
            '\u{F729}' => SashKey::Min,
            '\u{F72B}' => SashKey::Max,
            _ => return None,
        };
        Some((id, key))
    }))
    .ok()
    .flatten();
    match key {
        Some((id, key)) => guarded(|| core::event(id, Event::SashKey(key))),
        None => {
            let next = idm!(this, "nextResponder");
            if !next.is_null() {
                vm!(next, "keyDown:", Id: ev);
            }
        }
    }
}

extern "C" fn sash_mouse_dragged(this: Id, _c: Sel, ev: Id) {
    guarded(|| {
        let Some((id, _)) = lookup(this) else { return };
        let Some((did, p0, pos0)) = st(|s| s.drag) else {
            return;
        };
        if did != id {
            return;
        }
        let delta = sash_pointer(ev, sash_axis(id)) - p0;
        core::event(
            id,
            Event::SashDragged(pos0.saturating_add(delta.round() as i32)),
        );
    });
}

extern "C" fn sash_mouse_up(_t: Id, _c: Sel, _ev: Id) {
    let _ = catch_unwind(AssertUnwindSafe(|| st(|s| s.drag = None)));
}

extern "C" fn sash_reset_cursor_rects(this: Id, _c: Sel) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some((id, _)) = lookup(this) else { return };
        let c = match sash_axis(id) {
            Orientation::Horizontal => idm!(cls("NSCursor"), "resizeLeftRightCursor"),
            Orientation::Vertical => idm!(cls("NSCursor"), "resizeUpDownCursor"),
        };
        if !c.is_null() {
            vm!(this, "addCursorRect:cursor:", NSRect: rect_of!(this, "bounds"), Id: c);
        }
    }));
}

extern "C" fn sash_draw(this: Id, _c: Sel, _dirty: NSRect) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let b = rect_of!(this, "bounds");
        let Some((id, _)) = lookup(this) else { return };
        // keyboard focus: the whole handle in the system's focus colour
        if sash_has_focus(this) {
            let focus = idm!(cls("NSColor"), "keyboardFocusIndicatorColor");
            if !focus.is_null() {
                vm!(focus, "set");
                unsafe { NSRectFill(b) };
            }
        }
        // A one point separator line centred in the handle.
        let line = match sash_axis(id) {
            Orientation::Horizontal => rect(b.x + (b.w / 2.0).floor(), b.y, 1.0, b.h),
            Orientation::Vertical => rect(b.x, b.y + (b.h / 2.0).floor(), b.w, 1.0),
        };
        let color = idm!(cls("NSColor"), "gridColor");
        if !color.is_null() {
            vm!(color, "set");
            unsafe { NSRectFill(line) };
        }
    }));
}

extern "C" fn yes(_t: Id, _c: Sel, _a: Id) -> u8 {
    1
}
extern "C" fn no_flag(_t: Id, _c: Sel) -> u8 {
    0
}
extern "C" fn yes_flag(_t: Id, _c: Sel) -> u8 {
    1
}

fn define_classes() -> Result<(Id, Id)> {
    unsafe {
        let nsobject = cls("NSObject");
        let nsview = cls("NSView");
        if nsobject.is_null() || nsview.is_null() {
            return Err(Error::Backend(
                "Objective-C classes NSObject/NSView not found".into(),
            ));
        }
        // A second init() in the same process reuses the classes registered by the first.
        let known = cls("RunguiTarget");
        if !known.is_null() && !cls("RunguiFlipView").is_null() && !cls("RunguiSash").is_null() {
            return Ok((known, alloc_init_class(known)));
        }
        let flip = objc_allocateClassPair(nsview, c"RunguiFlipView".as_ptr(), 0);
        let t = objc_allocateClassPair(nsobject, c"RunguiTarget".as_ptr(), 0);
        if flip.is_null() || t.is_null() {
            return Err(Error::Backend("cannot define Objective-C classes".into()));
        }
        let add = |c: Id, s: &str, f: *const c_void, ty: &CStr| {
            class_addMethod(c, sel_named(s), f, ty.as_ptr());
        };
        add(flip, "isFlipped", is_flipped as *const c_void, c"c@:");
        objc_registerClassPair(flip);
        let sash = objc_allocateClassPair(nsview, c"RunguiSash".as_ptr(), 0);
        if !sash.is_null() {
            add(
                sash,
                "mouseDown:",
                sash_mouse_down as *const c_void,
                c"v@:@",
            );
            add(
                sash,
                "mouseDragged:",
                sash_mouse_dragged as *const c_void,
                c"v@:@",
            );
            add(sash, "mouseUp:", sash_mouse_up as *const c_void, c"v@:@");
            add(
                sash,
                "resetCursorRects",
                sash_reset_cursor_rects as *const c_void,
                c"v@:",
            );
            add(
                sash,
                "drawRect:",
                sash_draw as *const c_void,
                c"v@:{CGRect={CGPoint=dd}{CGSize=dd}}",
            );
            add(sash, "acceptsFirstMouse:", yes as *const c_void, c"c@:@");
            add(
                sash,
                "mouseDownCanMoveWindow",
                no_flag as *const c_void,
                c"c@:",
            );
            add(
                sash,
                "acceptsFirstResponder",
                yes_flag as *const c_void,
                c"c@:",
            );
            add(
                sash,
                "becomeFirstResponder",
                sash_focus_changed as *const c_void,
                c"c@:",
            );
            add(
                sash,
                "resignFirstResponder",
                sash_focus_changed as *const c_void,
                c"c@:",
            );
            add(sash, "keyDown:", sash_key_down as *const c_void, c"v@:@");
            objc_registerClassPair(sash);
        }
        add(t, "runguiWake:", on_wake as *const c_void, c"v@:@");
        add(t, "runguiQuit:", on_quit_item as *const c_void, c"v@:@");
        add(t, "runguiTimer:", on_timer as *const c_void, c"v@:@");
        add(t, "runguiAction:", on_action as *const c_void, c"v@:@");
        add(t, "runguiDouble:", on_double as *const c_void, c"v@:@");
        add(
            t,
            "controlTextDidChange:",
            on_text_changed as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "textDidChange:",
            on_text_changed as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "controlTextDidBeginEditing:",
            on_begin_edit as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "textDidBeginEditing:",
            on_begin_edit as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "controlTextDidEndEditing:",
            on_end_edit as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "textDidEndEditing:",
            on_end_edit as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "windowShouldClose:",
            on_should_close as *const c_void,
            c"c@:@",
        );
        add(t, "windowDidResize:", on_resized as *const c_void, c"v@:@");
        add(t, "windowDidMove:", on_moved as *const c_void, c"v@:@");
        add(
            t,
            "windowDidBecomeKey:",
            on_became_key as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "tabView:didSelectTabViewItem:",
            on_tab_selected as *const c_void,
            c"v@:@@",
        );
        add(
            t,
            "numberOfRowsInTableView:",
            tv_rows as *const c_void,
            c"q@:@",
        );
        add(
            t,
            "tableView:objectValueForTableColumn:row:",
            tv_value as *const c_void,
            c"@@:@@q",
        );
        add(
            t,
            "tableViewSelectionDidChange:",
            tv_selection as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "tableView:didClickTableColumn:",
            tv_column_clicked as *const c_void,
            c"v@:@@",
        );
        add(
            t,
            "outlineView:numberOfChildrenOfItem:",
            ov_count as *const c_void,
            c"q@:@@",
        );
        add(
            t,
            "outlineView:child:ofItem:",
            ov_child as *const c_void,
            c"@@:@q@",
        );
        add(
            t,
            "outlineView:isItemExpandable:",
            ov_expandable as *const c_void,
            c"c@:@@",
        );
        add(
            t,
            "outlineView:objectValueForTableColumn:byItem:",
            ov_value as *const c_void,
            c"@@:@@@",
        );
        #[cfg(rungui_gnustep)]
        add(
            t,
            "application:openFile:",
            app_open_file as *const c_void,
            c"c@:@@",
        );
        add(
            t,
            "outlineViewItemDidExpand:",
            ov_did_expand as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "outlineViewItemDidCollapse:",
            ov_did_collapse as *const c_void,
            c"v@:@",
        );
        add(
            t,
            "outlineViewSelectionDidChange:",
            ov_selection as *const c_void,
            c"v@:@",
        );
        objc_registerClassPair(t);
        let app_cls = objc_allocateClassPair(cls("NSApplication"), c"RunguiApp".as_ptr(), 0);
        if !app_cls.is_null() {
            add(
                app_cls,
                "sendEvent:",
                app_send_event as *const c_void,
                c"v@:@",
            );
            objc_registerClassPair(app_cls);
        }
        Ok((t, alloc_init_class(t)))
    }
}
fn alloc_init_class(c: Id) -> Id {
    idm!(idm!(c, "alloc"), "init")
}

// ---------------------------------------------------------------- menus & accelerators

const MOD_SHIFT: usize = 1 << 17;
const MOD_OPTION: usize = 1 << 19;
const MOD_COMMAND: usize = 1 << 20;

fn key_equivalent(a: &Accel) -> Option<(String, usize)> {
    let mut mask = 0;
    if a.ctrl {
        mask |= MOD_COMMAND;
    }
    if a.shift {
        mask |= MOD_SHIFT;
    }
    if a.alt {
        mask |= MOD_OPTION;
    }
    let fk = |c: u32| char::from_u32(c).map(String::from);
    let key = match a.key.as_str() {
        "ENTER" | "RETURN" => "\r".to_string(),
        "ESC" | "ESCAPE" => "\u{1b}".to_string(),
        "DEL" | "DELETE" => fk(0xF728)?,
        "BACKSPACE" => "\u{8}".to_string(),
        "TAB" => "\t".to_string(),
        "SPACE" => " ".to_string(),
        "LEFT" => fk(0xF702)?,
        "RIGHT" => fk(0xF703)?,
        "UP" => fk(0xF700)?,
        "DOWN" => fk(0xF701)?,
        "HOME" => fk(0xF729)?,
        "END" => fk(0xF72B)?,
        "PAGEUP" => fk(0xF72C)?,
        "PAGEDOWN" => fk(0xF72D)?,
        k if k.len() > 1
            && k.starts_with('F')
            && k[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            fk(0xF704 + k[1..].parse::<u32>().ok()? - 1)?
        }
        // GNUstep matches the shifted character (`G`); AppKit takes `g` plus the Shift mask.
        #[cfg(rungui_gnustep)]
        k if k.chars().count() == 1 && a.shift => k.to_uppercase(),
        k if k.chars().count() == 1 => k.to_lowercase(),
        _ => return None,
    };
    Some((key, mask))
}

fn new_menu(title: &str) -> Id {
    let m = idm!(idm!(cls("NSMenu"), "alloc"), "initWithTitle:", Id: ns(title));
    vm!(m, "setAutoenablesItems:", u8: 0);
    m
}

fn new_item(title: &str, action: Sel, key: &str) -> Id {
    idm!(idm!(cls("NSMenuItem"), "alloc"), "initWithTitle:action:keyEquivalent:", Id: ns(title), Sel: action, Id: ns(key))
}

/// Standard application menu (Quit) as a menu bar item.
fn app_menu_item(target: Id, name: &str) -> Id {
    let item = idm!(idm!(cls("NSMenuItem"), "alloc"), "init");
    vm!(item, "setTitle:", Id: ns(name));
    let menu = new_menu(name);
    let q = new_item(&format!("Quit {name}"), sel!("runguiQuit:"), "q");
    vm!(q, "setTarget:", Id: target);
    vm!(menu, "addItem:", Id: q);
    release(q);
    vm!(item, "setSubmenu:", Id: menu);
    release(menu);
    item
}

/// Standard Edit menu: first-responder (nil target) actions make copy/paste work in text fields.
fn edit_menu_item() -> Id {
    let item = idm!(idm!(cls("NSMenuItem"), "alloc"), "init");
    let menu = new_menu("Edit");
    for (title, action, key, mask) in [
        ("Undo", "undo:", "z", MOD_COMMAND),
        ("Redo", "redo:", "z", MOD_COMMAND | MOD_SHIFT),
        ("", "", "", 0),
        ("Cut", "cut:", "x", MOD_COMMAND),
        ("Copy", "copy:", "c", MOD_COMMAND),
        ("Paste", "paste:", "v", MOD_COMMAND),
        ("Select All", "selectAll:", "a", MOD_COMMAND),
    ] {
        let it = if title.is_empty() {
            idm!(cls("NSMenuItem"), "separatorItem")
        } else {
            let it = new_item(title, sel_named(action), key);
            vm!(it, "setKeyEquivalentModifierMask:", usize: mask);
            vm!(it, "setEnabled:", u8: 1);
            it
        };
        vm!(menu, "addItem:", Id: it);
    }
    vm!(item, "setSubmenu:", Id: menu);
    release(menu);
    item
}

fn new_menubar(target: Id, name: &str, with_edit: bool) -> (Id, Id) {
    let bar = new_menu("");
    let app = app_menu_item(target, name);
    vm!(bar, "addItem:", Id: app);
    release(app);
    let mut edit = NIL;
    if with_edit {
        edit = edit_menu_item();
        vm!(bar, "addItem:", Id: edit);
        release(edit); // the bar keeps it alive; pointer stays valid until removed
    }
    (bar, edit)
}

// ---------------------------------------------------------------- backend

pub struct Cocoa;

fn is_view_kind(k: Kind) -> bool {
    !matches!(
        k,
        Kind::Window
            | Kind::Page
            | Kind::MenuBar
            | Kind::Menu
            | Kind::MenuItem
            | Kind::CheckMenuItem
            | Kind::MenuSeparator
            | Kind::PopupMenu
    )
}

fn set_target_action(ctl: Id, target: Id, action: Sel) {
    vm!(ctl, "setTarget:", Id: target);
    vm!(ctl, "setAction:", Sel: action);
}

fn label_field(editable: bool, secure: bool) -> Id {
    let f = view_new(
        if secure {
            "NSSecureTextField"
        } else {
            "NSTextField"
        },
        rect(0.0, 0.0, 100.0, 22.0),
    );
    if !editable {
        vm!(f, "setEditable:", u8: 0);
        vm!(f, "setSelectable:", u8: 0);
        vm!(f, "setBezeled:", u8: 0);
        vm!(f, "setBordered:", u8: 0);
        vm!(f, "setDrawsBackground:", u8: 0);
    }
    f
}

fn cell_size(v: Id) -> Size {
    let cell = idm!(v, "cell");
    if cell.is_null() {
        return Size::default();
    }
    let s = send!(NSSize, cell, "cellSize");
    Size::new(s.w.ceil() as i32, s.h.ceil() as i32)
}

impl Backend for Cocoa {
    fn init(app_name: &str) -> Result<()> {
        if cls("NSApplication").is_null() {
            return Err(Error::Backend("AppKit not available".into()));
        }
        // Lives for the whole process: holds autoreleased setup-time objects.
        let _pool = alloc_init("NSAutoreleasePool");
        let (tclass, target) = define_classes()?;
        let app_cls = cls("RunguiApp");
        let app = idm!(
            if app_cls.is_null() {
                cls("NSApplication")
            } else {
                app_cls
            },
            "sharedApplication"
        );
        if app.is_null() {
            return Err(Error::Backend("NSApplication unavailable".into()));
        }
        if responds(app, "setActivationPolicy:") {
            send!(u8, app, "setActivationPolicy:", isize: 0);
        }
        let pinfo = idm!(cls("NSProcessInfo"), "processInfo");
        if responds(pinfo, "setProcessName:") {
            vm!(pinfo, "setProcessName:", Id: ns(app_name));
        }
        let (bar, _) = new_menubar(target, app_name, true);
        vm!(app, "setMainMenu:", Id: bar);
        #[cfg(rungui_gnustep)]
        vm!(app, "setDelegate:", Id: target);
        TARGET.store(target, Ordering::SeqCst);
        st(|s| {
            s.target = target;
            s.tclass = tclass;
            s.app = app;
            s.app_name = app_name.to_string();
        });
        Ok(())
    }

    fn run() {
        let app = st(|s| s.app);
        if app.is_null() {
            return;
        }
        vm!(app, "activateIgnoringOtherApps:", u8: 1);
        #[cfg(not(rungui_gnustep))]
        vm!(app, "run");
        // GNUstep's NSApp -stop: is only noticed after the next X event (a posted dummy event
        // does not wake its wait), so run our own loop with a short timeout instead.
        #[cfg(rungui_gnustep)]
        {
            QUIT.store(false, Ordering::SeqCst);
            vm!(app, "finishLaunching");
            while !QUIT.load(Ordering::SeqCst) {
                let pool = alloc_init("NSAutoreleasePool");
                let until = idm!(cls("NSDate"), "dateWithTimeIntervalSinceNow:", f64: 0.1);
                let ev = idm!(app, "nextEventMatchingMask:untilDate:inMode:dequeue:", usize: usize::MAX, Id: until, Id: ns(MODE_DEFAULT), u8: 1);
                if !ev.is_null() {
                    vm!(app, "sendEvent:", Id: ev);
                    vm!(app, "updateWindows");
                }
                release(pool);
            }
        }
    }

    fn quit() {
        let app = st(|s| s.app);
        if app.is_null() {
            return;
        }
        #[cfg(rungui_gnustep)]
        QUIT.store(true, Ordering::SeqCst);
        vm!(app, "stop:", Id: NIL);
        // `stop:` is only noticed after an event: post a dummy application-defined one.
        let ev = idm!(
            cls("NSEvent"),
            "otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
            usize: 15,
            NSPoint: NSPoint::default(),
            usize: 0,
            f64: 0.0,
            isize: 0,
            Id: NIL,
            i16: 0,
            isize: 0,
            isize: 0
        );
        if !ev.is_null() {
            vm!(app, "postEvent:atStart:", Id: ev, u8: 1);
        }
    }

    fn wake() {
        let t = TARGET.load(Ordering::SeqCst);
        if t.is_null() {
            return;
        }
        // May run on any thread: give it its own autorelease pool.
        let pool = alloc_init("NSAutoreleasePool");
        let modes = {
            let a = [
                ns(MODE_DEFAULT),
                ns("NSModalPanelRunLoopMode"),
                ns("NSEventTrackingRunLoopMode"),
            ];
            idm!(cls("NSArray"), "arrayWithObjects:count:", *const Id: a.as_ptr(), usize: a.len())
        };
        vm!(t, "performSelectorOnMainThread:withObject:waitUntilDone:modes:", Sel: sel!("runguiWake:"), Id: NIL, u8: 0, Id: modes);
        if !pool.is_null() {
            vm!(pool, "drain");
        }
    }

    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        let target = st(|s| s.target);
        let t = idm!(
            cls("NSTimer"),
            "scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:",
            f64: millis as f64 / 1000.0,
            Id: target,
            Sel: sel!("runguiTimer:"),
            Id: NIL,
            u8: b(repeat)
        );
        if t.is_null() {
            return Err(Error::Backend("NSTimer creation failed".into()));
        }
        // Make it fire while menus/modals/live resizes run too. On Apple platforms the common-modes
        // pseudo mode covers default + event tracking + modal panel; GNUstep lacks it.
        let rl = idm!(cls("NSRunLoop"), "currentRunLoop");
        #[cfg(not(rungui_gnustep))]
        vm!(rl, "addTimer:forMode:", Id: t, Id: ns("kCFRunLoopCommonModes")); // == NSRunLoopCommonModes
        #[cfg(rungui_gnustep)]
        {
            vm!(rl, "addTimer:forMode:", Id: t, Id: ns("NSEventTrackingRunLoopMode"));
            vm!(rl, "addTimer:forMode:", Id: t, Id: ns("NSModalPanelRunLoopMode"));
        }
        idm!(t, "retain");
        st(|s| {
            s.trev.insert(t as usize, token);
            s.timers.insert(token, t);
        });
        Ok(())
    }

    fn timer_stop(token: u64) {
        if let Some(t) = st(|s| {
            let t = s.timers.remove(&token)?;
            s.trev.remove(&(t as usize));
            Some(t)
        }) {
            vm!(t, "invalidate");
            autorelease(t);
            #[cfg(rungui_gnustep)]
            purge_invalid_timers();
        }
    }

    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
        let _q = Quiet::new();
        let (target, name) = st(|s| (s.target, s.app_name.clone()));
        if target.is_null() {
            return Err(Error::NotInitialized);
        }
        let pe = match parent {
            Some(p) => Some(ent(p).ok_or(Error::InvalidHandle)?),
            None => None,
        };
        let pview = pe
            .map(|e| if e.cont.is_null() { e.obj } else { e.cont })
            .unwrap_or(NIL);
        if kind != Kind::Window && kind != Kind::PopupMenu && pe.is_none() {
            return Err(Error::InvalidHandle);
        }
        let act = sel!("runguiAction:");
        let mut e = Entry {
            kind,
            ..Default::default()
        };
        match kind {
            Kind::Window => {
                let w = idm!(
                    idm!(cls("NSWindow"), "alloc"),
                    "initWithContentRect:styleMask:backing:defer:",
                    NSRect: rect(0.0, 0.0, 400.0, 300.0),
                    usize: 15,
                    usize: 2,
                    u8: 0
                );
                if w.is_null() {
                    return Err(Error::Backend("NSWindow creation failed".into()));
                }
                vm!(w, "setReleasedWhenClosed:", u8: 0);
                vm!(w, "setDelegate:", Id: target);
                let flip = flip_view();
                vm!(w, "setContentView:", Id: flip);
                release(flip);
                e.obj = w;
                e.cont = flip;
            }
            Kind::Label => e.obj = label_field(false, false),
            Kind::Sash => {
                if cls("RunguiSash").is_null() {
                    return Err(Error::Unsupported);
                }
                e.obj = view_new("RunguiSash", NSRect::default());
            }
            Kind::TextInput | Kind::PasswordInput => {
                let f = label_field(true, kind == Kind::PasswordInput);
                vm!(f, "setDelegate:", Id: target);
                e.obj = f;
            }
            Kind::Button => {
                let v = view_new("NSButton", rect(0.0, 0.0, 80.0, 24.0));
                vm!(v, "setButtonType:", usize: 7);
                vm!(v, "setBezelStyle:", usize: 1);
                set_target_action(v, target, act);
                e.obj = v;
            }
            Kind::CheckBox | Kind::RadioButton => {
                let v = view_new("NSButton", rect(0.0, 0.0, 80.0, 20.0));
                let radio = kind == Kind::RadioButton;
                vm!(v, "setButtonType:", usize: if radio { 4 } else { 3 });
                let mut a = act;
                if radio {
                    // AppKit auto-groups radios sharing target+action in one superview; the core
                    // owns exclusivity, so give every radio its own action selector.
                    let name = format!("runguiRadio{}:", id.0);
                    a = sel_named(&name);
                    unsafe {
                        class_addMethod(
                            st(|s| s.tclass),
                            a,
                            on_action as *const c_void,
                            c"v@:@".as_ptr(),
                        );
                    }
                }
                set_target_action(v, target, a);
                e.obj = v;
            }
            Kind::TextArea => {
                let sv = view_new("NSScrollView", rect(0.0, 0.0, 200.0, 100.0));
                vm!(sv, "setHasVerticalScroller:", u8: 1);
                vm!(sv, "setBorderType:", usize: 2);
                let tv = view_new("NSTextView", rect(0.0, 0.0, 200.0, 100.0));
                vm!(tv, "setMinSize:", NSSize: NSSize { w: 0.0, h: 100.0 });
                vm!(tv, "setMaxSize:", NSSize: NSSize { w: 1e7, h: 1e7 });
                vm!(tv, "setVerticallyResizable:", u8: 1);
                vm!(tv, "setHorizontallyResizable:", u8: 0);
                vm!(tv, "setAutoresizingMask:", usize: 2);
                let tc = idm!(tv, "textContainer");
                if !tc.is_null() {
                    vm!(tc, "setContainerSize:", NSSize: NSSize { w: 200.0, h: 1e7 });
                    vm!(tc, "setWidthTracksTextView:", u8: 1);
                }
                vm!(tv, "setRichText:", u8: 0);
                vm!(tv, "setAllowsUndo:", u8: 1);
                vm!(tv, "setDelegate:", Id: target);
                vm!(sv, "setDocumentView:", Id: tv);
                release(tv);
                e.obj = sv;
                e.aux = tv;
            }
            Kind::ComboBox => {
                let v = view_new("NSPopUpButton", rect(0.0, 0.0, 140.0, 26.0));
                set_target_action(v, target, act);
                e.obj = v;
            }
            Kind::ListBox => {
                let sv = view_new("NSScrollView", rect(0.0, 0.0, 160.0, 100.0));
                vm!(sv, "setHasVerticalScroller:", u8: 1);
                vm!(sv, "setBorderType:", usize: 2);
                let tv = view_new("NSTableView", rect(0.0, 0.0, 160.0, 100.0));
                let col =
                    idm!(idm!(cls("NSTableColumn"), "alloc"), "initWithIdentifier:", Id: ns("c"));
                vm!(col, "setWidth:", f64: 150.0);
                let cell = idm!(col, "dataCell");
                if !cell.is_null() {
                    vm!(cell, "setEditable:", u8: 0);
                }
                vm!(tv, "addTableColumn:", Id: col);
                release(col);
                vm!(tv, "setHeaderView:", Id: NIL);
                vm!(tv, "setAllowsMultipleSelection:", u8: 0);
                vm!(tv, "setAllowsEmptySelection:", u8: 1);
                vm!(tv, "setDataSource:", Id: target);
                vm!(tv, "setDelegate:", Id: target);
                vm!(tv, "setTarget:", Id: target);
                vm!(tv, "setDoubleAction:", Sel: sel!("runguiDouble:"));
                vm!(sv, "setDocumentView:", Id: tv);
                release(tv);
                e.obj = sv;
                e.aux = tv;
            }
            Kind::Table | Kind::Tree => {
                let tree = kind == Kind::Tree;
                let sv = view_new("NSScrollView", rect(0.0, 0.0, 300.0, 150.0));
                vm!(sv, "setHasVerticalScroller:", u8: 1);
                vm!(sv, "setHasHorizontalScroller:", u8: 1);
                vm!(sv, "setBorderType:", usize: 2);
                let tv = view_new(
                    if tree { "NSOutlineView" } else { "NSTableView" },
                    rect(0.0, 0.0, 300.0, 150.0),
                );
                vm!(tv, "setAllowsMultipleSelection:", u8: 0);
                vm!(tv, "setAllowsEmptySelection:", u8: 1);
                vm!(tv, "setDataSource:", Id: target);
                vm!(tv, "setDelegate:", Id: target);
                vm!(tv, "setTarget:", Id: target);
                vm!(tv, "setDoubleAction:", Sel: sel!("runguiDouble:"));
                if tree {
                    vm!(tv, "setHeaderView:", Id: NIL);
                    let col = idm!(idm!(cls("NSTableColumn"), "alloc"), "initWithIdentifier:", Id: ns("0"));
                    vm!(col, "setWidth:", f64: 200.0);
                    let cell = idm!(col, "dataCell");
                    if !cell.is_null() {
                        vm!(cell, "setEditable:", u8: 0);
                    }
                    vm!(tv, "addTableColumn:", Id: col);
                    vm!(tv, "setOutlineTableColumn:", Id: col);
                    release(col);
                }
                vm!(sv, "setDocumentView:", Id: tv);
                release(tv);
                e.obj = sv;
                e.aux = tv;
            }
            Kind::PopupMenu => {
                e.obj = new_menu("");
            }
            Kind::Slider => {
                let v = view_new("NSSlider", rect(0.0, 0.0, 150.0, 21.0));
                vm!(v, "setMinValue:", f64: 0.0);
                vm!(v, "setMaxValue:", f64: 100.0);
                set_target_action(v, target, act);
                e.obj = v;
            }
            Kind::ProgressBar => {
                let v = view_new("NSProgressIndicator", rect(0.0, 0.0, 150.0, 20.0));
                vm!(v, "setStyle:", usize: 0);
                vm!(v, "setIndeterminate:", u8: 0);
                vm!(v, "setMinValue:", f64: 0.0);
                vm!(v, "setMaxValue:", f64: 1.0);
                e.obj = v;
            }
            Kind::SpinBox => {
                let c = flip_view();
                let tf = label_field(true, false);
                let stp = view_new("NSStepper", rect(0.0, 0.0, 19.0, 27.0));
                vm!(stp, "setMinValue:", f64: 0.0);
                vm!(stp, "setMaxValue:", f64: 100.0);
                vm!(stp, "setIncrement:", f64: 1.0);
                vm!(stp, "setValueWraps:", u8: 0);
                set_target_action(stp, target, act);
                set_target_action(tf, target, act);
                vm!(tf, "setDelegate:", Id: target);
                vm!(c, "addSubview:", Id: tf);
                vm!(c, "addSubview:", Id: stp);
                release(tf);
                release(stp);
                e.obj = c;
                e.aux = tf;
                e.aux2 = stp;
            }
            Kind::Tabs => {
                let v = view_new("NSTabView", rect(0.0, 0.0, 300.0, 200.0));
                vm!(v, "setDelegate:", Id: target);
                e.obj = v;
            }
            Kind::Page => {
                let item =
                    idm!(idm!(cls("NSTabViewItem"), "alloc"), "initWithIdentifier:", Id: NIL);
                let pv = flip_view();
                vm!(item, "setView:", Id: pv);
                release(pv);
                {
                    let _q = Quiet::new();
                    vm!(pview, "addTabViewItem:", Id: item);
                }
                release(item);
                release_stale_page_view(pview);
                e.obj = pv;
                e.cont = pv;
                e.aux = item;
            }
            Kind::GroupBox => {
                let bx = view_new("NSBox", rect(0.0, 0.0, 200.0, 100.0));
                let flip = flip_view();
                vm!(bx, "setContentView:", Id: flip);
                release(flip);
                e.obj = bx;
                e.cont = flip;
            }
            Kind::Image => {
                let v = view_new("NSImageView", rect(0.0, 0.0, 32.0, 32.0));
                vm!(v, "setImageScaling:", usize: 0);
                e.obj = v;
            }
            Kind::MenuBar => {
                let (bar, edit) = new_menubar(target, &name, true);
                e.obj = bar;
                e.aux = edit;
                st(|s| {
                    s.menubars.insert(parent.unwrap_or(WidgetId::DEAD), bar);
                    s.std_edit.insert(id, edit);
                });
                if let Some(pe) = pe {
                    if bm!(pe.obj, "isKeyWindow") || bm!(pe.obj, "isVisible") {
                        install_menubar(parent.unwrap_or(WidgetId::DEAD));
                    }
                }
            }
            Kind::Menu => {
                let item = idm!(idm!(cls("NSMenuItem"), "alloc"), "init");
                let menu = new_menu("");
                vm!(item, "setSubmenu:", Id: menu);
                vm!(pe.map(|p| p.obj).unwrap_or(NIL), "addItem:", Id: item);
                release(item);
                e.obj = menu;
                e.aux = item;
                release(menu);
            }
            Kind::MenuItem | Kind::CheckMenuItem => {
                let it = new_item("", act, "");
                vm!(it, "setTarget:", Id: target);
                vm!(pe.map(|p| p.obj).unwrap_or(NIL), "addItem:", Id: it);
                release(it);
                e.obj = it;
            }
            Kind::MenuSeparator => {
                let it = idm!(cls("NSMenuItem"), "separatorItem");
                vm!(pe.map(|p| p.obj).unwrap_or(NIL), "addItem:", Id: it);
                e.obj = it;
            }
            _ => return Err(Error::Unsupported),
        }
        if e.obj.is_null() {
            return Err(Error::Backend(format!("cannot create native {kind:?}")));
        }
        if is_view_kind(kind) {
            if pview.is_null() {
                release(e.obj);
                return Err(Error::InvalidHandle);
            }
            vm!(pview, "addSubview:", Id: e.obj);
            release(e.obj); // the superview owns it now
        }
        st(|s| {
            s.rev.insert(e.obj as usize, id);
            for p in [e.aux, e.aux2] {
                if !p.is_null() && kind != Kind::MenuBar {
                    s.rev.insert(p as usize, id);
                }
            }
            if kind == Kind::Window {
                // Window delegate notifications carry the window itself.
            }
            s.ents.insert(id, e);
        });
        Ok(())
    }

    fn destroy(id: WidgetId) {
        let _q = Quiet::new();
        let Some(e) = st(|s| {
            let e = s.ents.remove(&id)?;
            s.rev.retain(|_, v| *v != id);
            s.items.remove(&id);
            s.rows.remove(&id);
            s.range.remove(&id);
            s.imgsz.remove(&id);
            s.readonly.remove(&id);
            s.disabled.remove(&id);
            s.shown.remove(&id);
            s.placed.remove(&id);
            s.fixed.remove(&id);
            s.min_size.remove(&id);
            s.sash_orient.remove(&id);
            if s.drag.is_some_and(|d| d.0 == id) {
                s.drag = None;
            }
            s.std_edit.remove(&id);
            Some(e)
        }) else {
            return;
        };
        match e.kind {
            Kind::Window => {
                st(|s| s.menubars.remove(&id));
                vm!(e.obj, "setDelegate:", Id: NIL);
                vm!(e.obj, "orderOut:", Id: NIL);
                vm!(e.obj, "close");
                autorelease(e.obj);
            }
            Kind::PopupMenu => autorelease(e.obj),
            Kind::MenuBar => {
                let app = st(|s| {
                    s.menubars.retain(|_, m| *m != e.obj);
                    s.app
                });
                if idm!(app, "mainMenu") == e.obj {
                    vm!(app, "setMainMenu:", Id: NIL);
                }
                autorelease(e.obj);
            }
            Kind::Menu => {
                let m = idm!(e.aux, "menu");
                if !m.is_null() {
                    vm!(m, "removeItem:", Id: e.aux);
                }
            }
            Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
                let m = idm!(e.obj, "menu");
                if !m.is_null() {
                    vm!(m, "removeItem:", Id: e.obj);
                }
            }
            Kind::Page => {
                let tv = idm!(e.aux, "tabView");
                if !tv.is_null() {
                    // GNUstep keeps a dangling pointer to the view of the last page it loses
                    #[cfg(rungui_gnustep)]
                    idm!(e.obj, "retain");
                    vm!(tv, "removeTabViewItem:", Id: e.aux);
                    #[cfg(rungui_gnustep)]
                    if send!(isize, tv, "numberOfTabViewItems") == 0 {
                        release_stale_page_view(tv);
                        st(|s| s.stale_page_views.insert(tv as usize, e.obj));
                    } else {
                        release(e.obj);
                    }
                }
            }
            _ => {
                if kind_has_delegate(e.kind) {
                    for o in [e.obj, e.aux] {
                        if !o.is_null() && responds(o, "setDelegate:") {
                            vm!(o, "setDelegate:", Id: NIL);
                        }
                    }
                    if matches!(e.kind, Kind::ListBox | Kind::Table | Kind::Tree) {
                        vm!(e.aux, "setDataSource:", Id: NIL);
                    }
                    if e.kind == Kind::Tree {
                        if let Some(m) = st(|s| s.trees.remove(&id)) {
                            m.nodes.iter().for_each(|n| autorelease(n.obj));
                        }
                    }
                }
                if e.kind == Kind::Tabs {
                    release_stale_page_view(e.obj);
                }
                autorelease(idm!(e.obj, "retain"));
                vm!(e.obj, "removeFromSuperview");
            }
        }
    }

    #[allow(unreachable_patterns)]
    fn set(id: WidgetId, prop: &Prop) {
        let _q = Quiet::new();
        let Some(e) = ent(id) else { return };
        match prop {
            Prop::Text(t) => set_text(id, &e, t),
            Prop::Tooltip(t) => {
                if is_view_kind(e.kind) {
                    vm!(e.obj, "setToolTip:", Id: ns(t));
                }
            }
            Prop::Placeholder(t) => {
                if matches!(e.kind, Kind::TextInput | Kind::PasswordInput) {
                    let cell = idm!(e.obj, "cell");
                    if responds(cell, "setPlaceholderString:") {
                        vm!(cell, "setPlaceholderString:", Id: ns(t));
                    }
                }
            }
            Prop::Enabled(en) => set_enabled(id, &e, *en),
            Prop::Visible(v) => set_visible(id, &e, *v),
            Prop::Checked(c) => {
                if matches!(
                    e.kind,
                    Kind::CheckBox | Kind::RadioButton | Kind::CheckMenuItem
                ) {
                    vm!(e.obj, "setState:", isize: *c as isize);
                }
            }
            Prop::Value(v) => match e.kind {
                Kind::Slider | Kind::ProgressBar => vm!(e.obj, "setDoubleValue:", f64: *v),
                Kind::SpinBox => set_spin(&e, *v),
                _ => {}
            },
            Prop::Range { min, max, step } => {
                st(|s| s.range.insert(id, (*min, *max, *step)));
                match e.kind {
                    Kind::Slider => {
                        vm!(e.obj, "setMinValue:", f64: *min);
                        vm!(e.obj, "setMaxValue:", f64: *max);
                    }
                    Kind::SpinBox => {
                        vm!(e.aux2, "setMinValue:", f64: *min);
                        vm!(e.aux2, "setMaxValue:", f64: *max);
                        vm!(e.aux2, "setIncrement:", f64: if *step > 0.0 { *step } else { 1.0 });
                    }
                    _ => {}
                }
            }
            Prop::Items(items) => set_items(id, &e, items),
            Prop::Selected(sel) => match e.kind {
                Kind::ComboBox => {
                    vm!(e.obj, "selectItemAtIndex:", isize: sel.map_or(-1, |i| i as isize))
                }
                Kind::Tabs => {
                    if let Some(i) = sel {
                        vm!(e.obj, "selectTabViewItemAtIndex:", isize: *i as isize);
                    }
                }
                Kind::ListBox | Kind::Table => match sel {
                    // GNUstep raises for a table without columns, and AppKit for a row its view
                    // does not (yet) have; the core re-sends the selection with the columns
                    Some(i)
                        if send!(isize, e.aux, "numberOfColumns") > 0
                            && (*i as isize) < send!(isize, e.aux, "numberOfRows") =>
                    {
                        let set = idm!(cls("NSIndexSet"), "indexSetWithIndex:", usize: *i);
                        vm!(e.aux, "selectRowIndexes:byExtendingSelection:", Id: set, u8: 0);
                        vm!(e.aux, "scrollRowToVisible:", isize: *i as isize);
                    }
                    _ => vm!(e.aux, "deselectAll:", Id: NIL),
                },
                _ => {}
            },
            Prop::Bounds(r) => set_bounds(&e, id, *r),
            Prop::Image(img) => set_image(id, &e, *img),
            Prop::Accel(a) => {
                if matches!(e.kind, Kind::MenuItem | Kind::CheckMenuItem) {
                    let (key, mask) = Accel::parse(a)
                        .and_then(|a| key_equivalent(&a))
                        .unwrap_or_default();
                    vm!(e.obj, "setKeyEquivalent:", Id: ns(&key));
                    vm!(e.obj, "setKeyEquivalentModifierMask:", usize: mask);
                }
            }
            Prop::ReadOnly(ro) => {
                st(|s| {
                    if *ro {
                        s.readonly.insert(id);
                    } else {
                        s.readonly.remove(&id);
                    }
                });
                match e.kind {
                    Kind::TextInput | Kind::PasswordInput => {
                        vm!(e.obj, "setEditable:", u8: b(!*ro))
                    }
                    Kind::TextArea => vm!(e.aux, "setEditable:", u8: b(!*ro)),
                    _ => {}
                }
            }
            Prop::Indeterminate(ind) => {
                if e.kind == Kind::ProgressBar {
                    vm!(e.obj, "setIndeterminate:", u8: b(*ind));
                    if *ind {
                        vm!(e.obj, "startAnimation:", Id: NIL);
                    } else {
                        vm!(e.obj, "stopAnimation:", Id: NIL);
                    }
                }
            }
            Prop::Resizable(r) => {
                if e.kind == Kind::Window {
                    set_resizable(id, &e, *r);
                }
            }
            Prop::Columns(cols) => set_columns(&e, cols),
            Prop::Rows(rows) => {
                st(|s| s.rows.insert(id, rows.to_vec()));
                vm!(e.aux, "reloadData");
            }
            Prop::SortIndicator(si) => set_sort_indicator(&e, *si),
            Prop::TreeRows(rows) => set_tree_rows(id, &e, rows),
            Prop::TreeSelected(n) => {
                let row = n.and_then(|n| {
                    let item = st(|s| {
                        s.trees.get(&id).and_then(|m| {
                            m.by_id.get(&n).and_then(|i| m.nodes.get(*i)).map(|n| n.obj)
                        })
                    })?;
                    let r = send!(isize, e.aux, "rowForItem:", Id: item);
                    (r >= 0).then_some(r)
                });
                match row {
                    Some(r) => {
                        let set = idm!(cls("NSIndexSet"), "indexSetWithIndex:", usize: r as usize);
                        vm!(e.aux, "selectRowIndexes:byExtendingSelection:", Id: set, u8: 0);
                        vm!(e.aux, "scrollRowToVisible:", isize: r);
                    }
                    None => vm!(e.aux, "deselectAll:", Id: NIL),
                }
            }
            Prop::Orientation(o) => {
                if e.kind == Kind::Sash {
                    st(|s| s.sash_orient.insert(id, *o));
                    let w = idm!(e.obj, "window");
                    if !w.is_null() {
                        vm!(w, "invalidateCursorRectsForView:", Id: e.obj);
                    }
                    vm!(e.obj, "setNeedsDisplay:", u8: 1);
                }
            }
            Prop::Monospace(m) => set_monospace(&e, *m),
            Prop::Wrap(w) => {
                if e.kind == Kind::TextArea {
                    set_wrap(&e, *w);
                }
            }
            Prop::Position { x, y } => {
                if e.kind == Kind::Window {
                    st(|s| s.placed.insert(id));
                    let p = NSPoint {
                        x: *x as f64,
                        y: primary_screen_height() - *y as f64,
                    };
                    vm!(e.obj, "setFrameTopLeftPoint:", NSPoint: p);
                }
            }
            Prop::MinSize(sz) => {
                if e.kind == Kind::Window {
                    let (mw, mh) = (sz.w.max(0) as f64, sz.h.max(0) as f64);
                    st(|s| s.min_size.insert(id, NSSize { w: mw, h: mh }));
                    if !st(|s| s.fixed.contains(&id)) {
                        vm!(e.obj, "setContentMinSize:", NSSize: NSSize { w: mw, h: mh });
                    }
                    // AppKit only enforces the minimum on the next resize: grow right away, as
                    // the core never lays out below it (and GNUstep would otherwise move the window).
                    let cur = client_size(&e);
                    if cur.w < mw || cur.h < mh {
                        resize_window(e.obj, cur.w.max(mw), cur.h.max(mh));
                    }
                }
            }
            Prop::Focus => {
                let target = match e.kind {
                    Kind::TextArea | Kind::ListBox | Kind::SpinBox | Kind::Table | Kind::Tree => {
                        e.aux
                    }
                    Kind::Window => return,
                    k if is_view_kind(k) => e.obj,
                    _ => return,
                };
                let w = idm!(target, "window");
                if !w.is_null() {
                    vm!(w, "makeFirstResponder:", Id: target);
                }
            }
            _ => {}
        }
    }

    fn preferred_size(id: WidgetId) -> Size {
        let Some(e) = ent(id) else {
            return Size::default();
        };
        match e.kind {
            Kind::Label | Kind::CheckBox | Kind::RadioButton | Kind::ComboBox => {
                let s = cell_size(e.obj);
                let min_h = if e.kind == Kind::ComboBox { 26 } else { 17 };
                Size::new(s.w.max(8), s.h.max(min_h))
            }
            Kind::Button => {
                let s = if responds(e.obj, "fittingSize") {
                    let f = send!(NSSize, e.obj, "fittingSize");
                    Size::new(f.w.ceil() as i32, f.h.ceil() as i32)
                } else {
                    let c = cell_size(e.obj);
                    Size::new(c.w + 16, c.h + 4)
                };
                Size::new(s.w.max(48), s.h.max(24))
            }
            Kind::TextInput | Kind::PasswordInput => Size::new(160, cell_size(e.obj).h.max(22)),
            Kind::SpinBox => Size::new(90, cell_size(e.aux).h.max(22)),
            Kind::TextArea => Size::new(200, 100),
            Kind::ListBox => Size::new(160, 100),
            Kind::Table => Size::new(300, 150),
            Kind::Tree => Size::new(200, 200),
            Kind::Slider => Size::new(150, cell_size(e.obj).h.clamp(16, 40)),
            Kind::ProgressBar => Size::new(150, 20),
            Kind::Image => st(|s| s.imgsz.get(&id).copied()).unwrap_or(Size::new(32, 32)),
            _ => Size::default(),
        }
    }

    fn chrome(id: WidgetId) -> Size {
        let _q = Quiet::new();
        let Some(e) = ent(id) else {
            return Size::default();
        };
        // Measure: give the container a known frame and see how big its client area becomes.
        let (probe_w, probe_h) = (300.0, 300.0);
        match e.kind {
            Kind::GroupBox | Kind::Tabs => {
                let old = rect_of!(e.obj, "frame");
                vm!(e.obj, "setFrame:", NSRect: rect(old.x, old.y, probe_w, probe_h));
                let c = if e.kind == Kind::Tabs {
                    rect_of!(e.obj, "contentRect")
                } else {
                    rect_of!(e.cont, "frame")
                };
                vm!(e.obj, "setFrame:", NSRect: old);
                Size::new(
                    (probe_w - c.w).round().max(0.0) as i32,
                    (probe_h - c.h).round().max(0.0) as i32,
                )
            }
            _ => Size::default(),
        }
    }

    fn native_handle(id: WidgetId) -> Option<NativeHandle> {
        ent(id).map(|e| NativeHandle::Cocoa(e.obj as usize))
    }

    fn message_box(_parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        let alert = alloc_init("NSAlert");
        if alert.is_null() {
            return Answer::Cancel;
        }
        vm!(alert, "setMessageText:", Id: ns(&spec.title));
        vm!(alert, "setInformativeText:", Id: ns(&spec.text));
        let style = match spec.kind {
            MessageKind::Info | MessageKind::Question => 1usize,
            MessageKind::Warning => 0,
            MessageKind::Error => 2,
        };
        vm!(alert, "setAlertStyle:", usize: style);
        let answers: &[(&str, Answer)] = match spec.buttons {
            Buttons::Ok => &[("OK", Answer::Ok)],
            Buttons::OkCancel => &[("OK", Answer::Ok), ("Cancel", Answer::Cancel)],
            Buttons::YesNo => &[("Yes", Answer::Yes), ("No", Answer::No)],
            Buttons::YesNoCancel => &[
                ("Yes", Answer::Yes),
                ("No", Answer::No),
                ("Cancel", Answer::Cancel),
            ],
        };
        for (title, _) in answers {
            idm!(alert, "addButtonWithTitle:", Id: ns(title));
        }
        let r = send!(isize, alert, "runModal");
        release(alert);
        // NSAlertFirstButtonReturn = 1000 (+ index); legacy GNUstep: 1 = first, 0 = second, -1 = third.
        let idx = if r >= 1000 {
            (r - 1000) as usize
        } else {
            match r {
                1 => 0,
                0 => 1,
                _ => 2,
            }
        };
        answers.get(idx).map_or(Answer::Cancel, |a| a.1)
    }

    fn file_dialog(_parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String> {
        let save = spec.mode == FileMode::Save;
        let panel = if save {
            idm!(cls("NSSavePanel"), "savePanel")
        } else {
            idm!(cls("NSOpenPanel"), "openPanel")
        };
        if panel.is_null() {
            return vec![];
        }
        if !spec.title.is_empty() {
            vm!(panel, "setTitle:", Id: ns(&spec.title));
        }
        if !save {
            let folder = spec.mode == FileMode::PickFolder;
            vm!(panel, "setCanChooseFiles:", u8: b(!folder));
            vm!(panel, "setCanChooseDirectories:", u8: b(folder));
            vm!(panel, "setAllowsMultipleSelection:", u8: b(spec.mode == FileMode::OpenMany));
        }
        let exts: Vec<Id> = spec
            .filters
            .iter()
            .flat_map(|(_, e)| e.iter())
            .map(|e| ns(e.trim_start_matches("*.").trim_start_matches('.')))
            .collect();
        // `allowedFileTypes` is deprecated since macOS 12 (UTType replacement) but still honoured.
        if !exts.is_empty()
            && spec.mode != FileMode::PickFolder
            && responds(panel, "setAllowedFileTypes:")
        {
            let arr = idm!(cls("NSArray"), "arrayWithObjects:count:", *const Id: exts.as_ptr(), usize: exts.len());
            vm!(panel, "setAllowedFileTypes:", Id: arr);
        }
        if let Some(dir) = &spec.initial_dir {
            let url = idm!(cls("NSURL"), "fileURLWithPath:", Id: ns(dir));
            vm!(panel, "setDirectoryURL:", Id: url);
        }
        if let (Some(name), true) = (&spec.initial_name, save) {
            vm!(panel, "setNameFieldStringValue:", Id: ns(name));
        }
        let r = send!(isize, panel, "runModal");
        if r != 1 {
            return vec![];
        }
        let path_of = |url: Id| from_ns(idm!(url, "path"));
        if save {
            return vec![path_of(idm!(panel, "URL"))];
        }
        let urls = idm!(panel, "URLs");
        let n = send!(usize, urls, "count");
        (0..n)
            .map(|i| path_of(idm!(urls, "objectAtIndex:", usize: i)))
            .collect()
    }

    fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
        let Some(m) = ent(menu).map(|e| e.obj) else {
            return;
        };
        let Some(we) = parent_window.and_then(ent) else {
            return;
        };
        let h = client_size(&we).h;
        let loc = match at {
            Some((x, y)) => NSPoint {
                x: x as f64,
                y: y as f64,
            },
            None => {
                let p = send!(NSPoint, we.obj, "mouseLocationOutsideOfEventStream");
                NSPoint { x: p.x, y: h - p.y }
            }
        };
        #[cfg(rungui_gnustep)]
        {
            // GNUstep sizes a menu popped up with popUpMenuPositioningItem:... to the window;
            // popUpContextMenu:withEvent:forView: with a synthesized right-click behaves.
            let winnum = send!(isize, we.obj, "windowNumber");
            let ev = idm!(
                cls("NSEvent"),
                "mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
                usize: 3,
                NSPoint: NSPoint { x: loc.x, y: h - loc.y },
                usize: 0,
                f64: 0.0,
                isize: winnum,
                Id: NIL,
                isize: 0,
                isize: 1,
                f32: 1.0
            );
            if !ev.is_null() {
                vm!(cls("NSMenu"), "popUpContextMenu:withEvent:forView:", Id: m, Id: ev, Id: we.cont);
            }
        }
        #[cfg(not(rungui_gnustep))]
        if responds(m, "popUpMenuPositioningItem:atLocation:inView:") {
            // `loc` is in the (flipped) content view's coordinates, which AppKit honours.
            send!(u8, m, "popUpMenuPositioningItem:atLocation:inView:", Id: NIL, NSPoint: loc, Id: we.cont);
        }
    }

    fn a11y_changed(window: WidgetId) {
        let _q = Quiet::new();
        let Some(nodes) = crate::a11y::resolve(window) else {
            return;
        };
        for node in &nodes {
            let Some(e) = ent(node.id) else { continue };
            if matches!(e.kind, Kind::Window | Kind::Page) {
                continue;
            }
            let target = match e.kind {
                Kind::TextArea | Kind::ListBox | Kind::SpinBox | Kind::Table | Kind::Tree => e.aux,
                _ => e.obj,
            };
            if !is_view_kind(e.kind) && !matches!(e.kind, Kind::MenuItem | Kind::CheckMenuItem) {
                continue;
            }
            if let Some(l) = &node.name {
                if responds(target, "setAccessibilityLabel:") {
                    vm!(target, "setAccessibilityLabel:", Id: ns(l));
                }
            }
            if let Some(d) = &node.description {
                if responds(target, "setAccessibilityHelp:") {
                    vm!(target, "setAccessibilityHelp:", Id: ns(d));
                }
            }
        }
    }
}

/// Release the page view `create` or `destroy` kept alive for tab view `tv` (GNUstep only).
fn release_stale_page_view(tv: Id) {
    if let Some(v) = st(|s| s.stale_page_views.remove(&(tv as usize))) {
        release(v);
    }
}

/// GNUstep drops an invalidated timer from a run-loop mode only when the loop next runs in that
/// mode, and our event loop never runs in the tracking and modal modes the timers are also added
/// to (so they fire during menus and dialogs): they would pile up. Asking each mode for its next
/// limit date does the housekeeping. Done every few stops, not each one.
#[cfg(rungui_gnustep)]
fn purge_invalid_timers() {
    thread_local! { static STOPS: Cell<u32> = const { Cell::new(0) }; }
    const PURGE_EVERY: u32 = 32;
    let n = STOPS.with(|c| {
        c.set(c.get().wrapping_add(1));
        c.get()
    });
    if n % PURGE_EVERY != 0 {
        return;
    }
    let rl = idm!(cls("NSRunLoop"), "currentRunLoop");
    for mode in ["NSEventTrackingRunLoopMode", "NSModalPanelRunLoopMode"] {
        idm!(rl, "limitDateForMode:", Id: ns(mode));
    }
}

fn kind_has_delegate(k: Kind) -> bool {
    matches!(
        k,
        Kind::TextInput
            | Kind::PasswordInput
            | Kind::TextArea
            | Kind::ListBox
            | Kind::Tabs
            | Kind::SpinBox
            | Kind::Table
            | Kind::Tree
    )
}

fn set_text(id: WidgetId, e: &Entry, t: &str) {
    match e.kind {
        Kind::Window => vm!(e.obj, "setTitle:", Id: ns(t)),
        Kind::Label | Kind::TextInput | Kind::PasswordInput => {
            if from_ns(idm!(e.obj, "stringValue")) != t {
                vm!(e.obj, "setStringValue:", Id: ns(t));
            }
        }
        Kind::Button | Kind::CheckBox | Kind::RadioButton | Kind::GroupBox => {
            vm!(e.obj, "setTitle:", Id: ns(&crate::mnemonic::strip_mnemonic(t)))
        }
        Kind::TextArea => {
            if from_ns(idm!(e.aux, "string")) != t {
                vm!(e.aux, "setString:", Id: ns(t));
            }
        }
        Kind::Page => vm!(e.aux, "setLabel:", Id: ns(t)),
        Kind::Menu => {
            let t = &crate::mnemonic::strip_mnemonic(t);
            vm!(e.obj, "setTitle:", Id: ns(t));
            vm!(e.aux, "setTitle:", Id: ns(t));
            // An app-defined "Edit" menu replaces the built-in one.
            if t.trim().eq_ignore_ascii_case("edit") {
                let bar = idm!(e.aux, "menu");
                let edit = st(|s| {
                    let w = s
                        .menubars
                        .iter()
                        .find(|(_, m)| **m == bar)
                        .map(|(w, _)| *w)?;
                    let mb = s
                        .ents
                        .iter()
                        .find(|(_, x)| x.kind == Kind::MenuBar && x.obj == bar)
                        .map(|(i, _)| *i)?;
                    let _ = w;
                    s.std_edit.remove(&mb)
                });
                if let Some(it) = edit.filter(|p| !p.is_null()) {
                    vm!(bar, "removeItem:", Id: it);
                }
            }
        }
        Kind::MenuItem | Kind::CheckMenuItem => {
            vm!(e.obj, "setTitle:", Id: ns(&crate::mnemonic::strip_mnemonic(t)))
        }
        _ => {}
    }
    let _ = id;
}

fn set_enabled(id: WidgetId, e: &Entry, en: bool) {
    st(|s| {
        if en {
            s.disabled.remove(&id);
        } else {
            s.disabled.insert(id);
        }
    });
    match e.kind {
        Kind::TextArea => {
            let ro = st(|s| s.readonly.contains(&id));
            vm!(e.aux, "setEditable:", u8: b(en && !ro));
            vm!(e.aux, "setSelectable:", u8: b(en));
        }
        Kind::ListBox | Kind::Table | Kind::Tree => vm!(e.aux, "setEnabled:", u8: b(en)),
        Kind::SpinBox => {
            vm!(e.aux, "setEnabled:", u8: b(en));
            vm!(e.aux2, "setEnabled:", u8: b(en));
        }
        k if k == Kind::Window
            || !(is_view_kind(k) || matches!(k, Kind::MenuItem | Kind::CheckMenuItem)) => {}
        _ => {
            if responds(e.obj, "setEnabled:") {
                vm!(e.obj, "setEnabled:", u8: b(en));
            }
        }
    }
}

fn set_visible(id: WidgetId, e: &Entry, v: bool) {
    match e.kind {
        Kind::Window => {
            if v {
                let first = st(|s| s.shown.insert(id));
                if first && !st(|s| s.placed.contains(&id)) {
                    vm!(e.obj, "center");
                }
                install_menubar(id);
                vm!(e.obj, "makeKeyAndOrderFront:", Id: NIL);
            } else {
                vm!(e.obj, "orderOut:", Id: NIL);
            }
        }
        Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
            if responds(e.obj, "setHidden:") {
                vm!(e.obj, "setHidden:", u8: b(!v));
            }
        }
        Kind::Menu => {
            if responds(e.aux, "setHidden:") {
                vm!(e.aux, "setHidden:", u8: b(!v));
            }
        }
        Kind::Page | Kind::MenuBar | Kind::PopupMenu => {}
        _ => vm!(e.obj, "setHidden:", u8: b(!v)),
    }
}

/// Set the client size around the top-left corner (frame origins are bottom-left based and
/// GNUstep keeps the bottom-left fixed, which would make the window jump).
fn resize_window(win: Id, w: f64, h: f64) {
    let f = rect_of!(win, "frame");
    vm!(win, "setContentSize:", NSSize: NSSize { w, h });
    vm!(win, "setFrameTopLeftPoint:", NSPoint: NSPoint { x: f.x, y: f.y + f.h });
}

/// Largest content size a resizable window may take (AppKit's default maximum is about this).
#[cfg(rungui_gnustep)]
const MAX_CONTENT: f64 = 1e7;

/// Make a window user-resizable or not. AppKit toggles the resizable style-mask bit; GNUstep raises
/// an exception for `setStyleMask:` on a live window, so there a fixed window has its minimum and
/// maximum content size pinned to its current size instead (see `pin_size`).
fn set_resizable(id: WidgetId, e: &Entry, resizable: bool) {
    #[cfg(not(rungui_gnustep))]
    {
        let _ = id;
        vm!(e.obj, "setStyleMask:", usize: if resizable { 15 } else { 7 });
    }
    #[cfg(rungui_gnustep)]
    {
        st(|s| {
            if resizable {
                s.fixed.remove(&id)
            } else {
                s.fixed.insert(id)
            }
        });
        if resizable {
            let min = st(|s| s.min_size.get(&id).copied()).unwrap_or_default();
            vm!(e.obj, "setContentMinSize:", NSSize: min);
            vm!(e.obj, "setContentMaxSize:", NSSize: NSSize { w: MAX_CONTENT, h: MAX_CONTENT });
        } else {
            let cur = client_size(e);
            pin_size(e.obj, cur.w, cur.h);
        }
    }
}

/// Pin a window's content size (min = max = `w` x `h`).
#[cfg(rungui_gnustep)]
fn pin_size(win: Id, w: f64, h: f64) {
    let size = NSSize { w, h };
    vm!(win, "setContentMinSize:", NSSize: size);
    vm!(win, "setContentMaxSize:", NSSize: size);
}

fn set_bounds(e: &Entry, id: WidgetId, r: Rect) {
    match e.kind {
        Kind::Window => {
            let (w, h) = (r.w.max(1) as f64, r.h.max(1) as f64);
            #[cfg(rungui_gnustep)]
            if st(|s| s.fixed.contains(&id)) {
                pin_size(e.obj, w, h); // the core resizes it; the user cannot
            }
            let _ = id;
            resize_window(e.obj, w, h)
        }
        k if is_view_kind(k) => {
            vm!(e.obj, "setFrame:", NSRect: rect_from(r));
            match k {
                Kind::SpinBox => {
                    let sw = 19.0;
                    let (w, h) = (r.w.max(0) as f64, r.h.max(0) as f64);
                    vm!(e.aux, "setFrame:", NSRect: rect(0.0, 0.0, (w - sw).max(0.0), h));
                    vm!(e.aux2, "setFrame:", NSRect: rect((w - sw).max(0.0), 0.0, sw, h));
                }
                Kind::ListBox | Kind::Tree => {
                    vm!(e.aux, "sizeLastColumnToFit");
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Fixed-pitch (or the control's normal) font for text controls.
fn set_monospace(e: &Entry, mono: bool) {
    let (target, area) = match e.kind {
        Kind::TextArea => (e.aux, true),
        Kind::TextInput | Kind::PasswordInput => (e.obj, false),
        _ => return,
    };
    // Size 0 selects the class default size.
    let font = match (mono, area) {
        (true, _) => idm!(cls("NSFont"), "userFixedPitchFontOfSize:", f64: 0.0),
        (false, true) => idm!(cls("NSFont"), "userFontOfSize:", f64: 0.0),
        (false, false) => idm!(cls("NSFont"), "systemFontOfSize:", f64: 0.0),
    };
    if !font.is_null() {
        vm!(target, "setFont:", Id: font);
    }
}

/// TextArea soft wrap: wrapping tracks the text view width; otherwise the container is
/// unbounded, the view grows horizontally and the scroll view shows a horizontal scroller.
fn set_wrap(e: &Entry, wrap: bool) {
    let (sv, tv) = (e.obj, e.aux);
    let tc = idm!(tv, "textContainer");
    if tc.is_null() {
        return;
    }
    let huge = 1e7;
    if wrap {
        vm!(sv, "setHasHorizontalScroller:", u8: 0);
        vm!(tv, "setHorizontallyResizable:", u8: 0);
        let cs = send!(NSSize, sv, "contentSize");
        let f = rect_of!(tv, "frame");
        vm!(tv, "setFrame:", NSRect: rect(f.x, f.y, cs.w, f.h));
        vm!(tc, "setWidthTracksTextView:", u8: 1);
        vm!(tc, "setContainerSize:", NSSize: NSSize { w: cs.w, h: huge });
    } else {
        vm!(tc, "setWidthTracksTextView:", u8: 0);
        vm!(tc, "setContainerSize:", NSSize: NSSize { w: huge, h: huge });
        vm!(tv, "setMaxSize:", NSSize: NSSize { w: huge, h: huge });
        vm!(tv, "setHorizontallyResizable:", u8: 1);
        vm!(sv, "setHasHorizontalScroller:", u8: 1);
    }
}

fn set_items(id: WidgetId, e: &Entry, items: &[String]) {
    st(|s| s.items.insert(id, items.to_vec()));
    match e.kind {
        Kind::ComboBox => {
            vm!(e.obj, "removeAllItems");
            let menu = idm!(e.obj, "menu");
            vm!(menu, "setAutoenablesItems:", u8: 0);
            for it in items {
                // via the menu so that duplicate titles are kept
                let mi = new_item(it, null_mut(), "");
                vm!(menu, "addItem:", Id: mi);
                release(mi);
            }
        }
        Kind::ListBox => {
            vm!(e.aux, "reloadData");
        }
        _ => {}
    }
}

fn set_image(id: WidgetId, e: &Entry, img: Option<&ImageData>) {
    if e.kind != Kind::Image {
        return;
    }
    let Some(img) = img.filter(|i| i.is_valid()) else {
        vm!(e.obj, "setImage:", Id: NIL);
        st(|s| s.imgsz.remove(&id));
        return;
    };
    let rep = idm!(
        idm!(cls("NSBitmapImageRep"), "alloc"),
        "initWithBitmapDataPlanes:pixelsWide:pixelsHigh:bitsPerSample:samplesPerPixel:hasAlpha:isPlanar:colorSpaceName:bytesPerRow:bitsPerPixel:",
        *mut *mut u8: null_mut(),
        isize: img.w as isize,
        isize: img.h as isize,
        isize: 8,
        isize: 4,
        u8: 1,
        u8: 0,
        Id: ns("NSCalibratedRGBColorSpace"),
        isize: img.w as isize * 4,
        isize: 32
    );
    if rep.is_null() {
        return;
    }
    let data = send!(*mut u8, rep, "bitmapData");
    if data.is_null() {
        release(rep);
        return;
    }
    unsafe { std::ptr::copy_nonoverlapping(img.rgba.as_ptr(), data, img.rgba.len()) };
    let image = idm!(
        idm!(cls("NSImage"), "alloc"),
        "initWithSize:",
        NSSize: NSSize { w: img.w as f64, h: img.h as f64 }
    );
    vm!(image, "addRepresentation:", Id: rep);
    release(rep);
    vm!(e.obj, "setImage:", Id: image);
    release(image);
    st(|s| s.imgsz.insert(id, Size::new(img.w as i32, img.h as i32)));
}

fn set_columns(e: &Entry, cols: &[Column]) {
    let tv = e.aux;
    let existing = idm!(tv, "tableColumns");
    let n = send!(usize, existing, "count");
    let old: Vec<Id> = (0..n)
        .map(|i| idm!(existing, "objectAtIndex:", usize: i))
        .collect();
    for c in old {
        vm!(tv, "removeTableColumn:", Id: c);
    }
    for (i, c) in cols.iter().enumerate() {
        let col = idm!(idm!(cls("NSTableColumn"), "alloc"), "initWithIdentifier:", Id: ns(&i.to_string()));
        let hc = idm!(col, "headerCell");
        if !hc.is_null() {
            vm!(hc, "setStringValue:", Id: ns(&c.title));
        }
        vm!(col, "setWidth:", f64: c.width.max(1) as f64);
        let cell = idm!(col, "dataCell");
        if !cell.is_null() {
            vm!(cell, "setEditable:", u8: 0);
            let align = match c.align {
                ColumnAlign::Left => 0usize,
                ColumnAlign::Right => 1,
                ColumnAlign::Center => 2,
            };
            vm!(cell, "setAlignment:", usize: align);
        }
        vm!(tv, "addTableColumn:", Id: col);
        release(col);
    }
}

fn set_sort_indicator(e: &Entry, si: Option<(usize, bool)>) {
    let tv = e.aux;
    // GNUstep declares this method but only logs "not implemented".
    if cfg!(rungui_gnustep) || !responds(tv, "setIndicatorImage:inTableColumn:") {
        return;
    }
    let existing = idm!(tv, "tableColumns");
    let n = send!(usize, existing, "count");
    for i in 0..n {
        let col = idm!(existing, "objectAtIndex:", usize: i);
        let img = match si {
            Some((c, asc)) if c == i => {
                idm!(cls("NSImage"), "imageNamed:", Id: ns(if asc { "NSAscendingSortIndicator" } else { "NSDescendingSortIndicator" }))
            }
            _ => NIL,
        };
        vm!(tv, "setIndicatorImage:inTableColumn:", Id: img, Id: col);
    }
}

fn set_tree_rows(id: WidgetId, e: &Entry, rows: &[TreeRow]) {
    let mut m = TreeModel::default();
    let mut stack: Vec<usize> = vec![]; // node index per depth
    for r in rows {
        let obj = idm!(cls("NSNumber"), "numberWithUnsignedLongLong:", u64: r.node);
        idm!(obj, "retain");
        let idx = m.nodes.len();
        m.nodes.push(TNode {
            text: r.text.clone(),
            expanded: r.expanded,
            has_children: r.has_children,
            children: vec![],
            obj,
        });
        m.by_id.insert(r.node, idx);
        stack.truncate(r.depth as usize);
        match stack.last() {
            Some(p) => m.nodes[*p].children.push(idx),
            None => m.roots.push(idx),
        }
        stack.push(idx);
    }
    let expanded: Vec<Id> = m
        .nodes
        .iter()
        .filter(|n| n.expanded)
        .map(|n| n.obj)
        .collect();
    let old = st(|s| s.trees.insert(id, m));
    vm!(e.aux, "reloadData");
    for item in expanded {
        vm!(e.aux, "expandItem:", Id: item);
    }
    if let Some(old) = old {
        old.nodes.iter().for_each(|n| autorelease(n.obj));
    }
}
