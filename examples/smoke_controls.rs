//! Scripted smoke-test target (driven by scripts/smoke-gtk.sh with xdotool): prints one line per
//! event / widget bounds on stdout so a shell script can click and type at known positions.
use rungui::*;

fn main() {
    let app = App::new("smoke").expect("init");
    let a = Window::new("smoke-a");
    let col = VBox::new(a);
    let btn = Button::new(col, "Press ünï");
    let txt = TextInput::new(col);
    let chk = CheckBox::new(col, "Check");
    let list = ListBox::new(col);
    list.set_items(&["one", "two", "three"]);
    let slider = Slider::new(col, 0.0, 100.0);
    let combo = ComboBox::new(col);
    combo.set_items(&["x", "y"]);
    let dlg = Button::new(col, "Dialog");
    dlg.on_click(move || {
        let r = message_box(
            Some(a),
            MessageKind::Question,
            Buttons::YesNo,
            "Title ü",
            "Really?",
        );
        println!("DIALOG {r:?}");
    });
    let g = RadioGroup::new();
    let ra = RadioButton::new(col, &g, "Ra");
    let rb = RadioButton::new(col, &g, "Rb");
    ra.set_checked(true);
    ra.on_toggle(|v| println!("RA {v}"));
    rb.on_toggle(|v| println!("RB {v}"));
    let table = Table::new(col);
    table.set_columns(&[
        Column::new("Name").width(80).sortable(true),
        Column::new("Size").align(ColumnAlign::Right),
    ]);
    table.set_rows(&[vec!["a", "1"], vec!["b", "2"], vec!["c", "3"]]);
    table.on_select(|i| println!("TABLE_SEL {i:?}"));
    table.on_column_click(|c| println!("TABLE_COL {c}"));
    let tree = Tree::new(col);
    let root = tree.add(None, "root");
    let kid = tree.add(Some(root), "kid");
    let lazy = tree.add(None, "lazy");
    tree.set_has_children(lazy, true);
    let _ = kid;
    tree.on_select(|n| println!("TREE_SEL {}", n.is_some()));
    tree.on_expand(|_, open| println!("TREE_EXPAND {open}"));
    a.set_context_menu({
        let p = PopupMenu::new();
        MenuItem::new(p, "Ctx item").on_click(|| println!("CTXMENU"));
        p
    });
    btn.on_click(|| println!("CLICK"));
    txt.on_change(|t| println!("TEXT {t}"));
    chk.on_toggle(|b| println!("TOGGLE {b}"));
    list.on_select(|i| println!("SELECT {i:?}"));
    slider.on_change(|v| println!("VALUE {v:.0}"));
    combo.on_select(|i| println!("COMBO {i:?}"));
    a.on_resize(|w, h| println!("RESIZED {w} {h}"));
    a.on_close(|| {
        println!("CLOSE_REQUESTED");
        true
    });

    let b = Window::new("smoke-b");
    let bar = MenuBar::new(b);
    let file = Menu::new(bar, "File");
    let item = MenuItem::new(file, "Quit");
    item.set_accel("Ctrl+Q");
    item.on_click(|| {
        println!("MENU_QUIT");
        App::quit();
    });
    let ck = CheckMenuItem::new(file, "Flag");
    ck.on_toggle(|v| println!("MENUTOGGLE {v}"));
    Label::new(b, "second window");

    a.show();
    b.show();
    println!(
        "HANDLE {}",
        a.native_handle().is_some() && btn.native_handle().is_some()
    );
    let _t = Timer::once(600, move || {
        for (n, w) in [
            ("btn", btn.bounds()),
            ("txt", txt.bounds()),
            ("chk", chk.bounds()),
            ("list", list.bounds()),
            ("slider", slider.bounds()),
            ("combo", combo.bounds()),
            ("dlg", dlg.bounds()),
            ("rb", rb.bounds()),
            ("table", table.bounds()),
            ("tree", tree.bounds()),
        ] {
            println!("BOUNDS {n} {} {} {} {}", w.x, w.y, w.w, w.h);
        }
        println!("READY");
    });
    // CI: RUNGUI_EXIT_AFTER_MS=n quits the loop after n ms so a startup crash shows up as a failure.
    let _exit = std::env::var("RUNGUI_EXIT_AFTER_MS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map(|ms| Timer::once(ms, App::quit));
    app.run();
    println!("BYE");
}
