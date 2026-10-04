//! Scripted smoke target for the Cocoa backend (driven by scripts/smoke-gnustep.sh with xdotool
//! under Xvfb; works with any backend). Prints one line per event and the bounds of the widgets
//! to click on, in window-client coordinates (all of them live directly in window `cc-a`).
use rungui::*;

fn main() {
    let app = App::new("smoke-cocoa").expect("init");
    let a = Window::new("cc-a");
    let bar = MenuBar::new(a);
    let file = Menu::new(bar, "File");
    let flag = CheckMenuItem::new(file, "Flag");
    flag.set_accel("Ctrl+K");
    flag.on_toggle(|v| println!("MENUTOGGLE {v}"));
    let go = MenuItem::new(file, "Go");
    go.set_accel("Ctrl+Shift+G");
    go.on_click(|| println!("MENU_GO"));
    let quit = MenuItem::new(file, "Quit");
    quit.set_accel("Ctrl+Q");
    quit.on_click(|| {
        println!("MENU_QUIT");
        App::quit();
    });

    let col = VBox::new(a);
    let row = HBox::new(col);
    let btn = Button::new(row, "Press ünï");
    let spin = SpinBox::new(row, 0.0, 10.0, 1.0);
    let txt = TextInput::new(col);
    let chk = CheckBox::new(col, "Monospace");
    let hs = Splitter::new(col, Orientation::Horizontal);
    let ta1 = TextArea::new(hs);
    let ta2 = TextArea::new(hs);
    ta2.set_text(
        "long line without wrapping: 0123456789 0123456789 0123456789 0123456789 0123456789",
    );
    ta2.set_wrap(false);
    hs.set_position(200);
    hs.set_min_pane_sizes(60, 60);
    let slider = Slider::new(col, 0.0, 100.0);
    let combo = ComboBox::new(col);
    combo.set_items(&["x", "y", "z"]);
    let tabs = Tabs::new(col);
    let p1 = tabs.add_page("First");
    let p2 = tabs.add_page("Second");
    Label::new(p1, "page one");
    Label::new(p2, "page two");
    let vs = Splitter::new(col, Orientation::Vertical);
    let table = Table::new(vs);
    table.set_columns(&[
        Column::new("Name").width(80).sortable(true),
        Column::new("Size").align(ColumnAlign::Right),
    ]);
    table.set_rows(&[vec!["a", "1"], vec!["b", "2"], vec!["c", "3"]]);
    let tree = Tree::new(vs);
    let root = tree.add(None, "root");
    tree.add(Some(root), "kid");
    let lazy = tree.add(None, "lazy");
    tree.set_has_children(lazy, true);
    vs.set_position(100);
    vs.set_min_pane_sizes(40, 40);

    btn.on_click(|| println!("CLICK"));
    spin.on_change(|v| println!("SPIN {v}"));
    txt.on_change(|t| println!("TEXT {t}"));
    ta1.on_change(|t| println!("AREA {}", t.replace('\n', "|")));
    chk.on_toggle(move |on| {
        println!("MONO {on}");
        txt.set_monospace(on);
        ta1.set_monospace(on);
    });
    hs.on_move(|p| println!("HSPLIT {p}"));
    vs.on_move(|p| println!("VSPLIT {p}"));
    slider.on_change(|v| println!("VALUE {v:.0}"));
    combo.on_select(|i| println!("COMBO {i:?}"));
    tabs.on_select(|i| println!("TAB {i:?}"));
    table.on_select(|i| println!("TABLE_SEL {i:?}"));
    table.on_column_click(move |c| {
        let ascending = table.sort_indicator() != Some((c, true));
        let mut rows = table.rows();
        rows.sort_by(|x, y| x[c].cmp(&y[c]));
        if !ascending {
            rows.reverse();
        }
        table.set_rows(&rows);
        table.set_sort_indicator(Some((c, ascending)));
        println!(
            "TABLE_COL {c} {} {}",
            if ascending { "asc" } else { "desc" },
            rows[0][0]
        );
    });
    tree.on_select(move |n| println!("TREE_SEL {}", n.is_some()));
    tree.on_expand(|_, open| println!("TREE_EXPAND {open}"));
    a.set_context_menu({
        let p = PopupMenu::new();
        MenuItem::new(p, "Ctx item").on_click(|| println!("CTXMENU"));
        p
    });
    a.on_resize(|w, h| println!("RESIZED {w} {h}"));
    a.on_move(|x, y| println!("MOVED {x} {y}"));
    a.set_min_size(300, 400);
    a.set_position(220, 100);
    a.set_size(560, 700);
    a.on_close(|| {
        println!("CLOSE_REQUESTED");
        true
    });
    a.show();
    println!(
        "HANDLE {}",
        a.native_handle().is_some() && btn.native_handle().is_some()
    );
    let _t = Timer::once(700, move || {
        for (n, w) in [
            ("btn", btn.bounds()),
            ("spin", spin.bounds()),
            ("txt", txt.bounds()),
            ("chk", chk.bounds()),
            ("hs", hs.bounds()),
            ("slider", slider.bounds()),
            ("combo", combo.bounds()),
            ("tabs", tabs.bounds()),
            ("vs", vs.bounds()),
            ("table", table.bounds()),
            ("tree", tree.bounds()),
        ] {
            println!("BOUNDS {n} {} {} {} {}", w.x, w.y, w.w, w.h);
        }
        println!("SPLITPOS h {} v {}", hs.position(), vs.position());
        println!("READY");
    });
    app.run();
    println!("BYE");
}
