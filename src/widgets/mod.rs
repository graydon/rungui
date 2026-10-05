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
        #[doc = concat!("A handle to a [`", stringify!($name), "`](crate::", stringify!($name), "); `Copy`, and inert once the widget is destroyed.")]
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

/// Largest grid cell index / span (keeps the track tables small).
const MAX_CELL: usize = 1 << 10;
/// Most columns a table keeps; backends index columns with 16 bits and nobody needs more.
pub const MAX_COLUMNS: usize = 1 << 12;

fn px(v: i32) -> i32 {
    v.clamp(0, MAX_PX)
}

mod buttons;
mod common;
mod containers;
mod dialogs;
mod display;
mod items;
mod menus;
mod numeric;
mod splitter;
mod table;
mod text_input;
mod timer;
mod tree;
mod window;

pub use buttons::*;
pub use dialogs::*;
use items::{on_select, selected, set_selected};
use table::Thaw;
pub use timer::*;
