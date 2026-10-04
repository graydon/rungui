//! Menu bars, menus, items and popup menus.

use super::*;

impl MenuBar {
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
    pub fn new(menu: impl Into<WidgetId>, text: &str) -> MenuItem {
        make(MenuItem::from_id, Kind::MenuItem, menu, |n| {
            n.text = text.to_string()
        })
    }
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
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, false)
    }
}
impl CheckMenuItem {
    pub fn new(menu: impl Into<WidgetId>, text: &str) -> CheckMenuItem {
        make(CheckMenuItem::from_id, Kind::CheckMenuItem, menu, |n| {
            n.text = text.to_string()
        })
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
    pub fn new(menu: impl Into<WidgetId>) -> MenuSeparator {
        make(MenuSeparator::from_id, Kind::MenuSeparator, menu, |_| {})
    }
}

impl PopupMenu {
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
