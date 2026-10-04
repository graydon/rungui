//! Operations common to every widget (`impl Widget`).

use super::*;

impl Widget {
    pub fn id(&self) -> WidgetId {
        self.0
    }
    /// False after `destroy`, or if creation failed (see [`crate::last_error`]).
    pub fn is_alive(&self) -> bool {
        core::is_alive(self.0)
    }
    /// Destroy this widget and all its children.
    pub fn destroy(&self) {
        core::destroy(self.0)
    }
    pub fn set_enabled(&self, v: bool) {
        core::set_flag(self.0, false, v)
    }
    pub fn enabled(&self) -> bool {
        core::read(self.0, |n| n.enabled).unwrap_or(false)
    }
    /// Hidden widgets take no layout space. Hiding a box hides everything inside it.
    pub fn set_visible(&self, v: bool) {
        core::set_flag(self.0, true, v)
    }
    pub fn visible(&self) -> bool {
        core::read(self.0, |n| n.visible).unwrap_or(false)
    }
    pub fn set_tooltip(&self, t: &str) {
        core::set(
            self.0,
            false,
            |n| n.tooltip = t.to_string(),
            Prop::Tooltip(t),
        );
    }
    pub fn focus(&self) {
        core::set(self.0, false, |_| {}, Prop::Focus);
    }
    /// Share of spare space along the parent stack's axis (0 = natural size, 1 = take a share).
    pub fn set_expand(&self, weight: f32) {
        core::update(self.0, true, |n| n.lay.expand = weight.max(0.0));
    }
    /// Alignment inside the parent's cell/cross axis (default `Fill`).
    pub fn set_align(&self, a: Align) {
        core::update(self.0, true, |n| n.lay.align = a);
    }
    /// Minimum laid-out size. On a [`Window`] this is the minimum CLIENT size: the user cannot
    /// resize below it (where the platform allows) and layout never goes below it.
    pub fn set_min_size(&self, w: i32, h: i32) {
        let min = Size::new(px(w), px(h));
        if core::update(self.0, true, |n| n.lay.min = min) == Some(Kind::Window) {
            B::set(self.0, &Prop::MinSize(min));
        }
    }
    /// Force the natural size (overrides the toolkit's preferred size).
    pub fn set_fixed_size(&self, w: i32, h: i32) {
        core::update(self.0, true, |n| {
            n.lay.fixed = Some(Size::new(px(w), px(h)))
        });
    }
    /// Space between children (stacks/grids/containers).
    pub fn set_spacing(&self, px: i32) {
        core::update(self.0, true, |n| n.lay.spacing = self::px(px));
    }
    /// Inner margin of a container.
    pub fn set_padding(&self, px: i32) {
        core::update(self.0, true, |n| n.lay.padding = self::px(px));
    }
    /// Explicit grid cell in the parent [`Grid`] (otherwise children auto-flow).
    pub fn set_cell(&self, col: usize, row: usize, colspan: usize, rowspan: usize) {
        core::update(self.0, true, |n| {
            n.lay.cell = Some((
                col.min(MAX_CELL),
                row.min(MAX_CELL),
                colspan.clamp(1, MAX_CELL),
                rowspan.clamp(1, MAX_CELL),
            ))
        });
    }
    /// Last laid-out bounds, relative to the nearest native parent (logical pixels).
    pub fn bounds(&self) -> Rect {
        core::read(self.0, |n| n.bounds).unwrap_or_default()
    }
    /// Accessible name override (otherwise derived from the widget text / preceding label).
    pub fn set_a11y_name(&self, s: &str) {
        core::update(self.0, false, |n| n.a11y.name = Some(s.to_string()));
    }
    pub fn set_a11y_description(&self, s: &str) {
        core::update(self.0, false, |n| n.a11y.desc = Some(s.to_string()));
    }
    pub fn set_a11y_role(&self, r: A11yRole) {
        core::update(self.0, false, |n| n.a11y.role = Some(r));
    }
    pub fn a11y(&self) -> A11yProps {
        core::read(self.0, |n| n.a11y.clone()).unwrap_or_default()
    }
    /// Attach a [`PopupMenu`] shown on right-click / Menu key / Shift+F10 (children without their own
    /// menu inherit it). Works on any widget and on windows.
    pub fn set_context_menu(&self, popup: impl Into<WidgetId>) {
        let p = popup.into();
        core::update(self.0, false, |n| n.context_menu = Some(p));
    }
    pub fn clear_context_menu(&self) {
        core::update(self.0, false, |n| n.context_menu = None);
    }
    /// Called with window-client coordinates just before the context menu is shown (and even when no
    /// menu is attached), so the app can rebuild/enable items or attach a different menu.
    pub fn on_context_menu(&self, mut f: impl FnMut(i32, i32) + 'static) {
        on(self.0, Ev::ContextMenu, move |e| {
            if let Event::ContextMenu { x, y } = e {
                f(*x, *y)
            }
        });
    }
    /// The raw GtkWidget* / HWND / NSView* (or HMENU/NSMenu for menus) for platform-specific code.
    pub fn native_handle(&self) -> Option<NativeHandle> {
        core::native_handle(self.0)
    }
}
