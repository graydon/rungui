//! Property tests for the pure helpers in text.rs (index conversion, graphemes, wide strings,
//! mnemonics) over random strings built from awkward pieces: surrogate pairs, combining marks,
//! ZWJ sequences, flags, CRLF, Hangul, bidi controls, NULs and stray ampersands.

use crate::tests_fuzz_layout::Rng;
use crate::text::*;

const PIECES: &[&str] = &[
    "a",
    "b",
    " ",
    "é",
    "e\u{301}",
    "日",
    "😀",
    "👍🏽",
    "👨\u{200D}👩\u{200D}👧",
    "🇯🇵",
    "🇺🇸",
    "\r\n",
    "\n",
    "\r",
    "\t",
    "\0",
    "\u{200D}",
    "\u{FE0F}",
    "\u{200F}",
    "\u{2068}",
    "\u{2069}",
    "שלום",
    "مرحبا",
    "한",
    "\u{1100}\u{1161}\u{11A8}",
    "&",
    "&&",
    "_",
    "\u{10FFFF}",
    "\u{FFFD}",
];

fn random_string(rng: &mut Rng) -> String {
    (0..rng.below(10))
        .map(|_| PIECES[rng.below(PIECES.len())])
        .collect()
}

#[test]
fn utf8_utf16_conversions_are_consistent() {
    let mut rng = Rng::new(11);
    for _ in 0..2000 {
        let s = random_string(&mut rng);
        let ulen = utf16_len(&s);
        assert_eq!(ulen, s.encode_utf16().count());
        // every byte offset, including garbage past the end
        for b in 0..=s.len() + 3 {
            let f = floor_boundary(&s, b);
            let c = ceil_boundary(&s, b);
            assert!(s.is_char_boundary(f) && s.is_char_boundary(c), "{s:?} {b}");
            assert!(f <= b.min(s.len()) && c >= b.min(s.len()) && c <= s.len());
            let u = utf8_to_utf16_clamped(&s, b);
            assert!(u <= ulen);
            assert_eq!(
                utf8_to_utf16(&s, b).is_some(),
                b <= s.len() && s.is_char_boundary(b)
            );
            assert!(byte_to_char(&s, b) <= s.chars().count());
        }
        // every UTF-16 offset, including past the end and inside surrogate pairs
        let units: Vec<u16> = s.encode_utf16().collect();
        for u in 0..=ulen + 3 {
            let b = utf16_to_utf8_clamped(&s, u);
            assert!(s.is_char_boundary(b));
            let exact = utf16_to_utf8(&s, u);
            let mid_pair = u < units.len()
                && u > 0
                && (0xDC00..0xE000).contains(&units[u])
                && (0xD800..0xDC00).contains(&units[u - 1]);
            assert_eq!(exact.is_some(), u <= ulen && !mid_pair, "{s:?} unit {u}");
            if let Some(e) = exact {
                assert_eq!(e, b);
                assert_eq!(utf8_to_utf16(&s, e), Some(u));
            }
        }
        for ch in 0..=s.chars().count() + 2 {
            let b = char_to_byte(&s, ch);
            assert!(s.is_char_boundary(b));
            if ch <= s.chars().count() {
                assert_eq!(byte_to_char(&s, b), ch);
            }
        }
    }
}

#[test]
fn grapheme_stepping_is_a_consistent_partition() {
    let mut rng = Rng::new(12);
    for _ in 0..2000 {
        let s = random_string(&mut rng);
        let gs: Vec<&str> = graphemes(&s).collect();
        assert_eq!(gs.concat(), s);
        assert!(gs.iter().all(|g| !g.is_empty()));
        assert_eq!(grapheme_count(&s), gs.len());
        let mut starts = vec![0];
        for g in &gs {
            starts.push(starts.last().unwrap() + g.len());
        }
        for w in starts.windows(2) {
            assert_eq!(next_grapheme(&s, w[0]), w[1], "{s:?}");
            assert_eq!(prev_grapheme(&s, w[1]), w[0], "{s:?}");
        }
        assert_eq!(next_grapheme(&s, s.len()), s.len());
        assert_eq!(prev_grapheme(&s, 0), 0);
        for b in 0..=s.len() + 2 {
            let f = floor_grapheme(&s, b);
            assert!(starts.contains(&f), "{s:?} {b} -> {f}");
            assert!(f <= b.min(s.len()));
            assert!(
                starts.iter().all(|x| *x <= f || *x > b.min(s.len())),
                "not the largest: {s:?} {b} {f}"
            );
            let n = next_grapheme(&s, b);
            assert!(n <= s.len() && s.is_char_boundary(n));
            if b < s.len() {
                assert!(n > floor_boundary(&s, b), "no progress: {s:?} {b}");
            }
            let p = prev_grapheme(&s, b);
            assert!(s.is_char_boundary(p) && p <= b.min(s.len()));
            if b > 0 {
                assert!(
                    p < floor_boundary(&s, b).max(1),
                    "no progress back: {s:?} {b}"
                );
            }
        }
        for k in 0..gs.len() + 2 {
            assert_eq!(
                truncate_graphemes(&s, k),
                gs.iter().take(k).copied().collect::<String>()
            );
        }
    }
}

#[test]
fn wide_and_c_string_conversions_never_lose_or_panic() {
    let mut rng = Rng::new(13);
    for _ in 0..2000 {
        let s = random_string(&mut rng);
        let w = to_wide_nul(&s);
        assert_eq!(w.last(), Some(&0));
        assert!(!w[..w.len() - 1].contains(&0), "interior NUL leaked");
        let expect = s.replace('\0', "\u{FFFD}");
        assert_eq!(from_wide_nul(&w), expect);
        assert_eq!(
            from_utf16(&w[..w.len() - 1]).as_deref(),
            Some(expect.as_str())
        );
        assert_eq!(unsafe { from_wide_ptr(w.as_ptr()) }, expect);
        let c = to_cstring(&s);
        assert_eq!(c.to_str().unwrap(), expect);
        assert_eq!(unsafe { from_cptr(c.as_ptr()) }, expect);
        // arbitrary u16 soup (lone surrogates): lossy decode never panics, strict decode agrees
        let soup: Vec<u16> = (0..rng.below(8))
            .map(|_| [0xD800u16, 0xDC00, 0x41, 0, 0xFFFF, 0xDBFF][rng.below(6)])
            .collect();
        let lossy = from_wide_nul(&soup);
        let end = soup.iter().position(|c| *c == 0).unwrap_or(soup.len());
        assert_eq!(
            from_utf16(&soup[..end]).is_some(),
            !lossy.contains('\u{FFFD}')
        );
    }
    assert_eq!(unsafe { from_wide_ptr(std::ptr::null()) }, "");
    assert_eq!(unsafe { from_cptr(std::ptr::null()) }, "");
}

/// What Win32 shows for a label with prefix processing: `&&` -> `&`, `&x` -> underlined x.
fn win32_interpret(s: &str) -> (String, Option<usize>) {
    let mut out = String::new();
    let mut idx = None;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '&' {
            match it.peek() {
                Some('&') => {
                    it.next();
                    out.push('&');
                }
                Some(_) => {
                    if idx.is_none() {
                        idx = Some(out.len());
                    }
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    (out, idx)
}

/// What GTK shows for a mnemonic label: `__` -> `_`, `_x` -> underlined x.
fn gtk_interpret(s: &str) -> (String, Option<usize>) {
    let mut out = String::new();
    let mut idx = None;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '_' {
            match it.peek() {
                Some('_') => {
                    it.next();
                    out.push('_');
                }
                Some(_) => {
                    if idx.is_none() {
                        idx = Some(out.len());
                    }
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    (out, idx)
}

#[test]
fn mnemonics_translate_consistently_across_platforms() {
    let mut rng = Rng::new(14);
    let alphabet = ["a", "b", "é", "😀", "&", "&", "&&", " ", "e\u{301}", "_"];
    for _ in 0..5000 {
        let s: String = (0..rng.below(8))
            .map(|_| alphabet[rng.below(alphabet.len())])
            .collect();
        let m = parse_mnemonic(&s);
        assert_eq!(strip_mnemonic(&s), m.text);
        if let Some(i) = m.index {
            assert!(i < m.text.len() && m.text.is_char_boundary(i), "{s:?}");
            assert!(m.key().is_some_and(|k| !k.is_empty()));
        } else {
            assert_eq!(m.key(), None);
        }
        // Win32: same text and same underlined character, and never more than one marker
        let w = to_win32_mnemonic(&s);
        assert_eq!(
            win32_interpret(&w),
            (m.text.clone(), m.index),
            "win32 of {s:?} = {w:?}"
        );
        // GTK: literal text always preserved (the mnemonic character itself can be unrepresentable
        // only when it is an underscore, so compare the index only when the key is not one)
        let g = to_gtk_mnemonic(&s);
        let (gt, gi) = gtk_interpret(&g);
        assert_eq!(gt, m.text, "gtk text of {s:?} = {g:?}");
        if m.key() != Some("_") && !m.text.starts_with("__") {
            assert_eq!(gi, m.index, "gtk mnemonic position of {s:?} = {g:?}");
        }
        // escaping makes any text literal under Win32 prefix processing
        assert_eq!(
            win32_interpret(&escape_win32_literal(&s)),
            (s.clone(), None)
        );
    }
}

#[test]
fn bidi_helpers_are_total() {
    let mut rng = Rng::new(15);
    for _ in 0..1000 {
        let s = random_string(&mut rng);
        let iso = isolate(&s);
        assert!(iso.starts_with('\u{2068}') && iso.ends_with('\u{2069}'));
        assert_eq!(
            iso.chars()
                .filter(|c| matches!(*c as u32, 0x2066..=0x2069))
                .count(),
            2,
            "payload isolates not stripped"
        );
        assert!(strip_bidi_controls(&s).chars().all(|c| !is_bidi_control(c)));
        let _ = first_strong(&s);
    }
}
