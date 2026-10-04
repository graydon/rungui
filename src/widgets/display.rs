//! Static display widgets: `Label` and `Image`.

use super::*;

impl Label {
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> Label {
        make(Label::from_id, Kind::Label, parent, |n| {
            n.text = text.to_string()
        })
    }
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    pub fn text(&self) -> String {
        text_of(self.id())
    }
}

impl Image {
    pub fn new(parent: impl Into<WidgetId>) -> Image {
        make(Image::from_id, Kind::Image, parent, |_| {})
    }
    pub fn set_image(&self, img: Option<&ImageData>) {
        core::set(
            self.id(),
            true,
            |n| n.image = img.cloned(),
            Prop::Image(img),
        );
    }
}
