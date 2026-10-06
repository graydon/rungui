//! Behaviour of the Win32 backend that only the real controls can show. Two kinds of test:
//!
//! * what the native control holds after the library configured it, read back with raw messages
//!   through `native_handle()`;
//! * what the application hears when the *user* acts: the tests send the notifications a click,
//!   a selection, a key press or a resize produces (`BM_CLICK`, `LVM_SETITEMSTATE`,
//!   `WM_COMMAND`/`WM_NOTIFY`, posted keys, ...) from outside the library, inside the event loop,
//!   and check the callbacks that fire.
//!
//! Runs as a Windows exe (`cargo test-win` under wine on Linux, or on Windows); empty everywhere
//! else. Each test is its own thread and so its own toolkit instance.
#![cfg(windows)]

use rungui::*;
use std::cell::RefCell;
use std::rc::Rc;

// ------------------------------------------------------------------ raw Win32

type Hwnd = isize;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
#[derive(Default)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
}
#[repr(C)]
struct NmHdr {
    hwnd_from: Hwnd,
    id_from: usize,
    code: i32,
}
#[repr(C)]
struct NmListView {
    hdr: NmHdr,
    item: i32,
    sub_item: i32,
    new_state: u32,
    old_state: u32,
    changed: u32,
    pt: Point,
    lparam: isize,
}
#[repr(C)]
struct NmKeyDown {
    hdr: NmHdr,
    vkey: u16,
    flags: u32,
}
#[repr(C)]
struct NmUpDown {
    hdr: NmHdr,
    pos: i32,
    delta: i32,
}
#[repr(C)]
struct LvItem {
    mask: u32,
    item: i32,
    sub_item: i32,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_max: i32,
    image: i32,
    lparam: isize,
}
#[repr(C)]
#[derive(Default)]
struct MinMaxInfo {
    reserved: Point,
    max_size: Point,
    max_pos: Point,
    min_track: Point,
    max_track: Point,
}

unsafe extern "system" {
    fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
    fn PostMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> i32;
    fn PeekMessageW(m: *mut Msg, h: Hwnd, lo: u32, hi: u32, remove: u32) -> i32;
    fn TranslateMessage(m: *const Msg) -> i32;
    fn DispatchMessageW(m: *const Msg) -> isize;
    fn IsWindowVisible(h: Hwnd) -> i32;
    fn GetParent(h: Hwnd) -> Hwnd;
    fn GetWindow(h: Hwnd, cmd: u32) -> Hwnd;
    fn GetClassNameW(h: Hwnd, buf: *mut u16, n: i32) -> i32;
    fn GetMenu(h: Hwnd) -> isize;
    fn GetSubMenu(m: isize, pos: i32) -> isize;
    fn GetMenuItemID(m: isize, pos: i32) -> u32;
    fn SetWindowTextW(h: Hwnd, s: *const u16) -> i32;
    fn GetWindowTextW(h: Hwnd, buf: *mut u16, n: i32) -> i32;
    fn SetWindowPos(h: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, hh: i32, flags: u32) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
}

const WM_SIZE: u32 = 0x5;
const WM_CLOSE: u32 = 0x10;
const WM_GETMINMAXINFO: u32 = 0x24;
const WM_NOTIFY: u32 = 0x4E;
const WM_CONTEXTMENU: u32 = 0x7B;
const WM_KEYDOWN: u32 = 0x100;
const WM_KILLFOCUS: u32 = 0x8;
const WM_COMMAND: u32 = 0x111;
const WM_HSCROLL: u32 = 0x114;
const WM_DPICHANGED: u32 = 0x2E0;
const BM_CLICK: u32 = 0xF5;
const TBM_GETPOS: u32 = 0x400;
const TBM_GETRANGEMIN: u32 = 0x401;
const TBM_GETRANGEMAX: u32 = 0x402;
const TBM_SETPOS: u32 = 0x405;
const LB_GETCOUNT: u32 = 0x18B;
const LB_GETCURSEL: u32 = 0x188;
const LB_SETCURSEL: u32 = 0x186;
const CB_GETCOUNT: u32 = 0x146;
const CB_GETCURSEL: u32 = 0x147;
const CB_SETCURSEL: u32 = 0x14E;
const PBM_GETPOS: u32 = 0x408;
const LVM_SETITEMSTATE: u32 = 0x102B;
const TVM_EXPAND: u32 = 0x1102;
const TVM_GETNEXTITEM: u32 = 0x110A;
const TVM_SELECTITEM: u32 = 0x110B;
const TCM_SETCURSEL: u32 = 0x130C;
const EM_GETSEL: u32 = 0xB0;
const EM_SETSEL: u32 = 0xB1;
const EM_REPLACESEL: u32 = 0xC2;
const LVIS_SELECTED: u32 = 2;
const TVGN_ROOT: usize = 0;
const TVGN_CHILD: usize = 4;
const TVGN_CARET: usize = 9;
const TVE_COLLAPSE: usize = 1;
const TVE_EXPAND: usize = 2;
const NM_DBLCLK: i32 = -3;
const LVN_ITEMCHANGED: i32 = -101;
const LVN_COLUMNCLICK: i32 = -108;
const LVN_KEYDOWN: i32 = -155;
const TVN_KEYDOWN: i32 = -412;
const TCN_SELCHANGE: i32 = -551;
const UDN_DELTAPOS: i32 = -722;
const CBN_SELCHANGE: usize = 1;
const LBN_SELCHANGE: usize = 1;
const LBN_DBLCLK: usize = 2;
const SB_THUMBTRACK: usize = 5;
const VK_RETURN: usize = 0x0D;
const VK_F5: usize = 0x74;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_END: usize = 0x23;
const GW_CHILD: u32 = 5;
const GW_HWNDNEXT: u32 = 2;
const SWP_NOSIZE: u32 = 1;
const SWP_NOMOVE: u32 = 2;
const SWP_NOZORDER: u32 = 4;
const SWP_NOACTIVATE: u32 = 0x10;
const IDOK: usize = 1;
const IDCANCEL: usize = 2;
const IDYES: usize = 6;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn hwnd(n: Option<NativeHandle>) -> Hwnd {
    match n {
        Some(NativeHandle::Win32(h)) => h as Hwnd,
        other => panic!("not a Win32 handle: {other:?}"),
    }
}

fn send(h: Hwnd, m: u32) -> isize {
    unsafe { SendMessageW(h, m, 0, 0) }
}

fn make_lparam(lo: usize, hi: usize) -> isize {
    ((hi << 16) | (lo & 0xFFFF)) as isize
}

/// `WM_COMMAND` from control `h` to its parent, as the control sends it.
fn command(h: Hwnd, code: usize) {
    unsafe { SendMessageW(GetParent(h), WM_COMMAND, code << 16, h) };
}

/// `WM_NOTIFY` from control `h` to its parent; `extra` wraps the header in the notification's struct.
fn notify<T>(h: Hwnd, code: i32, extra: impl FnOnce(NmHdr) -> T) {
    let n = extra(NmHdr {
        hwnd_from: h,
        id_from: 0,
        code,
    });
    unsafe { SendMessageW(GetParent(h), WM_NOTIFY, 0, &n as *const T as isize) };
}

/// Deliver everything that is queued, a few times (posted follow-ups queue more).
fn pump() {
    for _ in 0..5 {
        let mut m = Msg::default();
        while unsafe { PeekMessageW(&mut m, 0, 0, 0, 1) } != 0 {
            unsafe {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn window_text(h: Hwnd) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn set_window_text(h: Hwnd, s: &str) {
    unsafe { SetWindowTextW(h, wide(s).as_ptr()) };
}

fn class_of(h: Hwnd) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The first child window of `h` (direct children only) whose class is `class`.
fn child_of_class(h: Hwnd, class: &str) -> Option<Hwnd> {
    let mut c = unsafe { GetWindow(h, GW_CHILD) };
    while c != 0 {
        if class_of(c) == class {
            return Some(c);
        }
        c = unsafe { GetWindow(c, GW_HWNDNEXT) };
    }
    None
}

// ------------------------------------------------------------------ harness

type Log = Rc<RefCell<Vec<String>>>;

fn log_has(log: &Log, s: &str) -> bool {
    log.borrow().iter().any(|l| l == s)
}

fn assert_logged(log: &Log, s: &str) {
    assert!(log_has(log, s), "expected {s:?} in {:?}", log.borrow());
}

/// Run `body` once from inside the event loop (after the toolkit has started), then quit.
fn in_loop(app: App, body: impl FnOnce() + 'static) {
    let mut body = Some(body);
    let _t = Timer::once(1, move || {
        if let Some(b) = body.take() {
            b();
        }
        pump();
        App::quit();
    });
    app.run();
}

fn new_app(name: &str) -> App {
    App::new(name).unwrap_or_else(|e| panic!("init: {e}"))
}

// ------------------------------------------------------------------ state held by controls

#[test]
fn slider_range_wider_than_16_bits() {
    let _app = new_app("win32-native-slider");
    let win = Window::new("t");
    let col = VBox::new(win);
    // the trackbar's range is 32 bits wide, but TBM_SETRANGE packs min and max into 16 bits each
    let s = Slider::new(col, 0.0, 1_000_000.0);
    s.set_range(0.0, 1_000_000.0, 1.0);
    s.set_value(1_000_000.0);
    let h = hwnd(s.native_handle());
    assert_eq!(send(h, TBM_GETRANGEMIN), 0);
    let max = send(h, TBM_GETRANGEMAX);
    assert!(max > 65_535, "native max {max}");
    assert_eq!(send(h, TBM_GETPOS), max);
    s.set_value(500_000.0);
    let half = send(h, TBM_GETPOS);
    assert!((half * 2 - max).abs() <= 1, "half-way is {half} of {max}");
}

#[test]
fn list_and_combo_hold_every_item() {
    let _app = new_app("win32-native-lists");
    let win = Window::new("t");
    let col = VBox::new(win);
    let items: Vec<String> = (0..2_000).map(|i| format!("item {i}")).collect();
    let list = ListBox::new(col);
    list.set_items(&items);
    list.set_selected(Some(1_999));
    let lh = hwnd(list.native_handle());
    assert_eq!(send(lh, LB_GETCOUNT), 2_000);
    assert_eq!(send(lh, LB_GETCURSEL), 1_999);
    list.set_items(&items[..10]);
    assert_eq!(send(lh, LB_GETCOUNT), 10);
    assert_eq!(
        send(lh, LB_GETCURSEL),
        -1,
        "selection beyond the new end is dropped"
    );
    let combo = ComboBox::new(col);
    combo.set_items(&items);
    combo.set_selected(Some(7));
    let ch = hwnd(combo.native_handle());
    assert_eq!(send(ch, CB_GETCOUNT), 2_000);
    assert_eq!(send(ch, CB_GETCURSEL), 7);
}

#[test]
fn progress_bar_follows_the_value() {
    let _app = new_app("win32-native-progress");
    let win = Window::new("t");
    let col = VBox::new(win);
    let p = ProgressBar::new(col);
    p.set_fraction(0.5);
    assert_eq!(send(hwnd(p.native_handle()), PBM_GETPOS), 500);
    p.set_indeterminate(true);
    p.set_indeterminate(false);
}

#[test]
fn visibility_reaches_the_native_window() {
    let _app = new_app("win32-native-visible");
    let win = Window::new("t");
    let col = VBox::new(win);
    let label = Label::new(col, "x");
    win.show();
    let h = hwnd(label.native_handle());
    assert_ne!(unsafe { IsWindowVisible(h) }, 0);
    label.set_visible(false);
    assert_eq!(unsafe { IsWindowVisible(h) }, 0);
}

#[test]
fn text_area_keeps_text_and_selection_when_wrap_changes() {
    let _app = new_app("win32-native-wrap");
    let win = Window::new("t");
    let col = VBox::new(win);
    let area = TextArea::new(col);
    area.set_text("one\ntwo\nthree");
    area.set_tooltip("a tip");
    let old = hwnd(area.native_handle());
    let (mut a, mut b) = (0u32, 0u32);
    let sel = |h: Hwnd, a: &mut u32, b: &mut u32| unsafe {
        SendMessageW(h, EM_GETSEL, a as *mut u32 as usize, b as *mut u32 as isize);
    };
    unsafe { SendMessageW(old, EM_SETSEL, 4, 7) };
    sel(old, &mut a, &mut b);
    assert_eq!((a, b), (4, 7));
    area.set_wrap(false); // a new EDIT control: wrapping cannot be changed on a live one
    let new = hwnd(area.native_handle());
    assert_ne!(new, old);
    assert_eq!(window_text(new), "one\r\ntwo\r\nthree");
    sel(new, &mut a, &mut b);
    assert_eq!((a, b), (4, 7));
    area.set_tooltip("");
    area.set_monospace(true);
    area.set_read_only(true);
    area.set_wrap(true);
    assert_eq!(area.text(), "one\ntwo\nthree");
}

#[test]
fn images_group_boxes_and_pages_survive_painting() {
    let app = new_app("win32-native-paint");
    let win = Window::new("t");
    let col = VBox::new(win);
    let img = Image::new(col);
    img.set_image(Some(&ImageData {
        w: 4,
        h: 4,
        rgba: vec![128; 64],
    }));
    img.set_image(None);
    img.set_image(Some(&ImageData {
        w: 2,
        h: 2,
        rgba: vec![255; 16],
    }));
    let gb = GroupBox::new(col, "&Group");
    let inner = HBox::new(gb);
    Button::new(inner, "in group");
    let tabs = Tabs::new(col);
    let page = tabs.add_page("one");
    Label::new(page, "in a page");
    Slider::new(page, 0.0, 10.0);
    tabs.add_page("two");
    win.show();
    in_loop(app, move || {
        tabs.set_selected(1);
        tabs.set_selected(0);
        gb.set_title("Renamed");
    });
}

// ------------------------------------------------------------------ what the user does

#[test]
fn clicks_and_toggles() {
    let app = new_app("win32-native-clicks");
    let win = Window::new("t");
    let col = VBox::new(win);
    let log: Log = Rc::default();
    let button = Button::new(col, "Go");
    let l = log.clone();
    button.on_click(move || l.borrow_mut().push("click".into()));
    let check = CheckBox::new(col, "Check");
    let l = log.clone();
    check.on_toggle(move |on| l.borrow_mut().push(format!("check {on}")));
    let group = RadioGroup::new();
    let r1 = RadioButton::new(col, &group, "r1");
    let r2 = RadioButton::new(col, &group, "r2");
    let l = log.clone();
    r1.on_toggle(move |on| l.borrow_mut().push(format!("r1 {on}")));
    let l = log.clone();
    r2.on_toggle(move |on| l.borrow_mut().push(format!("r2 {on}")));
    win.show();
    let (hb, hc, h2) = (
        hwnd(button.native_handle()),
        hwnd(check.native_handle()),
        hwnd(r2.native_handle()),
    );
    in_loop(app, move || unsafe {
        SendMessageW(hb, BM_CLICK, 0, 0);
        SendMessageW(hc, BM_CLICK, 0, 0);
        SendMessageW(hc, BM_CLICK, 0, 0);
        SendMessageW(h2, BM_CLICK, 0, 0);
    });
    assert_logged(&log, "click");
    assert_logged(&log, "check true");
    assert_logged(&log, "check false");
    assert_logged(&log, "r2 true");
    assert!(r2.checked());
}

#[test]
fn text_entry_and_spin_box() {
    let app = new_app("win32-native-text");
    let win = Window::new("t");
    let col = VBox::new(win);
    let log: Log = Rc::default();
    let entry = TextInput::new(col);
    let l = log.clone();
    entry.on_change(move |t| l.borrow_mut().push(format!("text {t}")));
    let area = TextArea::new(col);
    let l = log.clone();
    area.on_change(move |t| {
        l.borrow_mut()
            .push(format!("area {}", t.replace('\n', "|")))
    });
    let spin = SpinBox::new(col, 0.0, 10.0, 0.5);
    let l = log.clone();
    spin.on_change(move |v| l.borrow_mut().push(format!("spin {v}")));
    win.show();
    let (he, ha, hs) = (
        hwnd(entry.native_handle()),
        hwnd(area.native_handle()),
        hwnd(spin.native_handle()),
    );
    in_loop(app, move || {
        set_window_text(he, "héllo");
        // typing into the multi-line edit: replace the selection (wine does not report WM_SETTEXT here)
        unsafe {
            SendMessageW(ha, EM_SETSEL, 0, -1);
            SendMessageW(ha, EM_REPLACESEL, 1, wide("a\r\nb").as_ptr() as isize);
        }
        set_window_text(hs, "7,5"); // a decimal comma
        unsafe { SendMessageW(hs, WM_KILLFOCUS, 0, 0) };
        // the up arrow of the spin button: the shown value plus one step
        notify(hs, UDN_DELTAPOS, |hdr| NmUpDown {
            hdr,
            pos: 0,
            delta: 1,
        });
        set_window_text(hs, "99"); // beyond the range: clamped
        unsafe { SendMessageW(hs, WM_KILLFOCUS, 0, 0) };
        set_window_text(hs, "nonsense");
        unsafe { SendMessageW(hs, WM_KILLFOCUS, 0, 0) };
    });
    assert_logged(&log, "text héllo");
    assert_logged(&log, "area a|b");
    assert_logged(&log, "spin 7.5");
    assert_logged(&log, "spin 8");
    assert_logged(&log, "spin 10");
    assert_eq!(spin.value(), 10.0);
}

#[test]
fn lists_combo_and_slider() {
    let app = new_app("win32-native-select");
    let win = Window::new("t");
    let col = VBox::new(win);
    let log: Log = Rc::default();
    let combo = ComboBox::new(col);
    combo.set_items(&["a", "b", "c"]);
    let l = log.clone();
    combo.on_select(move |s| l.borrow_mut().push(format!("combo {s:?}")));
    let list = ListBox::new(col);
    list.set_items(&["x", "y", "z"]);
    let l = log.clone();
    list.on_select(move |s| l.borrow_mut().push(format!("list {s:?}")));
    let l = log.clone();
    list.on_activate(move |i| l.borrow_mut().push(format!("activate {i}")));
    let slider = Slider::new(col, 0.0, 10.0);
    let l = log.clone();
    slider.on_change(move |v| l.borrow_mut().push(format!("slider {v}")));
    win.show();
    let (hc, hl, hs) = (
        hwnd(combo.native_handle()),
        hwnd(list.native_handle()),
        hwnd(slider.native_handle()),
    );
    in_loop(app, move || unsafe {
        SendMessageW(hc, CB_SETCURSEL, 2, 0);
        command(hc, CBN_SELCHANGE);
        SendMessageW(hl, LB_SETCURSEL, 1, 0);
        command(hl, LBN_SELCHANGE);
        command(hl, LBN_DBLCLK);
        SendMessageW(hl, WM_KEYDOWN, VK_RETURN, 0);
        // the trackbar has one position per step: 0..=10
        SendMessageW(hs, TBM_SETPOS, 1, 5);
        SendMessageW(GetParent(hs), WM_HSCROLL, SB_THUMBTRACK, hs);
    });
    assert_logged(&log, "combo Some(2)");
    assert_logged(&log, "list Some(1)");
    assert_logged(&log, "activate 1");
    assert_logged(&log, "slider 5");
    assert_eq!(combo.selected(), Some(2));
}

fn list_view_note(hdr: NmHdr, item: i32, sub_item: i32) -> NmListView {
    NmListView {
        hdr,
        item,
        sub_item,
        new_state: 0,
        old_state: 0,
        changed: 0,
        pt: Point::default(),
        lparam: 0,
    }
}

#[test]
fn table_selection_and_activation() {
    let app = new_app("win32-native-table");
    let win = Window::new("t");
    let col = VBox::new(win);
    let log: Log = Rc::default();
    let table = Table::new(col);
    table.set_columns(&[Column::new("a"), Column::new("b").sortable(true)]);
    table.set_rows(&[vec!["1", "x"], vec!["2", "y"], vec!["3", "z"]]);
    let l = log.clone();
    table.on_select(move |s| l.borrow_mut().push(format!("select {s:?}")));
    let l = log.clone();
    table.on_activate(move |i| l.borrow_mut().push(format!("activate {i}")));
    let l = log.clone();
    table.on_column_click(move |c| l.borrow_mut().push(format!("column {c}")));
    let l = log.clone();
    table.on_context_menu(move |_, _| l.borrow_mut().push("context".into()));
    win.show();
    let h = hwnd(table.native_handle());
    in_loop(app, move || {
        let it = LvItem {
            mask: 0,
            item: 0,
            sub_item: 0,
            state: LVIS_SELECTED,
            state_mask: LVIS_SELECTED,
            text: std::ptr::null_mut(),
            text_max: 0,
            image: 0,
            lparam: 0,
        };
        // selecting natively sends LVN_ITEMCHANGED; the library reports it after the burst
        unsafe { SendMessageW(h, LVM_SETITEMSTATE, 1, &it as *const LvItem as isize) };
        pump();
        notify(h, LVN_COLUMNCLICK, |hdr| list_view_note(hdr, -1, 1));
        notify(h, NM_DBLCLK, |hdr| list_view_note(hdr, 2, 0));
        notify(h, LVN_KEYDOWN, |hdr| NmKeyDown {
            hdr,
            vkey: VK_RETURN as u16,
            flags: 0,
        });
        // a change notification that is no selection change is ignored
        notify(h, LVN_ITEMCHANGED, |hdr| list_view_note(hdr, 0, 0));
        // the keyboard's context menu key, and a click at a position
        unsafe { SendMessageW(h, WM_CONTEXTMENU, h as usize, -1) };
        unsafe { SendMessageW(h, WM_CONTEXTMENU, h as usize, make_lparam(5, 5)) };
    });
    assert_logged(&log, "select Some(1)");
    assert_logged(&log, "column 1");
    assert_logged(&log, "activate 2");
    assert_logged(&log, "activate 1");
    assert_logged(&log, "context");
}

#[test]
fn tree_selection_expansion_and_activation() {
    let app = new_app("win32-native-tree");
    let win = Window::new("t");
    let col = VBox::new(win);
    let log: Log = Rc::default();
    let tree = Tree::new(col);
    let root = tree.add(None, "root");
    let child = tree.add(Some(root), "child");
    tree.add(Some(child), "grandchild");
    let l = log.clone();
    tree.on_select(move |n| l.borrow_mut().push(format!("select {}", n.is_some())));
    let l = log.clone();
    tree.on_expand(move |_, open| l.borrow_mut().push(format!("expand {open}")));
    let l = log.clone();
    tree.on_activate(move |_| l.borrow_mut().push("activate".into()));
    win.show();
    let h = hwnd(tree.native_handle());
    in_loop(app, move || unsafe {
        let top = SendMessageW(h, TVM_GETNEXTITEM, TVGN_ROOT, 0);
        SendMessageW(h, TVM_SELECTITEM, TVGN_CARET, top);
        SendMessageW(h, TVM_EXPAND, TVE_EXPAND, top);
        pump();
        let kid = SendMessageW(h, TVM_GETNEXTITEM, TVGN_CHILD, top);
        SendMessageW(h, TVM_SELECTITEM, TVGN_CARET, kid);
        notify(h, TVN_KEYDOWN, |hdr| NmKeyDown {
            hdr,
            vkey: VK_RETURN as u16,
            flags: 0,
        });
        // the application's reaction to `expand` rebuilt the native items: ask for the new handle
        let top = SendMessageW(h, TVM_GETNEXTITEM, TVGN_ROOT, 0);
        let r = SendMessageW(h, TVM_EXPAND, TVE_COLLAPSE, top);
        assert_ne!(r, 0);
        pump();
        SendMessageW(h, WM_CONTEXTMENU, h as usize, -1);
    });
    assert_logged(&log, "select true");
    assert_logged(&log, "expand true");
    // (wine sends no notification for a programmatic collapse, so none is asserted)
    assert_logged(&log, "activate");
}

#[test]
fn tabs_report_the_selected_page() {
    let app = new_app("win32-native-tabs");
    let win = Window::new("t");
    let tabs = Tabs::new(win);
    let a = tabs.add_page("one");
    Label::new(a, "first");
    let b = tabs.add_page("two");
    Label::new(b, "second");
    let log: Log = Rc::default();
    let l = log.clone();
    tabs.on_select(move |s| l.borrow_mut().push(format!("tab {s:?}")));
    win.show();
    let h = hwnd(tabs.native_handle());
    in_loop(app, move || unsafe {
        SendMessageW(h, TCM_SETCURSEL, 1, 0);
        notify(h, TCN_SELCHANGE, |hdr| hdr);
        b.set_title("renamed");
        a.destroy();
    });
    assert_logged(&log, "tab Some(1)");
}

#[test]
fn menus_and_accelerators() {
    let app = new_app("win32-native-menus");
    let win = Window::new("t");
    let bar = MenuBar::new(win);
    let file = Menu::new(bar, "&File");
    let open = MenuItem::new(file, "&Open");
    open.set_accel("F5");
    MenuSeparator::new(file);
    let check = CheckMenuItem::new(file, "Check");
    let log: Log = Rc::default();
    let l = log.clone();
    open.on_click(move || l.borrow_mut().push("open".into()));
    let l = log.clone();
    check.on_toggle(move |on| l.borrow_mut().push(format!("check {on}")));
    let col = VBox::new(win);
    Button::new(col, "focus");
    win.show();
    let wh = hwnd(win.native_handle());
    in_loop(app, move || unsafe {
        let menu = GetSubMenu(GetMenu(wh), 0);
        let (open_id, check_id) = (
            GetMenuItemID(menu, 0) as usize,
            GetMenuItemID(menu, 2) as usize,
        );
        SendMessageW(wh, WM_COMMAND, open_id, 0);
        SendMessageW(wh, WM_COMMAND, check_id, 0);
        SendMessageW(wh, WM_COMMAND, check_id, 0);
        // the accelerator goes through the loop and is translated into the same command
        PostMessageW(wh, WM_KEYDOWN, VK_F5, 0);
        // changes below the bar and to its own items
        open.set_text("Open &again");
        open.set_enabled(false);
        check.set_checked(true);
        open.set_accel("");
        open.set_accel("F6");
        file.set_enabled(false);
        file.set_enabled(true);
        MenuItem::new(file, "late");
    });
    assert!(log.borrow().iter().filter(|l| *l == "open").count() >= 1);
    assert_logged(&log, "check true");
    assert_logged(&log, "check false");
}

#[test]
fn window_events() {
    let app = new_app("win32-native-window");
    let win = Window::new("t");
    let col = VBox::new(win);
    Label::new(col, "label");
    win.set_min_size(300, 200);
    win.set_size(320, 240);
    let log: Log = Rc::default();
    let l = log.clone();
    win.on_resize(move |w, h| l.borrow_mut().push(format!("resize {w}x{h}")));
    let l = log.clone();
    win.on_move(move |x, y| l.borrow_mut().push(format!("move {x},{y}")));
    let vetoes = Rc::new(RefCell::new(2));
    let l = log.clone();
    win.on_close(move || {
        l.borrow_mut().push("close".into());
        let mut v = vetoes.borrow_mut();
        *v -= 1;
        *v <= 0
    });
    win.show();
    let wh = hwnd(win.native_handle());
    in_loop(app, move || unsafe {
        SetWindowPos(
            wh,
            0,
            0,
            0,
            500,
            420,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SetWindowPos(
            wh,
            0,
            40,
            50,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        // the system asks for the smallest size the window may be dragged to
        let mut mm = MinMaxInfo::default();
        SendMessageW(wh, WM_GETMINMAXINFO, 0, &mut mm as *mut MinMaxInfo as isize);
        assert!(mm.min_track.x >= 300 && mm.min_track.y >= 200);
        // moved to a monitor with 150% scaling and back
        let r = Rect {
            left: 0,
            top: 0,
            right: 600,
            bottom: 500,
        };
        let rp = &r as *const Rect as isize;
        SendMessageW(wh, WM_DPICHANGED, 144 | (144 << 16), rp);
        SendMessageW(wh, WM_DPICHANGED, 96 | (96 << 16), rp);
        SendMessageW(wh, WM_SIZE, 0, make_lparam(450, 380));
        SendMessageW(wh, WM_CLOSE, 0, 0); // vetoed
        SendMessageW(wh, WM_CLOSE, 0, 0); // allowed
    });
    assert!(log.borrow().iter().any(|l| l.starts_with("resize ")));
    assert_eq!(log.borrow().iter().filter(|l| *l == "close").count(), 2);
    assert!(!win.is_alive());
}

#[test]
fn splitter_keys_move_the_sash() {
    let app = new_app("win32-native-sash");
    let win = Window::new("t");
    let split = Splitter::new(win, Orientation::Horizontal);
    Label::new(split, "left");
    Label::new(split, "right");
    win.set_size(400, 200);
    let log: Log = Rc::default();
    let l = log.clone();
    split.on_move(move |p| l.borrow_mut().push(format!("sash {p}")));
    win.show();
    // the splitter has no window of its own: its sash is a child of the window
    let sash = child_of_class(hwnd(win.native_handle()), "RunguiSash").expect("sash window");
    in_loop(app, move || unsafe {
        SendMessageW(sash, WM_KEYDOWN, VK_RIGHT, 0);
        SendMessageW(sash, WM_KEYDOWN, VK_LEFT, 0);
        SendMessageW(sash, WM_KEYDOWN, VK_END, 0);
        SendMessageW(sash, WM_KEYDOWN, VK_UP, 0); // not along this sash's axis
        SendMessageW(sash, WM_KEYDOWN, VK_DOWN, 0);
    });
    assert!(
        log.borrow().iter().any(|l| l.starts_with("sash ")),
        "{:?}",
        log.borrow()
    );
}

#[test]
fn context_menu_events() {
    let app = new_app("win32-native-context");
    let win = Window::new("t");
    let col = VBox::new(win);
    let button = Button::new(col, "right-click me");
    let log: Log = Rc::default();
    let l = log.clone();
    button.on_context_menu(move |_, _| l.borrow_mut().push("button".into()));
    win.show();
    // (popup menus, and the default menu of an edit control, are modal and need a user to
    // dismiss them: scripts/smoke-win32.sh does)
    let hb = hwnd(button.native_handle());
    in_loop(app, move || unsafe {
        SendMessageW(hb, WM_CONTEXTMENU, hb as usize, make_lparam(10, 10));
    });
    assert_logged(&log, "button");
}

// ------------------------------------------------------------------ modal dialogs

/// A timer that dismisses the modal dialog (class `#32770`) with the command `id` as soon as one
/// exists.
fn dismiss_dialog(id: usize) -> Timer {
    Timer::every(20, move || unsafe {
        let dlg = FindWindowW(wide("#32770").as_ptr(), std::ptr::null());
        if dlg != 0 {
            SendMessageW(dlg, WM_COMMAND, id, 0);
            SendMessageW(dlg, WM_CLOSE, 0, 0);
        }
    })
}

#[test]
fn message_boxes_return_an_answer() {
    let app = new_app("win32-native-msgbox");
    let win = Window::new("t");
    let answers: Rc<RefCell<Vec<Answer>>> = Rc::default();
    let a = answers.clone();
    in_loop(app, move || {
        let t = dismiss_dialog(IDOK);
        a.borrow_mut().push(message_box(
            Some(win),
            MessageKind::Info,
            Buttons::Ok,
            "title",
            "text",
        ));
        t.stop();
        let t = dismiss_dialog(IDYES);
        a.borrow_mut().push(message_box(
            None,
            MessageKind::Question,
            Buttons::YesNo,
            "title",
            "really?",
        ));
        t.stop();
        let t = dismiss_dialog(IDCANCEL);
        a.borrow_mut().push(message_box(
            None,
            MessageKind::Warning,
            Buttons::OkCancel,
            "title",
            "sure?",
        ));
        a.borrow_mut().push(message_box(
            None,
            MessageKind::Error,
            Buttons::YesNoCancel,
            "title",
            "bad",
        ));
        t.stop();
    });
    let a = answers.borrow();
    assert_eq!(a.len(), 4);
    assert_eq!(a[0], Answer::Ok);
    assert_eq!(a[1], Answer::Yes);
    assert_eq!(a[2], Answer::Cancel);
    assert_eq!(a[3], Answer::Cancel);
}

#[test]
fn file_dialogs_can_be_cancelled() {
    let app = new_app("win32-native-files");
    let win = Window::new("t");
    let dir = std::env::temp_dir();
    let out: Rc<RefCell<Vec<usize>>> = Rc::default();
    let o = out.clone();
    in_loop(app, move || {
        let dialog = FileDialog::new()
            .title("pick")
            .filter("Text", &["txt", "*.md"])
            .filter("Everything", &[])
            .directory(&dir.to_string_lossy())
            .file_name("name.txt");
        let t = dismiss_dialog(IDCANCEL);
        o.borrow_mut()
            .push(usize::from(dialog.open(Some(win)).is_some()));
        o.borrow_mut().push(dialog.open_many(None).len());
        o.borrow_mut()
            .push(usize::from(dialog.save(Some(win)).is_some()));
        o.borrow_mut()
            .push(usize::from(dialog.pick_folder(None).is_some()));
        t.stop();
    });
    assert_eq!(out.borrow().len(), 4, "every dialog returned");
    assert!(out.borrow().iter().all(|n| *n == 0), "{:?}", out.borrow());
}
