//! A port of beets 2.14's `info` plugin (`beetsplug/info.py`): `beet info`
//! shows the tags read from files (paths, or the files of the items a
//! query matches) or, with `-l`/`-a`, the library's formatted values.

use crate::formatted::Formatted;
use crate::hooks::{py_float, py_repr};
use crate::library::Library;
use crate::mediafile::{Got, MediaFile, TAG_FIELDS};
use crate::model::{Kind, Record, Value};
use crate::plugins::PluginLog;
use crate::query::Query;
use crate::template::Template;
use anyhow::Result;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The plugin: only a command, no import hooks.
pub struct InfoPlugin;

impl crate::plugins::Plugin for InfoPlugin {
    fn name(&self) -> &'static str {
        "info"
    }
}

/// `beet info`'s options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub album: bool,
    pub library: bool,
    pub summarize: bool,
    pub included_keys: Vec<String>,
    pub keys_only: bool,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub args: Vec<String>,
}

/// A value as the plugin holds it: what MediaFile returns for a tag (None
/// for a missing one), or a formatted library value, or the summary's
/// `"[various]"`.
#[derive(Clone, Debug)]
pub enum PyVal {
    Str(String),
    Int(i64),
    Bool(bool),
    Float(f64),
    List(Vec<String>),
    /// `synced_lyrics`' `(text, milliseconds)` tuples.
    Tuples(Vec<(String, u32)>),
}

impl PyVal {
    fn number(&self) -> Option<f64> {
        match self {
            PyVal::Int(i) => Some(*i as f64),
            PyVal::Bool(b) => Some(f64::from(u8::from(*b))),
            PyVal::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// Python's `==` (numbers compare across int, bool and float).
impl PartialEq for PyVal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PyVal::Str(a), PyVal::Str(b)) => a == b,
            (PyVal::List(a), PyVal::List(b)) => a == b,
            (PyVal::Tuples(a), PyVal::Tuples(b)) => a == b,
            _ => match (self.number(), other.number()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            },
        }
    }
}

/// Python's `str()` of the value (lists joined with "; ").
impl std::fmt::Display for PyVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PyVal::Str(s) => f.write_str(s),
            PyVal::Int(i) => write!(f, "{i}"),
            PyVal::Bool(b) => f.write_str(if *b { "True" } else { "False" }),
            PyVal::Float(x) => f.write_str(&py_float(*x)),
            PyVal::List(v) => f.write_str(&v.join("; ")),
            // beets fails here ("; ".join of tuples); show the tuples.
            PyVal::Tuples(v) => {
                let parts: Vec<String> = v
                    .iter()
                    .map(|(t, ms)| format!("({}, {ms})", py_repr(t)))
                    .collect();
                f.write_str(&parts.join("; "))
            }
        }
    }
}

/// Field name to value (None: the tag is missing).
pub type Data = BTreeMap<String, Option<PyVal>>;

/// MediaFile's audio properties (from the item `Item.from_path` makes).
const PROPERTIES: &[&str] = &[
    "bitdepth",
    "bitrate",
    "bitrate_mode",
    "channels",
    "encoder_info",
    "encoder_settings",
    "format",
    "length",
    "samplerate",
];

/// `tag_fields()`: `MediaFile.readable_fields()` and `art`, less `images`
/// (which the plugin always drops), sorted.
pub fn tag_fields() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = TAG_FIELDS
        .iter()
        .copied()
        .chain(crate::mediafile::extra_field_names())
        .chain(PROPERTIES.iter().copied())
        .chain(["art", "date", "original_date", "synced_lyrics"])
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One thing to show: its data and the model to format (`-f`) and take
/// the path from.
pub(crate) struct Entry {
    pub data: Data,
    pub model: Record,
    /// The path line (None for a summary).
    pub path: Option<String>,
}

/// `tag_data_emitter`: the file's tags (`fields`: all of them, or the
/// `-i` keys).
pub(crate) fn tag_entry(path: &Path, keys: Option<&[String]>) -> Result<Entry> {
    let owned: Vec<String>;
    let fields: &[String] = match keys {
        Some(k) => k,
        None => {
            owned = tag_fields().into_iter().map(String::from).collect();
            &owned
        }
    };
    // As MediaFile's `UnreadableFileError`: "PATH: reason".
    let unreadable = |e: anyhow::Error| anyhow::anyhow!("{}: {}", path.display(), e.root_cause());
    let mf = MediaFile::open(path).map_err(unreadable)?;
    // A temporary item, for the properties and `-f` (`Item.from_path`).
    let item = crate::media::read(path).map_err(unreadable)?;
    let mut data = Data::new();
    for field in fields {
        if field == "images" {
            continue;
        }
        let v = match field.as_str() {
            "art" => Some(PyVal::Bool(!mf.pictures().is_empty())),
            "date" | "original_date" => mf.date(field).map(PyVal::Str),
            "synced_lyrics" => mf.synced_lyrics().map(PyVal::Tuples),
            // mutagen's WavPack: `channels = bool(flags & MONO) or 2`.
            "channels" if item.str("format") == "WavPack" && item.int("channels") == 1 => {
                Some(PyVal::Bool(true))
            }
            f if PROPERTIES.contains(&f) => match item.get(f) {
                Some(Value::Str(s)) => Some(PyVal::Str(s)),
                Some(Value::Int(i)) => Some(PyVal::Int(i)),
                Some(Value::Float(x)) => Some(PyVal::Float(x)),
                _ => None,
            },
            f => match mf.get(f) {
                Some(Got::Str(v)) => v.map(PyVal::Str),
                Some(Got::Int(v)) => v.map(PyVal::Int),
                Some(Got::Bool(v)) => v.map(PyVal::Bool),
                Some(Got::Float(v)) => v.map(PyVal::Float),
                Some(Got::List(v)) => v.map(PyVal::List),
                None => None,
            },
        };
        data.insert(field.clone(), v);
    }
    Ok(Entry {
        data,
        model: item,
        path: Some(path.to_string_lossy().into_owned()),
    })
}

/// `library_data_emitter`: the model's formatted values.
pub(crate) fn library_entry(lib: &Library, model: Record, keys: Option<&[String]>) -> Entry {
    let f = Formatted::new(&model, Some(lib));
    let keys = match keys {
        Some(k) => k.to_vec(),
        None => f.all_keys(),
    };
    let data = keys
        .into_iter()
        .map(|k| {
            // A missing key formats as its type's null (`_type(key).format`).
            let v = f.get(&k).unwrap_or_else(|| {
                crate::model::any_type(model.kind, &k)
                    .map(|ty| ty.default_value().format(Some(ty)))
                    .unwrap_or_default()
            });
            (k, Some(PyVal::Str(v)))
        })
        .collect();
    let path = match model.kind {
        Kind::Item => model.path().to_string_lossy().into_owned(),
        Kind::Album => f.get("path").unwrap_or_default(),
    };
    drop(f);
    Entry {
        data,
        model,
        path: Some(path),
    }
}

/// `update_summary`: a value that differs between entries becomes
/// "[various]".
pub fn update_summary(summary: &mut Data, data: Data) {
    for (k, v) in data {
        match summary.get_mut(&k) {
            None => {
                summary.insert(k, v);
            }
            Some(cur) if *cur != v => *cur = Some(PyVal::Str("[various]".into())),
            Some(_) => {}
        }
    }
}

/// `print_data` without a format: `field: value` lines, the fields right
/// aligned and sorted, missing values left out; nothing at all (not even
/// the path) when there are no values.
pub fn render_data(data: &Data, path: Option<&str>) -> Vec<String> {
    let shown: Vec<(&String, &PyVal)> = data
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| (k, v)))
        .collect();
    if shown.is_empty() {
        return vec![];
    }
    let width = shown.iter().map(|(k, _)| k.chars().count()).max().unwrap();
    let mut out = vec![];
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        out.push(p.to_string());
    }
    for (k, v) in shown {
        out.push(format!("{k:>width$}: {v}"));
    }
    out
}

/// `print_data_keys`: the path, then every key (with or without a value).
pub fn render_keys(data: &Data, path: Option<&str>) -> Vec<String> {
    if data.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        out.push(p.to_string());
    }
    out.extend(data.keys().map(|k| format!("    {k}")));
    out
}

/// The entries `tag_data` (paths first, then the query's items) or
/// `library_data` would yield, in order.
pub(crate) fn collect_entries(
    lib: &Library,
    album: bool,
    library: bool,
    args: &[String],
    limit: Option<usize>,
    keys: Option<&[String]>,
) -> Result<Vec<Result<Entry>>> {
    let mut entries: Vec<Result<Entry>> = vec![];
    if library || album {
        let kind = if album { Kind::Album } else { Kind::Item };
        let q = Query::parse(kind, args)?;
        let mut models = if album {
            lib.albums(&q)?
        } else {
            lib.items(&q)?
        };
        models.truncate(limit.unwrap_or(usize::MAX));
        entries.extend(models.into_iter().map(|m| Ok(library_entry(lib, m, keys))));
    } else {
        let mut query = vec![];
        let mut files: Vec<PathBuf> = vec![];
        for arg in args {
            let p = crate::query::normpath(Path::new(arg));
            if p.is_file() {
                files.push(p);
            } else {
                query.push(arg.clone());
            }
        }
        entries.extend(files.iter().map(|p| tag_entry(p, keys)));
        if !query.is_empty() {
            let mut items = lib.items(&Query::parse(Kind::Item, &query)?)?;
            items.truncate(limit.unwrap_or(usize::MAX));
            entries.extend(items.iter().map(|i| tag_entry(i.path(), keys)));
        }
    }
    Ok(entries)
}

/// `InfoPlugin.run`, printing to stdout.
pub fn command(lib: &Library, opts: CmdOpts) -> Result<()> {
    run(lib, opts, &mut std::io::stdout().lock())
}

/// `InfoPlugin.run`, writing to `out`.
pub fn run(lib: &Library, opts: CmdOpts, out: &mut impl Write) -> Result<()> {
    let log = PluginLog::new("info");
    let mut keys: Vec<String> = vec![];
    for k in &opts.included_keys {
        keys.extend(k.split(',').map(String::from));
    }
    // `path` is dropped, even if given more than once.
    keys.retain(|k| k != "path");
    let keys = (!keys.is_empty()).then_some(keys);
    let keys = keys.as_deref();

    let entries = collect_entries(lib, opts.album, opts.library, &opts.args, opts.limit, keys)?;

    let template = opts.format.as_deref().map(Template::parse);
    let mut first = true;
    let mut summary = Data::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log.error(&format!("cannot read file: {e:#}"));
                continue;
            }
        };
        if opts.summarize {
            update_summary(&mut summary, entry.data);
            continue;
        }
        if !first {
            writeln!(out)?;
        }
        if opts.keys_only {
            print_lines(out, render_keys(&entry.data, entry.path.as_deref()))?;
        } else if let Some(t) = &template {
            let line = crate::commands::format_record(t, &entry.model, Some(lib));
            writeln!(out, "{line}")?;
        } else {
            print_lines(out, render_data(&entry.data, entry.path.as_deref()))?;
        }
        first = false;
    }
    if opts.summarize {
        print_lines(out, render_data(&summary, None))?;
    }
    Ok(())
}

fn print_lines(out: &mut impl Write, lines: Vec<String>) -> Result<()> {
    for l in lines {
        writeln!(out, "{l}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "info_tests.rs"]
mod tests;
