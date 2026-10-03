//! Accessibility: builds an accesskit tree from the registry and routes AT actions back as if the
//! user had operated the widget. Platform adapters (hooked up by backends via
//! `Backend::a11y_attach` / `a11y_changed`) only need [`tree_for_window`] and [`do_action`].
//!
//! Mapping rules: Label text goes in `value` (accesskit convention); buttons/checkboxes/menu items
//! take their text as the accessible name; an input without an explicit name is labelled by the
//! nearest preceding Label in layout order (so simple forms are accessible for free). Hidden
//! subtrees and non-selected tab pages are omitted. Virtual layout boxes are flattened away.
//! Every widget supports overrides: `set_a11y_name`, `set_a11y_description`, `set_a11y_role`.
//! Mnemonic markers (`&File`) are stripped from names.
//!
//! Threading: AT requests may arrive on any thread. Adapters that are called off the UI thread
//! must use [`latest_tree`] (thread-safe snapshot, kept fresh by the core once the backend calls
//! [`enable_cache`] from `a11y_attach`) and [`post_action`] (queues [`do_action`] on the UI thread).

use crate::backend::{Backend, Event, Kind, Native as B, Prop};
use crate::core::{self, Node, Registry};
use crate::text::strip_mnemonic;
use crate::types::*;
use accesskit::{Action, ActionData, ActionRequest, NodeId, Role, Toggled, TreeId, TreeInfo, TreeUpdate};

use std::collections::HashMap;
use std::sync::Mutex;

static CACHE: Mutex<Option<HashMap<WidgetId, TreeUpdate>>> = Mutex::new(None);

fn cache<R>(f: impl FnOnce(&mut HashMap<WidgetId, TreeUpdate>) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// UI thread: start keeping a thread-safe snapshot of `window`'s tree (call from `a11y_attach`).
pub fn enable_cache(window: WidgetId) {
    refresh_cache_force(window);
}

/// Any thread: the most recent tree built for `window` (None until [`enable_cache`]).
pub fn latest_tree(window: WidgetId) -> Option<TreeUpdate> {
    cache(|c| c.get(&window).cloned())
}

pub(crate) fn refresh_cache(window: WidgetId) {
    if cache(|c| c.contains_key(&window)) {
        refresh_cache_force(window);
    }
}

fn refresh_cache_force(window: WidgetId) {
    if let Some(t) = tree_for_window(window) {
        cache(|c| c.insert(window, t));
    }
}

pub(crate) fn drop_cache(window: WidgetId) {
    cache(|c| c.remove(&window));
}

/// Any thread: deliver an AT action request; it runs [`do_action`] on the UI thread.
pub fn post_action(window: WidgetId, req: ActionRequest) {
    core::post(move || do_action(window, &req));
}

/// Per-widget accessibility overrides.
#[derive(Clone, Debug, Default)]
pub struct A11yProps {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub role: Option<Role>,
}

/// Pixels a splitter moves per AT increment/decrement.
const SASH_STEP: i32 = 10;

const SYNTH: u64 = 1 << 62;
/// Synthetic table nodes: SYNTH | TBL | widget(22 bits) << 38 | row(20 bits) << 18 | col(18 bits).
/// row 0 = header row, row r+1 = data row r; col 0 = the row node itself, col c+1 = cell c.
const TBL: u64 = 1 << 61;
/// Synthetic tree item: SYNTH | TREE | tree node id (globally unique, 40 bits).
const TREE: u64 = 1 << 60;
const MAX_A11Y_ROWS: usize = (1 << 20) - 2;
const MAX_A11Y_COLS: usize = (1 << 18) - 2;
const MAX_TBL_WIDGET: u64 = 1 << 22;
const MAX_A11Y_TREE_DEPTH: usize = 128;

fn tbl_id(id: WidgetId, row: usize, col: usize) -> NodeId {
    NodeId(SYNTH | TBL | ((id.0 & (MAX_TBL_WIDGET - 1)) << 38) | ((row as u64) << 18) | col as u64)
}
fn tree_item_id(node: u64) -> NodeId {
    NodeId(SYNTH | TREE | (node & ((1 << 40) - 1)))
}

enum Target {
    Plain,
    ListItem(usize),
    /// (row field, col field) of a table node.
    Table(usize, usize),
    Tree(u64),
}

fn nid(id: WidgetId) -> NodeId {
    NodeId(id.0)
}
fn option_id(id: WidgetId, i: usize) -> NodeId {
    NodeId(SYNTH | (id.0 << 20) | (i as u64 + 1))
}

pub(crate) fn default_role(k: Kind) -> Role {
    match k {
        Kind::Window => Role::Window,
        Kind::Label => Role::Label,
        Kind::Button => Role::Button,
        Kind::CheckBox => Role::CheckBox,
        Kind::RadioButton => Role::RadioButton,
        Kind::TextInput => Role::TextInput,
        Kind::PasswordInput => Role::PasswordInput,
        Kind::TextArea => Role::MultilineTextInput,
        Kind::ComboBox => Role::ComboBox,
        Kind::ListBox => Role::ListBox,
        Kind::Slider => Role::Slider,
        Kind::ProgressBar => Role::ProgressIndicator,
        Kind::SpinBox => Role::SpinButton,
        Kind::Tabs => Role::TabList,
        Kind::Page => Role::TabPanel,
        Kind::GroupBox => Role::Group,
        Kind::Image => Role::Image,
        Kind::MenuBar => Role::MenuBar,
        Kind::Menu => Role::Menu,
        Kind::MenuItem => Role::MenuItem,
        Kind::CheckMenuItem => Role::MenuItemCheckBox,
        Kind::Table => Role::Table,
        Kind::Tree => Role::Tree,
        Kind::PopupMenu => Role::Menu,
        Kind::Sash => Role::Splitter,
        _ => Role::GenericContainer,
    }
}

struct Ctx<'a> {
    r: &'a Registry,
    nodes: Vec<(NodeId, accesskit::Node)>,
    label: Option<String>,
}

/// Visible tree items below `parent` (collapsed nodes hide their children); nested like the tree.
fn tree_items(cx: &mut Ctx, t: &core::TreeData, parent: Option<u64>, level: usize) -> Vec<NodeId> {
    let list = t.children_of(parent);
    let mut out = vec![];
    for (i, c) in list.iter().enumerate() {
        let Some(tn) = t.nodes.get(c) else { continue };
        let mut it = accesskit::Node::new(Role::TreeItem);
        it.set_label(tn.text.clone());
        it.set_level(level);
        it.set_selected(t.selected == Some(*c));
        it.set_position_in_set(i);
        it.set_size_of_set(list.len());
        it.add_action(Action::Click);
        let expandable = tn.has_children || !tn.children.is_empty();
        if expandable {
            it.set_expanded(tn.expanded);
            it.add_action(if tn.expanded { Action::Collapse } else { Action::Expand });
        }
        // bounded recursion: a pathologically deep tree must not overflow the stack
        if tn.expanded && level < MAX_A11Y_TREE_DEPTH {
            let kids = tree_items(cx, t, Some(*c), level + 1);
            it.set_children(kids);
        }
        out.push(tree_item_id(*c));
        cx.nodes.push((tree_item_id(*c), it));
    }
    out
}

/// Build node(s) for widget `id`; returns the accesskit ids to attach to the parent
/// (empty for omitted nodes, the flattened children for virtual boxes).
fn build(cx: &mut Ctx, id: WidgetId, off: (i32, i32)) -> Vec<NodeId> {
    let Some(n) = cx.r.nodes.get(&id) else { return vec![] };
    if (!n.visible && n.kind != Kind::Window) || n.kind == Kind::MenuSeparator {
        return vec![];
    }
    if !n.kind.is_native() {
        return n.children.iter().flat_map(|c| build(cx, *c, off)).collect();
    }
    let k = n.kind;
    let abs = (off.0 + n.bounds.x, off.1 + n.bounds.y);
    let mut a = accesskit::Node::new(n.a11y.role.unwrap_or_else(|| default_role(k)));
    let name = n.a11y.name.clone();
    let named_by_text = matches!(
        k,
        Kind::Button | Kind::CheckBox | Kind::RadioButton | Kind::GroupBox | Kind::Page
            | Kind::Window | Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem
    );
    match name {
        Some(s) => a.set_label(s),
        None if named_by_text && !n.text.is_empty() => a.set_label(strip_mnemonic(&n.text)),
        None if matches!(k, Kind::TextInput | Kind::PasswordInput | Kind::TextArea | Kind::ComboBox
            | Kind::ListBox | Kind::Slider | Kind::SpinBox | Kind::ProgressBar | Kind::Table | Kind::Tree) =>
        {
            if let Some(l) = &cx.label {
                a.set_label(l.clone());
            }
        }
        None => {}
    }
    if let Some(d) = n.a11y.desc.clone().or_else(|| (!n.tooltip.is_empty()).then(|| n.tooltip.clone())) {
        a.set_description(d);
    }
    if !n.enabled {
        a.set_disabled();
    }
    if !matches!(k, Kind::Window | Kind::Menu | Kind::MenuBar | Kind::MenuItem | Kind::CheckMenuItem) {
        let (x, y) = (abs.0 as f64, abs.1 as f64);
        a.set_bounds(accesskit::Rect { x0: x, y0: y, x1: x + n.bounds.w as f64, y1: y + n.bounds.h as f64 });
    }
    if !n.accel.is_empty() {
        a.set_keyboard_shortcut(n.accel.clone());
    }
    let mut kids: Vec<NodeId> = vec![];
    match k {
        Kind::Label => {
            a.set_value(n.text.clone());
            cx.label = Some(strip_mnemonic(&n.text));
        }
        Kind::Button | Kind::MenuItem => a.add_action(Action::Click),
        Kind::CheckBox | Kind::RadioButton | Kind::CheckMenuItem => {
            a.set_toggled(Toggled::from(n.checked));
            a.add_action(Action::Click);
        }
        Kind::TextInput | Kind::TextArea => {
            a.set_value(n.text.clone());
            if !n.placeholder.is_empty() {
                a.set_placeholder(n.placeholder.clone());
            }
            if n.readonly {
                a.set_read_only();
            }
            a.add_action(Action::SetValue);
        }
        Kind::PasswordInput => a.add_action(Action::SetValue),
        Kind::ComboBox => {
            if let Some(s) = n.selected.and_then(|i| n.items.get(i)) {
                a.set_value(s.clone());
            }
        }
        Kind::ListBox => {
            for (i, it) in n.items.iter().enumerate() {
                let mut o = accesskit::Node::new(Role::ListBoxOption);
                o.set_label(it.clone());
                o.set_selected(n.selected == Some(i));
                o.set_position_in_set(i);
                o.set_size_of_set(n.items.len());
                o.add_action(Action::Click);
                kids.push(option_id(id, i));
                cx.nodes.push((option_id(id, i), o));
            }
        }
        Kind::Table => {
            if let Some(t) = &n.table {
                let ncols = t.columns.len().min(MAX_A11Y_COLS);
                let nrows = t.rows.len().min(MAX_A11Y_ROWS);
                a.set_row_count(nrows + 1);
                a.set_column_count(ncols);
                let mut header = accesskit::Node::new(Role::Row);
                header.set_row_index(0);
                let mut hk = vec![];
                for (c, col) in t.columns.iter().take(ncols).enumerate() {
                    let mut h = accesskit::Node::new(Role::ColumnHeader);
                    h.set_label(col.title.clone());
                    h.set_row_index(0);
                    h.set_column_index(c);
                    if col.sortable {
                        h.add_action(Action::Click);
                    }
                    hk.push(tbl_id(id, 0, c + 1));
                    cx.nodes.push((tbl_id(id, 0, c + 1), h));
                }
                header.set_children(hk);
                kids.push(tbl_id(id, 0, 0));
                cx.nodes.push((tbl_id(id, 0, 0), header));
                for (i, row) in t.rows.iter().take(nrows).enumerate() {
                    let mut r = accesskit::Node::new(Role::Row);
                    r.set_row_index(i + 1);
                    r.set_selected(n.selected == Some(i));
                    r.add_action(Action::Click);
                    let mut ck = vec![];
                    for c in 0..ncols {
                        let mut cell = accesskit::Node::new(Role::Cell);
                        cell.set_value(row.get(c).cloned().unwrap_or_default());
                        cell.set_row_index(i + 1);
                        cell.set_column_index(c);
                        cell.add_action(Action::Click);
                        ck.push(tbl_id(id, i + 1, c + 1));
                        cx.nodes.push((tbl_id(id, i + 1, c + 1), cell));
                    }
                    r.set_children(ck);
                    kids.push(tbl_id(id, i + 1, 0));
                    cx.nodes.push((tbl_id(id, i + 1, 0), r));
                }
            }
        }
        Kind::Tree => {
            if let Some(t) = &n.tree {
                kids.extend(tree_items(cx, t, None, 1));
            }
        }
        Kind::Sash => {
            // value = first-pane size; the separator runs across the split direction
            if let Some(sp) = n.parent.and_then(|p| cx.r.nodes.get(&p)).and_then(|p| p.split.as_ref()) {
                let len = if sp.orient == Orientation::Horizontal { sp.area.w } else { sp.area.h };
                a.set_numeric_value(sp.actual as f64);
                a.set_min_numeric_value(sp.clamp(0, len) as f64);
                a.set_max_numeric_value(sp.clamp(i32::MAX, len) as f64);
                a.set_numeric_value_step(SASH_STEP as f64);
                a.set_orientation(match sp.orient {
                    Orientation::Horizontal => accesskit::Orientation::Vertical,
                    Orientation::Vertical => accesskit::Orientation::Horizontal,
                });
                a.add_action(Action::SetValue);
                a.add_action(Action::Increment);
                a.add_action(Action::Decrement);
            }
        }
        Kind::Slider | Kind::SpinBox | Kind::ProgressBar => {
            if !(k == Kind::ProgressBar && n.indeterminate) {
                a.set_numeric_value(n.value);
            }
            a.set_min_numeric_value(n.range.0);
            a.set_max_numeric_value(n.range.1);
            if k != Kind::ProgressBar {
                a.set_numeric_value_step(n.range.2);
                a.add_action(Action::SetValue);
                a.add_action(Action::Increment);
                a.add_action(Action::Decrement);
            }
        }
        _ => {}
    }
    if matches!(k, Kind::Button | Kind::CheckBox | Kind::RadioButton | Kind::TextInput | Kind::PasswordInput
        | Kind::TextArea | Kind::ComboBox | Kind::ListBox | Kind::Slider | Kind::SpinBox | Kind::Tabs
        | Kind::Table | Kind::Tree | Kind::Sash)
    {
        a.add_action(Action::Focus);
    }
    let child_off = if k == Kind::Window { (0, 0) } else { abs };
    let chosen: Vec<WidgetId> = if k == Kind::Tabs {
        n.selected.and_then(|i| n.children.get(i)).copied().into_iter().collect()
    } else {
        n.children.clone()
    };
    for c in chosen {
        kids.extend(build(cx, c, child_off));
    }
    a.set_children(kids);
    cx.nodes.push((nid(id), a));
    vec![nid(id)]
}

/// Full tree for a window (root = the window). `None` if the id is not a live window.
pub fn tree_for_window(window: WidgetId) -> Option<TreeUpdate> {
    core::with(|r| {
        if r.nodes.get(&window)?.kind != Kind::Window {
            return None;
        }
        let mut cx = Ctx { r, nodes: vec![], label: None };
        build(&mut cx, window, (0, 0));
        let focus = r
            .focus
            .filter(|f| r.window_of(*f) == Some(window) && cx.nodes.iter().any(|(i, _)| *i == nid(*f)))
            .unwrap_or(window);
        let mut info = TreeInfo::new(nid(window));
        info.toolkit_name = Some("rungui".into());
        info.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
        Some(TreeUpdate { nodes: cx.nodes, tree: Some(info), tree_id: TreeId::ROOT, focus: nid(focus) })
    })
    .flatten()
}

/// Apply a change as if the user did it: push to the backend, then fire the app's callbacks.
fn user_change(id: WidgetId, prop: Prop, ev: Event) {
    B::set(id, &prop);
    core::event(id, ev);
}

/// Handle an action request from an assistive technology (call on the main thread).
pub fn do_action(window: WidgetId, req: &ActionRequest) {
    let t = req.target_node.0;
    let (id, target) = if t & SYNTH != 0 && t & TBL != 0 {
        (
            WidgetId((t >> 38) & (MAX_TBL_WIDGET - 1)),
            Target::Table(((t >> 18) & 0xF_FFFF) as usize, (t & 0x3_FFFF) as usize),
        )
    } else if t & SYNTH != 0 && t & TREE != 0 {
        let node = t & ((1 << 40) - 1);
        let owner = core::with(|r| {
            r.nodes.iter().find(|(_, n)| n.tree.as_ref().is_some_and(|tr| tr.nodes.contains_key(&node))).map(|(k, _)| *k)
        })
        .flatten();
        let Some(owner) = owner else { return };
        (owner, Target::Tree(node))
    } else if t & SYNTH != 0 {
        (WidgetId((t & !SYNTH) >> 20), Target::ListItem(((t & 0xF_FFFF) as usize).wrapping_sub(1)))
    } else {
        (WidgetId(t), Target::Plain)
    };
    let opt = match target {
        Target::ListItem(i) => Some(i),
        _ => None,
    };
    let snap = core::read(id, |n: &Node| (n.kind, n.checked, n.value, n.range, n.items.len()));
    let Some((kind, checked, value, range, nitems)) = snap else { return };
    // ignore requests for other windows, and for disabled or hidden widgets (AT must not bypass them)
    let usable = core::with(|r| {
        if r.window_of(id) != Some(window) {
            return false;
        }
        let mut cur = Some(id);
        while let Some(i) = cur {
            let Some(n) = r.nodes.get(&i) else { return false };
            if !n.enabled || (!n.visible && n.kind != Kind::Window) {
                return false;
            }
            cur = n.parent;
        }
        true
    })
    .unwrap_or(false);
    if !usable {
        return;
    }
    match (&target, &req.action) {
        (Target::Table(row, col), Action::Click) if kind == Kind::Table => {
            let (nrows, ncols) = core::read(id, |n| n.table.as_ref().map_or((0, 0), |t| (t.rows.len(), t.columns.len())))
                .unwrap_or((0, 0));
            if *row == 0 {
                if *col >= 1 && *col <= ncols {
                    core::event(id, Event::ColumnClicked(*col - 1));
                }
            } else if *row <= nrows {
                let i = *row - 1;
                user_change(id, Prop::Selected(Some(i)), Event::Selected(Some(i)));
            }
            return;
        }
        (Target::Tree(node), Action::Click) if kind == Kind::Tree => {
            user_change(id, Prop::TreeSelected(Some(*node)), Event::TreeSelected(Some(*node)));
            return;
        }
        (Target::Tree(node), Action::Expand | Action::Collapse) if kind == Kind::Tree => {
            let open = req.action == Action::Expand;
            let node = *node;
            core::data_update(id, core::Data::TreeRows, |n| {
                if let Some(x) = n.tree.as_mut().and_then(|t| t.nodes.get_mut(&node)) {
                    x.expanded = open;
                }
            });
            core::event(id, Event::TreeExpanded(node, open));
            return;
        }
        (Target::Table(..) | Target::Tree(_), _) => return,
        _ => {}
    }
    match (&req.action, kind) {
        (Action::Focus, _) => B::set(id, &Prop::Focus),
        (Action::Click, Kind::Button | Kind::MenuItem) => core::event(id, Event::Click),
        (Action::Click, Kind::CheckBox | Kind::CheckMenuItem) => {
            user_change(id, Prop::Checked(!checked), Event::Toggled(!checked))
        }
        (Action::Click, Kind::RadioButton) if !checked => {
            user_change(id, Prop::Checked(true), Event::Toggled(true))
        }
        (Action::Click, Kind::ListBox) => {
            if let Some(i) = opt.filter(|i| *i < nitems) {
                user_change(id, Prop::Selected(Some(i)), Event::Selected(Some(i)));
            }
        }
        (Action::SetValue, Kind::TextInput | Kind::TextArea | Kind::PasswordInput) => {
            if let Some(ActionData::Value(s)) = &req.data {
                // NULs cannot be represented natively; keep core state and native text identical
                let s: String = s.replace('\0', "\u{FFFD}");
                user_change(id, Prop::Text(&s), Event::Text(s.clone()));
            }
        }
        (Action::SetValue, Kind::Slider | Kind::SpinBox) => {
            if let Some(ActionData::NumericValue(v)) = &req.data {
                if !v.is_finite() {
                    return;
                }
                let v = v.clamp(range.0, range.1);
                user_change(id, Prop::Value(v), Event::Value(v));
            }
        }
        (Action::SetValue | Action::Increment | Action::Decrement, Kind::Sash) => {
            let Some((split, pos)) = core::with(|r| {
                let p = r.nodes.get(&id)?.parent?;
                Some((p, r.nodes.get(&p)?.split.as_ref()?.actual))
            })
            .flatten() else {
                return;
            };
            let want = match (&req.action, &req.data) {
                (Action::SetValue, Some(ActionData::NumericValue(v))) if v.is_finite() => {
                    v.clamp(i32::MIN as f64, i32::MAX as f64) as i32
                }
                (Action::Increment, _) => pos.saturating_add(SASH_STEP),
                (Action::Decrement, _) => pos.saturating_sub(SASH_STEP),
                _ => return,
            };
            core::split_set(split, want, true);
        }
        (Action::Increment | Action::Decrement, Kind::Slider | Kind::SpinBox) => {
            let d = if req.action == Action::Increment { range.2 } else { -range.2 };
            let v = (value + d).clamp(range.0, range.1);
            user_change(id, Prop::Value(v), Event::Value(v));
        }
        _ => {}
    }
}
