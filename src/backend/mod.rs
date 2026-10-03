//! The backend contract. EXACTLY ONE backend is compiled in as `backend::Native`:
//! `mock` (feature "mock" or `cargo test`), else `gtk` / `win32` / `cocoa` per the cfgs from build.rs.
//!
//! # Division of labour
//! The core (`core.rs`, `layout.rs`, `a11y.rs`) owns the widget graph, mirrored widget state
//! (text, checked, value, items, selection...), layout and accessibility trees. A backend only
//! mirrors that graph into native objects, **keyed by [`WidgetId`]**: keep a private
//! `id -> native object` map and, in the native object, the id (g_object_set_data / GWLP_USERDATA /
//! ivar or associated object) so native callbacks can call back into [`crate::core::event`].
//!
//! # Rules every backend must follow
//! 1. Everything is called on the main (UI) thread, except [`Backend::wake`], which may be called
//!    from any thread and must make the native loop call [`crate::core::drain_posted`] soon, on the
//!    main thread (g_idle_add / PostMessage to a hidden window / dispatch_async(main)).
//! 2. Programmatic changes via [`Backend::set`] must NOT emit events (block signals / set a guard
//!    flag; on Win32 ignore EN_CHANGE etc. while inside `set`). Only user actions emit events.
//! 3. [`Backend::preferred_size`], [`Backend::chrome`], [`Backend::native_handle`] are pure queries.
//! 4. Never hold backend-internal `RefCell` borrows while calling into `crate::core::*` (events can
//!    re-enter the backend through user callbacks) and never call core while inside `create/set/destroy`.
//! 5. Never panic or unwind across FFI; report failures as `Err` / ignore unknown ids (stale ids
//!    are normal: widgets can be destroyed while native messages are in flight).
//! 6. Coordinates are logical pixels, origin top-left, relative to the *native parent's client area*.
//!    Scale for DPI internally (Win32 per-monitor DPI, Cocoa points are already logical).
//! 7. Text is UTF-8 `&str` at this boundary: convert to UTF-16 (Win32) / NSString (Cocoa) / C string (GTK).
//! 8. Unknown/unsupported `Kind`s or `Prop`s: return `Err(Unsupported)` from `create` for a kind
//!    (core records the error and the widget is dead) and silently ignore unsupported props
//!    (additive evolution: new `Prop`/`Kind`/`Event` variants may be added; use `_ =>` arms).
//!
//! # Layout decision
//! Layout lives in Rust (`layout.rs`): stack (H/V) and grid layouts computed from
//! `preferred_size` and then pushed with `Prop::Bounds`. HBox/VBox/Grid/Spacer are *virtual*
//! (no native object, never passed to the backend); real widgets are children of the nearest
//! native ancestor (Window, Page, GroupBox), which is a plain absolute-positioning container
//! (GtkFixed / child HWND / flipped NSView). This is identical on all three platforms and needs
//! no toolkit layout machinery.
//!
//! # Containers a backend must provide
//! * `Window`: top-level; children are placed in its client area *below the menu bar*.
//!   `Prop::Bounds` on a window sets the CLIENT size (x/y ignored). `Event::Resized` reports the same.
//! * `GroupBox`: titled frame; children coordinates are relative to its inner client area.
//!   `chrome()` = outer size minus client size (border + title).
//! * `Tabs` + `Page`: `create(page, Some(tabs))` appends a tab; `Prop::Text` on the page is the
//!   title; children of the page use page-client coordinates. `chrome(tabs)` = header/border size.
//!   `Prop::Bounds` on a Page is advisory (toolkits that size pages themselves may ignore it).
//!   `Prop::Selected(Some(i))` on Tabs switches page; user switching emits `Event::Selected`.
//! * Menus: `MenuBar`(parent = window) > `Menu`(title in `Prop::Text`; parent = MenuBar or Menu =
//!   submenu) > `MenuItem` / `CheckMenuItem` / `MenuSeparator`. Created in order. `Prop::Accel`
//!   carries text like "Ctrl+S" (see [`crate::Accel::parse`]); the backend registers the shortcut
//!   (Ctrl means Command on macOS) and shows it in the item. Activation -> `Event::Click`
//!   (CheckMenuItem: `Event::Toggled`).
//!
//! # Splitters
//! `Splitter` is virtual like `HBox`: the core places its two panes (ordinary children,
//! parented to the nearest native ancestor) and, between them, a native `Kind::Sash` child it
//! creates itself. The sash is a thin, plain, focusable-if-convenient drag handle (a GtkEventBox
//! / custom child HWND / NSView subclass): it gets `Prop::Orientation` once, then the usual
//! `Bounds`/`Visible`/`Enabled`, shows a resize cursor and emits `Event::SashDragged(pos)` while
//! dragged (see there). It needs no painting beyond the platform's usual separator look.
//! A backend without a sash just returns `Err(Unsupported)` from `create(Kind::Sash)`: the core
//! keeps the splitter working (panes laid out at the default/app-set position, not draggable).
//!
//! # Radio buttons
//! `RadioButton` is just a toggle with radio appearance; exclusivity within a group is enforced
//! by the core (it sends `Prop::Checked(false)` to the others), so backends need no native grouping.
//!
//! # Event emission summary (backend -> core::event)
//! Button/MenuItem: `Click` | CheckBox/Radio/CheckMenuItem: `Toggled(bool)` | TextInput/TextArea/
//! PasswordInput/SpinBox-text: `Text(String)` (SpinBox uses `Value(f64)`) | Slider/SpinBox:
//! `Value(f64)` | ComboBox/ListBox/Tabs: `Selected(Option<usize>)` | ListBox double-click/Enter:
//! `Activated(usize)` | Window: `Resized{w,h}` and `core::close_requested(id)` (ALWAYS veto the native
//! close: the core destroys the window itself via `destroy` if the app allows it) | any focusable:
//! `Focus(bool)` | Table: `Selected(Option<usize>)` (row), `Activated(row)`, `ColumnClicked(col)` |
//! Tree: `TreeSelected`, `TreeActivated`, `TreeExpanded` | Sash: `SashDragged(pos)` | Window (optional):
//! `Moved{x,y}` | any widget: `ContextMenu{x,y}` | timers: `core::timer_fired(token)`.

use crate::types::*;

#[cfg(any(feature = "mock", test))]
pub mod mock;
#[cfg(any(feature = "mock", test))]
pub use mock::Mock as Native;

#[cfg(all(rungui_gtk, not(any(feature = "mock", test))))]
pub mod gtk;
#[cfg(all(rungui_gtk, not(any(feature = "mock", test))))]
pub use gtk::Gtk as Native;

#[cfg(all(rungui_win32, not(any(feature = "mock", test))))]
pub mod win32;
#[cfg(all(rungui_win32, not(any(feature = "mock", test))))]
pub use win32::Win32 as Native;

#[cfg(all(rungui_cocoa, not(any(feature = "mock", test))))]
pub mod cocoa;
#[cfg(all(rungui_cocoa, not(any(feature = "mock", test))))]
pub use cocoa::Cocoa as Native;

/// Widget kinds the backend creates. Virtual layout kinds (HBox, VBox, Grid, Spacer) exist only in the core.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Kind {
    Window,
    Label,
    Button,
    CheckBox,
    RadioButton,
    TextInput,
    PasswordInput,
    TextArea,
    ComboBox,
    ListBox,
    Slider,
    ProgressBar,
    SpinBox,
    Tabs,
    Page,
    GroupBox,
    Image,
    MenuBar,
    Menu,
    MenuItem,
    CheckMenuItem,
    MenuSeparator,
    /// Multi-column report list (GtkTreeView+ListStore / SysListView32 / NSTableView).
    Table,
    /// Hierarchical list (GtkTreeView+TreeStore / SysTreeView32 / NSOutlineView).
    Tree,
    /// Top-level (parentless) context menu. Children: `MenuItem`/`CheckMenuItem`/`MenuSeparator`/`Menu`
    /// (submenu), created in order exactly as for a `Menu` under a `MenuBar`.
    PopupMenu,
    /// Drag handle between the two panes of a [`crate::Splitter`] (see "Splitters" in the module
    /// docs). Created by the core, never by the app. Optional: `create` may return `Unsupported`.
    Sash,
    // ---- virtual (core only) ----
    HBox,
    VBox,
    Grid,
    Spacer,
    /// Two-pane container; the core places both panes and the `Sash`.
    Splitter,
}

impl Kind {
    /// Has a native object (is passed to the backend).
    pub fn is_native(self) -> bool {
        !matches!(self, Kind::HBox | Kind::VBox | Kind::Grid | Kind::Spacer | Kind::Splitter)
    }
    /// Participates in layout as a widget (not a menu).
    pub fn in_layout(self) -> bool {
        !matches!(
            self,
            Kind::MenuBar | Kind::Menu | Kind::MenuItem
                | Kind::CheckMenuItem
                | Kind::MenuSeparator
                | Kind::PopupMenu
        )
    }
    /// Can hold layout children (stacks, grids, window, page, group box).
    pub fn is_layout_container(self) -> bool {
        matches!(
            self,
            Kind::Window | Kind::Page | Kind::GroupBox | Kind::HBox | Kind::VBox | Kind::Grid | Kind::Splitter
        )
    }
}

/// Property changes pushed to the backend. Initial values are pushed right after `create`.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Prop<'a> {
    /// Label/button/checkbox/menu text, window title, group title, tab title (on Page), text input
    /// contents, combo-box edit text. Never contains mnemonics processing; pass through verbatim.
    Text(&'a str),
    Tooltip(&'a str),
    Placeholder(&'a str),
    Enabled(bool),
    Visible(bool),
    /// Check state of CheckBox/RadioButton/CheckMenuItem.
    Checked(bool),
    /// Slider/SpinBox value; ProgressBar fraction in 0.0..=1.0.
    Value(f64),
    Range { min: f64, max: f64, step: f64 },
    /// Full replacement of ComboBox/ListBox items.
    Items(&'a [String]),
    /// Selected item (ComboBox/ListBox) or tab index (Tabs).
    Selected(Option<usize>),
    /// Position + size relative to the native parent's client area (Window: client size only).
    Bounds(Rect),
    Image(Option<&'a ImageData>),
    Accel(&'a str),
    ReadOnly(bool),
    /// ProgressBar busy/indeterminate mode.
    Indeterminate(bool),
    Resizable(bool),
    /// Request keyboard focus.
    Focus,
    /// Table: full replacement of the columns (headers). The core re-sends `Rows`, `Selected` and
    /// `SortIndicator` right after, so rebuilding the native columns may drop the row data.
    Columns(&'a [Column]),
    /// Table: full replacement of the rows. Missing cells are empty, extra cells are ignored.
    /// Always followed by `Selected` (native rebuilds clear the selection).
    Rows(&'a [Vec<String>]),
    /// Table: sort arrow on (column, ascending); `None` clears it. Display only: never sort in the backend.
    SortIndicator(Option<(usize, bool)>),
    /// Tree: full replacement, pre-order flattening of ALL nodes (also those below collapsed nodes).
    /// Rebuild the native tree and apply each row's `expanded` flag (expand parents before children,
    /// after all rows are inserted). Always followed by `TreeSelected`.
    TreeRows(&'a [TreeRow]),
    /// Tree: select node (`TreeRow.node`) or clear; its ancestors are already flagged expanded.
    TreeSelected(Option<u64>),
    /// Sash: which way the panes are split, sent once right after `create` (before any `Bounds`).
    /// `Horizontal` = panes side by side, so the sash is a vertical bar dragged along x (use a
    /// left-right resize cursor); `Vertical` = stacked panes, sash dragged along y.
    Orientation(Orientation),
    /// TextArea/TextInput/PasswordInput: use a fixed-pitch font (`false` = the default UI font).
    /// Optional: ignore if unsupported. Changes the preferred size, so the core relayouts after it.
    Monospace(bool),
    /// TextArea: soft-wrap long lines at word boundaries (`true`, the default and the state a
    /// fresh TextArea must start in) or scroll horizontally (`false`). Optional: ignore if unsupported.
    Wrap(bool),
    /// Window: move the OUTER frame's top-left to screen position (x, y), logical pixels, origin
    /// at the top-left of the primary screen. Optional (Wayland and some WMs refuse): no-op if unsupported.
    Position { x: i32, y: i32 },
    /// Window: the smallest CLIENT size the user may resize to (`Size::default()` = no limit).
    /// Optional; the core additionally never lays a window out smaller than this.
    MinSize(Size),
}

/// Events from the backend (user actions only; see rule 2 in the module docs).
#[derive(Clone, PartialEq, Debug)]
#[non_exhaustive]
pub enum Event {
    Click,
    Text(String),
    Toggled(bool),
    Selected(Option<usize>),
    Value(f64),
    Activated(usize),
    Resized { w: i32, h: i32 },
    Focus(bool),
    /// Table: a column header was clicked (user wants to sort by it).
    ColumnClicked(usize),
    /// Tree: user selection changed (node id from `TreeRow.node`, `None` = cleared).
    TreeSelected(Option<u64>),
    /// Tree: double-click / Enter on a node.
    TreeActivated(u64),
    /// Tree: the USER expanded (`true`) or collapsed a node. Emit after the native state changed.
    TreeExpanded(u64, bool),
    /// Right-click / Menu key / Shift+F10 on any native widget (or window). `x`,`y` are logical
    /// pixels in the CLIENT coordinates of the widget's window; for keyboard invocation use the
    /// focused item's/widget's position. The core finds the nearest widget with a context menu or
    /// `on_context_menu` callback (the widget itself, else its ancestors), runs the callback and then
    /// calls [`Backend::popup_menu`]. Emit it for every right-click: the core ignores the ones nobody wants.
    ContextMenu { x: i32, y: i32 },
    /// Sash: the user dragged it (or moved it with the keyboard) so that its leading edge (left
    /// edge for a horizontal splitter, top edge for a vertical one) should be at `pos`, in the
    /// same native-parent client coordinates as the sash's `Prop::Bounds`. Easiest: remember
    /// the sash's bounds and the pointer at button-press and emit `bounds.x + (pointer.x -
    /// press.x)` (`y` for vertical) on every motion while the button is held. Do NOT move the
    /// sash yourself: the core clamps, relayouts both panes and pushes the sash's new `Bounds`
    /// synchronously from inside `core::event`. Positions out of range are fine (they are clamped).
    SashDragged(i32),
    /// Window: the user moved the window; outer frame top-left in screen coordinates (as in
    /// `Prop::Position`). Optional; mirrored into `Window::position`.
    Moved { x: i32, y: i32 },
}

/// The platform backend. All functions are associated (no `self`): the backend keeps its state in
/// its own thread-locals/statics. Required methods have no default; optional ones may stay no-ops.
pub trait Backend {
    // ---- lifecycle & loop ----
    /// Initialise the toolkit on the calling (main) thread. Called once from `App::new`.
    fn init(app_name: &str) -> Result<()>;
    /// Run the native event loop until [`Backend::quit`]. Must call `core::drain_posted()` when woken.
    fn run();
    /// Stop the native loop started by `run` (main thread; `App::quit` routes through the post queue).
    fn quit();
    /// THREAD-SAFE: schedule `core::drain_posted()` on the main thread.
    fn wake();
    /// Start a timer; on expiry call `core::timer_fired(token)`. One-shot unless `repeat`.
    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()>;
    fn timer_stop(token: u64);

    // ---- widgets ----
    /// Create the native object. `parent` is the nearest *native* ancestor (None for windows);
    /// children are appended in creation order. Created hidden-until-shown windows; other
    /// widgets are visible by default.
    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()>;
    /// Destroy the native object. The core calls this deepest-first for every native node of a
    /// destroyed subtree, so children are already gone. Drop all id mappings. Unknown id = no-op.
    fn destroy(id: WidgetId);
    fn set(id: WidgetId, prop: &Prop);
    /// Natural size of a leaf widget given its current text/items/etc. For containers: ignored.
    fn preferred_size(id: WidgetId) -> Size;
    /// For GroupBox/Tabs: outer size minus client size (what must be added around children).
    fn chrome(_id: WidgetId) -> Size {
        Size::default()
    }
    fn native_handle(id: WidgetId) -> Option<NativeHandle>;

    // ---- modal dialogs (run a nested loop, return the result; callbacks may re-enter core) ----
    fn message_box(parent: Option<WidgetId>, spec: &MessageSpec) -> Answer;
    /// Selected paths (UTF-8, lossy if necessary); empty = cancelled.
    fn file_dialog(parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String>;

    // ---- context menus ----
    /// Pop up the `PopupMenu` `menu` over `parent_window` (None if unknown) at window-client `at`
    /// (None = at pointer / focus). BLOCKS in the native nested loop until dismissed or an item fires;
    /// item events re-enter the core as usual (rule 4 applies). Default: no-op.
    fn popup_menu(_menu: WidgetId, _parent_window: Option<WidgetId>, _at: Option<(i32, i32)>) {}

    // ---- accessibility (optional until a platform adapter is wired) ----
    /// Called when a window is created: install an accesskit platform adapter for it, if the backend
    /// uses one. The adapter's activation handler returns `crate::a11y::tree_for_window(id)` and its
    /// action handler calls `crate::a11y::do_action(id, request)`. Backends whose native controls are
    /// already accessible (GTK, Cocoa, Win32) leave this empty.
    fn a11y_attach(_window: WidgetId) {}
    /// The window's tree changed (debounced, main thread). Push it through the adapter's
    /// `update_if_active`, or copy the core's names/descriptions/roles onto the native controls.
    fn a11y_changed(_window: WidgetId) {}
}
