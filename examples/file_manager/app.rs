//! The file manager UI: a dual-pane browser built only from rungui's public API.
//!
//! Layout (every `Splitter` is draggable where the backend has a sash):
//!
//! ```text
//! menu bar
//! toolbar: Up | Refresh | path box | Copy -> | Move ->
//! +--------+--------------------------------------+
//! | tree   |  pane A table   |   pane B table     |
//! | (lazy) +--------------------------------------+
//! |        |  preview: image + monospace text     |
//! +--------+--------------------------------------+
//! status bar
//! ```
//!
//! Every handle is `Copy`, so the UI is a plain struct of handles; the mutable model (paths,
//! listings) lives in a `RefCell` that is never borrowed across a call into rungui, because
//! those calls can re-enter our callbacks.

use crate::fsmodel::{self as fsm, Entry, Preview, SortKey};
use rungui::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::SystemTime;

/// `RUNGUI_FM_TRACE=1`: print machine-readable lines (NAV, SELECT, PREVIEW, BOUNDS, ...) on stdout
/// so scripts/smoke-filemanager.sh and the tests can follow what the app did.
pub fn tracing() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| {
        std::env::var_os("RUNGUI_FM_TRACE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}
macro_rules! trace {
    ($($a:tt)*) => { if tracing() { println!($($a)*) } };
}

const PANE_NAMES: [&str; 2] = ["A", "B"];

pub struct Options {
    pub dirs: [PathBuf; 2],
}

/// All widgets. Public so the mock-backend test can drive them.
#[allow(dead_code)]
pub struct Ui {
    pub win: Window,
    pub body: Splitter,
    pub right: Splitter,
    pub panes: Splitter,
    pub tree: Tree,
    pub tables: [Table; 2],
    pub pane_labels: [Label; 2],
    pub path: TextInput,
    pub up: Button,
    pub refresh: Button,
    pub copy_btn: Button,
    pub move_btn: Button,
    pub preview_title: Label,
    pub preview: TextArea,
    pub image: Image,
    pub status: Label,
    pub tag: Label,
    pub popup: PopupMenu,
    pub items: Items,
}

#[derive(Copy, Clone)]
pub struct Items {
    pub new_folder: MenuItem,
    pub rename: MenuItem,
    pub copy: MenuItem,
    pub mv: MenuItem,
    pub delete: MenuItem,
    pub quit: MenuItem,
    pub hidden: CheckMenuItem,
    pub refresh: MenuItem,
    pub up: MenuItem,
    pub home: MenuItem,
    pub switch: MenuItem,
    pub about: MenuItem,
    pub ctx_open: MenuItem,
    pub ctx_rename: MenuItem,
    pub ctx_delete: MenuItem,
    pub ctx_copy: MenuItem,
    pub ctx_new: MenuItem,
}

struct Pane {
    path: PathBuf,
    /// Row `i` of the table is `entries[i]`; a leading ".." row if the directory has a parent.
    entries: Vec<Entry>,
    sort: (SortKey, bool),
    mtime: Option<SystemTime>,
    truncated: bool,
}

#[derive(Default)]
struct State {
    panes: Vec<Pane>,
    active: usize,
    show_hidden: bool,
    node_paths: HashMap<TreeNodeId, PathBuf>,
    loaded: HashSet<TreeNodeId>,
    dialog: Option<Window>,
}

pub struct Fm {
    pub ui: Ui,
    st: RefCell<State>,
    /// >0 while we change the table ourselves: ignore the selection events that causes.
    quiet: Cell<u32>,
    /// The user is typing in the path box: do not rewrite its text under their cursor.
    typing: Cell<bool>,
    /// The open rename / new-folder dialog: (window, input, ok, cancel). For tests.
    pub dialog_ui: Cell<Option<(Window, TextInput, Button, Button)>>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

fn row_of(e: &Entry) -> Vec<String> {
    let mark = if e.is_dir { "\u{25B8} " } else { "   " }; // small right-pointing triangle for folders
    vec![
        format!("{mark}{}", e.name),
        e.size_text(),
        e.time_text(),
        e.kind(),
    ]
}

/// Integer upscale so tiny images (icons) are visible: nearest neighbour, longest edge up to ~128.
fn scaled(img: &ImageData) -> ImageData {
    let k = (128 / img.w.max(img.h).max(1)).clamp(1, 8) as usize;
    if k == 1 {
        return img.clone();
    }
    let (w, h) = (img.w as usize, img.h as usize);
    let mut rgba = Vec::with_capacity(w * h * k * k * 4);
    for y in 0..h * k {
        for x in 0..w * k {
            let s = ((y / k) * w + x / k) * 4;
            rgba.extend_from_slice(&img.rgba[s..s + 4]);
        }
    }
    ImageData {
        w: (w * k) as u32,
        h: (h * k) as u32,
        rgba,
    }
}

// ------------------------------------------------------------------ construction

pub fn build(opts: Options) -> Rc<Fm> {
    let win = Window::new("File Manager");
    win.set_size(1000, 700);
    win.set_min_size(640, 420);

    // ---- menus (accelerators are the keyboard interface; there is no key-event API)
    let bar = MenuBar::new(win);
    let file = Menu::new(bar, "File");
    let new_folder = MenuItem::new(file, "New Folder");
    new_folder.set_accel("F7");
    let rename = MenuItem::new(file, "Rename");
    rename.set_accel("F2");
    MenuSeparator::new(file);
    let copy = MenuItem::new(file, "Copy to Other Pane");
    copy.set_accel("F5");
    let mv = MenuItem::new(file, "Move to Other Pane");
    mv.set_accel("F6");
    let delete = MenuItem::new(file, "Delete");
    delete.set_accel("F8");
    MenuSeparator::new(file);
    let quit = MenuItem::new(file, "Quit");
    quit.set_accel("Ctrl+Q");
    let view = Menu::new(bar, "View");
    let hidden = CheckMenuItem::new(view, "Show Hidden Files");
    hidden.set_accel("Ctrl+H");
    let refresh_item = MenuItem::new(view, "Refresh");
    refresh_item.set_accel("Ctrl+R");
    let go = Menu::new(bar, "Go");
    let up_item = MenuItem::new(go, "Up One Level");
    up_item.set_accel("Alt+Up");
    let home_item = MenuItem::new(go, "Home Folder");
    home_item.set_accel("Alt+Home");
    let switch = MenuItem::new(go, "Switch Active Pane");
    switch.set_accel("F9");
    let helpm = Menu::new(bar, "Help");
    let about = MenuItem::new(helpm, "About");

    // ---- layout
    let root = VBox::new(win);
    root.set_spacing(6);
    root.set_expand(1.0); // a window stacks its children vertically; give the whole area to this one

    let tools = HBox::new(root);
    tools.set_spacing(6);
    let up = Button::new(tools, "\u{2191} Up");
    up.set_tooltip("Go to the parent folder (Alt+Up)");
    let refresh = Button::new(tools, "Refresh");
    refresh.set_tooltip("Reload both panes (Ctrl+R)");
    let path = TextInput::new(tools);
    path.set_expand(1.0);
    path.set_monospace(true);
    path.set_tooltip("Type a folder path to open it in the active pane");
    path.set_a11y_name("Path of the active pane");
    let copy_btn = Button::new(tools, "Copy \u{2192}");
    copy_btn.set_tooltip("Copy the selected item to the other pane (F5)");
    let move_btn = Button::new(tools, "Move \u{2192}");
    move_btn.set_tooltip("Move the selected item to the other pane (F6)");

    let body = Splitter::new(root, Orientation::Horizontal);
    body.set_expand(1.0);
    let tree = Tree::new(body);
    tree.set_a11y_name("Folders");
    let right = Splitter::new(body, Orientation::Vertical);
    let panes = Splitter::new(right, Orientation::Horizontal);
    let mut tables = Vec::new();
    let mut pane_labels = Vec::new();
    for (i, n) in PANE_NAMES.iter().enumerate() {
        let col = VBox::new(panes);
        col.set_spacing(2);
        pane_labels.push(Label::new(col, ""));
        let t = Table::new(col);
        t.set_expand(1.0);
        t.set_a11y_name(&format!("Files, pane {n}"));
        t.set_columns(&[
            Column::new("Name").width(125).sortable(true),
            Column::new("Size")
                .width(62)
                .align(ColumnAlign::Right)
                .sortable(true),
            Column::new("Modified").width(128).sortable(true),
            Column::new("Kind").width(90).sortable(true),
        ]);
        let _ = i;
        tables.push(t);
    }
    let pv = VBox::new(right);
    pv.set_spacing(2);
    let preview_title = Label::new(pv, "Preview");
    let pv_row = HBox::new(pv);
    pv_row.set_expand(1.0);
    let image = Image::new(pv_row);
    image.set_visible(false);
    image.set_align(Align::Start);
    let preview = TextArea::new(pv_row);
    preview.set_expand(1.0);
    preview.set_read_only(true);
    preview.set_monospace(true);
    preview.set_wrap(false);
    preview.set_a11y_name("Preview");

    let sbar = HBox::new(root);
    let status = Label::new(sbar, "");
    status.set_expand(1.0);
    let tag = Label::new(sbar, "");

    body.set_position(170);
    body.set_min_pane_sizes(100, 300);
    right.set_position(400);
    right.set_min_pane_sizes(120, 80);
    panes.set_min_pane_sizes(180, 180);

    // ---- context menu shared by both tables
    let popup = PopupMenu::new();
    let ctx_open = MenuItem::new(popup, "Open");
    let ctx_rename = MenuItem::new(popup, "Rename...");
    let ctx_delete = MenuItem::new(popup, "Delete...");
    MenuSeparator::new(popup);
    let ctx_copy = MenuItem::new(popup, "Copy to Other Pane");
    let ctx_new = MenuItem::new(popup, "New Folder...");
    for t in &tables {
        t.set_context_menu(popup);
    }

    let ui = Ui {
        win,
        body,
        right,
        panes,
        tree,
        tables: [tables[0], tables[1]],
        pane_labels: [pane_labels[0], pane_labels[1]],
        path,
        up,
        refresh,
        copy_btn,
        move_btn,
        preview_title,
        preview,
        image,
        status,
        tag,
        popup,
        items: Items {
            new_folder,
            rename,
            copy,
            mv,
            delete,
            quit,
            hidden,
            refresh: refresh_item,
            up: up_item,
            home: home_item,
            switch,
            about,
            ctx_open,
            ctx_rename,
            ctx_delete,
            ctx_copy,
            ctx_new,
        },
    };
    let fm = Rc::new(Fm {
        ui,
        st: RefCell::new(State::default()),
        quiet: Cell::new(0),
        typing: Cell::new(false),
        dialog_ui: Cell::new(None),
    });
    fm.wire();
    fm.init_tree();
    {
        let mut st = fm.st.borrow_mut();
        for p in &opts.dirs {
            st.panes.push(Pane {
                path: p.clone(),
                entries: vec![],
                sort: (SortKey::Name, true),
                mtime: None,
                truncated: false,
            });
        }
    }
    for (i, p) in opts.dirs.iter().enumerate() {
        if !fm.navigate(i, p.clone(), None, true, false) {
            // an unreadable start directory falls back to the home folder, then to "/"
            let fallback = home_dir().unwrap_or_else(|| PathBuf::from("/"));
            fm.navigate(i, fallback, None, true, false);
        }
    }
    fm.set_active(0);
    let first = fm.path_of(0);
    fm.tree_reveal(&first);
    fm.ui.win.show();
    fm
}

impl Fm {
    fn wire(self: &Rc<Self>) {
        let u = &self.ui;
        let it = u.items;
        macro_rules! click {
            ($btn:expr, $f:ident) => {{
                let fm = self.clone();
                $btn.on_click(move || fm.$f());
            }};
        }
        click!(it.new_folder, op_new_folder);
        click!(it.ctx_new, op_new_folder);
        click!(it.rename, op_rename);
        click!(it.ctx_rename, op_rename);
        click!(it.delete, op_delete);
        click!(it.ctx_delete, op_delete);
        click!(it.copy, op_copy);
        click!(it.ctx_copy, op_copy);
        click!(u.copy_btn, op_copy);
        click!(it.mv, op_move);
        click!(u.move_btn, op_move);
        click!(it.ctx_open, op_open);
        click!(it.refresh, refresh_all);
        click!(u.refresh, refresh_all);
        click!(it.up, go_up);
        click!(u.up, go_up);
        click!(it.home, go_home);
        click!(it.switch, switch_pane);
        click!(it.about, about);
        it.quit.on_click(App::quit);
        {
            let fm = self.clone();
            it.hidden.on_toggle(move |on| fm.set_show_hidden(on));
        }
        for i in 0..2 {
            let t = u.tables[i];
            let fm = self.clone();
            t.on_select(move |r| fm.on_row_selected(i, r));
            let fm = self.clone();
            t.on_activate(move |r| fm.on_row_activated(i, r));
            let fm = self.clone();
            t.on_column_click(move |c| fm.sort_by_column(i, c));
            let fm = self.clone();
            t.on_context_menu(move |_, _| fm.prepare_context_menu(i));
        }
        {
            let fm = self.clone();
            u.path.on_change(move |text| fm.on_path_typed(text));
        }
        {
            let fm = self.clone();
            u.tree.on_select(move |n| fm.on_tree_selected(n));
            let fm = self.clone();
            u.tree.on_expand(move |n, open| {
                if open {
                    fm.tree_load(n);
                }
            });
        }
        {
            let fm = self.clone();
            u.win.on_resize(move |w, h| {
                trace!("RESIZE {w} {h}");
                fm.relabel_soon();
            });
            for sp in [u.body, u.right, u.panes] {
                let fm = self.clone();
                sp.on_move(move |_| fm.relabel_soon());
            }
        }
        // pick up changes made by other programs
        let fm = self.clone();
        Timer::every(1500, move || fm.poll());
        // report geometry once the first layout has happened
        let fm = self.clone();
        Timer::once(500, move || {
            fm.print_bounds();
            trace!("READY");
        });
    }

    // ---------------------------------------------------------------- state helpers

    pub fn path_of(&self, pane: usize) -> PathBuf {
        self.st
            .borrow()
            .panes
            .get(pane)
            .map(|p| p.path.clone())
            .unwrap_or_default()
    }

    pub fn active(&self) -> usize {
        self.st.borrow().active
    }

    fn entry_at(&self, pane: usize, row: usize) -> Option<Entry> {
        self.st.borrow().panes.get(pane)?.entries.get(row).cloned()
    }

    /// The selected entry of `pane`, or None (nothing selected). The ".." row counts.
    pub fn selected(&self, pane: usize) -> Option<Entry> {
        let row = self.ui.tables[pane].selected()?;
        self.entry_at(pane, row)
    }

    /// The selected real item (not "..") of the active pane.
    fn target(&self) -> Option<Entry> {
        self.selected(self.active()).filter(|e| !e.is_parent)
    }

    pub fn say(&self, msg: &str) {
        self.ui.status.set_text(msg);
        trace!("STATUS {msg}");
    }

    fn quietly<R>(&self, f: impl FnOnce() -> R) -> R {
        self.quiet.set(self.quiet.get() + 1);
        let r = f();
        self.quiet.set(self.quiet.get() - 1);
        r
    }

    // ---------------------------------------------------------------- navigation

    /// Show `path` in `pane`. `select` picks a row afterwards (e.g. the folder we came from).
    /// `reveal` also selects the folder in the tree. Returns false (and says why) on failure.
    pub fn navigate(
        self: &Rc<Self>,
        pane: usize,
        path: PathBuf,
        select: Option<PathBuf>,
        reveal: bool,
        from_box: bool,
    ) -> bool {
        let hidden = self.st.borrow().show_hidden;
        let listing = match fsm::read_dir(&path, hidden) {
            Ok(l) => l,
            Err(e) => {
                self.say(&format!("Cannot open {}: {}", path.display(), e));
                trace!("ERROR NAV {} {}", PANE_NAMES[pane], path.display());
                return false;
            }
        };
        let n = listing.entries.len();
        let truncated = listing.truncated;
        {
            let mut st = self.st.borrow_mut();
            let Some(p) = st.panes.get_mut(pane) else {
                return false;
            };
            let mut entries = listing.entries;
            fsm::sort_entries(&mut entries, p.sort.0, p.sort.1);
            if let Some(parent) = Entry::parent_of(&path) {
                entries.insert(0, parent);
            }
            p.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            p.entries = entries;
            p.path = path.clone();
            p.truncated = truncated;
        }
        self.populate(pane, select.as_deref());
        self.typing.set(from_box);
        self.set_active(pane);
        self.refresh_labels();
        self.typing.set(false);
        if reveal {
            self.tree_reveal(&path);
        }
        // select + preview the requested row, else preview the directory itself
        let row = select.and_then(|s| {
            self.st.borrow().panes[pane]
                .entries
                .iter()
                .position(|e| e.path == s)
        });
        match row {
            Some(r) => self.on_row_selected(pane, Some(r)),
            None => {
                self.show_preview(Entry::from_path(&path));
                self.update_status();
            }
        }
        trace!("NAV {} {} {}", PANE_NAMES[pane], path.display(), n);
        if truncated {
            self.say(&format!(
                "Showing only the first {} entries of {}",
                fsm::MAX_ENTRIES,
                path.display()
            ));
        }
        true
    }

    /// Re-read `pane`, keeping the selected item selected when it still exists.
    fn reload(self: &Rc<Self>, pane: usize, keep: Option<PathBuf>) {
        let path = self.path_of(pane);
        let keep = keep.or_else(|| self.selected(pane).map(|e| e.path));
        if self.navigate_quiet(pane, &path, keep.clone()) {
            return;
        }
        // the directory vanished: climb to the nearest one that exists
        let mut p = path.clone();
        while let Some(parent) = p.parent().map(Path::to_path_buf) {
            if parent.is_dir() && self.navigate(pane, parent.clone(), Some(p.clone()), true, false)
            {
                return;
            }
            p = parent;
        }
    }

    /// Like `navigate` for the same directory but without the active-pane, tree and preview churn
    /// unless the selection changed. Returns false if the directory cannot be read.
    fn navigate_quiet(self: &Rc<Self>, pane: usize, path: &Path, keep: Option<PathBuf>) -> bool {
        let hidden = self.st.borrow().show_hidden;
        let Ok(listing) = fsm::read_dir(path, hidden) else {
            return false;
        };
        {
            let mut st = self.st.borrow_mut();
            let p = &mut st.panes[pane];
            let mut entries = listing.entries;
            fsm::sort_entries(&mut entries, p.sort.0, p.sort.1);
            if let Some(parent) = Entry::parent_of(path) {
                entries.insert(0, parent);
            }
            p.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            p.entries = entries;
            p.truncated = listing.truncated;
        }
        self.populate(pane, keep.as_deref());
        if pane == self.active() {
            let row = keep.and_then(|s| {
                self.st.borrow().panes[pane]
                    .entries
                    .iter()
                    .position(|e| e.path == s)
            });
            self.show_preview(
                row.and_then(|r| self.entry_at(pane, r))
                    .or_else(|| Entry::from_path(path)),
            );
        }
        self.update_status();
        true
    }

    #[allow(dead_code)] // used by tests/file_manager_mock.rs
    /// Test helper: navigate without touching the tree.
    pub fn navigate_for_test(self: &Rc<Self>, pane: usize, path: PathBuf) -> bool {
        self.navigate(pane, path, None, false, false)
    }

    pub fn refresh_all(self: &Rc<Self>) {
        for i in 0..2 {
            self.reload(i, None);
        }
        self.say("Refreshed");
        trace!("REFRESH");
    }

    /// Timer: reload panes whose directory changed on disk.
    fn poll(self: &Rc<Self>) {
        #[allow(clippy::needless_range_loop)]
        for i in 0..2 {
            let (path, old) = {
                let st = self.st.borrow();
                match st.panes.get(i) {
                    Some(p) => (p.path.clone(), p.mtime),
                    None => return,
                }
            };
            let now = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if now != old {
                trace!("CHANGED {}", PANE_NAMES[i]);
                self.reload(i, None);
            }
        }
    }

    pub fn go_up(self: &Rc<Self>) {
        let a = self.active();
        let cur = self.path_of(a);
        if let Some(parent) = cur.parent() {
            self.navigate(a, parent.to_path_buf(), Some(cur.clone()), true, false);
        }
    }

    fn go_home(self: &Rc<Self>) {
        if let Some(h) = home_dir() {
            let a = self.active();
            self.navigate(a, h, None, true, false);
        }
    }

    fn switch_pane(self: &Rc<Self>) {
        let other = 1 - self.active();
        self.set_active(other);
        self.ui.tables[other].focus();
    }

    fn set_show_hidden(self: &Rc<Self>, on: bool) {
        self.st.borrow_mut().show_hidden = on;
        trace!("HIDDEN {on}");
        for i in 0..2 {
            self.reload(i, None);
        }
        // rebuild the folder tree so hidden folders appear or vanish too
        self.init_tree();
        let a = self.active();
        let p = self.path_of(a);
        self.tree_reveal(&p);
    }

    fn on_path_typed(self: &Rc<Self>, text: &str) {
        if self.typing.get() {
            return;
        }
        let a = self.active();
        let t = text.trim();
        if t.is_empty() || Path::new(t) == self.path_of(a) {
            return;
        }
        let p = PathBuf::from(t);
        if p.is_dir() {
            self.navigate(a, p, None, true, true);
        }
    }

    pub fn set_active(&self, pane: usize) {
        let changed = {
            let mut st = self.st.borrow_mut();
            let c = st.active != pane;
            st.active = pane;
            c
        };
        self.refresh_labels();
        if changed {
            trace!("ACTIVE {}", PANE_NAMES[pane]);
        }
    }

    /// The pane captions depend on the pane widths, which are known only after layout.
    fn relabel_soon(self: &Rc<Self>) {
        let fm = self.clone();
        Timer::once(10, move || fm.refresh_labels());
    }

    /// Pane captions, path box, window title and the active-pane tag.
    fn refresh_labels(&self) {
        let (active, paths) = {
            let st = self.st.borrow();
            (
                st.active,
                st.panes.iter().map(|p| p.path.clone()).collect::<Vec<_>>(),
            )
        };
        for (i, p) in paths.iter().enumerate() {
            let mark = if i == active { "\u{25B6}" } else { " " };
            // labels do not clip, so fit the path to the pane (about 7 px per character)
            let room = (self.ui.tables[i].bounds().w / 7).max(12) as usize;
            let head = format!("{mark} {}  ", PANE_NAMES[i]);
            let path = fsm::ellipsize_path(
                &p.display().to_string(),
                room.saturating_sub(head.chars().count()),
            );
            self.ui.pane_labels[i].set_text(&format!("{head}{path}"));
        }
        let cur = paths.get(active).cloned().unwrap_or_default();
        let text = cur.display().to_string();
        if self.ui.path.text() != text && !self.typing.get() {
            self.ui.path.set_text(&text);
        }
        let name = cur
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| text.clone());
        self.ui
            .win
            .set_title(&format!("{name} [{}] - File Manager", PANE_NAMES[active]));
        self.ui
            .tag
            .set_text(&format!("  Active pane: {}", PANE_NAMES[active]));
        self.ui.up.set_enabled(cur.parent().is_some());
    }

    // ---------------------------------------------------------------- table

    /// Fill a pane's table from its entries. Uses one `batch` so huge directories send one update.
    fn populate(&self, pane: usize, select: Option<&Path>) {
        let (rows, sort, sel) = {
            let st = self.st.borrow();
            let p = &st.panes[pane];
            let rows: Vec<Vec<String>> = p.entries.iter().map(row_of).collect();
            (
                rows,
                p.sort,
                select.and_then(|s| p.entries.iter().position(|e| e.path == s)),
            )
        };
        let t = self.ui.tables[pane];
        self.quietly(|| {
            t.batch(|t| t.set_rows(&rows));
            t.set_sort_indicator(Some((sort.0.column(), sort.1)));
            t.set_selected(sel);
        });
    }

    fn sort_by_column(self: &Rc<Self>, pane: usize, col: usize) {
        let key = SortKey::from_column(col);
        let keep = self.selected(pane).map(|e| e.path);
        {
            let mut st = self.st.borrow_mut();
            let Some(p) = st.panes.get_mut(pane) else {
                return;
            };
            let asc = if p.sort.0 == key { !p.sort.1 } else { true };
            p.sort = (key, asc);
            let has_parent = p.entries.first().is_some_and(|e| e.is_parent);
            fsm::sort_entries(&mut p.entries[has_parent as usize..], key, asc);
        }
        self.populate(pane, keep.as_deref());
        let (k, asc) = self.st.borrow().panes[pane].sort;
        trace!(
            "SORT {} {} {}",
            PANE_NAMES[pane],
            k.column(),
            if asc { "asc" } else { "desc" }
        );
    }

    fn on_row_selected(self: &Rc<Self>, pane: usize, row: Option<usize>) {
        if self.quiet.get() > 0 {
            return;
        }
        self.set_active(pane);
        let e = row.and_then(|r| self.entry_at(pane, r));
        if let Some(e) = &e {
            trace!("SELECT {} {}", PANE_NAMES[pane], e.name);
        }
        self.show_preview(e);
        self.update_status();
    }

    fn on_row_activated(self: &Rc<Self>, pane: usize, row: usize) {
        self.set_active(pane);
        let Some(e) = self.entry_at(pane, row) else {
            return;
        };
        self.open_entry(pane, e);
    }

    fn open_entry(self: &Rc<Self>, pane: usize, e: Entry) {
        if e.is_dir {
            let from = self.path_of(pane);
            let select = e.is_parent.then_some(from);
            self.navigate(pane, e.path, select, true, false);
        } else {
            trace!("OPEN {} {}", PANE_NAMES[pane], e.name);
            self.show_preview(Some(e));
        }
    }

    fn op_open(self: &Rc<Self>) {
        if let Some(e) = self.selected(self.active()) {
            self.open_entry(self.active(), e);
        }
    }

    /// Runs just before the popup is shown: right-clicking a pane makes it the active one.
    fn prepare_context_menu(&self, pane: usize) {
        self.set_active(pane);
        let sel = self.selected(pane);
        let real = sel.as_ref().is_some_and(|e| !e.is_parent);
        let it = self.ui.items;
        it.ctx_open.set_enabled(sel.is_some());
        it.ctx_rename.set_enabled(real);
        it.ctx_delete.set_enabled(real);
        it.ctx_copy.set_enabled(real);
    }

    // ---------------------------------------------------------------- preview and status

    fn show_preview(&self, e: Option<Entry>) {
        let u = &self.ui;
        let Some(e) = e else {
            u.preview_title.set_text("Preview");
            u.preview.set_text("");
            u.image.set_image(None);
            u.image.set_visible(false);
            return;
        };
        let p = fsm::preview(&e);
        let (what, extra) = match &p {
            Preview::Text { truncated, .. } => (
                "text",
                if *truncated {
                    format!(" (first {} KB)", fsm::PREVIEW_LIMIT / 1024)
                } else {
                    String::new()
                },
            ),
            Preview::Hex { .. } => (
                "binary",
                format!(" (hex dump of the first {} bytes)", fsm::HEX_LIMIT),
            ),
            Preview::Dir(_) => ("folder", String::new()),
            Preview::Image { img, .. } => ("image", format!(" ({} x {})", img.w, img.h)),
            Preview::Error(_) => ("error", String::new()),
        };
        u.preview_title.set_text(&format!(
            "Preview: {}{extra}",
            if e.is_parent { ".." } else { &e.name }
        ));
        u.preview.set_text(p.body());
        match &p {
            Preview::Image { img, .. } => {
                u.image.set_image(Some(&scaled(img)));
                u.image.set_visible(true);
            }
            _ => {
                u.image.set_image(None);
                u.image.set_visible(false);
            }
        }
        trace!("PREVIEW {what} {} {}", p.shown_bytes(), e.name);
    }

    fn update_status(&self) {
        let a = self.active();
        let (n, dirs, total, trunc, sel) = {
            let st = self.st.borrow();
            let p = &st.panes[a];
            let real: Vec<&Entry> = p.entries.iter().filter(|e| !e.is_parent).collect();
            let dirs = real.iter().filter(|e| e.is_dir).count();
            let total: u64 = real.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
            (real.len(), dirs, total, p.truncated, p.entries.len())
        };
        let _ = sel;
        let mut s = format!(
            "{n} items ({dirs} folders, {} files), {} in files",
            n - dirs,
            fsm::human_size(total)
        );
        if trunc {
            s.push_str(" (truncated)");
        }
        if let Some(e) = self.selected(a) {
            s.push_str(&format!("   |   {}", fsm::describe(&e)));
        }
        self.ui.status.set_text(&s);
        trace!("STATUS {s}");
    }

    fn print_bounds(&self) {
        if !tracing() {
            return;
        }
        let u = &self.ui;
        let (b, ids): (Rect, Vec<(&str, Widget)>) = (
            u.win.bounds(),
            vec![
                ("tree", *u.tree),
                ("tableA", *u.tables[0]),
                ("tableB", *u.tables[1]),
                ("labelA", *u.pane_labels[0]),
                ("labelB", *u.pane_labels[1]),
                ("preview", *u.preview),
                ("image", *u.image),
                ("path", *u.path),
                ("up", *u.up),
                ("refresh", *u.refresh),
                ("copy", *u.copy_btn),
                ("move", *u.move_btn),
                ("status", *u.status),
                ("body", *u.body),
                ("right", *u.right),
                ("panes", *u.panes),
            ],
        );
        let _ = b;
        for (n, w) in ids {
            let r = w.bounds();
            println!("BOUNDS {n} {} {} {} {}", r.x, r.y, r.w, r.h);
        }
    }

    // ---------------------------------------------------------------- tree

    fn init_tree(self: &Rc<Self>) {
        let t = self.ui.tree;
        t.clear();
        let mut roots: Vec<(String, PathBuf)> = Vec::new();
        if let Some(h) = home_dir() {
            roots.push(("Home".into(), h));
        }
        #[cfg(windows)]
        for d in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", d as char));
            if p.is_dir() {
                roots.push((format!("{}:\\", d as char), p));
            }
        }
        #[cfg(not(windows))]
        roots.push(("File System (/)".into(), PathBuf::from("/")));
        let mut st = self.st.borrow_mut();
        st.node_paths.clear();
        st.loaded.clear();
        for (name, p) in roots {
            let n = t.add(None, &name);
            t.set_has_children(n, true);
            st.node_paths.insert(n, p);
        }
    }

    pub fn node_path(&self, n: TreeNodeId) -> Option<PathBuf> {
        self.st.borrow().node_paths.get(&n).cloned()
    }

    /// Add the sub-folders of `node` the first time it is needed.
    pub fn tree_load(&self, node: TreeNodeId) {
        let Some(path) = self.node_path(node) else {
            return;
        };
        if !self.st.borrow_mut().loaded.insert(node) {
            return;
        }
        let hidden = self.st.borrow().show_hidden;
        let dirs = fsm::subdirs(&path, hidden);
        let t = self.ui.tree;
        let mut new = Vec::new();
        t.batch(|t| {
            for d in &dirs {
                let c = t.add(Some(node), &d.name);
                t.set_has_children(c, true);
                new.push((c, d.path.clone()));
            }
            t.set_has_children(node, !dirs.is_empty());
        });
        self.st.borrow_mut().node_paths.extend(new);
    }

    /// Expand the tree down to `path` and select it (no callbacks fire).
    fn tree_reveal(&self, path: &Path) {
        let t = self.ui.tree;
        let mut best: Option<(TreeNodeId, usize)> = None;
        for r in t.children(None) {
            if let Some(rp) = self.node_path(r) {
                let depth = rp.components().count();
                if path.starts_with(&rp) && best.is_none_or(|(_, d)| depth > d) {
                    best = Some((r, depth));
                }
            }
        }
        let Some((mut node, _)) = best else { return };
        let Some(root_path) = self.node_path(node) else {
            return;
        };
        let Ok(rest) = path.strip_prefix(&root_path) else {
            return;
        };
        let mut cur = root_path;
        for comp in rest.components() {
            self.tree_load(node);
            cur.push(comp);
            match t
                .children(Some(node))
                .into_iter()
                .find(|c| self.node_path(*c).as_deref() == Some(cur.as_path()))
            {
                Some(c) => node = c,
                None => break,
            }
        }
        self.tree_load(node); // set_selected expands its ancestors; the node itself must not show a placeholder
        t.set_selected(Some(node));
    }

    fn on_tree_selected(self: &Rc<Self>, n: Option<TreeNodeId>) {
        let Some(path) = n.and_then(|n| self.node_path(n)) else {
            return;
        };
        let a = self.active();
        if path != self.path_of(a) {
            trace!("TREE {}", path.display());
            self.navigate(a, path, None, false, false);
        }
    }

    // ---------------------------------------------------------------- operations

    fn confirm(&self, title: &str, text: &str) -> bool {
        message_box(
            Some(self.ui.win),
            MessageKind::Question,
            Buttons::YesNo,
            title,
            text,
        ) == Answer::Yes
    }

    /// Reload every pane showing `dir` (or all, if `dir` is None) and select `select` in the active one.
    fn refresh_after(self: &Rc<Self>, select: Option<PathBuf>) {
        let a = self.active();
        for i in 0..2 {
            let keep = if i == a { select.clone() } else { None };
            self.reload(i, keep);
        }
    }

    fn op_new_folder(self: &Rc<Self>) {
        let a = self.active();
        let dir = self.path_of(a);
        self.dialog(
            "New Folder",
            "Name of the new folder:",
            "New Folder",
            move |fm, name| {
                let p = fsm::make_dir(&dir, name)?;
                trace!("MKDIR {}", p.display());
                fm.refresh_after(Some(p));
                fm.say(&format!("Created folder \"{name}\""));
                Ok(())
            },
        );
    }

    fn op_rename(self: &Rc<Self>) {
        let Some(e) = self.target() else {
            self.say("Select an item to rename first");
            return;
        };
        let old = e
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (prompt, initial) = (format!("New name for \"{old}\":"), old.clone());
        self.dialog("Rename", &prompt, &initial, move |fm, name| {
            let p = fsm::rename_path(&e.path, name)?;
            trace!("RENAME {} {}", e.name, name);
            fm.refresh_after(Some(p));
            fm.say(&format!("Renamed \"{old}\" to \"{name}\""));
            Ok(())
        });
    }

    fn op_delete(self: &Rc<Self>) {
        let Some(e) = self.target() else {
            self.say("Select an item to delete first");
            return;
        };
        let what = if e.is_dir && !e.is_link {
            "folder and everything in it"
        } else {
            "item"
        };
        if !self.confirm(
            "Delete",
            &format!(
                "Delete the {what} \"{}\"?\n\nThis cannot be undone.",
                e.name
            ),
        ) {
            self.say("Delete cancelled");
            return;
        }
        match fsm::remove_path(&e.path) {
            Ok(()) => {
                trace!("DELETE {}", e.name);
                self.refresh_after(None);
                self.say(&format!("Deleted \"{}\"", e.name));
            }
            Err(err) => {
                trace!("ERROR DELETE {}", e.name);
                self.say(&format!("Cannot delete \"{}\": {err}", e.name));
            }
        }
    }

    fn op_copy(self: &Rc<Self>) {
        self.transfer(false)
    }

    fn op_move(self: &Rc<Self>) {
        self.transfer(true)
    }

    fn transfer(self: &Rc<Self>, mv: bool) {
        let verb = if mv { "Move" } else { "Copy" };
        let Some(e) = self.target() else {
            self.say(&format!("Select an item to {} first", verb.to_lowercase()));
            return;
        };
        let a = self.active();
        let dst_dir = self.path_of(1 - a);
        let dest = match fsm::check_transfer(&e.path, &dst_dir) {
            Ok(d) => d,
            Err(err) => {
                self.say(&err);
                return;
            }
        };
        if std::fs::symlink_metadata(&dest).is_ok() {
            if !self.confirm(
                "Overwrite",
                &format!(
                    "\"{}\" already exists in {}.\n\nReplace it?",
                    e.name,
                    dst_dir.display()
                ),
            ) {
                self.say(&format!("{verb} cancelled"));
                return;
            }
            if let Err(err) = fsm::remove_path(&dest) {
                self.say(&format!("Cannot replace \"{}\": {err}", e.name));
                return;
            }
        }
        let r = if mv {
            fsm::move_into(&e.path, &dst_dir)
        } else {
            fsm::copy_into(&e.path, &dst_dir)
        };
        match r {
            Ok(new) => {
                trace!("{} {} {}", verb.to_uppercase(), e.name, dst_dir.display());
                // both panes re-read; the other pane shows the new item selected
                self.reload(a, None);
                self.reload(1 - a, Some(new));
                self.say(&format!(
                    "{} \"{}\" to {}",
                    if mv { "Moved" } else { "Copied" },
                    e.name,
                    dst_dir.display()
                ));
            }
            Err(err) => {
                trace!("ERROR {} {}", verb.to_uppercase(), e.name);
                self.say(&err);
            }
        }
    }

    fn about(&self) {
        message_box(
            Some(self.ui.win),
            MessageKind::Info,
            Buttons::Ok,
            "About File Manager",
            "A dual-pane file manager written with rungui's public API only.\n\nF2 rename, F5 copy, F6 move, F7 new folder, F8 delete,\nF9 switch pane, Ctrl+H hidden files, Ctrl+R refresh.\nTimes are shown in UTC.",
        );
    }

    // ---------------------------------------------------------------- the rename / new folder dialog

    /// A small window with a text field and OK / Cancel (rungui has no input-dialog API).
    /// `action` returns an error message to keep the dialog open.
    fn dialog(
        self: &Rc<Self>,
        title: &str,
        prompt: &str,
        initial: &str,
        action: impl Fn(&Rc<Fm>, &str) -> std::result::Result<(), String> + 'static,
    ) {
        if let Some(old) = self.st.borrow_mut().dialog.take() {
            old.destroy();
        }
        let w = Window::new(title);
        w.set_size(380, 150);
        w.set_resizable(false);
        let col = VBox::new(w);
        col.set_spacing(8);
        Label::new(col, prompt);
        let input = TextInput::new(col);
        input.set_text(initial);
        input.set_a11y_name(title);
        let err = Label::new(col, "");
        let row = HBox::new(col);
        row.set_spacing(8);
        Spacer::new(row);
        let cancel = Button::new(row, "Cancel");
        let ok = Button::new(row, "OK");
        self.st.borrow_mut().dialog = Some(w);
        self.dialog_ui.set(Some((w, input, ok, cancel)));
        trace!("DIALOG open {title}");
        {
            let fm = self.clone();
            ok.on_click(move || {
                let text = input.text();
                match action(&fm, text.trim()) {
                    Ok(()) => {
                        trace!("DIALOG done");
                        fm.st.borrow_mut().dialog = None;
                        w.destroy();
                    }
                    Err(e) => {
                        trace!("DIALOG error {e}");
                        err.set_text(&e);
                    }
                }
            });
            let fm = self.clone();
            cancel.on_click(move || {
                trace!("DIALOG cancel");
                fm.st.borrow_mut().dialog = None;
                w.destroy();
            });
            let fm = self.clone();
            w.on_close(move || {
                fm.st.borrow_mut().dialog = None;
                true
            });
        }
        w.show();
        input.focus();
        if tracing() {
            Timer::once(300, move || {
                for (n, wd) in [
                    ("dlg_input", *input),
                    ("dlg_ok", *ok),
                    ("dlg_cancel", *cancel),
                ] {
                    let r = wd.bounds();
                    println!("BOUNDS {n} {} {} {} {}", r.x, r.y, r.w, r.h);
                }
            });
        }
    }
}
