//! Public widget handles. Every handle is a tiny `Copy` wrapper around a [`WidgetId`]; the real
//! state lives in the registry (see core.rs). Methods on stale/dead handles do nothing and getters
//! return defaults, so nothing here can panic. All handles deref to [`Widget`], which carries the
//! operations common to every widget (enable/visible/layout hints/accessibility/native handle).

use crate::a11y::{A11yProps, A11yRole};
use crate::backend::{Backend, Event, Kind, Native as B, Prop};
use crate::core::{self, Ev};
use crate::types::*;
use std::ops::Deref;
use std::path::PathBuf;

/// Untyped handle with the operations every widget has.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Widget(pub WidgetId);

macro_rules! handle {
    ($($(#[$m:meta])* $name:ident),* $(,)?) => {$(
        $(#[$m])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
        pub struct $name(Widget);
        impl Deref for $name {
            type Target = Widget;
            fn deref(&self) -> &Widget { &self.0 }
        }
        impl From<$name> for WidgetId {
            fn from(h: $name) -> WidgetId { (h.0).0 }
        }
        impl From<&$name> for WidgetId {
            fn from(h: &$name) -> WidgetId { (h.0).0 }
        }
        impl $name {
            /// Wrap a raw id (e.g. one carried across threads). No checks: stale ids are inert.
            pub fn from_id(id: WidgetId) -> Self { $name(Widget(id)) }
        }
    )*};
}

handle! {
    /// Top-level window. Children added with other widgets' `new(&window, ..)`.
    Window,
    Label, Button, CheckBox, RadioButton, TextInput, TextArea, ComboBox, ListBox, Slider,
    ProgressBar, SpinBox, Tabs,
    /// One tab of a [`Tabs`]; a container like a window.
    Page,
    /// Titled frame container.
    GroupBox,
    Image,
    /// Horizontal stack (virtual: no native widget).
    HBox,
    /// Vertical stack (virtual).
    VBox,
    /// Grid with `cols` auto-flow columns (virtual).
    Grid,
    /// Flexible empty space (expand = 1 by default).
    Spacer,
    MenuBar, Menu, MenuItem, CheckMenuItem, MenuSeparator,
    /// Multi-column table of strings (see [`Table`] methods).
    Table,
    /// Hierarchical tree of text nodes (see [`Tree`] methods).
    Tree,
    /// Top-level context menu; build it with `MenuItem::new(&popup, ..)` etc.
    PopupMenu,
    /// Two panes with a draggable sash between them (see [`Splitter`] methods; virtual: the
    /// panes and the sash are native, the splitter itself is not).
    Splitter,
}

impl From<Widget> for WidgetId {
    fn from(w: Widget) -> WidgetId {
        w.0
    }
}

fn make<T>(
    wrap: fn(WidgetId) -> T,
    kind: Kind,
    parent: impl Into<WidgetId>,
    setup: impl FnOnce(&mut core::Node),
) -> T {
    wrap(core::create(kind, Some(parent.into()), setup))
}

fn text_of(id: WidgetId) -> String {
    core::read(id, |n| n.text.clone()).unwrap_or_default()
}
fn set_text(id: WidgetId, t: &str, relayout: bool) {
    core::set(id, relayout, |n| n.text = t.to_string(), Prop::Text(t));
}
fn on(id: WidgetId, ev: Ev, mut f: impl FnMut(&Event) + 'static) {
    core::set_callback(id, ev, Box::new(move |e| f(e)));
}

// ------------------------------------------------------------------ common

/// Largest pixel value accepted for sizes, spacing, padding and splitter positions; larger
/// (or negative, where nonsensical) inputs are clamped so layout arithmetic cannot overflow.
const MAX_PX: i32 = 1 << 16;
/// Largest grid cell index / span (keeps the track tables small).
const MAX_CELL: usize = 1 << 10;

fn px(v: i32) -> i32 {
    v.clamp(0, MAX_PX)
}

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

// ------------------------------------------------------------------ window

impl Window {
    pub fn new(title: &str) -> Window {
        Window::from_id(core::create(Kind::Window, None, |n| {
            n.text = title.to_string()
        }))
    }
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, false)
    }
    pub fn title(&self) -> String {
        text_of(self.id())
    }
    /// Client-area size in logical pixels. Without this call the window sizes itself to its content.
    pub fn set_size(&self, w: i32, h: i32) {
        if core::update(self.id(), true, |n| {
            n.client = Size::new(w.clamp(1, MAX_PX), h.clamp(1, MAX_PX));
            n.explicit_size = true;
        })
        .is_some()
        {
            core::layout_window(self.id());
        }
    }
    pub fn size(&self) -> (i32, i32) {
        core::read(self.id(), |n| (n.client.w, n.client.h)).unwrap_or((0, 0))
    }
    pub fn set_resizable(&self, v: bool) {
        core::set(self.id(), false, |n| n.resizable = v, Prop::Resizable(v));
    }
    pub fn show(&self) {
        self.set_visible(true)
    }
    pub fn hide(&self) {
        self.set_visible(false)
    }
    /// Close handler: return `true` to allow closing (default), `false` to veto.
    pub fn on_close(&self, f: impl FnMut() -> bool + 'static) {
        core::update(self.id(), false, |n| n.on_close = Some(Box::new(f)));
    }
    /// Called with the new client size when the user resizes the window.
    pub fn on_resize(&self, mut f: impl FnMut(i32, i32) + 'static) {
        on(self.id(), Ev::Resized, move |e| {
            if let Event::Resized { w, h } = e {
                f(*w, *h)
            }
        });
    }
    /// Move the window's outer frame to screen position (x, y) in logical pixels. Some platforms
    /// (Wayland, some window managers) ignore this.
    pub fn set_position(&self, x: i32, y: i32) {
        core::set(
            self.id(),
            false,
            |n| n.position = Some((x, y)),
            Prop::Position { x, y },
        );
    }
    /// The last position set with [`Window::set_position`] or reported by the platform after the
    /// user moved the window; `None` if neither happened (the window manager placed it).
    pub fn position(&self) -> Option<(i32, i32)> {
        core::read(self.id(), |n| n.position).flatten()
    }
    /// Called with the new screen position when the user moves the window (not on every
    /// platform; see [`Window::position`]).
    pub fn on_move(&self, mut f: impl FnMut(i32, i32) + 'static) {
        on(self.id(), Ev::Moved, move |e| {
            if let Event::Moved { x, y } = e {
                f(*x, *y)
            }
        });
    }
    /// Close now (no `on_close` check).
    pub fn close(&self) {
        self.destroy()
    }
}

// ------------------------------------------------------------------ simple widgets

impl Label {
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> Label {
        make(Label::from_id, Kind::Label, parent, |n| {
            n.text = text.to_string()
        })
    }
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    pub fn text(&self) -> String {
        text_of(self.id())
    }
}

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

fn set_checked(id: WidgetId, v: bool) {
    core::set(id, false, |n| n.checked = v, Prop::Checked(v));
}
fn checked(id: WidgetId) -> bool {
    core::read(id, |n| n.checked).unwrap_or(false)
}
fn on_toggle(id: WidgetId, mut f: impl FnMut(bool) + 'static) {
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

// ------------------------------------------------------------------ text

macro_rules! text_methods {
    ($t:ident) => {
        impl $t {
            pub fn set_text(&self, t: &str) {
                set_text(self.id(), t, false)
            }
            pub fn text(&self) -> String {
                text_of(self.id())
            }
            /// Called with the new full text after each user edit.
            pub fn on_change(&self, mut f: impl FnMut(&str) + 'static) {
                on(self.id(), Ev::Text, move |e| {
                    if let Event::Text(s) = e {
                        f(s)
                    }
                })
            }
            pub fn set_read_only(&self, v: bool) {
                core::set(self.id(), false, |n| n.readonly = v, Prop::ReadOnly(v));
            }
            /// Show the text in a fixed-pitch font (code, logs, hex dumps). Default off; ignored
            /// by backends that cannot change the font.
            pub fn set_monospace(&self, v: bool) {
                core::set(self.id(), true, |n| n.monospace = v, Prop::Monospace(v));
            }
            pub fn monospace(&self) -> bool {
                core::read(self.id(), |n| n.monospace).unwrap_or(false)
            }
        }
    };
}
text_methods!(TextInput);
text_methods!(TextArea);

impl TextInput {
    pub fn new(parent: impl Into<WidgetId>) -> TextInput {
        make(TextInput::from_id, Kind::TextInput, parent, |_| {})
    }
    /// Single-line input that masks its contents.
    pub fn password(parent: impl Into<WidgetId>) -> TextInput {
        make(TextInput::from_id, Kind::PasswordInput, parent, |_| {})
    }
    pub fn set_placeholder(&self, t: &str) {
        core::set(
            self.id(),
            false,
            |n| n.placeholder = t.to_string(),
            Prop::Placeholder(t),
        );
    }
}

impl TextArea {
    pub fn new(parent: impl Into<WidgetId>) -> TextArea {
        make(TextArea::from_id, Kind::TextArea, parent, |_| {})
    }
    /// Soft-wrap long lines (default `true`); `false` scrolls horizontally instead.
    pub fn set_wrap(&self, v: bool) {
        core::set(self.id(), false, |n| n.wrap = v, Prop::Wrap(v));
    }
    pub fn wrap(&self) -> bool {
        core::read(self.id(), |n| n.wrap).unwrap_or(false)
    }
}

// ------------------------------------------------------------------ item widgets

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
fn selected(id: WidgetId) -> Option<usize> {
    core::read(id, |n| n.selected).flatten()
}
fn set_selected(id: WidgetId, i: Option<usize>) {
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
fn on_select(id: WidgetId, mut f: impl FnMut(Option<usize>) + 'static) {
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

// ------------------------------------------------------------------ numeric

fn set_value(id: WidgetId, v: f64) {
    let mut vv = v;
    core::set(
        id,
        false,
        |n| {
            vv = if v.is_nan() {
                n.range.0
            } else {
                v.clamp(n.range.0, n.range.1.max(n.range.0))
            };
            n.value = vv;
        },
        Prop::Value(v),
    );
    // re-push the clamped value (the closure ran before `Prop::Value(v)` was consumed)
    core::set(id, false, |_| {}, Prop::Value(vv));
}
/// Normalise a (min, max, step) request: finite, `min <= max`, `step > 0` (NaN / infinities
/// would make `f64::clamp` panic later).
fn sane_range(min: f64, max: f64, step: f64) -> (f64, f64, f64) {
    let min = if min.is_finite() { min } else { 0.0 };
    let max = if max.is_finite() {
        max.max(min)
    } else {
        min.max(min + 100.0)
    };
    (
        min,
        max,
        if step.is_finite() && step > 0.0 {
            step
        } else {
            1.0
        },
    )
}

fn set_range(id: WidgetId, min: f64, max: f64, step: f64) {
    let (min, max, step) = sane_range(min, max, step);
    core::set(
        id,
        false,
        |n| {
            n.range = (min, max, step);
            n.value = n.value.clamp(min, max);
        },
        Prop::Range { min, max, step },
    );
    let v = core::read(id, |n| n.value).unwrap_or(min);
    core::set(id, false, |_| {}, Prop::Value(v));
}
fn value(id: WidgetId) -> f64 {
    core::read(id, |n| n.value).unwrap_or(0.0)
}
fn on_value(id: WidgetId, mut f: impl FnMut(f64) + 'static) {
    on(id, Ev::Value, move |e| {
        if let Event::Value(v) = e {
            f(*v)
        }
    })
}

macro_rules! value_methods {
    ($t:ident) => {
        impl $t {
            pub fn set_value(&self, v: f64) {
                set_value(self.id(), v)
            }
            pub fn value(&self) -> f64 {
                value(self.id())
            }
            pub fn set_range(&self, min: f64, max: f64, step: f64) {
                set_range(self.id(), min, max, step)
            }
            pub fn on_change(&self, f: impl FnMut(f64) + 'static) {
                on_value(self.id(), f)
            }
        }
    };
}
value_methods!(Slider);
value_methods!(SpinBox);

impl Slider {
    pub fn new(parent: impl Into<WidgetId>, min: f64, max: f64) -> Slider {
        let s = make(Slider::from_id, Kind::Slider, parent, |n| {
            n.range = sane_range(min, max, 1.0)
        });
        s.set_value(min);
        s
    }
}
impl SpinBox {
    pub fn new(parent: impl Into<WidgetId>, min: f64, max: f64, step: f64) -> SpinBox {
        let s = make(SpinBox::from_id, Kind::SpinBox, parent, |n| {
            n.range = sane_range(min, max, step)
        });
        s.set_value(min);
        s
    }
}

impl ProgressBar {
    pub fn new(parent: impl Into<WidgetId>) -> ProgressBar {
        make(ProgressBar::from_id, Kind::ProgressBar, parent, |_| {})
    }
    /// 0.0..=1.0 (clamped).
    pub fn set_fraction(&self, f: f64) {
        set_value(self.id(), f)
    }
    pub fn fraction(&self) -> f64 {
        value(self.id())
    }
    pub fn set_indeterminate(&self, v: bool) {
        core::set(
            self.id(),
            false,
            |n| n.indeterminate = v,
            Prop::Indeterminate(v),
        );
    }
}

// ------------------------------------------------------------------ containers

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
/// A two-pane container: create exactly two children with the splitter as parent (the first is
/// the left/top pane, under RTL the right pane for `Horizontal`); a third child is refused like
/// any invalid parent. Panes fill the splitter's cross axis; the first pane gets
/// [`Splitter::position`] pixels along the main axis, the sash `set_spacing` pixels (default 6)
/// and the second pane the rest. Hiding a pane gives the whole area to the other one. The user
/// drags the sash to move the split; on backends without a native sash the split is fixed.
impl Splitter {
    pub fn new(parent: impl Into<WidgetId>, orientation: Orientation) -> Splitter {
        let s = make(Splitter::from_id, Kind::Splitter, parent, |n| {
            n.lay.padding = 0;
            n.lay.expand = 1.0;
            if let Some(sp) = n.split.as_mut() {
                sp.orient = orientation;
            }
        });
        if s.is_alive() {
            core::create_sash(s.id(), orientation);
        }
        s
    }
    pub fn orientation(&self) -> Orientation {
        core::read(self.id(), |n| n.split.as_ref().map(|s| s.orient))
            .flatten()
            .unwrap_or_default()
    }
    /// Size of the first pane along the main axis, in pixels. Remembered as requested and clamped
    /// to the current size (and the pane minimums) at every layout, so shrinking and re-growing
    /// the window restores it. Does not call `on_move`.
    pub fn set_position(&self, px: i32) {
        core::split_set(self.id(), self::px(px), false);
    }
    /// The effective first-pane size from the last layout (before the first layout: the requested
    /// position, or 0 if none). Without `set_position` the space is split evenly.
    pub fn position(&self) -> i32 {
        core::read(self.id(), |n| {
            n.split.as_ref().map_or(0, |s| {
                if s.laid_out {
                    s.actual
                } else {
                    s.pos.unwrap_or(0)
                }
            })
        })
        .unwrap_or(0)
    }
    /// Minimum main-axis sizes of the first and second pane (default 0, 0). Positions are clamped
    /// so both fit; if the splitter is too small for both, the first pane wins.
    pub fn set_min_pane_sizes(&self, first: i32, second: i32) {
        core::update(self.id(), true, |n| {
            if let Some(s) = n.split.as_mut() {
                s.min = (self::px(first), self::px(second));
            }
        });
    }
    /// Called with the new position after the USER moved the sash (drag, keyboard, AT).
    pub fn on_move(&self, mut f: impl FnMut(i32) + 'static) {
        on(self.id(), Ev::SashMoved, move |e| {
            if let Event::SashDragged(p) = e {
                f(*p)
            }
        });
    }
}

impl Spacer {
    pub fn new(parent: impl Into<WidgetId>) -> Spacer {
        make(Spacer::from_id, Kind::Spacer, parent, |n| {
            n.lay.expand = 1.0
        })
    }
}

impl Image {
    pub fn new(parent: impl Into<WidgetId>) -> Image {
        make(Image::from_id, Kind::Image, parent, |_| {})
    }
    pub fn set_image(&self, img: Option<&ImageData>) {
        core::set(
            self.id(),
            true,
            |n| n.image = img.cloned(),
            Prop::Image(img),
        );
    }
}

// ------------------------------------------------------------------ menus

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
            |n| n.accel = a.to_string(),
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
            |n| n.accel = a.to_string(),
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

// ------------------------------------------------------------------ table

/// Calls `core::freeze(id, false)` on drop so a panicking batch closure cannot leave it frozen.
struct Thaw(WidgetId);
impl Drop for Thaw {
    fn drop(&mut self) {
        core::freeze(self.0, false)
    }
}

fn cells<S: AsRef<str>>(c: &[S]) -> Vec<String> {
    c.iter().map(|s| s.as_ref().to_string()).collect()
}

impl Table {
    pub fn new(parent: impl Into<WidgetId>) -> Table {
        make(Table::from_id, Kind::Table, parent, |_| {})
    }
    /// Replace the columns (rows and selection are kept).
    pub fn set_columns(&self, cols: &[Column]) {
        let v = cols.to_vec();
        core::data_update(self.id(), core::Data::TableAll, |n| {
            if let Some(t) = n.table.as_mut() {
                t.columns = v;
                if t.sort.is_some_and(|(c, _)| c >= t.columns.len()) {
                    t.sort = None;
                }
            }
        });
    }
    pub fn add_column(&self, col: Column) {
        core::data_update(self.id(), core::Data::TableAll, |n| {
            if let Some(t) = n.table.as_mut() {
                t.columns.push(col);
            }
        });
    }
    pub fn columns(&self) -> Vec<Column> {
        core::read(self.id(), |n| n.table.as_ref().map(|t| t.columns.clone()))
            .flatten()
            .unwrap_or_default()
    }
    /// Replace all rows. The selection is cleared if it is now out of range.
    pub fn set_rows<S: AsRef<str>>(&self, rows: &[Vec<S>]) {
        let v: Vec<Vec<String>> = rows.iter().map(|r| cells(r)).collect();
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table.as_mut() {
                t.rows = v;
                if n.selected.is_some_and(|i| i >= t.rows.len()) {
                    n.selected = None;
                }
            }
        });
    }
    pub fn push_row<S: AsRef<str>>(&self, row: &[S]) {
        let v = cells(row);
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table.as_mut() {
                t.rows.push(v);
            }
        });
    }
    /// Insert before `index` (clamped); the selection follows its row.
    pub fn insert_row<S: AsRef<str>>(&self, index: usize, row: &[S]) {
        let v = cells(row);
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table.as_mut() {
                let i = index.min(t.rows.len());
                t.rows.insert(i, v);
                if let Some(s) = n.selected.as_mut() {
                    if *s >= i {
                        *s += 1;
                    }
                }
            }
        });
    }
    /// Remove a row (ignored if out of range); the selection follows its row or clears.
    pub fn remove_row(&self, index: usize) {
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table.as_mut() {
                if index < t.rows.len() {
                    t.rows.remove(index);
                    n.selected = match n.selected {
                        Some(s) if s == index => None,
                        Some(s) if s > index => Some(s - 1),
                        o => o,
                    };
                }
            }
        });
    }
    pub fn clear(&self) {
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(t) = n.table.as_mut() {
                t.rows.clear();
            }
            n.selected = None;
        });
    }
    /// Set one cell (the row is padded with empty cells if needed; out-of-range rows are ignored).
    /// Note: this shadows the grid-placement `Widget::set_cell`, reachable as `(*table).set_cell(..)`.
    pub fn set_cell(&self, row: usize, col: usize, text: &str) {
        core::data_update(self.id(), core::Data::TableRows, |n| {
            if let Some(r) = n.table.as_mut().and_then(|t| t.rows.get_mut(row)) {
                if col >= MAX_CELL * 64 {
                    return; // absurd column index: would allocate gigabytes
                }
                if r.len() <= col {
                    r.resize(col + 1, String::new());
                }
                r[col] = text.to_string();
            }
        });
    }
    pub fn cell(&self, row: usize, col: usize) -> String {
        core::read(self.id(), |n| {
            n.table.as_ref()?.rows.get(row)?.get(col).cloned()
        })
        .flatten()
        .unwrap_or_default()
    }
    pub fn row(&self, row: usize) -> Vec<String> {
        core::read(self.id(), |n| n.table.as_ref()?.rows.get(row).cloned())
            .flatten()
            .unwrap_or_default()
    }
    pub fn rows(&self) -> Vec<Vec<String>> {
        core::read(self.id(), |n| n.table.as_ref().map(|t| t.rows.clone()))
            .flatten()
            .unwrap_or_default()
    }
    pub fn row_count(&self) -> usize {
        core::read(self.id(), |n| n.table.as_ref().map_or(0, |t| t.rows.len())).unwrap_or(0)
    }
    /// Run `f` and send the table to the backend once at the end (fast bulk updates).
    pub fn batch(&self, f: impl FnOnce(&Table)) {
        core::freeze(self.id(), true);
        let _g = Thaw(self.id());
        f(self)
    }
    /// Select a row (out of range = clear). No callback fires.
    pub fn set_selected(&self, row: Option<usize>) {
        core::data_update(self.id(), core::Data::TableSelected, |n| {
            let len = n.table.as_ref().map_or(0, |t| t.rows.len());
            n.selected = row.filter(|i| *i < len);
        });
    }
    pub fn selected(&self) -> Option<usize> {
        selected(self.id())
    }
    /// The selected row's cells.
    pub fn selected_row(&self) -> Option<Vec<String>> {
        core::read(self.id(), |n| {
            n.table.as_ref()?.rows.get(n.selected?).cloned()
        })
        .flatten()
    }
    /// Show the sort arrow on `(column, ascending)`. Display only: the app reorders the rows itself.
    pub fn set_sort_indicator(&self, s: Option<(usize, bool)>) {
        core::data_update(self.id(), core::Data::TableSort, |n| {
            if let Some(t) = n.table.as_mut() {
                t.sort = s.filter(|(c, _)| *c < t.columns.len());
            }
        });
    }
    pub fn sort_indicator(&self) -> Option<(usize, bool)> {
        core::read(self.id(), |n| n.table.as_ref().and_then(|t| t.sort)).flatten()
    }
    pub fn on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
        on_select(self.id(), f)
    }
    /// Double-click / Enter on a row.
    pub fn on_activate(&self, mut f: impl FnMut(usize) + 'static) {
        on(self.id(), Ev::Activated, move |e| {
            if let Event::Activated(i) = e {
                f(*i)
            }
        })
    }
    /// A column header was clicked (typically: sort the rows and call `set_sort_indicator`).
    pub fn on_column_click(&self, mut f: impl FnMut(usize) + 'static) {
        on(self.id(), Ev::ColumnClicked, move |e| {
            if let Event::ColumnClicked(c) = e {
                f(*c)
            }
        })
    }
}

// ------------------------------------------------------------------ tree

fn tree_do<R>(id: WidgetId, f: impl FnOnce(&core::TreeData) -> R) -> Option<R> {
    core::read(id, |n| n.tree.as_ref().map(|t| f(t))).flatten()
}

impl Tree {
    pub fn new(parent: impl Into<WidgetId>) -> Tree {
        make(Tree::from_id, Kind::Tree, parent, |_| {})
    }
    /// Append a node under `parent` (`None` = top level). Returns `TreeNodeId(0)` for an unknown parent.
    pub fn add(&self, parent: Option<TreeNodeId>, text: &str) -> TreeNodeId {
        self.insert(parent, usize::MAX, text)
    }
    /// Insert at child position `index` (clamped).
    pub fn insert(&self, parent: Option<TreeNodeId>, index: usize, text: &str) -> TreeNodeId {
        let mut out = 0;
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree.as_mut() {
                out = t.insert(parent.map(|p| p.0), index, text);
            }
        });
        TreeNodeId(out)
    }
    /// Remove a node and its subtree (the selection clears if it was inside).
    pub fn remove(&self, node: TreeNodeId) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree.as_mut() {
                t.remove(node.0);
            }
        });
    }
    pub fn clear(&self) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree.as_mut() {
                *t = Default::default();
            }
        });
    }
    pub fn set_text(&self, node: TreeNodeId, text: &str) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(x) = n.tree.as_mut().and_then(|t| t.nodes.get_mut(&node.0)) {
                x.text = text.to_string();
            }
        });
    }
    pub fn text(&self, node: TreeNodeId) -> String {
        tree_do(self.id(), |t| t.nodes.get(&node.0).map(|n| n.text.clone()))
            .flatten()
            .unwrap_or_default()
    }
    /// Children of `parent` (`None` = top level).
    pub fn children(&self, parent: Option<TreeNodeId>) -> Vec<TreeNodeId> {
        tree_do(self.id(), |t| {
            t.children_of(parent.map(|p| p.0))
                .iter()
                .map(|c| TreeNodeId(*c))
                .collect()
        })
        .unwrap_or_default()
    }
    pub fn parent(&self, node: TreeNodeId) -> Option<TreeNodeId> {
        tree_do(self.id(), |t| t.nodes.get(&node.0).and_then(|n| n.parent))
            .flatten()
            .map(TreeNodeId)
    }
    pub fn contains(&self, node: TreeNodeId) -> bool {
        tree_do(self.id(), |t| t.nodes.contains_key(&node.0)).unwrap_or(false)
    }
    /// Total number of nodes.
    pub fn len(&self) -> usize {
        tree_do(self.id(), |t| t.nodes.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Programmatic expand/collapse (no callback fires).
    pub fn set_expanded(&self, node: TreeNodeId, v: bool) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(x) = n.tree.as_mut().and_then(|t| t.nodes.get_mut(&node.0)) {
                x.expanded = v;
            }
        });
    }
    pub fn expanded(&self, node: TreeNodeId) -> bool {
        tree_do(self.id(), |t| {
            t.nodes.get(&node.0).is_some_and(|n| n.expanded)
        })
        .unwrap_or(false)
    }
    pub fn expand_all(&self, v: bool) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree.as_mut() {
                t.nodes.values_mut().for_each(|x| x.expanded = v);
            }
        });
    }
    /// Lazy loading hint: show an expander even without children; fill them in `on_expand`.
    pub fn set_has_children(&self, node: TreeNodeId, v: bool) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(x) = n.tree.as_mut().and_then(|t| t.nodes.get_mut(&node.0)) {
                x.has_children = v;
            }
        });
    }
    /// Select a node (its ancestors are expanded so it is visible); `None` clears. No callback fires.
    pub fn set_selected(&self, node: Option<TreeNodeId>) {
        core::data_update(self.id(), core::Data::TreeRows, |n| {
            if let Some(t) = n.tree.as_mut() {
                match node.filter(|x| t.nodes.contains_key(&x.0)) {
                    Some(x) => {
                        t.reveal(x.0);
                        t.selected = Some(x.0);
                    }
                    None => t.selected = None,
                }
            }
        });
    }
    pub fn selected(&self) -> Option<TreeNodeId> {
        tree_do(self.id(), |t| t.selected).flatten().map(TreeNodeId)
    }
    /// Run `f` and send the tree to the backend once at the end (fast bulk updates).
    pub fn batch(&self, f: impl FnOnce(&Tree)) {
        core::freeze(self.id(), true);
        let _g = Thaw(self.id());
        f(self)
    }
    pub fn on_select(&self, mut f: impl FnMut(Option<TreeNodeId>) + 'static) {
        on(self.id(), Ev::TreeSelected, move |e| {
            if let Event::TreeSelected(x) = e {
                f(x.map(TreeNodeId))
            }
        })
    }
    /// Double-click / Enter on a node.
    pub fn on_activate(&self, mut f: impl FnMut(TreeNodeId) + 'static) {
        on(self.id(), Ev::TreeActivated, move |e| {
            if let Event::TreeActivated(x) = e {
                f(TreeNodeId(*x))
            }
        })
    }
    /// The user expanded (`true`) or collapsed a node. Add children here for lazy loading.
    pub fn on_expand(&self, mut f: impl FnMut(TreeNodeId, bool) + 'static) {
        on(self.id(), Ev::TreeExpanded, move |e| {
            if let Event::TreeExpanded(x, b) = e {
                f(TreeNodeId(*x), *b)
            }
        })
    }
}

// ------------------------------------------------------------------ timers

/// A running timer; callbacks run on the main thread. Dropping the handle does NOT stop it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Timer(u64);

impl Timer {
    pub fn once(ms: u32, f: impl FnMut() + 'static) -> Timer {
        Timer::start(ms, false, f)
    }
    pub fn every(ms: u32, f: impl FnMut() + 'static) -> Timer {
        Timer::start(ms, true, f)
    }
    fn start(ms: u32, repeat: bool, f: impl FnMut() + 'static) -> Timer {
        match core::timer_start(ms, repeat, Box::new(f)) {
            Ok(t) => Timer(t),
            Err(e) => {
                core::set_error(e);
                Timer(0)
            }
        }
    }
    pub fn stop(&self) {
        core::timer_stop(self.0)
    }
}

// ------------------------------------------------------------------ dialogs

fn pid(parent: Option<Window>) -> Option<WidgetId> {
    parent.map(|w| w.id()).filter(|i| core::is_alive(*i))
}

/// Modal message box; returns which button was pressed.
pub fn message_box(
    parent: Option<Window>,
    kind: MessageKind,
    buttons: Buttons,
    title: &str,
    text: &str,
) -> Answer {
    let spec = MessageSpec {
        kind,
        buttons,
        title: title.into(),
        text: text.into(),
    };
    B::message_box(pid(parent), &spec)
}

/// Builder for open/save/folder dialogs.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    title: String,
    filters: Vec<(String, Vec<String>)>,
    dir: Option<String>,
    name: Option<String>,
}

impl FileDialog {
    pub fn new() -> FileDialog {
        FileDialog::default()
    }
    pub fn title(mut self, t: &str) -> Self {
        self.title = t.into();
        self
    }
    /// e.g. `.filter("Images", &["png", "jpg"])`.
    pub fn filter(mut self, label: &str, exts: &[&str]) -> Self {
        self.filters
            .push((label.into(), exts.iter().map(|e| e.to_string()).collect()));
        self
    }
    pub fn directory(mut self, d: &str) -> Self {
        self.dir = Some(d.into());
        self
    }
    pub fn file_name(mut self, n: &str) -> Self {
        self.name = Some(n.into());
        self
    }
    fn run(&self, parent: Option<Window>, mode: FileMode) -> Vec<PathBuf> {
        let spec = FileSpec {
            mode,
            title: self.title.clone(),
            filters: self.filters.clone(),
            initial_dir: self.dir.clone(),
            initial_name: self.name.clone(),
        };
        B::file_dialog(pid(parent), &spec)
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    pub fn open(&self, parent: Option<Window>) -> Option<PathBuf> {
        self.run(parent, FileMode::Open).into_iter().next()
    }
    pub fn open_many(&self, parent: Option<Window>) -> Vec<PathBuf> {
        self.run(parent, FileMode::OpenMany)
    }
    pub fn save(&self, parent: Option<Window>) -> Option<PathBuf> {
        self.run(parent, FileMode::Save).into_iter().next()
    }
    pub fn pick_folder(&self, parent: Option<Window>) -> Option<PathBuf> {
        self.run(parent, FileMode::PickFolder).into_iter().next()
    }
}
