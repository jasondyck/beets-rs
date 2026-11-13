//! The `inline` plugin (beets 2.14's `beetsplug/inline.py`): template
//! fields defined in the config (`item_fields`, `album_fields`, and the
//! legacy `pathfields`) by Python expressions or function bodies.
//!
//! beets-rs doesn't embed Python: it evaluates a subset of it (see
//! `parser`/`eval`), the expressions and simple function bodies inline
//! definitions are made of. Python outside the subset fails when the field
//! is evaluated, as a runtime error does in beets (`InlineError`), with
//! the construct named.

pub mod eval;
pub mod lexer;
pub mod parser;
pub mod value;

use crate::config::Config;
use crate::formatted::{FormatEnv, Formatted};
use crate::library::Library;
use crate::model::{Album, FieldType, Kind, Record, Value};
use crate::plugins::PluginLog;
use parser::{Code, ParseError};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use value::{Obj, PyErr, PyResult, V, err};

/// One defined field.
#[derive(Debug)]
pub struct Field {
    pub name: String,
    pub code: String,
    parsed: Result<Code, String>,
}

/// The compiled definitions.
#[derive(Debug, Default)]
pub struct Inline {
    pub item_fields: Vec<Field>,
    pub album_fields: Vec<Field>,
}

impl Inline {
    /// The fields from the config; definitions that aren't Python are
    /// logged (`log`) and left out, as beets does.
    pub fn new(cfg: &Config, log: bool) -> Self {
        let logger = PluginLog::new("inline");
        let compile = |name: &str, code: &str| -> Option<Field> {
            if log {
                logger.debug(&format!("adding item field {name}"));
            }
            match parser::parse(code) {
                Ok(c) => Some(Field {
                    name: name.to_string(),
                    code: code.to_string(),
                    parsed: Ok(c),
                }),
                Err(ParseError::Unsupported(what)) => Some(Field {
                    name: name.to_string(),
                    code: code.to_string(),
                    parsed: Err(what),
                }),
                Err(ParseError::Syntax(msg)) => {
                    if log {
                        logger.error(&format!("syntax error in inline field definition:\n{msg}"));
                    }
                    None
                }
            }
        };
        let mut out = Inline::default();
        let add = |fields: &mut Vec<Field>, f: Field| {
            fields.retain(|g| g.name != f.name);
            fields.push(f);
        };
        for key in ["item_fields", "pathfields"] {
            for (name, code) in cfg.mapping(key) {
                if let Some(f) = compile(&name, &code) {
                    add(&mut out.item_fields, f);
                }
            }
        }
        for (name, code) in cfg.mapping("album_fields") {
            if let Some(f) = compile(&name, &code) {
                add(&mut out.album_fields, f);
            }
        }
        out
    }

    pub fn field(&self, kind: Kind, name: &str) -> Option<&Field> {
        let fields = match kind {
            Kind::Item => &self.item_fields,
            Kind::Album => &self.album_fields,
        };
        fields.iter().find(|f| f.name == name)
    }
}

/// beets' `InlineError` message.
pub fn inline_error(code: &str, e: &PyErr) -> String {
    format!(
        "error in inline path field code:\n{code}\n{}: {}",
        e.kind, e.msg
    )
}

/// A runtime error in an inline field ends the command, as beets'
/// uncaught `InlineError` does.
pub fn fail(msg: &str) -> ! {
    #[cfg(test)]
    panic!("{msg}");
    #[cfg(not(test))]
    {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        eprintln!("{msg}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- values

/// The Python value beets has for a field (`obj._get(key)`), as it comes
/// from the database: paths are bytes, lists lists, booleans integers.
pub fn py_value<'a>(rec: &Record, key: &str) -> V<'a> {
    if key == "id" {
        return rec.id.map_or(V::None, V::Int);
    }
    let Some(ty) = rec.field_type(key) else {
        return match rec.flex.get(key) {
            Some(s) => V::str(s.as_str()),
            None => V::None,
        };
    };
    let v = rec.get(key).unwrap_or(Value::Null);
    match (ty, &v) {
        (_, Value::Null) => V::None,
        (_, Value::Int(0)) if key == "album_id" && rec.kind == Kind::Item => V::None,
        (FieldType::List(_), _) => V::list(rec.list(key).into_iter().map(V::str).collect()),
        (FieldType::Path, v) => {
            let s = v.raw();
            if s.is_empty() && rec.kind == Kind::Album {
                V::None
            } else {
                V::Bytes(Rc::new(s.into_bytes()))
            }
        }
        (FieldType::Bool, v) => V::Int(i64::from(v.as_f64().unwrap_or(0.0) != 0.0)),
        (FieldType::Int | FieldType::Padded(_) | FieldType::Bitrate | FieldType::SampleRate, v) => {
            V::Int(v.as_f64().unwrap_or(0.0) as i64)
        }
        (FieldType::Float(_) | FieldType::Duration | FieldType::Date | FieldType::NullFloat, v) => {
            V::Float(v.as_f64().unwrap_or(0.0))
        }
        (_, v) => V::str(v.raw()),
    }
}

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// A library object as Python code sees it (`db_obj`, `items`).
struct RecObj<'a> {
    rec: Record,
    /// An item's album.
    album: Option<Record>,
    lib: Option<&'a Library>,
    env: Rc<FormatEnv>,
}

impl<'a> RecObj<'a> {
    fn new(rec: Record, lib: Option<&'a Library>, env: Rc<FormatEnv>) -> Self {
        let album = match (rec.kind, lib) {
            (Kind::Item, Some(lib)) if rec.int("album_id") != 0 => {
                lib.get_album(rec.int("album_id")).ok().flatten()
            }
            _ => None,
        };
        RecObj {
            rec,
            album,
            lib,
            env,
        }
    }

    fn items(&self) -> V<'a> {
        let items = match (self.lib, self.rec.id) {
            (Some(lib), Some(id)) => lib.album_items(id).unwrap_or_default(),
            _ => vec![],
        };
        V::list(
            items
                .into_iter()
                .map(|i| eval::obj(RecObj::new(i, self.lib, self.env.clone())))
                .collect(),
        )
    }

    /// `obj[key]`: template fields first, then computed, own and (for
    /// an item) album fields; None if there's no such field.
    fn get(&self, key: &str) -> PyResult<Option<V<'a>>> {
        if let Some(inline) = &self.env.inline
            && let Some(f) = inline.field(self.rec.kind, key)
        {
            return evaluate(f, &self.rec, self.album.as_ref(), self.lib, &self.env).map(Some);
        }
        let f = Formatted::with_env(&self.rec, self.lib, self.env.clone());
        match (self.rec.kind, key) {
            (Kind::Item, "singleton") => return Ok(Some(V::Bool(self.rec.int("album_id") == 0))),
            (Kind::Item, "filesize") => {
                return Ok(Some(V::Int(
                    std::fs::metadata(self.rec.path())
                        .map(|m| m.len() as i64)
                        .unwrap_or(0),
                )));
            }
            (Kind::Album, "albumtotal" | "missing") if f.get(key).is_some() => {
                return Ok(Some(V::Int(f.get(key).unwrap().parse().unwrap_or(0))));
            }
            (Kind::Album, "path") => {
                return Ok(Some(V::Bytes(Rc::new(
                    f.get(key).unwrap_or_default().into_bytes(),
                ))));
            }
            _ => {}
        }
        if self.rec.has_field(key) || self.rec.field_type(key).is_some() {
            return Ok(Some(py_value(&self.rec, key)));
        }
        if let Some(a) = &self.album
            && (a.has_field(key) || a.field_type(key).is_some())
        {
            return Ok(Some(py_value(a, key)));
        }
        // Other plugins' template fields, as text.
        if self.env.album_template_fields.contains(&key) {
            return Ok(f.get(key).map(V::str));
        }
        Ok(None)
    }
}

impl<'a> Obj<'a> for RecObj<'a> {
    fn type_name(&self) -> &'static str {
        match self.rec.kind {
            Kind::Item => "Item",
            Kind::Album => "Album",
        }
    }

    fn getattr(&self, name: &str) -> PyResult<V<'a>> {
        if name.starts_with('_') {
            return err(
                "AttributeError",
                format!("model has no attribute {}", crate::hooks::py_repr(name)),
            );
        }
        match self.get(name)? {
            Some(v) => Ok(v),
            None => err(
                "AttributeError",
                format!("no such field {}", crate::hooks::py_repr(name)),
            ),
        }
    }

    fn has_method(&self, name: &str) -> bool {
        match self.rec.kind {
            Kind::Album => matches!(name, "items" | "get" | "keys"),
            Kind::Item => matches!(name, "get_album" | "get" | "keys"),
        }
    }

    fn call_method(&self, name: &str, args: &[V<'a>]) -> PyResult<V<'a>> {
        match name {
            "items" => Ok(self.items()),
            "get_album" => Ok(match &self.album {
                Some(a) => eval::obj(RecObj::new(a.clone(), self.lib, self.env.clone())),
                None => V::None,
            }),
            "keys" => Ok(V::list(self.rec.keys().into_iter().map(V::str).collect())),
            "get" => {
                let Some(V::Str(k)) = args.first() else {
                    return err("TypeError", "get() takes a field name");
                };
                Ok(self
                    .get(k)?
                    .unwrap_or_else(|| args.get(1).cloned().unwrap_or(V::None)))
            }
            _ => err(
                "AttributeError",
                format!("'{}' object has no attribute '{name}'", self.type_name()),
            ),
        }
    }

    fn repr(&self) -> String {
        format!("{}({})", self.type_name(), self.rec.id.unwrap_or(0))
    }
}

/// Evaluate a field for `rec` (an item, with its album; or an album): the
/// Python value, or the exception.
pub fn evaluate<'a>(
    f: &Field,
    rec: &Record,
    album: Option<&Album>,
    lib: Option<&'a Library>,
    env: &Rc<FormatEnv>,
) -> PyResult<V<'a>> {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(d.get() - 1));
        }
    }
    let code = match &f.parsed {
        Ok(c) => c,
        Err(what) => {
            return err(
                "NotImplementedError",
                format!("beets-rs' inline doesn't support {what}"),
            );
        }
    };
    let depth = DEPTH.with(|d| {
        d.set(d.get() + 1);
        d.get()
    });
    let _g = Guard;
    if depth > 50 {
        return err("RecursionError", "maximum recursion depth exceeded");
    }
    // The variables: the object's fields (an item's album's too), except
    // the field itself; an album's `items`.
    let mut globals: HashMap<String, V<'a>> = HashMap::new();
    for key in rec.keys() {
        if key != f.name {
            globals.insert(key.clone(), py_value(rec, &key));
        }
    }
    if let Some(a) = album {
        for key in a.keys() {
            if key != f.name && !globals.contains_key(&key) {
                globals.insert(key.clone(), py_value(a, &key));
            }
        }
    }
    let mut obj = RecObj::new(rec.clone(), lib, env.clone());
    if obj.album.is_none() {
        obj.album = album.cloned();
    }
    if rec.kind == Kind::Album {
        globals.insert("items".into(), obj.items());
    }
    let db_obj = eval::obj(obj);
    match code {
        Code::Expr(e) => {
            globals.insert("db_obj".into(), db_obj);
            eval::Interp::new(&globals).eval(e)
        }
        Code::Body(body) => eval::Interp::new(&globals).run_body(body, db_obj),
    }
}

/// The field's value as a template shows it: beets' type formatting of
/// the value (None is empty, bytes decoded, a fixed field's own format
/// when the definition shadows one).
pub fn format_result(v: &V, rec: &Record, name: &str) -> String {
    if let Some(ty) = rec.field_type(name) {
        let value = match v {
            V::None => Value::Null,
            V::Int(i) => Value::Int(*i),
            V::Bool(b) => Value::Bool(*b),
            V::Float(f) => Value::Float(*f),
            other => Value::Str(other.to_str()),
        };
        if !matches!(ty, FieldType::Str | FieldType::List(_) | FieldType::Path) {
            return value.format(Some(ty));
        }
    }
    match v {
        V::None => String::new(),
        V::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        other => other.to_str(),
    }
}

/// Load the plugin: log the definitions that aren't Python.
pub struct InlinePlugin;

impl crate::plugins::Plugin for InlinePlugin {
    fn name(&self) -> &'static str {
        "inline"
    }
}

pub fn load(cfg: &Config) -> InlinePlugin {
    Inline::new(cfg, true);
    InlinePlugin
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "inline",
        order: 12,
        defaults: r#"pathfields: {}
item_fields: {}
album_fields: {}
"#,
        config_keys: &["pathfields", "item_fields", "album_fields"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(crate::inline::load(cfg)))
}
