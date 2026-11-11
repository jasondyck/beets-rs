//! beets' templates (`beets/util/functemplate.py`): `$field`, `${field}`
//! and `%func{arg,...}`, with the default template functions of
//! `beets/library/models.py` (`DefaultTemplateFunctions`).
//!
//! The parser is a port of beets' ad-hoc left-to-right scanner, quirks
//! included: `$` escapes `$`, `%`, `}` and `,`; a `}` outside a function
//! call ends the template and the rest is literal text; an unterminated
//! call is literal text; unknown symbols and functions stay verbatim.

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone)]
enum Part {
    Text(String),
    Symbol {
        ident: String,
        original: String,
    },
    Call {
        ident: String,
        args: Vec<Vec<Part>>,
        original: String,
    },
}

#[derive(Debug, Clone)]
pub struct Template {
    parts: Vec<Part>,
}

/// What a template is evaluated against: formatted field values and any
/// functions beyond the built-in ones (library-dependent and plugin ones).
pub trait Context {
    /// The formatted value of a field; None if there is no such field (the
    /// symbol is then left as written).
    fn field(&self, name: &str) -> Option<String>;

    /// `%ifdef{field}`: the field's (non-path) formatted value if the
    /// object has the field.
    fn ifdef(&self, name: &str) -> Option<String> {
        self.field(name)
    }

    /// A function the context provides (e.g. `%aunique`), called with the
    /// evaluated arguments; None if it doesn't provide `name`. Errors are
    /// rendered as `<message>`, as beets does.
    fn function(&self, _name: &str, _args: &[String]) -> Option<Result<String, String>> {
        None
    }

    /// `path_sep_replace`, which `%asciify` uses for separators it creates.
    fn sep_replace(&self) -> String {
        "_".into()
    }
}

impl Template {
    pub fn parse(src: &str) -> Template {
        let s: Vec<char> = src.chars().collect();
        let mut p = Parser { s: &s, pos: 0 };
        let mut parts = p.parse_expression(false);
        // Anything left over (after a top-level `}`) is literal text.
        if p.pos < s.len() {
            parts.push(Part::Text(s[p.pos..].iter().collect()));
        }
        Template { parts }
    }

    pub fn render(&self, ctx: &dyn Context) -> String {
        eval(&self.parts, ctx)
    }
}

const SYMBOL_DELIM: char = '$';
const FUNC_DELIM: char = '%';
const GROUP_OPEN: char = '{';
const GROUP_CLOSE: char = '}';
const ARG_SEP: char = ',';
const ESCAPE_CHAR: char = '$';

/// Python's `\w`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Parser<'a> {
    s: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    fn text(&self, from: usize) -> String {
        self.s[from..self.pos].iter().collect()
    }

    /// beets' `Parser.parse_expression`; `in_argument` makes `,` special.
    fn parse_expression(&mut self, in_argument: bool) -> Vec<Part> {
        let special = |c: char| {
            matches!(c, SYMBOL_DELIM | FUNC_DELIM | GROUP_OPEN | GROUP_CLOSE)
                || (in_argument && c == ARG_SEP)
        };
        let terminator = |c: char| c == GROUP_CLOSE || (in_argument && c == ARG_SEP);
        let mut parts = vec![];
        let mut text = String::new();
        let flush = |text: &mut String, parts: &mut Vec<Part>| {
            if !text.is_empty() {
                parts.push(Part::Text(std::mem::take(text)));
            }
        };
        while self.pos < self.s.len() {
            let c = self.s[self.pos];
            if !special(c) {
                while self.pos < self.s.len() && !special(self.s[self.pos]) {
                    text.push(self.s[self.pos]);
                    self.pos += 1;
                }
                continue;
            }
            if self.pos == self.s.len() - 1 {
                // A special character at the very end is literal, unless it
                // ends this expression.
                if !terminator(c) {
                    text.push(c);
                    self.pos += 1;
                }
                break;
            }
            let next = self.s[self.pos + 1];
            if c == ESCAPE_CHAR && matches!(next, SYMBOL_DELIM | FUNC_DELIM | GROUP_CLOSE | ARG_SEP)
            {
                text.push(next);
                self.pos += 2;
                continue;
            }
            flush(&mut text, &mut parts);
            match c {
                SYMBOL_DELIM => self.parse_symbol(&mut parts),
                FUNC_DELIM => self.parse_call(&mut parts),
                c if terminator(c) => break,
                _ => {
                    // GROUP_OPEN outside a call is literal.
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        flush(&mut text, &mut parts);
        parts
    }

    fn parse_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.s.len() && is_word(self.s[self.pos]) {
            self.pos += 1;
        }
        self.text(start)
    }

    /// `$foo` or `${foo}` (anything up to the first `}`).
    fn parse_symbol(&mut self, parts: &mut Vec<Part>) {
        let start = self.pos;
        if self.pos == self.s.len() - 1 {
            parts.push(Part::Text(SYMBOL_DELIM.into()));
            self.pos += 1;
            return;
        }
        self.pos += 1;
        if self.s[self.pos] == GROUP_OPEN {
            self.pos += 1;
            let closer = self.s[self.pos..]
                .iter()
                .position(|&c| c == GROUP_CLOSE)
                .map(|i| i + self.pos);
            match closer {
                Some(closer) if closer != self.pos => {
                    let ident = self.s[self.pos..closer].iter().collect();
                    self.pos = closer + 1;
                    parts.push(Part::Symbol {
                        ident,
                        original: self.text(start),
                    });
                }
                _ => parts.push(Part::Text(self.text(start))),
            }
        } else {
            let ident = self.parse_ident();
            if ident.is_empty() {
                parts.push(Part::Text(SYMBOL_DELIM.into()));
            } else {
                parts.push(Part::Symbol {
                    ident,
                    original: self.text(start),
                });
            }
        }
    }

    /// `%foo{bar,baz}`; anything malformed is literal text.
    fn parse_call(&mut self, parts: &mut Vec<Part>) {
        let start = self.pos;
        self.pos += 1;
        let ident = self.parse_ident();
        if ident.is_empty() {
            parts.push(Part::Text(FUNC_DELIM.into()));
            return;
        }
        if self.pos >= self.s.len() || self.s[self.pos] != GROUP_OPEN {
            parts.push(Part::Text(self.text(start)));
            return;
        }
        self.pos += 1;
        let args = self.parse_argument_list();
        if self.pos >= self.s.len() || self.s[self.pos] != GROUP_CLOSE {
            parts.push(Part::Text(self.text(start)));
            return;
        }
        self.pos += 1;
        parts.push(Part::Call {
            ident,
            args,
            original: self.text(start),
        });
    }

    fn parse_argument_list(&mut self) -> Vec<Vec<Part>> {
        let mut args = vec![];
        while self.pos < self.s.len() {
            args.push(self.parse_expression(true));
            if self.pos >= self.s.len() || self.s[self.pos] == GROUP_CLOSE {
                break;
            }
            // An argument ends at `,` or `}` (or the end).
            self.pos += 1;
        }
        args
    }
}

fn eval(parts: &[Part], ctx: &dyn Context) -> String {
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Text(t) => out.push_str(t),
            Part::Symbol { ident, original } => {
                out.push_str(&ctx.field(ident).unwrap_or_else(|| original.clone()))
            }
            Part::Call {
                ident,
                args,
                original,
            } => {
                let args: Vec<String> = args.iter().map(|a| eval(a, ctx)).collect();
                match call(ident, &args, ctx) {
                    Some(Ok(s)) => out.push_str(&s),
                    Some(Err(e)) => {
                        out.push('<');
                        out.push_str(&e);
                        out.push('>');
                    }
                    None => out.push_str(original),
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------- functions

/// A default template function's Python signature, for beets' argument
/// count errors: parameter names, how many are required, and whether it is
/// a method (Python then counts `self`).
struct Sig {
    params: &'static [&'static str],
    required: usize,
    method: bool,
}

fn signature(name: &str) -> Option<Sig> {
    let sig = |params, required, method| {
        Some(Sig {
            params,
            required,
            method,
        })
    };
    match name {
        "lower" | "upper" | "capitalize" | "title" | "asciify" => sig(&["s"], 1, false),
        "left" | "right" => sig(&["s", "chars"], 2, false),
        "if" => sig(&["condition", "trueval", "falseval"], 2, false),
        "time" => sig(&["s", "fmt"], 2, false),
        "first" => sig(&["s", "count", "skip", "sep", "join_str"], 1, false),
        "ifdef" => sig(&["field", "trueval", "falseval"], 1, true),
        "aunique" | "sunique" | "tunique" => sig(&["keys", "disam", "bracket"], 0, true),
        _ => None,
    }
}

/// Python's `TypeError` for a call with the wrong number of arguments.
fn check_arity(name: &str, sig: &Sig, given: usize) -> Result<(), String> {
    let qual = format!("DefaultTemplateFunctions.tmpl_{name}()");
    let s = |n: usize| if n == 1 { "" } else { "s" };
    if given < sig.required {
        let missing: Vec<String> = sig.params[given..sig.required]
            .iter()
            .map(|p| format!("'{p}'"))
            .collect();
        let names = match missing.len() {
            1 => missing[0].clone(),
            n => format!("{} and {}", missing[..n - 1].join(", "), missing[n - 1]),
        };
        return Err(format!(
            "{qual} missing {} required positional argument{}: {names}",
            missing.len(),
            s(missing.len())
        ));
    }
    if given > sig.params.len() {
        let extra = sig.method as usize;
        let (min, max, given) = (
            sig.required + extra,
            sig.params.len() + extra,
            given + extra,
        );
        let takes = if min == max {
            format!("{max} positional argument{}", s(max))
        } else {
            format!("from {min} to {max} positional arguments")
        };
        let was = if given == 1 { "was" } else { "were" };
        return Err(format!("{qual} takes {takes} but {given} {was} given"));
    }
    Ok(())
}

/// Python's `repr()` of a string.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `int(s)`: surrounding whitespace, a sign, `_` between digits.
pub fn py_int(s: &str) -> Result<i64, String> {
    let err = || format!("invalid literal for int() with base 10: {}", py_repr(s));
    let t = s.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return Err(err());
    }
    let mut n: i64 = 0;
    for c in digits.chars().filter(|&c| c != '_') {
        let d = c.to_digit(10).ok_or_else(err)? as i64;
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(d))
            .ok_or_else(err)?;
    }
    Ok(if neg { -n } else { n })
}

/// beets' `_int_arg`: `int(s.strip())`.
fn int_arg(s: &str) -> Result<i64, String> {
    py_int(s.trim())
}

/// Python slice bounds `[start:end]` over a sequence of length `len`.
fn py_slice(len: usize, start: Option<i64>, end: Option<i64>) -> (usize, usize) {
    let len = len as i64;
    let clamp = |i: i64| -> usize {
        let i = if i < 0 { i + len } else { i };
        i.clamp(0, len) as usize
    };
    let a = start.map(clamp).unwrap_or(0);
    let b = end.map(clamp).unwrap_or(len as usize);
    (a, b.max(a))
}

/// Python's `str.capitalize()`: first character upper, the rest lower.
pub fn py_capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f
            .to_uppercase()
            .chain(c.as_str().to_lowercase().chars())
            .collect(),
        None => String::new(),
    }
}

/// Python's `unidecode` (via `deunicode`, whose tables mostly agree;
/// unidecode puts a space before vulgar fractions: `½` → ` 1/2`).
pub fn unidecode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        if matches!(c, '\u{bc}'..='\u{be}' | '\u{2150}'..='\u{215e}') {
            out.push(' ');
        }
        out.push_str(&deunicode::deunicode(c.encode_utf8(&mut buf)));
    }
    out
}

/// beets' `asciify_path`: transliterate each `/`-separated component,
/// replacing separators the transliteration produces.
pub fn asciify_path(s: &str, sep_replace: &str) -> String {
    let s: String = s.nfc().collect();
    s.split('/')
        .map(|p| unidecode(p).replace('/', sep_replace))
        .collect::<Vec<_>>()
        .join("/")
}

/// Format a time string in `time_format` with another strftime format.
fn tmpl_time(s: &str, fmt: &str) -> Result<String, String> {
    use std::fmt::Write;
    let cur = crate::model::TIME_FORMAT;
    let dt = chrono::NaiveDateTime::parse_from_str(s, cur).map_err(|_| {
        format!(
            "time data {} does not match format {}",
            py_repr(s),
            py_repr(cur)
        )
    })?;
    let mut out = String::new();
    write!(out, "{}", dt.format(fmt)).map_err(|_| "Invalid format string".to_string())?;
    Ok(out)
}

fn call(name: &str, args: &[String], ctx: &dyn Context) -> Option<Result<String, String>> {
    let Some(sig) = signature(name) else {
        // Plugin functions.
        return ctx.function(name, args);
    };
    if let Err(e) = check_arity(name, &sig, args.len()) {
        return Some(Err(e));
    }
    // Library-dependent functions (`%aunique` etc.) come from the context.
    if let Some(r) = ctx.function(name, args) {
        return Some(r);
    }
    let a = |i: usize| args.get(i).map(String::as_str);
    let s = a(0).unwrap_or("");
    Some((|| -> Result<String, String> {
        Ok(match name {
            "lower" => s.to_lowercase(),
            "upper" => s.to_uppercase(),
            "capitalize" => py_capitalize(s),
            // `string.capwords`: split on whitespace, capitalize, join.
            "title" => s
                .split_whitespace()
                .map(py_capitalize)
                .collect::<Vec<_>>()
                .join(" "),
            "left" => {
                let chars: Vec<char> = s.chars().collect();
                let (x, y) = py_slice(chars.len(), Some(0), Some(int_arg(args[1].as_str())?));
                chars[x..y].iter().collect()
            }
            "right" => {
                let chars: Vec<char> = s.chars().collect();
                let n = int_arg(&args[1])?;
                let (x, y) = py_slice(chars.len(), Some(-n), None);
                chars[x..y].iter().collect()
            }
            "if" => {
                let falseval = a(2).unwrap_or("").to_string();
                let truthy = match int_arg(s) {
                    Ok(n) => n != 0,
                    Err(_) if s.to_lowercase() == "false" => return Ok(falseval),
                    Err(_) => !s.is_empty(),
                };
                if truthy { args[1].clone() } else { falseval }
            }
            "asciify" => asciify_path(s, &ctx.sep_replace()),
            "time" => tmpl_time(s, &args[1])?,
            "first" => {
                let skip = a(2).map(py_int).transpose()?.unwrap_or(0);
                let count = skip + a(1).map(py_int).transpose()?.unwrap_or(1);
                let sep = a(3).unwrap_or("; ");
                let join = a(4).unwrap_or("; ");
                if sep.is_empty() {
                    return Err("empty separator".into());
                }
                let parts: Vec<&str> = s.split(sep).collect();
                let (x, y) = py_slice(parts.len(), Some(skip), Some(count));
                parts[x..y].join(join)
            }
            "ifdef" => {
                let trueval = a(1).unwrap_or("");
                match ctx.ifdef(s) {
                    Some(_) if !trueval.is_empty() => trueval.to_string(),
                    Some(v) => v,
                    None => a(2).unwrap_or("").to_string(),
                }
            }
            // Without a library these are empty, as in beets.
            "aunique" | "sunique" | "tunique" => String::new(),
            _ => unreachable!(),
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Map(HashMap<&'static str, &'static str>);
    impl Context for Map {
        fn field(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|s| s.to_string())
        }
    }

    fn ctx() -> Map {
        Map(HashMap::from([
            ("artist", "Björk"),
            ("album", "Post"),
            ("track", "03"),
            ("title", "hyper-ballad"),
            ("comp", "False"),
            ("year", "1995"),
            ("genres", "Pop; Electronic; Art Pop"),
            ("foo", "bar"),
            ("baz", "BaR"),
            ("bar", "qux"),
        ]))
    }

    fn render(t: &str) -> String {
        Template::parse(t).render(&ctx())
    }

    #[test]
    fn variables() {
        assert_eq!(
            render("$artist/$album/$track $title"),
            "Björk/Post/03 hyper-ballad"
        );
        assert_eq!(render("${artist}s"), "Björks");
        assert_eq!(render("$missing"), "$missing");
        assert_eq!(render("${missing}"), "${missing}");
        assert_eq!(render("cost $$5 100$% a$,b"), "cost $5 100% a,b");
        assert_eq!(render("a $ b"), "a $ b");
    }

    // Ported from beets' test/util/test_functemplate.py (ParseTest and
    // EvalTest), as parse results rendered against a context.
    #[test]
    fn upstream_parse_cases() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("foo", "foo"),
            ("foo bar", "foo bar"),
            ("foo $$ bar", "foo $ bar"),
            ("foo $% bar", "foo % bar"),
            ("foo $, bar", "foo , bar"),
            ("foo $} bar", "foo } bar"),
            ("foo $bar baz", "foo qux baz"),
            ("foo ${bar} baz", "foo qux baz"),
            ("foo $bar$bar", "foo quxqux"),
            ("$bar", "qux"),
            ("foo$ bar", "foo$ bar"),
            ("foo ${bar baz", "foo ${bar baz"),
            ("foo $bar.baz", "foo qux.baz"),
            ("foo $bar ", "foo qux "),
            // `${` with no closing brace, or `${}`.
            ("${", "${"),
            ("${}", "${}"),
            // A top-level `}` ends the template; the rest is literal.
            ("x}$bar", "x}$bar"),
            ("x{y", "x{y"),
            ("$", "$"),
            ("%", "%"),
            ("{", "{"),
            ("}", "}"),
            ("foo %bar", "foo %bar"),
            ("foo %", "foo %"),
            ("foo %bar{", "foo %bar{"),
            ("foo %bar{baz", "foo %bar{baz"),
            ("foo %bar{baz,$bar", "foo %bar{baz,$bar"),
            ("foo %bar baz", "foo %bar baz"),
            ("%upper{$bar} x", "QUX x"),
            ("%upper{$bar,", "%upper{$bar,"),
            ("%{x}", "%{x}"),
            ("%upper{a$}b}", "A}B"),
            ("%upper{a$,b}", "A,B"),
            ("%if{1,a{b}", "a{b"),
            ("%nosuch{x}", "%nosuch{x}"),
            ("%nosuch{$bar}", "%nosuch{$bar}"),
            ("%lower{%upper{$album}}", "post"),
        ];
        for (t, want) in cases {
            assert_eq!(render(t), *want, "template {t:?}");
        }
    }

    #[test]
    fn functions() {
        assert_eq!(render("%upper{$album}"), "POST");
        assert_eq!(render("%lower{$baz}"), "bar");
        assert_eq!(render("%capitalize{hELLO wORLD}"), "Hello world");
        assert_eq!(render("%title{$title}"), "Hyper-ballad");
        assert_eq!(render("%title{the  long   way}"), "The Long Way");
        assert_eq!(render("%asciify{$artist}"), "Bjork");
        assert_eq!(render("%asciify{a½/b}"), "a 1_2/b");
        assert_eq!(render("%asciify{ab\u{a2}\u{bd}d}"), "abC_ 1_2d");
        assert_eq!(render("%left{$artist,3}"), "Bjö");
        assert_eq!(render("%left{$artist, 3 }"), "Bjö");
        assert_eq!(render("%left{$artist,-2}"), "Bjö");
        assert_eq!(render("%right{$year,2}"), "95");
        assert_eq!(render("%right{$year,0}"), "1995");
        assert_eq!(render("%right{$year,-1}"), "995");
        assert_eq!(render("%right{ab,5}"), "ab");
        assert_eq!(render("%if{$comp,Compilations,$artist}"), "Björk");
        assert_eq!(render("%if{$year,y,n}"), "y");
        assert_eq!(render("%if{0,y,n}"), "n");
        assert_eq!(render("%if{ 0 ,y,n}"), "n");
        assert_eq!(render("%if{FALSE,y,n}"), "n");
        assert_eq!(render("%if{ ,y,n}"), "y");
        assert_eq!(render("%if{,y,n}"), "n");
        assert_eq!(render("%if{x,y}"), "y");
        assert_eq!(render("%if{0,y}"), "");
        assert_eq!(render("%ifdef{mood,has,none}"), "none");
        assert_eq!(render("%ifdef{album}"), "Post");
        assert_eq!(render("%ifdef{album,yes}"), "yes");
        assert_eq!(render("%ifdef{mood}"), "");
        assert_eq!(render("%first{$genres}"), "Pop");
        assert_eq!(render("%first{$genres,2}"), "Pop; Electronic");
        assert_eq!(render("%first{$genres,2,1}"), "Electronic; Art Pop");
        assert_eq!(render("%first{$genres,1,2}"), "Art Pop");
        assert_eq!(render("%first{a b c,2,0, ,-}"), "a-b");
        assert_eq!(render("%first{$genres,-1}"), "Pop; Electronic");
        assert_eq!(render("%aunique{}"), "");
        assert_eq!(render("%sunique{}"), "");
        assert_eq!(render("%time{2024-03-05 10:00:00,%Y}"), "2024");
        assert_eq!(render("%time{2024-03-05 10:00:00,%d.%m.}"), "05.03.");
    }

    #[test]
    fn function_errors() {
        assert_eq!(
            render("%left{abc}"),
            "<DefaultTemplateFunctions.tmpl_left() missing 1 required positional argument: 'chars'>"
        );
        assert_eq!(
            render("%left{abc,x}"),
            "<invalid literal for int() with base 10: 'x'>"
        );
        assert_eq!(
            render("%lower{a,b}"),
            "<DefaultTemplateFunctions.tmpl_lower() takes 1 positional argument but 2 were given>"
        );
        assert_eq!(
            render("%if{a,b,c,d}"),
            "<DefaultTemplateFunctions.tmpl_if() takes from 2 to 3 positional arguments but 4 were given>"
        );
        assert_eq!(
            render("%aunique{a,b,c,d}"),
            "<DefaultTemplateFunctions.tmpl_aunique() takes from 1 to 4 positional arguments but 5 were given>"
        );
        assert_eq!(
            render("%time{2024,%Y}"),
            "<time data '2024' does not match format '%Y-%m-%d %H:%M:%S'>"
        );
        assert_eq!(render("%first{a,1,0,}"), "<empty separator>");
    }

    #[test]
    fn literal_braces_and_commas() {
        assert_eq!(render("a,b}c"), "a,b}c");
        assert_eq!(render("100%"), "100%");
        assert_eq!(render("{$bar}"), "{qux}");
    }

    #[test]
    fn helpers() {
        assert_eq!(py_int(" +1_000 "), Ok(1000));
        assert!(py_int("1__0").is_err());
        assert!(py_int("").is_err());
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_capitalize("ébc DEF"), "Ébc def");
    }
}
