//! A port of beets 2.14's `export` plugin (`beetsplug/export.py`):
//! `beet export` writes the tags (or, with `-l`/`-a`, the library's
//! fields) of the matching items or albums as JSON, JSON Lines, CSV or XML.
//!
//! The output formats follow Python's `json`, `csv` and `ElementTree`
//! modules: the JSON layout comes from `export.<format>.formatting`
//! (`ensure_ascii`, `indent`, `separators`, `sort_keys`), CSV from
//! `delimiter`/`dialect` (only the `excel` dialect).

use crate::config::Config;
use crate::library::Library;
use crate::plugins::info::{self, PyVal};
use anyhow::{Result, bail};
use std::io::Write;

/// The output formats, as `VALID_FORMATS`.
const VALID_FORMATS: &[&str] = &["json", "jsonlines", "csv", "xml"];

/// The plugin, once its `default_format` checks out.
#[derive(Debug)]
pub struct ExportPlugin;

impl ExportPlugin {
    /// The plugin's `__init__`: `default_format` must be a valid format.
    pub fn new(cfg: &Config) -> Result<Self> {
        let fmt = cfg.str("export.default_format");
        if !VALID_FORMATS.contains(&fmt.as_str()) {
            bail!(
                "export.default_format: must be one of ['json', 'jsonlines', 'csv', 'xml'], \
                 not {}",
                crate::hooks::py_repr(&fmt)
            );
        }
        Ok(ExportPlugin)
    }
}

impl crate::plugins::Plugin for ExportPlugin {
    fn name(&self) -> &'static str {
        "export"
    }
}

/// `beet export`'s options.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    pub album: bool,
    pub library: bool,
    pub append: bool,
    pub included_keys: Vec<String>,
    pub limit: Option<usize>,
    pub output: Option<String>,
    pub format: Option<String>,
    pub query: Vec<String>,
}

/// Python's `json.dumps` layout options (`export.<format>.formatting`).
#[derive(Debug, Clone)]
struct JsonOpts {
    ensure_ascii: bool,
    indent: Option<usize>,
    item_sep: String,
    key_sep: String,
    sort_keys: bool,
}

impl JsonOpts {
    fn from_config(cfg: &Config, format: &str) -> Self {
        let base = format!("export.{format}.formatting");
        let mut item_sep = ", ".to_string();
        let mut key_sep = ": ".to_string();
        if let Some(sep) = cfg.get(&format!("{base}.separators"))
            && let Some(seq) = sep.as_sequence()
        {
            let mut it = seq.iter();
            if let Some(a) = it.next().and_then(|v| v.as_str()) {
                item_sep = a.to_string();
            }
            if let Some(b) = it.next().and_then(|v| v.as_str()) {
                key_sep = b.to_string();
            }
        }
        JsonOpts {
            ensure_ascii: cfg.bool(&format!("{base}.ensure_ascii")),
            indent: cfg
                .get(&format!("{base}.indent"))
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
            item_sep,
            key_sep,
            sort_keys: cfg.bool(&format!("{base}.sort_keys")),
        }
    }
}

/// A JSON value for the export writer.
#[derive(Clone, Debug)]
enum JVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<JVal>),
    Obj(Vec<(String, JVal)>),
}

impl JVal {
    fn from_py(v: &Option<PyVal>) -> JVal {
        match v {
            None => JVal::Null,
            Some(PyVal::Str(s)) => JVal::Str(s.clone()),
            Some(PyVal::Int(i)) => JVal::Int(*i),
            Some(PyVal::Bool(b)) => JVal::Bool(*b),
            Some(PyVal::Float(f)) => JVal::Float(*f),
            Some(PyVal::List(l)) => JVal::List(l.iter().cloned().map(JVal::Str).collect()),
            Some(PyVal::Tuples(t)) => JVal::List(
                t.iter()
                    .map(|(s, ms)| JVal::List(vec![JVal::Str(s.clone()), JVal::Int(*ms as i64)]))
                    .collect(),
            ),
        }
    }

    /// `json.dump` with these options (no trailing newline).
    fn dumps(&self, o: &JsonOpts) -> String {
        let mut out = String::new();
        self.write(&mut out, o, 0);
        out
    }

    fn write(&self, out: &mut String, o: &JsonOpts, depth: usize) {
        let newline = |out: &mut String, depth: usize| {
            if let Some(n) = o.indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * depth));
            }
        };
        match self {
            JVal::Null => out.push_str("null"),
            JVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JVal::Int(i) => out.push_str(&i.to_string()),
            JVal::Float(f) => out.push_str(&py_json_float(*f)),
            JVal::Str(s) => py_json_str(out, s, o.ensure_ascii),
            JVal::List(l) if l.is_empty() => out.push_str("[]"),
            JVal::Obj(m) if m.is_empty() => out.push_str("{}"),
            JVal::List(l) => {
                out.push('[');
                for (i, v) in l.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&o.item_sep);
                    }
                    newline(out, depth + 1);
                    v.write(out, o, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            JVal::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&o.item_sep);
                    }
                    newline(out, depth + 1);
                    py_json_str(out, k, o.ensure_ascii);
                    out.push_str(&o.key_sep);
                    v.write(out, o, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

/// `json.dumps` of a float.
fn py_json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        crate::pyyaml::py_float_repr(f)
    }
}

/// `json.dumps` of a string, with or without `ensure_ascii`.
fn py_json_str(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            ' '..='~' => out.push(c),
            c if !ensure_ascii => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// Python's `str()` of a value, as `csv.writer` writes it.
fn py_str(v: &Option<PyVal>) -> String {
    match v {
        None => String::new(),
        Some(PyVal::Str(s)) => s.clone(),
        Some(PyVal::Int(i)) => i.to_string(),
        Some(PyVal::Bool(b)) => if *b { "True" } else { "False" }.into(),
        Some(PyVal::Float(f)) => crate::hooks::py_float(*f),
        Some(PyVal::List(l)) => py_repr_list(l),
        Some(PyVal::Tuples(t)) => {
            let parts: Vec<String> = t
                .iter()
                .map(|(s, ms)| format!("({}, {ms})", crate::hooks::py_repr(s)))
                .collect();
            format!("[{}]", parts.join(", "))
        }
    }
}

fn py_repr_list(l: &[String]) -> String {
    let parts: Vec<String> = l.iter().map(|s| crate::hooks::py_repr(s)).collect();
    format!("[{}]", parts.join(", "))
}

/// A CSV field, quoted as Python's `csv` writer (dialect `excel`) does.
fn csv_field(s: &str, delimiter: char) -> String {
    if s.contains(delimiter) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// `ElementTree`'s text escaping.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `ElementTree.tostring(library, encoding="unicode")`.
fn xml_doc(rows: &[Vec<(String, String)>]) -> String {
    if rows.is_empty() {
        return "<library><tracks /></library>".to_string();
    }
    let mut out = String::from("<library><tracks>");
    for row in rows {
        out.push_str("<track>");
        for (k, v) in row {
            if v.is_empty() {
                out.push_str(&format!("<{k} />"));
            } else {
                out.push_str(&format!("<{k}>{}</{k}>", xml_escape(v)));
            }
        }
        out.push_str("</track>");
    }
    out.push_str("</tracks></library>");
    out
}

/// The collected data of one entry (keys in the order they'll be written).
type Row = Vec<(String, Option<PyVal>)>;

/// `ExportPlugin.run`: export to `out`.
pub fn run(cfg: &Config, lib: &Library, opts: &CmdOpts, out: &mut impl Write) -> Result<()> {
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| cfg.str("export.default_format"));
    let included: Vec<String> = opts
        .included_keys
        .iter()
        .flat_map(|k| k.split(',').map(String::from))
        .collect();
    let keys = (!included.is_empty()).then_some(included.as_slice());
    let entries =
        info::collect_entries(lib, opts.album, opts.library, &opts.query, opts.limit, keys)?;
    let mut rows: Vec<Row> = vec![];
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("export: cannot read file: {e:#}");
                continue;
            }
        };
        let mut row: Row = entry.data.into_iter().collect();
        // `tag_data` builds its dict in the `-i` keys' order.
        if !opts.library && !opts.album {
            row.sort_by_key(|(k, _)| included.iter().position(|i| i == k).unwrap_or(usize::MAX));
        }
        rows.push(row);
    }
    match format.as_str() {
        "json" | "jsonlines" => {
            let o = JsonOpts::from_config(cfg, &format);
            if format == "json" {
                let list = JVal::List(rows.iter().map(|r| row_json(r, o.sort_keys)).collect());
                writeln!(out, "{}", list.dumps(&o))?;
            } else {
                for r in &rows {
                    writeln!(out, "{}", row_json(r, o.sort_keys).dumps(&o))?;
                }
            }
        }
        "csv" => {
            let delimiter = cfg
                .str("export.csv.formatting.delimiter")
                .chars()
                .next()
                .unwrap_or(',');
            let header: Vec<String> = rows
                .first()
                .map(|r| r.iter().map(|(k, _)| k.clone()).collect())
                .unwrap_or_default();
            let line = |fields: Vec<String>| {
                let f: Vec<String> = fields.iter().map(|s| csv_field(s, delimiter)).collect();
                f.join(&delimiter.to_string()) + "\r\n"
            };
            out.write_all(line(header).as_bytes())?;
            for r in &rows {
                out.write_all(line(r.iter().map(|(_, v)| py_str(v)).collect()).as_bytes())?;
            }
        }
        "xml" => {
            let rows: Vec<Vec<(String, String)>> = rows
                .iter()
                .map(|r| r.iter().map(|(k, v)| (k.clone(), py_str(v))).collect())
                .collect();
            out.write_all(xml_doc(&rows).as_bytes())?;
        }
        other => bail!("unknown format: {other}"),
    }
    Ok(())
}

fn row_json(r: &Row, sort_keys: bool) -> JVal {
    let mut items: Vec<(String, JVal)> = r
        .iter()
        .map(|(k, v)| (k.clone(), JVal::from_py(v)))
        .collect();
    if sort_keys {
        items.sort_by(|a, b| a.0.cmp(&b.0));
    }
    JVal::Obj(items)
}

/// `beet export`, to stdout or `-o`.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    match &opts.output {
        None => run(cfg, lib, &opts, &mut std::io::stdout().lock()),
        Some(path) => {
            let mode = if opts.append {
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
            } else {
                std::fs::File::create(path)
            };
            let mut f = std::io::BufWriter::new(mode?);
            run(cfg, lib, &opts, &mut f)
        }
    }
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "export",
        order: 35,
        defaults: r#"export:
    default_format: json
    json:
        formatting:
            ensure_ascii: no
            indent: 4
            separators: [',', ': ']
            sort_keys: yes
    jsonlines:
        formatting:
            ensure_ascii: no
            separators: [',', ': ']
            sort_keys: yes
    csv:
        formatting:
            delimiter: ','
            dialect: excel
    xml:
        formatting: {}
"#,
        config_keys: &["export"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "export",
                build: build_export,
                run: run_export,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(ExportPlugin::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ExportArgs {
    /// Export albums instead of tracks
    #[arg(short = 'a', long)]
    album: bool,
    /// Show library fields instead of tags
    #[arg(short = 'l', long)]
    library: bool,
    /// Append data to the file
    #[arg(long)]
    append: bool,
    /// Comma separated list of keys to show
    #[arg(short = 'i', long = "include-keys", value_name = "KEYS")]
    included_keys: Vec<String>,
    /// Path for the output file; without it, print the data
    #[arg(short = 'o', long, value_name = "PATH")]
    output: Option<String>,
    /// The output format: json|jsonlines|csv|xml
    #[arg(short = 'f', long, value_parser = ["json", "jsonlines", "csv", "xml"])]
    format: Option<String>,
    /// Limit query results
    #[arg(long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_export() -> clap::Command {
    use clap::Args as _;
    ExportArgs::augment_args(
        clap::Command::new("export").about("export data from beets"),
    )
}

pub fn run_export(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ExportArgs::from_arg_matches(m)?;
    crate::plugins::export::command(
        cfg,
        lib,
        crate::plugins::export::CmdOpts {
            album: a.album,
            library: a.library,
            append: a.append,
            included_keys: a.included_keys,
            limit: a.limit,
            output: a.output,
            format: a.format,
            query: a.query,
        },
    )
}
