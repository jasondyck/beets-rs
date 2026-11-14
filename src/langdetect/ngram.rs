//! langdetect's `utils/ngram.py`: character normalization and the n-gram
//! window used to extract features.
//!
//! The normalization tables are langdetect's `utils/messages.properties`
//! (Apache-2.0), embedded verbatim and parsed once on first use.

use std::collections::HashMap;
use std::sync::OnceLock;

const MESSAGES: &str = include_str!("messages.properties");

/// `unicode_block()` results the normalizer distinguishes, by code-point
/// range (langdetect's `utils/unicode_block.py` table).
fn in_block(cp: u32, start: u32, end: u32) -> bool {
    (start..=end).contains(&cp)
}

const BASIC_LATIN: (u32, u32) = (0x0000, 0x007F);
const LATIN_1_SUPPLEMENT: (u32, u32) = (0x0080, 0x00FF);
const LATIN_EXTENDED_B: (u32, u32) = (0x0180, 0x024F);
const ARABIC: (u32, u32) = (0x0600, 0x06FF);
const LATIN_EXTENDED_ADDITIONAL: (u32, u32) = (0x1E00, 0x1EFF);
const GENERAL_PUNCTUATION: (u32, u32) = (0x2000, 0x206F);
const HIRAGANA: (u32, u32) = (0x3040, 0x309F);
const KATAKANA: (u32, u32) = (0x30A0, 0x30FF);
const BOPOMOFO: (u32, u32) = (0x3100, 0x312F);
const BOPOMOFO_EXTENDED: (u32, u32) = (0x31A0, 0x31BF);
const CJK_UNIFIED_IDEOGRAPHS: (u32, u32) = (0x4E00, 0x9FFF);
const HANGUL_SYLLABLES: (u32, u32) = (0xAC00, 0xD7AF);

struct Messages {
    latin1_excluded: String,
    /// `TO_NORMALIZE_VI_CHARS`.
    to_normalize_vi: Vec<char>,
    /// `NORMALIZED_VI_CHARS_0300` … `_0323`, by diacritical mark index.
    normalized_vi: [String; 5],
    /// `DMARK_CLASS`.
    dmark_class: Vec<char>,
    /// `NGram.KANJI_*` classes, in `CJK_CLASS` order.
    kanji: Vec<String>,
}

fn decode_unicode_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(cp) = u32::from_str_radix(&hex, 16)
                        && let Some(c) = char::from_u32(cp)
                    {
                        out.push(c);
                        continue;
                    }
                }
                Some('n') => {
                    out.push('\n');
                    continue;
                }
                Some('t') => {
                    out.push('\t');
                    continue;
                }
                Some('r') => {
                    out.push('\r');
                    continue;
                }
                Some(c) => {
                    out.push(c);
                    continue;
                }
                None => {}
            }
        }
        out.push(c);
    }
    out
}

fn messages() -> &'static Messages {
    static MESSAGES_ONCE: OnceLock<Messages> = OnceLock::new();
    MESSAGES_ONCE.get_or_init(|| {
        let mut values: Vec<(String, String)> = vec![];
        for line in MESSAGES.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some((key, value)) = line.trim().split_once('=') {
                values.push((key.to_string(), decode_unicode_escape(value)));
            }
        }
        let get = |name: &str| -> String {
            values
                .iter()
                .find(|(k, _)| k == name)
                .map_or_else(|| format!("!{name}!"), |(_, v)| v.clone())
        };
        let vi_keys = [
            "NORMALIZED_VI_CHARS_0300",
            "NORMALIZED_VI_CHARS_0301",
            "NORMALIZED_VI_CHARS_0303",
            "NORMALIZED_VI_CHARS_0309",
            "NORMALIZED_VI_CHARS_0323",
        ];
        Messages {
            latin1_excluded: get("NGram.LATIN1_EXCLUDE"),
            to_normalize_vi: get("TO_NORMALIZE_VI_CHARS").chars().collect(),
            normalized_vi: vi_keys.map(get),
            dmark_class: get("DMARK_CLASS").chars().collect(),
            kanji: values
                .iter()
                .filter(|(k, _)| k.starts_with("NGram.KANJI_"))
                .map(|(_, v)| v.clone())
                .collect(),
        }
    })
}

/// The Kanji-cluster map (`NGram._init_cjk_map`): each character of a
/// `KANJI_*` class maps to the class's first character.
fn cjk_map() -> &'static HashMap<char, char> {
    static CJK: OnceLock<HashMap<char, char>> = OnceLock::new();
    CJK.get_or_init(|| {
        let mut map = HashMap::new();
        for class in &messages().kanji {
            if let Some(representative) = class.chars().next() {
                for c in class.chars() {
                    map.insert(c, representative);
                }
            }
        }
        map
    })
}

/// `NGram.normalize_vi`: alphabet + combining diacritical mark (U+03xx) into
/// U+1Exx.
pub fn normalize_vi(text: &str) -> String {
    let m = messages();
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if let (Some(&next), Some(alphabet)) =
            (chars.peek(), m.to_normalize_vi.iter().position(|&v| v == c))
            && let Some(dmark) = m.dmark_class.iter().position(|&v| v == next)
        {
            chars.next();
            if let Some(replacement) = m.normalized_vi[dmark].chars().nth(alphabet) {
                out.push(replacement);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// `NGram.normalize(ch)`.
pub fn normalize(ch: char) -> char {
    let cp = ch as u32;
    if in_block(cp, BASIC_LATIN.0, BASIC_LATIN.1) {
        if ch < 'A' || ('Z' < ch && ch < 'a') || 'z' < ch {
            return ' ';
        }
        ch
    } else if in_block(cp, LATIN_1_SUPPLEMENT.0, LATIN_1_SUPPLEMENT.1) {
        if messages().latin1_excluded.contains(ch) {
            return ' ';
        }
        ch
    } else if in_block(cp, LATIN_EXTENDED_B.0, LATIN_EXTENDED_B.1) {
        // Romanian: s/t with comma below => with cedilla.
        match ch {
            '\u{0219}' => '\u{015f}',
            '\u{021b}' => '\u{0163}',
            _ => ch,
        }
    } else if in_block(cp, GENERAL_PUNCTUATION.0, GENERAL_PUNCTUATION.1) {
        ' '
    } else if in_block(cp, ARABIC.0, ARABIC.1) {
        // Farsi yeh => Arabic yeh.
        if ch == '\u{06cc}' { '\u{064a}' } else { ch }
    } else if in_block(cp, LATIN_EXTENDED_ADDITIONAL.0, LATIN_EXTENDED_ADDITIONAL.1) {
        if ch >= '\u{1ea0}' { '\u{1ec3}' } else { ch }
    } else if in_block(cp, HIRAGANA.0, HIRAGANA.1) {
        '\u{3042}'
    } else if in_block(cp, KATAKANA.0, KATAKANA.1) {
        '\u{30a2}'
    } else if in_block(cp, BOPOMOFO.0, BOPOMOFO.1)
        || in_block(cp, BOPOMOFO_EXTENDED.0, BOPOMOFO_EXTENDED.1)
    {
        '\u{3105}'
    } else if in_block(cp, CJK_UNIFIED_IDEOGRAPHS.0, CJK_UNIFIED_IDEOGRAPHS.1) {
        *cjk_map().get(&ch).unwrap_or(&ch)
    } else if in_block(cp, HANGUL_SYLLABLES.0, HANGUL_SYLLABLES.1) {
        '\u{ac00}'
    } else {
        ch
    }
}

/// `NGram`: the sliding window of up to three normalized characters.
pub struct NGram {
    grams: String,
    capitalword: bool,
}

impl NGram {
    /// `NGram.N_GRAM`.
    pub const MAX: usize = 3;

    pub fn new() -> Self {
        NGram {
            grams: " ".to_string(),
            capitalword: false,
        }
    }

    /// `add_char`: append a character, normalizing it first.
    pub fn add_char(&mut self, ch: char) {
        let ch = normalize(ch);
        let last_char = self.grams.chars().next_back().unwrap_or(' ');
        if last_char == ' ' {
            self.grams = " ".to_string();
            self.capitalword = false;
            if ch == ' ' {
                return;
            }
        } else if self.grams.chars().count() >= 3 {
            // Drop the first character.
            let mut chars = self.grams.chars();
            chars.next();
            self.grams = chars.collect();
        }
        self.grams.push(ch);

        if ch.is_uppercase() {
            if last_char.is_uppercase() {
                self.capitalword = true;
            }
        } else {
            self.capitalword = false;
        }
    }

    /// `get(n)`: the last *n* characters, or `None`.
    pub fn get(&self, n: usize) -> Option<&str> {
        if self.capitalword || !(1..=3).contains(&n) {
            return None;
        }
        let count = self.grams.chars().count();
        if count < n {
            return None;
        }
        if n == 1 && self.grams.ends_with(' ') {
            return None;
        }
        let start = self
            .grams
            .char_indices()
            .rev()
            .nth(n - 1)
            .map_or(0, |(i, _)| i);
        Some(&self.grams[start..])
    }

    pub fn capitalword(&self) -> bool {
        self.capitalword
    }
}

impl Default for NGram {
    fn default() -> Self {
        Self::new()
    }
}
