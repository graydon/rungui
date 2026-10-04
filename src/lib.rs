//! # rungui
//!
//! A small portable GUI toolkit that is a thin layer over the native toolkits: GTK3 on Linux,
//! Win32 on Windows, AppKit on macOS. Each backend declares the platform's C API by hand; there
//! are no binding crates. Text is UTF-8 in Rust and converted at the boundary (UTF-16 on Win32,
//! `NSString` on Cocoa). Accessibility names and roles are derived by the core and set on the native controls.
//!
//! ## Tutorial
//!
//! 1. Create the [`App`] once, on the main thread.
//! 2. Create widgets with `Widget::new(parent, ..)`. Parents are windows or containers
//!    ([`VBox`], [`HBox`], [`Grid`], [`GroupBox`], [`Tabs`]/[`Page`], [`Splitter`]). Containers
//!    lay out their children; tune with [`Widget::set_expand`] and friends. A [`Splitter`] takes
//!    exactly two children (its panes) and lets the user drag the boundary between them.
//! 3. Attach closures with `on_click`, `on_change`, ... Handles are `Copy` (and `Send`), so just
//!    `move` them into closures.
//! 4. `win.show()` then `app.run()`.
//!
//! ```no_run
//! use rungui::*;
//! let app = App::new("hello").unwrap();
//! let win = Window::new("Hello");
//! let col = VBox::new(win);
//! let label = Label::new(col, "Hi");
//! Button::new(col, "Click").on_click(move || label.set_text("Clicked"));
//! win.show();
//! app.run();
//! ```
//!
//! A two-pane layout, e.g. a file browser with a tree on the left and a fixed-pitch preview:
//!
//! ```no_run
//! use rungui::*;
//! let app = App::new("panes").unwrap();
//! let win = Window::new("Panes");
//! win.set_size(640, 400);
//! win.set_min_size(320, 200);
//! let split = Splitter::new(win, Orientation::Horizontal);
//! let tree = Tree::new(split); // first child: left pane
//! let preview = TextArea::new(split); // second child: right pane
//! preview.set_monospace(true);
//! preview.set_wrap(false);
//! preview.set_read_only(true);
//! split.set_position(200); // width of the left pane in pixels
//! split.on_move(move |px| println!("left pane is now {px}px wide"));
//! tree.on_select(move |n| preview.set_text(&n.map(|n| tree.text(n)).unwrap_or_default()));
//! win.show();
//! app.run();
//! ```
//!
//! ## Things to know
//!
//! * Handles never panic. A stale or destroyed handle is inert: setters do nothing, getters
//!   return defaults. Creation errors are reported through [`last_error`].
//! * Only [`App::post`] and [`App::quit`] may be called from other threads.
//! * Panics inside callbacks are caught and do not unwind into the toolkit.
//! * For anything the portable API lacks, get the native object with `widget.native_handle()`
//!   and call the toolkit directly.
//! * Optional features (accessibility overrides, sort indicators, popup menus) degrade to
//!   no-ops on a backend that lacks them rather than failing.
//! * Build modes: default is the native backend for the target; `--features mock` is a headless
//!   in-memory backend for tests; `--features emulate-mac` builds the Cocoa backend on Linux
//!   against GNUstep.

mod link_keepalive;

// The mock backend has no native objects to annotate, so there only the tests read the resolver.
#[cfg_attr(all(feature = "mock", not(test)), allow(dead_code))]
mod a11y;
pub mod backend;
pub mod core;
mod layout;
pub mod text;
mod types;
mod widgets;

pub use a11y::{A11yProps, A11yRole};
pub use backend::{Event, Kind, Prop, SashKey};
pub use types::*;
pub use widgets::*;

/// Application object. Create exactly one per process, on the main thread, before any widget.
pub struct App(());

impl App {
    /// Initialise the toolkit. Must be called on the thread that will run the UI (the main thread
    /// on macOS).
    pub fn new(name: &str) -> Result<App> {
        core::init(name)?;
        Ok(App(()))
    }
    /// Run the event loop until [`App::quit`] (or the last window closes, see
    /// [`App::set_quit_on_last_close`]).
    pub fn run(self) {
        core::flush();
        <backend::Native as backend::Backend>::run();
    }
    /// Ask the loop to stop. Callable from any thread.
    pub fn quit() {
        core::post(<backend::Native as backend::Backend>::quit);
    }
    /// Run `f` on the UI thread. Callable from any thread; this (and `quit`) is the only
    /// thread-safe entry point. Widget handles are `Send`, so `post(move || label.set_text(..))` works.
    pub fn post(f: impl FnOnce() + Send + 'static) {
        core::post(f)
    }
    /// Default `true`.
    pub fn set_quit_on_last_close(v: bool) {
        core::set_quit_on_last_close(v)
    }
    /// Apply pending layout/accessibility updates immediately (normally done when the loop is idle).
    pub fn update() {
        core::drain_posted()
    }
}

/// The most recent creation/timer error on this thread, if any (clears it).
pub fn last_error() -> Option<Error> {
    core::take_error()
}

pub use types::Accel;

/// Mirror horizontal layout (HBox order, Grid columns, VBox cross-axis alignment) for
/// right-to-left locales. Process-wide; relayouts all windows. Default off.
pub fn set_rtl_layout(rtl: bool) {
    core::set_rtl(rtl)
}

#[cfg(test)]
mod tests;
