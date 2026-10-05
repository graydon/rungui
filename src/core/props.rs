//! Property access: node updates pushed to the backend, flags, native handles.

use super::model::Node;
use super::post::layout_window;
use super::{wake, with};
use crate::backend::{Backend, Kind, Native as B, Prop};
use crate::types::*;

// ---------------------------------------------------------------- property updates

/// Modify a node and mark it dirty; returns the widget's kind if it exists.
pub fn update(id: WidgetId, relayout: bool, f: impl FnOnce(&mut Node)) -> Option<Kind> {
    let r = with(|r| {
        let n = r.nodes.get_mut(&id)?;
        f(n);
        let k = n.kind;
        Some((k, r.touch(id, relayout)))
    })
    .flatten()?;
    if r.1 {
        wake();
    }
    Some(r.0)
}

/// `update` + push `prop` to the backend when the widget is native.
pub fn set(id: WidgetId, relayout: bool, f: impl FnOnce(&mut Node), prop: Prop) {
    if let Some(k) = update(id, relayout, f) {
        if k.is_native() && prop.applies_to(k) {
            B::set(id, &prop);
        }
    }
}

pub fn read<R>(id: WidgetId, f: impl FnOnce(&Node) -> R) -> Option<R> {
    with(|r| r.nodes.get(&id).map(f)).flatten()
}

pub fn is_alive(id: WidgetId) -> bool {
    read(id, |_| ()).is_some()
}

/// Visible/enabled propagate through virtual boxes and containers to native descendants.
pub fn set_flag(id: WidgetId, visible: bool, v: bool) {
    let Some(k) = update(id, visible, |n| {
        if visible {
            n.visible = v
        } else {
            n.enabled = v
        }
    }) else {
        return;
    };
    if visible && v && k == Kind::Window {
        layout_window(id);
    }
    for (nid, eff) in with(|r| r.effective(id, visible)).unwrap_or_default() {
        B::set(
            nid,
            &if visible {
                Prop::Visible(eff)
            } else {
                Prop::Enabled(eff)
            },
        );
    }
}

pub fn native_handle(id: WidgetId) -> Option<NativeHandle> {
    if is_alive(id) {
        B::native_handle(id)
    } else {
        None
    }
}
