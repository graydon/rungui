//! Widget registry, event dispatch, post queue, timers. Main-thread-only state lives in a
//! thread-local; all other threads see an empty/uninitialised registry, so using a handle off
//! the main thread is a harmless no-op (use [`crate::App::post`] to hop threads).
//!
//! Re-entrancy discipline (this is what keeps us panic/deadlock free):
//! * the registry is only ever borrowed through `with` (a `try_borrow_mut`), for short
//!   non-reentrant sections; a conflicting access yields `None`, never a panic;
//! * backend calls that can synchronously produce events (`create`, `set`, `destroy`) happen
//!   *outside* the borrow; user callbacks are taken out of their slot, called, then put back.

use crate::a11y::A11yProps;
use crate::backend::{Backend, Event, Kind, Native as B, Prop, SashKey};
use crate::layout;
use crate::types::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TIMER: AtomicU64 = AtomicU64::new(1);
type Job = Box<dyn FnOnce() + Send>;
/// Posted jobs, per target (UI) thread, so independent toolkit instances (tests) never steal
/// each other's work. A normal app has exactly one UI thread.
static QUEUES: Mutex<Option<HashMap<std::thread::ThreadId, VecDeque<Job>>>> = Mutex::new(None);
/// The thread that most recently completed `init`: where `post` from other threads is delivered.
static UI_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

thread_local! {
    static REG: RefCell<Option<Registry>> = const { RefCell::new(None) };
    static LAST_ERR: RefCell<Option<Error>> = const { RefCell::new(None) };
}

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

/// Layout parameters of a node (see layout.rs).
#[derive(Clone, Debug)]
pub struct LayoutProps {
    /// Share of extra space along the stack axis (0 = natural size).
    pub expand: f32,
    pub align: Align,
    pub min: Size,
    pub fixed: Option<Size>,
    /// Explicit grid cell (col, row, colspan, rowspan) when the parent is a Grid.
    pub cell: Option<(usize, usize, usize, usize)>,
    pub spacing: i32,
    pub padding: i32,
    /// Grid column count.
    pub cols: usize,
}

/// Table model (rows are plain strings; selection lives in `Node::selected`).
#[derive(Clone, Debug, Default)]
pub struct TableData {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
    pub sort: Option<(usize, bool)>,
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub text: String,
    pub parent: Option<u64>,
    pub children: Vec<u64>,
    pub expanded: bool,
    pub has_children: bool,
}

/// Splitter state (Kind::Splitter only). Positions are the main-axis size of the FIRST pane.
#[derive(Clone, Debug, Default)]
pub struct SplitData {
    pub orient: Orientation,
    /// Requested position (app or last user drag); `None` = split the space evenly.
    pub pos: Option<i32>,
    /// Minimum main-axis size of the (first, second) pane.
    pub min: (i32, i32),
    /// The native sash, if the backend has one.
    pub sash: Option<WidgetId>,
    /// From the last layout: effective (clamped) position, inner area (unmirrored, native-parent
    /// coordinates) and sash thickness. `laid_out` is false until both panes were placed once.
    pub actual: i32,
    pub area: Rect,
    pub thick: i32,
    pub laid_out: bool,
}

impl SplitData {
    /// Clamp a requested first-pane size to the space available along the main axis.
    pub fn clamp(&self, want: i32, avail: i32) -> i32 {
        let room = (avail - self.thick).max(0);
        want.min(room.saturating_sub(self.min.1))
            .max(self.min.0)
            .min(room)
            .max(0)
    }
}

static NEXT_TREE_NODE: AtomicU64 = AtomicU64::new(1);

/// Tree model: an arena keyed by globally unique node ids.
#[derive(Clone, Debug, Default)]
pub struct TreeData {
    pub nodes: HashMap<u64, TreeNode>,
    pub roots: Vec<u64>,
    pub selected: Option<u64>,
}

impl TreeData {
    /// Insert under `parent` (None = root level) at `index` (clamped); returns 0 for an unknown parent.
    pub fn insert(&mut self, parent: Option<u64>, index: usize, text: &str) -> u64 {
        if parent.is_some_and(|p| !self.nodes.contains_key(&p)) {
            return 0;
        }
        let id = NEXT_TREE_NODE.fetch_add(1, Ordering::Relaxed);
        self.nodes.insert(
            id,
            TreeNode {
                text: text.to_string(),
                parent,
                children: vec![],
                expanded: false,
                has_children: false,
            },
        );
        let list = match parent {
            Some(p) => self.nodes.get_mut(&p).map(|n| &mut n.children),
            None => Some(&mut self.roots),
        };
        if let Some(l) = list {
            let i = index.min(l.len());
            l.insert(i, id);
        }
        id
    }
    /// Remove a node and its subtree; clears the selection if it was inside.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(n) = self.nodes.get(&id) else {
            return false;
        };
        match n.parent {
            Some(p) => {
                if let Some(pn) = self.nodes.get_mut(&p) {
                    pn.children.retain(|c| *c != id);
                }
            }
            None => self.roots.retain(|c| *c != id),
        }
        let mut stack = vec![id];
        while let Some(i) = stack.pop() {
            if let Some(n) = self.nodes.remove(&i) {
                stack.extend(n.children);
            }
            if self.selected == Some(i) {
                self.selected = None;
            }
        }
        true
    }
    pub fn children_of(&self, parent: Option<u64>) -> &[u64] {
        match parent {
            Some(p) => self.nodes.get(&p).map_or(&[], |n| &n.children[..]),
            None => &self.roots,
        }
    }
    /// Pre-order flattening of all nodes.
    pub fn flatten(&self) -> Vec<TreeRow> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<(u64, u32)> = self.roots.iter().rev().map(|r| (*r, 0)).collect();
        while let Some((id, depth)) = stack.pop() {
            let Some(n) = self.nodes.get(&id) else {
                continue;
            };
            out.push(TreeRow {
                node: id,
                depth,
                text: n.text.clone(),
                expanded: n.expanded,
                has_children: n.has_children || !n.children.is_empty(),
            });
            stack.extend(n.children.iter().rev().map(|c| (*c, depth + 1)));
        }
        out
    }
    /// Expand all ancestors of `id`.
    pub fn reveal(&mut self, id: u64) {
        let mut cur = self.nodes.get(&id).and_then(|n| n.parent);
        while let Some(p) = cur {
            let Some(n) = self.nodes.get_mut(&p) else {
                break;
            };
            n.expanded = true;
            cur = n.parent;
        }
    }
    /// Is every ancestor expanded (i.e. is the node visible)?
    pub fn is_visible(&self, id: u64) -> bool {
        let mut cur = self.nodes.get(&id).and_then(|n| n.parent);
        while let Some(p) = cur {
            let Some(n) = self.nodes.get(&p) else {
                return false;
            };
            if !n.expanded {
                return false;
            }
            cur = n.parent;
        }
        true
    }
}

pub struct Node {
    pub kind: Kind,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    pub text: String,
    pub tooltip: String,
    pub placeholder: String,
    pub accel: String,
    pub enabled: bool,
    pub visible: bool,
    pub checked: bool,
    pub readonly: bool,
    pub indeterminate: bool,
    pub resizable: bool,
    pub value: f64,
    pub range: (f64, f64, f64),
    pub items: Vec<String>,
    pub selected: Option<usize>,
    pub group: u32,
    pub image: Option<ImageData>,
    /// Last bounds pushed to the backend (relative to the native parent).
    pub bounds: Rect,
    /// Windows: client size; `explicit` once set by the app or the user.
    pub client: Size,
    pub explicit_size: bool,
    pub a11y: A11yProps,
    pub lay: LayoutProps,
    pub cbs: HashMap<Ev, Callback>,
    pub on_close: Option<Box<dyn FnMut() -> bool>>,
    /// Table model (Kind::Table only).
    pub table: Option<Box<TableData>>,
    /// Tree model (Kind::Tree only).
    pub tree: Option<Box<TreeData>>,
    /// Attached `PopupMenu` (see `Widget::set_context_menu`).
    pub context_menu: Option<WidgetId>,
    /// >0 while a table/tree batch update is running: pushes are deferred to the end.
    pub freeze: u32,
    pub pending: Option<Data>,
    /// Splitter model (Kind::Splitter only).
    pub split: Option<Box<SplitData>>,
    /// TextArea/TextInput: fixed-pitch font; TextArea: soft wrap (default on).
    pub monospace: bool,
    pub wrap: bool,
    /// Window: last requested (`set_position`) or reported (`Event::Moved`) screen position.
    pub position: Option<(i32, i32)>,
}

/// Which part of a table/tree model must be re-sent to the backend.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Data {
    /// Columns + rows + selection + sort indicator.
    TableAll,
    /// Rows + selection.
    TableRows,
    TableSelected,
    TableSort,
    /// Rows + selection.
    TreeRows,
    TreeSelected,
}

impl Node {
    fn new(kind: Kind) -> Node {
        let boxy = matches!(kind, Kind::Window | Kind::Page | Kind::GroupBox);
        Node {
            kind,
            parent: None,
            children: vec![],
            text: String::new(),
            tooltip: String::new(),
            placeholder: String::new(),
            accel: String::new(),
            enabled: true,
            visible: kind != Kind::Window,
            checked: false,
            readonly: false,
            indeterminate: false,
            resizable: true,
            value: 0.0,
            range: match kind {
                Kind::ProgressBar => (0.0, 1.0, 0.01),
                _ => (0.0, 100.0, 1.0),
            },
            items: vec![],
            selected: None,
            group: 0,
            image: None,
            bounds: Rect::default(),
            client: Size::default(),
            explicit_size: false,
            a11y: A11yProps::default(),
            lay: LayoutProps {
                expand: 0.0,
                align: Align::Fill,
                min: Size::default(),
                fixed: None,
                cell: None,
                spacing: 6,
                padding: if boxy { 10 } else { 0 },
                cols: 2,
            },
            cbs: HashMap::new(),
            on_close: None,
            table: (kind == Kind::Table).then(Box::default),
            tree: (kind == Kind::Tree).then(Box::default),
            context_menu: None,
            freeze: 0,
            pending: None,
            split: (kind == Kind::Splitter).then(Box::default),
            monospace: false,
            wrap: true,
            position: None,
        }
    }
}

struct TimerEntry {
    cb: Option<Box<dyn FnMut()>>,
    repeat: bool,
}

pub struct Registry {
    pub nodes: HashMap<WidgetId, Node>,
    pub windows: Vec<WidgetId>,
    pub focus: Option<WidgetId>,
    dirty_layout: BTreeSet<WidgetId>,
    dirty_a11y: BTreeSet<WidgetId>,
    scheduled: bool,
    timers: HashMap<u64, TimerEntry>,
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
    /// Mark dirty; returns true when the caller must call `B::wake()` (outside the borrow).
    fn touch(&mut self, id: WidgetId, relayout: bool) -> bool {
        if let Some(w) = self.window_of(id) {
            if relayout {
                self.dirty_layout.insert(w);
            }
            self.dirty_a11y.insert(w);
        }
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
                scheduled: false,
                timers: HashMap::new(),
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
fn sash_event(id: WidgetId, pos: i32) {
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
fn sash_key_event(id: WidgetId, key: SashKey) {
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
        if k.is_native() {
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

pub fn set_callback(id: WidgetId, ev: Ev, cb: Callback) {
    update(id, false, |n| {
        n.cbs.insert(ev, cb);
    });
}

pub fn native_handle(id: WidgetId) -> Option<NativeHandle> {
    if is_alive(id) {
        B::native_handle(id)
    } else {
        None
    }
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
                Event::Toggled(b) => n.checked = *b,
                Event::Selected(s) => {
                    if n.table
                        .as_ref()
                        .is_some_and(|t| s.is_some_and(|i| i >= t.rows.len()))
                    {
                        return None;
                    }
                    n.selected = *s;
                }
                Event::Activated(i) => {
                    if n.table.as_ref().is_some_and(|t| *i >= t.rows.len()) {
                        return None;
                    }
                }
                Event::ColumnClicked(c) => {
                    if n.table.as_ref().is_none_or(|t| *c >= t.columns.len()) {
                        return None;
                    }
                }
                Event::TreeSelected(s) => {
                    let t = n.tree.as_mut()?;
                    if s.is_some_and(|x| !t.nodes.contains_key(&x)) {
                        return None;
                    }
                    t.selected = *s;
                }
                Event::TreeActivated(x) => {
                    if !n.tree.as_ref()?.nodes.contains_key(x) {
                        return None;
                    }
                }
                Event::TreeExpanded(x, b) => {
                    n.tree.as_mut()?.nodes.get_mut(x)?.expanded = *b;
                }
                Event::Value(v) => n.value = *v,
                Event::Moved { x, y } => {
                    if n.kind != Kind::Window {
                        return None;
                    }
                    n.position = Some((*x, *y));
                }
                Event::Resized { w, h } => {
                    // a misbehaving backend must not feed layout negative or absurd sizes
                    let sz = Size::new((*w).clamp(0, 1 << 16), (*h).clamp(0, 1 << 16));
                    let same = n.client == sz;
                    n.client = sz;
                    n.explicit_size = true;
                    if !same {
                        r.dirty_layout.insert(id);
                    }
                    return Some(!same);
                }
                _ => {}
            }
            (n.kind, n.group)
        };
        match ev {
            Event::Focus(true) => r.focus = Some(id),
            Event::Focus(false) if r.focus == Some(id) => r.focus = None,
            Event::Toggled(true) if kind == Kind::RadioButton && group != 0 => {
                for (k, n) in r.nodes.iter_mut() {
                    if *k != id && n.group == group && n.checked {
                        n.checked = false;
                        unchecked.push(*k);
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

fn wake_if_scheduled() {
    if with(|r| r.scheduled).unwrap_or(false) {
        wake();
    }
}

/// Backend entry point: the user asked to close window `id` (title-bar X, Alt-F4, Cmd-W...).
/// The backend must veto the native close; the core destroys the window on the next loop turn
/// if the app's `on_close` handler allows it (default: allow).
pub fn close_requested(id: WidgetId) {
    let cb = with(|r| r.nodes.get_mut(&id).and_then(|n| n.on_close.take())).flatten();
    let mut allow = true;
    if let Some(mut cb) = cb {
        guarded(|| allow = cb());
        with(|r| {
            if let Some(n) = r.nodes.get_mut(&id) {
                if n.on_close.is_none() {
                    n.on_close = Some(cb);
                }
            }
        });
    }
    if allow && is_alive(id) {
        post(move || destroy(id));
    }
}

// ---------------------------------------------------------------- timers

pub fn timer_start(ms: u32, repeat: bool, cb: Box<dyn FnMut()>) -> Result<u64> {
    let token = NEXT_TIMER.fetch_add(1, Ordering::Relaxed);
    with(|r| {
        r.timers.insert(
            token,
            TimerEntry {
                cb: Some(cb),
                repeat,
            },
        )
    })
    .ok_or(Error::NotInitialized)?;
    if let Err(e) = B::timer_start(token, ms.max(1), repeat) {
        with(|r| r.timers.remove(&token));
        return Err(e);
    }
    Ok(token)
}

pub fn timer_stop(token: u64) {
    if with(|r| r.timers.remove(&token)).flatten().is_some() {
        B::timer_stop(token);
    }
}

/// Backend entry point: timer `token` expired.
pub fn timer_fired(token: u64) {
    let taken = with(|r| {
        let repeat = r.timers.get(&token)?.repeat;
        let cb = if repeat {
            r.timers.get_mut(&token)?.cb.take()
        } else {
            r.timers.remove(&token)?.cb
        };
        Some((cb, repeat))
    })
    .flatten();
    let Some((Some(mut cb), repeat)) = taken else {
        return;
    };
    guarded(|| cb());
    if repeat {
        with(|r| {
            if let Some(t) = r.timers.get_mut(&token) {
                t.cb = Some(cb);
            }
        });
    } else {
        B::timer_stop(token);
    }
    wake_if_scheduled();
}
