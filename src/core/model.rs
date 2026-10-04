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
    pub(super) fn new(kind: Kind) -> Node {
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
