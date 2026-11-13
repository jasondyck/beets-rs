//! Evaluating the parsed subset of Python, as CPython would.

use super::parser::{CompClause, Expr, FPart, Stmt, Target};
use super::value::{Obj, PyErr, PyResult, V, err, format_value, percent_format};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Control flow out of a statement.
enum Flow<'a> {
    Normal,
    Return(V<'a>),
    Break,
    Continue,
}

pub struct Interp<'a, 'g> {
    globals: &'g HashMap<String, V<'a>>,
    /// The function's locals (None while evaluating an expression).
    locals: Option<HashMap<String, V<'a>>>,
    /// Names the function body assigns (local throughout, as in Python).
    local_names: HashSet<String>,
    /// Comprehension scopes, innermost last.
    scopes: Vec<HashMap<String, V<'a>>>,
    steps: usize,
}

const BUILTINS: &[&str] = &[
    "len",
    "str",
    "int",
    "float",
    "bool",
    "round",
    "min",
    "max",
    "abs",
    "sum",
    "sorted",
    "list",
    "tuple",
    "dict",
    "range",
    "enumerate",
    "zip",
    "any",
    "all",
    "repr",
    "ord",
    "chr",
    "divmod",
    "pow",
    "reversed",
    "isinstance",
    "bytes",
    "format",
    "hex",
    "oct",
    "bin",
];

/// Statement steps before giving up (a `while` that never ends).
const MAX_STEPS: usize = 1_000_000;

fn assigned_names(body: &[Stmt], out: &mut HashSet<String>) {
    fn target(t: &Target, out: &mut HashSet<String>) {
        match t {
            Target::Name(n) => {
                out.insert(n.clone());
            }
            Target::Tuple(ts) => ts.iter().for_each(|t| target(t, out)),
            Target::Index(..) => {}
        }
    }
    for s in body {
        match s {
            Stmt::Assign(ts, _) => ts.iter().for_each(|t| target(t, out)),
            Stmt::AugAssign(t, _, _) => target(t, out),
            Stmt::If(branches, orelse) => {
                for (_, b) in branches {
                    assigned_names(b, out);
                }
                assigned_names(orelse, out);
            }
            Stmt::For(t, _, b, o) => {
                target(t, out);
                assigned_names(b, out);
                assigned_names(o, out);
            }
            Stmt::While(_, b, o) => {
                assigned_names(b, out);
                assigned_names(o, out);
            }
            _ => {}
        }
    }
}

impl<'a, 'g> Interp<'a, 'g> {
    pub fn new(globals: &'g HashMap<String, V<'a>>) -> Self {
        Interp {
            globals,
            locals: None,
            local_names: HashSet::new(),
            scopes: vec![],
            steps: 0,
        }
    }

    /// Run a function body (`def f(db_obj): ...`); its return value.
    pub fn run_body(&mut self, body: &[Stmt], db_obj: V<'a>) -> PyResult<V<'a>> {
        let mut names = HashSet::new();
        assigned_names(body, &mut names);
        self.local_names = names;
        let mut locals = HashMap::new();
        locals.insert("db_obj".to_string(), db_obj);
        self.local_names.insert("db_obj".to_string());
        self.locals = Some(locals);
        match self.block(body)? {
            Flow::Return(v) => Ok(v),
            _ => Ok(V::None),
        }
    }

    fn block(&mut self, body: &[Stmt]) -> PyResult<Flow<'a>> {
        for s in body {
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return err("RuntimeError", "inline code ran for too long");
            }
            match self.stmt(s)? {
                Flow::Normal => {}
                f => return Ok(f),
            }
        }
        Ok(Flow::Normal)
    }

    fn stmt(&mut self, s: &Stmt) -> PyResult<Flow<'a>> {
        match s {
            Stmt::Pass => Ok(Flow::Normal),
            Stmt::Break => Ok(Flow::Break),
            Stmt::Continue => Ok(Flow::Continue),
            Stmt::Return(e) => Ok(Flow::Return(match e {
                Some(e) => self.eval(e)?,
                None => V::None,
            })),
            Stmt::Expr(e) => {
                self.eval(e)?;
                Ok(Flow::Normal)
            }
            Stmt::Assign(targets, e) => {
                let v = self.eval(e)?;
                for t in targets {
                    self.assign(t, v.clone())?;
                }
                Ok(Flow::Normal)
            }
            Stmt::AugAssign(t, op, e) => {
                let cur = match t {
                    Target::Name(n) => self.lookup(n)?,
                    Target::Index(b, i) => {
                        let b = self.eval(b)?;
                        let i = self.eval(i)?;
                        index(&b, &i)?
                    }
                    Target::Tuple(_) => unreachable!(),
                };
                let r = self.eval(e)?;
                // `+=` on a list extends it in place.
                let v = match (&cur, *op) {
                    (V::List(l), "+") => {
                        let items = r.iter()?;
                        l.borrow_mut().extend(items);
                        cur.clone()
                    }
                    _ => binary(op, &cur, &r)?,
                };
                self.assign(t, v)?;
                Ok(Flow::Normal)
            }
            Stmt::If(branches, orelse) => {
                for (c, b) in branches {
                    if self.eval(c)?.truthy() {
                        return self.block(b);
                    }
                }
                self.block(orelse)
            }
            Stmt::For(t, iter, body, orelse) => {
                let items = self.eval(iter)?.iter()?;
                for it in items {
                    self.assign(t, it)?;
                    match self.block(body)? {
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
                self.block(orelse)
            }
            Stmt::While(c, body, orelse) => {
                while self.eval(c)?.truthy() {
                    self.steps += 1;
                    if self.steps > MAX_STEPS {
                        return err("RuntimeError", "inline code ran for too long");
                    }
                    match self.block(body)? {
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
                self.block(orelse)
            }
        }
    }

    // Python semantics: indexes, lengths and int() truncate/wrap like CPython.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    fn assign(&mut self, t: &Target, v: V<'a>) -> PyResult<()> {
        match t {
            Target::Name(n) => {
                if let Some(scope) = self.scopes.last_mut()
                    && scope.contains_key(n)
                {
                    scope.insert(n.clone(), v);
                } else if let Some(l) = &mut self.locals {
                    l.insert(n.clone(), v);
                }
                Ok(())
            }
            Target::Tuple(ts) => {
                let items = v.iter()?;
                if items.len() != ts.len() {
                    return if items.len() > ts.len() {
                        err(
                            "ValueError",
                            format!("too many values to unpack (expected {})", ts.len()),
                        )
                    } else {
                        err(
                            "ValueError",
                            format!(
                                "not enough values to unpack (expected {}, got {})",
                                ts.len(),
                                items.len()
                            ),
                        )
                    };
                }
                for (t, it) in ts.iter().zip(items) {
                    self.assign(t, it)?;
                }
                Ok(())
            }
            Target::Index(b, i) => {
                let b = self.eval(b)?;
                let i = self.eval(i)?;
                match &b {
                    V::List(l) => {
                        let mut l = l.borrow_mut();
                        let n = l.len() as i64;
                        let Some(k) = i.int() else {
                            return err("TypeError", "list indices must be integers or slices");
                        };
                        let k = if k < 0 { k + n } else { k };
                        if k < 0 || k >= n {
                            return err("IndexError", "list assignment index out of range");
                        }
                        l[k as usize] = v;
                        Ok(())
                    }
                    V::Dict(d) => {
                        let mut d = d.borrow_mut();
                        match d.iter_mut().find(|(k, _)| k.eq(&i)) {
                            Some(p) => p.1 = v,
                            None => d.push((i, v)),
                        }
                        Ok(())
                    }
                    other => err(
                        "TypeError",
                        format!(
                            "'{}' object does not support item assignment",
                            other.type_name()
                        ),
                    ),
                }
            }
        }
    }

    fn lookup(&self, n: &str) -> PyResult<V<'a>> {
        for s in self.scopes.iter().rev() {
            if let Some(v) = s.get(n) {
                return Ok(v.clone());
            }
        }
        if let Some(l) = &self.locals
            && self.local_names.contains(n)
        {
            return match l.get(n) {
                Some(v) => Ok(v.clone()),
                None => err(
                    "UnboundLocalError",
                    format!(
                        "cannot access local variable '{n}' where it is not associated with a value"
                    ),
                ),
            };
        }
        if let Some(v) = self.globals.get(n) {
            return Ok(v.clone());
        }
        if let Some(b) = BUILTINS.iter().find(|b| **b == n) {
            return Ok(V::Builtin(b));
        }
        match n {
            "True" => Ok(V::Bool(true)),
            "False" => Ok(V::Bool(false)),
            _ => err("NameError", format!("name '{n}' is not defined")),
        }
    }

    pub fn eval(&mut self, e: &Expr) -> PyResult<V<'a>> {
        match e {
            Expr::Name(n) => self.lookup(n),
            Expr::None => Ok(V::None),
            Expr::Bool(b) => Ok(V::Bool(*b)),
            Expr::Int(i) => Ok(V::Int(*i)),
            Expr::Float(f) => Ok(V::Float(*f)),
            Expr::Str(s) => Ok(V::str(s.as_str())),
            Expr::Bytes(b) => Ok(V::Bytes(Rc::new(b.clone()))),
            Expr::FStr(parts) => Ok(V::str(self.fstring(parts)?)),
            Expr::List(items) => {
                let v = items
                    .iter()
                    .map(|i| self.eval(i))
                    .collect::<PyResult<_>>()?;
                Ok(V::list(v))
            }
            Expr::Tuple(items) => {
                let v = items
                    .iter()
                    .map(|i| self.eval(i))
                    .collect::<PyResult<_>>()?;
                Ok(V::tuple(v))
            }
            Expr::Dict(pairs) => {
                let mut out: Vec<(V<'a>, V<'a>)> = vec![];
                for (k, v) in pairs {
                    let k = self.eval(k)?;
                    let v = self.eval(v)?;
                    match out.iter_mut().find(|(kk, _)| kk.eq(&k)) {
                        Some(p) => p.1 = v,
                        None => out.push((k, v)),
                    }
                }
                Ok(V::Dict(Rc::new(RefCell::new(out))))
            }
            Expr::Attr(b, name) => {
                let b = self.eval(b)?;
                getattr(&b, name)
            }
            Expr::Call(f, args, kwargs) => {
                let f = self.eval(f)?;
                let args: Vec<V<'a>> =
                    args.iter().map(|a| self.eval(a)).collect::<PyResult<_>>()?;
                let kwargs: Vec<(String, V<'a>)> = kwargs
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), self.eval(v)?)))
                    .collect::<PyResult<_>>()?;
                call(&f, args, kwargs)
            }
            Expr::Index(b, i) => {
                let b = self.eval(b)?;
                let i = self.eval(i)?;
                index(&b, &i)
            }
            Expr::Slice(b, start, stop, step) => {
                let b = self.eval(b)?;
                let mut part = |x: &Option<Box<Expr>>| -> PyResult<Option<i64>> {
                    match x {
                        None => Ok(None),
                        Some(e) => match self.eval(e)? {
                            V::None => Ok(None),
                            v => v.int().map(Some).ok_or(PyErr {
                                kind: "TypeError",
                                msg: "slice indices must be integers or None or have an __index__ method".into(),
                            }),
                        },
                    }
                };
                let (a, z, s) = (part(start)?, part(stop)?, part(step)?);
                slice(&b, a, z, s)
            }
            Expr::Unary(op, x) => {
                let v = self.eval(x)?;
                unary(op, &v)
            }
            Expr::Binary(op, l, r) => {
                let l = self.eval(l)?;
                let r = self.eval(r)?;
                binary(op, &l, &r)
            }
            Expr::Compare(first, rest) => {
                let mut left = self.eval(first)?;
                for (op, e) in rest {
                    let right = self.eval(e)?;
                    if !compare(op, &left, &right)? {
                        return Ok(V::Bool(false));
                    }
                    left = right;
                }
                Ok(V::Bool(true))
            }
            Expr::And(l, r) => {
                let l = self.eval(l)?;
                if !l.truthy() { Ok(l) } else { self.eval(r) }
            }
            Expr::Or(l, r) => {
                let l = self.eval(l)?;
                if l.truthy() { Ok(l) } else { self.eval(r) }
            }
            Expr::IfElse(c, a, b) => {
                if self.eval(c)?.truthy() {
                    self.eval(a)
                } else {
                    self.eval(b)
                }
            }
            Expr::Comp { elt, clauses, list } => {
                let mut out = vec![];
                self.scopes.push(HashMap::new());
                let r = self.comp(elt, clauses, &mut out);
                self.scopes.pop();
                r?;
                // A generator is consumed at once (as a list).
                let _ = list;
                Ok(V::list(out))
            }
        }
    }

    fn comp(&mut self, elt: &Expr, clauses: &[CompClause], out: &mut Vec<V<'a>>) -> PyResult<()> {
        let Some((first, rest)) = clauses.split_first() else {
            out.push(self.eval(elt)?);
            return Ok(());
        };
        match first {
            CompClause::If(c) => {
                if self.eval(c)?.truthy() {
                    self.comp(elt, rest, out)?;
                }
            }
            CompClause::For(t, iter) => {
                let items = self.eval(iter)?.iter()?;
                for it in items {
                    self.bind_scope(t, it)?;
                    self.comp(elt, rest, out)?;
                }
            }
        }
        Ok(())
    }

    fn bind_scope(&mut self, t: &Target, v: V<'a>) -> PyResult<()> {
        match t {
            Target::Name(n) => {
                self.scopes
                    .last_mut()
                    .expect("BUG: the interpreter always has the global scope")
                    .insert(n.clone(), v);
                Ok(())
            }
            Target::Tuple(ts) => {
                let items = v.iter()?;
                if items.len() != ts.len() {
                    return err("ValueError", "wrong number of values to unpack");
                }
                for (t, it) in ts.iter().zip(items) {
                    self.bind_scope(t, it)?;
                }
                Ok(())
            }
            Target::Index(..) => err("SyntaxError", "invalid comprehension target"),
        }
    }

    fn fstring(&mut self, parts: &[FPart]) -> PyResult<String> {
        let mut out = String::new();
        for p in parts {
            match p {
                FPart::Lit(s) => out.push_str(s),
                FPart::Field { expr, conv, spec } => {
                    let v = self.eval(expr)?;
                    let v = match conv {
                        Some('r') | Some('a') => V::str(v.repr()),
                        Some('s') => V::str(v.to_str()),
                        _ => v,
                    };
                    let spec = self.fstring(spec)?;
                    out.push_str(&format_value(&v, &spec)?);
                }
            }
        }
        Ok(out)
    }
}

// ------------------------------------------------------------- operators

fn type_err<T>(op: &str, a: &V, b: &V) -> PyResult<T> {
    err(
        "TypeError",
        format!(
            "unsupported operand type(s) for {op}: '{}' and '{}'",
            a.type_name(),
            b.type_name()
        ),
    )
}

fn overflow<T>() -> PyResult<T> {
    err(
        "OverflowError",
        "integer too large (beets-rs' inline uses 64-bit integers)",
    )
}

fn repeat<'a>(seq: &V<'a>, n: i64) -> V<'a> {
    let n = n.max(0) as usize;
    match seq {
        V::Str(s) => V::str(s.repeat(n)),
        V::Bytes(b) => V::Bytes(Rc::new(b.repeat(n))),
        V::List(l) => V::list((0..n).flat_map(|_| l.borrow().clone()).collect()),
        V::Tuple(t) => V::tuple((0..n).flat_map(|_| t.iter().cloned()).collect()),
        _ => unreachable!(),
    }
}

fn floor_div(a: f64, b: f64) -> f64 {
    (a / b).floor()
}

fn py_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) {
        r + b
    } else {
        r
    }
}

// Python semantics: indexes, lengths and int() truncate/wrap like CPython.
#[allow(clippy::cast_precision_loss)]
pub fn binary<'a>(op: &str, a: &V<'a>, b: &V<'a>) -> PyResult<V<'a>> {
    // Integers (bools count) stay integers.
    if let (Some(x), Some(y)) = (a.int(), b.int()) {
        let r = match op {
            "+" => x.checked_add(y),
            "-" => x.checked_sub(y),
            "*" => x.checked_mul(y),
            "/" => {
                if y == 0 {
                    return err("ZeroDivisionError", "division by zero");
                }
                return Ok(V::Float(x as f64 / y as f64));
            }
            "//" => {
                if y == 0 {
                    return err("ZeroDivisionError", "integer division or modulo by zero");
                }
                let q = x.checked_div(y);
                q.map(|q| {
                    if x % y != 0 && ((x < 0) != (y < 0)) {
                        q - 1
                    } else {
                        q
                    }
                })
            }
            "%" => {
                if y == 0 {
                    return err("ZeroDivisionError", "integer modulo by zero");
                }
                let r = x % y;
                Some(if r != 0 && ((r < 0) != (y < 0)) {
                    r + y
                } else {
                    r
                })
            }
            "**" => {
                if y < 0 {
                    if x == 0 {
                        return err("ZeroDivisionError", "zero to a negative power");
                    }
                    return Ok(V::Float((x as f64).powf(y as f64)));
                }
                u32::try_from(y).ok().and_then(|y| x.checked_pow(y))
            }
            "&" => Some(x & y),
            "|" => Some(x | y),
            "^" => Some(x ^ y),
            "<<" => {
                if y < 0 {
                    return err("ValueError", "negative shift count");
                }
                u32::try_from(y).ok().and_then(|y| x.checked_shl(y))
            }
            ">>" => {
                if y < 0 {
                    return err("ValueError", "negative shift count");
                }
                Some(x >> y.min(63))
            }
            _ => None,
        };
        return match r {
            Some(v) => Ok(
                if matches!((a, b), (V::Bool(_), V::Bool(_))) && matches!(op, "&" | "|" | "^") {
                    V::Bool(v != 0)
                } else {
                    V::Int(v)
                },
            ),
            None => overflow(),
        };
    }
    if let (Some(x), Some(y)) = (a.num(), b.num()) {
        if matches!(op, "&" | "|" | "^" | "<<" | ">>") {
            return type_err(op, a, b);
        }
        let zero = |m: &str| err("ZeroDivisionError", m.to_string());
        return Ok(V::Float(match op {
            "+" => x + y,
            "-" => x - y,
            "*" => x * y,
            "/" => {
                if y == 0.0 {
                    return zero("division by zero");
                }
                x / y
            }
            "//" => {
                if y == 0.0 {
                    return zero("float floor division by zero");
                }
                floor_div(x, y)
            }
            "%" => {
                if y == 0.0 {
                    return zero("float modulo by zero");
                }
                py_mod(x, y)
            }
            "**" => {
                if x == 0.0 && y < 0.0 {
                    return zero("zero to a negative power");
                }
                x.powf(y)
            }
            _ => return type_err(op, a, b),
        }));
    }
    match (op, a, b) {
        ("+", V::Str(x), V::Str(y)) => Ok(V::str(format!("{x}{y}"))),
        ("+", V::Str(_), other) => err(
            "TypeError",
            format!(
                "can only concatenate str (not \"{}\") to str",
                other.type_name()
            ),
        ),
        ("+", V::Bytes(x), V::Bytes(y)) => {
            let mut v = x.to_vec();
            v.extend_from_slice(y);
            Ok(V::Bytes(Rc::new(v)))
        }
        ("+", V::List(x), V::List(y)) => {
            let mut v = x.borrow().clone();
            v.extend(y.borrow().iter().cloned());
            Ok(V::list(v))
        }
        ("+", V::List(_), other) => err(
            "TypeError",
            format!(
                "can only concatenate list (not \"{}\") to list",
                other.type_name()
            ),
        ),
        ("+", V::Tuple(x), V::Tuple(y)) => {
            let mut v = x.to_vec();
            v.extend(y.iter().cloned());
            Ok(V::tuple(v))
        }
        ("*", s @ (V::Str(_) | V::Bytes(_) | V::List(_) | V::Tuple(_)), n)
        | ("*", n, s @ (V::Str(_) | V::Bytes(_) | V::List(_) | V::Tuple(_)))
            if n.int().is_some() =>
        {
            Ok(repeat(
                s,
                n.int().expect("BUG: the guard checked n.int() is Some"),
            ))
        }
        ("*", V::Str(_), V::Str(_)) => err(
            "TypeError",
            "can't multiply sequence by non-int of type 'str'",
        ),
        ("%", V::Str(f), args) => Ok(V::str(percent_format(f, args)?)),
        _ => type_err(op, a, b),
    }
}

fn unary<'a>(op: &str, v: &V<'a>) -> PyResult<V<'a>> {
    match (op, v) {
        ("not", v) => Ok(V::Bool(!v.truthy())),
        ("-", V::Float(f)) => Ok(V::Float(-f)),
        ("+", V::Float(f)) => Ok(V::Float(*f)),
        ("-", v) if v.int().is_some() => v
            .int()
            .expect("BUG: the guard checked v.int() is Some")
            .checked_neg()
            .map(V::Int)
            .map_or_else(overflow, Ok),
        ("+", v) if v.int().is_some() => Ok(V::Int(
            v.int().expect("BUG: the guard checked v.int() is Some"),
        )),
        ("~", v) if v.int().is_some() => Ok(V::Int(
            !v.int().expect("BUG: the guard checked v.int() is Some"),
        )),
        (op, v) => err(
            "TypeError",
            format!("bad operand type for unary {op}: '{}'", v.type_name()),
        ),
    }
}

fn contains<'a>(container: &V<'a>, x: &V<'a>) -> PyResult<bool> {
    match container {
        V::Str(s) => match x {
            V::Str(sub) => Ok(s.contains(&**sub)),
            other => err(
                "TypeError",
                format!(
                    "'in <string>' requires string as left operand, not {}",
                    other.type_name()
                ),
            ),
        },
        V::Bytes(b) => match x {
            V::Bytes(sub) => Ok(sub.is_empty() || b.windows(sub.len()).any(|w| w == &sub[..])),
            V::Int(i) => Ok(b.contains(&(*i as u8))),
            _ => err("TypeError", "a bytes-like object is required"),
        },
        V::List(_) | V::Tuple(_) | V::Dict(_) => Ok(container.iter()?.iter().any(|v| v.eq(x))),
        other => err(
            "TypeError",
            format!("argument of type '{}' is not iterable", other.type_name()),
        ),
    }
}

fn compare<'a>(op: &str, a: &V<'a>, b: &V<'a>) -> PyResult<bool> {
    use std::cmp::Ordering::*;
    Ok(match op {
        "==" => a.eq(b),
        "!=" => !a.eq(b),
        "<" => a.cmp(b, op)? == Less,
        "<=" => a.cmp(b, op)? != Greater,
        ">" => a.cmp(b, op)? == Greater,
        ">=" => a.cmp(b, op)? != Less,
        "in" => contains(b, a)?,
        "not in" => !contains(b, a)?,
        // Identity: the same singleton or object.
        "is" | "is not" => {
            let same = match (a, b) {
                (V::None, V::None) => true,
                (V::Bool(x), V::Bool(y)) => x == y,
                (V::Obj(x), V::Obj(y)) => Rc::ptr_eq(x, y),
                (V::List(x), V::List(y)) => Rc::ptr_eq(x, y),
                (V::Builtin(x), V::Builtin(y)) => x == y,
                (V::Int(x), V::Int(y)) => x == y && (-5..=256).contains(x),
                (V::Str(x), V::Str(y)) => Rc::ptr_eq(x, y) || (x == y && x.len() <= 1),
                _ => false,
            };
            if op == "is" { same } else { !same }
        }
        _ => unreachable!(),
    })
}

// Python semantics: indexes, lengths and int() truncate/wrap like CPython.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn norm_index(i: i64, len: usize) -> Option<usize> {
    let n = len as i64;
    let k = if i < 0 { i + n } else { i };
    (0..n).contains(&k).then_some(k as usize)
}

pub fn index<'a>(b: &V<'a>, i: &V<'a>) -> PyResult<V<'a>> {
    match b {
        V::Dict(d) => {
            let d = d.borrow();
            match d.iter().find(|(k, _)| k.eq(i)) {
                Some((_, v)) => Ok(v.clone()),
                None => err("KeyError", i.repr()),
            }
        }
        V::Str(_) | V::Bytes(_) | V::List(_) | V::Tuple(_) => {
            let Some(k) = i.int() else {
                return err(
                    "TypeError",
                    format!(
                        "{} indices must be integers or slices, not {}",
                        if matches!(b, V::Str(_)) {
                            "string"
                        } else {
                            b.type_name()
                        },
                        i.type_name()
                    ),
                );
            };
            let what = match b {
                V::Str(_) => "string",
                V::Bytes(_) => "index",
                V::List(_) => "list",
                _ => "tuple",
            };
            let out_of_range = || err("IndexError", format!("{what} index out of range"));
            match b {
                V::Str(s) => {
                    let chars: Vec<char> = s.chars().collect();
                    match norm_index(k, chars.len()) {
                        Some(k) => Ok(V::str(chars[k].to_string())),
                        None => out_of_range(),
                    }
                }
                V::Bytes(x) => match norm_index(k, x.len()) {
                    Some(k) => Ok(V::Int(i64::from(x[k]))),
                    None => out_of_range(),
                },
                V::List(l) => {
                    let l = l.borrow();
                    match norm_index(k, l.len()) {
                        Some(k) => Ok(l[k].clone()),
                        None => out_of_range(),
                    }
                }
                V::Tuple(t) => match norm_index(k, t.len()) {
                    Some(k) => Ok(t[k].clone()),
                    None => out_of_range(),
                },
                _ => unreachable!(),
            }
        }
        other => err(
            "TypeError",
            format!("'{}' object is not subscriptable", other.type_name()),
        ),
    }
}

/// Python's slice indices for a sequence of `len`.
// Python semantics: indexes, lengths and int() truncate/wrap like CPython.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn slice_indices(
    len: usize,
    a: Option<i64>,
    z: Option<i64>,
    s: Option<i64>,
) -> PyResult<Vec<usize>> {
    let n = len as i64;
    let step = s.unwrap_or(1);
    if step == 0 {
        return err("ValueError", "slice step cannot be zero");
    }
    let clamp = |x: i64, lo: i64, hi: i64| x.max(lo).min(hi);
    let norm = |x: i64| if x < 0 { x + n } else { x };
    let mut out = vec![];
    if step > 0 {
        let start = clamp(a.map_or(0, norm), 0, n);
        let stop = clamp(z.map_or(n, norm), 0, n);
        let mut k = start;
        while k < stop {
            out.push(k as usize);
            k += step;
        }
    } else {
        let start = clamp(a.map_or(n - 1, norm), -1, n - 1);
        let stop = clamp(z.map_or(-1, |x| norm(x).max(-1)), -1, n - 1);
        let stop = if z.is_some() && norm(z.expect("BUG: the guard checked z is Some")) < 0 {
            -1
        } else {
            stop
        };
        let mut k = start;
        while k > stop {
            out.push(k as usize);
            k += step;
        }
    }
    Ok(out)
}

fn slice<'a>(b: &V<'a>, a: Option<i64>, z: Option<i64>, s: Option<i64>) -> PyResult<V<'a>> {
    match b {
        V::Str(st) => {
            let chars: Vec<char> = st.chars().collect();
            let idx = slice_indices(chars.len(), a, z, s)?;
            Ok(V::str(idx.iter().map(|k| chars[*k]).collect::<String>()))
        }
        V::Bytes(x) => {
            let idx = slice_indices(x.len(), a, z, s)?;
            Ok(V::Bytes(Rc::new(idx.iter().map(|k| x[*k]).collect())))
        }
        V::List(l) => {
            let l = l.borrow();
            let idx = slice_indices(l.len(), a, z, s)?;
            Ok(V::list(idx.iter().map(|k| l[*k].clone()).collect()))
        }
        V::Tuple(t) => {
            let idx = slice_indices(t.len(), a, z, s)?;
            Ok(V::tuple(idx.iter().map(|k| t[*k].clone()).collect()))
        }
        other => err(
            "TypeError",
            format!("'{}' object is not subscriptable", other.type_name()),
        ),
    }
}

// ----------------------------------------------------------- attributes

const STR_METHODS: &[&str] = &[
    "lower",
    "upper",
    "strip",
    "lstrip",
    "rstrip",
    "startswith",
    "endswith",
    "replace",
    "split",
    "rsplit",
    "join",
    "title",
    "capitalize",
    "format",
    "find",
    "rfind",
    "index",
    "rindex",
    "count",
    "isdigit",
    "isalpha",
    "isnumeric",
    "isalnum",
    "isspace",
    "islower",
    "isupper",
    "isdecimal",
    "zfill",
    "ljust",
    "rjust",
    "center",
    "splitlines",
    "partition",
    "rpartition",
    "casefold",
    "swapcase",
    "encode",
    "removeprefix",
    "removesuffix",
];
const LIST_METHODS: &[&str] = &[
    "append", "extend", "index", "count", "pop", "insert", "remove", "sort", "reverse", "copy",
];
const DICT_METHODS: &[&str] = &["get", "keys", "values", "items"];
const BYTES_METHODS: &[&str] = &["decode", "startswith", "endswith", "lower", "upper"];

fn getattr<'a>(b: &V<'a>, name: &str) -> PyResult<V<'a>> {
    let method = match b {
        V::Str(_) => STR_METHODS.contains(&name),
        V::List(_) => LIST_METHODS.contains(&name),
        V::Tuple(_) => matches!(name, "index" | "count"),
        V::Dict(_) => DICT_METHODS.contains(&name),
        V::Bytes(_) => BYTES_METHODS.contains(&name),
        V::Float(_) => name == "is_integer",
        V::Obj(o) => o.has_method(name),
        _ => false,
    };
    if method {
        return Ok(V::Method(Box::new(b.clone()), name.into()));
    }
    if let V::Obj(o) = b {
        return o.getattr(name);
    }
    err(
        "AttributeError",
        format!("'{}' object has no attribute '{name}'", b.type_name()),
    )
}

// ------------------------------------------------------------------ calls

fn arity<T>(name: &str, min: usize, max: usize, n: usize) -> PyResult<T> {
    let msg = if min == max {
        if min == 0 {
            format!("{name}() takes no arguments ({n} given)")
        } else if min == 1 {
            format!("{name}() takes exactly one argument ({n} given)")
        } else {
            format!("{name}() takes exactly {min} arguments ({n} given)")
        }
    } else if n < min {
        format!(
            "{name} expected at least {min} argument{}, got {n}",
            if min == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{name} expected at most {max} argument{}, got {n}",
            if max == 1 { "" } else { "s" }
        )
    };
    err("TypeError", msg)
}

fn check<'a>(name: &str, args: &[V<'a>], min: usize, max: usize) -> PyResult<()> {
    if args.len() < min || args.len() > max {
        arity(name, min, max, args.len())
    } else {
        Ok(())
    }
}

fn want_str<'a>(v: &V<'a>, what: &str) -> PyResult<String> {
    match v {
        V::Str(s) => Ok(s.to_string()),
        other => err(
            "TypeError",
            format!("{what} must be str, not {}", other.type_name()),
        ),
    }
}

fn py_int_from_str(s: &str) -> Option<i64> {
    let t = s.trim().replace('_', "");
    t.parse::<i64>().ok()
}

fn py_float_from_str(s: &str) -> Option<f64> {
    let t = s.trim().replace('_', "").to_lowercase();
    match t.as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
        "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
        "nan" | "+nan" | "-nan" => Some(f64::NAN),
        _ => t.parse().ok(),
    }
}

/// Python's `round(x)` (half to even) and `round(x, n)`.
fn py_round(x: f64, n: Option<i64>) -> f64 {
    match n {
        None | Some(0) => {
            let r = x.round();
            if (x - x.trunc()).abs() == 0.5 {
                2.0 * (x / 2.0).round()
            } else {
                r
            }
        }
        Some(n) => {
            // Correctly rounded through the shortest decimal expansion.
            let s = format!("{:.*}", n.max(0) as usize, x);
            s.parse().unwrap_or(x)
        }
    }
}

pub fn call<'a>(f: &V<'a>, args: Vec<V<'a>>, kwargs: Vec<(String, V<'a>)>) -> PyResult<V<'a>> {
    match f {
        V::Builtin(name) => builtin(name, args, kwargs),
        V::Method(obj, name) => method(obj, name, args, kwargs),
        other => err(
            "TypeError",
            format!("'{}' object is not callable", other.type_name()),
        ),
    }
}

// Python semantics: indexes, lengths and int() truncate/wrap like CPython.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn builtin<'a>(name: &str, args: Vec<V<'a>>, kwargs: Vec<(String, V<'a>)>) -> PyResult<V<'a>> {
    let kw = |k: &str| kwargs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    if let Some((k, _)) = kwargs.iter().find(|(k, _)| {
        !matches!(
            (name, k.as_str()),
            ("sorted" | "min" | "max", "reverse" | "default") | ("round", "ndigits")
        )
    }) {
        return err(
            "TypeError",
            format!("{name}() got an unexpected keyword argument '{k}'"),
        );
    }
    match name {
        "len" => {
            check("len", &args, 1, 1)?;
            Ok(V::Int(match &args[0] {
                V::Str(s) => s.chars().count(),
                V::Bytes(b) => b.len(),
                V::List(l) => l.borrow().len(),
                V::Tuple(t) => t.len(),
                V::Dict(d) => d.borrow().len(),
                other => {
                    return err(
                        "TypeError",
                        format!("object of type '{}' has no len()", other.type_name()),
                    );
                }
            } as i64))
        }
        "str" => {
            check("str", &args, 0, 3)?;
            Ok(match args.first() {
                None => V::str(""),
                Some(V::Bytes(b)) if args.len() > 1 => V::str(String::from_utf8_lossy(b).as_ref()),
                Some(v) => V::str(v.to_str()),
            })
        }
        "repr" => {
            check("repr", &args, 1, 1)?;
            Ok(V::str(args[0].repr()))
        }
        "int" => {
            check("int", &args, 0, 2)?;
            let Some(v) = args.first() else {
                return Ok(V::Int(0));
            };
            if let Some(base) = args.get(1) {
                let s = want_str(
                    v,
                    "int() can't convert non-string with explicit base; argument",
                )?;
                let b = base.int().unwrap_or(10) as u32;
                let t = s.trim().to_lowercase();
                let t = t
                    .strip_prefix("0x")
                    .filter(|_| b == 16)
                    .or_else(|| t.strip_prefix("0o").filter(|_| b == 8))
                    .or_else(|| t.strip_prefix("0b").filter(|_| b == 2))
                    .unwrap_or(&t);
                return i64::from_str_radix(t, b).map(V::Int).map_err(|_| PyErr {
                    kind: "ValueError",
                    msg: format!(
                        "invalid literal for int() with base {b}: {}",
                        crate::hooks::py_repr(&s)
                    ),
                });
            }
            match v {
                V::Int(_) | V::Bool(_) => Ok(V::Int(
                    v.int().expect("BUG: Int and Bool values have int()"),
                )),
                V::Float(f) => {
                    if f.is_nan() {
                        err("ValueError", "cannot convert float NaN to integer")
                    } else if f.is_infinite() {
                        err("OverflowError", "cannot convert float infinity to integer")
                    } else {
                        Ok(V::Int(f.trunc() as i64))
                    }
                }
                V::Str(s) => py_int_from_str(s).map(V::Int).ok_or(PyErr {
                    kind: "ValueError",
                    msg: format!(
                        "invalid literal for int() with base 10: {}",
                        crate::hooks::py_repr(s)
                    ),
                }),
                V::Bytes(b) => py_int_from_str(&String::from_utf8_lossy(b))
                    .map(V::Int)
                    .ok_or(PyErr {
                        kind: "ValueError",
                        msg: format!("invalid literal for int() with base 10: {}", args[0].repr()),
                    }),
                other => err(
                    "TypeError",
                    format!(
                        "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                        other.type_name()
                    ),
                ),
            }
        }
        "float" => {
            check("float", &args, 0, 1)?;
            match args.first() {
                None => Ok(V::Float(0.0)),
                Some(V::Str(s)) => py_float_from_str(s).map(V::Float).ok_or(PyErr {
                    kind: "ValueError",
                    msg: format!(
                        "could not convert string to float: {}",
                        crate::hooks::py_repr(s)
                    ),
                }),
                Some(v) => v.num().map(V::Float).ok_or(PyErr {
                    kind: "TypeError",
                    msg: format!(
                        "float() argument must be a string or a real number, not '{}'",
                        v.type_name()
                    ),
                }),
            }
        }
        "bool" => {
            check("bool", &args, 0, 1)?;
            Ok(V::Bool(args.first().is_some_and(V::truthy)))
        }
        "abs" => {
            check("abs", &args, 1, 1)?;
            match &args[0] {
                V::Float(f) => Ok(V::Float(f.abs())),
                v if v.int().is_some() => v
                    .int()
                    .expect("BUG: the guard checked v.int() is Some")
                    .checked_abs()
                    .map(V::Int)
                    .map_or_else(overflow, Ok),
                v => err(
                    "TypeError",
                    format!("bad operand type for abs(): '{}'", v.type_name()),
                ),
            }
        }
        "round" => {
            check("round", &args, 1, 2)?;
            let nd = args.get(1).cloned().or(kw("ndigits"));
            let nd = match nd {
                None | Some(V::None) => None,
                Some(v) => Some(v.int().ok_or(PyErr {
                    kind: "TypeError",
                    msg: format!(
                        "'{}' object cannot be interpreted as an integer",
                        v.type_name()
                    ),
                })?),
            };
            match &args[0] {
                V::Float(f) => {
                    let r = py_round(*f, nd);
                    if nd.is_none() {
                        if !r.is_finite() {
                            return err(
                                "OverflowError",
                                "cannot convert float infinity to integer",
                            );
                        }
                        Ok(V::Int(r as i64))
                    } else {
                        Ok(V::Float(r))
                    }
                }
                v if v.int().is_some() => {
                    let i = v.int().expect("BUG: the guard checked v.int() is Some");
                    match nd {
                        Some(n) if n < 0 => {
                            let p = 10f64.powi((-n) as i32);
                            Ok(V::Int((py_round(i as f64 / p, None) * p) as i64))
                        }
                        _ => Ok(V::Int(i)),
                    }
                }
                v => err(
                    "TypeError",
                    format!("type {} doesn't define __round__ method", v.type_name()),
                ),
            }
        }
        "min" | "max" => {
            if args.is_empty() {
                return err(
                    "TypeError",
                    format!("{name} expected at least 1 argument, got 0"),
                );
            }
            let items = if args.len() == 1 {
                args[0].iter()?
            } else {
                args
            };
            if items.is_empty() {
                if let Some(d) = kw("default") {
                    return Ok(d);
                }
                return err("ValueError", format!("{name}() iterable argument is empty"));
            }
            let mut best = items[0].clone();
            for it in &items[1..] {
                let o = it.cmp(&best, if name == "min" { "<" } else { ">" })?;
                if (name == "min" && o == std::cmp::Ordering::Less)
                    || (name == "max" && o == std::cmp::Ordering::Greater)
                {
                    best = it.clone();
                }
            }
            Ok(best)
        }
        "sum" => {
            check("sum", &args, 1, 2)?;
            let mut acc = args.get(1).cloned().unwrap_or(V::Int(0));
            if matches!(acc, V::Str(_)) {
                return err(
                    "TypeError",
                    "sum() can't sum strings [use ''.join(seq) instead]",
                );
            }
            for it in args[0].iter()? {
                acc = binary("+", &acc, &it)?;
            }
            Ok(acc)
        }
        "any" => {
            check("any", &args, 1, 1)?;
            Ok(V::Bool(args[0].iter()?.iter().any(V::truthy)))
        }
        "all" => {
            check("all", &args, 1, 1)?;
            Ok(V::Bool(args[0].iter()?.iter().all(V::truthy)))
        }
        "sorted" => {
            check("sorted", &args, 1, 1)?;
            let mut items = args[0].iter()?;
            sort_values(&mut items)?;
            if kw("reverse").is_some_and(|r| r.truthy()) {
                items.reverse();
            }
            Ok(V::list(items))
        }
        "reversed" => {
            check("reversed", &args, 1, 1)?;
            let mut items = args[0].iter()?;
            items.reverse();
            Ok(V::list(items))
        }
        "list" => {
            check("list", &args, 0, 1)?;
            Ok(V::list(match args.first() {
                Some(v) => v.iter()?,
                None => vec![],
            }))
        }
        "tuple" => {
            check("tuple", &args, 0, 1)?;
            Ok(V::tuple(match args.first() {
                Some(v) => v.iter()?,
                None => vec![],
            }))
        }
        "dict" => {
            check("dict", &args, 0, 0)?;
            Ok(V::Dict(Rc::new(RefCell::new(vec![]))))
        }
        "bytes" => {
            check("bytes", &args, 0, 3)?;
            match args.first() {
                None => Ok(V::Bytes(Rc::new(vec![]))),
                Some(V::Str(s)) if args.len() > 1 => Ok(V::Bytes(Rc::new(s.as_bytes().to_vec()))),
                Some(V::Str(_)) => err("TypeError", "string argument without an encoding"),
                Some(V::Bytes(b)) => Ok(V::Bytes(b.clone())),
                Some(v) => err(
                    "TypeError",
                    format!("cannot convert '{}' object to bytes", v.type_name()),
                ),
            }
        }
        "range" => {
            check("range", &args, 1, 3)?;
            let ints: Vec<i64> = args
                .iter()
                .map(|a| {
                    a.int().ok_or(PyErr {
                        kind: "TypeError",
                        msg: format!(
                            "'{}' object cannot be interpreted as an integer",
                            a.type_name()
                        ),
                    })
                })
                .collect::<PyResult<_>>()?;
            let (start, stop, step) = match ints.len() {
                1 => (0, ints[0], 1),
                2 => (ints[0], ints[1], 1),
                _ => (ints[0], ints[1], ints[2]),
            };
            if step == 0 {
                return err("ValueError", "range() arg 3 must not be zero");
            }
            let mut out = vec![];
            let mut k = start;
            while (step > 0 && k < stop) || (step < 0 && k > stop) {
                out.push(V::Int(k));
                if out.len() > 1_000_000 {
                    return err("MemoryError", "range too large");
                }
                k += step;
            }
            Ok(V::list(out))
        }
        "enumerate" => {
            check("enumerate", &args, 1, 2)?;
            let start = args.get(1).and_then(V::int).unwrap_or(0);
            Ok(V::list(
                args[0]
                    .iter()?
                    .into_iter()
                    .enumerate()
                    .map(|(k, v)| V::tuple(vec![V::Int(start + k as i64), v]))
                    .collect(),
            ))
        }
        "zip" => {
            let lists: Vec<Vec<V<'a>>> = args.iter().map(V::iter).collect::<PyResult<_>>()?;
            let n = lists.iter().map(Vec::len).min().unwrap_or(0);
            Ok(V::list(
                (0..n)
                    .map(|k| V::tuple(lists.iter().map(|l| l[k].clone()).collect()))
                    .collect(),
            ))
        }
        "ord" => {
            check("ord", &args, 1, 1)?;
            match &args[0] {
                V::Str(s) if s.chars().count() == 1 => Ok(V::Int(
                    s.chars()
                        .next()
                        .expect("BUG: a one-character string has a first char")
                        as i64,
                )),
                V::Str(s) => err(
                    "TypeError",
                    format!(
                        "ord() expected a character, but string of length {} found",
                        s.chars().count()
                    ),
                ),
                v => err(
                    "TypeError",
                    format!(
                        "ord() expected string of length 1, but {} found",
                        v.type_name()
                    ),
                ),
            }
        }
        "chr" => {
            check("chr", &args, 1, 1)?;
            let i = args[0].int().ok_or(PyErr {
                kind: "TypeError",
                msg: format!(
                    "'{}' object cannot be interpreted as an integer",
                    args[0].type_name()
                ),
            })?;
            u32::try_from(i)
                .ok()
                .and_then(char::from_u32)
                .map(|c| V::str(c.to_string()))
                .ok_or(PyErr {
                    kind: "ValueError",
                    msg: "chr() arg not in range(0x110000)".into(),
                })
        }
        "divmod" => {
            check("divmod", &args, 2, 2)?;
            let q = binary("//", &args[0], &args[1])?;
            let r = binary("%", &args[0], &args[1])?;
            Ok(V::tuple(vec![q, r]))
        }
        "pow" => {
            check("pow", &args, 2, 2)?;
            binary("**", &args[0], &args[1])
        }
        "isinstance" => {
            check("isinstance", &args, 2, 2)?;
            let types: Vec<V<'a>> = match &args[1] {
                V::Tuple(t) => t.to_vec(),
                v => vec![v.clone()],
            };
            let mut hit = false;
            for t in types {
                let V::Builtin(t) = t else {
                    return err(
                        "TypeError",
                        "isinstance() arg 2 must be a type, a tuple of types, or a union",
                    );
                };
                hit |= args[0].type_name() == t || (t == "int" && matches!(args[0], V::Bool(_)));
            }
            Ok(V::Bool(hit))
        }
        "format" => {
            check("format", &args, 1, 2)?;
            let spec = match args.get(1) {
                Some(s) => want_str(s, "format() argument 2")?,
                None => String::new(),
            };
            Ok(V::str(format_value(&args[0], &spec)?))
        }
        "hex" | "oct" | "bin" => {
            check(name, &args, 1, 1)?;
            let i = args[0].int().ok_or(PyErr {
                kind: "TypeError",
                msg: format!(
                    "'{}' object cannot be interpreted as an integer",
                    args[0].type_name()
                ),
            })?;
            let t = match name {
                "hex" => "x",
                "oct" => "o",
                _ => "b",
            };
            Ok(V::str(format_value(&V::Int(i), &format!("#{t}"))?))
        }
        _ => err("NameError", format!("name '{name}' is not defined")),
    }
}

fn sort_values<'a>(items: &mut [V<'a>]) -> PyResult<()> {
    let mut error = None;
    items.sort_by(|a, b| match a.cmp(b, "<") {
        Ok(o) => o,
        Err(e) => {
            error.get_or_insert(e);
            std::cmp::Ordering::Equal
        }
    });
    error.map_or(Ok(()), Err)
}

/// Python's whitespace for `str.split()`/`strip()`.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

// Python semantics: indexes, lengths and int() truncate/wrap like CPython.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn str_method<'a>(
    s: &str,
    name: &str,
    args: &[V<'a>],
    kwargs: &[(String, V<'a>)],
) -> PyResult<V<'a>> {
    let arg_str = |i: usize, what: &str| -> PyResult<String> { want_str(&args[i], what) };
    let opt_chars = |i: usize| -> PyResult<Option<String>> {
        match args.get(i) {
            None | Some(V::None) => Ok(None),
            Some(V::Str(c)) => Ok(Some(c.to_string())),
            Some(v) => err(
                "TypeError",
                format!("{name} arg must be None or str, not {}", v.type_name()),
            ),
        }
    };
    let b = |x: bool| Ok(V::Bool(x));
    let st = |x: String| Ok(V::str(x));
    let chars: Vec<char> = s.chars().collect();
    match name {
        "lower" => st(s.to_lowercase()),
        "upper" => st(s.to_uppercase()),
        "casefold" => st(s.to_lowercase()),
        "swapcase" => st(s
            .chars()
            .map(|c| {
                if c.is_uppercase() {
                    c.to_lowercase().collect::<String>()
                } else {
                    c.to_uppercase().collect()
                }
            })
            .collect()),
        "strip" | "lstrip" | "rstrip" => {
            check(name, args, 0, 1)?;
            let set = opt_chars(0)?;
            let pred = |c: char| match &set {
                Some(cs) => cs.contains(c),
                None => is_py_space(c),
            };
            st(match name {
                "strip" => s.trim_matches(pred).to_string(),
                "lstrip" => s.trim_start_matches(pred).to_string(),
                _ => s.trim_end_matches(pred).to_string(),
            })
        }
        "startswith" | "endswith" => {
            check(name, args, 1, 3)?;
            let pats: Vec<V<'a>> = match &args[0] {
                V::Tuple(t) => t.to_vec(),
                v => vec![v.clone()],
            };
            let mut hit = false;
            for p in pats {
                let V::Str(p) = p else {
                    return err(
                        "TypeError",
                        format!(
                            "{name} first arg must be str or a tuple of str, not {}",
                            p.type_name()
                        ),
                    );
                };
                hit |= if name == "startswith" {
                    s.starts_with(&*p)
                } else {
                    s.ends_with(&*p)
                };
            }
            b(hit)
        }
        "replace" => {
            check("replace", args, 2, 3)?;
            let (old, new) = (
                arg_str(0, "replace() argument 1")?,
                arg_str(1, "replace() argument 2")?,
            );
            match args.get(2).and_then(V::int) {
                Some(c) if c >= 0 => st(s.replacen(&old, &new, c as usize)),
                _ => st(s.replace(&old, &new)),
            }
        }
        "split" | "rsplit" => {
            check(name, args, 0, 2)?;
            let sep = match args.first().cloned().or_else(|| {
                kwargs
                    .iter()
                    .find(|(k, _)| k == "sep")
                    .map(|(_, v)| v.clone())
            }) {
                None | Some(V::None) => None,
                Some(V::Str(x)) => Some(x.to_string()),
                Some(v) => {
                    return err(
                        "TypeError",
                        format!("must be str or None, not {}", v.type_name()),
                    );
                }
            };
            let max = args
                .get(1)
                .cloned()
                .or_else(|| {
                    kwargs
                        .iter()
                        .find(|(k, _)| k == "maxsplit")
                        .map(|(_, v)| v.clone())
                })
                .and_then(|v| v.int())
                .filter(|m| *m >= 0);
            let parts: Vec<String> = match sep {
                Some(sep) if sep.is_empty() => return err("ValueError", "empty separator"),
                Some(sep) => match (max, name) {
                    (Some(m), "split") => {
                        s.splitn(m as usize + 1, &*sep).map(String::from).collect()
                    }
                    (Some(m), _) => {
                        let mut v: Vec<String> =
                            s.rsplitn(m as usize + 1, &*sep).map(String::from).collect();
                        v.reverse();
                        v
                    }
                    (None, _) => s.split(&*sep).map(String::from).collect(),
                },
                None => {
                    let words: Vec<&str> = s.split(is_py_space).filter(|w| !w.is_empty()).collect();
                    match max {
                        Some(m) if (m as usize) < words.len() && name == "split" => {
                            // The rest after `m` splits, left-trimmed.
                            let mut out: Vec<String> = vec![];
                            let mut rest = s.trim_start_matches(is_py_space);
                            for _ in 0..m {
                                let end = rest.find(is_py_space).unwrap_or(rest.len());
                                out.push(rest[..end].to_string());
                                rest = rest[end..].trim_start_matches(is_py_space);
                            }
                            if !rest.is_empty() {
                                out.push(rest.to_string());
                            }
                            out
                        }
                        Some(m) if (m as usize) < words.len() => {
                            let mut out: Vec<String> = vec![];
                            let mut rest = s.trim_end_matches(is_py_space);
                            for _ in 0..m {
                                let st = rest
                                    .rfind(is_py_space)
                                    .map(|k| {
                                        k + rest[k..]
                                            .chars()
                                            .next()
                                            .expect("BUG: rfind found a char")
                                            .len_utf8()
                                    })
                                    .unwrap_or(0);
                                out.push(rest[st..].to_string());
                                rest = rest[..st].trim_end_matches(is_py_space);
                            }
                            if !rest.is_empty() {
                                out.push(rest.to_string());
                            }
                            out.reverse();
                            out
                        }
                        _ => words.into_iter().map(String::from).collect(),
                    }
                }
            };
            Ok(V::list(parts.into_iter().map(V::str).collect()))
        }
        "splitlines" => Ok(V::list(s.lines().map(V::str).collect())),
        "join" => {
            check("join", args, 1, 1)?;
            let mut out = vec![];
            for (k, it) in args[0].iter()?.into_iter().enumerate() {
                match it {
                    V::Str(x) => out.push(x.to_string()),
                    other => {
                        return err(
                            "TypeError",
                            format!(
                                "sequence item {k}: expected str instance, {} found",
                                other.type_name()
                            ),
                        );
                    }
                }
            }
            st(out.join(s))
        }
        "title" => {
            let mut out = String::new();
            let mut prev_cased = false;
            for c in s.chars() {
                if prev_cased {
                    out.extend(c.to_lowercase());
                } else {
                    out.extend(c.to_uppercase());
                }
                prev_cased = c.is_alphabetic();
            }
            st(out)
        }
        "capitalize" => {
            let mut it = s.chars();
            st(match it.next() {
                Some(f) => f
                    .to_uppercase()
                    .chain(it.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            })
        }
        "format" => st(str_format(s, args, kwargs)?),
        "find" | "rfind" | "index" | "rindex" | "count" => {
            check(name, args, 1, 3)?;
            let sub = arg_str(0, "must be str, not")?;
            let to_char = |byte: usize| s[..byte].chars().count() as i64;
            let found = match name {
                "find" | "index" => s.find(&sub).map(to_char),
                "count" => {
                    return Ok(V::Int(if sub.is_empty() {
                        chars.len() as i64 + 1
                    } else {
                        s.matches(&*sub).count() as i64
                    }));
                }
                _ => s.rfind(&sub).map(to_char),
            };
            match (found, name) {
                (Some(i), _) => Ok(V::Int(i)),
                (None, "find" | "rfind") => Ok(V::Int(-1)),
                (None, _) => err("ValueError", "substring not found"),
            }
        }
        "isdigit" | "isnumeric" | "isdecimal" => {
            b(!s.is_empty() && s.chars().all(|c| c.is_numeric()))
        }
        "isalpha" => b(!s.is_empty() && s.chars().all(char::is_alphabetic)),
        "isalnum" => b(!s.is_empty() && s.chars().all(char::is_alphanumeric)),
        "isspace" => b(!s.is_empty() && s.chars().all(is_py_space)),
        "islower" => b(s.chars().any(char::is_lowercase) && !s.chars().any(char::is_uppercase)),
        "isupper" => b(s.chars().any(char::is_uppercase) && !s.chars().any(char::is_lowercase)),
        "zfill" => {
            check("zfill", args, 1, 1)?;
            let w = args[0].int().unwrap_or(0).max(0) as usize;
            if chars.len() >= w {
                return st(s.to_string());
            }
            let (sign, rest) = match s.chars().next() {
                Some(c @ ('+' | '-')) => (c.to_string(), &s[1..]),
                _ => (String::new(), s),
            };
            st(format!("{sign}{}{rest}", "0".repeat(w - chars.len())))
        }
        "ljust" | "rjust" | "center" => {
            check(name, args, 1, 2)?;
            let w = args[0].int().unwrap_or(0).max(0) as usize;
            let fill = match args.get(1) {
                Some(V::Str(f)) if f.chars().count() == 1 => f
                    .chars()
                    .next()
                    .expect("BUG: a one-character string has a first char"),
                Some(_) => {
                    return err(
                        "TypeError",
                        "The fill character must be exactly one character long",
                    );
                }
                None => ' ',
            };
            if chars.len() >= w {
                return st(s.to_string());
            }
            let n = w - chars.len();
            let f = |k: usize| fill.to_string().repeat(k);
            st(match name {
                "ljust" => format!("{s}{}", f(n)),
                "rjust" => format!("{}{s}", f(n)),
                // CPython's centering puts the odd pad on the left when the
                // width is odd.
                _ => {
                    let left = n / 2 + (n & w & 1);
                    format!("{}{s}{}", f(left), f(n - left))
                }
            })
        }
        "partition" | "rpartition" => {
            check(name, args, 1, 1)?;
            let sep = arg_str(0, "must be str, not")?;
            let pos = if name == "partition" {
                s.find(&sep)
            } else {
                s.rfind(&sep)
            };
            Ok(V::tuple(match pos {
                Some(k) => vec![
                    V::str(&s[..k]),
                    V::str(sep.as_str()),
                    V::str(&s[k + sep.len()..]),
                ],
                None if name == "partition" => vec![V::str(s), V::str(""), V::str("")],
                None => vec![V::str(""), V::str(""), V::str(s)],
            }))
        }
        "removeprefix" => st(s
            .strip_prefix(&*arg_str(0, "removeprefix() argument")?)
            .unwrap_or(s)
            .to_string()),
        "removesuffix" => st(s
            .strip_suffix(&*arg_str(0, "removesuffix() argument")?)
            .unwrap_or(s)
            .to_string()),
        "encode" => Ok(V::Bytes(Rc::new(s.as_bytes().to_vec()))),
        _ => err(
            "AttributeError",
            format!("'str' object has no attribute '{name}'"),
        ),
    }
}

/// `str.format`.
fn str_format<'a>(fmt: &str, args: &[V<'a>], kwargs: &[(String, V<'a>)]) -> PyResult<String> {
    let c: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut auto = 0;
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '{' if c.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if c.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '}' => return err("ValueError", "Single '}' encountered in format string"),
            '{' => {
                let st = i + 1;
                let mut depth = 1;
                i += 1;
                while i < c.len() && depth > 0 {
                    match c[i] {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth > 0 {
                    return err("ValueError", "expected '}' before end of string");
                }
                let field: String = c[st..i - 1].iter().collect();
                let (head, spec) = match field.find(':') {
                    Some(k) => (&field[..k], &field[k + 1..]),
                    None => (field.as_str(), ""),
                };
                let (name, conv) = match head.find('!') {
                    Some(k) => (&head[..k], head[k + 1..].chars().next()),
                    None => (head, None),
                };
                // `name[.attr][[index]]`: the first part only (and indexes).
                let base_end = name.find(['.', '[']).unwrap_or(name.len());
                let base = &name[..base_end];
                let mut v = if base.is_empty() {
                    let v = args.get(auto).cloned();
                    auto += 1;
                    v.ok_or(PyErr {
                        kind: "IndexError",
                        msg: format!(
                            "Replacement index {} out of range for positional args tuple",
                            auto - 1
                        ),
                    })?
                } else if let Ok(k) = base.parse::<usize>() {
                    args.get(k).cloned().ok_or(PyErr {
                        kind: "IndexError",
                        msg: format!(
                            "Replacement index {k} out of range for positional args tuple"
                        ),
                    })?
                } else {
                    kwargs
                        .iter()
                        .find(|(n, _)| n == base)
                        .map(|(_, v)| v.clone())
                        .ok_or(PyErr {
                            kind: "KeyError",
                            msg: crate::hooks::py_repr(base),
                        })?
                };
                let mut rest = &name[base_end..];
                while !rest.is_empty() {
                    if let Some(r) = rest.strip_prefix('.') {
                        let e = r.find(['.', '[']).unwrap_or(r.len());
                        v = getattr(&v, &r[..e])?;
                        rest = &r[e..];
                    } else if let Some(r) = rest.strip_prefix('[') {
                        let e = r.find(']').unwrap_or(r.len());
                        let key = &r[..e];
                        let k = match key.parse::<i64>() {
                            Ok(n) => V::Int(n),
                            Err(_) => V::str(key),
                        };
                        v = index(&v, &k)?;
                        rest = r.get(e + 1..).unwrap_or("");
                    } else {
                        break;
                    }
                }
                let v = match conv {
                    Some('r') | Some('a') => V::str(v.repr()),
                    Some('s') => V::str(v.to_str()),
                    _ => v,
                };
                // Nested fields in the spec.
                let spec = if spec.contains('{') {
                    str_format(spec, args, kwargs)?
                } else {
                    spec.to_string()
                };
                out.push_str(&format_value(&v, &spec)?);
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    Ok(out)
}

// Python semantics: indexes, lengths and int() truncate/wrap like CPython.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn method<'a>(
    obj: &V<'a>,
    name: &str,
    args: Vec<V<'a>>,
    kwargs: Vec<(String, V<'a>)>,
) -> PyResult<V<'a>> {
    match obj {
        V::Str(s) => str_method(s, name, &args, &kwargs),
        V::Bytes(b) => match name {
            "decode" => Ok(V::str(String::from_utf8_lossy(b).as_ref())),
            "lower" => Ok(V::Bytes(Rc::new(b.to_ascii_lowercase()))),
            "upper" => Ok(V::Bytes(Rc::new(b.to_ascii_uppercase()))),
            "startswith" | "endswith" => match args.first() {
                Some(V::Bytes(p)) => Ok(V::Bool(if name == "startswith" {
                    b.starts_with(p)
                } else {
                    b.ends_with(p)
                })),
                _ => err(
                    "TypeError",
                    format!("{name} first arg must be bytes or a tuple of bytes"),
                ),
            },
            _ => unreachable!(),
        },
        V::Float(f) => Ok(V::Bool(f.fract() == 0.0)),
        V::List(l) => match name {
            "append" => {
                check("list.append", &args, 1, 1)?;
                l.borrow_mut().push(args[0].clone());
                Ok(V::None)
            }
            "extend" => {
                let items = args.first().map(V::iter).transpose()?.unwrap_or_default();
                l.borrow_mut().extend(items);
                Ok(V::None)
            }
            "insert" => {
                check("insert", &args, 2, 2)?;
                let mut l = l.borrow_mut();
                let n = l.len() as i64;
                let k = args[0].int().unwrap_or(0);
                let k = if k < 0 { (k + n).max(0) } else { k.min(n) } as usize;
                l.insert(k, args[1].clone());
                Ok(V::None)
            }
            "pop" => {
                let mut l = l.borrow_mut();
                if l.is_empty() {
                    return err("IndexError", "pop from empty list");
                }
                let n = l.len();
                let k = args.first().and_then(V::int).unwrap_or(-1);
                match norm_index(k, n) {
                    Some(k) => Ok(l.remove(k)),
                    None => err("IndexError", "pop index out of range"),
                }
            }
            "remove" => {
                let mut l = l.borrow_mut();
                match l.iter().position(|v| v.eq(&args[0])) {
                    Some(k) => {
                        l.remove(k);
                        Ok(V::None)
                    }
                    None => err("ValueError", "list.remove(x): x not in list"),
                }
            }
            "index" => match l.borrow().iter().position(|v| v.eq(&args[0])) {
                Some(k) => Ok(V::Int(k as i64)),
                None => err("ValueError", format!("{} is not in list", args[0].repr())),
            },
            "count" => Ok(V::Int(
                l.borrow().iter().filter(|v| v.eq(&args[0])).count() as i64
            )),
            "sort" => {
                let mut items = l.borrow().clone();
                sort_values(&mut items)?;
                if kwargs.iter().any(|(k, v)| k == "reverse" && v.truthy()) {
                    items.reverse();
                }
                *l.borrow_mut() = items;
                Ok(V::None)
            }
            "reverse" => {
                l.borrow_mut().reverse();
                Ok(V::None)
            }
            "copy" => Ok(V::list(l.borrow().clone())),
            _ => unreachable!(),
        },
        V::Tuple(t) => match name {
            "index" => match t.iter().position(|v| v.eq(&args[0])) {
                Some(k) => Ok(V::Int(k as i64)),
                None => err("ValueError", "tuple.index(x): x not in tuple"),
            },
            _ => Ok(V::Int(t.iter().filter(|v| v.eq(&args[0])).count() as i64)),
        },
        V::Dict(d) => {
            let d = d.borrow();
            match name {
                "get" => {
                    check("get", &args, 1, 2)?;
                    Ok(d.iter()
                        .find(|(k, _)| k.eq(&args[0]))
                        .map(|(_, v)| v.clone())
                        .unwrap_or_else(|| args.get(1).cloned().unwrap_or(V::None)))
                }
                "keys" => Ok(V::list(d.iter().map(|(k, _)| k.clone()).collect())),
                "values" => Ok(V::list(d.iter().map(|(_, v)| v.clone()).collect())),
                _ => Ok(V::list(
                    d.iter()
                        .map(|(k, v)| V::tuple(vec![k.clone(), v.clone()]))
                        .collect(),
                )),
            }
        }
        V::Obj(o) => o.call_method(name, &args),
        _ => err(
            "AttributeError",
            format!("'{}' object has no attribute '{name}'", obj.type_name()),
        ),
    }
}

/// Wrap an object as a value.
pub fn obj<'a>(o: impl Obj<'a> + 'a) -> V<'a> {
    V::Obj(Rc::new(o))
}
