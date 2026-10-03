//! Accessibility-tree consistency after random mutations, hostile AT action requests, deep
//! structures, callback re-entrancy for every event kind, and timer / post ordering.

use crate::backend::mock::{self, widget};
use crate::tests_fuzz_layout::Rng;
use crate::*;
use accesskit::{Action, ActionData, ActionRequest, NodeId, TreeId};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

fn init() {
    let _ = App::new("test");
}

fn req(a: Action, node: u64, data: Option<ActionData>) -> ActionRequest {
    ActionRequest { action: a, target_tree: TreeId::ROOT, target_node: NodeId(node), data }
}

/// Structural invariants of an accesskit tree: unique ids, every child exists, every node but
/// the root has exactly one parent, the focus exists, no cycles.
fn check_tree(t: &accesskit::TreeUpdate, ctx: &str) {
    let mut ids: HashMap<NodeId, &accesskit::Node> = HashMap::new();
    for (id, n) in &t.nodes {
        assert!(ids.insert(*id, n).is_none(), "duplicate node id {id:?}; {ctx}");
    }
    let root = t.tree.as_ref().expect("tree info").root;
    assert!(ids.contains_key(&root), "root missing; {ctx}");
    assert!(ids.contains_key(&t.focus), "focus {:?} not in tree; {ctx}", t.focus);
    let mut parents: HashMap<NodeId, usize> = HashMap::new();
    for (id, n) in &t.nodes {
        for c in n.children() {
            assert!(ids.contains_key(c), "{id:?} lists missing child {c:?}; {ctx}");
            *parents.entry(*c).or_default() += 1;
        }
        if let Some(b) = n.bounds() {
            assert!(b.x0.is_finite() && b.x1 >= b.x0 && b.y1 >= b.y0, "bad bounds {b:?}; {ctx}");
        }
    }
    for (id, _) in &t.nodes {
        let p = parents.get(id).copied().unwrap_or(0);
        assert_eq!(p, usize::from(*id != root), "{id:?} has {p} parents; {ctx}");
    }
    // reachability (also rules out cycles, since each node has one parent)
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        assert!(seen.insert(i), "cycle at {i:?}; {ctx}");
        stack.extend(ids[&i].children().iter().copied());
    }
    assert_eq!(seen.len(), ids.len(), "orphans; {ctx}");
}

fn word(rng: &mut Rng) -> String {
    ["", "a", "ünï", "日本", "&File", "x y"][rng.below(6)].to_string()
}

#[test]
fn a11y_tree_stays_consistent_under_random_mutation() {
    init();
    for seed in 1..=60u64 {
        let mut rng = Rng::new(300 + seed);
        let win = Window::new("w");
        let col = VBox::new(win);
        let tabs = Tabs::new(col);
        let pa = tabs.add_page("one");
        let pb = tabs.add_page("two");
        let mut live: Vec<Widget> = vec![];
        let tables = [Table::new(pa), Table::new(pb)];
        let tree = Tree::new(pa);
        let list = ListBox::new(pb);
        let sp = Splitter::new(col, Orientation::Vertical);
        ListBox::new(sp);
        ListBox::new(sp);
        win.show();
        let mut nodes = vec![];
        for step in 0..60 {
            match rng.below(14) {
                0 => {
                    let l = [Label::new(col, &word(&mut rng)).id(), Button::new(col, &word(&mut rng)).id(), CheckBox::new(col, "c").id(), TextInput::new(col).id(), Slider::new(col, 0.0, 5.0).id(), ProgressBar::new(col).id(), ComboBox::new(col).id()];
                    live.push(Widget(l[rng.below(l.len())]));
                }
                1 if !live.is_empty() => {
                    let i = rng.below(live.len());
                    live.remove(i).destroy();
                }
                2 if !live.is_empty() => live[rng.below(live.len())].set_visible(rng.chance(50)),
                3 if !live.is_empty() => live[rng.below(live.len())].set_enabled(rng.chance(50)),
                4 => {
                    let rows: Vec<Vec<String>> = (0..rng.below(4)).map(|_| (0..rng.below(4)).map(|_| word(&mut rng)).collect()).collect();
                    let t = tables[rng.below(2)];
                    t.set_columns(&[Column::new("A").sortable(true), Column::new("B")][..rng.below(3)]);
                    t.set_rows(&rows);
                    t.set_selected(Some(rng.below(4)));
                }
                5 => {
                    let p = if nodes.is_empty() || rng.chance(30) { None } else { Some(nodes[rng.below(nodes.len())]) };
                    let n = tree.add(p, &word(&mut rng));
                    if n.0 != 0 {
                        nodes.push(n);
                    }
                }
                6 if !nodes.is_empty() => {
                    let n = nodes.remove(rng.below(nodes.len()));
                    tree.remove(n);
                    nodes.retain(|x| tree.contains(*x));
                }
                7 if !nodes.is_empty() => tree.set_expanded(nodes[rng.below(nodes.len())], rng.chance(60)),
                8 => list.set_items(&(0..rng.below(5)).map(|i| format!("i{i}")).collect::<Vec<_>>()),
                9 => tabs.set_selected(rng.below(3)),
                10 => sp.set_position(rng.range(0, 300)),
                11 => mock::resize_window(win.id(), rng.range(50, 500), rng.range(50, 500)),
                12 => {
                    if let Some(w) = live.get(rng.below(live.len().max(1))) {
                        w.focus();
                    }
                }
                _ => {}
            }
            App::update();
            let t = a11y::tree_for_window(win.id()).unwrap();
            check_tree(&t, &format!("seed {seed} step {step}"));
        }
        win.destroy();
        assert!(a11y::tree_for_window(win.id()).is_none());
    }
}

#[test]
fn hostile_action_requests_never_panic() {
    init();
    let mut rng = Rng::new(99);
    let win = Window::new("w");
    let col = VBox::new(win);
    let _ = (Button::new(col, "b"), CheckBox::new(col, "c"), TextInput::new(col), Slider::new(col, 5.0, 1.0), SpinBox::new(col, 0.0, 10.0, 0.0));
    let _ = (ComboBox::new(col), ListBox::new(col));
    let table = Table::new(col);
    table.set_columns(&[Column::new("A")]);
    table.set_rows(&[vec!["x"], vec!["y"]]);
    let tree = Tree::new(col);
    let n = tree.add(None, "n");
    tree.add(Some(n), "k");
    let sp = Splitter::new(col, Orientation::Horizontal);
    ListBox::new(sp);
    ListBox::new(sp);
    win.show();
    App::update();
    let t = a11y::tree_for_window(win.id()).unwrap();
    let mut targets: Vec<u64> = t.nodes.iter().map(|(i, _)| i.0).collect();
    let actions = [Action::Click, Action::Focus, Action::SetValue, Action::Increment, Action::Decrement, Action::Expand, Action::Collapse, Action::ScrollIntoView];
    let datas = [None, Some(ActionData::NumericValue(f64::NAN)), Some(ActionData::NumericValue(f64::INFINITY)), Some(ActionData::NumericValue(-1e300)), Some(ActionData::NumericValue(3.0)), Some(ActionData::Value("a\0b".into())), Some(ActionData::Value("".into()))];
    for _ in 0..200 {
        targets.push(rng.next()); // garbage ids, including synthetic-flag combinations
    }
    for _ in 0..3000 {
        let tgt = targets[rng.below(targets.len())];
        let r = req(actions[rng.below(actions.len())], tgt, datas[rng.below(datas.len())].clone());
        a11y::do_action(win.id(), &r);
        if rng.chance(5) {
            // also from the wrong window and a dead window
            a11y::do_action(WidgetId(rng.next()), &r);
        }
    }
    App::update();
    check_tree(&a11y::tree_for_window(win.id()).unwrap(), "after hostile requests");
}

#[test]
fn degenerate_slider_ranges_do_not_panic() {
    init();
    let win = Window::new("w");
    let nan = f64::NAN;
    let inf = f64::INFINITY;
    for (a, b) in [(nan, 1.0), (1.0, nan), (nan, nan), (inf, -inf), (-inf, inf), (5.0, 1.0), (0.0, 0.0)] {
        let s = Slider::new(win, a, b);
        let p = SpinBox::new(win, a, b, nan);
        for w in [Widget(s.id()), Widget(p.id())] {
            let _ = w;
        }
        for v in [0.5, nan, inf, -inf, 1e300] {
            s.set_value(v);
            p.set_value(v);
            assert!(!s.value().is_nan() || a.is_nan() && b.is_nan(), "slider value {} for range {a} {b}", s.value());
            mock::user(s.id(), Event::Value(v));
            a11y::do_action(win.id(), &req(Action::Increment, s.id().0, None));
            a11y::do_action(win.id(), &req(Action::SetValue, p.id().0, Some(ActionData::NumericValue(v))));
        }
        s.set_range(a, b, nan);
        p.set_range(b, a, -1.0);
        s.set_value(0.3);
        p.set_value(0.3);
        a11y::do_action(win.id(), &req(Action::Decrement, p.id().0, None));
        let _ = widget(s.id());
    }
    win.show();
    App::update();
    let _ = a11y::tree_for_window(win.id());
}

#[test]
fn deep_trees_do_not_overflow_the_stack() {
    init();
    let win = Window::new("w");
    let tree = Tree::new(win);
    win.show();
    let mut parent = None;
    tree.batch(|t| {
        for i in 0..30_000 {
            let n = t.add(parent, "n");
            t.set_expanded(n, true);
            parent = Some(n);
            let _ = i;
        }
    });
    App::update();
    assert_eq!(tree.len(), 30_000);
    let t = a11y::tree_for_window(win.id()).unwrap();
    assert!(t.nodes.len() > 100);
    let leaf = parent.unwrap();
    tree.set_selected(Some(leaf));
    tree.remove(tree.children(None)[0]);
    assert_eq!(tree.len(), 0);
    // nested layout containers
    let col = VBox::new(win);
    let mut cur: WidgetId = col.id();
    for _ in 0..200 {
        cur = HBox::new(cur).id();
    }
    Label::new(cur, "deep");
    App::update();
    let _ = a11y::tree_for_window(win.id());
    win.destroy();
}

// ------------------------------------------------------------------ re-entrancy

/// Everything a callback can reach; `Copy` handles, so closures capture it freely.
#[derive(Clone, Copy)]
struct Fx {
    win: Window,
    col: VBox,
    sibling: Button,
    tab: Table,
    tree: Tree,
    list: ListBox,
    split: Splitter,
}

fn fixture() -> Fx {
    let win = Window::new("w");
    let col = VBox::new(win);
    let sibling = Button::new(col, "sib");
    let tab = Table::new(col);
    tab.set_columns(&[Column::new("A").sortable(true)]);
    tab.set_rows(&[vec!["r0"], vec!["r1"]]);
    let tree = Tree::new(col);
    let n = tree.add(None, "root");
    tree.add(Some(n), "kid");
    let list = ListBox::new(col);
    list.set_items(&["x", "y"]);
    let split = Splitter::new(col, Orientation::Horizontal);
    Label::new(split, "p1");
    Label::new(split, "p2");
    win.show();
    App::update();
    Fx { win, col, sibling, tab, tree, list, split }
}

const ACTIONS: usize = 16;

/// A hostile callback body: action `k` mutates or destroys things the running event is about.
fn chaos(k: usize, fx: Fx, me: WidgetId, hits: &Rc<Cell<u32>>) {
    let me = Widget(me);
    hits.set(hits.get() + 1);
    match k {
        0 => me.destroy(),
        1 => fx.win.destroy(),
        2 => fx.col.destroy(),
        3 => fx.sibling.destroy(),
        4 => {
            let b = Button::new(fx.col, "new");
            b.on_click(|| {});
        }
        5 => me.set_visible(false),
        6 => me.set_enabled(false),
        7 => {
            fx.tab.set_rows(&[vec!["z"]]);
            fx.list.set_items(&["only"]);
            fx.tree.clear();
        }
        8 => {
            fx.tab.clear();
            fx.tree.set_selected(None);
        }
        9 => me.set_a11y_name("renamed"),
        10 => {
            // replace the running callback of this very widget
            Widget(me.id()).on_context_menu(|_, _| {});
            if let Some(b) = Some(Button::from_id(me.id())) {
                b.on_click(|| {});
            }
        }
        11 => {
            let _ = Timer::once(1, || {});
            App::post(|| {});
        }
        12 => {
            fx.split.set_position(5);
            fx.split.destroy();
        }
        13 => {
            // nested user events from inside a callback (bounded)
            if hits.get() < 4 {
                mock::user_click(fx.sibling.id());
                mock::user_text(me.id(), "again");
            }
        }
        14 => panic!("callback panic {k}"),
        _ => {
            App::quit();
            fx.win.set_title("t");
        }
    }
}

fn assert_consistent(fx: Fx, what: &str) {
    App::update();
    // every mock widget belongs to a live core node and vice versa for the handles we hold
    let alive = [fx.win.id(), fx.col.id(), fx.sibling.id(), fx.tab.id(), fx.tree.id(), fx.list.id(), fx.split.id()];
    for id in alive {
        if !Widget(id).is_alive() {
            assert!(widget(id).is_none(), "{what}: destroyed {id:?} still native");
        } else if crate::core::read(id, |n| n.kind.is_native()) == Some(true) {
            assert!(widget(id).is_some(), "{what}: live {id:?} missing natively");
        }
    }
    if fx.win.is_alive() {
        let t = a11y::tree_for_window(fx.win.id()).unwrap();
        check_tree(&t, what);
    }
}

#[test]
fn hostile_callbacks_for_every_event_kind_and_action() {
    init();
    // only the deliberate panic is silenced; real failures stay visible
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|i| {
        if !i.to_string().contains("callback panic") {
            eprintln!("{i}");
        }
    }));
    for k in 0..ACTIONS {
        let hits = Rc::new(Cell::new(0));
        macro_rules! case {
            ($name:expr, |$fx:ident, $h:ident| $reg:expr, |$fx2:ident| $fire:expr) => {{
                let $fx = fixture();
                let $h = hits.clone();
                hits.set(0);
                $reg;
                let $fx2 = $fx;
                $fire;
                assert!(hits.get() >= 1, "{}: callback did not run (action {k})", $name);
                assert_consistent($fx, &format!("{} / action {k}", $name));
                // a second, identical stale event must not crash
                let $fx2 = $fx;
                $fire;
                assert_consistent($fx, &format!("{} / action {k} (again)", $name));
                if $fx.win.is_alive() {
                    $fx.win.destroy();
                }
            }};
        }
        case!("click", |fx, h| fx.sibling.on_click(move || chaos(k, fx, fx.sibling.id(), &h)), |fx| mock::user_click(fx.sibling.id()));
        case!("text", |fx, h| {
            let t = TextInput::new(fx.col);
            t.on_change(move |_| chaos(k, fx, t.id(), &h));
        }, |fx| {
            let id = fx.col.id();
            let t = mock::dump(); // keep optimiser honest
            let _ = (id, t);
            // find the text input: last child of the column that is a TextInput
            let ti = crate::core::read(fx.col.id(), |n| n.children.clone()).unwrap_or_default().into_iter().find(|c| widget(*c).and_then(|w| w.kind) == Some(Kind::TextInput));
            if let Some(ti) = ti {
                mock::user_text(ti, "typed");
            }
        });
        case!("toggle", |fx, h| {
            let c = CheckBox::new(fx.col, "c");
            c.on_toggle(move |_| chaos(k, fx, c.id(), &h));
        }, |fx| {
            let c = crate::core::read(fx.col.id(), |n| n.children.clone()).unwrap_or_default().into_iter().find(|c| widget(*c).and_then(|w| w.kind) == Some(Kind::CheckBox));
            if let Some(c) = c {
                mock::user(c, Event::Toggled(true));
            }
        });
        case!("slider", |fx, h| {
            let s = Slider::new(fx.col, 0.0, 10.0);
            s.on_change(move |_| chaos(k, fx, s.id(), &h));
        }, |fx| {
            let c = crate::core::read(fx.col.id(), |n| n.children.clone()).unwrap_or_default().into_iter().find(|c| widget(*c).and_then(|w| w.kind) == Some(Kind::Slider));
            if let Some(c) = c {
                mock::user(c, Event::Value(3.0));
            }
        });
        case!("list select", |fx, h| fx.list.on_select(move |_| chaos(k, fx, fx.list.id(), &h)), |fx| mock::user(fx.list.id(), Event::Selected(Some(1))));
        case!("list activate", |fx, h| fx.list.on_activate(move |_| chaos(k, fx, fx.list.id(), &h)), |fx| mock::user(fx.list.id(), Event::Activated(0)));
        case!("table select", |fx, h| fx.tab.on_select(move |_| chaos(k, fx, fx.tab.id(), &h)), |fx| mock::user_select_row(fx.tab.id(), Some(0)));
        case!("table activate", |fx, h| fx.tab.on_activate(move |_| chaos(k, fx, fx.tab.id(), &h)), |fx| mock::user_activate_row(fx.tab.id(), 1));
        case!("column click", |fx, h| fx.tab.on_column_click(move |_| chaos(k, fx, fx.tab.id(), &h)), |fx| mock::user_click_column(fx.tab.id(), 0));
        case!("tree select", |fx, h| fx.tree.on_select(move |_| chaos(k, fx, fx.tree.id(), &h)), |fx| {
            if let Some(n) = fx.tree.children(None).first() {
                mock::user_tree_select(fx.tree.id(), Some(n.0));
            }
        });
        case!("tree activate", |fx, h| fx.tree.on_activate(move |_| chaos(k, fx, fx.tree.id(), &h)), |fx| {
            if let Some(n) = fx.tree.children(None).first() {
                mock::user_tree_activate(fx.tree.id(), n.0);
            }
        });
        case!("tree expand", |fx, h| fx.tree.on_expand(move |_, _| chaos(k, fx, fx.tree.id(), &h)), |fx| {
            if let Some(n) = fx.tree.children(None).first() {
                mock::user_tree_expand(fx.tree.id(), n.0, true);
            }
        });
        case!("context menu", |fx, h| fx.sibling.on_context_menu(move |_, _| chaos(k, fx, fx.sibling.id(), &h)), |fx| mock::user_context_menu(fx.sibling.id(), 3, 4));
        case!("sash", |fx, h| fx.split.on_move(move |_| chaos(k, fx, fx.split.id(), &h)), |fx| mock::user_drag_sash_by(fx.split.id(), 7));
        case!("resize", |fx, h| fx.win.on_resize(move |_, _| chaos(k, fx, fx.win.id(), &h)), |fx| mock::resize_window(fx.win.id(), 333, 222));
        case!("window move", |fx, h| fx.win.on_move(move |_, _| chaos(k, fx, fx.win.id(), &h)), |fx| mock::user_move_window(fx.win.id(), 5, 6));
        case!("close", |fx, h| fx.win.on_close(move || { chaos(k, fx, fx.win.id(), &h); k % 2 == 0 }), |fx| mock::user_close(fx.win.id()));
        mock::pump();
    }
    std::panic::set_hook(prev);
}

// ------------------------------------------------------------------ timers and post

#[test]
fn timers_fire_in_due_order_and_cancel_cleanly() {
    init();
    let log = Rc::new(RefCell::new(vec![]));
    let l = |tag: &'static str| {
        let log = log.clone();
        move || log.borrow_mut().push(tag)
    };
    let _a = Timer::once(30, l("a30"));
    let _b = Timer::once(10, l("b10"));
    let ev = Timer::every(10, l("e10"));
    let c = Timer::once(20, l("c20"));
    mock::advance(10);
    assert_eq!(log.borrow().len(), 2);
    assert!(log.borrow().contains(&"b10") && log.borrow().contains(&"e10"));
    c.stop();
    mock::advance(10);
    mock::advance(10);
    let v = log.borrow().clone();
    assert!(!v.contains(&"c20"), "stopped timer fired: {v:?}");
    assert_eq!(v.iter().filter(|x| **x == "e10").count(), 3);
    assert_eq!(v.iter().filter(|x| **x == "a30").count(), 1);
    ev.stop();
    let n = v.len();
    mock::advance(100);
    assert_eq!(log.borrow().len(), n, "repeating timer fired after stop");
}

#[test]
fn timer_callbacks_can_cancel_restart_and_destroy() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "b");
    let count = Rc::new(Cell::new(0));
    let slot: Rc<RefCell<Option<Timer>>> = Rc::new(RefCell::new(None));
    let (c, s) = (count.clone(), slot.clone());
    *slot.borrow_mut() = Some(Timer::every(5, move || {
        c.set(c.get() + 1);
        if c.get() == 3 {
            if let Some(t) = *s.borrow() {
                t.stop(); // cancel myself from inside the callback
            }
            b.destroy();
            win.destroy();
            let _chained = Timer::once(1, || {});
        }
    }));
    for _ in 0..10 {
        mock::advance(5);
    }
    assert_eq!(count.get(), 3);
    assert_eq!(mock::timer_count(), 0, "all timers released: {}", mock::timer_count());
    let panicker = Timer::once(1, || panic!("timer boom"));
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    mock::advance(5);
    std::panic::set_hook(prev);
    panicker.stop();
    assert_eq!(mock::timer_count(), 0);
}

#[test]
fn posted_closures_run_in_order_once_and_nested_posts_wait() {
    init();
    // Send-only closures can reach thread-local state through a channel of plain data
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    for i in 0..5 {
        let s = seen.clone();
        App::post(move || {
            s.lock().unwrap().push(i);
            if i == 2 {
                let s2 = s.clone();
                App::post(move || s2.lock().unwrap().push(100)); // runs on the NEXT drain
            }
        });
    }
    mock::pump();
    assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3, 4]);
    mock::pump();
    assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3, 4, 100]);
    mock::pump();
    assert_eq!(seen.lock().unwrap().len(), 6, "each closure runs exactly once");
    // a panicking post does not stop the rest of the batch
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let s = seen.clone();
    App::post(|| panic!("post boom"));
    App::post(move || s.lock().unwrap().push(7));
    mock::pump();
    std::panic::set_hook(prev);
    assert_eq!(seen.lock().unwrap().last(), Some(&7));
}
