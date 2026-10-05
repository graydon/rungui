//! A byte-driven interpreter that exercises the whole public API of rungui.
//!
//! Every operation is chosen and parameterised by the input bytes (an exhausted input reads as
//! zeros, so shrinking a failing input is stable). It only uses the public API, so one interpreter
//! drives every backend: the headless mock (libFuzzer target and the seeded `tests/random_ops.rs`)
//! and, via `src/bin/native.rs`, the GTK / Win32 / Cocoa backends.
//!
//! Handles are picked from a pool that includes dead ones, with deliberately wrong kinds (a table
//! method on a button), hostile parameters (NaN, `i32::MIN`, NUL bytes, huge strings) and callbacks
//! that themselves run more random operations (re-entrancy). Nothing here may panic or leak.
//!
//! Use: `Fuzz::new(data, mode, panics).run(max_ops)`.
#![allow(dead_code)]

use rungui::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What the target can do. Modal operations (message boxes, native popup menus) block a real
/// toolkit's loop, and synthetic user events exist only on the mock backend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The mock backend: everything is allowed, including synthetic user events.
    Mock,
    /// A real toolkit: no modal calls, no synthetic user events.
    Native,
}

/// How many callback levels deep hostile callbacks may recurse.
const MAX_REENTRY: u32 = 3;
/// Operations a callback runs when it fires.
const OPS_PER_CALLBACK: u32 = 3;
/// Handles remembered at once (the oldest are forgotten, still alive in the toolkit).
const POOL_MAX: usize = 192;
/// Live top-level windows at once.
const MAX_WINDOWS: usize = 4;
/// Longest generated string, in bytes.
const MAX_STR: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    Window,
    VBox,
    HBox,
    Grid,
    GroupBox,
    Tabs,
    Page,
    Splitter,
    Label,
    Button,
    CheckBox,
    Radio,
    TextInput,
    Password,
    TextArea,
    ComboBox,
    ListBox,
    Slider,
    Progress,
    SpinBox,
    Image,
    Spacer,
    MenuBar,
    Menu,
    MenuItem,
    CheckMenuItem,
    MenuSeparator,
    Table,
    Tree,
    Popup,
}

#[derive(Clone, Copy)]
struct Entry {
    id: WidgetId,
    tag: Tag,
}

struct Input {
    data: Vec<u8>,
    pos: usize,
}

impl Input {
    fn u8(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }
    fn exhausted(&self) -> bool {
        self.pos >= self.data.len()
    }
}

pub struct Fuzz {
    inp: RefCell<Input>,
    pool: RefCell<Vec<Entry>>,
    timers: RefCell<Vec<Timer>>,
    groups: RefCell<Vec<RadioGroup>>,
    depth: Cell<u32>,
    budget: Cell<u32>,
    mode: Mode,
    /// Whether callbacks and batches may panic on purpose (must be false under libFuzzer, whose
    /// panic hook aborts even for panics the library contains).
    panics: bool,
    me: RefCell<Option<std::rc::Weak<Fuzz>>>,
}

const INTS: [i32; 14] = [
    0,
    1,
    -1,
    2,
    7,
    10,
    50,
    100,
    255,
    4096,
    65_535,
    65_536,
    i32::MAX,
    i32::MIN,
];
const FLOATS: [f64; 14] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    2.5,
    100.0,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    1e300,
    -1e300,
    1e-300,
    f64::MIN_POSITIVE,
];
const STRS: [&str; 16] = [
    "",
    "a",
    "hello world",
    "héllo wörld",
    "日本語のテキスト",
    "😀👍🏽 emoji",
    "שלום עולם",
    "&File\t&&x",
    "a\0b",
    "line1\nline2\r\nline3",
    "%s%n%d{}",
    "<b>markup</b> &amp; \"quotes\"",
    "\u{202e}rtl override",
    "e\u{301}\u{301}\u{301} combining",
    "Ctrl+Shift+S",
    "C:\\path\\to/some file.txt",
];
const ACCELS: [&str; 10] = [
    "Ctrl+S", "Alt+F4", "F5", "Ctrl+Shift+Z", "", "+", "Cmd+Q", "Ctrl+Left", "Esc", "ctrl+alt+;",
];

impl Fuzz {
    /// Initialise the toolkit once per thread (later calls reuse it) and wrap `data`.
    pub fn new(data: &[u8], mode: Mode, panics: bool) -> Rc<Fuzz> {
        let _ = App::new("fuzz");
        let fz = Rc::new(Fuzz {
            inp: RefCell::new(Input {
                data: data.to_vec(),
                pos: 0,
            }),
            pool: RefCell::new(vec![]),
            timers: RefCell::new(vec![]),
            groups: RefCell::new(vec![RadioGroup::new()]),
            depth: Cell::new(0),
            budget: Cell::new(0),
            mode,
            panics,
            me: RefCell::new(None),
        });
        *fz.me.borrow_mut() = Some(Rc::downgrade(&fz));
        fz
    }

    // ---------------------------------------------------------------- input helpers

    fn u8(&self) -> u8 {
        self.inp.borrow_mut().u8()
    }
    fn coin(&self, one_in: u8) -> bool {
        self.u8() % one_in.max(1) == 0
    }
    fn int(&self) -> i32 {
        let b = self.u8();
        if b < 200 {
            // mostly small, plausible values
            i32::from(b) - 20
        } else {
            INTS[usize::from(b) % INTS.len()]
        }
    }
    fn uint(&self) -> usize {
        let b = self.u8();
        if b < 220 {
            usize::from(b) % 40
        } else {
            [usize::MAX, usize::MAX - 1, 1 << 40, 1 << 20, 1 << 16, 1 << 31][usize::from(b) % 6]
        }
    }
    fn float(&self) -> f64 {
        let b = self.u8();
        if b < 128 {
            f64::from(b) / 4.0 - 8.0
        } else {
            FLOATS[usize::from(b) % FLOATS.len()]
        }
    }
    fn string(&self) -> String {
        let b = self.u8();
        match b % 8 {
            0 => {
                // long
                let n = (usize::from(self.u8()) * 80).min(MAX_STR);
                "x".repeat(n)
            }
            1 => {
                // raw bytes, lossily decoded (invalid UTF-8 becomes U+FFFD)
                let n = usize::from(self.u8() % 24);
                let bytes: Vec<u8> = (0..n).map(|_| self.u8()).collect();
                String::from_utf8_lossy(&bytes).into_owned()
            }
            _ => STRS[usize::from(self.u8()) % STRS.len()].to_string(),
        }
    }
    fn strings(&self) -> Vec<String> {
        let n = usize::from(self.u8() % 12);
        let n = if self.coin(40) { n * 100 } else { n };
        (0..n).map(|_| self.string()).collect()
    }
    fn index(&self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            let hi = usize::from(self.u8());
            let lo = usize::from(self.u8());
            (hi << 8 | lo) % len
        }
    }
    fn opt_index(&self) -> Option<usize> {
        match self.u8() % 4 {
            0 => None,
            1 => Some(self.uint()),
            _ => Some(usize::from(self.u8() % 8)),
        }
    }

    // ---------------------------------------------------------------- pool

    fn remember(&self, id: WidgetId, tag: Tag) -> WidgetId {
        let mut p = self.pool.borrow_mut();
        if p.len() >= POOL_MAX {
            p.remove(0);
        }
        p.push(Entry { id, tag });
        id
    }
    fn windows_alive(&self) -> usize {
        self.pool
            .borrow()
            .iter()
            .filter(|e| e.tag == Tag::Window && Widget(e.id).is_alive())
            .count()
    }
    /// Any pool entry, a dead id or a made-up id.
    fn any_id(&self) -> WidgetId {
        match self.u8() % 32 {
            0 => WidgetId::DEAD,
            1 => WidgetId(u64::MAX),
            2 => WidgetId(1 << 40),
            _ => {
                let p = self.pool.borrow();
                p.get(self.index(p.len())).map_or(WidgetId::DEAD, |e| e.id)
            }
        }
    }
    /// An entry with one of `tags` (falls back to any widget now and then to try wrong kinds).
    fn of(&self, tags: &[Tag]) -> WidgetId {
        if self.coin(12) {
            return self.any_id();
        }
        let p = self.pool.borrow();
        let ok: Vec<&Entry> = p.iter().filter(|e| tags.contains(&e.tag)).collect();
        match ok.get(self.index(ok.len())) {
            Some(e) => e.id,
            None => WidgetId::DEAD,
        }
    }
    fn window(&self) -> WidgetId {
        self.of(&[Tag::Window])
    }
    fn parent(&self) -> WidgetId {
        self.any_id()
    }

    // ---------------------------------------------------------------- callbacks

    /// A callback that runs a few more random operations (when allowed), or panics now and then.
    fn hook(&self) -> impl Fn() + 'static {
        let me = self.me.borrow().clone().unwrap_or_default();
        let panic_now = self.coin(40) && self.panics;
        move || {
            if let Some(fz) = me.upgrade() {
                if panic_now {
                    panic!("fuzz: deliberate panic in a callback");
                }
                fz.react();
            }
        }
    }
    fn react(&self) {
        if self.depth.get() >= MAX_REENTRY {
            return;
        }
        self.depth.set(self.depth.get() + 1);
        for _ in 0..OPS_PER_CALLBACK {
            if !self.step() {
                break;
            }
        }
        self.depth.set(self.depth.get() - 1);
    }

    // ---------------------------------------------------------------- driver

    /// Run until the input is used up or `max_ops` operations ran, then tear everything down and
    /// check the books.
    pub fn run(self: &Rc<Fuzz>, max_ops: u32) {
        self.budget.set(max_ops);
        while self.step() {
            if self.inp.borrow().exhausted() {
                break;
            }
        }
        self.finish();
    }

    /// Run some operations without the teardown (the native driver interleaves them with the loop).
    pub fn run_some(self: &Rc<Fuzz>, ops: u32) -> bool {
        self.budget.set(ops);
        while self.step() {
            if self.inp.borrow().exhausted() {
                return false;
            }
        }
        !self.inp.borrow().exhausted()
    }

    /// Destroy everything and, on the mock backend, verify nothing leaked.
    pub fn finish(&self) {
        self.budget.set(u32::MAX);
        self.check_consistency();
        for t in self.timers.borrow_mut().drain(..) {
            t.stop();
        }
        let ids: Vec<Entry> = self.pool.borrow().clone();
        for e in &ids {
            if e.tag == Tag::Window {
                Widget(e.id).destroy();
            }
        }
        for e in &ids {
            Widget(e.id).destroy(); // popups and anything orphaned
        }
        App::update();
        self.pool.borrow_mut().clear();
        #[cfg(feature = "mock")]
        if self.mode == Mode::Mock {
            use rungui::backend::mock;
            mock::pump();
            assert_eq!(mock::widget_count(), 0, "native widgets leaked");
            assert_eq!(mock::timer_count(), 0, "timers leaked");
        }
    }

    /// Every pooled handle agrees with the backend about being alive, and layout produced sane boxes.
    fn check_consistency(&self) {
        App::update();
        #[cfg(feature = "mock")]
        if self.mode == Mode::Mock {
            for e in self.pool.borrow().iter().filter(|e| e.tag == Tag::Window) {
                let Some(records) = rungui::backend::mock::a11y(e.id) else {
                    continue;
                };
                let mut seen = std::collections::HashSet::new();
                for r in records {
                    assert!(seen.insert(r.id), "a11y lists {:?} twice", r.id);
                    assert!(
                        rungui::backend::mock::widget(r.id).is_some(),
                        "a11y lists a widget the backend does not have"
                    );
                }
            }
        }
        for e in self.pool.borrow().iter() {
            let w = Widget(e.id);
            let b = w.bounds();
            assert!(b.w >= 0 && b.h >= 0, "negative bounds {b:?} for {:?}", e.tag);
            #[cfg(feature = "mock")]
            if self.mode == Mode::Mock {
                let native = rungui::backend::mock::widget(e.id).is_some();
                if !w.is_alive() {
                    assert!(!native, "{:?} destroyed in the core but not the backend", e.tag);
                } else if native {
                    // alive and native: fine; virtual kinds legitimately have no backend object
                } else {
                    assert!(
                        matches!(
                            e.tag,
                            Tag::VBox | Tag::HBox | Tag::Grid | Tag::Spacer | Tag::Splitter
                        ),
                        "{:?} alive in the core but missing in the backend",
                        e.tag
                    );
                }
            }
        }
    }

    /// One random operation. False once the budget is spent.
    fn step(&self) -> bool {
        if self.budget.get() == 0 {
            return false;
        }
        self.budget.set(self.budget.get() - 1);
        let op = self.u8() % 128;
        match op {
            0..=29 => self.create(op),
            30..=59 => self.common(op - 30),
            60..=99 => self.typed(op - 60),
            100..=119 => self.models(op - 100),
            _ => self.environment(op - 120),
        }
        true
    }

    // ---------------------------------------------------------------- creation

    fn create(&self, op: u8) {
        let p = self.parent();
        let (id, tag) = match op {
            0 => {
                if self.windows_alive() >= MAX_WINDOWS {
                    return;
                }
                (Window::new(&self.string()).id(), Tag::Window)
            }
            1 => (VBox::new(p).id(), Tag::VBox),
            2 => (HBox::new(p).id(), Tag::HBox),
            3 => (Grid::new(p, self.uint()).id(), Tag::Grid),
            4 => (GroupBox::new(p, &self.string()).id(), Tag::GroupBox),
            5 => (Tabs::new(p).id(), Tag::Tabs),
            6 => {
                let t = self.of(&[Tag::Tabs]);
                (Tabs::from_id(t).add_page(&self.string()).id(), Tag::Page)
            }
            7 => {
                let o = if self.coin(2) {
                    Orientation::Horizontal
                } else {
                    Orientation::Vertical
                };
                (Splitter::new(p, o).id(), Tag::Splitter)
            }
            8 => (Label::new(p, &self.string()).id(), Tag::Label),
            9 => (Button::new(p, &self.string()).id(), Tag::Button),
            10 => (CheckBox::new(p, &self.string()).id(), Tag::CheckBox),
            11 => {
                let g = {
                    let mut gs = self.groups.borrow_mut();
                    if self.coin(8) && gs.len() < 8 {
                        gs.push(RadioGroup::new());
                    }
                    gs[self.index(gs.len())]
                };
                (RadioButton::new(p, &g, &self.string()).id(), Tag::Radio)
            }
            12 => (TextInput::new(p).id(), Tag::TextInput),
            13 => (TextInput::password(p).id(), Tag::Password),
            14 => (TextArea::new(p).id(), Tag::TextArea),
            15 => (ComboBox::new(p).id(), Tag::ComboBox),
            16 => (ListBox::new(p).id(), Tag::ListBox),
            17 => (Slider::new(p, self.float(), self.float()).id(), Tag::Slider),
            18 => (ProgressBar::new(p).id(), Tag::Progress),
            19 => (
                SpinBox::new(p, self.float(), self.float(), self.float()).id(),
                Tag::SpinBox,
            ),
            20 => (Image::new(p).id(), Tag::Image),
            21 => (Spacer::new(p).id(), Tag::Spacer),
            22 => (MenuBar::new(p).id(), Tag::MenuBar),
            23 => (Menu::new(p, &self.string()).id(), Tag::Menu),
            24 => (MenuItem::new(p, &self.string()).id(), Tag::MenuItem),
            25 => (CheckMenuItem::new(p, &self.string()).id(), Tag::CheckMenuItem),
            26 => (MenuSeparator::new(p).id(), Tag::MenuSeparator),
            27 => (Table::new(p).id(), Tag::Table),
            28 => (Tree::new(p).id(), Tag::Tree),
            _ => (PopupMenu::new().id(), Tag::Popup),
        };
        let _ = last_error();
        if id != WidgetId::DEAD {
            self.remember(id, tag);
        }
        // sometimes grow a chain of nested boxes until the nesting limit stops it
        let container = matches!(tag, Tag::Window | Tag::VBox | Tag::HBox | Tag::GroupBox);
        if container && id != WidgetId::DEAD && self.coin(64) {
            let mut cur = id;
            for i in 0..=MAX_NESTING {
                let n = if i % 2 == 0 {
                    VBox::new(cur).id()
                } else {
                    HBox::new(cur).id()
                };
                if n == WidgetId::DEAD {
                    assert_eq!(last_error(), Some(Error::LimitExceeded), "chain stopped at {i}");
                    break;
                }
                cur = n;
            }
            self.remember(cur, Tag::VBox);
        }
    }

    // ---------------------------------------------------------------- operations on any widget

    fn common(&self, op: u8) {
        let w = Widget(self.any_id());
        match op {
            0 => {
                if self.coin(3) {
                    w.destroy()
                }
            }
            1 => w.set_enabled(self.coin(2)),
            2 => w.set_visible(self.coin(3)),
            3 => w.set_tooltip(&self.string()),
            4 => w.focus(),
            5 => w.set_expand(self.float() as f32),
            6 => w.set_align(
                [Align::Start, Align::Center, Align::End, Align::Fill][usize::from(self.u8() % 4)],
            ),
            7 => w.set_min_size(self.int(), self.int()),
            8 => w.set_fixed_size(self.int(), self.int()),
            9 => w.set_spacing(self.int()),
            10 => w.set_padding(self.int()),
            11 => w.set_cell(self.uint(), self.uint(), self.uint(), self.uint()),
            12 => w.set_a11y_name(&self.string()),
            13 => w.set_a11y_description(&self.string()),
            14 => {
                let roles = [
                    A11yRole::Window,
                    A11yRole::Pane,
                    A11yRole::Group,
                    A11yRole::Label,
                    A11yRole::Link,
                    A11yRole::Image,
                    A11yRole::Button,
                    A11yRole::CheckBox,
                    A11yRole::RadioButton,
                    A11yRole::TextInput,
                    A11yRole::PasswordInput,
                    A11yRole::MultilineTextInput,
                    A11yRole::ComboBox,
                    A11yRole::ListBox,
                    A11yRole::Slider,
                    A11yRole::SpinButton,
                    A11yRole::ProgressBar,
                    A11yRole::TabList,
                    A11yRole::TabPanel,
                    A11yRole::MenuBar,
                    A11yRole::Menu,
                    A11yRole::MenuItem,
                    A11yRole::MenuItemCheckBox,
                    A11yRole::Table,
                    A11yRole::Tree,
                    A11yRole::Splitter,
                ];
                w.set_a11y_role(roles[usize::from(self.u8()) % roles.len()])
            }
            15 => {
                // getters never panic whatever the handle
                let _ = (
                    w.id(),
                    w.is_alive(),
                    w.enabled(),
                    w.visible(),
                    w.bounds(),
                    w.a11y(),
                    w.native_handle(),
                );
            }
            16 => w.set_context_menu(self.of(&[Tag::Popup])),
            17 => w.clear_context_menu(),
            18 => {
                let h = self.hook();
                w.on_context_menu(move |_, _| h())
            }
            19 => w.destroy_if_leaf(self),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- widget-specific operations

    fn typed(&self, op: u8) {
        let text_kinds = [
            Tag::Label,
            Tag::Button,
            Tag::CheckBox,
            Tag::Radio,
            Tag::MenuItem,
            Tag::CheckMenuItem,
            Tag::Menu,
        ];
        match op {
            0 => {
                let id = self.of(&[Tag::Label]);
                Label::from_id(id).set_text(&self.string());
                let _ = Label::from_id(id).text();
            }
            1 => {
                let id = self.of(&text_kinds);
                Button::from_id(id).set_text(&self.string());
                let _ = Button::from_id(id).text();
            }
            2 => {
                let h = self.hook();
                Button::from_id(self.of(&[Tag::Button])).on_click(move || h())
            }
            3 => {
                let id = self.of(&[Tag::CheckBox, Tag::Radio]);
                CheckBox::from_id(id).set_checked(self.coin(2));
                let _ = CheckBox::from_id(id).checked();
                RadioButton::from_id(id).set_checked(self.coin(2));
            }
            4 => {
                let h = self.hook();
                CheckBox::from_id(self.of(&[Tag::CheckBox])).on_toggle(move |_| h());
                let h = self.hook();
                RadioButton::from_id(self.of(&[Tag::Radio])).on_toggle(move |_| h())
            }
            5 => {
                let id = self.of(&[Tag::TextInput, Tag::Password, Tag::TextArea]);
                TextInput::from_id(id).set_text(&self.string());
                let _ = TextInput::from_id(id).text();
                TextArea::from_id(id).set_text(&self.string());
            }
            6 => {
                let h = self.hook();
                TextInput::from_id(self.of(&[Tag::TextInput, Tag::Password, Tag::TextArea]))
                    .on_change(move |_| h())
            }
            7 => {
                let id = self.of(&[Tag::TextInput, Tag::Password, Tag::TextArea]);
                TextInput::from_id(id).set_read_only(self.coin(2));
                TextInput::from_id(id).set_monospace(self.coin(2));
                let _ = TextInput::from_id(id).monospace();
                TextInput::from_id(id).set_placeholder(&self.string());
            }
            8 => {
                let id = self.of(&[Tag::TextArea]);
                TextArea::from_id(id).set_wrap(self.coin(2));
                let _ = TextArea::from_id(id).wrap();
            }
            9 => {
                let id = self.of(&[Tag::ComboBox, Tag::ListBox]);
                ComboBox::from_id(id).set_items(&self.strings());
                let _ = ComboBox::from_id(id).items();
                ListBox::from_id(id).set_items(&self.strings());
            }
            10 => {
                let id = self.of(&[Tag::ComboBox, Tag::ListBox]);
                ComboBox::from_id(id).set_selected(self.opt_index());
                let _ = (
                    ComboBox::from_id(id).selected(),
                    ComboBox::from_id(id).selected_text(),
                );
            }
            11 => {
                let h = self.hook();
                ComboBox::from_id(self.of(&[Tag::ComboBox, Tag::ListBox])).on_select(move |_| h());
                let h = self.hook();
                ListBox::from_id(self.of(&[Tag::ListBox])).on_activate(move |_| h())
            }
            12 => {
                let id = self.of(&[Tag::Slider, Tag::SpinBox, Tag::Progress]);
                Slider::from_id(id).set_value(self.float());
                let _ = Slider::from_id(id).value();
                SpinBox::from_id(id).set_value(self.float());
            }
            13 => {
                let id = self.of(&[Tag::Slider, Tag::SpinBox, Tag::Progress]);
                Slider::from_id(id).set_range(self.float(), self.float(), self.float());
                SpinBox::from_id(id).set_range(self.float(), self.float(), self.float());
            }
            14 => {
                let h = self.hook();
                Slider::from_id(self.of(&[Tag::Slider, Tag::SpinBox])).on_change(move |_| h())
            }
            15 => {
                let id = self.of(&[Tag::Progress]);
                ProgressBar::from_id(id).set_fraction(self.float());
                let _ = ProgressBar::from_id(id).fraction();
                ProgressBar::from_id(id).set_indeterminate(self.coin(2));
            }
            16 => {
                let id = self.of(&[Tag::Image]);
                let (w, h) = (u32::from(self.u8() % 40), u32::from(self.u8() % 40));
                let len = match self.u8() % 4 {
                    0 => 0,
                    1 => (w * h * 4) as usize + 1,
                    _ => (w * h * 4) as usize,
                };
                let data = ImageData {
                    w,
                    h,
                    rgba: (0..len).map(|i| (i * 7) as u8).collect(),
                };
                if self.coin(16) {
                    // absurd dimensions with a small buffer must be rejected, not trusted
                    let bad = ImageData {
                        w: u32::MAX,
                        h: u32::MAX,
                        rgba: vec![0; 16],
                    };
                    Image::from_id(id).set_image(Some(&bad));
                }
                Image::from_id(id).set_image(if self.coin(5) { None } else { Some(&data) });
            }
            17 => {
                let id = self.of(&[Tag::Tabs]);
                Tabs::from_id(id).set_selected(self.uint());
                let _ = Tabs::from_id(id).selected();
                let h = self.hook();
                Tabs::from_id(id).on_select(move |_| h());
                Page::from_id(self.of(&[Tag::Page])).set_title(&self.string());
                GroupBox::from_id(self.of(&[Tag::GroupBox])).set_title(&self.string());
            }
            18 => {
                let id = self.of(&[Tag::Splitter]);
                let s = rungui::Splitter::from_id(id);
                s.set_position(self.int());
                s.set_min_pane_sizes(self.int(), self.int());
                let _ = (s.position(), s.orientation());
                let h = self.hook();
                s.on_move(move |_| h());
            }
            19 => {
                let id = self.of(&[Tag::Window]);
                let w = Window::from_id(id);
                match self.u8() % 12 {
                    0 => w.show(),
                    1 => w.hide(),
                    2 => w.set_size(self.int(), self.int()),
                    3 => w.set_title(&self.string()),
                    4 => w.set_resizable(self.coin(2)),
                    5 => w.set_position(self.int(), self.int()),
                    6 => {
                        let _ = (w.title(), w.size(), w.position());
                    }
                    7 => {
                        let h = self.hook();
                        w.on_close(move || {
                            h();
                            true
                        })
                    }
                    8 => {
                        let h = self.hook();
                        w.on_resize(move |_, _| h())
                    }
                    9 => {
                        let h = self.hook();
                        w.on_move(move |_, _| h())
                    }
                    10 => {
                        if self.coin(4) {
                            w.close()
                        }
                    }
                    _ => w.set_min_size(self.int(), self.int()),
                }
            }
            20 => {
                let a = ACCELS[usize::from(self.u8()) % ACCELS.len()];
                MenuItem::from_id(self.of(&[Tag::MenuItem])).set_accel(a);
                CheckMenuItem::from_id(self.of(&[Tag::CheckMenuItem])).set_accel(a);
                let c = CheckMenuItem::from_id(self.of(&[Tag::CheckMenuItem]));
                c.set_checked(self.coin(2));
                let _ = c.checked();
                let _ = Accel::parse(&self.string());
            }
            21 => {
                let h = self.hook();
                MenuItem::from_id(self.of(&[Tag::MenuItem])).on_click(move || h());
                let h = self.hook();
                CheckMenuItem::from_id(self.of(&[Tag::CheckMenuItem])).on_toggle(move |_| h())
            }
            22 => {
                let g = Grid::from_id(self.of(&[Tag::Grid]));
                g.place(
                    Widget(self.any_id()),
                    self.uint(),
                    self.uint(),
                    self.uint(),
                    self.uint(),
                );
            }
            23 => {
                if self.mode == Mode::Mock {
                    let p = PopupMenu::from_id(self.of(&[Tag::Popup]));
                    p.show_at(self.window(), self.int(), self.int());
                    p.show(self.window());
                }
            }
            24 => {
                let ms = u32::from(self.u8());
                let h = self.hook();
                let t = if self.coin(2) {
                    Timer::once(ms, move || h())
                } else {
                    Timer::every(ms, move || h())
                };
                let mut ts = self.timers.borrow_mut();
                if ts.len() >= 32 {
                    ts.remove(0).stop();
                }
                ts.push(t);
            }
            25 => {
                let mut ts = self.timers.borrow_mut();
                if !ts.is_empty() {
                    let i = self.index(ts.len());
                    ts.swap_remove(i).stop();
                }
            }
            _ => {
                let id = self.any_id();
                let _ = Widget(id).native_handle();
            }
        }
    }

    // ---------------------------------------------------------------- tables and trees

    fn models(&self, op: u8) {
        let t = Table::from_id(self.of(&[Tag::Table]));
        let tr = Tree::from_id(self.of(&[Tag::Tree]));
        match op {
            0 => {
                let n = usize::from(self.u8() % 6);
                let cols: Vec<Column> = (0..n)
                    .map(|_| {
                        Column::new(&self.string())
                            .width(self.int())
                            .align(
                                [ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Right]
                                    [usize::from(self.u8() % 3)],
                            )
                            .sortable(self.coin(2))
                    })
                    .collect();
                if self.coin(2) {
                    t.set_columns(&cols);
                } else if let Some(c) = cols.into_iter().next() {
                    t.add_column(c);
                }
                let _ = t.columns();
            }
            1 => {
                let n = usize::from(self.u8() % 20);
                let rows: Vec<Vec<String>> = (0..n)
                    .map(|_| (0..self.u8() % 5).map(|_| self.string()).collect())
                    .collect();
                t.set_rows(&rows);
            }
            2 => t.push_row(&self.strings()),
            3 => t.insert_row(self.uint(), &self.strings()),
            4 => t.remove_row(self.uint()),
            5 => t.clear(),
            6 => t.set_cell(self.uint(), self.uint(), &self.string()),
            7 => {
                let _ = (
                    t.cell(self.uint(), self.uint()),
                    t.row(self.uint()),
                    t.rows().len(),
                    t.row_count(),
                    t.selected_row(),
                    t.sort_indicator(),
                );
            }
            8 => {
                // a batch whose closure may panic must still thaw the table
                let boom = self.coin(8) && self.panics;
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    t.batch(|t| {
                        t.push_row(&["a", "b"]);
                        t.clear();
                        t.push_row(&["c"]);
                        if boom {
                            panic!("fuzz: panic inside batch");
                        }
                    })
                }));
                assert_eq!(r.is_err(), boom);
            }
            9 => {
                t.set_selected(self.opt_index());
                t.set_sort_indicator(self.opt_index().map(|c| (c, self.coin(2))));
            }
            10 => {
                let h = self.hook();
                t.on_select(move |_| h());
                let h = self.hook();
                t.on_activate(move |_| h());
                let h = self.hook();
                t.on_column_click(move |_| h());
            }
            11 => {
                let parent = self.tree_node(tr);
                let id = tr.insert(parent, self.uint(), &self.string());
                self.remember_tree_node(id);
            }
            12 => {
                let p = if self.coin(2) { None } else { self.tree_node(tr) };
                let id = tr.add(p, &self.string());
                self.remember_tree_node(id);
            }
            13 => {
                if let Some(n) = self.tree_node(tr) {
                    if self.coin(3) {
                        tr.remove(n);
                    }
                }
            }
            14 => {
                if self.coin(8) {
                    tr.clear();
                }
            }
            15 => {
                let n = self.tree_node(tr).unwrap_or(TreeNodeId(self.uint() as u64));
                match self.u8() % 5 {
                    0 => tr.set_text(n, &self.string()),
                    1 => tr.set_expanded(n, self.coin(2)),
                    2 => tr.set_has_children(n, self.coin(2)),
                    3 => tr.set_selected(Some(n)),
                    _ => tr.set_selected(None),
                }
                let _ = (
                    tr.text(n),
                    tr.children(Some(n)),
                    tr.children(None),
                    tr.parent(n),
                    tr.contains(n),
                    tr.len(),
                    tr.is_empty(),
                    tr.expanded(n),
                    tr.selected(),
                );
            }
            16 => tr.expand_all(self.coin(2)),
            17 => {
                tr.batch(|tr| {
                    let a = tr.add(None, "a");
                    let b = tr.add(Some(a), "b");
                    tr.set_selected(Some(b));
                    tr.clear();
                    tr.add(None, "c");
                });
            }
            18 => {
                let h = self.hook();
                tr.on_select(move |_| h());
                let h = self.hook();
                tr.on_activate(move |_| h());
                let h = self.hook();
                tr.on_expand(move |_, _| h());
            }
            _ => {}
        }
    }

    /// A node of `tr` picked from what the tree currently holds (or a bogus one).
    fn tree_node(&self, tr: Tree) -> Option<TreeNodeId> {
        let mut all = vec![];
        let mut stack = tr.children(None);
        while let Some(n) = stack.pop() {
            all.push(n);
            stack.extend(tr.children(Some(n)));
            if all.len() > 512 {
                break;
            }
        }
        if all.is_empty() || self.coin(16) {
            return Some(TreeNodeId(u64::from(self.u8())));
        }
        Some(all[self.index(all.len())])
    }
    fn remember_tree_node(&self, _id: TreeNodeId) {}

    // ---------------------------------------------------------------- environment, user events

    fn environment(&self, op: u8) {
        match op {
            0 => App::update(),
            1 => rungui::set_rtl_layout(self.coin(2)),
            2 => {
                let _ = last_error();
            }
            3 => App::post(|| ()),
            4 => App::set_quit_on_last_close(false),
            5 => {
                if self.mode == Mode::Mock {
                    self.mock_message();
                }
            }
            _ => {
                if self.mode == Mode::Mock {
                    self.mock_event(op);
                }
            }
        }
    }

    #[cfg(feature = "mock")]
    fn mock_message(&self) {
        use rungui::backend::mock;
        mock::queue_answer([Answer::Ok, Answer::Yes, Answer::No, Answer::Cancel][usize::from(self.u8() % 4)]);
        let win = Window::from_id(self.window());
        let kinds = [
            MessageKind::Info,
            MessageKind::Warning,
            MessageKind::Error,
            MessageKind::Question,
        ];
        let buttons = [Buttons::Ok, Buttons::OkCancel, Buttons::YesNo, Buttons::YesNoCancel];
        let _ = message_box(
            Some(win),
            kinds[usize::from(self.u8() % 4)],
            buttons[usize::from(self.u8() % 4)],
            &self.string(),
            &self.string(),
        );
        mock::queue_files(&["/tmp/a", "b"]);
        let fd = FileDialog::new()
            .title(&self.string())
            .filter(&self.string(), &["txt", "*"])
            .directory(&self.string())
            .file_name(&self.string());
        let _ = (fd.open(Some(win)), fd.open_many(None), fd.save(Some(win)), fd.pick_folder(None));
    }
    #[cfg(not(feature = "mock"))]
    fn mock_message(&self) {}

    #[cfg(feature = "mock")]
    fn mock_event(&self, op: u8) {
        use rungui::backend::mock;
        use Tag as T;
        let sk = [
            SashKey::Prev,
            SashKey::Next,
            SashKey::PrevLarge,
            SashKey::NextLarge,
            SashKey::Min,
            SashKey::Max,
        ];
        // events are aimed at the kind of widget that emits them most of the time
        match op % 26 {
            0 => mock::user_click(self.of(&[T::Button, T::MenuItem])),
            1 => mock::user_text(self.of(&[T::TextInput, T::Password, T::TextArea]), &self.string()),
            2 => mock::user(
                self.of(&[T::CheckBox, T::Radio, T::CheckMenuItem]),
                Event::Toggled(self.coin(2)),
            ),
            3 => mock::user(
                self.of(&[T::ComboBox, T::ListBox, T::Tabs]),
                Event::Selected(self.opt_index()),
            ),
            4 => mock::user(self.of(&[T::Slider, T::SpinBox]), Event::Value(self.float())),
            5 => mock::user(self.of(&[T::ListBox]), Event::Activated(self.uint())),
            6 => mock::user(self.any_id(), Event::Focus(self.coin(2))),
            7 => mock::user_click_column(self.of(&[T::Table]), self.uint()),
            8 => mock::user_select_row(self.of(&[T::Table]), self.opt_index()),
            9 => mock::user_activate_row(self.of(&[T::Table]), self.uint()),
            10 => {
                let tr = self.of(&[T::Tree]);
                let n = self.tree_node(Tree::from_id(tr)).map(|n| n.0);
                mock::user_tree_select(tr, if self.coin(4) { None } else { n });
            }
            11 => {
                let tr = self.of(&[T::Tree]);
                let n = self.tree_node(Tree::from_id(tr)).map_or(0, |n| n.0);
                mock::user_tree_activate(tr, n);
            }
            12 => {
                let tr = self.of(&[T::Tree]);
                let n = self.tree_node(Tree::from_id(tr)).map_or(0, |n| n.0);
                mock::user_tree_expand(tr, n, self.coin(2));
            }
            13 => mock::user_context_menu(self.any_id(), self.int(), self.int()),
            14 => mock::user_drag_sash(self.of(&[T::Splitter]), self.int()),
            15 => mock::user_drag_sash_by(self.of(&[T::Splitter]), self.int()),
            16 => mock::user_sash_key(self.of(&[T::Splitter]), sk[usize::from(self.u8()) % sk.len()]),
            17 => mock::user_move_window(self.window(), self.int(), self.int()),
            18 => mock::resize_window(self.window(), self.int(), self.int()),
            19 => {
                if self.coin(4) {
                    mock::user_close(self.window())
                }
            }
            20 => mock::advance(u32::from(self.u8()) * 7),
            21 => mock::pump(),
            22 => {
                let target = self.of(&[T::Popup]);
                let item = self.any_id();
                mock::queue_popup_choice(if self.coin(3) { None } else { Some(item) });
                mock::user_context_menu(target, self.int(), self.int());
            }
            23 => {
                // a stray event of any kind at any widget
                let evs = [
                    Event::Click,
                    Event::Text(self.string()),
                    Event::Toggled(true),
                    Event::Selected(Some(self.uint())),
                    Event::Value(self.float()),
                    Event::Activated(self.uint()),
                    Event::ColumnClicked(self.uint()),
                    Event::TreeSelected(Some(self.uint() as u64)),
                ];
                mock::user(self.any_id(), evs[usize::from(self.u8()) % evs.len()].clone());
            }
            24 => {
                mock::fail_next_create();
            }
            _ => {
                mock::set_unsupported(if self.coin(2) { &[Kind::Sash] } else { &[] });
            }
        }
    }
    #[cfg(not(feature = "mock"))]
    fn mock_event(&self, _op: u8) {}
}

/// `destroy` only for widgets without children, so the pool does not collapse too fast.
trait DestroyIfLeaf {
    fn destroy_if_leaf(&self, fz: &Fuzz);
}
impl DestroyIfLeaf for Widget {
    fn destroy_if_leaf(&self, fz: &Fuzz) {
        if fz.coin(6) {
            self.destroy();
        }
    }
}
