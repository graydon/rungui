//! Splitter, text-font/wrap and window position/min-size tests against the mock backend.

use crate::backend::mock::{self, widget};
use crate::*;
use std::cell::RefCell;
use std::rc::Rc;

fn init() {
    let _ = App::new("test");
}

fn rect(w: impl Into<WidgetId>) -> Rect {
    widget(w.into()).unwrap().bounds
}

fn sash(s: Splitter) -> WidgetId {
    mock::sash_of(s.id()).expect("mock backend creates a sash")
}

/// 400x200 window, padding 10: the splitter's area is (10, 10, 380, 180); default sash 6 px.
fn setup(o: Orientation) -> (Window, Splitter, TextArea, ListBox) {
    init();
    let win = Window::new("w");
    win.set_size(400, 200);
    let s = Splitter::new(win, o);
    let a = TextArea::new(s);
    let b = ListBox::new(s);
    win.show();
    (win, s, a, b)
}

/// Record every `on_move` call.
fn moves(s: Splitter) -> Rc<RefCell<Vec<i32>>> {
    let log = Rc::new(RefCell::new(vec![]));
    let l = log.clone();
    s.on_move(move |p| l.borrow_mut().push(p));
    log
}

#[test]
fn horizontal_layout_splits_evenly_by_default() {
    let (_win, s, a, b) = setup(Orientation::Horizontal);
    assert_eq!(s.orientation(), Orientation::Horizontal);
    assert_eq!(s.position(), 187); // (380 - 6) / 2
    assert_eq!(rect(a), Rect::new(10, 10, 187, 180));
    assert_eq!(rect(sash(s)), Rect::new(197, 10, 6, 180));
    assert_eq!(rect(b), Rect::new(203, 10, 187, 180));
    assert_eq!(s.bounds(), Rect::new(10, 10, 380, 180));
    let sw = widget(sash(s)).unwrap();
    assert_eq!(sw.kind, Some(Kind::Sash));
    assert_eq!(sw.orientation, Some(Orientation::Horizontal));
    assert!(sw.visible);
}

#[test]
fn vertical_layout_and_custom_sash_thickness() {
    let (_win, s, a, b) = setup(Orientation::Vertical);
    s.set_spacing(4);
    s.set_position(50);
    App::update();
    assert_eq!(rect(a), Rect::new(10, 10, 380, 50));
    assert_eq!(rect(sash(s)), Rect::new(10, 60, 380, 4));
    assert_eq!(rect(b), Rect::new(10, 64, 380, 126));
    assert_eq!(
        widget(sash(s)).unwrap().orientation,
        Some(Orientation::Vertical)
    );
}

#[test]
fn position_is_clamped_on_resize_and_restored() {
    let (win, s, a, b) = setup(Orientation::Horizontal);
    s.set_position(300);
    assert_eq!(s.position(), 300, "relayout is immediate once laid out");
    assert_eq!(rect(b).w, 380 - 306);
    mock::resize_window(win.id(), 200, 200); // area 180 wide
    assert_eq!(s.position(), 174);
    assert_eq!(rect(b).w, 0);
    mock::resize_window(win.id(), 400, 200);
    assert_eq!(s.position(), 300, "requested position is remembered");
    s.set_min_pane_sizes(40, 100);
    App::update();
    assert_eq!(s.position(), 380 - 6 - 100);
    s.set_position(5);
    assert_eq!(s.position(), 40);
    assert_eq!(rect(a).w, 40);
    // too small for both minimums: the first pane wins, nothing goes negative
    mock::resize_window(win.id(), 100, 200);
    assert_eq!(s.position(), 40);
    assert_eq!(rect(b).w, 80 - 46);
    mock::resize_window(win.id(), 30, 200);
    assert_eq!(s.position(), 4); // area 10 wide, room 4
    assert!(rect(b).w >= 0 && rect(sash(s)).w >= 0);
    s.set_position(-50);
    assert_eq!(s.position(), 4);
}

#[test]
fn natural_size_uses_position_and_both_panes() {
    init();
    let win = Window::new("w");
    let s = Splitter::new(win, Orientation::Horizontal);
    let _a = TextArea::new(s); // 200x100 in the mock
    let _b = TextArea::new(s);
    s.set_position(120);
    win.show();
    assert_eq!(win.size(), (120 + 6 + 200 + 20, 100 + 20));
    assert_eq!(s.position(), 120);
}

#[test]
fn user_drag_moves_sash_and_fires_on_move() {
    let (_win, s, a, b) = setup(Orientation::Horizontal);
    let log = moves(s);
    s.set_position(100);
    assert!(
        log.borrow().is_empty(),
        "programmatic moves do not call on_move"
    );
    mock::user_drag_sash(s.id(), 160); // leading edge at x = 160 -> first pane 150
    assert_eq!(s.position(), 150);
    assert_eq!(rect(a).w, 150);
    assert_eq!(rect(sash(s)).x, 160);
    assert_eq!(rect(b).x, 166);
    mock::user_drag_sash_by(s.id(), -20);
    assert_eq!(s.position(), 130);
    mock::user_drag_sash_by(s.id(), 0); // no change: no callback
    mock::user_drag_sash(s.id(), 10_000); // clamped to the far end
    assert_eq!(s.position(), 374);
    mock::user_drag_sash(s.id(), -10_000);
    assert_eq!(s.position(), 0);
    assert_eq!(*log.borrow(), vec![150, 130, 374, 0]);
}

#[test]
fn vertical_drag_and_dragged_position_sticks() {
    let (win, s, _a, b) = setup(Orientation::Vertical);
    let log = moves(s);
    mock::user_drag_sash(s.id(), 70);
    assert_eq!(s.position(), 60);
    assert_eq!(rect(b), Rect::new(10, 76, 380, 114));
    mock::resize_window(win.id(), 400, 400);
    assert_eq!(s.position(), 60, "a dragged position is kept on resize");
    assert_eq!(*log.borrow(), vec![60]);
}

#[test]
fn rtl_mirrors_panes_and_drags() {
    let (_win, s, a, b) = setup(Orientation::Horizontal);
    set_rtl_layout(true);
    s.set_position(100);
    App::update();
    // first pane on the right
    assert_eq!(rect(a), Rect::new(290, 10, 100, 180));
    assert_eq!(rect(sash(s)), Rect::new(284, 10, 6, 180));
    assert_eq!(rect(b), Rect::new(10, 10, 274, 180));
    let log = moves(s);
    mock::user_drag_sash_by(s.id(), -50); // sash moves left: the right (first) pane grows
    assert_eq!(s.position(), 150);
    assert_eq!(rect(a), Rect::new(240, 10, 150, 180));
    assert_eq!(rect(sash(s)).x, 234);
    assert_eq!(*log.borrow(), vec![150]);
    set_rtl_layout(false);
    App::update();
    assert_eq!(rect(a), Rect::new(10, 10, 150, 180));
}

#[test]
fn hidden_pane_gives_space_to_the_other() {
    let (_win, s, a, b) = setup(Orientation::Horizontal);
    let sh = sash(s);
    a.set_visible(false);
    App::update();
    assert_eq!(rect(b), Rect::new(10, 10, 380, 180));
    assert!(!widget(sh).unwrap().visible);
    mock::user_drag_sash(s.id(), 100); // hidden sash: ignored
    a.set_visible(true);
    App::update();
    assert!(widget(sh).unwrap().visible);
    assert_eq!(rect(a).w, 187);
    assert_eq!(rect(b).x, 203);
    // hiding the whole splitter hides the sash; showing it again keeps the layout's choice
    s.set_visible(false);
    assert!(!widget(sh).unwrap().visible);
    b.set_visible(false);
    s.set_visible(true);
    App::update();
    assert!(!widget(sh).unwrap().visible);
    assert_eq!(rect(a), Rect::new(10, 10, 380, 180));
}

#[test]
fn at_most_two_panes_and_replacing_a_pane() {
    let (_win, s, a, b) = setup(Orientation::Horizontal);
    let third = Label::new(s, "extra");
    assert!(!third.is_alive());
    assert_eq!(last_error(), Some(Error::InvalidHandle));
    a.destroy();
    App::update();
    assert_eq!(rect(b), Rect::new(10, 10, 380, 180));
    assert!(!widget(sash(s)).unwrap().visible);
    let c = ListBox::new(s); // appended: now the SECOND pane
    App::update();
    assert!(c.is_alive());
    assert_eq!(rect(b).x, 10);
    assert_eq!(rect(c).x, 203);
    assert!(widget(sash(s)).unwrap().visible);
    // nobody can add a second sash
    let extra = core::create(Kind::Sash, Some(s.id()), |_| {});
    assert_eq!(extra, WidgetId::DEAD);
    let stray = core::create(Kind::Sash, Some(_win.id()), |_| {});
    assert_eq!(stray, WidgetId::DEAD);
}

#[test]
fn stale_splitter_handles_are_inert() {
    let (_win, s, a, b) = setup(Orientation::Horizontal);
    let sh = sash(s);
    let n = mock::widget_count();
    s.destroy();
    assert!(!s.is_alive() && !a.is_alive() && !b.is_alive());
    assert!(widget(sh).is_none(), "the sash goes with the splitter");
    assert_eq!(mock::widget_count(), n - 3);
    s.set_position(10);
    s.set_min_pane_sizes(1, 1);
    s.on_move(|_| panic!("never"));
    assert_eq!(s.position(), 0);
    assert_eq!(s.orientation(), Orientation::Horizontal);
    core::event(sh, Event::SashDragged(50)); // late native event for a dead sash
    let dead = Splitter::new(Widget(WidgetId::DEAD), Orientation::Vertical);
    assert!(!dead.is_alive());
    assert!(last_error().is_some());
    dead.set_position(3);
    assert_eq!(dead.position(), 0);
    // a splitter cannot be a child of a leaf widget
    let btn = Button::new(_win, "b");
    assert!(!Splitter::new(btn, Orientation::Vertical).is_alive());
}

#[test]
fn nested_splitters() {
    init();
    let win = Window::new("w");
    win.set_size(400, 200);
    win.set_padding(0);
    let outer = Splitter::new(win, Orientation::Horizontal);
    let tree = Tree::new(outer);
    let inner = Splitter::new(outer, Orientation::Vertical);
    let table = Table::new(inner);
    let text = TextArea::new(inner);
    outer.set_position(100);
    inner.set_position(80);
    win.show();
    assert_eq!(rect(tree), Rect::new(0, 0, 100, 200));
    assert_eq!(inner.bounds(), Rect::new(106, 0, 294, 200));
    assert_eq!(rect(table), Rect::new(106, 0, 294, 80));
    assert_eq!(rect(sash(inner)), Rect::new(106, 80, 294, 6));
    assert_eq!(rect(text), Rect::new(106, 86, 294, 114));
    // dragging the outer sash relays out the inner splitter (and its sash) too
    let log = moves(inner);
    mock::user_drag_sash(outer.id(), 150);
    assert_eq!(rect(table), Rect::new(156, 0, 244, 80));
    assert_eq!(rect(sash(inner)).x, 156);
    mock::user_drag_sash(inner.id(), 40);
    assert_eq!(rect(text), Rect::new(156, 46, 244, 154));
    assert_eq!(*log.borrow(), vec![40]);
}

#[test]
fn splitter_inside_boxes_and_tabs() {
    init();
    let win = Window::new("w");
    win.set_size(300, 300);
    let tabs = Tabs::new(win);
    tabs.set_expand(1.0);
    let page = tabs.add_page("p");
    let col = VBox::new(page);
    col.set_expand(1.0);
    Label::new(col, "header");
    let s = Splitter::new(col, Orientation::Horizontal);
    let a = ListBox::new(s);
    let b = ListBox::new(s);
    win.show();
    // the splitter expands by default and fills the rest of the page; panes live in page coordinates
    let pb = s.bounds();
    assert!(pb.h > 100, "{pb:?}");
    assert_eq!(rect(a).y, pb.y);
    assert_eq!(rect(a).h, pb.h);
    assert_eq!(rect(b).x + rect(b).w, pb.x + pb.w);
    let before = s.position();
    mock::user_drag_sash_by(s.id(), 30);
    assert_eq!(s.position(), before + 30);
}

#[test]
fn backend_without_sash_still_lays_out() {
    init();
    mock::set_unsupported(&[Kind::Sash]);
    let win = Window::new("w");
    win.set_size(400, 200);
    let s = Splitter::new(win, Orientation::Horizontal);
    assert!(s.is_alive());
    assert_eq!(last_error(), None, "a missing sash is not an error");
    let a = TextArea::new(s);
    let b = TextArea::new(s);
    s.set_position(120);
    win.show();
    assert!(mock::sash_of(s.id()).is_none());
    assert_eq!(rect(a), Rect::new(10, 10, 120, 180));
    assert_eq!(rect(b), Rect::new(136, 10, 254, 180));
    mock::user_drag_sash(s.id(), 50); // nothing to drag
    assert_eq!(s.position(), 120);
    mock::set_unsupported(&[]);
}

#[test]
fn disabled_splitter_ignores_drags_and_disables_sash() {
    let (_win, s, _a, _b) = setup(Orientation::Horizontal);
    let log = moves(s);
    s.set_enabled(false);
    assert!(!widget(sash(s)).unwrap().enabled);
    mock::user_drag_sash(s.id(), 50);
    assert_eq!(s.position(), 187);
    s.set_enabled(true);
    mock::user_drag_sash(s.id(), 50);
    assert_eq!(s.position(), 40);
    assert_eq!(*log.borrow(), vec![40]);
}

#[test]
fn sash_keys_step_clamp_and_fire_on_move() {
    let (_win, s, a, _b) = setup(Orientation::Horizontal);
    s.set_min_pane_sizes(20, 30);
    s.set_position(100);
    let log = moves(s);
    mock::user_sash_key(s.id(), SashKey::Next);
    assert_eq!(s.position(), 110);
    mock::user_sash_key(s.id(), SashKey::Prev);
    mock::user_sash_key(s.id(), SashKey::Prev);
    assert_eq!(s.position(), 90);
    mock::user_sash_key(s.id(), SashKey::NextLarge);
    assert_eq!(s.position(), 140);
    mock::user_sash_key(s.id(), SashKey::PrevLarge);
    assert_eq!(s.position(), 90);
    assert_eq!(rect(a).w, 90, "the layout follows immediately");
    // Home / End go to the limits (min pane sizes 20 and 30, 380 wide, 6 px sash)
    mock::user_sash_key(s.id(), SashKey::Min);
    assert_eq!(s.position(), 20);
    mock::user_sash_key(s.id(), SashKey::Prev); // already at the limit: no change, no callback
    assert_eq!(s.position(), 20);
    mock::user_sash_key(s.id(), SashKey::Max);
    assert_eq!(s.position(), 344);
    mock::user_sash_key(s.id(), SashKey::NextLarge);
    assert_eq!(s.position(), 344);
    assert_eq!(*log.borrow(), vec![110, 100, 90, 140, 90, 20, 344]);
    // the app can still set it afterwards and keys continue from there
    s.set_position(200);
    mock::user_sash_key(s.id(), SashKey::Next);
    assert_eq!(s.position(), 210);
}

#[test]
fn sash_keys_work_on_a_vertical_splitter() {
    let (_win, s, a, b) = setup(Orientation::Vertical);
    s.set_position(50);
    mock::user_sash_key(s.id(), SashKey::Next); // down: the top pane grows
    assert_eq!(s.position(), 60);
    assert_eq!(rect(a).h, 60);
    assert_eq!(rect(b).y, 10 + 60 + 6);
    mock::user_sash_key(s.id(), SashKey::PrevLarge);
    assert_eq!(s.position(), 10);
}

#[test]
fn sash_keys_are_screen_directions_under_rtl() {
    let (_win, s, a, _b) = setup(Orientation::Horizontal);
    set_rtl_layout(true);
    s.set_position(100);
    App::update();
    assert_eq!(rect(a).x, 290, "first pane on the right");
    mock::user_sash_key(s.id(), SashKey::Prev); // sash moves left: the right (first) pane grows
    assert_eq!(s.position(), 110);
    assert_eq!(rect(sash(s)).x, 274);
    mock::user_sash_key(s.id(), SashKey::Next);
    mock::user_sash_key(s.id(), SashKey::Next);
    assert_eq!(s.position(), 90);
    // Min / Max are about the first pane's size, not about screen sides
    mock::user_sash_key(s.id(), SashKey::Min);
    assert_eq!(s.position(), 0);
    mock::user_sash_key(s.id(), SashKey::Max);
    assert_eq!(s.position(), 374);
    set_rtl_layout(false);
}

#[test]
fn sash_keys_are_ignored_when_stale_or_unusable() {
    let (_win, s, _a, _b) = setup(Orientation::Horizontal);
    let log = moves(s);
    s.set_enabled(false);
    mock::user_sash_key(s.id(), SashKey::Next);
    assert_eq!(s.position(), 187);
    s.set_enabled(true);
    s.set_visible(false);
    mock::user_sash_key(s.id(), SashKey::Next);
    assert_eq!(s.position(), 187);
    s.set_visible(true);
    // a key event aimed at some other widget is not a sash event
    let other = Button::new(_win, "b");
    core::event(other.id(), Event::SashKey(SashKey::Next));
    assert_eq!(s.position(), 187);
    assert!(log.borrow().is_empty());
    mock::user_sash_key(s.id(), SashKey::Next);
    assert_eq!(*log.borrow(), vec![197]);
    // destroyed splitter: nothing to do, no panic
    s.destroy();
    mock::user_sash_key(s.id(), SashKey::Next);
}

#[test]
fn on_move_callback_may_reenter() {
    let (_win, s, a, _b) = setup(Orientation::Horizontal);
    let seen = Rc::new(RefCell::new(vec![]));
    let s2 = seen.clone();
    s.on_move(move |p| {
        s2.borrow_mut().push(p);
        s.set_position(p + 1); // programmatic: no recursion
        a.set_text(&format!("{p}"));
    });
    mock::user_drag_sash(s.id(), 60);
    assert_eq!(*seen.borrow(), vec![50]);
    assert_eq!(s.position(), 51);
    assert_eq!(a.text(), "50");
    s.on_move(|_| panic!("contained"));
    mock::user_drag_sash(s.id(), 80);
    assert_eq!(s.position(), 70);
}

#[test]
fn a11y_sash_is_a_splitter() {
    let (win, s, a, b) = setup(Orientation::Horizontal);
    App::update();
    let nodes = a11y::resolve(win.id()).unwrap();
    let sh = sash(s);
    let n = nodes
        .iter()
        .find(|n| n.id == sh)
        .expect("the sash is reported");
    assert_eq!(n.role, A11yRole::Splitter);
    assert!(!n.role_explicit);
    // the splitter itself is virtual and the panes sit directly under the window in layout order
    assert!(nodes.iter().all(|n| n.id != s.id()));
    let ids: Vec<_> = nodes.iter().map(|n| n.id).collect();
    assert!(ids.contains(&a.id()) && ids.contains(&b.id()));
}

// ---------------------------------------------------------------- text font / wrap

#[test]
fn text_monospace_and_wrap() {
    init();
    let win = Window::new("w");
    win.set_padding(0);
    let col = VBox::new(win);
    let ta = TextArea::new(col);
    let ti = TextInput::new(col);
    win.show();
    assert!(ta.wrap() && !ta.monospace());
    assert!(widget(ta.id()).unwrap().wrap, "native default is wrapping");
    let w0 = rect(ta).w;
    ta.set_monospace(true);
    ta.set_wrap(false);
    ti.set_monospace(true);
    App::update();
    assert!(ta.monospace() && !ta.wrap() && ti.monospace());
    let m = widget(ta.id()).unwrap();
    assert!(m.monospace && !m.wrap);
    assert!(widget(ti.id()).unwrap().monospace);
    assert!(rect(ta).w > w0, "font change relayouts");
    ta.destroy();
    ta.set_wrap(true);
    assert!(!ta.wrap() && !ta.monospace());
}

// ---------------------------------------------------------------- window position / min size

#[test]
fn window_position() {
    init();
    let win = Window::new("w");
    assert_eq!(win.position(), None);
    win.set_position(30, 40);
    assert_eq!(win.position(), Some((30, 40)));
    assert_eq!(widget(win.id()).unwrap().position, Some((30, 40)));
    let seen = Rc::new(RefCell::new(None));
    let s = seen.clone();
    win.on_move(move |x, y| *s.borrow_mut() = Some((x, y)));
    mock::user_move_window(win.id(), -5, 7);
    assert_eq!(win.position(), Some((-5, 7)));
    assert_eq!(*seen.borrow(), Some((-5, 7)));
    // Moved on a non-window is dropped
    let b = Button::new(win, "b");
    core::event(b.id(), Event::Moved { x: 1, y: 1 });
    win.destroy();
    win.set_position(1, 2);
    assert_eq!(win.position(), None);
}

#[test]
fn window_min_size() {
    init();
    let win = Window::new("w");
    win.set_size(300, 200);
    win.set_min_size(250, 150);
    win.show();
    assert_eq!(widget(win.id()).unwrap().min_size, Size::new(250, 150));
    mock::resize_window(win.id(), 100, 100); // a backend that does not enforce the minimum
    assert_eq!(win.size(), (250, 150));
    assert_eq!(rect(win), Rect::new(0, 0, 250, 150));
    mock::resize_window(win.id(), 400, 300);
    assert_eq!(win.size(), (400, 300));
    // min size on an ordinary widget is a layout hint only
    let b = Button::new(win, "b");
    b.set_min_size(10, 10);
    assert_eq!(widget(b.id()).unwrap().min_size, Size::default());
}
