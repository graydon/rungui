//! Widgets holding a list of strings with one selection: `ComboBox`, `ListBox`.

use super::*;

fn set_items<S: AsRef<str>>(id: WidgetId, items: &[S]) {
    let v: Vec<String> = items.iter().map(|s| s.as_ref().to_string()).collect();
    let len = v.len();
    core::set(
        id,
        true,
        |n| {
            n.items = v.clone();
            if n.selected.is_some_and(|i| i >= len) {
                n.selected = None;
            }
        },
        Prop::Items(&v),
    );
    set_selected(id, selected(id));
}
pub(super) fn selected(id: WidgetId) -> Option<usize> {
    core::read(id, |n| n.selected).flatten()
}
pub(super) fn set_selected(id: WidgetId, i: Option<usize>) {
    let Some((tabs, len)) = core::read(id, |n| {
        (
            n.kind == Kind::Tabs,
            if n.kind == Kind::Tabs {
                n.children.len()
            } else {
                n.items.len()
            },
        )
    }) else {
        return;
    };
    let valid = i.filter(|i| *i < len);
    if tabs && valid.is_none() {
        return; // a tab strip always has a selection
    }
    core::set(id, false, |n| n.selected = valid, Prop::Selected(valid));
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
            pub fn set_items<S: AsRef<str>>(&self, items: &[S]) {
                set_items(self.id(), items)
            }
            pub fn items(&self) -> Vec<String> {
                core::read(self.id(), |n| n.items.clone()).unwrap_or_default()
            }
            pub fn set_selected(&self, i: Option<usize>) {
                set_selected(self.id(), i)
            }
            pub fn selected(&self) -> Option<usize> {
                selected(self.id())
            }
            pub fn selected_text(&self) -> Option<String> {
                core::read(self.id(), |n| {
                    n.selected.and_then(|i| n.items.get(i).cloned())
                })
                .flatten()
            }
            pub fn on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
                on_select(self.id(), f)
            }
        }
    };
}
item_methods!(ComboBox);
item_methods!(ListBox);

impl ComboBox {
    pub fn new(parent: impl Into<WidgetId>) -> ComboBox {
        make(ComboBox::from_id, Kind::ComboBox, parent, |_| {})
    }
}
impl ListBox {
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
