use rungui::*;

fn main() {
    let app = App::new("hello").expect("init");
    let win = Window::new("Hello, rungui");
    let col = VBox::new(win);
    let label = Label::new(col, "Hello, wörld! こんにちは");
    let name = TextInput::new(col);
    name.set_placeholder("Your name");
    let button = Button::new(col, "Greet");
    button.on_click(move || label.set_text(&format!("Hello, {}!", name.text())));
    win.show();
    app.run();
}
