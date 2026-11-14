//! beets' `beets.util.lyrics.Lyrics`: lyrics text with its source and
//! translation metadata, and the text helpers the plugin shares.
//!
//! beets detects `language` and `translation_language` with `langdetect`
//! (an optional dependency) when it's installed; beets-rs ports langdetect
//! with the seed beets uses (0), so detection is deterministic.

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
/// `ORIGINAL_PAT`: the original text of a translated line, for detection.
static ORIGINAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\n]+ / ").unwrap());

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
    /// `instrumental` flag with no text, and languages are guessed.
    pub fn new(text: impl Into<String>, backend: Option<&str>, url: Option<&str>) -> Self {
        let mut l = Lyrics {
            text: text.into(),
            backend: backend.map(String::from),
            url: url.map(String::from),
            ..Default::default()
        };
        l.finalize();
        l
    }

    /// `Lyrics.from_item`: the item's lyrics and `lyrics_*` attributes.
    pub fn from_item(item: &Item) -> Self {
        let flex = |k: &str| item.flex.get(&format!("lyrics_{k}")).cloned();
        let mut l = Lyrics {
            text: item.str("lyrics").to_string(),
            backend: flex("backend"),
            url: flex("url"),
            instrumental: flex("instrumental")
                .and_then(|v| crate::model::parse_bool(&v))
                .unwrap_or(false),
            language: flex("language"),
            translation_language: flex("translation_language"),
            ..Default::default()
        };
        l.finalize();
        l
    }

    /// beets' `Lyrics.__post_init__`: handle the instrumental marker, then
    /// guess `language` (from the original text) and, when the text looks
    /// like translations, `translation_language`, with `langdetect`.
    fn finalize(&mut self) {
        if self.text == INSTRUMENTAL_LYRICS {
            self.instrumental = true;
            self.text.clear();
        }
        if self.text.is_empty() {
            return;
        }
        // beets tests the truthiness of these: an empty string is unset too.
        if self.language.as_deref().is_none_or(str::is_empty)
            && let Some(lang) = crate::langdetect::detect(&self.original_text())
        {
            self.language = Some(lang.to_uppercase());
        }
        if self.translation_language.as_deref().is_none_or(str::is_empty) {
            let lines = splitlines(&self.text);
            let with_delimiter = lines.iter().filter(|l| l.contains(" / ")).count();
            // We are confident we are dealing with translations.
            if with_delimiter as f64 >= lines.len() as f64 / 2.0
                && let Some(lang) = crate::langdetect::detect(&ORIGINAL.replace_all(&self.text, ""))
            {
                self.translation_language = Some(lang.to_uppercase());
            }
        }
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
    fn language_detection() {
        // beets' `Lyrics.__post_init__` with langdetect (seed 0).
        let l = Lyrics::new("sing in the rain every hour of the day", None, None);
        assert_eq!(l.language.as_deref(), Some("EN"));
        assert_eq!(l.translation_language, None);

        // Upstream's `test_from_legacy_text`: the original text is English and
        // the translations are French.
        let l = Lyrics::new(
            "[00:00.00] Some synced lyrics / Quelques paroles synchronisées\n\
             [00:00.50]\n\
             [00:01.00] Some more synced lyrics / Quelques paroles plus synchronisées",
            Some("lrclib"),
            Some("https://lrclib.net/api/1/"),
        );
        assert_eq!(l.language.as_deref(), Some("EN"));
        assert_eq!(l.translation_language.as_deref(), Some("FR"));

        // Short lines without a delimiter don't look like translations.
        let l = Lyrics::new("Hello\nworld", None, None);
        assert_eq!(l.language.as_deref(), Some("EN"));
        assert_eq!(l.translation_language, None);

        // Instrumental and featureless text have no language.
        let l = Lyrics::new("[Instrumental]", None, None);
        assert_eq!(l.language, None);
        let l = Lyrics::new("", None, None);
        assert_eq!(l.language, None);
        let l = Lyrics::new("12345 !!!", None, None);
        assert_eq!(l.language, None);

        // An item's stored language wins; `from_item` guesses the rest.
        let mut i = crate::model::Item::new_item();
        i.set_parsed("lyrics", "sing in the rain every hour of the day")
            .unwrap();
        i.set_str("lyrics_language", "XX");
        assert_eq!(Lyrics::from_item(&i).language.as_deref(), Some("XX"));
        i.remove_flex("lyrics_language");
        assert_eq!(Lyrics::from_item(&i).language.as_deref(), Some("EN"));

        // An empty stored language is unset, as beets' `if not
        // self.language` treats it: detection runs and replaces it.
        i.set_str("lyrics_language", "");
        i.set_parsed(
            "lyrics",
            "Some synced lyrics / Quelques paroles synchronisées\n\
             [00:01.00] More lyrics / Quelques paroles plus synchronisées",
        )
        .unwrap();
        i.set_str("lyrics_translation_language", "");
        let l = Lyrics::from_item(&i);
        assert_eq!(l.language.as_deref(), Some("EN"));
        assert_eq!(l.translation_language.as_deref(), Some("FR"));
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
