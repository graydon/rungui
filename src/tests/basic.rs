//! Core logic tests against the mock backend (run with plain `cargo test`).

use crate::backend::mock::{self, widget};
use crate::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn init() {
    let _ = App::new("test");
}

fn rect(w: Widget) -> Rect {
    mock::widget(w.id()).unwrap().bounds
}

#[test]
fn creates_native_widgets_with_initial_state() {
    init();
    let win = Window::new("Title");
    let b = Button::new(win, "Go");
    b.set_tooltip("tip");
    let w = widget(b.id()).unwrap();
    assert_eq!(w.text, "Go");
    assert_eq!(w.tooltip, "tip");
    assert_eq!(widget(win.id()).unwrap().text, "Title");
    assert!(!widget(win.id()).unwrap().visible);
    win.show();
    assert!(widget(win.id()).unwrap().visible);
    assert!(b.native_handle().is_some());
}

#[test]
fn stale_handles_are_inert() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "x");
    b.destroy();
    assert!(!b.is_alive());
    b.set_text("y");
    b.set_enabled(false);
    b.on_click(|| {});
    assert_eq!(b.text(), "");
    assert!(b.native_handle().is_none());
    assert!(widget(b.id()).is_none());
    b.destroy();
    let dead = Button::new(Widget(WidgetId::DEAD), "z");
    assert!(!dead.is_alive());
    assert!(last_error().is_some());
    dead.set_text("q");
}

#[test]
fn invalid_parents_and_backend_failure() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "x");
    let bad = Label::new(b, "nope"); // a button cannot hold children
    assert!(!bad.is_alive());
    assert_eq!(last_error(), Some(Error::InvalidHandle));
    let page = Page::from_id(core::create(Kind::Page, Some(win.id()), |_| {}));
    assert!(!page.is_alive()); // pages only live in Tabs
    mock::fail_next_create();
    let l = Label::new(win, "x");
    assert!(!l.is_alive());
    assert!(matches!(last_error(), Some(Error::Backend(_))));
    assert!(widget(win.id()).is_some());
    // failed creation leaves no ghost children behind
    assert_eq!(core::read(win.id(), |n| n.children.len()), Some(1));
}

#[test]
fn click_callback_and_reentrancy() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "x");
    let n = Rc::new(Cell::new(0));
    let n2 = n.clone();
    b.on_click(move || {
        n2.set(n2.get() + 1);
        // all of these re-enter the registry from inside a callback
        b.set_text("clicked");
        let extra = Label::new(win, "new");
        extra.set_text("newer");
        assert!(b.is_alive());
    });
    mock::user_click(b.id());
    mock::user_click(b.id());
    assert_eq!(n.get(), 2);
    assert_eq!(widget(b.id()).unwrap().text, "clicked");
}

#[test]
fn callback_may_replace_or_destroy_itself() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "x");
    let log = Rc::new(RefCell::new(vec![]));
    let l1 = log.clone();
    let l2 = log.clone();
    b.on_click(move || {
        l1.borrow_mut().push("first");
        let l3 = l2.clone();
        b.on_click(move || l3.borrow_mut().push("second"));
    });
    mock::user_click(b.id());
    mock::user_click(b.id());
    assert_eq!(*log.borrow(), vec!["first", "second"]);

    let c = Button::new(win, "y");
    let hit = Rc::new(Cell::new(0));
    let h = hit.clone();
    c.on_click(move || {
        h.set(h.get() + 1);
        c.destroy();
    });
    mock::user_click(c.id());
    mock::user_click(c.id()); // stale: ignored
    assert_eq!(hit.get(), 1);
    assert!(!c.is_alive());
}

#[test]
fn panicking_callback_is_contained() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "x");
    b.on_click(|| panic!("boom"));
    mock::user_click(b.id());
    assert!(b.is_alive());
    b.set_text("still works");
    assert_eq!(b.text(), "still works");
}

#[test]
fn text_and_value_events_mirror_state() {
    init();
    let win = Window::new("w");
    let t = TextInput::new(win);
    let seen = Rc::new(RefCell::new(String::new()));
    let s = seen.clone();
    t.on_change(move |x| *s.borrow_mut() = x.to_string());
    mock::user_text(t.id(), "héllo ✓ 日本語");
    assert_eq!(t.text(), "héllo ✓ 日本語");
    assert_eq!(*seen.borrow(), "héllo ✓ 日本語");
    t.set_text("prog");
    assert_eq!(widget(t.id()).unwrap().text, "prog");

    let sl = Slider::new(win, 0.0, 10.0);
    sl.set_value(99.0);
    assert_eq!(sl.value(), 10.0);
    assert_eq!(widget(sl.id()).unwrap().value, 10.0);
    sl.set_value(f64::NAN);
    assert_eq!(sl.value(), 0.0);
    let vals = Rc::new(RefCell::new(vec![]));
    let v = vals.clone();
    sl.on_change(move |x| v.borrow_mut().push(x));
    mock::user(sl.id(), Event::Value(3.5));
    assert_eq!(*vals.borrow(), vec![3.5]);
    assert_eq!(sl.value(), 3.5);
}

#[test]
fn radio_group_is_exclusive() {
    init();
    let win = Window::new("w");
    let g = RadioGroup::new();
    let g2 = RadioGroup::new();
    let a = RadioButton::new(win, &g, "a");
    let b = RadioButton::new(win, &g, "b");
    let c = RadioButton::new(win, &g2, "c");
    a.set_checked(true);
    c.set_checked(true);
    mock::user(b.id(), Event::Toggled(true));
    assert!(b.checked() && !a.checked() && c.checked());
    assert!(!widget(a.id()).unwrap().checked, "backend told to uncheck");
    b.set_checked(false);
    a.set_checked(true);
    assert!(a.checked() && !b.checked());
    b.set_checked(true);
    assert!(b.checked() && !a.checked());
}

#[test]
fn checkbox_and_combo() {
    init();
    let win = Window::new("w");
    let cb = CheckBox::new(win, "c");
    let got = Rc::new(Cell::new(false));
    let g = got.clone();
    cb.on_toggle(move |v| g.set(v));
    mock::user(cb.id(), Event::Toggled(true));
    assert!(got.get() && cb.checked());

    let combo = ComboBox::new(win);
    combo.set_items(&["a", "b", "c"]);
    combo.set_selected(Some(2));
    assert_eq!(combo.selected_text().as_deref(), Some("c"));
    combo.set_selected(Some(9));
    assert_eq!(combo.selected(), None);
    combo.set_selected(Some(1));
    combo.set_items(&["x"]); // selection no longer valid
    assert_eq!(combo.selected(), None);
    assert_eq!(widget(combo.id()).unwrap().items, vec!["x"]);
}

#[test]
fn vbox_layout_and_window_autosize() {
    init();
    let win = Window::new("w");
    win.set_padding(10);
    let col = VBox::new(win);
    col.set_spacing(4);
    let l = Label::new(col, "abcd"); // 32x16
    let b = Button::new(col, "ok"); // 40x28
    win.show();
    assert_eq!(rect(*l), Rect::new(10, 10, 40, 16)); // fill cross axis to widest child (40)
    assert_eq!(rect(*b), Rect::new(10, 30, 40, 28));
    assert_eq!(win.size(), (60, 68));
    assert_eq!(widget(win.id()).unwrap().bounds, Rect::new(0, 0, 60, 68));
}

#[test]
fn hbox_expand_align_and_resize() {
    init();
    let win = Window::new("w");
    win.set_padding(0);
    win.set_size(300, 100);
    let row = HBox::new(win);
    row.set_spacing(10);
    row.set_expand(1.0); // fill the window vertically
    let a = Label::new(row, "aa"); // 16 wide
    let _sp = Spacer::new(row); // expand 1
    let b = Button::new(row, "b"); // 32 wide
    b.set_expand(3.0);
    a.set_align(Align::Center);
    win.show();
    // total fixed = 16 + 0 + 32 + 2*10 = 68; extra 232 split 1:3 between spacer and button
    assert_eq!(rect(*a).w, 16);
    assert_eq!(rect(*b).x, 16 + 10 + 58 + 10);
    assert_eq!(rect(*b).w, 32 + 174);
    assert_eq!(rect(*b).x + rect(*b).w, 300);
    assert_eq!(rect(*a).y, (100 - 16) / 2);
    assert_eq!(rect(*b).h, 100);

    mock::resize_window(win.id(), 200, 60);
    assert_eq!(rect(*b).x + rect(*b).w, 200);
    assert_eq!(rect(*b).h, 60);
    assert_eq!(win.size(), (200, 60));
}

#[test]
fn layout_only_pushes_changes() {
    init();
    let win = Window::new("w");
    let b = Button::new(win, "x");
    win.show();
    let n = widget(b.id()).unwrap().bounds_pushes;
    App::update();
    win.set_title("other"); // does not affect layout
    App::update();
    assert_eq!(widget(b.id()).unwrap().bounds_pushes, n);
    b.set_text("a much longer text");
    App::update();
    assert!(widget(b.id()).unwrap().bounds_pushes > n);
}

#[test]
fn hidden_children_take_no_space_and_visibility_propagates() {
    init();
    let win = Window::new("w");
    win.set_padding(0);
    let col = VBox::new(win);
    col.set_spacing(0);
    let a = Label::new(col, "a");
    let inner = HBox::new(col);
    let b = Button::new(inner, "b");
    let c = Label::new(col, "c");
    win.show();
    let y_c = rect(*c).y;
    inner.set_visible(false);
    App::update();
    assert!(
        !widget(b.id()).unwrap().visible,
        "native child hidden through virtual box"
    );
    assert_eq!(rect(*c).y, y_c - 28);
    inner.set_visible(true);
    b.set_visible(false);
    inner.set_visible(true);
    assert!(!widget(b.id()).unwrap().visible, "own flag still wins");
    inner.set_enabled(false);
    assert!(!widget(b.id()).unwrap().enabled);
    inner.set_enabled(true);
    assert!(widget(b.id()).unwrap().enabled);
    let _ = a;
}

#[test]
fn grid_layout() {
    init();
    let win = Window::new("w");
    win.set_padding(0);
    let g = Grid::new(win, 2);
    g.set_spacing(2);
    let l1 = Label::new(g, "name"); // 32x16
    let t1 = TextInput::new(g); // 160x24
    let l2 = Label::new(g, "x"); // 8x16
    let t2 = TextInput::new(g);
    t1.set_expand(1.0);
    win.set_size(300, 100);
    win.show();
    assert_eq!(rect(*l1), Rect::new(0, 0, 32, 24));
    assert_eq!(rect(*t1), Rect::new(34, 0, 266, 24));
    assert_eq!(rect(*l2), Rect::new(0, 26, 32, 24));
    assert_eq!(rect(*t2).y, 26);
    // explicit cell with span
    let b = Button::new(g, "wide");
    g.place(b, 0, 2, 2, 1);
    App::update();
    assert_eq!(rect(*b), Rect::new(0, 52, 300, 28));
}

#[test]
fn tabs_group_and_nesting() {
    init();
    let win = Window::new("w");
    let tabs = Tabs::new(win);
    let p1 = tabs.add_page("One");
    let p2 = tabs.add_page("Two");
    let b1 = Button::new(p1, "in one");
    let _b2 = Button::new(p2, "in two");
    assert_eq!(tabs.selected(), Some(0));
    win.show();
    assert_eq!(widget(p1.id()).unwrap().text, "One");
    assert_eq!(widget(p1.id()).unwrap().parent, Some(tabs.id()));
    assert_eq!(widget(b1.id()).unwrap().parent, Some(p1.id()));
    // page area = tabs bounds minus mock chrome (4,28)
    let tb = rect(*tabs);
    assert_eq!(rect(*p1), Rect::new(0, 0, tb.w - 4, tb.h - 28));
    let sel = Rc::new(Cell::new(None));
    let s = sel.clone();
    tabs.on_select(move |i| s.set(i));
    mock::user(tabs.id(), Event::Selected(Some(1)));
    assert_eq!(sel.get(), Some(1));
    assert_eq!(tabs.selected(), Some(1));
    tabs.set_selected(5); // out of range ignored
    assert_eq!(tabs.selected(), Some(1));
    // group box nests with chrome
    let gb = GroupBox::new(p1, "grp");
    let inner = Label::new(gb, "hi");
    App::update();
    assert!(rect(*inner).w > 0);
    assert!(rect(*gb).w >= rect(*inner).w + 12);
}

#[test]
fn destroy_removes_subtree_deepest_first() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    let a = Button::new(col, "a");
    let gb = GroupBox::new(col, "g");
    let c = Label::new(gb, "c");
    let before = mock::widget_count();
    col.destroy();
    assert!(!a.is_alive() && !c.is_alive() && !gb.is_alive() && !col.is_alive());
    assert_eq!(mock::widget_count(), before - 3); // a, gb, c (box is virtual)
    assert_eq!(core::read(win.id(), |n| n.children.len()), Some(0));
}

#[test]
fn window_close_veto_and_quit() {
    init();
    let win = Window::new("w");
    let allow = Rc::new(Cell::new(false));
    let a = allow.clone();
    win.on_close(move || a.get());
    mock::user_close(win.id());
    mock::pump();
    assert!(win.is_alive());
    allow.set(true);
    mock::user_close(win.id());
    assert!(win.is_alive(), "destroy is deferred to the next loop turn");
    mock::pump();
    assert!(!win.is_alive());
    assert!(widget(win.id()).is_none());
}

#[test]
fn post_from_other_thread_runs_on_main() {
    init();
    let win = Window::new("w");
    let l = Label::new(win, "before");
    let main = std::thread::current().id();
    let t = std::thread::spawn(move || {
        // handle use off the UI thread is a no-op, not a panic
        l.set_text("wrong thread");
        assert!(!l.is_alive());
        App::post(move || {
            assert_ne!(std::thread::current().id(), main);
        });
    });
    t.join().unwrap();
    let id = l.id();
    App::post(move || Label::from_id(id).set_text("after"));
    mock::pump();
    assert_eq!(l.text(), "after");
}

#[test]
fn timers() {
    init();
    let n = Rc::new(Cell::new(0));
    let once = Rc::new(Cell::new(0));
    let n2 = n.clone();
    let o2 = once.clone();
    let t = Timer::every(100, move || n2.set(n2.get() + 1));
    Timer::once(150, move || o2.set(o2.get() + 1));
    mock::advance(100);
    mock::advance(100);
    assert_eq!((n.get(), once.get()), (2, 1));
    mock::advance(1000);
    assert_eq!(once.get(), 1);
    t.stop();
    let before = n.get();
    mock::advance(1000);
    assert_eq!(n.get(), before);
    assert_eq!(mock::timer_count(), 0);
}

#[test]
fn timer_callback_can_stop_itself() {
    init();
    let n = Rc::new(Cell::new(0));
    let slot: Rc<Cell<Option<Timer>>> = Rc::new(Cell::new(None));
    let (n2, s2) = (n.clone(), slot.clone());
    let t = Timer::every(10, move || {
        n2.set(n2.get() + 1);
        if let Some(t) = s2.get() {
            t.stop();
        }
    });
    slot.set(Some(t));
    mock::advance(10);
    mock::advance(10);
    assert_eq!(n.get(), 1);
}

#[test]
fn menus() {
    init();
    let win = Window::new("w");
    let mb = MenuBar::new(win);
    let file = Menu::new(mb, "File");
    let open = MenuItem::new(file, "Open");
    open.set_accel("Ctrl+O");
    let sub = Menu::new(file, "Recent");
    let _it = MenuItem::new(sub, "a.txt");
    MenuSeparator::new(file);
    let wrap = CheckMenuItem::new(file, "Wrap");
    let hits = Rc::new(Cell::new(0));
    let h = hits.clone();
    open.on_click(move || h.set(h.get() + 1));
    let state = Rc::new(Cell::new(false));
    let s = state.clone();
    wrap.on_toggle(move |v| s.set(v));
    mock::user_click(open.id());
    mock::user(wrap.id(), Event::Toggled(true));
    assert_eq!((hits.get(), state.get(), wrap.checked()), (1, true, true));
    assert_eq!(widget(open.id()).unwrap().accel, "Ctrl+O");
    assert_eq!(widget(sub.id()).unwrap().parent, Some(file.id()));
    // menus are invisible to layout
    win.show();
    assert_eq!(widget(win.id()).unwrap().bounds, Rect::new(0, 0, 20, 20));
    // wrong nesting is rejected
    assert!(!MenuItem::new(win, "bad").is_alive());
    assert!(!Menu::new(win, "bad").is_alive());
}

#[test]
fn dialogs_route_through_backend() {
    init();
    let win = Window::new("w");
    mock::queue_answer(Answer::Yes);
    let a = message_box(
        Some(win),
        MessageKind::Question,
        Buttons::YesNo,
        "T",
        "Sure?",
    );
    assert_eq!(a, Answer::Yes);
    assert_eq!(mock::last_message().unwrap().text, "Sure?");
    mock::queue_files(&["/tmp/a.txt"]);
    let f = FileDialog::new()
        .title("Open")
        .filter("Text", &["txt"])
        .open(Some(win));
    assert_eq!(f, Some(std::path::PathBuf::from("/tmp/a.txt")));
    let spec = mock::last_file_spec().unwrap();
    assert_eq!(spec.filters[0].1, vec!["txt"]);
    assert_eq!(FileDialog::new().save(None), None); // cancelled
}

#[test]
fn accel_parsing() {
    let a = Accel::parse("Ctrl+Shift+s").unwrap();
    assert!(a.ctrl && a.shift && !a.alt);
    assert_eq!(a.key, "S");
    assert!(Accel::parse("Cmd+Q").unwrap().ctrl);
    assert_eq!(Accel::parse("F5").unwrap().key, "F5");
    assert_eq!(Accel::parse(""), None);
    assert_eq!(Accel::parse("Ctrl+"), None);
}

/// The resolved a11y metadata of `id` in `win` (None if AT cannot see it).
fn a11y_of(win: &Window, id: WidgetId) -> Option<a11y::Resolved> {
    a11y::resolve(win.id())
        .unwrap()
        .into_iter()
        .find(|n| n.id == id)
}

#[test]
fn a11y_names_roles_and_descriptions() {
    init();
    let win = Window::new("Settings");
    let form = Grid::new(win, 2);
    let _l = Label::new(form, "Name");
    let name = TextInput::new(form);
    let cb = CheckBox::new(form, "Enable");
    let pw = TextInput::password(form);
    pw.set_text("secret");
    pw.set_a11y_name("Password");
    let ok = Button::new(win, "OK");
    ok.set_a11y_description("Confirm");
    let tip = Button::new(win, "Tip");
    tip.set_tooltip("Hover text");
    let list = ListBox::new(win);
    win.show();
    App::update();

    let all = a11y::resolve(win.id()).unwrap();
    assert_eq!(all[0].id, win.id(), "the window comes first");
    let get = |id: WidgetId| a11y_of(&win, id).unwrap();
    assert_eq!(
        (get(win.id()).role, get(win.id()).name.as_deref()),
        (A11yRole::Window, Some("Settings"))
    );
    assert_eq!(
        (get(ok.id()).role, get(ok.id()).name.as_deref()),
        (A11yRole::Button, Some("OK"))
    );
    assert_eq!(get(ok.id()).description.as_deref(), Some("Confirm"));
    assert_eq!(
        get(tip.id()).description.as_deref(),
        Some("Hover text"),
        "tooltip is the fallback description"
    );
    assert_eq!(get(cb.id()).role, A11yRole::CheckBox);
    // an unnamed input is labelled by the preceding Label; the virtual Grid is flattened away
    assert_eq!(get(name.id()).name.as_deref(), Some("Name"));
    assert_eq!(get(name.id()).name_source, a11y::NameSource::PrecedingLabel);
    assert!(
        all.iter().all(|n| n.kind.is_native()),
        "layout boxes are not reported"
    );
    // an explicit name wins, and the role follows the kind
    assert_eq!(get(pw.id()).name.as_deref(), Some("Password"));
    assert_eq!(get(pw.id()).name_source, a11y::NameSource::Explicit);
    assert_eq!(get(pw.id()).role, A11yRole::PasswordInput);
    assert_eq!(get(list.id()).role, A11yRole::ListBox);
    // widgets are reported once each, in layout order
    let ids: Vec<_> = all.iter().map(|n| n.id).collect();
    let mut dedup = ids.clone();
    dedup.sort_by_key(|i| i.0);
    dedup.dedup();
    assert_eq!(dedup.len(), ids.len());
    let pos = |id: WidgetId| ids.iter().position(|i| *i == id).unwrap();
    assert!(pos(name.id()) < pos(cb.id()) && pos(cb.id()) < pos(ok.id()));
    // overrides and hidden subtrees
    ok.set_a11y_role(A11yRole::Link);
    assert_eq!(a11y_of(&win, ok.id()).unwrap().role, A11yRole::Link);
    assert!(a11y_of(&win, ok.id()).unwrap().role_explicit);
    ok.set_visible(false);
    App::update();
    assert!(a11y_of(&win, ok.id()).is_none());
    assert!(
        a11y::resolve(ok.id()).is_none(),
        "only windows can be resolved"
    );
}

#[test]
fn second_init_fails_and_handles_are_send() {
    init();
    assert_eq!(App::new("again").err(), Some(Error::AlreadyInitialized));
    fn is_send<T: Send + Sync + Copy>() {}
    is_send::<Button>();
    is_send::<Window>();
    is_send::<WidgetId>();
}
