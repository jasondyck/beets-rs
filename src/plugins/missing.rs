//! A port of beets 2.14's `missing` plugin (`beetsplug/missing.py`):
//! `beet missing` lists the tracks missing from albums (looked up on
//! MusicBrainz), counts them (`-c`, `-t`), or lists an artist's release
//! groups missing from the library (`-a`). It also gives albums the
//! `$missing` template field (in `formatted`).

use crate::config::Config;
use crate::formatted::Formatted;
use crate::library::Library;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo};
use crate::model::{Album, Item, Kind, Record, Value};
use crate::plugins::PluginLog;
use crate::query::Query;
use crate::template::Template;
use anyhow::Result;
use std::io::Write;

/// The albums whose album artist has a MusicBrainz id.
const MB_ARTIST_QUERY: &str = r"mb_albumartistid::^\w{8}-\w{4}-\w{4}-\w{4}-\w{12}$";

/// The plugin: a command and the `$missing` album field.
pub struct MissingPlugin;

impl crate::plugins::Plugin for MissingPlugin {
    fn name(&self) -> &'static str {
        "missing"
    }
}

/// `beet missing`'s options (flags not given leave the settings alone, as
/// beets' `config.set_args`).
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub count: bool,
    pub total: bool,
    pub album: bool,
    pub release_types: Vec<String>,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// `beet missing`, printing to stdout.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    run(cfg, lib, opts, &mut std::io::stdout().lock())
}

/// The settings with the command's flags over them.
struct Settings {
    count: bool,
    total: bool,
    album: bool,
    release_types: Vec<String>,
    format_item: String,
    format_album: String,
}

impl Settings {
    fn new(cfg: &Config, opts: &CmdOpts) -> Settings {
        let raw = if opts.release_types.is_empty() {
            cfg.list("missing.release_types")
        } else {
            opts.release_types.clone()
        };
        // `-f` without an album option sets both formats.
        let (format_item, format_album) = match &opts.format {
            Some(f) => (f.clone(), f.clone()),
            None => (cfg.str("format_item"), cfg.str("format_album")),
        };
        Settings {
            count: opts.count || cfg.bool("missing.count"),
            total: opts.total || cfg.bool("missing.total"),
            album: opts.album || cfg.bool("missing.album"),
            release_types: raw
                .iter()
                .flat_map(|r| r.split(',').map(String::from))
                .collect(),
            format_item,
            format_album,
        }
    }
}

/// `beet missing`, writing to `out`.
pub fn run(cfg: &Config, lib: &Library, opts: CmdOpts, out: &mut impl Write) -> Result<()> {
    let s = Settings::new(cfg, &opts);
    let log = PluginLog::new("missing");
    if s.album {
        missing_albums(cfg, lib, &s, &opts, &log, out)
    } else {
        missing_tracks(cfg, lib, &s, &opts, &log, out)
    }
}

fn albums(lib: &Library, query: &[String], limit: Option<usize>) -> Result<Vec<Album>> {
    let mut albums = lib.albums(&Query::parse(Kind::Album, query)?)?;
    albums.truncate(limit.unwrap_or(usize::MAX));
    Ok(albums)
}

/// `album.albumtotal` (the computed field).
fn albumtotal(lib: &Library, album: &Album) -> i64 {
    Formatted::new(album, Some(lib))
        .get("albumtotal")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// `_missing_count`: the album's total less the tracks it has.
pub fn missing_count(lib: &Library, album: &Album, items: usize) -> i64 {
    albumtotal(lib, album) - items as i64
}

fn render(t: &Template, rec: &Record, lib: Option<&Library>) -> String {
    crate::commands::format_record(t, rec, lib)
}

/// `_missing_tracks`.
fn missing_tracks(
    cfg: &Config,
    lib: &Library,
    s: &Settings,
    opts: &CmdOpts,
    log: &PluginLog,
    out: &mut impl Write,
) -> Result<()> {
    let albums = albums(lib, &opts.query, opts.limit)?;
    let mut fmt = if s.count {
        s.format_album.clone()
    } else {
        s.format_item.clone()
    };
    if s.total {
        let mut sum = 0;
        for a in &albums {
            sum += missing_count(lib, a, lib.album_items(a.id.unwrap_or(0))?.len());
        }
        writeln!(out, "{sum}")?;
        return Ok(());
    }
    if s.count {
        fmt.push_str(": $missing");
    }
    let t = Template::parse(&fmt);
    let mut mb: Option<MusicBrainz> = None;
    for album in &albums {
        let items = lib.album_items(album.id.unwrap_or(0))?;
        if s.count {
            if missing_count(lib, album, items.len()) != 0 {
                writeln!(out, "{}", render(&t, album, Some(lib)))?;
            }
            continue;
        }
        for item in missing(cfg, lib, album, &items, &mut mb, log)? {
            // A new item outside the library: no album to reach.
            writeln!(out, "{}", render(&t, &item, None))?;
        }
    }
    Ok(())
}

/// `_missing`: the album's tracks on MusicBrainz that it doesn't have, as
/// items.
fn missing(
    cfg: &Config,
    lib: &Library,
    album: &Album,
    items: &[Item],
    mb: &mut Option<MusicBrainz>,
    log: &PluginLog,
) -> Result<Vec<Item>> {
    if items.len() as i64 == albumtotal(lib, album) {
        return Ok(vec![]);
    }
    let Some(first) = items.first() else {
        return Ok(vec![]);
    };
    // `album.get("data_source") or items[0].get("data_source", "MusicBrainz")`.
    let data_source = match album.flex.get("data_source").filter(|d| !d.is_empty()) {
        Some(d) => d.clone(),
        None => first
            .flex
            .get("data_source")
            .cloned()
            .unwrap_or_else(|| "MusicBrainz".into()),
    };
    // `metadata_plugins.album_for_id`: only a loaded source answers.
    if !data_source.eq_ignore_ascii_case("musicbrainz")
        || !crate::plugins::enabled(cfg, "musicbrainz")
    {
        return Ok(vec![]);
    }
    let mb = mb.get_or_insert_with(|| MusicBrainz::new(cfg));
    let info = match mb.album_for_id(album.str("mb_albumid")) {
        Ok(Some(info)) => info,
        Ok(None) => return Ok(vec![]),
        Err(e) if !cfg.bool("raise_on_error") => {
            crate::logging::emit(format!("Error in 'MusicBrainz.album_for_id': {e:#}"));
            return Ok(vec![]);
        }
        Err(e) => return Err(e),
    };
    let have: Vec<&str> = items.iter().map(|i| i.str("mb_trackid")).collect();
    let mut out = vec![];
    for t in &info.tracks {
        if !have.contains(&t.track_id.as_str()) {
            log.debug(&format!("track {} in album {}", t.track_id, info.album_id));
            out.push(item(t, &info, album.id));
        }
    }
    Ok(out)
}

/// `_item`: an item from the track and album info (only what MusicBrainz
/// gives).
pub fn item(t: &TrackInfo, a: &AlbumInfo, album_id: Option<i64>) -> Item {
    let mut it = Item::new_item();
    it.set_int("album_id", album_id.unwrap_or(0));
    let strs: [(&str, &str); 23] = [
        ("album", &a.album),
        ("albumartist", &a.artist),
        ("albumartist_credit", &a.artist_credit),
        ("albumartist_sort", &a.artist_sort),
        ("albumdisambig", &a.albumdisambig),
        ("albumstatus", &a.albumstatus),
        ("albumtype", &a.albumtype),
        ("artist", &t.artist),
        ("artist_credit", &t.artist_credit),
        ("artist_sort", &t.artist_sort),
        ("asin", &a.asin),
        ("catalognum", &a.catalognum),
        ("country", &a.country),
        ("disctitle", &t.disctitle),
        ("label", &a.label),
        ("language", &a.language),
        ("mb_albumid", &a.album_id),
        ("mb_artistid", &t.artist_id),
        ("mb_releasegroupid", &a.releasegroup_id),
        ("mb_trackid", &t.track_id),
        ("media", &t.media),
        ("script", &a.script),
        ("title", &t.title),
    ];
    for (k, v) in strs {
        it.set_str(k, v);
    }
    let ints: [(&str, i64); 7] = [
        ("day", a.day),
        ("disc", t.medium as i64),
        ("disctotal", a.mediums as i64),
        ("month", a.month),
        ("track", t.index as i64),
        ("tracktotal", a.tracks.len() as i64),
        ("year", a.year),
    ];
    for (k, v) in ints {
        it.set_int(k, v);
    }
    it.set("comp", Value::Bool(a.va));
    it.set("length", Value::Float(t.length.unwrap_or(0.0)));
    it
}

/// `_missing_albums`.
fn missing_albums(
    cfg: &Config,
    lib: &Library,
    s: &Settings,
    opts: &CmdOpts,
    log: &PluginLog,
    out: &mut impl Write,
) -> Result<()> {
    let mut query = opts.query.clone();
    query.push(MB_ARTIST_QUERY.into());
    // (album artist, its id) to the release groups the library has, in
    // the order the artists come.
    let mut by_artist: Vec<((String, String), Vec<String>)> = vec![];
    for album in albums(lib, &query, opts.limit)? {
        let key = (
            album.str("albumartist").to_string(),
            album.str("mb_albumartistid").to_string(),
        );
        let rg = album.str("mb_releasegroupid").to_string();
        match by_artist.iter_mut().find(|(k, _)| *k == key) {
            Some((_, ids)) => ids.push(rg),
            None => by_artist.push((key, vec![rg])),
        }
    }
    let types = s.release_types.join("|");
    let t = Template::parse(&s.format_album);
    let mut total = 0;
    let mb = (!by_artist.is_empty()).then(|| MusicBrainz::new(cfg));
    for ((artist, artist_id), ids) in &by_artist {
        let mb = mb.as_ref().expect("made for any artist");
        let groups = match mb.browse_release_groups(artist_id, &types) {
            Ok(g) => g,
            Err(e) => {
                // beets logs the traceback (`exc_info`); its last line.
                let last = if e.is::<crate::mb::NotFound>() {
                    "beetsplug._utils.requests.HTTPNotFoundError: HTTP Error: 404 Not Found."
                        .to_string()
                } else {
                    format!("{e:#}")
                };
                log.info(&format!(
                    "Couldn't fetch info for artist '{artist}' ({artist_id})\n{last}"
                ));
                continue;
            }
        };
        let field = |rg: &serde_json::Value, k: &str| {
            rg.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        for rg in groups.iter().filter(|rg| !ids.contains(&field(rg, "id"))) {
            if s.total {
                total += 1;
                continue;
            }
            let mut a = Album::new_album();
            a.set_str("albumartist", artist.clone());
            a.set_str("album", field(rg, "title"));
            a.set_str("mb_releasegroupid", field(rg, "id"));
            a.set_str("albumtype", field(rg, "primary-type").to_lowercase());
            writeln!(out, "{}", render(&t, &a, None))?;
        }
    }
    if s.total {
        writeln!(out, "{total}")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "missing_tests.rs"]
mod tests;
