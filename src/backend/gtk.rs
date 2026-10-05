#![forbid(unsafe_code)]
//! GTK3 (Linux) backend on the gtk-rs crates (`gtk` 0.18 and what it re-exports: gdk, glib, atk,
//! cairo, pango, gdk-pixbuf). No FFI of our own and no `unsafe`.
//!
//! Every native widget is kept in a thread-local `id -> W` map (a cheap clone of refcounted
//! gtk-rs handles, never borrowed across a GTK call: GTK signals re-enter this module through the
//! core). User actions arrive via signal closures that capture only the widget id; programmatic
//! changes made inside `create/set/destroy` run under a guard that swallows the resulting signals.
//! Containers (Window, Page, GroupBox) are `GtkFixed`: the core's Rust layout places children
//! absolutely. Accessibility is native: GTK3 widgets already expose ATK (-> AT-SPI); `a11y_changed`
//! pushes the core's computed names/descriptions (e.g. "input labelled by preceding Label") into ATK.

use super::*;
use crate::a11y::A11yRole;
use crate::core;
use ::gtk::atk::prelude::*;
use ::gtk::glib::translate::IntoGlib;
use ::gtk as gt;
use gt::prelude::*;
use gt::{atk, cairo, gdk, gdk_pixbuf, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub struct Gtk;

#[derive(Clone)]
struct W {
    kind: Kind,
    /// The widget placed in the parent (GtkWindow, GtkButton, GtkScrolledWindow, GtkMenuItem ...).
    w: gt::Widget,
    /// Child container (GtkFixed / GtkMenu / menu bar) or the inner widget (GtkTextView, GtkListBox).
    inner: gt::Widget,
    /// Window: the vertical box holding menu bar + fixed.
    outer: Option<gt::Box>,
    accel_group: Option<gt::AccelGroup>,
    /// RadioButton: hidden sentinel sharing its group (GTK cannot deactivate a lone radio button).
    extra: Option<gt::Widget>,
    menubar: Option<gt::Widget>,
    parent: Option<WidgetId>,
    win: Option<WidgetId>,
    client: (i32, i32),
    emitted: (i32, i32),
    resizable: bool,
    accel_key: Option<(u32, gdk::ModifierType)>,
    /// Window: the scrolled window holding the fixed (its min size is not propagated to the toplevel).
    view: Option<gt::Widget>,
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
    static TIMERS: RefCell<HashMap<u64, glib::SourceId>> = RefCell::new(HashMap::new());
    static PULSES: RefCell<HashMap<WidgetId, glib::SourceId>> = RefCell::new(HashMap::new());
    static RESIZE_PENDING: RefCell<HashSet<WidgetId>> = RefCell::new(HashSet::new());
    static CHROME: RefCell<HashMap<u8, Size>> = RefCell::new(HashMap::new());
}

fn get(id: WidgetId) -> Option<W> {
    WIDGETS.with(|m| m.borrow().get(&id).cloned())
}
fn upd(id: WidgetId, f: impl FnOnce(&mut W)) {
    WIDGETS.with(|m| {
        if let Some(w) = m.borrow_mut().get_mut(&id) {
            f(w)
        }
    })
}
fn insert(id: WidgetId, w: W) {
    WIDGETS.with(|m| m.borrow_mut().insert(id, w));
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
fn emit(id: WidgetId, ev: Event) {
    if !suppressed() {
        core::event(id, ev);
    }
}
/// Run `f` on the widget if it really is a `T` (it always is, by construction of the kind).
fn with<T: IsA<gt::Widget>>(w: &gt::Widget, f: impl FnOnce(&T)) {
    if let Some(t) = w.downcast_ref::<T>() {
        f(t)
    }
}
fn stop() -> glib::Propagation {
    glib::Propagation::Stop
}
fn proceed() -> glib::Propagation {
    glib::Propagation::Proceed
}
fn as_window(w: &W) -> Option<&gt::Window> {
    w.w.downcast_ref::<gt::Window>()
}
/// Toplevel GtkWindow of the window that contains `id` (for dialog parents).
fn parent_gtk_window(id: Option<WidgetId>) -> Option<gt::Window> {
    let w = id.and_then(get)?;
    let win = w.win.and_then(get)?;
    win.w.downcast::<gt::Window>().ok()
}
fn uidx(i: i32) -> Option<usize> {
    (i >= 0).then_some(i as usize)
}

// ------------------------------------------------------------------ signal handlers

fn tree_node(model: &gt::TreeModel, it: &gt::TreeIter) -> u64 {
    model.value(it, 1).get::<u64>().unwrap_or(0)
}
fn path_index(path: &gt::TreePath) -> Option<usize> {
    path.indices().first().copied().and_then(uidx)
}

fn on_sel_changed(sel: &gt::TreeSelection, id: WidgetId) {
    let Some(w) = get(id) else { return };
    let cur = sel.selected();
    if w.kind == Kind::Tree {
        let node = cur.map_or(0, |(m, it)| tree_node(&m, &it));
        emit(id, Event::TreeSelected((node != 0).then_some(node)));
    } else {
        let row = cur.and_then(|(m, it)| m.path(&it).as_ref().and_then(path_index));
        emit(id, Event::Selected(row));
    }
}

fn on_tv_activated(tv: &gt::TreeView, path: &gt::TreePath, id: WidgetId) {
    let Some(w) = get(id) else { return };
    if w.kind == Kind::Tree {
        if let Some(model) = tv.model() {
            if let Some(it) = model.iter(path) {
                let n = tree_node(&model, &it);
                if n != 0 {
                    emit(id, Event::TreeActivated(n));
                }
            }
        }
    } else if let Some(i) = path_index(path) {
        emit(id, Event::Activated(i));
    }
}

fn on_row_expand(tv: &gt::TreeView, it: &gt::TreeIter, id: WidgetId, open: bool) {
    if suppressed() {
        return;
    }
    let Some(model) = tv.model() else { return };
    let node = tree_node(&model, it);
    if node != 0 {
        // Deferred: the app typically replaces the rows (lazy loading) in response.
        glib::idle_add_local_once(move || {
            if get(id).is_some() {
                core::event(id, Event::TreeExpanded(node, open));
            }
        });
    }
}

/// Window-client coordinates of a root-relative point.
fn client_xy(win: &W, root_x: f64, root_y: f64) -> (i32, i32) {
    let (ox, oy) = win.w.window().map_or((0, 0), |g| {
        let (_, x, y) = g.origin();
        (x, y)
    });
    win.w
        .translate_coordinates(&win.inner, root_x as i32 - ox, root_y as i32 - oy)
        .unwrap_or((0, 0))
}

fn ctx_hooks(obj: &impl IsA<gt::Widget>, id: WidgetId) {
    obj.connect_button_press_event(move |_, ev| {
        if ev.event_type() != gdk::EventType::ButtonPress || ev.button() != 3 || suppressed() {
            return proceed();
        }
        let Some(win) = get(id).and_then(|w| w.win).and_then(get) else {
            return proceed();
        };
        let (rx, ry) = ev.root();
        let (x, y) = client_xy(&win, rx, ry);
        core::event(id, Event::ContextMenu { x, y });
        stop()
    });
    obj.connect_popup_menu(move |w| {
        let Some(win) = get(id).and_then(|w| w.win).and_then(get) else {
            return false;
        };
        let (x, y) = w
            .translate_coordinates(&win.inner, w.allocated_width() / 2, w.allocated_height() / 2)
            .unwrap_or((0, 0));
        core::event(id, Event::ContextMenu { x, y });
        true
    });
}

fn focus_hooks(obj: &impl IsA<gt::Widget>, id: WidgetId) {
    obj.connect_focus_in_event(move |_, _| {
        emit(id, Event::Focus(true));
        proceed()
    });
    obj.connect_focus_out_event(move |_, _| {
        emit(id, Event::Focus(false));
        proceed()
    });
}

fn sash_key(id: WidgetId, ev: &gdk::EventKey) -> glib::Propagation {
    let Some(w) = get(id) else { return proceed() };
    let st = ev.state();
    if st.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::MOD1_MASK) {
        return proceed();
    }
    let big = st.contains(gdk::ModifierType::SHIFT_MASK);
    let (prev, next) = if big {
        (SashKey::PrevLarge, SashKey::NextLarge)
    } else {
        (SashKey::Prev, SashKey::Next)
    };
    let key = match ev.keyval().into_glib() {
        0xff50 | 0xff95 => SashKey::Min, // Home, KP_Home
        0xff57 | 0xff9c => SashKey::Max, // End, KP_End
        // the sash of a stacked (vertical) splitter moves up and down, otherwise left and right
        0xff52 | 0xff97 if w.sash_v => prev,   // Up, KP_Up
        0xff54 | 0xff99 if w.sash_v => next,   // Down, KP_Down
        0xff51 | 0xff96 if !w.sash_v => prev,  // Left, KP_Left
        0xff53 | 0xff98 if !w.sash_v => next,  // Right, KP_Right
        _ => return proceed(),
    };
    emit(id, Event::SashKey(key));
    stop()
}

fn connect_sash(eb: &gt::EventBox, id: WidgetId) {
    focus_hooks(eb, id);
    // The sash is a plain event box, which draws nothing when focused: repaint on focus changes
    // and draw the focus ring ourselves (after the child), over its separator.
    eb.connect_focus_in_event(|w, _| {
        w.queue_draw();
        proceed()
    });
    eb.connect_focus_out_event(|w, _| {
        w.queue_draw();
        proceed()
    });
    eb.connect_key_press_event(move |_, ev| sash_key(id, ev));
    eb.connect_local("draw", true, |args| {
        let w = args[0].get::<gt::Widget>().ok()?;
        let cr = args[1].get::<cairo::Context>().ok()?;
        if w.has_focus() {
            gt::render_focus(
                &w.style_context(),
                &cr,
                0.0,
                0.0,
                w.allocated_width() as f64,
                w.allocated_height() as f64,
            );
        }
        Some(false.into())
    });
    // Sash drag: the new leading-edge position is the position at press plus the pointer delta
    // (root coordinates, so it does not drift while the core moves the sash).
    eb.connect_button_press_event(move |w, ev| {
        if ev.event_type() != gdk::EventType::ButtonPress || ev.button() != 1 {
            return proceed();
        }
        let Some(s) = get(id) else { return proceed() };
        w.grab_focus();
        let (rx, ry) = ev.root();
        let root = if s.sash_v { ry } else { rx };
        upd(id, |s| s.drag = Some((root, s.sash_pos)));
        stop()
    });
    eb.connect_motion_notify_event(move |_, ev| {
        let Some(s) = get(id) else { return proceed() };
        let Some((start, pos0)) = s.drag else {
            return proceed();
        };
        if !ev.state().contains(gdk::ModifierType::BUTTON1_MASK) {
            upd(id, |s| s.drag = None);
            return proceed();
        }
        let (rx, ry) = ev.root();
        let root = if s.sash_v { ry } else { rx };
        emit(id, Event::SashDragged(pos0 + (root - start).round() as i32));
        stop()
    });
    eb.connect_button_release_event(move |_, ev| {
        if ev.button() != 1 {
            return proceed();
        }
        upd(id, |s| s.drag = None);
        stop()
    });
    eb.connect_enter_notify_event(move |w, _| {
        if let Some(s) = get(id) {
            let name = if s.sash_v { "row-resize" } else { "col-resize" };
            if let (Some(cur), Some(win)) = (gdk::Cursor::from_name(&w.display(), name), w.window()) {
                win.set_cursor(Some(&cur));
            }
        }
        proceed()
    });
}

// ------------------------------------------------------------------ helpers

fn blank(kind: Kind, w: &impl IsA<gt::Widget>, parent: Option<WidgetId>, win: Option<WidgetId>) -> W {
    let w: gt::Widget = w.clone().upcast();
    W {
        kind,
        inner: w.clone(),
        w,
        outer: None,
        accel_group: None,
        extra: None,
        menubar: None,
        parent,
        win,
        client: (0, 0),
        emitted: (0, 0),
        resizable: true,
        accel_key: None,
        view: None,
        wpos: None,
        sash_v: false,
        sash_pos: 0,
        drag: None,
    }
}

fn keyval(key: &str) -> u32 {
    let mut it = key.chars();
    if let (Some(c), None) = (it.next(), it.next()) {
        return gdk::keys::Key::from_unicode(c.to_lowercase().next().unwrap_or(c)).into_glib();
    }
    let k = key.to_ascii_uppercase();
    if let Some(n) = k.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
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

fn menubar_height(w: &W) -> i32 {
    w.menubar.as_ref().map_or(0, |m| m.preferred_size().1.height)
}

fn apply_window_size(w: &W) {
    let (cw, ch) = w.client;
    if cw <= 0 || ch <= 0 {
        return;
    }
    let mb = menubar_height(w);
    if !w.resizable {
        if let Some(v) = &w.view {
            v.set_size_request(cw, ch);
        }
    }
    with::<gt::Window>(&w.w, |win| win.resize(cw, ch + mb));
}

fn is_container(k: Kind) -> bool {
    matches!(k, Kind::Window | Kind::Page | Kind::GroupBox)
}

fn put(parent: &W, child: &gt::Widget) {
    with::<gt::Fixed>(&parent.inner, |f| f.put(child, 0, 0));
    child.show();
}

fn scrolled(child: &impl IsA<gt::Widget>) -> gt::ScrolledWindow {
    let sw = gt::ScrolledWindow::new(None::<&gt::Adjustment>, None::<&gt::Adjustment>);
    sw.set_shadow_type(gt::ShadowType::In);
    sw.set_policy(gt::PolicyType::Automatic, gt::PolicyType::Automatic);
    sw.add(child);
    child.show();
    sw
}

fn digits_for(step: f64) -> u32 {
    let (mut d, mut s) = (0, step.abs());
    while d < 6 && (s - s.round()).abs() > 1e-9 {
        s *= 10.0;
        d += 1;
    }
    d
}

/// Close (and so destroy) a toplevel without `gtk_widget_destroy`, which gtk-rs marks unsafe:
/// a realized GtkWindow closed with no vetoing delete-event handler destroys itself.
fn close_toplevel(win: &gt::Window) {
    win.realize();
    win.close();
}

/// Border/header size of a GroupBox (key 0) or Tabs (key 1): allocate a real, unmapped toplevel
/// once and compare outer and inner allocations (CSS-dependent, so measured, not guessed).
fn measure_chrome(key: u8) -> Size {
    if let Some(s) = CHROME.with(|c| c.borrow().get(&key).copied()) {
        return s;
    }
    let win = gt::Window::new(gt::WindowType::Toplevel);
    let page = gt::Fixed::new();
    let outer: gt::Widget = if key == 0 {
        let f = gt::Frame::new(Some("Xg"));
        f.add(&page);
        f.upcast()
    } else {
        let nb = gt::Notebook::new();
        let lbl = gt::Label::new(Some("Xg"));
        lbl.show();
        nb.append_page(&page, Some(&lbl));
        nb.upcast()
    };
    win.add(&outer);
    win.show_all();
    win.realize();
    win.size_allocate(&gdk::Rectangle::new(0, 0, 400, 300));
    let s = Size::new(
        outer.allocated_width() - page.allocated_width(),
        outer.allocated_height() - page.allocated_height(),
    );
    close_toplevel(&win);
    CHROME.with(|c| c.borrow_mut().insert(key, s));
    s
}

fn has_inner(k: Kind) -> bool {
    matches!(k, Kind::TextArea | Kind::ListBox | Kind::Table | Kind::Tree)
}

/// ATK role to force for a node, when GTK's own would be wrong. Only roles whose ATK equivalent is
/// unambiguous are mapped; anything else keeps GTK's default.
fn atk_role_for(kind: Kind, role: A11yRole) -> Option<atk::Role> {
    use A11yRole as R;
    match kind {
        Kind::Sash => return Some(atk::Role::SplitPane),
        Kind::Table => return Some(atk::Role::Table),
        Kind::Tree => return Some(atk::Role::TreeTable),
        _ => {}
    }
    // an explicit override on a kind whose default role it is not
    if crate::a11y::default_role(kind) == role {
        return None;
    }
    Some(match role {
        R::Label => atk::Role::Label,
        R::Group => atk::Role::Grouping,
        R::Pane => atk::Role::Panel,
        R::Image => atk::Role::Image,
        R::ListBox => atk::Role::ListBox,
        R::TextInput => atk::Role::Entry,
        R::PasswordInput => atk::Role::PasswordText,
        R::MultilineTextInput => atk::Role::Text,
        R::Splitter => atk::Role::SplitPane,
        _ => return None,
    })
}

fn kind_atk_name(w: &W) -> bool {
    !matches!(w.kind, Kind::MenuSeparator)
}

fn text_of(buf: &gt::TextBuffer) -> String {
    let (a, z) = buf.bounds();
    buf.text(&a, &z, true).map(|s| s.to_string()).unwrap_or_default()
}

// ------------------------------------------------------------------ the backend

impl Backend for Gtk {
    fn init(app_name: &str) -> Result<()> {
        gt::init().map_err(|_| Error::Backend("GTK could not open a display".into()))?;
        glib::set_prgname(Some(app_name));
        glib::set_application_name(app_name);
        Ok(())
    }

    fn run() {
        gt::main()
    }

    fn quit() {
        gt::main_quit()
    }

    fn wake() {
        // idle_add (the Send variant) is thread-safe; the closure runs on the main thread.
        glib::idle_add(|| {
            core::drain_posted();
            glib::ControlFlow::Break
        });
    }

    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        Self::timer_stop(token);
        let dur = Duration::from_millis(millis.max(1) as u64);
        let src = if repeat {
            glib::timeout_add_local(dur, move || {
                if !TIMERS.with(|t| t.borrow().contains_key(&token)) {
                    return glib::ControlFlow::Break;
                }
                core::timer_fired(token);
                if TIMERS.with(|t| t.borrow().contains_key(&token)) {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            })
        } else {
            glib::timeout_add_local(dur, move || {
                if TIMERS.with(|t| t.borrow_mut().remove(&token)).is_some() {
                    core::timer_fired(token);
                }
                glib::ControlFlow::Break
            })
        };
        TIMERS.with(|t| t.borrow_mut().insert(token, src));
        Ok(())
    }

    fn timer_stop(token: u64) {
        if let Some(src) = TIMERS.with(|t| t.borrow_mut().remove(&token)) {
            src.remove();
        }
    }

    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
        let pw = parent.and_then(get);
        let win = if kind == Kind::Window {
            Some(id)
        } else {
            pw.as_ref().and_then(|p| p.win)
        };
        let ok = match kind {
            Kind::Window | Kind::PopupMenu => parent.is_none(),
            Kind::MenuBar => pw.as_ref().is_some_and(|p| p.kind == Kind::Window),
            Kind::Page => pw.as_ref().is_some_and(|p| p.kind == Kind::Tabs),
            Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => pw
                .as_ref()
                .is_some_and(|p| matches!(p.kind, Kind::MenuBar | Kind::Menu | Kind::PopupMenu)),
            _ => pw.as_ref().is_some_and(|p| is_container(p.kind)),
        };
        if !ok {
            return Err(Error::InvalidHandle);
        }
        guarded(|| create_inner(id, kind, parent, pw, win))
    }

    fn destroy(id: WidgetId) {
        let Some(w) = get(id) else { return };
        guarded(|| {
            if let Some(src) = PULSES.with(|p| p.borrow_mut().remove(&id)) {
                src.remove();
            }
            // forget the id first so late signals/idles find nothing
            WIDGETS.with(|m| m.borrow_mut().remove(&id));
            let parent = w.parent.and_then(get);
            if let Some(p) = &parent {
                if w.kind == Kind::MenuBar && p.menubar.as_ref() == Some(&w.w) {
                    upd(w.parent.unwrap(), |p| p.menubar = None);
                }
            }
            match w.kind {
                Kind::Window => {
                    if let Some(win) = as_window(&w) {
                        close_toplevel(win);
                    }
                }
                Kind::PopupMenu => {
                    with::<gt::Menu>(&w.w, |m| m.popdown());
                }
                _ => {
                    // Detach from the parent container: the last reference then finalizes the
                    // widget (gtk_widget_destroy is unsafe in gtk-rs).
                    if let Some(c) = w.w.parent().and_then(|p| p.downcast::<gt::Container>().ok()) {
                        c.remove(&w.w);
                    }
                }
            }
        });
    }

    fn set(id: WidgetId, prop: &Prop) {
        let Some(w) = get(id) else { return };
        guarded(|| set_inner(id, &w, prop));
    }

    fn preferred_size(id: WidgetId) -> Size {
        let Some(w) = get(id) else {
            return Size::default();
        };
        if is_container(w.kind) || w.kind == Kind::Tabs {
            return Size::default();
        }
        // Ignore any size request we pushed earlier through Bounds.
        let (rw, rh) = w.w.size_request();
        w.w.set_size_request(-1, -1);
        let nat = w.w.preferred_size().1;
        w.w.set_size_request(rw, rh);
        let mut s = Size::new(nat.width, nat.height);
        match w.kind {
            Kind::TextArea => s = Size::new(s.w.max(240), s.h.max(100)),
            Kind::ListBox => s = Size::new(s.w.max(160), s.h.max(100)),
            Kind::Table | Kind::Tree => s = Size::new(s.w.max(240), s.h.max(140)),
            Kind::Slider | Kind::ProgressBar => s.w = s.w.max(160),
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
        // the GtkWidget* pointer value (as_ptr is a safe accessor)
        get(id).map(|w| NativeHandle::Gtk(w.w.as_ptr() as usize))
    }

    fn message_box(parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        let pw = parent_gtk_window(parent);
        let ty = match spec.kind {
            MessageKind::Info => gt::MessageType::Info,
            MessageKind::Warning => gt::MessageType::Warning,
            MessageKind::Error => gt::MessageType::Error,
            MessageKind::Question => gt::MessageType::Question,
        };
        let d = gt::MessageDialog::new(
            pw.as_ref(),
            gt::DialogFlags::MODAL,
            ty,
            gt::ButtonsType::None,
            &spec.text,
        );
        d.set_title(&spec.title);
        let r = |n: i32| gt::ResponseType::Other(n as u16);
        let (default_ans, default_resp) = match spec.buttons {
            Buttons::Ok => {
                d.add_button("OK", r(1));
                (Answer::Ok, 1)
            }
            Buttons::OkCancel => {
                d.add_button("Cancel", r(2));
                d.add_button("OK", r(1));
                (Answer::Cancel, 1)
            }
            Buttons::YesNo => {
                d.add_button("No", r(4));
                d.add_button("Yes", r(3));
                (Answer::No, 3)
            }
            Buttons::YesNoCancel => {
                d.add_button("Cancel", r(2));
                d.add_button("No", r(4));
                d.add_button("Yes", r(3));
                (Answer::Cancel, 3)
            }
        };
        d.set_default_response(r(default_resp));
        let resp = d.run();
        d.close();
        match resp {
            gt::ResponseType::Other(1) => Answer::Ok,
            gt::ResponseType::Other(3) => Answer::Yes,
            gt::ResponseType::Other(4) => Answer::No,
            gt::ResponseType::Other(2) => Answer::Cancel,
            _ => default_ans,
        }
    }

    fn file_dialog(parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String> {
        let pw = parent_gtk_window(parent);
        let (action, accept) = match spec.mode {
            FileMode::Open | FileMode::OpenMany => (gt::FileChooserAction::Open, "Open"),
            FileMode::Save => (gt::FileChooserAction::Save, "Save"),
            FileMode::PickFolder => (gt::FileChooserAction::SelectFolder, "Select"),
        };
        let d = gt::FileChooserDialog::with_buttons(
            Some(&spec.title),
            pw.as_ref(),
            action,
            &[
                ("Cancel", gt::ResponseType::Cancel),
                (accept, gt::ResponseType::Accept),
            ],
        );
        d.set_select_multiple(spec.mode == FileMode::OpenMany);
        d.set_do_overwrite_confirmation(true);
        if let Some(dir) = &spec.initial_dir {
            d.set_current_folder(dir);
        }
        if let (FileMode::Save, Some(n)) = (spec.mode, &spec.initial_name) {
            d.set_current_name(n);
        }
        if matches!(
            spec.mode,
            FileMode::Open | FileMode::OpenMany | FileMode::Save
        ) {
            for (label, exts) in &spec.filters {
                let f = gt::FileFilter::new();
                f.set_name(Some(label));
                for e in exts {
                    let e = e.trim_start_matches(['*', '.']);
                    if e.is_empty() {
                        f.add_pattern("*");
                        continue;
                    }
                    f.add_pattern(&format!("*.{}", e.to_lowercase()));
                    f.add_pattern(&format!("*.{}", e.to_uppercase()));
                }
                d.add_filter(f);
            }
        }
        let mut out = vec![];
        if d.run() == gt::ResponseType::Accept {
            out = d
                .filenames()
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
        }
        d.close();
        out
    }

    fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
        let Some(m) = get(menu) else { return };
        if m.kind != Kind::PopupMenu {
            return;
        }
        let Some(pm) = m.w.downcast_ref::<gt::Menu>() else {
            return;
        };
        pm.show_all();
        let nw = gdk::Gravity::NorthWest;
        match (at, parent_window.and_then(get)) {
            (Some((x, y)), Some(win)) => {
                let (tx, ty) = win.inner.translate_coordinates(&win.w, x, y).unwrap_or((x, y));
                match win.w.window() {
                    Some(gw) => pm.popup_at_rect(&gw, &gdk::Rectangle::new(tx, ty, 1, 1), nw, nw, None),
                    None => pm.popup_at_pointer(None),
                }
            }
            _ => pm.popup_at_pointer(None),
        }
        if !pm.is_visible() {
            return;
        }
        let lp = glib::MainLoop::new(None, false);
        let lp2 = lp.clone();
        let h = pm.connect_deactivate(move |_| lp2.quit());
        lp.run();
        pm.disconnect(h);
        // let a pending item activation be delivered before returning
        let mut n = 0;
        while gt::events_pending() && n < 20 {
            gt::main_iteration_do(false);
            n += 1;
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
            let target = if has_inner(w.kind) { &w.inner } else { &w.w };
            let Some(atk) = target.accessible() else {
                continue;
            };
            if let Some(l) = &node.name {
                if atk.name().as_deref() != Some(l.as_str()) {
                    atk.set_name(l);
                }
            }
            if let Some(d) = &node.description {
                atk.set_description(d);
            }
            // GTK's stock roles are close but not always the core's (a sash is a bare event box,
            // a flat table is reported as a tree table); app-set roles apply to all kinds.
            if let Some(r) = atk_role_for(w.kind, node.role) {
                if atk.role() != r {
                    atk.set_role(r);
                }
            }
        }
    }
}

fn create_inner(
    id: WidgetId,
    kind: Kind,
    parent: Option<WidgetId>,
    pw: Option<W>,
    win: Option<WidgetId>,
) -> Result<()> {
    let mut w: W;
    match kind {
        Kind::Window => {
            let window = gt::Window::new(gt::WindowType::Toplevel);
            let vbox = gt::Box::new(gt::Orientation::Vertical, 0);
            let fixed = gt::Fixed::new();
            // The fixed's min size is the union of the children's size requests; behind an
            // EXTERNAL-policy scrolled window it no longer stops the user from shrinking the window.
            let sw = gt::ScrolledWindow::new(None::<&gt::Adjustment>, None::<&gt::Adjustment>);
            sw.set_policy(gt::PolicyType::External, gt::PolicyType::External);
            sw.set_shadow_type(gt::ShadowType::None);
            sw.add(&fixed);
            if let Some(vp) = sw.child().and_then(|c| c.downcast::<gt::Viewport>().ok()) {
                vp.set_shadow_type(gt::ShadowType::None);
            }
            vbox.pack_start(&sw, true, true, 0);
            window.add(&vbox);
            vbox.show();
            sw.show();
            fixed.show();
            let ag = gt::AccelGroup::new();
            window.add_accel_group(&ag);
            // always veto the native close: the core destroys the window if the app allows it
            // (once the id is gone, `destroy` closes it for real and the veto steps aside)
            window.connect_delete_event(move |_, _| {
                if get(id).is_none() {
                    return proceed();
                }
                core::close_requested(id);
                stop()
            });
            ctx_hooks(&window, id);
            sw.connect_size_allocate(move |_, _| {
                if RESIZE_PENDING.with(|p| p.borrow_mut().insert(id)) {
                    glib::idle_add_local_once(move || {
                        RESIZE_PENDING.with(|p| p.borrow_mut().remove(&id));
                        if let Some(w) = get(id) {
                            let area = w.view.as_ref().unwrap_or(&w.inner);
                            let (cw, ch) = (area.allocated_width(), area.allocated_height());
                            if cw > 1 && ch > 1 && (cw, ch) != w.emitted {
                                upd(id, |w| w.emitted = (cw, ch));
                                core::event(id, Event::Resized { w: cw, h: ch });
                            }
                        }
                    });
                }
            });
            // Window moved (or resized): report the outer-frame position once it changed. The
            // first configure only records the initial placement.
            window.connect_configure_event(move |w, _| {
                let (x, y) = w.position();
                let Some(win) = get(id) else {
                    return false;
                };
                if win.wpos == Some((x, y)) {
                    return false;
                }
                upd(id, |w| w.wpos = Some((x, y)));
                if win.wpos.is_some() {
                    emit(id, Event::Moved { x, y });
                }
                false
            });
            w = blank(kind, &window, parent, win);
            w.inner = fixed.upcast();
            w.view = Some(sw.upcast());
            w.outer = Some(vbox);
            w.accel_group = Some(ag);
        }
        Kind::Label => {
            let l = gt::Label::new(Some(""));
            l.set_xalign(0.0);
            w = blank(kind, &l, parent, win);
        }
        Kind::Button => {
            let b = gt::Button::new();
            b.connect_clicked(move |_| emit(id, Event::Click));
            focus_hooks(&b, id);
            ctx_hooks(&b, id);
            w = blank(kind, &b, parent, win);
        }
        Kind::CheckBox | Kind::RadioButton => {
            let mut extra = None;
            let b: gt::ToggleButton = if kind == Kind::CheckBox {
                gt::CheckButton::new().upcast()
            } else {
                // The core owns exclusivity; the hidden sentinel keeps "no radio active" possible.
                let dummy = gt::RadioButton::new();
                let r = gt::RadioButton::from_widget(&dummy);
                extra = Some(dummy.upcast::<gt::Widget>());
                r.upcast()
            };
            b.connect_toggled(move |b| emit(id, Event::Toggled(b.is_active())));
            focus_hooks(&b, id);
            ctx_hooks(&b, id);
            w = blank(kind, &b, parent, win);
            w.extra = extra;
        }
        Kind::TextInput | Kind::PasswordInput => {
            let e = gt::Entry::new();
            if kind == Kind::PasswordInput {
                e.set_visibility(false);
            }
            e.connect_changed(move |e| emit(id, Event::Text(e.text().to_string())));
            focus_hooks(&e, id);
            w = blank(kind, &e, parent, win);
        }
        Kind::TextArea => {
            let tv = gt::TextView::new();
            tv.set_wrap_mode(gt::WrapMode::WordChar); // backend contract: wrap by default
            if let Some(buf) = tv.buffer() {
                buf.connect_changed(move |b| emit(id, Event::Text(text_of(b))));
            }
            focus_hooks(&tv, id);
            w = blank(kind, &scrolled(&tv), parent, win);
            w.inner = tv.upcast();
        }
        Kind::ComboBox => {
            let c = gt::ComboBoxText::new();
            c.connect_changed(move |c| emit(id, Event::Selected(c.active().map(|i| i as usize))));
            ctx_hooks(&c, id);
            focus_hooks(&c, id);
            w = blank(kind, &c, parent, win);
        }
        Kind::ListBox => {
            let lb = gt::ListBox::new();
            lb.set_activate_on_single_click(false);
            lb.connect_row_selected(move |_, row| {
                emit(id, Event::Selected(row.and_then(|r| uidx(r.index()))));
            });
            lb.connect_row_activated(move |_, row| {
                if let Some(i) = uidx(row.index()) {
                    emit(id, Event::Activated(i));
                }
            });
            ctx_hooks(&lb, id);
            focus_hooks(&lb, id);
            w = blank(kind, &scrolled(&lb), parent, win);
            w.inner = lb.upcast();
        }
        Kind::Table | Kind::Tree => {
            let tv = gt::TreeView::new();
            let sel = tv.selection();
            sel.set_mode(gt::SelectionMode::Single);
            sel.connect_changed(move |s| on_sel_changed(s, id));
            tv.connect_row_activated(move |tv, path, _| on_tv_activated(tv, path, id));
            focus_hooks(&tv, id);
            if kind == Kind::Tree {
                tv.set_headers_visible(false);
                let store = gt::TreeStore::new(&[glib::Type::STRING, glib::Type::U64]);
                tv.set_model(Some(&store));
                let col = gt::TreeViewColumn::new();
                let r = gt::CellRendererText::new();
                CellLayoutExt::pack_start(&col, &r, true);
                CellLayoutExt::add_attribute(&col, &r, "text", 0);
                tv.append_column(&col);
                tv.connect_row_expanded(move |tv, it, _| on_row_expand(tv, it, id, true));
                tv.connect_row_collapsed(move |tv, it, _| on_row_expand(tv, it, id, false));
            }
            w = blank(kind, &scrolled(&tv), parent, win);
            w.inner = tv.upcast();
        }
        Kind::PopupMenu => {
            let m = gt::Menu::new();
            insert(id, blank(kind, &m, parent, win));
            return Ok(());
        }
        Kind::Slider => {
            let s = gt::Scale::with_range(gt::Orientation::Horizontal, 0.0, 100.0, 1.0);
            s.set_draw_value(false);
            s.connect_value_changed(move |s| emit(id, Event::Value(s.value())));
            focus_hooks(&s, id);
            w = blank(kind, &s, parent, win);
        }
        Kind::ProgressBar => w = blank(kind, &gt::ProgressBar::new(), parent, win),
        Kind::SpinBox => {
            let s = gt::SpinButton::with_range(0.0, 100.0, 1.0);
            s.connect_value_changed(move |s| emit(id, Event::Value(s.value())));
            focus_hooks(&s, id);
            w = blank(kind, &s, parent, win);
        }
        Kind::Tabs => {
            let nb = gt::Notebook::new();
            ctx_hooks(&nb, id);
            nb.connect_switch_page(move |_, _, num| emit(id, Event::Selected(Some(num as usize))));
            w = blank(kind, &nb, parent, win);
        }
        Kind::Page => {
            let page = gt::Fixed::new();
            let nb = pw.as_ref().ok_or(Error::InvalidHandle)?;
            page.show();
            with::<gt::Notebook>(&nb.w, |nb| {
                nb.append_page(&page, None::<&gt::Widget>);
            });
            insert(id, blank(kind, &page, parent, win));
            return Ok(());
        }
        Kind::GroupBox => {
            let f = gt::Frame::new(Some(""));
            let fixed = gt::Fixed::new();
            f.add(&fixed);
            fixed.show();
            w = blank(kind, &f, parent, win);
            w.inner = fixed.upcast();
        }
        Kind::Image => w = blank(kind, &gt::Image::new(), parent, win),
        Kind::Sash => {
            // a draggable strip; the core owns its position, we only report the pointer
            let eb = gt::EventBox::new();
            let sep = gt::Separator::new(gt::Orientation::Vertical);
            eb.add(&sep);
            sep.show();
            // keyboard-operable: Tab reaches it, a click focuses it, arrows/Home/End move it
            eb.set_can_focus(true);
            eb.add_events(
                gdk::EventMask::BUTTON_PRESS_MASK
                    | gdk::EventMask::BUTTON_RELEASE_MASK
                    | gdk::EventMask::BUTTON_MOTION_MASK
                    | gdk::EventMask::POINTER_MOTION_MASK
                    | gdk::EventMask::ENTER_NOTIFY_MASK
                    | gdk::EventMask::KEY_PRESS_MASK,
            );
            connect_sash(&eb, id);
            w = blank(kind, &eb, parent, win);
        }
        Kind::MenuBar => {
            let bar = gt::MenuBar::new();
            let p = pw
                .as_ref()
                .filter(|p| p.kind == Kind::Window)
                .ok_or(Error::InvalidHandle)?;
            let outer = p.outer.as_ref().ok_or(Error::InvalidHandle)?;
            outer.pack_start(&bar, false, false, 0);
            outer.reorder_child(&bar, 0);
            bar.show();
            w = blank(kind, &bar, parent, win);
            let pid = parent.ok_or(Error::InvalidHandle)?;
            upd(pid, |p| p.menubar = Some(bar.clone().upcast()));
            insert(id, w);
            if let Some(p) = get(pid) {
                apply_window_size(&p);
            }
            return Ok(());
        }
        Kind::Menu => {
            let item = gt::MenuItem::new();
            let sub = gt::Menu::new();
            item.set_submenu(Some(&sub));
            if let Some(p) = &pw {
                with::<gt::MenuShell>(&p.inner, |s| s.append(&item));
            }
            item.show();
            w = blank(kind, &item, parent, win);
            w.inner = sub.upcast();
            insert(id, w);
            return Ok(());
        }
        Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
            let item: gt::MenuItem = match kind {
                Kind::MenuItem => {
                    let i = gt::MenuItem::new();
                    i.connect_activate(move |_| emit(id, Event::Click));
                    i
                }
                Kind::CheckMenuItem => {
                    let i = gt::CheckMenuItem::new();
                    i.connect_toggled(move |i| emit(id, Event::Toggled(i.is_active())));
                    i.upcast()
                }
                _ => gt::SeparatorMenuItem::new().upcast(),
            };
            if let Some(p) = &pw {
                with::<gt::MenuShell>(&p.inner, |s| s.append(&item));
            }
            item.show();
            insert(id, blank(kind, &item, parent, win));
            return Ok(());
        }
        _ => return Err(Error::Unsupported),
    }
    if kind != Kind::Window {
        put(pw.as_ref().ok_or(Error::InvalidHandle)?, &w.w);
    }
    insert(id, w);
    Ok(())
}

fn set_inner(id: WidgetId, w: &W, prop: &Prop) {
    match prop {
        Prop::Text(t) => match w.kind {
            Kind::Window => with::<gt::Window>(&w.w, |x| x.set_title(t)),
            Kind::Label => with::<gt::Label>(&w.w, |x| x.set_text(t)),
            Kind::Button | Kind::CheckBox | Kind::RadioButton => {
                with::<gt::Button>(&w.w, |x| {
                    x.set_use_underline(true);
                    x.set_label(&crate::text::to_gtk_mnemonic(t));
                });
            }
            Kind::TextInput | Kind::PasswordInput => with::<gt::Entry>(&w.w, |x| {
                if x.text() != *t {
                    x.set_text(t);
                }
            }),
            Kind::TextArea => with::<gt::TextView>(&w.inner, |tv| {
                if let Some(buf) = tv.buffer() {
                    if text_of(&buf) != *t {
                        buf.set_text(t);
                    }
                }
            }),
            Kind::GroupBox => with::<gt::Frame>(&w.w, |x| x.set_label(Some(t))),
            Kind::Page => {
                if let Some(nb) = w.parent.and_then(get) {
                    with::<gt::Notebook>(&nb.w, |n| n.set_tab_label_text(&w.w, t));
                }
            }
            Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem => {
                with::<gt::MenuItem>(&w.w, |x| {
                    x.set_use_underline(true);
                    x.set_label(&crate::text::to_gtk_mnemonic(t));
                });
            }
            _ => {}
        },
        Prop::Tooltip(t) => w.w.set_tooltip_text((!t.is_empty()).then_some(*t)),
        Prop::Placeholder(t) => {
            if matches!(w.kind, Kind::TextInput | Kind::PasswordInput) {
                with::<gt::Entry>(&w.w, |x| x.set_placeholder_text(Some(t)));
            }
        }
        Prop::Enabled(e) => w.w.set_sensitive(*e),
        Prop::Visible(v) => {
            if *v {
                w.w.show()
            } else {
                w.w.hide()
            }
        }
        Prop::Checked(c) => match w.kind {
            Kind::CheckBox | Kind::RadioButton => {
                with::<gt::ToggleButton>(&w.w, |x| x.set_active(*c))
            }
            Kind::CheckMenuItem => with::<gt::CheckMenuItem>(&w.w, |x| x.set_active(*c)),
            _ => {}
        },
        Prop::Value(v) => match w.kind {
            Kind::Slider => with::<gt::Range>(&w.w, |x| x.set_value(*v)),
            Kind::SpinBox => with::<gt::SpinButton>(&w.w, |x| x.set_value(*v)),
            Kind::ProgressBar => {
                with::<gt::ProgressBar>(&w.w, |x| x.set_fraction(v.clamp(0.0, 1.0)))
            }
            _ => {}
        },
        Prop::Range { min, max, step } => {
            if *max < *min || min.is_nan() || max.is_nan() {
                return;
            }
            let step = if *step > 0.0 { *step } else { 1.0 };
            match w.kind {
                Kind::Slider => with::<gt::Range>(&w.w, |x| {
                    x.set_range(*min, *max);
                    x.set_increments(step, step * 10.0);
                }),
                Kind::SpinBox => with::<gt::SpinButton>(&w.w, |x| {
                    x.set_range(*min, *max);
                    x.set_increments(step, step * 10.0);
                    x.set_digits(digits_for(step));
                }),
                _ => {}
            }
        }
        Prop::Items(items) => match w.kind {
            Kind::ComboBox => with::<gt::ComboBoxText>(&w.w, |c| {
                c.remove_all();
                for it in items.iter() {
                    c.append_text(it);
                }
            }),
            Kind::ListBox => with::<gt::ListBox>(&w.inner, |lb| {
                for row in lb.children() {
                    lb.remove(&row);
                }
                for it in items.iter() {
                    let lab = gt::Label::new(Some(it));
                    lab.set_xalign(0.0);
                    lb.insert(&lab, -1);
                }
                lb.show_all();
            }),
            _ => {}
        },
        Prop::Selected(sel) => match w.kind {
            Kind::ComboBox => with::<gt::ComboBox>(&w.w, |c| c.set_active(sel.map(|i| i as u32))),
            Kind::Tabs => {
                if let Some(i) = sel {
                    with::<gt::Notebook>(&w.w, |n| n.set_current_page(Some(*i as u32)));
                }
            }
            Kind::Table => with::<gt::TreeView>(&w.inner, |tv| {
                let selw = tv.selection();
                match sel {
                    Some(i) => {
                        let path = gt::TreePath::from_indicesv(&[*i as i32]);
                        selw.select_path(&path);
                        tv.scroll_to_cell(Some(&path), None::<&gt::TreeViewColumn>, false, 0.0, 0.0);
                    }
                    None => selw.unselect_all(),
                }
            }),
            Kind::ListBox => with::<gt::ListBox>(&w.inner, |lb| {
                match sel.and_then(|i| lb.row_at_index(i as i32)) {
                    Some(row) => lb.select_row(Some(&row)),
                    None => lb.unselect_all(),
                }
            }),
            _ => {}
        },
        Prop::Bounds(r) => match w.kind {
            Kind::Window => {
                let mut nw = w.clone();
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
                if let Some(fixed) = w.w.parent().and_then(|p| p.downcast::<gt::Fixed>().ok()) {
                    fixed.move_(&w.w, r.x, r.y);
                }
                w.w.set_size_request(r.w.max(0), r.h.max(0));
                if w.kind == Kind::Sash {
                    upd(id, |x| x.sash_pos = if x.sash_v { r.y } else { r.x });
                }
            }
        },
        Prop::Orientation(o) if w.kind == Kind::Sash => {
            let v = *o == Orientation::Vertical;
            upd(id, |x| x.sash_v = v);
            // a horizontal splitter has a vertical sash, so its line is a vertical separator
            with::<gt::EventBox>(&w.w, |eb| {
                if let Some(sep) = eb.child().and_then(|c| c.dynamic_cast::<gt::Orientable>().ok()) {
                    sep.set_orientation(if v {
                        gt::Orientation::Horizontal
                    } else {
                        gt::Orientation::Vertical
                    });
                }
            });
        }
        Prop::Image(img) => {
            if w.kind != Kind::Image {
                return;
            }
            with::<gt::Image>(&w.w, |im| match img {
                Some(d)
                    if d.w > 0 && d.h > 0 && d.rgba.len() == d.w as usize * d.h as usize * 4 =>
                {
                    let pb = gdk_pixbuf::Pixbuf::from_bytes(
                        &glib::Bytes::from(&d.rgba[..]),
                        gdk_pixbuf::Colorspace::Rgb,
                        true,
                        8,
                        d.w as i32,
                        d.h as i32,
                        d.w as i32 * 4,
                    );
                    im.set_from_pixbuf(Some(&pb));
                }
                _ => im.clear(),
            });
        }
        Prop::Accel(s) => {
            let Some(win) = w.win.and_then(get) else {
                return;
            };
            let Some(ag) = win.accel_group.as_ref() else {
                return;
            };
            if let Some((k, m)) = w.accel_key {
                w.w.remove_accelerator(ag, k, m);
                upd(id, |x| x.accel_key = None);
            }
            if let Some(a) = Accel::parse(s) {
                let key = keyval(&a.key);
                if key != 0 {
                    let mut mods = gdk::ModifierType::empty();
                    if a.ctrl {
                        mods |= gdk::ModifierType::CONTROL_MASK;
                    }
                    if a.shift {
                        mods |= gdk::ModifierType::SHIFT_MASK;
                    }
                    if a.alt {
                        mods |= gdk::ModifierType::MOD1_MASK;
                    }
                    w.w.add_accelerator("activate", ag, key, mods, gt::AccelFlags::VISIBLE);
                    upd(id, |x| x.accel_key = Some((key, mods)));
                }
            }
        }
        Prop::ReadOnly(ro) => match w.kind {
            Kind::TextInput | Kind::PasswordInput => {
                with::<gt::Entry>(&w.w, |x| x.set_editable(!*ro))
            }
            Kind::TextArea => with::<gt::TextView>(&w.inner, |x| x.set_editable(!*ro)),
            _ => {}
        },
        Prop::Indeterminate(on) => {
            if w.kind != Kind::ProgressBar {
                return;
            }
            let have = PULSES.with(|p| p.borrow().contains_key(&id));
            if *on && !have {
                let src = glib::timeout_add_local(Duration::from_millis(100), move || {
                    match get(id) {
                        Some(w) if PULSES.with(|p| p.borrow().contains_key(&id)) => {
                            with::<gt::ProgressBar>(&w.w, |p| p.pulse());
                            glib::ControlFlow::Continue
                        }
                        _ => glib::ControlFlow::Break,
                    }
                });
                PULSES.with(|p| p.borrow_mut().insert(id, src));
            } else if !*on && have {
                if let Some(src) = PULSES.with(|p| p.borrow_mut().remove(&id)) {
                    src.remove();
                }
                with::<gt::ProgressBar>(&w.w, |p| p.set_fraction(0.0));
            }
        }
        Prop::Monospace(on) => {
            // the theme's "monospace" style class is what gtk_text_view_set_monospace uses too
            let target = match w.kind {
                Kind::TextArea => &w.inner,
                Kind::TextInput | Kind::PasswordInput => &w.w,
                _ => return,
            };
            let sc = target.style_context();
            if *on {
                sc.add_class("monospace");
            } else {
                sc.remove_class("monospace");
            }
        }
        Prop::Wrap(on) if w.kind == Kind::TextArea => with::<gt::TextView>(&w.inner, |tv| {
            tv.set_wrap_mode(if *on {
                gt::WrapMode::WordChar
            } else {
                gt::WrapMode::None
            })
        }),
        Prop::Position { x, y } if w.kind == Kind::Window => {
            // remember it so the configure event that follows is not reported as a user move
            upd(id, |win| win.wpos = Some((*x, *y)));
            with::<gt::Window>(&w.w, |win| win.move_(*x, *y));
        }
        Prop::MinSize(m) if w.kind == Kind::Window => {
            // hints apply to the whole content (menu bar + client area); -1 = unset
            let mb = menubar_height(w);
            let g = gdk::Geometry::new(
                if m.w > 0 { m.w } else { -1 },
                if m.h > 0 { m.h + mb } else { -1 },
                0,
                0,
                0,
                0,
                0,
                0,
                0.0,
                0.0,
                gdk::Gravity::NorthWest,
            );
            with::<gt::Window>(&w.w, |win| {
                win.set_geometry_hints(None::<&gt::Widget>, Some(&g), gdk::WindowHints::MIN_SIZE)
            });
        }
        Prop::Resizable(r) => {
            if w.kind == Kind::Window {
                with::<gt::Window>(&w.w, |win| win.set_resizable(*r));
                upd(id, |x| x.resizable = *r);
                if let Some(nw) = get(id) {
                    if *r {
                        if let Some(v) = &nw.view {
                            v.set_size_request(-1, -1);
                        }
                    } else {
                        apply_window_size(&nw);
                    }
                }
            }
        }
        Prop::Columns(cols) if w.kind == Kind::Table => with::<gt::TreeView>(&w.inner, |tv| {
            let n = cols.len().max(1);
            let store = gt::ListStore::new(&vec![glib::Type::STRING; n]);
            tv.set_model(Some(&store));
            for c in tv.columns() {
                tv.remove_column(&c);
            }
            for (i, c) in cols.iter().enumerate() {
                let col = gt::TreeViewColumn::new();
                col.set_title(&c.title);
                let r = gt::CellRendererText::new();
                let xalign: f32 = match c.align {
                    ColumnAlign::Left => 0.0,
                    ColumnAlign::Center => 0.5,
                    ColumnAlign::Right => 1.0,
                };
                CellRendererExt::set_alignment(&r, xalign, 0.5);
                CellLayoutExt::pack_start(&col, &r, true);
                CellLayoutExt::add_attribute(&col, &r, "text", i as i32);
                col.set_alignment(xalign);
                col.set_resizable(true);
                col.set_sizing(gt::TreeViewColumnSizing::Fixed);
                col.set_fixed_width(c.width.max(1));
                col.set_clickable(true);
                col.connect_clicked(move |_| emit(id, Event::ColumnClicked(i)));
                tv.append_column(&col);
            }
            tv.set_headers_visible(!cols.is_empty());
        }),
        Prop::Rows(rows) if w.kind == Kind::Table => with::<gt::TreeView>(&w.inner, |tv| {
            let Some(store) = tv.model().and_then(|m| m.downcast::<gt::ListStore>().ok()) else {
                return;
            };
            let ncols = store.n_columns() as usize;
            tv.set_model(None::<&gt::TreeModel>);
            store.clear();
            for row in rows.iter() {
                let it = store.append();
                for (c, cell) in row.iter().take(ncols).enumerate() {
                    store.set(&it, &[(c as u32, &cell.as_str())]);
                }
            }
            tv.set_model(Some(&store));
        }),
        Prop::SortIndicator(si) if w.kind == Kind::Table => with::<gt::TreeView>(&w.inner, |tv| {
            for (i, col) in tv.columns().iter().enumerate() {
                match si {
                    Some((c, asc)) if *c == i => {
                        col.set_sort_indicator(true);
                        col.set_sort_order(if *asc {
                            gt::SortType::Ascending
                        } else {
                            gt::SortType::Descending
                        });
                    }
                    _ => col.set_sort_indicator(false),
                }
            }
        }),
        Prop::TreeRows(rows) if w.kind == Kind::Tree => with::<gt::TreeView>(&w.inner, |tv| {
            let Some(store) = tv.model().and_then(|m| m.downcast::<gt::TreeStore>().ok()) else {
                return;
            };
            store.clear();
            let mut stack: Vec<gt::TreeIter> = vec![];
            let mut iters: Vec<gt::TreeIter> = Vec::with_capacity(rows.len());
            for (i, r) in rows.iter().enumerate() {
                let depth = (r.depth as usize).min(stack.len());
                stack.truncate(depth);
                let parent = if depth > 0 { stack.get(depth - 1) } else { None };
                let it = store.append(parent);
                store.set(&it, &[(0, &r.text.as_str()), (1, &r.node)]);
                let has_kids = rows.get(i + 1).is_some_and(|n| n.depth > r.depth);
                if r.has_children && !has_kids {
                    // placeholder child so lazily loaded nodes show an expander
                    let d = store.append(Some(&it));
                    store.set(&d, &[(0, &""), (1, &0u64)]);
                }
                stack.push(it.clone());
                iters.push(it);
            }
            for (r, it) in rows.iter().zip(iters.iter()) {
                if r.expanded {
                    if let Some(p) = store.path(it) {
                        tv.expand_row(&p, false);
                    }
                }
            }
        }),
        Prop::TreeSelected(node) if w.kind == Kind::Tree => with::<gt::TreeView>(&w.inner, |tv| {
            let selw = tv.selection();
            let Some(store) = tv.model() else { return };
            let mut found: Option<gt::TreePath> = None;
            if let Some(n) = node {
                store.foreach(|m, path, it| {
                    if tree_node(m, it) == *n {
                        found = Some(path.clone());
                        return true;
                    }
                    false
                });
            }
            match found {
                Some(p) => {
                    tv.expand_to_path(&p);
                    selw.select_path(&p);
                    tv.scroll_to_cell(Some(&p), None::<&gt::TreeViewColumn>, false, 0.0, 0.0);
                }
                None => selw.unselect_all(),
            }
        }),
        Prop::Focus => if has_inner(w.kind) { &w.inner } else { &w.w }.grab_focus(),
        _ => {}
    }
}
