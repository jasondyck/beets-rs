//! beets' `metasync` plugin: copy ratings and play statistics from music
//! players' libraries (iTunes XML and Amarok over D-Bus).

use crate::config::Config;
use crate::library::Library;
use crate::model::{FieldType, Item, Kind, Value};
use crate::plugins::{Plugin, PluginLog};
use crate::query::Query;
use anyhow::{Context as _, Result};
use roxmltree::Node;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use zbus::zvariant::{OwnedValue, Value as DbusValue};

pub const ITEM_TYPES: &[(&str, FieldType)] = &[
    ("itunes_rating", FieldType::Int),
    ("itunes_playcount", FieldType::Int),
    ("itunes_skipcount", FieldType::Int),
    ("itunes_lastplayed", FieldType::Date),
    ("itunes_lastskipped", FieldType::Date),
    ("itunes_dateadded", FieldType::Date),
    ("amarok_rating", FieldType::Int),
    ("amarok_score", FieldType::Float(1)),
    ("amarok_uid", FieldType::Str),
    ("amarok_playcount", FieldType::Int),
    ("amarok_firstplayed", FieldType::Date),
    ("amarok_lastplayed", FieldType::Date),
];

pub struct MetaSync;

impl MetaSync {
    pub fn new() -> Self {
        MetaSync
    }
}

impl Plugin for MetaSync {
    fn name(&self) -> &'static str {
        "metasync"
    }
}

// ------------------------------------------------------------- plist (XML)

/// Some plist node kinds are parsed but not needed by the iTunes fields.
#[allow(dead_code)]
#[derive(Clone, Debug)]
enum Plist {
    Dict(HashMap<String, Plist>),
    Array(Vec<Plist>),
    Str(String),
    Int(i64),
    Real(f64),
    Date(f64),
    Bool(bool),
    Data,
}

fn parse_plist(path: &Path) -> Result<Plist> {
    let text = std::fs::read_to_string(path)?;
    let doc = roxmltree::Document::parse_with_options(
        &text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )?;
    let root = doc.root_element();
    let node = root
        .children()
        .find(roxmltree::Node::is_element)
        .ok_or_else(|| anyhow::anyhow!("empty plist"))?;
    Ok(parse_node(node))
}

fn node_text(n: Node) -> String {
    n.text().unwrap_or_default().trim().to_string()
}

fn parse_node(n: Node) -> Plist {
    match n.tag_name().name() {
        "dict" => {
            let mut map = HashMap::new();
            let kids: Vec<Node> = n.children().filter(roxmltree::Node::is_element).collect();
            for pair in kids.chunks(2) {
                if pair.len() == 2 {
                    let key = node_text(pair[0]);
                    map.insert(key, parse_node(pair[1]));
                }
            }
            Plist::Dict(map)
        }
        "array" => Plist::Array(n.children().filter(roxmltree::Node::is_element).map(parse_node).collect()),
        "string" => Plist::Str(node_text(n)),
        "integer" => Plist::Int(node_text(n).parse().unwrap_or(0)),
        "real" => Plist::Real(node_text(n).parse().unwrap_or(0.0)),
        "date" => Plist::Date(parse_date(&node_text(n)).unwrap_or(0.0)),
        "true" => Plist::Bool(true),
        "false" => Plist::Bool(false),
        "data" => Plist::Data,
        _ => Plist::Str(node_text(n)),
    }
}

/// beets' `time.mktime(date.timetuple())`: the plist's naive UTC timestamp
/// is interpreted in the local timezone.
fn parse_date(s: &str) -> Option<f64> {
    let d = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").ok()?;
    Some(mktime_local(d))
}

impl Plist {
    fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(m) => m.get(key),
            _ => None,
        }
    }
    fn as_i64(&self) -> Option<i64> {
        match self {
            Plist::Int(i) => Some(*i),
            Plist::Real(f) => Some(*f as i64),
            _ => None,
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            Plist::Date(f) | Plist::Real(f) => Some(*f),
            Plist::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Plist::Str(s) => Some(s),
            _ => None,
        }
    }
}

fn normalise_itunes_path(raw: &str) -> String {
    let rest = raw.strip_prefix("file://").unwrap_or(raw);
    let rest = rest.strip_prefix("localhost/").map(|r| format!("/{r}")).unwrap_or(rest.to_string());
    let decoded = percent_decode(&rest);
    let trimmed = decoded.trim_start_matches('\\');
    let mut path = PathBuf::from(trimmed);
    path = crate::library::lexical_normpath(&path);
    path.to_string_lossy().to_lowercase()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) struct Itunes {
    collection: HashMap<String, Plist>,
    log: PluginLog,
    format_item: String,
}

impl Itunes {
    pub(crate) fn new(cfg: &Config) -> Result<Self> {
        // beets' `Itunes.__init__`: `config.add({"itunes": {"library":
        // "~/Music/iTunes/iTunes Library.xml"}})`.
        let library = cfg.str("metasync.itunes.library");
        let library = if library.is_empty() {
            "~/Music/iTunes/iTunes Library.xml".to_string()
        } else {
            library
        };
        let path = crate::config::expand_tilde(&library);
        let plist = match parse_plist(&path) {
            Ok(p) => p,
            Err(e) => {
                // beets: an OSError shows its strerror, other errors only
                // the generic message, plus a hint for a `.itl` library.
                let is_xml = path
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("xml"));
                let hint = if is_xml {
                    ""
                } else {
                    ": please ensure that the configured path points to the .XML library"
                };
                if let Some(io) = e.downcast_ref::<std::io::Error>() {
                    let text = io.to_string();
                    let text = text.split(" (os error").next().unwrap_or(&text);
                    anyhow::bail!("invalid iTunes library: {text}");
                }
                anyhow::bail!("invalid iTunes library{hint}");
            }
        };
        let mut collection = HashMap::new();
        if let Some(tracks) = plist.get("Tracks").and_then(|t| match t {
            Plist::Dict(m) => Some(m),
            _ => None,
        }) {
            for track in tracks.values() {
                if let Some(loc) = track.get("Location").and_then(Plist::as_str) {
                    collection.insert(normalise_itunes_path(loc), track.clone());
                }
            }
        }
        Ok(Itunes {
            collection,
            log: PluginLog::new("metasync"),
            format_item: cfg.str("format_item"),
        })
    }

    pub(crate) fn sync(&self, item: &mut Item) {
        let key = item.path().to_string_lossy().to_lowercase();
        let Some(t) = self.collection.get(&key) else {
            // beets: "no iTunes match found for {}", item (format_item).
            let label = crate::commands::format_record(
                &crate::template::Template::parse(&self.format_item),
                item,
                None,
            );
            self.log.warning(&format!("no iTunes match found for {label}"));
            return;
        };
        // beets assigns these unconditionally: a missing value clears the
        // attribute.
        for (key, field) in [
            ("Rating", "itunes_rating"),
            ("Play Count", "itunes_playcount"),
            ("Skip Count", "itunes_skipcount"),
        ] {
            match t.get(key).and_then(Plist::as_i64) {
                Some(v) => item.set(field, Value::Int(v)),
                None => {
                    item.remove_flex(field);
                }
            }
        }
        for (key, field) in [
            ("Play Date UTC", "itunes_lastplayed"),
            ("Skip Date", "itunes_lastskipped"),
            ("Date Added", "itunes_dateadded"),
        ] {
            if let Some(v) = t.get(key).and_then(Plist::as_f64) {
                item.set(field, Value::Float(v));
            }
        }
    }
}

// ------------------------------------------------------------------ amarok

/// The Amarok collection object: `org.kde.amarok` at `/Collection`
/// (beets' `dbus.SessionBus().get_object("org.kde.amarok", "/Collection")`).
pub(crate) struct Amarok {
    conn: zbus::blocking::Connection,
    log: PluginLog,
}

/// beets' `Amarok.query_xml`.
const AMAROK_QUERY_XML: &str = r#"
        <query version="1.0">
            <filters>
                <and><include field="filename" value={} /></and>
            </filters>
        </query>"#;

/// `xml.sax.saxutils.quoteattr`.
fn quoteattr(data: &str) -> String {
    let escaped = data.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    if escaped.contains('"') {
        if escaped.contains('\'') {
            format!("\"{}\"", escaped.replace('"', "&quot;"))
        } else {
            format!("'{escaped}'")
        }
    } else {
        format!("\"{escaped}\"")
    }
}

fn dbus_value(v: &OwnedValue) -> Option<DbusValue<'static>> {
    v.try_clone().ok().map(DbusValue::from)
}

fn dbus_str(v: &DbusValue<'_>) -> Option<String> {
    match v {
        DbusValue::Str(s) => Some(s.to_string()),
        DbusValue::ObjectPath(p) => Some(p.to_string()),
        _ => None,
    }
}

fn dbus_f64(v: &DbusValue<'_>) -> Option<f64> {
    Some(match v {
        DbusValue::U8(n) => f64::from(*n),
        DbusValue::I16(n) => f64::from(*n),
        DbusValue::U16(n) => f64::from(*n),
        DbusValue::I32(n) => f64::from(*n),
        DbusValue::U32(n) => f64::from(*n),
        DbusValue::I64(n) => *n as f64,
        DbusValue::U64(n) => *n as f64,
        DbusValue::F64(n) => *n,
        _ => return None,
    })
}

/// Every integer in a (possibly nested) D-Bus value (Amarok's dates are
/// `[[year, month, day], [hour, minute, second]]`).
fn dbus_ints(v: &DbusValue<'_>, out: &mut Vec<i64>) {
    match v {
        DbusValue::Array(a) => a.iter().for_each(|e| dbus_ints(e, out)),
        DbusValue::Structure(s) => s.fields().iter().for_each(|e| dbus_ints(e, out)),
        DbusValue::Value(inner) => dbus_ints(inner, out),
        other => {
            if let Some(n) = dbus_f64(other) {
                out.push(n as i64);
            }
        }
    }
}

impl Amarok {
    pub(crate) fn new(_cfg: &Config) -> Result<Self> {
        let conn = zbus::blocking::Connection::session()
            .context("failed to connect to the session bus")?;
        Ok(Amarok {
            conn,
            log: PluginLog::new("metasync"),
        })
    }

    #[cfg(test)]
    fn from_connection(conn: zbus::blocking::Connection) -> Self {
        Amarok {
            conn,
            log: PluginLog::new("metasync"),
        }
    }

    /// `collection.Query(xml)`: the matching tracks as D-Bus dictionaries.
    fn query(&self, xml: &str) -> Result<Vec<HashMap<String, OwnedValue>>> {
        // dbus-python's proxy finds the method by introspection; Amarok's
        // collection interface is `org.kde.amarok.Collection`, with older
        // builds exposing `Query` on `org.kde.amarok`.
        let mut last = None;
        for iface in ["org.kde.amarok.Collection", "org.kde.amarok"] {
            let proxy = zbus::blocking::Proxy::new(
                &self.conn,
                "org.kde.amarok",
                "/Collection",
                iface,
            )?;
            match proxy.call_method("Query", &(xml,)) {
                Ok(reply) => {
                    return Ok(reply.body().deserialize::<Vec<HashMap<String, OwnedValue>>>()?);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("an interface to try").into())
    }

    pub(crate) fn sync(&self, item: &mut Item) -> Result<()> {
        let path = item.path().to_string_lossy().into_owned();
        let name = item
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Amarok only searches for the file name; the full paths are in the
        // results, so match the right one here.
        let query = AMAROK_QUERY_XML.replace("{}", &quoteattr(&name));
        for result in self.query(&query)? {
            let get = |key: &str| result.get(key).and_then(dbus_value);
            if get("xesam:url").as_ref().and_then(dbus_str).as_deref() != Some(path.as_str()) {
                continue;
            }
            if let Some(n) = get("xesam:userRating").as_ref().and_then(dbus_f64) {
                item.set("amarok_rating", Value::Int(n as i64));
            }
            if let Some(f) = get("xesam:autoRating").as_ref().and_then(dbus_f64) {
                item.set("amarok_score", Value::Float(f));
            }
            if let Some(n) = get("xesam:useCount").as_ref().and_then(dbus_f64) {
                item.set("amarok_playcount", Value::Int(n as i64));
            }
            if let Some(id) = get("xesam:id").as_ref().and_then(dbus_str) {
                item.set(
                    "amarok_uid",
                    Value::Str(id.replace("amarok-sqltrackuid://", "")),
                );
            }
            let mut first = vec![];
            if let Some(v) = get("xesam:firstUsed") {
                dbus_ints(&v, &mut first);
            }
            if first.len() >= 6 && first[0] != 0 {
                let Some(first_played) = date_from_ints(&first) else {
                    continue;
                };
                let mut last = vec![];
                if let Some(v) = get("xesam:lastUsed") {
                    dbus_ints(&v, &mut last);
                }
                let last_played = if last.len() >= 6 && last[0] != 0 {
                    match date_from_ints(&last) {
                        Some(d) => d,
                        None => continue,
                    }
                } else {
                    first_played
                };
                item.set("amarok_firstplayed", Value::Float(mktime_local(first_played)));
                item.set("amarok_lastplayed", Value::Float(mktime_local(last_played)));
            } else {
                self.log.debug(&format!("no Amarok play dates for {name}"));
            }
        }
        Ok(())
    }
}

fn date_from_ints(v: &[i64]) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDate::from_ymd_opt(v[0] as i32, v[1] as u32, v[2] as u32)?
        .and_hms_opt(v[3] as u32, v[4] as u32, v[5] as u32)
}

fn mktime_local(d: chrono::NaiveDateTime) -> f64 {
    use chrono::TimeZone as _;
    chrono::Local
        .from_local_datetime(&d)
        .earliest()
        .map_or(0.0, |t| t.timestamp() as f64)
}

pub struct CmdOpts {
    pub pretend: bool,
    pub sources: Vec<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
    /// `-f`: beets sets the global `format_item` (the item labels here).
    pub format: Option<String>,
}

enum Source {
    Itunes(Itunes),
    Amarok(Amarok),
}

pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    // beets' `-f` callback (`ui._set_format`) sets the global
    // `format_item`, which `str(item)` in the sources' warnings and the
    // change display use.
    let mut owned = cfg.clone();
    if let Some(format) = &opts.format {
        owned.set("format_item", format.as_str().into());
    }
    let cfg = &owned;
    let log = PluginLog::new("metasync");
    let mut sources = vec![];
    for s in &opts.sources {
        sources.extend(s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from));
    }
    if sources.is_empty() {
        sources = cfg.list("metasync.source");
    }
    let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
    if let Some(n) = opts.limit {
        items.truncate(n);
    }
    if items.is_empty() {
        log.info("No items found matching query");
        return Ok(());
    }
    let mut loaded = vec![];
    for player in &sources {
        match player.as_str() {
            "itunes" => match Itunes::new(cfg) {
                Ok(s) => loaded.push(("itunes", Source::Itunes(s))),
                Err(e) => log.error(&format!(
                    "Failed to instantiate metadata source 'itunes': {e}"
                )),
            },
            "amarok" => match Amarok::new(cfg) {
                Ok(s) => loaded.push(("amarok", Source::Amarok(s))),
                Err(e) => log.error(&format!(
                    "Failed to instantiate metadata source 'amarok': {e}"
                )),
            },
            other => log.error(&format!("Unknown metadata source '{other}'")),
        }
    }
    if loaded.is_empty() {
        log.error("No valid metadata sources found");
        return Ok(());
    }
    for mut item in items {
        let old = item.clone();
        for (_, source) in &loaded {
            match source {
                Source::Itunes(s) => s.sync(&mut item),
                Source::Amarok(s) => s.sync(&mut item)?,
            }
        }
        let label = crate::commands::format_record(
            &crate::template::Template::parse(&cfg.str("format_item")),
            &item,
            Some(lib),
        );
        if crate::commands::show_model_changes(&label, &old, &item) && !opts.pretend {
            lib.store(&item)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "metasync_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "metasync",
        order: 74,
        // beets' plugin adds no settings itself: the iTunes library default
        // is added when that source is instantiated.
        defaults: r#""#,
        config_keys: &[],
        item_types: crate::plugins::metasync::ITEM_TYPES,
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "metasync",
            build: build_metasync,
            run: run_metasync,
        }],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(MetaSync::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct MetasyncArgs {
    /// Show all changes but do nothing
    #[arg(short = 'p', long)]
    pretend: bool,
    /// Comma-separated list of sources to sync (repeatable)
    #[arg(short = 's', long = "source", value_name = "SOURCES", action = clap::ArgAction::Append)]
    sources: Vec<String>,
    /// Print with custom format (unused)
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_metasync() -> clap::Command {
    use clap::Args as _;
    MetasyncArgs::augment_args(
        clap::Command::new("metasync")
            .about("update metadata from music player libraries"),
    )
}

pub fn run_metasync(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = MetasyncArgs::from_arg_matches(m)?;
    crate::plugins::metasync::command(
        cfg,
        lib,
        &crate::plugins::metasync::CmdOpts {
            pretend: a.pretend,
            sources: a.sources,
            limit: a.limit,
            query: a.query,
            format: a.format,
        },
    )
}
