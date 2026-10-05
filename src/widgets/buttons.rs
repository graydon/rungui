//! Buttons, check boxes and radio buttons.

use super::*;

impl Button {
    /// A push button with this caption (`&` marks a mnemonic, `&&` is a literal ampersand).
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> Button {
        make(Button::from_id, Kind::Button, parent, |n| {
            n.text = text.to_string()
        })
    }
    /// Change the caption.
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    /// The caption.
    pub fn text(&self) -> String {
        text_of(self.id())
    }
    /// Run `f` when the button is clicked.
    pub fn on_click(&self, mut f: impl FnMut() + 'static) {
        on(self.id(), Ev::Click, move |_| f())
    }
}

pub(super) fn set_checked(id: WidgetId, v: bool) {
    core::set(
        id,
        false,
        |n| {
            if let Some(c) = n.checked_mut() {
                *c = v
            }
        },
        Prop::Checked(v),
    );
}
pub(super) fn checked(id: WidgetId) -> bool {
    core::read(id, |n| n.checked()).unwrap_or(false)
}
pub(super) fn on_toggle(id: WidgetId, mut f: impl FnMut(bool) + 'static) {
    on(id, Ev::Toggled, move |e| {
        if let Event::Toggled(b) = e {
            f(*b)
        }
    })
}

impl CheckBox {
    /// A check box with this caption.
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> CheckBox {
        make(CheckBox::from_id, Kind::CheckBox, parent, |n| {
            n.text = text.to_string()
        })
    }
    /// Change the caption.
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    /// Set the check mark (no callback fires).
    pub fn set_checked(&self, v: bool) {
        set_checked(self.id(), v)
    }
    /// Whether the box is checked.
    pub fn checked(&self) -> bool {
        checked(self.id())
    }
    /// Run `f` with the new state when the user toggles the box.
    pub fn on_toggle(&self, f: impl FnMut(bool) + 'static) {
        on_toggle(self.id(), f)
    }
}

/// Identifies a set of mutually exclusive radio buttons (they may live anywhere in the window).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RadioGroup(u32);

impl RadioGroup {
    /// A new group, distinct from every other.
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
    /// A radio button of `group`; checking it unchecks the others of the group.
    pub fn new(parent: impl Into<WidgetId>, group: &RadioGroup, text: &str) -> RadioButton {
        make(RadioButton::from_id, Kind::RadioButton, parent, |n| {
            n.text = text.to_string();
            if let Some(c) = n.check_mut() {
                c.group = group.0;
            }
        })
    }
    /// Change the caption.
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
            let g = r.nodes.get(&self.id())?.check()?.group;
            Some(r.checked_in_group(g, self.id()))
        })
        .flatten()
        .unwrap_or_default();
        for o in others {
            set_checked(o, false);
        }
    }
    /// Whether this button is the group's selected one.
    pub fn checked(&self) -> bool {
        checked(self.id())
    }
    /// Run `f` with the new state when the user checks the button.
    pub fn on_toggle(&self, f: impl FnMut(bool) + 'static) {
        on_toggle(self.id(), f)
    }
}
