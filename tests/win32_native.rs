//! Behaviour of the Win32 backend that only the real controls can show: what the native control
//! holds after the library has configured it. Reads the state back with raw messages through
//! `native_handle()`. Runs as a Windows exe (`cargo test-win` under wine on Linux, or on Windows);
//! empty everywhere else. Each test is its own thread and so its own toolkit instance.
#![cfg(windows)]

use rungui::*;

unsafe extern "system" {
    fn SendMessageW(h: isize, m: u32, w: usize, l: isize) -> isize;
    fn IsWindowVisible(h: isize) -> i32;
}

const TBM_GETPOS: u32 = 0x400;
const TBM_GETRANGEMIN: u32 = 0x401;
const TBM_GETRANGEMAX: u32 = 0x402;
const LB_GETCOUNT: u32 = 0x18B;
const LB_GETCURSEL: u32 = 0x188;
const CB_GETCOUNT: u32 = 0x146;
const CB_GETCURSEL: u32 = 0x147;
const PBM_GETPOS: u32 = 0x408;

fn hwnd(n: Option<NativeHandle>) -> isize {
    match n {
        Some(NativeHandle::Win32(h)) => h as isize,
        other => panic!("not a Win32 handle: {other:?}"),
    }
}

fn send(h: isize, m: u32) -> isize {
    unsafe { SendMessageW(h, m, 0, 0) }
}

#[test]
fn slider_range_wider_than_16_bits() {
    let _app = App::new("win32-native-slider").unwrap_or_else(|e| panic!("init: {e}"));
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
    let _app = App::new("win32-native-lists").unwrap_or_else(|e| panic!("init: {e}"));
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
    let _app = App::new("win32-native-progress").unwrap_or_else(|e| panic!("init: {e}"));
    let win = Window::new("t");
    let col = VBox::new(win);
    let p = ProgressBar::new(col);
    p.set_fraction(0.5);
    assert_eq!(send(hwnd(p.native_handle()), PBM_GETPOS), 500);
}

#[test]
fn visibility_reaches_the_native_window() {
    let _app = App::new("win32-native-visible").unwrap_or_else(|e| panic!("init: {e}"));
    let win = Window::new("t");
    let col = VBox::new(win);
    let label = Label::new(col, "x");
    win.show();
    let h = hwnd(label.native_handle());
    assert_ne!(unsafe { IsWindowVisible(h) }, 0);
    label.set_visible(false);
    assert_eq!(unsafe { IsWindowVisible(h) }, 0);
}
