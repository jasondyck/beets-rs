//! beets path-format templates: `$field`, `${field}` and `%func{arg,...}`.

use deunicode::deunicode;

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Var {
        name: String,
        raw: String,
    },
    Call {
        name: String,
        args: Vec<Vec<Node>>,
        raw: String,
    },
}

#[derive(Debug, Clone)]
pub struct Template {
    nodes: Vec<Node>,
}

/// Supplies field values (already formatted) and library-dependent functions.
pub trait Context {
    fn field(&self, name: &str) -> Option<String>;
    /// `%aunique{identifiers,disambiguators,brackets}`; empty by default.
    fn aunique(&self, _args: &[String]) -> String {
        String::new()
    }
}

impl Template {
    pub fn parse(src: &str) -> Template {
        let chars: Vec<char> = src.chars().collect();
        let mut pos = 0;
        let nodes = parse_seq(&chars, &mut pos, false);
        // Anything left over (an unmatched `}` or `,` at top level) is literal text.
        let mut nodes = nodes;
        if pos < chars.len() {
            nodes.push(Node::Text(chars[pos..].iter().collect()));
        }
        Template { nodes }
    }

    pub fn render(&self, ctx: &dyn Context) -> String {
        eval(&self.nodes, ctx)
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Parse until end of input, or (when `in_arg`) an unescaped `,` or `}`.
fn parse_seq(chars: &[char], pos: &mut usize, in_arg: bool) -> Vec<Node> {
    let mut nodes = vec![];
    let mut text = String::new();
    let flush = |text: &mut String, nodes: &mut Vec<Node>| {
        if !text.is_empty() {
            nodes.push(Node::Text(std::mem::take(text)));
        }
    };
    while *pos < chars.len() {
        let c = chars[*pos];
        if in_arg && (c == ',' || c == '}') {
            break;
        }
        match c {
            '$' => {
                let start = *pos;
                *pos += 1;
                match chars.get(*pos) {
                    Some(&e @ ('$' | '%' | ',' | '}')) => {
                        text.push(e);
                        *pos += 1;
                    }
                    Some('{') => {
                        let mut end = *pos + 1;
                        while end < chars.len() && is_ident(chars[end]) {
                            end += 1;
                        }
                        if end < chars.len() && chars[end] == '}' && end > *pos + 1 {
                            flush(&mut text, &mut nodes);
                            let name: String = chars[*pos + 1..end].iter().collect();
                            *pos = end + 1;
                            nodes.push(Node::Var {
                                name,
                                raw: chars[start..*pos].iter().collect(),
                            });
                        } else {
                            text.push('$');
                        }
                    }
                    Some(&n) if is_ident(n) => {
                        let mut end = *pos;
                        while end < chars.len() && is_ident(chars[end]) {
                            end += 1;
                        }
                        flush(&mut text, &mut nodes);
                        let name: String = chars[*pos..end].iter().collect();
                        *pos = end;
                        nodes.push(Node::Var {
                            name,
                            raw: chars[start..*pos].iter().collect(),
                        });
                    }
                    _ => text.push('$'),
                }
            }
            '%' => {
                let start = *pos;
                let mut end = *pos + 1;
                while end < chars.len() && is_ident(chars[end]) {
                    end += 1;
                }
                if end > *pos + 1 && chars.get(end) == Some(&'{') {
                    let name: String = chars[*pos + 1..end].iter().collect();
                    *pos = end + 1;
                    let mut args = vec![];
                    loop {
                        args.push(parse_seq(chars, pos, true));
                        match chars.get(*pos) {
                            Some(',') => *pos += 1,
                            Some('}') => {
                                *pos += 1;
                                break;
                            }
                            _ => break, // unterminated: treat what we have as the call
                        }
                    }
                    flush(&mut text, &mut nodes);
                    // `%f{}` is a call with no arguments.
                    if args.len() == 1 && args[0].is_empty() {
                        args.clear();
                    }
                    nodes.push(Node::Call {
                        name,
                        args,
                        raw: chars[start..*pos].iter().collect(),
                    });
                } else {
                    text.push('%');
                    *pos += 1;
                }
            }
            _ => {
                text.push(c);
                *pos += 1;
            }
        }
    }
    flush(&mut text, &mut nodes);
    nodes
}

fn eval(nodes: &[Node], ctx: &dyn Context) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Var { name, raw } => {
                out.push_str(&ctx.field(name).unwrap_or_else(|| raw.clone()))
            }
            Node::Call { name, args, raw } => {
                let args: Vec<String> = args.iter().map(|a| eval(a, ctx)).collect();
                match call(name, &args, ctx) {
                    Some(s) => out.push_str(&s),
                    None => out.push_str(raw),
                }
            }
        }
    }
    out
}

fn arg(args: &[String], i: usize) -> &str {
    args.get(i).map(|s| s.as_str()).unwrap_or("")
}

fn truthy(s: &str) -> bool {
    let s = s.trim();
    match s.parse::<i64>() {
        Ok(i) => i != 0,
        Err(_) => !s.is_empty() && !s.eq_ignore_ascii_case("false"),
    }
}

/// Python's str.title(): uppercase the first letter of each run of letters.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

fn call(name: &str, args: &[String], ctx: &dyn Context) -> Option<String> {
    let n = |i: usize| arg(args, i).trim().parse::<usize>().ok();
    Some(match name {
        "lower" => arg(args, 0).to_lowercase(),
        "upper" => arg(args, 0).to_uppercase(),
        "capitalize" => {
            let s = arg(args, 0);
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        }
        "title" => title_case(arg(args, 0)),
        "left" => arg(args, 0).chars().take(n(1)?).collect(),
        "right" => {
            let chars: Vec<char> = arg(args, 0).chars().collect();
            let k = n(1)?.min(chars.len());
            chars[chars.len() - k..].iter().collect()
        }
        "if" => {
            if truthy(arg(args, 0)) {
                arg(args, 1).to_string()
            } else {
                arg(args, 2).to_string()
            }
        }
        "ifdef" => {
            let field = arg(args, 0);
            match ctx.field(field) {
                Some(v) if args.len() < 2 => v,
                Some(_) => arg(args, 1).to_string(),
                None => arg(args, 2).to_string(),
            }
        }
        "asciify" => deunicode(arg(args, 0)),
        "initial" => deunicode(arg(args, 0))
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default(),
        "first" => {
            let count = if args.len() > 1 { n(1)? } else { 1 };
            let skip = if args.len() > 2 { n(2)? } else { 0 };
            let sep = if args.len() > 3 { arg(args, 3) } else { "; " };
            let join = if args.len() > 4 { arg(args, 4) } else { "; " };
            arg(args, 0)
                .split(sep)
                .skip(skip)
                .take(count)
                .collect::<Vec<_>>()
                .join(join)
        }
        "time" => {
            let dt = chrono::NaiveDateTime::parse_from_str(arg(args, 0), crate::model::TIME_FORMAT)
                .ok()?;
            dt.format(arg(args, 1)).to_string()
        }
        "aunique" => ctx.aunique(args),
        "sunique" => String::new(),
        _ => return None,
    })
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

    fn render(t: &str) -> String {
        let ctx = Map(HashMap::from([
            ("artist", "Björk"),
            ("album", "Post"),
            ("track", "03"),
            ("title", "hyper-ballad"),
            ("comp", "False"),
            ("year", "1995"),
            ("genres", "Pop; Electronic; Art Pop"),
        ]));
        Template::parse(t).render(&ctx)
    }

    #[test]
    fn variables() {
        assert_eq!(
            render("$artist/$album/$track $title"),
            "Björk/Post/03 hyper-ballad"
        );
        assert_eq!(render("${artist}s"), "Björks");
        assert_eq!(render("$missing"), "$missing");
        assert_eq!(render("cost $$5 100$% a$,b"), "cost $5 100% a,b");
        assert_eq!(render("a $ b"), "a $ b");
    }

    #[test]
    fn functions() {
        assert_eq!(render("%upper{$album}"), "POST");
        assert_eq!(render("%title{$title}"), "Hyper-Ballad");
        assert_eq!(render("%asciify{$artist}"), "Bjork");
        assert_eq!(render("%initial{$artist}"), "B");
        assert_eq!(render("%left{$artist,3}"), "Bjö");
        assert_eq!(render("%right{$year,2}"), "95");
        assert_eq!(render("%if{$comp,Compilations,$artist}"), "Björk");
        assert_eq!(render("%if{$year,y,n}"), "y");
        assert_eq!(render("%if{,y,n}"), "n");
        assert_eq!(render("%ifdef{mood,has,none}"), "none");
        assert_eq!(render("%ifdef{album}"), "Post");
        assert_eq!(render("%first{$genres}"), "Pop");
        assert_eq!(render("%first{$genres,2}"), "Pop; Electronic");
        assert_eq!(render("%nosuch{x}"), "%nosuch{x}");
        assert_eq!(render("%aunique{}"), "");
        assert_eq!(render("%time{2024-03-05 10:00:00,%Y}"), "2024");
        assert_eq!(render("%lower{%upper{$album}}"), "post");
    }

    #[test]
    fn literal_braces_and_commas() {
        assert_eq!(render("a,b}c"), "a,b}c");
        assert_eq!(render("100%"), "100%");
    }
}
