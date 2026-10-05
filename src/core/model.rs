//! The widget node and the per-kind models it carries.

use super::events::{Callback, Ev};
use crate::a11y::A11yProps;
use crate::backend::Kind;
use crate::types::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Batch-update state of a table or tree: while `freeze` > 0, backend pushes are deferred and
/// merged into `pending`.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub freeze: u32,
    pub pending: Option<Data>,
}

/// Table model (rows are plain strings).
#[derive(Clone, Debug, Default)]
pub struct TableData {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
    pub sort: Option<(usize, bool)>,
    pub selected: Option<usize>,
    pub batch: Batch,
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
    pub batch: Batch,
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
    /// Remove every node (and the selection); the batch state is kept.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        self.selected = None;
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
}

/// One widget in the registry: the state every kind has, plus [`NodeData`], which holds the state
/// only this kind of widget has (so a `Label` carries no table model and a `Slider` no item list).
pub struct Node {
    pub kind: Kind,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    pub text: String,
    pub tooltip: String,
    pub enabled: bool,
    pub visible: bool,
    /// Last bounds pushed to the backend (relative to the native parent).
    pub bounds: Rect,
    pub a11y: A11yProps,
    pub lay: LayoutProps,
    pub cbs: HashMap<Ev, Callback>,
    /// Attached `PopupMenu` (see `Widget::set_context_menu`).
    pub context_menu: Option<WidgetId>,
    pub data: NodeData,
}

/// Kind-specific node state. Which variant a node has is fixed by its [`Kind`] at creation (see
/// [`NodeData::for_kind`]); the `Node` accessors return `None` for any other kind, so a setter
/// aimed at the wrong kind of widget is a no-op rather than silently stored state.
pub enum NodeData {
    /// Kinds with nothing beyond the common state (labels, buttons, boxes, menus, ...).
    Plain,
    /// `Window`.
    Window(Box<WindowData>),
    /// `Tabs`: the selected page index.
    Tabs(Option<usize>),
    /// `CheckBox`, `RadioButton`.
    Check(CheckData),
    /// `MenuItem`, `CheckMenuItem`.
    MenuItem(MenuItemData),
    /// `TextInput`, `PasswordInput`, `TextArea`.
    Text(TextData),
    /// `Slider`, `SpinBox`, `ProgressBar`.
    Range(RangeData),
    /// `ComboBox`, `ListBox`.
    List(ListData),
    /// `Image`.
    Image(Option<ImageData>),
    /// `Table`.
    Table(Box<TableData>),
    /// `Tree`.
    Tree(Box<TreeData>),
    /// `Splitter`.
    Split(Box<SplitData>),
}

pub struct WindowData {
    /// Client size; `explicit_size` once set by the app or the user.
    pub client: Size,
    pub explicit_size: bool,
    pub resizable: bool,
    /// Last requested (`set_position`) or reported (`Event::Moved`) screen position.
    pub position: Option<(i32, i32)>,
    pub on_close: Option<Box<dyn FnMut() -> bool>>,
}

pub struct CheckData {
    pub checked: bool,
    /// Radio group (0 = none); checking one button unchecks the others of its group.
    pub group: u32,
}

#[derive(Default)]
pub struct MenuItemData {
    pub accel: String,
    /// Only meaningful for `CheckMenuItem`.
    pub checked: bool,
}

pub struct TextData {
    pub placeholder: String,
    pub readonly: bool,
    /// Fixed-pitch font.
    pub monospace: bool,
    /// Soft wrap (TextArea; default on).
    pub wrap: bool,
}

pub struct RangeData {
    pub value: f64,
    /// (min, max, step).
    pub range: (f64, f64, f64),
    /// ProgressBar only.
    pub indeterminate: bool,
}

#[derive(Default)]
pub struct ListData {
    pub items: Vec<String>,
    pub selected: Option<usize>,
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
}

macro_rules! accessors {
    ($($get:ident, $get_mut:ident, $variant:ident, $ty:ty;)*) => {$(
        // the getter of a pair is not needed for every variant
        #[allow(dead_code)]
        pub fn $get(&self) -> Option<&$ty> {
            match &self.data {
                NodeData::$variant(d) => Some(d),
                _ => None,
            }
        }
        pub fn $get_mut(&mut self) -> Option<&mut $ty> {
            match &mut self.data {
                NodeData::$variant(d) => Some(d),
                _ => None,
            }
        }
    )*};
}

impl NodeData {
    pub fn for_kind(kind: Kind) -> NodeData {
        match kind {
            Kind::Window => NodeData::Window(Box::new(WindowData {
                client: Size::default(),
                explicit_size: false,
                resizable: true,
                position: None,
                on_close: None,
            })),
            Kind::Tabs => NodeData::Tabs(None),
            Kind::CheckBox | Kind::RadioButton => NodeData::Check(CheckData {
                checked: false,
                group: 0,
            }),
            Kind::MenuItem | Kind::CheckMenuItem => NodeData::MenuItem(MenuItemData::default()),
            Kind::TextInput | Kind::PasswordInput | Kind::TextArea => NodeData::Text(TextData {
                placeholder: String::new(),
                readonly: false,
                monospace: false,
                wrap: true,
            }),
            Kind::Slider | Kind::SpinBox | Kind::ProgressBar => NodeData::Range(RangeData {
                value: 0.0,
                range: match kind {
                    Kind::ProgressBar => (0.0, 1.0, 0.01),
                    _ => (0.0, 100.0, 1.0),
                },
                indeterminate: false,
            }),
            Kind::ComboBox | Kind::ListBox => NodeData::List(ListData::default()),
            Kind::Image => NodeData::Image(None),
            Kind::Table => NodeData::Table(Box::default()),
            Kind::Tree => NodeData::Tree(Box::default()),
            Kind::Splitter => NodeData::Split(Box::default()),
            _ => NodeData::Plain,
        }
    }
}

impl Node {
    accessors! {
        window, window_mut, Window, WindowData;
        menu_item, menu_item_mut, MenuItem, MenuItemData;
        check, check_mut, Check, CheckData;
        text_data, text_data_mut, Text, TextData;
        range, range_mut, Range, RangeData;
        list, list_mut, List, ListData;
        table, table_mut, Table, TableData;
        tree, tree_mut, Tree, TreeData;
        split, split_mut, Split, SplitData;
    }
    /// The selected index of a list, tab strip or table.
    pub fn selection(&self) -> Option<usize> {
        match &self.data {
            NodeData::List(l) => l.selected,
            NodeData::Tabs(s) => *s,
            NodeData::Table(t) => t.selected,
            _ => None,
        }
    }
    /// How many things `selection` can point at: items, tabs or rows.
    pub fn selectable_len(&self) -> usize {
        match &self.data {
            NodeData::List(l) => l.items.len(),
            NodeData::Tabs(_) => self.children.len(),
            NodeData::Table(t) => t.rows.len(),
            _ => 0,
        }
    }
    pub fn selection_mut(&mut self) -> Option<&mut Option<usize>> {
        match &mut self.data {
            NodeData::List(l) => Some(&mut l.selected),
            NodeData::Tabs(s) => Some(s),
            NodeData::Table(t) => Some(&mut t.selected),
            _ => None,
        }
    }
    /// The checked state of a check box, radio button or check menu item (`false` for other kinds).
    pub fn checked(&self) -> bool {
        match &self.data {
            NodeData::Check(c) => c.checked,
            NodeData::MenuItem(m) => m.checked,
            _ => false,
        }
    }
    pub fn checked_mut(&mut self) -> Option<&mut bool> {
        match &mut self.data {
            NodeData::Check(c) => Some(&mut c.checked),
            NodeData::MenuItem(m) => Some(&mut m.checked),
            _ => None,
        }
    }
    /// Batch-update state of a table or tree.
    pub fn batch_mut(&mut self) -> Option<&mut Batch> {
        match &mut self.data {
            NodeData::Table(t) => Some(&mut t.batch),
            NodeData::Tree(t) => Some(&mut t.batch),
            _ => None,
        }
    }
    pub(super) fn new(kind: Kind) -> Node {
        let boxy = matches!(kind, Kind::Window | Kind::Page | Kind::GroupBox);
        Node {
            kind,
            parent: None,
            children: vec![],
            text: String::new(),
            tooltip: String::new(),
            enabled: true,
            visible: kind != Kind::Window,
            bounds: Rect::default(),
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
            context_menu: None,
            data: NodeData::for_kind(kind),
        }
    }
}
