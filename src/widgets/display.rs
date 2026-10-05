//! Static display widgets: `Label` and `Image`.

use super::*;

impl Label {
    /// A static text label.
    pub fn new(parent: impl Into<WidgetId>, text: &str) -> Label {
        make(Label::from_id, Kind::Label, parent, |n| {
            n.text = text.to_string()
        })
    }
    /// Change the text.
    pub fn set_text(&self, t: &str) {
        set_text(self.id(), t, true)
    }
    /// The text.
    pub fn text(&self) -> String {
        text_of(self.id())
    }
}

impl Image {
    /// An image, initially empty; set it with [`Image::set_image`].
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
