//! Buttons, check boxes and radio buttons.

use super::*;

impl Button {
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> Button {
        make(Button::from_id, Kind::Button, parent, |n| {
            n.text = text.to_string()
        })
    }
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    pub fn text(&self) -> String {
        text_of(self.id())
    }
    pub fn on_click(&self, mut f: impl FnMut() + 'static) {
        on(self.id(), Ev::Click, move |_| f())
    }
}

pub(super) fn set_checked(id: WidgetId, v: bool) {
    core::set(id, false, |n| n.checked = v, Prop::Checked(v));
}
pub(super) fn checked(id: WidgetId) -> bool {
    core::read(id, |n| n.checked).unwrap_or(false)
}
pub(super) fn on_toggle(id: WidgetId, mut f: impl FnMut(bool) + 'static) {
    on(id, Ev::Toggled, move |e| {
        if let Event::Toggled(b) = e {
            f(*b)
        }
    })
}

impl CheckBox {
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> CheckBox {
        make(CheckBox::from_id, Kind::CheckBox, parent, |n| {
            n.text = text.to_string()
        })
    }
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    pub fn set_checked(&self, v: bool) {
        set_checked(self.id(), v)
    }
    pub fn checked(&self) -> bool {
        checked(self.id())
    }
    pub fn on_toggle(&self, f: impl FnMut(bool) + 'static) {
        on_toggle(self.id(), f)
    }
}

/// Identifies a set of mutually exclusive radio buttons (they may live anywhere in the window).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RadioGroup(u32);

impl RadioGroup {
    pub fn new() -> RadioGroup {
        RadioGroup(core::new_group())
    }
}
impl Default for RadioGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl RadioButton {
    pub fn new(parent: impl Into<WidgetId>, group: &RadioGroup, text: &str) -> RadioButton {
        make(RadioButton::from_id, Kind::RadioButton, parent, |n| {
            n.text = text.to_string();
            n.group = group.0;
        })
    }
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    /// Selecting a radio button deselects the others of its group (no callbacks fire).
    pub fn set_checked(&self, v: bool) {
        set_checked(self.id(), v);
        if !v {
            return;
        }
        let others = core::with(|r| {
            let g = r.nodes.get(&self.id())?.group;
            let ids: Vec<_> = r
                .nodes
                .iter()
                .filter(|(k, n)| **k != self.id() && n.group == g && n.checked)
                .map(|(k, _)| *k)
                .collect();
            Some(ids)
        })
        .flatten()
        .unwrap_or_default();
        for o in others {
            set_checked(o, false);
        }
    }
    pub fn checked(&self) -> bool {
        checked(self.id())
    }
    pub fn on_toggle(&self, f: impl FnMut(bool) + 'static) {
        on_toggle(self.id(), f)
    }
}
