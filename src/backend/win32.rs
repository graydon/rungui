//! Win32 backend: user32 / comctl32 v6 / gdi32 / uxtheme / COM file dialogs, all hand-declared in
//! `win32/sys.rs` (no binding crates). Accessibility is the stock MSAA proxies plus `IAccPropServices` overrides
//! (`win32/a11y.rs`); there is no UIA provider.
//!
//! Design notes
//! * Every widget is a child HWND of its native parent (Window, or a *container*: Page / GroupBox).
//!   Containers use our own window class and receive the WM_COMMAND/WM_NOTIFY/WM_HSCROLL messages of
//!   their children. `HWND -> WidgetId` is a thread-local table (`State::by_hwnd`).
//! * Programmatic changes run under a `MuteGuard`; notifications arriving while it is held are ignored
//!   (rule 2 of the backend contract) and no `core::*` call is ever made while a `State` borrow exists.
//! * Text is UTF-16 everywhere (`...W` functions). Coordinates from the core are logical pixels and are
//!   scaled by the window's DPI (per-monitor v2 when available; `WM_DPICHANGED` is handled).
//! * Common controls v6 are activated at runtime with an activation context built from a manifest
//!   written to the temp directory (so no linker-level resource embedding is needed). comctl32 is
//!   `LoadLibrary`'d *after* activation and never linked statically. An application that embeds its
//!   own manifest simply makes this a no-op.
//! * SpinBox = EDIT + an `msctls_updown32` without buddy (we manage the value ourselves, so decimal
//!   steps work). GroupBox = container window + a BS_GROUPBOX frame control behind its children.
//! * Table = SysListView32 (report), Tree = SysTreeView32, PopupMenu = HMENU shown with
//!   `TrackPopupMenuEx`, Sash = `RunguiSash` window class with mouse capture. Tab pages are siblings
//!   stacked above the tab control (wine paints the control over its children). Bursty native
//!   notifications are coalesced through posted messages (`WM_SELCHECK`, `WM_TREEEXP`).
//! * Run under wine on a private Xvfb by `scripts/smoke-win32.sh`; no window manager runs there, so
//!   `Event::Moved`, `Prop::MinSize` and live resizing are only compile-checked.

use super::*;
use crate::core;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};

mod sys;
use sys::*;
mod a11y;

const WM_WAKE: u32 = WM_APP + 1;
const CLS_WINDOW: &str = "RunguiWindow";
const CLS_CONTAINER: &str = "RunguiContainer";
const CLS_MSG: &str = "RunguiMessage";
const CLS_SASH: &str = "RunguiSash";
/// Posted to the message window: re-check a table's selection once the burst of notifications is over.
const WM_SELCHECK: u32 = WM_APP + 2;
/// Posted to the message window: deliver queued tree expand/collapse events outside the notification.
const WM_TREEEXP: u32 = WM_APP + 3;

/// Range of the WM_COMMAND ids handed to menu items (below it are control ids, above system ones).
const CMD_FIRST: u16 = 1000;
const CMD_LAST: u16 = 0xEFFF;

static MSG_HWND: AtomicIsize = AtomicIsize::new(0);

// ------------------------------------------------------------------ state

struct W {
    kind: Kind,
    hwnd: HWND,
    /// SpinBox: the up-down control. GroupBox: the frame control.
    aux: HWND,
    parent: Option<WidgetId>,
    win: WidgetId,
    /// Tabs: pages. Menu/MenuBar: items, in order.
    children: Vec<WidgetId>,
    hmenu: isize,
    cmd: u16,
    text: String,
    items: Vec<String>,
    bounds: Rect,
    vis: bool,
    enabled: bool,
    checked: bool,
    range: (f64, f64, f64),
    value: f64,
    selected: Option<usize>,
    tip: Vec<u16>,
    has_tip: bool,
    image: Option<ImageData>,
    hbmp: isize,
    accel: Option<String>,
    // Window only
    dpi: u32,
    tip_hwnd: HWND,
    haccel: isize,
    menubar: isize,
    client_req: Option<Size>,
    shown: bool,
    last_focus: HWND,
    min_size: Size,
    last_pos: Option<(i32, i32)>,
    // text controls
    readonly: bool,
    placeholder: String,
    mono: bool,
    wrap: bool,
    // Table / Tree
    cols: Vec<Column>,
    last_sel: Option<usize>,
    nodes: HashMap<u64, isize>,
    last_tsel: Option<u64>,
    // Sash
    vertical: bool,
    drag: Option<(i32, i32)>,
}

impl W {
    fn new(kind: Kind, win: WidgetId, parent: Option<WidgetId>) -> W {
        W {
            kind,
            hwnd: 0,
            aux: 0,
            parent,
            win,
            children: vec![],
            hmenu: 0,
            cmd: 0,
            text: String::new(),
            items: vec![],
            bounds: Rect::default(),
            vis: true,
            enabled: true,
            checked: false,
            range: (0.0, 100.0, 1.0),
            value: 0.0,
            selected: None,
            tip: vec![],
            has_tip: false,
            image: None,
            hbmp: 0,
            accel: None,
            dpi: 96,
            tip_hwnd: 0,
            haccel: 0,
            menubar: 0,
            client_req: None,
            shown: false,
            last_focus: 0,
            min_size: Size::default(),
            last_pos: None,
            readonly: false,
            placeholder: String::new(),
            mono: false,
            wrap: true,
            cols: vec![],
            last_sel: None,
            nodes: HashMap::new(),
            last_tsel: None,
            vertical: false,
            drag: None,
        }
    }
}

#[derive(Copy, Clone, Default)]
struct Fns {
    set_dpi_ctx: Option<unsafe extern "system" fn(isize) -> BOOL>,
    dpi_for_window: Option<unsafe extern "system" fn(HWND) -> u32>,
    adjust_rect_dpi: Option<unsafe extern "system" fn(*mut RECT, u32, BOOL, u32, u32) -> BOOL>,
}

#[derive(Default)]
struct State {
    widgets: HashMap<WidgetId, W>,
    by_hwnd: HashMap<HWND, WidgetId>,
    by_cmd: HashMap<u16, WidgetId>,
    next_cmd: u16,
    fonts: HashMap<u32, isize>,
    timers: HashMap<u64, bool>,
    inst: isize,
    fns: Fns,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
    static MUTE: Cell<u32> = const { Cell::new(0) };
    /// Original window procedures of subclassed controls (kept until WM_NCDESTROY).
    static ORIG: RefCell<HashMap<HWND, isize>> = RefCell::new(HashMap::new());
    /// Tree expand/collapse events waiting for `WM_TREEEXP`.
    static TREE_EXP: RefCell<Vec<(WidgetId, u64, bool)>> = const { RefCell::new(Vec::new()) };
    /// Set by `popup_menu`: a context menu was shown during the current `ContextMenu` emission.
    static POPUP_SHOWN: Cell<bool> = const { Cell::new(false) };
}

/// Short, non-reentrant access to the state. Returns `None` if the state is already borrowed
/// (never panics).
fn st<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    S.try_with(|s| s.try_borrow_mut().ok().map(|mut g| f(&mut g)))
        .ok()
        .flatten()
}

fn get<R>(id: WidgetId, f: impl FnOnce(&W) -> R) -> Option<R> {
    st(|s| s.widgets.get(&id).map(f)).flatten()
}
fn with_w<R>(id: WidgetId, f: impl FnOnce(&mut W) -> R) -> Option<R> {
    st(|s| s.widgets.get_mut(&id).map(f)).flatten()
}
fn id_of(h: HWND) -> Option<WidgetId> {
    st(|s| s.by_hwnd.get(&h).copied()).flatten()
}

struct MuteGuard;
impl MuteGuard {
    fn new() -> MuteGuard {
        let _ = MUTE.try_with(|m| m.set(m.get() + 1));
        MuteGuard
    }
}
impl Drop for MuteGuard {
    fn drop(&mut self) {
        let _ = MUTE.try_with(|m| m.set(m.get().saturating_sub(1)));
    }
}
fn muted() -> bool {
    MUTE.try_with(|m| m.get() > 0).unwrap_or(true)
}

// ------------------------------------------------------------------ small helpers

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .filter(|c| *c != 0)
        .chain(std::iter::once(0))
        .collect()
}
/// Core text with `&` mnemonic markers -> Win32 prefix syntax (surplus markers dropped).
fn esc_amp(s: &str) -> String {
    crate::mnemonic::to_win32_mnemonic(s)
}
fn nl_in(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}
fn px(v: i32, dpi: u32) -> i32 {
    ((v as i64 * dpi as i64 + if v >= 0 { 48 } else { -48 }) / 96) as i32
}
fn lp(v: i32, dpi: u32) -> i32 {
    let d = dpi.max(1) as i64;
    ((v as i64 * 96 + if v >= 0 { d / 2 } else { -(d / 2) }) / d) as i32
}
fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}
fn hiword(v: usize) -> u32 {
    ((v >> 16) & 0xFFFF) as u32
}

fn get_text(h: HWND, multiline: bool) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 2];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        buf.truncate(got.max(0) as usize);
        let s = String::from_utf16_lossy(&buf);
        if multiline {
            s.replace("\r\n", "\n")
        } else {
            s
        }
    }
}
fn set_text(h: HWND, s: &str) {
    let w = wide(s);
    unsafe {
        SetWindowTextW(h, w.as_ptr());
    }
}
fn send(h: HWND, m: u32, w: usize, l: isize) -> isize {
    unsafe { SendMessageW(h, m, w, l) }
}

fn decimals(step: f64) -> usize {
    let mut d = 0;
    let mut s = step.abs();
    while d < 6 && (s - s.round()).abs() > 1e-9 {
        s *= 10.0;
        d += 1;
    }
    d
}
fn fmt_value(v: f64, step: f64) -> String {
    format!("{:.*}", decimals(step), v)
}
fn slider_steps(r: (f64, f64, f64)) -> i32 {
    let span = r.1 - r.0;
    if r.2 > 0.0 && span > 0.0 {
        (span / r.2).round().clamp(1.0, 100000.0) as i32
    } else {
        1000
    }
}

fn vk_for(key: &str) -> Option<u16> {
    let k = key.to_ascii_uppercase();
    let mut it = k.chars();
    if let (Some(c), None) = (it.next(), it.next()) {
        return match c {
            'A'..='Z' | '0'..='9' => Some(c as u16),
            '+' | '=' => Some(0xBB),
            '-' => Some(0xBD),
            ',' => Some(0xBC),
            '.' => Some(0xBE),
            '/' => Some(0xBF),
            ';' => Some(0xBA),
            '`' => Some(0xC0),
            '[' => Some(0xDB),
            '\\' => Some(0xDC),
            ']' => Some(0xDD),
            '\'' => Some(0xDE),
            _ => None,
        };
    }
    if let Some(n) = k.strip_prefix('F').and_then(|r| r.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    Some(match k.as_str() {
        "ENTER" | "RETURN" => 0x0D,
        "ESC" | "ESCAPE" => 0x1B,
        "DEL" | "DELETE" => 0x2E,
        "TAB" => 9,
        "SPACE" => 0x20,
        "BACKSPACE" | "BKSP" => 8,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PGUP" => 0x21,
        "PAGEDOWN" | "PGDN" => 0x22,
        "INS" | "INSERT" => 0x2D,
        _ => return None,
    })
}

// ------------------------------------------------------------------ DPI, fonts, measuring

fn dpi_of(id: WidgetId) -> u32 {
    st(|s| {
        let w = s.widgets.get(&id)?;
        Some(s.widgets.get(&w.win).map_or(96, |x| x.dpi))
    })
    .flatten()
    .unwrap_or(96)
}

fn system_dpi() -> i32 {
    unsafe {
        let dc = GetDC(0);
        let d = if dc != 0 {
            GetDeviceCaps(dc, LOGPIXELSY)
        } else {
            96
        };
        if dc != 0 {
            ReleaseDC(0, dc);
        }
        if d <= 0 { 96 } else { d }
    }
}

/// The UI font (system message font) for `dpi`, cached.
fn font(dpi: u32) -> isize {
    if let Some(f) = st(|s| s.fonts.get(&dpi).copied()).flatten() {
        return f;
    }
    let f = unsafe {
        let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
        ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        let ok = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            &mut ncm as *mut _ as *mut c_void,
            0,
        );
        let mut f = 0;
        if ok != 0 {
            let mut lf = ncm.lfMessageFont;
            lf.lfHeight = (lf.lfHeight as i64 * dpi as i64 / system_dpi() as i64) as i32;
            f = CreateFontIndirectW(&lf);
        }
        if f == 0 {
            GetStockObject(DEFAULT_GUI_FONT)
        } else {
            f
        }
    };
    st(|s| s.fonts.insert(dpi, f));
    f
}

/// Fixed-pitch font for `dpi` (Consolas when installed, else the system's fixed-pitch match), cached.
fn mono_font(dpi: u32) -> isize {
    let key = dpi | 0x8000_0000;
    if let Some(f) = st(|s| s.fonts.get(&key).copied()).flatten() {
        return f;
    }
    let f = unsafe {
        let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
        ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        let ok = SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            &mut ncm as *mut _ as *mut c_void,
            0,
        );
        let mut lf = ncm.lfMessageFont;
        if ok == 0 {
            lf = std::mem::zeroed();
            lf.lfHeight = -12;
        }
        lf.lfHeight = (lf.lfHeight as i64 * dpi as i64 / system_dpi() as i64) as i32;
        lf.lfWidth = 0;
        lf.lfPitchAndFamily = 1 | 0x30; // FIXED_PITCH | FF_MODERN
        lf.lfFaceName = [0; 32];
        for (d, c) in lf.lfFaceName.iter_mut().zip("Consolas".encode_utf16()) {
            *d = c;
        }
        let f = CreateFontIndirectW(&lf);
        if f == 0 {
            GetStockObject(ANSI_FIXED_FONT)
        } else {
            f
        }
    };
    st(|s| s.fonts.insert(key, f));
    f
}

/// The font a widget should use: the UI font, or the fixed-pitch one for monospace text controls.
fn font_of(id: WidgetId, dpi: u32) -> isize {
    if get(id, |w| w.mono).unwrap_or(false) {
        mono_font(dpi)
    } else {
        font(dpi)
    }
}

/// Text extent in physical pixels (multi-line aware) using the UI font for `dpi`.
fn measure(dpi: u32, text: &str) -> (i32, i32) {
    measure_with(font(dpi), text)
}
fn measure_with(f: isize, text: &str) -> (i32, i32) {
    unsafe {
        let dc = GetDC(0);
        if dc == 0 {
            return (text.chars().count() as i32 * 7, 16);
        }
        let old = SelectObject(dc, f);
        let (mut w, mut h) = (0, 0);
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            let ws: Vec<u16> = line.encode_utf16().collect();
            let mut sz = SIZE::default();
            if ws.is_empty() {
                let a = [b'A' as u16];
                GetTextExtentPoint32W(dc, a.as_ptr(), 1, &mut sz);
                sz.cx = 0;
            } else {
                GetTextExtentPoint32W(dc, ws.as_ptr(), ws.len() as i32, &mut sz);
            }
            w = w.max(sz.cx);
            h += sz.cy;
        }
        SelectObject(dc, old);
        ReleaseDC(0, dc);
        (w, h)
    }
}
fn text_h(dpi: u32) -> i32 {
    measure(dpi, "Ag").1
}

// ------------------------------------------------------------------ init / loop

fn proc_addr<T: Copy>(module: &str, name: &[u8]) -> Option<T> {
    unsafe {
        let m = LoadLibraryW(wide(module).as_ptr());
        if m == 0 {
            return None;
        }
        let p = GetProcAddress(m, name.as_ptr());
        if p.is_null() {
            None
        } else {
            Some(std::mem::transmute_copy::<*const c_void, T>(&p))
        }
    }
}

fn activate_visual_styles() {
    const MANIFEST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\"><dependency><dependentAssembly>\
<assemblyIdentity type=\"win32\" name=\"Microsoft.Windows.Common-Controls\" version=\"6.0.0.0\" \
processorArchitecture=\"*\" publicKeyToken=\"6595b64144ccf1df\" language=\"*\"/></dependentAssembly></dependency></assembly>";
    let path = std::env::temp_dir().join(format!("rungui-{}.manifest", std::process::id()));
    if std::fs::write(&path, MANIFEST).is_err() {
        return;
    }
    let src = wide(&path.to_string_lossy());
    unsafe {
        let ctx = ACTCTXW {
            cbSize: std::mem::size_of::<ACTCTXW>() as u32,
            dwFlags: 0,
            lpSource: src.as_ptr(),
            wProcessorArchitecture: 0,
            wLangId: 0,
            lpAssemblyDirectory: null(),
            lpResourceName: null(),
            lpApplicationName: null(),
            hModule: 0,
        };
        let h = CreateActCtxW(&ctx);
        if h != -1 && h != 0 {
            let mut cookie = 0usize;
            ActivateActCtx(h, &mut cookie); // stays active for the life of the process
        }
    }
    let _ = std::fs::remove_file(&path);
}

fn register_class(name: &str, proc_: WndProc, bg: isize, inst: isize) {
    let n = wide(name);
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(proc_),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: 0,
            hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: bg,
            lpszMenuName: null(),
            lpszClassName: n.as_ptr(),
            hIconSm: 0,
        };
        RegisterClassExW(&wc);
    }
}

pub struct Win32;

impl Backend for Win32 {
    fn init(_app_name: &str) -> Result<()> {
        let fns = Fns {
            set_dpi_ctx: proc_addr("user32.dll", b"SetProcessDpiAwarenessContext\0"),
            dpi_for_window: proc_addr("user32.dll", b"GetDpiForWindow\0"),
            adjust_rect_dpi: proc_addr("user32.dll", b"AdjustWindowRectExForDpi\0"),
        };
        unsafe {
            if let Some(f) = fns.set_dpi_ctx {
                f(-4); // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
            } else if let Some(f) = proc_addr::<unsafe extern "system" fn() -> BOOL>(
                "user32.dll",
                b"SetProcessDPIAware\0",
            ) {
                f();
            }
            activate_visual_styles();
            if let Some(f) = proc_addr::<
                unsafe extern "system" fn(*const INITCOMMONCONTROLSEX) -> BOOL,
            >("comctl32.dll", b"InitCommonControlsEx\0")
            {
                let icc = INITCOMMONCONTROLSEX {
                    dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: 0x40FF,
                };
                f(&icc);
            }
            CoInitializeEx(null_mut(), 2);
            let inst = GetModuleHandleW(null());
            register_class(CLS_WINDOW, window_proc, (COLOR_BTNFACE + 1) as isize, inst);
            register_class(CLS_CONTAINER, window_proc, 0, inst);
            register_class(CLS_SASH, sash_proc, (COLOR_BTNFACE + 1) as isize, inst);
            register_class(CLS_MSG, msg_proc, 0, inst);
            let h = CreateWindowExW(
                0,
                wide(CLS_MSG).as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                0,
                inst,
                null_mut(),
            );
            if h == 0 {
                return Err(Error::Backend(format!(
                    "cannot create message window (error {})",
                    GetLastError()
                )));
            }
            MSG_HWND.store(h, Ordering::SeqCst);
            st(|s| {
                s.inst = inst;
                s.fns = fns;
                s.next_cmd = CMD_FIRST;
            });
        }
        Ok(())
    }

    fn run() {
        unsafe {
            let mut msg = MSG::zeroed();
            loop {
                let r = GetMessageW(&mut msg, 0, 0, 0);
                if r <= 0 {
                    break;
                }
                let root = GetAncestor(msg.hwnd, GA_ROOT);
                if root != 0 {
                    if let Some(id) = id_of(root) {
                        let ha = get(id, |w| w.haccel).unwrap_or(0);
                        if ha != 0 && TranslateAcceleratorW(root, ha, &msg) != 0 {
                            continue;
                        }
                        // list boxes want Enter themselves (IsDialogMessage would eat it)
                        let own_enter = msg.message == WM_KEYDOWN
                            && msg.wparam == VK_RETURN
                            && matches!(
                                id_of(msg.hwnd).and_then(|c| get(c, |w| w.kind)),
                                Some(Kind::ListBox | Kind::Table | Kind::Tree)
                            );
                        if !own_enter
                            && get(id, |w| w.kind) == Some(Kind::Window)
                            && IsDialogMessageW(root, &msg) != 0
                        {
                            continue;
                        }
                    }
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn quit() {
        unsafe { PostQuitMessage(0) }
    }

    fn wake() {
        // the core already collapses bursts of wake-ups into one, so a posted message per call
        // cannot flood the (10,000 message) queue
        let h = MSG_HWND.load(Ordering::SeqCst);
        if h != 0 {
            unsafe {
                PostMessageW(h, WM_WAKE, 0, 0);
            }
        }
    }

    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        let h = MSG_HWND.load(Ordering::SeqCst);
        if h == 0 {
            return Err(Error::NotInitialized);
        }
        let ok = unsafe { SetTimer(h, token as usize, millis.max(1), 0) };
        if ok == 0 {
            return Err(Error::Backend("SetTimer failed".into()));
        }
        st(|s| s.timers.insert(token, repeat));
        Ok(())
    }

    fn timer_stop(token: u64) {
        let h = MSG_HWND.load(Ordering::SeqCst);
        st(|s| s.timers.remove(&token));
        if h != 0 {
            unsafe {
                KillTimer(h, token as usize);
            }
        }
    }

    fn create(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
        let _m = MuteGuard::new();
        create_impl(id, kind, parent)
    }

    fn destroy(id: WidgetId) {
        let _m = MuteGuard::new();
        destroy_impl(id);
    }

    fn set(id: WidgetId, prop: &Prop) {
        let _m = MuteGuard::new();
        set_impl(id, prop);
    }

    fn preferred_size(id: WidgetId) -> Size {
        preferred_impl(id).unwrap_or_default()
    }

    fn chrome(id: WidgetId) -> Size {
        chrome_impl(id).unwrap_or_default()
    }

    fn native_handle(id: WidgetId) -> Option<NativeHandle> {
        get(id, |w| if w.hwnd != 0 { w.hwnd } else { w.hmenu })
            .filter(|h| *h != 0)
            .map(|h| NativeHandle::Win32(h as usize))
    }

    fn message_box(parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        let owner = parent.and_then(|p| get(p, |w| w.hwnd)).unwrap_or(0);
        let mut flags = match spec.buttons {
            Buttons::Ok => 0,
            Buttons::OkCancel => MB_OKCANCEL,
            Buttons::YesNo => MB_YESNO,
            Buttons::YesNoCancel => MB_YESNOCANCEL,
        };
        flags |= match spec.kind {
            MessageKind::Info => MB_ICONINFORMATION,
            MessageKind::Warning => MB_ICONWARNING,
            MessageKind::Error => MB_ICONERROR,
            MessageKind::Question => MB_ICONQUESTION,
        };
        flags |= MB_APPLMODAL;
        let r = unsafe {
            MessageBoxW(
                owner,
                wide(&spec.text).as_ptr(),
                wide(&spec.title).as_ptr(),
                flags,
            )
        };
        match r {
            IDOK => Answer::Ok,
            IDYES => Answer::Yes,
            IDNO => Answer::No,
            _ => Answer::Cancel,
        }
    }

    fn file_dialog(parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String> {
        let owner = parent.and_then(|p| get(p, |w| w.hwnd)).unwrap_or(0);
        catch_unwind(AssertUnwindSafe(|| file_dialog_impl(owner, spec))).unwrap_or_default()
    }

    fn popup_menu(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            popup_menu_impl(menu, parent_window, at)
        }));
    }

    fn a11y_changed(window: WidgetId) {
        let _ = catch_unwind(AssertUnwindSafe(|| a11y::apply(window)));
    }
}

unsafe extern "system" fn msg_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_WAKE => {
            let _ = catch_unwind(core::drain_posted);
            0
        }
        WM_SELCHECK => {
            let _ = catch_unwind(|| table_sel_check(WidgetId(w as u64)));
            0
        }
        WM_TREEEXP => {
            let q = TREE_EXP
                .try_with(|q| std::mem::take(&mut *q.borrow_mut()))
                .unwrap_or_default();
            for (id, node, open) in q {
                if get(id, |_| ()).is_some() {
                    let _ = catch_unwind(|| emit(id, Event::TreeExpanded(node, open)));
                }
            }
            0
        }
        WM_TIMER => {
            let token = w as u64;
            let repeat = st(|s| s.timers.get(&token).copied()).flatten();
            if repeat == Some(false) {
                unsafe {
                    KillTimer(h, w);
                }
            }
            if repeat.is_some() {
                let _ = catch_unwind(|| core::timer_fired(token));
            }
            0
        }
        _ => unsafe { DefWindowProcW(h, m, w, l) },
    }
}

// ------------------------------------------------------------------ creation

fn ctl_spec(kind: Kind) -> Option<(&'static str, u32, u32)> {
    use Kind::*;
    let tab = WS_TABSTOP;
    Some(match kind {
        Label => ("STATIC", SS_NOPREFIX, 0),
        Button => ("BUTTON", tab, 0),
        CheckBox => ("BUTTON", tab | BS_AUTOCHECKBOX, 0),
        RadioButton => ("BUTTON", tab | BS_RADIOBUTTON, 0),
        TextInput => ("EDIT", tab | ES_AUTOHSCROLL, WS_EX_CLIENTEDGE),
        PasswordInput => ("EDIT", tab | ES_AUTOHSCROLL | ES_PASSWORD, WS_EX_CLIENTEDGE),
        TextArea => (
            "EDIT",
            tab | ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | WS_VSCROLL,
            WS_EX_CLIENTEDGE,
        ),
        ComboBox => ("COMBOBOX", tab | CBS_DROPDOWNLIST | WS_VSCROLL, 0),
        ListBox => (
            "LISTBOX",
            tab | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | WS_VSCROLL,
            WS_EX_CLIENTEDGE,
        ),
        Slider => ("msctls_trackbar32", tab | TBS_NOTICKS, 0),
        ProgressBar => ("msctls_progress32", 0, 0),
        SpinBox => ("EDIT", tab | ES_AUTOHSCROLL, WS_EX_CLIENTEDGE),
        Image => ("STATIC", SS_OWNERDRAW, 0),
        Table => (
            "SysListView32",
            tab | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER,
            WS_EX_CLIENTEDGE,
        ),
        Tree => (
            "SysTreeView32",
            tab | TVS_HASBUTTONS
                | TVS_HASLINES
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_DISABLEDRAGDROP,
            WS_EX_CLIENTEDGE,
        ),
        Tabs => (
            "SysTabControl32",
            tab | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
        ),
        _ => return None,
    })
}

fn create_window_raw(ex: u32, class: &str, style: u32, parent: HWND, inst: isize) -> HWND {
    unsafe {
        CreateWindowExW(
            ex,
            wide(class).as_ptr(),
            wide("").as_ptr(),
            style,
            0,
            0,
            10,
            10,
            parent,
            0,
            inst,
            null_mut(),
        )
    }
}

fn last_err(what: &str) -> Error {
    Error::Backend(format!("{what} failed (Win32 error {})", unsafe {
        GetLastError()
    }))
}

fn subclass(h: HWND) {
    unsafe {
        let orig = SetWindowLongPtrW(h, GWLP_WNDPROC, ctl_proc as *const () as usize as isize);
        if orig != 0 {
            let _ = ORIG.try_with(|o| o.borrow_mut().insert(h, orig));
        }
    }
}

fn create_impl(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
    let (inst, fns) = st(|s| (s.inst, s.fns)).ok_or(Error::NotInitialized)?;
    if kind == Kind::Window {
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let h = unsafe {
            CreateWindowExW(
                0,
                wide(CLS_WINDOW).as_ptr(),
                wide("").as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                0,
                0,
                inst,
                null_mut(),
            )
        };
        if h == 0 {
            return Err(last_err("CreateWindowEx"));
        }
        let dpi = match fns.dpi_for_window {
            Some(f) => unsafe { f(h) },
            None => system_dpi() as u32,
        }
        .max(48);
        let mut w = W::new(kind, id, None);
        w.hwnd = h;
        w.dpi = dpi;
        w.vis = false;
        st(|s| {
            s.widgets.insert(id, w);
            s.by_hwnd.insert(h, id);
        });
        return Ok(());
    }

    if kind == Kind::PopupMenu {
        let m = unsafe { CreatePopupMenu() };
        if m == 0 {
            return Err(last_err("CreatePopupMenu"));
        }
        let mut w = W::new(kind, id, None);
        w.hmenu = m;
        st(|s| s.widgets.insert(id, w));
        return Ok(());
    }

    let p = parent.ok_or(Error::InvalidHandle)?;
    let (phwnd, pkind, pwin, pmenu) =
        get(p, |w| (w.hwnd, w.kind, w.win, w.hmenu)).ok_or(Error::InvalidHandle)?;

    // ---- menus ----
    if matches!(
        kind,
        Kind::MenuBar | Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator
    ) {
        let mut w = W::new(kind, pwin, Some(p));
        let empty = wide("");
        match kind {
            Kind::MenuBar => {
                let m = unsafe { CreateMenu() };
                if m == 0 {
                    return Err(last_err("CreateMenu"));
                }
                w.hmenu = m;
                w.win = p;
                unsafe {
                    SetMenu(phwnd, m);
                }
            }
            Kind::Menu => {
                let m = unsafe { CreatePopupMenu() };
                if m == 0 {
                    return Err(last_err("CreatePopupMenu"));
                }
                w.hmenu = m;
                unsafe {
                    AppendMenuW(pmenu, MF_POPUP | MF_STRING, m as usize, empty.as_ptr());
                }
            }
            Kind::MenuSeparator => unsafe {
                AppendMenuW(pmenu, MF_SEPARATOR, 0, null());
            },
            _ => {
                let cmd = st(|s| {
                    // command ids are 16 bits: at most CMD_LAST - CMD_FIRST + 1 live items
                    for _ in CMD_FIRST..=CMD_LAST {
                        let c = s.next_cmd;
                        s.next_cmd = if c >= CMD_LAST { CMD_FIRST } else { c + 1 };
                        if let std::collections::hash_map::Entry::Vacant(e) = s.by_cmd.entry(c) {
                            e.insert(id);
                            return Some(c);
                        }
                    }
                    None
                })
                .ok_or(Error::NotInitialized)?
                .ok_or(Error::LimitExceeded)?;
                w.cmd = cmd;
                unsafe {
                    AppendMenuW(pmenu, MF_STRING, cmd as usize, empty.as_ptr());
                }
            }
        }
        st(|s| {
            s.widgets.insert(id, w);
            if let Some(pw) = s.widgets.get_mut(&p) {
                pw.children.push(id);
            }
        });
        let win = if kind == Kind::MenuBar { p } else { pwin };
        menu_changed(win);
        return Ok(());
    }

    // ---- containers ----
    if matches!(kind, Kind::Page | Kind::GroupBox) {
        // Pages are siblings stacked above the tab control (not its children): the tab control would
        // otherwise paint over them.
        let chost = if kind == Kind::Page {
            parent_hwnd(p).unwrap_or(phwnd)
        } else {
            phwnd
        };
        let h = create_window_raw(
            WS_EX_CONTROLPARENT,
            CLS_CONTAINER,
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            chost,
            inst,
        );
        if h == 0 {
            return Err(last_err("CreateWindowEx(container)"));
        }
        let mut w = W::new(kind, pwin, Some(p));
        w.hwnd = h;
        let dpi = dpi_of(pwin);
        if kind == Kind::GroupBox {
            let f = create_window_raw(0, "BUTTON", WS_CHILD | WS_VISIBLE | BS_GROUPBOX, h, inst);
            if f == 0 {
                unsafe {
                    DestroyWindow(h);
                }
                return Err(last_err("CreateWindowEx(groupbox)"));
            }
            send(f, WM_SETFONT, font(dpi) as usize, 1);
            w.aux = f;
        }
        if kind == Kind::Page {
            let tab_index = get(p, |t| t.children.len()).unwrap_or(0);
            w.vis = tab_index == 0;
            let mut empty = wide("");
            let item = TCITEMW {
                mask: TCIF_TEXT,
                dwState: 0,
                dwStateMask: 0,
                pszText: empty.as_mut_ptr(),
                cchTextMax: 0,
                iImage: -1,
                lParam: 0,
            };
            send(
                phwnd,
                TCM_INSERTITEMW,
                tab_index,
                &item as *const _ as isize,
            );
            if tab_index > 0 {
                unsafe {
                    ShowWindow(h, SW_HIDE);
                }
            }
        }
        let aux = w.aux;
        st(|s| {
            s.widgets.insert(id, w);
            s.by_hwnd.insert(h, id);
            if aux != 0 {
                s.by_hwnd.insert(aux, id);
            }
            if let Some(pw) = s.widgets.get_mut(&p) {
                pw.children.push(id);
                if kind == Kind::Page && pw.selected.is_none() {
                    pw.selected = Some(0);
                }
            }
        });
        if kind == Kind::Page {
            position_pages(p);
        }
        return Ok(());
    }

    // ---- splitter sash ----
    if kind == Kind::Sash {
        // WS_TABSTOP: keyboard-operable (arrows / Home / End, see `sash_msg`)
        let h = create_window_raw(
            0,
            CLS_SASH,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_TABSTOP,
            phwnd,
            inst,
        );
        if h == 0 {
            return Err(last_err("CreateWindowEx(sash)"));
        }
        let mut w = W::new(kind, pwin, Some(p));
        w.hwnd = h;
        st(|s| {
            s.widgets.insert(id, w);
            s.by_hwnd.insert(h, id);
        });
        return Ok(());
    }

    // ---- ordinary controls ----
    let (class, style, ex) = ctl_spec(kind).ok_or(Error::Unsupported)?;
    let h = create_window_raw(ex, class, WS_CHILD | WS_VISIBLE | style, phwnd, inst);
    if h == 0 {
        return Err(last_err("CreateWindowEx(control)"));
    }
    let _ = pkind;
    let dpi = dpi_of(pwin);
    let f = font(dpi) as usize;
    send(h, WM_SETFONT, f, 1);
    let mut w = W::new(kind, pwin, Some(p));
    w.hwnd = h;
    match kind {
        Kind::Slider => {
            send(h, TBM_SETRANGE, 1, (slider_steps(w.range) as isize) << 16);
        }
        Kind::ProgressBar => {
            send(h, PBM_SETRANGE32, 0, 1000);
        }
        Kind::ComboBox => {
            send(h, CB_SETMINVISIBLE, 10, 0);
        }
        Kind::TextInput | Kind::PasswordInput | Kind::TextArea => {
            send(h, EM_SETLIMITTEXT, 0, 0); // 0 = as much as the control supports
        }
        Kind::Table => {
            send(
                h,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                0,
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
            );
        }
        Kind::Tree => {
            send(h, TVM_SETEXTENDEDSTYLE, 0, TVS_EX_DOUBLEBUFFER as isize);
        }
        Kind::SpinBox => {
            let u = create_window_raw(0, "msctls_updown32", WS_CHILD | WS_VISIBLE, phwnd, inst);
            if u != 0 {
                send(u, UDM_SETRANGE32, 0, 100);
                w.aux = u;
            }
        }
        _ => {}
    }
    if !matches!(kind, Kind::Label | Kind::Image | Kind::ProgressBar) {
        subclass(h);
    }
    let aux = w.aux;
    st(|s| {
        s.widgets.insert(id, w);
        s.by_hwnd.insert(h, id);
        if aux != 0 {
            s.by_hwnd.insert(aux, id);
        }
    });
    if kind == Kind::SpinBox {
        let (v, r) = (0.0, (0.0, 100.0, 1.0));
        set_text(h, &fmt_value(v, r.2));
    }
    Ok(())
}

fn destroy_impl(id: WidgetId) {
    let Some((w, pos)) = st(|s| {
        let w = s.widgets.remove(&id)?;
        s.by_hwnd.remove(&w.hwnd);
        if w.aux != 0 {
            s.by_hwnd.remove(&w.aux);
        }
        if w.cmd != 0 {
            s.by_cmd.remove(&w.cmd);
        }
        let mut pos = None;
        if let Some(p) = w.parent {
            if let Some(pw) = s.widgets.get_mut(&p) {
                pos = pw.children.iter().position(|c| *c == id);
                pw.children.retain(|c| *c != id);
            }
        }
        Some((w, pos))
    })
    .flatten() else {
        return;
    };
    if w.kind == Kind::Window {
        a11y::forget_window(id);
    } else {
        a11y::forget(w.win, &[w.hwnd, w.aux]);
    }
    unsafe {
        match w.kind {
            Kind::Window => {
                if w.haccel != 0 {
                    DestroyAcceleratorTable(w.haccel);
                }
                DestroyWindow(w.hwnd);
            }
            Kind::Page => {
                if let (Some(t), Some(pos)) = (w.parent.and_then(|p| get(p, |t| t.hwnd)), pos) {
                    send(t, TCM_DELETEITEM, pos, 0);
                }
                DestroyWindow(w.hwnd);
                if let Some(t) = w.parent {
                    position_pages(t);
                }
            }
            Kind::PopupMenu => {
                DestroyMenu(w.hmenu);
            }
            Kind::MenuBar => {
                if let Some(win) = w.parent.and_then(|p| get(p, |x| x.hwnd)) {
                    SetMenu(win, 0);
                }
                DestroyMenu(w.hmenu);
                if let Some(p) = w.parent {
                    menu_changed(p);
                }
            }
            Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
                if let (Some(pm), Some(pos)) = (w.parent.and_then(|p| get(p, |x| x.hmenu)), pos) {
                    DeleteMenu(pm, pos as u32, MF_BYPOSITION);
                }
                menu_changed(w.win);
            }
            _ => {
                if w.has_tip {
                    remove_tooltip(w.win, w.hwnd);
                }
                if w.hbmp != 0 {
                    DeleteObject(w.hbmp);
                }
                if w.aux != 0 && w.kind != Kind::GroupBox {
                    DestroyWindow(w.aux);
                }
                DestroyWindow(w.hwnd);
            }
        }
    }
    if w.accel.is_some() {
        rebuild_accel(w.win);
    }
}

/// Take the tool of control `h` out of its window's tooltip control. The tool refers to a text
/// buffer owned by the widget record, which is about to go away.
fn remove_tooltip(win: WidgetId, h: HWND) {
    let Some((owner, tip)) = get(win, |w| (w.hwnd, w.tip_hwnd)) else {
        return;
    };
    if tip == 0 {
        return;
    }
    let ti = TOOLINFOW {
        cbSize: std::mem::size_of::<TOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: owner,
        uId: h as usize,
        rect: RECT::default(),
        hinst: 0,
        lpszText: null_mut(),
        lParam: 0,
        lpReserved: null_mut(),
    };
    send(tip, TTM_DELTOOLW, 0, &ti as *const _ as isize);
}

// ------------------------------------------------------------------ menus

fn menu_changed(win: WidgetId) {
    let Some((h, req)) = get(win, |w| (w.hwnd, w.client_req)) else {
        return;
    };
    if h == 0 {
        return; // a PopupMenu has no window
    }
    unsafe {
        DrawMenuBar(h);
    }
    if let Some(sz) = req {
        set_client_size(win, sz);
    }
}

fn menu_label(w: &W) -> String {
    let mut s = esc_amp(&w.text);
    if let Some(a) = &w.accel {
        s.push('\t');
        s.push_str(a);
    }
    s
}

fn menu_update_text(id: WidgetId) {
    let Some((kind, cmd, win, pm, pos, label)) = st(|s| {
        let w = s.widgets.get(&id)?;
        let p = s.widgets.get(&w.parent?)?;
        let pos = p.children.iter().position(|c| *c == id)?;
        Some((w.kind, w.cmd, w.win, p.hmenu, pos, menu_label(w)))
    })
    .flatten() else {
        return;
    };
    let mut buf = wide(&label);
    let mii = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING,
        fType: 0,
        fState: 0,
        wID: 0,
        hSubMenu: 0,
        hbmpChecked: 0,
        hbmpUnchecked: 0,
        dwItemData: 0,
        dwTypeData: buf.as_mut_ptr(),
        cch: 0,
        hbmpItem: 0,
    };
    let by_pos = kind == Kind::Menu;
    unsafe {
        SetMenuItemInfoW(
            pm,
            if by_pos { pos as u32 } else { cmd as u32 },
            by_pos as BOOL,
            &mii,
        );
    }
    menu_changed(win);
}

fn rebuild_accel(win: WidgetId) {
    if get(win, |w| w.kind) != Some(Kind::Window) {
        return; // popup menus show accelerators but never register them
    }
    let list: Vec<ACCEL> = st(|s| {
        let mut v = vec![];
        for w in s.widgets.values() {
            if w.win != win || w.cmd == 0 {
                continue;
            }
            let Some(a) = w.accel.as_deref().and_then(Accel::parse) else {
                continue;
            };
            let Some(vk) = vk_for(&a.key) else { continue };
            let mut f = FVIRTKEY;
            if a.ctrl {
                f |= FCONTROL;
            }
            if a.shift {
                f |= FSHIFT;
            }
            if a.alt {
                f |= FALT;
            }
            v.push(ACCEL {
                fVirt: f,
                key: vk,
                cmd: w.cmd,
            });
        }
        v
    })
    .unwrap_or_default();
    let new = if list.is_empty() {
        0
    } else {
        unsafe { CreateAcceleratorTableW(list.as_ptr(), list.len() as i32) }
    };
    let old = with_w(win, |w| std::mem::replace(&mut w.haccel, new)).unwrap_or(0);
    if old != 0 {
        unsafe {
            DestroyAcceleratorTable(old);
        }
    }
}

// ------------------------------------------------------------------ geometry

fn set_client_size(win: WidgetId, sz: Size) {
    let Some((h, dpi, has_menu, fns)) = st(|s| {
        let w = s.widgets.get(&win)?;
        Some((
            w.hwnd,
            w.dpi,
            w.menubar != 0
                || w.children
                    .iter()
                    .any(|c| s.widgets.get(c).is_some_and(|x| x.kind == Kind::MenuBar)),
            s.fns,
        ))
    })
    .flatten() else {
        return;
    };
    with_w(win, |w| w.client_req = Some(sz));
    let (cw, ch) = (px(sz.w.max(1), dpi), px(sz.h.max(1), dpi));
    unsafe {
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: cw,
            bottom: ch,
        };
        match fns.adjust_rect_dpi {
            Some(f) => {
                f(&mut rc, style, has_menu as BOOL, ex, dpi);
            }
            None => {
                AdjustWindowRectEx(&mut rc, style, has_menu as BOOL, ex);
            }
        }
        let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        SetWindowPos(h, 0, 0, 0, rc.right - rc.left, rc.bottom - rc.top, flags);
        // a wrapped menu bar changes the client height: correct once
        let mut cr = RECT::default();
        GetClientRect(h, &mut cr);
        let (dx, dy) = (cw - (cr.right - cr.left), ch - (cr.bottom - cr.top));
        if dx != 0 || dy != 0 {
            SetWindowPos(
                h,
                0,
                0,
                0,
                rc.right - rc.left + dx,
                rc.bottom - rc.top + dy,
                flags,
            );
        }
    }
}

fn client_logical(win: WidgetId) -> Option<Size> {
    let (h, dpi) = get(win, |w| (w.hwnd, w.dpi))?;
    let mut cr = RECT::default();
    unsafe {
        GetClientRect(h, &mut cr);
    }
    Some(Size::new(
        lp(cr.right - cr.left, dpi),
        lp(cr.bottom - cr.top, dpi),
    ))
}

/// Offset of a GroupBox's inner client area inside its outer rect, physical pixels.
fn group_inset(dpi: u32) -> (i32, i32, i32, i32) {
    (px(7, dpi), text_h(dpi) + px(4, dpi), px(7, dpi), px(7, dpi))
}

fn apply_bounds(id: WidgetId) {
    let Some((kind, h, aux, r, dpi, pkind)) = st(|s| {
        let w = s.widgets.get(&id)?;
        let dpi = s.widgets.get(&w.win).map_or(96, |x| x.dpi);
        let pkind = w.parent.and_then(|p| s.widgets.get(&p)).map(|p| p.kind);
        Some((w.kind, w.hwnd, w.aux, w.bounds, dpi, pkind))
    })
    .flatten() else {
        return;
    };
    let flags = SWP_NOZORDER | SWP_NOACTIVATE;
    let (mut x, mut y, w, hh) = (
        px(r.x, dpi),
        px(r.y, dpi),
        px(r.w.max(0), dpi),
        px(r.h.max(0), dpi),
    );
    match kind {
        Kind::Window => set_client_size(id, Size::new(r.w, r.h)),
        Kind::Page => {}
        Kind::Tabs => {
            unsafe {
                SetWindowPos(h, 0, x, y, w, hh, flags);
            }
            position_pages(id);
        }
        _ => {
            if pkind == Some(Kind::GroupBox) {
                let (l, t, _, _) = group_inset(dpi);
                x += l;
                y += t;
            }
            unsafe {
                match kind {
                    Kind::SpinBox => {
                        let uw = px(17, dpi).min(w / 2);
                        SetWindowPos(h, 0, x, y, (w - uw).max(0), hh, flags);
                        if aux != 0 {
                            SetWindowPos(aux, 0, x + w - uw, y, uw, hh, flags);
                        }
                    }
                    Kind::GroupBox => {
                        SetWindowPos(h, 0, x, y, w, hh, flags);
                        if aux != 0 {
                            SetWindowPos(aux, 0, 0, 0, w, hh, flags);
                        }
                    }
                    _ => {
                        SetWindowPos(h, 0, x, y, w, hh, flags);
                    }
                }
                if matches!(kind, Kind::Label) {
                    InvalidateRect(h, null(), 1);
                }
            }
        }
    }
}

fn tab_display_rect(tabs: HWND) -> RECT {
    let mut rc = RECT::default();
    unsafe {
        GetClientRect(tabs, &mut rc);
    }
    send(tabs, TCM_ADJUSTRECT, 0, &mut rc as *mut _ as isize);
    rc
}

/// Place every page of `tabs` over its display area and show only the selected one.
fn position_pages(tabs: WidgetId) {
    let Some((th, pages, sel)) = st(|s| {
        let t = s.widgets.get(&tabs)?;
        let pages: Vec<(HWND, bool)> = t
            .children
            .iter()
            .filter_map(|c| s.widgets.get(c))
            .map(|p| (p.hwnd, p.vis))
            .collect();
        Some((t.hwnd, pages, t.selected))
    })
    .flatten() else {
        return;
    };
    let d = tab_display_rect(th);
    // page windows are siblings of the tab control: place them in the parent's client coordinates
    let mut origin = POINT::default();
    unsafe {
        MapWindowPoints(th, GetParent(th), &mut origin, 1);
    }
    let (ox, oy) = (origin.x + d.left, origin.y + d.top);
    unsafe {
        for (i, (ph, vis)) in pages.iter().enumerate() {
            SetWindowPos(
                *ph,
                0,
                ox,
                oy,
                (d.right - d.left).max(0),
                (d.bottom - d.top).max(0),
                SWP_NOACTIVATE,
            );
            let show = *vis && sel == Some(i);
            ShowWindow(*ph, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if show {
                RedrawWindow(
                    *ph,
                    null(),
                    0,
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
            }
        }
    }
}

fn preferred_impl(id: WidgetId) -> Option<Size> {
    let (kind, text, items, image, range, mono) = get(id, |w| {
        (
            w.kind,
            w.text.clone(),
            w.items.clone(),
            w.image.clone(),
            w.range,
            w.mono,
        )
    })?;
    let dpi = dpi_of(id);
    let th = text_h(dpi);
    let widest = |items: &[String]| items.iter().map(|i| measure(dpi, i).0).max().unwrap_or(0);
    let _ = range;
    let (w, h) = match kind {
        Kind::Label => {
            let (w, h) = measure(dpi, &text);
            (w + px(1, dpi), h)
        }
        Kind::Button => {
            let (w, _) = measure(dpi, &esc_text(&text));
            (
                (w + px(24, dpi)).max(px(60, dpi)),
                (th + px(10, dpi)).max(px(23, dpi)),
            )
        }
        Kind::CheckBox | Kind::RadioButton => {
            let (w, _) = measure(dpi, &esc_text(&text));
            (w + px(22, dpi), th.max(px(16, dpi)) + px(2, dpi))
        }
        Kind::TextInput | Kind::PasswordInput => {
            let th = if mono {
                measure_with(mono_font(dpi), "Ag").1
            } else {
                th
            };
            (px(160, dpi), th + px(8, dpi))
        }
        Kind::TextArea => {
            let th = if mono {
                measure_with(mono_font(dpi), "Ag").1
            } else {
                th
            };
            (px(if mono { 240 } else { 200 }, dpi), th * 5 + px(8, dpi))
        }
        Kind::Table => (px(300, dpi), px(150, dpi)),
        Kind::Tree => (px(200, dpi), px(200, dpi)),
        Kind::ComboBox => (
            (widest(&items) + px(34, dpi)).max(px(80, dpi)),
            th + px(10, dpi),
        ),
        Kind::ListBox => {
            let rows = items.len().clamp(3, 8) as i32;
            (
                (widest(&items) + px(30, dpi)).max(px(120, dpi)),
                rows * (th + px(1, dpi)) + px(6, dpi),
            )
        }
        Kind::Slider => (px(150, dpi), px(28, dpi)),
        Kind::ProgressBar => (px(150, dpi), px(16, dpi)),
        Kind::SpinBox => (px(80, dpi), th + px(8, dpi)),
        Kind::Image => match image {
            Some(i) => (px(i.w as i32, dpi), px(i.h as i32, dpi)),
            None => (px(32, dpi), px(32, dpi)),
        },
        _ => (0, 0),
    };
    Some(Size::new(lp(w, dpi), lp(h, dpi)))
}
fn esc_text(s: &str) -> String {
    crate::mnemonic::strip_mnemonic(s)
}

fn chrome_impl(id: WidgetId) -> Option<Size> {
    let (kind, h) = get(id, |w| (w.kind, w.hwnd))?;
    let dpi = dpi_of(id);
    match kind {
        Kind::GroupBox => {
            let (l, t, r, b) = group_inset(dpi);
            Some(Size::new(lp(l + r, dpi), lp(t + b, dpi)))
        }
        Kind::Tabs => {
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 1000,
                bottom: 1000,
            };
            send(h, TCM_ADJUSTRECT, 0, &mut rc as *mut _ as isize);
            let (dw, dh) = (1000 - (rc.right - rc.left), 1000 - (rc.bottom - rc.top));
            Some(Size::new(lp(dw, dpi), lp(dh, dpi)))
        }
        _ => None,
    }
}

// ------------------------------------------------------------------ properties

fn update_tooltip(id: WidgetId, text: &str) {
    let Some((h, win, had)) = get(id, |w| (w.hwnd, w.win, w.has_tip)) else {
        return;
    };
    let Some((owner, mut tip, dpi)) = get(win, |w| (w.hwnd, w.tip_hwnd, w.dpi)) else {
        return;
    };
    let inst = st(|s| s.inst).unwrap_or(0);
    if h == 0 || matches!(get(id, |w| w.kind), Some(Kind::Window)) {
        return;
    }
    unsafe {
        if tip == 0 {
            if text.is_empty() {
                return;
            }
            tip = CreateWindowExW(
                8,
                wide("tooltips_class32").as_ptr(),
                null(),
                0x8000_0000 | TTS_ALWAYSTIP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                owner,
                0,
                inst,
                null_mut(),
            );
            if tip == 0 {
                return;
            }
            send(tip, TTM_SETMAXTIPWIDTH, 0, px(400, dpi) as isize);
            with_w(win, |w| w.tip_hwnd = tip);
        }
        let mut ti = TOOLINFOW {
            cbSize: std::mem::size_of::<TOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: owner,
            uId: h as usize,
            rect: RECT::default(),
            hinst: 0,
            lpszText: null_mut(),
            lParam: 0,
            lpReserved: null_mut(),
        };
        if text.is_empty() {
            if had {
                send(tip, TTM_DELTOOLW, 0, &ti as *const _ as isize);
                with_w(id, |w| {
                    w.has_tip = false;
                    w.tip.clear();
                });
            }
            return;
        }
        // the tooltip keeps the pointer, so the buffer lives in the widget record
        let ptr = with_w(id, |w| {
            w.tip = wide(text);
            w.has_tip = true;
            w.tip.as_mut_ptr()
        });
        let Some(ptr) = ptr else { return };
        ti.lpszText = ptr;
        send(
            tip,
            if had {
                TTM_UPDATETIPTEXTW
            } else {
                TTM_ADDTOOLW
            },
            0,
            &ti as *const _ as isize,
        );
    }
}

fn build_bitmap(img: &ImageData) -> isize {
    if !img.is_valid() {
        return 0;
    }
    unsafe {
        let bi = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: img.w as i32,
            biHeight: -(img.h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut bits: *mut c_void = null_mut();
        let bmp = CreateDIBSection(0, &bi, 0, &mut bits, 0, 0);
        if bmp == 0 || bits.is_null() {
            return 0;
        }
        let dst =
            std::slice::from_raw_parts_mut(bits as *mut u8, img.w as usize * img.h as usize * 4);
        for (d, s) in dst.chunks_exact_mut(4).zip(img.rgba.chunks_exact(4)) {
            let a = s[3] as u32;
            d[0] = (s[2] as u32 * a / 255) as u8;
            d[1] = (s[1] as u32 * a / 255) as u8;
            d[2] = (s[0] as u32 * a / 255) as u8;
            d[3] = s[3];
        }
        bmp
    }
}

fn set_pages_visible(tabs: WidgetId) {
    position_pages(tabs);
}

fn set_impl(id: WidgetId, prop: &Prop) {
    let Some((kind, h, aux, win)) = get(id, |w| (w.kind, w.hwnd, w.w_aux(), w.win)) else {
        return;
    };
    let dpi = dpi_of(id);
    match prop {
        Prop::Text(t) => {
            with_w(id, |w| w.text = t.to_string());
            match kind {
                Kind::Window => set_text(h, t),
                Kind::Label => {
                    set_text(h, t);
                    unsafe {
                        InvalidateRect(h, null(), 1);
                    }
                }
                Kind::Button | Kind::CheckBox | Kind::RadioButton => set_text(h, &esc_amp(t)),
                Kind::GroupBox => set_text(aux, &esc_amp(t)),
                Kind::TextInput | Kind::PasswordInput | Kind::TextArea => {
                    let multi = kind == Kind::TextArea;
                    if get_text(h, multi) != *t {
                        set_text(h, &if multi { nl_in(t) } else { t.to_string() });
                    }
                }
                Kind::Page => {
                    if let Some(th) = get(id, |w| w.parent)
                        .flatten()
                        .and_then(|p| get(p, |x| x.hwnd))
                    {
                        let idx = page_index(id);
                        let mut buf = wide(t);
                        let item = TCITEMW {
                            mask: TCIF_TEXT,
                            dwState: 0,
                            dwStateMask: 0,
                            pszText: buf.as_mut_ptr(),
                            cchTextMax: 0,
                            iImage: -1,
                            lParam: 0,
                        };
                        send(th, TCM_SETITEMW, idx, &item as *const _ as isize);
                        position_pages(get(id, |w| w.parent).flatten().unwrap_or(WidgetId::DEAD));
                    }
                }
                Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem => menu_update_text(id),
                _ => {}
            }
        }
        Prop::Tooltip(t) => {
            if matches!(
                kind,
                Kind::MenuItem
                    | Kind::CheckMenuItem
                    | Kind::Menu
                    | Kind::MenuBar
                    | Kind::MenuSeparator
                    | Kind::Page
            ) {
                return;
            }
            update_tooltip(id, t);
        }
        Prop::Placeholder(t) => {
            with_w(id, |w| w.placeholder = t.to_string());
            if matches!(kind, Kind::TextInput | Kind::PasswordInput) {
                let w = wide(t);
                send(h, EM_SETCUEBANNER, 1, w.as_ptr() as isize);
            }
        }
        Prop::Enabled(e) => {
            with_w(id, |w| w.enabled = *e);
            match kind {
                Kind::MenuItem | Kind::CheckMenuItem | Kind::Menu => {
                    let Some((pm, cmd, pos)) = st(|s| {
                        let w = s.widgets.get(&id)?;
                        let p = s.widgets.get(&w.parent?)?;
                        Some((p.hmenu, w.cmd, p.children.iter().position(|c| *c == id)?))
                    })
                    .flatten() else {
                        return;
                    };
                    let (item, by) = if kind == Kind::Menu {
                        (pos as u32, MF_BYPOSITION)
                    } else {
                        (cmd as u32, MF_BYCOMMAND)
                    };
                    unsafe {
                        EnableMenuItem(pm, item, by | if *e { 0 } else { MF_GRAYED });
                    }
                    menu_changed(win);
                }
                Kind::MenuBar | Kind::MenuSeparator => {}
                _ => unsafe {
                    EnableWindow(h, *e as BOOL);
                    if aux != 0 {
                        EnableWindow(aux, *e as BOOL);
                    }
                },
            }
        }
        Prop::Visible(v) => {
            with_w(id, |w| w.vis = *v);
            match kind {
                Kind::Window => unsafe {
                    if *v {
                        let first =
                            with_w(id, |w| !std::mem::replace(&mut w.shown, true)).unwrap_or(false);
                        ShowWindow(h, SW_SHOW);
                        UpdateWindow(h);
                        if first {
                            let mut rc = RECT::default();
                            GetWindowRect(h, &mut rc);
                            with_w(id, |w| {
                                w.last_pos = Some((lp(rc.left, w.dpi), lp(rc.top, w.dpi)))
                            });
                        }
                        if first {
                            // DefWindowProc ignores WM_NEXTDLGCTL: pick the first tab stop ourselves
                            let f = GetNextDlgTabItem(h, 0, 0);
                            if f != 0 {
                                SetFocus(f);
                            }
                        }
                    } else {
                        ShowWindow(h, SW_HIDE);
                    }
                },
                Kind::Page => {
                    if let Some(t) = get(id, |w| w.parent).flatten() {
                        set_pages_visible(t);
                    }
                }
                Kind::Menu
                | Kind::MenuBar
                | Kind::MenuItem
                | Kind::CheckMenuItem
                | Kind::MenuSeparator => {}
                _ => unsafe {
                    let c = if *v { SW_SHOWNOACTIVATE } else { SW_HIDE };
                    ShowWindow(h, c);
                    if aux != 0 && kind != Kind::GroupBox {
                        ShowWindow(aux, c);
                    }
                },
            }
        }
        Prop::Checked(c) => {
            with_w(id, |w| w.checked = *c);
            match kind {
                Kind::CheckBox | Kind::RadioButton => {
                    send(h, BM_SETCHECK, *c as usize, 0);
                }
                Kind::CheckMenuItem => {
                    if let Some((pm, cmd)) = st(|s| {
                        let w = s.widgets.get(&id)?;
                        Some((s.widgets.get(&w.parent?)?.hmenu, w.cmd))
                    })
                    .flatten()
                    {
                        unsafe {
                            CheckMenuItem(
                                pm,
                                cmd as u32,
                                MF_BYCOMMAND | if *c { MF_CHECKED } else { 0 },
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        Prop::Range { min, max, step } => {
            with_w(id, |w| w.range = (*min, *max, *step));
            if kind == Kind::Slider {
                send(
                    h,
                    TBM_SETRANGE,
                    1,
                    (slider_steps((*min, *max, *step)) as isize) << 16,
                );
                let v = get(id, |w| w.value).unwrap_or(*min);
                apply_value(id, v);
            }
        }
        Prop::Value(v) => apply_value(id, *v),
        Prop::Items(items) => {
            with_w(id, |w| w.items = items.to_vec());
            let (reset, add) = match kind {
                Kind::ComboBox => (CB_RESETCONTENT, CB_ADDSTRING),
                Kind::ListBox => (LB_RESETCONTENT, LB_ADDSTRING),
                _ => return,
            };
            send(h, reset, 0, 0);
            for it in items.iter() {
                let w = wide(it);
                send(h, add, 0, w.as_ptr() as isize);
            }
            let sel = get(id, |w| w.selected)
                .flatten()
                .filter(|i| *i < items.len());
            set_selection(id, sel);
        }
        Prop::Selected(s) => {
            with_w(id, |w| w.selected = *s);
            set_selection(id, *s);
            if kind == Kind::Tabs {
                position_pages(id);
            }
        }
        Prop::Bounds(r) => {
            with_w(id, |w| w.bounds = *r);
            apply_bounds(id);
        }
        Prop::Image(img) => {
            if kind != Kind::Image {
                return;
            }
            let bmp = img.map_or(0, build_bitmap);
            let old = with_w(id, |w| {
                w.image = img.cloned();
                std::mem::replace(&mut w.hbmp, bmp)
            })
            .unwrap_or(0);
            if old != 0 {
                unsafe {
                    DeleteObject(old);
                }
            }
            unsafe {
                InvalidateRect(h, null(), 1);
            }
        }
        Prop::Accel(a) => {
            with_w(id, |w| {
                w.accel = if a.is_empty() {
                    None
                } else {
                    Some(a.to_string())
                }
            });
            if matches!(kind, Kind::MenuItem | Kind::CheckMenuItem) {
                menu_update_text(id);
                rebuild_accel(win);
            }
        }
        Prop::ReadOnly(b) => {
            with_w(id, |w| w.readonly = *b);
            if matches!(
                kind,
                Kind::TextInput | Kind::PasswordInput | Kind::TextArea | Kind::SpinBox
            ) {
                send(h, EM_SETREADONLY, *b as usize, 0);
            }
        }
        Prop::Indeterminate(b) => {
            if kind == Kind::ProgressBar {
                unsafe {
                    let st_ = GetWindowLongPtrW(h, GWL_STYLE);
                    let n = if *b {
                        st_ | PBS_MARQUEE as isize
                    } else {
                        st_ & !(PBS_MARQUEE as isize)
                    };
                    SetWindowLongPtrW(h, GWL_STYLE, n);
                }
                send(h, PBM_SETMARQUEE, *b as usize, 30);
            }
        }
        Prop::Resizable(b) => {
            if kind == Kind::Window {
                unsafe {
                    let s0 = GetWindowLongPtrW(h, GWL_STYLE);
                    let bits = (WS_THICKFRAME | WS_MAXIMIZEBOX) as isize;
                    SetWindowLongPtrW(h, GWL_STYLE, if *b { s0 | bits } else { s0 & !bits });
                    SetWindowPos(
                        h,
                        0,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
                if let Some(sz) = get(id, |w| w.client_req).flatten() {
                    set_client_size(id, sz);
                }
            }
        }
        Prop::Columns(cols) => {
            if kind == Kind::Table {
                table_set_columns(id, cols);
            }
        }
        Prop::Rows(rows) => {
            if kind == Kind::Table {
                table_set_rows(id, rows);
            }
        }
        Prop::SortIndicator(s) => {
            if kind == Kind::Table {
                table_set_sort(id, *s);
            }
        }
        Prop::TreeRows(rows) => {
            if kind == Kind::Tree {
                tree_set_rows(id, rows);
            }
        }
        Prop::TreeSelected(n) => {
            if kind == Kind::Tree {
                tree_select(id, *n);
            }
        }
        Prop::Orientation(o) => {
            if kind == Kind::Sash {
                with_w(id, |w| w.vertical = *o == Orientation::Vertical);
            }
        }
        Prop::Monospace(on) => {
            if matches!(kind, Kind::TextInput | Kind::PasswordInput | Kind::TextArea) {
                with_w(id, |w| w.mono = *on);
                send(h, WM_SETFONT, font_of(id, dpi) as usize, 1);
            }
        }
        Prop::Wrap(on) => {
            if kind == Kind::TextArea && get(id, |w| w.wrap) != Some(*on) {
                with_w(id, |w| w.wrap = *on);
                recreate_edit(id);
            }
        }
        Prop::Position { x, y } => {
            if kind == Kind::Window {
                unsafe {
                    SetWindowPos(
                        h,
                        0,
                        px(*x, dpi),
                        px(*y, dpi),
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                with_w(id, |w| w.last_pos = Some((*x, *y)));
            }
        }
        Prop::MinSize(sz) => {
            if kind == Kind::Window {
                with_w(id, |w| w.min_size = *sz);
                if let Some(c) = get(id, |w| w.client_req).flatten() {
                    // grow the window if it is already smaller than the new minimum
                    set_client_size(id, Size::new(c.w.max(sz.w), c.h.max(sz.h)));
                }
            }
        }
        Prop::Focus => {
            if h != 0 {
                unsafe {
                    SetFocus(h);
                }
            }
        }
        #[allow(unreachable_patterns)] // new Props are ignored until a backend handles them
        _ => {}
    }
    let _ = dpi;
}

impl W {
    fn w_aux(&self) -> HWND {
        self.aux
    }
}

fn parent_hwnd(id: WidgetId) -> Option<HWND> {
    st(|s| {
        let p = s.widgets.get(&id)?.parent?;
        s.widgets.get(&p).map(|p| p.hwnd)
    })
    .flatten()
}

fn page_index(page: WidgetId) -> usize {
    st(|s| {
        let p = s.widgets.get(&page)?.parent?;
        s.widgets.get(&p)?.children.iter().position(|c| *c == page)
    })
    .flatten()
    .unwrap_or(0)
}

fn set_selection(id: WidgetId, sel: Option<usize>) {
    let Some((kind, h)) = get(id, |w| (w.kind, w.hwnd)) else {
        return;
    };
    let v = sel.map_or(usize::MAX, |i| i);
    match kind {
        Kind::ComboBox => {
            send(h, CB_SETCURSEL, v, 0);
        }
        Kind::ListBox => {
            send(h, LB_SETCURSEL, v, 0);
        }
        Kind::Tabs => {
            if let Some(i) = sel {
                send(h, TCM_SETCURSEL, i, 0);
            }
        }
        Kind::Table => {
            let mut it = LVITEMW {
                mask: 0,
                iItem: 0,
                iSubItem: 0,
                state: 0,
                stateMask: LVIS_SELECTED,
                pszText: null_mut(),
                cchTextMax: 0,
                iImage: 0,
                lParam: 0,
            };
            send(h, LVM_SETITEMSTATE, usize::MAX, &it as *const _ as isize);
            if let Some(i) = sel {
                it.state = LVIS_SELECTED | LVIS_FOCUSED;
                it.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
                send(h, LVM_SETITEMSTATE, i, &it as *const _ as isize);
                send(h, LVM_ENSUREVISIBLE, i, 0);
            }
            with_w(id, |w| w.last_sel = sel);
        }
        _ => {}
    }
}

fn apply_value(id: WidgetId, v: f64) {
    let Some((kind, h, r)) = get(id, |w| (w.kind, w.hwnd, w.range)) else {
        return;
    };
    with_w(id, |w| w.value = v);
    match kind {
        Kind::Slider => {
            let n = slider_steps(r);
            let span = r.1 - r.0;
            let pos = if span > 0.0 {
                ((v - r.0) / span * n as f64).round() as i32
            } else {
                0
            };
            send(h, TBM_SETPOS, 1, pos.clamp(0, n) as isize);
        }
        Kind::ProgressBar => {
            send(
                h,
                PBM_SETPOS,
                (v.clamp(0.0, 1.0) * 1000.0).round() as usize,
                0,
            );
        }
        Kind::SpinBox => set_text(h, &fmt_value(v, r.2)),
        _ => {}
    }
}

// ------------------------------------------------------------------ window procedures

unsafe extern "system" fn window_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| handle_msg(h, m, w, l))) {
        Ok(Some(r)) => r,
        _ => unsafe { DefWindowProcW(h, m, w, l) },
    }
}

fn emit(id: WidgetId, ev: Event) {
    core::event(id, ev);
}

fn handle_msg(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let id = id_of(h)?;
    let kind = get(id, |x| x.kind)?;
    match m {
        WM_CLOSE if kind == Kind::Window => {
            core::close_requested(id);
            Some(0)
        }
        WM_SIZE if kind == Kind::Window => {
            if !muted() && w != SIZE_MINIMIZED {
                let dpi = get(id, |x| x.dpi).unwrap_or(96);
                let sz = Size::new(
                    lp(loword(l as usize) as i32, dpi),
                    lp(hiword(l as usize) as i32, dpi),
                );
                with_w(id, |x| x.client_req = Some(sz));
                emit(id, Event::Resized { w: sz.w, h: sz.h });
            }
            Some(0)
        }
        WM_CONTEXTMENU => {
            // controls handle their own (ctl_proc); this is the window, a container or a
            // non-subclassed child (label, image, progress bar) whose message bubbled up
            let src = id_of(w as HWND).unwrap_or(id);
            let own = get(src, |x| x.kind).is_some_and(|k| {
                !matches!(
                    k,
                    Kind::Label
                        | Kind::Image
                        | Kind::ProgressBar
                        | Kind::Window
                        | Kind::Page
                        | Kind::GroupBox
                )
            });
            if !own {
                context_menu(src, l);
            }
            Some(0)
        }
        WM_MOVE if kind == Kind::Window => {
            if !muted() {
                let dpi = get(id, |x| x.dpi).unwrap_or(96);
                let mut rc = RECT::default();
                unsafe {
                    GetWindowRect(h, &mut rc);
                }
                let pos = (lp(rc.left, dpi), lp(rc.top, dpi));
                // only once shown (last_pos is recorded then) and only real changes
                if get(id, |x| x.last_pos).flatten().is_some_and(|p| p != pos) {
                    with_w(id, |x| x.last_pos = Some(pos));
                    emit(id, Event::Moved { x: pos.0, y: pos.1 });
                }
            }
            Some(0)
        }
        WM_GETMINMAXINFO if kind == Kind::Window => {
            let (ms, dpi, has_menu, fns) = st(|s| {
                let x = s.widgets.get(&id)?;
                Some((
                    x.min_size,
                    x.dpi,
                    x.children
                        .iter()
                        .any(|c| s.widgets.get(c).is_some_and(|k| k.kind == Kind::MenuBar)),
                    s.fns,
                ))
            })
            .flatten()?;
            if ms.w <= 0 && ms.h <= 0 {
                return None;
            }
            unsafe {
                let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
                let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
                let mut rc = RECT {
                    left: 0,
                    top: 0,
                    right: px(ms.w.max(0), dpi),
                    bottom: px(ms.h.max(0), dpi),
                };
                match fns.adjust_rect_dpi {
                    Some(f) => {
                        f(&mut rc, style, has_menu as BOOL, ex, dpi);
                    }
                    None => {
                        AdjustWindowRectEx(&mut rc, style, has_menu as BOOL, ex);
                    }
                }
                let mmi = &mut *(l as *mut MINMAXINFO);
                mmi.ptMinTrackSize = POINT {
                    x: rc.right - rc.left,
                    y: rc.bottom - rc.top,
                };
            }
            Some(0)
        }
        WM_DPICHANGED if kind == Kind::Window => {
            on_dpi_changed(id, h, loword(w), l);
            Some(0)
        }
        WM_ACTIVATE if kind == Kind::Window => {
            if loword(w) != 0 {
                let lf = get(id, |x| x.last_focus).unwrap_or(0);
                unsafe {
                    if lf != 0 && IsWindow(lf) != 0 && IsChild(h, lf) != 0 {
                        SetFocus(lf);
                    }
                }
            }
            None
        }
        WM_COMMAND => on_command(w, l),
        WM_NOTIFY => on_notify(l),
        WM_HSCROLL => {
            if l != 0 && !muted() {
                if let Some(sid) = id_of(l) {
                    if let Some((r, old)) = get(sid, |x| (x.range, x.value)) {
                        let n = slider_steps(r);
                        let pos = send(l, TBM_GETPOS, 0, 0) as i32;
                        let v = (r.0 + (r.1 - r.0) * pos as f64 / n as f64)
                            .clamp(r.0.min(r.1), r.1.max(r.0));
                        if (v - old).abs() > f64::EPSILON {
                            with_w(sid, |x| x.value = v);
                            emit(sid, Event::Value(v));
                        }
                    }
                }
            }
            Some(0)
        }
        WM_DRAWITEM => {
            let ds = unsafe { &*(l as *const DRAWITEMSTRUCT) };
            if let Some(iid) = id_of(ds.hwndItem) {
                draw_image(iid, ds);
                return Some(1);
            }
            None
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let sid = id_of(l)?;
            if matches!(
                get(sid, |x| x.kind),
                Some(
                    Kind::Label
                        | Kind::GroupBox
                        | Kind::CheckBox
                        | Kind::RadioButton
                        | Kind::Slider
                )
            ) {
                unsafe {
                    // paint what the parent would show behind the label (page body, group box...)
                    fill_bg(l, w as isize, true);
                    SetBkMode(w as isize, TRANSPARENT);
                    return Some(GetStockObject(NULL_BRUSH));
                }
            }
            None
        }
        WM_ERASEBKGND if kind == Kind::Page => {
            unsafe {
                if !paint_page_body(h, h, w as isize) {
                    let mut rc = RECT::default();
                    GetClientRect(h, &mut rc);
                    FillRect(w as isize, &rc, GetSysColorBrush(COLOR_BTNFACE));
                }
            }
            Some(1)
        }
        WM_ERASEBKGND if kind == Kind::GroupBox => {
            unsafe {
                fill_bg(h, w as isize, true);
            }
            Some(1)
        }
        _ => None,
    }
}

/// Paint the themed tab-body background of the page `page` into `dc`, the DC of its descendant
/// `h` (client origin of `h`). Returns false when there is no theme.
unsafe fn paint_page_body(page: HWND, h: HWND, dc: isize) -> bool {
    unsafe {
        let Some(tabs) = id_of(page)
            .and_then(|i| get(i, |x| x.parent))
            .flatten()
            .and_then(|p| get(p, |x| x.hwnd))
        else {
            return false;
        };
        let theme = OpenThemeData(page, wide("TAB").as_ptr());
        if theme == 0 {
            return false;
        }
        // the tab control's whole client rect, in the client coordinates of `h`
        let mut tc = RECT::default();
        GetClientRect(tabs, &mut tc);
        let mut o = POINT::default();
        MapWindowPoints(tabs, h, &mut o, 1);
        let rc = RECT {
            left: o.x,
            top: o.y,
            right: o.x + tc.right,
            bottom: o.y + tc.bottom,
        };
        let mut vis = RECT::default();
        GetClientRect(h, &mut vis);
        DrawThemeBackground(theme, dc, TABP_BODY, 0, &rc, &vis);
        CloseThemeData(theme);
        true
    }
}

/// Background behind a widget: the themed page body when it sits on a tab page, else the dialog colour.
unsafe fn fill_bg(h: HWND, dc: isize, parent_bg: bool) {
    unsafe {
        if parent_bg {
            let mut p = GetParent(h);
            while p != 0 {
                if id_of(p).and_then(|i| get(i, |x| x.kind)) == Some(Kind::Page) {
                    if paint_page_body(p, h, dc) {
                        return;
                    }
                    break;
                }
                p = GetParent(p);
            }
        }
        let mut rc = RECT::default();
        GetClientRect(h, &mut rc);
        FillRect(dc, &rc, GetSysColorBrush(COLOR_BTNFACE));
    }
}

fn draw_image(id: WidgetId, ds: &DRAWITEMSTRUCT) {
    unsafe {
        fill_bg(ds.hwndItem, ds.hDC, true);
        let Some((bmp, iw, ih)) = get(id, |w| {
            (
                w.hbmp,
                w.image.as_ref().map_or(0, |i| i.w as i32),
                w.image.as_ref().map_or(0, |i| i.h as i32),
            )
        }) else {
            return;
        };
        if bmp == 0 || iw == 0 || ih == 0 {
            return;
        }
        let mem = CreateCompatibleDC(ds.hDC);
        if mem == 0 {
            return;
        }
        let old = SelectObject(mem, bmp);
        let bf = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1,
        };
        let r = ds.rcItem;
        AlphaBlend(
            ds.hDC,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            mem,
            0,
            0,
            iw,
            ih,
            bf,
        );
        SelectObject(mem, old);
        DeleteDC(mem);
    }
}

fn on_dpi_changed(id: WidgetId, h: HWND, dpi: u32, l: LPARAM) {
    if dpi == 0 || l == 0 {
        return;
    }
    let ids: Vec<WidgetId> = {
        let _m = MuteGuard::new();
        with_w(id, |w| w.dpi = dpi);
        let r = unsafe { *(l as *const RECT) };
        unsafe {
            SetWindowPos(
                h,
                0,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let f = font(dpi) as usize;
        let list: Vec<(WidgetId, HWND, HWND)> = st(|s| {
            s.widgets
                .iter()
                .filter(|(_, w)| w.win == id && w.hwnd != 0)
                .map(|(i, w)| (*i, w.hwnd, w.aux))
                .collect()
        })
        .unwrap_or_default();
        for (wid_, hw, aux) in &list {
            send(*hw, WM_SETFONT, font_of(*wid_, dpi) as usize, 1);
            if *aux != 0 {
                send(*aux, WM_SETFONT, f, 1);
            }
        }
        let ids: Vec<WidgetId> = list.iter().map(|x| x.0).collect();
        for i in &ids {
            if *i != id {
                apply_bounds(*i);
            }
        }
        ids
    };
    let _ = ids;
    if let Some(sz) = client_logical(id) {
        emit(id, Event::Resized { w: sz.w, h: sz.h });
    }
}

fn on_command(w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let code = hiword(w);
    if l != 0 {
        if muted() {
            return Some(0);
        }
        let cid = id_of(l)?;
        let (kind, checked, range, text_h_) = get(cid, |x| (x.kind, x.checked, x.range, x.hwnd))?;
        match (kind, code) {
            (Kind::Button, 0) => emit(cid, Event::Click),
            (Kind::CheckBox, 0) => {
                let on = send(l, BM_GETCHECK, 0, 0) == 1;
                with_w(cid, |x| x.checked = on);
                emit(cid, Event::Toggled(on));
            }
            (Kind::RadioButton, 0) => {
                if !checked {
                    send(l, BM_SETCHECK, 1, 0);
                    with_w(cid, |x| x.checked = true);
                    emit(cid, Event::Toggled(true));
                }
            }
            (Kind::TextInput | Kind::PasswordInput | Kind::TextArea, 0x300) => {
                emit(cid, Event::Text(get_text(l, kind == Kind::TextArea)));
            }
            (Kind::SpinBox, 0x300) => {
                let t = get_text(text_h_, false).trim().replace(',', ".");
                if let Ok(v) = t.parse::<f64>() {
                    let v = v.clamp(range.0.min(range.1), range.1.max(range.0));
                    let old = get(cid, |x| x.value).unwrap_or(f64::NAN);
                    if v != old {
                        with_w(cid, |x| x.value = v);
                        emit(cid, Event::Value(v));
                    }
                }
            }
            (Kind::ComboBox, 1) => {
                let i = send(l, CB_GETCURSEL, 0, 0);
                let sel = if i < 0 { None } else { Some(i as usize) };
                with_w(cid, |x| x.selected = sel);
                emit(cid, Event::Selected(sel));
            }
            (Kind::ListBox, 1) => {
                let i = send(l, LB_GETCURSEL, 0, 0);
                let sel = if i < 0 { None } else { Some(i as usize) };
                with_w(cid, |x| x.selected = sel);
                emit(cid, Event::Selected(sel));
            }
            (Kind::ListBox, 2) => {
                let i = send(l, LB_GETCURSEL, 0, 0);
                if i >= 0 {
                    emit(cid, Event::Activated(i as usize));
                }
            }
            _ => {}
        }
        return Some(0);
    }
    // menu item or accelerator (code 0 / 1)
    if code <= 1 && !muted() {
        fire_menu_cmd(loword(w) as u16);
    }
    Some(0)
}

/// A menu item (menu bar, popup or accelerator) was chosen: emit `Click` / flip and emit `Toggled`.
fn fire_menu_cmd(cmd: u16) {
    if let Some(mid) = st(|s| s.by_cmd.get(&cmd).copied()).flatten() {
        match get(mid, |x| x.kind) {
            Some(Kind::CheckMenuItem) => {
                let on = !get(mid, |x| x.checked).unwrap_or(false);
                {
                    let _m = MuteGuard::new();
                    set_impl(mid, &Prop::Checked(on));
                }
                emit(mid, Event::Toggled(on));
            }
            Some(Kind::MenuItem) => emit(mid, Event::Click),
            _ => {}
        }
    }
}

fn on_notify(l: LPARAM) -> Option<LRESULT> {
    if l == 0 {
        return None;
    }
    let hdr = unsafe { &*(l as *const NMHDR) };
    let code = hdr.code as i32;
    let cid = id_of(hdr.hwndFrom)?;
    let kind = get(cid, |x| x.kind)?;
    match (kind, code) {
        (Kind::Tabs, TCN_SELCHANGE) if !muted() => {
            let i = send(hdr.hwndFrom, TCM_GETCURSEL, 0, 0);
            let sel = if i < 0 { None } else { Some(i as usize) };
            with_w(cid, |x| x.selected = sel);
            {
                let _m = MuteGuard::new();
                position_pages(cid);
            }
            emit(cid, Event::Selected(sel));
            Some(0)
        }
        (Kind::Table, LVN_ITEMCHANGED) if !muted() => {
            let nm = unsafe { &*(l as *const NMLISTVIEW) };
            if nm.uChanged & LVIF_STATE != 0 && (nm.uNewState ^ nm.uOldState) & LVIS_SELECTED != 0 {
                let msg = MSG_HWND.load(Ordering::SeqCst);
                unsafe {
                    PostMessageW(msg, WM_SELCHECK, cid.0 as usize, 0);
                }
            }
            Some(0)
        }
        (Kind::Table, LVN_COLUMNCLICK) if !muted() => {
            let nm = unsafe { &*(l as *const NMLISTVIEW) };
            if nm.iSubItem >= 0 {
                emit(cid, Event::ColumnClicked(nm.iSubItem as usize));
            }
            Some(0)
        }
        (Kind::Table, NM_DBLCLK) if !muted() => {
            let nm = unsafe { &*(l as *const NMLISTVIEW) };
            if nm.iItem >= 0 {
                emit(cid, Event::Activated(nm.iItem as usize));
            }
            Some(0)
        }
        (Kind::Table, LVN_KEYDOWN) if !muted() => {
            let vk = unsafe { *((l as *const u8).add(std::mem::size_of::<NMHDR>()) as *const u16) };
            if vk as usize == VK_RETURN {
                let i = send(
                    hdr.hwndFrom,
                    LVM_GETNEXTITEM,
                    usize::MAX,
                    LVNI_SELECTED as isize,
                );
                if i >= 0 {
                    emit(cid, Event::Activated(i as usize));
                }
            }
            Some(0)
        }
        (Kind::Tree, TVN_SELCHANGEDW) if !muted() => {
            let nm = unsafe { &*(l as *const NMTREEVIEWW) };
            let node = (nm.itemNew.hItem != 0).then_some(nm.itemNew.lParam as u64);
            if get(cid, |x| x.last_tsel) != Some(node) {
                with_w(cid, |x| x.last_tsel = node);
                emit(cid, Event::TreeSelected(node));
            }
            Some(0)
        }
        (Kind::Tree, TVN_ITEMEXPANDEDW) if !muted() => {
            let nm = unsafe { &*(l as *const NMTREEVIEWW) };
            let open = nm.action & 3 == TVE_EXPAND as u32;
            // the app usually reacts by replacing the rows: deliver outside this notification
            let _ =
                TREE_EXP.try_with(|q| q.borrow_mut().push((cid, nm.itemNew.lParam as u64, open)));
            unsafe {
                PostMessageW(MSG_HWND.load(Ordering::SeqCst), WM_TREEEXP, 0, 0);
            }
            Some(0)
        }
        (Kind::Tree, NM_DBLCLK) if !muted() => {
            let mut p = POINT::default();
            let mut ht = TVHITTESTINFO {
                pt: p,
                flags: 0,
                hItem: 0,
            };
            unsafe {
                GetCursorPos(&mut p);
                ScreenToClient(hdr.hwndFrom, &mut p);
            }
            ht.pt = p;
            send(hdr.hwndFrom, TVM_HITTEST, 0, &mut ht as *mut _ as isize);
            if ht.hItem != 0 && ht.flags & TVHT_ONITEMBUTTON == 0 {
                if let Some(n) = tree_node_of(hdr.hwndFrom, ht.hItem) {
                    emit(cid, Event::TreeActivated(n));
                }
            }
            Some(0)
        }
        (Kind::Tree, TVN_KEYDOWN) if !muted() => {
            let vk = unsafe { *((l as *const u8).add(std::mem::size_of::<NMHDR>()) as *const u16) };
            if vk as usize == VK_RETURN {
                tree_activate_selected(cid);
            }
            Some(0)
        }
        (Kind::SpinBox, UDN_DELTAPOS) => {
            if !muted() {
                let nm = unsafe { &*(l as *const NMUPDOWN) };
                let (r, old, eh) = get(cid, |x| (x.range, x.value, x.hwnd))?;
                let step = if r.2 > 0.0 { r.2 } else { 1.0 };
                // the field may hold unparsed text: start from what is shown
                let shown = get_text(eh, false)
                    .trim()
                    .replace(',', ".")
                    .parse::<f64>()
                    .unwrap_or(old);
                let v = (shown + nm.iDelta as f64 * step).clamp(r.0.min(r.1), r.1.max(r.0));
                {
                    let _m = MuteGuard::new();
                    apply_value(cid, v);
                }
                emit(cid, Event::Value(v));
            }
            Some(1) // we manage the value ourselves
        }
        _ => None,
    }
}

/// Subclass procedure for native controls: focus events, Enter in list boxes, spin-box normalisation.
unsafe extern "system" fn ctl_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let orig = ORIG
        .try_with(|o| o.borrow().get(&h).copied())
        .ok()
        .flatten()
        .unwrap_or(0);
    let call = || unsafe {
        if orig != 0 {
            CallWindowProcW(orig, h, m, w, l)
        } else {
            DefWindowProcW(h, m, w, l)
        }
    };
    if m == WM_CONTEXTMENU {
        if let Some(id) = id_of(h) {
            let kind = get(id, |x| x.kind);
            let shown = catch_unwind(AssertUnwindSafe(|| context_menu(id, l))).unwrap_or(false);
            // edit controls keep their own menu unless the app popped one up
            return if matches!(
                kind,
                Some(Kind::TextInput | Kind::PasswordInput | Kind::TextArea | Kind::SpinBox)
            ) && !shown
            {
                call()
            } else {
                0
            };
        }
    }
    if m == WM_ERASEBKGND && id_of(h).and_then(|i| get(i, |x| x.kind)) == Some(Kind::Slider) {
        // trackbars leave a grey box on page bodies: show what the parent shows
        unsafe { fill_bg(h, w as isize, true) };
        return 1;
    }
    let r = call();
    let _ = catch_unwind(AssertUnwindSafe(|| match m {
        WM_SETFOCUS | WM_KILLFOCUS => {
            if muted() {
                return;
            }
            let Some(id) = id_of(h) else { return };
            let Some((kind, win)) = get(id, |x| (x.kind, x.win)) else {
                return;
            };
            if m == WM_SETFOCUS {
                with_w(win, |x| x.last_focus = h);
            }
            if m == WM_KILLFOCUS && kind == Kind::SpinBox {
                spin_commit(id);
            }
            emit(id, Event::Focus(m == WM_SETFOCUS));
        }
        WM_KEYDOWN if w == VK_RETURN && !muted() => {
            if let Some(id) = id_of(h) {
                if get(id, |x| x.kind) == Some(Kind::ListBox) {
                    let i = send(h, LB_GETCURSEL, 0, 0);
                    if i >= 0 {
                        emit(id, Event::Activated(i as usize));
                    }
                }
            }
        }
        _ => {}
    }));
    if m == WM_NCDESTROY {
        let _ = ORIG.try_with(|o| o.borrow_mut().remove(&h));
    }
    r
}

/// Spin box lost focus: clamp, reformat the text and report a changed value.
fn spin_commit(id: WidgetId) {
    let Some((h, r, old)) = get(id, |x| (x.hwnd, x.range, x.value)) else {
        return;
    };
    let parsed = get_text(h, false)
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .unwrap_or(old);
    let v = parsed.clamp(r.0.min(r.1), r.1.max(r.0));
    {
        let _m = MuteGuard::new();
        apply_value(id, v);
    }
    if v != old {
        emit(id, Event::Value(v));
    }
}

// ------------------------------------------------------------------ table (SysListView32)

fn table_set_columns(id: WidgetId, cols: &[Column]) {
    let Some(h) = get(id, |w| w.hwnd) else { return };
    let dpi = dpi_of(id);
    with_w(id, |w| w.cols = cols.to_vec());
    // note: comctl32 always left-aligns the first column, whatever its `fmt`
    while send(h, LVM_DELETECOLUMN, 0, 0) != 0 {}
    for (i, c) in cols.iter().enumerate() {
        let mut title = wide(&c.title);
        let col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT | LVCF_SUBITEM,
            fmt: match c.align {
                ColumnAlign::Left => LVCFMT_LEFT,
                ColumnAlign::Center => LVCFMT_CENTER,
                ColumnAlign::Right => LVCFMT_RIGHT,
            },
            cx: px(c.width.max(0), dpi),
            pszText: title.as_mut_ptr(),
            cchTextMax: 0,
            iSubItem: i as i32,
            iImage: 0,
            iOrder: 0,
        };
        send(h, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
    }
}

fn table_set_rows(id: WidgetId, rows: &[Vec<String>]) {
    let Some((h, ncols)) = get(id, |w| (w.hwnd, w.cols.len().max(1))) else {
        return;
    };
    let item_y = |h: HWND| -> Option<i32> {
        if send(h, LVM_GETITEMCOUNT, 0, 0) == 0 {
            return None;
        }
        let mut p = POINT::default();
        (send(h, LVM_GETITEMPOSITION, 0, &mut p as *mut _ as isize) != 0).then_some(p.y)
    };
    let before = item_y(h);
    send(h, WM_SETREDRAW, 0, 0);
    send(h, LVM_DELETEALLITEMS, 0, 0);
    send(h, LVM_SETITEMCOUNT, rows.len(), 0);
    for (i, row) in rows.iter().enumerate() {
        for c in 0..ncols {
            let mut text = wide(row.get(c).map_or("", |s| s.as_str()));
            let it = LVITEMW {
                mask: LVIF_TEXT,
                iItem: i as i32,
                iSubItem: c as i32,
                state: 0,
                stateMask: 0,
                pszText: text.as_mut_ptr(),
                cchTextMax: 0,
                iImage: 0,
                lParam: 0,
            };
            if c == 0 {
                send(h, LVM_INSERTITEMW, 0, &it as *const _ as isize);
            } else {
                send(h, LVM_SETITEMTEXTW, i, &it as *const _ as isize);
            }
        }
    }
    // keep the scroll position across the rebuild
    if let (Some(b), Some(a)) = (before, item_y(h)) {
        if b != a {
            send(h, LVM_SCROLL, 0, (a - b) as isize);
        }
    }
    send(h, WM_SETREDRAW, 1, 0);
    unsafe {
        InvalidateRect(h, null(), 1);
    }
    with_w(id, |w| w.last_sel = None);
}

fn table_set_sort(id: WidgetId, sort: Option<(usize, bool)>) {
    let Some((h, n)) = get(id, |w| (w.hwnd, w.cols.len())) else {
        return;
    };
    let hdr = send(h, LVM_GETHEADER, 0, 0);
    if hdr == 0 {
        return;
    }
    for i in 0..n {
        let mut it = HDITEMW {
            mask: HDI_FORMAT,
            cxy: 0,
            pszText: null_mut(),
            hbm: 0,
            cchTextMax: 0,
            fmt: 0,
            lParam: 0,
            iImage: 0,
            iOrder: 0,
        };
        if send(hdr, HDM_GETITEMW, i, &mut it as *mut _ as isize) == 0 {
            continue;
        }
        it.fmt &= !(HDF_SORTUP | HDF_SORTDOWN);
        if let Some((c, asc)) = sort {
            if c == i {
                it.fmt |= if asc { HDF_SORTUP } else { HDF_SORTDOWN };
            }
        }
        send(hdr, HDM_SETITEMW, i, &it as *const _ as isize);
    }
}

/// The table's selection may have changed natively: report it once the notification burst is over
/// (a click deselects the old row and selects the new one in two notifications).
fn table_sel_check(id: WidgetId) {
    let Some((h, last)) = get(id, |w| (w.hwnd, w.last_sel)) else {
        return;
    };
    let i = send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    let sel = if i < 0 { None } else { Some(i as usize) };
    if sel != last {
        with_w(id, |w| w.last_sel = sel);
        emit(id, Event::Selected(sel));
    }
}

// ------------------------------------------------------------------ tree (SysTreeView32)

fn tree_node_of(h: HWND, item: isize) -> Option<u64> {
    let mut it = TVITEMEXW {
        mask: TVIF_PARAM,
        hItem: item,
        state: 0,
        stateMask: 0,
        pszText: null_mut(),
        cchTextMax: 0,
        iImage: 0,
        iSelectedImage: 0,
        cChildren: 0,
        lParam: 0,
    };
    (item != 0 && send(h, TVM_GETITEMW, 0, &mut it as *mut _ as isize) != 0)
        .then_some(it.lParam as u64)
}

fn tree_set_rows(id: WidgetId, rows: &[TreeRow]) {
    let Some(h) = get(id, |w| w.hwnd) else { return };
    let top = unsafe { GetScrollPos(h, SB_VERT) };
    send(h, WM_SETREDRAW, 0, 0);
    send(h, TVM_DELETEITEM, 0, TVI_ROOT);
    let mut stack: Vec<isize> = vec![];
    let mut items: Vec<isize> = Vec::with_capacity(rows.len());
    let mut nodes = HashMap::new();
    for r in rows {
        stack.truncate(r.depth as usize);
        let parent = stack.last().copied().unwrap_or(TVI_ROOT);
        let mut text = wide(&r.text);
        let ins = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            item: TVITEMEXW {
                mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN,
                hItem: 0,
                state: 0,
                stateMask: 0,
                pszText: text.as_mut_ptr(),
                cchTextMax: 0,
                iImage: 0,
                iSelectedImage: 0,
                cChildren: r.has_children as i32,
                lParam: r.node as isize,
            },
        };
        let it = send(h, TVM_INSERTITEMW, 0, &ins as *const _ as isize);
        stack.push(it);
        items.push(it);
        nodes.insert(r.node, it);
    }
    // expand parents before children (pre-order); only nodes with real child rows
    for (i, r) in rows.iter().enumerate() {
        if r.expanded && items[i] != 0 && rows.get(i + 1).is_some_and(|n| n.depth > r.depth) {
            send(h, TVM_EXPAND, TVE_EXPAND, items[i]);
        }
    }
    send(h, WM_SETREDRAW, 1, 0);
    if top > 0 {
        send(
            h,
            0x115, /* WM_VSCROLL */
            4 /* SB_THUMBPOSITION */ | ((top as usize) << 16),
            0,
        );
    }
    unsafe {
        InvalidateRect(h, null(), 1);
    }
    with_w(id, |w| {
        w.nodes = nodes;
        w.last_tsel = None;
    });
}

fn tree_select(id: WidgetId, node: Option<u64>) {
    let Some((h, item)) = get(id, |w| {
        (w.hwnd, node.and_then(|n| w.nodes.get(&n).copied()))
    }) else {
        return;
    };
    send(h, TVM_SELECTITEM, TVGN_CARET, item.unwrap_or(0));
    let cur = send(h, TVM_GETNEXTITEM, TVGN_CARET, 0);
    let sel = tree_node_of(h, cur);
    with_w(id, |w| w.last_tsel = sel);
}

fn tree_activate_selected(id: WidgetId) {
    let Some(h) = get(id, |w| w.hwnd) else { return };
    let cur = send(h, TVM_GETNEXTITEM, TVGN_CARET, 0);
    if let Some(n) = tree_node_of(h, cur) {
        emit(id, Event::TreeActivated(n));
    }
}

// ------------------------------------------------------------------ editing: wrap needs a new EDIT

/// WS_HSCROLL / ES_AUTOHSCROLL cannot be changed on a live EDIT control: build a new one with the
/// right styles and move all state over.
fn recreate_edit(id: WidgetId) {
    let Some((old, wrap, vis, enabled, ro, has_tip, tip, parent)) = get(id, |w| {
        (
            w.hwnd,
            w.wrap,
            w.vis,
            w.enabled,
            w.readonly,
            w.has_tip,
            String::from_utf16_lossy(&w.tip[..w.tip.len().saturating_sub(1)]),
            w.parent,
        )
    }) else {
        return;
    };
    let Some(phwnd) = parent.and_then(|p| get(p, |x| x.hwnd)) else {
        return;
    };
    let (inst, dpi) = (st(|s| s.inst).unwrap_or(0), dpi_of(id));
    let text = get_text(old, true);
    let (mut s0, mut s1) = (0u32, 0u32);
    send(
        old,
        EM_GETSEL,
        &mut s0 as *mut _ as usize,
        &mut s1 as *mut _ as isize,
    );
    let mut style =
        WS_CHILD | WS_TABSTOP | ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | WS_VSCROLL;
    if !wrap {
        style |= ES_AUTOHSCROLL | WS_HSCROLL;
    }
    if vis {
        style |= WS_VISIBLE;
    }
    let new = create_window_raw(WS_EX_CLIENTEDGE, "EDIT", style, phwnd, inst);
    if new == 0 {
        return;
    }
    unsafe {
        SetWindowPos(new, old, 0, 0, 10, 10, SWP_NOMOVE | SWP_NOACTIVATE);
    }
    send(new, WM_SETFONT, font_of(id, dpi) as usize, 1);
    send(new, EM_SETLIMITTEXT, 0, 0);
    send(new, EM_SETREADONLY, ro as usize, 0);
    unsafe {
        EnableWindow(new, enabled as BOOL);
    }
    subclass(new);
    let had_focus = unsafe { GetFocus() } == old;
    if has_tip {
        // the tooltip tool is keyed by the old HWND
        let win = get(id, |w| w.win).unwrap_or(id);
        if let Some((owner, tiph)) = get(win, |w| (w.hwnd, w.tip_hwnd)) {
            let ti = TOOLINFOW {
                cbSize: std::mem::size_of::<TOOLINFOW>() as u32,
                uFlags: TTF_IDISHWND,
                hwnd: owner,
                uId: old as usize,
                rect: RECT::default(),
                hinst: 0,
                lpszText: null_mut(),
                lParam: 0,
                lpReserved: null_mut(),
            };
            send(tiph, TTM_DELTOOLW, 0, &ti as *const _ as isize);
        }
    }
    st(|s| {
        s.by_hwnd.remove(&old);
        s.by_hwnd.insert(new, id);
        if let Some(w) = s.widgets.get_mut(&id) {
            w.hwnd = new;
            w.has_tip = false;
        }
    });
    if let Some(win) = get(id, |w| w.win) {
        a11y::rehome(win, old, new);
    }
    unsafe {
        DestroyWindow(old);
        if had_focus {
            SetFocus(new);
        }
    }
    if has_tip {
        update_tooltip(id, &tip);
    }
    // size first, then the text: wrapping is computed for the final width
    apply_bounds(id);
    set_text(new, &nl_in(&text));
    send(new, EM_SETSEL, s0 as usize, s1 as isize);
}

// ------------------------------------------------------------------ splitter sash

unsafe extern "system" fn sash_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| sash_msg(h, m, w, l))) {
        Ok(Some(r)) => r,
        _ => unsafe { DefWindowProcW(h, m, w, l) },
    }
}

/// Drag = leading-edge position at press + pointer delta (screen coordinates, so it does not drift
/// while the core moves the sash under the pointer). The core clamps and relayouts.
fn sash_msg(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let id = id_of(h)?;
    let (vertical, bounds, drag) = get(id, |x| (x.vertical, x.bounds, x.drag))?;
    let axis = |p: POINT| if vertical { p.y } else { p.x };
    match m {
        WM_SETCURSOR => unsafe {
            SetCursor(LoadCursorW(
                0,
                if vertical { IDC_SIZENS } else { IDC_SIZEWE },
            ));
            Some(1)
        },
        WM_ERASEBKGND => unsafe {
            fill_bg(h, w as isize, true);
            // keyboard focus: a plain focus rectangle (erasing is how a sash repaints)
            if GetFocus() == h {
                let mut rc = RECT::default();
                GetClientRect(h, &mut rc);
                DrawFocusRect(w as isize, &rc);
            }
            Some(1)
        },
        // the dialog manager (IsDialogMessage) would otherwise use the arrow keys to move the focus
        WM_GETDLGCODE => Some(DLGC_WANTARROWS),
        WM_SETFOCUS | WM_KILLFOCUS => unsafe {
            if m == WM_SETFOCUS {
                // so that re-activating the window puts the focus back here
                if let Some(win) = get(id, |x| x.win) {
                    with_w(win, |x| x.last_focus = h);
                }
            }
            InvalidateRect(h, std::ptr::null(), 1);
            None
        },
        // Arrow keys along the sash's axis (Shift = large step), Home and End. Other keys, and any
        // chord with Ctrl, are not ours.
        WM_KEYDOWN => unsafe {
            if muted() || GetKeyState(VK_CONTROL) < 0 {
                return None;
            }
            let big = GetKeyState(VK_SHIFT) < 0;
            let (prev, next) = if vertical {
                (VK_UP, VK_DOWN)
            } else {
                (VK_LEFT, VK_RIGHT)
            };
            let key = match w {
                VK_HOME => SashKey::Min,
                VK_END => SashKey::Max,
                k if k == prev => {
                    if big {
                        SashKey::PrevLarge
                    } else {
                        SashKey::Prev
                    }
                }
                k if k == next => {
                    if big {
                        SashKey::NextLarge
                    } else {
                        SashKey::Next
                    }
                }
                _ => return None,
            };
            emit(id, Event::SashKey(key));
            Some(0)
        },
        WM_LBUTTONDOWN => unsafe {
            SetFocus(h);
            let mut p = POINT::default();
            GetCursorPos(&mut p);
            with_w(id, |x| {
                x.drag = Some((axis(p), if vertical { bounds.y } else { bounds.x }))
            });
            SetCapture(h);
            Some(0)
        },
        WM_MOUSEMOVE => unsafe {
            if let Some((start, pos0)) = drag {
                if GetCapture() == h && !muted() {
                    let mut p = POINT::default();
                    GetCursorPos(&mut p);
                    emit(
                        id,
                        Event::SashDragged(pos0.saturating_add(lp(axis(p) - start, dpi_of(id)))),
                    );
                }
            }
            Some(0)
        },
        WM_LBUTTONUP => unsafe {
            with_w(id, |x| x.drag = None);
            ReleaseCapture();
            Some(0)
        },
        WM_CAPTURECHANGED => {
            with_w(id, |x| x.drag = None);
            Some(0)
        }
        _ => {
            let _ = l;
            None
        }
    }
}

// ------------------------------------------------------------------ context menus

/// `WM_CONTEXTMENU` for widget `id` (`l` = screen position or -1 from the keyboard): select the
/// item under the pointer in tables/trees, then tell the core. Returns true if a popup was shown.
fn context_menu(id: WidgetId, l: LPARAM) -> bool {
    let Some((kind, h, win)) = get(id, |x| (x.kind, x.hwnd, x.win)) else {
        return false;
    };
    let Some((whwnd, dpi)) = get(win, |x| (x.hwnd, x.dpi)) else {
        return false;
    };
    let keyboard = (l as u32) == u32::MAX;
    let mut pt = POINT {
        x: (l & 0xFFFF) as u16 as i16 as i32,
        y: ((l >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    unsafe {
        if keyboard {
            // at the selected item when there is one, else near the control's top-left
            pt = POINT { x: 8, y: 8 };
            let mut rc = RECT::default();
            match kind {
                Kind::Table => {
                    let i = send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
                    rc.left = LVIR_LABEL;
                    if i >= 0
                        && send(h, LVM_GETITEMRECT, i as usize, &mut rc as *mut _ as isize) != 0
                    {
                        pt = POINT {
                            x: rc.left + 8,
                            y: rc.bottom,
                        };
                    }
                }
                Kind::Tree => {
                    let cur = send(h, TVM_GETNEXTITEM, TVGN_CARET, 0);
                    // TVM_GETITEMRECT takes the HTREEITEM in the first field of the RECT
                    let mut raw = [0u8; 16];
                    raw[..8].copy_from_slice(&cur.to_ne_bytes());
                    if cur != 0 && send(h, TVM_GETITEMRECT, 1, raw.as_mut_ptr() as isize) != 0 {
                        let r = |i: usize| {
                            i32::from_ne_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]))
                        };
                        pt = POINT {
                            x: r(0) + 8,
                            y: r(3),
                        };
                    }
                }
                _ => {}
            }
            ClientToScreen(h, &mut pt);
        } else if matches!(kind, Kind::Table | Kind::Tree) {
            // right-click selects the row under the pointer (normal selection event first)
            let mut c = pt;
            ScreenToClient(h, &mut c);
            if kind == Kind::Table {
                let mut ht = LVHITTESTINFO {
                    pt: c,
                    flags: 0,
                    iItem: -1,
                    iSubItem: 0,
                    iGroup: 0,
                };
                send(h, LVM_HITTEST, 0, &mut ht as *mut _ as isize);
                if ht.iItem >= 0 && get(id, |x| x.last_sel) != Some(Some(ht.iItem as usize)) {
                    {
                        let _m = MuteGuard::new();
                        set_selection(id, Some(ht.iItem as usize));
                    }
                    emit(id, Event::Selected(Some(ht.iItem as usize)));
                }
            } else {
                let mut ht = TVHITTESTINFO {
                    pt: c,
                    flags: 0,
                    hItem: 0,
                };
                send(h, TVM_HITTEST, 0, &mut ht as *mut _ as isize);
                if let Some(n) = tree_node_of(h, ht.hItem) {
                    if get(id, |x| x.last_tsel) != Some(Some(n)) {
                        {
                            let _m = MuteGuard::new();
                            tree_select(id, Some(n));
                        }
                        emit(id, Event::TreeSelected(Some(n)));
                    }
                }
            }
        }
    }
    let mut cp = pt;
    unsafe {
        ScreenToClient(whwnd, &mut cp);
    }
    POPUP_SHOWN.with(|p| p.set(false));
    emit(
        id,
        Event::ContextMenu {
            x: lp(cp.x, dpi),
            y: lp(cp.y, dpi),
        },
    );
    POPUP_SHOWN.with(|p| p.replace(false))
}

fn popup_menu_impl(menu: WidgetId, parent_window: Option<WidgetId>, at: Option<(i32, i32)>) {
    let Some(hm) = get(menu, |w| w.hmenu).filter(|m| *m != 0) else {
        return;
    };
    let win = parent_window.and_then(|p| get(p, |w| (w.hwnd, w.dpi)));
    let owner = match win {
        Some((h, _)) if h != 0 => h,
        _ => unsafe { GetForegroundWindow() },
    };
    if owner == 0 {
        return;
    }
    let mut pt = POINT::default();
    unsafe {
        match (at, win) {
            (Some((x, y)), Some((_, dpi))) => {
                pt = POINT {
                    x: px(x, dpi),
                    y: px(y, dpi),
                };
                ClientToScreen(owner, &mut pt);
            }
            _ => {
                GetCursorPos(&mut pt);
            }
        }
    }
    POPUP_SHOWN.with(|p| p.set(true));
    let cmd = unsafe {
        SetForegroundWindow(owner); // otherwise the menu does not dismiss on an outside click
        let c = TrackPopupMenuEx(
            hm,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            owner,
            null(),
        );
        PostMessageW(owner, WM_NULL, 0, 0);
        c
    };
    if cmd > 0 {
        fire_menu_cmd(cmd as u16);
    }
}

// ------------------------------------------------------------------ file dialogs (IFileDialog via raw vtables)

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> GUID {
    GUID { d1, d2, d3, d4 }
}
static CLSID_FILE_OPEN: GUID = guid(
    0xDC1C5A9C,
    0xE88A,
    0x4DDE,
    [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
);
static IID_FILE_OPEN: GUID = guid(
    0xD57C7288,
    0xD4AD,
    0x4768,
    [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
);
static CLSID_FILE_SAVE: GUID = guid(
    0xC0B4E2F3,
    0xBA21,
    0x4773,
    [0x8D, 0xBA, 0x33, 0x5E, 0xC9, 0x46, 0xEB, 0x8B],
);
static IID_FILE_SAVE: GUID = guid(
    0x84BCCD23,
    0x5FDE,
    0x4CDB,
    [0xAE, 0xA4, 0xAF, 0x64, 0xB8, 0x3D, 0x78, 0xAB],
);
static IID_SHELL_ITEM: GUID = guid(
    0x43826D1E,
    0xE718,
    0x42EE,
    [0xBC, 0x55, 0xA1, 0xE2, 0x61, 0xC3, 0x7B, 0xFE],
);

type Obj = *mut c_void;

// IUnknown / IFileDialog / IFileOpenDialog / IShellItem / IShellItemArray vtable slots.
const COM_RELEASE: usize = 2;
const FD_SHOW: usize = 3;
const FD_SET_FILE_TYPES: usize = 4;
const FD_SET_OPTIONS: usize = 9;
const FD_GET_OPTIONS: usize = 10;
const FD_SET_FOLDER: usize = 12;
const FD_SET_FILE_NAME: usize = 15;
const FD_SET_TITLE: usize = 17;
const FD_GET_RESULT: usize = 20;
const FOD_GET_RESULTS: usize = 27;
const SI_GET_DISPLAY_NAME: usize = 5;
const SIA_GET_COUNT: usize = 7;
const SIA_GET_ITEM_AT: usize = 8;
/// FILEOPENDIALOGOPTIONS bits.
const FOS_PICKFOLDERS: u32 = 0x20;
const FOS_FORCEFILESYSTEM: u32 = 0x40;
const FOS_ALLOWMULTISELECT: u32 = 0x200;
/// SIGDN_FILESYSPATH: ask a shell item for its path.
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
/// CLSCTX_INPROC_SERVER.
const CLSCTX_INPROC_SERVER: u32 = 1;

/// Pointer to slot `idx` of the COM vtable of `o`, as a function pointer type `F`.
unsafe fn vt<F: Copy>(o: Obj, idx: usize) -> F {
    unsafe {
        let vtbl = *(o as *const *const *const c_void);
        std::mem::transmute_copy::<*const c_void, F>(&*vtbl.add(idx))
    }
}
unsafe fn release(o: Obj) {
    if !o.is_null() {
        unsafe { vt::<unsafe extern "system" fn(Obj) -> u32>(o, COM_RELEASE)(o) };
    }
}
unsafe fn item_path(item: Obj) -> Option<String> {
    unsafe {
        let mut p: *mut u16 = null_mut();
        let hr = vt::<unsafe extern "system" fn(Obj, u32, *mut *mut u16) -> i32>(
            item,
            SI_GET_DISPLAY_NAME,
        )(item, SIGDN_FILESYSPATH, &mut p);
        if hr < 0 || p.is_null() {
            return None;
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        CoTaskMemFree(p as *mut c_void);
        Some(s)
    }
}

fn file_dialog_impl(owner: HWND, spec: &FileSpec) -> Vec<String> {
    let save = spec.mode == FileMode::Save;
    let (clsid, iid) = if save {
        (&CLSID_FILE_SAVE, &IID_FILE_SAVE)
    } else {
        (&CLSID_FILE_OPEN, &IID_FILE_OPEN)
    };
    let mut out = vec![];
    unsafe {
        let mut dlg: Obj = null_mut();
        if CoCreateInstance(clsid, null_mut(), CLSCTX_INPROC_SERVER, iid, &mut dlg) < 0
            || dlg.is_null()
        {
            return out;
        }
        let mut opts = 0u32;
        vt::<unsafe extern "system" fn(Obj, *mut u32) -> i32>(dlg, FD_GET_OPTIONS)(dlg, &mut opts);
        opts |= FOS_FORCEFILESYSTEM;
        match spec.mode {
            FileMode::PickFolder => opts |= FOS_PICKFOLDERS,
            FileMode::OpenMany => opts |= FOS_ALLOWMULTISELECT,
            _ => {}
        }
        vt::<unsafe extern "system" fn(Obj, u32) -> i32>(dlg, FD_SET_OPTIONS)(dlg, opts);
        if !spec.title.is_empty() {
            vt::<unsafe extern "system" fn(Obj, *const u16) -> i32>(dlg, FD_SET_TITLE)(
                dlg,
                wide(&spec.title).as_ptr(),
            );
        }
        if spec.mode != FileMode::PickFolder && !spec.filters.is_empty() {
            let names: Vec<Vec<u16>> = spec.filters.iter().map(|(n, _)| wide(n)).collect();
            let pats: Vec<Vec<u16>> = spec
                .filters
                .iter()
                .map(|(_, e)| {
                    if e.is_empty() {
                        wide("*.*")
                    } else {
                        wide(
                            &e.iter()
                                .map(|x| {
                                    format!(
                                        "*.{}",
                                        x.trim_start_matches("*.").trim_start_matches('.')
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(";"),
                        )
                    }
                })
                .collect();
            let specs: Vec<COMDLG_FILTERSPEC> = names
                .iter()
                .zip(&pats)
                .map(|(n, p)| COMDLG_FILTERSPEC {
                    name: n.as_ptr(),
                    spec: p.as_ptr(),
                })
                .collect();
            vt::<unsafe extern "system" fn(Obj, u32, *const COMDLG_FILTERSPEC) -> i32>(
                dlg,
                FD_SET_FILE_TYPES,
            )(dlg, specs.len() as u32, specs.as_ptr());
        }
        if let Some(dir) = &spec.initial_dir {
            let mut item: Obj = null_mut();
            if SHCreateItemFromParsingName(
                wide(dir).as_ptr(),
                null_mut(),
                &IID_SHELL_ITEM,
                &mut item,
            ) >= 0
                && !item.is_null()
            {
                vt::<unsafe extern "system" fn(Obj, Obj) -> i32>(dlg, FD_SET_FOLDER)(dlg, item);
                release(item);
            }
        }
        if let Some(name) = &spec.initial_name {
            vt::<unsafe extern "system" fn(Obj, *const u16) -> i32>(dlg, FD_SET_FILE_NAME)(
                dlg,
                wide(name).as_ptr(),
            );
        }
        let hr = vt::<unsafe extern "system" fn(Obj, HWND) -> i32>(dlg, FD_SHOW)(dlg, owner);
        if hr >= 0 {
            if spec.mode == FileMode::OpenMany {
                let mut arr: Obj = null_mut();
                if vt::<unsafe extern "system" fn(Obj, *mut Obj) -> i32>(dlg, FOD_GET_RESULTS)(
                    dlg, &mut arr,
                ) >= 0
                    && !arr.is_null()
                {
                    let mut n = 0u32;
                    vt::<unsafe extern "system" fn(Obj, *mut u32) -> i32>(arr, SIA_GET_COUNT)(
                        arr, &mut n,
                    );
                    for i in 0..n {
                        let mut item: Obj = null_mut();
                        if vt::<unsafe extern "system" fn(Obj, u32, *mut Obj) -> i32>(
                            arr,
                            SIA_GET_ITEM_AT,
                        )(arr, i, &mut item)
                            >= 0
                            && !item.is_null()
                        {
                            out.extend(item_path(item));
                            release(item);
                        }
                    }
                    release(arr);
                }
            } else {
                let mut item: Obj = null_mut();
                if vt::<unsafe extern "system" fn(Obj, *mut Obj) -> i32>(dlg, FD_GET_RESULT)(
                    dlg, &mut item,
                ) >= 0
                    && !item.is_null()
                {
                    out.extend(item_path(item));
                    release(item);
                }
            }
        }
        release(dlg);
    }
    out
}
