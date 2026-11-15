//! The library worker: one thread owns the beets-rs `Library` (its
//! configuration and query context are thread-local) and answers the UI's
//! requests over channels. Everything that changes the library goes through
//! the `beet` command line instead (see `runner`), so the GUI gets exactly
//! the CLI's behaviour: moves, tag writes, plugin events.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use beets_rs::commands::{Globals, load_config, open_library};
use beets_rs::library::Library;
use beets_rs::model::{FieldType, Kind, Record};
use beets_rs::query::Query;

/// The command line's global options, shared by the worker and every `beet`
/// run the GUI starts.
#[derive(Clone, Default, Debug)]
pub struct Opts {
    pub library: Option<PathBuf>,
    pub directory: Option<PathBuf>,
    pub config: Vec<PathBuf>,
}

impl Opts {
    pub fn globals(&self) -> Globals {
        Globals {
            format_item: None,
            format_album: None,
            library: self.library.clone(),
            directory: self.directory.clone(),
            verbose: 0,
            config: self.config.clone(),
            plugins: None,
            disable_plugins: None,
            version: false,
        }
    }

    /// The same options as `beet` arguments (they go before the command).
    pub fn cli_args(&self) -> Vec<OsString> {
        let mut out = vec![];
        for c in &self.config {
            out.push("-c".into());
            out.push(c.into());
        }
        if let Some(l) = &self.library {
            out.push("-l".into());
            out.push(l.into());
        }
        if let Some(d) = &self.directory {
            out.push("-d".into());
            out.push(d.into());
        }
        out
    }
}

pub enum Req {
    /// (Re)load the configuration and reopen the library.
    Open,
    List {
        generation: u64,
        kind: Kind,
        query: String,
    },
    Detail {
        generation: u64,
        kind: Kind,
        id: i64,
    },
}

pub enum Resp {
    Opened(Result<LibInfo, String>),
    Rows {
        generation: u64,
        kind: Kind,
        rows: Result<Vec<Row>, String>,
        /// The library's (items, albums) totals at the same moment.
        counts: Option<(i64, i64)>,
    },
    Detail {
        generation: u64,
        detail: Result<Detail, String>,
    },
}

#[derive(Clone, Debug)]
pub struct LibInfo {
    pub library: PathBuf,
    pub directory: PathBuf,
    pub items: i64,
    pub albums: i64,
    pub user_config: PathBuf,
    pub config_sources: Vec<PathBuf>,
    pub plugins: Vec<String>,
}

/// One line of the album or track list.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub track: String,
    pub length: String,
    pub format: String,
    pub genre: String,
    /// Tracks in the album (albums) / the album's id (tracks).
    pub count: i64,
    pub art: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub key: String,
    pub display: String,
    /// The text to edit (None: computed or file-derived, not editable).
    pub edit: Option<String>,
    pub flex: bool,
}

#[derive(Clone, Debug)]
pub struct Detail {
    pub kind: Kind,
    pub id: i64,
    pub title: String,
    pub subtitle: String,
    pub fields: Vec<Field>,
    /// An album's tracks.
    pub tracks: Vec<Row>,
    pub art: Option<PathBuf>,
    /// The file (track) or directory (album).
    pub path: Option<PathBuf>,
}

pub struct Backend {
    pub tx: flume::Sender<Req>,
    pub rx: flume::Receiver<Resp>,
}

pub fn spawn(opts: Opts) -> Backend {
    let (req_tx, req_rx) = flume::unbounded::<Req>();
    let (resp_tx, resp_rx) = flume::unbounded::<Resp>();
    std::thread::Builder::new()
        .name("library".into())
        .spawn(move || worker(opts, req_rx, resp_tx))
        .expect("spawn the library thread");
    Backend {
        tx: req_tx,
        rx: resp_rx,
    }
}

/// The generation of a list request made only for the totals that come
/// with every list response.
pub const TOTALS_ONLY: u64 = u64::MAX;

fn worker(opts: Opts, rx: flume::Receiver<Req>, tx: flume::Sender<Resp>) {
    let mut lib: Option<Library> = None;
    while let Ok(mut req) = rx.recv() {
        // Coalesce: when the user types fast only the newest list request
        // matters.
        if matches!(req, Req::List { .. }) {
            while let Ok(next) = rx.try_recv() {
                match next {
                    // Any list response carries the totals: drop a
                    // totals-only request rather than a real one.
                    Req::List {
                        generation: TOTALS_ONLY,
                        ..
                    } => {}
                    Req::List { .. } => req = next,
                    other => {
                        handle(&opts, &mut lib, req, &tx);
                        req = other;
                    }
                }
            }
        }
        handle(&opts, &mut lib, req, &tx);
    }
}

fn handle(opts: &Opts, lib: &mut Option<Library>, req: Req, tx: &flume::Sender<Resp>) {
    let resp = match req {
        Req::Open => {
            *lib = None;
            let result = open(opts).map(|(l, info)| {
                *lib = Some(l);
                info
            });
            Resp::Opened(result.map_err(|e| format!("{e:#}")))
        }
        Req::List {
            generation,
            kind,
            query,
        } => Resp::Rows {
            generation,
            kind,
            rows: match lib {
                Some(lib) => list(lib, kind, &query).map_err(|e| format!("{e:#}")),
                None => Err("the library is not open".into()),
            },
            counts: lib.as_ref().and_then(|l| l.counts().ok()),
        },
        Req::Detail {
            generation,
            kind,
            id,
        } => Resp::Detail {
            generation,
            detail: match lib {
                Some(lib) => detail(lib, kind, id).map_err(|e| format!("{e:#}")),
                None => Err("the library is not open".into()),
            },
        },
    };
    tx.send(resp).ok();
}

fn open(opts: &Opts) -> Result<(Library, LibInfo)> {
    let cfg = load_config(&opts.globals())?;
    let (lib, plugins) = open_library(&cfg)?;
    let (items, albums) = lib.counts()?;
    let info = LibInfo {
        library: cfg.library_path(),
        directory: cfg.directory(),
        items,
        albums,
        user_config: cfg.config_dir.join("config.yaml"),
        config_sources: cfg.source_paths(),
        plugins: plugins.iter().map(|p| p.name().to_string()).collect(),
    };
    Ok((lib, info))
}

/// Split a query string as a shell would (quotes group words), so that
/// `artist:"the beatles"` is one term.
pub fn split_words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut any = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    any = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn list(lib: &Library, kind: Kind, query: &str) -> Result<Vec<Row>> {
    let q = Query::parse(kind, &split_words(query))?;
    Ok(match kind {
        Kind::Album => {
            let mut counts: HashMap<i64, i64> = HashMap::new();
            for v in lib.query_column("SELECT album_id FROM items WHERE album_id IS NOT NULL")? {
                if let Some(id) = sql_int(&v) {
                    *counts.entry(id).or_default() += 1;
                }
            }
            lib.albums(&q)?
                .iter()
                .map(|a| album_row(a, counts.get(&a.id.unwrap_or(0)).copied().unwrap_or(0)))
                .collect()
        }
        Kind::Item => lib.items(&q)?.iter().map(item_row).collect(),
    })
}

fn sql_int(v: &rusqlite::types::Value) -> Option<i64> {
    use rusqlite::types::Value as SqlValue;
    match v {
        SqlValue::Integer(i) => Some(*i),
        SqlValue::Real(f) => Some(*f as i64),
        SqlValue::Text(s) => s.parse().ok(),
        _ => None,
    }
}

fn fmt(r: &Record, key: &str) -> String {
    r.formatted(key).unwrap_or_default()
}

fn non_empty_path(p: &Path) -> Option<PathBuf> {
    (!p.as_os_str().is_empty()).then(|| p.to_path_buf())
}

fn album_row(a: &Record, count: i64) -> Row {
    Row {
        id: a.id.unwrap_or(0),
        title: fmt(a, "album"),
        artist: fmt(a, "albumartist"),
        album: fmt(a, "album"),
        year: year_of(a),
        track: String::new(),
        length: String::new(),
        format: String::new(),
        genre: fmt(a, "genre"),
        count,
        art: non_empty_path(a.path_field("artpath")),
    }
}

fn year_of(r: &Record) -> String {
    match r.int("year") {
        0 => String::new(),
        y => y.to_string(),
    }
}

fn item_row(i: &Record) -> Row {
    Row {
        id: i.id.unwrap_or(0),
        title: fmt(i, "title"),
        artist: fmt(i, "artist"),
        album: fmt(i, "album"),
        year: year_of(i),
        track: match i.int("track") {
            0 => String::new(),
            t => match i.int("disctotal") {
                n if n > 1 => format!("{}-{t:02}", i.int("disc")),
                _ => format!("{t:02}"),
            },
        },
        length: fmt(i, "length"),
        format: fmt(i, "format"),
        genre: fmt(i, "genre"),
        count: i.int("album_id"),
        art: None,
    }
}

/// Fields that come from the file or the database, not from the user.
const READ_ONLY: &[&str] = &[
    "id",
    "album_id",
    "path",
    "artpath",
    "mtime",
    "added",
    "format",
    "bitrate",
    "bitrate_mode",
    "encoder_info",
    "encoder_settings",
    "samplerate",
    "bitdepth",
    "channels",
    "length",
    "filesize",
    "singleton",
];

fn fields(r: &Record) -> Vec<Field> {
    r.keys()
        .into_iter()
        .map(|key| {
            let ty = r.field_type(&key);
            let flex = r.flex.contains_key(&key);
            let display = if key == "id" {
                r.id.unwrap_or(0).to_string()
            } else {
                fmt(r, &key)
            };
            let editable = !READ_ONLY.contains(&key.as_str())
                && !matches!(
                    ty,
                    Some(
                        FieldType::Path
                            | FieldType::Date
                            | FieldType::Duration
                            | FieldType::Bitrate
                            | FieldType::SampleRate
                    )
                );
            let edit = editable.then(|| match ty {
                Some(FieldType::Float(_) | FieldType::NullFloat) => {
                    r.get(&key).map(|v| v.raw()).unwrap_or_default()
                }
                _ => display.clone(),
            });
            Field {
                key,
                display,
                edit,
                flex,
            }
        })
        .collect()
}

/// Cover art for an album: its `artpath`, else an image in its directory.
fn find_art(artpath: Option<PathBuf>, dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = artpath.filter(|p| p.is_file()) {
        return Some(p);
    }
    let dir = dir?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp"
        ) {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let rank = ["cover", "folder", "front", "album", "art"]
            .iter()
            .position(|w| stem.contains(w))
            .unwrap_or(9);
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, path));
        }
    }
    best.map(|(_, p)| p)
}

fn detail(lib: &Library, kind: Kind, id: i64) -> Result<Detail> {
    match kind {
        Kind::Album => {
            let a = lib
                .get_album(id)?
                .ok_or_else(|| anyhow::anyhow!("album {id} is gone"))?;
            let items = lib.album_items(id)?;
            let dir = items
                .first()
                .and_then(|i| i.path().parent())
                .map(Path::to_path_buf);
            Ok(Detail {
                kind,
                id,
                title: fmt(&a, "album"),
                subtitle: [fmt(&a, "albumartist"), year_of(&a)]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                fields: fields(&a),
                tracks: items.iter().map(item_row).collect(),
                art: find_art(non_empty_path(a.path_field("artpath")), dir.as_deref()),
                path: dir,
            })
        }
        Kind::Item => {
            let i = lib
                .get_item(id)?
                .ok_or_else(|| anyhow::anyhow!("track {id} is gone"))?;
            let album_art = match i.int("album_id") {
                0 => None,
                aid => lib
                    .get_album(aid)?
                    .and_then(|a| non_empty_path(a.path_field("artpath"))),
            };
            Ok(Detail {
                kind,
                id,
                title: fmt(&i, "title"),
                subtitle: [fmt(&i, "artist"), fmt(&i, "album"), year_of(&i)]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                fields: fields(&i),
                tracks: vec![],
                art: find_art(album_art, i.path().parent()),
                path: Some(i.path().to_path_buf()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_words;

    #[test]
    fn splits_like_a_shell() {
        assert_eq!(
            split_words(r#"artist:"the beatles" year:1969"#),
            ["artist:the beatles", "year:1969"]
        );
        assert_eq!(split_words("  a  'b c' \"\" "), ["a", "b c", ""]);
        assert_eq!(split_words(r"a\ b"), ["a b"]);
        assert!(split_words("   ").is_empty());
    }
}
