//! Cross-thread post queue and the idle flush (layout + accessibility).

use super::{REG, guarded, wake, with};
use crate::backend::{Backend, Native as B};
use crate::layout;
use crate::types::WidgetId;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

type Job = Box<dyn FnOnce() + Send>;
/// Posted jobs, per target (UI) thread, so independent toolkit instances (tests) never steal
/// each other's work. A normal app has exactly one UI thread.
static QUEUES: Mutex<Option<HashMap<std::thread::ThreadId, VecDeque<Job>>>> = Mutex::new(None);
/// The thread that most recently completed `init`: where `post` from other threads is delivered.
pub(super) static UI_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

// ---------------------------------------------------------------- post queue

/// Queue `f` for the UI thread: the calling thread itself when it owns a toolkit instance,
/// otherwise the thread that called `init`.
pub fn post(f: impl FnOnce() + Send + 'static) {
    let own = REG
        .try_with(|c| c.try_borrow().map_or(true, |g| g.is_some()))
        .unwrap_or(false);
    let me = std::thread::current().id();
    let target = if own {
        me
    } else {
        UI_THREAD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(me)
    };
    post_to(target, f);
}

/// Queue `f` for a specific UI thread.
pub fn post_to(target: std::thread::ThreadId, f: impl FnOnce() + Send + 'static) {
    QUEUES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .entry(target)
        .or_default()
        .push_back(Box::new(f));
    wake();
}

/// Backend entry point: called on the main thread after `Backend::wake`. Runs queued closures
/// (a snapshot, so closures that post again run on the next wake) and then flushes layout/a11y.
pub fn drain_posted() {
    let me = std::thread::current().id();
    let batch = QUEUES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|m| m.get_mut(&me).map(std::mem::take))
        .unwrap_or_default();
    for job in batch {
        guarded(job);
    }
    flush();
}

/// Apply pending layout and notify the backend of accessibility changes. Also available as `App::update()`.
pub fn flush() {
    for _ in 0..4 {
        let (lay, a11y) = match with(|r| {
            r.scheduled = false;
            (
                std::mem::take(&mut r.dirty_layout),
                std::mem::take(&mut r.dirty_a11y),
            )
        }) {
            Some(x) => x,
            None => return,
        };
        if lay.is_empty() && a11y.is_empty() {
            return;
        }
        for w in lay {
            layout_window(w);
        }
        for w in a11y {
            if with(|r| r.nodes.contains_key(&w)).unwrap_or(false) {
                B::a11y_changed(w);
            }
        }
    }
}

/// Compute and apply layout for one window now.
pub fn layout_window(w: WidgetId) {
    let jobs = with(|r| layout::compute(r, w)).unwrap_or_default();
    for (id, prop) in jobs {
        B::set(id, &prop);
    }
}
