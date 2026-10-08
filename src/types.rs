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
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Rect {
    /// A rectangle from its position and size.
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
}

/// Width/height in logical pixels.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Size {
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Size {
    /// A size from its width and height.
    pub const fn new(w: i32, h: i32) -> Size {
        Size { w, h }
    }
}

/// A calendar date in the proleptic Gregorian calendar, years 1753 to 9999 (the range every
/// native calendar control supports).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Date {
    /// Year, 1753..=9999.
    pub year: i32,
    /// Month, 1..=12.
    pub month: u32,
    /// Day of the month, 1..=31 (as many as the month has).
    pub day: u32,
}

impl Date {
    /// `None` unless this is a real date in the supported years.
    pub fn new(year: i32, month: u32, day: u32) -> Option<Date> {
        let d = Date { year, month, day };
        d.is_valid().then_some(d)
    }
    /// Is this a real date in the supported years?
    pub fn is_valid(&self) -> bool {
        (1753..=9999).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=days_in_month(self.year, self.month)).contains(&self.day)
    }
    /// Today in the user's time zone.
    pub fn today() -> Date {
        crate::core::today()
    }
    /// The date of a Unix timestamp in UTC (the fallback for backends that cannot tell the local date).
    pub(crate) fn from_unix_utc(secs: i64) -> Date {
        // Howard Hinnant's civil-from-days
        let z = secs.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        Date { year: year.clamp(1753, 9999), month, day }
    }
}

/// How many days `month` of `year` has (0 for a month outside 1..=12).
pub(crate) fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
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
/// `std::result::Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Raw native object, for users who want to write platform-specific code.
/// The payload is the pointer value (GtkWidget*, HWND/HMENU, NSView*/NSWindow*/NSMenu*...).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum NativeHandle {
    /// A `GtkWidget*` (or `GtkMenuItem*` for menus).
    Gtk(usize),
    /// An `HWND` (or `HMENU` for menus).
    Win32(usize),
    /// An `NSView*`, `NSWindow*` or `NSMenu*`.
    Cocoa(usize),
}

impl NativeHandle {
    /// The raw pointer value.
    pub fn ptr(self) -> usize {
        match self {
            NativeHandle::Gtk(p) | NativeHandle::Win32(p) | NativeHandle::Cocoa(p) => p,
        }
    }
}

/// Cross-axis alignment of a child inside its stack/grid cell.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Align {
    /// Place at the start (left, or top) of the cell.
    Start,
    /// Center in the cell.
    Center,
    /// Place at the end (right, or bottom) of the cell.
    End,
    #[default]
    /// Stretch to fill the cell (the default).
    Fill,
}

/// Straight (non-premultiplied) RGBA8 pixels, row-major, `rgba.len() == w*h*4`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ImageData {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    /// `w * h * 4` bytes: red, green, blue, alpha per pixel.
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
/// Which icon a message box shows.
pub enum MessageKind {
    /// Information.
    Info,
    /// Warning.
    Warning,
    /// Error.
    Error,
    /// A question.
    Question,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
/// Which buttons a message box offers.
pub enum Buttons {
    /// Just "OK".
    Ok,
    /// "OK" and "Cancel".
    OkCancel,
    /// "Yes" and "No".
    YesNo,
    /// "Yes", "No" and "Cancel".
    YesNoCancel,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
/// The button a message box was answered with.
pub enum Answer {
    /// "OK".
    Ok,
    /// "Cancel" (also what closing the box counts as when it has one).
    Cancel,
    /// "Yes".
    Yes,
    /// "No".
    No,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
/// What a file dialog is for.
pub enum FileMode {
    /// Choose one existing file.
    Open,
    /// Choose several existing files.
    OpenMany,
    /// Choose where to save.
    Save,
    /// Choose a directory.
    PickFolder,
}

/// A modal message box request (title/text are UTF-8).
#[derive(Clone, Debug)]
pub struct MessageSpec {
    /// Which icon to show.
    pub kind: MessageKind,
    /// Which buttons to offer.
    pub buttons: Buttons,
    /// Window title.
    pub title: String,
    /// Message text.
    pub text: String,
}

/// A modal file dialog request. `filters` are (label, extensions without dot); empty = all files.
#[derive(Clone, Debug)]
pub struct FileSpec {
    /// What the dialog is for.
    pub mode: FileMode,
    /// Dialog title.
    pub title: String,
    /// (label, extensions) pairs; empty means all files.
    pub filters: Vec<(String, Vec<String>)>,
    /// Directory to start in.
    pub initial_dir: Option<String>,
    /// Suggested file name.
    pub initial_name: Option<String>,
}

/// Parsed keyboard accelerator such as "Ctrl+Shift+S". `ctrl` means the platform's primary
/// modifier (Command on macOS). `key` is an upper-case char or a named key ("F5", "Enter",
/// "Esc", "Del", "Tab", "Space", "Left", ...).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Accel {
    /// The platform's primary modifier (Command on macOS).
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// The key: an upper-case character or a name such as "F5" or "ENTER".
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
    /// Left aligned (the default).
    Left,
    /// Centered.
    Center,
    /// Right aligned.
    Right,
}

/// One column of a table. `width` is in logical pixels (initial width; the user may resize).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Column {
    /// Header text.
    pub title: String,
    /// Initial width in logical pixels.
    pub width: i32,
    /// Alignment of the cells.
    pub align: ColumnAlign,
    /// Header click emits `Event::ColumnClicked` (headers of non-sortable columns may still be
    /// clickable natively; the core forwards the event either way).
    pub sortable: bool,
}

impl Column {
    /// A left-aligned, 100 pixel wide, unsortable column with this header.
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
    /// Set the alignment of the cells.
    pub fn align(mut self, a: ColumnAlign) -> Self {
        self.align = a;
        self
    }
    /// Say whether clicking the header reports `on_column_click`.
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
    /// The node's text.
    pub text: String,
    /// Whether the node is expanded.
    pub expanded: bool,
    /// True if the node has children OR is flagged as lazily loadable (show an expander).
    pub has_children: bool,
}

/// Direction of a [`crate::Splitter`]: `Horizontal` puts the panes side by side (left | right,
/// mirrored under RTL) with a vertical sash between them; `Vertical` stacks them (top / bottom).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Orientation {
    #[default]
    /// Panes side by side (left | right), separated by a vertical sash.
    Horizontal,
    /// Panes stacked (top / bottom), separated by a horizontal sash.
    Vertical,
}
