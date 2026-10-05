# rungui design

rungui is a small toolkit: a Rust-side widget graph plus a thin per-platform backend that mirrors
that graph into native widgets (GTK3 / Win32 / AppKit). Target size is FLTK/libui/IUP, not Qt.

## Layers

```
user code ──► widgets/     Copy handles (Button, Label, ...) deref to Widget; closures for events
              core/        registry: HashMap<WidgetId, Node>, mirrored state, event dispatch,
                           post queue, timers, dirty tracking
              layout.rs    stack/grid layout in Rust from backend preferred sizes
              a11y.rs      accessible name/description/role of every widget, derived from the registry
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
* Table and tree models are debounced the same way: a mutation updates the core's copy at once
  and marks the model dirty; the next loop turn sends it to the backend (one transfer however many
  rows were pushed). `batch` still holds the transfer back until the closure ends.
* Wake-ups are coalesced in `core::wake`: after one `Backend::wake`, further ones are dropped until
  the loop has answered (`drain_posted`), so a burst of mutations queues one native wake-up.

## Limits that keep the core total

* Widgets nest at most `MAX_NESTING` (128) deep: layout, accessibility and the toolkits recurse
  over the tree. `create` fails with `Error::LimitExceeded` past it.
* Sizes, spacing and coordinates are clamped (`types::MAX_PX` = 32767, windows `MAX_WINDOW_PX` =
  16384) and layout arithmetic saturates, so no input overflows or trips a toolkit limit; tables
  keep at most `MAX_COLUMNS` columns; images must satisfy `ImageData::is_valid`.
* Numbers reaching a backend are finite and inside their range (`set_value`, `set_range`); a
  selection index the backend reports that is out of range never becomes state.
* `Prop::applies_to(kind)` filters properties by widget kind in the core: a method called on a
  handle of the wrong kind (`MenuItem::from_id(combo).set_accel(..)`) never reaches a backend, which
  may therefore assume each property is meant for its widget. The mock backend asserts it.

## Layout decision

Rust-side layout. Backends only provide `preferred_size(id)` (and `chrome(id)` for GroupBox/Tabs)
and apply absolute `Prop::Bounds`. HBox/VBox/Grid/Spacer are virtual (never reach the backend); real widgets
parent to the nearest *native* container (Window, Page, GroupBox), which is a dumb absolute container
(GtkFixed / child HWND / flipped NSView). Why: identical on three platforms, ~250 lines, testable
with the mock, no per-toolkit layout semantics to reconcile. Cost: no native baseline alignment or
height-for-width; accepted.

Natural sizes are measured once per layout pass (a cache keyed by widget, so arranging a container
does not re-measure its subtree: O(nodes), not O(nodes x depth)).

Model: natural size along the stack axis + share of spare space by `expand` weight; cross axis follows
`Align` (default Fill); hidden widgets take no space; never shrinks below natural size. Window,
Page, GroupBox behave as vertical stacks with padding 10. Grid auto-flows `cols` columns or uses
`set_cell` for explicit placement and spans. Coordinates are logical pixels (backend scales for DPI).
Windows without an explicit `set_size` follow their content's natural size.

## Accessibility

Every backend wraps native controls, and every platform already exposes those to assistive
technology (ATK, NSAccessibility, oleacc/MSAA), so rungui does not build an accessibility tree of its
own. What the native control cannot know is what the app wants it called. `a11y::resolve(window)`
walks the window once, in layout order, and returns the name, description and role of each native
widget that AT can currently see (virtual boxes flattened, hidden subtrees and unselected tabs
omitted). Names: a button/checkbox/radio/group/page/window/menu item uses its own text (mnemonic
markers stripped); an unnamed input, list, slider, table or tree is labelled by the nearest preceding
Label; `set_a11y_name` overrides either. Description: `set_a11y_description`, else the tooltip.
Role: `set_a11y_role(A11yRole)`, else the kind's default. `A11yRole` is a small rungui enum of roles
every backend can express.

Backends copy the parts that matter onto the native controls in `a11y_changed` (debounced, once per
dirty window): GTK sets the ATK name, description and role; Cocoa sets `accessibilityLabel` and
`accessibilityHelp` (no roles); Win32 uses `IAccPropServices` MSAA annotations, and only on the HWNDs
where oleacc's stock proxy is inadequate (explicit app overrides, tooltips, the sash,
Page/GroupBox containers). Win32 deliberately does not push names that are just the widget's own
text or the preceding Label: oleacc derives those itself. There is no AT-action path: assistive
technology operates the native controls, which report ordinary user events to the app.

## Unicode

Public API is UTF-8 `&str`/`String`. Backends convert at the edge (UTF-16 for Win32, NSString for
Cocoa, C strings for GTK). Paths use `PathBuf` publicly and `String` (lossy) at the backend boundary.

## Public API

The crate root exports the widget handles, `App`, the plain data types and `A11yRole`/`A11yProps`
and nothing else: the registry (`core`), the backend contract (`Backend`, `Prop`, `Kind`, `Event`,
`SashKey`) and the specs it passes (`MessageSpec`, `FileSpec`, `Accel`, `TreeRow`) are crate-private,
and become public together with `rungui::backend::mock` only under the `mock` feature (which is
for testing an application's UI headlessly). Enums a platform could extend (`Error`,
`NativeHandle`, `MessageKind`, `Buttons`, `Answer`, `A11yRole`) are `#[non_exhaustive]`.

## Backend selection

`build.rs` sets exactly one of `rungui_gtk`, `rungui_win32`, `rungui_cocoa` (+ `rungui_gnustep`). Feature
`mock` (or `cfg(test)`) swaps in `backend::mock::Mock`. Everything else is shared code. Build modes: see doc/BUILDING.md
and `scripts/check-all.sh`.

## Adding a widget

1. `backend/mod.rs`: add a `Kind` variant (and `Prop`/`Event` variants if needed; both are `#[non_exhaustive]`, additive).
   Decide `is_native`, `in_layout`, `is_layout_container`.
2. `core/lifecycle.rs`: if it is a container, extend `accepts`. If it has state beyond the common `Node` fields, add (or reuse)
   a `NodeData` variant in `core/model.rs` (`NodeData::for_kind` picks it, plus a typed accessor on `Node`) and push its
   initial value in `sync_initial`.
3. `widgets/`: add the name to the `handle!` list in `mod.rs`, then put the impl in the file for its family, a `new(parent, ..)` using `make(..)`, setters via
   `core::set(id, relayout, |n| mirror, Prop::X)`, and an `on_*` using `on(id, Ev::X, ..)`.
4. `a11y.rs`: add the default `A11yRole` (and a naming rule if it takes its name from its text or the preceding Label).
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
out, it is just not draggable. A11y: the sash has the `Splitter` role (Win32 also exposes the
position as its value). It is keyboard-operable on every backend: a click or Tab focuses it
(with a visible focus indicator), the arrow keys along its axis move it by 10 px (Shift: 50 px) and
Home/End jump to the pane limits. Backends only translate keys into `Event::SashKey`; the core
applies the step, the clamping, the RTL mirroring and `on_move`, exactly as for a drag.

## Known limits

Design-level non-goals: no custom drawing/canvas, multi-monitor API, virtualised (million-row)
tables/trees, or radio-group keyboard navigation beyond what the toolkit offers. Current gaps and
progress are tracked in `doc/STATUS.md`.
