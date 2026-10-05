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
    /// Show `img`, or nothing for `None` or an image that is not [`ImageData::is_valid`].
    pub fn set_image(&self, img: Option<&ImageData>) {
        let img = img.filter(|i| i.is_valid());
        core::set(
            self.id(),
            true,
            |n| {
                if let core::NodeData::Image(i) = &mut n.data {
                    *i = img.cloned()
                }
            },
            Prop::Image(img),
        );
    }
}
