//! Thin wrappers over the `windows` crate for the calls that take only handles and plain values
//! (no raw pointers owned by the caller), so the rest of the backend does not repeat `unsafe { }`
//! around every `SetWindowPos`. Return values the backend never looks at are dropped here.
//!
//! These are not unsound for any argument: Win32 reports a stale or null handle as a failure
//! (`ERROR_INVALID_WINDOW_HANDLE`), it does not dereference it.

use super::*;

/// `SendMessageW` with integer-typed parameters (pointer parameters are passed as `isize`; the
/// caller guarantees the message's contract, exactly as with the C macros).
pub fn send(h: HWND, m: u32, w: usize, l: isize) -> isize {
    unsafe { SendMessageW(h, m, Some(WPARAM(w)), Some(LPARAM(l))).0 }
}
pub fn post(h: HWND, m: u32, w: usize, l: isize) {
    unsafe {
        let _ = PostMessageW(opt(h), m, WPARAM(w), LPARAM(l));
    }
}

pub fn get_text(h: HWND, multiline: bool) -> String {
    let n = unsafe { GetWindowTextLengthW(h) };
    if n <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; n as usize + 2];
    let got = unsafe { GetWindowTextW(h, &mut buf) };
    buf.truncate(got.max(0) as usize);
    let s = String::from_utf16_lossy(&buf);
    if multiline {
        s.replace("\r\n", "\n")
    } else {
        s
    }
}
pub fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &hs(s));
    }
}

pub fn show(h: HWND, cmd: SHOW_WINDOW_CMD) {
    unsafe {
        let _ = ShowWindow(h, cmd);
    }
}
pub fn update(h: HWND) {
    unsafe {
        let _ = UpdateWindow(h);
    }
}
pub fn set_pos(
    h: HWND,
    after: Option<HWND>,
    x: i32,
    y: i32,
    w: i32,
    hh: i32,
    flags: SET_WINDOW_POS_FLAGS,
) {
    unsafe {
        let _ = SetWindowPos(h, after, x, y, w, hh, flags);
    }
}
pub fn destroy(h: HWND) {
    unsafe {
        let _ = DestroyWindow(h);
    }
}
pub fn enable(h: HWND, on: bool) {
    unsafe {
        let _ = EnableWindow(h, on);
    }
}
pub fn set_focus(h: HWND) {
    unsafe {
        let _ = SetFocus(opt(h));
    }
}
pub fn focus() -> HWND {
    unsafe { GetFocus() }
}
pub fn capture() -> HWND {
    unsafe { GetCapture() }
}
pub fn set_capture(h: HWND) {
    unsafe {
        SetCapture(h);
    }
}
pub fn release_capture() {
    unsafe {
        let _ = ReleaseCapture();
    }
}
pub fn parent(h: HWND) -> HWND {
    unsafe { GetParent(h).unwrap_or_default() }
}
pub fn root_of(h: HWND) -> HWND {
    unsafe { GetAncestor(h, GA_ROOT) }
}
pub fn is_window(h: HWND) -> bool {
    unsafe { IsWindow(opt(h)).as_bool() }
}
pub fn is_child(parent: HWND, h: HWND) -> bool {
    unsafe { IsChild(parent, h).as_bool() }
}
pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}
pub fn set_foreground(h: HWND) {
    unsafe {
        let _ = SetForegroundWindow(h);
    }
}
pub fn next_tab_item(dlg: HWND) -> HWND {
    unsafe { GetNextDlgTabItem(dlg, None, false).unwrap_or_default() }
}

pub fn client_rect(h: HWND) -> RECT {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut rc);
    }
    rc
}
pub fn window_rect(h: HWND) -> RECT {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut rc);
    }
    rc
}
pub fn cursor_pos() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}
pub fn to_client(h: HWND, mut p: POINT) -> POINT {
    unsafe {
        let _ = ScreenToClient(h, &mut p);
    }
    p
}
pub fn to_screen(h: HWND, mut p: POINT) -> POINT {
    unsafe {
        let _ = ClientToScreen(h, &mut p);
    }
    p
}
/// Origin of `from`'s client area expressed in `to`'s client coordinates.
pub fn map_origin(from: HWND, to: HWND) -> POINT {
    let mut p = [POINT::default()];
    unsafe {
        MapWindowPoints(opt(from), opt(to), &mut p);
    }
    p[0]
}
pub fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

pub fn invalidate(h: HWND) {
    unsafe {
        let _ = InvalidateRect(opt(h), None, true);
    }
}
pub fn redraw_now(h: HWND) {
    unsafe {
        let _ = RedrawWindow(
            opt(h),
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
}
pub fn draw_menu_bar(h: HWND) {
    unsafe {
        let _ = DrawMenuBar(h);
    }
}
pub fn vscroll_pos(h: HWND) -> i32 {
    unsafe { GetScrollPos(h, SB_VERT) }
}

pub fn style_of(h: HWND) -> isize {
    unsafe { GetWindowLongPtrW(h, GWL_STYLE) }
}
pub fn ex_style_of(h: HWND) -> isize {
    unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) }
}
pub fn set_style(h: HWND, v: isize) {
    unsafe {
        SetWindowLongPtrW(h, GWL_STYLE, v);
    }
}

pub fn set_timer(h: HWND, id: usize, ms: u32) -> bool {
    unsafe { SetTimer(opt(h), id, ms, None) != 0 }
}
pub fn kill_timer(h: HWND, id: usize) {
    unsafe {
        let _ = KillTimer(opt(h), id);
    }
}
pub fn post_quit() {
    unsafe { PostQuitMessage(0) }
}

// ---- menus ----
pub fn create_popup_menu() -> windows::core::Result<HMENU> {
    unsafe { CreatePopupMenu() }
}
pub fn create_menu() -> windows::core::Result<HMENU> {
    unsafe { CreateMenu() }
}
pub fn destroy_menu(m: HMENU) {
    unsafe {
        let _ = DestroyMenu(m);
    }
}
pub fn set_menu(h: HWND, m: Option<HMENU>) {
    unsafe {
        let _ = SetMenu(h, m);
    }
}
pub fn append_menu(m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: Option<&str>) {
    unsafe {
        let _ = match text {
            Some(t) => AppendMenuW(m, flags, id, &hs(t)),
            None => AppendMenuW(m, flags, id, PCWSTR::null()),
        };
    }
}
pub fn delete_menu_pos(m: HMENU, pos: u32) {
    unsafe {
        let _ = DeleteMenu(m, pos, MF_BYPOSITION);
    }
}
pub fn enable_menu_item(m: HMENU, item: u32, by: MENU_ITEM_FLAGS, on: bool) {
    let flags = MENU_ITEM_FLAGS(by.0 | if on { 0 } else { MF_GRAYED.0 });
    unsafe {
        let _ = EnableMenuItem(m, item, flags);
    }
}
pub fn check_menu_item(m: HMENU, cmd: u32, on: bool) {
    unsafe {
        CheckMenuItem(m, cmd, MF_BYCOMMAND.0 | if on { MF_CHECKED.0 } else { 0 });
    }
}
pub fn set_menu_item_text(m: HMENU, item: u32, by_pos: bool, text: &str) {
    let mut buf = wide(text);
    let mii = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING,
        dwTypeData: PWSTR(buf.as_mut_ptr()),
        ..Default::default()
    };
    unsafe {
        let _ = SetMenuItemInfoW(m, item, by_pos, &mii);
    }
}
pub fn track_popup(m: HMENU, owner: HWND, pt: POINT) -> i32 {
    unsafe {
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON;
        TrackPopupMenuEx(m, flags.0, pt.x, pt.y, owner, None).0
    }
}
pub fn make_accel_table(list: &[ACCEL]) -> Option<HACCEL> {
    unsafe { CreateAcceleratorTableW(list).ok() }
}
pub fn destroy_accel_table(h: HACCEL) {
    unsafe {
        let _ = DestroyAcceleratorTable(h);
    }
}

// ---- GDI ----
pub fn delete_object(o: HGDIOBJ) {
    unsafe {
        let _ = DeleteObject(o);
    }
}
pub fn stock_font(i: GET_STOCK_OBJECT_FLAGS) -> HFONT {
    unsafe { HFONT(GetStockObject(i).0) }
}
pub fn sys_brush(i: SYS_COLOR_INDEX) -> HBRUSH {
    unsafe { GetSysColorBrush(i) }
}
pub fn fill_rect(dc: HDC, rc: &RECT, brush: HBRUSH) {
    unsafe {
        FillRect(dc, rc, brush);
    }
}
/// Pixels per logical inch of the screen (vertical), 96 when unavailable.
pub fn system_dpi() -> i32 {
    unsafe {
        let dc = GetDC(None);
        let d = if dc.is_invalid() {
            96
        } else {
            let d = GetDeviceCaps(Some(dc), LOGPIXELSY);
            ReleaseDC(None, dc);
            d
        };
        if d <= 0 { 96 } else { d }
    }
}
/// `NONCLIENTMETRICSW::lfMessageFont` of the current theme, when the system reports it.
pub fn message_logfont() -> Option<LOGFONTW> {
    let mut ncm = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    Some(ncm.lfMessageFont)
}
pub fn create_font(lf: &LOGFONTW) -> Option<HFONT> {
    let f = unsafe { CreateFontIndirectW(lf) };
    (!f.is_invalid()).then_some(f)
}
/// Text extent of one line in `font`, measured on the screen DC. `None` without a DC.
pub fn text_extents(font: HFONT, lines: &[Vec<u16>]) -> Option<Vec<SIZE>> {
    unsafe {
        let dc = GetDC(None);
        if dc.is_invalid() {
            return None;
        }
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let out = lines
            .iter()
            .map(|l| {
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(dc, l, &mut sz);
                sz
            })
            .collect();
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        Some(out)
    }
}
