//! Plain API behaviour that the larger suites do not reach: text round trips for every text-bearing
//! widget, defaults, error messages, limits and clamps, deferred model delivery, wake-up
//! coalescing and the initial state a widget is created with.

use crate::backend::mock::{self, widget};
use crate::core::{self, NodeData};
use crate::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn init() {
    let _ = App::new("test");
}

#[test]
fn every_text_bearing_widget_round_trips_its_text() {
    init();
    let win = Window::new("title");
    assert_eq!(win.title(), "title");
    win.set_title("renamed");
    assert_eq!(widget(win.id()).unwrap().text, "renamed");
    let col = VBox::new(win);
    let label = Label::new(col, "l");
    label.set_text("l2");
    assert_eq!(
        (label.text(), widget(label.id()).unwrap().text),
        ("l2".into(), "l2".into())
    );
    let button = Button::new(col, "b");
    button.set_text("b2");
    assert_eq!(button.text(), "b2");
    let check = CheckBox::new(col, "c");
    check.set_text("c2");
    assert_eq!(widget(check.id()).unwrap().text, "c2");
    let group = RadioGroup::default();
    let radio = RadioButton::new(col, &group, "r");
    radio.set_text("r2");
    assert_eq!(widget(radio.id()).unwrap().text, "r2");
    let gb = GroupBox::new(col, "g");
    gb.set_title("g2");
    assert_eq!(widget(gb.id()).unwrap().text, "g2");
    let tabs = Tabs::new(col);
    let page = tabs.add_page("p");
    page.set_title("p2");
    assert_eq!(widget(page.id()).unwrap().text, "p2");
    let bar = MenuBar::new(win);
    let menu = Menu::new(bar, "m");
    let item = MenuItem::new(menu, "i");
    item.set_text("i2");
    assert_eq!(widget(item.id()).unwrap().text, "i2");
    let popup = PopupMenu::default();
    assert!(popup.is_alive());
}

#[test]
fn wrong_kind_handles_are_inert_and_never_reach_the_backend() {
    init();
    let win = Window::new("w");
    let combo = ComboBox::new(win);
    // an accelerator on a combo box, a table call on a label, a value on a button: the mock
    // asserts the core never forwards a property to a kind that does not take it
    MenuItem::from_id(combo.id()).set_accel("Ctrl+S");
    let label = Label::new(win, "x");
    Table::from_id(label.id()).push_row(&["a"]);
    Slider::from_id(label.id()).set_value(5.0);
    TextInput::from_id(label.id()).set_placeholder("p");
    Tabs::from_id(label.id()).set_selected(3);
    App::update();
    let w = widget(label.id()).unwrap();
    assert!(w.rows.is_empty() && w.placeholder.is_empty());
    assert_eq!(widget(combo.id()).unwrap().accel, "");
}

#[test]
fn error_messages_and_small_helpers() {
    for (e, text) in [
        (Error::Unsupported, "not supported"),
        (Error::AlreadyInitialized, "already initialized"),
        (Error::NotInitialized, "not initialized"),
        (Error::InvalidHandle, "invalid or stale"),
        (Error::LimitExceeded, "nesting limit"),
        (Error::Backend("boom".into()), "backend error: boom"),
    ] {
        assert!(e.to_string().contains(text), "{e}");
    }
    assert_eq!(NativeHandle::Gtk(7).ptr(), 7);
    assert_eq!(NativeHandle::Win32(8).ptr(), 8);
    assert_eq!(NativeHandle::Cocoa(9).ptr(), 9);
    let a = Accel::parse("Alt+Option+Opt+Meta+Shift+x").unwrap();
    assert!(a.alt && a.ctrl && a.shift);
    assert_eq!(a.key, "X");
    assert_eq!(Accel::parse(" + "), None);
    // widgets compare, hash and convert to ids
    let win = Window::new("w");
    let w: Widget = *win;
    assert_eq!(WidgetId::from(w), win.id());
    assert_eq!(WidgetId::from(&win), WidgetId::from(win));
}

#[test]
fn image_validity_is_checked_once_in_the_core() {
    init();
    let ok = ImageData {
        w: 2,
        h: 3,
        rgba: vec![0; 24],
    };
    assert!(ok.is_valid());
    for bad in [
        ImageData {
            w: 0,
            h: 3,
            rgba: vec![],
        },
        ImageData {
            w: 2,
            h: 3,
            rgba: vec![0; 23],
        },
        ImageData {
            w: 2,
            h: 3,
            rgba: vec![0; 25],
        },
        ImageData {
            w: u32::MAX,
            h: u32::MAX,
            rgba: vec![0; 4],
        },
        ImageData {
            w: MAX_IMAGE_EDGE + 1,
            h: 1,
            rgba: vec![0; 4],
        },
    ] {
        assert!(!bad.is_valid(), "{}x{}", bad.w, bad.h);
    }
    let win = Window::new("w");
    let img = Image::new(win);
    img.set_image(Some(&ok));
    assert_eq!(widget(img.id()).unwrap().image, Some(ok));
    img.set_image(Some(&ImageData {
        w: u32::MAX,
        h: u32::MAX,
        rgba: vec![0; 4],
    }));
    assert_eq!(
        widget(img.id()).unwrap().image,
        None,
        "an invalid image clears the picture"
    );
}

#[test]
fn nesting_is_limited_and_recovers() {
    init();
    let win = Window::new("deep");
    let mut cur: WidgetId = win.id();
    let mut made = 0;
    while let id @ 1.. = VBox::new(cur).id().0 {
        cur = WidgetId(id);
        made += 1;
        assert!(made <= MAX_NESTING, "the limit never kicked in");
    }
    assert_eq!(made, MAX_NESTING);
    assert_eq!(last_error(), Some(Error::LimitExceeded));
    // layout and destruction of the deepest tree work, and the pieces can be used again
    win.show();
    App::update();
    Label::new(Widget(win.id()), "after");
    assert_eq!(last_error(), None);
    win.destroy();
    assert_eq!(mock::widget_count(), 0);
}

#[test]
fn numbers_are_clamped_before_the_backend_sees_them() {
    init();
    let win = Window::new("w");
    let s = Slider::new(win, 0.0, 10.0);
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -5.0, 5.0] {
        s.set_value(v);
        let pushed = widget(s.id()).unwrap().value;
        assert!(
            pushed.is_finite() && (0.0..=10.0).contains(&pushed),
            "{v} -> {pushed}"
        );
        assert_eq!(s.value(), pushed);
    }
    s.set_range(f64::NAN, f64::INFINITY, f64::NAN);
    let (lo, hi, step) = widget(s.id()).unwrap().range;
    assert!(lo.is_finite() && hi.is_finite() && lo <= hi && step > 0.0);
    // a stale backend index cannot become selection state
    let combo = ComboBox::new(win);
    combo.set_items(&["a", "b"]);
    mock::user(combo.id(), Event::Selected(Some(99)));
    assert_eq!(combo.selected(), None);
    mock::user(combo.id(), Event::Selected(Some(1)));
    assert_eq!(combo.selected(), Some(1));
    let tabs = Tabs::new(win);
    tabs.add_page("one");
    mock::user(tabs.id(), Event::Selected(Some(5)));
    assert_eq!(tabs.selected(), Some(0));
}

#[test]
fn window_sizes_and_columns_are_bounded() {
    init();
    let win = Window::new("w");
    win.set_size(i32::MAX, i32::MIN);
    let (w, h) = win.size();
    assert!((1..=16_384).contains(&w) && (1..=16_384).contains(&h));
    win.set_min_size(i32::MAX, i32::MAX);
    win.show();
    App::update();
    let b = widget(win.id()).unwrap().bounds;
    assert!(b.w <= 16_384 && b.h <= 16_384, "{b:?}");
    let t = Table::new(win);
    let cols: Vec<Column> = (0..MAX_COLUMNS + 10)
        .map(|i| Column::new(&i.to_string()).width(i32::MAX))
        .collect();
    t.set_columns(&cols);
    assert_eq!(t.columns().len(), MAX_COLUMNS);
    assert!(t.columns().iter().all(|c| c.width <= 32_767));
    t.add_column(Column::new("one too many"));
    assert_eq!(t.columns().len(), MAX_COLUMNS);
    t.set_rows(&[vec!["x"]]);
    t.set_cell(0, MAX_COLUMNS + 5, "ignored");
    assert_eq!(t.row(0), vec!["x".to_string()]);
}

#[test]
fn huge_layouts_do_not_overflow() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    for _ in 0..70_000 {
        let b = Spacer::new(col);
        b.set_fixed_size(i32::MAX, i32::MAX);
    }
    col.set_spacing(i32::MAX);
    win.show();
    App::update();
    assert!(widget(win.id()).unwrap().bounds.w <= 16_384);
    win.destroy();
}

#[test]
fn table_and_tree_models_reach_the_backend_at_the_next_loop_turn() {
    init();
    let win = Window::new("w");
    let t = Table::new(win);
    t.set_columns(&[Column::new("a")]);
    for i in 0..500 {
        t.push_row(&[i.to_string()]);
    }
    // the model is the core's state at once...
    assert_eq!(t.row_count(), 500);
    // ...and goes out in one transfer at the next turn (the mock accessor plays the loop)
    assert_eq!(widget(t.id()).unwrap().rows.len(), 500);
    t.batch(|t| {
        t.clear();
        t.push_row(&["only"]);
        // a flush inside a batch leaves the frozen table alone
        App::update();
        assert_eq!(widget(t.id()).unwrap().rows.len(), 500);
    });
    assert_eq!(widget(t.id()).unwrap().rows, vec![vec!["only".to_string()]]);
    let tr = Tree::new(win);
    tr.batch(|tr| {
        let a = tr.add(None, "a");
        tr.add(Some(a), "b");
        // clear inside a batch keeps the batch open (it used to reset the freeze count)
        tr.clear();
        let c = tr.add(None, "c");
        tr.set_selected(Some(c));
        App::update();
        assert!(
            widget(tr.id()).unwrap().tree_rows.is_empty(),
            "still frozen"
        );
    });
    let rows = widget(tr.id()).unwrap().tree_rows;
    assert_eq!(
        rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
        ["c"]
    );
}

#[test]
fn wake_ups_are_coalesced_until_the_loop_runs() {
    init();
    mock::pump();
    let win = Window::new("w");
    let before = mock::wake_count();
    let label = Label::new(win, "x");
    for i in 0..1000 {
        label.set_text(&i.to_string());
        label.set_tooltip("t");
    }
    for _ in 0..100 {
        App::post(|| {});
    }
    let during = mock::wake_count() - before;
    assert!(during <= 2, "{during} wake-ups for one burst");
    mock::pump();
    label.set_text("again");
    assert!(
        mock::wake_count() > before + during,
        "the loop answered, so the next change wakes it again"
    );
}

#[test]
fn radio_groups_stay_exclusive_across_destroy() {
    init();
    let win = Window::new("w");
    let g = RadioGroup::new();
    let a = RadioButton::new(win, &g, "a");
    let b = RadioButton::new(win, &g, "b");
    let c = RadioButton::new(win, &g, "c");
    a.set_checked(true);
    b.set_checked(true);
    assert!(!a.checked() && b.checked());
    b.destroy();
    c.set_checked(true);
    assert!(c.checked() && !a.checked());
    mock::user(a.id(), Event::Toggled(true));
    assert!(
        a.checked() && !c.checked(),
        "a user click unchecks the rest of the group"
    );
    assert!(!widget(c.id()).unwrap().checked);
}

#[test]
fn timer_failure_is_reported_not_swallowed() {
    init();
    mock::fail_next_timer();
    let t = Timer::once(5, || {});
    assert!(matches!(last_error(), Some(Error::Backend(_))));
    t.stop(); // a dead timer is inert
    assert_eq!(mock::timer_count(), 0);
}

#[test]
fn run_pumps_the_mock_loop() {
    init();
    let hit = Arc::new(AtomicBool::new(false));
    let h = hit.clone();
    App::post(move || h.store(true, Ordering::SeqCst));
    App(()).run();
    assert!(hit.load(Ordering::SeqCst));
}
/// Widgets are created with whatever state their constructor's setup closure gave the node:
/// each kind's non-default state must reach the backend before the first property change.
#[test]
fn initial_state_is_pushed_right_after_creation() {
    init();
    let win = Window::new("w");
    let col = VBox::new(win);
    let parent = Some(col.id());
    let rec = Rc::new(RefCell::new(0));
    let make = |kind: Kind, setup: &dyn Fn(&mut core::Node)| -> mock::MockWidget {
        let id = core::create(kind, parent, |n| {
            n.tooltip = "tip".into();
            n.enabled = false;
            setup(n);
        });
        *rec.borrow_mut() += 1;
        let w = widget(id).unwrap_or_else(|| panic!("{kind:?} was not created"));
        assert_eq!(w.tooltip, "tip", "{kind:?}");
        assert!(!w.enabled, "{kind:?}");
        w
    };
    let text = make(Kind::TextInput, &|n| {
        if let NodeData::Text(t) = &mut n.data {
            t.placeholder = "hint".into();
            t.readonly = true;
        }
    });
    assert_eq!((text.placeholder.as_str(), text.readonly), ("hint", true));
    let bar = make(Kind::ProgressBar, &|n| {
        if let NodeData::Range(r) = &mut n.data {
            r.indeterminate = true;
            r.value = 0.5;
        }
    });
    assert!(bar.indeterminate && bar.value == 0.5);
    let list = make(Kind::ListBox, &|n| {
        if let NodeData::List(l) = &mut n.data {
            l.items = vec!["a".into(), "b".into()];
            l.selected = Some(1);
        }
    });
    assert_eq!((list.items.len(), list.selected), (2, Some(1)));
    let check = make(Kind::CheckBox, &|n| {
        if let NodeData::Check(c) = &mut n.data {
            c.checked = true;
        }
    });
    assert!(check.checked);
    let img = ImageData {
        w: 1,
        h: 1,
        rgba: vec![1, 2, 3, 4],
    };
    let image = make(Kind::Image, &|n| {
        n.data = NodeData::Image(Some(img.clone()))
    });
    assert_eq!(image.image.as_ref(), Some(&img));
    // menu items live under a menu
    let menu = Menu::new(MenuBar::new(win), "m");
    let item = core::create(Kind::CheckMenuItem, Some(menu.id()), |n| {
        if let NodeData::MenuItem(m) = &mut n.data {
            m.accel = "Ctrl+K".into();
            m.checked = true;
        }
    });
    let item = widget(item).unwrap();
    assert_eq!((item.accel.as_str(), item.checked), ("Ctrl+K", true));
    assert_eq!(*rec.borrow(), 5);
}

#[test]
fn tree_clear_after_selection_and_expansion_leaves_no_state() {
    init();
    let win = Window::new("w");
    let tr = Tree::new(win);
    let a = tr.add(None, "a");
    let b = tr.add(Some(a), "b");
    tr.set_selected(Some(b));
    tr.expand_all(true);
    tr.clear();
    assert!(tr.is_empty() && tr.selected().is_none());
    assert!(tr.children(None).is_empty());
    let n = Rc::new(Cell::new(0));
    let n2 = n.clone();
    tr.on_select(move |_| n2.set(n2.get() + 1));
    mock::user_tree_select(tr.id(), Some(b.0));
    assert_eq!(n.get(), 0, "events for removed nodes are dropped");
}
