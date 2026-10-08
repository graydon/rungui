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
                l.sel.truncate_to(len);
            }
        },
        Prop::Items(&v),
    );
    push_selection(id);
}
/// Send the node's selection to the backend (the whole set in multi-select mode).
fn push_selection(id: WidgetId) {
    let Some((multi, first, all)) =
        core::read(id, |n| n.sel().map(|s| (s.multi, s.first(), s.items.clone()))).flatten()
    else {
        return;
    };
    if multi {
        B::set(id, &Prop::Selection(&all));
    } else {
        B::set(id, &Prop::Selected(first));
    }
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
    if tabs {
        core::set(id, false, |n| n.set_selection(valid), Prop::Selected(valid));
    } else {
        core::update(id, false, |n| n.set_selection(valid));
        send_selection(id);
    }
}

/// Make the backend show the node's selection: a table's goes with its model, a list's now.
fn send_selection(id: WidgetId) {
    if core::read(id, |n| n.kind == Kind::Table) == Some(true) {
        core::push(id, core::Data::TableSelected);
    } else {
        push_selection(id);
    }
}
pub(super) fn set_multi_select(id: WidgetId, v: bool) {
    core::set(
        id,
        false,
        |n| {
            if let Some(s) = n.sel_mut() {
                s.set_multi(v)
            }
        },
        Prop::MultiSelect(v),
    );
    send_selection(id);
}
pub(super) fn multi_select(id: WidgetId) -> bool {
    core::read(id, |n| n.sel().is_some_and(|s| s.multi)).unwrap_or(false)
}
pub(super) fn selection(id: WidgetId) -> Vec<usize> {
    core::read(id, |n| n.sel().map(|s| s.items.clone()))
        .flatten()
        .unwrap_or_default()
}
pub(super) fn set_selection(id: WidgetId, rows: &[usize]) {
    core::update(id, false, |n| {
        let len = n.selectable_len();
        if let Some(s) = n.sel_mut() {
            s.set_many(rows, len)
        }
    });
    send_selection(id);
}
pub(super) fn on_selection(id: WidgetId, mut f: impl FnMut(&[usize]) + 'static) {
    on(id, Ev::Selection, move |e| {
        if let Event::Selection(v) = e {
            f(v)
        }
    })
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
                    l.items.get(l.sel.first()?).cloned()
                })
                .flatten()
            }
            /// Run `f` with the new index when the user changes the selection (`None` when cleared).
            /// With several rows selected the index is the lowest one.
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
    /// Let the user select several items (Ctrl/Shift-click, or the platform's way). Default off.
    /// Turning it off keeps only the first selected item. No callback fires.
    pub fn set_multi_select(&self, v: bool) {
        set_multi_select(self.id(), v)
    }
    /// Whether several items can be selected.
    pub fn multi_select(&self) -> bool {
        multi_select(self.id())
    }
    /// Every selected item, ascending (at most one unless [`ListBox::set_multi_select`] is on).
    pub fn selection(&self) -> Vec<usize> {
        selection(self.id())
    }
    /// Select exactly these items; indices past the end are ignored, and without multi-select
    /// only the lowest one is kept. No callback fires.
    pub fn set_selection(&self, items: &[usize]) {
        set_selection(self.id(), items)
    }
    /// The text of every selected item, in list order.
    pub fn selected_texts(&self) -> Vec<String> {
        core::read(self.id(), |n| {
            let l = n.list()?;
            Some(
                l.sel
                    .items
                    .iter()
                    .filter_map(|i| l.items.get(*i).cloned())
                    .collect(),
            )
        })
        .flatten()
        .unwrap_or_default()
    }
    /// Run `f` with the whole selection when the user changes it.
    pub fn on_selection(&self, f: impl FnMut(&[usize]) + 'static) {
        on_selection(self.id(), f)
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
