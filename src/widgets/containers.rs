//! Layout containers: `Tabs`/`Page`, `GroupBox`, `HBox`, `VBox`, `Grid`, `Spacer`.

use super::*;

impl Tabs {
    pub fn new(parent: impl Into<WidgetId>) -> Tabs {
        make(Tabs::from_id, Kind::Tabs, parent, |_| {})
    }
    pub fn add_page(&self, title: &str) -> Page {
        make(Page::from_id, Kind::Page, self.id(), |n| {
            n.text = title.to_string()
        })
    }
    pub fn set_selected(&self, i: usize) {
        set_selected(self.id(), Some(i))
    }
    pub fn selected(&self) -> Option<usize> {
        selected(self.id())
    }
    pub fn on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
        on_select(self.id(), f)
    }
}

impl Page {
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, false)
    }
}

impl GroupBox {
    pub fn new(parent: impl Into<WidgetId>, title: &str) -> GroupBox {
        make(GroupBox::from_id, Kind::GroupBox, parent, |n| {
            n.text = title.to_string()
        })
    }
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, true)
    }
}

impl HBox {
    pub fn new(parent: impl Into<WidgetId>) -> HBox {
        make(HBox::from_id, Kind::HBox, parent, |n| n.lay.padding = 0)
    }
}
impl VBox {
    pub fn new(parent: impl Into<WidgetId>) -> VBox {
        make(VBox::from_id, Kind::VBox, parent, |n| n.lay.padding = 0)
    }
}
impl Grid {
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
    pub fn new(parent: impl Into<WidgetId>) -> Spacer {
        make(Spacer::from_id, Kind::Spacer, parent, |n| {
            n.lay.expand = 1.0
        })
    }
}
