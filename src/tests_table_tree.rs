//! Table, Tree and context-menu tests against the mock backend.

use crate::backend::mock::{self, widget};
use crate::*;
use accesskit::{Action, ActionRequest, NodeId, Role, TreeId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn init() {
    let _ = App::new("test");
}

fn setup() -> (Window, Table) {
    init();
    let win = Window::new("w");
    let t = Table::new(win);
    t.set_columns(&[Column::new("Name").width(120), Column::new("Size").align(ColumnAlign::Right).sortable(true)]);
    (win, t)
}

#[test]
fn table_mirrors_columns_rows_and_cells() {
    let (_win, t) = setup();
    assert!(t.is_alive());
    t.push_row(&["a", "1"]);
    t.push_row(&["b", "2"]);
    t.set_cell(1, 1, "22");
    t.set_cell(5, 0, "ignored");
    let w = widget(t.id()).unwrap();
    assert_eq!(w.kind, Some(Kind::Table));
    assert_eq!(w.columns.len(), 2);
    assert_eq!(w.columns[0].width, 120);
    assert_eq!(w.columns[1].align, ColumnAlign::Right);
    assert!(w.columns[1].sortable);
    assert_eq!(w.rows, vec![vec!["a", "1"], vec!["b", "22"]]);
    assert_eq!(t.cell(1, 1), "22");
    assert_eq!(t.row_count(), 2);
    // padding of short rows
    t.push_row(&["c"]);
    t.set_cell(2, 3, "x");
    assert_eq!(t.row(2), vec!["c", "", "", "x"]);
    t.set_rows(&[vec!["z", "9"]]);
    assert_eq!(widget(t.id()).unwrap().rows, vec![vec!["z", "9"]]);
    t.clear();
    assert!(widget(t.id()).unwrap().rows.is_empty());
}

#[test]
fn table_columns_set_before_and_after_rows_and_initial_sync() {
    init();
    let win = Window::new("w");
    let t = Table::new(win);
    t.push_row(&["a"]);
    t.add_column(Column::new("A"));
    let w = widget(t.id()).unwrap();
    assert_eq!(w.columns.len(), 1);
    assert_eq!(w.rows.len(), 1, "rows are re-sent after columns");
    assert_eq!(t.columns()[0].title, "A");
}

#[test]
fn table_selection_follows_row_edits() {
    let (_win, t) = setup();
    t.set_rows(&[vec!["a"], vec!["b"], vec!["c"]]);
    t.set_selected(Some(2));
    assert_eq!(widget(t.id()).unwrap().selected, Some(2));
    t.remove_row(0);
    assert_eq!(t.selected(), Some(1));
    assert_eq!(widget(t.id()).unwrap().selected, Some(1));
    t.insert_row(0, &["n"]);
    assert_eq!(t.selected(), Some(2));
    t.remove_row(2);
    assert_eq!(t.selected(), None);
    assert_eq!(widget(t.id()).unwrap().selected, None);
    t.set_selected(Some(99));
    assert_eq!(t.selected(), None);
    t.set_selected(Some(1));
    t.set_rows(&[vec!["only"]]);
    assert_eq!(t.selected(), None, "selection cleared when out of range");
    assert_eq!(widget(t.id()).unwrap().selected, None);
    t.set_selected(Some(0));
    assert_eq!(t.selected_row(), Some(vec!["only".to_string()]));
    t.clear();
    assert_eq!(t.selected(), None);
}

#[test]
fn table_events_and_callbacks() {
    let (_win, t) = setup();
    t.set_rows(&[vec!["a", "1"], vec!["b", "2"]]);
    let log = Rc::new(RefCell::new(vec![]));
    let l = log.clone();
    t.on_select(move |s| l.borrow_mut().push(format!("sel {s:?}")));
    let l = log.clone();
    t.on_activate(move |r| l.borrow_mut().push(format!("act {r}")));
    let l = log.clone();
    t.on_column_click(move |c| l.borrow_mut().push(format!("col {c}")));
    mock::user_select_row(t.id(), Some(1));
    assert_eq!(t.selected(), Some(1));
    mock::user_activate_row(t.id(), 1);
    mock::user_click_column(t.id(), 1);
    // out-of-range events from a confused backend are dropped
    mock::user_activate_row(t.id(), 7);
    mock::user_click_column(t.id(), 9);
    mock::user_select_row(t.id(), Some(7));
    assert_eq!(t.selected(), Some(1));
    mock::user_select_row(t.id(), None);
    assert_eq!(t.selected(), None);
    assert_eq!(*log.borrow(), ["sel Some(1)", "act 1", "col 1", "sel None"]);
}

#[test]
fn table_sort_indicator_and_app_side_sorting() {
    let (_win, t) = setup();
    t.set_rows(&[vec!["b", "2"], vec!["a", "1"]]);
    t.on_column_click(move |c| {
        let mut rows = t.rows();
        rows.sort_by(|x, y| x[c].cmp(&y[c]));
        t.set_rows(&rows);
        t.set_sort_indicator(Some((c, true)));
    });
    mock::user_click_column(t.id(), 0);
    let w = widget(t.id()).unwrap();
    assert_eq!(w.rows[0][0], "a");
    assert_eq!(w.sort, Some((0, true)));
    assert_eq!(t.sort_indicator(), Some((0, true)));
    t.set_sort_indicator(Some((9, true)));
    assert_eq!(t.sort_indicator(), None);
    assert_eq!(widget(t.id()).unwrap().sort, None);
}

#[test]
fn table_batch_sends_once() {
    let (_win, t) = setup();
    t.batch(|t| {
        for i in 0..50 {
            t.push_row(&[i.to_string(), "x".to_string()]);
        }
        assert!(widget(t.id()).unwrap().rows.is_empty(), "deferred inside the batch");
        t.set_selected(Some(3));
    });
    let w = widget(t.id()).unwrap();
    assert_eq!(w.rows.len(), 50);
    assert_eq!(w.selected, Some(3));
}

#[test]
fn table_dead_and_stale_handles_are_inert() {
    let (_win, t) = setup();
    t.destroy();
    t.push_row(&["a"]);
    t.set_cell(0, 0, "x");
    t.set_selected(Some(0));
    t.on_select(|_| {});
    assert_eq!(t.row_count(), 0);
    assert_eq!(t.selected(), None);
    t.batch(|t| t.push_row(&["a"]));
    let bad = Table::new(Widget(WidgetId::DEAD));
    assert!(!bad.is_alive());
    let _ = last_error();
    // wrong-kind handle
    let (win, _) = setup();
    let b = Button::new(win, "b");
    let fake = Table::from_id(b.id());
    fake.push_row(&["a"]);
    fake.set_columns(&[Column::new("x")]);
    assert_eq!(fake.row_count(), 0);
    assert_eq!(widget(b.id()).unwrap().rows.len(), 0);
}

#[test]
fn table_in_layout_gets_bounds() {
    let (win, t) = setup();
    t.set_expand(1.0);
    win.set_size(400, 300);
    win.show();
    App::update();
    let b = widget(t.id()).unwrap().bounds;
    assert!(b.w >= 300 && b.h >= 150, "{b:?}");
}

// ---------------------------------------------------------------- tree

fn tree_setup() -> (Window, Tree, TreeNodeId, TreeNodeId, TreeNodeId) {
    init();
    let win = Window::new("w");
    let t = Tree::new(win);
    let root = t.add(None, "root");
    let a = t.add(Some(root), "a");
    let b = t.add(Some(root), "b");
    (win, t, root, a, b)
}

fn rows(t: Tree) -> Vec<(u64, u32, String, bool, bool)> {
    widget(t.id()).unwrap().tree_rows.into_iter().map(|r| (r.node, r.depth, r.text, r.expanded, r.has_children)).collect()
}

#[test]
fn tree_flattens_preorder_with_depth_and_flags() {
    let (_w, t, root, a, b) = tree_setup();
    let a1 = t.add(Some(a), "a1");
    let c = t.add(None, "c");
    t.set_expanded(root, true);
    let r = rows(t);
    let names: Vec<_> = r.iter().map(|x| (x.2.as_str(), x.1)).collect();
    assert_eq!(names, [("root", 0), ("a", 1), ("a1", 2), ("b", 1), ("c", 0)]);
    assert_eq!(r[0].0, root.0);
    assert!(r[0].3 && !r[1].3);
    assert!(r[0].4 && r[1].4 && !r[2].4 && !r[3].4 && !r[4].4);
    assert_eq!(t.children(Some(root)), vec![a, b]);
    assert_eq!(t.children(None), vec![root, c]);
    assert_eq!(t.parent(a1), Some(a));
    assert_eq!(t.parent(root), None);
    assert_eq!(t.len(), 5);
    assert_eq!(t.text(a1), "a1");
    t.set_text(a1, "renamed");
    assert_eq!(rows(t)[2].2, "renamed");
}

#[test]
fn tree_insert_remove_and_dead_ids() {
    let (_w, t, root, a, b) = tree_setup();
    let mid = t.insert(Some(root), 1, "mid");
    assert_eq!(t.children(Some(root)), vec![a, mid, b]);
    let bogus = t.add(Some(TreeNodeId(999_999_999)), "x");
    assert_eq!(bogus, TreeNodeId(0));
    assert!(!t.contains(bogus));
    let a1 = t.add(Some(a), "a1");
    t.set_selected(Some(a1));
    t.remove(a);
    assert!(!t.contains(a) && !t.contains(a1));
    assert_eq!(t.selected(), None);
    assert_eq!(widget(t.id()).unwrap().tree_selected, None);
    assert_eq!(rows(t).len(), 3);
    t.remove(a); // twice: no-op
    t.set_expanded(a, true);
    t.set_text(a, "x");
    assert_eq!(t.text(a), "");
    t.clear();
    assert!(t.is_empty());
    assert!(widget(t.id()).unwrap().tree_rows.is_empty());
}

#[test]
fn tree_selection_reveals_ancestors_and_is_silent() {
    let (_w, t, root, a, _b) = tree_setup();
    let a1 = t.add(Some(a), "a1");
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    t.on_select(move |_| f.set(f.get() + 1));
    let f = fired.clone();
    t.on_expand(move |_, _| f.set(f.get() + 10));
    t.set_selected(Some(a1));
    assert_eq!(fired.get(), 0);
    assert!(t.expanded(root) && t.expanded(a));
    assert_eq!(t.selected(), Some(a1));
    let w = widget(t.id()).unwrap();
    assert_eq!(w.tree_selected, Some(a1.0));
    assert!(w.tree_rows.iter().find(|r| r.node == a.0).unwrap().expanded);
    t.set_selected(Some(TreeNodeId(424242)));
    assert_eq!(t.selected(), None);
    t.expand_all(false);
    assert!(!t.expanded(root));
    t.expand_all(true);
    assert!(t.expanded(a));
}

#[test]
fn tree_user_events_update_state_and_call_back() {
    let (_w, t, root, a, _b) = tree_setup();
    let log = Rc::new(RefCell::new(vec![]));
    let l = log.clone();
    t.on_select(move |s| l.borrow_mut().push(format!("sel {:?}", s.map(|n| n.0))));
    let l = log.clone();
    t.on_activate(move |n| l.borrow_mut().push(format!("act {}", n.0)));
    let l = log.clone();
    t.on_expand(move |n, e| l.borrow_mut().push(format!("exp {} {e}", n.0)));
    mock::user_tree_expand(t.id(), root.0, true);
    assert!(t.expanded(root));
    mock::user_tree_select(t.id(), Some(a.0));
    assert_eq!(t.selected(), Some(a));
    mock::user_tree_activate(t.id(), a.0);
    mock::user_tree_expand(t.id(), root.0, false);
    assert!(!t.expanded(root));
    // unknown nodes are dropped without callbacks
    mock::user_tree_select(t.id(), Some(7777));
    mock::user_tree_activate(t.id(), 7777);
    mock::user_tree_expand(t.id(), 7777, true);
    mock::user_tree_select(t.id(), None);
    assert_eq!(
        *log.borrow(),
        [format!("exp {} true", root.0), format!("sel Some({})", a.0), format!("act {}", a.0), format!("exp {} false", root.0), "sel None".to_string()]
    );
}

#[test]
fn tree_lazy_loading_in_expand_callback() {
    init();
    let win = Window::new("w");
    let t = Tree::new(win);
    let dir = t.add(None, "dir");
    t.set_has_children(dir, true);
    assert!(rows(t)[0].4, "expander shown without children");
    t.on_expand(move |n, open| {
        if open && t.children(Some(n)).is_empty() {
            t.add(Some(n), "file1");
            t.add(Some(n), "file2");
        }
    });
    mock::user_tree_expand(t.id(), dir.0, true);
    let r = rows(t);
    assert_eq!(r.len(), 3);
    assert!(r[0].3, "stays expanded after the core re-sent the rows");
    assert_eq!(r[1].2, "file1");
    assert_eq!(r[1].1, 1);
}

#[test]
fn tree_batch_and_nested_batches() {
    init();
    let win = Window::new("w");
    let t = Tree::new(win);
    t.batch(|t| {
        let r = t.add(None, "r");
        t.batch(|t| {
            for i in 0..20 {
                t.add(Some(r), &i.to_string());
            }
        });
        assert!(widget(t.id()).unwrap().tree_rows.is_empty());
        t.set_selected(Some(r));
    });
    let w = widget(t.id()).unwrap();
    assert_eq!(w.tree_rows.len(), 21);
    assert!(w.tree_selected.is_some());
}

#[test]
fn tree_destroyed_in_callback_is_safe() {
    let (_w, t, root, ..) = tree_setup();
    t.on_expand(move |_, _| t.destroy());
    mock::user_tree_expand(t.id(), root.0, true);
    assert!(!t.is_alive());
}

// ---------------------------------------------------------------- context menus

fn menu_setup() -> (Window, PopupMenu, MenuItem, CheckMenuItem, MenuItem) {
    init();
    let win = Window::new("w");
    let pm = PopupMenu::new();
    let cut = MenuItem::new(&pm, "Cut");
    MenuSeparator::new(&pm);
    let chk = CheckMenuItem::new(&pm, "Wrap");
    let sub = Menu::new(&pm, "More");
    let deep = MenuItem::new(sub, "Deep");
    (win, pm, cut, chk, deep)
}

#[test]
fn popup_menu_builds_natively_and_is_parentless() {
    let (_win, pm, cut, chk, deep) = menu_setup();
    assert!(pm.is_alive() && cut.is_alive() && chk.is_alive() && deep.is_alive());
    assert_eq!(widget(pm.id()).unwrap().kind, Some(Kind::PopupMenu));
    assert_eq!(widget(pm.id()).unwrap().parent, None);
    assert_eq!(widget(cut.id()).unwrap().parent, Some(pm.id()));
    // a popup cannot live inside a window, and menu bars cannot live in popups
    let bad = core::create(Kind::PopupMenu, Some(_win.id()), |_| {});
    assert_eq!(bad, WidgetId::DEAD);
    let _ = last_error();
    let bar = core::create(Kind::MenuBar, Some(pm.id()), |_| {});
    assert_eq!(bar, WidgetId::DEAD);
    let _ = last_error();
    assert!(!Kind::PopupMenu.in_layout());
}

#[test]
fn right_click_runs_callback_then_popup_and_item_fires() {
    let (win, pm, cut, chk, _deep) = menu_setup();
    let lst = ListBox::new(win);
    lst.set_context_menu(&pm);
    let order = Rc::new(RefCell::new(vec![]));
    let o = order.clone();
    lst.on_context_menu(move |x, y| {
        o.borrow_mut().push(format!("cb {x},{y} popups={}", mock::popup_log().len()));
        cut.set_enabled(false); // rebuilt just before display
    });
    let o = order.clone();
    chk.on_toggle(move |b| o.borrow_mut().push(format!("toggle {b}")));
    mock::queue_popup_choice(Some(chk.id()));
    mock::user_context_menu(lst.id(), 30, 40);
    let rec = mock::last_popup().unwrap();
    assert_eq!(rec.menu, pm.id());
    assert_eq!(rec.window, Some(win.id()));
    assert_eq!(rec.at, Some((30, 40)));
    assert_eq!(
        rec.items,
        vec![
            (Kind::MenuItem, "Cut".into(), false),
            (Kind::MenuSeparator, "".into(), true),
            (Kind::CheckMenuItem, "Wrap".into(), true),
            (Kind::Menu, "More".into(), true),
        ]
    );
    assert_eq!(*order.borrow(), ["cb 30,40 popups=0", "toggle true"]);
    assert!(chk.checked());
}

#[test]
fn popup_item_click_and_disabled_item_and_dismiss() {
    let (win, pm, cut, _chk, deep) = menu_setup();
    win.set_context_menu(&pm);
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    cut.on_click(move || c.set(c.get() + 1));
    let c = clicks.clone();
    deep.on_click(move || c.set(c.get() + 100));
    mock::queue_popup_choice(Some(cut.id()));
    mock::user_context_menu(win.id(), 1, 2);
    assert_eq!(clicks.get(), 1);
    mock::queue_popup_choice(Some(deep.id())); // item of a submenu
    mock::user_context_menu(win.id(), 1, 2);
    assert_eq!(clicks.get(), 101);
    cut.set_enabled(false);
    mock::queue_popup_choice(Some(cut.id()));
    mock::user_context_menu(win.id(), 1, 2);
    assert_eq!(clicks.get(), 101, "disabled items cannot fire");
    mock::queue_popup_choice(None); // dismissed
    mock::user_context_menu(win.id(), 1, 2);
    assert_eq!(clicks.get(), 101);
    assert_eq!(mock::popup_log().len(), 4);
}

#[test]
fn context_menu_bubbles_to_ancestor_and_ignores_unwanted() {
    let (win, pm, ..) = menu_setup();
    mock::clear_popup_log();
    let col = VBox::new(win);
    let lbl = Label::new(col, "x");
    mock::user_context_menu(lbl.id(), 5, 6);
    assert!(mock::popup_log().is_empty(), "nobody wants it");
    win.set_context_menu(&pm);
    mock::user_context_menu(lbl.id(), 5, 6);
    assert_eq!(mock::popup_log().len(), 1);
    assert_eq!(mock::last_popup().unwrap().at, Some((5, 6)));
    // the widget's own menu wins; clearing it falls back to the window's
    let other = PopupMenu::new();
    lbl.set_context_menu(&other);
    mock::user_context_menu(lbl.id(), 1, 1);
    assert_eq!(mock::last_popup().unwrap().menu, other.id());
    lbl.clear_context_menu();
    mock::user_context_menu(lbl.id(), 1, 1);
    assert_eq!(mock::last_popup().unwrap().menu, pm.id());
    // disabled widgets do not show menus
    lbl.set_context_menu(&other);
    lbl.set_enabled(false);
    let n = mock::popup_log().len();
    mock::user_context_menu(lbl.id(), 1, 1);
    assert_eq!(mock::popup_log().len(), n);
}

#[test]
fn callback_only_context_menu_and_menu_attached_in_callback() {
    let (win, pm, ..) = menu_setup();
    mock::clear_popup_log();
    let b = Button::new(win, "b");
    let got = Rc::new(Cell::new((0, 0)));
    let g = got.clone();
    b.on_context_menu(move |x, y| {
        g.set((x, y));
        b.set_context_menu(&pm);
    });
    mock::user_context_menu(b.id(), 9, 8);
    assert_eq!(got.get(), (9, 8));
    assert_eq!(mock::popup_log().len(), 1, "menu attached by the callback is shown");
}

#[test]
fn programmatic_show_at_and_stale_menus() {
    let (win, pm, ..) = menu_setup();
    mock::clear_popup_log();
    pm.show_at(win, 10, 20);
    assert_eq!(mock::last_popup().unwrap().at, Some((10, 20)));
    assert_eq!(mock::last_popup().unwrap().window, Some(win.id()));
    pm.show(win);
    assert_eq!(mock::last_popup().unwrap().at, None);
    // a destroyed menu / non-popup widget never reaches the backend
    let b = Button::new(win, "b");
    PopupMenu::from_id(b.id()).show_at(win, 1, 1);
    let n = mock::popup_log().len();
    win.set_context_menu(&pm);
    pm.destroy();
    mock::user_context_menu(win.id(), 1, 1);
    pm.show_at(win, 1, 1);
    assert_eq!(mock::popup_log().len(), n);
    // destroyed window as parent is dropped to None
    let w2 = Window::new("w2");
    let pm2 = PopupMenu::new();
    w2.destroy();
    pm2.show_at(w2, 1, 1);
    assert_eq!(mock::last_popup().unwrap().window, None);
}

#[test]
fn popup_menu_destroy_removes_native_children() {
    let (_win, pm, cut, ..) = menu_setup();
    let before = mock::widget_count();
    pm.destroy();
    assert!(!cut.is_alive());
    assert!(widget(cut.id()).is_none());
    assert_eq!(mock::widget_count(), before - 6);
}

// ---------------------------------------------------------------- accessibility

fn req(a: Action, id: NodeId) -> ActionRequest {
    ActionRequest { action: a, target_tree: TreeId::ROOT, target_node: id, data: None }
}

fn node_of(t: &accesskit::TreeUpdate, id: NodeId) -> accesskit::Node {
    t.nodes.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone()).unwrap()
}

#[test]
fn a11y_table_structure_and_actions() {
    let (win, t) = setup();
    t.set_rows(&[vec!["a", "1"], vec!["b", "2"]]);
    t.set_selected(Some(1));
    win.show();
    App::update();
    let tree = a11y::tree_for_window(win.id()).unwrap();
    let tn = node_of(&tree, NodeId(t.id().0));
    assert_eq!(tn.role(), Role::Table);
    assert_eq!(tn.row_count(), Some(3));
    assert_eq!(tn.column_count(), Some(2));
    let kids = tn.children().to_vec();
    assert_eq!(kids.len(), 3, "header row + 2 rows");
    let header = node_of(&tree, kids[0]);
    assert_eq!(header.role(), Role::Row);
    let h1 = node_of(&tree, header.children()[1]);
    assert_eq!(h1.role(), Role::ColumnHeader);
    assert_eq!(h1.label(), Some("Size"));
    assert_eq!(h1.column_index(), Some(1));
    let r1 = node_of(&tree, kids[2]);
    assert_eq!(r1.role(), Role::Row);
    assert_eq!(r1.is_selected(), Some(true));
    assert_eq!(r1.row_index(), Some(2));
    let cell = node_of(&tree, r1.children()[1]);
    assert_eq!(cell.role(), Role::Cell);
    assert_eq!(cell.value(), Some("2"));
    assert_eq!(cell.column_index(), Some(1));
    // actions: click row 0 selects it; header click reports the column
    let sel = Rc::new(RefCell::new(vec![]));
    let s = sel.clone();
    t.on_select(move |x| s.borrow_mut().push(format!("s{x:?}")));
    let s = sel.clone();
    t.on_column_click(move |c| s.borrow_mut().push(format!("c{c}")));
    a11y::do_action(win.id(), &req(Action::Click, kids[1]));
    a11y::do_action(win.id(), &req(Action::Click, header.children()[1]));
    assert_eq!(t.selected(), Some(0));
    assert_eq!(widget(t.id()).unwrap().selected, Some(0));
    assert_eq!(*sel.borrow(), ["sSome(0)", "c1"]);
}

#[test]
fn a11y_tree_items_levels_and_expand_actions() {
    let (win, t, root, a, b) = tree_setup();
    let a1 = t.add(Some(a), "a1");
    t.set_expanded(root, true);
    t.set_selected(Some(b));
    win.show();
    App::update();
    let id = |n: TreeNodeId| NodeId((1 << 62) | (1 << 60) | n.0);
    let tree = a11y::tree_for_window(win.id()).unwrap();
    let tn = node_of(&tree, NodeId(t.id().0));
    assert_eq!(tn.role(), Role::Tree);
    assert_eq!(tn.children(), &[id(root)]);
    let rn = node_of(&tree, id(root));
    assert_eq!(rn.role(), Role::TreeItem);
    assert_eq!(rn.level(), Some(1));
    assert_eq!(rn.is_expanded(), Some(true));
    assert_eq!(rn.children(), &[id(a), id(b)]);
    let an = node_of(&tree, id(a));
    assert_eq!(an.level(), Some(2));
    assert_eq!(an.is_expanded(), Some(false));
    assert!(an.children().is_empty(), "collapsed: children hidden");
    assert!(tree.nodes.iter().all(|(i, _)| *i != id(a1)));
    assert_eq!(node_of(&tree, id(b)).is_selected(), Some(true));
    assert_eq!(node_of(&tree, id(b)).is_expanded(), None, "leaf has no expanded state");
    // actions
    let log = Rc::new(RefCell::new(vec![]));
    let l = log.clone();
    t.on_expand(move |n, e| l.borrow_mut().push(format!("e{} {e}", n.0 == a.0)));
    let l = log.clone();
    t.on_select(move |n| l.borrow_mut().push(format!("s{}", n.map_or(0, |n| n.0) == a.0)));
    a11y::do_action(win.id(), &req(Action::Expand, id(a)));
    assert!(t.expanded(a));
    assert!(widget(t.id()).unwrap().tree_rows.iter().find(|r| r.node == a.0).unwrap().expanded);
    a11y::do_action(win.id(), &req(Action::Click, id(a)));
    assert_eq!(t.selected(), Some(a));
    assert_eq!(widget(t.id()).unwrap().tree_selected, Some(a.0));
    a11y::do_action(win.id(), &req(Action::Collapse, id(a)));
    assert!(!t.expanded(a));
    assert_eq!(*log.borrow(), ["etrue true", "strue", "etrue false"]);
    // synthetic ids for unknown nodes are ignored
    a11y::do_action(win.id(), &req(Action::Click, NodeId((1 << 62) | (1 << 60) | 987654321)));
}

#[test]
fn a11y_popup_menu_role() {
    let (_win, pm, ..) = menu_setup();
    assert_eq!(pm.a11y().role, None);
    // popup menus are top-level objects: they are not part of a window tree
    assert!(a11y::tree_for_window(pm.id()).is_none());
}
