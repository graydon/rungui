# Native notes: Table, Tree, PopupMenu (for backend authors)

Contract recap (see `src/backend/mod.rs`; the mock in `src/backend/mock.rs` is the reference behaviour).

| Item | Meaning |
|---|---|
| `Kind::Table`, `Kind::Tree` | leaf widgets, laid out like a ListBox (`preferred_size`: pick ~300x150 / ~200x200 or the native default) |
| `Kind::PopupMenu` | parentless (`create(id, PopupMenu, None)`); children are `MenuItem`/`CheckMenuItem`/`MenuSeparator`/`Menu`(submenu) created in order with `parent = popup (or submenu)`, exactly like a `Menu` under a `MenuBar`. `is_native() == true`, `in_layout() == false`. Never gets `Bounds`/`Visible`. |
| `Prop::Columns(&[Column])` | replace headers. `Column{title, width (logical px), align: ColumnAlign, sortable}`. Core follows with `Rows`, `Selected`, `SortIndicator`. |
| `Prop::Rows(&[Vec<String>])` | replace all rows. Missing cells = empty, extra cells ignored. Core follows with `Selected`. |
| `Prop::Selected(Option<usize>)` | on a Table = selected row (single selection). |
| `Prop::SortIndicator(Option<(col, asc)>)` | display only. NEVER sort natively. |
| `Prop::TreeRows(&[TreeRow])` | replace everything. Pre-order list of ALL nodes (also under collapsed ones). `TreeRow{node: u64, depth, text, expanded, has_children}`. `has_children` is true for real children OR the lazy-loading hint (show an expander even though no child rows follow). Core follows with `TreeSelected`. |
| `Prop::TreeSelected(Option<u64>)` | select by node id; ancestors are already flagged `expanded` in the preceding rows. |
| `Event::Selected(Option<usize>)` (Table), `Activated(row)`, `ColumnClicked(col)` | user actions only |
| `Event::TreeSelected(Option<u64>)`, `TreeActivated(u64)`, `TreeExpanded(u64, bool)` | user actions only; emit `TreeExpanded` AFTER native state changed |
| `Event::ContextMenu{x,y}` | window-client logical pixels; emit for any widget/window on right-click, Menu key, Shift+F10 |
| `Backend::popup_menu(menu, Option<window>, Option<(x,y)>)` | blocks until dismissed; `(x,y)` window-client logical px, `None` = at pointer/focus. Default is a no-op. |

General rules that bite here:

* Rule 2: `set(Rows|TreeRows|Selected|TreeSelected|Columns)` must NOT emit `Selected`/`TreeSelected`/`TreeExpanded`
  (rebuilding a model clears selection, which natively fires "selection changed": guard it).
* Rule 4: events may re-enter `set` synchronously. In particular the app's `on_expand` callback typically adds
  children, so the core calls `set(TreeRows)` from INSIDE your `TreeExpanded` emission (lazy loading). Never emit while
  holding a `RefCell` borrow, and make rebuild robust when it happens inside a native signal/notification handler
  (see per-platform pitfalls). The core drops events naming unknown rows/nodes, so stale ids are harmless.
* Keep native scroll position and column widths across `Rows`/`TreeRows` rebuilds where cheap (the core rebuilds on every
  mutation; tiny edits otherwise scroll the view to the top). Re-applying `Columns` should only happen on `Columns`.
* Events carry model row indices / node ids, never native indices that depend on sorting. No native sorting at all.
* Row `i` of `Rows` is native row `i`. Do not enable native header-click sorting; just emit `ColumnClicked`.
* `Prop::Enabled(false)` should grey the control; `Prop::Focus` focuses it. Tooltip as for other widgets.
* Right-click on a Table/Tree should first select the row under the pointer if it is not already selected (emit the
  normal `Selected`/`TreeSelected` event), then emit `ContextMenu`, so the app's `on_context_menu` can see the selection.
* Popup items trigger the usual `Click` / `Toggled` events on the item ids; `Prop::Enabled/Text/Accel/Checked` on items
  behave as for menu-bar items. Item state set by `on_context_menu` before `popup_menu` must be reflected: do not cache
  a native menu built earlier without applying the pending props (props are applied on the native item at `set` time, so
  this is automatic as long as you keep one native menu per PopupMenu id).
* Accelerators (`Prop::Accel`) on popup items: display only (do not register global shortcuts for them).

---

## GTK3

**Table** = `GtkScrolledWindow` > `GtkTreeView` with a `GtkListStore` of N string columns (rebuilt when column count
changes). Map `Kind::Table` to the scrolled window (that is what is parented/positioned); keep the tree view in your
per-id struct. `native_handle` should return the scrolled window (document it) .

* Columns: `gtk_tree_view_column_new_with_attributes(title, gtk_cell_renderer_text_new(), "text", i, NULL)`; remove old
  ones with `gtk_tree_view_remove_column`. `set_fixed_width(width)` + `set_sizing(GTK_TREE_VIEW_COLUMN_FIXED)` (also
  `set_resizable(TRUE)`). Alignment: `xalign` property on the renderer (0.0/0.5/1.0) and
  `gtk_tree_view_column_set_alignment` for the header. Sortable: `set_clickable(TRUE)` and connect the column's
  `"clicked"` signal (carry the column index) -> `ColumnClicked`. Do NOT call `set_sort_column_id` (that sorts natively).
  Make every header clickable if you like; the core forwards regardless.
* Sort arrow: `gtk_tree_view_column_set_sort_indicator(col, TRUE)` +
  `gtk_tree_view_column_set_sort_order(col, ASCENDING/DESCENDING)`; clear the indicator on all other columns.
  These only draw the arrow; they do not sort because no sort column id is set on the model.
* Rows: block the selection `"changed"` signal (`g_signal_handler_block`) or set a guard flag, `gtk_list_store_clear`,
  then `gtk_list_store_insert_with_values`/`set` for each row (G_TYPE_STRING columns, `""` for missing cells). For big
  tables detach the model (`gtk_tree_view_set_model(NULL)`), fill, re-attach: much faster. Preserve the vertical
  adjustment value around the rebuild.
* Selection: `gtk_tree_view_get_selection`, `GTK_SELECTION_SINGLE`; `"changed"` -> read
  `gtk_tree_selection_get_selected` -> `gtk_tree_model_get_path` -> `gtk_tree_path_get_indices()[0]` -> `Selected(Some(i))`
  / `Selected(None)` when nothing is selected. Programmatic `Prop::Selected`: `gtk_tree_selection_select_path` /
  `unselect_all` under the guard, plus `gtk_tree_view_scroll_to_cell`.
* Activation: `"row-activated"(tree_view, path, column)` -> `Activated(index)` (double-click and Enter).

**Tree**: `GtkTreeView` + `GtkTreeStore` with columns {text: string, node: G_TYPE_UINT64}; a single column header hidden
with `gtk_tree_view_set_headers_visible(FALSE)`. Rebuild from `TreeRows` using a stack of `GtkTreeIter` indexed by
depth: row with depth d is appended under `iters[d-1]` (or root) with `gtk_tree_store_append` + `set`, then store the iter
at `iters[d]` (iters are valid until the store is modified by removal/reorder; appends are fine for `GtkTreeStore`).
After ALL rows are inserted, in pre-order call `gtk_tree_view_expand_row(path, FALSE)` for each `expanded` row (a
parent must be expanded before a child expand is visible; pre-order guarantees this). Block your `"row-expanded"` /
`"row-collapsed"` handlers during the whole rebuild and expansion pass, or you will emit bogus `TreeExpanded`.

* Lazy loading: rows with `has_children` but no following child row at depth+1 need a placeholder child (e.g. an empty
  dummy row marked with node = 0) so GTK draws an expander; `"test-expand-row"` fires BEFORE the row expands. Emit
  `TreeExpanded(id, true)` from `"row-expanded"` (after the state changed) and let the core re-send `TreeRows`. The
  resend happens inside the signal emission: clearing a `GtkTreeStore` while GTK is still expanding that row is
  fragile. Recommended: in the `"row-expanded"`/`"row-collapsed"` handlers do the emit from a `g_idle_add` callback
  (re-check the node still exists; use the node id, never a path/iter captured earlier), or at least do not touch the
  `GtkTreePath`/iter passed to the handler after calling `core::event`. Dummy placeholder rows must never reach the app:
  skip node 0 in selection/activation handlers and when mapping paths.
* Selection: `"changed"` -> `gtk_tree_model_get(.., COL_NODE, &id)` -> `TreeSelected(Some(id))`; nothing selected ->
  `TreeSelected(None)`. Ignore changes while the guard is set. Programmatic `TreeSelected`: find the path by node id (keep
  a `HashMap<u64, GtkTreeRowReference*>` or search; rebuild the map on `TreeRows`; free refs on rebuild/destroy), then
  `gtk_tree_view_expand_to_path` is unnecessary (already expanded) -> `gtk_tree_selection_select_path` and
  `gtk_tree_view_scroll_to_cell`.
* Activation: `"row-activated"` -> `TreeActivated(id)`. Note GTK also toggles expansion on double-click only when
  `gtk_tree_view_set_activate_on_single_click(FALSE)` (default); that is fine, but emit both events as they happen.
* Collapse of an ancestor of the selected row: GTK unselects it and emits `"changed"`; forward as `TreeSelected(None)`
  (the core mirrors this; the app sees it as a normal user change).

**Context menu / popup**

* `Kind::PopupMenu` -> `gtk_menu_new()` (floating: `g_object_ref_sink`). Children: `gtk_menu_item_new_with_label`,
  `gtk_check_menu_item_new_with_label`, `gtk_separator_menu_item_new`; submenu `Menu` = menu item + `gtk_menu_new()` as
  its `submenu`. Same code path as menu-bar menus: reuse it; only the top-level container differs. Remember `gtk_widget_show`
  on every item (GTK3 menus do not `show_all` for you). `Click` from `"activate"`, `Toggled` from `"toggled"`, with
  programmatic `Checked` guarded.
* Emitting `ContextMenu`: GtkWidgets only get `button-press-event` if the widget has an event window. For widgets
  without one (labels, boxes, images) wrap in/emit from the toplevel instead: connect `"button-press-event"` on the
  window (via `gtk_widget_add_events(GDK_BUTTON_PRESS_MASK)`) and use `gdk_event_get_coords` + hit-test your `GtkFixed`
  children (deepest child whose allocation contains the point; the id is stored in `g_object_set_data`). Simpler: connect
  `"button-press-event"` (button 3, `gdk_event_triggers_context_menu`) and `"popup-menu"` (keyboard: Menu key,
  Shift+F10; returns TRUE when handled) on every widget that has an event window (TreeView, Entry, ListBox,
  Button...) and on the window, and return TRUE only when you emitted. Compute `(x,y)` with
  `gtk_widget_translate_coordinates(widget, toplevel, ex, ey, &x, &y)`; for the `popup-menu` keyboard case use the
  widget's centre/the selected row's cell rect (`gtk_tree_view_get_cell_area`).
  `GtkTreeView` and `GtkEntry` have their own default context handling: TreeView right-click does not select
  (select the row yourself via `gtk_tree_view_get_path_at_pos`), Entry builds its own menu on `"populate-popup"`
  (if the app attached a menu, return TRUE from `button-press-event` to suppress it).
* `popup_menu`: `gtk_menu_popup_at_pointer(menu, NULL)` when `at == None` or when called from a real event; for
  `Some((x,y))` use `gtk_menu_popup_at_rect(menu, gdk_window_of(window), &GdkRect{x,y,1,1}, GDK_GRAVITY_NORTH_WEST,
  GDK_GRAVITY_NORTH_WEST, trigger_event)` where the rect is in the coordinates of `gtk_widget_get_window(toplevel)`
  (client coordinates: account for the menu bar height if your Window container sits below it, i.e. add the
  child-area offset, since the contract's coordinates are relative to the window client area BELOW the menu bar).
  `gtk_menu_popup_at_*` returns immediately: to satisfy "blocks until dismissed" run a nested loop: connect `"deactivate"`
  -> quit a `GMainLoop` you spin with `g_main_loop_run` (that also pumps `drain_posted` idles), and note `"activate"` of
  the item runs before `"deactivate"` in GTK3 (it fires from an idle after hiding), so after `g_main_loop_run` returns
  call `while g_main_context_pending { iteration }` once more or just quit the loop from an idle queued in `deactivate`.
  Without a trigger event pass `gtk_get_current_event()` (may be NULL: fine in 3.22+).
* Pitfall: set `gtk_menu_attach_to_widget(menu, parent, NULL)` or the menu has no screen/theme context; detach on destroy.
  Destroying a menu while shown: call `gtk_menu_popdown` first.

---

## Win32

**Table** = `SysListView32` (`WC_LISTVIEWW`), style `WS_CHILD|WS_VISIBLE|WS_TABSTOP|LVS_REPORT|LVS_SINGLESEL|LVS_SHOWSELALWAYS|LVS_NOSORTHEADER`
(add `LVS_EX_FULLROWSELECT|LVS_EX_DOUBLEBUFFER|LVS_EX_LABELTIP` via `LVM_SETEXTENDEDLISTVIEWSTYLE`; NO `LVS_OWNERDATA` in v1,
NO `LVS_SORTASCENDING/DESCENDING`). Needs `InitCommonControlsEx(ICC_LISTVIEW_CLASSES|ICC_TREEVIEW_CLASSES)` and a manifest
with comctl32 v6 (otherwise no visual styles and some messages differ).

* Columns: `LVM_DELETECOLUMN` repeatedly (index 0) then `LVM_INSERTCOLUMNW` with `LVCOLUMNW{mask: LVCF_TEXT|LVCF_WIDTH|
  LVCF_FMT|LVCF_SUBITEM, fmt: LVCFMT_LEFT/CENTER/RIGHT, cx: width * dpi/96, pszText, iSubItem: i}`. Known quirk: the FIRST
  column ignores `LVCFMT_RIGHT/CENTER` (it is always left-aligned); work around by inserting a zero-width dummy column
  0 and shifting the real columns by one (then ALL index mappings must subtract 1) or accept the limitation.
  Sort arrow: `LVM_GETHEADER` -> `Header_GetItem/Header_SetItem` with `HDF_SORTUP|HDF_SORTDOWN` in `fmt` (clear on the others;
  needs v6 manifest). Header click: `WM_NOTIFY` / `LVN_COLUMNCLICK` (`NMLISTVIEW.iSubItem`) -> `ColumnClicked`.
* Rows: guard flag, `WM_SETREDRAW(FALSE)`, `LVM_DELETEALLITEMS`, then per row `LVM_INSERTITEMW` (`LVIF_TEXT`, iItem=i,
  iSubItem 0, text = cell 0) and `LVM_SETITEMTEXTW` for cells 1.. (`LVITEMW.iSubItem=c`); `LVM_SETITEMCOUNT(n, 0)` first
  for speed; `WM_SETREDRAW(TRUE)` + `InvalidateRect`. Long text: `pszText` is a UTF-16 NUL-terminated buffer that must stay
  alive during the call only. Cells must not contain `\0` (truncates).
* Selection: `LVN_ITEMCHANGED` with `(uChanged & LVIF_STATE)` and `(uNewState ^ uOldState) & LVIS_SELECTED`. A single
  user click produces TWO notifications (old row deselected, new row selected) and `LVN_ITEMCHANGED` also fires during
  `LVM_DELETEALLITEMS`: ignore while guarded, and coalesce user selection changes: on notification read
  `LVM_GETNEXTITEM(-1, LVNI_SELECTED)` and emit only when it differs from the last emitted value (`-1` -> `Selected(None)`).
  Programmatic: `LVM_SETITEMSTATE(i, LVIS_SELECTED|LVIS_FOCUSED)` (mask both) under the guard, `LVM_ENSUREVISIBLE`;
  clear with `LVM_SETITEMSTATE(-1, 0, LVIS_SELECTED)`.
* Activation: `NM_DBLCLK` (use `NMITEMACTIVATE.iItem`, `-1` = ignore) and `LVN_KEYDOWN` with `VK_RETURN` (selected item).
  `NM_RETURN` is not sent by report listviews reliably.
* Notifications go to the PARENT window's WndProc as `WM_NOTIFY`; your parent container (Window/Page/GroupBox HWND)
  must forward and decode `hwndFrom -> id` (store `WidgetId` in `GWLP_USERDATA` of the listview, as for other widgets).
  Use a window-subclass (`SetWindowSubclass`) on the listview for keys/context menu if you prefer.
* DPI: column widths and row heights scale with the control's DPI (`GetDpiForWindow`); on `WM_DPICHANGED_AFTERPARENT`
  re-apply column widths.

**Tree** = `SysTreeView32`, style `WS_CHILD|WS_VISIBLE|WS_TABSTOP|TVS_HASBUTTONS|TVS_HASLINES|TVS_LINESATROOT|
TVS_SHOWSELALWAYS|TVS_DISABLEDRAGDROP` (+ `TVS_EX_DOUBLEBUFFER` via `TVM_SETEXTENDEDSTYLE`, `TVS_FULLROWSELECT` if you drop
lines). Keep `HashMap<u64, HTREEITEM>` (and the reverse via `TVITEM.lParam = node id`) rebuilt on `TreeRows`.

* Rebuild: guard, `WM_SETREDRAW(FALSE)`, `TVM_DELETEITEM(TVI_ROOT)`, then insert in pre-order with a stack of `HTREEITEM`
  by depth: `TVM_INSERTITEMW` with `TVINSERTSTRUCTW{hParent = stack[d-1] or TVI_ROOT, hInsertAfter = TVI_LAST,
  item{mask: TVIF_TEXT|TVIF_PARAM|TVIF_CHILDREN, pszText, lParam: node, cChildren: has_children ? 1 : 0}}`. `cChildren = 1`
  with no real children is exactly what draws the `[+]` for lazy nodes (use `I_CHILDRENCALLBACK` only for owner data).
  Then in pre-order for `expanded` rows with real children `TVM_EXPAND(TVE_EXPAND, item)`. Expanding a lazily flagged node
  with no children collapses it again natively (Windows removes the button when the expand finds no children), which is
  fine since the app adds children in its callback and the core re-sends rows with `expanded = true`.
  Restore scroll with `TVM_GETSCROLLPOS`-style `GetScrollInfo`/`SetScrollInfo` or by `TVM_ENSUREVISIBLE` on the selection.
* Notifications (`WM_NOTIFY`, `NMTREEVIEWW`): `TVN_SELCHANGEDW` (`itemNew.lParam` = node id; `itemNew.hItem == NULL`
  = cleared) -> `TreeSelected`. Fires during `TVM_DELETEITEM`/`TVM_SELECTITEM`: guard. Use `action` (`TVC_BYMOUSE|BYKEYBOARD`)
  to separate user from programmatic if you do not guard everything. `TVN_ITEMEXPANDEDW` (`action`: `TVE_EXPAND`/
  `TVE_COLLAPSE`, `itemNew.lParam`) -> `TreeExpanded(node, bool)` AFTER the state changed (use the `...ED` message, not
  `...ING`; `TVN_ITEMEXPANDINGW` is where you could veto). `TVE_COLLAPSE|TVE_COLLAPSERESET` also appears for the reset case:
  treat as collapse. Activation: `NM_DBLCLK` (hit-test `TVM_HITTEST` at the cursor: ignore double-clicks on the expand button
  `TVHT_ONITEMBUTTON`) and `TVN_KEYDOWN` + `VK_RETURN` -> `TreeActivated(selected node)`. Note `NM_DBLCLK` is also
  followed by the default expand/collapse toggle; both events should be emitted.
* Re-entrancy: the app's `on_expand` callback adds children while you are inside `WM_NOTIFY(TVN_ITEMEXPANDED)`; the core
  calls `TreeRows` -> your rebuild runs `TVM_DELETEITEM(TVI_ROOT)` inside the notification of an item that is deleted.
  Windows tolerates it for `...ED` but not reliably for `...ING` (use only `...ED`); after the emit never use `hItem`
  captured from the notification. Better: `PostMessage` a private `WM_APP+n` to the control's parent carrying the node id and
  emit from there. Do not emit from `TVN_ITEMEXPANDING` (the state is not yet changed).
* Selection by programmatic `TreeSelected`: `TVM_SELECTITEM(TVGN_CARET, item)` under the guard (this also scrolls).

**Context menu / popup**

* `Kind::PopupMenu` -> `CreatePopupMenu()`; items `AppendMenuW(MF_STRING|MF_CHECKED/UNCHECKED|MF_GRAYED, id, text)`,
  separators `MF_SEPARATOR`, submenus `MF_POPUP` with the submenu `HMENU` as `uIDNewItem`. Reuse the menu-bar code (`CreateMenu`
  there). The command id used for `WM_COMMAND`/`TPM_RETURNCMD` maps to a `WidgetId` via your own table (keep ids unique across
  ALL menus; allocate from a counter, never reuse the `WidgetId` value (it can exceed 16 bits)). Because items are changed
  after creation (`Enabled`, `Text`, `Checked`), use `ModifyMenuW`/`EnableMenuItem`/`CheckMenuItem` with `MF_BYCOMMAND`,
  and `DrawMenuBar` is NOT needed for popups. `&` in text is a mnemonic: the contract says text is verbatim: escape `&` as `&&`
  (the same as for the menu bar). Accelerator text: append `"\t" + accel` to the label for display.
* Emitting `ContextMenu`: handle `WM_CONTEXTMENU` (wParam = hwnd clicked, lParam = SCREEN coordinates, or `(-1,-1)` when
  triggered from the keyboard (Menu key / Shift+F10)). It is sent by `DefWindowProc` for `WM_RBUTTONUP`/keyboard, and
  bubbles to the parent if the child does not handle it, so handle it in each container/control WndProc (or via
  `SetWindowSubclass` on every native widget) and map `wParam` to a `WidgetId`. Convert screen -> window client:
  `ScreenToClient(top_level_hwnd, &pt)`, then subtract the offset of the Window's child area below the menu bar
  (`GetClientRect` of your content container vs top-level), then divide by DPI scale to logical pixels. Keyboard case:
  position from the caret/selected item: ListView `LVM_GETITEMRECT(LVIR_LABEL)` of the selected item, TreeView
  `TVM_GETITEMRECT`, otherwise the control's top-left + a small offset; convert `ClientToScreen` first, then as above.
  Return 0 after emitting; the core decides whether anyone wants it. Edit controls show their own default menu if you pass the
  message to `DefWindowProc`: call it only when the core did nothing (cannot know) -> instead let the app decide and suppress
  the native edit menu only for widgets that had `on_context_menu`/menu attached; since the contract does not tell you, the
  simple rule is: always handle `WM_CONTEXTMENU` for non-edit controls, and for edit controls pass through unless
  `popup_menu` was actually called by the core during the emit (track a thread-local "popup shown" flag).
  ListView/TreeView right-click: select the item under the cursor first (`LVM_HITTEST` / `TVM_HITTEST`), emitting the normal
  selection events, before emitting `ContextMenu`.
* `popup_menu`: `SetForegroundWindow(hwnd)` first (otherwise the menu does not dismiss on outside click), then
  `TrackPopupMenuEx(hmenu, TPM_RETURNCMD|TPM_NONOTIFY|TPM_RIGHTBUTTON|TPM_LEFTALIGN|TPM_TOPALIGN, sx, sy, hwnd, NULL)` (screen
  coords: `ClientToScreen` of the window's content area + (x,y)*dpi; `None` -> `GetCursorPos`, or for a keyboard-origin
  event the position computed above), then `PostMessage(hwnd, WM_NULL, 0, 0)`. `TPM_RETURNCMD` makes it block and return the
  chosen command id (0 = dismissed); look up the item id and emit `Click` (MenuItem) or toggle the check mark yourself then emit
  `Toggled(new)` (CheckMenuItem: Windows does not toggle checks automatically). Emit AFTER `TrackPopupMenuEx` returned, with no
  borrows held. With `TPM_NONOTIFY` you do not get `WM_COMMAND`, so nothing else fires. Owner window must be the top-level HWND
  of `parent_window` (fall back to the hidden message window / `GetForegroundWindow` when `None`).
* Pitfall: `WM_INITMENUPOPUP`/`WM_MENUSELECT` are not needed; the core already ran `on_context_menu` before calling you.
* Pitfall: destroy submenus with the owning menu: `DestroyMenu` on the top-level destroys attached submenu `HMENU`s, so do not
  double-destroy (core destroys deepest-first: destroy children's native items by `DeleteMenu`/no-op and only
  `DestroyMenu` the PopupMenu itself, ignoring errors).

---

## Cocoa (AppKit, runtime-built classes)

All classes are built with `objc2`'s `ClassBuilder`/`declare_class!` (as the rest of the backend). Store the `WidgetId` in an
ivar (or in a Rust-side `HashMap<*mut Object, WidgetId>`), never rely on `tag`.

**Table** = `NSScrollView` (hasVerticalScroller) containing an `NSTableView`; the scroll view is what you size/position
(flipped container; see the other widgets). Setup: `setUsesAlternatingRowBackgroundColors(YES)`,
`setAllowsMultipleSelection(NO)`, `setAllowsEmptySelection(YES)`, `setColumnAutoresizingStyle(NSTableViewNoColumnAutoresizing)`,
`setSelectionHighlightStyle(Regular)`. Delegate AND dataSource = ONE runtime class `RunGuiTableDelegate` implementing:
 * `numberOfRowsInTableView:` -> rows.len() (Rust state behind the id; do NOT keep a pointer to a Rust Vec that can be freed:
   look it up through the id map on every call).
 * `tableView:objectValueForTableColumn:row:` -> `NSString` of the cell (`""` if missing) (cell-based tables). Prefer this
   over view-based (`viewForTableColumn`) for v1: less code, no cell reuse bugs.
 * `tableViewSelectionDidChange:` -> `Selected(row >= 0 ? Some(row) : None)` using `selectedRow`; ignore under guard.
 * `tableView:didClickTableColumn:` -> `ColumnClicked(index of column)` (find with `tableColumns.indexOfObject`; the
   delegate method requires NO `sortDescriptorPrototype`, so no native sort). Use `setAllowsColumnSelection(NO)`.
 * double click: `setTarget(delegate)` + `setDoubleAction(@selector(runguiDoubleClick:))`; in the action read
   `clickedRow` (-1 = header/empty -> ignore) -> `Activated(row)`. Enter key: subclass `NSTableView` (runtime subclass)
   to override `keyDown:` for Return (keyCode 36/76) when a row is selected -> `Activated(selectedRow)`.
 * Columns: remove all (`removeTableColumn:` for each of `tableColumns` snapshot), then per column
   `NSTableColumn(identifier: String(i))`, `headerCell.stringValue = title`, `headerCell.alignment = left/center/right`,
   `dataCell.alignment = ...` (the data cell alignment is on `column.dataCell`, as `NSTextFieldCell`), `width = w`,
   `minWidth = 20`, `resizingMask = NSTableColumnUserResizingMask`. Identify columns by `identifier` -> index, never by
   position (users can reorder columns; call `setAllowsColumnReordering(NO)` for simplicity).
 * Sort arrow: `tableView.setIndicatorImage(NSImage(named: NSImageNameAscendingSortIndicator /
   NSImageNameDescendingSortIndicator), inTableColumn: col)` and `nil` on others; `setHighlightedTableColumn:` optional.
 * Rows: update the Rust state, then `reloadData()` under the guard (`reloadData` fires
   `tableViewSelectionDidChange:` when the selection changes). Re-apply the selection after reload
   (`selectRowIndexes:byExtendingSelection:NO` / `deselectAll:`; the core also sends `Prop::Selected`).
 * The delegate and dataSource are WEAK references on NSTableView: retain them in your per-id struct (or the callbacks
   silently vanish and the table is empty) and clear (`setDelegate(nil)`, `setDataSource(nil)`) before releasing/destroy
   (stale callbacks during dealloc are a common crash).
 * Table in `NSScrollView`: `setDocumentView:`; header visibility is automatic; `setHasHorizontalScroller(YES)` for wide
   tables.

**Tree** = `NSScrollView` + `NSOutlineView` with ONE column (`setOutlineTableColumn:`), `setHeaderView(nil)`. Class
`RunGuiOutlineDelegate` (dataSource + delegate):
 * Items MUST be objects with stable identity: use a small runtime class (or `NSNumber`) `RunGuiNode` wrapping the node id
   and keep a `HashMap<u64, Retained<RunGuiNode>>` (rebuilt on `TreeRows`, but reuse existing objects for ids that persist,
   because NSOutlineView keeps item identity for expanded state and selection, and uses pointer equality / `isEqual:`).
   Use `NSNumber numberWithUnsignedLongLong:` only if you intern them (equal NSNumbers are `isEqual:` so this works too,
   but small-number tagged pointers differ from boxed ones: always compare via `isEqual:`, i.e. do not use pointer maps).
 * dataSource: `outlineView:numberOfChildrenOfItem:` (item nil = root), `outlineView:child:ofItem:`,
   `outlineView:isItemExpandable:` (= `has_children`: true for lazy nodes with no children yet, which is what draws the
   disclosure triangle), `outlineView:objectValueForTableColumn:byItem:` (NSString text). Keep your tree structure in Rust
   keyed by id (`children: Vec<u64>`), built from the flattened `TreeRows` (depth gives parentage with a stack).
 * Rebuild: guard, `reloadData()`, then expand in pre-order: `expandItem:expandChildren:NO` for each `expanded` row
   with children (parents first). `reloadData` collapses everything and forgets selection; this is why expansion is
   restored from flags. `setAutosaveExpandedItems` must stay NO. Selection: `selectRowIndexes:` with
   `rowForItem:` (returns -1 when the item is not visible: skip). Under the guard use also
   `scrollRowToVisible:`.
 * Events: `outlineViewSelectionDidChange:` -> `TreeSelected(item id or None)` (`selectedRow`, `itemAtRow:`);
   `outlineViewItemDidExpand:` / `outlineViewItemDidCollapse:` (notification `userInfo[@"NSObject"]` = the item) ->
   `TreeExpanded(id, true/false)`. They also fire for programmatic `expandItem:`: guard them.
   Use the `...DidExpand` notifications (after), not `outlineViewItemWillExpand`. Lazy loading: emit `TreeExpanded` from
   `outlineViewItemDidExpand`; the core re-sends `TreeRows` (re-entrancy: you will call `reloadData` + re-expansion from
   inside the delegate callback which AppKit is not happy with. Defer it: `dispatch_async(main)` the emit, or defer the
   rebuild, and look the node up by id again). Double click: `setDoubleAction:` + `clickedRow` (-1 ignore) ->
   `TreeActivated(id)`; Return key via a runtime `NSOutlineView` subclass `keyDown:`.
 * Same weak-delegate pitfall as the table: retain the delegate object yourself and nil it before dealloc.
   Disclosure triangle clicks do not trigger the double-action.

**Context menu / popup**

* `Kind::PopupMenu` -> `NSMenu` (`setAutoenablesItems(NO)` so `Prop::Enabled` is honoured; the same applies to the menu bar).
  Items: `NSMenuItem(title, action: @selector(runguiMenuClick:), keyEquivalent: "")` with `target = a shared RunGuiMenuTarget`
  object (retained globally) and `representedObject`/`tag` = menu item id (use an `NSNumber` for the u64 or keep a
  Rust map keyed by the `NSMenuItem` pointer; `tag` is `NSInteger` = 64-bit, fine for ids but prefer the map);
  separators `NSMenuItem.separatorItem()`; submenus: item with `setSubmenu:` (a nested `NSMenu`). Same code as the menu
  bar items. `CheckMenuItem` = toggle `setState:` (`NSControlStateValueOn/Off`) yourself in the action, then emit
  `Toggled(new)`. Key equivalents on popup items are display only (set `keyEquivalent` + modifier mask for the label but they would
  also match globally: skip them for popup menus, or accept it).
* Emitting `ContextMenu`: NSView has `menuForEvent:`; since views are your own runtime subclasses (the flipped
  container etc.), implement `-menuForEvent:` on EVERY native widget class you subclass (NSTableView/NSOutlineView
  subclasses too) to emit `ContextMenu{x,y}` and return `nil` (the core pops the menu itself via `popup_menu`). Convert
  `event.locationInWindow` -> window content coordinates: `contentView.convert(p, from: nil)`; flipped views: the
  y coordinate is already top-left; if the content view is not flipped use `bounds.height - p.y`. Standard controls you do not
  subclass (NSButton, NSTextField...) need either a subclass or an `NSEvent.addLocalMonitorForEvents(
  matching: [.rightMouseDown, .leftMouseDown with ctrl])`: with a local monitor, hit-test with `contentView.hitTest:`
  and walk `superview` to the first view with a registered id; return `nil` from the monitor block to consume it. Ctrl+click is
  a right-click on macOS (`NSEventTypeLeftMouseDown` with `controlKeyMask`; `menuForEvent:` already covers it). Keyboard
  (Menu key / Shift+F10 / Ctrl+Return): NSResponder has no standard shortcut: handle `keyDown:` for `kVK_F10 + shift` or
  `NSMenuFunctionKey` in the subclasses, use the selected row rect (`rectOfRow:`) or view centre.
  For NSTableView/NSOutlineView, `menuForEvent:` should first select the clicked row (`rowAtPoint:` ->
  `selectRowIndexes:`, emitting the normal selection event) unless it is already selected, then emit.
* `popup_menu`: `NSMenu.popUpMenuPositioningItem:atLocation:inView:` (blocks in a menu-tracking loop until dismissed;
  item actions are delivered before it returns, on the same thread): `inView` = the window's content view (flipped),
  `atLocation` = `(x,y)` as an NSPoint in that view's coordinates (flipped y, no conversion). `at == None`:
  `NSEvent.mouseLocation` -> `window.convertPointFromScreen` -> `contentView.convert(_, from: nil)`. `positioningItem: nil`.
  If `parent_window` is None use `NSApp.keyWindow`; if there is no window at all use `inView: nil` with a screen location.
  Do not retain a pointer to the menu across the call without ownership: it is owned by your id map.
* Pitfalls: `NSMenuItem.target` is weak (retain the target); menus must have been built before `popUp...`, and you may
  not mutate the menu while it is tracking (the core never does: `on_context_menu` runs before `popup_menu`); with
  `autoenablesItems = YES` your `Enabled(false)` is overridden by `validateMenuItem:`: set it to NO on every `NSMenu`.
