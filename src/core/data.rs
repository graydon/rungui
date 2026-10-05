//! Table/tree model pushes to the backend (with batching via `freeze`).

use super::model::{Data, Node};
use super::props::modify;
use super::{wake, with};
use crate::backend::{Backend, Native as B, Prop};
use crate::types::*;

// ---------------------------------------------------------------- table/tree model pushes

/// Mutate a table/tree model, then re-send `what` to the backend (deferred while frozen).
pub fn data_update(id: WidgetId, what: Data, f: impl FnOnce(&mut Node)) {
    if modify(id, f).is_some() {
        push(id, what);
    }
}

/// Combine two kinds of pending update into one that covers both.
fn merge(pending: Option<Data>, what: Data) -> Data {
    match pending {
        None => what,
        Some(p) if p == what => p,
        Some(_) if what == Data::TreeRows => Data::TreeRows,
        Some(_) => Data::TableAll,
    }
}

/// Note that part of the model of table/tree `id` changed. The backend is updated at the next
/// loop turn (see [`flush_models`]), so any number of mutations costs one transfer of the model.
pub fn push(id: WidgetId, what: Data) {
    let need_wake = with(|r| {
        let b = r.nodes.get_mut(&id)?.batch_mut()?;
        b.pending = Some(merge(b.pending, what));
        r.dirty_models.insert(id);
        Some(r.schedule())
    })
    .flatten();
    if need_wake == Some(true) {
        wake();
    }
}

/// Send every changed table/tree model to the backend, except those inside a `batch` (they are
/// sent when the batch ends).
pub fn flush_models() {
    let ready: Vec<(WidgetId, Data)> = with(|r| {
        let dirty = std::mem::take(&mut r.dirty_models);
        dirty
            .into_iter()
            .filter_map(|id| {
                let b = r.nodes.get_mut(&id)?.batch_mut()?;
                if b.freeze > 0 {
                    return None;
                }
                Some((id, b.pending.take()?))
            })
            .collect()
    })
    .unwrap_or_default();
    for (id, what) in ready {
        send(id, what);
    }
}

/// Start/stop a batch (nested calls count). Stopping the last level sends what was deferred.
pub fn freeze(id: WidgetId, on: bool) {
    let pending = with(|r| {
        let b = r.nodes.get_mut(&id)?.batch_mut()?;
        if on {
            b.freeze += 1;
            None
        } else {
            b.freeze = b.freeze.saturating_sub(1);
            if b.freeze == 0 {
                b.pending.take()
            } else {
                None
            }
        }
    })
    .flatten();
    if let Some(p) = pending {
        send(id, p);
    }
}

fn send(id: WidgetId, what: Data) {
    enum Snap {
        Table {
            cols: Option<Vec<Column>>,
            rows: Option<Vec<Vec<String>>>,
            sel: Option<usize>,
            sort: Option<(usize, bool)>,
            send_sel: bool,
            send_sort: bool,
        },
        Tree {
            rows: Option<Vec<TreeRow>>,
            sel: Option<u64>,
        },
    }
    let Some(snap) = with(|r| {
        let n = r.nodes.get(&id)?;
        if let Some(t) = n.table() {
            let all = what == Data::TableAll;
            return Some(Snap::Table {
                cols: all.then(|| t.columns.clone()),
                rows: (all || what == Data::TableRows).then(|| t.rows.clone()),
                sel: t.selected,
                sort: t.sort,
                send_sel: all || matches!(what, Data::TableRows | Data::TableSelected),
                send_sort: all || what == Data::TableSort,
            });
        }
        let t = n.tree()?;
        Some(Snap::Tree {
            rows: (what == Data::TreeRows).then(|| t.flatten()),
            sel: t.selected,
        })
    })
    .flatten() else {
        return;
    };
    match snap {
        Snap::Table {
            cols,
            rows,
            sel,
            sort,
            send_sel,
            send_sort,
        } => {
            if let Some(c) = cols {
                B::set(id, &Prop::Columns(&c));
            }
            if let Some(r) = rows {
                B::set(id, &Prop::Rows(&r));
            }
            if send_sel {
                B::set(id, &Prop::Selected(sel));
            }
            if send_sort {
                B::set(id, &Prop::SortIndicator(sort));
            }
        }
        Snap::Tree { rows, sel } => {
            if let Some(r) = rows {
                B::set(id, &Prop::TreeRows(&r));
            }
            B::set(id, &Prop::TreeSelected(sel));
        }
    }
}
