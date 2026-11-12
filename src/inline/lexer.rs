//! Python's tokenizer, for the subset `inline` evaluates: names, numbers,
//! strings (with `r`/`u`/`b`/`f` prefixes and triple quotes), operators,
//! and NEWLINE/INDENT/DEDENT for function bodies.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Name(String),
    Int(i64),
    Float(f64),
    /// A string literal: its value, and whether it's an f-string (then
    /// the value is the raw text between the quotes, escapes processed
    /// unless raw) or bytes.
    Str {
        value: String,
        fstring: bool,
        bytes: bool,
        raw: bool,
    },
    Op(&'static str),
    Newline,
    Indent,
    Dedent,
    End,
}

const OPS: &[&str] = &[
    "**=", "//=", ">>=", "<<=", "...", "**", "//", "==", "!=", "<=", ">=", "+=", "-=", "*=", "/=",
    "%=", "&=", "|=", "^=", "->", ":=", "<<", ">>", "+", "-", "*", "/", "%", "<", ">", "=", "(",
    ")", "[", "]", "{", "}", ",", ":", ".", ";", "@", "~", "&", "|", "^",
];

/// Tokens, each with its line (1-based) for messages. `implicit_join`:
/// the whole text is inside brackets (an expression), so newlines don't
/// count.
pub fn tokenize(src: &str, implicit_join: bool) -> Result<Vec<(Tok, usize)>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    let mut line = 1;
    let mut depth: usize = if implicit_join { 1 } else { 0 };
    let mut indents = vec![0usize];
    let mut at_line_start = true;
    while i < chars.len() {
        if at_line_start && depth == 0 {
            // Measure indentation; blank and comment-only lines don't count.
            let mut col = 0;
            let mut j = i;
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                col += if chars[j] == '\t' { 8 - col % 8 } else { 1 };
                j += 1;
            }
            if j >= chars.len() || chars[j] == '\n' || chars[j] == '#' || chars[j] == '\r' {
                // Skip the blank line.
                while j < chars.len() && chars[j] != '\n' {
                    j += 1;
                }
                i = j + 1;
                line += 1;
                continue;
            }
            let cur = *indents.last().unwrap();
            if col > cur {
                indents.push(col);
                out.push((Tok::Indent, line));
            } else {
                while col < *indents.last().unwrap() {
                    indents.pop();
                    out.push((Tok::Dedent, line));
                }
                if col != *indents.last().unwrap() {
                    return Err(format!(
                        "unindent does not match any outer indentation level (line {line})"
                    ));
                }
            }
            i = j;
            at_line_start = false;
            continue;
        }
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\x0c' => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '\\' if chars.get(i + 1) == Some(&'\n') => {
                i += 2;
                line += 1;
            }
            '\n' => {
                if depth == 0 {
                    if !matches!(out.last(), Some((Tok::Newline, _)) | None) {
                        out.push((Tok::Newline, line));
                    }
                    at_line_start = true;
                }
                line += 1;
                i += 1;
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                // A string prefix?
                let lower = word.to_lowercase();
                if i < chars.len()
                    && (chars[i] == '\'' || chars[i] == '"')
                    && matches!(
                        lower.as_str(),
                        "r" | "u" | "b" | "f" | "br" | "rb" | "fr" | "rf"
                    )
                {
                    let (tok, ni, nl) = string(&chars, i, &lower, line)?;
                    out.push((tok, line));
                    i = ni;
                    line = nl;
                } else {
                    out.push((Tok::Name(word), line));
                }
            }
            '\'' | '"' => {
                let (tok, ni, nl) = string(&chars, i, "", line)?;
                out.push((tok, line));
                i = ni;
                line = nl;
            }
            c if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let (tok, ni) = number(&chars, i)?;
                out.push((tok, line));
                i = ni;
            }
            _ => {
                let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
                let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) else {
                    return Err(format!("invalid character '{c}' (U+{:04X})", c as u32));
                };
                match *op {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => {
                        if depth == 0 || (implicit_join && depth == 1) {
                            return Err(format!("unmatched '{op}'"));
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
                out.push((Tok::Op(op), line));
                i += op.len();
            }
        }
    }
    if !matches!(out.last(), Some((Tok::Newline, _)) | None) && depth == 0 {
        out.push((Tok::Newline, line));
    }
    if depth > if implicit_join { 1 } else { 0 } {
        return Err("'(' was never closed".into());
    }
    while indents.len() > 1 {
        indents.pop();
        out.push((Tok::Dedent, line));
    }
    out.push((Tok::End, line));
    Ok(out)
}

fn number(chars: &[char], mut i: usize) -> Result<(Tok, usize), String> {
    let start = i;
    let lower = |k: usize| chars.get(k).map(|c| c.to_ascii_lowercase());
    if chars[i] == '0' && matches!(lower(i + 1), Some('x' | 'o' | 'b')) {
        let radix = match lower(i + 1) {
            Some('x') => 16,
            Some('o') => 8,
            _ => 2,
        };
        i += 2;
        let s = i;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        let digits: String = chars[s..i].iter().filter(|c| **c != '_').collect();
        let v = i64::from_str_radix(&digits, radix).map_err(|_| "invalid number".to_string())?;
        return Ok((Tok::Int(v), i));
    }
    let mut float = false;
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
        i += 1;
    }
    if i < chars.len() && chars[i] == '.' {
        float = true;
        i += 1;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
            i += 1;
        }
    }
    if matches!(lower(i), Some('e'))
        && (chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())
            || (matches!(chars.get(i + 1), Some('+' | '-'))
                && chars.get(i + 2).is_some_and(|c| c.is_ascii_digit())))
    {
        float = true;
        i += 2;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    if matches!(lower(i), Some('j')) {
        return Err("complex numbers".into());
    }
    let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
    if float {
        Ok((Tok::Float(text.parse().map_err(|_| "invalid number")?), i))
    } else {
        if text.len() > 1 && text.starts_with('0') && text.chars().any(|c| c != '0') {
            return Err("leading zeros in decimal integer literals are not permitted".into());
        }
        match text.parse::<i64>() {
            Ok(v) => Ok((Tok::Int(v), i)),
            Err(_) => Ok((Tok::Float(text.parse().unwrap_or(f64::INFINITY)), i)),
        }
    }
}

/// A string literal at `chars[i]` (the opening quote) with `prefix`.
fn string(
    chars: &[char],
    mut i: usize,
    prefix: &str,
    mut line: usize,
) -> Result<(Tok, usize, usize), String> {
    let raw = prefix.contains('r');
    let fstring = prefix.contains('f');
    let bytes = prefix.contains('b');
    let q = chars[i];
    let triple = chars.get(i + 1) == Some(&q) && chars.get(i + 2) == Some(&q);
    i += if triple { 3 } else { 1 };
    let mut body = String::new();
    loop {
        let Some(&c) = chars.get(i) else {
            return Err("unterminated string literal".into());
        };
        if c == q && (!triple || (chars.get(i + 1) == Some(&q) && chars.get(i + 2) == Some(&q))) {
            i += if triple { 3 } else { 1 };
            break;
        }
        if c == '\n' {
            if !triple {
                return Err("unterminated string literal".into());
            }
            line += 1;
        }
        if c == '\\' {
            let Some(&n) = chars.get(i + 1) else {
                return Err("unterminated string literal".into());
            };
            if raw || fstring {
                // f-strings keep their escapes for the second pass (their
                // braces must be seen first); raw strings keep them all.
                body.push('\\');
                body.push(n);
                if n == '\n' {
                    line += 1;
                }
                i += 2;
                continue;
            }
            i += 2;
            match n {
                '\n' => line += 1,
                'n' => body.push('\n'),
                't' => body.push('\t'),
                'r' => body.push('\r'),
                '0'..='7' => {
                    let mut v = n.to_digit(8).unwrap();
                    for _ in 0..2 {
                        match chars.get(i).and_then(|d| d.to_digit(8)) {
                            Some(d) => {
                                v = v * 8 + d;
                                i += 1;
                            }
                            None => break,
                        }
                    }
                    body.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                }
                'x' | 'u' | 'U' if !(bytes && n != 'x') => {
                    let len = match n {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let hex: String = chars.get(i..i + len).unwrap_or(&[]).iter().collect();
                    let v = u32::from_str_radix(&hex, 16)
                        .map_err(|_| format!("truncated \\{n} escape"))?;
                    body.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                    i += len;
                }
                'a' => body.push('\x07'),
                'b' => body.push('\x08'),
                'f' => body.push('\x0c'),
                'v' => body.push('\x0b'),
                '\\' | '\'' | '"' => body.push(n),
                other => {
                    body.push('\\');
                    body.push(other);
                }
            }
            continue;
        }
        body.push(c);
        i += 1;
    }
    Ok((
        Tok::Str {
            value: body,
            fstring,
            bytes,
            raw,
        },
        i,
        line,
    ))
}

/// The escapes of an f-string's literal parts (after the braces are
/// split out).
pub fn unescape(s: &str) -> String {
    let toks = tokenize(&format!("'''{}'''", s.replace("'''", "\\'\\'\\'")), true);
    match toks.ok().and_then(|t| t.into_iter().next()) {
        Some((Tok::Str { value, .. }, _)) => value,
        _ => s.to_string(),
    }
}
