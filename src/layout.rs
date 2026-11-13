//! Two-column change layout for the import display (beets'
//! `beets.util.layout`): text may carry colour codes, which take no width.

use crate::ui::{RESET_COLOR, color_len, color_split, uncolorize};
use regex::Regex;
use std::sync::LazyLock;

/// One side of a layout row: prefix, contents and suffix, and the width
/// it gets (None: share the line).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Side {
    pub prefix: String,
    pub contents: String,
    pub suffix: String,
    pub width: Option<usize>,
}

impl Side {
    pub fn new(
        prefix: impl Into<String>,
        contents: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Side {
        Side {
            prefix: prefix.into(),
            contents: contents.into(),
            suffix: suffix.into(),
            width: None,
        }
    }

    pub fn rendered(&self) -> String {
        format!("{}{}{}", self.prefix, self.contents, self.suffix)
    }

    pub fn prefix_width(&self) -> usize {
        color_len(&self.prefix)
    }

    pub fn suffix_width(&self) -> usize {
        color_len(&self.suffix)
    }

    pub fn rendered_width(&self) -> usize {
        color_len(&self.rendered())
    }
}

/// `count` spaces.
pub fn indent(count: usize) -> String {
    " ".repeat(count)
}

/// A run of coloured text: text before it, the escape(s), the text, the
/// reset, and plain text after it (beets' `ESC_TEXT_REGEX`).
static ESC_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<pretext>[^\x1b]*)(?P<esc>(?:\x1b\[[;0-9]*m)+)(?P<text>[^\x1b]+)(?P<reset>\x1b\[39;49;00m)(?P<posttext>[^\x1b]*)",
    )
    .unwrap()
});

/// The words of `s`, keeping each coloured word wrapped in its colour.
fn words(s: &str) -> Vec<String> {
    if uncolorize(s) == s {
        return s.split_whitespace().map(String::from).collect();
    }
    let mut words: Vec<String> = vec![];
    let mut prev_ends_with_space = true;
    for m in ESC_TEXT.captures_iter(s) {
        let (pretext, esc, text, posttext) = (&m["pretext"], &m["esc"], &m["text"], &m["posttext"]);
        let mut space_before_text;
        if !pretext.is_empty() {
            words.extend(pretext.split_whitespace().map(String::from));
            space_before_text = pretext.ends_with(' ');
        } else {
            space_before_text = prev_ends_with_space;
        }
        if text.starts_with(' ') {
            space_before_text = true;
        }
        let raw: Vec<&str> = text.split_whitespace().collect();
        if space_before_text || words.is_empty() {
            words.extend(raw.iter().map(|w| format!("{esc}{w}{RESET_COLOR}")));
        } else if !raw.is_empty() {
            if esc != RESET_COLOR {
                let last = words.last_mut().unwrap();
                *last += &format!("{esc}{}{RESET_COLOR}", raw[0]);
                words.extend(raw[1..].iter().map(|w| format!("{esc}{w}{RESET_COLOR}")));
            } else {
                *words.last_mut().unwrap() += raw[0];
                words.extend(raw[1..].iter().map(ToString::to_string));
            }
        }
        if !text.ends_with(' ') && !posttext.is_empty() && !posttext.starts_with(' ') {
            let mut post = posttext.split_whitespace();
            if let (Some(first), Some(last)) = (post.next(), words.last_mut()) {
                *last += first;
            }
            words.extend(post.map(String::from));
        } else {
            words.extend(posttext.split_whitespace().map(String::from));
        }
        prev_ends_with_space = if posttext.is_empty() {
            text.ends_with(' ')
        } else {
            posttext.ends_with(' ')
        };
    }
    words
}

/// beets' `split_into_lines`: break `s` at whitespace into lines of at
/// most `first_width` (the first) and `width` (the rest) visible
/// characters, cutting words that fit nowhere.
pub fn split_into_lines(s: &str, first_width: usize, width: usize) -> Vec<String> {
    let words = words(s);
    let mut result: Vec<String> = vec![];
    let mut next = String::new();
    let mut previous_fit = false;
    for (i, word) in words.iter().enumerate() {
        let pot = if i == 0 {
            word.clone()
        } else {
            format!("{next} {word}")
        };
        let fits_first = result.is_empty() && color_len(&pot) <= first_width;
        let fits_middle = !result.is_empty() && color_len(&pot) <= width;
        if fits_first || fits_middle {
            next = pot;
            previous_fit = true;
        } else if previous_fit {
            result.push(std::mem::take(&mut next));
            next.clone_from(word);
            previous_fit = color_len(&next) <= width;
        } else {
            let w = if result.is_empty() {
                first_width
            } else {
                width
            };
            let (this, rest) = if uncolorize(&pot) == pot {
                let at = pot.char_indices().nth(w).map_or(pot.len(), |(i, _)| i);
                (pot[..at].to_string(), pot[at..].to_string())
            } else {
                color_split(&pot, w)
            };
            result.push(this);
            next = rest;
            previous_fit = color_len(&next) <= width;
        }
    }
    result.push(next);
    result
}

/// beets' `get_column_layout`: left and right side by side, each wrapped
/// in its own column.
pub fn column_layout(
    indent_str: &str,
    left: &Side,
    right: &Side,
    max_width: usize,
    separator: &str,
) -> Vec<String> {
    let (mut left, mut right) = (left.clone(), right.clone());
    if left.width.is_none() || right.width.is_none() {
        let w = max_width.saturating_sub(indent_str.len() + separator.len()) / 2;
        left.width = Some(w);
        right.width = Some(w);
    }
    let (lw, rw) = (left.width.unwrap(), right.width.unwrap());
    let l_no_prefix = lw.saturating_sub(left.prefix_width());
    let left_split = split_into_lines(
        &left.contents,
        l_no_prefix.saturating_sub(left.suffix_width()),
        l_no_prefix,
    );
    let r_no_prefix = rw.saturating_sub(right.prefix_width());
    let right_split = split_into_lines(
        &right.contents,
        r_no_prefix.saturating_sub(right.suffix_width()),
        r_no_prefix,
    );
    let lines = left_split.len().max(right_split.len());
    let mut out = String::new();
    for i in 0..lines {
        out += indent_str;
        if i == 0 {
            out += &left.prefix;
        } else {
            out += &indent(left.prefix_width());
        }
        let left_len = match left_split.get(i) {
            Some(l) => {
                out += l;
                color_len(l)
            }
            None => 0,
        };
        let mut padding = lw.saturating_sub(left.prefix_width() + left_len);
        if i == 0 {
            padding = padding.saturating_sub(left.suffix_width());
        }
        out += &indent(padding);
        if i == 0 {
            out += &left.suffix;
            out += separator;
            out += &right.prefix;
        } else {
            out += &indent(separator.len());
            out += &indent(right.prefix_width());
        }
        let right_len = match right_split.get(i) {
            Some(r) => {
                out += r;
                color_len(r)
            }
            None => 0,
        };
        let mut padding = rw.saturating_sub(right.prefix_width() + right_len);
        if i == 0 {
            padding = padding.saturating_sub(right.suffix_width());
        }
        out += &indent(padding);
        if i == 0 {
            out += &right.suffix;
        }
        if i + 1 < lines {
            out.push('\n');
        }
    }
    vec![out]
}

/// beets' `get_newline_layout`: the right side on its own line under the
/// left when they don't fit, wrapped lines indented twice.
pub fn newline_layout(
    indent_str: &str,
    left: &Side,
    right: &Side,
    max_width: usize,
    separator: &str,
) -> Vec<String> {
    let w = max_width.saturating_sub(indent_str.len());
    let w2 = max_width.saturating_sub(2 * indent_str.len());
    let left_split = split_into_lines(&left.rendered(), w, w2);
    let right_split = split_into_lines(&right.rendered(), w.saturating_sub(separator.len()), w2);
    let mut out = vec![];
    for (i, line) in left_split.iter().enumerate() {
        if i == 0 {
            out.push(format!("{indent_str}{line}"));
        } else if !line.is_empty() {
            out.push(format!("{indent_str}{indent_str}{line}"));
        }
    }
    for (i, line) in right_split.iter().enumerate() {
        if i == 0 {
            out.push(format!("{indent_str}{separator}{line}"));
        } else if !line.is_empty() {
            out.push(format!("{indent_str}{indent_str}{line}"));
        }
    }
    out
}

/// beets' `get_layout_lines`: one line `left -> right` if it fits in
/// `max_width`, else the `ui.import.layout` layout (`column` or
/// `newline`).
pub fn layout_lines(
    indent_str: &str,
    left: &Side,
    right: &Side,
    max_width: usize,
    newline: bool,
) -> Vec<String> {
    let separator = if right.rendered().is_empty() {
        ""
    } else {
        " -> "
    };
    let first = format!(
        "{indent_str}{}{separator}{}",
        left.rendered(),
        right.rendered()
    );
    if color_len(&first) < max_width {
        vec![first]
    } else if newline {
        newline_layout(indent_str, left, right, max_width, separator)
    } else {
        column_layout(indent_str, left, right, max_width, separator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: &str = "\x1b[31m";

    #[test]
    fn splits_plain_text_at_words_and_cuts_long_words() {
        assert_eq!(
            split_into_lines("one two three four", 9, 9),
            ["one two", "three", "four"]
        );
        assert_eq!(
            split_into_lines("one two three", 3, 20),
            ["one", "two three"]
        );
        assert_eq!(split_into_lines("abcdefghij", 4, 4), ["abcd", "efghij"]);
        assert_eq!(split_into_lines("", 4, 4), [""]);
    }

    #[test]
    fn splits_coloured_text_keeping_colours_per_word() {
        let s = format!("plain {RED}red words{RESET_COLOR} end");
        let lines = split_into_lines(&s, 9, 9);
        assert_eq!(
            lines,
            [
                format!("plain {RED}red{RESET_COLOR}"),
                format!("{RED}words{RESET_COLOR} end"),
            ]
        );
        // A colour change mid-word stays one word.
        let s = format!("ab{RED}cd{RESET_COLOR}ef gh");
        assert_eq!(
            split_into_lines(&s, 6, 6),
            [format!("ab{RED}cd{RESET_COLOR}ef"), "gh".into()]
        );
    }

    #[test]
    fn short_rows_stay_on_one_line() {
        let l = Side::new("* ", "Title", " (3:00)");
        let r = Side::new("", "New title", " (3:01)");
        assert_eq!(
            layout_lines("  ", &l, &r, 80, false),
            ["  * Title (3:00) -> New title (3:01)"]
        );
        assert_eq!(
            layout_lines("  ", &l, &Side::default(), 80, false),
            ["  * Title (3:00)"]
        );
    }

    #[test]
    fn long_rows_use_columns_or_newlines() {
        let l = Side::new("* ", "a long old title here", " (1:00)");
        let r = Side::new("", "a long new title here", " (1:00)");
        let cols = layout_lines("  ", &l, &r, 40, false);
        assert_eq!(
            cols,
            [concat!(
                // First lines leave room for the suffix: 17 - 2 - 7 = 8 wide.
                "  * a long   (1:00) -> a long new (1:00)\n",
                "    old title here     title here       "
            )]
        );
        let nl = layout_lines("  ", &l, &r, 40, true);
        assert_eq!(
            nl,
            [
                "  * a long old title here (1:00)",
                "   -> a long new title here (1:00)",
            ]
        );
    }
}
