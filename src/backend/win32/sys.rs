//! Hand-written Win32 declarations (the subset rungui needs). Everything is `extern "system"`;
//! handles are pointer-sized integers (`isize`), which has the same ABI as a pointer.
#![allow(
    non_snake_case,
    non_camel_case_types,
    dead_code,
    clippy::upper_case_acronyms
)]

use std::ffi::c_void;

pub type HWND = isize;
pub type HANDLE = isize;
pub type LPARAM = isize;
pub type WPARAM = usize;
pub type LRESULT = isize;
pub type BOOL = i32;
pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct SIZE {
    pub cx: i32,
    pub cy: i32,
}
#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wparam: WPARAM,
    pub lparam: LPARAM,
    pub time: u32,
    pub pt: POINT,
}
impl MSG {
    pub fn zeroed() -> MSG {
        MSG {
            hwnd: 0,
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: POINT::default(),
        }
    }
}
#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: Option<WndProc>,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: isize,
    pub hIcon: isize,
    pub hCursor: isize,
    pub hbrBackground: isize,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: isize,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct LOGFONTW {
    pub lfHeight: i32,
    pub lfWidth: i32,
    pub lfEscapement: i32,
    pub lfOrientation: i32,
    pub lfWeight: i32,
    pub lfItalic: u8,
    pub lfUnderline: u8,
    pub lfStrikeOut: u8,
    pub lfCharSet: u8,
    pub lfOutPrecision: u8,
    pub lfClipPrecision: u8,
    pub lfQuality: u8,
    pub lfPitchAndFamily: u8,
    pub lfFaceName: [u16; 32],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct NONCLIENTMETRICSW {
    pub cbSize: u32,
    pub iBorderWidth: i32,
    pub iScrollWidth: i32,
    pub iScrollHeight: i32,
    pub iCaptionWidth: i32,
    pub iCaptionHeight: i32,
    pub lfCaptionFont: LOGFONTW,
    pub iSmCaptionWidth: i32,
    pub iSmCaptionHeight: i32,
    pub lfSmCaptionFont: LOGFONTW,
    pub iMenuWidth: i32,
    pub iMenuHeight: i32,
    pub lfMenuFont: LOGFONTW,
    pub lfStatusFont: LOGFONTW,
    pub lfMessageFont: LOGFONTW,
    pub iPaddedBorderWidth: i32,
}
#[repr(C)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}
#[repr(C)]
pub struct DRAWITEMSTRUCT {
    pub CtlType: u32,
    pub CtlID: u32,
    pub itemID: u32,
    pub itemAction: u32,
    pub itemState: u32,
    pub hwndItem: HWND,
    pub hDC: isize,
    pub rcItem: RECT,
    pub itemData: usize,
}
#[repr(C)]
pub struct NMHDR {
    pub hwndFrom: HWND,
    pub idFrom: usize,
    pub code: u32,
}
#[repr(C)]
pub struct NMUPDOWN {
    pub hdr: NMHDR,
    pub iPos: i32,
    pub iDelta: i32,
}
#[repr(C)]
pub struct LVCOLUMNW {
    pub mask: u32,
    pub fmt: i32,
    pub cx: i32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iSubItem: i32,
    pub iImage: i32,
    pub iOrder: i32,
}
#[repr(C)]
pub struct LVITEMW {
    pub mask: u32,
    pub iItem: i32,
    pub iSubItem: i32,
    pub state: u32,
    pub stateMask: u32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iImage: i32,
    pub lParam: isize,
}
#[repr(C)]
pub struct LVHITTESTINFO {
    pub pt: POINT,
    pub flags: u32,
    pub iItem: i32,
    pub iSubItem: i32,
    pub iGroup: i32,
}
#[repr(C)]
pub struct NMLISTVIEW {
    pub hdr: NMHDR,
    pub iItem: i32,
    pub iSubItem: i32,
    pub uNewState: u32,
    pub uOldState: u32,
    pub uChanged: u32,
    pub ptAction: POINT,
    pub lParam: isize,
}
#[repr(C)]
pub struct HDITEMW {
    pub mask: u32,
    pub cxy: i32,
    pub pszText: *mut u16,
    pub hbm: isize,
    pub cchTextMax: i32,
    pub fmt: i32,
    pub lParam: isize,
    pub iImage: i32,
    pub iOrder: i32,
}
#[repr(C)]
pub struct TVITEMEXW {
    pub mask: u32,
    pub hItem: isize,
    pub state: u32,
    pub stateMask: u32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iImage: i32,
    pub iSelectedImage: i32,
    pub cChildren: i32,
    pub lParam: isize,
}
#[repr(C)]
pub struct TVINSERTSTRUCTW {
    pub hParent: isize,
    pub hInsertAfter: isize,
    pub item: TVITEMEXW,
}
#[repr(C)]
pub struct NMTREEVIEWW {
    pub hdr: NMHDR,
    pub action: u32,
    pub itemOld: TVITEMEXW,
    pub itemNew: TVITEMEXW,
    pub ptDrag: POINT,
}
#[repr(C)]
pub struct TVHITTESTINFO {
    pub pt: POINT,
    pub flags: u32,
    pub hItem: isize,
}
#[repr(C)]
pub struct MINMAXINFO {
    pub ptReserved: POINT,
    pub ptMaxSize: POINT,
    pub ptMaxPosition: POINT,
    pub ptMinTrackSize: POINT,
    pub ptMaxTrackSize: POINT,
}
#[repr(C)]
pub struct MENUITEMINFOW {
    pub cbSize: u32,
    pub fMask: u32,
    pub fType: u32,
    pub fState: u32,
    pub wID: u32,
    pub hSubMenu: isize,
    pub hbmpChecked: isize,
    pub hbmpUnchecked: isize,
    pub dwItemData: usize,
    pub dwTypeData: *mut u16,
    pub cch: u32,
    pub hbmpItem: isize,
}
#[repr(C)]
pub struct TCITEMW {
    pub mask: u32,
    pub dwState: u32,
    pub dwStateMask: u32,
    pub pszText: *mut u16,
    pub cchTextMax: i32,
    pub iImage: i32,
    pub lParam: isize,
}
#[repr(C)]
pub struct TOOLINFOW {
    pub cbSize: u32,
    pub uFlags: u32,
    pub hwnd: HWND,
    pub uId: usize,
    pub rect: RECT,
    pub hinst: isize,
    pub lpszText: *mut u16,
    pub lParam: isize,
    pub lpReserved: *mut c_void,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ACCEL {
    pub fVirt: u8,
    pub key: u16,
    pub cmd: u16,
}
#[repr(C)]
pub struct INITCOMMONCONTROLSEX {
    pub dwSize: u32,
    pub dwICC: u32,
}
#[repr(C)]
pub struct ACTCTXW {
    pub cbSize: u32,
    pub dwFlags: u32,
    pub lpSource: *const u16,
    pub wProcessorArchitecture: u16,
    pub wLangId: u16,
    pub lpAssemblyDirectory: *const u16,
    pub lpResourceName: *const u16,
    pub lpApplicationName: *const u16,
    pub hModule: isize,
}
#[repr(C)]
pub struct GUID {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}
#[repr(C)]
pub struct COMDLG_FILTERSPEC {
    pub name: *const u16,
    pub spec: *const u16,
}

// ---- compile-time layout checks against the Windows SDK ----
const _: () = {
    use std::mem::size_of;
    assert!(size_of::<POINT>() == 8);
    assert!(size_of::<RECT>() == 16);
    assert!(size_of::<SIZE>() == 8);
    assert!(size_of::<LOGFONTW>() == 92);
    assert!(size_of::<NONCLIENTMETRICSW>() == 504);
    assert!(size_of::<BITMAPINFOHEADER>() == 40);
    assert!(size_of::<BLENDFUNCTION>() == 4);
    assert!(size_of::<ACCEL>() == 6);
    assert!(size_of::<INITCOMMONCONTROLSEX>() == 8);
    assert!(size_of::<GUID>() == 16);
    #[cfg(target_pointer_width = "64")]
    {
        assert!(size_of::<MSG>() == 48);
        assert!(size_of::<WNDCLASSEXW>() == 80);
        assert!(size_of::<DRAWITEMSTRUCT>() == 64);
        assert!(size_of::<NMHDR>() == 24);
        assert!(size_of::<NMUPDOWN>() == 32);
        assert!(size_of::<MENUITEMINFOW>() == 80);
        assert!(size_of::<TCITEMW>() == 40);
        assert!(size_of::<TOOLINFOW>() == 72);
        assert!(size_of::<ACTCTXW>() == 56);
        assert!(size_of::<COMDLG_FILTERSPEC>() == 16);
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert!(size_of::<MSG>() == 28);
        assert!(size_of::<WNDCLASSEXW>() == 48);
        assert!(size_of::<DRAWITEMSTRUCT>() == 48);
        assert!(size_of::<NMHDR>() == 12);
        assert!(size_of::<MENUITEMINFOW>() == 48);
        assert!(size_of::<TCITEMW>() == 28);
        assert!(size_of::<TOOLINFOW>() == 48);
    }
};

// ---- window styles / messages / misc constants ----
pub const WS_OVERLAPPEDWINDOW: u32 = 0x00CF0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x40000000;
pub const WS_VISIBLE: u32 = 0x10000000;
pub const WS_TABSTOP: u32 = 0x00010000;
pub const WS_VSCROLL: u32 = 0x00200000;
pub const WS_CLIPSIBLINGS: u32 = 0x04000000;
pub const WS_CLIPCHILDREN: u32 = 0x02000000;
pub const WS_THICKFRAME: u32 = 0x00040000;
pub const WS_MAXIMIZEBOX: u32 = 0x00010000;
pub const WS_EX_CLIENTEDGE: u32 = 0x200;
pub const WS_EX_TRANSPARENT: u32 = 0x20;
pub const WS_EX_CONTROLPARENT: u32 = 0x10000;
pub const CW_USEDEFAULT: i32 = i32::MIN;

pub const SS_NOPREFIX: u32 = 0x80;
pub const SS_OWNERDRAW: u32 = 0xD;
pub const BS_AUTOCHECKBOX: u32 = 3;
pub const BS_RADIOBUTTON: u32 = 4;
pub const BS_GROUPBOX: u32 = 7;
pub const ES_PASSWORD: u32 = 0x20;
pub const ES_MULTILINE: u32 = 4;
pub const ES_AUTOVSCROLL: u32 = 0x40;
pub const ES_AUTOHSCROLL: u32 = 0x80;
pub const ES_WANTRETURN: u32 = 0x1000;
pub const CBS_DROPDOWNLIST: u32 = 3;
pub const LBS_NOTIFY: u32 = 1;
pub const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
pub const TBS_NOTICKS: u32 = 0x10;
pub const PBS_MARQUEE: u32 = 8;

pub const WM_CREATE: u32 = 0x1;
pub const WM_SIZE: u32 = 0x5;
pub const WM_ACTIVATE: u32 = 0x6;
pub const WM_SETFOCUS: u32 = 0x7;
pub const WM_KILLFOCUS: u32 = 0x8;
pub const WM_SETFONT: u32 = 0x30;
pub const WM_CLOSE: u32 = 0x10;
pub const WM_ERASEBKGND: u32 = 0x14;
pub const WM_SETTEXT: u32 = 0xC;
pub const WM_NOTIFY: u32 = 0x4E;
pub const WM_COMMAND: u32 = 0x111;
pub const WM_TIMER: u32 = 0x113;
pub const WM_HSCROLL: u32 = 0x114;
pub const WM_DRAWITEM: u32 = 0x2B;
pub const WM_KEYDOWN: u32 = 0x100;
pub const WM_CTLCOLORSTATIC: u32 = 0x138;
pub const WM_CTLCOLORBTN: u32 = 0x135;
pub const WM_NEXTDLGCTL: u32 = 0x28;
pub const WM_NCDESTROY: u32 = 0x82;
pub const WM_DPICHANGED: u32 = 0x2E0;
pub const WM_APP: u32 = 0x8000;
pub const SIZE_MINIMIZED: usize = 1;
pub const VK_RETURN: usize = 0x0D;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_END: usize = 0x23;
pub const VK_HOME: usize = 0x24;
pub const VK_LEFT: usize = 0x25;
pub const VK_UP: usize = 0x26;
pub const VK_RIGHT: usize = 0x27;
pub const VK_DOWN: usize = 0x28;
pub const WM_GETDLGCODE: u32 = 0x87;
pub const DLGC_WANTARROWS: LRESULT = 0x1;

pub const BM_GETCHECK: u32 = 0xF0;
pub const BM_SETCHECK: u32 = 0xF1;
pub const EM_SETREADONLY: u32 = 0xCF;
pub const EM_SETLIMITTEXT: u32 = 0xC5;
pub const EM_SETCUEBANNER: u32 = 0x1501;
pub const CB_ADDSTRING: u32 = 0x143;
pub const CB_INITSTORAGE: u32 = 0x161;
pub const CB_RESETCONTENT: u32 = 0x14B;
pub const CB_GETCURSEL: u32 = 0x147;
pub const CB_SETCURSEL: u32 = 0x14E;
pub const CB_SETMINVISIBLE: u32 = 0x1701;
pub const LB_ADDSTRING: u32 = 0x180;
pub const LB_INITSTORAGE: u32 = 0x1A8;
pub const LB_RESETCONTENT: u32 = 0x184;
pub const LB_SETCURSEL: u32 = 0x186;
pub const LB_GETCURSEL: u32 = 0x188;
pub const TBM_GETPOS: u32 = 0x400;
pub const TBM_SETPOS: u32 = 0x405;
pub const TBM_SETRANGEMIN: u32 = 0x407;
pub const TBM_SETRANGEMAX: u32 = 0x408;
pub const PBM_SETRANGE32: u32 = 0x406;
pub const PBM_SETPOS: u32 = 0x402;
pub const PBM_SETMARQUEE: u32 = 0x40A;
pub const UDM_SETRANGE32: u32 = 0x46F;
pub const TCM_GETCURSEL: u32 = 0x130B;
pub const TCM_SETCURSEL: u32 = 0x130C;
pub const TCM_ADJUSTRECT: u32 = 0x1328;
pub const TCM_DELETEITEM: u32 = 0x1308;
pub const TCM_SETITEMW: u32 = 0x133D;
pub const TCM_INSERTITEMW: u32 = 0x133E;
pub const TCIF_TEXT: u32 = 1;
pub const TTM_ADDTOOLW: u32 = 0x432;
pub const TTM_DELTOOLW: u32 = 0x433;
pub const TTM_UPDATETIPTEXTW: u32 = 0x439;
pub const TTM_SETMAXTIPWIDTH: u32 = 0x418;
pub const TTF_IDISHWND: u32 = 1;
pub const TTF_SUBCLASS: u32 = 0x10;
pub const TTS_ALWAYSTIP: u32 = 1;
pub const TCN_SELCHANGE: i32 = -551;
pub const UDN_DELTAPOS: i32 = -722;

pub const RDW_INVALIDATE: u32 = 1;
pub const RDW_ERASE: u32 = 4;
pub const RDW_ALLCHILDREN: u32 = 0x80;
pub const RDW_UPDATENOW: u32 = 0x100;
pub const WS_BORDER: u32 = 0x00800000;
pub const WS_HSCROLL: u32 = 0x00100000;
pub const LVS_REPORT: u32 = 1;
pub const LVS_SINGLESEL: u32 = 4;
pub const LVS_SHOWSELALWAYS: u32 = 8;
pub const LVS_NOSORTHEADER: u32 = 0x8000;
pub const LVS_EX_FULLROWSELECT: u32 = 0x20;
pub const LVS_EX_LABELTIP: u32 = 0x4000;
pub const LVS_EX_DOUBLEBUFFER: u32 = 0x10000;
pub const TVS_HASBUTTONS: u32 = 1;
pub const TVS_HASLINES: u32 = 2;
pub const TVS_LINESATROOT: u32 = 4;
pub const TVS_SHOWSELALWAYS: u32 = 0x20;
pub const TVS_DISABLEDRAGDROP: u32 = 0x10;
pub const WM_SETREDRAW: u32 = 0xB;
pub const WM_MOVE: u32 = 0x3;
pub const WM_GETMINMAXINFO: u32 = 0x24;
pub const WM_SETCURSOR: u32 = 0x20;
pub const WM_CONTEXTMENU: u32 = 0x7B;
pub const WM_MOUSEMOVE: u32 = 0x200;
pub const WM_LBUTTONDOWN: u32 = 0x201;
pub const WM_LBUTTONUP: u32 = 0x202;
pub const WM_CAPTURECHANGED: u32 = 0x215;
pub const WM_NULL: u32 = 0;
pub const EM_GETSEL: u32 = 0xB0;
pub const EM_SETSEL: u32 = 0xB1;
pub const LVM_GETITEMCOUNT: u32 = 0x1004;
pub const LVM_DELETEALLITEMS: u32 = 0x1009;
pub const LVM_GETNEXTITEM: u32 = 0x100C;
pub const LVM_GETITEMRECT: u32 = 0x100E;
pub const LVM_GETITEMPOSITION: u32 = 0x1010;
pub const LVM_HITTEST: u32 = 0x1012;
pub const LVM_ENSUREVISIBLE: u32 = 0x1013;
pub const LVM_SCROLL: u32 = 0x1014;
pub const LVM_DELETECOLUMN: u32 = 0x101C;
pub const LVM_GETHEADER: u32 = 0x101F;
pub const LVM_SETITEMSTATE: u32 = 0x102B;
pub const LVM_SETITEMCOUNT: u32 = 0x102F;
pub const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
pub const LVM_INSERTITEMW: u32 = 0x104D;
pub const LVM_INSERTCOLUMNW: u32 = 0x1061;
pub const LVM_GETCOLUMNWIDTH: u32 = 0x101D;
pub const LVM_SETCOLUMNWIDTH: u32 = 0x101E;
pub const LVM_SETITEMTEXTW: u32 = 0x1074;
pub const LVCF_FMT: u32 = 1;
pub const LVCF_WIDTH: u32 = 2;
pub const LVCF_TEXT: u32 = 4;
pub const LVCF_SUBITEM: u32 = 8;
pub const LVCFMT_LEFT: i32 = 0;
pub const LVCFMT_RIGHT: i32 = 1;
pub const LVCFMT_CENTER: i32 = 2;
pub const LVIF_TEXT: u32 = 1;
pub const LVIF_STATE: u32 = 8;
pub const LVIS_FOCUSED: u32 = 1;
pub const LVIS_SELECTED: u32 = 2;
pub const LVNI_SELECTED: usize = 2;
pub const LVIR_LABEL: i32 = 2;
pub const HDM_GETITEMW: u32 = 0x120B;
pub const HDM_SETITEMW: u32 = 0x120C;
pub const HDI_FORMAT: u32 = 4;
pub const HDF_SORTUP: i32 = 0x400;
pub const HDF_SORTDOWN: i32 = 0x200;
pub const LVN_ITEMCHANGED: i32 = -101;
pub const LVN_COLUMNCLICK: i32 = -108;
pub const LVN_KEYDOWN: i32 = -155;
pub const NM_DBLCLK: i32 = -3;
pub const TVM_INSERTITEMW: u32 = 0x1132;
pub const TVM_DELETEITEM: u32 = 0x1101;
pub const TVM_EXPAND: u32 = 0x1102;
pub const TVM_GETITEMRECT: u32 = 0x1104;
pub const TVM_GETNEXTITEM: u32 = 0x110A;
pub const TVM_SELECTITEM: u32 = 0x110B;
pub const TVM_HITTEST: u32 = 0x1111;
pub const TVM_ENSUREVISIBLE: u32 = 0x1114;
pub const TVM_GETITEMW: u32 = 0x113E;
pub const TVM_SETEXTENDEDSTYLE: u32 = 0x112C;
pub const TVS_EX_DOUBLEBUFFER: u32 = 4;
pub const TVGN_CARET: usize = 9;
pub const TVE_COLLAPSE: usize = 1;
pub const TVE_EXPAND: usize = 2;
pub const TVIF_TEXT: u32 = 1;
pub const TVIF_PARAM: u32 = 4;
pub const TVIF_CHILDREN: u32 = 0x40;
pub const TVHT_ONITEMBUTTON: u32 = 0x10;
pub const TVI_ROOT: isize = -0x10000;
pub const TVI_LAST: isize = -0xFFFE;
pub const TVN_SELCHANGEDW: i32 = -451;
pub const TVN_ITEMEXPANDEDW: i32 = -455;
pub const TVN_KEYDOWN: i32 = -412;
pub const TPM_RIGHTBUTTON: u32 = 2;
pub const TPM_NONOTIFY: u32 = 0x80;
pub const TPM_RETURNCMD: u32 = 0x100;
pub const IDC_SIZEWE: usize = 32644;
pub const IDC_SIZENS: usize = 32645;
pub const VK_APPS: usize = 0x5D;
pub const SB_VERT: i32 = 1;
pub const HTCLIENT: isize = 1;
pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const SW_SHOW: i32 = 5;
pub const SWP_NOSIZE: u32 = 1;
pub const SWP_NOMOVE: u32 = 2;
pub const SWP_NOZORDER: u32 = 4;
pub const SWP_NOACTIVATE: u32 = 0x10;
pub const SWP_FRAMECHANGED: u32 = 0x20;
pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
pub const GWLP_WNDPROC: i32 = -4;
pub const GA_ROOT: u32 = 2;
pub const HWND_MESSAGE: HWND = -3;
pub const IDC_ARROW: usize = 32512;
pub const COLOR_BTNFACE: i32 = 15;
pub const COLOR_WINDOW: i32 = 5;
pub const NULL_BRUSH: i32 = 5;
pub const DEFAULT_GUI_FONT: i32 = 17;
pub const ANSI_FIXED_FONT: i32 = 11;
pub const TRANSPARENT: i32 = 1;
pub const LOGPIXELSY: i32 = 90;
pub const SPI_GETNONCLIENTMETRICS: u32 = 0x29;
pub const SM_CXVSCROLL: i32 = 2;
pub const MF_STRING: u32 = 0;
pub const MF_POPUP: u32 = 0x10;
pub const MF_SEPARATOR: u32 = 0x800;
pub const MF_BYCOMMAND: u32 = 0;
pub const MF_BYPOSITION: u32 = 0x400;
pub const MF_GRAYED: u32 = 1;
pub const MF_CHECKED: u32 = 8;
pub const MIIM_STRING: u32 = 0x40;
pub const FVIRTKEY: u8 = 1;
pub const FSHIFT: u8 = 4;
pub const FCONTROL: u8 = 8;
pub const FALT: u8 = 0x10;
pub const MB_OKCANCEL: u32 = 1;
pub const MB_YESNOCANCEL: u32 = 3;
pub const MB_YESNO: u32 = 4;
pub const MB_ICONERROR: u32 = 0x10;
pub const MB_ICONQUESTION: u32 = 0x20;
pub const MB_ICONWARNING: u32 = 0x30;
pub const MB_ICONINFORMATION: u32 = 0x40;
pub const MB_APPLMODAL: u32 = 0;
pub const IDOK: i32 = 1;
pub const IDYES: i32 = 6;
pub const IDNO: i32 = 7;
pub const TABP_BODY: i32 = 10;

#[link(name = "user32")]
unsafe extern "system" {
    pub fn RegisterClassExW(c: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: isize,
        inst: isize,
        param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(h: HWND) -> BOOL;
    pub fn DefWindowProcW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn CallWindowProcW(p: isize, h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn GetMessageW(m: *mut MSG, h: HWND, min: u32, max: u32) -> BOOL;
    pub fn TranslateMessage(m: *const MSG) -> BOOL;
    pub fn DispatchMessageW(m: *const MSG) -> LRESULT;
    pub fn PostMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> BOOL;
    pub fn SendMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn PostQuitMessage(code: i32);
    pub fn ShowWindow(h: HWND, cmd: i32) -> BOOL;
    pub fn UpdateWindow(h: HWND) -> BOOL;
    pub fn SetWindowPos(h: HWND, after: HWND, x: i32, y: i32, w: i32, cy: i32, flags: u32) -> BOOL;
    pub fn GetClientRect(h: HWND, r: *mut RECT) -> BOOL;
    pub fn GetWindowTextLengthW(h: HWND) -> i32;
    pub fn GetWindowTextW(h: HWND, buf: *mut u16, max: i32) -> i32;
    pub fn SetWindowTextW(h: HWND, s: *const u16) -> BOOL;
    pub fn EnableWindow(h: HWND, e: BOOL) -> BOOL;
    pub fn SetFocus(h: HWND) -> HWND;
    pub fn NotifyWinEvent(event: u32, h: HWND, id_object: i32, id_child: i32);
    pub fn GetFocus() -> HWND;
    pub fn IsChild(parent: HWND, h: HWND) -> BOOL;
    pub fn IsWindow(h: HWND) -> BOOL;
    pub fn GetParent(h: HWND) -> HWND;
    pub fn MapWindowPoints(from: HWND, to: HWND, pts: *mut POINT, n: u32) -> i32;
    pub fn RedrawWindow(h: HWND, r: *const RECT, rgn: isize, flags: u32) -> BOOL;
    pub fn SetCapture(h: HWND) -> HWND;
    pub fn ReleaseCapture() -> BOOL;
    pub fn GetCapture() -> HWND;
    pub fn SetCursor(c: isize) -> isize;
    pub fn GetCursorPos(p: *mut POINT) -> BOOL;
    pub fn ScreenToClient(h: HWND, p: *mut POINT) -> BOOL;
    pub fn ClientToScreen(h: HWND, p: *mut POINT) -> BOOL;
    pub fn SetForegroundWindow(h: HWND) -> BOOL;
    pub fn GetForegroundWindow() -> HWND;
    pub fn TrackPopupMenuEx(
        m: isize,
        flags: u32,
        x: i32,
        y: i32,
        h: HWND,
        tpm: *const c_void,
    ) -> i32;
    pub fn GetScrollPos(h: HWND, bar: i32) -> i32;
    pub fn GetKeyState(vk: i32) -> i16;
    pub fn DrawFocusRect(dc: isize, r: *const RECT) -> BOOL;
    pub fn IsWindowVisible(h: HWND) -> BOOL;
    pub fn GetWindowRect(h: HWND, r: *mut RECT) -> BOOL;
    pub fn SetTimer(h: HWND, id: usize, ms: u32, f: usize) -> usize;
    pub fn KillTimer(h: HWND, id: usize) -> BOOL;
    pub fn SetMenu(h: HWND, m: isize) -> BOOL;
    pub fn DrawMenuBar(h: HWND) -> BOOL;
    pub fn CreateMenu() -> isize;
    pub fn CreatePopupMenu() -> isize;
    pub fn DestroyMenu(m: isize) -> BOOL;
    pub fn AppendMenuW(m: isize, flags: u32, id: usize, s: *const u16) -> BOOL;
    pub fn DeleteMenu(m: isize, pos: u32, flags: u32) -> BOOL;
    pub fn SetMenuItemInfoW(m: isize, item: u32, by_pos: BOOL, mii: *const MENUITEMINFOW) -> BOOL;
    pub fn EnableMenuItem(m: isize, item: u32, flags: u32) -> BOOL;
    pub fn CheckMenuItem(m: isize, item: u32, flags: u32) -> u32;
    pub fn GetDC(h: HWND) -> isize;
    pub fn ReleaseDC(h: HWND, dc: isize) -> i32;
    pub fn GetSysColorBrush(i: i32) -> isize;
    pub fn FillRect(dc: isize, r: *const RECT, brush: isize) -> i32;
    pub fn InvalidateRect(h: HWND, r: *const RECT, erase: BOOL) -> BOOL;
    pub fn SetWindowLongPtrW(h: HWND, idx: i32, v: isize) -> isize;
    pub fn GetWindowLongPtrW(h: HWND, idx: i32) -> isize;
    pub fn GetAncestor(h: HWND, flags: u32) -> HWND;
    pub fn LoadCursorW(inst: isize, name: usize) -> isize;
    pub fn MessageBoxW(h: HWND, text: *const u16, title: *const u16, flags: u32) -> i32;
    pub fn GetNextDlgTabItem(dlg: HWND, ctl: HWND, prev: BOOL) -> HWND;
    pub fn IsDialogMessageW(h: HWND, m: *const MSG) -> BOOL;
    pub fn TranslateAcceleratorW(h: HWND, t: isize, m: *const MSG) -> i32;
    pub fn CreateAcceleratorTableW(a: *const ACCEL, n: i32) -> isize;
    pub fn DestroyAcceleratorTable(t: isize) -> BOOL;
    pub fn AdjustWindowRectEx(r: *mut RECT, style: u32, menu: BOOL, ex: u32) -> BOOL;
    pub fn SystemParametersInfoW(action: u32, p1: u32, p2: *mut c_void, ini: u32) -> BOOL;
    pub fn GetSystemMetrics(i: i32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    pub fn CreateFontIndirectW(lf: *const LOGFONTW) -> isize;
    pub fn DeleteObject(o: isize) -> BOOL;
    pub fn SelectObject(dc: isize, o: isize) -> isize;
    pub fn GetStockObject(i: i32) -> isize;
    pub fn GetTextExtentPoint32W(dc: isize, s: *const u16, n: i32, sz: *mut SIZE) -> BOOL;
    pub fn GetDeviceCaps(dc: isize, idx: i32) -> i32;
    pub fn CreateCompatibleDC(dc: isize) -> isize;
    pub fn DeleteDC(dc: isize) -> BOOL;
    pub fn CreateDIBSection(
        dc: isize,
        bmi: *const BITMAPINFOHEADER,
        usage: u32,
        bits: *mut *mut c_void,
        section: isize,
        offset: u32,
    ) -> isize;
    pub fn SetBkMode(dc: isize, mode: i32) -> i32;
}

#[link(name = "msimg32")]
unsafe extern "system" {
    pub fn AlphaBlend(
        dst: isize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: isize,
        xs: i32,
        ys: i32,
        ws: i32,
        hs: i32,
        bf: BLENDFUNCTION,
    ) -> BOOL;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> isize;
    pub fn LoadLibraryW(name: *const u16) -> isize;
    pub fn GetProcAddress(m: isize, name: *const u8) -> *const c_void;
    pub fn GetLastError() -> u32;
    pub fn CreateActCtxW(c: *const ACTCTXW) -> isize;
    pub fn ActivateActCtx(h: isize, cookie: *mut usize) -> BOOL;
}

#[link(name = "uxtheme")]
unsafe extern "system" {
    pub fn OpenThemeData(h: HWND, classes: *const u16) -> isize;
    pub fn CloseThemeData(t: isize) -> i32;
    pub fn DrawThemeBackground(
        t: isize,
        dc: isize,
        part: i32,
        state: i32,
        r: *const RECT,
        clip: *const RECT,
    ) -> i32;
    pub fn DrawThemeParentBackground(h: HWND, dc: isize, r: *const RECT) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    pub fn SHCreateItemFromParsingName(
        path: *const u16,
        bc: *mut c_void,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    pub fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    pub fn CoCreateInstance(
        clsid: *const GUID,
        outer: *mut c_void,
        ctx: u32,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> i32;
    pub fn CoTaskMemFree(p: *mut c_void);
}

// notification codes and messages that were bare numbers in the backend
pub const WM_VSCROLL: u32 = 0x115;
pub const SB_THUMBPOSITION: usize = 4;
/// `EN_CHANGE`: the text of an edit control changed.
pub const EN_CHANGE: u32 = 0x300;
/// `InitCommonControlsEx` classes: every common control class the backend creates
/// (list view, tree view, tab, tooltip, trackbar, up-down, progress, standard).
pub const ICC_ALL_USED: u32 = 0x40FF;
