//! Widgets holding a list of strings with one selection: `ComboBox`, `ListBox`.

use super::*;

fn set_items<S: AsRef<str>>(id: WidgetId, items: &[S]) {
    let v: Vec<String> = items.iter().map(|s| s.as_ref().to_string()).collect();
    let len = v.len();
    core::set(
        id,
        true,
        |n| {
            if let Some(l) = n.list_mut() {
                l.items = v.clone();
                if l.selected.is_some_and(|i| i >= len) {
                    l.selected = None;
                }
            }
        },
        Prop::Items(&v),
    );
    set_selected(id, selected(id));
}
pub(super) fn selected(id: WidgetId) -> Option<usize> {
    core::read(id, |n| n.selection()).flatten()
}
pub(super) fn set_selected(id: WidgetId, i: Option<usize>) {
    let Some((tabs, len)) = core::read(id, |n| {
        (
            n.kind == Kind::Tabs,
            if n.kind == Kind::Tabs {
                n.children.len()
            } else {
                n.list().map_or(0, |l| l.items.len())
            },
        )
    }) else {
        return;
    };
    let valid = i.filter(|i| *i < len);
    if tabs && valid.is_none() {
        return; // a tab strip always has a selection
    }
    core::set(
        id,
        false,
        |n| {
            if let Some(s) = n.selection_mut() {
                *s = valid
            }
        },
        Prop::Selected(valid),
    );
}
pub(super) fn on_select(id: WidgetId, mut f: impl FnMut(Option<usize>) + 'static) {
    on(id, Ev::Selected, move |e| {
        if let Event::Selected(i) = e {
            f(*i)
        }
    })
}

macro_rules! item_methods {
    ($t:ident) => {
        impl $t {
            /// Replace the items; a selection past the end clears.
            pub fn set_items<S: AsRef<str>>(&self, items: &[S]) {
                set_items(self.id(), items)
            }
            /// The items.
            pub fn items(&self) -> Vec<String> {
                core::read(self.id(), |n| n.list().map(|l| l.items.clone()))
                    .flatten()
                    .unwrap_or_default()
            }
            /// Select item `i` (`None` clears; an index past the end is ignored); no callback fires.
            pub fn set_selected(&self, i: Option<usize>) {
                set_selected(self.id(), i)
            }
            /// The selected index.
            pub fn selected(&self) -> Option<usize> {
                selected(self.id())
            }
            /// The text of the selected item.
            pub fn selected_text(&self) -> Option<String> {
                core::read(self.id(), |n| {
                    let l = n.list()?;
                    l.items.get(l.selected?).cloned()
                })
                .flatten()
            }
            /// Run `f` with the new index when the user changes the selection (`None` when cleared).
            pub fn on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
                on_select(self.id(), f)
            }
        }
    };
}
item_methods!(ComboBox);
item_methods!(ListBox);

impl ComboBox {
    /// A drop-down list; fill it with `set_items`. Keep the list short: GTK 3 builds a menu item
    /// per entry, which takes seconds for thousands (a [`ListBox`] scales to hundreds of thousands).
    pub fn new(parent: impl Into<WidgetId>) -> ComboBox {
        make(ComboBox::from_id, Kind::ComboBox, parent, |_| {})
    }
}
impl ListBox {
    /// A scrolling list; fill it with `set_items`.
    pub fn new(parent: impl Into<WidgetId>) -> ListBox {
        make(ListBox::from_id, Kind::ListBox, parent, |_| {})
    }
    /// Double-click / Enter on an item.
    pub fn on_activate(&self, mut f: impl FnMut(usize) + 'static) {
        on(self.id(), Ev::Activated, move |e| {
            if let Event::Activated(i) = e {
                f(*i)
            }
        })
    }
}
