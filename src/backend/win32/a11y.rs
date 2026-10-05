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
use crate::a11y::{A11yRole, NameSource};

const CLSID_ACC_PROP_SERVICES: GUID = guid(
    0xB5F8350B,
    0x0548,
    0x48B1,
    [0xA6, 0xEE, 0x88, 0xBD, 0x00, 0xB4, 0xA5, 0xE7],
);
const IID_ACC_PROP_SERVICES: GUID = guid(
    0x6E26E776,
    0x04F0,
    0x495D,
    [0x80, 0xE4, 0x33, 0x30, 0x35, 0x2E, 0x31, 0x69],
);
const PROP_NAME: GUID = guid(
    0x608D3DF8,
    0x8128,
    0x4AA7,
    [0xA4, 0x28, 0xF5, 0x5E, 0x49, 0x26, 0x72, 0x91],
);
const PROP_VALUE: GUID = guid(
    0x123FE443,
    0x211A,
    0x4615,
    [0x95, 0x27, 0xC4, 0x5A, 0x7E, 0x93, 0x71, 0x7A],
);
const PROP_DESCRIPTION: GUID = guid(
    0x4D48DFE4,
    0xBD3F,
    0x491F,
    [0xA6, 0x48, 0x49, 0x2D, 0x6F, 0x20, 0xC5, 0x88],
);
const PROP_ROLE: GUID = guid(
    0xCB905FF2,
    0x7BD1,
    0x4C05,
    [0xB3, 0xC8, 0xE6, 0xC2, 0x41, 0x36, 0x4D, 0x70],
);
const PROP_STATE: GUID = guid(
    0xA8D4D5B0,
    0x0A21,
    0x42D0,
    [0xA5, 0xC0, 0x51, 0x4E, 0x98, 0x4F, 0x45, 0x7B],
);

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
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_ACC_PROP_SERVICES,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ACC_PROP_SERVICES,
            &mut o,
        )
    };
    if hr < 0 {
        o = null_mut();
    }
    SERVICE.with(|s| s.set(o as usize));
    o
}

/// ROLE_SYSTEM_* for a role.
fn msaa_role(r: A11yRole) -> i32 {
    use A11yRole as R;
    match r {
        R::Window => 9,
        R::Pane => 16,
        R::Group => 20,
        R::Splitter => 21,
        R::Table => 24,
        R::Link => 30,
        R::ListBox => 33,
        R::Tree => 35,
        R::TabPanel => 38,
        R::Image => 40,
        R::Label => 41,
        R::TextInput | R::PasswordInput | R::MultilineTextInput => 42,
        R::Button => 43,
        R::CheckBox => 44,
        R::RadioButton => 45,
        R::ComboBox => 46,
        R::ProgressBar => 48,
        R::Slider => 51,
        R::SpinButton => 52,
        R::TabList => 60,
        R::MenuBar => 2,
        R::Menu => 11,
        R::MenuItem | R::MenuItemCheckBox => 12,
    }
}

/// What each native HWND of `window` should carry, from the core's resolved names and roles.
///
/// Only what the stock proxies get wrong or cannot know is pushed: explicit app overrides, tooltips
/// (descriptions), and the custom controls (sash, Page/GroupBox containers). Own-text names and names
/// derived from a preceding Label are left to oleacc, which reads the caption itself and labels an
/// input by the preceding STATIC in z-order (how the controls are created).
fn desired(window: WidgetId) -> Vec<(WidgetId, HWND, Props)> {
    let Some(nodes) = crate::a11y::resolve(window) else {
        return vec![];
    };
    let mut out = vec![];
    for n in nodes {
        let Some((hwnd, aux)) = get(n.id, |w| (w.hwnd, w.aux)) else {
            continue;
        };
        if hwnd == 0 {
            continue;
        }
        let custom = matches!(n.kind, Kind::Sash | Kind::Page | Kind::GroupBox);
        let mut p = Props {
            name: n.name.clone().filter(|_| {
                n.name_source == NameSource::Explicit
                    || matches!(n.kind, Kind::Page | Kind::GroupBox)
            }),
            desc: n.description.clone(),
            role: (n.role_explicit || custom).then(|| msaa_role(n.role)),
            ..Props::default()
        };
        if n.kind == Kind::Sash {
            p.value = core::with(|r| {
                let parent = r.nodes.get(&n.id)?.parent?;
                r.nodes
                    .get(&parent)?
                    .split()
                    .map(|sp| sp.actual.to_string())
            })
            .flatten();
        }
        if n.kind == Kind::SpinBox && aux != 0 && (p.name.is_some() || p.desc.is_some()) {
            out.push((
                n.id,
                aux,
                Props {
                    name: p.name.clone(),
                    desc: p.desc.clone(),
                    ..Props::default()
                },
            ));
        }
        if !p.is_empty() {
            out.push((n.id, hwnd, p));
        }
        if n.kind == Kind::GroupBox && aux != 0 {
            out.push((
                n.id,
                aux,
                Props {
                    name: Some(String::new()),
                    role: Some(msaa_role(A11yRole::Pane)),
                    state: Some(STATE_SYSTEM_INVISIBLE),
                    ..Props::default()
                },
            ));
        }
    }
    out
}

unsafe fn set_str(svc: Obj, hwnd: HWND, prop: GUID, v: &str) {
    let w = wide(v);
    unsafe { vt::<SetHwndPropStr>(svc, 7)(svc, hwnd, OBJID_CLIENT as u32, 0, prop, w.as_ptr()) };
}
unsafe fn set_i4(svc: Obj, hwnd: HWND, prop: GUID, v: i32) {
    let var = Variant {
        vt: VT_I4,
        _pad: [0; 3],
        val: v as i64,
    };
    unsafe { vt::<SetHwndProp>(svc, 6)(svc, hwnd, OBJID_CLIENT as u32, 0, prop, var) };
}
unsafe fn clear(svc: Obj, hwnd: HWND, prop: GUID) {
    unsafe { vt::<ClearHwndProps>(svc, 9)(svc, hwnd, OBJID_CLIENT as u32, 0, &prop, 1) };
}

/// Bring one HWND's annotations from `old` to `new`; `notify` raises the matching WinEvents.
unsafe fn sync(svc: Obj, hwnd: HWND, old: &Props, new: &Props, notify: bool) {
    unsafe fn text(
        svc: Obj,
        hwnd: HWND,
        prop: GUID,
        ev: u32,
        old: &Option<String>,
        new: &Option<String>,
        notify: bool,
    ) {
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
    unsafe fn int(
        svc: Obj,
        hwnd: HWND,
        prop: GUID,
        ev: u32,
        old: Option<i32>,
        new: Option<i32>,
        notify: bool,
    ) {
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
        text(
            svc,
            hwnd,
            PROP_NAME,
            EVENT_OBJECT_NAMECHANGE,
            &old.name,
            &new.name,
            notify,
        );
        text(
            svc,
            hwnd,
            PROP_DESCRIPTION,
            EVENT_OBJECT_DESCRIPTIONCHANGE,
            &old.desc,
            &new.desc,
            notify,
        );
        text(
            svc,
            hwnd,
            PROP_VALUE,
            EVENT_OBJECT_VALUECHANGE,
            &old.value,
            &new.value,
            notify,
        );
        int(svc, hwnd, PROP_ROLE, 0, old.role, new.role, false);
        int(
            svc,
            hwnd,
            PROP_STATE,
            EVENT_OBJECT_STATECHANGE,
            old.state,
            new.state,
            notify,
        );
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
    let mut applied = APPLIED
        .with(|a| a.borrow_mut().remove(&window))
        .unwrap_or_default();
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
