//! A parser for the subset of Python `inline` evaluates. Valid Python the
//! subset doesn't cover is reported as `Unsupported` (naming the
//! construct), anything else as a syntax error.

use super::lexer::{Tok, tokenize, unescape};

#[derive(Debug, Clone)]
pub enum Expr {
    Name(String),
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    /// An f-string: literal parts and replacement fields.
    FStr(Vec<FPart>),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    Attr(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>, Vec<(String, Expr)>),
    Index(Box<Expr>, Box<Expr>),
    Slice(
        Box<Expr>,
        Option<Box<Expr>>,
        Option<Box<Expr>>,
        Option<Box<Expr>>,
    ),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// `a < b <= c`: the operands and operators.
    Compare(Box<Expr>, Vec<(&'static str, Expr)>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    IfElse(Box<Expr>, Box<Expr>, Box<Expr>),
    /// A list comprehension or generator expression (`list`: brackets).
    Comp {
        elt: Box<Expr>,
        clauses: Vec<CompClause>,
        list: bool,
    },
}

#[derive(Debug, Clone)]
pub enum FPart {
    Lit(String),
    Field {
        expr: Expr,
        conv: Option<char>,
        spec: Vec<FPart>,
    },
}

#[derive(Debug, Clone)]
pub enum CompClause {
    For(Target, Expr),
    If(Expr),
}

#[derive(Debug, Clone)]
pub enum Target {
    Name(String),
    Tuple(Vec<Target>),
    Index(Expr, Expr),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Expr(Expr),
    Assign(Vec<Target>, Expr),
    AugAssign(Target, &'static str, Expr),
    Return(Option<Expr>),
    If(Vec<(Expr, Vec<Stmt>)>, Vec<Stmt>),
    For(Target, Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>, Vec<Stmt>),
    Pass,
    Break,
    Continue,
}

/// Why a definition can't be used.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// Not Python (beets logs a syntax error and leaves the field out).
    Syntax(String),
    /// Python beyond the supported subset: the construct.
    Unsupported(String),
}

type R<T> = Result<T, ParseError>;

/// A call's positional and keyword arguments.
type CallArgs = (Vec<Expr>, Vec<(String, Expr)>);

fn syntax<T>(msg: impl Into<String>) -> R<T> {
    Err(ParseError::Syntax(msg.into()))
}

fn unsupported<T>(what: impl Into<String>) -> R<T> {
    Err(ParseError::Unsupported(what.into()))
}

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

struct P {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

impl P {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].0
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Name(n) if n == kw)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> R<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            syntax(format!("expected '{op}'"))
        }
    }

    // ------------------------------------------------------- expressions

    /// `expressions`: one expression, or a tuple of them.
    fn exprlist(&mut self) -> R<Expr> {
        if self.is_op("*") {
            return unsupported("starred expressions");
        }
        let first = self.expr()?;
        if !self.is_op(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_op(",") {
            if self.ends_exprlist() {
                break;
            }
            if self.is_op("*") {
                return unsupported("starred expressions");
            }
            items.push(self.expr()?);
        }
        Ok(Expr::Tuple(items))
    }

    fn ends_exprlist(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Newline | Tok::End | Tok::Op(")" | "]" | "}" | "=" | ":")
        ) || matches!(self.peek(), Tok::Op(o) if o.ends_with('=') && *o != "==")
    }

    fn expr(&mut self) -> R<Expr> {
        if self.is_kw("lambda") {
            return unsupported("lambda expressions");
        }
        if self.is_kw("yield") {
            return unsupported("yield");
        }
        if self.is_kw("await") {
            return unsupported("await");
        }
        let e = self.disjunction()?;
        if self.is_op(":=") {
            return unsupported("assignment expressions (:=)");
        }
        if self.eat_kw("if") {
            let cond = self.disjunction()?;
            if !self.eat_kw("else") {
                return syntax("expected 'else' after 'if' expression");
            }
            let other = self.expr()?;
            return Ok(Expr::IfElse(Box::new(cond), Box::new(e), Box::new(other)));
        }
        Ok(e)
    }

    fn disjunction(&mut self) -> R<Expr> {
        let mut e = self.conjunction()?;
        while self.eat_kw("or") {
            let r = self.conjunction()?;
            e = Expr::Or(Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn conjunction(&mut self) -> R<Expr> {
        let mut e = self.inversion()?;
        while self.eat_kw("and") {
            let r = self.inversion()?;
            e = Expr::And(Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn inversion(&mut self) -> R<Expr> {
        if self.eat_kw("not") {
            let e = self.inversion()?;
            return Ok(Expr::Unary("not", Box::new(e)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> R<Expr> {
        let first = self.bitor()?;
        let mut ops = vec![];
        loop {
            let op: &'static str = match self.peek() {
                Tok::Op(o @ ("==" | "!=" | "<" | "<=" | ">" | ">=")) => {
                    let o = *o;
                    self.next();
                    o
                }
                Tok::Name(n) if n == "in" => {
                    self.next();
                    "in"
                }
                Tok::Name(n)
                    if n == "not" && matches!(self.peek_at(1), Tok::Name(m) if m == "in") =>
                {
                    self.next();
                    self.next();
                    "not in"
                }
                Tok::Name(n) if n == "is" => {
                    self.next();
                    if self.eat_kw("not") { "is not" } else { "is" }
                }
                _ => break,
            };
            ops.push((op, self.bitor()?));
        }
        if ops.is_empty() {
            Ok(first)
        } else {
            Ok(Expr::Compare(Box::new(first), ops))
        }
    }

    fn binary_level(&mut self, ops: &[&'static str], next: fn(&mut Self) -> R<Expr>) -> R<Expr> {
        let mut e = next(self)?;
        while let Some(op) = ops.iter().find(|o| self.is_op(o)) {
            self.next();
            let r = next(self)?;
            e = Expr::Binary(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }

    fn bitor(&mut self) -> R<Expr> {
        self.binary_level(&["|"], Self::bitxor)
    }

    fn bitxor(&mut self) -> R<Expr> {
        self.binary_level(&["^"], Self::bitand)
    }

    fn bitand(&mut self) -> R<Expr> {
        self.binary_level(&["&"], Self::shift)
    }

    fn shift(&mut self) -> R<Expr> {
        self.binary_level(&["<<", ">>"], Self::sum)
    }

    fn sum(&mut self) -> R<Expr> {
        self.binary_level(&["+", "-"], Self::term)
    }

    fn term(&mut self) -> R<Expr> {
        if self.is_op("@") {
            return unsupported("the @ operator");
        }
        self.binary_level(&["*", "/", "//", "%"], Self::factor)
    }

    fn factor(&mut self) -> R<Expr> {
        for op in ["+", "-", "~"] {
            if self.eat_op(op) {
                let e = self.factor()?;
                return Ok(Expr::Unary(op, Box::new(e)));
            }
        }
        self.power()
    }

    fn power(&mut self) -> R<Expr> {
        let base = self.primary()?;
        if self.eat_op("**") {
            let exp = self.factor()?;
            return Ok(Expr::Binary("**", Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> R<Expr> {
        let mut e = self.atom()?;
        loop {
            if self.eat_op(".") {
                match self.next() {
                    Tok::Name(n) => e = Expr::Attr(Box::new(e), n),
                    _ => return syntax("invalid syntax"),
                }
            } else if self.eat_op("(") {
                let (args, kwargs) = self.call_args()?;
                e = Expr::Call(Box::new(e), args, kwargs);
            } else if self.eat_op("[") {
                e = self.subscript(e)?;
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> R<CallArgs> {
        let mut args = vec![];
        let mut kwargs = vec![];
        while !self.eat_op(")") {
            if self.is_op("*") || self.is_op("**") {
                return unsupported("* and ** arguments");
            }
            if let (Tok::Name(n), Tok::Op("=")) = (self.peek().clone(), self.peek_at(1).clone()) {
                self.next();
                self.next();
                kwargs.push((n, self.expr()?));
            } else {
                let e = self.expr()?;
                if self.is_kw("for") {
                    // A generator expression as the only argument.
                    let clauses = self.comp_clauses()?;
                    args.push(Expr::Comp {
                        elt: Box::new(e),
                        clauses,
                        list: false,
                    });
                } else {
                    if !kwargs.is_empty() {
                        return syntax("positional argument follows keyword argument");
                    }
                    args.push(e);
                }
            }
            if !self.eat_op(",") {
                self.expect_op(")")?;
                break;
            }
        }
        Ok((args, kwargs))
    }

    fn subscript(&mut self, base: Expr) -> R<Expr> {
        let slice_part = |p: &mut Self| -> R<Option<Box<Expr>>> {
            if p.is_op(":") || p.is_op("]") || p.is_op(",") {
                Ok(None)
            } else {
                Ok(Some(Box::new(p.expr()?)))
            }
        };
        let start = slice_part(self)?;
        let e = if self.eat_op(":") {
            let stop = slice_part(self)?;
            let step = if self.eat_op(":") {
                slice_part(self)?
            } else {
                None
            };
            Expr::Slice(Box::new(base), start, stop, step)
        } else {
            let Some(idx) = start else {
                return syntax("invalid syntax");
            };
            if self.is_op(",") {
                return unsupported("tuple subscripts");
            }
            Expr::Index(Box::new(base), idx)
        };
        self.expect_op("]")?;
        Ok(e)
    }

    fn comp_clauses(&mut self) -> R<Vec<CompClause>> {
        let mut clauses = vec![];
        loop {
            if self.eat_kw("for") {
                let target = self.target_list()?;
                if !self.eat_kw("in") {
                    return syntax("expected 'in'");
                }
                let iter = self.disjunction()?;
                clauses.push(CompClause::For(target, iter));
            } else if self.eat_kw("if") {
                clauses.push(CompClause::If(self.disjunction()?));
            } else if self.is_kw("async") {
                return unsupported("async comprehensions");
            } else {
                break;
            }
        }
        Ok(clauses)
    }

    /// A `for` target: names, possibly a tuple of them.
    fn target_list(&mut self) -> R<Target> {
        let mut items = vec![self.target_atom()?];
        let mut tuple = false;
        while self.eat_op(",") {
            tuple = true;
            if self.is_kw("in") || self.is_op("=") {
                break;
            }
            items.push(self.target_atom()?);
        }
        Ok(if tuple {
            Target::Tuple(items)
        } else {
            items.pop().unwrap()
        })
    }

    fn target_atom(&mut self) -> R<Target> {
        if self.eat_op("(") || self.eat_op("[") {
            let t = self.target_list()?;
            if !(self.eat_op(")") || self.eat_op("]")) {
                return syntax("invalid syntax");
            }
            return Ok(t);
        }
        if self.is_op("*") {
            return unsupported("starred assignment targets");
        }
        match self.next() {
            Tok::Name(n) if !KEYWORDS.contains(&n.as_str()) => Ok(Target::Name(n)),
            _ => syntax("invalid syntax"),
        }
    }

    fn atom(&mut self) -> R<Expr> {
        match self.next() {
            Tok::Name(n) => match n.as_str() {
                "None" => Ok(Expr::None),
                "True" => Ok(Expr::Bool(true)),
                "False" => Ok(Expr::Bool(false)),
                "lambda" => unsupported("lambda expressions"),
                "yield" => unsupported("yield"),
                "await" => unsupported("await"),
                k if KEYWORDS.contains(&k) => syntax("invalid syntax"),
                _ => Ok(Expr::Name(n)),
            },
            Tok::Int(i) => Ok(Expr::Int(i)),
            Tok::Float(f) => Ok(Expr::Float(f)),
            t @ Tok::Str { .. } => self.strings(t),
            Tok::Op("(") => {
                if self.eat_op(")") {
                    return Ok(Expr::Tuple(vec![]));
                }
                if self.is_op("*") {
                    return unsupported("starred expressions");
                }
                let first = self.expr()?;
                if self.is_kw("for") {
                    let clauses = self.comp_clauses()?;
                    self.expect_op(")")?;
                    return Ok(Expr::Comp {
                        elt: Box::new(first),
                        clauses,
                        list: false,
                    });
                }
                if self.eat_op(")") {
                    return Ok(first);
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.is_op(")") {
                        break;
                    }
                    items.push(self.expr()?);
                }
                self.expect_op(")")?;
                Ok(Expr::Tuple(items))
            }
            Tok::Op("[") => {
                if self.eat_op("]") {
                    return Ok(Expr::List(vec![]));
                }
                if self.is_op("*") {
                    return unsupported("starred expressions");
                }
                let first = self.expr()?;
                if self.is_kw("for") {
                    let clauses = self.comp_clauses()?;
                    self.expect_op("]")?;
                    return Ok(Expr::Comp {
                        elt: Box::new(first),
                        clauses,
                        list: true,
                    });
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.is_op("]") {
                        break;
                    }
                    items.push(self.expr()?);
                }
                self.expect_op("]")?;
                Ok(Expr::List(items))
            }
            Tok::Op("{") => {
                if self.eat_op("}") {
                    return Ok(Expr::Dict(vec![]));
                }
                if self.is_op("**") {
                    return unsupported("dict unpacking");
                }
                let k = self.expr()?;
                if !self.eat_op(":") {
                    return unsupported("set displays");
                }
                let v = self.expr()?;
                if self.is_kw("for") {
                    return unsupported("dict comprehensions");
                }
                let mut pairs = vec![(k, v)];
                while self.eat_op(",") {
                    if self.is_op("}") {
                        break;
                    }
                    let k = self.expr()?;
                    self.expect_op(":")?;
                    pairs.push((k, self.expr()?));
                }
                self.expect_op("}")?;
                Ok(Expr::Dict(pairs))
            }
            Tok::Op("...") => unsupported("Ellipsis"),
            _ => syntax("invalid syntax"),
        }
    }

    /// Adjacent string literals, concatenated.
    fn strings(&mut self, first: Tok) -> R<Expr> {
        let mut parts: Vec<FPart> = vec![];
        let mut any_f = false;
        let mut bytes: Option<bool> = None;
        let mut tok = first;
        loop {
            let Tok::Str {
                value,
                fstring,
                bytes: b,
                raw,
            } = tok
            else {
                unreachable!()
            };
            if bytes.is_some_and(|x| x != b) {
                return syntax("cannot mix bytes and nonbytes literals");
            }
            bytes = Some(b);
            if fstring {
                any_f = true;
                parts.extend(fstring_parts(&value, raw)?);
            } else {
                parts.push(FPart::Lit(value));
            }
            if matches!(self.peek(), Tok::Str { .. }) {
                tok = self.next();
            } else {
                break;
            }
        }
        if bytes == Some(true) {
            let s: String = parts
                .into_iter()
                .map(|p| match p {
                    FPart::Lit(s) => s,
                    _ => String::new(),
                })
                .collect();
            return Ok(Expr::Bytes(s.chars().map(|c| c as u32 as u8).collect()));
        }
        if !any_f {
            let s: String = parts
                .into_iter()
                .map(|p| match p {
                    FPart::Lit(s) => s,
                    _ => String::new(),
                })
                .collect();
            return Ok(Expr::Str(s));
        }
        Ok(Expr::FStr(parts))
    }

    // -------------------------------------------------------- statements

    fn block(&mut self) -> R<Vec<Stmt>> {
        // After ':' either a simple statement on the same line or an
        // indented block.
        if matches!(self.peek(), Tok::Newline) {
            self.next();
            if !matches!(self.next(), Tok::Indent) {
                return syntax("expected an indented block");
            }
            let mut body = vec![];
            while !matches!(self.peek(), Tok::Dedent | Tok::End) {
                body.extend(self.statement()?);
            }
            if matches!(self.peek(), Tok::Dedent) {
                self.next();
            }
            Ok(body)
        } else {
            self.simple_statements()
        }
    }

    fn statement(&mut self) -> R<Vec<Stmt>> {
        let Tok::Name(kw) = self.peek().clone() else {
            return self.simple_statements();
        };
        match kw.as_str() {
            "if" => {
                self.next();
                let mut branches = vec![];
                let cond = self.expr()?;
                self.expect_op(":")?;
                branches.push((cond, self.block()?));
                let mut orelse = vec![];
                loop {
                    if self.eat_kw("elif") {
                        let c = self.expr()?;
                        self.expect_op(":")?;
                        branches.push((c, self.block()?));
                    } else if self.eat_kw("else") {
                        self.expect_op(":")?;
                        orelse = self.block()?;
                        break;
                    } else {
                        break;
                    }
                }
                Ok(vec![Stmt::If(branches, orelse)])
            }
            "for" => {
                self.next();
                let target = self.target_list()?;
                if !self.eat_kw("in") {
                    return syntax("expected 'in'");
                }
                let iter = self.exprlist()?;
                self.expect_op(":")?;
                let body = self.block()?;
                let orelse = if self.eat_kw("else") {
                    self.expect_op(":")?;
                    self.block()?
                } else {
                    vec![]
                };
                Ok(vec![Stmt::For(target, iter, body, orelse)])
            }
            "while" => {
                self.next();
                let cond = self.expr()?;
                self.expect_op(":")?;
                let body = self.block()?;
                let orelse = if self.eat_kw("else") {
                    self.expect_op(":")?;
                    self.block()?
                } else {
                    vec![]
                };
                Ok(vec![Stmt::While(cond, body, orelse)])
            }
            "def" => unsupported("function definitions (def)"),
            "class" => unsupported("class definitions"),
            "try" => unsupported("try statements"),
            "with" => unsupported("with statements"),
            "async" => unsupported("async statements"),
            "match" if !matches!(self.peek_at(1), Tok::Op(_) | Tok::Newline) => {
                unsupported("match statements")
            }
            "elif" | "else" | "except" | "finally" => syntax("invalid syntax"),
            _ => self.simple_statements(),
        }
    }

    fn simple_statements(&mut self) -> R<Vec<Stmt>> {
        let mut out = vec![self.simple_statement()?];
        while self.eat_op(";") {
            if matches!(self.peek(), Tok::Newline | Tok::End) {
                break;
            }
            out.push(self.simple_statement()?);
        }
        match self.next() {
            Tok::Newline | Tok::End => Ok(out),
            _ => syntax("invalid syntax"),
        }
    }

    fn simple_statement(&mut self) -> R<Stmt> {
        if let Tok::Name(kw) = self.peek().clone() {
            match kw.as_str() {
                "return" => {
                    self.next();
                    if matches!(self.peek(), Tok::Newline | Tok::End | Tok::Op(";")) {
                        return Ok(Stmt::Return(None));
                    }
                    return Ok(Stmt::Return(Some(self.exprlist()?)));
                }
                "pass" => {
                    self.next();
                    return Ok(Stmt::Pass);
                }
                "break" => {
                    self.next();
                    return Ok(Stmt::Break);
                }
                "continue" => {
                    self.next();
                    return Ok(Stmt::Continue);
                }
                "import" | "from" => return unsupported("import statements"),
                "global" | "nonlocal" => return unsupported("global and nonlocal statements"),
                "del" => return unsupported("del statements"),
                "assert" => return unsupported("assert statements"),
                "raise" => return unsupported("raise statements"),
                "yield" => return unsupported("yield"),
                _ => {}
            }
        }
        let e = self.exprlist()?;
        if self.is_op(":") {
            return unsupported("annotated assignments");
        }
        let aug = [
            "+=", "-=", "*=", "/=", "//=", "%=", "**=", "&=", "|=", "^=", "<<=", ">>=",
        ];
        if let Some(&op) = aug.iter().find(|o| self.is_op(o)) {
            self.next();
            let target = to_target(&e)?;
            if matches!(target, Target::Tuple(_)) {
                return syntax("illegal expression for augmented assignment");
            }
            let v = self.exprlist()?;
            let op: &'static str = &op[..op.len() - 1];
            return Ok(Stmt::AugAssign(target, op, v));
        }
        if self.is_op("=") {
            let mut targets = vec![to_target(&e)?];
            let mut value;
            loop {
                self.expect_op("=")?;
                value = self.exprlist()?;
                if self.is_op("=") {
                    targets.push(to_target(&value)?);
                } else {
                    break;
                }
            }
            return Ok(Stmt::Assign(targets, value));
        }
        Ok(Stmt::Expr(e))
    }
}

fn to_target(e: &Expr) -> R<Target> {
    match e {
        Expr::Name(n) => Ok(Target::Name(n.clone())),
        Expr::Tuple(items) | Expr::List(items) => Ok(Target::Tuple(
            items.iter().map(to_target).collect::<R<_>>()?,
        )),
        Expr::Index(b, i) => Ok(Target::Index((**b).clone(), (**i).clone())),
        Expr::Attr(..) => unsupported("attribute assignment"),
        Expr::Slice(..) => unsupported("slice assignment"),
        _ => syntax("cannot assign to expression"),
    }
}

/// Split an f-string's text into literal parts and fields.
fn fstring_parts(s: &str, raw: bool) -> R<Vec<FPart>> {
    let chars: Vec<char> = s.chars().collect();
    let mut parts = vec![];
    let mut lit = String::new();
    let mut i = 0;
    let flush = |lit: &mut String, parts: &mut Vec<FPart>| {
        if !lit.is_empty() {
            let text = if raw { lit.clone() } else { unescape(lit) };
            parts.push(FPart::Lit(text));
            lit.clear();
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            lit.push('{');
            i += 2;
        } else if c == '}' && chars.get(i + 1) == Some(&'}') {
            lit.push('}');
            i += 2;
        } else if c == '}' {
            return syntax("f-string: single '}' is not allowed");
        } else if c == '{' {
            flush(&mut lit, &mut parts);
            let (field, ni) = fstring_field(&chars, i + 1)?;
            parts.push(field);
            i = ni;
        } else {
            lit.push(c);
            i += 1;
        }
    }
    flush(&mut lit, &mut parts);
    Ok(parts)
}

/// One `{expr!conv:spec}` field starting after its `{`; returns the field
/// and the index after its `}`.
fn fstring_field(chars: &[char], mut i: usize) -> R<(FPart, usize)> {
    // The expression runs to a top-level '!', ':' or '}' (outside
    // brackets and strings; '!=' is an operator).
    let start = i;
    let mut depth = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else {
            match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth > 0 => depth -= 1,
                '}' | ':' if depth == 0 => break,
                '!' if depth == 0 && chars.get(i + 1) != Some(&'=') => break,
                _ => {}
            }
        }
        i += 1;
    }
    let mut text: String = chars[start..i].iter().collect();
    // `{x=}`: unsupported self-documenting expressions.
    if text.trim_end().ends_with('=') && !text.trim_end().ends_with("==") {
        let t = text.trim_end();
        if !t.ends_with("!=") && !t.ends_with("<=") && !t.ends_with(">=") {
            return unsupported("f-string '=' specifiers");
        }
    }
    if text.trim().is_empty() {
        return syntax("f-string: empty expression not allowed");
    }
    text = text.trim().to_string();
    let expr = parse_expr_text(&text)?;
    let mut conv = None;
    if chars.get(i) == Some(&'!') {
        conv = chars.get(i + 1).copied();
        if !matches!(conv, Some('r' | 's' | 'a')) {
            return syntax("f-string: invalid conversion character");
        }
        i += 2;
    }
    let mut spec = vec![];
    if chars.get(i) == Some(&':') {
        i += 1;
        let mut lit = String::new();
        while i < chars.len() && chars[i] != '}' {
            if chars[i] == '{' {
                if !lit.is_empty() {
                    spec.push(FPart::Lit(std::mem::take(&mut lit)));
                }
                let (f, ni) = fstring_field(chars, i + 1)?;
                spec.push(f);
                i = ni;
            } else {
                lit.push(chars[i]);
                i += 1;
            }
        }
        if !lit.is_empty() {
            spec.push(FPart::Lit(lit));
        }
    }
    if chars.get(i) != Some(&'}') {
        return syntax("f-string: expecting '}'");
    }
    Ok((FPart::Field { expr, conv, spec }, i + 1))
}

fn parse_expr_text(text: &str) -> R<Expr> {
    let toks = tokenize(text, true).map_err(ParseError::Syntax)?;
    let mut p = P { toks, pos: 0 };
    let e = p.exprlist()?;
    if !matches!(p.peek(), Tok::End) {
        return syntax("invalid syntax");
    }
    Ok(e)
}

/// A parsed definition: an expression, or a function body.
#[derive(Debug, Clone)]
pub enum Code {
    Expr(Expr),
    Body(Vec<Stmt>),
}

/// beets' `compile_inline`: the code as an expression (in parentheses, so
/// newlines don't matter), else as a function body.
pub fn parse(code: &str) -> Result<Code, ParseError> {
    match parse_expr_text(code) {
        Ok(e) => return Ok(Code::Expr(e)),
        Err(ParseError::Unsupported(u)) => return Err(ParseError::Unsupported(u)),
        Err(ParseError::Syntax(_)) => {}
    }
    let toks = tokenize(code, false).map_err(ParseError::Syntax)?;
    let mut p = P { toks, pos: 0 };
    if matches!(p.peek(), Tok::Indent) {
        return syntax("unexpected indent");
    }
    let mut body = vec![];
    while !matches!(p.peek(), Tok::End) {
        body.extend(p.statement()?);
    }
    Ok(Code::Body(body))
}
