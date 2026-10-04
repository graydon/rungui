//! Win32 backend: user32 / comctl32 v6 / gdi32 / uxtheme / COM file dialogs, all through Microsoft's
//! `windows` crate (bindings, typed handles/flags, `Result` returns, COM interface wrappers); there
//! is no hand-declared FFI and `build.rs` emits no link directives for this backend. Accessibility
//! is the stock MSAA proxies plus `IAccPropServices` overrides (`win32/a11y.rs`); there is no UIA
//! provider.
//!
//! Layout of the module: `api.rs` = thin wrappers for handle-only calls, `lists.rs` = table / tree,
//! `dialogs.rs` = message box and `IFileDialog`, `a11y.rs` = accessibility annotations.
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
//!   `LoadLibrary`'d *after* activation and never imported statically (the `windows` crate's
//!   `InitCommonControlsEx` import would load v5 at process start), and the DPI entry points added
//!   after Windows 7 are looked up at runtime so the binary still starts there. An application
//!   that embeds its own manifest simply makes this a no-op.
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
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::ApplicationInstallationAndServicing::{
    ACTCTXW, ActivateActCtx, CreateActCtxW,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemServices::{SS_NOPREFIX, SS_OWNERDRAW};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT;
use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCSTR, PCWSTR, PWSTR, s, w};

mod a11y;
mod api;
mod dialogs;
mod lists;
use api::*;
use lists::*;

/// `TBM_GETPOS` (= `WM_USER`) is the one trackbar message the bindings do not define.
const TBM_GETPOS: u32 = WM_USER;
const WM_WAKE: u32 = WM_APP + 1;
const CLS_WINDOW: PCWSTR = w!("RunguiWindow");
const CLS_CONTAINER: PCWSTR = w!("RunguiContainer");
const CLS_MSG: PCWSTR = w!("RunguiMessage");
const CLS_SASH: PCWSTR = w!("RunguiSash");
/// Posted to the message window: re-check a table's selection once the burst of notifications is over.
const WM_SELCHECK: u32 = WM_APP + 2;
/// Posted to the message window: deliver queued tree expand/collapse events outside the notification.
const WM_TREEEXP: u32 = WM_APP + 3;

static MSG_HWND: AtomicIsize = AtomicIsize::new(0);
static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

fn msg_hwnd() -> HWND {
    hw(MSG_HWND.load(Ordering::SeqCst))
}

// ------------------------------------------------------------------ handles

/// `HWND` has no `Hash` impl: map keys are the raw handle value.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
struct HKey(isize);
impl From<HWND> for HKey {
    fn from(h: HWND) -> HKey {
        HKey(h.0 as isize)
    }
}
impl HKey {
    fn hwnd(self) -> HWND {
        hw(self.0)
    }
}

fn hw(v: isize) -> HWND {
    HWND(v as *mut c_void)
}
/// `None` for the null handle (the crate spells optional handle parameters `Option<HWND>`).
fn opt(h: HWND) -> Option<HWND> {
    (!h.is_invalid()).then_some(h)
}

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
    hmenu: HMENU,
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
    hbmp: HBITMAP,
    accel: Option<String>,
    // Window only
    dpi: u32,
    tip_hwnd: HWND,
    haccel: HACCEL,
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
    nodes: HashMap<u64, HTREEITEM>,
    last_tsel: Option<u64>,
    // Sash
    vertical: bool,
    drag: Option<(i32, i32)>,
}

impl W {
    fn new(kind: Kind, win: WidgetId, parent: Option<WidgetId>) -> W {
        W {
            kind,
            hwnd: HWND::default(),
            aux: HWND::default(),
            parent,
            win,
            children: vec![],
            hmenu: HMENU::default(),
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
            hbmp: HBITMAP::default(),
            accel: None,
            dpi: 96,
            tip_hwnd: HWND::default(),
            haccel: HACCEL::default(),
            client_req: None,
            shown: false,
            last_focus: HWND::default(),
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

/// Entry points that do not exist on every supported Windows version (resolved at runtime).
#[derive(Copy, Clone, Default)]
struct Fns {
    set_dpi_ctx: Option<unsafe extern "system" fn(DPI_AWARENESS_CONTEXT) -> BOOL>,
    dpi_for_window: Option<unsafe extern "system" fn(HWND) -> u32>,
    adjust_rect_dpi: Option<
        unsafe extern "system" fn(*mut RECT, WINDOW_STYLE, BOOL, WINDOW_EX_STYLE, u32) -> BOOL,
    >,
}

#[derive(Default)]
struct State {
    widgets: HashMap<WidgetId, W>,
    by_hwnd: HashMap<HKey, WidgetId>,
    by_cmd: HashMap<u16, WidgetId>,
    next_cmd: u16,
    fonts: HashMap<u32, HFONT>,
    timers: HashMap<u64, bool>,
    inst: HINSTANCE,
    fns: Fns,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
    static MUTE: Cell<u32> = const { Cell::new(0) };
    /// Original window procedures of subclassed controls (kept until WM_NCDESTROY).
    static ORIG: RefCell<HashMap<HKey, WNDPROC>> = RefCell::new(HashMap::new());
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
    st(|s| s.by_hwnd.get(&HKey::from(h)).copied()).flatten()
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

/// NUL-terminated UTF-16 (embedded NULs dropped), for buffers the backend keeps or hands out as `*mut`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .filter(|c| *c != 0)
        .chain(std::iter::once(0))
        .collect()
}
/// `HSTRING` (an `&HSTRING` is a `PCWSTR` parameter) with embedded NULs dropped.
fn hs(s: &str) -> HSTRING {
    if s.contains('\0') {
        HSTRING::from(s.replace('\0', ""))
    } else {
        HSTRING::from(s)
    }
}
/// Core text with `&` mnemonic markers -> Win32 prefix syntax (surplus markers dropped).
fn esc_amp(s: &str) -> String {
    crate::text::to_win32_mnemonic(s)
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
/// A `LPARAM` carrying a pointer to a message structure.
fn ptr_arg<T>(p: &T) -> isize {
    p as *const T as isize
}
fn ptr_arg_mut<T>(p: &mut T) -> isize {
    p as *mut T as isize
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
            '+' | '=' => Some(VK_OEM_PLUS.0),
            '-' => Some(VK_OEM_MINUS.0),
            ',' => Some(VK_OEM_COMMA.0),
            '.' => Some(VK_OEM_PERIOD.0),
            '/' => Some(VK_OEM_2.0),
            ';' => Some(VK_OEM_1.0),
            '`' => Some(VK_OEM_3.0),
            '[' => Some(VK_OEM_4.0),
            '\\' => Some(VK_OEM_5.0),
            ']' => Some(VK_OEM_6.0),
            '\'' => Some(VK_OEM_7.0),
            _ => None,
        };
    }
    if let Some(n) = k.strip_prefix('F').and_then(|r| r.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some(VK_F1.0 - 1 + n);
        }
    }
    Some(
        match k.as_str() {
            "ENTER" | "RETURN" => VK_RETURN,
            "ESC" | "ESCAPE" => VK_ESCAPE,
            "DEL" | "DELETE" => VK_DELETE,
            "TAB" => VK_TAB,
            "SPACE" => VK_SPACE,
            "BACKSPACE" | "BKSP" => VK_BACK,
            "LEFT" => VK_LEFT,
            "UP" => VK_UP,
            "RIGHT" => VK_RIGHT,
            "DOWN" => VK_DOWN,
            "HOME" => VK_HOME,
            "END" => VK_END,
            "PAGEUP" | "PGUP" => VK_PRIOR,
            "PAGEDOWN" | "PGDN" => VK_NEXT,
            "INS" | "INSERT" => VK_INSERT,
            _ => return None,
        }
        .0,
    )
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

/// The UI font (system message font) for `dpi`, cached.
fn font(dpi: u32) -> HFONT {
    if let Some(f) = st(|s| s.fonts.get(&dpi).copied()).flatten() {
        return f;
    }
    let f = message_logfont()
        .and_then(|mut lf| {
            lf.lfHeight = (lf.lfHeight as i64 * dpi as i64 / system_dpi() as i64) as i32;
            create_font(&lf)
        })
        .unwrap_or_else(|| stock_font(DEFAULT_GUI_FONT));
    st(|s| s.fonts.insert(dpi, f));
    f
}

/// Fixed-pitch font for `dpi` (Consolas when installed, else the system's fixed-pitch match), cached.
fn mono_font(dpi: u32) -> HFONT {
    let key = dpi | 0x8000_0000;
    if let Some(f) = st(|s| s.fonts.get(&key).copied()).flatten() {
        return f;
    }
    let mut lf = message_logfont().unwrap_or_else(|| LOGFONTW {
        lfHeight: -12,
        ..Default::default()
    });
    lf.lfHeight = (lf.lfHeight as i64 * dpi as i64 / system_dpi() as i64) as i32;
    lf.lfWidth = 0;
    lf.lfPitchAndFamily = FIXED_PITCH.0 | FF_MODERN.0;
    lf.lfFaceName = [0; 32];
    for (d, c) in lf.lfFaceName.iter_mut().zip("Consolas".encode_utf16()) {
        *d = c;
    }
    let f = create_font(&lf).unwrap_or_else(|| stock_font(ANSI_FIXED_FONT));
    st(|s| s.fonts.insert(key, f));
    f
}

/// The font a widget should use: the UI font, or the fixed-pitch one for monospace text controls.
fn font_of(id: WidgetId, dpi: u32) -> HFONT {
    if get(id, |w| w.mono).unwrap_or(false) {
        mono_font(dpi)
    } else {
        font(dpi)
    }
}
/// `WM_SETFONT` wParam.
fn font_arg(f: HFONT) -> usize {
    f.0 as usize
}

/// Text extent in physical pixels (multi-line aware) using the UI font for `dpi`.
fn measure(dpi: u32, text: &str) -> (i32, i32) {
    measure_with(font(dpi), text)
}
fn measure_with(f: HFONT, text: &str) -> (i32, i32) {
    let lines: Vec<Vec<u16>> = text
        .split('\n')
        .map(|l| {
            let l = l.trim_end_matches('\r');
            if l.is_empty() {
                vec![b'A' as u16]
            } else {
                l.encode_utf16().collect()
            }
        })
        .collect();
    let Some(sizes) = text_extents(f, &lines) else {
        return (text.chars().count() as i32 * 7, 16);
    };
    let (mut w, mut h) = (0, 0);
    for (l, sz) in text.split('\n').zip(sizes) {
        let empty = l.trim_end_matches('\r').is_empty();
        w = w.max(if empty { 0 } else { sz.cx });
        h += sz.cy;
    }
    (w, h)
}
fn text_h(dpi: u32) -> i32 {
    measure(dpi, "Ag").1
}

// ------------------------------------------------------------------ init / loop

/// Look up `name` in `module` (loaded on demand) as a function pointer of type `T`.
fn proc_addr<T: Copy>(module: PCWSTR, name: PCSTR) -> Option<T> {
    const { assert!(std::mem::size_of::<T>() == std::mem::size_of::<usize>()) };
    unsafe {
        let m = LoadLibraryW(module).ok()?;
        let p = GetProcAddress(m, name)?;
        Some(std::mem::transmute_copy::<
            unsafe extern "system" fn() -> isize,
            T,
        >(&p))
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
    let src = hs(&path.to_string_lossy());
    let ctx = ACTCTXW {
        cbSize: std::mem::size_of::<ACTCTXW>() as u32,
        lpSource: PCWSTR(src.as_ptr()),
        ..Default::default()
    };
    unsafe {
        if let Ok(h) = CreateActCtxW(&ctx) {
            if !h.is_invalid() {
                let mut cookie = 0usize;
                let _ = ActivateActCtx(Some(h), &mut cookie); // stays active for the life of the process
            }
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// `HBRUSH` for a `COLOR_*` index + 1 (the class-brush convention), or none.
fn class_brush(color: Option<SYS_COLOR_INDEX>) -> HBRUSH {
    match color {
        Some(c) => HBRUSH((c.0 + 1) as usize as *mut c_void),
        None => HBRUSH::default(),
    }
}

fn register_class(
    name: PCWSTR,
    proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    bg: HBRUSH,
    inst: HINSTANCE,
) {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: inst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: bg,
            lpszClassName: name,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

pub struct Win32;

impl Backend for Win32 {
    fn init(_app_name: &str) -> Result<()> {
        let user32 = w!("user32.dll");
        let fns = Fns {
            set_dpi_ctx: proc_addr(user32, s!("SetProcessDpiAwarenessContext")),
            dpi_for_window: proc_addr(user32, s!("GetDpiForWindow")),
            adjust_rect_dpi: proc_addr(user32, s!("AdjustWindowRectExForDpi")),
        };
        unsafe {
            if let Some(f) = fns.set_dpi_ctx {
                let _ = f(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            } else if let Some(f) =
                proc_addr::<unsafe extern "system" fn() -> BOOL>(user32, s!("SetProcessDPIAware"))
            {
                let _ = f();
            }
            activate_visual_styles();
            if let Some(f) = proc_addr::<
                unsafe extern "system" fn(*const INITCOMMONCONTROLSEX) -> BOOL,
            >(w!("comctl32.dll"), s!("InitCommonControlsEx"))
            {
                let icc = INITCOMMONCONTROLSEX {
                    dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: INITCOMMONCONTROLSEX_ICC(0x40FF),
                };
                let _ = f(&icc);
            }
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let inst = GetModuleHandleW(PCWSTR::null())
                .map(|m| HINSTANCE(m.0))
                .unwrap_or_default();
            let btnface = class_brush(Some(COLOR_BTNFACE));
            register_class(CLS_WINDOW, window_proc, btnface, inst);
            register_class(CLS_CONTAINER, window_proc, class_brush(None), inst);
            register_class(CLS_SASH, sash_proc, btnface, inst);
            register_class(CLS_MSG, msg_proc, class_brush(None), inst);
            let h = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLS_MSG,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(inst),
                None,
            ) {
                Ok(h) => h,
                Err(e) => {
                    return Err(Error::Backend(format!(
                        "cannot create message window ({e})"
                    )));
                }
            };
            MSG_HWND.store(h.0 as isize, Ordering::SeqCst);
            st(|s| {
                s.inst = inst;
                s.fns = fns;
                s.next_cmd = 1000;
            });
        }
        Ok(())
    }

    fn run() {
        unsafe {
            let mut msg = MSG::default();
            loop {
                let r = GetMessageW(&mut msg, None, 0, 0);
                if r.0 <= 0 {
                    break;
                }
                let root = root_of(msg.hwnd);
                if !root.is_invalid() {
                    if let Some(id) = id_of(root) {
                        let ha = get(id, |w| w.haccel).unwrap_or_default();
                        if !ha.is_invalid() && TranslateAcceleratorW(root, ha, &msg) != 0 {
                            continue;
                        }
                        // list boxes want Enter themselves (IsDialogMessage would eat it)
                        let own_enter = msg.message == WM_KEYDOWN
                            && msg.wParam.0 == VK_RETURN.0 as usize
                            && matches!(
                                id_of(msg.hwnd).and_then(|c| get(c, |w| w.kind)),
                                Some(Kind::ListBox | Kind::Table | Kind::Tree)
                            );
                        if !own_enter
                            && get(id, |w| w.kind) == Some(Kind::Window)
                            && IsDialogMessageW(root, &msg).as_bool()
                        {
                            continue;
                        }
                    }
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn quit() {
        post_quit()
    }

    fn wake() {
        let h = msg_hwnd();
        if !h.is_invalid() && !WAKE_PENDING.swap(true, Ordering::SeqCst) {
            post(h, WM_WAKE, 0, 0);
        }
    }

    fn timer_start(token: u64, millis: u32, repeat: bool) -> Result<()> {
        let h = msg_hwnd();
        if h.is_invalid() {
            return Err(Error::NotInitialized);
        }
        if !set_timer(h, token as usize, millis.max(1)) {
            return Err(Error::Backend("SetTimer failed".into()));
        }
        st(|s| s.timers.insert(token, repeat));
        Ok(())
    }

    fn timer_stop(token: u64) {
        let h = msg_hwnd();
        st(|s| s.timers.remove(&token));
        if !h.is_invalid() {
            kill_timer(h, token as usize);
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
        get(id, |w| {
            if !w.hwnd.is_invalid() {
                w.hwnd.0 as usize
            } else {
                w.hmenu.0 as usize
            }
        })
        .filter(|h| *h != 0)
        .map(NativeHandle::Win32)
    }

    fn message_box(parent: Option<WidgetId>, spec: &MessageSpec) -> Answer {
        let owner = parent.and_then(|p| get(p, |w| w.hwnd)).unwrap_or_default();
        dialogs::message_box(owner, spec)
    }

    fn file_dialog(parent: Option<WidgetId>, spec: &FileSpec) -> Vec<String> {
        let owner = parent.and_then(|p| get(p, |w| w.hwnd)).unwrap_or_default();
        catch_unwind(AssertUnwindSafe(|| dialogs::file_dialog(owner, spec))).unwrap_or_default()
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
    let w = w.0;
    match m {
        WM_WAKE => {
            WAKE_PENDING.store(false, Ordering::SeqCst);
            let _ = catch_unwind(core::drain_posted);
            LRESULT(0)
        }
        WM_SELCHECK => {
            let _ = catch_unwind(|| table_sel_check(WidgetId(w as u64)));
            LRESULT(0)
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
            LRESULT(0)
        }
        WM_TIMER => {
            let token = w as u64;
            let repeat = st(|s| s.timers.get(&token).copied()).flatten();
            if repeat == Some(false) {
                kill_timer(h, w);
            }
            if repeat.is_some() {
                let _ = catch_unwind(|| core::timer_fired(token));
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, m, WPARAM(w), l) },
    }
}

// ------------------------------------------------------------------ creation

/// Window class, style bits (`WS_*` | class specific) and extended style of an ordinary control.
fn ctl_spec(kind: Kind) -> Option<(PCWSTR, u32, WINDOW_EX_STYLE)> {
    use Kind::*;
    let tab = WS_TABSTOP.0;
    let none = WINDOW_EX_STYLE(0);
    Some(match kind {
        Label => (w!("STATIC"), SS_NOPREFIX.0, none),
        Button => (w!("BUTTON"), tab, none),
        CheckBox => (w!("BUTTON"), tab | BS_AUTOCHECKBOX as u32, none),
        RadioButton => (w!("BUTTON"), tab | BS_RADIOBUTTON as u32, none),
        TextInput => (w!("EDIT"), tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE),
        PasswordInput => (
            w!("EDIT"),
            tab | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
            WS_EX_CLIENTEDGE,
        ),
        TextArea => (
            w!("EDIT"),
            tab | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32 | WS_VSCROLL.0,
            WS_EX_CLIENTEDGE,
        ),
        ComboBox => (
            w!("COMBOBOX"),
            tab | CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0,
            none,
        ),
        ListBox => (
            w!("LISTBOX"),
            tab | LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32 | WS_VSCROLL.0,
            WS_EX_CLIENTEDGE,
        ),
        Slider => (w!("msctls_trackbar32"), tab | TBS_NOTICKS, none),
        ProgressBar => (w!("msctls_progress32"), 0, none),
        SpinBox => (w!("EDIT"), tab | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE),
        Image => (w!("STATIC"), SS_OWNERDRAW.0, none),
        Table => (
            w!("SysListView32"),
            tab | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER,
            WS_EX_CLIENTEDGE,
        ),
        Tree => (
            w!("SysTreeView32"),
            tab | TVS_HASBUTTONS
                | TVS_HASLINES
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_DISABLEDRAGDROP,
            WS_EX_CLIENTEDGE,
        ),
        Tabs => (
            w!("SysTabControl32"),
            tab | WS_CLIPCHILDREN.0 | WS_CLIPSIBLINGS.0,
            none,
        ),
        _ => return None,
    })
}

fn create_window_raw(
    ex: WINDOW_EX_STYLE,
    class: PCWSTR,
    style: u32,
    parent: HWND,
    inst: HINSTANCE,
) -> windows::core::Result<HWND> {
    unsafe {
        CreateWindowExW(
            ex,
            class,
            PCWSTR::null(),
            WINDOW_STYLE(style),
            0,
            0,
            10,
            10,
            opt(parent),
            None,
            Some(inst),
            None,
        )
    }
}

fn last_err(what: &str, e: windows::core::Error) -> Error {
    Error::Backend(format!("{what} failed ({e})"))
}

fn subclass(h: HWND) {
    unsafe {
        let orig = SetWindowLongPtrW(h, GWLP_WNDPROC, ctl_proc as usize as isize);
        if orig != 0 {
            // SAFETY: a non-null window procedure returned by GetWindowLongPtr(GWLP_WNDPROC)
            let orig = std::mem::transmute::<isize, WNDPROC>(orig);
            let _ = ORIG.try_with(|o| o.borrow_mut().insert(h.into(), orig));
        }
    }
}

fn create_impl(id: WidgetId, kind: Kind, parent: Option<WidgetId>) -> Result<()> {
    let (inst, fns) = st(|s| (s.inst, s.fns)).ok_or(Error::NotInitialized)?;
    if kind == Kind::Window {
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLS_WINDOW,
                PCWSTR::null(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                None,
                None,
                Some(inst),
                None,
            )
        }
        .map_err(|e| last_err("CreateWindowEx", e))?;
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
            s.by_hwnd.insert(h.into(), id);
        });
        return Ok(());
    }

    if kind == Kind::PopupMenu {
        let m = create_popup_menu().map_err(|e| last_err("CreatePopupMenu", e))?;
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
        match kind {
            Kind::MenuBar => {
                let m = create_menu().map_err(|e| last_err("CreateMenu", e))?;
                w.hmenu = m;
                w.win = p;
                set_menu(phwnd, Some(m));
            }
            Kind::Menu => {
                let m = create_popup_menu().map_err(|e| last_err("CreatePopupMenu", e))?;
                w.hmenu = m;
                append_menu(pmenu, MF_POPUP | MF_STRING, m.0 as usize, Some(""));
            }
            Kind::MenuSeparator => append_menu(pmenu, MF_SEPARATOR, 0, None),
            _ => {
                let cmd = st(|s| {
                    loop {
                        let c = s.next_cmd;
                        s.next_cmd = if c >= 0xEFFF { 1000 } else { c + 1 };
                        if !s.by_cmd.contains_key(&c) {
                            s.by_cmd.insert(c, id);
                            return c;
                        }
                    }
                })
                .ok_or(Error::NotInitialized)?;
                w.cmd = cmd;
                append_menu(pmenu, MF_STRING, cmd as usize, Some(""));
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
            (WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS).0,
            chost,
            inst,
        )
        .map_err(|e| last_err("CreateWindowEx(container)", e))?;
        let mut w = W::new(kind, pwin, Some(p));
        w.hwnd = h;
        let dpi = dpi_of(pwin);
        if kind == Kind::GroupBox {
            let f = match create_window_raw(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                (WS_CHILD | WS_VISIBLE).0 | BS_GROUPBOX as u32,
                h,
                inst,
            ) {
                Ok(f) => f,
                Err(e) => {
                    destroy(h);
                    return Err(last_err("CreateWindowEx(groupbox)", e));
                }
            };
            send(f, WM_SETFONT, font_arg(font(dpi)), 1);
            w.aux = f;
        }
        if kind == Kind::Page {
            let tab_index = get(p, |t| t.children.len()).unwrap_or(0);
            w.vis = tab_index == 0;
            let mut empty = wide("");
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(empty.as_mut_ptr()),
                iImage: -1,
                ..Default::default()
            };
            send(phwnd, TCM_INSERTITEMW, tab_index, ptr_arg(&item));
            if tab_index > 0 {
                show(h, SW_HIDE);
            }
        }
        let aux = w.aux;
        st(|s| {
            s.widgets.insert(id, w);
            s.by_hwnd.insert(h.into(), id);
            if !aux.is_invalid() {
                s.by_hwnd.insert(aux.into(), id);
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
            WINDOW_EX_STYLE(0),
            CLS_SASH,
            (WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_TABSTOP).0,
            phwnd,
            inst,
        )
        .map_err(|e| last_err("CreateWindowEx(sash)", e))?;
        let mut w = W::new(kind, pwin, Some(p));
        w.hwnd = h;
        st(|s| {
            s.widgets.insert(id, w);
            s.by_hwnd.insert(h.into(), id);
        });
        return Ok(());
    }

    // ---- ordinary controls ----
    let (class, style, ex) = ctl_spec(kind).ok_or(Error::Unsupported)?;
    let h = create_window_raw(ex, class, (WS_CHILD | WS_VISIBLE).0 | style, phwnd, inst)
        .map_err(|e| last_err("CreateWindowEx(control)", e))?;
    let _ = pkind;
    let dpi = dpi_of(pwin);
    send(h, WM_SETFONT, font_arg(font(dpi)), 1);
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
            if let Ok(u) = create_window_raw(
                WINDOW_EX_STYLE(0),
                w!("msctls_updown32"),
                (WS_CHILD | WS_VISIBLE).0,
                phwnd,
                inst,
            ) {
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
        s.by_hwnd.insert(h.into(), id);
        if !aux.is_invalid() {
            s.by_hwnd.insert(aux.into(), id);
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
        s.by_hwnd.remove(&w.hwnd.into());
        if !w.aux.is_invalid() {
            s.by_hwnd.remove(&w.aux.into());
        }
        if w.cmd != 0 {
            s.by_cmd.remove(&w.cmd);
        }
        let mut pos = 0;
        if let Some(p) = w.parent {
            if let Some(pw) = s.widgets.get_mut(&p) {
                pos = pw.children.iter().position(|c| *c == id).unwrap_or(0);
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
    match w.kind {
        Kind::Window => {
            if !w.haccel.is_invalid() {
                destroy_accel_table(w.haccel);
            }
            destroy(w.hwnd);
        }
        Kind::Page => {
            if let Some(t) = w.parent.and_then(|p| get(p, |t| t.hwnd)) {
                send(t, TCM_DELETEITEM, pos, 0);
            }
            destroy(w.hwnd);
            if let Some(t) = w.parent {
                position_pages(t);
            }
        }
        Kind::PopupMenu => destroy_menu(w.hmenu),
        Kind::MenuBar => {
            if let Some(win) = w.parent.and_then(|p| get(p, |x| x.hwnd)) {
                set_menu(win, None);
            }
            destroy_menu(w.hmenu);
            if let Some(p) = w.parent {
                menu_changed(p);
            }
        }
        Kind::Menu | Kind::MenuItem | Kind::CheckMenuItem | Kind::MenuSeparator => {
            if let Some(pm) = w.parent.and_then(|p| get(p, |x| x.hmenu)) {
                delete_menu_pos(pm, pos as u32);
            }
            menu_changed(w.win);
        }
        _ => {
            if !w.hbmp.is_invalid() {
                delete_object(HGDIOBJ(w.hbmp.0));
            }
            if !w.aux.is_invalid() && w.kind != Kind::GroupBox {
                destroy(w.aux);
            }
            destroy(w.hwnd);
        }
    }
    if w.accel.is_some() {
        rebuild_accel(w.win);
    }
}

// ------------------------------------------------------------------ menus

fn menu_changed(win: WidgetId) {
    let Some((h, req)) = get(win, |w| (w.hwnd, w.client_req)) else {
        return;
    };
    if h.is_invalid() {
        return; // a PopupMenu has no window
    }
    draw_menu_bar(h);
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
    let by_pos = kind == Kind::Menu;
    set_menu_item_text(
        pm,
        if by_pos { pos as u32 } else { cmd as u32 },
        by_pos,
        &label,
    );
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
        None
    } else {
        make_accel_table(&list)
    };
    let old = with_w(win, |w| {
        std::mem::replace(&mut w.haccel, new.unwrap_or_default())
    })
    .unwrap_or_default();
    if !old.is_invalid() {
        destroy_accel_table(old);
    }
}

// ------------------------------------------------------------------ geometry

/// `AdjustWindowRectEx` at `dpi` where the OS can (per-monitor aware), else at the system DPI.
fn adjust_rect(fns: &Fns, rc: &mut RECT, style: u32, has_menu: bool, ex: u32, dpi: u32) {
    unsafe {
        match fns.adjust_rect_dpi {
            Some(f) => {
                let _ = f(
                    rc,
                    WINDOW_STYLE(style),
                    has_menu.into(),
                    WINDOW_EX_STYLE(ex),
                    dpi,
                );
            }
            None => {
                let _ = AdjustWindowRectEx(rc, WINDOW_STYLE(style), has_menu, WINDOW_EX_STYLE(ex));
            }
        }
    }
}

fn set_client_size(win: WidgetId, sz: Size) {
    let Some((h, dpi, has_menu, fns)) = st(|s| {
        let w = s.widgets.get(&win)?;
        Some((
            w.hwnd,
            w.dpi,
            w.children
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
    let style = style_of(h) as u32;
    let ex = ex_style_of(h) as u32;
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: cw,
        bottom: ch,
    };
    adjust_rect(&fns, &mut rc, style, has_menu, ex, dpi);
    let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
    set_pos(h, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, flags);
    // a wrapped menu bar changes the client height: correct once
    let cr = client_rect(h);
    let (dx, dy) = (cw - (cr.right - cr.left), ch - (cr.bottom - cr.top));
    if dx != 0 || dy != 0 {
        set_pos(
            h,
            None,
            0,
            0,
            rc.right - rc.left + dx,
            rc.bottom - rc.top + dy,
            flags,
        );
    }
}

fn client_logical(win: WidgetId) -> Option<Size> {
    let (h, dpi) = get(win, |w| (w.hwnd, w.dpi))?;
    let cr = client_rect(h);
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
            set_pos(h, None, x, y, w, hh, flags);
            position_pages(id);
        }
        _ => {
            if pkind == Some(Kind::GroupBox) {
                let (l, t, _, _) = group_inset(dpi);
                x += l;
                y += t;
            }
            match kind {
                Kind::SpinBox => {
                    let uw = px(17, dpi).min(w / 2);
                    set_pos(h, None, x, y, (w - uw).max(0), hh, flags);
                    if !aux.is_invalid() {
                        set_pos(aux, None, x + w - uw, y, uw, hh, flags);
                    }
                }
                Kind::GroupBox => {
                    set_pos(h, None, x, y, w, hh, flags);
                    if !aux.is_invalid() {
                        set_pos(aux, None, 0, 0, w, hh, flags);
                    }
                }
                _ => set_pos(h, None, x, y, w, hh, flags),
            }
            if matches!(kind, Kind::Label) {
                invalidate(h);
            }
        }
    }
}

fn tab_display_rect(tabs: HWND) -> RECT {
    let mut rc = client_rect(tabs);
    send(tabs, TCM_ADJUSTRECT, 0, ptr_arg_mut(&mut rc));
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
    let origin = map_origin(th, parent(th));
    let (ox, oy) = (origin.x + d.left, origin.y + d.top);
    for (i, (ph, vis)) in pages.iter().enumerate() {
        set_pos(
            *ph,
            None,
            ox,
            oy,
            (d.right - d.left).max(0),
            (d.bottom - d.top).max(0),
            SWP_NOACTIVATE,
        );
        let show_it = *vis && sel == Some(i);
        show(*ph, if show_it { SW_SHOWNOACTIVATE } else { SW_HIDE });
        if show_it {
            redraw_now(*ph);
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
    crate::text::strip_mnemonic(s)
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
            send(h, TCM_ADJUSTRECT, 0, ptr_arg_mut(&mut rc));
            let (dw, dh) = (1000 - (rc.right - rc.left), 1000 - (rc.bottom - rc.top));
            Some(Size::new(lp(dw, dpi), lp(dh, dpi)))
        }
        _ => None,
    }
}

// ------------------------------------------------------------------ properties

fn tool_info(owner: HWND, flags: TOOLTIP_FLAGS, target: HWND) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: flags,
        hwnd: owner,
        uId: target.0 as usize,
        ..Default::default()
    }
}

fn update_tooltip(id: WidgetId, text: &str) {
    let Some((h, win, had)) = get(id, |w| (w.hwnd, w.win, w.has_tip)) else {
        return;
    };
    let Some((owner, mut tip, dpi)) = get(win, |w| (w.hwnd, w.tip_hwnd, w.dpi)) else {
        return;
    };
    let inst = st(|s| s.inst).unwrap_or_default();
    if h.is_invalid() || matches!(get(id, |w| w.kind), Some(Kind::Window)) {
        return;
    }
    if tip.is_invalid() {
        if text.is_empty() {
            return;
        }
        let made = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                opt(owner),
                None,
                Some(inst),
                None,
            )
        };
        let Ok(t) = made else { return };
        tip = t;
        send(tip, TTM_SETMAXTIPWIDTH, 0, px(400, dpi) as isize);
        with_w(win, |w| w.tip_hwnd = tip);
    }
    let mut ti = tool_info(owner, TTF_IDISHWND | TTF_SUBCLASS, h);
    if text.is_empty() {
        if had {
            send(tip, TTM_DELTOOLW, 0, ptr_arg(&ti));
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
    ti.lpszText = PWSTR(ptr);
    send(
        tip,
        if had {
            TTM_UPDATETIPTEXTW
        } else {
            TTM_ADDTOOLW
        },
        0,
        ptr_arg(&ti),
    );
}

/// A premultiplied-alpha top-down 32-bit DIB section from RGBA pixels (null on failure).
fn build_bitmap(img: &ImageData) -> HBITMAP {
    let none = HBITMAP::default();
    if img.w == 0 || img.h == 0 || img.rgba.len() < (img.w as usize * img.h as usize * 4) {
        return none;
    }
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: img.w as i32,
            biHeight: -(img.h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = null_mut();
    let bmp = unsafe { CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) };
    let Ok(bmp) = bmp else { return none };
    if bits.is_null() {
        return none;
    }
    let dst = unsafe {
        std::slice::from_raw_parts_mut(bits as *mut u8, img.w as usize * img.h as usize * 4)
    };
    for (d, s) in dst.chunks_exact_mut(4).zip(img.rgba.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = (s[2] as u32 * a / 255) as u8;
        d[1] = (s[1] as u32 * a / 255) as u8;
        d[2] = (s[0] as u32 * a / 255) as u8;
        d[3] = s[3];
    }
    bmp
}

fn set_pages_visible(tabs: WidgetId) {
    position_pages(tabs);
}

fn set_impl(id: WidgetId, prop: &Prop) {
    let Some((kind, h, aux, win)) = get(id, |w| (w.kind, w.hwnd, w.aux, w.win)) else {
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
                    invalidate(h);
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
                            pszText: PWSTR(buf.as_mut_ptr()),
                            iImage: -1,
                            ..Default::default()
                        };
                        send(th, TCM_SETITEMW, idx, ptr_arg(&item));
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
                    enable_menu_item(pm, item, by, *e);
                    menu_changed(win);
                }
                Kind::MenuBar | Kind::MenuSeparator => {}
                _ => {
                    enable(h, *e);
                    if !aux.is_invalid() {
                        enable(aux, *e);
                    }
                }
            }
        }
        Prop::Visible(v) => {
            with_w(id, |w| w.vis = *v);
            match kind {
                Kind::Window => {
                    if *v {
                        let first =
                            with_w(id, |w| !std::mem::replace(&mut w.shown, true)).unwrap_or(false);
                        show(h, SW_SHOW);
                        update(h);
                        if first {
                            let rc = window_rect(h);
                            with_w(id, |w| {
                                w.last_pos = Some((lp(rc.left, w.dpi), lp(rc.top, w.dpi)))
                            });
                            // DefWindowProc ignores WM_NEXTDLGCTL: pick the first tab stop ourselves
                            let f = next_tab_item(h);
                            if !f.is_invalid() {
                                set_focus(f);
                            }
                        }
                    } else {
                        show(h, SW_HIDE);
                    }
                }
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
                _ => {
                    let c = if *v { SW_SHOWNOACTIVATE } else { SW_HIDE };
                    show(h, c);
                    if !aux.is_invalid() && kind != Kind::GroupBox {
                        show(aux, c);
                    }
                }
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
                        check_menu_item(pm, cmd as u32, *c);
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
            let bmp = img.map_or(HBITMAP::default(), build_bitmap);
            let old = with_w(id, |w| {
                w.image = img.cloned();
                std::mem::replace(&mut w.hbmp, bmp)
            })
            .unwrap_or_default();
            if !old.is_invalid() {
                delete_object(HGDIOBJ(old.0));
            }
            invalidate(h);
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
                let st_ = style_of(h);
                let n = if *b {
                    st_ | PBS_MARQUEE as isize
                } else {
                    st_ & !(PBS_MARQUEE as isize)
                };
                set_style(h, n);
                send(h, PBM_SETMARQUEE, *b as usize, 30);
            }
        }
        Prop::Resizable(b) => {
            if kind == Kind::Window {
                let s0 = style_of(h);
                let bits = (WS_THICKFRAME | WS_MAXIMIZEBOX).0 as isize;
                set_style(h, if *b { s0 | bits } else { s0 & !bits });
                set_pos(
                    h,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
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
                send(h, WM_SETFONT, font_arg(font_of(id, dpi)), 1);
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
                set_pos(
                    h,
                    None,
                    px(*x, dpi),
                    px(*y, dpi),
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
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
            if !h.is_invalid() {
                set_focus(h);
            }
        }
        #[allow(unreachable_patterns)] // new Props are ignored until a backend handles them
        _ => {}
    }
    let _ = dpi;
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
                stateMask: LVIS_SELECTED,
                ..Default::default()
            };
            send(h, LVM_SETITEMSTATE, usize::MAX, ptr_arg(&it));
            if let Some(i) = sel {
                let both = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
                it.state = both;
                it.stateMask = both;
                send(h, LVM_SETITEMSTATE, i, ptr_arg(&it));
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
    match catch_unwind(AssertUnwindSafe(|| handle_msg(h, m, w.0, l.0))) {
        Ok(Some(r)) => LRESULT(r),
        _ => unsafe { DefWindowProcW(h, m, w, l) },
    }
}

fn emit(id: WidgetId, ev: Event) {
    core::event(id, ev);
}

fn handle_msg(h: HWND, m: u32, w: usize, l: isize) -> Option<isize> {
    let id = id_of(h)?;
    let kind = get(id, |x| x.kind)?;
    match m {
        WM_CLOSE if kind == Kind::Window => {
            core::close_requested(id);
            Some(0)
        }
        WM_SIZE if kind == Kind::Window => {
            if !muted() && w != SIZE_MINIMIZED as usize {
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
            let src = id_of(hw(w as isize)).unwrap_or(id);
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
                let rc = window_rect(h);
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
            let style = style_of(h) as u32;
            let ex = ex_style_of(h) as u32;
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: px(ms.w.max(0), dpi),
                bottom: px(ms.h.max(0), dpi),
            };
            adjust_rect(&fns, &mut rc, style, has_menu, ex, dpi);
            // SAFETY: for WM_GETMINMAXINFO, lParam points to a MINMAXINFO owned by the system
            let mmi = unsafe { &mut *(l as *mut MINMAXINFO) };
            mmi.ptMinTrackSize = POINT {
                x: rc.right - rc.left,
                y: rc.bottom - rc.top,
            };
            Some(0)
        }
        WM_DPICHANGED if kind == Kind::Window => {
            on_dpi_changed(id, h, loword(w), l);
            Some(0)
        }
        WM_ACTIVATE if kind == Kind::Window => {
            if loword(w) != 0 {
                let lf = get(id, |x| x.last_focus).unwrap_or_default();
                if !lf.is_invalid() && is_window(lf) && is_child(h, lf) {
                    set_focus(lf);
                }
            }
            None
        }
        WM_COMMAND => on_command(w, l),
        WM_NOTIFY => on_notify(l),
        WM_HSCROLL => {
            if l != 0 && !muted() {
                if let Some(sid) = id_of(hw(l)) {
                    if let Some((r, old)) = get(sid, |x| (x.range, x.value)) {
                        let n = slider_steps(r);
                        let pos = send(hw(l), TBM_GETPOS, 0, 0) as i32;
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
            // SAFETY: for WM_DRAWITEM, lParam points to a DRAWITEMSTRUCT valid during the message
            let ds = unsafe { &*(l as *const DRAWITEMSTRUCT) };
            if let Some(iid) = id_of(ds.hwndItem) {
                draw_image(iid, ds);
                return Some(1);
            }
            None
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let sid = id_of(hw(l))?;
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
                let dc = HDC(w as *mut c_void);
                // paint what the parent would show behind the label (page body, group box...)
                fill_bg(hw(l), dc, true);
                unsafe { SetBkMode(dc, TRANSPARENT) };
                return Some(unsafe { GetStockObject(NULL_BRUSH) }.0 as isize);
            }
            None
        }
        WM_ERASEBKGND if kind == Kind::Page => {
            let dc = HDC(w as *mut c_void);
            if !paint_page_body(h, h, dc) {
                let rc = client_rect(h);
                fill_rect(dc, &rc, sys_brush(COLOR_BTNFACE));
            }
            Some(1)
        }
        WM_ERASEBKGND if kind == Kind::GroupBox => {
            fill_bg(h, HDC(w as *mut c_void), true);
            Some(1)
        }
        _ => None,
    }
}

/// Paint the themed tab-body background of the page `page` into `dc`, the DC of its descendant
/// `h` (client origin of `h`). Returns false when there is no theme.
fn paint_page_body(page: HWND, h: HWND, dc: HDC) -> bool {
    let Some(tabs) = id_of(page)
        .and_then(|i| get(i, |x| x.parent))
        .flatten()
        .and_then(|p| get(p, |x| x.hwnd))
    else {
        return false;
    };
    unsafe {
        let theme = OpenThemeData(Some(page), w!("TAB"));
        if theme.is_invalid() {
            return false;
        }
        // the tab control's whole client rect, in the client coordinates of `h`
        let tc = client_rect(tabs);
        let o = map_origin(tabs, h);
        let rc = RECT {
            left: o.x,
            top: o.y,
            right: o.x + tc.right,
            bottom: o.y + tc.bottom,
        };
        let vis = client_rect(h);
        let _ = DrawThemeBackground(theme, dc, TABP_BODY.0, 0, &rc, Some(&vis));
        let _ = CloseThemeData(theme);
    }
    true
}

/// Background behind a widget: the themed page body when it sits on a tab page, else the dialog colour.
fn fill_bg(h: HWND, dc: HDC, parent_bg: bool) {
    if parent_bg {
        let mut p = parent(h);
        while !p.is_invalid() {
            if id_of(p).and_then(|i| get(i, |x| x.kind)) == Some(Kind::Page) {
                if paint_page_body(p, h, dc) {
                    return;
                }
                break;
            }
            p = parent(p);
        }
    }
    let rc = client_rect(h);
    fill_rect(dc, &rc, sys_brush(COLOR_BTNFACE));
}

fn draw_image(id: WidgetId, ds: &DRAWITEMSTRUCT) {
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
    if bmp.is_invalid() || iw == 0 || ih == 0 {
        return;
    }
    unsafe {
        let mem = CreateCompatibleDC(Some(ds.hDC));
        if mem.is_invalid() {
            return;
        }
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let bf = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1,
        };
        let r = ds.rcItem;
        let _ = AlphaBlend(
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
        let _ = DeleteDC(mem);
    }
}

fn on_dpi_changed(id: WidgetId, h: HWND, dpi: u32, l: isize) {
    if dpi == 0 || l == 0 {
        return;
    }
    {
        let _m = MuteGuard::new();
        with_w(id, |w| w.dpi = dpi);
        // SAFETY: for WM_DPICHANGED, lParam points to the suggested RECT
        let r = unsafe { *(l as *const RECT) };
        set_pos(
            h,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let f = font(dpi);
        let list: Vec<(WidgetId, HWND, HWND)> = st(|s| {
            s.widgets
                .iter()
                .filter(|(_, w)| w.win == id && !w.hwnd.is_invalid())
                .map(|(i, w)| (*i, w.hwnd, w.aux))
                .collect()
        })
        .unwrap_or_default();
        for (wid_, hw_, aux) in &list {
            send(*hw_, WM_SETFONT, font_arg(font_of(*wid_, dpi)), 1);
            if !aux.is_invalid() {
                send(*aux, WM_SETFONT, font_arg(f), 1);
            }
        }
        for (i, _, _) in &list {
            if *i != id {
                apply_bounds(*i);
            }
        }
    }
    if let Some(sz) = client_logical(id) {
        emit(id, Event::Resized { w: sz.w, h: sz.h });
    }
}

fn on_command(w: usize, l: isize) -> Option<isize> {
    let code = hiword(w);
    if l != 0 {
        if muted() {
            return Some(0);
        }
        let lh = hw(l);
        let cid = id_of(lh)?;
        let (kind, checked, range, text_h_) = get(cid, |x| (x.kind, x.checked, x.range, x.hwnd))?;
        match (kind, code) {
            (Kind::Button, BN_CLICKED) => emit(cid, Event::Click),
            (Kind::CheckBox, BN_CLICKED) => {
                let on = send(lh, BM_GETCHECK, 0, 0) == 1;
                with_w(cid, |x| x.checked = on);
                emit(cid, Event::Toggled(on));
            }
            (Kind::RadioButton, BN_CLICKED) => {
                if !checked {
                    send(lh, BM_SETCHECK, 1, 0);
                    with_w(cid, |x| x.checked = true);
                    emit(cid, Event::Toggled(true));
                }
            }
            (Kind::TextInput | Kind::PasswordInput | Kind::TextArea, EN_CHANGE) => {
                emit(cid, Event::Text(get_text(lh, kind == Kind::TextArea)));
            }
            (Kind::SpinBox, EN_CHANGE) => {
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
            (Kind::ComboBox, CBN_SELCHANGE) => {
                let i = send(lh, CB_GETCURSEL, 0, 0);
                let sel = if i < 0 { None } else { Some(i as usize) };
                with_w(cid, |x| x.selected = sel);
                emit(cid, Event::Selected(sel));
            }
            (Kind::ListBox, LBN_SELCHANGE) => {
                let i = send(lh, LB_GETCURSEL, 0, 0);
                let sel = if i < 0 { None } else { Some(i as usize) };
                with_w(cid, |x| x.selected = sel);
                emit(cid, Event::Selected(sel));
            }
            (Kind::ListBox, LBN_DBLCLK) => {
                let i = send(lh, LB_GETCURSEL, 0, 0);
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

fn on_notify(l: isize) -> Option<isize> {
    if l == 0 {
        return None;
    }
    // SAFETY: for WM_NOTIFY, lParam points to an NMHDR (or a larger structure starting with one)
    let hdr = unsafe { &*(l as *const NMHDR) };
    let code = hdr.code;
    let cid = id_of(hdr.hwndFrom)?;
    let kind = get(cid, |x| x.kind)?;
    // SAFETY (all `nm` casts below): the notification code identifies the structure lParam points to
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
            if (nm.uChanged & LVIF_STATE).0 != 0
                && (nm.uNewState ^ nm.uOldState) & LVIS_SELECTED.0 != 0
            {
                post(msg_hwnd(), WM_SELCHECK, cid.0 as usize, 0);
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
            let nm = unsafe { &*(l as *const NMLVKEYDOWN) };
            if nm.wVKey == VK_RETURN.0 {
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
            let node = (nm.itemNew.hItem.0 != 0).then_some(nm.itemNew.lParam.0 as u64);
            if get(cid, |x| x.last_tsel) != Some(node) {
                with_w(cid, |x| x.last_tsel = node);
                emit(cid, Event::TreeSelected(node));
            }
            Some(0)
        }
        (Kind::Tree, TVN_ITEMEXPANDEDW) if !muted() => {
            let nm = unsafe { &*(l as *const NMTREEVIEWW) };
            let open = nm.action.0 & 3 == TVE_EXPAND.0;
            // the app usually reacts by replacing the rows: deliver outside this notification
            let _ =
                TREE_EXP.try_with(|q| q.borrow_mut().push((cid, nm.itemNew.lParam.0 as u64, open)));
            post(msg_hwnd(), WM_TREEEXP, 0, 0);
            Some(0)
        }
        (Kind::Tree, NM_DBLCLK) if !muted() => {
            let p = to_client(hdr.hwndFrom, cursor_pos());
            let mut ht = TVHITTESTINFO {
                pt: p,
                ..Default::default()
            };
            send(hdr.hwndFrom, TVM_HITTEST, 0, ptr_arg_mut(&mut ht));
            if ht.hItem.0 != 0 && (ht.flags & TVHT_ONITEMBUTTON).0 == 0 {
                if let Some(n) = tree_node_of(hdr.hwndFrom, ht.hItem) {
                    emit(cid, Event::TreeActivated(n));
                }
            }
            Some(0)
        }
        (Kind::Tree, TVN_KEYDOWN) if !muted() => {
            let nm = unsafe { &*(l as *const NMTVKEYDOWN) };
            if nm.wVKey == VK_RETURN.0 {
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
        .try_with(|o| o.borrow().get(&h.into()).copied())
        .ok()
        .flatten()
        .flatten();
    let call = || unsafe {
        match orig {
            Some(_) => CallWindowProcW(orig, h, m, w, l),
            None => DefWindowProcW(h, m, w, l),
        }
    };
    if m == WM_CONTEXTMENU {
        if let Some(id) = id_of(h) {
            let kind = get(id, |x| x.kind);
            let shown = catch_unwind(AssertUnwindSafe(|| context_menu(id, l.0))).unwrap_or(false);
            // edit controls keep their own menu unless the app popped one up
            return if matches!(
                kind,
                Some(Kind::TextInput | Kind::PasswordInput | Kind::TextArea | Kind::SpinBox)
            ) && !shown
            {
                call()
            } else {
                LRESULT(0)
            };
        }
    }
    if m == WM_ERASEBKGND && id_of(h).and_then(|i| get(i, |x| x.kind)) == Some(Kind::Slider) {
        // trackbars leave a grey box on page bodies: show what the parent shows
        fill_bg(h, HDC(w.0 as *mut c_void), true);
        return LRESULT(1);
    }
    let r = call();
    let w = w.0;
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
        WM_KEYDOWN if w == VK_RETURN.0 as usize && !muted() => {
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
        let _ = ORIG.try_with(|o| o.borrow_mut().remove(&h.into()));
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
    let (inst, dpi) = (st(|s| s.inst).unwrap_or_default(), dpi_of(id));
    let text = get_text(old, true);
    let (mut s0, mut s1) = (0u32, 0u32);
    send(
        old,
        EM_GETSEL,
        &mut s0 as *mut _ as usize,
        &mut s1 as *mut _ as isize,
    );
    let mut style = (WS_CHILD | WS_TABSTOP | WS_VSCROLL).0
        | ES_MULTILINE as u32
        | ES_AUTOVSCROLL as u32
        | ES_WANTRETURN as u32;
    if !wrap {
        style |= ES_AUTOHSCROLL as u32 | WS_HSCROLL.0;
    }
    if vis {
        style |= WS_VISIBLE.0;
    }
    let Ok(new) = create_window_raw(WS_EX_CLIENTEDGE, w!("EDIT"), style, phwnd, inst) else {
        return;
    };
    set_pos(new, Some(old), 0, 0, 10, 10, SWP_NOMOVE | SWP_NOACTIVATE);
    send(new, WM_SETFONT, font_arg(font_of(id, dpi)), 1);
    send(new, EM_SETLIMITTEXT, 0, 0);
    send(new, EM_SETREADONLY, ro as usize, 0);
    enable(new, enabled);
    subclass(new);
    let had_focus = focus() == old;
    if has_tip {
        // the tooltip tool is keyed by the old HWND
        let win = get(id, |w| w.win).unwrap_or(id);
        if let Some((owner, tiph)) = get(win, |w| (w.hwnd, w.tip_hwnd)) {
            let ti = tool_info(owner, TTF_IDISHWND, old);
            send(tiph, TTM_DELTOOLW, 0, ptr_arg(&ti));
        }
    }
    st(|s| {
        s.by_hwnd.remove(&old.into());
        s.by_hwnd.insert(new.into(), id);
        if let Some(w) = s.widgets.get_mut(&id) {
            w.hwnd = new;
            w.has_tip = false;
        }
    });
    if let Some(win) = get(id, |w| w.win) {
        a11y::rehome(win, old, new);
    }
    destroy(old);
    if had_focus {
        set_focus(new);
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
    match catch_unwind(AssertUnwindSafe(|| sash_msg(h, m, w.0, l.0))) {
        Ok(Some(r)) => LRESULT(r),
        _ => unsafe { DefWindowProcW(h, m, w, l) },
    }
}

/// Drag = leading-edge position at press + pointer delta (screen coordinates, so it does not drift
/// while the core moves the sash under the pointer). The core clamps and relayouts.
fn sash_msg(h: HWND, m: u32, w: usize, l: isize) -> Option<isize> {
    let id = id_of(h)?;
    let (vertical, bounds, drag) = get(id, |x| (x.vertical, x.bounds, x.drag))?;
    let axis = |p: POINT| if vertical { p.y } else { p.x };
    match m {
        WM_SETCURSOR => {
            unsafe {
                let cur = LoadCursorW(None, if vertical { IDC_SIZENS } else { IDC_SIZEWE });
                SetCursor(cur.ok());
            }
            Some(1)
        }
        WM_ERASEBKGND => {
            let dc = HDC(w as *mut c_void);
            fill_bg(h, dc, true);
            // keyboard focus: a plain focus rectangle (erasing is how a sash repaints)
            if focus() == h {
                let rc = client_rect(h);
                unsafe {
                    let _ = DrawFocusRect(dc, &rc);
                }
            }
            Some(1)
        }
        // the dialog manager (IsDialogMessage) would otherwise use the arrow keys to move the focus
        WM_GETDLGCODE => Some(DLGC_WANTARROWS as isize),
        WM_SETFOCUS | WM_KILLFOCUS => {
            if m == WM_SETFOCUS {
                // so that re-activating the window puts the focus back here
                if let Some(win) = get(id, |x| x.win) {
                    with_w(win, |x| x.last_focus = h);
                }
            }
            invalidate(h);
            None
        }
        // Arrow keys along the sash's axis (Shift = large step), Home and End. Other keys, and any
        // chord with Ctrl, are not ours.
        WM_KEYDOWN => {
            if muted() || key_down(VK_CONTROL) {
                return None;
            }
            let big = key_down(VK_SHIFT);
            let (prev, next) = if vertical {
                (VK_UP, VK_DOWN)
            } else {
                (VK_LEFT, VK_RIGHT)
            };
            let key = match w {
                k if k == VK_HOME.0 as usize => SashKey::Min,
                k if k == VK_END.0 as usize => SashKey::Max,
                k if k == prev.0 as usize => {
                    if big {
                        SashKey::PrevLarge
                    } else {
                        SashKey::Prev
                    }
                }
                k if k == next.0 as usize => {
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
        }
        WM_LBUTTONDOWN => {
            set_focus(h);
            let p = cursor_pos();
            with_w(id, |x| {
                x.drag = Some((axis(p), if vertical { bounds.y } else { bounds.x }))
            });
            set_capture(h);
            Some(0)
        }
        WM_MOUSEMOVE => {
            if let Some((start, pos0)) = drag {
                if capture() == h && !muted() {
                    let p = cursor_pos();
                    emit(
                        id,
                        Event::SashDragged(pos0 + lp(axis(p) - start, dpi_of(id))),
                    );
                }
            }
            Some(0)
        }
        WM_LBUTTONUP => {
            with_w(id, |x| x.drag = None);
            release_capture();
            Some(0)
        }
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
fn context_menu(id: WidgetId, l: isize) -> bool {
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
    if keyboard {
        // at the selected item when there is one, else near the control's top-left
        pt = POINT { x: 8, y: 8 };
        match kind {
            Kind::Table => {
                let i = send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
                let mut rc = RECT {
                    left: LVIR_LABEL as i32,
                    ..Default::default()
                };
                if i >= 0 && send(h, LVM_GETITEMRECT, i as usize, ptr_arg_mut(&mut rc)) != 0 {
                    pt = POINT {
                        x: rc.left + 8,
                        y: rc.bottom,
                    };
                }
            }
            Kind::Tree => {
                let cur = HTREEITEM(send(h, TVM_GETNEXTITEM, TVGN_CARET as usize, 0));
                // TVM_GETITEMRECT takes the HTREEITEM in the first field of the RECT
                let mut rc = RECT::default();
                // SAFETY: a RECT (16 bytes) is large enough to hold one HTREEITEM
                unsafe { std::ptr::write_unaligned(&mut rc as *mut RECT as *mut HTREEITEM, cur) };
                if cur.0 != 0 && send(h, TVM_GETITEMRECT, 1, ptr_arg_mut(&mut rc)) != 0 {
                    pt = POINT {
                        x: rc.left + 8,
                        y: rc.bottom,
                    };
                }
            }
            _ => {}
        }
        pt = to_screen(h, pt);
    } else if matches!(kind, Kind::Table | Kind::Tree) {
        // right-click selects the row under the pointer (normal selection event first)
        let c = to_client(h, pt);
        if kind == Kind::Table {
            let mut ht = LVHITTESTINFO {
                pt: c,
                iItem: -1,
                ..Default::default()
            };
            send(h, LVM_HITTEST, 0, ptr_arg_mut(&mut ht));
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
                ..Default::default()
            };
            send(h, TVM_HITTEST, 0, ptr_arg_mut(&mut ht));
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
    let cp = to_client(whwnd, pt);
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
    let Some(hm) = get(menu, |w| w.hmenu).filter(|m| !m.is_invalid()) else {
        return;
    };
    let win = parent_window.and_then(|p| get(p, |w| (w.hwnd, w.dpi)));
    let owner = match win {
        Some((h, _)) if !h.is_invalid() => h,
        _ => foreground(),
    };
    if owner.is_invalid() {
        return;
    }
    let pt = match (at, win) {
        (Some((x, y)), Some((_, dpi))) => to_screen(
            owner,
            POINT {
                x: px(x, dpi),
                y: px(y, dpi),
            },
        ),
        _ => cursor_pos(),
    };
    POPUP_SHOWN.with(|p| p.set(true));
    set_foreground(owner); // otherwise the menu does not dismiss on an outside click
    let cmd = track_popup(hm, owner, pt);
    post(owner, WM_NULL, 0, 0);
    if cmd > 0 {
        fire_menu_cmd(cmd as u16);
    }
}
