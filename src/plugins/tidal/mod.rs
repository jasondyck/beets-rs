//! A port of beets 2.14's `tidal` plugin (`beetsplug/tidal`): a metadata
//! source for the autotagger (album and track searches, ID lookups, ISRC and
//! barcode lookups) using Tidal's JSON:API (`openapi.tidal.com/v2`), plus
//! `beet tidal --auth` (OAuth2 with PKCE) and `beet tidalsync` (popularity
//! synchronisation).

mod api;
mod session;

use crate::config::Config;
use crate::library::Library;
use crate::mb::{AlbumInfo, TrackInfo};
use crate::model::{Item, Kind, Record, Value};
use crate::plugins::PluginLog;
use crate::query::Query;
use anyhow::{Result, bail};
use api::TidalApi;
use regex::Regex;
use serde_json::Value as J;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

#[cfg(test)]
use std::cell::RefCell;

use crate::model::FieldType;

/// The plugin's `item_types`.
pub const ITEM_TYPES: &[(&str, FieldType)] = &[
    ("tidal_track_id", FieldType::Str),
    ("tidal_artist_id", FieldType::Str),
    ("tidal_track_popularity", FieldType::Int),
    ("tidal_updated", FieldType::Date),
];

/// The plugin's `album_types`.
pub const ALBUM_TYPES: &[(&str, FieldType)] = &[
    ("tidal_album_id", FieldType::Str),
    ("tidal_artist_id", FieldType::Str),
    ("tidal_album_popularity", FieldType::Int),
    ("tidal_updated", FieldType::Date),
];

#[cfg(test)]
pub use session::query_param;

/// beets' hardcoded endpoints (tests override them with a local server).
#[derive(Clone, Debug)]
pub(crate) struct Urls {
    pub api_base: String,
    pub auth_url: String,
    pub token_url: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            api_base: "https://openapi.tidal.com/v2".into(),
            auth_url: "https://login.tidal.com/authorize".into(),
            token_url: "https://auth.tidal.com/v1/oauth2/token".into(),
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_URLS: RefCell<Option<Urls>> = const { RefCell::new(None) };
}

fn urls() -> Urls {
    #[cfg(test)]
    if let Some(u) = TEST_URLS.with(|u| u.borrow().clone()) {
        return u;
    }
    Urls::default()
}

/// Point the session and API at a local test server for the rest of this
/// thread (`None` restores beets' endpoints).
#[cfg(test)]
pub(crate) fn set_test_urls(u: Option<Urls>) {
    TEST_URLS.with(|v| *v.borrow_mut() = u);
}

/// The plugin: the `import_begin` check and the commands' shared config.
pub struct TidalPlugin {
    token_path: std::path::PathBuf,
}

impl TidalPlugin {
    pub fn new(cfg: &Config) -> Self {
        TidalPlugin {
            token_path: session::token_path(cfg),
        }
    }
}

impl crate::plugins::Plugin for TidalPlugin {
    fn name(&self) -> &'static str {
        "tidal"
    }

    /// `TidalPlugin.require_authentication`: beets raises a `UserError`
    /// from the `import_begin` listener, which ends the import.
    fn import_begin(&self, _ctx: &crate::plugins::PluginCtx) -> Result<()> {
        if !self.token_path.is_file() {
            bail!(
                "Please login to TIDAL using `beet tidal --auth` or disable tidal plugin"
            );
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ lookups

/// One lookup session: the API client and the plugin's logger.
pub struct Tidal {
    api: TidalApi,
    log: PluginLog,
}

impl Tidal {
    pub fn new(cfg: &Config) -> Self {
        Tidal {
            api: TidalApi::new(
                &cfg.str("tidal.client_id"),
                &session::token_path(cfg),
                urls(),
            ),
            log: PluginLog::new("tidal"),
        }
    }

    /// beets' `MetadataSourcePlugin._extract_id` for tidal (`tidal.com` URLs
    /// and bare numeric IDs).
    pub fn extract_id(id: &str) -> Option<String> {
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?:^|tidal\.com/(?:browse/)?(?:album|track)/)(\d+)").unwrap());
        RE.captures(id).map(|c| c[1].to_string())
    }

    /// beets' `album_for_id`.
    pub fn album_for_id(&mut self, album_id: &str) -> Result<Option<AlbumInfo>> {
        let Some(id) = Self::extract_id(album_id) else {
            return Ok(None);
        };
        if let Some(info) = self.search_albums_by_ids(std::slice::from_ref(&id), &[])?.into_iter().next() {
            return Ok(info);
        }
        self.log.warning(&format!("Could not find album:{id}"));
        Ok(None)
    }

    /// beets' `track_for_id`.
    pub fn track_for_id(&mut self, track_id: &str) -> Result<Option<TrackInfo>> {
        let Some(id) = Self::extract_id(track_id) else {
            return Ok(None);
        };
        if let Some(info) = self.search_tracks_by_ids(std::slice::from_ref(&id), &[])?.into_iter().next() {
            return Ok(info);
        }
        self.log.warning(&format!("Could not find track:{id}"));
        Ok(None)
    }

    /// beets' `candidates`: a barcode lookup first (a 100% match when it
    /// finds something), then a search per artist/album pair.
    pub fn candidates(
        &mut self,
        items: &[Item],
        _artist: &str,
        _album: &str,
        _va_likely: bool,
    ) -> Result<Vec<AlbumInfo>> {
        let barcodes: Vec<String> = items
            .iter()
            .map(|i| i.str("barcode").to_string())
            .filter(|b| !b.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !barcodes.is_empty() {
            let found: Vec<Option<AlbumInfo>> = self.search_albums_by_ids(&[], &barcodes)?;
            let found: Vec<AlbumInfo> = found.into_iter().flatten().collect();
            if !found.is_empty() {
                return Ok(found);
            }
        }
        let mut out = vec![];
        for query in album_queries(items) {
            out.extend(self.search_albums_by_query(&query)?);
        }
        self.log
            .debug(&format!("Found {} candidates", out.len()));
        Ok(out)
    }

    /// beets' `item_candidates`: an ISRC lookup first, then a search.
    pub fn item_candidates(
        &mut self,
        item: &Item,
        _artist: &str,
        _title: &str,
    ) -> Result<Vec<TrackInfo>> {
        let isrc = item.str("isrc").to_string();
        if !isrc.is_empty() {
            let found: Vec<Option<TrackInfo>> = self.search_tracks_by_ids(&[], &[isrc])?;
            let found: Vec<TrackInfo> = found.into_iter().flatten().collect();
            if !found.is_empty() {
                return Ok(found);
            }
        }
        let mut out = vec![];
        for query in item_queries(item) {
            out.extend(self.search_tracks_by_query(&query)?);
        }
        self.log
            .debug(&format!("Found {} candidates", out.len()));
        Ok(out)
    }

    /// beets' `search_tracks_by_query`.
    pub fn search_tracks_by_query(&mut self, query: &str) -> Result<Vec<TrackInfo>> {
        let doc = self
            .api
            .search_results(query, "INCLUDE", &["tracks.artists"], "US")?;
        let data = array(&doc, "data");
        let Some(first) = data.first() else {
            return Ok(vec![]);
        };
        let included = array(&doc, "included");
        let track_by_id = by_type(included, "tracks");
        let artist_by_id = by_type(included, "artists");
        let mut out = vec![];
        for rel in relationship(first, "tracks") {
            let id = s(rel, "id");
            match track_by_id.get(&id) {
                Some(track) => out.push(self.get_track_info(track, &artist_by_id)),
                None => self
                    .log
                    .warning(&format!("Track with id {id} not found in lookup")),
            }
        }
        Ok(out)
    }

    /// beets' `search_albums_by_query`.
    pub fn search_albums_by_query(&mut self, query: &str) -> Result<Vec<AlbumInfo>> {
        let doc = self
            .api
            .search_results(query, "INCLUDE", &["albums"], "US")?;
        let data = array(&doc, "data");
        let Some(first) = data.first() else {
            return Ok(vec![]);
        };
        let ids: Vec<String> = relationship(first, "albums")
            .iter()
            .map(|r| s(r, "id"))
            .collect();
        Ok(self
            .search_albums_by_ids(&ids, &[])?
            .into_iter()
            .flatten()
            .collect())
    }

    /// beets' `search_tracks_by_ids`: the ID lookups in order (None for a
    /// miss), then the ISRCs.
    pub fn search_tracks_by_ids(
        &mut self,
        tidal_ids: &[String],
        isrcs: &[String],
    ) -> Result<Vec<Option<TrackInfo>>> {
        let doc = self
            .api
            .get_tracks(tidal_ids, isrcs, &["artists"], "US")?;
        let track_by_id = by_type(array(&doc, "data"), "tracks");
        let artist_by_id = by_type(array(&doc, "included"), "artists");
        let mut out = vec![];
        for id in tidal_ids {
            out.push(
                track_by_id
                    .get(id)
                    .map(|t| self.get_track_info(t, &artist_by_id)),
            );
        }
        if !isrcs.is_empty() {
            let isrc_to_track: BTreeMap<String, &J> = track_by_id
                .values()
                .filter_map(|t| {
                    let isrc = t.get("attributes")?.get("isrc")?.as_str()?;
                    Some((isrc.to_string(), *t))
                })
                .collect();
            for isrc in isrcs {
                out.push(
                    isrc_to_track
                        .get(isrc)
                        .map(|t| self.get_track_info(t, &artist_by_id)),
                );
            }
        }
        Ok(out)
    }

    /// beets' `search_albums_by_ids`: the ID lookups in order, then the
    /// barcodes.
    pub fn search_albums_by_ids(
        &mut self,
        tidal_ids: &[String],
        barcode_ids: &[String],
    ) -> Result<Vec<Option<AlbumInfo>>> {
        let doc = self.api.get_albums(
            tidal_ids,
            barcode_ids,
            &["items.artists", "artists", "coverArt"],
            "US",
        )?;
        let album_by_id = by_type(array(&doc, "data"), "albums");
        let track_by_id = by_type(array(&doc, "included"), "tracks");
        let artist_by_id = by_type(array(&doc, "included"), "artists");
        let artwork_by_id = by_type(array(&doc, "included"), "artworks");
        let mut out = vec![];
        for id in tidal_ids {
            out.push(album_by_id.get(id).map(|a| {
                self.get_album_info(a, &track_by_id, &artist_by_id, &artwork_by_id)
            }));
        }
        if !barcode_ids.is_empty() {
            let barcode_to_album: BTreeMap<String, &J> = album_by_id
                .values()
                .filter_map(|a| {
                    let barcode = a.get("attributes")?.get("barcodeId")?.as_str()?;
                    Some((barcode.to_string(), *a))
                })
                .collect();
            for barcode in barcode_ids {
                out.push(barcode_to_album.get(barcode).map(|a| {
                    self.get_album_info(a, &track_by_id, &artist_by_id, &artwork_by_id)
                }));
            }
        }
        Ok(out)
    }

    /// beets' `_get_album_info`.
    pub fn get_album_info(
        &self,
        album: &J,
        track_by_id: &BTreeMap<String, &J>,
        artist_by_id: &BTreeMap<String, &J>,
        artwork_by_id: &BTreeMap<String, &J>,
    ) -> AlbumInfo {
        let attrs = album.get("attributes").unwrap_or(&J::Null);
        let mut tracks = vec![];
        for (i, rel) in relationship(album, "items").iter().enumerate() {
            if let Some(track) = track_by_id.get(&s(rel, "id")) {
                let mut info = self.get_track_info(track, artist_by_id);
                info.index = i + 1;
                tracks.push(info);
            }
        }
        let (artist_names, artist_ids) =
            parse_artists(relationship(album, "artists"), artist_by_id, &self.log);
        let label = parse_label(attrs);
        let date = parse_release_date(attrs);
        let mut nulls: BTreeSet<&'static str> = [
            "albumdisambig",
            "albumstatus",
            "albumtype",
            "artist_credit",
            "artist_sort",
            "artists_credit",
            "artists_sort",
            "asin",
            "catalognum",
            "country",
            "label",
            "language",
            "media",
            "mediums",
            "releasegroup_id",
            "releasegroupdisambig",
            "script",
        ]
        .into();
        let barcode = opt_s(attrs, "barcodeId");
        if barcode.is_none() {
            nulls.insert("barcode");
        }
        let data_url = parse_data_url(attrs);
        if data_url.is_none() {
            nulls.insert("data_url");
        }
        let cover = parse_artwork_url(album, artwork_by_id);
        let duration = duration_to_seconds(&s(attrs, "duration"), &self.log);
        let albumtype = s(attrs, "albumType").to_lowercase();
        let mut extra: Vec<(String, crate::hooks::V)> = vec![
            (
                "cover_art_url".into(),
                crate::hooks::V::opt_str(cover),
            ),
            (
                "duration".into(),
                duration.map_or(crate::hooks::V::None, |d| {
                    crate::hooks::V::Int(d)
                }),
            ),
            ("tidal_album_id".into(), crate::hooks::V::Str(s(album, "id"))),
            (
                "tidal_artist_id".into(),
                crate::hooks::V::opt_str(artist_ids.first().cloned()),
            ),
            (
                "tidal_album_popularity".into(),
                crate::hooks::V::Int(parse_popularity(attrs)),
            ),
            (
                "tidal_updated".into(),
                crate::hooks::V::Float(now()),
            ),
        ];
        if let Some(url) = data_url {
            extra.push(("data_url".into(), crate::hooks::V::Str(url)));
        }
        AlbumInfo {
            album_id: s(album, "id"),
            album: parse_title(attrs),
            artists_ids: artist_ids.clone(),
            data_source: "Tidal".into(),
            data_url: String::new(),
            barcode: barcode.unwrap_or_default(),
            albumtypes: vec![albumtype],
            tracks,
            artist: artist_names.join(", "),
            artists: artist_names,
            label: label.unwrap_or_default(),
            year: date.map_or(0, |d| d.0),
            month: date.map_or(0, |d| d.1),
            day: date.map_or(0, |d| d.2),
            nulls,
            extra,
            ..Default::default()
        }
    }

    /// beets' `_get_track_info`.
    pub fn get_track_info(
        &self,
        track: &J,
        artist_by_id: &BTreeMap<String, &J>,
    ) -> TrackInfo {
        let attrs = track.get("attributes").unwrap_or(&J::Null);
        let (artist_names, artist_ids) =
            parse_artists(relationship(track, "artists"), artist_by_id, &self.log);
        let mut nulls: BTreeSet<&'static str> = [
            "artist_credit",
            "artist_id",
            "artist_sort",
            "artists_credit",
            "artists_sort",
            "medium",
            "medium_index",
            "medium_total",
        ]
        .into();
        let data_url = parse_data_url(attrs);
        if data_url.is_none() {
            nulls.insert("data_url");
        }
        let label = parse_label(attrs);
        let duration = duration_to_seconds(&s(attrs, "duration"), &self.log);
        let extra = vec![
            (
                "label".into(),
                crate::hooks::V::opt_str(label),
            ),
            ("tidal_track_id".into(), crate::hooks::V::Str(s(track, "id"))),
            (
                "tidal_artist_id".into(),
                crate::hooks::V::opt_str(artist_ids.first().cloned()),
            ),
            (
                "tidal_track_popularity".into(),
                crate::hooks::V::Int(parse_popularity(attrs)),
            ),
            ("tidal_updated".into(), crate::hooks::V::Float(now())),
        ];
        let mut extra = extra;
        if let Some(url) = data_url {
            extra.push(("data_url".into(), crate::hooks::V::Str(url)));
        }
        TrackInfo {
            title: parse_title(attrs),
            track_id: s(track, "id"),
            artists_ids: artist_ids.clone(),
            data_source: "Tidal".into(),
            data_url: String::new(),
            isrc: s(attrs, "isrc"),
            artist: artist_names.join(", "),
            artists: artist_names,
            length: duration.map(|d| d as f64),
            nulls,
            omit: ["trackdisambig"].into(),
            extra,
            ..Default::default()
        }
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

// ---------------------------------------------------------------- parsing

fn array<'a>(j: &'a J, key: &str) -> &'a [J] {
    j.get(key)
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn s(j: &J, key: &str) -> String {
    j.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn opt_s(j: &J, key: &str) -> Option<String> {
    j.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// `resource["relationships"][name]["data"]`.
fn relationship<'a>(j: &'a J, name: &str) -> &'a [J] {
    j.get("relationships")
        .and_then(|r| r.get(name))
        .map(|r| array(r, "data"))
        .unwrap_or_default()
}

/// The included resources of one type, keyed by id (the first of a repeated
/// id, as beets' dict comprehension).
fn by_type<'a>(included: &'a [J], ty: &str) -> BTreeMap<String, &'a J> {
    let mut out = BTreeMap::new();
    for item in included {
        if s(item, "type") == ty {
            out.entry(s(item, "id")).or_insert(item);
        }
    }
    out
}

/// beets' `_item_queries`: the title, then artist and title.
fn item_queries(item: &Item) -> Vec<String> {
    let mut out = vec![item.str("title").to_string()];
    if !item.str("artist").is_empty() {
        out.push(format!("{} {}", item.str("artist"), item.str("title")));
    }
    out
}

/// beets' `_album_queries`: artist and album, for each pair.
fn album_queries(items: &[Item]) -> Vec<String> {
    let albums: BTreeSet<&str> = items.iter().map(|i| i.str("album")).collect();
    let artists: BTreeSet<&str> = items.iter().map(|i| i.str("artist")).collect();
    let mut out = vec![];
    for album in albums {
        for artist in &artists {
            out.push(format!("{artist} {album}"));
        }
    }
    out
}

/// beets' `_parse_artists`: the relationship's order, with names and ids.
fn parse_artists(
    rels: &[J],
    artist_by_id: &BTreeMap<String, &J>,
    log: &PluginLog,
) -> (Vec<String>, Vec<String>) {
    let (mut names, mut ids) = (vec![], vec![]);
    for rel in rels {
        match artist_by_id.get(&s(rel, "id")) {
            Some(artist) => {
                ids.push(s(artist, "id"));
                names.push(s(artist.get("attributes").unwrap_or(&J::Null), "name"));
            }
            None => log.warning(&format!(
                "Artist with id {} not found in lookup",
                s(rel, "id")
            )),
        }
    }
    (names, ids)
}

/// beets' `_parse_title`: Tidal's version suffix is part of the title.
fn parse_title(attrs: &J) -> String {
    let title = s(attrs, "title");
    match attrs.get("version").and_then(|v| v.as_str()) {
        Some(version) if !version.is_empty() => format!("{title} ({version})"),
        _ => title,
    }
}

/// beets' `_parse_data_url`.
fn parse_data_url(attrs: &J) -> Option<String> {
    array(attrs, "externalLinks")
        .first()
        .and_then(|l| opt_s(l, "href"))
}

/// beets' `_parse_label`.
fn parse_label(attrs: &J) -> Option<String> {
    let copyright = attrs.get("copyright")?.get("text")?.as_str()?;
    Some(normalize_label(copyright))
}

/// beets' `_normalize_label_re` (VERBOSE).
fn normalize_label_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(?:.*(?:©|℗|\([cp]\))\s*)?(?:\d{4}\s+)?(?:.*? under exclusive licen[sc]e to )?(.*?)(?:,? (?:inc|llc|ltd|co)\.?)?(?:, a .*)?(?:,? for the united states and .*)?$",
        )
        .unwrap()
    });
    &RE
}

fn normalize_label(text: &str) -> String {
    match normalize_label_re().captures(text) {
        Some(m) => m[1].to_string(),
        None => text.to_string(),
    }
}

/// ISO 8601's `PT...H...M...S` (`ISO_8601_RE`).
fn duration_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?$").unwrap()
    });
    &RE
}

/// beets' `_duration_to_seconds`; an invalid duration is logged and None.
fn duration_to_seconds(duration: &str, log: &PluginLog) -> Option<i64> {
    let Some(m) = duration_re().captures(duration) else {
        log.warning(&format!("Invalid ISO 8601 duration: {duration}"));
        return None;
    };
    let part = |i: usize| m.get(i).map_or(0, |g| g.as_str().parse::<i64>().unwrap_or(0));
    Some(part(3) + part(2) * 60 + part(1) * 3600)
}

/// beets' `_parse_release_date`.
fn parse_release_date(attrs: &J) -> Option<(i64, i64, i64)> {
    let date = attrs.get("releaseDate")?.as_str()?;
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

/// beets' `_parse_popularity`: Python's bounded round of the percentage
/// (banker's rounding at exact halves).
fn parse_popularity(attrs: &J) -> i64 {
    attrs
        .get("popularity")
        .and_then(serde_json::Value::as_f64)
        .map_or(0, |p| (p * 100.0).round_ties_even() as i64)
}

/// beets' `_parse_artwork_url`.
fn parse_artwork_url(album: &J, artwork_by_id: &BTreeMap<String, &J>) -> Option<String> {
    let cover = album.get("relationships")?.get("coverArt")?;
    for rel in array(cover, "data") {
        if s(rel, "type") != "artworks" {
            continue;
        }
        let Some(artwork) = artwork_by_id.get(&s(rel, "id")) else {
            continue;
        };
        let files = array(artwork.get("attributes").unwrap_or(&J::Null), "files");
        if let Some(href) = files.first().and_then(|f| opt_s(f, "href")) {
            return Some(href);
        }
    }
    None
}

// ---------------------------------------------------------------- commands

/// The plugin's commands (`beet tidal` and `beet tidalsync`).
pub fn command_tidal(cfg: &Config, auth: bool) -> Result<()> {
    use std::io::Write as _;
    if !auth {
        let mut cmd = build_tidal();
        cmd.print_help()?;
        println!();
        return Ok(());
    }
    let mut api = TidalApi::new(
        &cfg.str("tidal.client_id"),
        &session::token_path(cfg),
        urls(),
    );
    let (auth_url, _) = api.session().authorization_url();
    println!("Visit: {auth_url}");
    if let Ok(status) = std::process::Command::new(webbrowser())
        .arg(&auth_url)
        .status()
    {
        let _ = status;
    }
    print!("Paste redirected URL: ");
    std::io::stdout().flush()?;
    let mut redirect = String::new();
    std::io::stdin().read_line(&mut redirect)?;
    api.session().fetch_token(redirect.trim())?;
    println!(
        "Saved tidal token in {}",
        api.session().token_path.display()
    );
    Ok(())
}

/// `webbrowser.open`'s opener (best effort; failures are ignored as
/// `webbrowser.Error` is suppressed).
fn webbrowser() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "cmd"
    } else {
        "xdg-open"
    }
}

/// `beet tidalsync`: sync popularity for the items (or albums) matching the
/// query, prefixed with `data_source:tidal`.
pub fn command_tidalsync(
    cfg: &Config,
    lib: &Library,
    album: bool,
    force: bool,
    write: bool,
    limit: Option<usize>,
    query: &[String],
) -> Result<()> {
    let mut q = vec!["data_source:tidal".to_string()];
    q.extend(query.iter().cloned());
    let mut tidal = Tidal::new(cfg);
    let log = PluginLog::new("tidal");
    if album {
        let mut albums = lib.albums(&Query::parse(Kind::Album, &q)?)?;
        albums.truncate(limit.unwrap_or(usize::MAX));
        tidal.sync_album_popularity(lib, &albums, write, force, &log)?;
    } else {
        let mut items = lib.items(&Query::parse(Kind::Item, &q)?)?;
        items.truncate(limit.unwrap_or(usize::MAX));
        tidal.sync_item_popularity(lib, &items, write, force, &log)?;
    }
    Ok(())
}

impl Tidal {
    pub fn sync_item_popularity(
        &mut self,
        lib: &Library,
        results: &[Item],
        write: bool,
        force: bool,
        log: &PluginLog,
    ) -> Result<()> {
        self.sync_popularity(
            lib,
            results.to_vec(),
            write,
            force,
            "tidal_track_id",
            "tidal_track_popularity",
            false,
            log,
        )
    }

    pub fn sync_album_popularity(
        &mut self,
        lib: &Library,
        results: &[crate::model::Album],
        write: bool,
        force: bool,
        log: &PluginLog,
    ) -> Result<()> {
        self.sync_popularity(
            lib,
            results.to_vec(),
            write,
            force,
            "tidal_album_id",
            "tidal_album_popularity",
            true,
            log,
        )
    }

    /// beets' `_sync_popularity` for items or albums.
    #[allow(clippy::too_many_arguments)]
    fn sync_popularity(
        &mut self,
        lib: &Library,
        results: Vec<Record>,
        write: bool,
        force: bool,
        id_field: &str,
        popularity_field: &str,
        albums: bool,
        log: &PluginLog,
    ) -> Result<()> {
        let label = if albums { "album" } else { "item" };
        log.info(&format!(
            "Syncing popularity for {} {label}s",
            results.len()
        ));
        // The models needing an update, in the results' order.
        let mut needing: Vec<(Record, String)> = vec![];
        for rec in results {
            let Some(id) = rec.flex.get(id_field).filter(|v| !v.is_empty()).cloned() else {
                continue;
            };
            let has = rec
                .flex
                .get(popularity_field)
                .is_some_and(|v| !v.is_empty());
            if force || !has {
                needing.push((rec, id));
            }
        }
        let total = needing.len();
        log.debug(&format!("{total} {label}s need updates"));
        if total == 0 {
            log.info(&format!(
                "Successfully synchronised popularity for {total} {label}s"
            ));
            return Ok(());
        }
        let ids: Vec<String> = needing.iter().map(|(_, id)| id.clone()).collect();
        let infos: Vec<Option<(i64, bool)>> = if albums {
            let doc = self.api.get_albums(
                &ids,
                &[],
                &["items.artists", "artists", "coverArt"],
                "US",
            )?;
            let album_by_id = by_type(array(&doc, "data"), "albums");
            ids.iter()
                .map(|id| {
                    album_by_id.get(id).map(|a| {
                        (
                            parse_popularity(a.get("attributes").unwrap_or(&J::Null)),
                            true,
                        )
                    })
                })
                .collect()
        } else {
            let doc = self.api.get_tracks(&ids, &[], &["artists"], "US")?;
            let track_by_id = by_type(array(&doc, "data"), "tracks");
            ids.iter()
                .map(|id| {
                    track_by_id.get(id).map(|t| {
                        (
                            parse_popularity(t.get("attributes").unwrap_or(&J::Null)),
                            false,
                        )
                    })
                })
                .collect()
        };
        let mut processed = 0;
        for ((mut rec, _), info) in needing.into_iter().zip(infos) {
            let Some((popularity, _)) = info else {
                continue;
            };
            rec.set(popularity_field, Value::Int(popularity));
            rec.set("tidal_updated", Value::Float(now()));
            if rec.kind == Kind::Item
                && write
                && let Err(e) = crate::media::write_item(&mut rec)
            {
                crate::logging::emit(format!("{e:#}"));
            }
            lib.store(&rec)?;
            processed += 1;
            if processed % 100 == 0 || processed == total {
                log.debug(&format!("Synced {processed}/{total} {label}s"));
            }
        }
        log.info(&format!(
            "Successfully synchronised popularity for {total} {label}s"
        ));
        Ok(())
    }
}

// ----------------------------------------------------------------- clap

#[derive(clap::Args)]
pub struct TidalArgs {
    /// Authenticate and login to Tidal
    #[arg(short = 'a', long)]
    auth: bool,
}

pub fn build_tidal() -> clap::Command {
    use clap::Args as _;
    TidalArgs::augment_args(
        clap::Command::new("tidal").about("Tidal metadata plugin commands"),
    )
}

pub fn run_tidal(
    cfg: &Config,
    _lib: &Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> Result<()> {
    use clap::FromArgMatches as _;
    let a = TidalArgs::from_arg_matches(m)?;
    command_tidal(cfg, a.auth)
}

#[derive(clap::Args)]
pub struct TidalsyncArgs {
    /// sync albums instead of individual tracks
    #[arg(short = 'a', long)]
    album: bool,
    /// re-fetch popularity even if already present (default: False)
    #[arg(short = 'f', long)]
    force: bool,
    /// write updated tags to media files (default: False)
    #[arg(short = 'w', long)]
    write: bool,
    /// limit the number of matched items or albums
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_tidalsync() -> clap::Command {
    use clap::Args as _;
    TidalsyncArgs::augment_args(clap::Command::new("tidalsync").about(
        "Synchronize Tidal popularity data for library items and albums",
    ))
}

pub fn run_tidalsync(
    cfg: &Config,
    lib: &Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> Result<()> {
    use clap::FromArgMatches as _;
    let a = TidalsyncArgs::from_arg_matches(m)?;
    command_tidalsync(cfg, lib, a.album, a.force, a.write, a.limit, &a.query)
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "tidal",
        order: 88,
        defaults: r#"tidal:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    client_id: mcjmpl1bPATJXcBT
    tokenfile: tidal_token.json
"#,
        config_keys: &["tidal"],
        item_types: crate::plugins::tidal::ITEM_TYPES,
        album_types: crate::plugins::tidal::ALBUM_TYPES,
        redacted: &["tidal.client_id"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "tidal",
                build: build_tidal,
                run: run_tidal,
            },
            crate::plugin_registry::PluginCommand {
                plugin: "tidal",
                build: build_tidalsync,
                run: run_tidalsync,
            },
        ],
    }
}

fn registry_make(cfg: &Config) -> Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(TidalPlugin::new(cfg)))
}

// ------------------------------------------------------- metadata source

/// The tidal source as the autotagger's [`MetadataSource`]: its lookups
/// need the API client mutably, and the trait takes `&self`.
///
/// [`MetadataSource`]: crate::metadata_plugins::MetadataSource
struct TidalSource(std::cell::RefCell<Tidal>);

impl crate::metadata_plugins::MetadataSource for TidalSource {
    fn data_source(&self) -> &'static str {
        "Tidal"
    }

    fn album_for_id(
        &self,
        _ctx: &crate::metadata_plugins::SourceCtx,
        id: &str,
    ) -> Result<Option<AlbumInfo>> {
        self.0.borrow_mut().album_for_id(id)
    }

    fn track_for_id(
        &self,
        _ctx: &crate::metadata_plugins::SourceCtx,
        id: &str,
    ) -> Result<Option<TrackInfo>> {
        self.0.borrow_mut().track_for_id(id)
    }

    fn candidates(
        &self,
        _ctx: &crate::metadata_plugins::SourceCtx,
        items: &[Item],
        _artist: &str,
        _album: &str,
        _va_likely: bool,
    ) -> Result<Vec<AlbumInfo>> {
        self.0.borrow_mut().candidates(items, "", "", false)
    }

    fn item_candidates(
        &self,
        _ctx: &crate::metadata_plugins::SourceCtx,
        item: &Item,
        _artist: &str,
        _title: &str,
    ) -> Result<Vec<TrackInfo>> {
        self.0.borrow_mut().item_candidates(item, "", "")
    }
}

inventory::submit! {
    crate::metadata_plugins::MetadataSourceDef {
        // 5.21b
        order: 88,
        plugin: "tidal",
        data_source: "Tidal",
        // Loading doesn't need the token (the `import_begin` hook checks
        // it); a lookup without one fails like beets'.
        configured: |_cfg| true,
        make: |cfg| Ok(Box::new(TidalSource(std::cell::RefCell::new(Tidal::new(cfg))))),
    }
}

#[cfg(test)]
#[path = "../tidal_tests.rs"]
mod tests;
