//! beets' `Html` transforms and the parts of BeautifulSoup the lyrics
//! backends use (`get_text`, `stripped_strings`, `select`), over `scraper`.

use fancy_regex::Regex as FRegex;
use scraper::{ElementRef, Html as Doc, Node, Selector};
use std::sync::LazyLock;

fn re(p: &str) -> FRegex {
    FRegex::new(p).unwrap()
}

macro_rules! transform {
    ($name:ident, $pat:expr, $rep:expr) => {
        pub fn $name(text: &str) -> String {
            static RE: LazyLock<FRegex> = LazyLock::new(|| re($pat));
            RE.replace_all(text, $rep).into_owned()
        }
    };
}

transform!(collapse_space, r"(?m)(^| ) +", "$1");
transform!(expand_br, r"(?i)\s*<br[^>]*>\s*", "\n");
// two newlines between paragraphs on the same line (musica, letras.mus.br)
transform!(merge_blocks, r"(?s)(?<!>)(</p><p[^>]*>)+", "\n\n");
// a single new line between paragraphs on separate lines
transform!(merge_lines, r"</p>\s+<p[^>]*>(?!___)", "\n");
transform!(remove_empty_tags, r"(<(div|span)[^>]*>\s*</\2>)", "");
transform!(remove_aside, r"<aside .+?</aside>", "");
transform!(
    remove_scripts,
    r"(?is)<script\b[^>]*>.*?</script\b[^>]*>",
    ""
);
transform!(remove_comments, r"(?s)<!--.*?-->", "");
transform!(
    remove_musica_title,
    r#"(?is)(<div\s+id=['"]letra['"][^>]*>.*?)<p>\s*<strong>[^<]+</strong>\s*</p>"#,
    "$1"
);
transform!(
    remove_significado,
    r#"(?is)<div\s+id=['"]significado['"][^>]*>.*"#,
    ""
);
transform!(
    remove_adslot,
    r"(?s)\n</div>[^\n]+-- Content_\d+ --.*?\n<div>",
    "\n"
);
transform!(remove_formatting, r" *</?(i|em|pre|strong)[^>]*>", "");

/// `html.unescape`.
pub fn unescape(text: &str) -> String {
    html_escape::decode_html_entities(text).into_owned()
}

pub fn normalize_space(text: &str) -> String {
    let text = unescape(text).replace('\r', "").replace('\u{a0}', " ");
    expand_br(&collapse_space(&text))
}

pub fn remove_ads(text: &str) -> String {
    remove_adslot(&remove_aside(text))
}

pub fn merge_paragraphs(text: &str) -> String {
    merge_blocks(&merge_lines(&remove_empty_tags(text)))
}

/// `SoupMixin.pre_process_html`.
pub fn pre_process(html: &str) -> String {
    remove_musica_title(&remove_scripts(&remove_significado(&normalize_space(html))))
}

/// `Google.pre_process_html`.
pub fn google_pre_process(html: &str) -> String {
    remove_formatting(&merge_paragraphs(&remove_comments(&remove_ads(
        &pre_process(html),
    ))))
}

/// A parsed page (`BeautifulSoup(html, "html.parser")`).
pub struct Soup(Doc);

impl Soup {
    pub fn parse(html: &str) -> Soup {
        Soup(Doc::parse_document(html))
    }

    /// The text of every string in the document, as BeautifulSoup's
    /// `get_text()`: script, style and template contents and comments
    /// aren't text.
    pub fn get_text(&self) -> String {
        strings(self.0.root_element()).concat()
    }

    /// `stripped_strings`: each string stripped, empty ones left out.
    pub fn stripped_strings(&self) -> Vec<String> {
        strings(self.0.root_element())
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn select(&self, selector: &str) -> Vec<ElementRef<'_>> {
        let sel = Selector::parse(selector).unwrap();
        self.0.select(&sel).collect()
    }
}

/// An element's strings, in document order (its `_all_strings()`).
pub fn strings<'a>(el: ElementRef<'a>) -> Vec<&'a str> {
    let mut out = vec![];
    for node in el.descendants() {
        if let Node::Text(t) = node.value() {
            let in_code = node.ancestors().any(|a| {
                a.value()
                    .as_element()
                    .is_some_and(|e| matches!(e.name(), "script" | "style" | "template"))
            });
            if !in_code {
                out.push(&**t);
            }
        }
    }
    out
}

pub fn element_text(el: ElementRef) -> String {
    strings(el).concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    // beets' TestHtml.
    #[test]
    fn strip_cruft() {
        let initial = "<!--lyrics below-->
                  &nbsp;one
                  <br class='myclass'>
                  two  !
                  <br><br \\>
                  <blink>four</blink>";
        assert_eq!(
            normalize_space(initial),
            "<!--lyrics below-->\none\ntwo !\n\n<blink>four</blink>"
        );
    }

    #[test]
    fn merge_paragraphs_as_beets() {
        assert_eq!(
            merge_paragraphs(r#"one</p><p class="myclass"></p><p>two</p><p>three"#),
            "one\n\ntwo\n\nthree"
        );
    }

    #[test]
    fn text_skips_scripts_and_comments() {
        let s = Soup::parse("<p>a<script>x()</script><!-- c -->b</p><style>p{}</style>");
        assert_eq!(s.get_text(), "ab");
        assert_eq!(
            Soup::parse("<p> x </p><p>  </p><p>yy</p>").stripped_strings(),
            ["x", "yy"]
        );
    }
}
