//! Times the real backend on large UIs (run it under Xvfb): how long it takes to create, lay out and
//! destroy thousands of widgets, to fill a table and a tree with tens of thousands of rows, and to
//! change one row. Prints milliseconds per step so a pathological backend (quadratic in what the
//! app holds) shows up as a step that grows much faster than its size.
//!
//!   xvfb-run -a cargo run --release --manifest-path fuzz/Cargo.toml --bin bench_native [-- scale=1]
use rungui::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

/// Widgets in the form, rows of the table and nodes of the tree at scale 1.
const WIDGETS: usize = 1_000;
const ROWS: usize = 10_000;
const NODES: usize = 10_000;
/// Columns of the table.
const COLS: usize = 5;

/// One timed step: runs `f`, lets the toolkit settle with `App::update` and prints the time.
fn step(name: &str, size: usize, f: impl FnOnce()) {
    let t0 = Instant::now();
    f();
    App::update();
    println!(
        "{name:<34}{size:>8} {:>10.1} ms",
        t0.elapsed().as_secs_f64() * 1e3
    );
}

fn main() {
    let scale: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.trim_start_matches("scale=").parse().ok())
        .unwrap_or(1);
    let app = App::new("rungui-bench").expect("toolkit init");
    let done = Rc::new(RefCell::new(false));
    let d = done.clone();
    Timer::once(1, move || {
        run(scale);
        *d.borrow_mut() = true;
        App::quit();
    });
    app.run();
    assert!(*done.borrow());
}

fn run(scale: usize) {
    let (widgets, rows, nodes) = (WIDGETS * scale, ROWS * scale, NODES * scale);
    let win = Window::new("bench");
    win.set_size(800, 600);
    let split = Splitter::new(win, Orientation::Horizontal);
    let form = VBox::new(split);
    let mut labels = vec![];
    // in batches, so a cost that grows with the number of widgets already there shows up as batches
    // getting slower
    const BATCH: usize = 500;
    for b in 0..(widgets / BATCH).max(1) {
        step(
            &format!("create {BATCH} labels+entries (#{b})"),
            b * BATCH,
            || {
                for i in 0..BATCH / 2 {
                    labels.push(Label::new(form, &format!("label {i}")));
                    TextInput::new(form).set_text(&i.to_string());
                }
            },
        );
    }
    let right = VBox::new(split);
    let table = Table::new(right);
    let tree = Tree::new(right);
    step("show", widgets, || win.show());
    step("resize", widgets, || {
        win.set_size(900, 700);
        win.set_size(800, 600);
    });
    step("change one label (relayout)", widgets, || {
        labels[0].set_text("changed")
    });
    let cols: Vec<Column> = (0..COLS).map(|i| Column::new(&format!("c{i}"))).collect();
    step("table columns", COLS, || table.set_columns(&cols));
    let data: Vec<Vec<String>> = (0..rows)
        .map(|r| (0..COLS).map(|c| format!("{r}:{c}")).collect())
        .collect();
    step("table set_rows", rows, || table.set_rows(&data));
    step("table select last", rows, || {
        table.set_selected(Some(rows - 1))
    });
    step("table change one cell", rows, || {
        table.set_cell(rows / 2, 1, "changed")
    });
    let list = ListBox::new(right);
    let items: Vec<String> = (0..rows).map(|i| format!("item {i}")).collect();
    step("listbox set_items", rows, || list.set_items(&items));
    step("listbox select last", rows, || {
        list.set_selected(Some(rows - 1))
    });
    let combo = ComboBox::new(right);
    step("combo set_items", rows / 2, || {
        combo.set_items(&items[..rows / 2])
    });
    let text = TextArea::new(right);
    let big = "the quick brown fox jumps over the lazy dog\n".repeat(rows / 2);
    step("textarea set_text", big.len(), || text.set_text(&big));
    step("tree add (batched)", nodes, || {
        tree.batch(|t| {
            let root = t.add(None, "root");
            for i in 0..nodes {
                t.add(Some(root), &i.to_string());
            }
            t.set_expanded(root, true);
        })
    });
    step("tree rename one node", nodes, || {
        if let Some(n) = tree.children(None).first() {
            tree.set_text(*n, "renamed");
        }
    });
    // menus: every item with an accelerator re-registers the window's accelerator table
    const MENU_ITEMS: usize = 1_000;
    let bar = MenuBar::new(win);
    let menu = Menu::new(bar, "&Big");
    let mut items_m = vec![];
    step("menu items with accelerators", MENU_ITEMS, || {
        for i in 0..MENU_ITEMS {
            let it = MenuItem::new(menu, &format!("Item {i}"));
            it.set_accel(&format!("Ctrl+Alt+F{}", i % 24 + 1));
            items_m.push(it);
        }
    });
    step("destroy menu", MENU_ITEMS, || menu.destroy());
    step("destroy table", rows, || table.destroy());
    step("destroy form", widgets, || form.destroy());
    step("destroy window", 1, || win.destroy());
}
