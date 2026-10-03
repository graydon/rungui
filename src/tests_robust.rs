//! Robustness, unicode, a11y-plumbing and RTL tests against the mock backend.

use crate::backend::mock::{self, widget};
use crate::*;
use accesskit::{Action, ActionData, ActionRequest, NodeId, Role, TreeId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn init() {
    let _ = App::new("test");
}

fn req(a: Action, id: WidgetId, data: Option<ActionData>) -> ActionRequest {
    ActionRequest { action: a, target_tree: TreeId::ROOT, target_node: NodeId(id.0), data }
}

fn rect(w: impl Into<WidgetId>) -> Rect {
    widget(w.into()).unwrap().bounds
}

#[test]
fn callback_destroys_its_own_window_and_siblings() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    let a = Button::new(col, "a");
    let b = Button::new(col, "b");
    let l = Label::new(col, "l");
    win.show();
    App::update();
    let ran = Rc::new(Cell::new(0));
    let r = ran.clone();
    a.on_click(move || {
        r.set(r.get() + 1);
        b.destroy();
        l.set_text("after");
        win.destroy(); // destroys the widget currently running this callback
        a.set_text("still safe");
        win.set_title("nothing");
    });
    mock::user_click(a.id());
    App::update();
    assert_eq!(ran.get(), 1);
    assert!(!a.is_alive() && !win.is_alive() && !col.is_alive() && !l.is_alive());
    assert_eq!(mock::widget_count(), 0);
    mock::user_click(a.id()); // stale event from the backend: ignored
    assert_eq!(ran.get(), 1);
}

#[test]
fn set_text_from_own_change_callback_does_not_loop_or_panic() {
    init();
    let win = Window::new("w");
    let t = TextInput::new(win);
    let n = Rc::new(Cell::new(0));
    let n2 = n.clone();
    t.on_change(move |s| {
        n2.set(n2.get() + 1);
        // normalise: force upper case from inside the callback
        let up = s.to_uppercase();
        if up != s {
            t.set_text(&up);
        }
        assert_eq!(t.text(), up.as_str(), "state readable inside callback");
    });
    mock::user_text(t.id(), "héllo ß");
    assert_eq!(n.get(), 1, "programmatic set_text must not fire on_change");
    assert_eq!(t.text(), "HÉLLO SS");
    assert_eq!(widget(t.id()).unwrap().text, "HÉLLO SS");
}

#[test]
fn callback_creates_and_destroys_while_events_fire() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    let b = Button::new(col, "add");
    let made = Rc::new(RefCell::new(Vec::<Button>::new()));
    let m = made.clone();
    b.on_click(move || {
        let nb = Button::new(col, "new");
        let m2 = m.clone();
        nb.on_click(move || {
            for x in m2.borrow().iter() {
                x.destroy();
            }
        });
        m.borrow_mut().push(nb);
    });
    mock::user_click(b.id());
    mock::user_click(b.id());
    assert_eq!(made.borrow().len(), 2);
    let first = made.borrow()[0];
    mock::user_click(first.id());
    assert!(made.borrow().iter().all(|x| !x.is_alive()));
    assert!(b.is_alive());
}

#[test]
fn panic_in_every_callback_kind_is_contained() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "b");
    let t = TextInput::new(win);
    let c = CheckBox::new(win, "c");
    let s = Slider::new(win, 0.0, 1.0);
    b.on_click(|| panic!("boom"));
    t.on_change(|_| panic!("boom"));
    c.on_toggle(|_| panic!("boom"));
    s.on_change(|_| panic!("boom"));
    win.on_close(|| panic!("boom"));
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    mock::user_click(b.id());
    mock::user_text(t.id(), "x");
    mock::user(c.id(), Event::Toggled(true));
    mock::user(s.id(), Event::Value(0.5));
    mock::user_close(win.id());
    std::panic::set_hook(prev);
    assert_eq!(t.text(), "x");
    assert!(c.checked());
    assert_eq!(s.value(), 0.5);
    // widgets remain usable
    b.set_text("after");
    assert_eq!(widget(b.id()).unwrap().text, "after");
}

#[test]
fn off_thread_misuse_is_an_error_not_a_panic() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "b");
    b.set_text("main");
    let before = mock::widget_count();
    std::thread::spawn(move || {
        // no registry on this thread: everything is inert
        b.set_text("other");
        b.set_enabled(false);
        b.destroy();
        b.on_click(|| {});
        win.show();
        assert_eq!(b.text(), "");
        assert!(!b.is_alive());
        assert!(b.native_handle().is_none());
        let nb = Button::new(win, "x");
        assert!(!nb.is_alive());
        assert!(matches!(last_error(), Some(Error::NotInitialized) | Some(Error::InvalidHandle)));
        let t = Timer::once(1, || {});
        let _ = t;
        t.stop();
        assert_eq!(App::new("again").is_ok(), true, "a different thread gets its own toolkit state");
    })
    .join()
    .unwrap();
    assert_eq!(widget(b.id()).unwrap().text, "main");
    assert!(widget(b.id()).unwrap().enabled);
    assert_eq!(mock::widget_count(), before);
}

#[test]
fn post_and_quit_are_the_thread_safe_entry_points() {
    init();
    let win = Window::new("w");
    let l = Label::new(win, "a");
    let ui = std::thread::current().id();
    std::thread::spawn(move || {
        core::post_to(ui, move || l.set_text("posted"));
    })
    .join()
    .unwrap();
    mock::pump();
    assert_eq!(l.text(), "posted");
}

#[test]
fn stale_ids_for_every_widget_family() {
    init();
    let win = Window::new("w");
    let g = RadioGroup::new();
    let r = RadioButton::new(win, &g, "r");
    let cb = ComboBox::new(win);
    cb.set_items(&["a", "b"]);
    let tabs = Tabs::new(win);
    let page = tabs.add_page("p");
    let inner = Button::new(page, "in");
    win.destroy();
    r.set_checked(true);
    assert!(!r.checked());
    cb.set_selected(Some(1));
    assert_eq!(cb.selected(), None);
    assert_eq!(cb.items(), Vec::<String>::new());
    tabs.set_selected(0);
    page.set_title("x");
    inner.set_text("x");
    inner.focus();
    assert_eq!(inner.bounds(), Rect::default());
    let b2 = Button::new(inner, "x");
    assert!(!b2.is_alive());
    assert_eq!(mock::widget_count(), 0);
}

#[test]
fn unicode_text_survives_round_trip_verbatim() {
    init();
    let win = Window::new("日本語 — окно 🪟");
    let col = VBox::new(win);
    let samples = ["שלום עולם", "مرحبا بالعالم", "日本語のテキスト", "👨‍👩‍👧‍👦 e\u{301}", "a\u{200F}b", "tab\there\nnewline"];
    let t = TextArea::new(col);
    let l = Label::new(col, "");
    for s in samples {
        t.set_text(s);
        l.set_text(s);
        assert_eq!(t.text(), s);
        assert_eq!(widget(t.id()).unwrap().text, s);
        assert_eq!(widget(l.id()).unwrap().text, s);
        mock::user_text(t.id(), s);
        assert_eq!(t.text(), s);
    }
    assert_eq!(win.title(), "日本語 — окно 🪟");
    assert_eq!(widget(win.id()).unwrap().text, "日本語 — окно 🪟");
}

#[test]
fn a11y_names_strip_mnemonics_and_isolate_labels() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "&Save && close");
    let l = Label::new(win, "&Name");
    let t = TextInput::new(win);
    let mb = MenuBar::new(win);
    let m = Menu::new(mb, "&File");
    let mi = MenuItem::new(m, "E&xit");
    let tree = a11y::tree_for_window(win.id()).unwrap();
    let get = |id: WidgetId| tree.nodes.iter().find(|(i, _)| i.0 == id.0).map(|(_, n)| n).unwrap();
    assert_eq!(get(b.id()).label(), Some("Save & close"));
    assert_eq!(get(t.id()).label(), Some("Name"));
    assert_eq!(get(m.id()).label(), Some("File"));
    assert_eq!(get(mi.id()).label(), Some("Exit"));
    let _ = l;
}

#[test]
fn a11y_actions_respect_disabled_hidden_and_bounds() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    let b = Button::new(col, "b");
    let list = ListBox::new(col);
    list.set_items(&["x", "y"]);
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    b.on_click(move || c.set(c.get() + 1));
    let sel = Rc::new(Cell::new(None));
    let s2 = sel.clone();
    list.on_select(move |i| s2.set(i));
    b.set_enabled(false);
    a11y::do_action(win.id(), &req(Action::Click, b.id(), None));
    assert_eq!(clicks.get(), 0, "disabled");
    b.set_enabled(true);
    col.set_enabled(false);
    a11y::do_action(win.id(), &req(Action::Click, b.id(), None));
    assert_eq!(clicks.get(), 0, "disabled ancestor");
    col.set_enabled(true);
    b.set_visible(false);
    a11y::do_action(win.id(), &req(Action::Click, b.id(), None));
    assert_eq!(clicks.get(), 0, "hidden");
    b.set_visible(true);
    a11y::do_action(win.id(), &req(Action::Click, b.id(), None));
    assert_eq!(clicks.get(), 1);
    // list option ids come from the tree
    let tree = a11y::tree_for_window(win.id()).unwrap();
    let opts: Vec<NodeId> = tree.nodes.iter().filter(|(_, n)| n.role() == Role::ListBoxOption).map(|(i, _)| *i).collect();
    assert_eq!(opts.len(), 2);
    let click_opt = |i: usize| a11y::do_action(win.id(), &ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: opts[i], data: None });
    click_opt(1);
    assert_eq!(sel.get(), Some(1));
    assert_eq!(list.selected(), Some(1));
    // synthetic id past the end of the list
    let bogus = NodeId(opts[1].0 + 5);
    a11y::do_action(win.id(), &ActionRequest { action: Action::Click, target_tree: TreeId::ROOT, target_node: bogus, data: None });
    assert_eq!(list.selected(), Some(1));
    // garbage ids and NaN values are ignored
    a11y::do_action(win.id(), &req(Action::Click, WidgetId(u64::MAX), None));
    a11y::do_action(win.id(), &req(Action::Click, WidgetId(0), None));
    let s = Slider::new(win, 0.0, 10.0);
    a11y::do_action(win.id(), &req(Action::SetValue, s.id(), Some(ActionData::NumericValue(f64::NAN))));
    assert_eq!(s.value(), 0.0);
    a11y::do_action(win.id(), &req(Action::SetValue, s.id(), Some(ActionData::NumericValue(99.0))));
    assert_eq!(s.value(), 10.0, "clamped");
    // wrong data type for the action
    let t = TextInput::new(win);
    a11y::do_action(win.id(), &req(Action::SetValue, t.id(), Some(ActionData::NumericValue(1.0))));
    assert_eq!(t.text(), "");
    a11y::do_action(win.id(), &req(Action::SetValue, t.id(), Some(ActionData::Value("a\0b".into()))));
    assert_eq!(t.text(), "a\u{FFFD}b");
}

#[test]
fn a11y_radio_click_is_exclusive_and_idempotent() {
    init();
    let win = Window::new("w");
    let g = RadioGroup::new();
    let r1 = RadioButton::new(win, &g, "1");
    let r2 = RadioButton::new(win, &g, "2");
    let fired = Rc::new(Cell::new(0));
    let f = fired.clone();
    r2.on_toggle(move |_| f.set(f.get() + 1));
    a11y::do_action(win.id(), &req(Action::Click, r1.id(), None));
    a11y::do_action(win.id(), &req(Action::Click, r2.id(), None));
    a11y::do_action(win.id(), &req(Action::Click, r2.id(), None));
    assert!(!r1.checked() && r2.checked());
    assert_eq!(fired.get(), 1);
    assert!(!widget(r1.id()).unwrap().checked);
}

#[test]
fn a11y_cache_and_post_action_cross_thread() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "b");
    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    b.on_click(move || c.set(c.get() + 1));
    assert!(a11y::latest_tree(win.id()).is_none());
    a11y::enable_cache(win.id());
    let (wid, bid) = (win.id(), b.id());
    let ui = std::thread::current().id();
    let seen = std::thread::spawn(move || {
        let t = a11y::latest_tree(wid).expect("cached");
        let found = t.nodes.iter().any(|(i, _)| i.0 == bid.0);
        core::post_to(ui, move || a11y::do_action(wid, &req(Action::Click, bid, None)));
        found
    })
    .join()
    .unwrap();
    assert!(seen);
    mock::pump();
    assert_eq!(clicks.get(), 1);
    // cache follows changes
    b.set_text("renamed");
    App::update();
    let t = a11y::latest_tree(win.id()).unwrap();
    assert_eq!(t.nodes.iter().find(|(i, _)| i.0 == b.id().0).unwrap().1.label(), Some("renamed"));
    win.destroy();
    assert!(a11y::latest_tree(wid).is_none());
}

#[test]
fn rtl_layout_mirrors_hbox_vbox_and_grid() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    let row = HBox::new(col);
    row.set_spacing(10);
    let a = Label::new(row, "aaaa");
    let b = Label::new(row, "bb");
    let v = Button::new(col, "v");
    v.set_align(Align::Start);
    let g = Grid::new(col, 2);
    let g1 = Label::new(g, "1");
    let g2 = Label::new(g, "2");
    win.set_padding(0);
    win.set_size(300, 200);
    win.show();
    App::update();
    let (a0, b0, v0, g10, g20) = (rect(a), rect(b), rect(v), rect(g1), rect(g2));
    assert!(a0.x < b0.x && v0.x == 0 && g10.x < g20.x, "{a0:?} {b0:?} {v0:?} {g10:?} {g20:?}");
    set_rtl_layout(true);
    App::update();
    let (a1, b1, v1, g11, g21) = (rect(a), rect(b), rect(v), rect(g1), rect(g2));
    assert!(b1.x < a1.x, "hbox order reversed");
    assert_eq!(a1.x + a1.w, 300, "flush with the right edge");
    assert_eq!(v1.x + v1.w, 300, "Start alignment becomes the right edge");
    assert!(g21.x < g11.x, "grid columns reversed");
    assert_eq!((a1.w, a1.h), (a0.w, a0.h), "sizes unchanged");
    set_rtl_layout(false);
    App::update();
    assert_eq!(rect(a), a0);
    assert_eq!(rect(g2), g20);
}

#[test]
fn layout_with_degenerate_inputs_does_not_panic() {
    init();
    let win = Window::new("w");
    let g = Grid::new(win, 0); // zero columns
    for i in 0..3 {
        let l = Label::new(g, "x");
        l.set_cell(7 + i, 100, 0, 0); // far cells, zero spans
    }
    let h = HBox::new(win);
    h.set_expand(f32::NAN);
    h.set_spacing(-5);
    h.set_padding(i32::MAX / 4);
    let l = Label::new(h, "x");
    l.set_min_size(-10, -10);
    l.set_expand(-1.0);
    win.set_size(0, 0);
    win.show();
    App::update();
    mock::resize_window(win.id(), 1, 1);
    mock::resize_window(win.id(), -5, -5);
    App::update();
    assert!(win.is_alive());
}

#[test]
fn text_inputs_with_embedded_nul_keep_state_consistent() {
    init();
    let win = Window::new("w");
    let t = TextInput::new(win);
    t.set_text("a\0b");
    assert_eq!(t.text(), widget(t.id()).unwrap().text.as_str());
}
