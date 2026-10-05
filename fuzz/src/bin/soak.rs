//! Leak soak for a real backend: builds a window holding one of every widget kind (menus, table,
//! tree, text, splitter, tabs...), shows it, destroys it, and repeats, reporting resident memory
//! and open file descriptors. After the toolkit's caches warm up both must stay flat; a steady
//! climb means native objects, handles or Rust-side state are not released.
//!
//!   xvfb-run -a cargo run --release --manifest-path fuzz/Cargo.toml --bin soak -- [iterations=2000] [section...]
//!
//! Naming sections (menus basic text lists numeric image tabs table tree group) builds only those.
use rungui::*;
use std::cell::Cell;
use std::rc::Rc;

/// Widgets per table/tree in each round.
const ROWS: usize = 50;
/// Report every this many iterations.
const REPORT_EVERY: u32 = 250;
/// Let the loop run this long between building and destroying a window.
const LIVE_MS: u32 = 2;

fn resources() -> (u64, usize) {
    let rss_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .unwrap_or(0);
    let fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count());
    (rss_pages * 4, fds)
}

/// The parts of the window, so a leak can be narrowed down by naming only some of them.
const SECTIONS: [&str; 13] = [
    "menus", "menubar", "accel", "popup", "basic", "text", "lists", "numeric", "image", "tabs", "table", "tree", "group",
];

/// A window and the popup menu that belongs to it: popups have no parent, so the app destroys them.
fn build(only: &[String]) -> (Window, Option<PopupMenu>) {
    let on = |name: &str| only.is_empty() || only.iter().any(|o| o == name);
    let win = Window::new("soak");
    if on("menus") || on("menubar") || on("accel") {
        let bar = MenuBar::new(win);
        let file = Menu::new(bar, "&File");
        let open = MenuItem::new(file, "&Open");
        if on("menus") || on("accel") {
            open.set_accel("Ctrl+O");
        }
        CheckMenuItem::new(file, "Check");
        MenuSeparator::new(file);
        let sub = Menu::new(file, "Sub");
        MenuItem::new(sub, "Item");
    }
    let col = VBox::new(win);
    let popup = (on("menus") || on("popup")).then(|| {
        let popup = PopupMenu::new();
        MenuItem::new(popup, "Popup item");
        col.set_context_menu(popup);
        popup
    });
    if on("basic") {
        Label::new(col, "label");
        Button::new(col, "button");
        CheckBox::new(col, "check");
        let g = RadioGroup::new();
        RadioButton::new(col, &g, "r1");
        RadioButton::new(col, &g, "r2");
    }
    if on("text") {
        TextInput::new(col).set_text("text");
        TextInput::password(col);
        TextArea::new(col).set_text("multi\nline");
    }
    if on("lists") {
        ComboBox::new(col).set_items(&["a", "b", "c"]);
        ListBox::new(col).set_items(&["x", "y", "z"]);
    }
    if on("numeric") {
        Slider::new(col, 0.0, 10.0);
        SpinBox::new(col, 0.0, 10.0, 1.0);
        ProgressBar::new(col).set_indeterminate(true);
    }
    if on("image") {
        Image::new(col).set_image(Some(&ImageData {
            w: 8,
            h: 8,
            rgba: vec![200; 8 * 8 * 4],
        }));
    }
    if on("tabs") {
        let tabs = Tabs::new(col);
        let page = tabs.add_page("one");
        Label::new(page, "in page");
        let two = tabs.add_page("two");
        // removing every page and adding another used to leave GNUstep's NSTabView pointing at
        // a freed view (run with NSZombieEnabled=YES to see it)
        page.destroy();
        two.destroy();
        tabs.add_page("three");
    }
    let split = Splitter::new(col, Orientation::Horizontal);
    if on("table") {
        let table = Table::new(split);
        table.set_columns(&[Column::new("a"), Column::new("b").sortable(true)]);
        let rows: Vec<Vec<String>> = (0..ROWS).map(|i| vec![i.to_string(), "x".into()]).collect();
        table.set_rows(&rows);
        table.set_sort_indicator(Some((1, true)));
    }
    if on("tree") {
        let tree = Tree::new(split);
        let root = tree.add(None, "root");
        for i in 0..ROWS {
            tree.add(Some(root), &i.to_string());
        }
        tree.set_expanded(root, true);
    }
    if on("group") {
        let gb = GroupBox::new(col, "group");
        let inner = HBox::new(gb);
        Button::new(inner, "in group");
    }
    win.set_size(500, 500);
    win.show();
    (win, popup)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let iterations: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(2000);
    let only: Vec<String> = args.collect();
    if let Some(bad) = only.iter().find(|o| !SECTIONS.contains(&o.as_str())) {
        eprintln!("unknown section {bad}; sections: {}", SECTIONS.join(", "));
        std::process::exit(2);
    }
    let app = App::new("rungui-soak").expect("toolkit init");
    App::set_quit_on_last_close(false);
    let n = Rc::new(Cell::new(0u32));
    let live: Rc<Cell<Option<(Window, Option<PopupMenu>)>>> = Rc::new(Cell::new(None));
    let (n2, live2) = (n.clone(), live.clone());
    Timer::every(LIVE_MS, move || {
        if let Some((w, popup)) = live2.take() {
            w.destroy();
            if let Some(p) = popup {
                p.destroy();
            }
            n2.set(n2.get() + 1);
            if n2.get() % REPORT_EVERY == 0 {
                let (rss, fds) = resources();
                println!("iteration {}: rss {rss} KiB, {fds} fds", n2.get());
            }
            if n2.get() >= iterations {
                App::quit();
            }
        } else {
            live2.set(Some(build(&only)));
        }
    });
    app.run();
}
