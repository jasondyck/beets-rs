//! Python values for `inline`, with Python's `str()`/`repr()`, truthiness,
//! equality and ordering, and its string formatting (`format()` specs,
//! `%` and `str.format`).

use std::cell::RefCell;
use std::rc::Rc;

/// A Python exception: its type name and message.
#[derive(Debug, Clone, PartialEq)]
pub struct PyErr {
    pub kind: &'static str,
    pub msg: String,
}

pub fn err<T>(kind: &'static str, msg: impl Into<String>) -> Result<T, PyErr> {
    Err(PyErr {
        kind,
        msg: msg.into(),
    })
}

pub type PyResult<T> = Result<T, PyErr>;

/// A library object (`db_obj`, the album's `items`).
pub trait Obj<'a> {
    fn type_name(&self) -> &'static str;
    /// `obj.name` for a field (methods are separate).
    fn getattr(&self, name: &str) -> PyResult<V<'a>>;
    fn has_method(&self, name: &str) -> bool;
    fn call_method(&self, name: &str, args: &[V<'a>]) -> PyResult<V<'a>>;
    fn repr(&self) -> String;
}

#[derive(Clone)]
pub enum V<'a> {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<Vec<u8>>),
    List(Rc<RefCell<Vec<V<'a>>>>),
    Tuple(Rc<Vec<V<'a>>>),
    Dict(Rc<RefCell<Vec<(V<'a>, V<'a>)>>>),
    Obj(Rc<dyn Obj<'a> + 'a>),
    /// A built-in function (or type: `str`, `int`, ...).
    Builtin(&'static str),
    /// A bound method.
    Method(Box<V<'a>>, Rc<str>),
}

impl std::fmt::Debug for V<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.repr())
    }
}

impl<'a> V<'a> {
    pub fn str(s: impl Into<Rc<str>>) -> Self {
        V::Str(s.into())
    }

    pub fn list(v: Vec<V<'a>>) -> Self {
        V::List(Rc::new(RefCell::new(v)))
    }

    pub fn tuple(v: Vec<V<'a>>) -> Self {
        V::Tuple(Rc::new(v))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            V::None => "NoneType",
            V::Bool(_) => "bool",
            V::Int(_) => "int",
            V::Float(_) => "float",
            V::Str(_) => "str",
            V::Bytes(_) => "bytes",
            V::List(_) => "list",
            V::Tuple(_) => "tuple",
            V::Dict(_) => "dict",
            V::Obj(o) => o.type_name(),
            V::Builtin(n) if is_type(n) => "type",
            V::Builtin(_) => "builtin_function_or_method",
            V::Method(..) => "builtin_function_or_method",
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            V::None => false,
            V::Bool(b) => *b,
            V::Int(i) => *i != 0,
            V::Float(f) => *f != 0.0,
            V::Str(s) => !s.is_empty(),
            V::Bytes(b) => !b.is_empty(),
            V::List(l) => !l.borrow().is_empty(),
            V::Tuple(t) => !t.is_empty(),
            V::Dict(d) => !d.borrow().is_empty(),
            _ => true,
        }
    }

    /// Python's `str()`.
    pub fn to_str(&self) -> String {
        match self {
            V::Str(s) => s.to_string(),
            other => other.repr(),
        }
    }

    /// Python's `repr()`.
    pub fn repr(&self) -> String {
        match self {
            V::None => "None".into(),
            V::Bool(b) => if *b { "True" } else { "False" }.into(),
            V::Int(i) => i.to_string(),
            V::Float(f) => float_repr(*f),
            V::Str(s) => crate::hooks::py_repr(s),
            V::Bytes(b) => crate::pyyaml::PyVal::Bytes(b.to_vec()).py_repr(),
            V::List(l) => format!(
                "[{}]",
                l.borrow()
                    .iter()
                    .map(V::repr)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            V::Tuple(t) if t.len() == 1 => format!("({},)", t[0].repr()),
            V::Tuple(t) => format!("({})", t.iter().map(V::repr).collect::<Vec<_>>().join(", ")),
            V::Dict(d) => format!(
                "{{{}}}",
                d.borrow()
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.repr(), v.repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            V::Obj(o) => o.repr(),
            V::Builtin(n) if is_type(n) => format!("<class '{n}'>"),
            V::Builtin(n) => format!("<built-in function {n}>"),
            V::Method(o, n) => format!("<built-in method {n} of {} object>", o.type_name()),
        }
    }

    /// A number as f64 (bools count), for arithmetic.
    pub fn num(&self) -> Option<f64> {
        match self {
            V::Bool(b) => Some(*b as i64 as f64),
            V::Int(i) => Some(*i as f64),
            V::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// An integer (bools count).
    pub fn int(&self) -> Option<i64> {
        match self {
            V::Bool(b) => Some(*b as i64),
            V::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Python's `==`.
    pub fn eq(&self, other: &V<'a>) -> bool {
        match (self, other) {
            (V::None, V::None) => true,
            (V::Str(a), V::Str(b)) => a == b,
            (V::Bytes(a), V::Bytes(b)) => a == b,
            (V::List(a), V::List(b)) => {
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq(y))
            }
            (V::Tuple(a), V::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq(y))
            }
            (V::Dict(a), V::Dict(b)) => {
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.iter().any(|(k2, v2)| k.eq(k2) && v.eq(v2)))
            }
            (V::Obj(a), V::Obj(b)) => Rc::ptr_eq(a, b),
            (V::Builtin(a), V::Builtin(b)) => a == b,
            (a, b) => match (a.int(), b.int()) {
                (Some(x), Some(y)) => x == y,
                _ => match (a.num(), b.num()) {
                    (Some(x), Some(y)) => x == y,
                    _ => false,
                },
            },
        }
    }

    /// Python's ordering (`<`), or the `TypeError`.
    pub fn cmp(&self, other: &V<'a>, op: &str) -> PyResult<std::cmp::Ordering> {
        use std::cmp::Ordering;
        let type_err = || {
            err(
                "TypeError",
                format!(
                    "'{op}' not supported between instances of '{}' and '{}'",
                    self.type_name(),
                    other.type_name()
                ),
            )
        };
        match (self, other) {
            (V::Str(a), V::Str(b)) => Ok(a.cmp(b)),
            (V::Bytes(a), V::Bytes(b)) => Ok(a.cmp(b)),
            (V::List(a), V::List(b)) => seq_cmp(&a.borrow(), &b.borrow(), op),
            (V::Tuple(a), V::Tuple(b)) => seq_cmp(a, b, op),
            (a, b) => match (a.int(), b.int()) {
                (Some(x), Some(y)) => Ok(x.cmp(&y)),
                _ => match (a.num(), b.num()) {
                    (Some(x), Some(y)) => Ok(x.partial_cmp(&y).unwrap_or(Ordering::Equal)),
                    _ => type_err(),
                },
            },
        }
    }

    /// The items of an iterable (`for x in ...`).
    pub fn iter(&self) -> PyResult<Vec<V<'a>>> {
        Ok(match self {
            V::Str(s) => s.chars().map(|c| V::str(c.to_string())).collect(),
            V::Bytes(b) => b.iter().map(|x| V::Int(*x as i64)).collect(),
            V::List(l) => l.borrow().clone(),
            V::Tuple(t) => t.to_vec(),
            V::Dict(d) => d.borrow().iter().map(|(k, _)| k.clone()).collect(),
            other => {
                return err(
                    "TypeError",
                    format!("'{}' object is not iterable", other.type_name()),
                );
            }
        })
    }
}

fn seq_cmp<'a>(a: &[V<'a>], b: &[V<'a>], op: &str) -> PyResult<std::cmp::Ordering> {
    for (x, y) in a.iter().zip(b) {
        if !x.eq(y) {
            return x.cmp(y, op);
        }
    }
    Ok(a.len().cmp(&b.len()))
}

pub fn is_type(n: &str) -> bool {
    matches!(
        n,
        "str" | "int" | "float" | "bool" | "list" | "tuple" | "dict" | "bytes"
    )
}

/// Python's `repr(float)`.
pub fn float_repr(f: f64) -> String {
    crate::pyyaml::py_float_repr(f)
}

// ------------------------------------------------------------ formatting

/// The digits of `x` (finite, >= 0) rounded to `prec` places after the
/// point, as Python does (round half to even on the exact binary value).
fn fixed(x: f64, prec: usize) -> String {
    // Rust's formatting rounds the exact decimal expansion correctly
    // (half to even on exact ties), like Python's.
    format!("{x:.prec$}")
}

/// `x` in exponent form with `prec` digits after the point: mantissa
/// and exponent.
fn expo(x: f64, prec: usize) -> (String, i32) {
    let s = format!("{x:.prec$e}");
    let (m, e) = s.split_once('e').unwrap();
    (m.to_string(), e.parse().unwrap())
}

fn exp_str(m: &str, e: i32, upper: bool) -> String {
    let sign = if e < 0 { '-' } else { '+' };
    let out = format!("{m}e{sign}{:02}", e.abs());
    if upper { out.to_uppercase() } else { out }
}

/// `format(x, 'g')`-style: `prec` significant digits, `alt` keeps zeros.
fn general(x: f64, prec: usize, upper: bool, alt: bool, repr_like: bool) -> String {
    let p = prec.max(1);
    if x == 0.0 {
        let mut s = if repr_like {
            "0.0".to_string()
        } else {
            "0".to_string()
        };
        if alt && p > 1 {
            s = format!("0.{}", "0".repeat(p - 1));
        }
        return s;
    }
    let (_, e) = expo(x, p - 1);
    if -4 <= e && (e as i64) < p as i64 {
        let digits = (p as i32 - 1 - e).max(0) as usize;
        let mut s = fixed(x, digits);
        if !alt && s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        if repr_like && !s.contains('.') {
            s.push_str(".0");
        }
        s
    } else {
        let (mut m, e) = expo(x, p - 1);
        if !alt && m.contains('.') {
            m = m.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        exp_str(&m, e, upper)
    }
}

/// A parsed format spec (`[[fill]align][sign][z][#][0][width][,_][.prec][type]`).
#[derive(Default)]
struct Spec {
    fill: Option<char>,
    align: Option<char>,
    sign: Option<char>,
    alt: bool,
    zero: bool,
    width: usize,
    grouping: Option<char>,
    prec: Option<usize>,
    ty: Option<char>,
}

fn parse_spec(spec: &str) -> PyResult<Spec> {
    let c: Vec<char> = spec.chars().collect();
    let mut s = Spec::default();
    let mut i = 0;
    let is_align = |ch: char| matches!(ch, '<' | '>' | '^' | '=');
    if c.len() >= 2 && is_align(c[1]) {
        s.fill = Some(c[0]);
        s.align = Some(c[1]);
        i = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        s.align = Some(c[0]);
        i = 1;
    }
    if i < c.len() && matches!(c[i], '+' | '-' | ' ') {
        s.sign = Some(c[i]);
        i += 1;
    }
    if i < c.len() && c[i] == 'z' {
        i += 1;
    }
    if i < c.len() && c[i] == '#' {
        s.alt = true;
        i += 1;
    }
    if i < c.len() && c[i] == '0' {
        s.zero = true;
        i += 1;
    }
    let st = i;
    while i < c.len() && c[i].is_ascii_digit() {
        i += 1;
    }
    if i > st {
        s.width = c[st..i].iter().collect::<String>().parse().unwrap_or(0);
    }
    if i < c.len() && matches!(c[i], ',' | '_') {
        s.grouping = Some(c[i]);
        i += 1;
    }
    if i < c.len() && c[i] == '.' {
        i += 1;
        let st = i;
        while i < c.len() && c[i].is_ascii_digit() {
            i += 1;
        }
        if i == st {
            return err("ValueError", "Format specifier missing precision");
        }
        s.prec = Some(c[st..i].iter().collect::<String>().parse().unwrap_or(0));
    }
    if i < c.len() {
        s.ty = Some(c[i]);
        i += 1;
    }
    if i < c.len() {
        return err("ValueError", "Invalid format specifier");
    }
    Ok(s)
}

fn group(int_part: &str, sep: char, every: usize) -> String {
    let digits: Vec<char> = int_part.chars().collect();
    let mut out = String::new();
    for (k, d) in digits.iter().enumerate() {
        if k > 0 && (digits.len() - k).is_multiple_of(every) {
            out.push(sep);
        }
        out.push(*d);
    }
    out
}

fn pad(body: &str, sign: &str, s: &Spec, default_align: char) -> String {
    let fill = s.fill.unwrap_or(if s.zero && s.align.is_none() {
        '0'
    } else {
        ' '
    });
    let align = s.align.unwrap_or(if s.zero { '=' } else { default_align });
    let len = sign.chars().count() + body.chars().count();
    if len >= s.width {
        return format!("{sign}{body}");
    }
    let n = s.width - len;
    let f = |k: usize| fill.to_string().repeat(k);
    match align {
        '<' => format!("{sign}{body}{}", f(n)),
        '^' => format!("{}{sign}{body}{}", f(n / 2), f(n - n / 2)),
        '=' => format!("{sign}{}{body}", f(n)),
        _ => format!("{}{sign}{body}", f(n)),
    }
}

/// `format(value, spec)`.
pub fn format_value(v: &V, spec: &str) -> PyResult<String> {
    if spec.is_empty() {
        return Ok(v.to_str());
    }
    let s = parse_spec(spec)?;
    let unknown = |t: char, ty: &str| {
        err(
            "ValueError",
            format!("Unknown format code '{t}' for object of type '{ty}'"),
        )
    };
    match v {
        V::Int(_) | V::Bool(_) if s.ty.is_none_or(|t| "bcdoxXn".contains(t)) => {
            if matches!(v, V::Bool(_)) && s.ty.is_none() {
                return Ok(pad(&v.to_str(), "", &s, '<'));
            }
            let i = v.int().unwrap();
            let sign = if i < 0 {
                "-"
            } else {
                match s.sign {
                    Some('+') => "+",
                    Some(' ') => " ",
                    _ => "",
                }
            };
            let a = i.unsigned_abs();
            let (prefix, digits) = match s.ty {
                Some('b') => ("0b", format!("{a:b}")),
                Some('o') => ("0o", format!("{a:o}")),
                Some('x') => ("0x", format!("{a:x}")),
                Some('X') => ("0X", format!("{a:X}")),
                Some('c') => {
                    return Ok(pad(
                        &char::from_u32(a as u32).unwrap_or('?').to_string(),
                        "",
                        &s,
                        '<',
                    ));
                }
                _ => ("", a.to_string()),
            };
            let digits = match s.grouping {
                Some(g) => group(&digits, g, if prefix.is_empty() { 3 } else { 4 }),
                None => digits,
            };
            let sign = format!("{sign}{}", if s.alt { prefix } else { "" });
            Ok(pad(&digits, &sign, &s, '>'))
        }
        V::Int(_) | V::Bool(_) | V::Float(_) => {
            let x = v.num().unwrap();
            let ty = s.ty;
            if let Some(t) = ty
                && !"eEfFgGn%".contains(t)
            {
                return unknown(t, v.type_name());
            }
            let neg = x.is_sign_negative() && !(x == 0.0 && !x.is_sign_negative());
            let a = x.abs();
            let sign = if neg && !x.is_nan() {
                "-"
            } else {
                match s.sign {
                    Some('+') => "+",
                    Some(' ') => " ",
                    _ => "",
                }
            };
            let body = if a.is_infinite() {
                if matches!(ty, Some('E' | 'F' | 'G')) {
                    "INF"
                } else {
                    "inf"
                }
                .to_string()
            } else if a.is_nan() {
                if matches!(ty, Some('E' | 'F' | 'G')) {
                    "NAN"
                } else {
                    "nan"
                }
                .to_string()
            } else {
                match ty {
                    Some('f' | 'F') => fixed(a, s.prec.unwrap_or(6)),
                    Some('e' | 'E') => {
                        let (m, e) = expo(a, s.prec.unwrap_or(6));
                        exp_str(&m, e, ty == Some('E'))
                    }
                    Some('%') => format!("{}%", fixed(a * 100.0, s.prec.unwrap_or(6))),
                    Some('g' | 'G' | 'n') => {
                        general(a, s.prec.unwrap_or(6), ty == Some('G'), s.alt, false)
                    }
                    None => match s.prec {
                        Some(p) => general(a, p, false, s.alt, true),
                        None => float_repr(a),
                    },
                    _ => unreachable!(),
                }
            };
            let body = match s.grouping {
                Some(g) if body.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                    let (int, rest) = body
                        .find(|c: char| !c.is_ascii_digit())
                        .map(|k| (&body[..k], &body[k..]))
                        .unwrap_or((&body, ""));
                    format!("{}{rest}", group(int, g, 3))
                }
                _ => body,
            };
            Ok(pad(&body, sign, &s, '>'))
        }
        V::Str(st) => {
            if let Some(t) = s.ty
                && t != 's'
            {
                return unknown(t, "str");
            }
            if s.sign.is_some() {
                return err("ValueError", "Sign not allowed in string format specifier");
            }
            let st: String = match s.prec {
                Some(p) => st.chars().take(p).collect(),
                None => st.to_string(),
            };
            Ok(pad(&st, "", &s, '<'))
        }
        other => {
            if spec.is_empty() {
                Ok(other.to_str())
            } else {
                err(
                    "TypeError",
                    format!(
                        "unsupported format string passed to {}.__format__",
                        other.type_name()
                    ),
                )
            }
        }
    }
}

/// `fmt % args` (printf-style).
pub fn percent_format<'a>(fmt: &str, args: &V<'a>) -> PyResult<String> {
    let mut vals: Vec<V<'a>> = match args {
        V::Tuple(t) => t.to_vec(),
        other => vec![other.clone()],
    };
    let dict = matches!(args, V::Dict(_));
    vals.reverse();
    let c: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut used_dict = false;
    let next = |vals: &mut Vec<V<'a>>| -> PyResult<V<'a>> {
        vals.pop().map_or_else(
            || err("TypeError", "not enough arguments for format string"),
            Ok,
        )
    };
    while i < c.len() {
        if c[i] != '%' {
            out.push(c[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= c.len() {
            return err("ValueError", "incomplete format");
        }
        let mut key: Option<String> = None;
        if c[i] == '(' {
            let st = i + 1;
            while i < c.len() && c[i] != ')' {
                i += 1;
            }
            key = Some(c[st..i].iter().collect());
            i += 1;
        }
        let mut flags = String::new();
        while i < c.len() && "#0- +".contains(c[i]) {
            flags.push(c[i]);
            i += 1;
        }
        let mut width = String::new();
        if i < c.len() && c[i] == '*' {
            width = next(&mut vals)?.int().unwrap_or(0).to_string();
            i += 1;
        }
        while i < c.len() && c[i].is_ascii_digit() {
            width.push(c[i]);
            i += 1;
        }
        let mut prec: Option<usize> = None;
        if i < c.len() && c[i] == '.' {
            i += 1;
            let mut p = String::new();
            if i < c.len() && c[i] == '*' {
                p = next(&mut vals)?.int().unwrap_or(0).to_string();
                i += 1;
            }
            while i < c.len() && c[i].is_ascii_digit() {
                p.push(c[i]);
                i += 1;
            }
            prec = Some(p.parse().unwrap_or(0));
        }
        while i < c.len() && "hlL".contains(c[i]) {
            i += 1;
        }
        let Some(&t) = c.get(i) else {
            return err("ValueError", "incomplete format");
        };
        i += 1;
        if t == '%' {
            out.push('%');
            continue;
        }
        let v = match (&key, args) {
            (Some(k), V::Dict(d)) => {
                used_dict = true;
                let d = d.borrow();
                match d.iter().find(|(kk, _)| kk.eq(&V::str(k.as_str()))) {
                    Some((_, v)) => v.clone(),
                    None => return err("KeyError", crate::hooks::py_repr(k)),
                }
            }
            (Some(_), _) => return err("TypeError", "format requires a mapping"),
            (None, V::Dict(_)) if !dict => next(&mut vals)?,
            (None, _) => next(&mut vals)?,
        };
        let mut spec = String::new();
        if flags.contains('-') {
            spec.push('<');
        } else if flags.contains('0') && !matches!(t, 's' | 'r' | 'a' | 'c') {
            spec.push_str("0=");
        }
        if flags.contains('+') {
            spec.push('+');
        } else if flags.contains(' ') {
            spec.push(' ');
        }
        if flags.contains('#') {
            spec.push('#');
        }
        let spec_w = width.clone();
        let piece = match t {
            's' | 'r' | 'a' => {
                let s = if t == 's' { v.to_str() } else { v.repr() };
                let s: String = match prec {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                };
                let align = if flags.contains('-') { "<" } else { ">" };
                format_value(&V::str(s), &format!("{align}{spec_w}"))?
            }
            'd' | 'i' | 'u' => {
                let n = match &v {
                    V::Float(f) => V::Int(f.trunc() as i64),
                    V::Int(_) | V::Bool(_) => V::Int(v.int().unwrap()),
                    other => {
                        return err(
                            "TypeError",
                            format!(
                                "%{t} format: a real number is required, not {}",
                                other.type_name()
                            ),
                        );
                    }
                };
                format_value(&n, &format!("{spec}{spec_w}d"))?
            }
            'x' | 'X' | 'o' => {
                let Some(n) = v.int() else {
                    return err(
                        "TypeError",
                        format!("%{t} format: an integer is required, not {}", v.type_name()),
                    );
                };
                format_value(&V::Int(n), &format!("{spec}{spec_w}{t}"))?
            }
            'e' | 'E' | 'f' | 'F' | 'g' | 'G' => {
                let Some(x) = v.num() else {
                    return err(
                        "TypeError",
                        format!("must be real number, not {}", v.type_name()),
                    );
                };
                let p = prec.unwrap_or(6);
                format_value(&V::Float(x), &format!("{spec}{spec_w}.{p}{t}"))?
            }
            'c' => {
                let ch = match &v {
                    V::Int(n) => char::from_u32(*n as u32).unwrap_or('?').to_string(),
                    V::Str(s) if s.chars().count() == 1 => s.to_string(),
                    _ => return err("TypeError", "%c requires an int or a unicode character"),
                };
                let align = if flags.contains('-') { "<" } else { ">" };
                format_value(&V::str(ch), &format!("{align}{spec_w}"))?
            }
            other => {
                return err(
                    "ValueError",
                    format!(
                        "unsupported format character '{other}' (0x{:x}) at index {}",
                        other as u32,
                        i - 1
                    ),
                );
            }
        };
        out.push_str(&piece);
    }
    if !vals.is_empty() && !dict && !used_dict {
        return err(
            "TypeError",
            "not all arguments converted during string formatting",
        );
    }
    Ok(out)
}
