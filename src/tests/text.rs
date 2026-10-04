//! Property tests for the pure helpers in text.rs (index conversion, graphemes, wide strings,
//! mnemonics) over random strings built from awkward pieces: surrogate pairs, combining marks,
//! ZWJ sequences, flags, CRLF, Hangul, bidi controls, NULs and stray ampersands.

use crate::tests::fuzz_layout::Rng;
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

// ---------------------------------------------------------------- example-based unit tests

const SAMPLES: &[&str] = &[
    "",
    "abc",
    "héllo",
    "日本語",
    "😀x",
    "a😀b",
    "שלום",
    "مرحبا",
    "e\u{301}x",
    "👨‍👩‍👧‍👦!",
    "🇯🇵🇺🇸🇫",
];

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
    assert_eq!(
        grapheme_count("\u{1112}\u{1161}\u{11AB}"),
        1,
        "conjoining jamo"
    );
    assert_eq!(
        grapheme_count("a\u{200D}b"),
        2,
        "ZWJ joins only after pictographs"
    );
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
    assert_eq!(
        truncate_graphemes("e\u{301}e\u{301}e\u{301}", 2),
        "e\u{301}e\u{301}"
    );
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
    assert_eq!(
        unsafe { from_wide_ptr(to_wide_nul("日本").as_ptr()) },
        "日本"
    );
    assert_eq!(unsafe { from_wide_ptr(std::ptr::null()) }, "");
    assert_eq!(to_cstring("a\0b").to_bytes(), "a\u{FFFD}b".as_bytes());
    let c = to_cstring("日本");
    assert_eq!(unsafe { from_cptr(c.as_ptr()) }, "日本");
}

#[test]
fn mnemonics() {
    let m = parse_mnemonic("&File");
    assert_eq!(
        (m.text.as_str(), m.index, m.key()),
        ("File", Some(0), Some("F"))
    );
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
