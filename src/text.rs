//! Unicode helpers shared by the core and the backends. Pure functions, no platform code.
//!
//! * Index conversion between UTF-8 byte offsets (what Rust uses), UTF-16 code-unit offsets
//!   (Win32, Cocoa `NSRange`, AT-SPI/UIA text ranges) and `char` offsets.
//! * Grapheme-cluster stepping (approximation of UAX #29 without tables: combining marks,
//!   variation selectors, emoji modifiers, ZWJ sequences, flags, CRLF, Hangul syllables), so
//!   cursor/backspace/truncation helpers never split what the user sees as one character.
//! * Bidi-neutral helpers: first-strong direction, stripping/isolating bidi controls.
//! * Conversion to NUL-terminated UTF-16 / UTF-8 without silent truncation at interior NULs.
//! * Mnemonics ("&File", "&&" for a literal ampersand) translated per platform.
//!
//! Mnemonics apply only to Button, CheckBox, RadioButton, GroupBox, Menu and MenuItem text. The
//! core passes text through verbatim; a backend calls [`to_gtk_mnemonic`] (GTK `_`-style),
//! [`to_win32_mnemonic`] (native `&`, safe against stray prefix handling) or [`strip_mnemonic`]
//! (Cocoa has no mnemonics) when pushing `Prop::Text` for those kinds. Accessible names always
//! use [`strip_mnemonic`].

use std::ffi::CString;

// ------------------------------------------------------------------ index conversion

/// Number of UTF-16 code units in `s`.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// UTF-8 byte offset -> UTF-16 offset. `None` if `byte` is out of range or inside a character.
pub fn utf8_to_utf16(s: &str, byte: usize) -> Option<usize> {
    if byte > s.len() || !s.is_char_boundary(byte) {
        return None;
    }
    Some(utf16_len(&s[..byte]))
}

/// UTF-16 offset -> UTF-8 byte offset. `None` if out of range or in the middle of a surrogate pair.
pub fn utf16_to_utf8(s: &str, unit: usize) -> Option<usize> {
    let mut u = 0;
    for (b, c) in s.char_indices() {
        if u == unit {
            return Some(b);
        }
        if u > unit {
            return None;
        }
        u += c.len_utf16();
    }
    if u == unit { Some(s.len()) } else { None }
}

/// Like [`utf8_to_utf16`] but never fails: out of range clamps to the end, a byte inside a
/// character snaps back to the start of that character.
pub fn utf8_to_utf16_clamped(s: &str, byte: usize) -> usize {
    utf16_len(&s[..floor_boundary(s, byte)])
}

/// Like [`utf16_to_utf8`] but never fails: out of range clamps to the end, an offset inside a
/// surrogate pair snaps back to the start of the pair.
pub fn utf16_to_utf8_clamped(s: &str, unit: usize) -> usize {
    let mut u = 0;
    for (b, c) in s.char_indices() {
        let n = c.len_utf16();
        if unit < u + n {
            return b;
        }
        u += n;
    }
    s.len()
}

/// Largest char boundary `<= byte` (clamped to `s.len()`).
pub fn floor_boundary(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Smallest char boundary `>= byte` (clamped to `s.len()`).
pub fn ceil_boundary(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    while !s.is_char_boundary(b) {
        b += 1;
    }
    b
}

/// UTF-8 byte offset -> `char` (scalar value) offset.
pub fn byte_to_char(s: &str, byte: usize) -> usize {
    s[..floor_boundary(s, byte)].chars().count()
}

/// `char` offset -> UTF-8 byte offset (clamped to the end).
pub fn char_to_byte(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(b, _)| b)
}

// ------------------------------------------------------------------ graphemes

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Gc {
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    Ri,
    Pict,
    L,
    V,
    T,
    Lv,
    Lvt,
    Other,
}

fn gc(c: char) -> Gc {
    let u = c as u32;
    match u {
        0x0D => Gc::Cr,
        0x0A => Gc::Lf,
        0x200D => Gc::Zwj,
        0..=0x1F | 0x7F..=0x9F | 0x2028 | 0x2029 | 0xFEFF | 0x200B | 0x200E | 0x200F | 0x202A..=0x202E
        | 0x2060..=0x206F => Gc::Control,
        0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5BD | 0x5BF | 0x5C1..=0x5C2 | 0x5C4..=0x5C5 | 0x5C7
        | 0x610..=0x61A | 0x64B..=0x65F | 0x670 | 0x6D6..=0x6DC | 0x6DF..=0x6E4 | 0x6E7..=0x6E8
        | 0x6EA..=0x6ED | 0x711 | 0x730..=0x74A | 0x7A6..=0x7B0 | 0x900..=0x903 | 0x93A..=0x93C
        | 0x93E..=0x94F | 0x951..=0x957 | 0x962..=0x963 | 0x981..=0x983 | 0x9BC | 0x9BE..=0x9CD
        | 0xA01..=0xA03 | 0xA3C..=0xA51 | 0xB01..=0xB03 | 0xB3C..=0xB57 | 0xBBE..=0xBCD | 0xC00..=0xC04
        | 0xC3E..=0xC56 | 0xCBC..=0xCD6 | 0xD00..=0xD03 | 0xD3B..=0xD4D | 0xE31 | 0xE34..=0xE3A
        | 0xE47..=0xE4E | 0xEB1 | 0xEB4..=0xEBC | 0xEC8..=0xECE | 0xF18..=0xF19 | 0xF35 | 0xF37 | 0xF39
        | 0xF71..=0xF84 | 0x102B..=0x103E | 0x135D..=0x135F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x200C | 0x20D0..=0x20FF | 0x302A..=0x302F | 0x3099..=0x309A | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F
        | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF => Gc::Extend,
        0x1F1E6..=0x1F1FF => Gc::Ri,
        0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA | 0x231A..=0x23FF | 0x24C2
        | 0x25AA..=0x25FE | 0x2600..=0x27BF | 0x2934..=0x2935 | 0x2B05..=0x2B55 | 0x3030 | 0x303D
        | 0x3297 | 0x3299 | 0x1F000..=0x1FAFF => Gc::Pict,
        0x1100..=0x115F | 0xA960..=0xA97C => Gc::L,
        0x1160..=0x11A7 | 0xD7B0..=0xD7C6 => Gc::V,
        0x11A8..=0x11FF | 0xD7CB..=0xD7FB => Gc::T,
        0xAC00..=0xD7A3 => {
            if (u - 0xAC00) % 28 == 0 { Gc::Lv } else { Gc::Lvt }
        }
        _ => Gc::Other,
    }
}

/// Is there a grapheme boundary between `a` and `b`? `prev_pict` / `ri_run` carry context.
fn breaks(a: Gc, b: Gc, pict_zwj: bool, ri_odd: bool) -> bool {
    use Gc::*;
    match (a, b) {
        (Cr, Lf) => false,
        (Cr | Lf | Control, _) | (_, Cr | Lf | Control) => true,
        (L, L | V | Lv | Lvt) | (Lv | V, V | T) | (Lvt | T, T) => false,
        (_, Extend | Zwj) => false,
        (Zwj, Pict) if pict_zwj => false,
        (Ri, Ri) if ri_odd => false,
        _ => true,
    }
}

/// Byte offset of the end of the grapheme cluster starting at `from` (`s.len()` at the end).
/// `from` is snapped down to a char boundary. Always makes progress.
pub fn next_grapheme(s: &str, from: usize) -> usize {
    let from = floor_boundary(s, from);
    let mut it = s[from..].char_indices();
    let Some((_, first)) = it.next() else { return s.len() };
    let mut prev = gc(first);
    let mut pict_seen = prev == Gc::Pict; // inside Pict Extend* ZWJ ?
    let mut pict_zwj = false;
    let mut ri = usize::from(prev == Gc::Ri);
    for (i, c) in it {
        let cur = gc(c);
        if breaks(prev, cur, pict_zwj, ri % 2 == 1) {
            return from + i;
        }
        match cur {
            Gc::Pict => {
                pict_seen = true;
                pict_zwj = false;
            }
            Gc::Extend => {}
            Gc::Zwj => pict_zwj = pict_seen,
            _ => {
                pict_seen = false;
                pict_zwj = false;
            }
        }
        ri = if cur == Gc::Ri { ri + 1 } else { 0 };
        prev = cur;
    }
    s.len()
}

/// Byte offset of the start of the grapheme cluster that ends at `from` (0 at the start).
pub fn prev_grapheme(s: &str, from: usize) -> usize {
    let from = floor_boundary(s, from);
    if from == 0 {
        return 0;
    }
    // clusters are short; scan forward from a safe restart point (a hard boundary) for exactness
    let mut start = 0;
    let head = &s[..from];
    // restart at the last control char / newline (a cluster of its own, or CR LF), bounded to keep
    // this cheap on long lines
    if let Some((i, c)) = head.char_indices().rev().find(|(_, c)| matches!(gc(*c), Gc::Control | Gc::Lf | Gc::Cr)) {
        start = if gc(c) == Gc::Lf && head[..i].ends_with('\r') { i - 1 } else { i };
    }
    let mut last = start;
    let mut p = start;
    while p < from {
        last = p;
        p = next_grapheme(s, p);
    }
    if p == from { last } else { floor_boundary(s, from - 1) }
}

/// Iterator over grapheme clusters.
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        if pos >= s.len() {
            return None;
        }
        let e = next_grapheme(s, pos);
        let g = &s[pos..e];
        pos = e;
        Some(g)
    })
}

pub fn grapheme_count(s: &str) -> usize {
    graphemes(s).count()
}

/// Snap a byte offset to the nearest grapheme boundary at or before it.
pub fn floor_grapheme(s: &str, byte: usize) -> usize {
    let b = floor_boundary(s, byte);
    let mut p = 0;
    loop {
        let n = next_grapheme(s, p);
        if n > b || n == p {
            return p;
        }
        p = n;
        if p == b {
            return p;
        }
    }
}

/// Keep at most `max` grapheme clusters.
pub fn truncate_graphemes(s: &str, max: usize) -> &str {
    let mut p = 0;
    for _ in 0..max {
        if p >= s.len() {
            break;
        }
        p = next_grapheme(s, p);
    }
    &s[..p.min(s.len())]
}

// ------------------------------------------------------------------ bidi

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Dir {
    Ltr,
    Rtl,
    /// No strongly directional character (digits, punctuation, empty).
    Neutral,
}

/// Is `c` a strong right-to-left character (Hebrew, Arabic, Syriac, Thaana, NKo, ... and the
/// presentation forms)?
pub fn is_rtl_char(c: char) -> bool {
    matches!(c as u32,
        0x590..=0x5FF | 0x600..=0x6FF | 0x700..=0x74F | 0x750..=0x77F | 0x780..=0x7BF | 0x7C0..=0x7FF
        | 0x800..=0x8FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
        && !c.is_numeric()
}

/// Direction of the first strong character (UBA rule P2/P3 without isolates handling).
pub fn first_strong(s: &str) -> Dir {
    for c in s.chars() {
        if is_rtl_char(c) {
            return Dir::Rtl;
        }
        if c.is_alphabetic() {
            return Dir::Ltr;
        }
    }
    Dir::Neutral
}

pub fn is_bidi_control(c: char) -> bool {
    matches!(c as u32, 0x200E | 0x200F | 0x061C | 0x202A..=0x202E | 0x2066..=0x2069)
}

/// Remove all bidi formatting characters (for comparison, accessible names, file names).
pub fn strip_bidi_controls(s: &str) -> String {
    s.chars().filter(|c| !is_bidi_control(*c)).collect()
}

/// Wrap untrusted text in a First-Strong Isolate so it cannot reorder surrounding text
/// (e.g. a file name inserted into a sentence). Idempotent for already-balanced input is not
/// attempted; call once at the point of composition.
pub fn isolate(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 6);
    out.push('\u{2068}');
    out.push_str(&strip_unbalanced_isolates(s));
    out.push('\u{2069}');
    out
}

fn strip_unbalanced_isolates(s: &str) -> String {
    // an embedded PDI could close our isolate early; drop isolate controls from the payload
    s.chars().filter(|c| !matches!(*c as u32, 0x2066..=0x2069)).collect()
}

// ------------------------------------------------------------------ platform string conversion

/// Replace NULs, which cannot be represented in C strings, by U+FFFD (instead of truncating).
fn no_nul(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('\0') { s.replace('\0', "\u{FFFD}").into() } else { s.into() }
}

/// UTF-16 with a trailing NUL, for Win32 `W` functions. Interior NULs become U+FFFD.
pub fn to_wide_nul(s: &str) -> Vec<u16> {
    no_nul(s).encode_utf16().chain(std::iter::once(0)).collect()
}

/// Strict decode (lone surrogates -> `None`).
pub fn from_utf16(w: &[u16]) -> Option<String> {
    String::from_utf16(w).ok()
}

/// Decode UTF-16 up to the first NUL; lone surrogates become U+FFFD (the only lossy case, and
/// unavoidable: Rust strings cannot hold them).
pub fn from_wide_nul(w: &[u16]) -> String {
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Decode a NUL-terminated UTF-16 buffer from a raw pointer.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated `u16` sequence.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

/// C string for GTK/Cocoa APIs. Interior NULs become U+FFFD.
pub fn to_cstring(s: &str) -> CString {
    CString::new(no_nul(s).as_bytes()).unwrap_or_default()
}

/// Decode a NUL-terminated UTF-8 C string; invalid sequences become U+FFFD.
///
/// # Safety
/// `p` must be null or a valid NUL-terminated C string.
pub unsafe fn from_cptr(p: *const std::ffi::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

// ------------------------------------------------------------------ mnemonics

/// A parsed "&File"-style label.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mnemonic {
    /// Label with markers removed and `&&` collapsed to `&`.
    pub text: String,
    /// Byte offset in `text` of the character to underline.
    pub index: Option<usize>,
}

impl Mnemonic {
    /// The mnemonic character (the full grapheme cluster that gets underlined).
    pub fn key(&self) -> Option<&str> {
        let i = self.index?;
        Some(&self.text[i..next_grapheme(&self.text, i)])
    }
}

/// Parse `&X` (first single `&` wins; later single `&` are dropped; `&&` is a literal `&`; a
/// trailing lone `&` is dropped).
pub fn parse_mnemonic(s: &str) -> Mnemonic {
    let mut text = String::with_capacity(s.len());
    let mut index = None;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            text.push(c);
            continue;
        }
        match it.peek() {
            Some('&') => {
                it.next();
                text.push('&');
            }
            Some(_) => {
                if index.is_none() {
                    index = Some(text.len());
                }
            }
            None => {}
        }
    }
    Mnemonic { text, index }
}

/// Label without mnemonic markers (Cocoa, accessible names).
pub fn strip_mnemonic(s: &str) -> String {
    parse_mnemonic(s).text
}

/// GTK form: `_` marks the mnemonic, literal underscores are doubled.
pub fn to_gtk_mnemonic(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 1);
    let mut it = s.chars().peekable();
    let mut used = false;
    while let Some(c) = it.next() {
        match c {
            '_' => out.push_str("__"),
            '&' => match it.peek() {
                Some('&') => {
                    it.next();
                    out.push('&');
                }
                Some(_) if !used => {
                    used = true;
                    out.push('_');
                }
                _ => {}
            },
            c => out.push(c),
        }
    }
    out
}

/// Win32 form: only the first single `&` keeps its meaning; surplus markers are removed so
/// stray ampersands cannot hide characters; `&&` stays `&&` (renders as `&`).
pub fn to_win32_mnemonic(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 1);
    let mut it = s.chars().peekable();
    let mut used = false;
    while let Some(c) = it.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('&') => {
                it.next();
                out.push_str("&&");
            }
            Some(_) if !used => {
                used = true;
                out.push('&');
            }
            _ => {}
        }
    }
    out
}

/// Escape plain text so it displays literally when fed to Win32 controls with prefix processing.
pub fn escape_win32_literal(s: &str) -> String {
    s.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: &[&str] = &["", "abc", "héllo", "日本語", "😀x", "a😀b", "שלום", "مرحبا", "e\u{301}x", "👨‍👩‍👧‍👦!", "🇯🇵🇺🇸🇫"];

    #[test]
    fn index_roundtrips() {
        for s in SAMPLES {
            for (b, _) in s.char_indices().chain(std::iter::once((s.len(), ' '))) {
                let u = utf8_to_utf16(s, b).unwrap();
                assert_eq!(utf16_to_utf8(s, u), Some(b), "{s:?} {b}");
                assert_eq!(utf8_to_utf16_clamped(s, b), u);
                assert_eq!(char_to_byte(s, byte_to_char(s, b)), b);
            }
            assert_eq!(utf16_len(s), s.encode_utf16().count());
        }
    }

    #[test]
    fn invalid_offsets() {
        assert_eq!(utf8_to_utf16("é", 1), None);
        assert_eq!(utf8_to_utf16("é", 3), None);
        assert_eq!(utf16_to_utf8("😀", 1), None, "inside a surrogate pair");
        assert_eq!(utf16_to_utf8("😀", 2), Some(4));
        assert_eq!(utf16_to_utf8("😀", 3), None);
        assert_eq!(utf16_to_utf8_clamped("a😀", 2), 1);
        assert_eq!(utf16_to_utf8_clamped("a😀", 99), 5);
        assert_eq!(utf8_to_utf16_clamped("a😀", 3), 1);
        assert_eq!(utf8_to_utf16_clamped("a😀", 99), 3);
        assert_eq!(floor_boundary("日", 2), 0);
        assert_eq!(ceil_boundary("日", 1), 3);
    }

    #[test]
    fn cjk_and_emoji_lengths() {
        assert_eq!(utf16_len("日本語"), 3);
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf8_to_utf16("日本語", 6), Some(2));
    }

    #[test]
    fn grapheme_clusters() {
        assert_eq!(grapheme_count("e\u{301}x"), 2);
        assert_eq!(grapheme_count("👨‍👩‍👧‍👦!"), 2);
        assert_eq!(grapheme_count("🇯🇵🇺🇸🇫"), 3, "flags pair up, odd one alone");
        assert_eq!(grapheme_count("a\r\nb"), 3);
        assert_eq!(grapheme_count("👍🏽"), 1);
        assert_eq!(grapheme_count("❤\u{FE0F}"), 1);
        assert_eq!(grapheme_count("שָׁלוֹם"), 4);
        assert_eq!(grapheme_count("日本語"), 3);
        assert_eq!(grapheme_count("한국어"), 3);
        assert_eq!(grapheme_count("\u{1112}\u{1161}\u{11AB}"), 1, "conjoining jamo");
        assert_eq!(grapheme_count("a\u{200D}b"), 2, "ZWJ joins only after pictographs");
    }

    #[test]
    fn grapheme_stepping_is_consistent() {
        for s in SAMPLES {
            let mut fwd = vec![0];
            let mut p = 0;
            while p < s.len() {
                p = next_grapheme(s, p);
                fwd.push(p);
            }
            let mut back = vec![s.len()];
            let mut p = s.len();
            while p > 0 {
                p = prev_grapheme(s, p);
                back.push(p);
            }
            back.reverse();
            assert_eq!(fwd, back, "{s:?}");
            for b in 0..=s.len() {
                let f = floor_grapheme(s, b);
                assert!(f <= b && fwd.contains(&f));
            }
        }
        assert_eq!(next_grapheme("", 0), 0);
        assert_eq!(prev_grapheme("abc", 0), 0);
        assert_eq!(next_grapheme("é", 1), 2, "snaps to boundary and progresses");
    }

    #[test]
    fn truncation_never_splits() {
        assert_eq!(truncate_graphemes("e\u{301}e\u{301}e\u{301}", 2), "e\u{301}e\u{301}");
        assert_eq!(truncate_graphemes("👨‍👩‍👧‍👦👨‍👩‍👧‍👦", 1), "👨‍👩‍👧‍👦");
        assert_eq!(truncate_graphemes("ab", 10), "ab");
        assert_eq!(truncate_graphemes("ab", 0), "");
    }

    #[test]
    fn bidi() {
        assert_eq!(first_strong("שלום world"), Dir::Rtl);
        assert_eq!(first_strong("123 مرحبا"), Dir::Rtl);
        assert_eq!(first_strong("  hi"), Dir::Ltr);
        assert_eq!(first_strong("123 !?"), Dir::Neutral);
        assert_eq!(first_strong(""), Dir::Neutral);
        assert_eq!(strip_bidi_controls("a\u{200F}b\u{202E}c\u{2067}"), "abc");
        assert_eq!(isolate("x\u{2069}y"), "\u{2068}xy\u{2069}");
    }

    #[test]
    fn wide_and_c_strings() {
        assert_eq!(to_wide_nul("a😀"), vec![0x61, 0xD83D, 0xDE00, 0]);
        assert_eq!(to_wide_nul("a\0b"), vec![0x61, 0xFFFD, 0x62, 0]);
        assert_eq!(from_wide_nul(&[0x61, 0xD83D, 0xDE00, 0, 0x62]), "a😀");
        assert_eq!(from_wide_nul(&[0xD800, 0x62]), "\u{FFFD}b");
        assert_eq!(from_utf16(&[0xD800]), None);
        assert_eq!(unsafe { from_wide_ptr(to_wide_nul("日本").as_ptr()) }, "日本");
        assert_eq!(unsafe { from_wide_ptr(std::ptr::null()) }, "");
        assert_eq!(to_cstring("a\0b").to_bytes(), "a\u{FFFD}b".as_bytes());
        let c = to_cstring("日本");
        assert_eq!(unsafe { from_cptr(c.as_ptr()) }, "日本");
    }

    #[test]
    fn mnemonics() {
        let m = parse_mnemonic("&File");
        assert_eq!((m.text.as_str(), m.index, m.key()), ("File", Some(0), Some("F")));
        let m = parse_mnemonic("Save &As...");
        assert_eq!((m.text.as_str(), m.index), ("Save As...", Some(5)));
        assert_eq!(parse_mnemonic("Fish && &Chips").text, "Fish & Chips");
        assert_eq!(parse_mnemonic("Fish && &Chips").key(), Some("C"));
        assert_eq!(parse_mnemonic("trailing &").text, "trailing ");
        assert_eq!(parse_mnemonic("&a &b").text, "a b");
        assert_eq!(parse_mnemonic("&a &b").index, Some(0));
        assert_eq!(parse_mnemonic("日本&語").key(), Some("語"));
        assert_eq!(parse_mnemonic("&e\u{301}").key(), Some("e\u{301}"));
        assert_eq!(strip_mnemonic("&Open && close"), "Open & close");
        assert_eq!(to_gtk_mnemonic("&File"), "_File");
        assert_eq!(to_gtk_mnemonic("snake_case && &x"), "snake__case & _x");
        assert_eq!(to_gtk_mnemonic("&a &b"), "_a b");
        assert_eq!(to_win32_mnemonic("&a &b && c &"), "&a b && c ");
        assert_eq!(escape_win32_literal("R&D"), "R&&D");
    }
}
