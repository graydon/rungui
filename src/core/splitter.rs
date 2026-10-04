//! Splitter behaviour: pane bookkeeping, position changes, and sash events.

use super::events::Ev;
use super::lifecycle::create;
use super::model::Node;
use super::model::SplitData;
use super::post::layout_window;
use super::props::read;
use super::{Registry, guarded, take_error, wake, wake_if_scheduled, with};
use crate::backend::{Backend, Event, Kind, Native as B, Prop, SashKey};
use crate::layout;
use crate::types::*;

/// The pane children of a splitter node (every child except the sash; at most two).
pub fn panes_of(r: &Registry, n: &Node) -> Vec<WidgetId> {
    n.children
        .iter()
        .copied()
        .filter(|c| r.nodes.get(c).is_some_and(|c| c.kind != Kind::Sash))
        .collect()
}

/// Create the native sash of a new splitter. Optional for backends: on failure the splitter
/// still lays out its panes (they just can't be resized by dragging) and no error is reported.
pub fn create_sash(splitter: WidgetId, orient: Orientation) {
    // hidden until the layout has two visible panes to put it between
    let sash = create(Kind::Sash, Some(splitter), |n| n.visible = false);
    if sash == WidgetId::DEAD {
        take_error();
        with(|r| {
            r.nodes
                .get_mut(&splitter)
                .and_then(|n| n.split.as_mut())
                .map(|sp| sp.sash = None)
        });
        return;
    }
    B::set(sash, &Prop::Orientation(orient));
    B::set(sash, &Prop::Visible(false));
}

/// Set the position (first-pane size) of a splitter and relayout its window right away.
/// `user`: the change came from the sash/AT, so clamp, remember the clamped value and fire `on_move`.
pub fn split_set(id: WidgetId, want: i32, user: bool) {
    let r = with(|r| {
        let win = r.window_of(id);
        let sp = r.nodes.get_mut(&id)?.split.as_mut()?;
        let old = sp.actual;
        sp.pos = Some(if user && sp.laid_out {
            sp.clamp(want, main_len(sp))
        } else {
            want.max(0)
        });
        let laid = sp.laid_out;
        let wake = r.touch(id, true);
        Some((win, old, laid, wake))
    })
    .flatten();
    let Some((win, old, laid, need_wake)) = r else {
        return;
    };
    if let Some(w) = win.filter(|_| laid) {
        layout_window(w);
    }
    if need_wake {
        wake();
    }
    let new = read(id, |n| n.split.as_ref().map_or(0, |s| s.actual)).unwrap_or(0);
    if !user || new == old || !laid {
        return;
    }
    let cb = with(|r| {
        r.nodes
            .get_mut(&id)
            .and_then(|n| n.cbs.remove(&Ev::SashMoved))
    })
    .flatten();
    if let Some(mut cb) = cb {
        guarded(|| cb(&Event::SashDragged(new)));
        with(|r| {
            if let Some(n) = r.nodes.get_mut(&id) {
                n.cbs.entry(Ev::SashMoved).or_insert(cb);
            }
        });
    }
    wake_if_scheduled();
}

fn main_len(sp: &SplitData) -> i32 {
    if sp.orient == Orientation::Horizontal {
        sp.area.w
    } else {
        sp.area.h
    }
}

/// Pixels a splitter moves per arrow key, and with Shift.
const SASH_STEP: i32 = 10;
const SASH_STEP_LARGE: i32 = 50;

/// The splitter a user event on sash `id` is for, if the event should be honoured: nothing disabled
/// or hidden up the chain (a stale event), `id` really is that splitter's sash, and it was laid out.
fn live_sash(r: &Registry, id: WidgetId) -> Option<(WidgetId, &SplitData)> {
    let mut cur = Some(id);
    while let Some(i) = cur {
        let n = r.nodes.get(&i)?;
        if !n.enabled || (!n.visible && n.kind != Kind::Window) {
            return None;
        }
        cur = n.parent;
    }
    let split = r.nodes.get(&id)?.parent?;
    let sp = r.nodes.get(&split)?.split.as_ref()?;
    (sp.sash == Some(id) && sp.laid_out).then_some((split, sp))
}

/// `Event::SashDragged` on sash `id`: convert the sash's leading edge to a first-pane size.
pub(super) fn sash_event(id: WidgetId, pos: i32) {
    let target = with(|r| {
        let (split, sp) = live_sash(r, id)?;
        let a = sp.area;
        let want = match sp.orient {
            Orientation::Vertical => pos.saturating_sub(a.y),
            // mirrored: the first pane is on the right, so measure from the right edge
            Orientation::Horizontal if layout::is_rtl() => {
                (a.x + a.w - sp.thick).saturating_sub(pos)
            }
            Orientation::Horizontal => pos.saturating_sub(a.x),
        };
        Some((split, want))
    })
    .flatten();
    if let Some((split, want)) = target {
        split_set(split, want, true);
    }
}

/// `Event::SashKey` on sash `id`: step the first-pane size, or jump to its limits.
pub(super) fn sash_key_event(id: WidgetId, key: SashKey) {
    let target = with(|r| {
        let (split, sp) = live_sash(r, id)?;
        // Prev/Next are screen directions. With the first pane on the right (RTL, side by side)
        // moving the sash toward the left makes the first pane bigger, not smaller.
        let sign = if sp.orient == Orientation::Horizontal && layout::is_rtl() {
            -1
        } else {
            1
        };
        let step = |by: i32| sp.actual.saturating_add(sign * by);
        let want = match key {
            SashKey::Prev => step(-SASH_STEP),
            SashKey::Next => step(SASH_STEP),
            SashKey::PrevLarge => step(-SASH_STEP_LARGE),
            SashKey::NextLarge => step(SASH_STEP_LARGE),
            // logical, not screen, directions: the clamp turns these into the pane limits
            SashKey::Min => i32::MIN,
            SashKey::Max => i32::MAX,
        };
        Some((split, want))
    })
    .flatten();
    if let Some((split, want)) = target {
        split_set(split, want, true);
    }
}
