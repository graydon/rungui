//! Every widget kind in one window. Public API only.
use rungui::*;

fn main() {
    let app = App::new("kitchen-sink").expect("init");
    let win = Window::new("rungui kitchen sink");

    // Menus
    let bar = MenuBar::new(win);
    let file = Menu::new(bar, "File");
    let open = MenuItem::new(file, "Open...");
    open.set_accel("Ctrl+O");
    let wrap = CheckMenuItem::new(file, "Word wrap");
    wrap.set_checked(true);
    let mono = CheckMenuItem::new(file, "Monospace notes");
    MenuSeparator::new(file);
    let quit = MenuItem::new(file, "Quit");
    quit.set_accel("Ctrl+Q");
    quit.on_click(App::quit);

    let root = VBox::new(win);
    let status = Label::new(root, "Ready");
    let tabs = Tabs::new(root);
    tabs.set_expand(1.0);

    // Page 1: form in a grid
    let form = tabs.add_page("Form");
    let grid = Grid::new(form, 2);
    Label::new(grid, "Name");
    let name = TextInput::new(grid);
    name.set_expand(1.0);
    Label::new(grid, "Password");
    TextInput::password(grid);
    Label::new(grid, "Colour");
    let combo = ComboBox::new(grid);
    combo.set_items(&["Red", "Green", "Blue"]);
    combo.set_selected(Some(0));
    Label::new(grid, "Count");
    let spin = SpinBox::new(grid, 0.0, 10.0, 1.0);
    Label::new(grid, "Level");
    let slider = Slider::new(grid, 0.0, 100.0);
    let progress = ProgressBar::new(form);
    slider.on_change(move |v| progress.set_fraction(v / 100.0));
    spin.on_change(move |v| status.set_text(&format!("count = {v}")));
    combo.on_select(move |i| status.set_text(&format!("colour index {i:?}")));

    // Page 2: choices
    let choices = tabs.add_page("Choices");
    let boxed = GroupBox::new(choices, "Options");
    let group = RadioGroup::new();
    let a = RadioButton::new(boxed, &group, "Option A");
    RadioButton::new(boxed, &group, "Option B");
    a.set_checked(true);
    let check = CheckBox::new(boxed, "Enabled");
    check.set_checked(true);
    let list = ListBox::new(choices);
    list.set_items(&["alpha", "beta", "gamma", "delta"]);
    list.set_expand(1.0);
    list.on_activate(move |i| status.set_text(&format!("activated {i}")));

    // Page 3: text; a vertical splitter with notes on top and a fixed-pitch, unwrapped view below
    let notes = tabs.add_page("Notes");
    let vsplit = Splitter::new(notes, Orientation::Vertical);
    let area = TextArea::new(vsplit);
    let code = TextArea::new(vsplit);
    code.set_monospace(true);
    code.set_wrap(false);
    code.set_read_only(true);
    code.set_text(
        "fn main() {\n    println!(\"a long line that does not wrap: {}\", \"-\".repeat(60));\n}\n",
    );
    vsplit.set_position(120);
    vsplit.set_min_pane_sizes(40, 40);
    vsplit.on_move(move |p| status.set_text(&format!("notes split at {p}px")));
    wrap.on_toggle(move |on| area.set_wrap(on));
    mono.on_toggle(move |on| area.set_monospace(on));

    // Page 4: table, tree and a context menu
    let data = tabs.add_page("Data");
    let split = Splitter::new(data, Orientation::Horizontal);
    split.on_move(move |p| status.set_text(&format!("data split at {p}px")));
    let table = Table::new(split);
    table.set_columns(&[
        Column::new("Name").width(140).sortable(true),
        Column::new("Size")
            .width(70)
            .align(ColumnAlign::Right)
            .sortable(true),
    ]);
    table.set_rows(&[
        vec!["kernel", "9120"],
        vec!["README", "312"],
        vec!["Cargo.toml", "1024"],
    ]);
    table.on_select(move |r| status.set_text(&format!("row {r:?}")));
    table.on_activate(move |r| status.set_text(&format!("row {r} activated")));
    table.on_column_click(move |c| {
        // sorting is the app's job: reorder the rows, then show the arrow
        let ascending = table.sort_indicator() != Some((c, true));
        let mut rows = table.rows();
        rows.sort_by(|a, b| match c {
            1 => a[1]
                .parse::<u64>()
                .unwrap_or(0)
                .cmp(&b[1].parse::<u64>().unwrap_or(0)),
            _ => a[c].cmp(&b[c]),
        });
        if !ascending {
            rows.reverse();
        }
        table.set_rows(&rows);
        table.set_sort_indicator(Some((c, ascending)));
    });

    let tree = Tree::new(split);
    let src = tree.add(None, "src");
    tree.add(Some(src), "lib.rs");
    tree.add(Some(src), "core.rs");
    let docs = tree.add(None, "doc");
    tree.set_has_children(docs, true); // lazily filled when first expanded
    tree.set_expanded(src, true);
    tree.on_select(move |n| status.set_text(&format!("node {:?}", n.map(|n| tree.text(n)))));
    tree.on_expand(move |n, open| {
        if open && tree.children(Some(n)).is_empty() {
            tree.add(Some(n), "DESIGN.md");
            tree.add(Some(n), "BUILDING.md");
        }
    });

    // Page 5: a calendar, a multi-select list and a prompt
    let dates = tabs.add_page("Dates");
    let cal = Calendar::new(dates);
    cal.on_change(move |d| status.set_text(&format!("{}-{:02}-{:02}", d.year, d.month, d.day)));
    let many = ListBox::new(dates);
    many.set_items(&["one", "two", "three", "four"]);
    many.set_multi_select(true);
    many.set_expand(1.0);
    many.on_selection(move |rows| status.set_text(&format!("selected {rows:?}")));
    let ask = Button::new(dates, "Ask...");
    ask.on_click(move || {
        let r = prompt(Some(win), "Name", "What is your name?", "");
        status.set_text(&format!("name: {r:?}"));
    });

    // Right-click the table for a context menu; its items are rebuilt/enabled just before display.
    let popup = PopupMenu::new();
    let del = MenuItem::new(popup, "Delete row");
    MenuSeparator::new(popup);
    let hdr = CheckMenuItem::new(popup, "Show sizes");
    hdr.set_checked(true);
    table.set_context_menu(popup);
    table.on_context_menu(move |_, _| del.set_enabled(table.selected().is_some()));
    del.on_click(move || {
        if let Some(r) = table.selected() {
            table.remove_row(r);
        }
    });
    hdr.on_toggle(move |on| status.set_text(&format!("show sizes: {on}")));
    tree.set_context_menu(popup);

    open.on_click(move || {
        if let Some(p) = FileDialog::new()
            .title("Open")
            .filter("Text", &["txt", "md"])
            .open(Some(win))
        {
            if let Ok(s) = std::fs::read_to_string(&p) {
                area.set_text(&s);
            }
            status.set_text(&format!("opened {}", p.display()));
        }
    });

    // Buttons row
    let row = HBox::new(root);
    Spacer::new(row);
    let about = Button::new(row, "About");
    about.on_click(move || {
        message_box(
            Some(win),
            MessageKind::Info,
            Buttons::Ok,
            "About",
            "rungui kitchen sink",
        );
    });
    let ok = Button::new(row, "Close");
    ok.on_click(move || win.close());
    ok.set_a11y_description("Closes the window");

    let n = std::cell::Cell::new(0);
    Timer::every(1000, move || {
        n.set(n.get() + 1);
        status.set_text(&format!("uptime {}s", n.get()));
    });

    win.set_size(480, 400);
    win.show();
    app.run();
}
