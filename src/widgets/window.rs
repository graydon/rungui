//! Top-level windows.

use super::*;

impl Window {
    pub fn new(title: &str) -> Window {
        Window::from_id(core::create(Kind::Window, None, |n| {
            n.text = title.to_string()
        }))
    }
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, false)
    }
    pub fn title(&self) -> String {
        text_of(self.id())
    }
    /// Client-area size in logical pixels. Without this call the window sizes itself to its content.
    pub fn set_size(&self, w: i32, h: i32) {
        if core::update(self.id(), true, |n| {
            n.client = Size::new(w.clamp(1, MAX_PX), h.clamp(1, MAX_PX));
            n.explicit_size = true;
        })
        .is_some()
        {
            core::layout_window(self.id());
        }
    }
    pub fn size(&self) -> (i32, i32) {
        core::read(self.id(), |n| (n.client.w, n.client.h)).unwrap_or((0, 0))
    }
    pub fn set_resizable(&self, v: bool) {
        core::set(self.id(), false, |n| n.resizable = v, Prop::Resizable(v));
    }
    pub fn show(&self) {
        self.set_visible(true)
    }
    pub fn hide(&self) {
        self.set_visible(false)
    }
    /// Close handler: return `true` to allow closing (default), `false` to veto.
    pub fn on_close(&self, f: impl FnMut() -> bool + 'static) {
        core::update(self.id(), false, |n| n.on_close = Some(Box::new(f)));
    }
    /// Called with the new client size when the user resizes the window.
    pub fn on_resize(&self, mut f: impl FnMut(i32, i32) + 'static) {
        on(self.id(), Ev::Resized, move |e| {
            if let Event::Resized { w, h } = e {
                f(*w, *h)
            }
        });
    }
    /// Move the window's outer frame to screen position (x, y) in logical pixels. Some platforms
    /// (Wayland, some window managers) ignore this.
    pub fn set_position(&self, x: i32, y: i32) {
        core::set(
            self.id(),
            false,
            |n| n.position = Some((x, y)),
            Prop::Position { x, y },
        );
    }
    /// The last position set with [`Window::set_position`] or reported by the platform after the
    /// user moved the window; `None` if neither happened (the window manager placed it).
    pub fn position(&self) -> Option<(i32, i32)> {
        core::read(self.id(), |n| n.position).flatten()
    }
    /// Called with the new screen position when the user moves the window (not on every
    /// platform; see [`Window::position`]).
    pub fn on_move(&self, mut f: impl FnMut(i32, i32) + 'static) {
        on(self.id(), Ev::Moved, move |e| {
            if let Event::Moved { x, y } = e {
                f(*x, *y)
            }
        });
    }
    /// Close now (no `on_close` check).
    pub fn close(&self) {
        self.destroy()
    }
}
