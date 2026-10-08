//! Modal dialogs: message box, file dialogs and the text prompt.

use super::*;

fn pid(parent: Option<Window>) -> Option<WidgetId> {
    parent.map(|w| w.id()).filter(|i| core::is_alive(*i))
}

/// Modal message box; returns which button was pressed.
pub fn message_box(
    parent: Option<Window>,
    kind: MessageKind,
    buttons: Buttons,
    title: &str,
    text: &str,
) -> Answer {
    let spec = MessageSpec {
        kind,
        buttons,
        title: title.into(),
        text: text.into(),
    };
    B::message_box(pid(parent), &spec)
}

/// Builder for open/save/folder dialogs.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    title: String,
    filters: Vec<(String, Vec<String>)>,
    dir: Option<String>,
    name: Option<String>,
}

impl FileDialog {
    /// A dialog with no title, filters or starting location.
    pub fn new() -> FileDialog {
        FileDialog::default()
    }
    /// Set the dialog title.
    pub fn title(mut self, t: &str) -> Self {
        self.title = t.into();
        self
    }
    /// e.g. `.filter("Images", &["png", "jpg"])`.
    pub fn filter(mut self, label: &str, exts: &[&str]) -> Self {
        self.filters
            .push((label.into(), exts.iter().map(|e| e.to_string()).collect()));
        self
    }
    /// Start in this directory.
    pub fn directory(mut self, d: &str) -> Self {
        self.dir = Some(d.into());
        self
    }
    /// Suggest this file name (save dialogs).
    pub fn file_name(mut self, n: &str) -> Self {
        self.name = Some(n.into());
        self
    }
    fn run(&self, parent: Option<Window>, mode: FileMode) -> Vec<PathBuf> {
        let spec = FileSpec {
            mode,
            title: self.title.clone(),
            filters: self.filters.clone(),
            initial_dir: self.dir.clone(),
            initial_name: self.name.clone(),
        };
        B::file_dialog(pid(parent), &spec)
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    /// Choose one existing file; `None` when cancelled.
    pub fn open(&self, parent: Option<Window>) -> Option<PathBuf> {
        self.run(parent, FileMode::Open).into_iter().next()
    }
    /// Choose any number of existing files; empty when cancelled.
    pub fn open_many(&self, parent: Option<Window>) -> Vec<PathBuf> {
        self.run(parent, FileMode::OpenMany)
    }
    /// Choose where to save; `None` when cancelled.
    pub fn save(&self, parent: Option<Window>) -> Option<PathBuf> {
        self.run(parent, FileMode::Save).into_iter().next()
    }
    /// Choose a directory; `None` when cancelled.
    pub fn pick_folder(&self, parent: Option<Window>) -> Option<PathBuf> {
        self.run(parent, FileMode::PickFolder).into_iter().next()
    }
}

/// A modal dialog asking for one line of text. Built from ordinary widgets and run with
/// [`Window::run_modal`], so it looks the same everywhere and needs no backend support of its own.
#[derive(Clone, Debug)]
pub struct Prompt {
    title: String,
    message: String,
    initial: String,
    ok: String,
    cancel: String,
    password: bool,
}

impl Prompt {
    /// A prompt with this window title and "OK" / "Cancel" buttons.
    pub fn new(title: &str) -> Prompt {
        Prompt {
            title: title.into(),
            message: String::new(),
            initial: String::new(),
            ok: "OK".into(),
            cancel: "Cancel".into(),
            password: false,
        }
    }
    /// The question shown above the field.
    pub fn message(mut self, m: &str) -> Self {
        self.message = m.into();
        self
    }
    /// The text the field starts with.
    pub fn initial(mut self, t: &str) -> Self {
        self.initial = t.into();
        self
    }
    /// Captions of the two buttons (for other languages; `&` marks a mnemonic).
    pub fn buttons(mut self, ok: &str, cancel: &str) -> Self {
        self.ok = ok.into();
        self.cancel = cancel.into();
        self
    }
    /// Mask what is typed.
    pub fn password(mut self, v: bool) -> Self {
        self.password = v;
        self
    }
    /// Show the prompt over `parent` and wait. The typed text when the user pressed OK or Enter,
    /// `None` when they cancelled (Cancel, Escape or closing the window).
    pub fn run(&self, parent: Option<Window>) -> Option<String> {
        use std::{cell::RefCell, rc::Rc};
        let win = Window::new(&self.title);
        if !win.is_alive() {
            return None;
        }
        win.set_resizable(false);
        win.set_min_size(320, 0);
        let col = VBox::new(win);
        if !self.message.is_empty() {
            Label::new(col, &self.message);
        }
        let field = if self.password {
            TextInput::password(col)
        } else {
            TextInput::new(col)
        };
        field.set_text(&self.initial);
        let row = HBox::new(col);
        Spacer::new(row);
        let ok = Button::new(row, &self.ok);
        let cancel = Button::new(row, &self.cancel);
        let answer: Rc<RefCell<Option<String>>> = Rc::default();
        let accept = {
            let answer = answer.clone();
            move || {
                *answer.borrow_mut() = Some(field.text());
                win.hide();
            }
        };
        ok.on_click(accept.clone());
        field.on_activate(accept);
        cancel.on_click(move || win.hide());
        win.on_cancel(move || win.hide());
        field.focus();
        win.run_modal(parent);
        win.destroy();
        answer.take()
    }
}

/// Ask for one line of text in a modal dialog (see [`Prompt`] for buttons and password fields):
/// the text on OK or Enter, `None` on cancel.
pub fn prompt(parent: Option<Window>, title: &str, message: &str, initial: &str) -> Option<String> {
    Prompt::new(title)
        .message(message)
        .initial(initial)
        .run(parent)
}
