# rungui design

rungui is a small toolkit: a Rust-side widget graph plus a thin per-platform backend that mirrors
that graph into native widgets (GTK3 / Win32 / AppKit). Target size is FLTK/libui/IUP, not Qt.

## Layers

```
user code ──► widgets.rs   Copy handles (Button, Label, ...) deref to Widget; closures for events
              core.rs      registry: HashMap<WidgetId, Node>, mirrored state, event dispatch,
                           post queue, timers, dirty tracking
              layout.rs    stack/grid layout in Rust from backend preferred sizes
              a11y.rs      accesskit TreeUpdate from the registry; AT actions -> app callbacks
              backend/     `trait Backend` (associated fns), one impl compiled in as `Native`
                mod.rs       the contract (read its module docs: it is the spec for backend authors)
                mock.rs      in-memory backend for tests/examples (feature `mock` or cargo test)
                gtk.rs win32.rs cocoa.rs   platform backends (cfgs from build.rs)
```

## Handle model

* `WidgetId(u64)`: global monotonic, never reused. Handles are `Copy + Send + Sync` newtypes.
* The registry is thread-local to the thread that called `App::new` (the UI thread). On any other
  thread it does not exist, so every handle operation is a silent no-op. The only thread-safe
  entry points are `App::post(closure)` and `App::quit()`; handles can be moved into posted closures.
* Constructors never fail loudly: on a bad parent or backend error they record the error
  (`rungui::last_error()`) and return a dead handle (`id == 0`). Every method on a dead or destroyed
  handle does nothing; getters return defaults. There are no `unwrap`s on user-reachable paths.
* Native objects are keyed by `WidgetId` inside the backend (id -> native map; native object carries the id
  back for events). No raw pointers cross the boundary, so lifetime/ownership races become
  "unknown id, ignore". `Widget::native_handle()` exposes the raw object for platform-specific code.

## Threading and re-entrancy (no panics, no deadlocks)

* Registry access only through `core::with`, a `try_borrow_mut` over short non-reentrant sections.
  A conflicting access returns `None` instead of panicking.
* Backend calls that may synchronously emit events (`create`, `set`, `destroy`) run *outside* the borrow.
  Backends must additionally not emit events for programmatic `set` (contract rule 2).
* Callbacks are `Box<dyn FnMut>` taken out of their slot, called with no borrows held, and put back
  unless the callback replaced itself (or destroyed its widget). User callbacks run inside
  `catch_unwind`, since unwinding through native frames would abort.
* Cross-thread: `App::post` pushes onto a mutex-guarded queue (poison-tolerant) and calls the
  thread-safe `Backend::wake`; the loop calls `core::drain_posted` on the UI thread.
* Layout and a11y updates are debounced: mutators mark the owning window dirty and wake the loop;
  `drain_posted`/`App::update()` run them. `show()` and resize events lay out synchronously.

## Layout decision

Rust-side layout. Backends only provide `preferred_size(id)` (and `chrome(id)` for GroupBox/Tabs)
and apply absolute `Prop::Bounds`. HBox/VBox/Grid/Spacer are virtual (never reach the backend); real widgets
parent to the nearest *native* container (Window, Page, GroupBox), which is a dumb absolute container
(GtkFixed / child HWND / flipped NSView). Why: identical on three platforms, ~250 lines, testable
with the mock, no per-toolkit layout semantics to reconcile. Cost: no native baseline alignment or
height-for-width; accepted.

Model: natural size along the stack axis + share of spare space by `expand` weight; cross axis follows
`Align` (default Fill); hidden widgets take no space; never shrinks below natural size. Window,
Page, GroupBox behave as vertical stacks with padding 10. Grid auto-flows `cols` columns or uses
`set_cell` for explicit placement and spans. Coordinates are logical pixels (backend scales for DPI).
Windows without an explicit `set_size` follow their content's natural size.

## Accessibility

`a11y::tree_for_window(window) -> TreeUpdate` builds the whole tree from the registry (virtual boxes
flattened, hidden subtrees and unselected tabs omitted, list items as option nodes, unnamed inputs
labelled by the preceding Label, passwords never exposed). `a11y::do_action(window, request)` turns
AT actions (click, set value, increment, focus) into the same native update + app callback a real
user action would cause. Overrides: `set_a11y_name/description/role`. Backends push the
core's names/descriptions/roles onto the native controls in `a11y_changed` (GTK: ATK, Cocoa:
`accessibilityLabel`, Win32: `IAccPropServices` MSAA annotations on the few HWNDs where oleacc's
stock proxy is inadequate: the sash, Page/GroupBox containers and explicit app overrides). No
backend uses an accesskit platform adapter.

## Unicode

Public API is UTF-8 `&str`/`String`. Backends convert at the edge (UTF-16 for Win32, NSString for
Cocoa, C strings for GTK). Paths use `PathBuf` publicly and `String` (lossy) at the backend boundary.

## Backend selection

`build.rs` sets exactly one of `rungui_gtk`, `rungui_win32`, `rungui_cocoa` (+ `rungui_gnustep`). Feature
`mock` (or `cfg(test)`) swaps in `backend::mock::Mock`. Everything else is shared code. Build modes: see doc/BUILDING.md
and `scripts/check-all.sh`.

## Adding a widget

1. `backend/mod.rs`: add a `Kind` variant (and `Prop`/`Event` variants if needed; both are `#[non_exhaustive]`, additive).
   Decide `is_native`, `in_layout`, `is_layout_container`.
2. `core.rs`: if it is a container, extend `accepts`; if it has initial state beyond the existing fields, add the field to `Node`
   and `sync_initial`.
3. `widgets.rs`: add the name to the `handle!` list, a `new(parent, ..)` using `make(..)`, setters via
   `core::set(id, relayout, |n| mirror, Prop::X)`, and an `on_*` using `on(id, Ev::X, ..)`.
4. `a11y.rs`: add the role mapping and properties.
5. `mock.rs`: give it a `preferred_size`; add a test. Then implement it in each real backend.

## Extending the backend contract

Only additively: new `Kind`/`Prop`/`Event` variants, new trait methods *with default bodies*.
Backends ignore unknown props (`_ =>`) and return `Unsupported` for unknown kinds.

## Tree, Table and context menus (required; stretch-hard)

These three are important enough to be in scope. They follow the existing pattern: the core owns the
data and mirrors it to the backend by *full replacement* props (like `Items`), and backends rebuild
their native widget from it. Simplicity over scale: no virtualisation in v1 (a data-source callback
mode is a later, natively-supported extension: GtkTreeModel / LVS_OWNERDATA / NSTableViewDataSource).

**Table** (`Kind::Table`; GtkTreeView+GtkListStore, SysListView32 report view, NSTableView).
* Core state: `columns: Vec<Column{title, width, align, sortable}>`, `rows: Vec<Vec<String>>`, selection
  (`Option<usize>`; `multi` optional), sort indicator `Option<(col, ascending)>`.
* Backend contract: `Prop::Columns(&[Column])`, `Prop::Rows(&[Vec<String>])`, `Prop::Selected`,
  `Prop::SortIndicator(Option<(usize,bool)>)`. Events: `Selected(Option<usize>)`, `Activated(row)`,
  `ColumnClicked(col)`. Sorting is the app's job (it reorders rows and sets the indicator): no sort logic
  in backends. Cell editing, per-cell checkboxes/icons: optional extras, skip if awkward.
* Cheap incremental API on the handle (`push_row`, `set_cell`) just mutates core state and re-sends.

**Tree** (`Kind::Tree`; GtkTreeView+GtkTreeStore, SysTreeView32, NSOutlineView). Core keeps an arena of
nodes (`TreeNodeId(u64)`, text, optional icon, children, expanded, `has_children` hint for lazy loading).
* Backend contract: `Prop::TreeRows(&[TreeRow{node: u64, depth: u32, text, expanded, has_children}])`
  (pre-order flattening; the backend rebuilds the native tree and restores expansion/selection from the
  flags), `Prop::TreeSelected(Option<u64>)`. Events: `TreeSelected(Option<u64>)`, `TreeActivated(u64)`,
  `TreeExpanded(u64, bool)` (user expand/collapse). Lazy loading: app populates children in the
  expanded callback; the core re-sends `TreeRows`. Programmatic expand/collapse/select never emit events.

**Context menus.** A popup menu is a top-level `Kind::PopupMenu` (no parent) that reuses the existing
`MenuItem`/`CheckMenuItem`/`MenuSeparator`/submenu `Menu` kinds and their `Click`/`Toggled` events.
* API: `widget.set_context_menu(&popup)` attaches it to any widget (and windows); `popup.show_at(window, x, y)`
  pops it up programmatically; `widget.on_context_menu(|x, y| ..)` lets the app rebuild/enable items
  just before display (called before the popup is shown).
* Backend contract: `Backend::popup_menu(menu, parent_window, Option<(x,y)>)` (None = at pointer/focus;
  blocks in the native nested loop until dismissed or an item fires; item events re-enter the core as
  usual, so rule 4 applies) and `Event::ContextMenu{x,y}` emitted for right-click / Menu key /
  Shift+F10 on a widget that has a context menu attached or an `on_context_menu` callback. The core then
  calls `popup_menu`. (GTK: gtk_menu_popup_at_pointer + button-press/popup-menu; Win32: WM_CONTEXTMENU +
  TrackPopupMenu; Cocoa: NSMenu popUpMenuPositioningItem / menuForEvent.)

**As implemented in the core** (details beyond the above):
* After `Prop::Columns` the core re-sends `Rows`, `Selected`, `SortIndicator`; after `Rows` it re-sends `Selected`;
  after `TreeRows` it re-sends `TreeSelected`. `Table::batch(|t| ..)` / `Tree::batch` defer pushes to one at the end.
* The core drops events that name an unknown row/column/node, so backends can be sloppy during rebuilds.
* `ContextMenu{x,y}` is in window-client coordinates and may be emitted for any widget: the core bubbles to the
  nearest ancestor with a menu or `on_context_menu`, skips disabled widgets, runs the callback, then reads the
  attached menu (the callback may attach one) and calls `popup_menu(menu, window, Some((x,y)))`.
* Tree node ids are globally unique (`TreeNodeId`); `Tree::set_selected` expands the ancestors.
* Per-platform implementation notes: doc/NATIVE_TABLE_TREE_NOTES.md.

**Accessibility:** Table -> Table/Row/Cell/ColumnHeader roles with row/column indices; Tree -> Tree/TreeItem
with level, expanded and selected states; popup menus -> Menu/MenuItem. **Mock:** implements all of it
(in-memory columns/rows/tree/popup log) with tests, ahead of the real backends.

**Splitter:** a virtual container like HBox (`Kind::Splitter`) whose two panes are placed by
layout.rs (first pane = position px, clamped to the pane minimums at every layout; the second
takes the rest; RTL mirrors it like an HBox). Between them the core creates a native
`Kind::Sash` child and pushes its bounds; the backend's sash only reports
`Event::SashDragged(leading_edge)` and the core converts that to a position, relayouts
synchronously and calls `on_move`. If the backend refuses to create a sash the split still lays
out, it is just not draggable. A11y: the sash is a `Splitter` role with a numeric value
(the position) and SetValue/Increment/Decrement actions.

## Known limits

Design-level non-goals: no custom drawing/canvas, multi-monitor API, virtualised (million-row)
tables/trees, or radio-group keyboard navigation beyond what the toolkit offers. Current gaps and
progress are tracked in `doc/STATUS.md`.
