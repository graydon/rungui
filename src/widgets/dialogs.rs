//! Modal dialogs: message box and file dialogs.

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
