//! Second scripted smoke target (driven by scripts/gtk-smoke.sh): splitter drag, monospace / wrap,
//! window position, window shrinking and a table + tree inside splitters. Prints one line per
//! event and the splitter / window bounds on stdout.
use rungui::*;

fn main() {
    let app = App::new("smoke2").expect("init");
    let w = Window::new("smoke2");
    w.set_position(120, 90);
    let col = VBox::new(w);
    let mono = TextInput::new(col);
    mono.set_monospace(true);
    mono.set_text("monospace entry");
    let hs = Splitter::new(col, Orientation::Horizontal);
    hs.set_position(200);
    hs.set_min_pane_sizes(60, 60);
    let area = TextArea::new(hs);
    area.set_monospace(true);
    area.set_wrap(false);
    area.set_text("a long line that does not wrap: 0123456789 0123456789 0123456789 0123456789\n");
    let table = Table::new(hs);
    table.set_columns(&[Column::new("Name").width(80), Column::new("Size")]);
    table.set_rows(&[vec!["a", "1"], vec!["b", "2"]]);
    let vs = Splitter::new(col, Orientation::Vertical);
    let tree = Tree::new(vs);
    let r = tree.add(None, "root");
    tree.add(Some(r), "kid");
    let list = ListBox::new(vs);
    list.set_items(&["one", "two"]);
    table.on_select(|i| println!("TABLE_SEL {i:?}"));
    tree.on_select(|n| println!("TREE_SEL {}", n.is_some()));
    tree.on_expand(|_, open| println!("TREE_EXPAND {open}"));
    hs.on_move(|p| println!("HSPLIT {p}"));
    vs.on_move(|p| println!("VSPLIT {p}"));
    w.on_move(|x, y| println!("MOVED {x} {y}"));
    w.on_resize(|w, h| println!("RESIZED {w} {h}"));
    w.on_close(|| {
        App::quit();
        true
    });
    w.show();
    let _t = Timer::once(600, move || {
        println!("POS {:?}", w.position());
        for (n, b) in [
            ("hs", hs.bounds()),
            ("vs", vs.bounds()),
            ("table", table.bounds()),
            ("tree", tree.bounds()),
            ("area", area.bounds()),
        ] {
            println!("BOUNDS {n} {} {} {} {}", b.x, b.y, b.w, b.h);
        }
        println!("HPOS {} VPOS {}", hs.position(), vs.position());
        println!(
            "MONO {} {} WRAP {}",
            mono.monospace(),
            area.monospace(),
            area.wrap()
        );
        println!("READY");
    });
    app.run();
    println!("BYE");
}
