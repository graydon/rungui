//! Pure-Rust in-memory backend: selected by feature `mock` and always under `cargo test`.
//! Records everything the core pushes, fakes text metrics (8 px per char) and lets tests act as
//! the "user" (`user_click`, `user_text`, ...) and as the OS (`pump`, `advance`, `resize_window`).
//! All state is thread-local, so parallel test threads are isolated.

use super::*;
use crate::core;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct MockWidget {
    pub kind: Option<Kind>,
    pub parent: Option<WidgetId>,
    pub text: String,
    pub tooltip: String,
    pub placeholder: String,
    pub accel: String,
    pub enabled: bool,
    pub visible: bool,
    pub checked: bool,
    pub readonly: bool,
    pub indeterminate: bool,
    pub focused: bool,
    pub value: f64,
    pub range: (f64, f64, f64),
    pub items: Vec<String>,
    pub selected: Option<usize>,
    pub bounds: Rect,
    pub image: Option<ImageData>,
    /// Number of `Prop::Bounds` pushes received (to test change-only updates).
    pub bounds_pushes: u32,
    /// Table state as last pushed by the core.
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
    pub sort: Option<(usize, bool)>,
    /// Tree state as last pushed by the core (user_tree_expand mutates `expanded`).
    pub tree_rows: Vec<TreeRow>,
    pub tree_selected: Option<u64>,
    /// Sash orientation (`Prop::Orientation`).
    pub orientation: Option<Orientation>,
    pub monospace: bool,
    /// TextArea soft wrap; starts `true` like a native text view.
    pub wrap: bool,
    /// Window screen position (`Prop::Position`) and minimum client size (`Prop::MinSize`).
    pub position: Option<(i32, i32)>,
    pub min_size: Size,
}

/// What the core last told the mock about one native widget's accessibility (`a11y_changed`).
#[derive(Clone, Debug, PartialEq)]
pub struct A11yRecord {
    pub id: WidgetId,
    pub name: Option<String>,
    pub description: Option<String>,
    pub role: crate::A11yRole,
}

/// One `Backend::popup_menu` call.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupRecord {
    pub menu: WidgetId,
    pub window: Option<WidgetId>,
    pub at: Option<(i32, i32)>,
    /// Direct children of the menu at show time: (kind, text, enabled).
    pub items: Vec<(Kind, String, bool)>,
}

#[derive(Default)]
struct State {
    /// Ordered by id, which is creation order (ids are monotonic).
    widgets: BTreeMap<WidgetId, MockWidget>,
    timers: HashMap<u64, (u32, bool, u32)>, // token -> (period, repeat, elapsed)
    answers: VecDeque<Answer>,
    files: VecDeque<Vec<String>>,
    last_message: Option<MessageSpec>,
    last_file_spec: Option<FileSpec>,
    fail_create: bool,
    fail_timer: bool,
    /// Kinds `create` refuses with `Unsupported` (to test optional-widget fallbacks).
    unsupported: Vec<Kind>,
    popups: Vec<PopupRecord>,
    /// Per window: the accessibility metadata of its last `a11y_changed`.
    a11y: HashMap<WidgetId, Vec<A11yRecord>>,
    /// Per popup: the item the simulated user picks (None = dismissed).
    popup_choices: VecDeque<Option<WidgetId>>,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
    /// `Backend::wake` calls made on this thread since `init` (see [`wake_count`]).
    static WAKES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
static WOKEN: AtomicBool = AtomicBool::new(false);

fn st<R>(f: impl FnOnce(&mut State) -> R) -> R {
    S.with(|s| f(&mut s.borrow_mut()))
}

pub struct Mock;

impl Backend for Mock {
    fn init(_app_name: &str) -> Result<()> {
        st(|s| *s = State::default());
        Ok(())
    }
    fn run() {
        pump();
    }
    fn quit() {}
    fn wake() {
        WOKEN.store(true, Ordering::SeqCst);
        let _ = WAKES.try_with(|w| w.set(w.get() + 1));
    }
    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        st(|s| {
            if s.fail_timer {
                s.fail_timer = false;
                return Err(Error::Backend("mock: timer failed".into()));
            }
            s.timers.insert(token, (millis, repeat, 0));
            Ok(())
        })
    }
    fn timer_stop(token: u64) {
        st(|s| s.timers.remove(&token));
    }
    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
        st(|s| {
            if s.fail_create {
                s.fail_create = false;
                return Err(Error::Backend("mock: create failed".into()));
            }
            if s.unsupported.contains(&kind) {
                return Err(Error::Unsupported);
            }
            if let Some(p) = parent {
                if !s.widgets.contains_key(&p) {
                    return Err(Error::Backend("mock: unknown parent".into()));
                }
            }
            s.widgets.insert(
                id,
                MockWidget {
                    kind: Some(kind),
                    parent,
                    enabled: true,
                    visible: kind != Kind::Window,
                    range: (0.0, 100.0, 1.0),
                    wrap: true,
                    ..Default::default()
                },
            );
            Ok(())
        })
    }
    fn destroy(id: WidgetId) {
        st(|s| {
            s.widgets.remove(&id);
            s.a11y.remove(&id);
        });
    }
    fn set(id: WidgetId, prop: &Prop) {
        st(|s| {
            let Some(w) = s.widgets.get_mut(&id) else {
                return;
            };
            if let Some(kind) = w.kind {
                assert!(
                    prop.applies_to(kind),
                    "core sent {prop:?} to a {kind:?}, which does not take it"
                );
            }
            match prop {
                Prop::Text(t) => w.text = t.to_string(),
                Prop::Tooltip(t) => w.tooltip = t.to_string(),
                Prop::Placeholder(t) => w.placeholder = t.to_string(),
                Prop::Enabled(b) => w.enabled = *b,
                Prop::Visible(b) => w.visible = *b,
                Prop::Checked(b) => w.checked = *b,
                Prop::Value(v) => w.value = *v,
                Prop::Range { min, max, step } => w.range = (*min, *max, *step),
                Prop::Items(i) => w.items = i.to_vec(),
                Prop::Selected(i) => w.selected = *i,
                Prop::Bounds(r) => {
                    w.bounds = *r;
                    w.bounds_pushes += 1;
                }
                Prop::Image(i) => w.image = i.cloned(),
                Prop::Accel(a) => w.accel = a.to_string(),
                Prop::ReadOnly(b) => w.readonly = *b,
                Prop::Indeterminate(b) => w.indeterminate = *b,
                Prop::Focus => w.focused = true,
                Prop::Columns(c) => w.columns = c.to_vec(),
                Prop::Rows(r) => w.rows = r.to_vec(),
                Prop::SortIndicator(x) => w.sort = *x,
                Prop::TreeRows(r) => w.tree_rows = r.to_vec(),
                Prop::TreeSelected(x) => w.tree_selected = *x,
                Prop::Orientation(o) => w.orientation = Some(*o),
                Prop::Monospace(b) => w.monospace = *b,
                Prop::Wrap(b) => w.wrap = *b,
                Prop::Position { x, y } => w.position = Some((*x, *y)),
                Prop::MinSize(m) => w.min_size = *m,
                _ => {}
            }
        });
    }
    fn a11y_changed(window: WidgetId) {
        let records = crate::a11y::resolve(window).map(|v| {
            v.into_iter()
                .map(|r| A11yRecord {
                    id: r.id,
                    name: r.name,
                    description: r.description,
                    role: r.role,
                })
                .collect()
        });
        st(|s| match records {
            Some(v) => s.a11y.insert(window, v),
            None => s.a11y.remove(&window),
        });
    }
    fn preferred_size(id: WidgetId) -> Size {
        st(|s| {
            let Some(w) = s.widgets.get(&id) else {
                return Size::default();
            };
            let tw = 8 * w.text.chars().count() as i32;
            match w.kind {
                Some(Kind::Label) => Size::new(tw, 16),
                Some(Kind::Button) => Size::new(tw + 24, 28),
                Some(Kind::CheckBox | Kind::RadioButton) => Size::new(tw + 24, 20),
                Some(Kind::TextInput | Kind::PasswordInput) => Size::new(160, 24),
                Some(Kind::TextArea) => Size::new(if w.monospace { 240 } else { 200 }, 100),
                Some(Kind::ComboBox) => Size::new(140, 26),
                Some(Kind::ListBox) => Size::new(160, 100),
                Some(Kind::Table) => Size::new(300, 150),
                Some(Kind::Tree) => Size::new(200, 200),
                Some(Kind::Slider) => Size::new(150, 24),
                Some(Kind::ProgressBar) => Size::new(150, 16),
                Some(Kind::SpinBox) => Size::new(80, 24),
                Some(Kind::Image) => w
                    .image
                    .as_ref()
                    .map_or(Size::new(32, 32), |i| Size::new(i.w as i32, i.h as i32)),
                _ => Size::default(),
            }
        })
    }
    fn chrome(id: WidgetId) -> Size {
        st(|s| match s.widgets.get(&id).and_then(|w| w.kind) {
            Some(Kind::GroupBox) => Size::new(12, 24),
            Some(Kind::Tabs) => Size::new(4, 28),
            _ => Size::default(),
        })
    }
    fn native_handle(id: WidgetId) -> Option<NativeHandle> {
        st(|s| s.widgets.contains_key(&id)).then_some(NativeHandle::Gtk(id.0 as usize))
    }
    fn message_box(_parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        st(|s| {
            s.last_message = Some(spec.clone());
            s.answers.pop_front().unwrap_or(Answer::Ok)
        })
    }
    fn file_dialog(_parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String> {
        st(|s| {
            s.last_file_spec = Some(spec.clone());
            s.files.pop_front().unwrap_or_default()
        })
    }
    fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
        let choice = st(|s| {
            let items = s
                .widgets
                .values()
                .filter(|w| w.parent == Some(menu))
                .map(|w| (w.kind.unwrap_or(Kind::Spacer), w.text.clone(), w.enabled))
                .collect();
            s.popups.push(PopupRecord {
                menu,
                window: parent_window,
                at,
                items,
            });
            s.popup_choices.pop_front().flatten()
        });
        // the simulated user picks an item (enabled, a descendant of this menu) like a real nested loop would
        let Some(item) = choice else { return };
        let (kind, checked, ok) = st(|s| {
            // enabled item whose parent chain reaches `menu` through enabled submenus
            let mut ok = false;
            let mut cur = Some(item);
            while let Some(c) = cur {
                let Some(w) = s.widgets.get(&c) else { break };
                if !w.enabled {
                    break;
                }
                if w.parent == Some(menu) {
                    ok = true;
                    break;
                }
                cur = w.parent;
            }
            let w = s.widgets.get(&item);
            (w.and_then(|w| w.kind), w.is_some_and(|w| w.checked), ok)
        });
        if !ok {
            return;
        }
        match kind {
            Some(Kind::MenuItem) => user(item, Event::Click),
            Some(Kind::CheckMenuItem) => user(item, Event::Toggled(!checked)),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- test/driver helpers

/// Snapshot of the mock's view of a widget.
pub fn widget(id: WidgetId) -> Option<MockWidget> {
    core::flush_models(); // table/tree models reach the backend at the next loop turn
    st(|s| s.widgets.get(&id).cloned())
}
/// The accessibility records pushed for `window` by its last `a11y_changed`, in layout order.
pub fn a11y(window: WidgetId) -> Option<Vec<A11yRecord>> {
    core::flush_models();
    st(|s| s.a11y.get(&window).cloned())
}
/// How many times the core asked the backend to wake the loop on this thread. The core collapses a
/// burst of changes into one wake-up until the loop has run ([`pump`]).
pub fn wake_count() -> u32 {
    WAKES.with(|w| w.get())
}
/// Make the next `timer_start` fail.
pub fn fail_next_timer() {
    st(|s| s.fail_timer = true);
}
pub fn widget_count() -> usize {
    st(|s| s.widgets.len())
}
/// Run what the OS loop would: process posted closures and layout if `wake` was called.
pub fn pump() {
    WOKEN.store(false, Ordering::SeqCst);
    core::drain_posted(); // per-thread queue, so unconditional is safe under parallel tests
}
/// Advance virtual time, firing due timers.
pub fn advance(ms: u32) {
    let due: Vec<u64> = st(|s| {
        let mut due = vec![];
        for (tok, (period, _, el)) in s.timers.iter_mut() {
            *el += ms;
            while *el >= *period {
                *el -= *period;
                due.push(*tok);
                if *period == 0 {
                    break;
                }
            }
        }
        due
    });
    for tok in due {
        core::timer_fired(tok);
    }
}
pub fn timer_count() -> usize {
    st(|s| s.timers.len())
}
/// Simulate the user (state change + event). `Text`/`Toggled`/`Selected`/`Value` are mirrored first.
pub fn user(id: WidgetId, ev: Event) {
    st(|s| {
        if let Some(w) = s.widgets.get_mut(&id) {
            match &ev {
                Event::Text(t) => w.text = t.clone(),
                Event::Toggled(b) => w.checked = *b,
                Event::Selected(i) => w.selected = *i,
                Event::Value(v) => w.value = *v,
                _ => {}
            }
        }
    });
    core::event(id, ev);
}
pub fn user_click(id: WidgetId) {
    user(id, Event::Click)
}
pub fn user_text(id: WidgetId, t: &str) {
    user(id, Event::Text(t.into()))
}
/// Simulate the user resizing a window's client area.
pub fn resize_window(id: WidgetId, w: i32, h: i32) {
    st(|s| {
        if let Some(x) = s.widgets.get_mut(&id) {
            x.bounds = Rect::new(0, 0, w, h);
        }
    });
    core::event(id, Event::Resized { w, h });
}
/// Simulate the title-bar close button.
pub fn user_close(id: WidgetId) {
    core::close_requested(id)
}
pub fn queue_answer(a: Answer) {
    st(|s| s.answers.push_back(a));
}
pub fn queue_files(f: &[&str]) {
    st(|s| s.files.push_back(f.iter().map(|x| x.to_string()).collect()));
}
pub fn last_message() -> Option<MessageSpec> {
    st(|s| s.last_message.clone())
}
pub fn last_file_spec() -> Option<FileSpec> {
    st(|s| s.last_file_spec.clone())
}
/// Queue what the simulated user does in the next `popup_menu`: `Some(item)` picks it, `None` dismisses.
pub fn queue_popup_choice(item: Option<WidgetId>) {
    st(|s| s.popup_choices.push_back(item));
}
pub fn popup_log() -> Vec<PopupRecord> {
    st(|s| s.popups.clone())
}
pub fn last_popup() -> Option<PopupRecord> {
    st(|s| s.popups.last().cloned())
}
pub fn clear_popup_log() {
    st(|s| s.popups.clear());
}
/// Simulate a right-click / Menu key at window-client (x, y) on `id`.
pub fn user_context_menu(id: WidgetId, x: i32, y: i32) {
    core::event(id, Event::ContextMenu { x, y })
}
/// Simulate the user selecting table row `row` (None = clear).
pub fn user_select_row(id: WidgetId, row: Option<usize>) {
    user(id, Event::Selected(row))
}
pub fn user_activate_row(id: WidgetId, row: usize) {
    core::event(id, Event::Activated(row))
}
pub fn user_click_column(id: WidgetId, col: usize) {
    core::event(id, Event::ColumnClicked(col))
}
pub fn user_tree_select(id: WidgetId, node: Option<u64>) {
    st(|s| {
        if let Some(w) = s.widgets.get_mut(&id) {
            w.tree_selected = node;
        }
    });
    core::event(id, Event::TreeSelected(node))
}
pub fn user_tree_activate(id: WidgetId, node: u64) {
    core::event(id, Event::TreeActivated(node))
}
/// Simulate the user expanding/collapsing a tree node (native state first, then the event).
pub fn user_tree_expand(id: WidgetId, node: u64, expanded: bool) {
    st(|s| {
        if let Some(r) = s
            .widgets
            .get_mut(&id)
            .and_then(|w| w.tree_rows.iter_mut().find(|r| r.node == node))
        {
            r.expanded = expanded;
        }
    });
    core::event(id, Event::TreeExpanded(node, expanded))
}
/// Make `create` of these kinds fail with `Unsupported` from now on (empty = none), like a
/// backend that has not implemented them.
pub fn set_unsupported(kinds: &[Kind]) {
    st(|s| s.unsupported = kinds.to_vec());
}
/// The native sash of a splitter (None if the backend refused to create one).
pub fn sash_of(splitter: WidgetId) -> Option<WidgetId> {
    core::read(splitter, |n| n.split().and_then(|s| s.sash)).flatten()
}
/// Simulate the user dragging `splitter`'s sash so that its leading edge is at `pos`
/// (native-parent client coordinates, like its bounds). No-op without a sash.
pub fn user_drag_sash(splitter: WidgetId, pos: i32) {
    if let Some(s) = sash_of(splitter) {
        core::event(s, Event::SashDragged(pos));
    }
}
/// Simulate a drag by `delta` pixels along the splitter's axis from where the sash is now.
pub fn user_drag_sash_by(splitter: WidgetId, delta: i32) {
    let Some(s) = sash_of(splitter) else { return };
    let Some(w) = widget(s) else { return };
    let start = if w.orientation == Some(Orientation::Vertical) {
        w.bounds.y
    } else {
        w.bounds.x
    };
    core::event(s, Event::SashDragged(start + delta));
}
/// Simulate a key press on `splitter`'s focused sash (see [`SashKey`](crate::SashKey)).
pub fn user_sash_key(splitter: WidgetId, key: super::SashKey) {
    if let Some(s) = sash_of(splitter) {
        core::event(s, Event::SashKey(key));
    }
}
/// Simulate the user moving a window to screen position (x, y).
pub fn user_move_window(id: WidgetId, x: i32, y: i32) {
    st(|s| {
        if let Some(w) = s.widgets.get_mut(&id) {
            w.position = Some((x, y));
        }
    });
    core::event(id, Event::Moved { x, y });
}
pub fn fail_next_create() {
    st(|s| s.fail_create = true);
}
/// Indented text rendering of the native widget tree (handy in examples).
pub fn dump() -> String {
    st(|s| {
        fn rec(s: &State, id: WidgetId, depth: usize, out: &mut String) {
            let Some(w) = s.widgets.get(&id) else { return };
            let b = w.bounds;
            out.push_str(&format!(
                "{}{:?} {:?} [{},{} {}x{}]{}{}\n",
                "  ".repeat(depth),
                w.kind.unwrap_or(Kind::Spacer),
                w.text,
                b.x,
                b.y,
                b.w,
                b.h,
                if w.visible { "" } else { " hidden" },
                if w.enabled { "" } else { " disabled" },
            ));
            for (c, _) in s.widgets.iter().filter(|(_, x)| x.parent == Some(id)) {
                rec(s, *c, depth + 1, out);
            }
        }
        let mut out = String::new();
        for (id, _) in s.widgets.iter().filter(|(_, w)| w.parent.is_none()) {
            rec(s, *id, 0, &mut out);
        }
        out
    })
}
