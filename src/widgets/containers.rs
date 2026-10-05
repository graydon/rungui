//! Layout containers: `Tabs`/`Page`, `GroupBox`, `HBox`, `VBox`, `Grid`, `Spacer`.

use super::*;

impl Tabs {
    /// A tab control; add pages with [`Tabs::add_page`].
    pub fn new(parent: impl Into<WidgetId>) -> Tabs {
        make(Tabs::from_id, Kind::Tabs, parent, |_| {})
    }
    /// Append a page with this title; the first page is selected.
    pub fn add_page(&self, title: &str) -> Page {
        make(Page::from_id, Kind::Page, self.id(), |n| {
            n.text = title.to_string()
        })
    }
    /// Select page `i` (ignored when out of range); no callback fires.
    pub fn set_selected(&self, i: usize) {
        set_selected(self.id(), Some(i))
    }
    /// The selected page index.
    pub fn selected(&self) -> Option<usize> {
        selected(self.id())
    }
    /// Run `f` with the new index when the user switches tabs.
    pub fn on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
        on_select(self.id(), f)
    }
}

impl Page {
    /// Change the tab's title.
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, false)
    }
}

impl GroupBox {
    /// A titled frame around its children (which stack vertically like a window's).
    pub fn new(parent: impl Into<WidgetId>, title: &str) -> GroupBox {
        make(GroupBox::from_id, Kind::GroupBox, parent, |n| {
            n.text = title.to_string()
        })
    }
    /// Change the frame's title.
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, true)
    }
}

impl HBox {
    /// A horizontal stack with no padding.
    pub fn new(parent: impl Into<WidgetId>) -> HBox {
        make(HBox::from_id, Kind::HBox, parent, |n| n.lay.padding = 0)
    }
}
impl VBox {
    /// A vertical stack with no padding.
    pub fn new(parent: impl Into<WidgetId>) -> VBox {
        make(VBox::from_id, Kind::VBox, parent, |n| n.lay.padding = 0)
    }
}
impl Grid {
    /// A grid that auto-flows its children into `cols` columns (or place them with `set_cell`).
    pub fn new(parent: impl Into<WidgetId>, cols: usize) -> Grid {
        make(Grid::from_id, Kind::Grid, parent, |n| {
            n.lay.cols = cols.max(1)
        })
    }
    /// Place an existing child of this grid in an explicit cell.
    pub fn place(
        &self,
        child: impl Into<WidgetId>,
        col: usize,
        row: usize,
        colspan: usize,
        rowspan: usize,
    ) {
        Widget(child.into()).set_cell(col, row, colspan, rowspan)
    }
}

impl Spacer {
    /// Flexible empty space (expand weight 1).
    pub fn new(parent: impl Into<WidgetId>) -> Spacer {
        make(Spacer::from_id, Kind::Spacer, parent, |n| {
            n.lay.expand = 1.0
        })
    }
}
