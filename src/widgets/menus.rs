//! Menu bars, menus, items and popup menus.

use super::*;

impl MenuBar {
    /// The menu bar of `window` (a window has one).
    pub fn new(window: impl Into<WidgetId>) -> MenuBar {
        make(MenuBar::from_id, Kind::MenuBar, window, |_| {})
    }
}
impl Menu {
    /// `parent` is a `MenuBar` (top-level menu) or a `Menu` (submenu).
    pub fn new(parent: impl Into<WidgetId>, title: &str) -> Menu {
        make(Menu::from_id, Kind::Menu, parent, |n| {
            n.text = title.to_string()
        })
    }
}
impl MenuItem {
    /// An item of `menu` (a [`Menu`] or [`PopupMenu`]).
    pub fn new(menu: impl Into<WidgetId>, text: &str) -> MenuItem {
        make(MenuItem::from_id, Kind::MenuItem, menu, |n| {
            n.text = text.to_string()
        })
    }
    /// Run `f` when the item is chosen.
    pub fn on_click(&self, mut f: impl FnMut() + 'static) {
        on(self.id(), Ev::Click, move |_| f())
    }
    /// e.g. "Ctrl+S" (Ctrl is Command on macOS), "F5", "Alt+Enter".
    pub fn set_accel(&self, a: &str) {
        core::set(
            self.id(),
            false,
            |n| {
                if let Some(m) = n.menu_item_mut() {
                    m.accel = a.to_string()
                }
            },
            Prop::Accel(a),
        );
    }
    /// Change the item's text.
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, false)
    }
}
impl CheckMenuItem {
    /// A check item of `menu`.
    pub fn new(menu: impl Into<WidgetId>, text: &str) -> CheckMenuItem {
        make(CheckMenuItem::from_id, Kind::CheckMenuItem, menu, |n| {
            n.text = text.to_string()
        })
    }
    /// Set the check mark (no callback fires).
    pub fn set_checked(&self, v: bool) {
        set_checked(self.id(), v)
    }
    /// Whether the item is checked.
    pub fn checked(&self) -> bool {
        checked(self.id())
    }
    /// Run `f` with the new state when the user toggles the item.
    pub fn on_toggle(&self, f: impl FnMut(bool) + 'static) {
        on_toggle(self.id(), f)
    }
    /// Set the keyboard accelerator, e.g. "Ctrl+Shift+S" (empty removes it).
    pub fn set_accel(&self, a: &str) {
        core::set(
            self.id(),
            false,
            |n| {
                if let Some(m) = n.menu_item_mut() {
                    m.accel = a.to_string()
                }
            },
            Prop::Accel(a),
        );
    }
}
impl MenuSeparator {
    /// A separator line in `menu`.
    pub fn new(menu: impl Into<WidgetId>) -> MenuSeparator {
        make(MenuSeparator::from_id, Kind::MenuSeparator, menu, |_| {})
    }
}

impl PopupMenu {
    /// A new, empty popup menu. It has no parent: destroy it yourself when done.
    pub fn new() -> PopupMenu {
        PopupMenu::from_id(core::create(Kind::PopupMenu, None, |_| {}))
    }
    /// Pop up over `window` at window-client coordinates (blocks until dismissed).
    pub fn show_at(&self, window: impl Into<WidgetId>, x: i32, y: i32) {
        core::popup_menu(self.id(), Some(window.into()), Some((x, y)))
    }
    /// Pop up at the pointer / focused widget (blocks until dismissed).
    pub fn show(&self, window: impl Into<WidgetId>) {
        core::popup_menu(self.id(), Some(window.into()), None)
    }
}
impl Default for PopupMenu {
    fn default() -> Self {
        Self::new()
    }
}
