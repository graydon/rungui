//! The Cocoa backend proper. `#![forbid(unsafe_code)]`: every AppKit call that objc2 declares
//! `unsafe` goes through a wrapper in [`super::unsafe_calls`].
//!
//! Verification status: type-checked for both apple-darwin targets (`cargo check`), and run for real
//! on GNUstep + libobjc2 (scripts/smoke-gnustep.sh). Not run on Apple's frameworks: anything where
//! AppKit and GNUstep differ (see the `rungui_gnustep` blocks) is checked by reading only.
#![forbid(unsafe_code)]
#![allow(deprecated)]

#[allow(clippy::unsafe_removed_from_name)]
use super::unsafe_calls as uc;
use crate::backend::*;
use crate::core;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, extern_methods, sel};
use objc2_app_kit::*;
use objc2_foundation::*;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::panic::{AssertUnwindSafe, catch_unwind};

// ---------------------------------------------------------------- small helpers

/// Run-loop mode name used by the GNUstep hand-rolled loop (Apple's `run` needs none).
#[cfg(rungui_gnustep)]
const MODE_DEFAULT: &str = "NSDefaultRunLoopMode";

/// UTF-8 to NSString (interior NULs are dropped, as the C-string based original did).
fn ns(s: &str) -> Retained<NSString> {
    if s.contains('\0') {
        NSString::from_str(&s.replace('\0', ""))
    } else {
        NSString::from_str(s)
    }
}
fn from_ns(s: &NSString) -> String {
    s.to_string()
}
/// Address of an Objective-C object: the key of the sender -> widget table.
fn addr<T>(o: &T) -> usize {
    o as *const T as *const () as usize
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
fn rect_from(r: Rect) -> NSRect {
    rect(r.x as f64, r.y as f64, r.w.max(0) as f64, r.h.max(0) as f64)
}
/// Every callback and every backend entry point runs on the main thread (backend rule 1).
fn mt() -> MainThreadMarker {
    MainThreadMarker::new().expect("rungui cocoa backend used off the main thread")
}

// ---------------------------------------------------------------- classes

define_class!(
    /// Target, delegate and data source of every native object.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RunguiTarget"]
    pub(super) struct RunguiTarget;

    impl RunguiTarget {
        #[unsafe(method(runguiWake:))]
        fn wake_sel(&self, _a: Option<&AnyObject>) {
            let _ = catch_unwind(core::drain_posted);
        }

        /// Releases the objects of destroyed widgets (see `bury`).
        #[unsafe(method(runguiBury:))]
        fn bury_sel(&self, _a: Option<&AnyObject>) {
            BURY_PENDING.with(|p| p.set(false));
            let dead = GRAVEYARD.with(|g| std::mem::take(&mut *g.borrow_mut()));
            drop(dead);
        }

        #[unsafe(method(runguiQuit:))]
        fn quit_sel(&self, _a: Option<&AnyObject>) {
            let _ = catch_unwind(<Cocoa as Backend>::quit);
        }

        #[unsafe(method(runguiTimer:))]
        fn timer_sel(&self, timer: &NSTimer) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                if let Some(tok) = st(|s| s.trev.get(&addr(timer)).copied()) {
                    core::timer_fired(tok);
                }
            }));
        }

        /// Button, check box, radio, menu item, slider, combo, spin box.
        #[unsafe(method(runguiAction:))]
        fn action_sel(&self, sender: &AnyObject) {
            on_action(sender);
        }

        /// Table / list / tree double click.
        #[unsafe(method(runguiDouble:))]
        fn double_sel(&self, sender: &AnyObject) {
            on_double(sender);
        }

        // ---- text editing (NSTextField / NSTextView)
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_changed(&self, n: &NSNotification) {
            on_text_changed(n);
        }
        #[unsafe(method(textDidChange:))]
        fn text_changed(&self, n: &NSNotification) {
            on_text_changed(n);
        }
        #[unsafe(method(controlTextDidBeginEditing:))]
        fn control_begin_edit(&self, n: &NSNotification) {
            on_begin_edit(n);
        }
        #[unsafe(method(textDidBeginEditing:))]
        fn text_begin_edit(&self, n: &NSNotification) {
            on_begin_edit(n);
        }
        #[unsafe(method(controlTextDidEndEditing:))]
        fn control_end_edit(&self, n: &NSNotification) {
            on_end_edit(n);
        }
        #[unsafe(method(textDidEndEditing:))]
        fn text_end_edit(&self, n: &NSNotification) {
            on_end_edit(n);
        }

        // ---- window delegate
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, win: &NSWindow) -> bool {
            guarded(|| {
                if let Some((id, _)) = lookup(addr(win)) {
                    core::close_requested(id);
                }
            });
            false
        }
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, n: &NSNotification) {
            on_resized(n);
        }
        #[unsafe(method(windowDidMove:))]
        fn window_did_move(&self, n: &NSNotification) {
            on_moved(n);
        }
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, n: &NSNotification) {
            on_became_key(n);
        }

        // ---- tab view delegate
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn tab_selected(&self, tv: &NSTabView, item: Option<&NSTabViewItem>) {
            guarded(|| {
                let Some((id, _)) = lookup(addr(tv)) else {
                    return;
                };
                let Some(item) = item else { return };
                let i = tv.indexOfTabViewItem(item);
                if i >= 0 {
                    core::event(id, Event::Selected(Some(i as usize)));
                }
            });
        }

        // ---- table / list data source and delegate
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, tv: &NSTableView) -> NSInteger {
            let Some((id, e)) = lookup(addr(tv)) else {
                return 0;
            };
            st(|s| match e.kind {
                Kind::Table => s.rows.get(&id).map_or(0, |r| r.len()),
                _ => s.items.get(&id).map_or(0, |r| r.len()),
            }) as NSInteger
        }

        #[unsafe(method_id(tableView:objectValueForTableColumn:row:))]
        fn table_value(
            &self,
            tv: &NSTableView,
            col: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<AnyObject>> {
            let text = (|| {
                let (id, e) = lookup(addr(tv))?;
                let row = row.max(0) as usize;
                st(|s| match e.kind {
                    Kind::Table => s
                        .rows
                        .get(&id)
                        .and_then(|r| r.get(row))
                        .and_then(|r| r.get(col.map_or(0, column_index)))
                        .cloned(),
                    _ => s.items.get(&id).and_then(|v| v.get(row)).cloned(),
                })
            })();
            Some(any(ns(&text.unwrap_or_default())))
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn table_selection(&self, n: &NSNotification) {
            guarded(|| {
                let Some(o) = note_object(n) else { return };
                let Some((id, _)) = lookup(addr(&*o)) else {
                    return;
                };
                let Some(tv) = o.downcast_ref::<NSTableView>() else {
                    return;
                };
                let row = tv.selectedRow();
                core::event(id, Event::Selected((row >= 0).then_some(row as usize)));
            });
        }

        #[unsafe(method(tableView:didClickTableColumn:))]
        fn table_column_clicked(&self, tv: &NSTableView, col: &NSTableColumn) {
            guarded(|| {
                if let Some((id, e)) = lookup(addr(tv)) {
                    if e.kind == Kind::Table {
                        core::event(id, Event::ColumnClicked(column_index(col)));
                    }
                }
            });
        }

        // ---- outline view data source and delegate
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn ov_count(&self, ov: &NSOutlineView, item: Option<&AnyObject>) -> NSInteger {
            let Some((id, _)) = lookup(addr(ov)) else {
                return 0;
            };
            let key = item.and_then(tree_node_of);
            st(|s| {
                let Some(m) = s.trees.get(&id) else { return 0 };
                match (item, key) {
                    (None, _) => m.roots.len(),
                    (Some(_), Some(k)) => m
                        .by_id
                        .get(&k)
                        .and_then(|i| m.nodes.get(*i))
                        .map_or(0, |n| n.children.len()),
                    _ => 0,
                }
            }) as NSInteger
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn ov_child(
            &self,
            ov: &NSOutlineView,
            index: NSInteger,
            item: Option<&AnyObject>,
        ) -> Option<Retained<AnyObject>> {
            outline_child(ov, index, item)
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn ov_expandable(&self, ov: &NSOutlineView, item: &AnyObject) -> bool {
            item_expandable(ov, item)
        }

        #[unsafe(method_id(outlineView:objectValueForTableColumn:byItem:))]
        fn ov_value(
            &self,
            ov: &NSOutlineView,
            _col: Option<&NSTableColumn>,
            item: Option<&AnyObject>,
        ) -> Option<Retained<AnyObject>> {
            let text = (|| {
                let (id, _) = lookup(addr(ov))?;
                let k = tree_node_of(item?)?;
                st(|s| {
                    s.trees.get(&id).and_then(|m| {
                        m.by_id
                            .get(&k)
                            .and_then(|i| m.nodes.get(*i))
                            .map(|n| n.text.clone())
                    })
                })
            })();
            Some(any(ns(&text.unwrap_or_default())))
        }

        #[unsafe(method(outlineViewItemDidExpand:))]
        fn ov_did_expand(&self, n: &NSNotification) {
            ov_expansion(n, true);
        }
        #[unsafe(method(outlineViewItemDidCollapse:))]
        fn ov_did_collapse(&self, n: &NSNotification) {
            ov_expansion(n, false);
        }
        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn ov_selection(&self, n: &NSNotification) {
            guarded(|| {
                let Some(o) = note_object(n) else { return };
                let Some((id, _)) = lookup(addr(&*o)) else {
                    return;
                };
                let Some(ov) = o.downcast_ref::<NSOutlineView>() else {
                    return;
                };
                let row = ov.selectedRow();
                core::event(id, Event::TreeSelected(outline_item_node(ov, row)));
            });
        }

        /// GNUstep treats command-line paths as files to open and shows an "Alert / No information"
        /// panel when the delegate does not claim them. The app handles its own arguments.
        #[cfg(rungui_gnustep)]
        #[unsafe(method(application:openFile:))]
        fn app_open_file(&self, _app: &NSApplication, _file: &NSString) -> bool {
            true
        }
    }

    unsafe impl NSObjectProtocol for RunguiTarget {}
    unsafe impl NSWindowDelegate for RunguiTarget {}
    unsafe impl NSControlTextEditingDelegate for RunguiTarget {}
    unsafe impl NSTextFieldDelegate for RunguiTarget {}
    unsafe impl NSTextDelegate for RunguiTarget {}
    unsafe impl NSTextViewDelegate for RunguiTarget {}
    unsafe impl NSTabViewDelegate for RunguiTarget {}
    unsafe impl NSTableViewDataSource for RunguiTarget {}
    unsafe impl NSTableViewDelegate for RunguiTarget {}
    unsafe impl NSOutlineViewDataSource for RunguiTarget {}
    unsafe impl NSOutlineViewDelegate for RunguiTarget {}
    #[cfg(rungui_gnustep)]
    unsafe impl NSApplicationDelegate for RunguiTarget {}
);

impl RunguiTarget {
    extern_methods!(
        #[unsafe(method(new))]
        pub(super) fn new(mtm: MainThreadMarker) -> Retained<Self>;
    );
}

define_class!(
    /// A flipped NSView: every container uses it so that coordinates are top-left based.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RunguiFlipView"]
    pub(super) struct RunguiFlipView;

    impl RunguiFlipView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl RunguiFlipView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        fn init_with_frame(this: objc2::rc::Allocated<Self>, frame: NSRect) -> Retained<Self>;
    );
}

impl RunguiFlipView {
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        Self::init_with_frame(mtm.alloc::<Self>(), NSRect::ZERO)
    }
}

define_class!(
    /// The Splitter drag handle: drag, cursor rects, a one point separator line and keyboard operation.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RunguiSash"]
    pub(super) struct RunguiSash;

    impl RunguiSash {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, ev: &NSEvent) {
            guarded(|| {
                let Some((id, _)) = lookup(addr(self)) else {
                    return;
                };
                if st(|s| s.disabled.contains(&id)) {
                    return;
                }
                let o = sash_axis(id);
                let f = self.frame();
                let start = match o {
                    Orientation::Horizontal => f.origin.x,
                    Orientation::Vertical => f.origin.y,
                };
                let ptr = sash_pointer(ev, o);
                st(|s| s.drag = Some((id, ptr, start.round() as i32)));
                // a click focuses the sash so the arrow keys work right after
                if let Some(w) = self.window() {
                    w.makeFirstResponder(Some(self));
                }
            });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, ev: &NSEvent) {
            guarded(|| {
                let Some((id, _)) = lookup(addr(self)) else {
                    return;
                };
                let Some((did, p0, pos0)) = st(|s| s.drag) else {
                    return;
                };
                if did != id {
                    return;
                }
                let delta = sash_pointer(ev, sash_axis(id)) - p0;
                core::event(id, Event::SashDragged(pos0 + delta.round() as i32));
            });
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _ev: &NSEvent) {
            let _ = catch_unwind(AssertUnwindSafe(|| st(|s| s.drag = None)));
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let Some((id, _)) = lookup(addr(self)) else {
                    return;
                };
                let c = match sash_axis(id) {
                    Orientation::Horizontal => NSCursor::resizeLeftRightCursor(),
                    Orientation::Vertical => NSCursor::resizeUpDownCursor(),
                };
                self.addCursorRect_cursor(self.bounds(), &c);
            }));
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let b = self.bounds();
                let Some((id, _)) = lookup(addr(self)) else {
                    return;
                };
                // keyboard focus: the whole handle in the system's focus colour
                if self.has_focus() {
                    NSColor::keyboardFocusIndicatorColor().set();
                    NSRectFill(b);
                }
                // A one point separator line centred in the handle.
                let (x, y, w, h) = (b.origin.x, b.origin.y, b.size.width, b.size.height);
                let line = match sash_axis(id) {
                    Orientation::Horizontal => rect(x + (w / 2.0).floor(), y, 1.0, h),
                    Orientation::Vertical => rect(x, y + (h / 2.0).floor(), w, 1.0),
                };
                NSColor::gridColor().set();
                NSRectFill(line);
            }));
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _ev: Option<&NSEvent>) -> bool {
            true
        }
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // the focus indicator comes or goes; the redraw happens after the window has updated its first responder
            self.setNeedsDisplay(true);
            true
        }
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.setNeedsDisplay(true);
            true
        }

        /// Arrow keys along the sash's axis (Shift = large step), Home and End; any other key goes on
        /// up the responder chain (Tab moves the key view, shortcuts reach the menus).
        #[unsafe(method(keyDown:))]
        fn key_down(&self, ev: &NSEvent) {
            let key = catch_unwind(AssertUnwindSafe(|| sash_key(self, ev)))
                .ok()
                .flatten();
            match key {
                Some((id, key)) => guarded(|| core::event(id, Event::SashKey(key))),
                None => uc::forward_key_down(self, ev),
            }
        }
    }
);

impl RunguiSash {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        fn init_with_frame(this: objc2::rc::Allocated<Self>, frame: NSRect) -> Retained<Self>;
    );
}

impl RunguiSash {
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        Self::init_with_frame(mtm.alloc::<Self>(), NSRect::ZERO)
    }
    fn has_focus(&self) -> bool {
        self.window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| addr(&*r) == addr(self))
    }
}

fn sash_axis(id: WidgetId) -> Orientation {
    st(|s| s.sash_orient.get(&id).copied()).unwrap_or_default()
}
/// Pointer coordinate along the sash's drag axis, in window coordinates but oriented like the
/// (flipped) parent: x grows right, y grows down.
fn sash_pointer(ev: &NSEvent, o: Orientation) -> f64 {
    let p = ev.locationInWindow();
    match o {
        Orientation::Horizontal => p.x,
        Orientation::Vertical => -p.y,
    }
}
fn sash_key(this: &RunguiSash, ev: &NSEvent) -> Option<(WidgetId, SashKey)> {
    let (id, _) = lookup(addr(this))?;
    if st(|s| s.disabled.contains(&id)) {
        return None;
    }
    let flags = ev.modifierFlags();
    // Control, Option, Command: not ours
    if flags.intersects(
        NSEventModifierFlags::Control
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Command,
    ) {
        return None;
    }
    let big = flags.contains(NSEventModifierFlags::Shift);
    let typed = ev.charactersIgnoringModifiers()?;
    let chars = from_ns(&typed);
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
}

/// Upcast any retained object to `Retained<AnyObject>` (for `id`-typed returns).
fn any<T: ClassType + 'static>(o: Retained<T>) -> Retained<AnyObject> {
    o.into()
}

// ---------------------------------------------------------------- state

/// The typed native objects behind one widget id.
#[derive(Clone)]
enum Native {
    Window {
        win: Retained<NSWindow>,
        cont: Retained<RunguiFlipView>,
    },
    Label(Retained<NSTextField>),
    /// TextInput / PasswordInput
    Text(Retained<NSTextField>),
    Sash(Retained<RunguiSash>),
    /// Button / CheckBox / RadioButton
    Button(Retained<NSButton>),
    TextArea {
        sv: Retained<NSScrollView>,
        tv: Retained<NSTextView>,
    },
    Combo(Retained<NSPopUpButton>),
    /// ListBox / Table
    Grid {
        sv: Retained<NSScrollView>,
        tv: Retained<NSTableView>,
    },
    Tree {
        sv: Retained<NSScrollView>,
        tv: Retained<NSOutlineView>,
    },
    Slider(Retained<NSSlider>),
    Progress(Retained<NSProgressIndicator>),
    Spin {
        c: Retained<RunguiFlipView>,
        tf: Retained<NSTextField>,
        stp: Retained<NSStepper>,
    },
    Tabs(Retained<NSTabView>),
    Page {
        item: Retained<NSTabViewItem>,
        view: Retained<RunguiFlipView>,
    },
    Group {
        bx: Retained<NSBox>,
        cont: Retained<RunguiFlipView>,
    },
    Image(Retained<NSImageView>),
    MenuBar {
        menu: Retained<NSMenu>,
        /// built-in Edit menu item (removed when the app defines its own "Edit" menu)
        edit: Option<Retained<NSMenuItem>>,
    },
    Menu {
        menu: Retained<NSMenu>,
        item: Retained<NSMenuItem>,
    },
    Popup(Retained<NSMenu>),
    /// MenuItem / CheckMenuItem / MenuSeparator
    Item(Retained<NSMenuItem>),
}

#[derive(Clone)]
struct Entry {
    kind: Kind,
    n: Native,
}

impl Entry {
    /// The main NSView of view kinds (what is added to the parent's container).
    fn view(&self) -> Option<&NSView> {
        Some(match &self.n {
            Native::Label(v) | Native::Text(v) => v,
            Native::Sash(v) => v,
            Native::Button(v) => v,
            Native::TextArea { sv, .. } | Native::Grid { sv, .. } | Native::Tree { sv, .. } => sv,
            Native::Combo(v) => v,
            Native::Slider(v) => v,
            Native::Progress(v) => v,
            Native::Spin { c, .. } => c,
            Native::Tabs(v) => v,
            Native::Group { bx, .. } => bx,
            Native::Image(v) => v,
            _ => return None,
        })
    }
    /// The view that receives children: content view of a window, page view, group content view.
    fn cont(&self) -> Option<&NSView> {
        match &self.n {
            Native::Window { cont, .. } | Native::Group { cont, .. } => Some(cont),
            Native::Page { view, .. } => Some(view),
            _ => self.view(),
        }
    }
    fn win(&self) -> Option<&NSWindow> {
        match &self.n {
            Native::Window { win, .. } => Some(win),
            _ => None,
        }
    }
    fn table(&self) -> Option<&NSTableView> {
        match &self.n {
            Native::Grid { tv, .. } => Some(tv),
            Native::Tree { tv, .. } => Some(tv),
            _ => None,
        }
    }
    fn menu_item(&self) -> Option<&NSMenuItem> {
        match &self.n {
            Native::Item(i) | Native::Menu { item: i, .. } => Some(i),
            _ => None,
        }
    }
    /// The view that takes keyboard focus / accessibility attributes (the inner control of composites).
    fn focus_view(&self) -> Option<&NSView> {
        Some(match &self.n {
            Native::TextArea { tv, .. } => tv,
            Native::Grid { tv, .. } => tv,
            Native::Tree { tv, .. } => tv,
            Native::Spin { tf, .. } => tf,
            _ => return self.view(),
        })
    }
    /// Addresses under which native callbacks may identify this widget.
    fn keys(&self) -> Vec<usize> {
        match &self.n {
            Native::Window { win, .. } => vec![addr(&**win)],
            Native::Label(v) | Native::Text(v) => vec![addr(&**v)],
            Native::Sash(v) => vec![addr(&**v)],
            Native::Button(v) => vec![addr(&**v)],
            Native::TextArea { sv, tv } => vec![addr(&**sv), addr(&**tv)],
            Native::Combo(v) => vec![addr(&**v)],
            Native::Grid { sv, tv } => vec![addr(&**sv), addr(&**tv)],
            Native::Tree { sv, tv } => vec![addr(&**sv), addr(&**tv)],
            Native::Slider(v) => vec![addr(&**v)],
            Native::Progress(v) => vec![addr(&**v)],
            Native::Spin { c, tf, stp } => vec![addr(&**c), addr(&**tf), addr(&**stp)],
            Native::Tabs(v) => vec![addr(&**v)],
            Native::Page { item, view } => vec![addr(&**view), addr(&**item)],
            Native::Group { bx, .. } => vec![addr(&**bx)],
            Native::Image(v) => vec![addr(&**v)],
            Native::MenuBar { menu, .. } => vec![addr(&**menu)],
            Native::Menu { menu, item } => vec![addr(&**menu), addr(&**item)],
            Native::Popup(m) => vec![addr(&**m)],
            Native::Item(i) => vec![addr(&**i)],
        }
    }
}

/// Model of an NSOutlineView: items are NSNumbers holding the core's node id.
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
    obj: Retained<NSNumber>,
}

#[derive(Default)]
struct State {
    rows: HashMap<WidgetId, Vec<Vec<String>>>,
    trees: HashMap<WidgetId, TreeModel>,
    ents: HashMap<WidgetId, Entry>,
    /// native object address -> widget
    rev: HashMap<usize, WidgetId>,
    items: HashMap<WidgetId, Vec<String>>,
    range: HashMap<WidgetId, (f64, f64, f64)>,
    imgsz: HashMap<WidgetId, Size>,
    readonly: HashSet<WidgetId>,
    disabled: HashSet<WidgetId>,
    /// window -> its NSMenu bar
    menubars: HashMap<WidgetId, Retained<NSMenu>>,
    shown: HashSet<WidgetId>,
    timers: HashMap<u64, Retained<NSTimer>>,
    trev: HashMap<usize, u64>,
    /// sash -> orientation (Horizontal splitter = vertical sash that moves along x)
    sash_orient: HashMap<WidgetId, Orientation>,
    /// active sash drag: (sash, pointer coordinate at press along the drag axis, sash position at press)
    drag: Option<(WidgetId, f64, i32)>,
    /// windows the app positioned explicitly: not centered on first show
    placed: HashSet<WidgetId>,
    target: Option<Retained<RunguiTarget>>,
    app: Option<Retained<NSApplication>>,
    app_name: String,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
    static QUIET: Cell<u32> = const { Cell::new(0) };
    /// Native objects of destroyed widgets, released one run-loop turn later: the destroy may come
    /// from inside one of the object's own action callbacks.
    static GRAVEYARD: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };
    /// A `runguiBury:` is already queued.
    static BURY_PENDING: Cell<bool> = const { Cell::new(false) };
}
/// GNUstep only: set by `quit` to leave the hand-rolled event loop in `run`.
#[cfg(rungui_gnustep)]
static QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn st<R>(f: impl FnOnce(&mut State) -> R) -> R {
    S.with(|s| f(&mut s.borrow_mut()))
}
fn ent(id: WidgetId) -> Option<Entry> {
    st(|s| s.ents.get(&id).cloned())
}
fn lookup(p: usize) -> Option<(WidgetId, Entry)> {
    st(|s| {
        let id = *s.rev.get(&p)?;
        Some((id, s.ents.get(&id)?.clone()))
    })
}
fn target() -> Option<Retained<RunguiTarget>> {
    st(|s| s.target.clone())
}
fn app() -> Option<Retained<NSApplication>> {
    st(|s| s.app.clone())
}
/// Keep `o` alive until the run loop has turned once.
fn bury<T: 'static>(o: Retained<T>) {
    GRAVEYARD.with(|g| g.borrow_mut().push(Box::new(o)));
    if !BURY_PENDING.with(|p| p.replace(true)) {
        uc::perform_on_main(sel!(runguiBury:));
    }
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

// ---------------------------------------------------------------- callbacks (bodies)

fn note_object(n: &NSNotification) -> Option<Retained<AnyObject>> {
    n.object()
}

fn parse_spin(text: &str, id: WidgetId) -> Option<f64> {
    let v: f64 = text.trim().parse().ok()?;
    let (lo, hi, _) = st(|s| s.range.get(&id).copied()).unwrap_or((0.0, 100.0, 1.0));
    Some(v.clamp(lo.min(hi), hi.max(lo)))
}

fn set_spin(tf: &NSTextField, stp: &NSStepper, v: f64) {
    let _q = Quiet::new();
    stp.setDoubleValue(v);
    tf.setStringValue(&ns(&format!("{v}")));
}

fn on_action(sender: &AnyObject) {
    guarded(|| {
        let Some((id, e)) = lookup(addr(sender)) else {
            return;
        };
        match (&e.n, e.kind) {
            (_, Kind::Button | Kind::MenuItem) => core::event(id, Event::Click),
            (Native::Button(b), Kind::CheckBox | Kind::RadioButton) => {
                core::event(id, Event::Toggled(b.state() == 1));
            }
            (Native::Item(item), Kind::CheckMenuItem) => {
                let on = item.state() != 1;
                {
                    let _q = Quiet::new();
                    item.setState(on as NSInteger);
                }
                core::event(id, Event::Toggled(on));
            }
            (Native::Slider(sl), Kind::Slider) => {
                let mut v = sl.doubleValue();
                let (lo, _hi, step) =
                    st(|s| s.range.get(&id).copied()).unwrap_or((0.0, 100.0, 0.0));
                if step > 0.0 {
                    let snapped = lo + ((v - lo) / step).round() * step;
                    if snapped != v {
                        v = snapped;
                        let _q = Quiet::new();
                        sl.setDoubleValue(v);
                    }
                }
                core::event(id, Event::Value(v));
            }
            (Native::Combo(c), Kind::ComboBox) => {
                let i = c.indexOfSelectedItem();
                core::event(id, Event::Selected((i >= 0).then_some(i as usize)));
            }
            (Native::Spin { tf, stp, .. }, Kind::SpinBox) => {
                let v = if addr(sender) == addr(&**stp) {
                    stp.doubleValue()
                } else {
                    match parse_spin(&from_ns(&tf.stringValue()), id) {
                        Some(v) => v,
                        None => {
                            set_spin(tf, stp, stp.doubleValue());
                            return;
                        }
                    }
                };
                set_spin(tf, stp, v);
                core::event(id, Event::Value(v));
            }
            _ => {}
        }
    });
}

fn on_double(sender: &AnyObject) {
    guarded(|| {
        let Some((id, e)) = lookup(addr(sender)) else {
            return;
        };
        let Some(tv) = e.table() else { return };
        let row = tv.clickedRow();
        if row < 0 {
            return;
        }
        match e.kind {
            Kind::ListBox | Kind::Table => core::event(id, Event::Activated(row as usize)),
            Kind::Tree => {
                if let Native::Tree { tv, .. } = &e.n {
                    if let Some(n) = outline_item_node(tv, row) {
                        core::event(id, Event::TreeActivated(n));
                    }
                }
            }
            _ => {}
        }
    });
}

/// controlTextDidChange: (NSTextField) and textDidChange: (NSTextView)
fn on_text_changed(note: &NSNotification) {
    guarded(|| {
        let Some(o) = note_object(note) else { return };
        let Some((id, e)) = lookup(addr(&*o)) else {
            return;
        };
        match &e.n {
            Native::Text(f) => core::event(id, Event::Text(from_ns(&f.stringValue()))),
            Native::TextArea { tv, .. } => core::event(id, Event::Text(from_ns(&tv.string()))),
            _ => {}
        }
    });
}

fn on_begin_edit(note: &NSNotification) {
    guarded(|| {
        if let Some((id, _)) = note_object(note).and_then(|o| lookup(addr(&*o))) {
            core::event(id, Event::Focus(true));
        }
    });
}

fn on_end_edit(note: &NSNotification) {
    guarded(|| {
        let Some(o) = note_object(note) else { return };
        let Some((id, e)) = lookup(addr(&*o)) else {
            return;
        };
        if let Native::Spin { tf, stp, .. } = &e.n {
            if addr(&*o) == addr(&**tf) {
                if let Some(v) = parse_spin(&from_ns(&tf.stringValue()), id) {
                    if v != stp.doubleValue() {
                        set_spin(tf, stp, v);
                        core::event(id, Event::Value(v));
                    }
                }
            }
        }
        core::event(id, Event::Focus(false));
    });
}

fn client_size(e: &Entry) -> NSSize {
    e.cont().map_or(NSSize::ZERO, |c| c.frame().size)
}

fn on_resized(note: &NSNotification) {
    guarded(|| {
        let Some(o) = note_object(note) else { return };
        let Some((id, e)) = lookup(addr(&*o)) else {
            return;
        };
        let s = client_size(&e);
        core::event(
            id,
            Event::Resized {
                w: s.width.round() as i32,
                h: s.height.round() as i32,
            },
        );
    });
}

fn install_menubar(win: WidgetId) {
    let bar = st(|s| s.menubars.get(&win).cloned());
    if let (Some(bar), Some(app)) = (bar, app()) {
        app.setMainMenu(Some(&bar));
    }
}

fn on_moved(note: &NSNotification) {
    guarded(|| {
        let Some(o) = note_object(note) else { return };
        let Some((id, e)) = lookup(addr(&*o)) else {
            return;
        };
        let Some(win) = e.win() else { return };
        let f = win.frame();
        // Cocoa screen coordinates are bottom-left based; the API reports the outer top-left.
        let y = primary_screen_height() - (f.origin.y + f.size.height);
        core::event(
            id,
            Event::Moved {
                x: f.origin.x.round() as i32,
                y: y.round() as i32,
            },
        );
    });
}

/// Height of the primary screen (the one holding the menu bar, origin of the screen coordinates).
fn primary_screen_height() -> f64 {
    let screens = NSScreen::screens(mt());
    if screens.count() == 0 {
        return 0.0;
    }
    screens.objectAtIndex(0).frame().size.height
}

fn on_became_key(note: &NSNotification) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some((id, _)) = note_object(note).and_then(|o| lookup(addr(&*o))) {
            install_menubar(id);
            if !quiet() {
                core::event(id, Event::Focus(true));
            }
        }
    }));
}

fn column_index(col: &NSTableColumn) -> usize {
    from_ns(&col.identifier()).parse().unwrap_or(0)
}

fn tree_node_of(item: &AnyObject) -> Option<u64> {
    item.downcast_ref::<NSNumber>().map(|n| n.as_u64())
}

fn ov_expansion(note: &NSNotification, open: bool) {
    guarded(|| {
        let Some(o) = note_object(note) else { return };
        let Some((id, _)) = lookup(addr(&*o)) else {
            return;
        };
        let Some(item) = note
            .userInfo()
            .and_then(|info| info.objectForKey(&NSString::from_str("NSObject")))
        else {
            return;
        };
        let Some(node) = tree_node_of(&item) else {
            return;
        };
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

fn outline_item_node(ov: &NSOutlineView, row: isize) -> Option<u64> {
    if row < 0 {
        return None;
    }
    let item = ov.itemAtRow(row)?;
    tree_node_of(&item)
}

fn outline_child(
    ov: &NSOutlineView,
    index: NSInteger,
    item: Option<&AnyObject>,
) -> Option<Retained<AnyObject>> {
    let (id, _) = lookup(addr(ov))?;
    let key = item.and_then(tree_node_of);
    st(|s| {
        let m = s.trees.get(&id)?;
        let list = match (item, key) {
            (None, _) => &m.roots,
            (Some(_), Some(k)) => &m.nodes.get(*m.by_id.get(&k)?)?.children,
            _ => return None,
        };
        Some(any(m
            .nodes
            .get(*list.get(index.max(0) as usize)?)?
            .obj
            .clone()))
    })
}

fn item_expandable(ov: &NSOutlineView, item: &AnyObject) -> bool {
    let Some((id, _)) = lookup(addr(ov)) else {
        return false;
    };
    let Some(k) = tree_node_of(item) else {
        return false;
    };
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
    })
}

/// Called by `RunguiApp.sendEvent:` for every event (after the normal dispatch): right clicks
/// become `Event::ContextMenu`.
pub(super) fn app_event(ev: &NSEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let ty = ev.r#type();
        // control-click is the Mac way to right click.
        let right = ty == NSEventType::RightMouseDown
            || (ty == NSEventType::LeftMouseDown
                && ev.modifierFlags().contains(NSEventModifierFlags::Control));
        if !right {
            return;
        }
        let Some(win) = ev.window(mt()) else { return };
        let Some((wid, we)) = lookup(addr(&*win)) else {
            return;
        };
        let loc = ev.locationInWindow();
        let h = client_size(&we).height;
        let mut v = we.cont().and_then(|c| c.hitTest(loc));
        let mut target = wid;
        while let Some(view) = v {
            if let Some((id, _)) = lookup(addr(&*view)) {
                target = id;
                break;
            }
            v = uc::superview(&view);
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

/// The action handler behind a per-radio selector (see `unsafe_calls::add_action_selector`).
pub(super) fn dispatch_action(sender: &AnyObject) {
    on_action(sender);
}

// ---------------------------------------------------------------- menus & accelerators

const MOD_SHIFT: NSEventModifierFlags = NSEventModifierFlags::Shift;
const MOD_OPTION: NSEventModifierFlags = NSEventModifierFlags::Option;
const MOD_COMMAND: NSEventModifierFlags = NSEventModifierFlags::Command;

fn key_equivalent(a: &Accel) -> Option<(String, NSEventModifierFlags)> {
    let mut mask = NSEventModifierFlags::empty();
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

fn new_menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    let m = NSMenu::initWithTitle(mtm.alloc(), &ns(title));
    m.setAutoenablesItems(false);
    m
}

/// Standard application menu (Quit) as a menu bar item.
fn app_menu_item(mtm: MainThreadMarker, target: &RunguiTarget, name: &str) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&ns(name));
    let menu = new_menu(mtm, name);
    let q = uc::menu_item(mtm, &format!("Quit {name}"), Some(sel!(runguiQuit:)), "q");
    uc::item_target_action(&q, target, Some(sel!(runguiQuit:)));
    menu.addItem(&q);
    item.setSubmenu(Some(&menu));
    item
}

/// Standard Edit menu: first-responder (nil target) actions make copy/paste work in text fields.
fn edit_menu_item(mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    let menu = new_menu(mtm, "Edit");
    let std: [(&str, Option<Sel>, &str, NSEventModifierFlags); 7] = [
        ("Undo", Some(sel!(undo:)), "z", MOD_COMMAND),
        ("Redo", Some(sel!(redo:)), "z", MOD_COMMAND | MOD_SHIFT),
        ("", None, "", NSEventModifierFlags::empty()),
        ("Cut", Some(sel!(cut:)), "x", MOD_COMMAND),
        ("Copy", Some(sel!(copy:)), "c", MOD_COMMAND),
        ("Paste", Some(sel!(paste:)), "v", MOD_COMMAND),
        ("Select All", Some(sel!(selectAll:)), "a", MOD_COMMAND),
    ];
    for (title, action, key, mask) in std {
        let it = if title.is_empty() {
            NSMenuItem::separatorItem(mtm)
        } else {
            let it = uc::menu_item(mtm, title, action, key);
            it.setKeyEquivalentModifierMask(mask);
            it.setEnabled(true);
            it
        };
        menu.addItem(&it);
    }
    item.setSubmenu(Some(&menu));
    item
}

fn new_menubar(
    mtm: MainThreadMarker,
    target: &RunguiTarget,
    name: &str,
    with_edit: bool,
) -> (Retained<NSMenu>, Option<Retained<NSMenuItem>>) {
    let bar = new_menu(mtm, "");
    bar.addItem(&app_menu_item(mtm, target, name));
    let edit = with_edit.then(|| edit_menu_item(mtm));
    if let Some(edit) = &edit {
        bar.addItem(edit);
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

fn label_field(mtm: MainThreadMarker, editable: bool, secure: bool) -> Retained<NSTextField> {
    let f = rect(0.0, 0.0, 100.0, 22.0);
    let f = if secure {
        // NSSecureTextField is an NSTextField subclass.
        Retained::into_super(NSSecureTextField::initWithFrame(mtm.alloc(), f))
    } else {
        NSTextField::initWithFrame(mtm.alloc(), f)
    };
    if !editable {
        f.setEditable(false);
        f.setSelectable(false);
        f.setBezeled(false);
        f.setBordered(false);
        f.setDrawsBackground(false);
    }
    f
}

fn cell_size(c: &NSControl) -> Size {
    let Some(cell) = c.cell() else {
        return Size::default();
    };
    let s = cell.cellSize();
    Size::new(s.width.ceil() as i32, s.height.ceil() as i32)
}

fn new_column(mtm: MainThreadMarker, ident: &str, width: f64) -> Retained<NSTableColumn> {
    let col = NSTableColumn::initWithIdentifier(mtm.alloc(), &ns(ident));
    col.setWidth(width);
    col
}

/// Make the column's data cell read-only (and optionally aligned).
fn style_data_cell(col: &NSTableColumn, align: Option<NSTextAlignment>) {
    if let Ok(cell) = col.dataCell().downcast::<NSCell>() {
        cell.setEditable(false);
        if let Some(a) = align {
            cell.setAlignment(a);
        }
    }
}

fn scroll_view(mtm: MainThreadMarker, w: f64, h: f64, hscroll: bool) -> Retained<NSScrollView> {
    let sv = NSScrollView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, w, h));
    sv.setHasVerticalScroller(true);
    if hscroll {
        sv.setHasHorizontalScroller(true);
    }
    sv.setBorderType(NSBorderType::BezelBorder);
    sv
}

impl Backend for Cocoa {
    fn init(app_name: &str) -> Result<()> {
        let mtm = MainThreadMarker::new().ok_or(Error::NotInitialized)?;
        let target = RunguiTarget::new(mtm);
        // The RunguiApp subclass must be instantiated by the first `sharedApplication`.
        let app = uc::shared_application(mtm);
        if app.respondsToSelector(sel!(setActivationPolicy:)) {
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        }
        let pinfo = NSProcessInfo::processInfo();
        if pinfo.respondsToSelector(sel!(setProcessName:)) {
            pinfo.setProcessName(&ns(app_name));
        }
        let (bar, _) = new_menubar(mtm, &target, app_name, true);
        app.setMainMenu(Some(&bar));
        #[cfg(rungui_gnustep)]
        uc::set_app_delegate(&app, &target);
        uc::publish_target(&target);
        st(|s| {
            s.target = Some(target);
            s.app = Some(app);
            s.app_name = app_name.to_string();
        });
        Ok(())
    }

    fn run() {
        let Some(app) = app() else { return };
        app.activateIgnoringOtherApps(true);
        #[cfg(not(rungui_gnustep))]
        app.run();
        // GNUstep's NSApp -stop: is only noticed after the next X event (a posted dummy event
        // does not wake its wait), so run our own loop with a short timeout instead.
        #[cfg(rungui_gnustep)]
        {
            use std::sync::atomic::Ordering;
            QUIT.store(false, Ordering::SeqCst);
            app.finishLaunching();
            while !QUIT.load(Ordering::SeqCst) {
                objc2::rc::autoreleasepool(|_| {
                    let until = NSDate::dateWithTimeIntervalSinceNow(0.1);
                    if let Some(ev) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        Some(&until),
                        &ns(MODE_DEFAULT),
                        true,
                    ) {
                        app.sendEvent(&ev);
                        app.updateWindows();
                    }
                });
            }
        }
    }

    fn quit() {
        let Some(app) = app() else { return };
        #[cfg(rungui_gnustep)]
        QUIT.store(true, std::sync::atomic::Ordering::SeqCst);
        app.stop(None);
        // `stop:` is only noticed after an event: post a dummy application-defined one.
        if let Some(ev) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        ) {
            app.postEvent_atStart(&ev, true);
        }
    }

    fn wake() {
        // Any thread: `runguiWake:` runs on the main thread in the default, modal and event-tracking modes.
        uc::perform_on_main(sel!(runguiWake:));
    }

    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        let target = target().ok_or(Error::NotInitialized)?;
        let t = uc::scheduled_timer(millis as f64 / 1000.0, &target, sel!(runguiTimer:), repeat);
        // Make it fire while menus/modals/live resizes run too. On Apple platforms the common-modes
        // pseudo mode covers default + event tracking + modal panel; GNUstep lacks it.
        let rl = NSRunLoop::currentRunLoop();
        #[cfg(not(rungui_gnustep))]
        uc::add_timer_mode(&rl, &t, &ns("kCFRunLoopCommonModes")); // == NSRunLoopCommonModes
        #[cfg(rungui_gnustep)]
        {
            uc::add_timer_mode(&rl, &t, &ns("NSEventTrackingRunLoopMode"));
            uc::add_timer_mode(&rl, &t, &ns("NSModalPanelRunLoopMode"));
        }
        st(|s| {
            s.trev.insert(addr(&*t), token);
            s.timers.insert(token, t);
        });
        Ok(())
    }

    fn timer_stop(token: u64) {
        if let Some(t) = st(|s| {
            let t = s.timers.remove(&token)?;
            s.trev.remove(&addr(&*t));
            Some(t)
        }) {
            t.invalidate();
            bury(t);
        }
    }

    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
        let _q = Quiet::new();
        let mtm = MainThreadMarker::new().ok_or(Error::NotInitialized)?;
        let (target, name) = st(|s| (s.target.clone(), s.app_name.clone()));
        let target = target.ok_or(Error::NotInitialized)?;
        let pe = match parent {
            Some(p) => Some(ent(p).ok_or(Error::InvalidHandle)?),
            None => None,
        };
        if kind != Kind::Window && kind != Kind::PopupMenu && pe.is_none() {
            return Err(Error::InvalidHandle);
        }
        let act = sel!(runguiAction:);
        let n = match kind {
            Kind::Window => {
                let win = uc::new_window(
                    mtm,
                    rect(0.0, 0.0, 400.0, 300.0),
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Miniaturizable
                        | NSWindowStyleMask::Resizable,
                );
                win.setDelegate(Some(ProtocolObject::from_ref(&*target)));
                let cont = RunguiFlipView::create(mtm);
                win.setContentView(Some(&cont));
                Native::Window { win, cont }
            }
            Kind::Label => Native::Label(label_field(mtm, false, false)),
            Kind::Sash => Native::Sash(RunguiSash::create(mtm)),
            Kind::TextInput | Kind::PasswordInput => {
                let f = label_field(mtm, true, kind == Kind::PasswordInput);
                uc::text_field_delegate(&f, Some(&target));
                Native::Text(f)
            }
            Kind::Button => {
                let b = NSButton::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 80.0, 24.0));
                b.setButtonType(NSButtonType::MomentaryPushIn);
                b.setBezelStyle(NSBezelStyle::Rounded);
                uc::control_target_action(&b, &target, Some(act));
                Native::Button(b)
            }
            Kind::CheckBox | Kind::RadioButton => {
                let b = NSButton::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 80.0, 20.0));
                let radio = kind == Kind::RadioButton;
                b.setButtonType(if radio {
                    NSButtonType::Radio
                } else {
                    NSButtonType::Switch
                });
                let mut a = act;
                if radio {
                    // AppKit auto-groups radios sharing target+action in one superview; the core
                    // owns exclusivity, so give every radio its own action selector.
                    if let Ok(name) = CString::new(format!("runguiRadio{}:", id.0)) {
                        a = Sel::register(&name);
                        uc::add_action_selector(a);
                    }
                }
                uc::control_target_action(&b, &target, Some(a));
                Native::Button(b)
            }
            Kind::TextArea => {
                let sv = scroll_view(mtm, 200.0, 100.0, false);
                let tv = NSTextView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 200.0, 100.0));
                tv.setMinSize(NSSize::new(0.0, 100.0));
                tv.setMaxSize(NSSize::new(1e7, 1e7));
                tv.setVerticallyResizable(true);
                tv.setHorizontallyResizable(false);
                tv.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                if let Some(tc) = uc::text_container(&tv) {
                    tc.setContainerSize(NSSize::new(200.0, 1e7));
                    tc.setWidthTracksTextView(true);
                }
                tv.setRichText(false);
                tv.setAllowsUndo(true);
                uc::text_view_delegate(&tv, Some(&target));
                sv.setDocumentView(Some(&tv));
                Native::TextArea { sv, tv }
            }
            Kind::ComboBox => {
                let v = NSPopUpButton::initWithFrame_pullsDown(
                    mtm.alloc(),
                    rect(0.0, 0.0, 140.0, 26.0),
                    false,
                );
                uc::control_target_action(&v, &target, Some(act));
                Native::Combo(v)
            }
            Kind::ListBox => {
                let sv = scroll_view(mtm, 160.0, 100.0, false);
                let tv = NSTableView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 160.0, 100.0));
                let col = new_column(mtm, "c", 150.0);
                style_data_cell(&col, None);
                tv.addTableColumn(&col);
                tv.setHeaderView(None);
                tv.setAllowsMultipleSelection(false);
                tv.setAllowsEmptySelection(true);
                uc::wire_table(&tv, Some(&target));
                sv.setDocumentView(Some(&tv));
                Native::Grid { sv, tv }
            }
            Kind::Table => {
                let sv = scroll_view(mtm, 300.0, 150.0, true);
                let tv = NSTableView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 300.0, 150.0));
                tv.setAllowsMultipleSelection(false);
                tv.setAllowsEmptySelection(true);
                uc::wire_table(&tv, Some(&target));
                sv.setDocumentView(Some(&tv));
                Native::Grid { sv, tv }
            }
            Kind::Tree => {
                let sv = scroll_view(mtm, 300.0, 150.0, true);
                let tv = NSOutlineView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 300.0, 150.0));
                tv.setAllowsMultipleSelection(false);
                tv.setAllowsEmptySelection(true);
                uc::wire_outline(&tv, Some(&target));
                tv.setHeaderView(None);
                let col = new_column(mtm, "0", 200.0);
                style_data_cell(&col, None);
                tv.addTableColumn(&col);
                uc::set_outline_column(&tv, &col);
                sv.setDocumentView(Some(&tv));
                Native::Tree { sv, tv }
            }
            Kind::PopupMenu => Native::Popup(new_menu(mtm, "")),
            Kind::Slider => {
                let v = NSSlider::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 150.0, 21.0));
                v.setMinValue(0.0);
                v.setMaxValue(100.0);
                uc::control_target_action(&v, &target, Some(act));
                Native::Slider(v)
            }
            Kind::ProgressBar => {
                let v =
                    NSProgressIndicator::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 150.0, 20.0));
                v.setStyle(NSProgressIndicatorStyle::Bar);
                v.setIndeterminate(false);
                v.setMinValue(0.0);
                v.setMaxValue(1.0);
                Native::Progress(v)
            }
            Kind::SpinBox => {
                let c = RunguiFlipView::create(mtm);
                let tf = label_field(mtm, true, false);
                let stp = NSStepper::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 19.0, 27.0));
                stp.setMinValue(0.0);
                stp.setMaxValue(100.0);
                stp.setIncrement(1.0);
                stp.setValueWraps(false);
                uc::control_target_action(&stp, &target, Some(act));
                uc::control_target_action(&tf, &target, Some(act));
                uc::text_field_delegate(&tf, Some(&target));
                c.addSubview(&tf);
                c.addSubview(&stp);
                Native::Spin { c, tf, stp }
            }
            Kind::Tabs => {
                let v = NSTabView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 300.0, 200.0));
                v.setDelegate(Some(ProtocolObject::from_ref(&*target)));
                Native::Tabs(v)
            }
            Kind::Page => {
                let Some(Native::Tabs(tabs)) = pe.as_ref().map(|p| &p.n) else {
                    return Err(Error::InvalidHandle);
                };
                let item = NSTabViewItem::new();
                let view = RunguiFlipView::create(mtm);
                item.setView(Some(&view));
                {
                    let _q = Quiet::new();
                    tabs.addTabViewItem(&item);
                }
                Native::Page { item, view }
            }
            Kind::GroupBox => {
                let bx = NSBox::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 200.0, 100.0));
                let cont = RunguiFlipView::create(mtm);
                bx.setContentView(Some(&cont));
                Native::Group { bx, cont }
            }
            Kind::Image => {
                let v = NSImageView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, 32.0, 32.0));
                v.setImageScaling(NSImageScaling::ScaleProportionallyDown);
                Native::Image(v)
            }
            Kind::MenuBar => {
                let (menu, edit) = new_menubar(mtm, &target, &name, true);
                let win = parent.unwrap_or(WidgetId::DEAD);
                st(|s| s.menubars.insert(win, menu.clone()));
                if let Some(w) = pe.as_ref().and_then(|p| p.win()) {
                    if w.isKeyWindow() || w.isVisible() {
                        install_menubar(win);
                    }
                }
                Native::MenuBar { menu, edit }
            }
            Kind::Menu => {
                let item = NSMenuItem::new(mtm);
                let menu = new_menu(mtm, "");
                item.setSubmenu(Some(&menu));
                parent_menu(&pe)?.addItem(&item);
                Native::Menu { menu, item }
            }
            Kind::MenuItem | Kind::CheckMenuItem => {
                let it = uc::menu_item(mtm, "", Some(act), "");
                uc::item_target_action(&it, &target, Some(act));
                parent_menu(&pe)?.addItem(&it);
                Native::Item(it)
            }
            Kind::MenuSeparator => {
                let it = NSMenuItem::separatorItem(mtm);
                parent_menu(&pe)?.addItem(&it);
                Native::Item(it)
            }
            _ => return Err(Error::Unsupported),
        };
        let e = Entry { kind, n };
        if is_view_kind(kind) {
            let (Some(pview), Some(view)) = (pe.as_ref().and_then(|p| p.cont()), e.view()) else {
                return Err(Error::InvalidHandle);
            };
            pview.addSubview(view);
        }
        st(|s| {
            for k in e.keys() {
                s.rev.insert(k, id);
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
            s.sash_orient.remove(&id);
            if s.drag.is_some_and(|d| d.0 == id) {
                s.drag = None;
            }
            Some(e)
        }) else {
            return;
        };
        match e.n {
            Native::Window { win, cont } => {
                st(|s| s.menubars.remove(&id));
                win.setDelegate(None);
                win.orderOut(None);
                win.close();
                bury(win);
                bury(cont);
            }
            Native::Popup(m) => bury(m),
            Native::MenuBar { menu, edit } => {
                let same = |m: &Retained<NSMenu>| addr(&**m) == addr(&*menu);
                st(|s| s.menubars.retain(|_, m| !same(m)));
                if let Some(app) = app() {
                    if app.mainMenu().is_some_and(|m| same(&m)) {
                        app.setMainMenu(None);
                    }
                }
                bury(menu);
                if let Some(edit) = edit {
                    bury(edit);
                }
            }
            Native::Menu { item, menu } => {
                if let Some(m) = uc::menu_of(&item) {
                    m.removeItem(&item);
                }
                bury(item);
                bury(menu);
            }
            Native::Item(item) => {
                if let Some(m) = uc::menu_of(&item) {
                    m.removeItem(&item);
                }
                bury(item);
            }
            Native::Page { item, view } => {
                if let Some(tv) = item.tabView(mt()) {
                    tv.removeTabViewItem(&item);
                }
                bury(item);
                bury(view);
            }
            n => {
                let e = Entry { kind: e.kind, n };
                match &e.n {
                    Native::Text(f) => uc::text_field_delegate(f, None),
                    Native::TextArea { tv, .. } => uc::text_view_delegate(tv, None),
                    Native::Tabs(t) => t.setDelegate(None),
                    Native::Spin { tf, .. } => uc::text_field_delegate(tf, None),
                    Native::Grid { tv, .. } => uc::wire_table(tv, None),
                    Native::Tree { tv, .. } => {
                        uc::wire_outline(tv, None);
                        if let Some(m) = st(|s| s.trees.remove(&id)) {
                            m.nodes.into_iter().for_each(|n| bury(n.obj));
                        }
                    }
                    _ => {}
                }
                if let Some(v) = e.view() {
                    v.removeFromSuperview();
                }
                match e.n {
                    Native::Label(v) | Native::Text(v) => bury(v),
                    Native::Sash(v) => bury(v),
                    Native::Button(v) => bury(v),
                    Native::TextArea { sv, tv } => {
                        bury(sv);
                        bury(tv);
                    }
                    Native::Combo(v) => bury(v),
                    Native::Grid { sv, tv } => {
                        bury(sv);
                        bury(tv);
                    }
                    Native::Tree { sv, tv } => {
                        bury(sv);
                        bury(tv);
                    }
                    Native::Slider(v) => bury(v),
                    Native::Progress(v) => bury(v),
                    Native::Spin { c, tf, stp } => {
                        bury(c);
                        bury(tf);
                        bury(stp);
                    }
                    Native::Tabs(v) => bury(v),
                    Native::Group { bx, cont } => {
                        bury(bx);
                        bury(cont);
                    }
                    Native::Image(v) => bury(v),
                    _ => {}
                }
            }
        }
    }

    #[allow(unreachable_patterns)]
    fn set(id: WidgetId, prop: &Prop) {
        let _q = Quiet::new();
        let Some(e) = ent(id) else { return };
        match prop {
            Prop::Text(t) => set_text(&e, t),
            Prop::Tooltip(t) => {
                if is_view_kind(e.kind) {
                    if let Some(v) = e.view() {
                        v.setToolTip(Some(&ns(t)));
                    }
                }
            }
            Prop::Placeholder(t) => {
                if let Native::Text(f) = &e.n {
                    if let Some(cell) = f.cell().and_then(|c| c.downcast::<NSTextFieldCell>().ok())
                    {
                        if cell.respondsToSelector(sel!(setPlaceholderString:)) {
                            cell.setPlaceholderString(Some(&ns(t)));
                        }
                    }
                }
            }
            Prop::Enabled(en) => set_enabled(id, &e, *en),
            Prop::Visible(v) => set_visible(id, &e, *v),
            Prop::Checked(c) => match &e.n {
                Native::Button(b) if matches!(e.kind, Kind::CheckBox | Kind::RadioButton) => {
                    b.setState(*c as NSInteger)
                }
                Native::Item(i) if e.kind == Kind::CheckMenuItem => i.setState(*c as NSInteger),
                _ => {}
            },
            Prop::Value(v) => match &e.n {
                Native::Slider(s) => s.setDoubleValue(*v),
                Native::Progress(p) => p.setDoubleValue(*v),
                Native::Spin { tf, stp, .. } => set_spin(tf, stp, *v),
                _ => {}
            },
            Prop::Range { min, max, step } => {
                st(|s| s.range.insert(id, (*min, *max, *step)));
                match &e.n {
                    Native::Slider(s) => {
                        s.setMinValue(*min);
                        s.setMaxValue(*max);
                    }
                    Native::Spin { stp, .. } => {
                        stp.setMinValue(*min);
                        stp.setMaxValue(*max);
                        stp.setIncrement(if *step > 0.0 { *step } else { 1.0 });
                    }
                    _ => {}
                }
            }
            Prop::Items(items) => set_items(id, &e, items),
            Prop::Selected(sel) => match &e.n {
                Native::Combo(c) => c.selectItemAtIndex(sel.map_or(-1, |i| i as isize)),
                Native::Tabs(t) => {
                    if let Some(i) = sel {
                        t.selectTabViewItemAtIndex(*i as isize);
                    }
                }
                Native::Grid { tv, .. } => match sel {
                    Some(i) => select_row(tv, *i as isize),
                    None => uc::deselect_all(tv),
                },
                _ => {}
            },
            Prop::Bounds(r) => set_bounds(&e, *r),
            Prop::Image(img) => set_image(id, &e, *img),
            Prop::Accel(a) => {
                if let (Some(item), Kind::MenuItem | Kind::CheckMenuItem) = (e.menu_item(), e.kind)
                {
                    let (key, mask) = Accel::parse(a)
                        .and_then(|a| key_equivalent(&a))
                        .unwrap_or_else(|| (String::new(), NSEventModifierFlags::empty()));
                    item.setKeyEquivalent(&ns(&key));
                    item.setKeyEquivalentModifierMask(mask);
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
                match &e.n {
                    Native::Text(f) => f.setEditable(!*ro),
                    Native::TextArea { tv, .. } => tv.setEditable(!*ro),
                    _ => {}
                }
            }
            Prop::Indeterminate(ind) => {
                if let Native::Progress(p) = &e.n {
                    p.setIndeterminate(*ind);
                    if *ind {
                        uc::start_animation(p);
                    } else {
                        uc::stop_animation(p);
                    }
                }
            }
            Prop::Resizable(r) => {
                if let Some(w) = e.win() {
                    let mut m = NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Miniaturizable;
                    if *r {
                        m |= NSWindowStyleMask::Resizable;
                    }
                    w.setStyleMask(m);
                }
            }
            Prop::Columns(cols) => set_columns(mt(), &e, cols),
            Prop::Rows(rows) => {
                st(|s| s.rows.insert(id, rows.to_vec()));
                if let Some(tv) = e.table() {
                    tv.reloadData();
                }
            }
            Prop::SortIndicator(si) => set_sort_indicator(&e, *si),
            Prop::TreeRows(rows) => set_tree_rows(id, &e, rows),
            Prop::TreeSelected(n) => {
                if let Native::Tree { tv, .. } = &e.n {
                    let row = n.and_then(|n| {
                        let item = st(|s| {
                            s.trees.get(&id).and_then(|m| {
                                m.by_id
                                    .get(&n)
                                    .and_then(|i| m.nodes.get(*i))
                                    .map(|n| n.obj.clone())
                            })
                        })?;
                        let r = uc::row_for_item(tv, &item);
                        (r >= 0).then_some(r)
                    });
                    match row {
                        Some(r) => select_row(tv, r),
                        None => uc::deselect_all(tv),
                    }
                }
            }
            Prop::Orientation(o) => {
                if let Native::Sash(s) = &e.n {
                    st(|st| st.sash_orient.insert(id, *o));
                    if let Some(w) = s.window() {
                        w.invalidateCursorRectsForView(s);
                    }
                    s.setNeedsDisplay(true);
                }
            }
            Prop::Monospace(m) => set_monospace(&e, *m),
            Prop::Wrap(w) => {
                if e.kind == Kind::TextArea {
                    set_wrap(&e, *w);
                }
            }
            Prop::Position { x, y } => {
                if let Some(w) = e.win() {
                    st(|s| s.placed.insert(id));
                    let p = NSPoint::new(*x as f64, primary_screen_height() - *y as f64);
                    w.setFrameTopLeftPoint(p);
                }
            }
            Prop::MinSize(sz) => {
                if let Some(w) = e.win() {
                    let (mw, mh) = (sz.w.max(0) as f64, sz.h.max(0) as f64);
                    w.setContentMinSize(NSSize::new(mw, mh));
                    // AppKit only enforces the minimum on the next resize: grow right away, as
                    // the core never lays out below it (and GNUstep would otherwise move the window).
                    let cur = client_size(&e);
                    if cur.width < mw || cur.height < mh {
                        resize_window(w, cur.width.max(mw), cur.height.max(mh));
                    }
                }
            }
            Prop::Focus => {
                if e.kind == Kind::Window || !is_view_kind(e.kind) {
                    return;
                }
                if let Some(v) = e.focus_view() {
                    if let Some(w) = v.window() {
                        w.makeFirstResponder(Some(v));
                    }
                }
            }
            _ => {}
        }
    }

    fn preferred_size(id: WidgetId) -> Size {
        let Some(e) = ent(id) else {
            return Size::default();
        };
        match &e.n {
            Native::Label(c) => {
                let s = cell_size(c);
                Size::new(s.w.max(8), s.h.max(17))
            }
            Native::Button(c) if matches!(e.kind, Kind::CheckBox | Kind::RadioButton) => {
                let s = cell_size(c);
                Size::new(s.w.max(8), s.h.max(17))
            }
            Native::Combo(c) => {
                let s = cell_size(c);
                Size::new(s.w.max(8), s.h.max(26))
            }
            Native::Button(b) => {
                let s = if b.respondsToSelector(sel!(fittingSize)) {
                    let f = b.fittingSize();
                    Size::new(f.width.ceil() as i32, f.height.ceil() as i32)
                } else {
                    let c = cell_size(b);
                    Size::new(c.w + 16, c.h + 4)
                };
                Size::new(s.w.max(48), s.h.max(24))
            }
            Native::Text(f) => Size::new(160, cell_size(f).h.max(22)),
            Native::Spin { tf, .. } => Size::new(90, cell_size(tf).h.max(22)),
            Native::TextArea { .. } => Size::new(200, 100),
            Native::Grid { .. } if e.kind == Kind::ListBox => Size::new(160, 100),
            Native::Grid { .. } => Size::new(300, 150),
            Native::Tree { .. } => Size::new(200, 200),
            Native::Slider(s) => Size::new(150, cell_size(s).h.clamp(16, 40)),
            Native::Progress(_) => Size::new(150, 20),
            Native::Image(_) => st(|s| s.imgsz.get(&id).copied()).unwrap_or(Size::new(32, 32)),
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
        let (outer, client): (&NSView, &dyn Fn() -> NSSize) = match &e.n {
            Native::Group { bx, cont } => (bx, &|| cont.frame().size),
            Native::Tabs(t) => (t, &|| t.contentRect().size),
            _ => return Size::default(),
        };
        let old = outer.frame();
        outer.setFrame(rect(old.origin.x, old.origin.y, probe_w, probe_h));
        let c = client();
        outer.setFrame(old);
        Size::new(
            (probe_w - c.width).round().max(0.0) as i32,
            (probe_h - c.height).round().max(0.0) as i32,
        )
    }

    fn native_handle(id: WidgetId) -> Option<NativeHandle> {
        let e = ent(id)?;
        let p = match &e.n {
            Native::Window { win, .. } => addr(&**win),
            Native::MenuBar { menu, .. } | Native::Menu { menu, .. } | Native::Popup(menu) => {
                addr(&**menu)
            }
            Native::Item(i) => addr(&**i),
            Native::Page { view, .. } => addr(&**view),
            _ => addr(e.view()?),
        };
        Some(NativeHandle::Cocoa(p))
    }

    fn message_box(_parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        let mtm = mt();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&ns(&spec.title));
        alert.setInformativeText(&ns(&spec.text));
        alert.setAlertStyle(match spec.kind {
            MessageKind::Info | MessageKind::Question => NSAlertStyle::Informational,
            MessageKind::Warning => NSAlertStyle::Warning,
            MessageKind::Error => NSAlertStyle::Critical,
        });
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
            alert.addButtonWithTitle(&ns(title));
        }
        let r = alert.runModal();
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
        let mtm = mt();
        let save = spec.mode == FileMode::Save;
        let open = (!save).then(|| NSOpenPanel::openPanel(mtm));
        let panel: Retained<NSSavePanel> = match &open {
            Some(o) => Retained::into_super(o.clone()),
            None => NSSavePanel::savePanel(mtm),
        };
        if !spec.title.is_empty() {
            panel.setTitle(Some(&ns(&spec.title)));
        }
        if let Some(open) = &open {
            let folder = spec.mode == FileMode::PickFolder;
            open.setCanChooseFiles(!folder);
            open.setCanChooseDirectories(folder);
            open.setAllowsMultipleSelection(spec.mode == FileMode::OpenMany);
        }
        let exts: Vec<Retained<NSString>> = spec
            .filters
            .iter()
            .flat_map(|(_, e)| e.iter())
            .map(|e| ns(e.trim_start_matches("*.").trim_start_matches('.')))
            .collect();
        // `allowedFileTypes` is deprecated since macOS 12 (UTType replacement) but still honoured.
        if !exts.is_empty()
            && spec.mode != FileMode::PickFolder
            && panel.respondsToSelector(sel!(setAllowedFileTypes:))
        {
            panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&exts)));
        }
        if let Some(dir) = &spec.initial_dir {
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&ns(dir))));
        }
        if let (Some(name), true) = (&spec.initial_name, save) {
            panel.setNameFieldStringValue(&ns(name));
        }
        if panel.runModal() != 1 {
            return vec![];
        }
        let path_of = |url: &NSURL| url.path().map(|p| from_ns(&p)).unwrap_or_default();
        match &open {
            None => panel.URL().iter().map(|u| path_of(u)).collect(),
            Some(o) => o.URLs().iter().map(|u| path_of(&u)).collect(),
        }
    }

    fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
        let Some(Native::Popup(m)) = ent(menu).map(|e| e.n) else {
            return;
        };
        let Some(we) = parent_window.and_then(ent) else {
            return;
        };
        let (Some(win), Some(cont)) = (we.win(), we.cont()) else {
            return;
        };
        let h = client_size(&we).height;
        let loc = match at {
            Some((x, y)) => NSPoint::new(x as f64, y as f64),
            None => {
                let p = win.mouseLocationOutsideOfEventStream();
                NSPoint::new(p.x, h - p.y)
            }
        };
        #[cfg(rungui_gnustep)]
        {
            // GNUstep sizes a menu popped up with popUpMenuPositioningItem:... to the window;
            // popUpContextMenu:withEvent:forView: with a synthesized right-click behaves.
            if let Some(ev) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::RightMouseDown,
                NSPoint::new(loc.x, h - loc.y),
                NSEventModifierFlags::empty(),
                0.0,
                win.windowNumber(),
                None,
                0,
                1,
                1.0,
            ) {
                NSMenu::popUpContextMenu_withEvent_forView(&m, &ev, cont);
            }
        }
        #[cfg(not(rungui_gnustep))]
        if m.respondsToSelector(sel!(popUpMenuPositioningItem:atLocation:inView:)) {
            // `loc` is in the (flipped) content view's coordinates, which AppKit honours.
            m.popUpMenuPositioningItem_atLocation_inView(None, loc, Some(cont));
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
            if let (Some(i), Kind::MenuItem | Kind::CheckMenuItem) = (e.menu_item(), e.kind) {
                set_a11y(i, node);
            } else if is_view_kind(e.kind) {
                if let Some(v) = e.focus_view() {
                    set_a11y(v, node);
                }
            }
        }
    }
}

fn set_a11y<T: NSAccessibility + objc2::Message>(target: &T, node: &crate::a11y::Resolved) {
    if let Some(l) = &node.name {
        if target.respondsToSelector(sel!(setAccessibilityLabel:)) {
            target.setAccessibilityLabel(Some(&ns(l)));
        }
    }
    if let Some(d) = &node.description {
        if target.respondsToSelector(sel!(setAccessibilityHelp:)) {
            target.setAccessibilityHelp(Some(&ns(d)));
        }
    }
}

/// The NSMenu that receives new items for a parent entry (MenuBar, Menu, PopupMenu).
fn parent_menu(pe: &Option<Entry>) -> Result<Retained<NSMenu>> {
    match pe.as_ref().map(|p| &p.n) {
        Some(Native::MenuBar { menu, .. })
        | Some(Native::Menu { menu, .. })
        | Some(Native::Popup(menu)) => Ok(menu.clone()),
        _ => Err(Error::InvalidHandle),
    }
}

fn select_row(tv: &NSTableView, row: isize) {
    tv.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row as usize), false);
    tv.scrollRowToVisible(row);
}

fn set_text(e: &Entry, t: &str) {
    match &e.n {
        Native::Window { win, .. } => win.setTitle(&ns(t)),
        Native::Label(f) | Native::Text(f) => {
            if from_ns(&f.stringValue()) != t {
                f.setStringValue(&ns(t));
            }
        }
        Native::Button(b) => b.setTitle(&ns(&crate::text::strip_mnemonic(t))),
        Native::Group { bx, .. } => bx.setTitle(&ns(&crate::text::strip_mnemonic(t))),
        Native::TextArea { tv, .. } => {
            if from_ns(&tv.string()) != t {
                tv.setString(&ns(t));
            }
        }
        Native::Page { item, .. } => item.setLabel(&ns(t)),
        Native::Menu { menu, item } => {
            let t = &crate::text::strip_mnemonic(t);
            menu.setTitle(&ns(t));
            item.setTitle(&ns(t));
            // An app-defined "Edit" menu replaces the built-in one.
            if t.trim().eq_ignore_ascii_case("edit") {
                if let Some(bar) = uc::menu_of(item) {
                    let edit = st(|s| {
                        let (_, ent) = s.ents.iter().find(|(_, x)| {
                            matches!(&x.n, Native::MenuBar { menu: m, .. } if addr(&**m) == addr(&*bar))
                        })?;
                        match &ent.n {
                            Native::MenuBar { edit, .. } => edit.clone(),
                            _ => None,
                        }
                    });
                    if let Some(it) = edit {
                        if uc::menu_of(&it).is_some_and(|m| addr(&*m) == addr(&*bar)) {
                            bar.removeItem(&it);
                        }
                    }
                }
            }
        }
        Native::Item(i) if e.kind != Kind::MenuSeparator => {
            i.setTitle(&ns(&crate::text::strip_mnemonic(t)))
        }
        _ => {}
    }
}

fn set_enabled(id: WidgetId, e: &Entry, en: bool) {
    st(|s| {
        if en {
            s.disabled.remove(&id);
        } else {
            s.disabled.insert(id);
        }
    });
    match &e.n {
        Native::TextArea { tv, .. } => {
            let ro = st(|s| s.readonly.contains(&id));
            tv.setEditable(en && !ro);
            tv.setSelectable(en);
        }
        Native::Grid { tv, .. } => tv.setEnabled(en),
        Native::Tree { tv, .. } => tv.setEnabled(en),
        Native::Spin { tf, stp, .. } => {
            tf.setEnabled(en);
            stp.setEnabled(en);
        }
        Native::Label(c) | Native::Text(c) => c.setEnabled(en),
        Native::Button(c) => c.setEnabled(en),
        Native::Combo(c) => c.setEnabled(en),
        Native::Slider(c) => c.setEnabled(en),
        Native::Image(c) => c.setEnabled(en),
        Native::Item(i) if e.kind != Kind::MenuSeparator => i.setEnabled(en),
        _ => {}
    }
}

fn set_visible(id: WidgetId, e: &Entry, v: bool) {
    match &e.n {
        Native::Window { win, .. } => {
            if v {
                let first = st(|s| s.shown.insert(id));
                if first && !st(|s| s.placed.contains(&id)) {
                    win.center();
                }
                install_menubar(id);
                win.makeKeyAndOrderFront(None);
            } else {
                win.orderOut(None);
            }
        }
        // GNUstep's NSMenuItem has no setHidden:
        Native::Item(i) => {
            if i.respondsToSelector(sel!(setHidden:)) {
                i.setHidden(!v)
            }
        }
        Native::Menu { item, .. } => {
            if item.respondsToSelector(sel!(setHidden:)) {
                item.setHidden(!v)
            }
        }
        Native::Page { .. } | Native::MenuBar { .. } | Native::Popup(_) => {}
        _ => {
            if let Some(view) = e.view() {
                view.setHidden(!v);
            }
        }
    }
}

/// Set the client size around the top-left corner (frame origins are bottom-left based and
/// GNUstep keeps the bottom-left fixed, which would make the window jump).
fn resize_window(win: &NSWindow, w: f64, h: f64) {
    let f = win.frame();
    win.setContentSize(NSSize::new(w, h));
    win.setFrameTopLeftPoint(NSPoint::new(f.origin.x, f.origin.y + f.size.height));
}

fn set_bounds(e: &Entry, r: Rect) {
    if let Some(w) = e.win() {
        resize_window(w, r.w.max(1) as f64, r.h.max(1) as f64);
        return;
    }
    if !is_view_kind(e.kind) {
        return;
    }
    if let Some(v) = e.view() {
        v.setFrame(rect_from(r));
    }
    match &e.n {
        Native::Spin { tf, stp, .. } => {
            let sw = 19.0;
            let (w, h) = (r.w.max(0) as f64, r.h.max(0) as f64);
            tf.setFrame(rect(0.0, 0.0, (w - sw).max(0.0), h));
            stp.setFrame(rect((w - sw).max(0.0), 0.0, sw, h));
        }
        Native::Grid { tv, .. } if e.kind == Kind::ListBox => tv.sizeLastColumnToFit(),
        Native::Tree { tv, .. } => tv.sizeLastColumnToFit(),
        _ => {}
    }
}

/// Fixed-pitch (or the control's normal) font for text controls.
fn set_monospace(e: &Entry, mono: bool) {
    // Size 0 selects the class default size.
    let font = match (mono, &e.n) {
        (true, Native::TextArea { .. } | Native::Text(_)) => NSFont::userFixedPitchFontOfSize(0.0),
        (false, Native::TextArea { .. }) => NSFont::userFontOfSize(0.0),
        (false, Native::Text(_)) => Some(NSFont::systemFontOfSize(0.0)),
        _ => return,
    };
    let Some(font) = font else { return };
    match &e.n {
        Native::TextArea { tv, .. } => tv.setFont(Some(&font)),
        Native::Text(f) => f.setFont(Some(&font)),
        _ => {}
    }
}

/// TextArea soft wrap: wrapping tracks the text view width; otherwise the container is
/// unbounded, the view grows horizontally and the scroll view shows a horizontal scroller.
fn set_wrap(e: &Entry, wrap: bool) {
    let Native::TextArea { sv, tv } = &e.n else {
        return;
    };
    let Some(tc) = uc::text_container(tv) else {
        return;
    };
    let huge = 1e7;
    if wrap {
        sv.setHasHorizontalScroller(false);
        tv.setHorizontallyResizable(false);
        let cs = sv.contentSize();
        let f = tv.frame();
        tv.setFrame(rect(f.origin.x, f.origin.y, cs.width, f.size.height));
        tc.setWidthTracksTextView(true);
        tc.setContainerSize(NSSize::new(cs.width, huge));
    } else {
        tc.setWidthTracksTextView(false);
        tc.setContainerSize(NSSize::new(huge, huge));
        tv.setMaxSize(NSSize::new(huge, huge));
        tv.setHorizontallyResizable(true);
        sv.setHasHorizontalScroller(true);
    }
}

fn set_items(id: WidgetId, e: &Entry, items: &[String]) {
    st(|s| s.items.insert(id, items.to_vec()));
    match &e.n {
        Native::Combo(c) => {
            c.removeAllItems();
            let Some(menu) = c.menu() else { return };
            menu.setAutoenablesItems(false);
            for it in items {
                // via the menu so that duplicate titles are kept
                menu.addItem(&uc::menu_item(mt(), it, None, ""));
            }
        }
        Native::Grid { tv, .. } if e.kind == Kind::ListBox => tv.reloadData(),
        _ => {}
    }
}

fn set_image(id: WidgetId, e: &Entry, img: Option<&ImageData>) {
    let Native::Image(v) = &e.n else { return };
    let Some(img) =
        img.filter(|i| i.w > 0 && i.h > 0 && i.rgba.len() == (i.w as usize) * (i.h as usize) * 4)
    else {
        v.setImage(None);
        st(|s| s.imgsz.remove(&id));
        return;
    };
    let Some(image) = uc::rgba_image(img.w as isize, img.h as isize, &img.rgba) else {
        return;
    };
    v.setImage(Some(&image));
    st(|s| s.imgsz.insert(id, Size::new(img.w as i32, img.h as i32)));
}

fn set_columns(mtm: MainThreadMarker, e: &Entry, cols: &[Column]) {
    let Some(tv) = e.table() else { return };
    for c in tv.tableColumns().iter() {
        tv.removeTableColumn(&c);
    }
    for (i, c) in cols.iter().enumerate() {
        let col = new_column(mtm, &i.to_string(), c.width.max(1) as f64);
        col.headerCell().setStringValue(&ns(&c.title));
        style_data_cell(
            &col,
            Some(match c.align {
                ColumnAlign::Left => NSTextAlignment::Left,
                ColumnAlign::Right => NSTextAlignment::Right,
                ColumnAlign::Center => NSTextAlignment::Center,
            }),
        );
        tv.addTableColumn(&col);
    }
}

fn set_sort_indicator(e: &Entry, si: Option<(usize, bool)>) {
    let Some(tv) = e.table() else { return };
    // GNUstep declares this method but only logs "not implemented".
    if cfg!(rungui_gnustep) || !tv.respondsToSelector(sel!(setIndicatorImage:inTableColumn:)) {
        return;
    }
    for (i, col) in tv.tableColumns().iter().enumerate() {
        let img = match si {
            Some((c, asc)) if c == i => NSImage::imageNamed(&ns(if asc {
                "NSAscendingSortIndicator"
            } else {
                "NSDescendingSortIndicator"
            })),
            _ => None,
        };
        tv.setIndicatorImage_inTableColumn(img.as_deref(), &col);
    }
}

fn set_tree_rows(id: WidgetId, e: &Entry, rows: &[TreeRow]) {
    let Native::Tree { tv, .. } = &e.n else {
        return;
    };
    let mut m = TreeModel::default();
    let mut stack: Vec<usize> = vec![]; // node index per depth
    for r in rows {
        let idx = m.nodes.len();
        m.nodes.push(TNode {
            text: r.text.clone(),
            expanded: r.expanded,
            has_children: r.has_children,
            children: vec![],
            obj: NSNumber::new_u64(r.node),
        });
        m.by_id.insert(r.node, idx);
        stack.truncate(r.depth as usize);
        match stack.last() {
            Some(p) => m.nodes[*p].children.push(idx),
            None => m.roots.push(idx),
        }
        stack.push(idx);
    }
    let expanded: Vec<Retained<NSNumber>> = m
        .nodes
        .iter()
        .filter(|n| n.expanded)
        .map(|n| n.obj.clone())
        .collect();
    let old = st(|s| s.trees.insert(id, m));
    tv.reloadData();
    for item in &expanded {
        uc::expand_item(tv, item);
    }
    if let Some(old) = old {
        old.nodes.into_iter().for_each(|n| bury(n.obj));
    }
}
