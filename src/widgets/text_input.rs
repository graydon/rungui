//! Single- and multi-line text entry.

use super::*;

macro_rules! text_methods {
    ($t:ident) => {
        impl $t {
            pub fn set_text(&self, t: &str) {
                set_text(self.id(), t, false)
            }
            pub fn text(&self) -> String {
                text_of(self.id())
            }
            /// Called with the new full text after each user edit.
            pub fn on_change(&self, mut f: impl FnMut(&str) + 'static) {
                on(self.id(), Ev::Text, move |e| {
                    if let Event::Text(s) = e {
                        f(s)
                    }
                })
            }
            pub fn set_read_only(&self, v: bool) {
                core::set(
                    self.id(),
                    false,
                    |n| {
                        if let Some(t) = n.text_data_mut() {
                            t.readonly = v
                        }
                    },
                    Prop::ReadOnly(v),
                );
            }
            /// Show the text in a fixed-pitch font (code, logs, hex dumps). Default off; ignored
            /// by backends that cannot change the font.
            pub fn set_monospace(&self, v: bool) {
                core::set(
                    self.id(),
                    true,
                    |n| {
                        if let Some(t) = n.text_data_mut() {
                            t.monospace = v
                        }
                    },
                    Prop::Monospace(v),
                );
            }
            pub fn monospace(&self) -> bool {
                core::read(self.id(), |n| n.text_data().is_some_and(|t| t.monospace))
                    .unwrap_or(false)
            }
        }
    };
}
text_methods!(TextInput);
text_methods!(TextArea);

impl TextInput {
    pub fn new(parent: impl Into<WidgetId>) -> TextInput {
        make(TextInput::from_id, Kind::TextInput, parent, |_| {})
    }
    /// Single-line input that masks its contents.
    pub fn password(parent: impl Into<WidgetId>) -> TextInput {
        make(TextInput::from_id, Kind::PasswordInput, parent, |_| {})
    }
    pub fn set_placeholder(&self, t: &str) {
        core::set(
            self.id(),
            false,
            |n| {
                if let Some(d) = n.text_data_mut() {
                    d.placeholder = t.to_string()
                }
            },
            Prop::Placeholder(t),
        );
    }
}

impl TextArea {
    pub fn new(parent: impl Into<WidgetId>) -> TextArea {
        make(TextArea::from_id, Kind::TextArea, parent, |_| {})
    }
    /// Soft-wrap long lines (default `true`); `false` scrolls horizontally instead.
    pub fn set_wrap(&self, v: bool) {
        core::set(
            self.id(),
            false,
            |n| {
                if let Some(d) = n.text_data_mut() {
                    d.wrap = v
                }
            },
            Prop::Wrap(v),
        );
    }
    pub fn wrap(&self) -> bool {
        core::read(self.id(), |n| n.text_data().is_some_and(|t| t.wrap)).unwrap_or(false)
    }
}
