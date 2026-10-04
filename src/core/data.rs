//! Table/tree model pushes to the backend (with batching via `freeze`).

use super::model::{Data, Node};
use super::props::update;
use super::with;
use crate::backend::{Backend, Native as B, Prop};
use crate::types::*;

// ---------------------------------------------------------------- table/tree model pushes

/// Mutate a table/tree model, then re-send `what` to the backend (deferred while frozen).
pub fn data_update(id: WidgetId, what: Data, f: impl FnOnce(&mut Node)) {
    if update(id, false, f).is_some() {
        push(id, what);
    }
}

/// Re-send part of the model of table/tree `id` to the backend (deferred while the node is frozen).
pub fn push(id: WidgetId, what: Data) {
    let deferred = with(|r| {
        let n = r.nodes.get_mut(&id)?;
        if n.freeze > 0 {
            n.pending = Some(match n.pending {
                None => what,
                Some(p) if p == what => p,
                Some(_) if matches!(what, Data::TreeRows | Data::TreeSelected) => Data::TreeRows,
                Some(_) => Data::TableAll,
            });
            return Some(true);
        }
        Some(false)
    })
    .flatten();
    if deferred == Some(false) {
        send(id, what);
    }
}

/// Start/stop a batch (nested calls count). Stopping the last level sends what was deferred.
pub fn freeze(id: WidgetId, on: bool) {
    let pending = with(|r| {
        let n = r.nodes.get_mut(&id)?;
        if on {
            n.freeze += 1;
            None
        } else {
            n.freeze = n.freeze.saturating_sub(1);
            if n.freeze == 0 {
                n.pending.take()
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
        if let Some(t) = &n.table {
            let all = what == Data::TableAll;
            return Some(Snap::Table {
                cols: all.then(|| t.columns.clone()),
                rows: (all || what == Data::TableRows).then(|| t.rows.clone()),
                sel: n.selected,
                sort: t.sort,
                send_sel: all || matches!(what, Data::TableRows | Data::TableSelected),
                send_sort: all || what == Data::TableSort,
            });
        }
        let t = n.tree.as_ref()?;
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
