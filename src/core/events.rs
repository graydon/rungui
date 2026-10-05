//! User-callback slots and the dispatch of backend events into the registry and callbacks.

use super::lifecycle::destroy;
use super::post::layout_window;
use super::props::{is_alive, read, update};
use super::splitter::{sash_event, sash_key_event};
use super::{guarded, post, wake_if_scheduled, with};
use crate::backend::{Backend, Event, Kind, Native as B, Prop};
use crate::types::*;

/// Which user-callback slot an event maps to.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ev {
    Click,
    Text,
    Toggled,
    Selected,
    Value,
    Activated,
    Resized,
    Focus,
    ColumnClicked,
    TreeSelected,
    TreeActivated,
    TreeExpanded,
    ContextMenu,
    /// Splitter: position changed by the user (callback gets `Event::SashDragged(new_position)`).
    SashMoved,
    /// Window moved by the user.
    Moved,
}

impl Ev {
    fn of(e: &Event) -> Option<Ev> {
        Some(match e {
            Event::Click => Ev::Click,
            Event::Text(_) => Ev::Text,
            Event::Toggled(_) => Ev::Toggled,
            Event::Selected(_) => Ev::Selected,
            Event::Value(_) => Ev::Value,
            Event::Activated(_) => Ev::Activated,
            Event::Resized { .. } => Ev::Resized,
            Event::Focus(_) => Ev::Focus,
            Event::ColumnClicked(_) => Ev::ColumnClicked,
            Event::TreeSelected(_) => Ev::TreeSelected,
            Event::TreeActivated(_) => Ev::TreeActivated,
            Event::TreeExpanded(..) => Ev::TreeExpanded,
            Event::ContextMenu { .. } => Ev::ContextMenu,
            Event::SashDragged(_) | Event::SashKey(_) => Ev::SashMoved,
            Event::Moved { .. } => Ev::Moved,
            #[allow(unreachable_patterns)]
            _ => return None,
        })
    }
}

pub type Callback = Box<dyn FnMut(&Event)>;

pub fn set_callback(id: WidgetId, ev: Ev, cb: Callback) {
    update(id, false, |n| {
        n.cbs.insert(ev, cb);
    });
}

// ---------------------------------------------------------------- events from the backend

/// Backend entry point for user-initiated changes. Mirrors state, then runs the user callback.
pub fn event(id: WidgetId, ev: Event) {
    let Some(key) = Ev::of(&ev) else { return };
    if let Event::ContextMenu { x, y } = ev {
        context_menu_event(id, x, y, &ev);
        return;
    }
    if let Event::SashDragged(pos) = ev {
        sash_event(id, pos);
        return;
    }
    if let Event::SashKey(k) = ev {
        sash_key_event(id, k);
        return;
    }
    let mut unchecked = vec![];
    let go = with(|r| {
        let (kind, group) = {
            let n = r.nodes.get_mut(&id)?;
            match &ev {
                Event::Text(s) => n.text = s.clone(),
                Event::Toggled(b) => {
                    if let Some(c) = n.checked_mut() {
                        *c = *b
                    }
                }
                Event::Selected(s) => {
                    if n.table()
                        .is_some_and(|t| s.is_some_and(|i| i >= t.rows.len()))
                    {
                        return None;
                    }
                    if let Some(sel) = n.selection_mut() {
                        *sel = *s
                    }
                }
                Event::Activated(i) => {
                    if n.table().is_some_and(|t| *i >= t.rows.len()) {
                        return None;
                    }
                }
                Event::ColumnClicked(c) => {
                    if n.table().is_none_or(|t| *c >= t.columns.len()) {
                        return None;
                    }
                }
                Event::TreeSelected(s) => {
                    let t = n.tree_mut()?;
                    if s.is_some_and(|x| !t.nodes.contains_key(&x)) {
                        return None;
                    }
                    t.selected = *s;
                }
                Event::TreeActivated(x) => {
                    if !n.tree()?.nodes.contains_key(x) {
                        return None;
                    }
                }
                Event::TreeExpanded(x, b) => {
                    n.tree_mut()?.nodes.get_mut(x)?.expanded = *b;
                }
                Event::Value(v) => {
                    if let Some(r) = n.range_mut() {
                        r.value = *v
                    }
                }
                Event::Moved { x, y } => {
                    n.window_mut()?.position = Some((*x, *y));
                }
                Event::Resized { w, h } => {
                    // a misbehaving backend must not feed layout negative or absurd sizes
                    let sz = Size::new((*w).clamp(0, MAX_WINDOW_PX), (*h).clamp(0, MAX_WINDOW_PX));
                    let win = n.window_mut()?;
                    let same = win.client == sz;
                    win.client = sz;
                    win.explicit_size = true;
                    if !same {
                        r.dirty_layout.insert(id);
                    }
                    return Some(!same);
                }
                _ => {}
            }
            (n.kind, n.check().map_or(0, |c| c.group))
        };
        match ev {
            Event::Focus(true) => r.focus = Some(id),
            Event::Focus(false) if r.focus == Some(id) => r.focus = None,
            Event::Toggled(true) if kind == Kind::RadioButton && group != 0 => {
                unchecked = r.checked_in_group(group, id);
                for k in &unchecked {
                    if let Some(c) = r.nodes.get_mut(k).and_then(|n| n.check_mut()) {
                        c.checked = false;
                    }
                }
            }
            _ => {}
        }
        r.touch(id, false);
        Some(true)
    })
    .flatten();
    if go != Some(true) {
        return;
    }
    for u in unchecked {
        B::set(u, &Prop::Checked(false));
    }
    if key == Ev::Resized {
        layout_window(id);
    }
    // take the callback out, call it with no borrows held, put it back unless replaced
    let cb = with(|r| r.nodes.get_mut(&id).and_then(|n| n.cbs.remove(&key))).flatten();
    if let Some(mut cb) = cb {
        guarded(|| cb(&ev));
        with(|r| {
            if let Some(n) = r.nodes.get_mut(&id) {
                n.cbs.entry(key).or_insert(cb);
            }
        });
    }
    wake_if_scheduled();
}

/// `Event::ContextMenu`: find the nearest widget (self, then ancestors) with a menu or callback.
fn context_menu_event(id: WidgetId, x: i32, y: i32, ev: &Event) {
    let target = with(|r| {
        let mut cur = Some(id);
        while let Some(i) = cur {
            let n = r.nodes.get(&i)?;
            if !n.enabled {
                return None;
            }
            if n.context_menu.is_some() || n.cbs.contains_key(&Ev::ContextMenu) {
                return Some(i);
            }
            cur = n.parent;
        }
        None
    })
    .flatten();
    let Some(target) = target else { return };
    // the app may rebuild/enable items (or even attach a different menu) before it is shown
    let cb = with(|r| {
        r.nodes
            .get_mut(&target)
            .and_then(|n| n.cbs.remove(&Ev::ContextMenu))
    })
    .flatten();
    if let Some(mut cb) = cb {
        guarded(|| cb(ev));
        with(|r| {
            if let Some(n) = r.nodes.get_mut(&target) {
                n.cbs.entry(Ev::ContextMenu).or_insert(cb);
            }
        });
    }
    let menu = read(target, |n| n.context_menu).flatten();
    if let Some(m) = menu {
        let win = with(|r| r.window_of(target)).flatten();
        popup_menu(m, win, Some((x, y)));
    }
    wake_if_scheduled();
}

/// Pop up `menu` (a live `Kind::PopupMenu`; anything else is ignored). Blocks until dismissed.
pub fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
    if read(menu, |n| n.kind == Kind::PopupMenu) != Some(true) {
        return;
    }
    let win = parent_window.filter(|w| read(*w, |n| n.kind == Kind::Window) == Some(true));
    B::popup_menu(menu, win, at);
    wake_if_scheduled();
}

/// Backend entry point: the user asked to close window `id` (title-bar X, Alt-F4, Cmd-W...).
/// The backend must veto the native close; the core destroys the window on the next loop turn
/// if the app's `on_close` handler allows it (default: allow).
pub fn close_requested(id: WidgetId) {
    let cb = with(|r| {
        r.nodes
            .get_mut(&id)
            .and_then(|n| n.window_mut()?.on_close.take())
    })
    .flatten();
    let mut allow = true;
    if let Some(mut cb) = cb {
        guarded(|| allow = cb());
        with(|r| {
            if let Some(w) = r.nodes.get_mut(&id).and_then(|n| n.window_mut()) {
                if w.on_close.is_none() {
                    w.on_close = Some(cb);
                }
            }
        });
    }
    if allow && is_alive(id) {
        post(move || destroy(id));
    }
}
