//! Widget creation, initial native sync and destruction.

use super::model::{Node, NodeData, RangeData};
use super::props::read;
use super::splitter::panes_of;
use super::{post, set_error, wake, with};
use crate::backend::{Backend, Kind, Native as B, Prop};
use crate::types::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// See [`crate::MAX_NESTING`].
pub const MAX_NESTING: usize = 128;

// ---------------------------------------------------------------- creation / destruction

fn accepts(p: Kind, k: Kind) -> bool {
    match k {
        Kind::Window => false,
        Kind::Page => p == Kind::Tabs,
        Kind::MenuBar => p == Kind::Window,
        Kind::Menu => matches!(p, Kind::MenuBar | Kind::Menu | Kind::PopupMenu),
        Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
            matches!(p, Kind::Menu | Kind::PopupMenu)
        }
        Kind::PopupMenu => false,
        Kind::Sash => p == Kind::Splitter,
        _ => k.in_layout() && p.is_layout_container(),
    }
}

/// Create a widget. `setup` initialises the node (text etc.) before the backend sees it. On any
/// failure the error is recorded ([`super::take_error`]) and `WidgetId::DEAD` returned.
pub fn create(kind: Kind, parent: Option<WidgetId>, setup: impl FnOnce(&mut Node)) -> WidgetId {
    let id = WidgetId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let made = with(|r| {
        let mut native_parent = None;
        if let Some(p) = parent {
            let ok = r.nodes.get(&p).is_some_and(|n| {
                accepts(n.kind, kind)
                    && n.split().is_none_or(|sp| {
                        // a splitter holds one sash (created by the core) and at most two panes
                        if kind == Kind::Sash {
                            sp.sash.is_none()
                        } else {
                            panes_of(r, n).len() < 2
                        }
                    })
            });
            if !ok {
                return Err(Error::InvalidHandle);
            }
            if r.depth_of(p) > MAX_NESTING {
                return Err(Error::LimitExceeded);
            }
            native_parent = r.native_of(p);
        } else if kind != Kind::Window && kind != Kind::PopupMenu {
            return Err(Error::InvalidHandle);
        }
        let mut n = Node::new(kind);
        n.parent = parent;
        setup(&mut n);
        if kind == Kind::Page {
            // first page of a Tabs becomes selected
            if let Some(t) = parent.and_then(|p| r.nodes.get_mut(&p)) {
                if let NodeData::Tabs(sel @ None) = &mut t.data {
                    *sel = Some(0);
                }
            }
        }
        if let Some(c) = n.check().filter(|c| c.group != 0) {
            r.radio_groups.entry(c.group).or_default().push(id);
        }
        r.nodes.insert(id, n);
        if let Some(p) = parent.and_then(|p| r.nodes.get_mut(&p)) {
            p.children.push(id);
            if let Some(sp) = p.split_mut().filter(|_| kind == Kind::Sash) {
                sp.sash = Some(id);
            }
        }
        if kind == Kind::Window {
            r.windows.push(id);
        }
        let wake = r.touch(id, true);
        Ok((native_parent, wake))
    });
    let (native_parent, need_wake) = match made {
        Some(Ok(x)) => x,
        Some(Err(e)) => {
            set_error(e);
            return WidgetId::DEAD;
        }
        None => {
            set_error(Error::NotInitialized);
            return WidgetId::DEAD;
        }
    };
    if kind.is_native() {
        if let Err(e) = B::create(id, kind, native_parent) {
            remove_nodes(id);
            set_error(e);
            return WidgetId::DEAD;
        }
        sync_initial(id);
    }
    if need_wake {
        wake();
    }
    id
}

/// Push the non-default initial state of a freshly created native widget.
fn sync_initial(id: WidgetId) {
    /// The kind-specific part of the node's state, cloned out of the registry.
    enum Init {
        Nothing,
        Text {
            placeholder: String,
            readonly: bool,
        },
        Range(RangeData),
        List {
            items: Vec<String>,
            selected: Option<usize>,
        },
        Check(bool),
        MenuItem {
            accel: String,
            checked: bool,
        },
        Image(Option<ImageData>),
    }
    let Some((text, tooltip, enabled, init)) = with(|r| {
        let n = r.nodes.get(&id)?;
        let init = match &n.data {
            NodeData::Text(t) => Init::Text {
                placeholder: t.placeholder.clone(),
                readonly: t.readonly,
            },
            NodeData::Range(r) => Init::Range(RangeData { ..*r }),
            NodeData::List(l) => Init::List {
                items: l.items.clone(),
                selected: l.selected,
            },
            NodeData::Check(c) => Init::Check(c.checked),
            NodeData::MenuItem(m) => Init::MenuItem {
                accel: m.accel.clone(),
                checked: m.checked,
            },
            NodeData::Image(i) => Init::Image(i.clone()),
            _ => Init::Nothing,
        };
        Some((n.text.clone(), n.tooltip.clone(), n.enabled, init))
    })
    .flatten() else {
        return;
    };
    if !text.is_empty() {
        B::set(id, &Prop::Text(&text));
    }
    if !tooltip.is_empty() {
        B::set(id, &Prop::Tooltip(&tooltip));
    }
    match &init {
        Init::Nothing => {}
        Init::Text {
            placeholder,
            readonly,
        } => {
            if !placeholder.is_empty() {
                B::set(id, &Prop::Placeholder(placeholder));
            }
            if *readonly {
                B::set(id, &Prop::ReadOnly(true));
            }
        }
        Init::Range(r) => {
            B::set(
                id,
                &Prop::Range {
                    min: r.range.0,
                    max: r.range.1,
                    step: r.range.2,
                },
            );
            B::set(id, &Prop::Value(r.value));
            if r.indeterminate {
                B::set(id, &Prop::Indeterminate(true));
            }
        }
        Init::List { items, selected } => {
            if !items.is_empty() {
                B::set(id, &Prop::Items(items));
            }
            if selected.is_some() {
                B::set(id, &Prop::Selected(*selected));
            }
        }
        Init::Check(checked) => {
            if *checked {
                B::set(id, &Prop::Checked(true));
            }
        }
        Init::MenuItem { accel, checked } => {
            if !accel.is_empty() {
                B::set(id, &Prop::Accel(accel));
            }
            if *checked {
                B::set(id, &Prop::Checked(true));
            }
        }
        Init::Image(_) => {}
    }
    if !enabled {
        B::set(id, &Prop::Enabled(false));
    }
    if let Init::Image(Some(img)) = &init {
        B::set(id, &Prop::Image(Some(img)));
    }
}

/// Remove `id` and descendants from the registry; returns native ids, deepest first.
fn remove_nodes(id: WidgetId) -> Vec<WidgetId> {
    with(|r| {
        let mut order = vec![];
        let mut stack = vec![id];
        while let Some(i) = stack.pop() {
            if let Some(n) = r.nodes.get(&i) {
                stack.extend(n.children.iter().copied());
                order.push(i);
            }
        }
        let win = r.window_of(id);
        let is_window = r.nodes.get(&id).is_some_and(|n| n.kind == Kind::Window);
        if let Some(p) = r.nodes.get(&id).and_then(|n| n.parent) {
            if let Some(pn) = r.nodes.get_mut(&p) {
                pn.children.retain(|c| *c != id);
            }
            if let Some(w) = win {
                if w != id {
                    r.touch(w, true);
                }
            }
        }
        let mut natives = vec![];
        for i in order.iter().rev() {
            if let Some(n) = r.nodes.remove(i) {
                if n.kind.is_native() {
                    natives.push(*i);
                }
                if let Some(g) = n.check().map(|c| c.group).filter(|g| *g != 0) {
                    if let Some(members) = r.radio_groups.get_mut(&g) {
                        members.retain(|m| m != i);
                        if members.is_empty() {
                            r.radio_groups.remove(&g);
                        }
                    }
                }
            }
            if r.focus == Some(*i) {
                r.focus = None;
            }
        }
        if is_window {
            r.windows.retain(|w| *w != id); // windows are never nested: only the root can be one
        }
        r.dirty_layout.retain(|w| r.nodes.contains_key(w));
        r.dirty_a11y.retain(|w| r.nodes.contains_key(w));
        natives
    })
    .unwrap_or_default()
}

/// Destroy a widget and everything below it. No-op for stale ids.
pub fn destroy(id: WidgetId) {
    let was_window = read(id, |n| n.kind == Kind::Window).unwrap_or(false);
    for n in remove_nodes(id) {
        B::destroy(n);
    }
    if was_window && with(|r| r.windows.is_empty() && r.quit_on_last_close).unwrap_or(false) {
        post(B::quit);
    }
    wake();
}
