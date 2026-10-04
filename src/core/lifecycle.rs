//! Widget creation, initial native sync and destruction.

use super::data::push;
use super::model::{Data, Node};
use super::props::read;
use super::splitter::panes_of;
use super::{post, set_error, wake, with};
use crate::backend::{Backend, Kind, Native as B, Prop};
use crate::types::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

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
/// failure the error is recorded ([`take_error`]) and `WidgetId::DEAD` returned.
pub fn create(kind: Kind, parent: Option<WidgetId>, setup: impl FnOnce(&mut Node)) -> WidgetId {
    let id = WidgetId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let made = with(|r| {
        let mut native_parent = None;
        if let Some(p) = parent {
            let ok = r.nodes.get(&p).is_some_and(|n| {
                accepts(n.kind, kind)
                    && n.split.as_ref().is_none_or(|sp| {
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
                if t.selected.is_none() {
                    t.selected = Some(0);
                }
            }
        }
        r.nodes.insert(id, n);
        if let Some(p) = parent.and_then(|p| r.nodes.get_mut(&p)) {
            p.children.push(id);
            if let Some(sp) = p.split.as_mut().filter(|_| kind == Kind::Sash) {
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
    struct S {
        text: String,
        tooltip: String,
        placeholder: String,
        accel: String,
        enabled: bool,
        checked: bool,
        readonly: bool,
        indet: bool,
        value: f64,
        range: (f64, f64, f64),
        items: Vec<String>,
        selected: Option<usize>,
        image: Option<ImageData>,
        kind: Kind,
    }
    let Some(s) = with(|r| {
        let n = r.nodes.get(&id)?;
        Some(S {
            text: n.text.clone(),
            tooltip: n.tooltip.clone(),
            placeholder: n.placeholder.clone(),
            accel: n.accel.clone(),
            enabled: n.enabled,
            checked: n.checked,
            readonly: n.readonly,
            indet: n.indeterminate,
            value: n.value,
            range: n.range,
            items: n.items.clone(),
            selected: n.selected,
            image: n.image.clone(),
            kind: n.kind,
        })
    })
    .flatten() else {
        return;
    };
    if !s.text.is_empty() {
        B::set(id, &Prop::Text(&s.text));
    }
    if !s.tooltip.is_empty() {
        B::set(id, &Prop::Tooltip(&s.tooltip));
    }
    if !s.placeholder.is_empty() {
        B::set(id, &Prop::Placeholder(&s.placeholder));
    }
    if !s.accel.is_empty() {
        B::set(id, &Prop::Accel(&s.accel));
    }
    if matches!(s.kind, Kind::Slider | Kind::SpinBox | Kind::ProgressBar) {
        B::set(
            id,
            &Prop::Range {
                min: s.range.0,
                max: s.range.1,
                step: s.range.2,
            },
        );
        B::set(id, &Prop::Value(s.value));
    }
    if !s.items.is_empty() {
        B::set(id, &Prop::Items(&s.items));
    }
    if s.selected.is_some() && matches!(s.kind, Kind::ComboBox | Kind::ListBox) {
        B::set(id, &Prop::Selected(s.selected));
    }
    if s.checked {
        B::set(id, &Prop::Checked(true));
    }
    if s.readonly {
        B::set(id, &Prop::ReadOnly(true));
    }
    if s.indet {
        B::set(id, &Prop::Indeterminate(true));
    }
    if !s.enabled {
        B::set(id, &Prop::Enabled(false));
    }
    if s.image.is_some() {
        B::set(id, &Prop::Image(s.image.as_ref()));
    }
    match s.kind {
        Kind::Table => push(id, Data::TableAll),
        Kind::Tree => push(id, Data::TreeRows),
        _ => {}
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
            }
            if r.focus == Some(*i) {
                r.focus = None;
            }
        }
        r.windows.retain(|w| !order.contains(w));
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
