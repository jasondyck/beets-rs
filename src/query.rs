//! beets' query language: a port of `dbcore/query.py`, `dbcore/queryparse.py`,
//! `dbcore/sort.py` and `library/queries.py`.
//!
//! As in beets, a query compiles to an SQL `WHERE` clause when every part of
//! it is on a fixed field of the model or of its related model (items ↔
//! albums, through a join); otherwise ("slow" queries on flexible or computed
//! fields) every row is loaded and matched here, with Python's semantics.
//! Sorting works the same way: a trailing run of fixed-field sorts goes to
//! SQL, the rest is sorted here.

use crate::config::Config;
use crate::model::{FieldType, Kind, Record, Value, schema};
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;
use rusqlite::types::Value as SqlValue;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

// ------------------------------------------------------------------ context

/// What query parsing takes from the configuration (beets reads these from
/// its global config and `context.music_dir`).
#[derive(Clone, Debug)]
pub struct Context {
    /// The library directory: relative `path:` queries are relative to it.
    pub music_dir: Option<PathBuf>,
    /// `sort_case_insensitive`, for sorts given in a query.
    pub sort_case_insensitive: bool,
    /// `sort_item` / `sort_album`: the default sorts.
    pub sort_item: Vec<String>,
    pub sort_album: Vec<String>,
    /// The playlist plugin's `playlist_dir` (for `playlist:` queries).
    pub playlist_dir: PathBuf,
    /// `playlist.relative_to`: None for "playlist" (the playlist's own
    /// directory), else the base paths in a playlist are relative to.
    pub playlist_relative_to: Option<PathBuf>,
    /// Whether the playlist plugin is enabled (its `playlist:` query).
    pub playlist: bool,
}

impl Default for Context {
    fn default() -> Self {
        Context {
            music_dir: None,
            sort_case_insensitive: true,
            sort_item: ["artist+", "album+", "disc+", "track+"]
                .map(String::from)
                .to_vec(),
            sort_album: ["albumartist+", "album+"].map(String::from).to_vec(),
            playlist_dir: PathBuf::from("."),
            playlist_relative_to: None,
            playlist: false,
        }
    }
}

thread_local! {
    static CONTEXT: RefCell<Context> = RefCell::new(Context::default());
}

/// Take the query settings from the configuration (for this thread).
pub fn configure(cfg: &Config) {
    let relative_to = match cfg.str("playlist.relative_to").as_str() {
        "playlist" => None,
        "library" => Some(cfg.directory()),
        other => Some(crate::config::expand_tilde(other)),
    };
    let plugins = cfg.list("plugins");
    let disabled = cfg.list("disabled_plugins");
    set_context(Context {
        music_dir: Some(normpath(&cfg.directory())),
        sort_case_insensitive: cfg.bool("sort_case_insensitive"),
        sort_item: cfg.list("sort_item"),
        sort_album: cfg.list("sort_album"),
        playlist_dir: crate::config::expand_tilde(&cfg.str("playlist.playlist_dir")),
        playlist_relative_to: relative_to,
        playlist: plugins.iter().any(|p| p == "playlist")
            && !disabled.iter().any(|p| p == "playlist"),
    });
}

pub fn set_context(c: Context) {
    CONTEXT.with(|ctx| *ctx.borrow_mut() = c);
}

pub fn context() -> Context {
    CONTEXT.with(|c| c.borrow().clone())
}

// ------------------------------------------------------------------- errors

/// beets' `InvalidQueryError`: "'QUERY': EXPLANATION".
#[derive(Debug)]
pub struct InvalidQueryError(pub String);

impl fmt::Display for InvalidQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidQueryError {}

/// beets' `InvalidQueryArgumentValueError`: "'WHAT' is not EXPECTED[: DETAIL]".
fn bad_arg(what: &str, expected: &str, detail: Option<String>) -> String {
    match detail {
        Some(d) => format!("'{what}' is not {expected}: {d}"),
        None => format!("'{what}' is not {expected}"),
    }
}

// ----------------------------------------------------------- Python values

/// A field value as beets' models hand it to queries.
#[derive(Clone, Debug, PartialEq)]
pub enum PyVal {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    /// A list field's values (beets' `DelimitedString`).
    List(Vec<String>),
}

impl PyVal {
    fn from_value(v: Value) -> PyVal {
        match v {
            Value::Null => PyVal::None,
            Value::Str(s) => PyVal::Str(s),
            Value::Int(i) => PyVal::Int(i),
            Value::Float(f) => PyVal::Float(f),
            Value::Bool(b) => PyVal::Bool(b),
            Value::Path(p) => PyVal::Bytes(p.as_os_str().as_bytes().to_vec()),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            PyVal::Bool(b) => Some(*b as i64 as f64),
            PyVal::Int(i) => Some(*i as f64),
            PyVal::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// beets' `util.as_string`.
    fn as_string(&self) -> String {
        match self {
            PyVal::None => String::new(),
            PyVal::Bool(b) => if *b { "True" } else { "False" }.into(),
            PyVal::Int(i) => i.to_string(),
            PyVal::Float(f) => py_float(*f),
            PyVal::Str(s) => s.clone(),
            PyVal::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            // `str(list)`.
            PyVal::List(v) => format!(
                "[{}]",
                v.iter()
                    .map(|s| format!("'{s}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// `StringFieldQuery.value_match`: a list matches if any value does.
    fn strings(self) -> Vec<String> {
        match self {
            PyVal::List(v) => v,
            v => vec![v.as_string()],
        }
    }

    /// Python's `==`.
    fn py_eq(&self, other: &PyVal) -> bool {
        match (self.number(), other.number()) {
            (Some(a), Some(b)) => a == b,
            _ => self == other,
        }
    }
}

/// Python's `repr(float)`.
fn py_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    let a = f.abs();
    if a != 0.0 && !(1e-4..1e16).contains(&a) {
        // Python: shortest digits, exponent with at least two digits.
        let s = format!("{f:e}");
        let (m, e) = s.split_once('e').unwrap();
        let (sign, digits) = match e.strip_prefix('-') {
            Some(d) => ("-", d),
            None => ("+", e),
        };
        return format!("{m}e{sign}{digits:0>2}");
    }
    if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// Python's `str.lower()`.
fn lower(s: &str) -> String {
    s.to_lowercase()
}

// ------------------------------------------------------------ model shapes

fn table(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "items",
        Kind::Album => "albums",
    }
}

fn other(kind: Kind) -> Kind {
    match kind {
        Kind::Item => Kind::Album,
        Kind::Album => Kind::Item,
    }
}

/// The model's fixed (database) fields, `id` included.
fn has_own(kind: Kind, f: &str) -> bool {
    f == "id" || schema(kind).iter().any(|(n, _)| *n == f)
}

fn own_type(kind: Kind, f: &str) -> Option<FieldType> {
    if f == "id" {
        return Some(FieldType::Int);
    }
    schema(kind).iter().find(|(n, _)| *n == f).map(|(_, t)| *t)
}

/// `all_db_fields`: fixed fields of the model or its relation.
fn in_all_db(kind: Kind, f: &str) -> bool {
    has_own(kind, f) || has_own(other(kind), f)
}

/// `other_db_fields`: fields only the related model has.
fn in_other_db(kind: Kind, f: &str) -> bool {
    has_own(other(kind), f) && !has_own(kind, f)
}

/// Computed fields (`_getters`).
fn is_getter(kind: Kind, f: &str) -> bool {
    let builtin = match kind {
        Kind::Item => matches!(f, "singleton" | "filesize" | "has_cover_art"),
        Kind::Album => matches!(f, "path" | "albumtotal"),
    };
    // Plugins' template fields (rewritten fixed fields among them).
    builtin || {
        let env = crate::formatted::env();
        env.rewriters.iter().any(|r| r.defines(kind, f))
            || env
                .inline
                .as_ref()
                .is_some_and(|i| i.field(kind, f).is_some())
            || (kind == Kind::Album && env.album_template_fields.contains(&f))
    }
}

/// The SQL column for a fixed field, qualified with its table.
fn column(kind: Kind, f: &str) -> String {
    if has_own(kind, f) {
        format!("{}.{f}", table(kind))
    } else if has_own(other(kind), f) {
        format!("{}.{f}", table(other(kind)))
    } else {
        f.to_string()
    }
}

/// `Model._type(field)` is a path type (falling back to the relation).
fn is_path_typed(kind: Kind, f: &str) -> bool {
    let path =
        |k: Kind| own_type(k, f) == Some(FieldType::Path) || (k == Kind::Album && f == "path");
    path(kind) || path(other(kind))
}

/// The default fields an unqualified term searches (`_search_fields`).
fn search_fields(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Item => &[
            "artist",
            "title",
            "comments",
            "album",
            "albumartist",
            "genres",
        ],
        Kind::Album => &["album", "albumartist", "genres"],
    }
}

/// Query classes, as beets' `FieldQueryType`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Substring,
    String,
    Match,
    Regexp,
    Numeric,
    Duration,
    Date,
    Boolean,
    Path,
    Singleton,
}

/// The query class for a field named `key` (as typed: beets doesn't
/// lowercase it for this lookup): its own fixed fields' types, the
/// album's `path`, the item's `singleton`; anything else is a substring.
fn field_class(kind: Kind, key: &str) -> Class {
    if kind == Kind::Item && key == "singleton" {
        return Class::Singleton;
    }
    if kind == Kind::Album && key == "path" {
        return Class::Path;
    }
    // The model's own fields and declared types (not its relation's).
    match own_type(kind, key).or_else(|| crate::model::flex_type(kind, key)) {
        Some(FieldType::Duration) => Class::Duration,
        Some(FieldType::Date) => Class::Date,
        Some(FieldType::Bool) => Class::Boolean,
        Some(FieldType::Path) => Class::Path,
        Some(t) if t.is_numeric() => Class::Numeric,
        _ => Class::Substring,
    }
}

// ------------------------------------------------------------ the objects

/// A record being matched, with what beets' models would reach through it:
/// an item's album (for album fields and flexible attributes), an album's
/// directory (its computed `path`) and the album's computed counts.
#[derive(Clone, Copy)]
pub struct Obj<'a> {
    pub rec: &'a Record,
    pub album: Option<&'a Record>,
    pub album_dir: Option<&'a Path>,
    /// The library, for template fields that reach other records (the
    /// inline plugin's album `items`).
    pub lib: Option<&'a crate::library::Library>,
    /// The album's (an album's own, an item's album's) [`AlbumCounts`].
    pub album_counts: Option<AlbumCounts>,
}

/// What an album's computed fields need: its `albumtotal` and how many
/// items it has (the missing plugin's `$missing` is the difference).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AlbumCounts {
    pub albumtotal: i64,
    pub items: i64,
}

impl<'a> Obj<'a> {
    pub fn new(rec: &'a Record) -> Self {
        Obj {
            rec,
            album: None,
            album_dir: None,
            lib: None,
            album_counts: None,
        }
    }

    /// The inline plugin's field `key`, if it's one (beets' getters come
    /// before the fields).
    fn inline(&self, key: &str) -> Option<PyVal> {
        let env = crate::formatted::env();
        let f = env.inline.as_ref()?.field(self.rec.kind, key)?;
        use crate::inline::value::V;
        match crate::inline::evaluate(f, self.rec, self.album, self.lib, &env) {
            Ok(v) => Some(match v {
                V::None => PyVal::None,
                V::Bool(b) => PyVal::Bool(b),
                V::Int(i) => PyVal::Int(i),
                V::Float(x) => PyVal::Float(x),
                V::Str(s) => PyVal::Str(s.to_string()),
                V::Bytes(b) => PyVal::Bytes(b.to_vec()),
                V::List(l) => PyVal::List(l.borrow().iter().map(V::to_str).collect()),
                other => PyVal::Str(other.to_str()),
            }),
            Err(e) => crate::inline::fail(&crate::inline::inline_error(&f.code, &e)),
        }
    }

    /// beets' `Model.get(key)` (with an item's album fallback); None when
    /// the field doesn't exist.
    pub fn get(&self, key: &str) -> Option<PyVal> {
        if let Some(v) = self.inline(key) {
            return Some(v);
        }
        // rewrite/advancedrewrite's getters for fixed fields.
        let env = crate::formatted::env();
        if let Some(r) = env
            .rewriters
            .iter()
            .rev()
            .find(|r| r.defines(self.rec.kind, key))
            && let Some(v) = r.value(self.rec, self.album, key)
        {
            use crate::plugins::rewrite::Repl;
            return Some(match v {
                Repl::List(l) => PyVal::List(l),
                Repl::Str(s) => PyVal::Str(s),
            });
        }
        let rec = self.rec;
        match rec.kind {
            Kind::Item => match key {
                "singleton" => return Some(PyVal::Bool(rec.int("album_id") == 0)),
                "filesize" => {
                    let n = std::fs::metadata(rec.path()).map(|m| m.len()).unwrap_or(0);
                    return Some(PyVal::Int(n as i64));
                }
                "has_cover_art" => {
                    return Some(PyVal::Bool(
                        crate::media::has_art(rec.path()).unwrap_or(false),
                    ));
                }
                // Singletons have no album (beets stores NULL).
                "album_id" if rec.int("album_id") == 0 => return Some(PyVal::None),
                _ => {}
            },
            Kind::Album => match key {
                "path" => {
                    return Some(PyVal::Bytes(
                        self.album_dir
                            .map(|p| p.as_os_str().as_bytes().to_vec())
                            .unwrap_or_default(),
                    ));
                }
                // beets' album getters, ahead of any attribute of the name.
                "albumtotal" => return self.album_counts.map(|c| PyVal::Int(c.albumtotal)),
                // The missing plugin's template field (a string).
                "missing"
                    if crate::formatted::env()
                        .album_template_fields
                        .contains(&"missing") =>
                {
                    return self
                        .album_counts
                        .map(|c| PyVal::Str((c.albumtotal - c.items).to_string()));
                }
                _ => {}
            },
        }
        if has_own(rec.kind, key) {
            if let Some(FieldType::List(_)) = own_type(rec.kind, key) {
                return Some(PyVal::List(rec.list(key)));
            }
            return rec.get(key).map(PyVal::from_value);
        }
        // A typed attribute reads back as its type (`from_sql`).
        if rec.flex.contains_key(key) {
            return rec.get(key).map(PyVal::from_value);
        }
        // Query keys are lowercased; flexible attributes keep their case.
        let lk = lower(key);
        if let Some((k, _)) = rec.flex.iter().find(|(k, _)| lower(k) == lk) {
            return rec.get(k).map(PyVal::from_value);
        }
        if rec.kind == Kind::Item
            && let Some(a) = self.album
        {
            let mut o = Obj::new(a);
            o.lib = self.lib;
            o.album_counts = self.album_counts;
            return o.get(key);
        }
        None
    }
}

// ---------------------------------------------------------------- queries

#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn f(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }
    fn sql(self) -> SqlValue {
        match self {
            Num::Int(i) => SqlValue::Integer(i),
            Num::Float(f) => SqlValue::Real(f),
        }
    }
}

#[derive(Clone, Debug)]
enum Test {
    /// `SubstringQuery`.
    Substring(String),
    /// `StringQuery` (`=~`): whole string, ignoring case.
    String(String),
    /// `MatchQuery` (`=`, and booleans).
    Match(PyVal),
    /// `RegexpQuery`: the NFC-normalized pattern.
    Regexp(fancy_regex::Regex),
    /// `NumericQuery` / `DurationQuery`.
    Numeric {
        point: Option<Num>,
        min: Option<Num>,
        max: Option<Num>,
    },
    /// `DateQuery`: a closed-open interval of timestamps.
    Date {
        start: Option<f64>,
        end: Option<f64>,
    },
    /// `PathQuery`.
    Path {
        pattern: Vec<u8>,
        dir: Vec<u8>,
        case_sensitive: bool,
    },
    /// `NoneQuery`.
    IsNone,
    /// beets' `InQuery` (the playlist plugin's `PlaylistQuery`): one of
    /// these paths.
    In(Vec<Vec<u8>>),
}

#[derive(Clone, Debug)]
pub struct FieldQuery {
    field: String,
    col: String,
    fast: bool,
    /// A string query on a path field: it sees the path as beets stores
    /// it, relative to the library directory when inside it.
    db_path: bool,
    test: Test,
}

#[derive(Clone, Debug)]
pub enum Pred {
    True,
    #[cfg_attr(not(test), allow(dead_code))]
    False,
    Field(FieldQuery),
    Not(Box<Pred>),
    And(Vec<Pred>),
    Or(Vec<Pred>),
}

/// The SQL functions queries use; registered per connection.
pub fn register_sql_functions(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    // beets' `regexp(value, pattern)`: a search in `str(value)`.
    conn.create_scalar_function("regexp", 2, flags, |ctx| {
        let re = ctx.get_or_create_aux(1, |v| -> Result<fancy_regex::Regex, BoxError> {
            Ok(fancy_regex::Regex::new(v.as_str()?)?)
        })?;
        let value = sql_str(ctx.get_raw(0));
        Ok(re.is_match(&value).unwrap_or(false))
    })?;
    // `bytelower`: ASCII-lowercase a path blob.
    conn.create_scalar_function("bytelower", 1, flags, |ctx| {
        Ok(match ctx.get::<SqlValue>(0)? {
            SqlValue::Blob(b) => SqlValue::Blob(b.to_ascii_lowercase()),
            SqlValue::Text(t) => SqlValue::Text(t.to_lowercase()),
            v => v,
        })
    })?;
    // `dbpath`: a path as beets stores it (relative to the library root).
    conn.create_scalar_function("dbpath", 1, FunctionFlags::SQLITE_UTF8, |ctx| {
        Ok(match ctx.get::<SqlValue>(0)? {
            SqlValue::Blob(b) => SqlValue::Text(String::from_utf8_lossy(&db_path(&b)).into_owned()),
            v => v,
        })
    })?;
    Ok(())
}

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Python's `str(value)` for an SQLite value (as beets' `regexp` sees it).
fn sql_str(v: rusqlite::types::ValueRef) -> String {
    use rusqlite::types::ValueRef as V;
    match v {
        V::Null => "None".into(),
        V::Integer(i) => i.to_string(),
        V::Real(f) => py_float(f),
        V::Text(t) => String::from_utf8_lossy(t).into_owned(),
        V::Blob(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

impl Pred {
    /// The fields the query looks at (`field_names`).
    fn field_names(&self, out: &mut Vec<String>) {
        match self {
            Pred::Field(q) => out.push(q.field.clone()),
            Pred::Not(p) => p.field_names(out),
            Pred::And(ps) | Pred::Or(ps) => ps.iter().for_each(|p| p.field_names(out)),
            Pred::True | Pred::False => {}
        }
    }

    /// Whether running it in SQL needs the related table joined in.
    pub fn needs_join(&self, kind: Kind) -> bool {
        let mut names = vec![];
        self.field_names(&mut names);
        names.iter().any(|f| in_other_db(kind, f))
    }

    /// beets' `clause()`: an SQL expression with its parameters, or None
    /// when part of the query can only be matched in memory.
    pub fn clause(&self) -> Option<(String, Vec<SqlValue>)> {
        match self {
            Pred::True => Some(("1".into(), vec![])),
            Pred::False => Some(("0".into(), vec![])),
            Pred::Field(q) => q.clause(),
            Pred::Not(p) => p.clause().map(|(c, v)| (format!("not ({c})"), v)),
            Pred::And(ps) | Pred::Or(ps) => {
                let joiner = if matches!(self, Pred::And(_)) {
                    " and "
                } else {
                    " or "
                };
                let mut parts = vec![];
                let mut vals = vec![];
                for p in ps {
                    let (c, v) = p.clause()?;
                    parts.push(format!("({c})"));
                    vals.extend(v);
                }
                Some((parts.join(joiner), vals))
            }
        }
    }

    pub fn matches(&self, obj: &Obj) -> bool {
        match self {
            Pred::True => true,
            Pred::False => false,
            Pred::Field(q) => q.matches(obj),
            Pred::Not(p) => !p.matches(obj),
            Pred::And(ps) => ps.iter().all(|p| p.matches(obj)),
            Pred::Or(ps) => ps.iter().any(|p| p.matches(obj)),
        }
    }
}

/// `%`, `_` and `\` escaped for `LIKE ... ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

impl FieldQuery {
    fn clause(&self) -> Option<(String, Vec<SqlValue>)> {
        if !self.fast {
            return None;
        }
        let col = if self.db_path {
            format!("dbpath({})", self.col)
        } else {
            self.col.clone()
        };
        Some(match &self.test {
            Test::Substring(p) => (
                format!("{col} like ? escape '\\'"),
                vec![SqlValue::Text(format!("%{}%", like_escape(p)))],
            ),
            Test::String(p) => (
                format!("{col} like ? escape '\\'"),
                vec![SqlValue::Text(like_escape(p))],
            ),
            Test::Match(v) => (
                format!("{col} = ?"),
                vec![match v {
                    PyVal::Int(i) => SqlValue::Integer(*i),
                    PyVal::Bool(b) => SqlValue::Integer(*b as i64),
                    PyVal::Float(f) => SqlValue::Real(*f),
                    PyVal::Bytes(b) => SqlValue::Blob(b.clone()),
                    PyVal::None => SqlValue::Null,
                    PyVal::Str(s) => SqlValue::Text(s.clone()),
                    PyVal::List(v) => SqlValue::Text(v.join(crate::model::MULTI)),
                }],
            ),
            Test::Regexp(re) => (
                format!("regexp({col}, ?)"),
                vec![SqlValue::Text(re.as_str().to_string())],
            ),
            Test::Numeric { point, min, max } => match (point, min, max) {
                (Some(p), _, _) => (format!("{col}=?"), vec![p.sql()]),
                (None, Some(a), Some(b)) => {
                    (format!("{col} >= ? AND {col} <= ?"), vec![a.sql(), b.sql()])
                }
                (None, Some(a), None) => (format!("{col} >= ?"), vec![a.sql()]),
                (None, None, Some(b)) => (format!("{col} <= ?"), vec![b.sql()]),
                (None, None, None) => ("1".into(), vec![]),
            },
            Test::Date { start, end } => {
                let mut parts = vec![];
                let mut vals = vec![];
                // beets binds `int(datetime.timestamp())`.
                if let Some(s) = start {
                    parts.push(format!("{col} >= ?"));
                    vals.push(SqlValue::Integer(s.trunc() as i64));
                }
                if let Some(e) = end {
                    parts.push(format!("{col} < ?"));
                    vals.push(SqlValue::Integer(e.trunc() as i64));
                }
                if parts.is_empty() {
                    ("1".into(), vec![])
                } else {
                    (parts.join(" AND "), vals)
                }
            }
            Test::Path {
                pattern,
                dir,
                case_sensitive,
            } => {
                let (l, r) = if *case_sensitive {
                    (col.clone(), "?".to_string())
                } else {
                    (format!("bytelower({col})"), "bytelower(?)".to_string())
                };
                (
                    format!("({l} = {r}) OR (substr({l}, 1, ?) = {r})"),
                    vec![
                        SqlValue::Blob(pattern.clone()),
                        SqlValue::Integer(dir.len() as i64),
                        SqlValue::Blob(dir.clone()),
                    ],
                )
            }
            Test::In(_) => return None,
            Test::IsNone => {
                // A singleton's album_id is NULL (an album_id of 0, which
                // beets-rs used to store, links nothing either).
                if self.field == "album_id" {
                    (format!("({col} IS NULL OR {col} = 0)"), vec![])
                } else {
                    (format!("{col} IS NULL"), vec![])
                }
            }
        })
    }

    fn value(&self, obj: &Obj) -> Option<PyVal> {
        let v = obj.get(&self.field)?;
        Some(match v {
            PyVal::Bytes(b) if self.db_path => {
                PyVal::Str(String::from_utf8_lossy(&db_path(&b)).into_owned())
            }
            v => v,
        })
    }

    fn matches(&self, obj: &Obj) -> bool {
        let value = self.value(obj);
        match &self.test {
            Test::Substring(p) => value
                .unwrap_or(PyVal::None)
                .strings()
                .iter()
                .any(|s| lower(s).contains(&lower(p))),
            Test::String(p) => value
                .unwrap_or(PyVal::None)
                .strings()
                .iter()
                .any(|s| lower(s) == lower(p)),
            // `MatchQuery.value_match`: `pattern in value` for a list.
            Test::Match(p) => match value.unwrap_or(PyVal::None) {
                PyVal::List(v) => v.iter().any(|s| p.py_eq(&PyVal::Str(s.clone()))),
                v => p.py_eq(&v),
            },
            Test::In(paths) => match value {
                Some(PyVal::Bytes(b)) => {
                    let p = crate::library::lexical_normpath(&PathBuf::from(
                        std::ffi::OsStr::from_bytes(&b),
                    ));
                    let p = p.as_os_str().as_bytes();
                    paths.iter().any(|q| q.as_slice() == p)
                }
                _ => false,
            },
            Test::Regexp(re) => value.unwrap_or(PyVal::None).strings().iter().any(|s| {
                let s: String = s.nfc().collect();
                re.is_match(&s).unwrap_or(false)
            }),
            Test::Numeric { point, min, max } => {
                let Some(v) = value else { return false };
                let v = match v {
                    PyVal::Str(s) => match parse_number(&s) {
                        Ok(Some(n)) => n.f(),
                        _ => return false,
                    },
                    v => match v.number() {
                        Some(n) => n,
                        None => return false,
                    },
                };
                if let Some(p) = point {
                    return v == p.f();
                }
                !(min.is_some_and(|m| v < m.f()) || max.is_some_and(|m| v > m.f()))
            }
            Test::Date { start, end } => {
                let ts = match value {
                    Some(PyVal::Str(s)) => match s.trim().parse::<f64>() {
                        Ok(f) => f,
                        Err(_) => return false,
                    },
                    Some(v) => match v.number() {
                        Some(f) => f,
                        None => return false,
                    },
                    None => return false,
                };
                !(start.is_some_and(|s| ts < s) || end.is_some_and(|e| ts >= e))
            }
            Test::Path {
                pattern,
                dir,
                case_sensitive,
            } => {
                let mut path = match value {
                    Some(PyVal::Bytes(b)) => db_path(&b),
                    Some(v) => db_path(v.as_string().as_bytes()),
                    None => return false,
                };
                if !case_sensitive {
                    path.make_ascii_lowercase();
                }
                path == *pattern || path.starts_with(dir)
            }
            Test::IsNone => matches!(value, None | Some(PyVal::None)),
        }
    }
}

// ------------------------------------------------------------ path helpers

/// Python's `os.path.normpath(os.path.abspath(os.path.expanduser(p)))`.
pub fn normpath(p: &Path) -> PathBuf {
    let p = if p.as_os_str().as_bytes().starts_with(b"~") {
        crate::config::expand_tilde(&p.to_string_lossy())
    } else {
        p.to_path_buf()
    };
    let p = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    crate::library::lexical_normpath(&p)
}

/// beets' `normalize_path_for_db` with the configured library directory:
/// a path inside it relative to it, others unchanged.
fn db_path(path: &[u8]) -> Vec<u8> {
    crate::library::to_db_path(path, context().music_dir.as_deref())
}

/// beets' `util.case_sensitive`: whether the filesystem holding `path` is
/// case sensitive, judged from the nearest existing ancestor.
pub fn case_sensitive(path: &Path) -> bool {
    let mut path = path.to_path_buf();
    loop {
        let (Some(head), Some(tail)) = (path.parent(), path.file_name()) else {
            return true;
        };
        let head = head.to_path_buf();
        if !path.exists() {
            path = head;
            continue;
        }
        let tail = tail.to_string_lossy();
        let (upper, low) = (tail.to_uppercase(), tail.to_lowercase());
        if upper == low {
            path = head;
            continue;
        }
        let (u, l) = (head.join(&upper), head.join(&low));
        let (Ok(um), Ok(lm)) = (std::fs::metadata(&u), std::fs::metadata(&l)) else {
            return true;
        };
        use std::os::unix::fs::MetadataExt;
        return (um.dev(), um.ino()) != (lm.dev(), lm.ino());
    }
}

/// beets' `PathQuery.is_path_query`: the part before any colon contains a
/// `/` and exists.
pub fn is_path_query(part: &str) -> bool {
    let p = part.split(':').next().unwrap_or("");
    p.contains('/') && normpath(Path::new(p)).exists()
}

fn path_test(pattern: &str) -> Test {
    let ctx = context();
    let mut p = PathBuf::from(pattern);
    if !pattern.starts_with('/')
        && let Some(md) = &ctx.music_dir
    {
        p = md.join(pattern);
    }
    let path = normpath(&p);
    let case_sensitive = case_sensitive(&path);
    // Path queries compare against the stored form, relative to the
    // library directory when inside it.
    let mut pattern = db_path_of(&path);
    if !case_sensitive {
        pattern.make_ascii_lowercase();
    }
    let mut dir = pattern.clone();
    if !dir.ends_with(b"/") {
        dir.push(b'/');
    }
    Test::Path {
        pattern,
        dir,
        case_sensitive,
    }
}

// --------------------------------------------------------- numbers, dates

/// Python's `int(s)`, else `float(s)`; None for "".
fn parse_number(s: &str) -> Result<Option<Num>, ()> {
    if s.is_empty() {
        return Ok(None);
    }
    let t = s.trim();
    let digits_ok = |d: &str| {
        !d.is_empty()
            && !d.starts_with('_')
            && !d.ends_with('_')
            && !d.contains("__")
            && d.chars().all(|c| c.is_ascii_digit() || c == '_')
    };
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits_ok(unsigned)
        && let Ok(i) = t.replace('_', "").parse::<i64>()
    {
        return Ok(Some(Num::Int(i)));
    }
    let lower_t = t.to_ascii_lowercase();
    let word = lower_t.trim_start_matches(['+', '-']);
    if (matches!(word, "inf" | "infinity" | "nan") || t.chars().any(|c| c.is_ascii_digit()))
        && let Ok(f) = t.parse::<f64>()
    {
        return Ok(Some(Num::Float(f)));
    }
    Err(())
}

fn numeric_test(pattern: &str, duration: bool) -> Result<Test, String> {
    let convert = |s: &str| -> Result<Option<Num>, String> {
        if duration {
            if s.is_empty() {
                return Ok(None);
            }
            static MSS: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r"^(\d+):([0-5]\d)$").unwrap());
            if let Some(c) = MSS.captures(s) {
                let m: f64 = c[1].parse().unwrap_or(0.0);
                let ss: f64 = c[2].parse().unwrap_or(0.0);
                return Ok(Some(Num::Float(m * 60.0 + ss)));
            }
            match parse_number(s) {
                Ok(n) => Ok(n.map(|n| Num::Float(n.f()))),
                Err(()) => Err(bad_arg(s, "a M:SS string or a float", None)),
            }
        } else {
            parse_number(s).map_err(|()| bad_arg(s, "an int or a float", None))
        }
    };
    Ok(match pattern.split_once("..") {
        None => Test::Numeric {
            point: convert(pattern)?,
            min: None,
            max: None,
        },
        Some((a, b)) => Test::Numeric {
            point: None,
            min: convert(a)?,
            max: convert(b)?,
        },
    })
}

/// beets' `Period`: a date with a precision.
#[derive(Clone, Copy, Debug)]
struct Period {
    date: NaiveDateTime,
    /// 0 year, 1 month, 2 day, 3 hour, 4 minute, 5 second.
    precision: usize,
}

/// `time.strptime`'s regexes for beets' date formats, in precision order
/// (each with a "T" and a whitespace variant once a time is involved).
static DATE_FORMATS: LazyLock<Vec<(usize, Regex)>> = LazyLock::new(|| {
    let y = r"(\d\d\d\d)";
    let m = r"(1[0-2]|0[1-9]|[1-9])";
    let d = r"(3[01]|[12]\d|0[1-9]|[1-9]| [1-9])";
    let h = r"(2[0-3]|[0-1]\d|\d)";
    let mi = r"([0-5]\d|\d)";
    let s = r"(6[0-1]|[0-5]\d|\d)";
    let date = format!("{y}-{m}-{d}");
    let mut out = vec![
        (0, y.to_string()),
        (1, format!("{y}-{m}")),
        (2, date.clone()),
    ];
    for sep in ["T", r"\s+"] {
        out.push((3, format!("{date}{sep}{h}")));
        out.push((4, format!("{date}{sep}{h}:{mi}")));
        out.push((5, format!("{date}{sep}{h}:{mi}:{s}")));
    }
    out.sort_by_key(|(p, _)| *p);
    out.into_iter()
        .map(|(p, r)| (p, Regex::new(&format!("(?i)^{r}")).unwrap()))
        .collect()
});

static RELATIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([+-]?)([0-9]+)([ymwd])").unwrap());

impl Period {
    fn parse(s: &str) -> Result<Option<Period>, String> {
        if s.is_empty() {
            return Ok(None);
        }
        if let Some(c) = RELATIVE.captures(s) {
            let days: i64 = match &c[3] {
                "y" => 365,
                "m" => 30,
                "w" => 7,
                _ => 1,
            };
            let qty: i64 = c[2]
                .parse()
                .map_err(|_| bad_arg(s, "a valid date/time string", None))?;
            let delta = Duration::try_days(qty.saturating_mul(days))
                .ok_or_else(|| bad_arg(s, "a valid date/time string", None))?;
            let now = Local::now().naive_local();
            let date = if &c[1] == "-" {
                now.checked_sub_signed(delta)
            } else {
                now.checked_add_signed(delta)
            }
            .ok_or_else(|| bad_arg(s, "a valid date/time string", None))?;
            return Ok(Some(Period { date, precision: 5 }));
        }
        for (precision, re) in DATE_FORMATS.iter() {
            let Some(c) = re.captures(s) else { continue };
            // `strptime` wants the whole string used.
            if c.get(0).unwrap().end() != s.len() {
                continue;
            }
            let n = |i: usize| -> u32 {
                c.get(i)
                    .map_or(0, |m| m.as_str().trim().parse().unwrap_or(0))
            };
            let year = n(1) as i32;
            let date = NaiveDate::from_ymd_opt(year, n(2).max(1), n(3).max(1))
                .and_then(|d| d.and_hms_opt(n(4), n(5), n(6)));
            if let Some(date) = date {
                return Ok(Some(Period {
                    date,
                    precision: *precision,
                }));
            }
        }
        Err(bad_arg(s, "a valid date/time string", None))
    }

    /// The end of the period, for a right-open interval.
    fn open_right_endpoint(&self) -> Option<NaiveDateTime> {
        use chrono::Datelike;
        let d = self.date;
        match self.precision {
            0 => d.with_month(1)?.with_year(d.year() + 1),
            1 => {
                if d.month() < 12 {
                    d.with_month(d.month() + 1)
                } else {
                    d.with_month(1)?.with_year(d.year() + 1)
                }
            }
            2 => d.checked_add_signed(Duration::days(1)),
            3 => d.checked_add_signed(Duration::hours(1)),
            4 => d.checked_add_signed(Duration::minutes(1)),
            _ => d.checked_add_signed(Duration::seconds(1)),
        }
    }
}

/// A local naive datetime as a Unix timestamp (`datetime.timestamp()`).
fn timestamp(d: NaiveDateTime) -> f64 {
    let local = Local.from_local_datetime(&d).earliest().or_else(|| {
        Local
            .from_local_datetime(&(d + Duration::hours(1)))
            .earliest()
    });
    match local {
        Some(l) => l.timestamp() as f64 + l.timestamp_subsec_micros() as f64 / 1e6,
        None => d.and_utc().timestamp() as f64,
    }
}

/// beets' `DateInterval.from_periods` over `_parse_periods`.
fn date_test(pattern: &str) -> Result<Test, String> {
    let (start, end) = match pattern.split_once("..") {
        None => {
            let p = Period::parse(pattern)?;
            (p, p)
        }
        Some((a, b)) => (Period::parse(a)?, Period::parse(b)?),
    };
    let overflow = || bad_arg(pattern, "a valid date/time string", None);
    let (start, end) = match (start, end) {
        (Some(s), Some(e)) if s.date >= e.open_right_endpoint().ok_or_else(overflow)? => {
            (Some(e), Some(s))
        }
        x => x,
    };
    Ok(Test::Date {
        start: start.map(|s| timestamp(s.date)),
        end: match end {
            Some(e) => Some(timestamp(e.open_right_endpoint().ok_or_else(overflow)?)),
            None => None,
        },
    })
}

// ----------------------------------------------------------------- parsing

/// beets' `parse_query_part`: (field, term, negated). The field is what
/// comes before the first unescaped colon, if that's all non-space;
/// `\:` in the term stands for a colon.
pub fn parse_query_part(part: &str) -> (Option<String>, String, bool) {
    let part = part.trim();
    let (negate, rest) = match part.strip_prefix(['-', '^']) {
        Some(r) => (true, r),
        None => (false, part),
    };
    let mut key = None;
    let mut prev: Option<char> = None;
    for (i, c) in rest.char_indices() {
        if c.is_whitespace() {
            break;
        }
        if c == ':' && i > 0 && prev != Some('\\') {
            key = Some(i);
            break;
        }
        prev = Some(c);
    }
    match key {
        Some(i) => (
            Some(rest[..i].to_string()),
            rest[i + 1..].replace("\\:", ":"),
            negate,
        ),
        None => (None, rest.replace("\\:", ":"), negate),
    }
}

/// The query prefixes beets registers, in order.
const PREFIXES: [(&str, Class); 3] = [
    (":", Class::Regexp),
    ("=~", Class::String),
    ("=", Class::Match),
];

/// beets' `construct_query_part`.
fn construct_query_part(kind: Kind, part: &str) -> Result<Pred, String> {
    if part.is_empty() {
        return Ok(Pred::True);
    }
    let (key, term, negate) = parse_query_part(part);
    let (class, pattern) = match PREFIXES.iter().find(|(p, _)| term.starts_with(p)) {
        Some((p, c)) => (*c, term[p.len()..].to_string()),
        None => (
            key.as_deref()
                .map_or(Class::Substring, |k| field_class(kind, k)),
            term,
        ),
    };
    let q = match key {
        None => Pred::Or(
            search_fields(kind)
                .iter()
                .map(|f| field_query(kind, f, &pattern, class))
                .collect::<Result<_, _>>()?,
        ),
        Some(k) => {
            let field = crate::model::replace_legacy_field(kind, &lower(&k), false);
            field_query(kind, &field, &pattern, class)?
        }
    };
    Ok(if negate { Pred::Not(Box::new(q)) } else { q })
}

/// beets' `LibModel.field_query`.
fn field_query(kind: Kind, field: &str, pattern: &str, class: Class) -> Result<Pred, String> {
    // beets' plugin `item_queries`: the playlist plugin's `playlist:`.
    if kind == Kind::Item && field == "playlist" && context().playlist {
        let paths = crate::plugins::playlist::query_paths(pattern, &context());
        return Ok(Pred::Field(FieldQuery {
            field: "path".into(),
            col: column(kind, "path"),
            fast: false,
            db_path: false,
            test: Test::In(paths),
        }));
    }
    let mut pattern = pattern.to_string();
    let db_path = is_path_typed(kind, field) && class != Class::Path;
    if db_path {
        // String queries on a path see the stored, library-relative form.
        let p = crate::config::expand_tilde(&pattern);
        if p.is_absolute() {
            pattern = String::from_utf8_lossy(&db_path_of(&p)).into_owned();
        }
    }
    let test = match class {
        Class::Substring => Test::Substring(pattern),
        Class::String => Test::String(pattern),
        Class::Match => Test::Match(PyVal::Str(pattern)),
        Class::Regexp => {
            let p: String = pattern.nfc().collect();
            match fancy_regex::Regex::new(&p) {
                Ok(re) => Test::Regexp(re),
                Err(e) => return Err(bad_arg(&p, "a regular expression", Some(e.to_string()))),
            }
        }
        Class::Numeric => numeric_test(&pattern, false)?,
        Class::Duration => numeric_test(&pattern, true)?,
        Class::Date => date_test(&pattern)?,
        Class::Boolean => Test::Match(PyVal::Int(str2bool(&pattern) as i64)),
        Class::Path => path_test(&pattern),
        Class::Singleton => {
            let q = Pred::Field(FieldQuery {
                field: "album_id".into(),
                col: column(kind, "album_id"),
                fast: true,
                db_path: false,
                test: Test::IsNone,
            });
            return Ok(if str2bool(&pattern) {
                q
            } else {
                Pred::Not(Box::new(q))
            });
        }
    };
    Ok(Pred::Field(FieldQuery {
        field: field.to_string(),
        col: column(kind, field),
        fast: in_all_db(kind, field),
        db_path,
        test,
    }))
}

fn db_path_of(p: &Path) -> Vec<u8> {
    db_path(p.as_os_str().as_bytes())
}

/// beets' `util.str2bool`.
pub fn str2bool(s: &str) -> bool {
    matches!(lower(s).as_str(), "yes" | "1" | "true" | "t" | "y")
}

// -------------------------------------------------------------------- sorts

#[derive(Clone, Debug, PartialEq)]
enum SortKind {
    /// `SmartArtistSort`: `<field>_sort`, falling back to `<field>`.
    Smart,
    /// `FixedFieldSort`: in SQL.
    Fixed,
    /// `SlowFieldSort`: flexible or computed fields.
    Slow,
}

#[derive(Clone, Debug)]
pub struct FieldSort {
    pub field: String,
    pub ascending: bool,
    case_insensitive: bool,
    kind: SortKind,
}

/// beets' `Sort`: no keys is `NullSort`.
#[derive(Clone, Debug, Default)]
pub struct Sort(pub Vec<FieldSort>);

/// beets' `LibModel.field_sort`.
fn field_sort(kind: Kind, field: &str, ascending: bool, case_insensitive: bool) -> FieldSort {
    let (field, k) = match (kind, field) {
        (Kind::Item, "artist") => ("artist", SortKind::Smart),
        (Kind::Album, "artist" | "albumartist") => ("albumartist", SortKind::Smart),
        (_, f) if in_all_db(kind, f) && !is_getter(kind, f) => (f, SortKind::Fixed),
        (_, f) => (f, SortKind::Slow),
    };
    FieldSort {
        field: field.to_string(),
        ascending,
        case_insensitive,
        kind: k,
    }
}

/// beets' `sort_from_strings`: each part is a field and `+` or `-`.
pub fn sort_from_strings(kind: Kind, parts: &[String], case_insensitive: bool) -> Sort {
    Sort(
        parts
            .iter()
            .filter(|p| p.len() > 1)
            .map(|p| {
                let (f, dir) = p.split_at(p.len() - 1);
                field_sort(kind, f, dir == "+", case_insensitive)
            })
            .collect(),
    )
}

impl FieldSort {
    fn order_clause(&self, kind: Kind) -> Option<String> {
        let order = if self.ascending { "ASC" } else { "DESC" };
        let col = column(kind, &self.field);
        match self.kind {
            SortKind::Slow => None,
            SortKind::Smart => {
                let collate = if self.case_insensitive {
                    "COLLATE NOCASE"
                } else {
                    ""
                };
                Some(format!(
                    "COALESCE(NULLIF({col}_sort, ''), {col}) {collate} {order}"
                ))
            }
            SortKind::Fixed => Some(if self.case_insensitive {
                format!(
                    "(CASE WHEN TYPEOF({col})='text' THEN LOWER({col}) \
                     WHEN TYPEOF({col})='blob' THEN LOWER({col}) ELSE {col} END) {order}"
                )
            } else {
                format!("{col} {order}")
            }),
        }
    }

    /// Python's sort key for the slow path.
    fn key(&self, obj: &Obj) -> PyVal {
        let v = match self.kind {
            SortKind::Smart => match obj.get(&format!("{}_sort", self.field)) {
                Some(PyVal::Str(s)) if !s.is_empty() => PyVal::Str(s),
                _ => obj.get(&self.field).unwrap_or(PyVal::Str(String::new())),
            },
            _ => match obj.get(&self.field) {
                None | Some(PyVal::None) => {
                    if obj.rec.kind == Kind::Album && self.field == "path" {
                        PyVal::Bytes(vec![])
                    } else if let Some(ty) = crate::model::flex_type(obj.rec.kind, &self.field) {
                        // A typed attribute's null.
                        PyVal::from_value(ty.default_value())
                    } else {
                        PyVal::Str(String::new())
                    }
                }
                Some(v) => v,
            },
        };
        match v {
            PyVal::Str(s) if self.case_insensitive => PyVal::Str(lower(&s)),
            v => v,
        }
    }
}

/// Python-ish ordering of sort keys (numbers, then strings, then bytes,
/// where Python would refuse to compare mixed types).
fn py_cmp(a: &PyVal, b: &PyVal) -> Ordering {
    let rank = |v: &PyVal| match v {
        PyVal::None => 0,
        PyVal::Bool(_) | PyVal::Int(_) | PyVal::Float(_) => 1,
        PyVal::Str(_) => 2,
        PyVal::Bytes(_) => 3,
        PyVal::List(_) => 4,
    };
    match (a, b) {
        (PyVal::Str(x), PyVal::Str(y)) => x.cmp(y),
        (PyVal::Bytes(x), PyVal::Bytes(y)) => x.cmp(y),
        _ => match (a.number(), b.number()) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => rank(a).cmp(&rank(b)),
        },
    }
}

impl Sort {
    pub fn is_null(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_slow(&self) -> bool {
        self.0.iter().any(|s| s.kind == SortKind::Slow)
    }

    /// beets' `MultipleSort.order_clause`: the trailing run of SQL sorts.
    pub fn order_clause(&self, kind: Kind) -> Option<String> {
        let mut parts = vec![];
        for s in self.0.iter().rev() {
            match s.order_clause(kind) {
                Some(c) => parts.push(c),
                None => break,
            }
        }
        parts.reverse();
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// Whether the SQL sorts need the related table.
    pub fn needs_join(&self, kind: Kind) -> bool {
        self.0
            .iter()
            .any(|s| s.kind == SortKind::Fixed && in_other_db(kind, &s.field))
    }

    /// The sorts SQL couldn't do (everything up to the last slow one),
    /// applied as beets does: stable sorts, least significant first.
    /// Returns the new order as indices into `objs`.
    pub fn sort_slow(&self, objs: &[Obj]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..objs.len()).collect();
        let Some(last_slow) = self.0.iter().rposition(|s| s.kind == SortKind::Slow) else {
            return order;
        };
        for s in self.0[..=last_slow].iter().rev() {
            let keys: Vec<PyVal> = objs.iter().map(|o| s.key(o)).collect();
            order.sort_by(|&a, &b| {
                if s.ascending {
                    py_cmp(&keys[a], &keys[b])
                } else {
                    py_cmp(&keys[b], &keys[a])
                }
            });
        }
        order
    }
}

// --------------------------------------------------------------- the query

/// A parsed query: what to match and how to sort.
#[derive(Clone, Debug)]
pub struct Query {
    pub kind: Kind,
    pub pred: Pred,
    /// The sort the query asked for; empty means the default (`sort_item`
    /// or `sort_album`).
    pub sort: Sort,
}

impl Query {
    /// beets' `parse_query_parts`: implicit path queries, `,` for OR, sort
    /// parts, the rest query parts.
    pub fn parse<S: AsRef<str>>(kind: Kind, args: &[S]) -> anyhow::Result<Self> {
        let parts: Vec<String> = args
            .iter()
            .map(|s| {
                let s = s.as_ref();
                if is_path_query(s) {
                    format!("path:{s}")
                } else {
                    s.to_string()
                }
            })
            .collect();
        let ci = context().sort_case_insensitive;
        Self::parse_sorted(kind, &parts, ci).map_err(|e| {
            let q: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
            anyhow::Error::new(InvalidQueryError(format!("'{}': {e}", q.join(" "))))
        })
    }

    /// beets' `parse_sorted_query`.
    fn parse_sorted(kind: Kind, parts: &[String], case_insensitive: bool) -> Result<Self, String> {
        let mut ors = vec![];
        let mut sorts = vec![];
        let mut sub: Vec<String> = vec![];
        let comma = ",".to_string();
        for part in parts.iter().chain(std::iter::once(&comma)) {
            if let Some(last) = part.strip_suffix(',') {
                if !last.is_empty() {
                    sub.push(last.to_string());
                }
                let mut and = sub
                    .drain(..)
                    .map(|p| construct_query_part(kind, &p))
                    .collect::<Result<Vec<_>, _>>()?;
                if and.is_empty() {
                    and.push(Pred::True);
                }
                ors.push(Pred::And(and));
            } else if (part.ends_with('+') || part.ends_with('-'))
                && !part.contains(':')
                && part.chars().count() > 1
            {
                sorts.push(part.clone());
            } else {
                sub.push(part.clone());
            }
        }
        let pred = if ors.len() > 1 {
            Pred::Or(ors)
        } else {
            ors.pop().unwrap()
        };
        Ok(Query {
            kind,
            pred,
            sort: sort_from_strings(kind, &sorts, case_insensitive),
        })
    }

    /// beets' `PathQuery(field, pattern)` on its own (no query parsing).
    pub fn path(kind: Kind, field: &str, pattern: &str) -> Self {
        Query {
            kind,
            pred: Pred::Field(FieldQuery {
                field: field.to_string(),
                col: column(kind, field),
                fast: true,
                db_path: false,
                test: path_test(pattern),
            }),
            sort: Sort::default(),
        }
    }

    /// The sort to use: the query's own, else the configured default (which
    /// beets always sorts case-insensitively).
    pub fn effective_sort(&self) -> Sort {
        if !self.sort.is_null() {
            return self.sort.clone();
        }
        let ctx = context();
        let parts = match self.kind {
            Kind::Item => ctx.sort_item,
            Kind::Album => ctx.sort_album,
        };
        sort_from_strings(self.kind, &parts, true)
    }

    /// Whether matching or sorting reads any of `fields`.
    pub fn uses_any(&self, fields: &[&str]) -> bool {
        fn walk(p: &Pred, fields: &[&str]) -> bool {
            match p {
                Pred::Field(f) => fields.contains(&f.field.as_str()),
                Pred::Not(p) => walk(p, fields),
                Pred::And(ps) | Pred::Or(ps) => ps.iter().any(|p| walk(p, fields)),
                Pred::True | Pred::False => false,
            }
        }
        walk(&self.pred, fields)
            || self
                .effective_sort()
                .0
                .iter()
                .any(|s| fields.contains(&s.field.as_str()))
    }

    pub fn matches_obj(&self, obj: &Obj) -> bool {
        self.pred.matches(obj)
    }
}

#[cfg(test)]
mod tests;
