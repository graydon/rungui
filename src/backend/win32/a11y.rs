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
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    CAccPropServices, IAccPropServices, NotifyWinEvent, PROPID_ACC_DESCRIPTION, PROPID_ACC_NAME,
    PROPID_ACC_ROLE, PROPID_ACC_STATE, PROPID_ACC_VALUE, ROLE_SYSTEM_CHECKBUTTON,
    ROLE_SYSTEM_COMBOBOX, ROLE_SYSTEM_GRAPHIC, ROLE_SYSTEM_GROUPING, ROLE_SYSTEM_LINK,
    ROLE_SYSTEM_LIST, ROLE_SYSTEM_MENUBAR, ROLE_SYSTEM_MENUITEM, ROLE_SYSTEM_MENUPOPUP,
    ROLE_SYSTEM_OUTLINE, ROLE_SYSTEM_PAGETABLIST, ROLE_SYSTEM_PANE, ROLE_SYSTEM_PROGRESSBAR,
    ROLE_SYSTEM_PROPERTYPAGE, ROLE_SYSTEM_PUSHBUTTON, ROLE_SYSTEM_RADIOBUTTON,
    ROLE_SYSTEM_SEPARATOR, ROLE_SYSTEM_SLIDER, ROLE_SYSTEM_SPINBUTTON, ROLE_SYSTEM_STATICTEXT,
    ROLE_SYSTEM_TABLE, ROLE_SYSTEM_TEXT, ROLE_SYSTEM_WINDOW,
};
use windows::core::GUID;

const STATE_SYSTEM_INVISIBLE: i32 = 0x8000;

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
    /// `IAccPropServices`, created lazily (None = not yet / unavailable).
    static SERVICE: RefCell<Option<IAccPropServices>> = const { RefCell::new(None) };
    /// What has been annotated so far, per window and HWND.
    static APPLIED: RefCell<HashMap<WidgetId, HashMap<HKey, Props>>> = RefCell::new(HashMap::new());
}

fn service() -> Option<IAccPropServices> {
    if let Some(s) = existing_service() {
        return Some(s);
    }
    let svc = unsafe {
        CoCreateInstance::<_, IAccPropServices>(&CAccPropServices, None, CLSCTX_INPROC_SERVER)
    }
    .ok()?;
    SERVICE.with(|s| *s.borrow_mut() = Some(svc.clone()));
    Some(svc)
}
fn existing_service() -> Option<IAccPropServices> {
    SERVICE.with(|s| s.borrow().clone())
}

/// ROLE_SYSTEM_* for a role.
fn msaa_role(r: A11yRole) -> i32 {
    use A11yRole as R;
    (match r {
        R::Window => ROLE_SYSTEM_WINDOW,
        R::Pane => ROLE_SYSTEM_PANE,
        R::Group => ROLE_SYSTEM_GROUPING,
        R::Splitter => ROLE_SYSTEM_SEPARATOR,
        R::Table => ROLE_SYSTEM_TABLE,
        R::Link => ROLE_SYSTEM_LINK,
        R::ListBox => ROLE_SYSTEM_LIST,
        R::Tree => ROLE_SYSTEM_OUTLINE,
        R::TabPanel => ROLE_SYSTEM_PROPERTYPAGE,
        R::Image => ROLE_SYSTEM_GRAPHIC,
        R::Label => ROLE_SYSTEM_STATICTEXT,
        R::TextInput | R::PasswordInput | R::MultilineTextInput => ROLE_SYSTEM_TEXT,
        R::Button => ROLE_SYSTEM_PUSHBUTTON,
        R::CheckBox => ROLE_SYSTEM_CHECKBUTTON,
        R::RadioButton => ROLE_SYSTEM_RADIOBUTTON,
        R::ComboBox => ROLE_SYSTEM_COMBOBOX,
        R::ProgressBar => ROLE_SYSTEM_PROGRESSBAR,
        R::Slider => ROLE_SYSTEM_SLIDER,
        R::SpinButton => ROLE_SYSTEM_SPINBUTTON,
        R::TabList => ROLE_SYSTEM_PAGETABLIST,
        R::MenuBar => ROLE_SYSTEM_MENUBAR,
        R::Menu => ROLE_SYSTEM_MENUPOPUP,
        R::MenuItem | R::MenuItemCheckBox => ROLE_SYSTEM_MENUITEM,
    }) as i32
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
        if hwnd.is_invalid() {
            continue;
        }
        let custom = matches!(n.kind, Kind::Sash | Kind::Page | Kind::GroupBox);
        let mut p = Props::default();
        p.name = n.name.clone().filter(|_| {
            n.name_source == NameSource::Explicit || matches!(n.kind, Kind::Page | Kind::GroupBox)
        });
        p.desc = n.description.clone();
        p.role = (n.role_explicit || custom).then(|| msaa_role(n.role));
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
        if n.kind == Kind::SpinBox && !aux.is_invalid() && (p.name.is_some() || p.desc.is_some()) {
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
        if n.kind == Kind::GroupBox && !aux.is_invalid() {
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

const OBJECT_CLIENT: u32 = -4i32 as u32; // OBJID_CLIENT

fn set_str(svc: &IAccPropServices, hwnd: HWND, prop: GUID, v: &str) {
    unsafe {
        let _ = svc.SetHwndPropStr(hwnd, OBJECT_CLIENT, 0, prop, &hs(v));
    }
}
fn set_i4(svc: &IAccPropServices, hwnd: HWND, prop: GUID, v: i32) {
    unsafe {
        let mut var = VARIANT::default();
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = v;
        let _ = svc.SetHwndProp(hwnd, OBJECT_CLIENT, 0, prop, &var);
    }
}
fn clear(svc: &IAccPropServices, hwnd: HWND, prop: GUID) {
    unsafe {
        let _ = svc.ClearHwndProps(hwnd, OBJECT_CLIENT, 0, &[prop]);
    }
}
fn notify(event: u32, hwnd: HWND) {
    unsafe { NotifyWinEvent(event, hwnd, OBJECT_CLIENT as i32, 0) }
}

/// Bring one HWND's annotations from `old` to `new`; `raise` raises the matching WinEvents.
fn sync(svc: &IAccPropServices, hwnd: HWND, old: &Props, new: &Props, raise: bool) {
    fn text(
        svc: &IAccPropServices,
        hwnd: HWND,
        prop: GUID,
        ev: u32,
        old: &Option<String>,
        new: &Option<String>,
        raise: bool,
    ) {
        if old == new {
            return;
        }
        match new {
            Some(v) => set_str(svc, hwnd, prop, v),
            None => clear(svc, hwnd, prop),
        }
        if raise {
            notify(ev, hwnd);
        }
    }
    fn int(
        svc: &IAccPropServices,
        hwnd: HWND,
        prop: GUID,
        ev: u32,
        old: Option<i32>,
        new: Option<i32>,
        raise: bool,
    ) {
        if old == new {
            return;
        }
        match new {
            Some(v) => set_i4(svc, hwnd, prop, v),
            None => clear(svc, hwnd, prop),
        }
        if raise {
            notify(ev, hwnd);
        }
    }
    text(
        svc,
        hwnd,
        PROPID_ACC_NAME,
        EVENT_OBJECT_NAMECHANGE,
        &old.name,
        &new.name,
        raise,
    );
    text(
        svc,
        hwnd,
        PROPID_ACC_DESCRIPTION,
        EVENT_OBJECT_DESCRIPTIONCHANGE,
        &old.desc,
        &new.desc,
        raise,
    );
    text(
        svc,
        hwnd,
        PROPID_ACC_VALUE,
        EVENT_OBJECT_VALUECHANGE,
        &old.value,
        &new.value,
        raise,
    );
    int(svc, hwnd, PROPID_ACC_ROLE, 0, old.role, new.role, false);
    int(
        svc,
        hwnd,
        PROPID_ACC_STATE,
        EVENT_OBJECT_STATECHANGE,
        old.state,
        new.state,
        raise,
    );
}

/// Make the annotations of every native HWND in `window` match the core's current model.
pub(super) fn apply(window: WidgetId) {
    let want = desired(window);
    let Some(svc) = service() else { return };
    let mut want_by_hwnd: HashMap<HKey, Props> = HashMap::new();
    for (_, h, p) in want {
        want_by_hwnd.insert(h.into(), p);
    }
    let mut applied = APPLIED
        .with(|a| a.borrow_mut().remove(&window))
        .unwrap_or_default();
    let none = Props::default();
    for (h, old) in applied.iter() {
        if !want_by_hwnd.contains_key(h) {
            sync(&svc, h.hwnd(), old, &none, false);
        }
    }
    applied.retain(|h, _| want_by_hwnd.contains_key(h));
    for (h, new) in want_by_hwnd {
        let seen = applied.contains_key(&h);
        let old = applied.get(&h).cloned().unwrap_or_default();
        sync(&svc, h.hwnd(), &old, &new, seen);
        applied.insert(h, new);
    }
    APPLIED.with(|a| a.borrow_mut().insert(window, applied));
}

/// Remove the annotations of `hwnds` (call before the windows are destroyed).
pub(super) fn forget(window: WidgetId, hwnds: &[HWND]) {
    let svc = existing_service();
    APPLIED.with(|a| {
        let mut a = a.borrow_mut();
        let Some(m) = a.get_mut(&window) else { return };
        for h in hwnds {
            if let Some(old) = m.remove(&HKey::from(*h)) {
                if let Some(svc) = &svc {
                    sync(svc, *h, &old, &Props::default(), false);
                }
            }
        }
    });
}

/// Remove every annotation of `window` (call before the window is destroyed).
pub(super) fn forget_window(window: WidgetId) {
    let svc = existing_service();
    if let Some(m) = APPLIED.with(|a| a.borrow_mut().remove(&window)) {
        if let Some(svc) = &svc {
            for (h, old) in m {
                sync(svc, h.hwnd(), &old, &Props::default(), false);
            }
        }
    }
}

/// The HWND of a widget was replaced: move its annotations to the new one.
pub(super) fn rehome(window: WidgetId, old: HWND, new: HWND) {
    let svc = existing_service();
    APPLIED.with(|a| {
        let mut a = a.borrow_mut();
        let Some(m) = a.get_mut(&window) else { return };
        if let Some(p) = m.remove(&HKey::from(old)) {
            if let Some(svc) = &svc {
                sync(svc, old, &p, &Props::default(), false);
                sync(svc, new, &Props::default(), &p, false);
            }
            m.insert(new.into(), p);
        }
    });
}
