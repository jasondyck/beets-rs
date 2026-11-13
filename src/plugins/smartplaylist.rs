//! A port of beets 2.14's `smartplaylist` plugin
//! (`beetsplug/smartplaylist.py`): `beet splupdate` writes M3U (or
//! extended M3U) playlists of the items matching each configured
//! playlist's `query` and/or `album_query`.
//!
//! beets' `auto` (regenerating at the end of any command that changed a
//! matching item, through `database_change` and `cli_exit`) never fires in
//! 2.14: its listener only looks at playlists `splupdate` has built, so it
//! isn't wired up here either.

use crate::config::Config;
use crate::formatted::Formatted;
use crate::library::Library;
use crate::model::{FieldType, Item, Kind};
use crate::plugins::PluginLog;
use crate::query::{Obj, PyVal, Query};
use crate::template::Template;
use anyhow::{Result, bail};
use serde_yaml::Value as Y;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The plugin, once its `output` setting checks out.
pub struct SmartPlaylist;

impl SmartPlaylist {
    /// The plugin's `__init__`: `output` must be `m3u` or `extm3u`.
    pub fn new(cfg: &Config) -> Result<Self> {
        let out = cfg.str("smartplaylist.output");
        if out != "m3u" && out != "extm3u" {
            bail!(
                "smartplaylist.output: must be one of ['m3u', 'extm3u'], not {}",
                py_str_repr(&out)
            );
        }
        Ok(SmartPlaylist)
    }
}

impl crate::plugins::Plugin for SmartPlaylist {
    fn name(&self) -> &'static str {
        "smartplaylist"
    }
}

/// `beet splupdate`'s options; each defaults to its setting.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    pub pretend: bool,
    pub format: Option<String>,
    pub playlist_dir: Option<String>,
    pub dest_regen: bool,
    pub relative_to: Option<String>,
    pub prefix: Option<String>,
    pub forward_slash: bool,
    pub urlencode: bool,
    pub uri_format: Option<String>,
    pub output: Option<String>,
    pub names: Vec<String>,
}

/// A playlist's `query` or `album_query`: none, one query, or several
/// (each with its own sort).
#[derive(Debug, Clone)]
enum PlQuery {
    None,
    One(Query),
    Many(Vec<Query>),
}

impl PlQuery {
    fn queries(&self) -> Vec<&Query> {
        match self {
            PlQuery::None => vec![],
            PlQuery::One(q) => vec![q],
            PlQuery::Many(qs) => qs.iter().collect(),
        }
    }
}

#[derive(Debug, Clone)]
struct Playlist {
    name: String,
    query: PlQuery,
    album_query: PlQuery,
}

/// beets' `parse_query_string`: shell-split, then parsed with its sort.
/// An invalid query's error is beets' `ParsingError` message (without the
/// `'QUERY': ` that `invalid query:` errors start with).
fn parse_query_string(kind: Kind, s: &str) -> Result<Query> {
    let parts = crate::plugins::duplicates::shlex_split(s)?;
    Query::parse(kind, &parts).map_err(|e| {
        match e.downcast_ref::<crate::query::InvalidQueryError>() {
            Some(q) => {
                let prefix = format!("'{}': ", parts.join(" "));
                anyhow::anyhow!("{}", q.0.strip_prefix(&prefix).unwrap_or(&q.0))
            }
            None => e,
        }
    })
}

/// `_parse_one_query`.
fn parse_one(kind: Kind, v: Option<&Y>) -> Result<PlQuery> {
    Ok(match v {
        None | Some(Y::Null) => PlQuery::None,
        Some(Y::Sequence(qs)) if qs.len() == 1 => {
            PlQuery::One(parse_query_string(kind, &scalar(&qs[0]))?)
        }
        Some(Y::Sequence(qs)) => PlQuery::Many(
            qs.iter()
                .map(|q| parse_query_string(kind, &scalar(q)))
                .collect::<Result<_>>()?,
        ),
        Some(q) => PlQuery::One(parse_query_string(kind, &scalar(q))?),
    })
}

fn scalar(v: &Y) -> String {
    match v {
        Y::String(s) => s.clone(),
        Y::Null => String::new(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// `build_queries`: the configured playlists (a nameless one or one with
/// an invalid query is left out, with a warning).
fn build_queries(cfg: &Config, log: &PluginLog) -> Vec<Playlist> {
    let mut out = vec![];
    let Some(Y::Sequence(pls)) = cfg.get("smartplaylist.playlists") else {
        return out;
    };
    for pl in pls {
        let get = |k: &str| pl.as_mapping().and_then(|m| m.get(Y::String(k.into())));
        let Some(name) = get("name") else {
            log.warning("playlist configuration is missing name");
            continue;
        };
        let name = scalar(name);
        let parsed = parse_one(Kind::Item, get("query"))
            .and_then(|q| Ok((q, parse_one(Kind::Album, get("album_query"))?)));
        match parsed {
            Ok((query, album_query)) => out.push(Playlist {
                name,
                query,
                album_query,
            }),
            Err(e) => log.warning(&format!("invalid query in playlist {name}: {e:#}")),
        }
    }
    out
}

/// Python's `shlex.quote`.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if s.chars().all(safe) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Python's `repr()` of a string.
fn py_str_repr(s: &str) -> String {
    crate::hooks::py_repr(s)
}

/// Python's `repr()` of a `bytes` value.
fn py_bytes_repr(b: &[u8]) -> String {
    let q = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{q}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c == q as u8 => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(q);
    out
}

/// Python's `str()` of a model value (`item[key]`).
fn py_str(v: &PyVal) -> String {
    match v {
        PyVal::None => "None".into(),
        PyVal::Bool(b) => if *b { "True" } else { "False" }.into(),
        PyVal::Int(i) => i.to_string(),
        PyVal::Float(f) => crate::hooks::py_float(*f),
        PyVal::Str(s) => s.clone(),
        PyVal::Bytes(b) => py_bytes_repr(b),
        PyVal::List(l) => l.join("; "),
    }
}

/// `urllib.parse.quote(s, safe)` of UTF-8 (or raw) bytes.
fn quote(b: &[u8], safe: &str) -> String {
    let mut out = String::new();
    for &c in b {
        if c.is_ascii_alphanumeric() || b"_.-~".contains(&c) || safe.as_bytes().contains(&c) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// `os.path.relpath(path, start)` for absolute paths.
fn relpath(path: &Path, start: &Path) -> PathBuf {
    let (path, start) = (
        crate::library::lexical_normpath(path),
        crate::library::lexical_normpath(start),
    );
    let p: Vec<_> = path.components().collect();
    let s: Vec<_> = start.components().collect();
    let common = p.iter().zip(&s).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..s.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// The settings, with `splupdate`'s options over them (beets'
/// `config.set(vars(opts))`).
struct Settings {
    pretend: bool,
    format: String,
    playlist_dir: PathBuf,
    dest_regen: bool,
    relative_to: Option<PathBuf>,
    prefix: Vec<u8>,
    forward_slash: bool,
    urlencode: bool,
    uri_format: Option<String>,
    extm3u: bool,
    fields: Vec<String>,
}

impl Settings {
    fn new(cfg: &Config, o: &CmdOpts) -> Settings {
        let s = |k: &str| cfg.str(&format!("smartplaylist.{k}"));
        let opt_s = |k: &str| match cfg.get(&format!("smartplaylist.{k}")) {
            None | Some(Y::Null) => None,
            Some(_) => Some(s(k)).filter(|v| !v.is_empty()),
        };
        let b = |k: &str| cfg.bool(&format!("smartplaylist.{k}"));
        let dir = o.playlist_dir.clone().unwrap_or_else(|| s("playlist_dir"));
        Settings {
            // The setting is replaced by the option (unset: off).
            pretend: o.pretend,
            format: o.format.clone().unwrap_or_else(|| s("format")),
            // `as_filename`: relative to the working directory.
            playlist_dir: crate::query::normpath(Path::new(&dir)),
            dest_regen: o.dest_regen || b("dest_regen"),
            relative_to: o
                .relative_to
                .clone()
                .filter(|r| !r.is_empty())
                .or_else(|| opt_s("relative_to"))
                .map(|r| crate::query::normpath(Path::new(&r))),
            prefix: o.prefix.clone().unwrap_or_else(|| s("prefix")).into_bytes(),
            forward_slash: o.forward_slash || b("forward_slash"),
            urlencode: o.urlencode || b("urlencode"),
            uri_format: o.uri_format.clone().or_else(|| opt_s("uri_format")),
            extm3u: o.output.clone().unwrap_or_else(|| s("output")) == "extm3u",
            fields: cfg.list("smartplaylist.fields"),
        }
    }
}

/// `beet splupdate`, printing to stdout.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    // A plugin that failed to load has no command.
    if SmartPlaylist::new(cfg).is_err() {
        bail!("unknown command 'splupdate'");
    }
    let log = PluginLog::new("smartplaylist");
    let playlists = build_queries(cfg, &log);
    let matched: Vec<Playlist> = if opts.names.is_empty() {
        playlists
    } else {
        let mut wanted: HashSet<String> = opts.names.iter().cloned().collect();
        for a in &opts.names {
            if !a.ends_with(".m3u") {
                wanted.insert(format!("{a}.m3u"));
            }
        }
        let (m, unmatched): (Vec<_>, Vec<_>) = playlists
            .into_iter()
            .partition(|p| wanted.contains(&p.name));
        if m.is_empty() {
            let mut names: Vec<&str> = unmatched.iter().map(|p| p.name.as_str()).collect();
            names.sort_unstable();
            let quoted: Vec<String> = names.iter().map(|n| shell_quote(n)).collect();
            bail!("No playlist matching any of {} found", quoted.join(" "));
        }
        m
    };
    let s = Settings::new(cfg, opts);
    update_playlists(cfg, lib, &s, &matched, &log)
}

/// `get_playlist_items`: the items of the item queries, then of the
/// album queries' albums, each once, in that order.
fn playlist_items(lib: &Library, pl: &Playlist) -> Result<Vec<Item>> {
    let mut items = vec![];
    for q in pl.query.queries() {
        items.extend(lib.items(q)?);
    }
    let mut seen_albums = HashSet::new();
    for q in pl.album_query.queries() {
        for a in lib.albums(q)? {
            let id = a.id.unwrap_or(0);
            if seen_albums.insert(id) {
                let q = Query::parse(Kind::Item, &[format!("album_id:{id}")])?;
                items.extend(lib.items(&q)?);
            }
        }
    }
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert(i.id));
    Ok(items)
}

/// `get_item_uri`.
fn item_uri(cfg: &Config, lib: &Library, s: &Settings, item: &Item) -> Result<Vec<u8>> {
    if let Some(f) = &s.uri_format {
        return Ok(f
            .replace("$id", &item.id.unwrap_or(0).to_string())
            .into_bytes());
    }
    let mut uri = item.path().to_path_buf();
    if s.dest_regen {
        let album = lib.get_album(item.int("album_id"))?;
        uri = crate::paths::PathFormatter::new(cfg)?.destination(item, album.as_ref(), Some(lib));
    }
    if let Some(r) = &s.relative_to {
        uri = relpath(&uri, r);
    }
    let mut bytes = uri.as_os_str().as_bytes().to_vec();
    if s.forward_slash {
        bytes = bytes
            .into_iter()
            .map(|c| if c == b'\\' { b'/' } else { c })
            .collect();
    }
    if s.urlencode {
        // `pathname2url`.
        let mut p = bytes.clone();
        if p.starts_with(b"//") {
            p.splice(0..0, b"//".iter().copied());
        }
        bytes = quote(&p, "/").into_bytes();
    }
    let mut out = s.prefix.clone();
    out.extend(bytes);
    Ok(out)
}

/// `PlaylistItem.get_comment`: the `#EXTINF` line (extended M3U) and the
/// URI line.
fn entry(lib: &Library, s: &Settings, item: &Item, uri: &[u8]) -> Result<Vec<u8>> {
    let mut out = vec![];
    if s.extm3u {
        let album = lib.get_album(item.int("album_id"))?;
        let obj = Obj {
            rec: item,
            album: album.as_ref(),
            album_dir: None,
            lib: Some(lib),
            album_counts: None,
        };
        let mut attrs = String::new();
        for k in &s.fields {
            let Some(v) = obj.get(k) else {
                // beets fails here (`KeyError`), mid-file.
                bail!("KeyError: {}", py_str_repr(k));
            };
            // A fixed boolean field reads back from SQLite as an int.
            let v = match (v, crate::model::field_type(Kind::Item, k)) {
                (PyVal::Bool(b), Some(FieldType::Bool)) => PyVal::Int(i64::from(b)),
                (v, _) => v,
            };
            attrs.push_str(&format!(" {k}=\"{}\"", quote(py_str(&v).as_bytes(), "/:")));
        }
        out.extend(
            format!(
                "#EXTINF:{}{attrs},{} - {}\n",
                item.float("length").trunc() as i64,
                item.str("artist"),
                item.str("title")
            )
            .into_bytes(),
        );
    }
    out.extend_from_slice(uri);
    out.push(b'\n');
    Ok(out)
}

/// One playlist line: the item and its URI.
type Entry = (Item, Vec<u8>);

/// `update_playlists`.
fn update_playlists(
    cfg: &Config,
    lib: &Library,
    s: &Settings,
    playlists: &[Playlist],
    log: &PluginLog,
) -> Result<()> {
    let n = playlists.len();
    log.info(&format!("Updating {n} smart playlists..."));
    // Playlist file names, in order, with their URIs.
    let mut m3us: Vec<(String, Vec<Entry>)> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut seen: HashMap<String, HashSet<Vec<u8>>> = HashMap::new();
    let env = crate::formatted::env();
    let fmt = Template::parse(&s.format);
    for pl in playlists {
        let name_t = Template::parse(&pl.name);
        let mut matched: Vec<Item> = vec![];
        for item in playlist_items(lib, pl)? {
            let raw = name_t.render(&Formatted::new(&item, Some(lib)).for_path());
            let m3u = env.sanitize_path(&raw, true);
            let uri = item_uri(cfg, lib, s, &item)?;
            if seen.entry(m3u.clone()).or_default().insert(uri.clone()) {
                let i = *index.entry(m3u.clone()).or_insert_with(|| {
                    m3us.push((m3u.clone(), vec![]));
                    m3us.len() - 1
                });
                m3us[i].1.push((item.clone(), uri));
                matched.push(item);
            }
        }
        log.info(&format!(
            "Creating playlist {}: {} tracks.",
            pl.name,
            matched.len()
        ));
        for item in &matched {
            log.debug(&crate::commands::format_record(&fmt, item, Some(lib)));
        }
    }
    if s.pretend {
        log.info(&format!("{n} playlists would be updated"));
        return Ok(());
    }
    for (m3u, entries) in &m3us {
        let path = crate::library::lexical_normpath(&s.playlist_dir.join(m3u));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path)?);
        if s.extm3u {
            f.write_all(b"#EXTM3U\n")?;
        }
        for (item, uri) in entries {
            // What was written stays written if an entry fails, as in beets.
            let e = entry(lib, s, item, uri);
            match e {
                Ok(bytes) => f.write_all(&bytes)?,
                Err(err) => {
                    f.flush()?;
                    return Err(err);
                }
            }
        }
        f.flush()?;
    }
    log.info(&format!("{n} playlists updated"));
    Ok(())
}

#[cfg(test)]
#[path = "smartplaylist_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "smartplaylist",
        order: 22,
        defaults: r#"smartplaylist:
    dest_regen: no
    relative_to:
    playlist_dir: .
    auto: yes
    playlists: []
    uri_format:
    fields: []
    forward_slash: no
    prefix: ''
    urlencode: no
    format: $artist - $title
    output: m3u
    pretend: no
"#,
        config_keys: &["smartplaylist"],
        item_types: &[],
        album_types: &[],
        redacted: &["smartplaylist.prefix"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "smartplaylist",
                build: build_splupdate,
                run: run_splupdate,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(SmartPlaylist::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct SplupdateArgs {
    /// Display query results but don't write playlist files.
    #[arg(short = 'p', long)]
    pretend: bool,
    /// Print per-track log lines with custom format
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Directory to write the generated playlist files to.
    #[arg(short = 'd', long = "playlist-dir", value_name = "PATH")]
    playlist_dir: Option<String>,
    /// Regenerate the destination path as 'move' or 'convert' commands would do.
    #[arg(long = "dest-regen")]
    dest_regen: bool,
    /// Generate playlist item paths relative to this path.
    #[arg(long = "relative-to", value_name = "PATH")]
    relative_to: Option<String>,
    /// Prepend string to every path in the playlist file.
    #[arg(long)]
    prefix: Option<String>,
    /// Force forward slash in paths within playlists.
    #[arg(long = "forward-slash")]
    forward_slash: bool,
    /// URL-encode all paths.
    #[arg(long)]
    urlencode: bool,
    /// Playlist item URI template, e.g. <http://beets:8337/item/$id/file>.
    #[arg(long = "uri-format")]
    uri_format: Option<String>,
    /// Specify the playlist format: m3u|extm3u.
    #[arg(long, value_parser = ["m3u", "extm3u"])]
    output: Option<String>,
    names: Vec<String>,
}

pub fn build_splupdate() -> clap::Command {
    use clap::Args as _;
    SplupdateArgs::augment_args(
        clap::Command::new("splupdate").about("update the smart playlists. Playlist names may be passed as arguments."),
    )
}

pub fn run_splupdate(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = SplupdateArgs::from_arg_matches(m)?;
    crate::plugins::smartplaylist::command(
        cfg,
        lib,
        &crate::plugins::smartplaylist::CmdOpts {
            pretend: a.pretend,
            format: a.format,
            playlist_dir: a.playlist_dir,
            dest_regen: a.dest_regen,
            relative_to: a.relative_to,
            prefix: a.prefix,
            forward_slash: a.forward_slash,
            urlencode: a.urlencode,
            uri_format: a.uri_format,
            output: a.output,
            names: a.names,
        },
    )
}
