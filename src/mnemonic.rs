//! Keyboard mnemonics in labels: `&File` underlines `F`, `&&` is a literal ampersand.
//!
//! Mnemonics apply only to Button, CheckBox, RadioButton, GroupBox, Menu and MenuItem text. The
//! core passes text through verbatim; a backend calls [`to_gtk_mnemonic`] (GTK `_`-style),
//! [`to_win32_mnemonic`] (native `&`, safe against stray prefix handling) or [`strip_mnemonic`]
//! (Cocoa has no mnemonics) when pushing `Prop::Text` for those kinds. Accessible names always
//! use [`strip_mnemonic`].

/// Label without mnemonic markers (Cocoa, accessible names): the first single `&` is dropped
/// together with every later one, `&&` collapses to `&`, a trailing lone `&` is dropped.
pub fn strip_mnemonic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            out.push(c);
        } else if it.peek() == Some(&'&') {
            it.next();
            out.push('&');
        }
    }
    out
}

/// Translate to a toolkit's markup. `marker` is the character that introduces a mnemonic there
/// (`_` for GTK, `&` for Win32); `literal` is what a literal `&` becomes (`&` / `&&`); `escape_marker`
/// says whether a literal `marker` character in the text must be doubled (GTK's `_`).
/// Only the first single `&` keeps its meaning; later ones are removed so stray ampersands cannot
/// hide characters.
#[allow(dead_code)] // reached through the two functions below, each used by one backend
fn translate(s: &str, marker: char, literal: &str, escape_marker: bool) -> String {
    let mut out = String::with_capacity(s.len() + 1);
    let mut it = s.chars().peekable();
    let mut used = false;
    while let Some(c) = it.next() {
        match c {
            c if escape_marker && c == marker => {
                out.push(marker);
                out.push(marker);
            }
            '&' => match it.peek() {
                Some('&') => {
                    it.next();
                    out.push_str(literal);
                }
                Some(_) if !used => {
                    used = true;
                    out.push(marker);
                }
                _ => {}
            },
            c => out.push(c),
        }
    }
    out
}

/// GTK form: `_` marks the mnemonic, literal underscores are doubled.
#[allow(dead_code)] // used by the GTK backend only
pub fn to_gtk_mnemonic(s: &str) -> String {
    translate(s, '_', "&", true)
}

/// Win32 form: only the first single `&` keeps its meaning; surplus markers are removed so
/// stray ampersands cannot hide characters; `&&` stays `&&` (renders as `&`).
#[allow(dead_code)] // used by the Win32 backend only
pub fn to_win32_mnemonic(s: &str) -> String {
    translate(s, '&', "&&", false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fuzz_layout::Rng;

    /// What a toolkit shows for a label whose mnemonic marker is `marker`: a doubled marker is that
    /// character, a single one underlines the next character (`index` = its byte offset in the text).
    fn interpret(s: &str, marker: char) -> (String, Option<usize>) {
        let mut out = String::new();
        let mut index = None;
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c != marker {
                out.push(c);
                continue;
            }
            match it.peek() {
                Some(m) if *m == marker => {
                    it.next();
                    out.push(marker);
                }
                Some(_) => {
                    if index.is_none() {
                        index = Some(out.len());
                    }
                }
                None => {}
            }
        }
        (out, index)
    }

    #[test]
    fn examples() {
        assert_eq!(strip_mnemonic("&Open && close"), "Open & close");
        assert_eq!(strip_mnemonic("trailing &"), "trailing ");
        assert_eq!(strip_mnemonic("&a &b"), "a b");
        assert_eq!(strip_mnemonic("Fish && &Chips"), "Fish & Chips");
        assert_eq!(to_gtk_mnemonic("&File"), "_File");
        assert_eq!(to_gtk_mnemonic("snake_case && &x"), "snake__case & _x");
        assert_eq!(to_gtk_mnemonic("&a &b"), "_a b");
        assert_eq!(to_win32_mnemonic("&a &b && c &"), "&a b && c ");
        assert_eq!(to_win32_mnemonic("日本&語"), "日本&語");
    }

    #[test]
    fn translations_agree_with_each_other() {
        let mut rng = Rng::new(14);
        let alphabet = ["a", "b", "é", "😀", "&", "&", "&&", " ", "e\u{301}", "_"];
        for _ in 0..5000 {
            let s: String = (0..rng.below(8))
                .map(|_| alphabet[rng.below(alphabet.len())])
                .collect();
            let stripped = strip_mnemonic(&s);
            // Win32: the same text and the same underlined character as the `&` source
            let (wt, wi) = interpret(&to_win32_mnemonic(&s), '&');
            assert_eq!(wt, stripped, "win32 text of {s:?}");
            let (src_text, src_index) = interpret(&s, '&');
            assert_eq!((wt, wi), (src_text, src_index), "win32 of {s:?}");
            // GTK: the literal text is always preserved; the mnemonic position too unless the
            // underlined character is an underscore, which GTK cannot express
            let g = to_gtk_mnemonic(&s);
            let (gt, gi) = interpret(&g, '_');
            assert_eq!(gt, stripped, "gtk text of {s:?} = {g:?}");
            let key_is_underscore = src_index.is_some_and(|i| stripped[i..].starts_with('_'));
            if !key_is_underscore && !stripped.starts_with("__") {
                assert_eq!(gi, src_index, "gtk mnemonic position of {s:?} = {g:?}");
            }
        }
    }
}
