//! A port of beets 2.14's `deezer` plugin (`beetsplug/deezer.py`): release
//! and track search on the Deezer API as autotag candidates, lookups by
//! Deezer id or URL, and `beet deezerupdate` (track ranks).
//!
//! The API's base URLs are the plugin's, except in tests (which build the
//! plugin with [`Deezer::with`] and a fake server's URLs).

use super::requests::{Session, format_url};
use super::{PluginLog, USER_AGENT};
use crate::config::Config;
use crate::hooks::V;
use crate::mb::{AlbumInfo, TrackInfo};
use crate::metadata_plugins::{MetadataSource, SourceCtx, extract_id, get_artist};
use crate::model::{FieldType, Item, Kind};
use anyhow::Result;
use serde_json::Value as J;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Deezer's "Various Artists" artist id.
const VARIOUS_ARTISTS_ID: i64 = 5080;
/// The importer's `SINGLE_ARTIST_THRESH` (beets/importer/tasks.py).
const SINGLE_ARTIST_THRESH: f64 = 0.25;

/// Deezer's item fields and their types (`DeezerPlugin.item_types`).
pub const ITEM_TYPES: &[(&str, FieldType)] = &[
    ("deezer_track_rank", FieldType::Int),
    ("deezer_track_id", FieldType::Int),
    ("deezer_updated", FieldType::Date),
];

/// The API's base URLs (`DeezerPlugin.search_url` etc.); each ends with the
/// slash the plugin appends nothing to.
#[derive(Debug, Clone)]
pub struct Urls {
    pub search: String,
    pub album: String,
    pub track: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            search: "https://api.deezer.com/search/".into(),
            album: "https://api.deezer.com/album/".into(),
            track: "https://api.deezer.com/track/".into(),
        }
    }
}

/// The API's base URLs, with `BEETSRS_DEEZER_API_URL` (a fake server in
/// tests and for debugging) replacing the host.
fn urls_from_env() -> Urls {
    match std::env::var("BEETSRS_DEEZER_API_URL") {
        Ok(base) => Urls {
            search: format!("{base}/search/"),
            album: format!("{base}/album/"),
            track: format!("{base}/track/"),
        },
        Err(_) => Urls::default(),
    }
}

#[derive(Debug, Clone)]
pub struct Conf {
    pub urls: Urls,
    /// `search_limit`.
    pub search_limit: usize,
    /// `search_query_ascii`.
    pub search_query_ascii: bool,
}

impl Conf {
    pub fn from_config(cfg: &Config) -> Self {
        Conf {
            urls: urls_from_env(),
            search_limit: cfg.f64("deezer.search_limit").max(0.0) as usize,
            search_query_ascii: cfg.bool("deezer.search_query_ascii"),
        }
    }
}

pub struct Deezer {
    pub conf: Conf,
    pub log: PluginLog,
    session: Session,
}

/// `time.time()`.
fn time_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

/// An integer id from a JSON value, as Python's `str(int)`.
fn id_str(v: &J) -> Option<String> {
    match v {
        J::Number(n) => Some(
            n.as_i64()
                .map_or_else(|| n.to_string(), |i| i.to_string()),
        ),
        J::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// An API object's `(id, name)` as an artist triple for `get_artist`.
fn artist_triples(artists: &[J]) -> Vec<(String, String, Option<String>)> {
    artists
        .iter()
        .map(|a| {
            (
                a.get("id").and_then(id_str).unwrap_or_default(),
                a.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                None,
            )
        })
        .collect()
}

fn get_artists(artists: &[J]) -> (String, Option<String>) {
    let triples = artist_triples(artists);
    let refs: Vec<(&str, &str, Option<&str>)> = triples
        .iter()
        .map(|(id, name, join)| (id.as_str(), name.as_str(), join.as_deref()))
        .collect();
    get_artist(&refs, false)
}

impl Deezer {
    pub fn new(cfg: &Config) -> Result<Self> {
        Ok(Self::with(Conf::from_config(cfg)))
    }

    pub fn with(conf: Conf) -> Self {
        Deezer {
            conf,
            log: PluginLog::new("deezer"),
            session: Session::plain(USER_AGENT),
        }
    }

    /// `fetch_data`: a GET and its JSON body, None (logged) on an error
    /// response or an API error object.
    pub fn fetch_data(&self, url: &str) -> Option<J> {
        let data = match self.session.get(url, crate::plugins::requests::Request::default()) {
            Ok(r) => match r.json() {
                Ok(j) => j,
                Err(e) => {
                    self.log.error(&format!(
                        "Error fetching data from {url}\n Error: {e}"
                    ));
                    return None;
                }
            },
            Err(e) => {
                self.log
                    .error(&format!("Error fetching data from {url}\n Error: {e}"));
                return None;
            }
        };
        if let Some(err) = data.get("error") {
            self.log.debug(&format!(
                "Deezer API error: {}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
            ));
            return None;
        }
        Some(data)
    }

    /// `_get_track`: a Deezer track object as a `TrackInfo`.
    pub fn get_track(&self, data: &J) -> TrackInfo {
        let contributors: Option<Vec<J>> = match data.get("contributors").and_then(|c| c.as_array()) {
            Some(c) => Some(c.clone()),
            None => data.get("artist").map(|a| vec![a.clone()]),
        };
        let (artist, artist_id) = match &contributors {
            Some(c) => {
                let (a, id) = get_artists(c);
                (a, id)
            }
            None => (String::new(), None),
        };
        let num = |k: &str| data.get(k).and_then(serde_json::Value::as_i64);
        let mut extra = vec![];
        if let Some(id) = data.get("id") {
            extra.push(("deezer_track_id".to_string(), V::Int(id.as_i64().unwrap_or(0))));
        }
        if let Some(rank) = data.get("rank") {
            extra.push((
                "deezer_track_rank".to_string(),
                V::Int(rank.as_i64().unwrap_or(0)),
            ));
        }
        extra.push(("deezer_updated".to_string(), V::Float(time_time())));
        let mut nulls = BTreeSet::new();
        let isrc = match data.get("isrc").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                nulls.insert("isrc");
                String::new()
            }
        };
        TrackInfo {
            title: data
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            track_id: data.get("id").and_then(id_str).unwrap_or_default(),
            artist,
            artist_id: artist_id.unwrap_or_default(),
            isrc,
            length: num("duration").map(|d| d as f64),
            index: num("track_position").unwrap_or(0) as usize,
            medium: num("disk_number").unwrap_or(0) as usize,
            medium_index: num("track_position").unwrap_or(0) as usize,
            data_source: self.data_source().to_string(),
            data_url: data
                .get("link")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            nulls,
            extra,
            ..Default::default()
        }
    }

    /// `album_for_id`: a Deezer album object (and its track list).
    pub fn album_from_data(&self, id: &str, album_data: &J) -> Option<AlbumInfo> {
        let contributors: Option<Vec<J>> =
            album_data.get("contributors").and_then(|c| c.as_array()).cloned();
        let (artist, artist_id) = match &contributors {
            Some(c) => {
                let (a, id) = get_artists(c);
                (Some(a), id)
            }
            None => (None, None),
        };
        let release_date = album_data
            .get("release_date")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let parts: Vec<i64> = release_date
            .split('-')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let (year, month, day) = match parts.len() {
            3 => (parts[0], Some(parts[1]), Some(parts[2])),
            2 => (parts[0], Some(parts[1]), None),
            1 => (parts[0], None, None),
            _ => {
                self.log.error(&format!(
                    "Invalid `release_date` returned by {} API: {release_date:?}",
                    self.data_source()
                ));
                return None;
            }
        };
        let mut tracks_obj = self.fetch_data(&format!("{}{id}/tracks", self.conf.urls.album))?;
        let mut tracks_data = match tracks_obj.get("data").and_then(|d| d.as_array()) {
            Some(d) => d.clone(),
            None => {
                self.log
                    .debug(&format!("Error fetching album tracks for {id}"));
                vec![]
            }
        };
        if tracks_data.is_empty() {
            return None;
        }
        // `while "next" in tracks_obj`: follow the pagination.
        while let Some(next) = tracks_obj.get("next").and_then(|n| n.as_str()) {
            let page = match self.session.get(next, crate::plugins::requests::Request::default()) {
                Ok(r) => r.json().unwrap_or(J::Null),
                Err(_) => J::Null,
            };
            if let Some(d) = page.get("data").and_then(|d| d.as_array()) {
                tracks_data.extend(d.clone());
            }
            tracks_obj = page;
        }
        let mut tracks = vec![];
        let mut medium_totals: Vec<(usize, usize)> = vec![];
        for (i, data) in tracks_data.iter().enumerate() {
            let mut track = self.get_track(data);
            track.index = i + 1;
            match medium_totals.iter_mut().find(|(m, _)| *m == track.medium) {
                Some((_, n)) => *n += 1,
                None => medium_totals.push((track.medium, 1)),
            }
            tracks.push(track);
        }
        for track in &mut tracks {
            track.medium_total = medium_totals
                .iter()
                .find(|(m, _)| *m == track.medium)
                .map_or(0, |(_, n)| *n);
        }
        let album_artist_id = album_data
            .get("artist")
            .and_then(|a| a.get("id"))
            .and_then(id_str)
            .unwrap_or_default();
        // Deezer assigns some compilations a single "main" artist instead
        // of the Various Artists entity: treat a release as VA when its
        // album artist performs on too few tracks.
        let performing = tracks
            .iter()
            .filter(|t| t.artist_id == album_artist_id)
            .count();
        let is_va = album_artist_id == VARIOUS_ARTISTS_ID.to_string()
            || (performing as f64) < tracks.len() as f64 * SINGLE_ARTIST_THRESH;
        let va_name = "Various Artists".to_string();
        let artist = if is_va { Some(va_name) } else { artist };
        let artist_credit = if is_va {
            artist.clone().unwrap_or_default()
        } else {
            album_data
                .get("artist")
                .map(|a| get_artists(std::slice::from_ref(a)).0)
                .unwrap_or_default()
        };
        let mut extra = vec![("deezer_album_id".to_string(), V::Str(id.to_string()))];
        if let Some(cover) = album_data.get("cover_xl")
            && !cover.is_null() {
                extra.push((
                    "cover_art_url".to_string(),
                    V::Str(cover.as_str().unwrap_or_default().to_string()),
                ));
            }
        Some(AlbumInfo {
            album: album_data
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            album_id: id.to_string(),
            artist: artist.unwrap_or_default(),
            artist_id: artist_id.unwrap_or_default(),
            artist_credit,
            tracks,
            albumtype: album_data
                .get("record_type")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            va: is_va,
            year,
            month: month.unwrap_or(0),
            day: day.unwrap_or(0),
            label: album_data
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            mediums: medium_totals
                .iter()
                .filter(|(m, _)| *m != 0)
                .map(|(m, _)| *m)
                .max()
                .unwrap_or(0),
            data_source: self.data_source().to_string(),
            data_url: album_data
                .get("link")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            extra,
            ..Default::default()
        })
    }

    /// `get_search_query_with_filters`: the album search's field filters,
    /// the track search's plain free text.
    pub fn search_query(&self, query_type: &str, artist: &str, name: &str, va_likely: bool) -> String {
        if query_type == "album" {
            let mut q = format!("album:\"{name}\"");
            if !va_likely {
                q.push_str(&format!(" artist:\"{artist}\""));
            }
            q
        } else {
            format!("{name} {artist}").trim().to_string()
        }
    }

    /// `_search_api`: the search request's result objects.
    fn search_api(&self, query_type: &str, query: &str) -> Vec<J> {
        let mut query = query.to_string();
        if self.conf.search_query_ascii {
            query = deunicode::deunicode(&query);
        }
        if query.is_empty() {
            self.log.debug("Skipping search with empty query and filters");
            return vec![];
        }
        let limit = self.conf.search_limit;
        self.log
            .debug(&format!("Searching for '{query}' with {{}}"));
        let url = format_url(
            &format!("{}{query_type}", self.conf.urls.search),
            &[("q", query), ("limit", limit.to_string())],
        );
        let data = match self.session.get(&url, crate::plugins::requests::Request::default()) {
            Ok(r) => match r.json() {
                Ok(j) => j,
                Err(e) => {
                    self.log.error(&format!("Error searching Deezer: {e}"));
                    return vec![];
                }
            },
            Err(e) => {
                self.log.error(&format!("Error searching Deezer: {e}"));
                return vec![];
            }
        };
        let results = data
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        self.log
            .debug(&format!("Found {} result(s)", results.len()));
        results
    }

    /// `deezerupdate`: fetch each item's rank and store it.
    pub fn deezerupdate(
        &self,
        cfg: &Config,
        lib: &crate::library::Library,
        items: &mut [Item],
        write: bool,
    ) -> Result<()> {
        let total = items.len();
        for (index, item) in items.iter_mut().enumerate() {
            self.log.info(&format!(
                "Processing {}/{} tracks - {} ",
                index + 1,
                total,
                format_record(cfg, item)
            ));
            let Some(track_id) = item.get("deezer_track_id").filter(|v| !v.raw().is_empty()) else {
                self.log
                    .debug(&format!("No deezer_track_id present for: {}", format_record(cfg, item)));
                continue;
            };
            let track_id = track_id.raw();
            let track = self.fetch_data(&format!("{}{track_id}", self.conf.urls.track));
            let Some(track) = track else {
                self.log
                    .debug(&format!("Invalid Deezer track_id: {}", format_record(cfg, item)));
                continue;
            };
            if let Some(rank) = track.get("rank").filter(|r| !r.is_null()) {
                self.log.debug(&format!(
                    "Deezer track: {track_id} has {} rank",
                    rank.as_i64().unwrap_or(0)
                ));
                item.set("deezer_track_rank", crate::model::Value::Int(rank.as_i64().unwrap_or(0)));
                lib.store(item)?;
                // As beets: `deezer_updated` is set after the store, so it
                // only reaches the file (not the database) with `write`.
                item.set("deezer_updated", crate::model::Value::Float(time_time()));
                if write {
                    // beets' `try_write`: a failure is logged, not fatal.
                    let mut item = item.clone();
                    if let Err(e) = crate::media::write_item(&mut item) {
                        crate::logging::emit(format!("{e:#}"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// `{item}`: the `format_item` template.
fn format_record(cfg: &Config, item: &Item) -> String {
    let t = crate::template::Template::parse(&cfg.str("format_item"));
    crate::commands::format_record(&t, item, None)
}

impl MetadataSource for Deezer {
    fn data_source(&self) -> &'static str {
        "Deezer"
    }

    fn album_for_id(&self, _ctx: &SourceCtx, album_id: &str) -> Result<Option<AlbumInfo>> {
        let Some(id) = extract_id("deezer", album_id) else {
            return Ok(None);
        };
        let url = format!("{}{id}", self.conf.urls.album);
        let Some(data) = self.fetch_data(&url) else {
            return Ok(None);
        };
        Ok(self.album_from_data(&id, &data))
    }

    fn track_for_id(&self, _ctx: &SourceCtx, track_id: &str) -> Result<Option<TrackInfo>> {
        let Some(id) = extract_id("deezer", track_id) else {
            self.log.debug(&format!("Invalid Deezer track_id: {track_id}"));
            return Ok(None);
        };
        let Some(data) = self.fetch_data(&format!("{}{id}", self.conf.urls.track)) else {
            self.log.debug(&format!("Track not found: {track_id}"));
            return Ok(None);
        };
        let mut track = self.get_track(&data);
        let Some(album_id) = data
            .get("album")
            .and_then(|a| a.get("id"))
            .and_then(id_str)
        else {
            return Ok(None);
        };
        let Some(tracks_obj) = self.fetch_data(&format!("{}{album_id}/tracks", self.conf.urls.album))
        else {
            return Ok(None);
        };
        let Some(album_tracks) = tracks_obj.get("data").and_then(|d| d.as_array()) else {
            self.log
                .debug(&format!("Error fetching album tracks for {album_id}"));
            return Ok(None);
        };
        let mut medium_total = 0;
        for (i, t) in album_tracks.iter().enumerate() {
            if t.get("disk_number").and_then(serde_json::Value::as_i64).unwrap_or(0) as usize == track.medium {
                medium_total += 1;
                if t.get("id").and_then(id_str) == Some(track.track_id.clone()) {
                    track.index = i + 1;
                }
            }
        }
        track.medium_total = medium_total;
        Ok(Some(track))
    }

    fn candidates(
        &self,
        ctx: &SourceCtx,
        items: &[Item],
        artist: &str,
        album: &str,
        va_likely: bool,
    ) -> Result<Vec<AlbumInfo>> {
        let query = self.search_query("album", artist, album, va_likely);
        let mut out = vec![];
        for result in self.search_api("album", &query) {
            if let Some(id) = result.get("id").and_then(id_str)
                && let Some(info) = self.album_for_id(ctx, &id)? {
                    out.push(info);
                }
        }
        let _ = items;
        Ok(out)
    }

    fn item_candidates(
        &self,
        ctx: &SourceCtx,
        _item: &Item,
        artist: &str,
        title: &str,
    ) -> Result<Vec<TrackInfo>> {
        let query = self.search_query("track", artist, title, false);
        let mut out = vec![];
        for result in self.search_api("track", &query) {
            if let Some(id) = result.get("id").and_then(id_str)
                && let Some(info) = self.track_for_id(ctx, &id)? {
                    out.push(info);
                }
        }
        Ok(out)
    }
}

impl super::Plugin for Deezer {
    fn name(&self) -> &'static str {
        "deezer"
    }
}

// ---------------------------------------------------------------- command

#[derive(Debug, Default)]
pub struct CmdOpts {
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// `beet deezerupdate`.
pub fn command(cfg: &Config, lib: &crate::library::Library, opts: &CmdOpts) -> Result<()> {
    let plugin = Deezer::new(cfg)?;
    let items = lib.items(&crate::query::Query::parse(Kind::Item, &opts.query)?)?;
    let mut items: Vec<Item> = match opts.limit {
        Some(n) => items.into_iter().take(n).collect(),
        None => items,
    };
    plugin.deezerupdate(cfg, lib, &mut items, cfg.bool("import.write"))
}

// --------------------------------------------------------------- registry

fn registry_make(cfg: &Config) -> Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Deezer::new(cfg)?))
}

fn metadata_source_make(
    cfg: &Config,
) -> Result<Box<dyn crate::metadata_plugins::MetadataSource>> {
    Ok(Box::new(Deezer::new(cfg)?))
}

inventory::submit! {
    crate::metadata_plugins::MetadataSourceDef {
        // 5.19
        order: 90,
        plugin: "deezer",
        data_source: "Deezer",
        configured: |_cfg| true,
        make: metadata_source_make,
    }
}

// `beet deezerupdate`'s options (`add_limit_option`, a query).
#[derive(clap::Args)]
pub struct DeezerupdateArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_deezerupdate() -> clap::Command {
    use clap::Args as _;
    DeezerupdateArgs::augment_args(
        clap::Command::new("deezerupdate").about("Update Deezer rank"),
    )
}

pub fn run_deezerupdate(
    cfg: &Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> Result<()> {
    use clap::FromArgMatches as _;
    let a = DeezerupdateArgs::from_arg_matches(m)?;
    command(
        cfg,
        lib,
        &CmdOpts {
            limit: a.limit,
            query: a.query,
        },
    )
}

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "deezer",
        // 5.19
        order: 90,
        defaults: r#"deezer:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    search_query_ascii: no
"#,
        config_keys: &["deezer"],
        item_types: crate::plugins::deezer::ITEM_TYPES,
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            // 5.19
            plugin: "deezer",
            build: build_deezerupdate,
            run: run_deezerupdate,
        }],
    }
}

#[cfg(test)]
#[path = "deezer_tests.rs"]
mod tests;
