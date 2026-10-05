//! Small plain-data types shared by the public API and the backends.

use std::fmt;

/// Symbolic, never-reused handle to a widget in the registry. `0` is the "dead" id.
/// Plain `u64` inside: `Copy`, `Send`, safe to keep after the widget is destroyed
/// (operations on stale ids are no-ops).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WidgetId(pub u64);

impl WidgetId {
    /// The id carried by handles whose creation failed.
    pub const DEAD: WidgetId = WidgetId(0);
}

/// Largest widget extent or coordinate pushed to a backend, in logical pixels. X11 and Win32 keep
/// window-system coordinates in 16 bits, so anything beyond this could not be shown anyway; larger
/// requests are clamped instead of overflowing layout arithmetic or tripping toolkit warnings.
pub(crate) const MAX_PX: i32 = i16::MAX as i32;
/// Largest window client size. Toolkits allocate a backing store for a window, so this is kept to
/// what a real display can have rather than the coordinate limit.
pub(crate) const MAX_WINDOW_PX: i32 = 1 << 14;

/// Rectangle in logical (DPI-independent) pixels; origin top-left, y grows downward.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
}

/// Width/height in logical pixels.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

impl Size {
    pub const fn new(w: i32, h: i32) -> Size {
        Size { w, h }
    }
}

/// Errors. Constructors never return these (they yield a dead handle and record the error,
/// see [`crate::last_error`]); fallible entry points such as `App::new` do.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Error {
    /// The platform backend does not implement this (or is a stub).
    Unsupported,
    /// `App::new` was already called on this thread.
    AlreadyInitialized,
    /// `App::new` has not been called on this thread (or wrong thread).
    NotInitialized,
    /// Stale or wrongly-typed id / parent.
    InvalidHandle,
    /// A built-in limit was hit, e.g. widgets nested deeper than [`crate::MAX_NESTING`].
    LimitExceeded,
    /// Backend-specific failure message.
    Backend(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported => write!(f, "not supported by this backend"),
            Error::AlreadyInitialized => write!(f, "rungui already initialized on this thread"),
            Error::NotInitialized => write!(f, "rungui not initialized on this thread"),
            Error::InvalidHandle => write!(f, "invalid or stale widget handle"),
            Error::LimitExceeded => write!(f, "widget nesting limit exceeded"),
            Error::Backend(s) => write!(f, "backend error: {s}"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Raw native object, for users who want to write platform-specific code.
/// The payload is the pointer value (GtkWidget*, HWND/HMENU, NSView*/NSWindow*/NSMenu*...).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum NativeHandle {
    Gtk(usize),
    Win32(usize),
    Cocoa(usize),
}

impl NativeHandle {
    pub fn ptr(self) -> usize {
        match self {
            NativeHandle::Gtk(p) | NativeHandle::Win32(p) | NativeHandle::Cocoa(p) => p,
        }
    }
}

/// Cross-axis alignment of a child inside its stack/grid cell.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Fill,
}

/// Straight (non-premultiplied) RGBA8 pixels, row-major, `rgba.len() == w*h*4`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ImageData {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Largest image edge accepted, in pixels. Keeps `w * h * 4` far from overflow on every target and
/// well inside what the toolkits can allocate.
pub const MAX_IMAGE_EDGE: u32 = 1 << 15;

impl ImageData {
    /// Is the pixel buffer exactly `w * h * 4` bytes, with both edges in `1..=MAX_IMAGE_EDGE`?
    /// Invalid images are shown as "no image" by every backend.
    pub fn is_valid(&self) -> bool {
        (1..=MAX_IMAGE_EDGE).contains(&self.w)
            && (1..=MAX_IMAGE_EDGE).contains(&self.h)
            && self.rgba.len() as u64 == u64::from(self.w) * u64::from(self.h) * 4
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum MessageKind {
    Info,
    Warning,
    Error,
    Question,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Buttons {
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Answer {
    Ok,
    Cancel,
    Yes,
    No,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FileMode {
    Open,
    OpenMany,
    Save,
    PickFolder,
}

/// A modal message box request (title/text are UTF-8).
#[derive(Clone, Debug)]
pub struct MessageSpec {
    pub kind: MessageKind,
    pub buttons: Buttons,
    pub title: String,
    pub text: String,
}

/// A modal file dialog request. `filters` are (label, extensions without dot); empty = all files.
#[derive(Clone, Debug)]
pub struct FileSpec {
    pub mode: FileMode,
    pub title: String,
    pub filters: Vec<(String, Vec<String>)>,
    pub initial_dir: Option<String>,
    pub initial_name: Option<String>,
}

/// Parsed keyboard accelerator such as "Ctrl+Shift+S". `ctrl` means the platform's primary
/// modifier (Command on macOS). `key` is an upper-case char or a named key ("F5", "Enter",
/// "Esc", "Del", "Tab", "Space", "Left", ...).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Accel {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: String,
}

impl Accel {
    /// Parse "Ctrl+Shift+S", "Alt+F4", "F5", "Cmd+Q" (Cmd/Meta/Super count as Ctrl). `None` if empty.
    pub fn parse(s: &str) -> Option<Accel> {
        let mut a = Accel::default();
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" | "meta" | "super" => a.ctrl = true,
                "shift" => a.shift = true,
                "alt" | "option" | "opt" => a.alt = true,
                _ => a.key = part.to_uppercase(),
            }
        }
        if a.key.is_empty() { None } else { Some(a) }
    }
}

/// Horizontal text alignment of a [`Column`].
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ColumnAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// One column of a table. `width` is in logical pixels (initial width; the user may resize).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Column {
    pub title: String,
    pub width: i32,
    pub align: ColumnAlign,
    /// Header click emits `Event::ColumnClicked` (headers of non-sortable columns may still be
    /// clickable natively; the core forwards the event either way).
    pub sortable: bool,
}

impl Column {
    pub fn new(title: &str) -> Column {
        Column {
            title: title.to_string(),
            width: 100,
            align: ColumnAlign::Left,
            sortable: false,
        }
    }
    /// Initial width in logical pixels, clamped to `1..=32767`.
    pub fn width(mut self, w: i32) -> Self {
        self.width = w.clamp(1, MAX_PX);
        self
    }
    pub fn align(mut self, a: ColumnAlign) -> Self {
        self.align = a;
        self
    }
    pub fn sortable(mut self, s: bool) -> Self {
        self.sortable = s;
        self
    }
}

/// Handle to a node of a [`crate::Tree`]. Globally unique, never reused; `0` is the dead id
/// returned when an insertion failed.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TreeNodeId(pub u64);

/// One flattened tree row as sent to the backend in `Prop::TreeRows`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TreeRow {
    /// Opaque node id (`TreeNodeId.0`); echoed back in tree events.
    pub node: u64,
    /// 0 for roots.
    pub depth: u32,
    pub text: String,
    pub expanded: bool,
    /// True if the node has children OR is flagged as lazily loadable (show an expander).
    pub has_children: bool,
}

/// Direction of a [`crate::Splitter`]: `Horizontal` puts the panes side by side (left | right,
/// mirrored under RTL) with a vertical sash between them; `Vertical` stacks them (top / bottom).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}
