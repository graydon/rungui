//! Accessibility metadata. rungui wraps native controls, and every platform already exposes those
//! to assistive technology (ATK on GTK, NSAccessibility on Cocoa, oleacc/MSAA on Win32). What the
//! native controls cannot know is what *the app* wants them called, so the core works out, for
//! every native widget of a window, an accessible name, description and role ([`resolve`]) and each
//! backend's `a11y_changed` copies the parts that matter onto the native objects.
//!
//! Naming rules: buttons, check boxes, radio buttons, group boxes, pages, windows and menu items
//! take their text (mnemonic markers like `&File` stripped); an input, list, slider, table or tree
//! without an explicit name is labelled by the nearest preceding Label in layout order (so simple
//! forms are accessible for free). The description is the explicit one or else the tooltip.
//! Every widget supports overrides: `set_a11y_name`, `set_a11y_description`, `set_a11y_role`.
//! Hidden subtrees and non-selected tab pages are omitted, and virtual layout boxes are flattened
//! away (they have no native object).

use crate::backend::Kind;
use crate::core::{self, Registry};
use crate::text::strip_mnemonic;
use crate::types::*;

/// What a widget is, for assistive technology. Only roles that every backend can express are
/// offered; each widget kind has a default (a `Button` is a [`Button`](A11yRole::Button)) and
/// [`crate::Widget::set_a11y_role`] overrides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum A11yRole {
    Window,
    /// A generic container (also the role of anything unrecognised).
    Pane,
    Group,
    Label,
    Link,
    Image,
    Button,
    CheckBox,
    RadioButton,
    TextInput,
    PasswordInput,
    MultilineTextInput,
    ComboBox,
    ListBox,
    Slider,
    SpinButton,
    ProgressBar,
    TabList,
    TabPanel,
    MenuBar,
    Menu,
    MenuItem,
    MenuItemCheckBox,
    Table,
    Tree,
    Splitter,
}

/// Per-widget accessibility overrides (`None` = derive from the widget).
#[derive(Clone, Debug, Default)]
pub struct A11yProps {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub role: Option<A11yRole>,
}

pub(crate) fn default_role(k: Kind) -> A11yRole {
    use A11yRole as R;
    match k {
        Kind::Window => R::Window,
        Kind::Label => R::Label,
        Kind::Button => R::Button,
        Kind::CheckBox => R::CheckBox,
        Kind::RadioButton => R::RadioButton,
        Kind::TextInput => R::TextInput,
        Kind::PasswordInput => R::PasswordInput,
        Kind::TextArea => R::MultilineTextInput,
        Kind::ComboBox => R::ComboBox,
        Kind::ListBox => R::ListBox,
        Kind::Slider => R::Slider,
        Kind::ProgressBar => R::ProgressBar,
        Kind::SpinBox => R::SpinButton,
        Kind::Tabs => R::TabList,
        Kind::Page => R::TabPanel,
        Kind::GroupBox => R::Group,
        Kind::Image => R::Image,
        Kind::MenuBar => R::MenuBar,
        Kind::Menu | Kind::PopupMenu => R::Menu,
        Kind::MenuItem => R::MenuItem,
        Kind::CheckMenuItem => R::MenuItemCheckBox,
        Kind::Table => R::Table,
        Kind::Tree => R::Tree,
        Kind::Sash => R::Splitter,
        _ => R::Pane,
    }
}

/// Where a resolved name came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameSource {
    /// `set_a11y_name`.
    Explicit,
    /// The widget's own text (a button's caption, a group box's title...). Native controls of
    /// these kinds already report it themselves.
    Text,
    /// The preceding Label in layout order.
    PrecedingLabel,
}

/// The accessible metadata of one native widget. Each backend reads the subset it needs.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct Resolved {
    pub id: WidgetId,
    pub kind: Kind,
    pub name: Option<String>,
    pub name_source: NameSource,
    pub description: Option<String>,
    /// The override if the app set one, else the kind's default.
    pub role: A11yRole,
    pub role_explicit: bool,
}

struct Ctx<'a> {
    r: &'a Registry,
    out: Vec<Resolved>,
    label: Option<String>,
}

fn named_by_text(k: Kind) -> bool {
    matches!(
        k,
        Kind::Button
            | Kind::CheckBox
            | Kind::RadioButton
            | Kind::GroupBox
            | Kind::Page
            | Kind::Window
            | Kind::Menu
            | Kind::MenuItem
            | Kind::CheckMenuItem
    )
}

fn named_by_label(k: Kind) -> bool {
    matches!(
        k,
        Kind::TextInput
            | Kind::PasswordInput
            | Kind::TextArea
            | Kind::ComboBox
            | Kind::ListBox
            | Kind::Slider
            | Kind::SpinBox
            | Kind::ProgressBar
            | Kind::Table
            | Kind::Tree
    )
}

fn walk(cx: &mut Ctx, id: WidgetId) {
    let Some(n) = cx.r.nodes.get(&id) else { return };
    if (!n.visible && n.kind != Kind::Window) || n.kind == Kind::MenuSeparator {
        return;
    }
    if !n.kind.is_native() {
        for c in &n.children {
            walk(cx, *c);
        }
        return;
    }
    let k = n.kind;
    let (name, name_source) = match &n.a11y.name {
        Some(s) => (Some(s.clone()), NameSource::Explicit),
        None if named_by_text(k) && !n.text.is_empty() => {
            (Some(strip_mnemonic(&n.text)), NameSource::Text)
        }
        None if named_by_label(k) => (cx.label.clone(), NameSource::PrecedingLabel),
        None => (None, NameSource::Text),
    };
    let description = n
        .a11y
        .desc
        .clone()
        .or_else(|| (!n.tooltip.is_empty()).then(|| n.tooltip.clone()));
    cx.out.push(Resolved {
        id,
        kind: k,
        name,
        name_source,
        description,
        role: n.a11y.role.unwrap_or_else(|| default_role(k)),
        role_explicit: n.a11y.role.is_some(),
    });
    if k == Kind::Label {
        cx.label = Some(strip_mnemonic(&n.text));
    }
    if k == Kind::Tabs {
        // only the selected page is visible to AT
        if let Some(c) = n.selected.and_then(|i| n.children.get(i)) {
            walk(cx, *c);
        }
    } else {
        for c in &n.children {
            walk(cx, *c);
        }
    }
}

/// Metadata for every native widget of `window` that assistive technology can currently see, in
/// layout order. `None` if the id is not a live window.
pub(crate) fn resolve(window: WidgetId) -> Option<Vec<Resolved>> {
    core::with(|r| {
        if r.nodes.get(&window)?.kind != Kind::Window {
            return None;
        }
        let mut cx = Ctx {
            r,
            out: vec![],
            label: None,
        };
        walk(&mut cx, window);
        Some(cx.out)
    })
    .flatten()
}
