//! Multi-select, `TextInput::on_activate`, `Window::on_cancel`, modal windows and `Prompt`.

use crate::backend::mock::{self, widget};
use crate::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn init() {
    let _ = App::new("test");
}

fn list(n: usize) -> (Window, ListBox) {
    init();
    let win = Window::new("w");
    let l = ListBox::new(win);
    let items: Vec<String> = (0..n).map(|i| format!("item {i}")).collect();
    l.set_items(&items);
    (win, l)
}

fn table(n: usize) -> (Window, Table) {
    init();
    let win = Window::new("w");
    let t = Table::new(win);
    t.set_columns(&[Column::new("Name")]);
    for i in 0..n {
        t.push_row(&[format!("row {i}")]);
    }
    (win, t)
}

#[test]
fn listbox_single_mode_is_unchanged() {
    let (_w, l) = list(5);
    assert!(!l.multi_select());
    l.set_selected(Some(2));
    assert_eq!(l.selection(), vec![2]);
    assert_eq!(widget(l.id()).unwrap().selected, Some(2));
    // a single-mode widget keeps one row even when asked for several
    l.set_selection(&[4, 1, 3]);
    assert_eq!(l.selection(), vec![1]);
    assert_eq!(l.selected(), Some(1));
}

#[test]
fn listbox_multi_select_mirrors_and_reports() {
    let (_w, l) = list(6);
    l.set_selected(Some(3));
    l.set_multi_select(true);
    let w = widget(l.id()).unwrap();
    assert!(w.multi_select);
    assert_eq!(w.selection, vec![3]);

    let seen: Rc<RefCell<Vec<Vec<usize>>>> = Rc::default();
    let firsts: Rc<RefCell<Vec<Option<usize>>>> = Rc::default();
    let (s2, f2) = (seen.clone(), firsts.clone());
    l.on_selection(move |v| s2.borrow_mut().push(v.to_vec()));
    l.on_select(move |i| f2.borrow_mut().push(i));

    // programmatic changes fire nothing; out-of-range, duplicates and order are normalised
    l.set_selection(&[5, 1, 1, 9]);
    assert_eq!(l.selection(), vec![1, 5]);
    assert_eq!(widget(l.id()).unwrap().selection, vec![1, 5]);
    assert!(seen.borrow().is_empty());

    mock::user_select_rows(l.id(), &[0, 2, 4]);
    assert_eq!(l.selection(), vec![0, 2, 4]);
    assert_eq!(l.selected(), Some(0));
    assert_eq!(l.selected_texts(), vec!["item 0", "item 2", "item 4"]);
    assert_eq!(*seen.borrow(), vec![vec![0, 2, 4]]);
    assert_eq!(*firsts.borrow(), vec![Some(0)]);

    // a stale backend index never becomes state
    mock::user_select_rows(l.id(), &[1, 99]);
    assert_eq!(l.selection(), vec![0, 2, 4]);

    mock::user_select_rows(l.id(), &[]);
    assert_eq!(l.selection(), Vec::<usize>::new());
    assert_eq!(*firsts.borrow(), vec![Some(0), None]);

    // set_selected selects that one alone, and shrinking the list drops the lost rows
    l.set_selection(&[1, 5]);
    l.set_selected(Some(2));
    assert_eq!(widget(l.id()).unwrap().selection, vec![2]);
    l.set_selection(&[0, 3, 5]);
    l.set_items(&["a", "b", "c", "d"]);
    assert_eq!(l.selection(), vec![0, 3]);
    assert_eq!(widget(l.id()).unwrap().selection, vec![0, 3]);

    // leaving multi-select keeps the first row and tells the backend
    l.set_multi_select(false);
    assert_eq!(l.selection(), vec![0]);
    let w = widget(l.id()).unwrap();
    assert!(!w.multi_select);
    assert_eq!(w.selected, Some(0));
}

#[test]
fn single_mode_events_feed_both_callbacks() {
    let (_w, l) = list(3);
    let n = Rc::new(Cell::new(0));
    let sets: Rc<RefCell<Vec<Vec<usize>>>> = Rc::default();
    let (n2, s2) = (n.clone(), sets.clone());
    l.on_select(move |_| n2.set(n2.get() + 1));
    l.on_selection(move |v| s2.borrow_mut().push(v.to_vec()));
    mock::user_select_row(l.id(), Some(1));
    mock::user_select_row(l.id(), None);
    assert_eq!(n.get(), 2);
    assert_eq!(*sets.borrow(), vec![vec![1], vec![]]);
}

#[test]
fn table_multi_select_follows_row_edits() {
    let (_w, t) = table(5);
    t.set_multi_select(true);
    assert!(widget(t.id()).unwrap().multi_select);
    t.set_selection(&[1, 3, 4]);
    assert_eq!(widget(t.id()).unwrap().selection, vec![1, 3, 4]);

    t.insert_row(2, &["new"]);
    assert_eq!(t.selection(), vec![1, 4, 5]);
    t.remove_row(4);
    assert_eq!(t.selection(), vec![1, 4]);
    assert_eq!(widget(t.id()).unwrap().selection, vec![1, 4]);
    assert_eq!(
        t.selected_rows(),
        vec![vec!["row 1".to_string()], vec!["row 4".to_string()]]
    );

    // new rows reach the backend together with the whole selection
    t.set_rows(&[vec!["a"], vec!["b"]]);
    assert_eq!(t.selection(), vec![1]);
    assert_eq!(widget(t.id()).unwrap().selection, vec![1]);

    let seen: Rc<RefCell<Vec<Vec<usize>>>> = Rc::default();
    let s2 = seen.clone();
    t.on_selection(move |v| s2.borrow_mut().push(v.to_vec()));
    mock::user_select_rows(t.id(), &[0, 1]);
    assert_eq!(*seen.borrow(), vec![vec![0, 1]]);
    t.clear();
    assert!(t.selection().is_empty());
    assert!(widget(t.id()).unwrap().selection.is_empty());
}

#[test]
fn text_input_on_activate_and_window_on_cancel() {
    init();
    let win = Window::new("w");
    let f = TextInput::new(win);
    let enters = Rc::new(Cell::new(0));
    let e2 = enters.clone();
    f.on_activate(move || e2.set(e2.get() + 1));
    mock::user_text(f.id(), "x");
    assert_eq!(enters.get(), 0);
    mock::user_submit(f.id());
    assert_eq!(enters.get(), 1);
    assert_eq!(f.text(), "x");

    let esc = Rc::new(Cell::new(0));
    let c2 = esc.clone();
    win.on_cancel(move || c2.set(c2.get() + 1));
    mock::user_cancel(win.id());
    assert_eq!(esc.get(), 1);
    // nobody listening: ignored
    let other = Window::new("o");
    mock::user_cancel(other.id());
}

#[test]
fn run_modal_shows_the_window_and_returns() {
    init();
    let parent = Window::new("p");
    let dlg = Window::new("d");
    let b = Button::new(dlg, "Close");
    b.on_click(move || dlg.hide());
    mock::queue_modal(move |w| {
        assert_eq!(w, dlg.id());
        assert!(widget(w).unwrap().visible);
        mock::user_click(b.id());
    });
    dlg.run_modal(Some(parent));
    let log = mock::modal_log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].parent, Some(parent.id()));
    assert!(!log[0].still_open);
    assert!(dlg.is_alive());
    assert!(!widget(dlg.id()).unwrap().visible);

    // a hidden window can be run again; closing it with the title bar destroys it
    mock::queue_modal(mock::user_close);
    dlg.run_modal(None);
    assert!(!dlg.is_alive());
    assert_eq!(mock::modal_log().len(), 2);

    // dead windows, non-windows and a window running already do nothing
    dlg.run_modal(None);
    assert_eq!(mock::modal_log().len(), 2);
    let again = Window::new("again");
    let b2 = Button::new(again, "x");
    b2.on_click(move || {
        again.run_modal(None); // re-entrant: ignored
        again.hide();
    });
    mock::queue_modal(move |_| mock::user_click(b2.id()));
    again.run_modal(Some(again)); // its own parent is dropped
    assert_eq!(mock::modal_log().len(), 3);
    assert_eq!(mock::modal_log()[2].parent, None);
}

#[test]
fn prompt_returns_text_on_ok_and_enter_and_none_on_cancel() {
    init();
    let parent = Window::new("p");
    let count_windows = || mock::widget_count();
    let before = count_windows();

    // OK button
    mock::queue_modal(|dlg| {
        let (field, ok) = find(dlg);
        assert_eq!(widget(field).unwrap().text, "start");
        mock::user_text(field, "typed");
        mock::user_click(ok);
    });
    let got = Prompt::new("Name")
        .message("Pick a name")
        .initial("start")
        .run(Some(parent));
    assert_eq!(got.as_deref(), Some("typed"));
    assert_eq!(count_windows(), before, "the dialog is destroyed afterwards");

    // Enter in the field
    mock::queue_modal(|dlg| {
        let (field, _) = find(dlg);
        mock::user_text(field, "entered");
        mock::user_submit(field);
    });
    assert_eq!(prompt(Some(parent), "t", "m", "").as_deref(), Some("entered"));

    // Cancel, Escape, closing the window, and a script that does nothing at all
    mock::queue_modal(|dlg| {
        let (field, _) = find(dlg);
        mock::user_text(field, "dropped");
        let cancel = buttons(dlg)[1];
        mock::user_click(cancel);
    });
    assert_eq!(prompt(Some(parent), "t", "m", ""), None);
    mock::queue_modal(mock::user_cancel);
    assert_eq!(prompt(Some(parent), "t", "m", "x"), None);
    mock::queue_modal(mock::user_close);
    assert_eq!(prompt(None, "t", "m", "x"), None);
    assert_eq!(count_windows(), before);

    // password variant and custom button captions
    mock::queue_modal(|dlg| {
        let kinds = kinds_in(dlg);
        assert!(kinds.contains(&Kind::PasswordInput));
        assert_eq!(widget(buttons(dlg)[0]).unwrap().text, "Weiter");
        mock::user_click(buttons(dlg)[0]);
    });
    let got = Prompt::new("pw").password(true).buttons("Weiter", "Abbruch").run(None);
    assert_eq!(got.as_deref(), Some(""));
}

fn kinds_in(win: WidgetId) -> Vec<Kind> {
    descendants(win)
        .into_iter()
        .filter_map(|c| widget(c).and_then(|w| w.kind))
        .collect()
}
fn descendants(id: WidgetId) -> Vec<WidgetId> {
    let mut out = vec![];
    let mut stack = vec![id];
    while let Some(i) = stack.pop() {
        for c in mock::children_of(i) {
            out.push(c);
            stack.push(c);
        }
    }
    out.sort();
    out
}
fn buttons(win: WidgetId) -> Vec<WidgetId> {
    descendants(win)
        .into_iter()
        .filter(|c| widget(*c).and_then(|w| w.kind) == Some(Kind::Button))
        .collect()
}
fn find(win: WidgetId) -> (WidgetId, WidgetId) {
    let field = descendants(win)
        .into_iter()
        .find(|c| matches!(widget(*c).and_then(|w| w.kind), Some(Kind::TextInput | Kind::PasswordInput)))
        .unwrap();
    (field, buttons(win)[0])
}

#[test]
fn date_validation() {
    assert!(Date::new(2024, 2, 29).is_some());
    assert!(Date::new(2023, 2, 29).is_none());
    assert!(Date::new(1900, 2, 29).is_none());
    assert!(Date::new(2000, 2, 29).is_some());
    assert!(Date::new(1752, 12, 31).is_none());
    assert!(Date::new(2024, 13, 1).is_none());
    assert!(Date::new(2024, 4, 31).is_none());
    assert_eq!(Date::from_unix_utc(0), Date { year: 1970, month: 1, day: 1 });
    assert_eq!(
        Date::from_unix_utc(1_709_164_800),
        Date { year: 2024, month: 2, day: 29 }
    );
    assert!(Date::today().is_valid());
}

#[test]
fn calendar_mirrors_and_reports() {
    init();
    let win = Window::new("w");
    let c = Calendar::new(win);
    assert_eq!(c.date(), Date::today());
    assert_eq!(widget(c.id()).unwrap().date, Some(Date::today()));
    let d = Date::new(2031, 7, 4).unwrap();
    c.set_date(d);
    assert_eq!(widget(c.id()).unwrap().date, Some(d));
    c.set_date(Date { year: 2031, month: 2, day: 30 }); // invalid: ignored
    assert_eq!(c.date(), d);

    let seen: Rc<RefCell<Vec<Date>>> = Rc::default();
    let s2 = seen.clone();
    c.on_change(move |x| s2.borrow_mut().push(x));
    let e = Date::new(2031, 8, 1).unwrap();
    mock::user_pick_date(c.id(), e);
    assert_eq!(c.date(), e);
    assert_eq!(*seen.borrow(), vec![e]);
    // a bogus date from a backend never becomes state
    mock::user_pick_date(c.id(), Date { year: 2031, month: 2, day: 30 });
    assert_eq!(c.date(), e);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(c.a11y().role, None);
}
