//! Widget registry, event dispatch, post queue, timers. Main-thread-only state lives in a
//! thread-local; all other threads see an empty/uninitialised registry, so using a handle off
//! the main thread is a harmless no-op (use [`crate::App::post`] to hop threads).
//!
//! Re-entrancy discipline (this is what keeps us panic/deadlock free):
//! * the registry is only ever borrowed through `with` (a `try_borrow_mut`), for short
//!   non-reentrant sections; a conflicting access yields `None`, never a panic;
//! * backend calls that can synchronously produce events (`create`, `set`, `destroy`) happen
//!   *outside* the borrow; user callbacks are taken out of their slot, called, then put back.

mod data;
mod events;
mod lifecycle;
mod model;
mod post;
mod props;
mod splitter;
mod timers;

pub use data::*;
pub use events::*;
pub use lifecycle::*;
pub use model::*;
pub use post::*;
pub use props::*;
pub use splitter::*;
pub use timers::*;

use crate::backend::{Backend, Kind, Native as B};
use crate::layout;
use crate::types::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use timers::TimerEntry;

thread_local! {
    static REG: RefCell<Option<Registry>> = const { RefCell::new(None) };
    static LAST_ERR: RefCell<Option<Error>> = const { RefCell::new(None) };
}

pub struct Registry {
    pub nodes: HashMap<WidgetId, Node>,
    pub windows: Vec<WidgetId>,
    pub focus: Option<WidgetId>,
    dirty_layout: BTreeSet<WidgetId>,
    dirty_a11y: BTreeSet<WidgetId>,
    /// Tables and trees whose model changed since it was last sent to the backend.
    dirty_models: BTreeSet<WidgetId>,
    scheduled: bool,
    timers: HashMap<u64, TimerEntry>,
    /// Members of each radio group (`CheckData::group`), so toggling one needs no scan of all nodes.
    radio_groups: HashMap<u32, Vec<WidgetId>>,
    next_group: u32,
    pub quit_on_last_close: bool,
}

impl Registry {
    pub fn window_of(&self, mut id: WidgetId) -> Option<WidgetId> {
        loop {
            let n = self.nodes.get(&id)?;
            if n.kind == Kind::Window {
                return Some(id);
            }
            id = n.parent?;
        }
    }
    /// Number of ancestors above `id` plus one (a window has depth 1).
    pub fn depth_of(&self, mut id: WidgetId) -> usize {
        let mut depth = 1;
        while let Some(p) = self.nodes.get(&id).and_then(|n| n.parent) {
            depth += 1;
            id = p;
        }
        depth
    }
    /// Nearest native ancestor-or-self.
    pub fn native_of(&self, mut id: WidgetId) -> Option<WidgetId> {
        loop {
            let n = self.nodes.get(&id)?;
            if n.kind.is_native() {
                return Some(id);
            }
            id = n.parent?;
        }
    }
    /// The checked members of radio group `group`, other than `except`.
    pub fn checked_in_group(&self, group: u32, except: WidgetId) -> Vec<WidgetId> {
        let members = self.radio_groups.get(&group).map_or(&[][..], Vec::as_slice);
        members
            .iter()
            .copied()
            .filter(|m| *m != except && self.nodes.get(m).is_some_and(Node::checked))
            .collect()
    }
    /// Mark dirty; returns true when the caller must call `B::wake()` (outside the borrow).
    fn touch(&mut self, id: WidgetId, relayout: bool) -> bool {
        if let Some(w) = self.window_of(id) {
            if relayout {
                self.dirty_layout.insert(w);
            }
            self.dirty_a11y.insert(w);
        }
        self.schedule()
    }
    /// Note that the next loop turn has work to do; returns true when the caller must call
    /// `B::wake()` (outside the borrow).
    fn schedule(&mut self) -> bool {
        !std::mem::replace(&mut self.scheduled, true)
    }
    /// Effective (own && all ancestors) value of visible/enabled for every native node in the subtree.
    fn effective(&self, id: WidgetId, visible: bool) -> Vec<(WidgetId, bool)> {
        let flag = |n: &Node| if visible { n.visible } else { n.enabled };
        let mut acc = true;
        let mut p = self.nodes.get(&id).and_then(|n| n.parent);
        while let Some(pid) = p {
            let Some(n) = self.nodes.get(&pid) else { break };
            acc &= flag(n);
            p = n.parent;
        }
        let mut out = vec![];
        let mut stack = vec![(id, acc)];
        while let Some((i, a)) = stack.pop() {
            let Some(n) = self.nodes.get(&i) else {
                continue;
            };
            let a = a && flag(n);
            if n.kind.is_native() {
                out.push((i, a));
            }
            stack.extend(n.children.iter().map(|c| (*c, a)));
        }
        out
    }
}

/// The only way to touch the registry. `None` = not initialised on this thread or re-entrant access.
pub(crate) fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> Option<R> {
    REG.try_with(|c| match c.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
    .ok()
    .flatten()
}

fn wake() {
    B::wake();
}

pub fn set_error(e: Error) {
    let _ = LAST_ERR.try_with(|c| *c.borrow_mut() = Some(e));
}
pub fn take_error() -> Option<Error> {
    LAST_ERR.try_with(|c| c.borrow_mut().take()).ok().flatten()
}

/// Run user code, containing panics (unwinding through native frames would abort the process).
fn guarded(f: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(f));
}

// ---------------------------------------------------------------- lifecycle

pub fn init(app_name: &str) -> Result<()> {
    let fresh = REG
        .try_with(|c| {
            let mut g = c.borrow_mut();
            if g.is_some() {
                return false;
            }
            *g = Some(Registry {
                nodes: HashMap::new(),
                windows: vec![],
                focus: None,
                dirty_layout: BTreeSet::new(),
                dirty_a11y: BTreeSet::new(),
                dirty_models: BTreeSet::new(),
                scheduled: false,
                timers: HashMap::new(),
                radio_groups: HashMap::new(),
                next_group: 1,
                quit_on_last_close: true,
            });
            true
        })
        .map_err(|_| Error::NotInitialized)?;
    if !fresh {
        return Err(Error::AlreadyInitialized);
    }
    if let Err(e) = B::init(app_name) {
        let _ = REG.try_with(|c| *c.borrow_mut() = None);
        return Err(e);
    }
    *UI_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current().id());
    Ok(())
}

pub fn new_group() -> u32 {
    with(|r| {
        r.next_group += 1;
        r.next_group
    })
    .unwrap_or(0)
}

/// Enable/disable right-to-left mirroring of layouts and relayout every window.
pub fn set_rtl(v: bool) {
    layout::set_rtl(v);
    let wake_it = with(|r| {
        let ws = r.windows.clone();
        let mut w = false;
        for win in ws {
            w |= r.touch(win, true);
        }
        w
    })
    .unwrap_or(false);
    if wake_it {
        wake();
    }
}

pub fn set_quit_on_last_close(v: bool) {
    with(|r| r.quit_on_last_close = v);
}

fn wake_if_scheduled() {
    if with(|r| r.scheduled).unwrap_or(false) {
        wake();
    }
}
