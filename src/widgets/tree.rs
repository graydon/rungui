//! Hierarchical `Tree`.

use super::*;

fn tree_do<R>(id: WidgetId, f: impl FnOnce(&core::TreeData) -> R) -> Option<R> {
    core::read(id, |n| n.tree().map(f)).flatten()
}

impl Tree {
    /// An empty tree.
    pub fn new(parent: impl Into<WidgetId>) -> Tree {
        make(Tree::from_id, Kind::Tree, parent, |_| {})
    }
    /// Append a node under `parent` (`None` = top level). Returns `TreeNodeId(0)` for an unknown parent.
    pub fn add(&self, parent: Option<TreeNodeId>, text: &str) -> TreeNodeId {
        self.insert(parent, usize::MAX, text)
    }
    /// Insert at child position `index` (clamped).
    pub fn insert(&self, parent: Option<TreeNodeId>, index: usize, text: &str) -> TreeNodeId {
        let mut out = 0;
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree_mut() {
                out = t.insert(parent.map(|p| p.0), index, text);
            }
        });
        TreeNodeId(out)
    }
    /// Remove a node and its subtree (the selection clears if it was inside).
    pub fn remove(&self, node: TreeNodeId) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree_mut() {
                t.remove(node.0);
            }
        });
    }
    /// Remove every node.
    pub fn clear(&self) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree_mut() {
                t.clear();
            }
        });
    }
    /// Change a node's text.
    pub fn set_text(&self, node: TreeNodeId, text: &str) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(x) = n.tree_mut().and_then(|t| t.nodes.get_mut(&node.0)) {
                x.text = text.to_string();
            }
        });
    }
    /// A node's text (empty for an unknown node).
    pub fn text(&self, node: TreeNodeId) -> String {
        tree_do(self.id(), |t| t.nodes.get(&node.0).map(|n| n.text.clone()))
            .flatten()
            .unwrap_or_default()
    }
    /// Children of `parent` (`None` = top level).
    pub fn children(&self, parent: Option<TreeNodeId>) -> Vec<TreeNodeId> {
        tree_do(self.id(), |t| {
            t.children_of(parent.map(|p| p.0))
                .iter()
                .map(|c| TreeNodeId(*c))
                .collect()
        })
        .unwrap_or_default()
    }
    /// A node's parent (`None` for a top-level or unknown node).
    pub fn parent(&self, node: TreeNodeId) -> Option<TreeNodeId> {
        tree_do(self.id(), |t| t.nodes.get(&node.0).and_then(|n| n.parent))
            .flatten()
            .map(TreeNodeId)
    }
    /// Whether the node exists.
    pub fn contains(&self, node: TreeNodeId) -> bool {
        tree_do(self.id(), |t| t.nodes.contains_key(&node.0)).unwrap_or(false)
    }
    /// Total number of nodes.
    pub fn len(&self) -> usize {
        tree_do(self.id(), |t| t.nodes.len()).unwrap_or(0)
    }
    /// Whether the tree has no nodes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Programmatic expand/collapse (no callback fires).
    pub fn set_expanded(&self, node: TreeNodeId, v: bool) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(x) = n.tree_mut().and_then(|t| t.nodes.get_mut(&node.0)) {
                x.expanded = v;
            }
        });
    }
    /// Whether the node is expanded.
    pub fn expanded(&self, node: TreeNodeId) -> bool {
        tree_do(self.id(), |t| {
            t.nodes.get(&node.0).is_some_and(|n| n.expanded)
        })
        .unwrap_or(false)
    }
    /// Expand or collapse every node.
    pub fn expand_all(&self, v: bool) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree_mut() {
                t.nodes.values_mut().for_each(|x| x.expanded = v);
            }
        });
    }
    /// Lazy loading hint: show an expander even without children; fill them in `on_expand`.
    pub fn set_has_children(&self, node: TreeNodeId, v: bool) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(x) = n.tree_mut().and_then(|t| t.nodes.get_mut(&node.0)) {
                x.has_children = v;
            }
        });
    }
    /// Select a node (its ancestors are expanded so it is visible); `None` clears. No callback fires.
    pub fn set_selected(&self, node: Option<TreeNodeId>) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree_mut() {
                match node.filter(|x| t.nodes.contains_key(&x.0)) {
                    Some(x) => {
                        t.reveal(x.0);
                        t.selected = Some(x.0);
                    }
                    None => t.selected = None,
                }
            }
        });
    }
    /// The selected node.
    pub fn selected(&self) -> Option<TreeNodeId> {
        tree_do(self.id(), |t| t.selected).flatten().map(TreeNodeId)
    }
    /// Run `f` and send the tree to the backend once at the end (fast bulk updates).
    pub fn batch(&self, f: impl FnOnce(&Tree)) {
        core::freeze(self.id(), true);
        let _g = Thaw(self.id());
        f(self)
    }
    /// Run `f` with the new node when the user changes the selection (`None` when cleared).
    pub fn on_select(&self, mut f: impl FnMut(Option<TreeNodeId>) + 'static) {
        on(self.id(), Ev::TreeSelected, move |e| {
            if let Event::TreeSelected(x) = e {
                f(x.map(TreeNodeId))
            }
        })
    }
    /// Double-click / Enter on a node.
    pub fn on_activate(&self, mut f: impl FnMut(TreeNodeId) + 'static) {
        on(self.id(), Ev::TreeActivated, move |e| {
            if let Event::TreeActivated(x) = e {
                f(TreeNodeId(*x))
            }
        })
    }
    /// The user expanded (`true`) or collapsed a node. Add children here for lazy loading.
    pub fn on_expand(&self, mut f: impl FnMut(TreeNodeId, bool) + 'static) {
        on(self.id(), Ev::TreeExpanded, move |e| {
            if let Event::TreeExpanded(x, b) = e {
                f(TreeNodeId(*x), *b)
            }
        })
    }
}
