//! Top-level windows.

use super::*;

impl Window {
    /// A hidden top-level window; add widgets to it and call [`Window::show`].
    pub fn new(title: &str) -> Window {
        Window::from_id(core::create(Kind::Window, None, |n| {
            n.text = title.to_string()
        }))
    }
    /// Change the title.
    pub fn set_title(&self, t: &str) {
        set_text(self.id(), t, false)
    }
    /// The title.
    pub fn title(&self) -> String {
        text_of(self.id())
    }
    /// Client-area size in logical pixels. Without this call the window sizes itself to its content.
    pub fn set_size(&self, w: i32, h: i32) {
        if core::update(self.id(), true, |n| {
            if let Some(win) = n.window_mut() {
                win.client = Size::new(w.clamp(1, MAX_WINDOW_PX), h.clamp(1, MAX_WINDOW_PX));
                win.explicit_size = true;
            }
        })
        .is_some()
        {
            core::layout_window(self.id());
        }
    }
    /// The client-area size: the last size set or reported by the user, else the size layout chose.
    pub fn size(&self) -> (i32, i32) {
        core::read(self.id(), |n| n.window().map(|w| (w.client.w, w.client.h)))
            .flatten()
            .unwrap_or((0, 0))
    }
    /// Allow or forbid resizing by the user.
    pub fn set_resizable(&self, v: bool) {
        core::set(
            self.id(),
            false,
            |n| {
                if let Some(w) = n.window_mut() {
                    w.resizable = v
                }
            },
            Prop::Resizable(v),
        );
    }
    /// Show the window (and lay it out).
    pub fn show(&self) {
        self.set_visible(true)
    }
    /// Hide the window.
    pub fn hide(&self) {
        self.set_visible(false)
    }
    /// Close handler: return `true` to allow closing (default), `false` to veto.
    pub fn on_close(&self, f: impl FnMut() -> bool + 'static) {
        core::modify(self.id(), |n| {
            if let Some(w) = n.window_mut() {
                w.on_close = Some(Box::new(f))
            }
        });
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
            |n| {
                if let Some(w) = n.window_mut() {
                    w.position = Some((x, y))
                }
            },
            Prop::Position { x, y },
        );
    }
    /// The last position set with [`Window::set_position`] or reported by the platform after the
    /// user moved the window; `None` if neither happened (the window manager placed it).
    pub fn position(&self) -> Option<(i32, i32)> {
        core::read(self.id(), |n| n.window().and_then(|w| w.position)).flatten()
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
    /// Called when the user presses Escape while this window is active, whichever of its widgets
    /// has the focus (unless that widget used the key itself, as an open drop-down does).
    pub fn on_cancel(&self, mut f: impl FnMut() + 'static) {
        on(self.id(), Ev::Cancel, move |_| f())
    }
    /// Show the window as a modal dialog over `parent` and wait: the application's other windows
    /// take no input, and this call returns only when the window is hidden ([`Window::hide`]),
    /// closed or destroyed, or the application quits. Events (including those of this window)
    /// are handled while it waits, so the window's own callbacks end the dialog, typically with
    /// `hide()` from a button's `on_click`. A closed window is destroyed; one that was only
    /// hidden can be run again. Called on a dead window, or on one that is already running
    /// modally, it does nothing.
    pub fn run_modal(&self, parent: Option<Window>) {
        core::run_modal(self.id(), parent.map(|p| p.id()))
    }
    /// Close now (no `on_close` check).
    pub fn close(&self) {
        self.destroy()
    }
}
