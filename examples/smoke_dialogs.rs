//! Scripted smoke-test target for modal windows, prompt, Enter/Escape, multi-select and the
//! calendar (driven by scripts/smoke-dialogs.sh with xdotool; same stdout protocol as smoke_controls).
use rungui::*;

fn main() {
    let app = App::new("smoke-dialogs").expect("init");
    let win = Window::new("smoke-dialogs");
    let col = VBox::new(win);
    let field = TextInput::new(col);
    field.on_activate(|| println!("ACTIVATE"));
    let ask = Button::new(col, "Ask");
    ask.on_click(move || {
        let r = Prompt::new("Rename").message("New name?").initial("old").run(Some(win));
        println!("PROMPT {r:?}");
    });
    let modal = Button::new(col, "Modal");
    modal.on_click(move || {
        let d = Window::new("smoke-modal");
        Label::new(d, "modal window");
        let close = Button::new(d, "Close");
        close.on_click(move || d.hide());
        d.on_cancel(move || {
            println!("MODAL_CANCEL");
            d.hide();
        });
        println!("MODAL_OPEN");
        d.run_modal(Some(win));
        println!("MODAL_DONE");
        d.destroy();
    });
    let list = ListBox::new(col);
    list.set_items(&["one", "two", "three", "four", "five"]);
    list.set_multi_select(true);
    list.on_selection(|s| println!("LIST_SEL {s:?}"));
    let table = Table::new(col);
    table.set_columns(&[Column::new("Name").width(100)]);
    table.set_rows(&[vec!["a"], vec!["b"], vec!["c"], vec!["d"]]);
    table.set_multi_select(true);
    table.on_selection(|s| println!("TABLE_SEL {s:?}"));
    let cal = Calendar::new(col);
    cal.set_date(Date::new(2031, 7, 4).unwrap());
    cal.on_change(|d| println!("DATE {}-{:02}-{:02}", d.year, d.month, d.day));
    win.on_close(|| {
        App::quit();
        true
    });
    win.show();
    println!("TODAY {:?}", Date::today().is_valid());
    let _t = Timer::once(600, move || {
        for (n, w) in [
            ("field", field.bounds()),
            ("ask", ask.bounds()),
            ("modal", modal.bounds()),
            ("list", list.bounds()),
            ("table", table.bounds()),
            ("cal", cal.bounds()),
        ] {
            println!("BOUNDS {n} {} {} {} {}", w.x, w.y, w.w, w.h);
        }
        println!("READY");
    });
    let _exit = std::env::var("RUNGUI_EXIT_AFTER_MS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map(|ms| Timer::once(ms, App::quit));
    app.run();
    println!("BYE");
}
