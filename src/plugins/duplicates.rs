//! A port of beets 2.14's `duplicates` plugin (`beetsplug/duplicates.py`):
//! `beet duplicates` (`dup`) groups items (or albums, `-a`) by keys (or by
//! an external program's output, `-C`) and lists the duplicates in each
//! group, optionally merging, copying, moving, deleting, removing or
//! tagging them. (The importer's duplicate handling is `crate::duplicates`.)

use crate::config::Config;
use crate::fileops::FileOp;
use crate::library::Library;
use crate::model::{FieldType, Kind, Record, Value, schema};
use crate::plugins::PluginLog;
use crate::query::{Obj, PyVal, Query};
use crate::template::Template;
use anyhow::{Result, bail};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The plugin: only a command.
pub struct DuplicatesPlugin;

impl crate::plugins::Plugin for DuplicatesPlugin {
    fn name(&self) -> &'static str {
        "duplicates"
    }
}

/// `beet duplicates`' options; those not given leave the settings alone
/// (beets' `config.set_args`).
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub count: bool,
    pub checksum: Option<String>,
    pub delete: bool,
    pub full: bool,
    pub strict: bool,
    pub keys: Vec<String>,
    pub merge: bool,
    pub move_to: Option<String>,
    pub copy_to: Option<String>,
    pub tag: Option<String>,
    pub remove: bool,
    pub album: bool,
    pub path: bool,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// The settings with the command's options over them.
struct Settings {
    album: bool,
    checksum: String,
    copy: String,
    count: bool,
    delete: bool,
    format: String,
    full: bool,
    keys: Vec<String>,
    merge: bool,
    move_to: String,
    path: bool,
    tiebreak: HashMap<String, Vec<String>>,
    strict: bool,
    tag: String,
    remove: bool,
}

impl Settings {
    fn new(cfg: &Config, o: &CmdOpts) -> Settings {
        let s = |key: &str, opt: &Option<String>| match opt {
            Some(v) => v.clone(),
            None => cfg.str(&format!("duplicates.{key}")),
        };
        let b = |key: &str, opt: bool| opt || cfg.bool(&format!("duplicates.{key}"));
        let mut tiebreak = HashMap::new();
        for (kind, _) in cfg.mapping("duplicates.tiebreak") {
            tiebreak.insert(
                kind.clone(),
                cfg.list(&format!("duplicates.tiebreak.{kind}")),
            );
        }
        Settings {
            album: b("album", o.album),
            checksum: s("checksum", &o.checksum),
            copy: s("copy", &o.copy_to),
            count: b("count", o.count),
            delete: b("delete", o.delete),
            format: s("format", &o.format),
            full: b("full", o.full),
            keys: if o.keys.is_empty() {
                cfg.list("duplicates.keys")
            } else {
                o.keys.clone()
            },
            merge: b("merge", o.merge),
            move_to: s("move", &o.move_to),
            path: b("path", o.path),
            tiebreak,
            strict: b("strict", o.strict),
            tag: s("tag", &o.tag),
            remove: b("remove", o.remove),
        }
    }
}

/// `beet duplicates`, printing to stdout.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    run(cfg, lib, opts, &mut std::io::stdout().lock())
}

/// `beet duplicates`, writing to `out`.
pub fn run(cfg: &Config, lib: &Library, opts: CmdOpts, out: &mut impl Write) -> Result<()> {
    let s = Settings::new(cfg, &opts);
    let d = Dup {
        cfg,
        lib,
        s: &s,
        log: PluginLog::new("duplicates"),
    };
    let limit = opts.limit.unwrap_or(usize::MAX);
    if s.album {
        let mut albums = lib.albums(&Query::parse(Kind::Album, &opts.query)?)?;
        albums.truncate(limit);
        d.run_command(albums, &["mb_albumid"], "$albumartist - $album", out)
    } else {
        let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
        items.truncate(limit);
        d.run_command(
            items,
            &["mb_trackid", "mb_albumid"],
            "$albumartist - $album - $title",
            out,
        )
    }
}

/// One element of a group key: a value as Python compares and hashes it
/// (numbers across int, bool and float; `bytes` never equal to `str`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum KeyPart {
    Num(u64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<String>),
}

impl KeyPart {
    /// None for the values `_group_by` drops (`None` and `""`).
    fn of(v: &PyVal) -> Option<KeyPart> {
        let num = |f: f64| KeyPart::Num(if f == 0.0 { 0.0f64 } else { f }.to_bits());
        Some(match v {
            PyVal::None => return None,
            PyVal::Str(s) if s.is_empty() => return None,
            PyVal::Str(s) => KeyPart::Str(s.clone()),
            PyVal::Bool(b) => num(f64::from(u8::from(*b))),
            PyVal::Int(i) => num(*i as f64),
            PyVal::Float(f) => num(*f),
            PyVal::Bytes(b) => KeyPart::Bytes(b.clone()),
            // beets would fail here (a list isn't hashable).
            PyVal::List(l) => KeyPart::List(l.clone()),
        })
    }
}

/// Python's truthiness.
fn truthy(v: &PyVal) -> bool {
    match v {
        PyVal::None => false,
        PyVal::Bool(b) => *b,
        PyVal::Int(i) => *i != 0,
        PyVal::Float(f) => *f != 0.0,
        PyVal::Str(s) => !s.is_empty(),
        PyVal::Bytes(b) => !b.is_empty(),
        PyVal::List(l) => !l.is_empty(),
    }
}

/// Python's `<` between two values (incomparable ones count as equal).
fn py_cmp(a: &PyVal, b: &PyVal) -> Ordering {
    let num = |v: &PyVal| match v {
        PyVal::Bool(x) => Some(f64::from(u8::from(*x))),
        PyVal::Int(i) => Some(*i as f64),
        PyVal::Float(f) => Some(*f),
        _ => None,
    };
    match (a, b) {
        (PyVal::Str(x), PyVal::Str(y)) => x.cmp(y),
        (PyVal::Bytes(x), PyVal::Bytes(y)) => x.cmp(y),
        (PyVal::List(x), PyVal::List(y)) => x.cmp(y),
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => Ordering::Equal,
        },
    }
}

/// A model with what `getattr` reaches through it: an item's album, an
/// album's directory.
struct Model {
    rec: Record,
    album: Option<Record>,
    dir: Option<PathBuf>,
    /// Checksums computed in this run (Python `bytes`; cached ones read
    /// back from the database are `str`).
    fresh: HashMap<String, Vec<u8>>,
}

impl Model {
    fn load(lib: &Library, rec: Record) -> Result<Model> {
        let (album, dir) = match rec.kind {
            Kind::Item => (lib.get_album(rec.int("album_id"))?, None),
            Kind::Album => {
                let items = lib.album_items(rec.id.unwrap_or(0))?;
                let dir = items
                    .first()
                    .and_then(|i| i.path().parent())
                    .map(Path::to_path_buf);
                (None, dir)
            }
        };
        Ok(Model {
            rec,
            album,
            dir,
            fresh: HashMap::new(),
        })
    }

    /// `getattr(model, key, None)`.
    fn get(&self, key: &str) -> PyVal {
        if let Some(b) = self.fresh.get(key) {
            return PyVal::Bytes(b.clone());
        }
        Obj {
            rec: &self.rec,
            album: self.album.as_ref(),
            album_dir: self.dir.as_deref(),
            album_counts: None,
        }
        .get(key)
        .unwrap_or(PyVal::None)
    }

    /// `model.path` (an album's directory).
    fn path(&self) -> PathBuf {
        match self.rec.kind {
            Kind::Item => self.rec.path().to_path_buf(),
            Kind::Album => self.dir.clone().unwrap_or_default(),
        }
    }
}

/// Item fields and computed fields (`Item.all_keys()`), with their types
/// (None for computed fields).
fn item_all_keys() -> Vec<(&'static str, Option<FieldType>)> {
    let mut v: Vec<(&'static str, Option<FieldType>)> = vec![("id", None)];
    v.extend(schema(Kind::Item).iter().map(|(n, t)| (*n, Some(*t))));
    v.extend(["singleton", "filesize", "has_cover_art"].map(|k| (k, None)));
    v
}

/// An item's own value of an `Item.all_keys()` field, as beets reads it.
fn own_value(rec: &Record, key: &str, ty: Option<FieldType>) -> PyVal {
    match (key, ty) {
        ("id", _) => rec.id.map_or(PyVal::None, PyVal::Int),
        // Computed: always a bool or a size.
        (_, None) => PyVal::Int(1),
        ("album_id", _) if rec.int("album_id") == 0 => PyVal::None,
        (_, Some(FieldType::List(_))) => PyVal::List(rec.list(key)),
        _ => match rec.get(key) {
            None | Some(Value::Null) => PyVal::None,
            Some(Value::Str(s)) => PyVal::Str(s),
            Some(Value::Int(i)) => PyVal::Int(i),
            Some(Value::Float(f)) => PyVal::Float(f),
            Some(Value::Bool(b)) => PyVal::Bool(b),
            Some(Value::Path(p)) => PyVal::Bytes(p.as_os_str().as_encoded_bytes().to_vec()),
        },
    }
}

/// Python's `shlex.split` (POSIX mode): whitespace-separated words,
/// single quotes literal, double quotes with `\\` escaping `\\ " $ \``, a
/// backslash outside quotes escaping the next character, `#` comments.
pub fn shlex_split(s: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '#' if !in_word => break,
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(n) => cur.push(n),
                    None => bail!("No escaped character"),
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(n) => cur.push(n),
                        None => bail!("No closing quotation"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(n @ ('\\' | '"' | '$' | '`' | '\n')) => cur.push(n),
                            Some(n) => {
                                cur.push('\\');
                                cur.push(n);
                            }
                            None => bail!("No closing quotation"),
                        },
                        Some(n) => cur.push(n),
                        None => bail!("No closing quotation"),
                    }
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// Python `str.format(file=...)` of one command argument.
fn format_arg(arg: &str, file: &str) -> Result<String> {
    let mut out = String::new();
    let mut rest = arg;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if let Some(t) = tail.strip_prefix("{file}") {
            out.push_str(file);
            rest = t;
        } else {
            bail!("duplicates: bad replacement field in checksum command argument: {arg}");
        }
    }
    out.push_str(rest);
    Ok(out)
}

struct Dup<'a> {
    cfg: &'a Config,
    lib: &'a Library,
    s: &'a Settings,
    log: PluginLog,
}

impl Dup<'_> {
    /// `_run_command`.
    fn run_command(
        &self,
        recs: Vec<Record>,
        default_keys: &[&str],
        fallback_fmt: &str,
        out: &mut impl Write,
    ) -> Result<()> {
        if recs.is_empty() {
            return Ok(());
        }
        let mut models = recs
            .into_iter()
            .map(|r| Model::load(self.lib, r))
            .collect::<Result<Vec<_>>>()?;
        let fmt = if self.s.path {
            "$path".to_string()
        } else if self.s.format.is_empty() {
            fallback_fmt.to_string()
        } else {
            self.s.format.clone()
        };
        let mut keys: Vec<String> = if self.s.keys.is_empty() {
            default_keys.iter().map(|k| k.to_string()).collect()
        } else {
            self.s.keys.clone()
        };
        if !self.s.checksum.is_empty() {
            let mut key = String::new();
            for m in &mut models {
                key = self.checksum(m, &self.s.checksum)?;
            }
            keys = vec![key];
        }
        for (count, objs) in self.duplicates(models, &keys)? {
            let fmt = if self.s.count {
                format!("{fmt}: {count}")
            } else {
                fmt.clone()
            };
            let t = Template::parse(&fmt);
            for m in objs {
                self.process(&m.rec, &t, out)?;
            }
        }
        Ok(())
    }

    /// `_process_item`.
    fn process(&self, rec: &Record, t: &Template, out: &mut impl Write) -> Result<()> {
        writeln!(
            out,
            "{}",
            crate::commands::format_record(t, rec, Some(self.lib))
        )?;
        out.flush()?;
        let s = self.s;
        if !s.copy.is_empty() {
            let base = PathBuf::from(&s.copy);
            crate::commands::move_model(self.cfg, self.lib, rec, FileOp::Copy, Some(&base))?;
        }
        if !s.move_to.is_empty() {
            let rec = self.reload(rec)?;
            let base = PathBuf::from(&s.move_to);
            crate::commands::move_model(self.cfg, self.lib, &rec, FileOp::Move, Some(&base))?;
        }
        let removed = s.delete || s.remove;
        if removed {
            let rec = self.reload(rec)?;
            crate::commands::remove_model(self.cfg, self.lib, &rec, s.delete)?;
        }
        if !s.tag.is_empty() {
            let parts: Vec<&str> = s.tag.split('=').collect();
            let [k, v] = parts[..] else {
                bail!("duplicates: can't parse k=v tag: {}", s.tag);
            };
            // A removed model has nothing left to store the tag in.
            if !removed {
                let old = self.reload(rec)?;
                let mut new = old.clone();
                new.set_parsed(k, v)?;
                self.store(&old, &new)?;
            }
        }
        Ok(())
    }

    /// The model as stored now (after a copy or move changed its path).
    fn reload(&self, rec: &Record) -> Result<Record> {
        let id = rec.id.unwrap_or(0);
        let fresh = match rec.kind {
            Kind::Item => self.lib.get_item(id)?,
            Kind::Album => self.lib.get_album(id)?,
        };
        Ok(fresh.unwrap_or_else(|| rec.clone()))
    }

    /// `model.store()` (an album's changes inherited by its items).
    fn store(&self, old: &Record, new: &Record) -> Result<()> {
        match new.kind {
            Kind::Item => self.lib.store(new),
            Kind::Album => crate::commands::store_album_inherit(self.lib, old, new),
        }
    }

    fn filepath(m: &Model) -> String {
        m.path().display().to_string()
    }

    /// `_checksum`: run `prog` on the model's file unless its output is
    /// cached in the flexible attribute named after the program; returns
    /// that key.
    fn checksum(&self, m: &mut Model, prog: &str) -> Result<String> {
        let file = m.path().to_string_lossy().into_owned();
        let words = shlex_split(prog)?;
        let args = words
            .iter()
            .map(|w| format_arg(w, &file))
            .collect::<Result<Vec<_>>>()?;
        let Some(key) = args.first().cloned() else {
            bail!("duplicates: empty checksum command");
        };
        if truthy(&m.get(&key)) {
            self.log.debug(&format!(
                "key {key} on item {} cached:not computing checksum",
                Self::filepath(m)
            ));
            return Ok(key);
        }
        self.log.debug(&format!(
            "key {key} on model {} not cached:computing checksum",
            Self::filepath(m)
        ));
        let output = std::process::Command::new(&args[0])
            .args(&args[1..])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| anyhow::anyhow!("{}: {e}", args[0]))?;
        if !output.status.success() {
            self.log.debug(&format!(
                "failed to checksum {}: Command '{}' returned non-zero exit status {}.",
                Self::filepath(m),
                args.join(" "),
                output.status.code().unwrap_or(-1)
            ));
            return Ok(key);
        }
        let old = m.rec.clone();
        m.rec.flex.insert(
            key.clone(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        );
        self.store(&old, &m.rec)?;
        m.fresh.insert(key.clone(), output.stdout);
        let title = m.rec.str("title").to_string();
        self.log
            .debug(&format!("computed checksum for {title} using {key}"));
        Ok(key)
    }

    /// `_group_by`: models by their key values, in first-seen order.
    fn group_by(&self, models: Vec<Model>, keys: &[String]) -> Vec<Vec<Model>> {
        let mut index: HashMap<Vec<KeyPart>, usize> = HashMap::new();
        let mut groups: Vec<Vec<Model>> = vec![];
        let keys_repr = format!(
            "[{}]",
            keys.iter()
                .map(|k| crate::hooks::py_repr(k))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for m in models {
            let values: Vec<KeyPart> = keys.iter().filter_map(|k| KeyPart::of(&m.get(k))).collect();
            if self.s.strict && values.len() < keys.len() {
                self.log.debug(&format!(
                    "some keys {keys_repr} on item {} are null or empty: skipping",
                    Self::filepath(&m)
                ));
            } else if !self.s.strict && values.is_empty() {
                self.log.debug(&format!(
                    "all keys {keys_repr} on item {} are null or empty: skipping",
                    Self::filepath(&m)
                ));
            } else {
                match index.get(&values) {
                    Some(&i) => groups[i].push(m),
                    None => {
                        index.insert(values, groups.len());
                        groups.push(vec![m]);
                    }
                }
            }
        }
        groups
    }

    /// `_order`: most complete (or by `tiebreak`) first; Python's stable
    /// sort, reversed.
    fn order(&self, mut objs: Vec<Model>) -> Result<Vec<Model>> {
        let kind = if objs.iter().all(|m| m.rec.kind == Kind::Item) {
            "items"
        } else {
            "albums"
        };
        let desc = |objs: &mut Vec<Model>, key: &dyn Fn(&Model) -> Vec<PyVal>| {
            let mut keyed: Vec<(Vec<PyVal>, Model)> =
                objs.drain(..).map(|m| (key(&m), m)).collect();
            keyed.sort_by(|(a, _), (b, _)| {
                for (x, y) in a.iter().zip(b) {
                    match py_cmp(y, x) {
                        Ordering::Equal => continue,
                        o => return o,
                    }
                }
                Ordering::Equal
            });
            objs.extend(keyed.into_iter().map(|(_, m)| m));
        };
        if let Some(fields) = self
            .s
            .tiebreak
            .get(kind)
            .filter(|_| !self.s.tiebreak.is_empty())
        {
            desc(&mut objs, &|m| fields.iter().map(|f| m.get(f)).collect());
            return Ok(objs);
        }
        if kind == "items" {
            let fields = item_all_keys();
            desc(&mut objs, &|m| {
                let n = fields
                    .iter()
                    .filter(|(f, ty)| {
                        !matches!(
                            own_value(&m.rec, f, *ty),
                            PyVal::None
                        ) && !matches!(own_value(&m.rec, f, *ty), PyVal::Str(ref s) if s.is_empty())
                    })
                    .count();
                vec![PyVal::Int(n as i64)]
            });
            return Ok(objs);
        }
        let mut sizes = HashMap::new();
        for m in &objs {
            let id = m.rec.id.unwrap_or(0);
            sizes.insert(id, self.lib.album_items(id)?.len() as i64);
        }
        desc(&mut objs, &|m| {
            vec![PyVal::Int(sizes[&m.rec.id.unwrap_or(0)])]
        });
        Ok(objs)
    }

    /// `_merge_items`: fill the head item's empty fields from the others.
    fn merge_items(&self, objs: &mut [Model]) -> Result<()> {
        let (head, tail) = objs.split_first_mut().expect("a group has two or more");
        for (f, ty) in item_all_keys() {
            for o in tail.iter() {
                let cur = own_value(&head.rec, f, ty);
                if !matches!(cur, PyVal::None) && !matches!(cur, PyVal::Str(ref s) if s.is_empty())
                {
                    break;
                }
                let value = own_value(&o.rec, f, ty);
                if truthy(&value) {
                    self.log.debug(&format!(
                        "key {f} on item {} is null or empty: setting from item {}",
                        head.rec.path().display(),
                        o.rec.path().display()
                    ));
                    if let Some(v) = o.rec.get(f) {
                        head.rec.set(f, v);
                    }
                    self.lib.store(&head.rec)?;
                    break;
                }
            }
        }
        Ok(())
    }

    /// `_merge_albums`: copy the others' tracks the head album lacks (by
    /// `mb_trackid`) into it, read afresh from their files.
    fn merge_albums(&self, objs: &mut [Model]) -> Result<()> {
        let head_id = objs[0].rec.id.unwrap_or(0);
        let ids: Vec<String> = self
            .lib
            .album_items(head_id)?
            .iter()
            .map(|i| i.str("mb_trackid").to_string())
            .collect();
        for o in &objs[1..] {
            for i in self.lib.album_items(o.rec.id.unwrap_or(0))? {
                if ids.iter().any(|id| id == i.str("mb_trackid")) {
                    continue;
                }
                let mut missing = crate::media::read(i.path())?;
                missing.set_int("album_id", head_id);
                self.lib.add(&mut missing)?;
                let fmt = |key: &str, rec: &Record| {
                    let t = Template::parse(&self.cfg.str(key));
                    crate::commands::format_record(&t, rec, Some(self.lib))
                };
                let dest = crate::paths::PathFormatter::new(self.cfg)?.destination(
                    &missing,
                    Some(&objs[0].rec),
                    Some(self.lib),
                );
                self.log.debug(&format!(
                    "item {} missing from album {}: merging from {} into {}",
                    fmt("format_item", &missing),
                    fmt("format_album", &objs[0].rec),
                    Self::filepath(o),
                    dest.display()
                ));
                crate::commands::move_model(self.cfg, self.lib, &missing, FileOp::Copy, None)?;
            }
        }
        Ok(())
    }

    /// `_duplicates`: each group of two or more (ordered, merged with
    /// `-M`) with its count, less the first model unless `-F`.
    fn duplicates(&self, models: Vec<Model>, keys: &[String]) -> Result<Vec<(usize, Vec<Model>)>> {
        let offset = usize::from(!self.s.full);
        let mut out = vec![];
        for objs in self.group_by(models, keys) {
            if objs.len() < 2 {
                continue;
            }
            let mut objs = self.order(objs)?;
            if self.s.merge {
                match objs[0].rec.kind {
                    Kind::Item => self.merge_items(&mut objs)?,
                    Kind::Album => self.merge_albums(&mut objs)?,
                }
            }
            let n = objs.len() - offset;
            out.push((n, objs.into_iter().skip(offset).collect()));
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "duplicates_tests.rs"]
mod tests;
