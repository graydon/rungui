//! GTK3 (Linux) backend. Hand-written FFI in `gtk/sys.rs`, linked by build.rs.
//!
//! Every native widget is kept in a thread-local `id -> W` map (plain `Copy` struct of raw
//! pointers, never borrowed across a GTK call: GTK signals re-enter this module through the core).
//! User actions arrive via GObject signals whose user-data is the widget id; programmatic changes
//! made inside `create/set/destroy` run under a guard that swallows the resulting signals.
//! Containers (Window, Page, GroupBox) are `GtkFixed`: the core's Rust layout places children
//! absolutely. Accessibility is native: GTK3 widgets already expose ATK (-> AT-SPI); `a11y_changed`
//! pushes the core's computed names/descriptions (e.g. "input labelled by preceding Label") into ATK.

mod sys;

use super::*;
use crate::a11y::A11yRole;
use crate::core;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use sys::*;

pub struct Gtk;

#[derive(Copy, Clone)]
struct W {
    kind: Kind,
    /// The widget placed in the parent (GtkWindow, GtkButton, GtkScrolledWindow, GtkMenuItem ...).
    w: P,
    /// Child container (GtkFixed / GtkMenu / menu bar) or the inner widget (GtkTextView, GtkTreeView).
    inner: P,
    /// Window: the vertical box holding menu bar + fixed.
    outer: P,
    accel_group: P,
    /// RadioButton: hidden sentinel sharing its group (GTK cannot deactivate a lone radio button).
    extra: P,
    menubar: P,
    parent: Option<WidgetId>,
    win: Option<WidgetId>,
    client: (i32, i32),
    emitted: (i32, i32),
    resizable: bool,
    accel_key: (c_uint, c_uint),
    /// Window: the scrolled window holding the fixed (its min size is not propagated to the toplevel).
    view: P,
    /// Window: last known screen position (None until the first configure).
    wpos: Option<(i32, i32)>,
    /// Sash: vertical splitter (horizontal sash), main-axis leading position of the last Bounds, and
    /// the drag in progress as (pointer root coordinate, position at press).
    sash_v: bool,
    sash_pos: i32,
    drag: Option<(f64, i32)>,
}

thread_local! {
    static WIDGETS: RefCell<HashMap<WidgetId, W>> = RefCell::new(HashMap::new());
    static GUARD: Cell<u32> = const { Cell::new(0) };
    static TIMERS: RefCell<HashMap<u64, c_uint>> = RefCell::new(HashMap::new());
    static PULSES: RefCell<HashMap<WidgetId, c_uint>> = RefCell::new(HashMap::new());
    static RESIZE_PENDING: RefCell<HashSet<WidgetId>> = RefCell::new(HashSet::new());
    static CHROME: RefCell<HashMap<u8, Size>> = RefCell::new(HashMap::new());
}

fn get(id: WidgetId) -> Option<W> {
    WIDGETS.with(|m| m.borrow().get(&id).copied())
}
fn upd(id: WidgetId, f: impl FnOnce(&mut W)) {
    WIDGETS.with(|m| {
        if let Some(w) = m.borrow_mut().get_mut(&id) {
            f(w)
        }
    })
}
fn guarded<R>(f: impl FnOnce() -> R) -> R {
    struct Dec;
    impl Drop for Dec {
        fn drop(&mut self) {
            GUARD.with(|g| g.set(g.get().saturating_sub(1)));
        }
    }
    GUARD.with(|g| g.set(g.get() + 1));
    let _d = Dec;
    f()
}
fn suppressed() -> bool {
    GUARD.with(|g| g.get() > 0)
}
fn cs(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}
unsafe fn from_c(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}
fn data(id: WidgetId) -> P {
    id.0 as usize as P
}
fn wid(d: P) -> WidgetId {
    WidgetId(d as usize as u64)
}
fn connect_raw(obj: P, sig: &'static [u8], f: *const (), d: P) {
    unsafe {
        let cb: Callback = std::mem::transmute::<*const (), Callback>(f);
        g_signal_connect_data(obj, sig.as_ptr() as *const c_char, cb, d, NULL, 0);
    }
}
fn connect(obj: P, sig: &'static [u8], f: *const (), id: WidgetId) {
    debug_assert!(sig.ends_with(&[0]));
    unsafe {
        let cb: Callback = std::mem::transmute::<*const (), Callback>(f);
        g_signal_connect_data(obj, sig.as_ptr() as *const c_char, cb, data(id), NULL, 0);
    }
}
/// Like [`connect`], but the handler runs after the widget's own (e.g. to draw over its child).
fn connect_after(obj: P, sig: &'static [u8], f: *const (), id: WidgetId) {
    debug_assert!(sig.ends_with(&[0]));
    unsafe {
        let cb: Callback = std::mem::transmute::<*const (), Callback>(f);
        g_signal_connect_data(
            obj,
            sig.as_ptr() as *const c_char,
            cb,
            data(id),
            NULL,
            CONNECT_AFTER,
        );
    }
}
fn emit(id: WidgetId, ev: Event) {
    if !suppressed() {
        core::event(id, ev);
    }
}

// ------------------------------------------------------------------ signal handlers

unsafe extern "C" fn h_clicked(_w: P, d: P) {
    emit(wid(d), Event::Click);
}
unsafe extern "C" fn h_toggled(w: P, d: P) {
    let on = unsafe { gtk_toggle_button_get_active(w) } != 0;
    emit(wid(d), Event::Toggled(on));
}
unsafe extern "C" fn h_menu_toggled(w: P, d: P) {
    let on = unsafe { gtk_check_menu_item_get_active(w) } != 0;
    emit(wid(d), Event::Toggled(on));
}
unsafe extern "C" fn h_entry_changed(w: P, d: P) {
    let t = unsafe { from_c(gtk_entry_get_text(w)) };
    emit(wid(d), Event::Text(t));
}
unsafe extern "C" fn h_buffer_changed(b: P, d: P) {
    let (mut a, mut z) = (TextIter::new(), TextIter::new());
    let t = unsafe {
        gtk_text_buffer_get_start_iter(b, &mut a);
        gtk_text_buffer_get_end_iter(b, &mut z);
        let p = gtk_text_buffer_get_text(b, &a, &z, 1);
        let s = from_c(p);
        g_free(p as P);
        s
    };
    emit(wid(d), Event::Text(t));
}
unsafe extern "C" fn h_value_changed(w: P, d: P) {
    let id = wid(d);
    let v = match get(id).map(|w| w.kind) {
        Some(Kind::SpinBox) => unsafe { gtk_spin_button_get_value(w) },
        Some(_) => unsafe { gtk_range_get_value(w) },
        None => return,
    };
    emit(id, Event::Value(v));
}
unsafe extern "C" fn h_combo_changed(w: P, d: P) {
    let i = unsafe { gtk_combo_box_get_active(w) };
    emit(wid(d), Event::Selected((i >= 0).then_some(i as usize)));
}
unsafe extern "C" fn h_switch_page(_nb: P, _page: P, num: c_uint, d: P) {
    emit(wid(d), Event::Selected(Some(num as usize)));
}
unsafe extern "C" fn h_delete(_w: P, _ev: P, d: P) -> c_int {
    core::close_requested(wid(d));
    1 // always veto: the core destroys the window if the app allows it
}
unsafe extern "C" fn h_focus_in(_w: P, _ev: P, d: P) -> c_int {
    emit(wid(d), Event::Focus(true));
    0
}
unsafe extern "C" fn h_focus_out(_w: P, _ev: P, d: P) -> c_int {
    emit(wid(d), Event::Focus(false));
    0
}
unsafe extern "C" fn h_size_allocate(_w: P, _r: P, d: P) {
    let id = wid(d);
    if RESIZE_PENDING.with(|p| p.borrow_mut().insert(id)) {
        unsafe { g_idle_add(resized_idle, d) };
    }
}
unsafe extern "C" fn resized_idle(d: P) -> c_int {
    let id = wid(d);
    RESIZE_PENDING.with(|p| p.borrow_mut().remove(&id));
    if let Some(w) = get(id) {
        let area = if w.view.is_null() { w.inner } else { w.view };
        let (cw, ch) = unsafe {
            (
                gtk_widget_get_allocated_width(area),
                gtk_widget_get_allocated_height(area),
            )
        };
        if cw > 1 && ch > 1 && (cw, ch) != w.emitted {
            upd(id, |w| w.emitted = (cw, ch));
            core::event(id, Event::Resized { w: cw, h: ch });
        }
    }
    0
}
unsafe extern "C" fn h_sel_changed(sel: P, d: P) {
    let id = wid(d);
    let Some(w) = get(id) else { return };
    let mut it = TreeIter::new();
    let mut model = NULL;
    let any = unsafe { gtk_tree_selection_get_selected(sel, &mut model, &mut it) } != 0;
    if w.kind == Kind::Tree {
        let node = if any {
            unsafe { tree_node(model, &mut it) }
        } else {
            0
        };
        emit(id, Event::TreeSelected((node != 0).then_some(node)));
    } else {
        let row = if any {
            unsafe { path_index(model, &mut it) }
        } else {
            None
        };
        emit(id, Event::Selected(row));
    }
}
unsafe fn tree_node(model: P, it: &mut TreeIter) -> u64 {
    let mut n: u64 = 0;
    unsafe { gtk_tree_model_get(model, it, 1 as c_int, &mut n as *mut u64, -1 as c_int) };
    n
}
/// The first (for a flat table: the only) index of a tree path, if it has one.
unsafe fn first_index(path: P) -> Option<usize> {
    unsafe {
        let indices = gtk_tree_path_get_indices(path);
        if indices.is_null() || gtk_tree_path_get_depth(path) < 1 {
            return None;
        }
        usize::try_from(*indices).ok()
    }
}
unsafe fn path_index(model: P, it: &mut TreeIter) -> Option<usize> {
    unsafe {
        let path = gtk_tree_model_get_path(model, it);
        if path.is_null() {
            return None;
        }
        let i = first_index(path);
        gtk_tree_path_free(path);
        i
    }
}
unsafe extern "C" fn h_tv_activated(tv: P, path: P, _col: P, d: P) {
    let id = wid(d);
    let Some(w) = get(id) else { return };
    unsafe {
        if w.kind == Kind::Tree {
            let model = gtk_tree_view_get_model(tv);
            let mut it = TreeIter::new();
            if gtk_tree_model_get_iter(model, &mut it, path) != 0 {
                let n = tree_node(model, &mut it);
                if n != 0 {
                    emit(id, Event::TreeActivated(n));
                }
            }
        } else if let Some(i) = first_index(path) {
            emit(id, Event::Activated(i));
        }
    }
}
unsafe extern "C" fn h_col_clicked(_col: P, d: P) {
    let v = d as usize as u64;
    emit(
        WidgetId(v >> 16),
        Event::ColumnClicked((v & 0xffff) as usize),
    );
}
unsafe extern "C" fn h_row_expanded(tv: P, it: *mut TreeIter, _path: P, d: P) {
    unsafe { tree_expand_event(tv, it, d, true) }
}
unsafe extern "C" fn h_row_collapsed(tv: P, it: *mut TreeIter, _path: P, d: P) {
    unsafe { tree_expand_event(tv, it, d, false) }
}
unsafe fn tree_expand_event(tv: P, it: *mut TreeIter, d: P, open: bool) {
    if suppressed() {
        return;
    }
    let node = unsafe { tree_node(gtk_tree_view_get_model(tv), &mut *it) };
    if node != 0 {
        // Deferred: the app typically replaces the rows (lazy loading) in response.
        let b = Box::into_raw(Box::new((wid(d), node, open)));
        unsafe { g_idle_add(tree_expand_idle, b as P) };
    }
}
unsafe extern "C" fn tree_expand_idle(d: P) -> c_int {
    let (id, node, open) = *unsafe { Box::from_raw(d as *mut (WidgetId, u64, bool)) };
    if get(id).is_some() {
        core::event(id, Event::TreeExpanded(node, open));
    }
    0
}
/// Window-client coordinates of a root-relative point.
unsafe fn client_xy(win: &W, root_x: f64, root_y: f64) -> (i32, i32) {
    unsafe {
        let (mut ox, mut oy) = (0, 0);
        gdk_window_get_origin(gtk_widget_get_window(win.w), &mut ox, &mut oy);
        let (mut cx, mut cy) = (0, 0);
        gtk_widget_translate_coordinates(
            win.w,
            win.inner,
            root_x as i32 - ox,
            root_y as i32 - oy,
            &mut cx,
            &mut cy,
        );
        (cx, cy)
    }
}
unsafe extern "C" fn h_button_press(_w: P, ev: *const EventButton, d: P) -> c_int {
    let id = wid(d);
    let ev = unsafe { &*ev };
    if ev.ty != 4 || ev.button != 3 || suppressed() {
        return 0;
    }
    let Some(win) = get(id).and_then(|w| w.win).and_then(get) else {
        return 0;
    };
    let (x, y) = unsafe { client_xy(&win, ev.x_root, ev.y_root) };
    core::event(id, Event::ContextMenu { x, y });
    1
}
/// Sash drag: the new leading-edge position is the position at press plus the pointer delta (root
/// coordinates, so it does not drift while the core moves the sash). The core clamps and relayouts.
unsafe extern "C" fn h_sash_press(widget: P, ev: *const EventButton, d: P) -> c_int {
    let id = wid(d);
    let ev = unsafe { &*ev };
    if ev.ty != 4 || ev.button != 1 {
        return 0;
    }
    let Some(w) = get(id) else { return 0 };
    unsafe { gtk_widget_grab_focus(widget) };
    let root = if w.sash_v { ev.y_root } else { ev.x_root };
    upd(id, |w| w.drag = Some((root, w.sash_pos)));
    1
}
unsafe extern "C" fn h_sash_motion(_w: P, ev: *const EventMotion, d: P) -> c_int {
    let id = wid(d);
    let ev = unsafe { &*ev };
    let Some(w) = get(id) else { return 0 };
    let Some((start, pos0)) = w.drag else {
        return 0;
    };
    if ev.state & EV_BUTTON1_MASK == 0 {
        upd(id, |w| w.drag = None);
        return 0;
    }
    let root = if w.sash_v { ev.y_root } else { ev.x_root };
    emit(
        id,
        Event::SashDragged(pos0.saturating_add((root - start).round() as i32)),
    );
    1
}
unsafe extern "C" fn h_sash_release(_w: P, ev: *const EventButton, d: P) -> c_int {
    let ev = unsafe { &*ev };
    if ev.button != 1 {
        return 0;
    }
    upd(wid(d), |w| w.drag = None);
    1
}
/// Arrow keys along the sash's axis (Shift = large step), Home and End; everything else (Tab, other
/// modifiers' shortcuts) is left to GTK.
unsafe extern "C" fn h_sash_key(_w: P, ev: *const EventKey, d: P) -> c_int {
    let id = wid(d);
    let ev = unsafe { &*ev };
    let Some(w) = get(id) else { return 0 };
    if ev.state & (MOD_CTRL | MOD_ALT) != 0 {
        return 0;
    }
    let big = ev.state & MOD_SHIFT != 0;
    let key = match ev.keyval {
        KEY_HOME | KEY_KP_HOME => SashKey::Min,
        KEY_END | KEY_KP_END => SashKey::Max,
        // the sash of a stacked (vertical) splitter moves up and down, otherwise left and right
        KEY_UP | KEY_KP_UP if w.sash_v => {
            if big {
                SashKey::PrevLarge
            } else {
                SashKey::Prev
            }
        }
        KEY_DOWN | KEY_KP_DOWN if w.sash_v => {
            if big {
                SashKey::NextLarge
            } else {
                SashKey::Next
            }
        }
        KEY_LEFT | KEY_KP_LEFT if !w.sash_v => {
            if big {
                SashKey::PrevLarge
            } else {
                SashKey::Prev
            }
        }
        KEY_RIGHT | KEY_KP_RIGHT if !w.sash_v => {
            if big {
                SashKey::NextLarge
            } else {
                SashKey::Next
            }
        }
        _ => return 0,
    };
    emit(id, Event::SashKey(key));
    1
}
/// The sash is a plain event box, which draws nothing when focused: draw the focus ring ourselves,
/// over its separator.
unsafe extern "C" fn h_sash_draw(w: P, cr: P, _d: P) -> c_int {
    unsafe {
        if gtk_widget_has_focus(w) != 0 {
            let ctx = gtk_widget_get_style_context(w);
            gtk_render_focus(
                ctx,
                cr,
                0.0,
                0.0,
                gtk_widget_get_allocated_width(w) as f64,
                gtk_widget_get_allocated_height(w) as f64,
            );
        }
    }
    0
}
/// Repaint the sash when it gains or loses focus (the focus ring comes and goes).
unsafe extern "C" fn h_sash_focus(w: P, _ev: P, _d: P) -> c_int {
    unsafe { gtk_widget_queue_draw(w) };
    0
}
unsafe extern "C" fn h_sash_enter(w: P, _ev: P, d: P) -> c_int {
    if let Some(s) = get(wid(d)) {
        unsafe {
            let name = if s.sash_v {
                c"row-resize"
            } else {
                c"col-resize"
            };
            let cur = gdk_cursor_new_from_name(gtk_widget_get_display(w), name.as_ptr());
            let win = gtk_widget_get_window(w);
            if !cur.is_null() {
                if !win.is_null() {
                    gdk_window_set_cursor(win, cur);
                }
                g_object_unref(cur);
            }
        }
    }
    0
}
/// Window moved (or resized): report the outer-frame position once it changed. The first
/// configure only records the initial placement.
unsafe extern "C" fn h_configure(w: P, _ev: *const EventConfigure, d: P) -> c_int {
    let id = wid(d);
    let (mut x, mut y) = (0, 0);
    unsafe { gtk_window_get_position(w, &mut x, &mut y) };
    let Some(win) = get(id) else { return 0 };
    if win.wpos == Some((x, y)) {
        return 0;
    }
    upd(id, |w| w.wpos = Some((x, y)));
    if win.wpos.is_some() {
        emit(id, Event::Moved { x, y });
    }
    0
}
unsafe extern "C" fn h_popup_key(w: P, d: P) -> c_int {
    let id = wid(d);
    let Some(win) = get(id).and_then(|w| w.win).and_then(get) else {
        return 0;
    };
    unsafe {
        let (mut x, mut y) = (0, 0);
        gtk_widget_translate_coordinates(
            w,
            win.inner,
            gtk_widget_get_allocated_width(w) / 2,
            gtk_widget_get_allocated_height(w) / 2,
            &mut x,
            &mut y,
        );
        core::event(id, Event::ContextMenu { x, y });
    }
    1
}
unsafe extern "C" fn h_menu_deactivate(_m: P, lp: P) {
    unsafe { g_main_loop_quit(lp) };
}
unsafe extern "C" fn tree_find(model: P, path: P, it: *mut TreeIter, d: P) -> c_int {
    let st = unsafe { &mut *(d as *mut (u64, P)) };
    if unsafe { tree_node(model, &mut *it) } == st.0 {
        st.1 = unsafe { gtk_tree_path_copy(path) };
        return 1;
    }
    0
}
unsafe extern "C" fn drain_idle(_d: P) -> c_int {
    core::drain_posted();
    0
}
unsafe extern "C" fn timer_cb(d: P) -> c_int {
    let token = d as usize as u64;
    let repeat = TIMERS.with(|t| t.borrow().contains_key(&token));
    if !repeat {
        return 0;
    }
    core::timer_fired(token);
    TIMERS.with(|t| t.borrow().contains_key(&token)) as c_int
}
unsafe extern "C" fn oneshot_cb(d: P) -> c_int {
    let token = d as usize as u64;
    if TIMERS.with(|t| t.borrow_mut().remove(&token)).is_some() {
        core::timer_fired(token);
    }
    0
}
unsafe extern "C" fn pulse_cb(d: P) -> c_int {
    let id = wid(d);
    match get(id) {
        Some(w) if PULSES.with(|p| p.borrow().contains_key(&id)) => {
            unsafe { gtk_progress_bar_pulse(w.w) };
            1
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ helpers

fn blank(kind: Kind, w: P, parent: Option<WidgetId>, win: Option<WidgetId>) -> W {
    W {
        kind,
        w,
        inner: w,
        outer: NULL,
        accel_group: NULL,
        extra: NULL,
        menubar: NULL,
        parent,
        win,
        client: (0, 0),
        emitted: (0, 0),
        resizable: true,
        accel_key: (0, 0),
        view: NULL,
        wpos: None,
        sash_v: false,
        sash_pos: 0,
        drag: None,
    }
}

fn keyval(key: &str) -> c_uint {
    let mut it = key.chars();
    if let (Some(c), None) = (it.next(), it.next()) {
        return unsafe { gdk_unicode_to_keyval(c.to_lowercase().next().unwrap_or(c) as c_uint) };
    }
    let k = key.to_ascii_uppercase();
    if let Some(n) = k.strip_prefix('F').and_then(|n| n.parse::<c_uint>().ok()) {
        if (1..=35).contains(&n) {
            return 0xffbe + n - 1;
        }
    }
    match k.as_str() {
        "ENTER" | "RETURN" => 0xff0d,
        "ESC" | "ESCAPE" => 0xff1b,
        "DEL" | "DELETE" => 0xffff,
        "TAB" => 0xff09,
        "SPACE" => 0x20,
        "BACKSPACE" => 0xff08,
        "INSERT" | "INS" => 0xff63,
        "HOME" => 0xff50,
        "END" => 0xff57,
        "PAGEUP" | "PGUP" => 0xff55,
        "PAGEDOWN" | "PGDN" => 0xff56,
        "LEFT" => 0xff51,
        "UP" => 0xff52,
        "RIGHT" => 0xff53,
        "DOWN" => 0xff54,
        _ => 0,
    }
}

/// The size of the X screen (all monitors), or `i32::MAX` when it cannot be asked.
fn screen_size() -> (i32, i32) {
    unsafe {
        let screen = gdk_screen_get_default();
        if screen.is_null() {
            return (i32::MAX, i32::MAX);
        }
        let (w, h) = (gdk_screen_get_width(screen), gdk_screen_get_height(screen));
        if w > 0 && h > 0 {
            (w, h)
        } else {
            (i32::MAX, i32::MAX)
        }
    }
}

fn apply_window_size(w: &W) {
    // A window larger than the screen is never useful, and the server allocates a backing store
    // for it (a 16384 x 16384 window made Xvfb fail): content that does not fit is clipped, as
    // when a window manager constrains the window.
    let (sw, sh) = screen_size();
    let (cw, ch) = (w.client.0.min(sw), w.client.1.min(sh));
    if cw <= 0 || ch <= 0 {
        return;
    }
    unsafe {
        let mut mb = 0;
        if !w.menubar.is_null() {
            let (mut min, mut nat) = (Req::default(), Req::default());
            gtk_widget_get_preferred_size(w.menubar, &mut min, &mut nat);
            mb = nat.h;
        }
        if !w.resizable {
            gtk_widget_set_size_request(w.view, cw, ch);
        }
        gtk_window_resize(w.w, cw, ch + mb);
    }
}

fn is_container(k: Kind) -> bool {
    matches!(k, Kind::Window | Kind::Page | Kind::GroupBox)
}

unsafe fn put(parent: &W, child: P) {
    unsafe {
        gtk_fixed_put(parent.inner, child, 0, 0);
        gtk_widget_show(child);
    }
}

fn ctx_hooks(obj: P, id: WidgetId) {
    connect(
        obj,
        b"button-press-event\0",
        h_button_press as *const (),
        id,
    );
    connect(obj, b"popup-menu\0", h_popup_key as *const (), id);
}

fn focus_hooks(obj: P, id: WidgetId) {
    connect(obj, b"focus-in-event\0", h_focus_in as *const (), id);
    connect(obj, b"focus-out-event\0", h_focus_out as *const (), id);
}

unsafe fn scrolled(child: P) -> P {
    unsafe {
        let sw = gtk_scrolled_window_new(NULL, NULL);
        gtk_scrolled_window_set_shadow_type(sw, SHADOW_IN);
        gtk_scrolled_window_set_policy(sw, POLICY_AUTOMATIC, POLICY_AUTOMATIC);
        gtk_container_add(sw, child);
        gtk_widget_show(child);
        sw
    }
}

fn digits_for(step: f64) -> c_uint {
    let (mut d, mut s) = (0, step.abs());
    while d < 6 && (s - s.round()).abs() > 1e-9 {
        s *= 10.0;
        d += 1;
    }
    d
}

/// The event a popup menu is opened with. Inside an event handler that is the current event;
/// otherwise (a timer, a posted closure) GTK has none, warns "no trigger event for menu popup" and
/// has no device to grab with, so a button press is made up.
struct Trigger(P);

impl Trigger {
    /// `window`: the GdkWindow the press happened in (None: the root window).
    unsafe fn new(window: Option<P>) -> Trigger {
        unsafe {
            let current = gtk_get_current_event();
            if !current.is_null() {
                return Trigger(current);
            }
            let ev = gdk_event_new(EV_TYPE_BUTTON_PRESS);
            if ev.is_null() {
                return Trigger(NULL);
            }
            let win =
                window.unwrap_or_else(|| gdk_screen_get_root_window(gdk_screen_get_default()));
            let e = ev as *mut EventButton;
            (*e).window = g_object_ref(win); // gdk_event_free drops this reference
            (*e).send_event = 1;
            (*e).button = POPUP_BUTTON;
            let seat = gdk_display_get_default_seat(gdk_display_get_default());
            gdk_event_set_device(ev, gdk_seat_get_pointer(seat));
            Trigger(ev)
        }
    }
}

impl Drop for Trigger {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { gdk_event_free(self.0) };
        }
    }
}

/// GDK_BUTTON_PRESS, and the right button a context menu is conventionally opened with.
const EV_TYPE_BUTTON_PRESS: c_int = 4;
const POPUP_BUTTON: c_uint = 3;

/// Response codes of the buttons of a message box (any distinct, non-GTK-reserved values).
const RESP_OK: c_int = 1;
const RESP_CANCEL: c_int = 2;
const RESP_YES: c_int = 3;
const RESP_NO: c_int = 4;

/// Show the pages of a notebook that was hidden when they were added (see `Kind::Page` creation).
unsafe fn show_pages(notebook: P) {
    unsafe {
        let list = gtk_container_get_children(notebook);
        let mut l = list;
        while !l.is_null() {
            gtk_widget_show((*l).data);
            l = (*l).next;
        }
        g_list_free(list);
    }
}

/// Natural-size floors for widgets whose GTK natural size is too small to use.
const MIN_TEXT_AREA: Size = Size::new(240, 100);
const MIN_LIST_BOX: Size = Size::new(160, 100);
const MIN_TABLE: Size = Size::new(240, 140);
const MIN_RANGE_WIDTH: i32 = 160;
/// Narrowest table column. The header button needs room for its own border and padding, and GTK
/// warns when it is allocated less than that.
const MIN_COLUMN_WIDTH: i32 = 24;
/// Allocations under this many pixels are checked against the widget's real minimum size.
const TINY_ALLOC: i32 = 32;

/// (minimum, natural) size of `widget`, ignoring any size request pushed earlier through Bounds.
unsafe fn request_sizes(widget: P) -> (Req, Req) {
    unsafe {
        let (mut rw, mut rh) = (-1, -1);
        gtk_widget_get_size_request(widget, &mut rw, &mut rh);
        gtk_widget_set_size_request(widget, -1, -1);
        let (mut min, mut nat) = (Req::default(), Req::default());
        gtk_widget_get_preferred_size(widget, &mut min, &mut nat);
        gtk_widget_set_size_request(widget, rw, rh);
        (min, nat)
    }
}

/// The smallest size `widget` can be allocated without GTK complaining.
unsafe fn min_request(widget: P) -> (i32, i32) {
    let (min, _) = unsafe { request_sizes(widget) };
    (min.w, min.h)
}

/// Border/header size of a GroupBox (key 0) or Tabs (key 1): allocate a real, unmapped toplevel
/// once and compare outer and inner allocations (CSS-dependent, so measured, not guessed).
fn measure_chrome(key: u8) -> Size {
    if let Some(s) = CHROME.with(|c| c.borrow().get(&key).copied()) {
        return s;
    }
    let s = unsafe {
        let win = gtk_window_new(WINDOW_TOPLEVEL);
        let page = gtk_fixed_new();
        let outer = if key == 0 {
            let f = gtk_frame_new(c"Xg".as_ptr());
            gtk_container_add(f, page);
            f
        } else {
            let nb = gtk_notebook_new();
            let lbl = gtk_label_new(c"Xg".as_ptr());
            gtk_widget_show(lbl);
            gtk_notebook_append_page(nb, page, lbl);
            nb
        };
        gtk_container_add(win, outer);
        gtk_widget_show_all(win);
        gtk_widget_realize(win);
        let r = Rectangle {
            x: 0,
            y: 0,
            w: 400,
            h: 300,
        };
        gtk_widget_size_allocate(win, &r);
        let s = Size::new(
            gtk_widget_get_allocated_width(outer) - gtk_widget_get_allocated_width(page),
            gtk_widget_get_allocated_height(outer) - gtk_widget_get_allocated_height(page),
        );
        gtk_widget_destroy(win);
        s
    };
    CHROME.with(|c| c.borrow_mut().insert(key, s));
    s
}

fn has_inner(k: Kind) -> bool {
    matches!(k, Kind::TextArea | Kind::ListBox | Kind::Table | Kind::Tree)
}

/// ATK role to force for a node, when GTK's own would be wrong. Only roles whose ATK equivalent is
/// unambiguous are mapped; anything else keeps GTK's default.
fn atk_role_for(kind: Kind, role: A11yRole) -> Option<c_int> {
    use A11yRole as R;
    match kind {
        Kind::Sash => return Some(ATK_ROLE_SPLIT_PANE),
        Kind::Table => return Some(ATK_ROLE_TABLE),
        // a one-column tree view, which GTK would report as a table
        Kind::ListBox => return Some(ATK_ROLE_LIST_BOX),
        Kind::Tree => return Some(ATK_ROLE_TREE_TABLE),
        _ => {}
    }
    // an explicit override on a kind whose default role it is not
    if crate::a11y::default_role(kind) == role {
        return None;
    }
    Some(match role {
        R::Label => ATK_ROLE_LABEL,
        R::Group => ATK_ROLE_GROUPING,
        R::Pane => ATK_ROLE_PANEL,
        R::Image => ATK_ROLE_IMAGE,
        R::ListBox => ATK_ROLE_LIST_BOX,
        R::TextInput => ATK_ROLE_ENTRY,
        R::PasswordInput => ATK_ROLE_PASSWORD_TEXT,
        R::MultilineTextInput => ATK_ROLE_TEXT,
        R::Splitter => ATK_ROLE_SPLIT_PANE,
        _ => return None,
    })
}

fn kind_atk_name(w: &W) -> bool {
    !matches!(w.kind, Kind::MenuSeparator)
}

// ------------------------------------------------------------------ the backend

impl Backend for Gtk {
    fn init(app_name: &str) -> Result<()> {
        unsafe {
            let ok = gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut());
            if ok == 0 {
                return Err(Error::Backend("GTK could not open a display".into()));
            }
            g_set_prgname(cs(app_name).as_ptr());
            g_set_application_name(cs(app_name).as_ptr());
        }
        Ok(())
    }

    fn run() {
        unsafe { gtk_main() }
    }

    fn quit() {
        unsafe { gtk_main_quit() }
    }

    fn wake() {
        // g_idle_add is thread-safe.
        unsafe { g_idle_add(drain_idle, NULL) };
    }

    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        Self::timer_stop(token);
        let f: SourceFn = if repeat { timer_cb } else { oneshot_cb };
        let src = unsafe { g_timeout_add(millis.max(1), f, token as usize as P) };
        if src == 0 {
            return Err(Error::Backend("g_timeout_add failed".into()));
        }
        TIMERS.with(|t| t.borrow_mut().insert(token, src));
        Ok(())
    }

    fn timer_stop(token: u64) {
        if let Some(src) = TIMERS.with(|t| t.borrow_mut().remove(&token)) {
            unsafe { g_source_remove(src) };
        }
    }

    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
        let pw = parent.and_then(get);
        let win = if kind == Kind::Window {
            Some(id)
        } else {
            pw.and_then(|p| p.win)
        };
        let ok = match kind {
            Kind::Window | Kind::PopupMenu => parent.is_none(),
            Kind::MenuBar => pw.is_some_and(|p| p.kind == Kind::Window),
            Kind::Page => pw.is_some_and(|p| p.kind == Kind::Tabs),
            Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
                pw.is_some_and(|p| matches!(p.kind, Kind::MenuBar | Kind::Menu | Kind::PopupMenu))
            }
            _ => pw.is_some_and(|p| is_container(p.kind)),
        };
        if !ok {
            return Err(Error::InvalidHandle);
        }
        guarded(|| unsafe { create_inner(id, kind, parent, pw, win) })
    }

    fn destroy(id: WidgetId) {
        let Some(w) = get(id) else { return };
        guarded(|| unsafe {
            if let Some(src) = PULSES.with(|p| p.borrow_mut().remove(&id)) {
                g_source_remove(src);
            }
            // forget the id first so late signals/idles find nothing
            WIDGETS.with(|m| m.borrow_mut().remove(&id));
            if let (Some(pid), Kind::MenuBar) = (w.parent, w.kind) {
                upd(pid, |p| {
                    if p.menubar == w.w {
                        p.menubar = NULL
                    }
                });
            }
            gtk_widget_destroy(w.w);
            if !w.extra.is_null() {
                g_object_unref(w.extra);
            }
        });
    }

    fn set(id: WidgetId, prop: &Prop) {
        let Some(w) = get(id) else { return };
        guarded(|| unsafe { set_inner(id, &w, prop) });
    }

    fn preferred_size(id: WidgetId) -> Size {
        let Some(w) = get(id) else {
            return Size::default();
        };
        if is_container(w.kind) || w.kind == Kind::Tabs {
            return Size::default();
        }
        let (_, nat) = unsafe { request_sizes(w.w) };
        let mut s = Size::new(nat.w, nat.h);
        // scrolled and range widgets report a tiny natural size; give them a usable one
        let at_least = |s: Size, floor: Size| Size::new(s.w.max(floor.w), s.h.max(floor.h));
        match w.kind {
            Kind::TextArea => s = at_least(s, MIN_TEXT_AREA),
            Kind::ListBox => s = at_least(s, MIN_LIST_BOX),
            Kind::Table | Kind::Tree => s = at_least(s, MIN_TABLE),
            Kind::Slider | Kind::ProgressBar => s.w = s.w.max(MIN_RANGE_WIDTH),
            _ => {}
        }
        s
    }

    fn chrome(id: WidgetId) -> Size {
        match get(id).map(|w| w.kind) {
            Some(Kind::GroupBox) => measure_chrome(0),
            Some(Kind::Tabs) => measure_chrome(1),
            _ => Size::default(),
        }
    }

    fn native_handle(id: WidgetId) -> Option<NativeHandle> {
        get(id).map(|w| NativeHandle::Gtk(w.w as usize))
    }

    fn message_box(parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        unsafe {
            let pw = parent
                .and_then(get)
                .map_or(NULL, |p| p.win.and_then(get).map_or(NULL, |w| w.w));
            let ty = match spec.kind {
                MessageKind::Info => MSG_INFO,
                MessageKind::Warning => MSG_WARNING,
                MessageKind::Error => MSG_ERROR,
                MessageKind::Question => MSG_QUESTION,
            };
            let d = gtk_message_dialog_new(pw, 1, ty, 0, c"%s".as_ptr(), cs(&spec.text).as_ptr());
            if d.is_null() {
                return Answer::Cancel;
            }
            gtk_window_set_title(d, cs(&spec.title).as_ptr());
            // (label, response) in button order, and the answer a closed dialog counts as
            let (buttons, closed_as): (&[(&CStr, c_int)], Answer) = match spec.buttons {
                Buttons::Ok => (&[(c"OK", RESP_OK)], Answer::Ok),
                Buttons::OkCancel => (
                    &[(c"Cancel", RESP_CANCEL), (c"OK", RESP_OK)],
                    Answer::Cancel,
                ),
                Buttons::YesNo => (&[(c"No", RESP_NO), (c"Yes", RESP_YES)], Answer::No),
                Buttons::YesNoCancel => (
                    &[
                        (c"Cancel", RESP_CANCEL),
                        (c"No", RESP_NO),
                        (c"Yes", RESP_YES),
                    ],
                    Answer::Cancel,
                ),
            };
            for (label, response) in buttons {
                gtk_dialog_add_button(d, label.as_ptr(), *response);
            }
            // Enter picks the affirmative button
            let default_response = buttons.last().map_or(RESP_OK, |b| b.1);
            gtk_dialog_set_default_response(d, default_response);
            let r = gtk_dialog_run(d);
            gtk_widget_destroy(d);
            match r {
                RESP_OK => Answer::Ok,
                RESP_YES => Answer::Yes,
                RESP_NO => Answer::No,
                RESP_CANCEL => Answer::Cancel,
                _ => closed_as,
            }
        }
    }

    fn file_dialog(parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String> {
        unsafe {
            let pw = parent
                .and_then(get)
                .map_or(NULL, |p| p.win.and_then(get).map_or(NULL, |w| w.w));
            let (action, accept): (c_int, &CStr) = match spec.mode {
                FileMode::Open | FileMode::OpenMany => (FC_OPEN, c"Open"),
                FileMode::Save => (FC_SAVE, c"Save"),
                FileMode::PickFolder => (FC_FOLDER, c"Select"),
            };
            let d = gtk_file_chooser_dialog_new(
                cs(&spec.title).as_ptr(),
                pw,
                action,
                c"Cancel".as_ptr(),
                RESPONSE_CANCEL,
                accept.as_ptr(),
                RESPONSE_ACCEPT,
                NULL,
            );
            if d.is_null() {
                return vec![];
            }
            gtk_file_chooser_set_select_multiple(d, (spec.mode == FileMode::OpenMany) as c_int);
            gtk_file_chooser_set_do_overwrite_confirmation(d, 1);
            if let Some(dir) = &spec.initial_dir {
                gtk_file_chooser_set_current_folder(d, cs(dir).as_ptr());
            }
            if let (FileMode::Save, Some(n)) = (spec.mode, &spec.initial_name) {
                gtk_file_chooser_set_current_name(d, cs(n).as_ptr());
            }
            if matches!(
                spec.mode,
                FileMode::Open | FileMode::OpenMany | FileMode::Save
            ) {
                for (label, exts) in &spec.filters {
                    let f = gtk_file_filter_new();
                    gtk_file_filter_set_name(f, cs(label).as_ptr());
                    for e in exts {
                        let e = e.trim_start_matches(['*', '.']);
                        if e.is_empty() {
                            gtk_file_filter_add_pattern(f, c"*".as_ptr());
                            continue;
                        }
                        for pat in [
                            format!("*.{}", e.to_lowercase()),
                            format!("*.{}", e.to_uppercase()),
                        ] {
                            gtk_file_filter_add_pattern(f, cs(&pat).as_ptr());
                        }
                    }
                    gtk_file_chooser_add_filter(d, f);
                }
            }
            let mut out = vec![];
            if gtk_dialog_run(d) == RESPONSE_ACCEPT {
                let list = gtk_file_chooser_get_filenames(d);
                let mut l = list;
                while !l.is_null() {
                    out.push(from_c((*l).data as *const c_char));
                    g_free((*l).data);
                    l = (*l).next;
                }
                g_slist_free(list);
            }
            gtk_widget_destroy(d);
            out
        }
    }

    fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
        let Some(m) = get(menu) else { return };
        if m.kind != Kind::PopupMenu {
            return;
        }
        unsafe {
            gtk_widget_show_all(m.w);
            let parent = parent_window.and_then(get);
            // an unrealized window has no GdkWindow to anchor the menu to
            let anchor = parent
                .map(|w| gtk_widget_get_window(w.w))
                .filter(|w| !w.is_null());
            let trigger = Trigger::new(anchor);
            match (at, parent.zip(anchor)) {
                (Some((x, y)), Some((win, gdk_win))) => {
                    let (mut tx, mut ty) = (0, 0);
                    gtk_widget_translate_coordinates(win.inner, win.w, x, y, &mut tx, &mut ty);
                    let r = Rectangle {
                        x: tx,
                        y: ty,
                        w: 1,
                        h: 1,
                    };
                    gtk_menu_popup_at_rect(m.w, gdk_win, &r, 1, 1, trigger.0);
                }
                _ => gtk_menu_popup_at_pointer(m.w, trigger.0),
            }
            if gtk_widget_get_visible(m.w) == 0 {
                return;
            }
            let lp = g_main_loop_new(NULL, 0);
            let h = g_signal_connect_data(
                m.w,
                c"deactivate".as_ptr(),
                std::mem::transmute::<*const (), Callback>(h_menu_deactivate as *const ()),
                lp,
                NULL,
                0,
            );
            g_main_loop_run(lp);
            g_signal_handler_disconnect(m.w, h);
            g_main_loop_unref(lp);
            // let a pending item activation be delivered before returning
            let mut n = 0;
            while gtk_events_pending() != 0 && n < 20 {
                gtk_main_iteration_do(0);
                n += 1;
            }
        }
    }

    // GTK3 widgets are already exposed through ATK/AT-SPI; names are refreshed in a11y_changed.
    fn a11y_changed(window: WidgetId) {
        let Some(nodes) = crate::a11y::resolve(window) else {
            return;
        };
        for node in &nodes {
            let Some(w) = get(node.id) else { continue };
            if !kind_atk_name(&w) {
                continue;
            }
            unsafe {
                let target = if has_inner(w.kind) { w.inner } else { w.w };
                let atk = gtk_widget_get_accessible(target);
                if atk.is_null() {
                    continue;
                }
                if let Some(l) = &node.name {
                    if from_c(atk_object_get_name(atk)) != *l {
                        atk_object_set_name(atk, cs(l).as_ptr());
                    }
                }
                if let Some(d) = &node.description {
                    atk_object_set_description(atk, cs(d).as_ptr());
                }
                // GTK's stock roles are close but not always the core's (a sash is a bare event box,
                // a flat table is reported as a tree table); app-set roles apply to all kinds.
                if let Some(r) = atk_role_for(w.kind, node.role) {
                    if atk_object_get_role(atk) != r {
                        atk_object_set_role(atk, r);
                    }
                }
            }
        }
    }
}

unsafe fn create_inner(
    id: WidgetId,
    kind: Kind,
    parent: Option<WidgetId>,
    pw: Option<W>,
    win: Option<WidgetId>,
) -> Result<()> {
    unsafe {
        let mut w = blank(kind, NULL, parent, win);
        match kind {
            Kind::Window => build_window(id, &mut w),
            Kind::Label => build_label(&mut w),
            Kind::Button => build_button(id, &mut w),
            Kind::CheckBox | Kind::RadioButton => build_check(id, kind, &mut w),
            Kind::TextInput | Kind::PasswordInput => build_entry(id, kind, &mut w),
            Kind::TextArea => build_text_area(id, &mut w),
            Kind::ComboBox => build_combo(id, &mut w),
            Kind::ListBox => build_list(id, &mut w),
            Kind::Table | Kind::Tree => build_tree_view(id, kind, &mut w),
            Kind::PopupMenu => {
                let m = gtk_menu_new();
                w.w = m;
                w.inner = m;
                WIDGETS.with(|m| m.borrow_mut().insert(id, w));
                return Ok(());
            }
            Kind::Slider => build_slider(id, &mut w),
            Kind::ProgressBar => w.w = gtk_progress_bar_new(),
            Kind::SpinBox => build_spin(id, &mut w),
            Kind::Tabs => build_tabs(id, &mut w),
            Kind::Page => {
                let page = gtk_fixed_new();
                let nb = pw.map_or(NULL, |p| p.w);
                // GTK 3 raises a critical when a notebook that is hidden while it gets a visible
                // page is later shown in a mapped window: show the page only once the notebook is
                // (and `show_pages` does it when the notebook is shown later)
                if gtk_widget_get_visible(nb) != 0 {
                    gtk_widget_show(page);
                }
                gtk_notebook_append_page(nb, page, NULL);
                w.w = page;
                w.inner = page;
                WIDGETS.with(|m| m.borrow_mut().insert(id, w));
                return Ok(());
            }
            Kind::GroupBox => build_group(&mut w),
            Kind::Image => w.w = gtk_image_new(),
            Kind::Sash => build_sash(id, &mut w),
            Kind::MenuBar => {
                let bar = gtk_menu_bar_new();
                let (Some(pid), Some(p)) = (parent, pw.filter(|p| p.kind == Kind::Window)) else {
                    gtk_widget_destroy(bar); // never added to anything: release the floating ref
                    return Err(Error::InvalidHandle);
                };
                gtk_box_pack_start(p.outer, bar, 0, 0, 0);
                gtk_box_reorder_child(p.outer, bar, 0);
                gtk_widget_show(bar);
                w.w = bar;
                w.inner = bar;
                upd(pid, |p| p.menubar = bar);
                WIDGETS.with(|m| m.borrow_mut().insert(id, w));
                if let Some(p) = get(pid) {
                    apply_window_size(&p);
                }
                return Ok(());
            }
            Kind::Menu => {
                let item = gtk_menu_item_new();
                let sub = gtk_menu_new();
                gtk_menu_item_set_submenu(item, sub);
                gtk_menu_shell_append(pw.map_or(NULL, |p| p.inner), item);
                gtk_widget_show(item);
                w.w = item;
                w.inner = sub;
                WIDGETS.with(|m| m.borrow_mut().insert(id, w));
                return Ok(());
            }
            Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
                let item = match kind {
                    Kind::MenuItem => {
                        let i = gtk_menu_item_new();
                        connect(i, b"activate\0", h_clicked as *const (), id);
                        i
                    }
                    Kind::CheckMenuItem => {
                        let i = gtk_check_menu_item_new();
                        connect(i, b"toggled\0", h_menu_toggled as *const (), id);
                        i
                    }
                    _ => gtk_separator_menu_item_new(),
                };
                gtk_menu_shell_append(pw.map_or(NULL, |p| p.inner), item);
                gtk_widget_show(item);
                w.w = item;
                w.inner = item;
                WIDGETS.with(|m| m.borrow_mut().insert(id, w));
                return Ok(());
            }
            _ => return Err(Error::Unsupported),
        }
        if kind != Kind::Window {
            let Some(p) = pw else {
                gtk_widget_destroy(w.w); // never added to anything: release the floating ref
                return Err(Error::InvalidHandle);
            };
            put(&p, w.w);
        }
        WIDGETS.with(|m| m.borrow_mut().insert(id, w));
        Ok(())
    }
}

unsafe fn set_inner(id: WidgetId, w: &W, prop: &Prop) {
    unsafe {
        match prop {
            Prop::Text(t) => set_text(w, t),
            Prop::Tooltip(t) => set_tooltip(w, t),
            Prop::Placeholder(t) => set_placeholder(w, t),
            Prop::Enabled(e) => gtk_widget_set_sensitive(w.w, *e as c_int),
            Prop::Visible(v) => set_visible(w, *v),
            Prop::Checked(c) => set_checked(w, *c),
            Prop::Value(v) => set_value(w, *v),
            Prop::Range { min, max, step } => set_range(w, *min, *max, *step),
            Prop::Items(items) => set_items(w, items),
            Prop::Selected(sel) => set_selected(w, *sel),
            Prop::Bounds(r) => set_bounds(id, w, r),
            Prop::Orientation(o) if w.kind == Kind::Sash => set_orientation(id, w, *o),
            Prop::Image(img) => set_image(w, img),
            Prop::Accel(s) => set_accel(id, w, s),
            Prop::ReadOnly(ro) => set_read_only(w, *ro),
            Prop::Indeterminate(on) => set_indeterminate(id, w, *on),
            Prop::Monospace(on) => set_monospace(w, *on),
            Prop::Wrap(on) if w.kind == Kind::TextArea => set_wrap(w, *on),
            Prop::Position { x, y } if w.kind == Kind::Window => set_position(id, w, *x, *y),
            Prop::MinSize(m) if w.kind == Kind::Window => set_min_size(w, m),
            Prop::Resizable(r) => set_resizable(id, w, *r),
            Prop::Columns(cols) if w.kind == Kind::Table => set_columns(id, w, cols),
            Prop::Rows(rows) if w.kind == Kind::Table => set_rows(w, rows),
            Prop::SortIndicator(si) if w.kind == Kind::Table => set_sort_indicator(w, si),
            Prop::TreeRows(rows) if w.kind == Kind::Tree => set_tree_rows(w, rows),
            Prop::TreeSelected(node) if w.kind == Kind::Tree => set_tree_selected(w, node),
            Prop::Focus => gtk_widget_grab_focus(if has_inner(w.kind) { w.inner } else { w.w }),
            _ => {}
        }
    }
}

/// `Prop::Text`.
unsafe fn set_text(w: &W, t: &&str) {
    unsafe {
        let c = cs(t);
        match w.kind {
            Kind::Window => gtk_window_set_title(w.w, c.as_ptr()),
            Kind::Label => gtk_label_set_text(w.w, c.as_ptr()),
            Kind::Button | Kind::CheckBox | Kind::RadioButton => {
                let m = cs(&crate::mnemonic::to_gtk_mnemonic(t));
                gtk_button_set_use_underline(w.w, 1);
                gtk_button_set_label(w.w, m.as_ptr())
            }
            Kind::TextInput | Kind::PasswordInput => {
                if from_c(gtk_entry_get_text(w.w)) != *t {
                    gtk_entry_set_text(w.w, c.as_ptr());
                }
            }
            Kind::TextArea => {
                let buf = gtk_text_view_get_buffer(w.inner);
                let (mut a, mut z) = (TextIter::new(), TextIter::new());
                gtk_text_buffer_get_start_iter(buf, &mut a);
                gtk_text_buffer_get_end_iter(buf, &mut z);
                let p = gtk_text_buffer_get_text(buf, &a, &z, 1);
                let cur = from_c(p);
                g_free(p as P);
                if cur != *t {
                    gtk_text_buffer_set_text(buf, c.as_ptr(), -1);
                }
            }
            Kind::GroupBox => gtk_frame_set_label(w.w, c.as_ptr()),
            Kind::Page => {
                if let Some(nb) = w.parent.and_then(get) {
                    gtk_notebook_set_tab_label_text(nb.w, w.w, c.as_ptr());
                }
            }
            Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem => {
                let m = cs(&crate::mnemonic::to_gtk_mnemonic(t));
                gtk_menu_item_set_use_underline(w.w, 1);
                gtk_menu_item_set_label(w.w, m.as_ptr())
            }
            _ => {}
        }
    }
}

/// `Prop::Tooltip`.
unsafe fn set_tooltip(w: &W, t: &&str) {
    unsafe {
        // the CString must outlive the call: a temporary in an `else` block tail would not
        let c = cs(t);
        gtk_widget_set_tooltip_text(
            w.w,
            if t.is_empty() {
                std::ptr::null()
            } else {
                c.as_ptr()
            },
        );
    }
}

/// `Prop::Placeholder`.
unsafe fn set_placeholder(w: &W, t: &&str) {
    unsafe {
        if matches!(w.kind, Kind::TextInput | Kind::PasswordInput) {
            gtk_entry_set_placeholder_text(w.w, cs(t).as_ptr());
        }
    }
}

/// `Prop::Visible`.
unsafe fn set_visible(w: &W, v: bool) {
    unsafe {
        match (w.kind, v) {
            // A page is shown or hidden by its notebook (hiding the child would also remove the
            // tab, which no other backend does); `show_pages` handles a notebook shown late.
            (Kind::Page, _) => {}
            (Kind::Tabs, true) => {
                gtk_widget_show(w.w);
                show_pages(w.w);
            }
            (_, true) => gtk_widget_show(w.w),
            (_, false) => gtk_widget_hide(w.w),
        }
    }
}

/// `Prop::Checked`.
unsafe fn set_checked(w: &W, c: bool) {
    unsafe {
        match w.kind {
            Kind::CheckBox | Kind::RadioButton => gtk_toggle_button_set_active(w.w, c as c_int),
            Kind::CheckMenuItem => gtk_check_menu_item_set_active(w.w, c as c_int),
            _ => {}
        }
    }
}

/// `Prop::Value`.
unsafe fn set_value(w: &W, v: f64) {
    unsafe {
        match w.kind {
            Kind::Slider => gtk_range_set_value(w.w, v),
            Kind::SpinBox => gtk_spin_button_set_value(w.w, v),
            Kind::ProgressBar => gtk_progress_bar_set_fraction(w.w, v.clamp(0.0, 1.0)),
            _ => {}
        }
    }
}

/// `Prop::Range`.
unsafe fn set_range(w: &W, min: f64, max: f64, step: f64) {
    unsafe {
        if max < min || min.is_nan() || max.is_nan() {
            return;
        }
        let step = if step > 0.0 { step } else { 1.0 };
        match w.kind {
            Kind::Slider => {
                gtk_range_set_range(w.w, min, max);
                gtk_range_set_increments(w.w, step, step * 10.0);
            }
            Kind::SpinBox => {
                gtk_spin_button_set_range(w.w, min, max);
                gtk_spin_button_set_increments(w.w, step, step * 10.0);
                gtk_spin_button_set_digits(w.w, digits_for(step));
            }
            _ => {}
        }
    }
}

/// `Prop::Items`.
unsafe fn set_items(w: &W, items: &&[String]) {
    unsafe {
        match w.kind {
            Kind::ComboBox => {
                gtk_combo_box_text_remove_all(w.w);
                for it in items.iter() {
                    gtk_combo_box_text_append_text(w.w, cs(it).as_ptr());
                }
            }
            Kind::ListBox => {
                // fill the store while the view is detached (the view reacts to every row)
                let tv = w.inner;
                let store = gtk_tree_view_get_model(tv);
                if store.is_null() {
                    return;
                }
                g_object_ref(store);
                gtk_tree_view_set_model(tv, NULL);
                gtk_list_store_clear(store);
                for it in items.iter() {
                    let mut iter = TreeIter::new();
                    gtk_list_store_append(store, &mut iter);
                    gtk_list_store_set(store, &mut iter, 0 as c_int, cs(it).as_ptr(), -1 as c_int);
                }
                gtk_tree_view_set_model(tv, store);
                g_object_unref(store);
            }
            _ => {}
        }
    }
}

/// `Prop::Selected`.
unsafe fn set_selected(w: &W, sel: Option<usize>) {
    unsafe {
        match w.kind {
            Kind::ComboBox => gtk_combo_box_set_active(w.w, sel.map_or(-1, |i| i as c_int)),
            Kind::Tabs => {
                if let Some(i) = sel {
                    gtk_notebook_set_current_page(w.w, i as c_int);
                }
            }
            Kind::Table | Kind::ListBox => {
                let selw = gtk_tree_view_get_selection(w.inner);
                match sel {
                    Some(i) => {
                        let path = gtk_tree_path_new_from_indices(i as c_int, -1 as c_int);
                        gtk_tree_selection_select_path(selw, path);
                        gtk_tree_view_scroll_to_cell(w.inner, path, NULL, 0, 0.0, 0.0);
                        gtk_tree_path_free(path);
                    }
                    None => gtk_tree_selection_unselect_all(selw),
                }
            }
            _ => {}
        }
    }
}

/// `Prop::Bounds`.
unsafe fn set_bounds(id: WidgetId, w: &W, r: &Rect) {
    unsafe {
        match w.kind {
            Kind::Window => {
                let mut nw = *w;
                nw.client = (r.w, r.h);
                if r.w > 0 && r.h > 0 {
                    nw.emitted = (r.w, r.h);
                }
                upd(id, |x| {
                    x.client = nw.client;
                    x.emitted = nw.emitted
                });
                apply_window_size(&nw);
            }
            Kind::Page
            | Kind::Menu
            | Kind::MenuBar
            | Kind::MenuItem
            | Kind::CheckMenuItem
            | Kind::MenuSeparator => {}
            _ => {
                let parent = gtk_widget_get_parent(w.w);
                if !parent.is_null() {
                    gtk_fixed_move(parent, w.w, r.x, r.y);
                }
                let (mut want_w, mut want_h) = (r.w.max(0), r.h.max(0));
                if want_w < TINY_ALLOC || want_h < TINY_ALLOC || w.kind == Kind::Tabs {
                    // GTK warns (and draws garbage) when a widget is allocated less than its
                    // own border and padding (a notebook: its tabs); never go below the widget's real minimum
                    let (min_w, min_h) = min_request(w.w);
                    want_w = want_w.max(min_w);
                    want_h = want_h.max(min_h);
                }
                gtk_widget_set_size_request(w.w, want_w, want_h);
                if w.kind == Kind::Sash {
                    upd(id, |x| x.sash_pos = if x.sash_v { r.y } else { r.x });
                }
            }
        }
    }
}

/// `Prop::Orientation`.
unsafe fn set_orientation(id: WidgetId, w: &W, o: Orientation) {
    unsafe {
        let v = o == Orientation::Vertical;
        upd(id, |x| x.sash_v = v);
        // a horizontal splitter has a vertical sash, so its line is a vertical separator
        let sep = gtk_bin_get_child(w.w);
        if !sep.is_null() {
            gtk_orientable_set_orientation(sep, if v { ORIENT_H } else { ORIENT_V });
        }
    }
}

/// `Prop::Image`.
unsafe fn set_image(w: &W, img: &Option<&ImageData>) {
    unsafe {
        if w.kind != Kind::Image {
            return;
        }
        match img {
            Some(d) if d.is_valid() => {
                let pb = gdk_pixbuf_new(0, 1, 8, d.w as c_int, d.h as c_int);
                if pb.is_null() {
                    return;
                }
                let stride = gdk_pixbuf_get_rowstride(pb) as usize;
                let px = gdk_pixbuf_get_pixels(pb);
                let row = d.w as usize * 4;
                for y in 0..d.h as usize {
                    std::ptr::copy_nonoverlapping(
                        d.rgba.as_ptr().add(y * row),
                        px.add(y * stride),
                        row,
                    );
                }
                gtk_image_set_from_pixbuf(w.w, pb);
                g_object_unref(pb);
            }
            _ => gtk_image_clear(w.w),
        }
    }
}

/// `Prop::Accel`.
unsafe fn set_accel(id: WidgetId, w: &W, s: &&str) {
    unsafe {
        let Some(win) = w.win.and_then(get) else {
            return;
        };
        if w.accel_key != (0, 0) {
            gtk_widget_remove_accelerator(w.w, win.accel_group, w.accel_key.0, w.accel_key.1);
            upd(id, |x| x.accel_key = (0, 0));
        }
        if let Some(a) = Accel::parse(s) {
            let key = keyval(&a.key);
            if key != 0 {
                let mods = (if a.ctrl { MOD_CTRL } else { 0 })
                    | (if a.shift { MOD_SHIFT } else { 0 })
                    | (if a.alt { MOD_ALT } else { 0 });
                gtk_widget_add_accelerator(
                    w.w,
                    c"activate".as_ptr(),
                    win.accel_group,
                    key,
                    mods,
                    ACCEL_VISIBLE,
                );
                upd(id, |x| x.accel_key = (key, mods));
            }
        }
    }
}

/// `Prop::ReadOnly`.
unsafe fn set_read_only(w: &W, ro: bool) {
    unsafe {
        match w.kind {
            Kind::TextInput | Kind::PasswordInput => gtk_editable_set_editable(w.w, !ro as c_int),
            Kind::TextArea => gtk_text_view_set_editable(w.inner, !ro as c_int),
            _ => {}
        }
    }
}

/// `Prop::Indeterminate`.
unsafe fn set_indeterminate(id: WidgetId, w: &W, on: bool) {
    unsafe {
        if w.kind != Kind::ProgressBar {
            return;
        }
        let have = PULSES.with(|p| p.borrow().contains_key(&id));
        if on && !have {
            let src = g_timeout_add(100, pulse_cb, data(id));
            PULSES.with(|p| p.borrow_mut().insert(id, src));
        } else if !on && have {
            if let Some(src) = PULSES.with(|p| p.borrow_mut().remove(&id)) {
                g_source_remove(src);
            }
            gtk_progress_bar_set_fraction(w.w, 0.0);
        }
    }
}

/// `Prop::Monospace`.
unsafe fn set_monospace(w: &W, on: bool) {
    unsafe {
        // the theme's "monospace" style class is what gtk_text_view_set_monospace uses too
        let target = match w.kind {
            Kind::TextArea => w.inner,
            Kind::TextInput | Kind::PasswordInput => w.w,
            _ => return,
        };
        let sc = gtk_widget_get_style_context(target);
        if on {
            gtk_style_context_add_class(sc, c"monospace".as_ptr());
        } else {
            gtk_style_context_remove_class(sc, c"monospace".as_ptr());
        }
    }
}

/// `Prop::Wrap`.
unsafe fn set_wrap(w: &W, on: bool) {
    unsafe {
        gtk_text_view_set_wrap_mode(w.inner, if on { WRAP_WORD_CHAR } else { WRAP_NONE });
    }
}

/// `Prop::Position`.
unsafe fn set_position(id: WidgetId, w: &W, x: i32, y: i32) {
    unsafe {
        // remember it so the configure event that follows is not reported as a user move
        upd(id, |win| win.wpos = Some((x, y)));
        gtk_window_move(w.w, x, y)
    }
}

/// `Prop::MinSize`.
unsafe fn set_min_size(w: &W, m: &Size) {
    unsafe {
        // hints apply to the whole content (menu bar + client area); -1 = unset
        let mut mb = 0;
        if !w.menubar.is_null() {
            let (mut min, mut nat) = (Req::default(), Req::default());
            gtk_widget_get_preferred_size(w.menubar, &mut min, &mut nat);
            mb = nat.h;
        }
        let g = Geometry {
            min_width: if m.w > 0 { m.w } else { -1 },
            min_height: if m.h > 0 { m.h + mb } else { -1 },
            ..Default::default()
        };
        gtk_window_set_geometry_hints(w.w, NULL, &g, HINT_MIN_SIZE);
    }
}

/// `Prop::Resizable`.
unsafe fn set_resizable(id: WidgetId, w: &W, r: bool) {
    unsafe {
        if w.kind == Kind::Window {
            gtk_window_set_resizable(w.w, r as c_int);
            upd(id, |x| x.resizable = r);
            if let Some(nw) = get(id) {
                if r {
                    gtk_widget_set_size_request(nw.view, -1, -1);
                } else {
                    apply_window_size(&nw);
                }
            }
        }
    }
}

/// `Prop::Columns`.
unsafe fn set_columns(id: WidgetId, w: &W, cols: &&[Column]) {
    unsafe {
        let tv = w.inner;
        let n = cols.len().max(1);
        let mut types = vec![G_TYPE_STRING; n];
        let store = gtk_list_store_newv(n as c_int, types.as_mut_ptr());
        gtk_tree_view_set_model(tv, store);
        g_object_unref(store);
        let list = gtk_tree_view_get_columns(tv);
        let mut l = list;
        while !l.is_null() {
            gtk_tree_view_remove_column(tv, (*l).data);
            l = (*l).next;
        }
        g_list_free(list);
        for (i, c) in cols.iter().enumerate() {
            let col = gtk_tree_view_column_new();
            gtk_tree_view_column_set_title(col, cs(&c.title).as_ptr());
            let r = gtk_cell_renderer_text_new();
            let xalign: f32 = match c.align {
                ColumnAlign::Left => 0.0,
                ColumnAlign::Center => 0.5,
                ColumnAlign::Right => 1.0,
            };
            g_object_set(
                r,
                c"xalign".as_ptr(),
                xalign as c_double,
                std::ptr::null::<c_char>(),
            );
            gtk_tree_view_column_pack_start(col, r, 1);
            gtk_tree_view_column_add_attribute(col, r, c"text".as_ptr(), i as c_int);
            gtk_tree_view_column_set_alignment(col, xalign);
            gtk_tree_view_column_set_resizable(col, 1);
            gtk_tree_view_column_set_sizing(col, 2);
            gtk_tree_view_column_set_fixed_width(col, c.width.max(MIN_COLUMN_WIDTH));
            gtk_tree_view_column_set_clickable(col, 1);
            let tok = ((id.0 << 16) | (i as u64 & 0xffff)) as usize as P;
            connect_raw(col, b"clicked\0", h_col_clicked as *const (), tok);
            gtk_tree_view_append_column(tv, col);
        }
        gtk_tree_view_set_headers_visible(tv, (!cols.is_empty()) as c_int);
    }
}

/// `Prop::Rows`.
unsafe fn set_rows(w: &W, rows: &&[Vec<String>]) {
    unsafe {
        let tv = w.inner;
        let store = gtk_tree_view_get_model(tv);
        if store.is_null() {
            return;
        }
        let ncols = gtk_tree_model_get_n_columns(store) as usize;
        g_object_ref(store);
        gtk_tree_view_set_model(tv, NULL);
        gtk_list_store_clear(store);
        for row in rows.iter() {
            let mut it = TreeIter::new();
            gtk_list_store_append(store, &mut it);
            for (c, cell) in row.iter().take(ncols).enumerate() {
                gtk_list_store_set(store, &mut it, c as c_int, cs(cell).as_ptr(), -1 as c_int);
            }
        }
        gtk_tree_view_set_model(tv, store);
        g_object_unref(store);
    }
}

/// `Prop::SortIndicator`.
unsafe fn set_sort_indicator(w: &W, si: &Option<(usize, bool)>) {
    unsafe {
        let mut i = 0;
        loop {
            let col = gtk_tree_view_get_column(w.inner, i);
            if col.is_null() {
                break;
            }
            match si {
                Some((c, asc)) if *c == i as usize => {
                    gtk_tree_view_column_set_sort_indicator(col, 1);
                    gtk_tree_view_column_set_sort_order(col, if *asc { 0 } else { 1 });
                }
                _ => gtk_tree_view_column_set_sort_indicator(col, 0),
            }
            i += 1;
        }
    }
}

/// `Prop::TreeRows`.
unsafe fn set_tree_rows(w: &W, rows: &&[TreeRow]) {
    unsafe {
        let tv = w.inner;
        let store = gtk_tree_view_get_model(tv);
        if store.is_null() {
            return;
        }
        let rows: &[TreeRow] = rows;
        // children of every row (and the roots) from the pre-order flattening
        let mut kids: Vec<Vec<usize>> = vec![vec![]; rows.len()];
        let mut roots: Vec<usize> = vec![];
        let mut stack: Vec<usize> = vec![];
        for (i, r) in rows.iter().enumerate() {
            stack.truncate((r.depth as usize).min(stack.len()));
            match stack.last() {
                Some(p) => kids[*p].push(i),
                None => roots.push(i),
            }
            stack.push(i);
        }
        // Fill the store while the view is detached (otherwise the view reacts to every row), and
        // insert each node's children last-to-first with `prepend`: `append` walks the sibling
        // list, which makes a node with thousands of children quadratic.
        g_object_ref(store);
        gtk_tree_view_set_model(tv, NULL);
        gtk_tree_store_clear(store);
        let mut iters: Vec<TreeIter> = vec![TreeIter::new(); rows.len()];
        let mut todo: Vec<(usize, Option<usize>)> = roots.iter().map(|r| (*r, None)).collect();
        while let Some((row, parent)) = todo.pop() {
            let r = &rows[row];
            let parent_iter =
                parent.map_or(std::ptr::null_mut(), |p| &mut iters[p] as *mut TreeIter);
            let mut it = TreeIter::new();
            gtk_tree_store_prepend(store, &mut it, parent_iter);
            gtk_tree_store_set(
                store,
                &mut it,
                0 as c_int,
                cs(&r.text).as_ptr(),
                1 as c_int,
                r.node,
                -1 as c_int,
            );
            if r.has_children && kids[row].is_empty() {
                // placeholder child so lazily loaded nodes show an expander
                let mut d = TreeIter::new();
                gtk_tree_store_append(store, &mut d, &mut it);
                gtk_tree_store_set(
                    store,
                    &mut d,
                    0 as c_int,
                    c"".as_ptr(),
                    1 as c_int,
                    0u64,
                    -1 as c_int,
                );
            }
            iters[row] = it;
            todo.extend(kids[row].iter().map(|k| (*k, Some(row))));
        }
        gtk_tree_view_set_model(tv, store);
        g_object_unref(store);
        // expand parents before children (the flattening is pre-order)
        for (r, it) in rows.iter().zip(iters.iter_mut()) {
            if r.expanded {
                let path = gtk_tree_model_get_path(store, it);
                gtk_tree_view_expand_row(tv, path, 0);
                gtk_tree_path_free(path);
            }
        }
    }
}

/// `Prop::TreeSelected`.
unsafe fn set_tree_selected(w: &W, node: &Option<u64>) {
    unsafe {
        let selw = gtk_tree_view_get_selection(w.inner);
        let store = gtk_tree_view_get_model(w.inner);
        match node {
            Some(n) if !store.is_null() => {
                let mut st: (u64, P) = (*n, NULL);
                gtk_tree_model_foreach(store, tree_find, &mut st as *mut _ as P);
                if st.1.is_null() {
                    gtk_tree_selection_unselect_all(selw);
                } else {
                    gtk_tree_view_expand_to_path(w.inner, st.1);
                    gtk_tree_selection_select_path(selw, st.1);
                    gtk_tree_view_scroll_to_cell(w.inner, st.1, NULL, 0, 0.0, 0.0);
                    gtk_tree_path_free(st.1);
                }
            }
            _ => gtk_tree_selection_unselect_all(selw),
        }
    }
}

/// Build the native widget(s) of a `Window` into `w`.
unsafe fn build_window(id: WidgetId, w: &mut W) {
    unsafe {
        let win = gtk_window_new(WINDOW_TOPLEVEL);
        let vbox = gtk_box_new(ORIENT_V, 0);
        let fixed = gtk_fixed_new();
        // The fixed's min size is the union of the children's size requests; behind an
        // EXTERNAL-policy scrolled window it no longer stops the user from shrinking the window.
        let sw = gtk_scrolled_window_new(NULL, NULL);
        gtk_scrolled_window_set_policy(sw, POLICY_EXTERNAL, POLICY_EXTERNAL);
        gtk_scrolled_window_set_shadow_type(sw, SHADOW_NONE);
        gtk_container_add(sw, fixed);
        let vp = gtk_bin_get_child(sw);
        if !vp.is_null() {
            gtk_viewport_set_shadow_type(vp, SHADOW_NONE);
        }
        gtk_box_pack_start(vbox, sw, 1, 1, 0);
        gtk_container_add(win, vbox);
        gtk_widget_show(vbox);
        gtk_widget_show(sw);
        gtk_widget_show(fixed);
        let ag = gtk_accel_group_new();
        gtk_window_add_accel_group(win, ag);
        // the window now holds its own reference; ours would leak the group (and every
        // accelerator closure in it) with each window
        g_object_unref(ag);
        connect(win, b"delete-event\0", h_delete as *const (), id);
        ctx_hooks(win, id);
        connect(sw, b"size-allocate\0", h_size_allocate as *const (), id);
        connect(win, b"configure-event\0", h_configure as *const (), id);
        w.view = sw;
        w.w = win;
        w.inner = fixed;
        w.outer = vbox;
        w.accel_group = ag;
    }
}

/// Build the native widget(s) of a `Label` into `w`.
unsafe fn build_label(w: &mut W) {
    unsafe {
        let l = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(l, 0.0);
        w.w = l;
        w.inner = l;
    }
}

/// Build the native widget(s) of a `Button` into `w`.
unsafe fn build_button(id: WidgetId, w: &mut W) {
    unsafe {
        w.w = gtk_button_new();
        connect(w.w, b"clicked\0", h_clicked as *const (), id);
        focus_hooks(w.w, id);
        ctx_hooks(w.w, id);
    }
}

/// Build the native widget(s) of a `CheckBox | Kind::RadioButton` into `w`.
unsafe fn build_check(id: WidgetId, kind: Kind, w: &mut W) {
    unsafe {
        w.w = if kind == Kind::CheckBox {
            gtk_check_button_new()
        } else {
            // The core owns exclusivity; the hidden sentinel keeps "no radio active" possible.
            let dummy = gtk_radio_button_new(NULL);
            g_object_ref_sink(dummy);
            w.extra = dummy;
            gtk_radio_button_new_from_widget(dummy)
        };
        connect(w.w, b"toggled\0", h_toggled as *const (), id);
        focus_hooks(w.w, id);
        ctx_hooks(w.w, id);
    }
}

/// Build the native widget(s) of a `TextInput | Kind::PasswordInput` into `w`.
unsafe fn build_entry(id: WidgetId, kind: Kind, w: &mut W) {
    unsafe {
        let e = gtk_entry_new();
        if kind == Kind::PasswordInput {
            gtk_entry_set_visibility(e, 0);
        }
        connect(e, b"changed\0", h_entry_changed as *const (), id);
        focus_hooks(e, id);
        w.w = e;
    }
}

/// Build the native widget(s) of a `TextArea` into `w`.
unsafe fn build_text_area(id: WidgetId, w: &mut W) {
    unsafe {
        let tv = gtk_text_view_new();
        gtk_text_view_set_wrap_mode(tv, WRAP_WORD_CHAR); // backend contract: wrap by default
        let buf = gtk_text_view_get_buffer(tv);
        connect(buf, b"changed\0", h_buffer_changed as *const (), id);
        focus_hooks(tv, id);
        w.w = scrolled(tv);
        w.inner = tv;
    }
}

/// Build the native widget(s) of a `ComboBox` into `w`.
unsafe fn build_combo(id: WidgetId, w: &mut W) {
    unsafe {
        let c = gtk_combo_box_text_new();
        connect(c, b"changed\0", h_combo_changed as *const (), id);
        ctx_hooks(c, id);
        focus_hooks(c, id);
        w.w = c;
    }
}

/// Build the native widget(s) of a `ListBox` into `w`: a headerless one-column tree view over a
/// list store. (A `GtkListBox` makes a widget per row and takes seconds, quadratically, to fill
/// once it is on screen; the tree view is virtual.)
unsafe fn build_list(id: WidgetId, w: &mut W) {
    unsafe {
        let tv = gtk_tree_view_new();
        gtk_tree_view_set_headers_visible(tv, 0);
        let sel = gtk_tree_view_get_selection(tv);
        gtk_tree_selection_set_mode(sel, 1);
        connect(sel, b"changed\0", h_sel_changed as *const (), id);
        connect(tv, b"row-activated\0", h_tv_activated as *const (), id);
        ctx_hooks(tv, id);
        focus_hooks(tv, id);
        let mut types = [G_TYPE_STRING];
        let store = gtk_list_store_newv(1, types.as_mut_ptr());
        gtk_tree_view_set_model(tv, store);
        g_object_unref(store);
        let col = gtk_tree_view_column_new();
        let r = gtk_cell_renderer_text_new();
        gtk_tree_view_column_pack_start(col, r, 1);
        gtk_tree_view_column_add_attribute(col, r, c"text".as_ptr(), 0);
        gtk_tree_view_append_column(tv, col);
        w.w = scrolled(tv);
        w.inner = tv;
    }
}

/// Build the native widget(s) of a `Table | Kind::Tree` into `w`.
unsafe fn build_tree_view(id: WidgetId, kind: Kind, w: &mut W) {
    unsafe {
        let tv = gtk_tree_view_new();
        let sel = gtk_tree_view_get_selection(tv);
        gtk_tree_selection_set_mode(sel, 1);
        connect(sel, b"changed\0", h_sel_changed as *const (), id);
        connect(tv, b"row-activated\0", h_tv_activated as *const (), id);
        focus_hooks(tv, id);
        if kind == Kind::Tree {
            gtk_tree_view_set_headers_visible(tv, 0);
            let mut types = [G_TYPE_STRING, G_TYPE_UINT64];
            let store = gtk_tree_store_newv(2, types.as_mut_ptr());
            gtk_tree_view_set_model(tv, store);
            g_object_unref(store);
            let col = gtk_tree_view_column_new();
            let r = gtk_cell_renderer_text_new();
            gtk_tree_view_column_pack_start(col, r, 1);
            gtk_tree_view_column_add_attribute(col, r, c"text".as_ptr(), 0);
            gtk_tree_view_append_column(tv, col);
            connect(tv, b"row-expanded\0", h_row_expanded as *const (), id);
            connect(tv, b"row-collapsed\0", h_row_collapsed as *const (), id);
        }
        w.w = scrolled(tv);
        w.inner = tv;
    }
}

/// Build the native widget(s) of a `Slider` into `w`.
unsafe fn build_slider(id: WidgetId, w: &mut W) {
    unsafe {
        let s = gtk_scale_new_with_range(ORIENT_H, 0.0, 100.0, 1.0);
        gtk_scale_set_draw_value(s, 0);
        connect(s, b"value-changed\0", h_value_changed as *const (), id);
        focus_hooks(s, id);
        w.w = s;
    }
}

/// Build the native widget(s) of a `SpinBox` into `w`.
unsafe fn build_spin(id: WidgetId, w: &mut W) {
    unsafe {
        let s = gtk_spin_button_new_with_range(0.0, 100.0, 1.0);
        connect(s, b"value-changed\0", h_value_changed as *const (), id);
        focus_hooks(s, id);
        w.w = s;
    }
}

/// Build the native widget(s) of a `Tabs` into `w`.
unsafe fn build_tabs(id: WidgetId, w: &mut W) {
    unsafe {
        let nb = gtk_notebook_new();
        ctx_hooks(nb, id);
        connect(nb, b"switch-page\0", h_switch_page as *const (), id);
        w.w = nb;
    }
}

/// Build the native widget(s) of a `GroupBox` into `w`.
unsafe fn build_group(w: &mut W) {
    unsafe {
        let f = gtk_frame_new(c"".as_ptr());
        let fixed = gtk_fixed_new();
        gtk_container_add(f, fixed);
        gtk_widget_show(fixed);
        w.w = f;
        w.inner = fixed;
    }
}

/// Build the native widget(s) of a `Sash` into `w`.
unsafe fn build_sash(id: WidgetId, w: &mut W) {
    unsafe {
        // a draggable strip; the core owns its position, we only report the pointer
        let eb = gtk_event_box_new();
        let sep = gtk_separator_new(ORIENT_V);
        gtk_container_add(eb, sep);
        gtk_widget_show(sep);
        // keyboard-operable: Tab reaches it, a click focuses it, arrows/Home/End move it
        gtk_widget_set_can_focus(eb, 1);
        gtk_widget_add_events(
            eb,
            EV_BUTTON_PRESS
                | EV_BUTTON_RELEASE
                | EV_BUTTON_MOTION
                | EV_POINTER_MOTION
                | EV_ENTER_NOTIFY
                | EV_KEY_PRESS,
        );
        focus_hooks(eb, id);
        connect(eb, b"focus-in-event\0", h_sash_focus as *const (), id);
        connect(eb, b"focus-out-event\0", h_sash_focus as *const (), id);
        connect(eb, b"key-press-event\0", h_sash_key as *const (), id);
        connect_after(eb, b"draw\0", h_sash_draw as *const (), id);
        connect(eb, b"button-press-event\0", h_sash_press as *const (), id);
        connect(eb, b"motion-notify-event\0", h_sash_motion as *const (), id);
        connect(
            eb,
            b"button-release-event\0",
            h_sash_release as *const (),
            id,
        );
        connect(eb, b"enter-notify-event\0", h_sash_enter as *const (), id);
        w.w = eb;
    }
}
