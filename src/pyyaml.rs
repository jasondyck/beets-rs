//! PyYAML's `safe_dump_all` and `safe_load_all`, as beets' edit plugin
//! uses them: Python values in and out, YAML 1.1 (PyYAML's implicit
//! resolvers: `yes`/`no`/`on`/`off` are booleans, `0x1f` and `017` ints,
//! `1_000` too, `~` null, `2020-01-01` a date) and PyYAML's output (sorted
//! keys, block style, PyYAML's quoting and line folding).
//!
//! The events are emitted and parsed with libyaml (`unsafe-libyaml`, a
//! translation of the C library), which PyYAML's pure-Python emitter and
//! parser are ports of; the representer, resolver and constructor on top
//! are PyYAML's `SafeRepresenter`, `Resolver` and `SafeConstructor`.

use std::ffi::{CStr, c_void};
use std::mem::MaybeUninit;
use std::ptr;
use unsafe_libyaml as unsafe_sys;

/// A Python value as PyYAML loads and dumps it.
#[derive(Debug, Clone, PartialEq)]
pub enum PyVal {
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    /// `datetime.date` / `datetime.datetime`, kept as Python's `str()`.
    Timestamp(String),
    /// `!!binary`: Python's `bytes`.
    Bytes(Vec<u8>),
    List(Vec<PyVal>),
    /// Insertion order kept (a later duplicate key replaces the value).
    Map(Vec<(PyVal, PyVal)>),
}

impl PyVal {
    /// Python's `str()`.
    pub fn py_str(&self) -> String {
        match self {
            PyVal::Str(s) | PyVal::Timestamp(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// Python's `repr()`.
    pub fn py_repr(&self) -> String {
        match self {
            PyVal::None => "None".into(),
            PyVal::Bool(b) => if *b { "True" } else { "False" }.into(),
            PyVal::Int(i) => i.to_string(),
            PyVal::Float(f) => py_float_repr(*f),
            PyVal::Str(s) => crate::hooks::py_repr(s),
            PyVal::Timestamp(s) => format!("datetime({s})"),
            PyVal::Bytes(b) => {
                let mut out = String::from("b'");
                for &c in b {
                    match c {
                        b'\\' => out.push_str("\\\\"),
                        b'\'' => out.push_str("\\'"),
                        b'\n' => out.push_str("\\n"),
                        b'\r' => out.push_str("\\r"),
                        b'\t' => out.push_str("\\t"),
                        0x20..=0x7e => out.push(c as char),
                        _ => out.push_str(&format!("\\x{c:02x}")),
                    }
                }
                out.push('\'');
                out
            }
            PyVal::List(v) => format!(
                "[{}]",
                v.iter().map(PyVal::py_repr).collect::<Vec<_>>().join(", ")
            ),
            PyVal::Map(m) => format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!("{}: {}", k.py_repr(), v.py_repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Python's type name (for beets' "found {type}" message).
    pub fn type_name(&self) -> &'static str {
        match self {
            PyVal::None => "NoneType",
            PyVal::Bool(_) => "bool",
            PyVal::Int(_) => "int",
            PyVal::Float(_) => "float",
            PyVal::Str(_) => "str",
            PyVal::Timestamp(s) if s.contains(':') => "datetime",
            PyVal::Timestamp(_) => "date",
            PyVal::Bytes(_) => "bytes",
            PyVal::List(_) => "list",
            PyVal::Map(_) => "dict",
        }
    }

    /// Python's `==` (numbers compare across int, float and bool).
    pub fn py_eq(&self, other: &PyVal) -> bool {
        let num = |v: &PyVal| match v {
            PyVal::Bool(b) => Some(*b as i64 as f64),
            PyVal::Int(i) => Some(*i as f64),
            PyVal::Float(f) => Some(*f),
            _ => None,
        };
        match (num(self), num(other)) {
            (Some(a), Some(b)) => a == b,
            _ => match (self, other) {
                (PyVal::List(a), PyVal::List(b)) => {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.py_eq(y))
                }
                _ => self == other,
            },
        }
    }
}

/// Python's `repr(float)`.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        // Shortest round-trip digits in exponent form, as Python.
        let s = format!("{f:e}");
        let (m, e) = s.split_once('e').unwrap();
        let e: i32 = e.parse().unwrap();
        let sign = if e < 0 { '-' } else { '+' };
        return format!("{m}e{sign}{:02}", e.abs());
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

// ------------------------------------------------------------- resolving

fn re(p: &str) -> regex::Regex {
    regex::Regex::new(p).unwrap()
}

/// PyYAML's implicit resolvers, in the order they're registered.
fn resolve_plain(value: &str) -> &'static str {
    use std::sync::OnceLock;
    static RES: OnceLock<Vec<(&'static str, regex::Regex)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        vec![
            (
                "bool",
                re(r"^(?:yes|Yes|YES|no|No|NO|true|True|TRUE|false|False|FALSE|on|On|ON|off|Off|OFF)$"),
            ),
            (
                "float",
                re(r"^(?:[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN))$"),
            ),
            (
                "int",
                re(r"^(?:[-+]?0b[0-1_]+|[-+]?0[0-7_]+|[-+]?(?:0|[1-9][0-9_]*)|[-+]?0x[0-9a-fA-F_]+|[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+)$"),
            ),
            ("merge", re(r"^(?:<<)$")),
            ("null", re(r"^(?:~|null|Null|NULL|)$")),
            (
                "timestamp",
                re(r"^(?:[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]|[0-9][0-9][0-9][0-9]-[0-9][0-9]?-[0-9][0-9]?(?:[Tt]|[ \t]+)[0-9][0-9]?:[0-9][0-9]:[0-9][0-9](?:\.[0-9]*)?(?:[ \t]*(?:Z|[-+][0-9][0-9]?(?::[0-9][0-9])?))?)$"),
            ),
            ("value", re(r"^(?:=)$")),
        ]
    });
    // PyYAML looks resolvers up by the value's first character, and each
    // pattern only matches values starting with one of its characters.
    for (tag, r) in res {
        if r.is_match(value) {
            return tag;
        }
    }
    "str"
}

// --------------------------------------------------------------- dumping

/// The libyaml emitter's output callback: append the bytes it produced.
///
/// # Safety
///
/// `data` must point to the live `Vec<u8>` registered with
/// `yaml_emitter_set_output` (see `Emitter::new`), and `buffer`/`size` must
/// describe a readable region, as libyaml guarantees for its write handler.
unsafe fn write_handler(data: *mut c_void, buffer: *mut u8, size: u64) -> i32 {
    // SAFETY: libyaml calls the handler with the pointer passed to
    // `yaml_emitter_set_output`; it points at `Emitter::out`, which is boxed
    // so its address stays valid for as long as the emitter.
    let out = unsafe { &mut *data.cast::<Vec<u8>>() };
    // SAFETY: libyaml passes a readable buffer of `size` output bytes.
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer, size as usize) });
    1
}

enum Ev<'a> {
    StreamStart,
    StreamEnd,
    DocStart,
    DocEnd,
    Scalar {
        tag: &'static str,
        value: &'a str,
        plain_implicit: bool,
        quoted_implicit: bool,
    },
    /// `flow`: a `[...]` sequence (confuse's short lists); `anchor` names
    /// a shared value (`&id001`).
    SeqStart {
        flow: bool,
        anchor: Option<&'a str>,
    },
    SeqEnd,
    /// `flow`: a `{...}` mapping (confuse never uses it).
    MapStart {
        flow: bool,
    },
    MapEnd,
    /// A `*id001` reference to an anchored value.
    Alias(&'a str),
}

struct Emitter {
    sys: Box<MaybeUninit<unsafe_sys::yaml_emitter_t>>,
    /// Boxed: libyaml's write handler holds a pointer to it.
    #[allow(clippy::box_collection)]
    out: Box<Vec<u8>>,
    /// confuse's Dumper settings: indent 4, width 1000, `yes`/`no`
    /// booleans, empty nulls, flow lists shorter than 4, block mappings,
    /// insertion order.
    confuse: bool,
    /// `sort_keys=True` (PyYAML's default; confuse's dump keeps order).
    sort_keys: bool,
    /// Whether the tagged `&id001` value has been written.
    wrote_separators_anchor: bool,
}

impl Emitter {
    fn new() -> Self {
        let mut e = Emitter {
            sys: Box::new(MaybeUninit::uninit()),
            out: Box::default(),
            confuse: false,
            sort_keys: true,
            wrote_separators_anchor: false,
        };
        // SAFETY: `e.sys` is a fresh `MaybeUninit` that
        // `yaml_emitter_initialize` fills in; `e.out` is boxed, so the pointer
        // registered as the write handler's data stays valid when the emitter
        // is moved.
        unsafe {
            let p = e.sys.as_mut_ptr();
            assert!(!unsafe_sys::yaml_emitter_initialize(p).fail, "yaml emitter");
            unsafe_sys::yaml_emitter_set_unicode(p, true);
            let data: *mut Vec<u8> = &mut *e.out;
            unsafe_sys::yaml_emitter_set_output(p, write_handler, data.cast());
        }
        e
    }

    fn emit(&mut self, ev: Ev) {
        let mut event = MaybeUninit::<unsafe_sys::yaml_event_t>::uninit();
        let event = event.as_mut_ptr();
        let mut anchor_buf = String::new();
        // SAFETY: `self.sys` was initialized by `Emitter::new` and is freed
        // once in `finish`. `event` starts as `MaybeUninit` and each
        // `yaml_*_event_initialize` fills it; libyaml copies the tag/anchor
        // strings it is given, so the temporaries above need only live until
        // the call returns. Every event is passed to `yaml_emitter_emit`,
        // which takes ownership of it, exactly once.
        unsafe {
            let em = self.sys.as_mut_ptr();
            let ok = match ev {
                Ev::StreamStart => unsafe_sys::yaml_stream_start_event_initialize(
                    event,
                    unsafe_sys::YAML_UTF8_ENCODING,
                ),
                Ev::StreamEnd => unsafe_sys::yaml_stream_end_event_initialize(event),
                Ev::DocStart => unsafe_sys::yaml_document_start_event_initialize(
                    event,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    true,
                ),
                Ev::DocEnd => unsafe_sys::yaml_document_end_event_initialize(event, true),
                Ev::Scalar {
                    tag,
                    value,
                    plain_implicit,
                    quoted_implicit,
                } => {
                    let tag = format!("tag:yaml.org,2002:{tag}\0");
                    unsafe_sys::yaml_scalar_event_initialize(
                        event,
                        ptr::null(),
                        tag.as_ptr(),
                        value.as_ptr(),
                        value.len() as i32,
                        plain_implicit,
                        quoted_implicit,
                        unsafe_sys::YAML_ANY_SCALAR_STYLE,
                    )
                }
                Ev::SeqStart { flow, anchor } => unsafe_sys::yaml_sequence_start_event_initialize(
                    event,
                    match anchor {
                        Some(a) => {
                            anchor_buf.clear();
                            anchor_buf.push_str(a);
                            anchor_buf.push('\0');
                            anchor_buf.as_ptr()
                        }
                        None => ptr::null(),
                    },
                    c"tag:yaml.org,2002:seq".as_ptr().cast(),
                    true,
                    if flow {
                        unsafe_sys::YAML_FLOW_SEQUENCE_STYLE
                    } else {
                        unsafe_sys::YAML_BLOCK_SEQUENCE_STYLE
                    },
                ),
                Ev::SeqEnd => unsafe_sys::yaml_sequence_end_event_initialize(event),
                Ev::MapStart { flow } => unsafe_sys::yaml_mapping_start_event_initialize(
                    event,
                    ptr::null(),
                    c"tag:yaml.org,2002:map".as_ptr().cast(),
                    true,
                    if flow {
                        unsafe_sys::YAML_FLOW_MAPPING_STYLE
                    } else {
                        unsafe_sys::YAML_BLOCK_MAPPING_STYLE
                    },
                ),
                Ev::MapEnd => unsafe_sys::yaml_mapping_end_event_initialize(event),
                Ev::Alias(anchor) => {
                    unsafe_sys::yaml_alias_event_initialize(event, format!("{anchor}\0").as_ptr())
                }
            };
            assert!(!ok.fail, "yaml event");
            assert!(!unsafe_sys::yaml_emitter_emit(em, event).fail, "yaml emit");
        }
    }

    fn finish(mut self) -> String {
        // SAFETY: `self.sys` was initialized by `Emitter::new`; `finish`
        // consumes `self`, so the emitter is flushed and freed exactly once.
        unsafe {
            let _ = unsafe_sys::yaml_emitter_flush(self.sys.as_mut_ptr());
            unsafe_sys::yaml_emitter_delete(self.sys.as_mut_ptr());
        }
        String::from_utf8_lossy(&std::mem::take(&mut *self.out)).into_owned()
    }

    /// PyYAML's `SafeRepresenter` + `Serializer` for one value.
    fn value(&mut self, v: &PyVal) {
        let scalar = |e: &mut Self, tag: &'static str, s: &str| {
            let plain_implicit = resolve_plain(s) == tag;
            e.emit(Ev::Scalar {
                tag,
                value: s,
                plain_implicit,
                quoted_implicit: tag == "str",
            });
        };
        match v {
            PyVal::None => scalar(self, "null", "null"),
            PyVal::Bool(b) => scalar(self, "bool", if *b { "true" } else { "false" }),
            PyVal::Int(i) => scalar(self, "int", &i.to_string()),
            PyVal::Float(f) => scalar(self, "float", &represent_float(*f)),
            PyVal::Str(s) => scalar(self, "str", s),
            PyVal::Timestamp(s) => scalar(self, "timestamp", &s.replace(' ', "T")),
            PyVal::Bytes(b) => scalar(self, "binary", &base64(b)),
            PyVal::List(items) => {
                self.emit(Ev::SeqStart {
                    flow: false,
                    anchor: None,
                });
                for i in items {
                    self.value(i);
                }
                self.emit(Ev::SeqEnd);
            }
            PyVal::Map(pairs) => {
                let mut pairs: Vec<&(PyVal, PyVal)> = pairs.iter().collect();
                if self.sort_keys {
                    pairs.sort_by_key(|a| a.0.py_str());
                }
                self.emit(Ev::MapStart { flow: false });
                for (k, v) in pairs {
                    self.value(k);
                    self.value(v);
                }
                self.emit(Ev::MapEnd);
            }
        }
    }
}

/// `SafeRepresenter.represent_float`.
fn represent_float(f: f64) -> String {
    if f.is_nan() {
        return ".nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { ".inf" } else { "-.inf" }.into();
    }
    let mut v = py_float_repr(f).to_lowercase();
    if !v.contains('.') && v.contains('e') {
        v = v.replacen('e', ".0e", 1);
    }
    v
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `yaml.safe_dump_all(docs, allow_unicode=True, default_flow_style=False)`.
pub fn dump_all(docs: &[PyVal]) -> String {
    let mut e = Emitter::new();
    e.emit(Ev::StreamStart);
    for d in docs {
        e.emit(Ev::DocStart);
        e.value(d);
        e.emit(Ev::DocEnd);
    }
    e.emit(Ev::StreamEnd);
    e.finish()
}

/// confuse's `config.dump`: PyYAML's `yaml.dump` with confuse's Dumper
/// (`default_flow_style=None`, `indent=4`, `width=1000`), keys in insertion
/// order, booleans as `yes`/`no`, `None` as nothing, lists shorter than
/// four items inline and mappings always in block style.
pub fn dump_confuse(value: &serde_yaml::Value) -> String {
    let mut e = Emitter::new();
    e.confuse = true;
    e.sort_keys = false;
    // SAFETY: `e` was just created, so `e.sys` is an initialized emitter.
    unsafe {
        let p = e.sys.as_mut_ptr();
        unsafe_sys::yaml_emitter_set_indent(p, 4);
        unsafe_sys::yaml_emitter_set_width(p, 1000);
    }
    // beets' export plugin reuses one tuple for the json and jsonlines
    // separators, so PyYAML writes it once with an anchor and then as an
    // alias.
    let alias = export_separators_alias(value);
    e.emit(Ev::StreamStart);
    e.emit(Ev::DocStart);
    let mut path: Vec<String> = vec![];
    e.confuse_value_at(value, &mut path, &alias);
    e.emit(Ev::DocEnd);
    e.emit(Ev::StreamEnd);
    e.finish()
}

/// The two `export.*.formatting.separators` paths, when the values are the
/// same list (beets' shared tuple).
fn export_separators_alias(value: &serde_yaml::Value) -> Option<serde_yaml::Value> {
    let get = |format: &str| {
        value
            .get("export")?
            .get(format)?
            .get("formatting")?
            .get("separators")
            .cloned()
    };
    let a = get("json")?;
    let b = get("jsonlines")?;
    (a == b && a.is_sequence()).then_some(a)
}

/// Whether `path` is one of the export separator lists.
fn is_export_separators(path: &[String]) -> bool {
    matches!(
        path,
        [a, b, c, d] if a == "export"
            && (b == "json" || b == "jsonlines")
            && c == "formatting"
            && d == "separators"
    )
}

impl Emitter {
    /// confuse's Dumper + PyYAML's `SafeRepresenter` for a config value.
    fn confuse_value_at(
        &mut self,
        v: &serde_yaml::Value,
        path: &mut Vec<String>,
        alias: &Option<serde_yaml::Value>,
    ) {
        use serde_yaml::Value as Y;
        let scalar = |e: &mut Self, tag: &'static str, s: &str| {
            let plain_implicit = resolve_plain(s) == tag;
            e.emit(Ev::Scalar {
                tag,
                value: s,
                plain_implicit,
                quoted_implicit: tag == "str" || plain_implicit,
            });
        };
        match v {
            Y::Null => scalar(self, "null", ""),
            Y::Bool(b) => scalar(self, "bool", if *b { "yes" } else { "no" }),
            Y::Number(n) => {
                if let Some(i) = n.as_i64() {
                    scalar(self, "int", &i.to_string())
                } else if let Some(u) = n.as_u64() {
                    scalar(self, "int", &u.to_string())
                } else {
                    let f = n.as_f64().unwrap_or(0.0);
                    scalar(self, "float", &represent_float(f))
                }
            }
            Y::String(s) => scalar(self, "str", s),
            Y::Sequence(items) => {
                if self.wrote_separators_anchor {
                    // The second of beets' shared tuples.
                    self.emit(Ev::Alias("id001"));
                    return;
                }
                let anchor = if alias.is_some() && is_export_separators(path) {
                    self.wrote_separators_anchor = true;
                    Some("id001")
                } else {
                    None
                };
                self.emit(Ev::SeqStart {
                    flow: items.len() < 4,
                    anchor,
                });
                for (i, item) in items.iter().enumerate() {
                    path.push(i.to_string());
                    self.confuse_value_at(item, path, alias);
                    path.pop();
                }
                self.emit(Ev::SeqEnd);
            }
            Y::Mapping(m) => {
                self.emit(Ev::MapStart { flow: false });
                for (k, val) in m {
                    if let Some(s) = k.as_str() {
                        path.push(s.to_string());
                    }
                    self.confuse_value_at(k, path, alias);
                    self.confuse_value_at(val, path, alias);
                    if k.as_str().is_some() {
                        path.pop();
                    }
                }
                self.emit(Ev::MapEnd);
            }
            Y::Tagged(t) => self.confuse_value_at(&t.value, path, alias),
        }
    }
}

// --------------------------------------------------------------- loading

/// A position in the input, for PyYAML's error messages.
#[derive(Clone, Copy, Debug)]
struct Mark {
    line: usize,
    column: usize,
}

impl Mark {
    fn from_sys(m: unsafe_sys::yaml_mark_t) -> Self {
        Mark {
            line: m.line as usize,
            column: m.column as usize,
        }
    }

    /// PyYAML's `Mark.__str__` (with its snippet) for a string input.
    fn describe(&self, input: &str) -> String {
        let mut out = format!(
            "  in \"<unicode string>\", line {}, column {}",
            self.line + 1,
            self.column + 1
        );
        // The buffer is the input plus "\0"; find the pointer.
        let buf: Vec<char> = input.chars().chain(std::iter::once('\0')).collect();
        let breaks = ['\0', '\r', '\n', '\u{85}', '\u{2028}', '\u{2029}'];
        let mut line = 0;
        let mut pointer = 0;
        let mut i = 0;
        while i < buf.len() && line < self.line {
            let c = buf[i];
            let crlf = c == '\r' && buf.get(i + 1) == Some(&'\n');
            if matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}') && !crlf {
                line += 1;
            }
            i += 1;
        }
        pointer = (i + self.column)
            .min(buf.len().saturating_sub(1))
            .max(pointer);
        let max_half = 75.0 / 2.0 - 1.0;
        let mut head = "";
        let mut start = pointer;
        while start > 0 && !breaks.contains(&buf[start - 1]) {
            start -= 1;
            if (pointer - start) as f64 > max_half {
                head = " ... ";
                start += 5;
                break;
            }
        }
        let mut tail = "";
        let mut end = pointer;
        while end < buf.len() && !breaks.contains(&buf[end]) {
            end += 1;
            if (end - pointer) as f64 > max_half {
                tail = " ... ";
                end -= 5;
                break;
            }
        }
        let snippet: String = buf[start..end].iter().collect();
        out.push_str(":\n");
        out.push_str(&format!("    {head}{snippet}{tail}\n"));
        out.push_str(&" ".repeat(4 + pointer - start + head.len()));
        out.push('^');
        out
    }
}

/// PyYAML's `MarkedYAMLError.__str__`.
fn marked_error(
    input: &str,
    context: Option<&str>,
    context_mark: Option<Mark>,
    problem: &str,
    problem_mark: Option<Mark>,
) -> String {
    let mut lines = vec![];
    if let Some(c) = context {
        lines.push(c.to_string());
    }
    if let Some(cm) = context_mark
        && problem_mark.is_none_or(|pm| pm.line != cm.line || pm.column != cm.column)
    {
        lines.push(cm.describe(input));
    }
    lines.push(problem.to_string());
    if let Some(pm) = problem_mark {
        lines.push(pm.describe(input));
    }
    lines.join("\n")
}

enum PEv {
    StreamEnd,
    DocStart,
    DocEnd,
    Alias(String, Mark),
    Scalar {
        anchor: Option<String>,
        tag: Option<String>,
        value: String,
        plain: bool,
        mark: Mark,
    },
    SeqStart(Option<String>, Option<String>, Mark),
    SeqEnd,
    MapStart(Option<String>, Option<String>, Mark),
    MapEnd,
    Other,
}

struct Parser<'a> {
    sys: Box<MaybeUninit<unsafe_sys::yaml_parser_t>>,
    input: &'a str,
}

impl Drop for Parser<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.sys` was initialized by `Parser::new` (which panics
        // otherwise), and `drop` runs it exactly once.
        unsafe { unsafe_sys::yaml_parser_delete(self.sys.as_mut_ptr()) }
    }
}

/// A libyaml string pointer as an owned Rust string.
///
/// # Safety
///
/// `p` must be null or a valid NUL-terminated C string that stays valid for
/// the duration of the call, as libyaml's event/token strings are.
unsafe fn cstr(p: *const u8) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(
            // SAFETY: the checker's contract: non-null means a valid
            // NUL-terminated string, borrowed only for this call.
            unsafe { CStr::from_ptr(p.cast()) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        let mut p = Parser {
            sys: Box::new(MaybeUninit::uninit()),
            input,
        };
        // SAFETY: `p.sys` is a fresh `MaybeUninit` filled by
        // `yaml_parser_initialize`; libyaml borrows `input`'s buffer, which
        // the `&'a str` field keeps alive for the parser's lifetime.
        unsafe {
            let sp = p.sys.as_mut_ptr();
            assert!(!unsafe_sys::yaml_parser_initialize(sp).fail, "yaml parser");
            unsafe_sys::yaml_parser_set_encoding(sp, unsafe_sys::YAML_UTF8_ENCODING);
            unsafe_sys::yaml_parser_set_input_string(sp, input.as_ptr(), input.len() as u64);
        }
        p
    }

    fn next(&mut self) -> Result<PEv, String> {
        let mut event = MaybeUninit::<unsafe_sys::yaml_event_t>::uninit();
        // SAFETY: `self.sys` was initialized by `Parser::new`. `event` is a
        // `MaybeUninit` that `yaml_parser_parse` writes on success; every
        // string/byte pointer read out of it is copied into owned data before
        // `yaml_event_delete` frees the event; on a parse error libyaml has
        // not filled the event, so there is nothing to delete.
        unsafe {
            let sp = self.sys.as_mut_ptr();
            let ev = event.as_mut_ptr();
            if unsafe_sys::yaml_parser_parse(sp, ev).fail {
                let p = &*sp;
                let pm = Mark::from_sys(p.problem_mark);
                let problem = pyyaml_problem(
                    self.input,
                    &cstr(p.problem.cast()).unwrap_or_default(),
                    p.problem_mark,
                );
                let context = cstr(p.context.cast());
                let cm = context.as_ref().map(|_| Mark::from_sys(p.context_mark));
                return Err(marked_error(
                    self.input,
                    context.as_deref(),
                    cm,
                    &problem,
                    Some(pm),
                ));
            }
            let e = &*ev;
            let mark = Mark::from_sys(e.start_mark);
            let out = match e.type_ {
                unsafe_sys::YAML_STREAM_END_EVENT => PEv::StreamEnd,
                unsafe_sys::YAML_DOCUMENT_START_EVENT => PEv::DocStart,
                unsafe_sys::YAML_DOCUMENT_END_EVENT => PEv::DocEnd,
                unsafe_sys::YAML_ALIAS_EVENT => {
                    PEv::Alias(cstr(e.data.alias.anchor).unwrap(), mark)
                }
                unsafe_sys::YAML_SCALAR_EVENT => {
                    let s = e.data.scalar;
                    let bytes = std::slice::from_raw_parts(s.value, s.length as usize);
                    PEv::Scalar {
                        anchor: cstr(s.anchor),
                        tag: cstr(s.tag),
                        value: String::from_utf8_lossy(bytes).into_owned(),
                        plain: s.plain_implicit,
                        mark,
                    }
                }
                unsafe_sys::YAML_SEQUENCE_START_EVENT => PEv::SeqStart(
                    cstr(e.data.sequence_start.anchor),
                    cstr(e.data.sequence_start.tag),
                    mark,
                ),
                unsafe_sys::YAML_SEQUENCE_END_EVENT => PEv::SeqEnd,
                unsafe_sys::YAML_MAPPING_START_EVENT => PEv::MapStart(
                    cstr(e.data.mapping_start.anchor),
                    cstr(e.data.mapping_start.tag),
                    mark,
                ),
                unsafe_sys::YAML_MAPPING_END_EVENT => PEv::MapEnd,
                _ => PEv::Other,
            };
            unsafe_sys::yaml_event_delete(ev);
            Ok(out)
        }
    }
}

/// PyYAML's wording of a libyaml problem: its parser names the token it
/// found ("expected ',' or ']', but got '<stream end>'").
fn pyyaml_problem(input: &str, problem: &str, mark: unsafe_sys::yaml_mark_t) -> String {
    let token = || token_at(input, mark).unwrap_or("<stream end>");
    match problem {
        "did not find expected ',' or ']'" => {
            format!("expected ',' or ']', but got '{}'", token())
        }
        "did not find expected ',' or '}'" => {
            format!("expected ',' or '}}', but got '{}'", token())
        }
        "did not find expected key" | "did not find expected '-' indicator" => {
            format!("expected <block end>, but found '{}'", token())
        }
        "did not find expected node content" => {
            format!("expected the node content, but found '{}'", token())
        }
        "did not find expected <document start>" => {
            format!("expected '<document start>', but found '{}'", token())
        }
        "mapping values are not allowed in this context" => {
            "mapping values are not allowed here".into()
        }
        "block sequence entries are not allowed in this context" => {
            "sequence entries are not allowed here".into()
        }
        "found character that cannot start any token" => {
            let c = char_at(input, mark).unwrap_or('\0');
            format!(
                "found character {} that cannot start any token",
                crate::hooks::py_repr(&c.to_string())
            )
        }
        other => other.to_string(),
    }
}

fn char_at(input: &str, mark: unsafe_sys::yaml_mark_t) -> Option<char> {
    let line = input.split('\n').nth(mark.line as usize)?;
    line.chars().nth(mark.column as usize)
}

/// PyYAML's id of the token libyaml's scanner finds at `mark`.
fn token_at(input: &str, mark: unsafe_sys::yaml_mark_t) -> Option<&'static str> {
    let mut parser = Box::new(MaybeUninit::<unsafe_sys::yaml_parser_t>::uninit());
    let mut found = None;
    // SAFETY: `parser` is a local `MaybeUninit` initialized here (and left
    // uninitialized only on the error return, before libyaml used it); the
    // input string lives in `input` for the whole call. Each scanned token is
    // a `MaybeUninit` filled by `yaml_parser_scan` and deleted before the next
    // iteration or return, and the parser is deleted before returning.
    unsafe {
        let sp = parser.as_mut_ptr();
        if unsafe_sys::yaml_parser_initialize(sp).fail {
            return None;
        }
        unsafe_sys::yaml_parser_set_encoding(sp, unsafe_sys::YAML_UTF8_ENCODING);
        unsafe_sys::yaml_parser_set_input_string(sp, input.as_ptr(), input.len() as u64);
        loop {
            let mut tok = MaybeUninit::<unsafe_sys::yaml_token_t>::uninit();
            let tp = tok.as_mut_ptr();
            if unsafe_sys::yaml_parser_scan(sp, tp).fail {
                break;
            }
            let t = &*tp;
            let ty = t.type_;
            let at = t.start_mark.line == mark.line && t.start_mark.column == mark.column;
            unsafe_sys::yaml_token_delete(tp);
            if at && (found.is_none() || found == Some("<block end>")) {
                found = Some(match ty {
                    unsafe_sys::YAML_STREAM_END_TOKEN => "<stream end>",
                    unsafe_sys::YAML_DOCUMENT_START_TOKEN => "<document start>",
                    unsafe_sys::YAML_DOCUMENT_END_TOKEN => "<document end>",
                    unsafe_sys::YAML_BLOCK_SEQUENCE_START_TOKEN => "<block sequence start>",
                    unsafe_sys::YAML_BLOCK_MAPPING_START_TOKEN => "<block mapping start>",
                    unsafe_sys::YAML_BLOCK_END_TOKEN => "<block end>",
                    unsafe_sys::YAML_FLOW_SEQUENCE_START_TOKEN => "[",
                    unsafe_sys::YAML_FLOW_SEQUENCE_END_TOKEN => "]",
                    unsafe_sys::YAML_FLOW_MAPPING_START_TOKEN => "{",
                    unsafe_sys::YAML_FLOW_MAPPING_END_TOKEN => "}",
                    unsafe_sys::YAML_BLOCK_ENTRY_TOKEN => "-",
                    unsafe_sys::YAML_FLOW_ENTRY_TOKEN => ",",
                    unsafe_sys::YAML_KEY_TOKEN => "?",
                    unsafe_sys::YAML_VALUE_TOKEN => ":",
                    unsafe_sys::YAML_ALIAS_TOKEN => "<alias>",
                    unsafe_sys::YAML_ANCHOR_TOKEN => "<anchor>",
                    unsafe_sys::YAML_TAG_TOKEN => "<tag>",
                    unsafe_sys::YAML_SCALAR_TOKEN => "<scalar>",
                    _ => "<directive>",
                });
                // libyaml's block end can share the mark with the token
                // PyYAML's parser reports: look on.
                if found != Some("<block end>") {
                    break;
                }
            }
            if ty == unsafe_sys::YAML_STREAM_END_TOKEN {
                break;
            }
        }
        unsafe_sys::yaml_parser_delete(sp);
    }
    found
}

const TAG: &str = "tag:yaml.org,2002:";

struct Composer<'a> {
    p: Parser<'a>,
    anchors: std::collections::HashMap<String, PyVal>,
}

impl Composer<'_> {
    fn err(&self, problem: &str, mark: Mark) -> String {
        marked_error(self.p.input, None, None, problem, Some(mark))
    }

    fn constructor_error(&self, problem: &str, mark: Mark) -> String {
        // PyYAML's ConstructorError has a context of None.
        self.err(problem, mark)
    }

    /// Compose and construct one node, given its first event.
    fn node(&mut self, ev: PEv) -> Result<PyVal, String> {
        match ev {
            PEv::Alias(name, mark) => self.anchors.get(&name).cloned().ok_or_else(|| {
                marked_error(
                    self.p.input,
                    None,
                    None,
                    &format!("found undefined alias '{name}'"),
                    Some(mark),
                )
            }),
            PEv::Scalar {
                anchor,
                tag,
                value,
                plain,
                mark,
            } => {
                let tag = match tag.as_deref() {
                    None | Some("!") if plain && tag.is_none() => {
                        format!("{TAG}{}", resolve_plain(&value))
                    }
                    None | Some("!") => format!("{TAG}str"),
                    Some(t) => t.to_string(),
                };
                let v = self.scalar(&tag, &value, mark)?;
                if let Some(a) = anchor {
                    self.anchors.insert(a, v.clone());
                }
                Ok(v)
            }
            PEv::SeqStart(anchor, tag, mark) => {
                if let Some(t) = tag.as_deref()
                    && t != "!"
                    && t != format!("{TAG}seq")
                {
                    return Err(self.constructor_error(
                        &format!("could not determine a constructor for the tag '{t}'"),
                        mark,
                    ));
                }
                let mut items = vec![];
                loop {
                    match self.p.next()? {
                        PEv::SeqEnd => break,
                        ev => items.push(self.node(ev)?),
                    }
                }
                let v = PyVal::List(items);
                if let Some(a) = anchor {
                    self.anchors.insert(a, v.clone());
                }
                Ok(v)
            }
            PEv::MapStart(anchor, tag, mark) => {
                if let Some(t) = tag.as_deref()
                    && t != "!"
                    && t != format!("{TAG}map")
                {
                    return Err(self.constructor_error(
                        &format!("could not determine a constructor for the tag '{t}'"),
                        mark,
                    ));
                }
                let mut pairs: Vec<(PyVal, PyVal)> = vec![];
                loop {
                    let kev = self.p.next()?;
                    if let PEv::MapEnd = kev {
                        break;
                    }
                    let kmark = match &kev {
                        PEv::Scalar { mark, .. }
                        | PEv::Alias(_, mark)
                        | PEv::SeqStart(_, _, mark)
                        | PEv::MapStart(_, _, mark) => *mark,
                        _ => mark,
                    };
                    let k = self.node(kev)?;
                    if matches!(k, PyVal::List(_) | PyVal::Map(_)) {
                        return Err(marked_error(
                            self.p.input,
                            Some("while constructing a mapping"),
                            Some(mark),
                            "found unhashable key",
                            Some(kmark),
                        ));
                    }
                    let vev = self.p.next()?;
                    let v = self.node(vev)?;
                    match pairs.iter_mut().find(|(pk, _)| pk.py_eq(&k)) {
                        Some(p) => p.1 = v,
                        None => pairs.push((k, v)),
                    }
                }
                let v = PyVal::Map(pairs);
                if let Some(a) = anchor {
                    self.anchors.insert(a, v.clone());
                }
                Ok(v)
            }
            _ => Ok(PyVal::None),
        }
    }

    /// `SafeConstructor`'s scalar constructors.
    fn scalar(&self, tag: &str, value: &str, mark: Mark) -> Result<PyVal, String> {
        let Some(short) = tag.strip_prefix(TAG) else {
            return Err(self.constructor_error(
                &format!("could not determine a constructor for the tag '{tag}'"),
                mark,
            ));
        };
        Ok(match short {
            "str" => PyVal::Str(value.to_string()),
            "null" => PyVal::None,
            "bool" => PyVal::Bool(matches!(
                value.to_lowercase().as_str(),
                "yes" | "true" | "on"
            )),
            "int" => construct_int(value)
                .ok_or_else(|| self.err(&format!("invalid literal for int(): {value:?}"), mark))?,
            "float" => construct_float(value).ok_or_else(|| {
                self.err(
                    &format!(
                        "could not convert string to float: {}",
                        crate::hooks::py_repr(value)
                    ),
                    mark,
                )
            })?,
            "timestamp" => PyVal::Timestamp(construct_timestamp(value)),
            "binary" => PyVal::Bytes(debase64(value)),
            _ => {
                return Err(self.constructor_error(
                    &format!("could not determine a constructor for the tag '{tag}'"),
                    mark,
                ));
            }
        })
    }
}

fn construct_int(value: &str) -> Option<PyVal> {
    let v = value.replace('_', "");
    let (sign, v) = match v.strip_prefix('-') {
        Some(r) => (-1i128, r.to_string()),
        None => (1, v.strip_prefix('+').unwrap_or(&v).to_string()),
    };
    let n = if v == "0" {
        0
    } else if let Some(b) = v.strip_prefix("0b") {
        i128::from_str_radix(b, 2).ok()?
    } else if let Some(h) = v.strip_prefix("0x") {
        i128::from_str_radix(h, 16).ok()?
    } else if let Some(o) = v.strip_prefix('0') {
        i128::from_str_radix(o, 8).ok()?
    } else if v.contains(':') {
        let mut n = 0i128;
        for part in v.split(':') {
            n = n * 60 + part.parse::<i128>().ok()?;
        }
        n
    } else {
        v.parse().ok()?
    };
    Some(PyVal::Int(sign * n))
}

fn construct_float(value: &str) -> Option<PyVal> {
    let v = value.replace('_', "").to_lowercase();
    let (sign, v) = match v.strip_prefix('-') {
        Some(r) => (-1.0, r.to_string()),
        None => (1.0, v.strip_prefix('+').unwrap_or(&v).to_string()),
    };
    let f = if v == ".inf" {
        f64::INFINITY
    } else if v == ".nan" {
        return Some(PyVal::Float(f64::NAN));
    } else if v.contains(':') {
        let mut n = 0.0;
        for part in v.split(':') {
            n = n * 60.0 + part.parse::<f64>().ok()?;
        }
        n
    } else {
        v.parse().ok()?
    };
    Some(PyVal::Float(sign * f))
}

/// `construct_yaml_timestamp`, as Python's `str()` of the date or datetime.
fn construct_timestamp(value: &str) -> String {
    let r = re(
        r"^([0-9]{4})-([0-9]{1,2})-([0-9]{1,2})(?:(?:[Tt]|[ \t]+)([0-9]{1,2}):([0-9]{2}):([0-9]{2})(?:\.([0-9]*))?(?:[ \t]*(Z|([-+])([0-9]{1,2})(?::([0-9]{2}))?))?)?$",
    );
    let Some(c) = r.captures(value) else {
        return value.to_string();
    };
    let g = |i: usize| c.get(i).map(|m| m.as_str());
    let n = |i: usize| g(i).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    let date = format!("{:04}-{:02}-{:02}", n(1), n(2), n(3));
    if g(4).is_none() {
        return date;
    }
    let mut s = format!("{date} {:02}:{:02}:{:02}", n(4), n(5), n(6));
    if let Some(frac) = g(7).filter(|f| !f.is_empty()) {
        let mut f: String = frac.chars().take(6).collect();
        while f.len() < 6 {
            f.push('0');
        }
        if f != "000000" {
            s.push('.');
            s.push_str(&f);
        }
    }
    if let Some(z) = g(8) {
        if z == "Z" {
            s.push_str("+00:00");
        } else {
            s.push_str(&format!("{}{:02}:{:02}", g(9).unwrap_or("+"), n(10), n(11)));
        }
    }
    s
}

fn debase64(s: &str) -> Vec<u8> {
    let mut out = vec![];
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buf = buf << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

/// `yaml.safe_load_all(s)`: every document. The error is PyYAML's message.
pub fn load_all(input: &str) -> Result<Vec<PyVal>, String> {
    let mut c = Composer {
        p: Parser::new(input),
        anchors: Default::default(),
    };
    let mut docs = vec![];
    loop {
        match c.p.next()? {
            PEv::StreamEnd => break,
            PEv::DocStart => {
                c.anchors.clear();
                let ev = c.p.next()?;
                let v = c.node(ev)?;
                // The document end.
                loop {
                    match c.p.next()? {
                        PEv::DocEnd => break,
                        PEv::StreamEnd => return Ok(docs),
                        _ => {}
                    }
                }
                docs.push(v);
            }
            _ => {}
        }
    }
    Ok(docs)
}

#[cfg(test)]
mod tests;
