//! beets' `beets.util.lyrics.Lyrics`: lyrics text with its source and
//! translation metadata, and the text helpers the plugin shares.
//!
//! beets detects `language` and `translation_language` with `langdetect`
//! when it's installed (an optional dependency); beets-rs doesn't, as
//! beets without it.

use crate::model::Item;
use regex::Regex;
use std::sync::LazyLock;

pub const INSTRUMENTAL_LYRICS: &str = "[Instrumental]";

/// `LINE_PARTS_PAT`: an LRC timestamp (or nothing) and the line's text.
static LINE_PARTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\[\d\d:\d\d\.\d{2,3}\]|) *(.*)$").unwrap());
static LRC_TIMESTAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d{2}):(\d{2})\.(\d{2,3})\]").unwrap());
static TRANSLATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" / [^\n]+").unwrap());

/// Python's `str.splitlines()`.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r'
                && let Some((_, '\n')) = chars.peek()
            {
                chars.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A line split into its LRC timestamp (or "") and its text.
pub fn line_parts(line: &str) -> (&str, &str) {
    // `.` doesn't match a newline; `splitlines` leaves none but "\r"-free
    // separators Python's regex treats as text, so this always matches.
    match LINE_PARTS.captures(line) {
        Some(c) => (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str()),
        None => ("", line),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    pub text: String,
    pub backend: Option<String>,
    pub url: Option<String>,
    pub instrumental: bool,
    pub language: Option<String>,
    pub translation_language: Option<String>,
    pub translations: Vec<String>,
}

impl Lyrics {
    /// `Lyrics(text, backend, url)`: the instrumental marker becomes the
    /// `instrumental` flag with no text.
    pub fn new(text: impl Into<String>, backend: Option<&str>, url: Option<&str>) -> Self {
        let mut l = Lyrics {
            text: text.into(),
            backend: backend.map(String::from),
            url: url.map(String::from),
            ..Default::default()
        };
        if l.text == INSTRUMENTAL_LYRICS {
            l.instrumental = true;
            l.text.clear();
        }
        l
    }

    /// `Lyrics.from_item`: the item's lyrics and `lyrics_*` attributes.
    pub fn from_item(item: &Item) -> Self {
        let flex = |k: &str| item.flex.get(&format!("lyrics_{k}")).cloned();
        let mut l = Lyrics::new(item.str("lyrics"), None, None);
        l.backend = flex("backend");
        l.url = flex("url");
        l.instrumental |= flex("instrumental")
            .and_then(|v| crate::model::parse_bool(&v))
            .unwrap_or(false);
        l.language = flex("language");
        l.translation_language = flex("translation_language");
        l
    }

    /// The text without translations.
    pub fn original_text(&self) -> String {
        TRANSLATION.replace_all(&self.text, "").trim().to_string()
    }

    fn split_lines(&self) -> Vec<(&str, &str)> {
        splitlines(&self.text).into_iter().map(line_parts).collect()
    }

    pub fn timestamps(&self) -> Vec<&str> {
        self.split_lines().into_iter().map(|(t, _)| t).collect()
    }

    pub fn text_lines(&self) -> Vec<&str> {
        self.split_lines().into_iter().map(|(_, l)| l).collect()
    }

    /// SYLT `(text, milliseconds)` pairs from the LRC timestamps; lines
    /// without one are left out.
    pub fn sylt(&self) -> Vec<(String, u32)> {
        self.split_lines()
            .into_iter()
            .filter(|(ts, _)| !ts.is_empty())
            .filter_map(|(ts, text)| {
                let c = LRC_TIMESTAMP.captures(ts)?;
                let m: u32 = c[1].parse().ok()?;
                let s: u32 = c[2].parse().ok()?;
                let frac: u32 = format!("{:0<3}", &c[3]).parse().ok()?;
                Some((text.to_string(), (m * 60 + s) * 1000 + frac))
            })
            .collect()
    }

    /// Whether the lyrics have LRC timestamps.
    pub fn synced(&self) -> bool {
        self.timestamps().iter().any(|t| !t.is_empty())
    }

    pub fn translated(&self) -> bool {
        self.translation_language
            .as_deref()
            .is_some_and(|l| !l.is_empty())
    }

    /// The text with translations merged in (" / " after each line).
    pub fn full_text(&self) -> String {
        if self.translations.is_empty() {
            return self.text.clone();
        }
        let parts = self.split_lines();
        parts
            .iter()
            .zip(&self.translations)
            .map(|((ts, line), tr)| {
                let mut texts: Vec<&str> = vec![];
                for t in [*line, tr.as_str()] {
                    if !t.is_empty() && !texts.contains(&t) {
                        texts.push(t);
                    }
                }
                let text = texts.join(" / ");
                [*ts, text.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // beets' TestLyricsSyltProperty.
    #[test]
    fn sylt() {
        let s = |t: &str| Lyrics::new(t, None, None).sylt();
        let v = |p: &[(&str, u32)]| -> Vec<(String, u32)> {
            p.iter().map(|(t, ms)| (t.to_string(), *ms)).collect()
        };
        assert_eq!(
            s("[00:01.00] line one\n[00:02.50] line two"),
            v(&[("line one", 1000), ("line two", 2500)])
        );
        assert_eq!(
            s("[01:30.50] one and a half minutes"),
            v(&[("one and a half minutes", 90500)])
        );
        assert_eq!(s("plain lyrics without timestamps"), v(&[]));
        assert_eq!(
            s("[00:00.00] timed\nuntimed line\n[00:05.00] timed again"),
            v(&[("timed", 0), ("timed again", 5000)])
        );
        assert_eq!(
            s("[00:00.00] \n[00:01.00] second"),
            v(&[("", 0), ("second", 1000)])
        );
        assert_eq!(s("[00:01.234] x"), v(&[("x", 1234)]));
    }

    #[test]
    fn instrumental_and_synced() {
        let l = Lyrics::new("[Instrumental]", Some("lrclib"), None);
        assert!(l.instrumental);
        assert_eq!(l.text, "");
        assert!(Lyrics::new("[00:00.00] a", None, None).synced());
        assert!(!Lyrics::new("a", None, None).synced());
    }

    #[test]
    fn full_text_merges_translations() {
        let mut l = Lyrics::new("[00:00.00] Hi\n[00:00.50]\n[00:01.00] Bye", None, None);
        l.translations = vec!["Salut".into(), "".into(), "Bye".into()];
        assert_eq!(
            l.full_text(),
            "[00:00.00] Hi / Salut\n[00:00.50]\n[00:01.00] Bye"
        );
        let l = Lyrics::new("a / b\nc / d", None, None);
        assert_eq!(l.original_text(), "a\nc");
    }

    #[test]
    fn python_splitlines() {
        assert_eq!(splitlines("a\r\nb\rc\n"), ["a", "b", "c"]);
        assert_eq!(splitlines("\n"), [""]);
        assert!(splitlines("").is_empty());
    }
}
