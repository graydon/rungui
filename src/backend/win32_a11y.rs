//! Win32 accessibility: the stock controls are already exposed through oleacc's MSAA proxies
//! (name, role, value, state, default action), so no UIA provider is needed. This module only
//! patches the gaps with `IAccPropServices` annotations on each HWND's `OBJID_CLIENT` object:
//!
//! * explicit app overrides (`set_a11y_name` / `set_a11y_description` / `set_a11y_role`) and tooltips
//!   as the description;
//! * the custom controls oleacc knows nothing about: the splitter sash (separator role + position as
//!   value), and the container windows behind `Page` and `GroupBox` (role + name from the caption);
//! * the `BS_GROUPBOX` frame that duplicates the container's caption is hidden.
//!
//! Names that the core only *derives* (an input labelled by the preceding `Label`) are not pushed:
//! oleacc's own label heuristic (preceding STATIC in z-order, which is how the controls are created)
//! already produces them.
//!
//! Not covered: menu items (no HWND; `SetHmenuProp` with the 1-based item position would do it, but
//! explicit overrides on menu items are expected to be rare), and AT actions such as increment or
//! set-value (the sash has no keyboard support either).
//!
//! Annotations live in the process-wide property store keyed by HWND, so they must be cleared
//! before the window is destroyed (`forget`).

use super::*;

const CLSID_ACC_PROP_SERVICES: GUID = guid(0xB5F8350B, 0x0548, 0x48B1, [0xA6, 0xEE, 0x88, 0xBD, 0x00, 0xB4, 0xA5, 0xE7]);
const IID_ACC_PROP_SERVICES: GUID = guid(0x6E26E776, 0x04F0, 0x495D, [0x80, 0xE4, 0x33, 0x30, 0x35, 0x2E, 0x31, 0x69]);
const PROP_NAME: GUID = guid(0x608D3DF8, 0x8128, 0x4AA7, [0xA4, 0x28, 0xF5, 0x5E, 0x49, 0x26, 0x72, 0x91]);
const PROP_VALUE: GUID = guid(0x123FE443, 0x211A, 0x4615, [0x95, 0x27, 0xC4, 0x5A, 0x7E, 0x93, 0x71, 0x7A]);
const PROP_DESCRIPTION: GUID = guid(0x4D48DFE4, 0xBD3F, 0x491F, [0xA6, 0x48, 0x49, 0x2D, 0x6F, 0x20, 0xC5, 0x88]);
const PROP_ROLE: GUID = guid(0xCB905FF2, 0x7BD1, 0x4C05, [0xB3, 0xC8, 0xE6, 0xC2, 0x41, 0x36, 0x4D, 0x70]);
const PROP_STATE: GUID = guid(0xA8D4D5B0, 0x0A21, 0x42D0, [0xA5, 0xC0, 0x51, 0x4E, 0x98, 0x4F, 0x45, 0x7B]);

const OBJID_CLIENT: i32 = -4;
const EVENT_OBJECT_STATECHANGE: u32 = 0x800A;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;
const EVENT_OBJECT_DESCRIPTIONCHANGE: u32 = 0x800D;
const EVENT_OBJECT_VALUECHANGE: u32 = 0x800E;
const VT_I4: u16 = 3;
const STATE_SYSTEM_INVISIBLE: i32 = 0x8000;

/// `VARIANT` (16 bytes on both x86 and x64) holding a `VT_I4`.
#[repr(C)]
struct Variant {
    vt: u16,
    _pad: [u16; 3],
    val: i64,
}

type SetHwndProp = unsafe extern "system" fn(Obj, HWND, u32, u32, GUID, Variant) -> i32;
type SetHwndPropStr = unsafe extern "system" fn(Obj, HWND, u32, u32, GUID, *const u16) -> i32;
type ClearHwndProps = unsafe extern "system" fn(Obj, HWND, u32, u32, *const GUID, i32) -> i32;

/// The desired (or currently applied) annotations of one HWND.
#[derive(Clone, Default, PartialEq, Debug)]
struct Props {
    name: Option<String>,
    desc: Option<String>,
    value: Option<String>,
    role: Option<i32>,
    state: Option<i32>,
}

impl Props {
    fn is_empty(&self) -> bool {
        *self == Props::default()
    }
}

thread_local! {
    /// `IAccPropServices`, created lazily (0 = not yet / unavailable).
    static SERVICE: Cell<usize> = const { Cell::new(0) };
    /// What has been annotated so far, per window and HWND.
    static APPLIED: RefCell<HashMap<WidgetId, HashMap<HWND, Props>>> = RefCell::new(HashMap::new());
}

fn service() -> Obj {
    let cur = SERVICE.with(|s| s.get());
    if cur != 0 {
        return cur as Obj;
    }
    let mut o: Obj = null_mut();
    let hr = unsafe { CoCreateInstance(&CLSID_ACC_PROP_SERVICES, null_mut(), 1, &IID_ACC_PROP_SERVICES, &mut o) };
    if hr < 0 {
        o = null_mut();
    }
    SERVICE.with(|s| s.set(o as usize));
    o
}

/// ROLE_SYSTEM_* for an accesskit role; `None` = leave the native role alone.
fn msaa_role(r: accesskit::Role) -> Option<i32> {
    use accesskit::Role as R;
    Some(match r {
        R::Window => 9,
        R::Pane | R::GenericContainer | R::Section | R::Region => 16,
        R::Dialog | R::AlertDialog => 18,
        R::Group | R::RadioGroup => 20,
        R::Splitter => 21,
        R::Toolbar => 22,
        R::Status => 23,
        R::Table | R::Grid => 24,
        R::ColumnHeader => 25,
        R::RowHeader => 26,
        R::Row => 28,
        R::Cell | R::GridCell => 29,
        R::Link => 30,
        R::List => 33,
        R::ListItem => 34,
        R::Tree => 35,
        R::TreeItem => 36,
        R::Tab => 37,
        R::TabPanel => 38,
        R::Image => 40,
        R::Label => 41,
        R::TextInput | R::MultilineTextInput | R::PasswordInput | R::SearchInput => 42,
        R::Button | R::DefaultButton => 43,
        R::CheckBox | R::Switch => 44,
        R::RadioButton => 45,
        R::ComboBox | R::EditableComboBox => 46,
        R::ListBox => 33,
        R::ListBoxOption => 34,
        R::ProgressIndicator | R::Meter => 48,
        R::Slider => 51,
        R::SpinButton => 52,
        R::TabList => 60,
        R::MenuBar => 2,
        R::Menu | R::MenuListPopup => 11,
        R::MenuItem | R::MenuItemCheckBox | R::MenuItemRadio | R::MenuListOption => 12,
        R::Tooltip => 13,
        R::Document => 15,
        R::Alert => 8,
        R::TitleBar => 1,
        R::ScrollBar => 3,
        R::Application => 14,
        _ => return None,
    })
}

/// What each native HWND of `window` should carry, read straight from the core registry.
fn desired(window: WidgetId) -> Vec<(WidgetId, HWND, Props)> {
    struct Snap {
        id: WidgetId,
        kind: Kind,
        text: String,
        tooltip: String,
        a11y: crate::a11y::A11yProps,
        sash: Option<i32>,
    }
    let snaps: Vec<Snap> = core::with(|r| {
        r.nodes
            .iter()
            .filter(|(id, n)| n.kind.is_native() && r.window_of(**id) == Some(window))
            .map(|(id, n)| Snap {
                id: *id,
                kind: n.kind,
                text: n.text.clone(),
                tooltip: n.tooltip.clone(),
                a11y: n.a11y.clone(),
                sash: (n.kind == Kind::Sash)
                    .then(|| n.parent.and_then(|p| r.nodes.get(&p)).and_then(|p| p.split.as_ref()).map(|sp| sp.actual))
                    .flatten(),
            })
            .collect()
    })
    .unwrap_or_default();

    let mut out = vec![];
    for s in snaps {
        let Some((hwnd, aux)) = get(s.id, |w| (w.hwnd, w.aux)) else { continue };
        if hwnd == 0 {
            continue;
        }
        let mut p = Props::default();
        p.name = s.a11y.name.clone();
        p.desc = s.a11y.desc.clone().or_else(|| (!s.tooltip.is_empty()).then(|| s.tooltip.clone()));
        p.role = s.a11y.role.and_then(msaa_role);
        match s.kind {
            Kind::Sash => {
                p.role = p.role.or(msaa_role(accesskit::Role::Splitter));
                p.value = s.sash.map(|v| v.to_string());
            }
            Kind::Page | Kind::GroupBox => {
                p.role = p.role.or(msaa_role(crate::a11y::default_role(s.kind)));
                if p.name.is_none() && !s.text.is_empty() {
                    p.name = Some(crate::text::strip_mnemonic(&s.text));
                }
            }
            _ => {}
        }
        if s.kind == Kind::SpinBox && aux != 0 && (p.name.is_some() || p.desc.is_some()) {
            out.push((s.id, aux, Props { name: p.name.clone(), desc: p.desc.clone(), ..Props::default() }));
        }
        if !p.is_empty() {
            out.push((s.id, hwnd, p));
        }
        if s.kind == Kind::GroupBox && aux != 0 {
            out.push((s.id, aux, Props { name: Some(String::new()), role: msaa_role(accesskit::Role::Pane), state: Some(STATE_SYSTEM_INVISIBLE), ..Props::default() }));
        }
    }
    out
}

unsafe fn set_str(svc: Obj, hwnd: HWND, prop: GUID, v: &str) {
    let w = wide(v);
    unsafe { vt::<SetHwndPropStr>(svc, 7)(svc, hwnd, OBJID_CLIENT as u32, 0, prop, w.as_ptr()) };
}
unsafe fn set_i4(svc: Obj, hwnd: HWND, prop: GUID, v: i32) {
    let var = Variant { vt: VT_I4, _pad: [0; 3], val: v as i64 };
    unsafe { vt::<SetHwndProp>(svc, 6)(svc, hwnd, OBJID_CLIENT as u32, 0, prop, var) };
}
unsafe fn clear(svc: Obj, hwnd: HWND, prop: GUID) {
    unsafe { vt::<ClearHwndProps>(svc, 9)(svc, hwnd, OBJID_CLIENT as u32, 0, &prop, 1) };
}

/// Bring one HWND's annotations from `old` to `new`; `notify` raises the matching WinEvents.
unsafe fn sync(svc: Obj, hwnd: HWND, old: &Props, new: &Props, notify: bool) {
    unsafe fn text(svc: Obj, hwnd: HWND, prop: GUID, ev: u32, old: &Option<String>, new: &Option<String>, notify: bool) {
        if old == new {
            return;
        }
        unsafe {
            match new {
                Some(v) => set_str(svc, hwnd, prop, v),
                None => clear(svc, hwnd, prop),
            }
            if notify {
                NotifyWinEvent(ev, hwnd, OBJID_CLIENT, 0);
            }
        }
    }
    unsafe fn int(svc: Obj, hwnd: HWND, prop: GUID, ev: u32, old: Option<i32>, new: Option<i32>, notify: bool) {
        if old == new {
            return;
        }
        unsafe {
            match new {
                Some(v) => set_i4(svc, hwnd, prop, v),
                None => clear(svc, hwnd, prop),
            }
            if notify {
                NotifyWinEvent(ev, hwnd, OBJID_CLIENT, 0);
            }
        }
    }
    unsafe {
        text(svc, hwnd, PROP_NAME, EVENT_OBJECT_NAMECHANGE, &old.name, &new.name, notify);
        text(svc, hwnd, PROP_DESCRIPTION, EVENT_OBJECT_DESCRIPTIONCHANGE, &old.desc, &new.desc, notify);
        text(svc, hwnd, PROP_VALUE, EVENT_OBJECT_VALUECHANGE, &old.value, &new.value, notify);
        int(svc, hwnd, PROP_ROLE, 0, old.role, new.role, false);
        int(svc, hwnd, PROP_STATE, EVENT_OBJECT_STATECHANGE, old.state, new.state, notify);
    }
}

/// Make the annotations of every native HWND in `window` match the core's current model.
pub(super) fn apply(window: WidgetId) {
    let want = desired(window);
    let svc = service();
    if svc.is_null() {
        return;
    }
    let mut want_by_hwnd: HashMap<HWND, Props> = HashMap::new();
    for (_, h, p) in want {
        want_by_hwnd.insert(h, p);
    }
    let mut applied = APPLIED.with(|a| a.borrow_mut().remove(&window)).unwrap_or_default();
    let none = Props::default();
    for (h, old) in applied.iter() {
        if !want_by_hwnd.contains_key(h) {
            unsafe { sync(svc, *h, old, &none, false) };
        }
    }
    applied.retain(|h, _| want_by_hwnd.contains_key(h));
    for (h, new) in want_by_hwnd {
        let seen = applied.contains_key(&h);
        let old = applied.get(&h).cloned().unwrap_or_default();
        unsafe { sync(svc, h, &old, &new, seen) };
        applied.insert(h, new);
    }
    APPLIED.with(|a| a.borrow_mut().insert(window, applied));
}

/// Remove the annotations of `hwnds` (call before the windows are destroyed).
pub(super) fn forget(window: WidgetId, hwnds: &[HWND]) {
    let svc = SERVICE.with(|s| s.get()) as Obj;
    APPLIED.with(|a| {
        let mut a = a.borrow_mut();
        let Some(m) = a.get_mut(&window) else { return };
        for h in hwnds {
            if let Some(old) = m.remove(h) {
                if !svc.is_null() {
                    unsafe { sync(svc, *h, &old, &Props::default(), false) };
                }
            }
        }
    });
}

/// Remove every annotation of `window` (call before the window is destroyed).
pub(super) fn forget_window(window: WidgetId) {
    let svc = SERVICE.with(|s| s.get()) as Obj;
    if let Some(m) = APPLIED.with(|a| a.borrow_mut().remove(&window)) {
        if !svc.is_null() {
            for (h, old) in m {
                unsafe { sync(svc, h, &old, &Props::default(), false) };
            }
        }
    }
}

/// The HWND of a widget was replaced: move its annotations to the new one.
pub(super) fn rehome(window: WidgetId, old: HWND, new: HWND) {
    let svc = SERVICE.with(|s| s.get()) as Obj;
    APPLIED.with(|a| {
        let mut a = a.borrow_mut();
        let Some(m) = a.get_mut(&window) else { return };
        if let Some(p) = m.remove(&old) {
            if !svc.is_null() {
                unsafe {
                    sync(svc, old, &p, &Props::default(), false);
                    sync(svc, new, &Props::default(), &p, false);
                }
            }
            m.insert(new, p);
        }
    });
}
