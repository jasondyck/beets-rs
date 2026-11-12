//! MusicBrainz web service client (release + recording search and lookup).

use crate::config::Config;
use anyhow::{Context, Result, bail};
use serde_json::Value as J;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const VA_ARTIST_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";
const USER_AGENT: &str = concat!(
    "beets-rs/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/beetbox/beets )"
);

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub title: String,
    pub track_id: String,
    pub release_track_id: String,
    pub artist: String,
    pub artist_id: String,
    pub artist_sort: String,
    pub artist_credit: String,
    pub length: Option<f64>,
    /// 1-based position across the whole release.
    pub index: usize,
    pub medium: usize,
    pub medium_index: usize,
    pub medium_total: usize,
    pub disctitle: String,
    pub media: String,
    pub isrc: String,
    /// The track's number on the release ("A1", "3"): beets' `track_alt`.
    pub track_alt: Option<String>,
    pub trackdisambig: String,
    pub data_source: String,
    pub data_url: String,
    /// Fields beets would have as None (MusicBrainz gave nothing), so that
    /// a disambiguation string or `match.required` sees them as beets does.
    pub nulls: BTreeSet<&'static str>,
}

#[derive(Debug, Clone, Default)]
pub struct AlbumInfo {
    pub album_id: String,
    pub album: String,
    pub artist: String,
    pub artist_id: String,
    pub artist_sort: String,
    pub artist_credit: String,
    pub va: bool,
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub original_year: i64,
    pub original_month: i64,
    pub original_day: i64,
    pub country: String,
    pub label: String,
    pub catalognum: String,
    pub albumtype: String,
    pub albumstatus: String,
    pub albumdisambig: String,
    pub releasegroup_id: String,
    pub releasegroupdisambig: String,
    pub media: String,
    pub mediums: usize,
    pub tracks: Vec<TrackInfo>,
    pub barcode: String,
    pub asin: String,
    pub script: String,
    pub language: String,
    pub data_source: String,
    pub data_url: String,
    /// Fields beets would have as None (see [`TrackInfo::nulls`]).
    pub nulls: BTreeSet<&'static str>,
}

/// Python's `str()` of a float: always with a decimal point.
fn py_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

/// A string field for [`AlbumInfo::field`]: `Some(None)` if it's None.
fn shown(nulls: &BTreeSet<&'static str>, name: &'static str, v: String) -> Option<Option<String>> {
    Some((!nulls.contains(name)).then_some(v))
}

fn int_or_null(v: i64) -> Option<String> {
    (v != 0).then(|| v.to_string())
}

impl AlbumInfo {
    /// beets' `info[name]`, as `str()` shows it: None if the info has no
    /// such field, `Some(None)` if the field is None.
    pub fn field(&self, name: &str) -> Option<Option<String>> {
        let n = &self.nulls;
        Some(match name {
            "album" => Some(self.album.clone()),
            "album_id" => Some(self.album_id.clone()),
            "artist" => Some(self.artist.clone()),
            "artist_id" => Some(self.artist_id.clone()),
            "artist_sort" => Some(self.artist_sort.clone()),
            "artist_credit" => Some(self.artist_credit.clone()),
            "va" => Some(if self.va { "True" } else { "False" }.into()),
            "year" => int_or_null(self.year),
            "month" => int_or_null(self.month),
            "day" => int_or_null(self.day),
            "original_year" => int_or_null(self.original_year),
            "original_month" => int_or_null(self.original_month),
            "original_day" => int_or_null(self.original_day),
            "mediums" => Some(self.mediums.to_string()),
            "country" => return shown(n, "country", self.country.clone()),
            "label" => return shown(n, "label", self.label.clone()),
            "catalognum" => return shown(n, "catalognum", self.catalognum.clone()),
            "albumtype" => return shown(n, "albumtype", self.albumtype.clone()),
            "albumstatus" => return shown(n, "albumstatus", self.albumstatus.clone()),
            "albumdisambig" => return shown(n, "albumdisambig", self.albumdisambig.clone()),
            "releasegroup_id" => Some(self.releasegroup_id.clone()),
            "releasegroupdisambig" => {
                return shown(n, "releasegroupdisambig", self.releasegroupdisambig.clone());
            }
            "media" => return shown(n, "media", self.media.clone()),
            "barcode" => return shown(n, "barcode", self.barcode.clone()),
            "asin" => return shown(n, "asin", self.asin.clone()),
            "script" => return shown(n, "script", self.script.clone()),
            "language" => return shown(n, "language", self.language.clone()),
            "data_source" => return shown(n, "data_source", self.data_source.clone()),
            "data_url" => return shown(n, "data_url", self.data_url.clone()),
            _ => return None,
        })
    }
}

impl TrackInfo {
    /// beets' `info[name]` for a track (see [`AlbumInfo::field`]).
    pub fn field(&self, name: &str) -> Option<Option<String>> {
        let n = &self.nulls;
        let num = |v: usize| (v != 0).then(|| v.to_string());
        Some(match name {
            "title" => Some(self.title.clone()),
            "track_id" => Some(self.track_id.clone()),
            "release_track_id" => Some(self.release_track_id.clone()).filter(|s| !s.is_empty()),
            "artist" => Some(self.artist.clone()),
            "artist_id" => Some(self.artist_id.clone()),
            "artist_sort" => Some(self.artist_sort.clone()),
            "artist_credit" => Some(self.artist_credit.clone()),
            "length" => self.length.map(py_float),
            "index" => num(self.index),
            "medium" => num(self.medium),
            "medium_index" => (self.index != 0).then(|| self.medium_index.to_string()),
            "medium_total" => num(self.medium_total),
            "disctitle" => return shown(n, "disctitle", self.disctitle.clone()),
            "media" => return shown(n, "media", self.media.clone()),
            "track_alt" => self.track_alt.clone(),
            "trackdisambig" => return shown(n, "trackdisambig", self.trackdisambig.clone()),
            "isrc" => return shown(n, "isrc", self.isrc.clone()),
            // beets 2.14's MusicBrainz source never fills a track's album.
            "album" => None,
            "data_source" => return shown(n, "data_source", self.data_source.clone()),
            "data_url" => return shown(n, "data_url", self.data_url.clone()),
            _ => return None,
        })
    }
}

pub struct MusicBrainz {
    agent: ureq::Agent,
    base: String,
    interval: Duration,
    /// The last request's time; locked while waiting for the next slot, so
    /// threads sharing the client share the rate limit.
    last: Mutex<Option<Instant>>,
    pub search_limit: usize,
    opts: ParseOpts,
}

/// Which MusicBrainz alias to use for names and titles: `import.languages`,
/// `import.ignored_alias_types` and `musicbrainz.aliases_as_credits`.
#[derive(Debug, Clone, Default)]
pub struct AliasPrefs {
    pub languages: Vec<String>,
    /// Lower-cased alias types never used.
    pub ignored_types: Vec<String>,
    /// Use the alias for the artist credit too, not just name and sort name.
    pub as_credits: bool,
}

impl AliasPrefs {
    pub fn from_config(cfg: &Config) -> Self {
        AliasPrefs {
            languages: cfg.list("import.languages"),
            ignored_types: cfg
                .list("import.ignored_alias_types")
                .iter()
                .map(|t| t.to_lowercase())
                .collect(),
            as_credits: cfg.bool("musicbrainz.aliases_as_credits"),
        }
    }

    /// beets' `_preferred_alias`: the first primary alias for the first
    /// configured locale that has one, skipping ignored alias types.
    fn preferred<'j>(&self, obj: &'j J) -> Option<&'j J> {
        let aliases = obj.get("aliases")?.as_array()?;
        self.languages.iter().find_map(|locale| {
            aliases.iter().find(|al| {
                al.get("locale").and_then(|l| l.as_str()) == Some(locale.as_str())
                    && al.get("primary").and_then(|p| p.as_bool()) == Some(true)
                    && !self.ignored_types.contains(&s(al, "type").to_lowercase())
            })
        })
    }

    /// An object's title, or its preferred alias's name.
    fn title(&self, obj: &J) -> String {
        match self.preferred(obj) {
            Some(al) => s(al, "name"),
            None => s(obj, "title"),
        }
    }
}

/// What shapes a release beyond aliases: beets' `match.ignored_media`,
/// `ignore_data_tracks`, `ignore_video_tracks` and, for the release event,
/// `match.preferred.countries`.
#[derive(Debug, Clone)]
pub struct ParseOpts {
    pub aliases: AliasPrefs,
    pub ignored_media: Vec<String>,
    pub ignore_data_tracks: bool,
    pub ignore_video_tracks: bool,
    pub preferred_countries: Vec<String>,
}

impl Default for ParseOpts {
    fn default() -> Self {
        ParseOpts {
            aliases: AliasPrefs::default(),
            ignored_media: vec![],
            ignore_data_tracks: true,
            ignore_video_tracks: true,
            preferred_countries: vec![],
        }
    }
}

impl ParseOpts {
    pub fn from_config(cfg: &Config) -> Self {
        ParseOpts {
            aliases: AliasPrefs::from_config(cfg),
            ignored_media: cfg.list("match.ignored_media"),
            ignore_data_tracks: cfg.bool("match.ignore_data_tracks"),
            ignore_video_tracks: cfg.bool("match.ignore_video_tracks"),
            preferred_countries: cfg.list("match.preferred.countries"),
        }
    }
}

/// A string that's None in beets when null or missing.
fn opt_s(v: &J, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(String::from)
}

fn s(v: &J, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Lucene-escape a search term.
fn escape(term: &str) -> String {
    let mut out = String::new();
    for c in term.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn parse_date(d: &str) -> (i64, i64, i64) {
    let mut it = d.split('-').map(|p| p.parse::<i64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Flatten an artist-credit list into (display name, first artist id, sort
/// name, credit name), using each artist's preferred alias for the name and
/// sort name (and the credit, with `aliases_as_credits`).
fn artist_credit(ac: Option<&J>, prefs: &AliasPrefs) -> (String, String, String, String) {
    let Some(list) = ac.and_then(|a| a.as_array()) else {
        return Default::default();
    };
    let (mut name, mut sort, mut credit, mut id) =
        (String::new(), String::new(), String::new(), String::new());
    for c in list {
        let artist = c.get("artist").cloned().unwrap_or(J::Null);
        let join = s(c, "joinphrase");
        let alias = prefs.preferred(&artist);
        let named = alias.unwrap_or(&artist);
        let aname = s(named, "name");
        let cname = match alias {
            Some(al) if prefs.as_credits => s(al, "name"),
            _ => c
                .get("name")
                .and_then(|x| x.as_str())
                .map(String::from)
                .unwrap_or(aname.clone()),
        };
        name.push_str(&aname);
        name.push_str(&join);
        credit.push_str(&cname);
        credit.push_str(&join);
        let sn = s(named, "sort-name");
        sort.push_str(if sn.is_empty() { &aname } else { &sn });
        sort.push_str(&join);
        if id.is_empty() {
            id = s(&artist, "id");
        }
    }
    (name, id, sort, credit)
}

impl MusicBrainz {
    pub fn new(cfg: &Config) -> Self {
        let scheme = if cfg.bool("musicbrainz.https") {
            "https"
        } else {
            "http"
        };
        let host = cfg.str("musicbrainz.host");
        let rate = cfg.f64("musicbrainz.ratelimit").max(0.0);
        let per = cfg.f64("musicbrainz.ratelimit_interval").max(0.0);
        let interval = if rate > 0.0 {
            Duration::from_secs_f64(per / rate)
        } else {
            Duration::ZERO
        };
        MusicBrainz {
            agent: ureq::AgentBuilder::new()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build(),
            base: format!("{scheme}://{host}/ws/2"),
            interval,
            last: Mutex::new(None),
            search_limit: cfg.f64("musicbrainz.searchlimit").max(1.0) as usize,
            opts: ParseOpts::from_config(cfg),
        }
    }

    fn throttle(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < self.interval {
                std::thread::sleep(self.interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<J> {
        let url = format!("{}/{}", self.base, path);
        crate::logging::trace!("musicbrainz: GET {url} {query:?}");
        let mut attempt = 0;
        loop {
            self.throttle();
            let mut req = self.agent.get(&url).query("fmt", "json");
            for (k, v) in query {
                req = req.query(k, v);
            }
            match req.call() {
                Ok(resp) => return resp.into_json().context("parsing MusicBrainz response"),
                Err(ureq::Error::Status(503 | 429, _)) if attempt < 4 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(attempt));
                }
                Err(ureq::Error::Status(404, _)) => bail!("not found on MusicBrainz: {path}"),
                Err(e) => return Err(e).with_context(|| format!("MusicBrainz request {path}")),
            }
        }
    }

    /// Search releases; returns MBIDs in score order.
    pub fn search_releases(&self, artist: &str, album: &str, tracks: usize) -> Result<Vec<String>> {
        let mut parts = vec![];
        if !album.trim().is_empty() {
            parts.push(format!("release:({})", escape(album.trim())));
        }
        if !artist.trim().is_empty() {
            parts.push(format!("artist:({})", escape(artist.trim())));
        }
        if parts.is_empty() {
            return Ok(vec![]);
        }
        if tracks > 0 {
            parts.push(format!("tracks:{tracks}"));
        }
        let q = parts.join(" AND ");
        let limit = self.search_limit.to_string();
        crate::logging::debug!("Searching for MusicBrainz releases with: {q}");
        let res = self.get("release/", &[("query", &q), ("limit", &limit)])?;
        Ok(res
            .get("releases")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .map(|r| s(r, "id"))
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn release(&self, id: &str) -> Result<AlbumInfo> {
        crate::logging::debug!("Requesting MusicBrainz release {id}");
        let j = self.get(
            &format!("release/{id}"),
            &[(
                "inc",
                "recordings artist-credits labels release-groups isrcs aliases",
            )],
        )?;
        Ok(parse_release(&j, &self.opts))
    }

    pub fn search_recordings(&self, artist: &str, title: &str) -> Result<Vec<TrackInfo>> {
        let mut parts = vec![];
        if !title.trim().is_empty() {
            parts.push(format!("recording:({})", escape(title.trim())));
        }
        if !artist.trim().is_empty() {
            parts.push(format!("artist:({})", escape(artist.trim())));
        }
        if parts.is_empty() {
            return Ok(vec![]);
        }
        let limit = self.search_limit.to_string();
        crate::logging::debug!(
            "Searching for MusicBrainz recordings with: {}",
            parts.join(" AND ")
        );
        let res = self.get(
            "recording/",
            &[("query", &parts.join(" AND ")), ("limit", &limit)],
        )?;
        Ok(res
            .get("recordings")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .map(|r| parse_recording(r, &self.opts.aliases))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn recording(&self, id: &str) -> Result<TrackInfo> {
        crate::logging::debug!("Requesting MusicBrainz recording {id}");
        let j = self.get(
            &format!("recording/{id}"),
            &[("inc", "artist-credits isrcs aliases")],
        )?;
        Ok(parse_recording(&j, &self.opts.aliases))
    }
}

/// beets' `track_info`: a recording as a track. Positional fields (index,
/// medium, ...) are None until the release sets them.
pub fn parse_recording(r: &J, prefs: &AliasPrefs) -> TrackInfo {
    let (artist, artist_id, artist_sort, credit) = artist_credit(r.get("artist-credit"), prefs);
    let isrcs: Vec<&str> = r
        .get("isrcs")
        .and_then(|i| i.as_array())
        .map(|a| a.iter().filter_map(|i| i.as_str()).collect())
        .unwrap_or_default();
    let mut nulls: BTreeSet<&'static str> = ["disctitle", "media"].into();
    if isrcs.is_empty() {
        nulls.insert("isrc");
    }
    let trackdisambig = s(r, "disambiguation");
    if trackdisambig.is_empty() {
        nulls.insert("trackdisambig");
    }
    let id = s(r, "id");
    TrackInfo {
        title: prefs.title(r),
        data_url: format!("https://musicbrainz.org/recording/{id}"),
        track_id: id,
        artist,
        artist_id,
        artist_sort,
        artist_credit: credit,
        length: r
            .get("length")
            .and_then(|l| l.as_f64())
            .filter(|l| *l != 0.0)
            .map(|ms| ms / 1000.0),
        isrc: isrcs.join(";"),
        trackdisambig,
        data_source: "MusicBrainz".into(),
        nulls,
        ..Default::default()
    }
}

/// beets' `_album_media`: the media's shared format, else their common base
/// when they differ only in a parenthetical, else "Media". None (empty) when
/// the formats are unknown.
fn album_media(medias: &BTreeSet<Option<String>>) -> Option<String> {
    if medias.len() == 1 {
        return medias.iter().next().cloned().flatten();
    }
    let bases: BTreeSet<Option<String>> = medias
        .iter()
        .map(|m| {
            m.as_ref()
                .map(|m| m.split(" (").next().unwrap_or("").to_string())
        })
        .collect();
    match bases.len() {
        1 => bases.into_iter().next().flatten(),
        0 => None,
        _ => Some("Media".into()),
    }
}

/// beets' `get_tracks_from_medium`: the pregap, the tracks and (unless
/// ignored) the data tracks, without "[data track]" placeholders or (with
/// `ignore_video_tracks`) videos.
fn medium_tracks(medium: &J, opts: &ParseOpts) -> Vec<TrackInfo> {
    let arr = |k: &str| {
        medium
            .get(k)
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let mut all = vec![];
    if let Some(p) = medium.get("pregap").filter(|p| !p.is_null()) {
        all.push(p.clone());
    }
    all.extend(arr("tracks"));
    if !opts.ignore_data_tracks {
        all.extend(arr("data-tracks"));
    }
    let valid: Vec<J> = all
        .into_iter()
        .filter(|t| {
            let rec = t.get("recording").cloned().unwrap_or(J::Null);
            s(&rec, "title") != "[data track]"
                && !(opts.ignore_video_tracks
                    && rec.get("video").and_then(|v| v.as_bool()) == Some(true))
        })
        .collect();
    let format = opt_s(medium, "format");
    let disctitle = opt_s(medium, "title");
    let position = medium.get("position").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
    let total = valid.len();
    let prefs = &opts.aliases;
    valid
        .iter()
        .map(|t| {
            let rec = t.get("recording").cloned().unwrap_or(J::Null);
            let mut info = parse_recording(&rec, prefs);
            // The release's track title/credit/length win over the
            // recording's, unless the recording has a preferred alias.
            let title = s(t, "title");
            if !title.is_empty() && prefs.preferred(&rec).is_none() {
                info.title = title;
            }
            if t.get("artist-credit")
                .and_then(|a| a.as_array())
                .is_some_and(|a| !a.is_empty())
            {
                let (a, aid, asort, acred) = artist_credit(t.get("artist-credit"), prefs);
                info.artist = a;
                info.artist_id = aid;
                info.artist_sort = asort;
                info.artist_credit = acred;
            }
            if let Some(l) = t
                .get("length")
                .and_then(|l| l.as_f64())
                .filter(|l| *l != 0.0)
            {
                info.length = Some(l / 1000.0);
            }
            info.release_track_id = s(t, "id");
            info.track_alt = opt_s(t, "number");
            info.medium = position;
            info.medium_index = t
                .get("position")
                .and_then(|p| p.as_u64().or_else(|| p.as_str()?.parse().ok()))
                .unwrap_or(0) as usize;
            info.medium_total = total;
            info.nulls.remove("disctitle");
            info.nulls.remove("media");
            match &disctitle {
                Some(d) => info.disctitle = d.clone(),
                None => {
                    info.nulls.insert("disctitle");
                }
            }
            match &format {
                Some(f) => info.media = f.clone(),
                None => {
                    info.nulls.insert("media");
                }
            }
            info
        })
        .collect()
}

/// beets' `_preferred_release_event`: the first preferred country with a
/// release event there, and that event's date; else the release's own.
fn release_event(j: &J, preferred: &[String]) -> (Option<String>, Option<String>) {
    let events = j
        .get("release-events")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    for country in preferred {
        for e in &events {
            let codes = e
                .get("area")
                .and_then(|a| a.get("iso-3166-1-codes"))
                .and_then(|c| c.as_array());
            if codes.is_some_and(|c| c.iter().any(|c| c.as_str() == Some(country))) {
                return (Some(country.clone()), opt_s(e, "date"));
            }
        }
    }
    (opt_s(j, "country"), opt_s(j, "date"))
}

/// beets' `album_info` for a release.
pub fn parse_release(j: &J, opts: &ParseOpts) -> AlbumInfo {
    let prefs = &opts.aliases;
    let (artist, artist_id, artist_sort, artist_credit_s) =
        artist_credit(j.get("artist-credit"), prefs);
    let rg = j.get("release-group").cloned().unwrap_or(J::Null);
    let (oy, om, od) = parse_date(&s(&rg, "first-release-date"));
    let mut nulls = BTreeSet::new();
    let mut null_if = |name: &'static str, v: Option<String>| match v {
        Some(v) => v,
        None => {
            nulls.insert(name);
            String::new()
        }
    };
    // beets' `_parse_label_infos`: the first label info.
    let label_info = j
        .get("label-info")
        .and_then(|l| l.as_array())
        .and_then(|a| a.first())
        .cloned();
    let label = label_info
        .as_ref()
        .and_then(|li| li.get("label"))
        .and_then(|l| opt_s(l, "name"))
        .filter(|n| n != "[no label]");
    let catalognum = label_info
        .as_ref()
        .and_then(|li| opt_s(li, "catalog-number"));
    let label = null_if("label", label);
    let catalognum = null_if("catalognum", catalognum);

    let media_list = j
        .get("media")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    let mediums = media_list.len();
    let mut tracks = vec![];
    for medium in &media_list {
        let fmt = s(medium, "format");
        if opts.ignored_media.contains(&fmt) {
            continue;
        }
        tracks.extend(medium_tracks(medium, opts));
    }
    for (i, t) in tracks.iter_mut().enumerate() {
        t.index = i + 1;
    }
    let medias: BTreeSet<Option<String>> = tracks
        .iter()
        .map(|t| (!t.nulls.contains("media")).then(|| t.media.clone()))
        .collect();
    let media = null_if("media", album_media(&medias));

    let (country, date) = release_event(j, &opts.preferred_countries);
    let country = null_if("country", country);
    let (year, month, day) = match date.filter(|d| !d.is_empty()) {
        Some(d) => parse_date(&d),
        None => (oy, om, od),
    };
    let albumdisambig = null_if(
        "albumdisambig",
        opt_s(j, "disambiguation").filter(|d| !d.is_empty()),
    );
    let releasegroupdisambig = null_if(
        "releasegroupdisambig",
        opt_s(&rg, "disambiguation").filter(|d| !d.is_empty()),
    );
    let albumtype = null_if(
        "albumtype",
        opt_s(&rg, "primary-type")
            .filter(|t| !t.is_empty())
            .map(|t| t.to_lowercase()),
    );
    let albumstatus = null_if("albumstatus", opt_s(j, "status").map(|t| t.to_lowercase()));
    let text = j.get("text-representation").cloned().unwrap_or(J::Null);
    let script = null_if("script", opt_s(&text, "script"));
    let language = null_if("language", opt_s(&text, "language"));
    let barcode = null_if("barcode", opt_s(j, "barcode"));
    let asin = null_if("asin", opt_s(j, "asin"));
    let id = s(j, "id");
    AlbumInfo {
        data_url: format!("https://musicbrainz.org/release/{id}"),
        album_id: id,
        album: prefs.title(j),
        va: artist_id == VA_ARTIST_ID,
        artist,
        artist_id,
        artist_sort,
        artist_credit: artist_credit_s,
        year,
        month,
        day,
        original_year: oy,
        original_month: om,
        original_day: od,
        country,
        label,
        catalognum,
        albumtype,
        albumstatus,
        albumdisambig,
        releasegroup_id: s(&rg, "id"),
        releasegroupdisambig,
        media,
        mediums,
        tracks,
        barcode,
        asin,
        script,
        language,
        data_source: "MusicBrainz".into(),
        nulls,
    }
}

/// Pull an MBID out of a bare id or a musicbrainz.org URL.
pub fn extract_id(s: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
    )
    .unwrap();
    re.find(s).map(|m| m.as_str().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_json() {
        let j: J = serde_json::from_str(
            r#"{
            "id": "rel-1", "title": "Album", "date": "2003-05-12", "country": "GB", "status": "Official",
            "artist-credit": [{"name": "A", "joinphrase": " & ", "artist": {"id": "a1", "name": "A", "sort-name": "A, The"}},
                              {"name": "B", "joinphrase": "", "artist": {"id": "b1", "name": "B", "sort-name": "B"}}],
            "release-group": {"id": "rg", "primary-type": "Album", "first-release-date": "2001"},
            "label-info": [{"catalog-number": "CAT1", "label": {"name": "Lbl"}}],
            "media": [
              {"position": 1, "format": "CD", "tracks": [
                {"id": "t1", "position": 1, "title": "One", "length": 180000, "recording": {"id": "r1", "title": "One"}},
                {"id": "t2", "position": 2, "title": "Two", "length": null, "recording": {"id": "r2", "title": "Two", "length": 200000}}]},
              {"position": 2, "format": "CD", "title": "Bonus", "tracks": [
                {"id": "t3", "position": 1, "title": "Three", "recording": {"id": "r3", "title": "Three"},
                 "artist-credit": [{"name": "C", "artist": {"id": "c1", "name": "C"}}]}]}
            ]}"#,
        )
        .unwrap();
        let a = parse_release(&j, &ParseOpts::default());
        assert_eq!(a.artist, "A & B");
        assert_eq!(a.artist_sort, "A, The & B");
        assert_eq!(a.artist_id, "a1");
        assert_eq!(
            (a.year, a.month, a.day, a.original_year),
            (2003, 5, 12, 2001)
        );
        assert_eq!(a.label, "Lbl");
        assert_eq!(a.catalognum, "CAT1");
        assert_eq!(a.albumtype, "album");
        assert_eq!(a.media, "CD");
        assert_eq!(a.mediums, 2);
        assert_eq!(a.tracks.len(), 3);
        assert_eq!(a.tracks[1].length, Some(200.0));
        assert_eq!(a.tracks[2].index, 3);
        assert_eq!(a.tracks[2].medium, 2);
        assert_eq!(a.tracks[2].medium_index, 1);
        assert_eq!(a.tracks[2].artist, "C");
        assert_eq!(a.tracks[2].disctitle, "Bonus");
    }

    #[test]
    fn preferred_aliases_follow_languages_and_ignored_types() {
        let j: J = serde_json::from_str(
            r#"{
            "id": "rel", "title": "Arubamu",
            "aliases": [{"name": "Album (en)", "locale": "en", "primary": true, "type": "Release name"}],
            "artist-credit": [{"name": "Kurejitto", "joinphrase": "", "artist": {
                "id": "a1", "name": "Aatisuto", "sort-name": "Aatisuto",
                "aliases": [
                  {"name": "Not primary", "sort-name": "x", "locale": "en", "primary": false, "type": "Artist name"},
                  {"name": "Search", "sort-name": "Search", "locale": "de", "primary": true, "type": "Search hint"},
                  {"name": "Artist", "sort-name": "Artist, The", "locale": "en", "primary": true, "type": "Artist name"},
                  {"name": "Kuenstler", "sort-name": "Kuenstler", "locale": "de", "primary": true, "type": "Artist name"}]}}],
            "media": [{"position": 1, "tracks": [
                {"id": "t1", "position": 1, "title": "Torakku", "recording": {"id": "r1", "title": "Torakku",
                  "aliases": [{"name": "Track", "locale": "en", "primary": true, "type": null}]}},
                {"id": "t2", "position": 2, "title": "Track title", "recording": {"id": "r2", "title": "Rec title"}}]}]
            }"#,
        )
        .unwrap();
        let prefs = |langs: &[&str], ignored: &[&str], as_credits| ParseOpts {
            aliases: AliasPrefs {
                languages: langs.iter().map(|s| s.to_string()).collect(),
                ignored_types: ignored.iter().map(|s| s.to_string()).collect(),
                as_credits,
            },
            ..Default::default()
        };
        // No languages: the plain names, and track titles win.
        let a = parse_release(&j, &ParseOpts::default());
        assert_eq!(
            (a.artist.as_str(), a.album.as_str()),
            ("Aatisuto", "Arubamu")
        );
        assert_eq!(a.artist_credit, "Kurejitto");
        assert_eq!(a.tracks[0].title, "Torakku");
        // English: the primary English aliases for artist, sort, album and
        // the recording (which then beats the track title).
        let a = parse_release(&j, &prefs(&["fr", "en", "de"], &[], false));
        assert_eq!(a.artist, "Artist");
        assert_eq!(a.artist_sort, "Artist, The");
        assert_eq!(a.artist_credit, "Kurejitto");
        assert_eq!(a.album, "Album (en)");
        assert_eq!(a.tracks[0].title, "Track");
        assert_eq!(a.tracks[1].title, "Track title");
        // German, with search hints ignored (case-insensitively), and the
        // alias used for the credit too.
        let a = parse_release(&j, &prefs(&["de"], &["search hint"], true));
        assert_eq!(a.artist, "Kuenstler");
        assert_eq!(a.artist_credit, "Kuenstler");
        assert_eq!(a.album, "Arubamu");
    }

    fn fixture(id: &str, opts: &ParseOpts) -> AlbumInfo {
        let path = format!(
            "{}/tests/fixtures/mb/release-{id}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let j: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        parse_release(&j, opts)
    }

    type Row = (usize, usize, usize, usize, String, String);

    fn rows(a: &AlbumInfo) -> Vec<Row> {
        a.tracks
            .iter()
            .map(|t| {
                (
                    t.index,
                    t.medium,
                    t.medium_index,
                    t.medium_total,
                    t.title.clone(),
                    t.track_alt.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    fn row(r: (usize, usize, usize, usize, &str, &str)) -> Row {
        (r.0, r.1, r.2, r.3, r.4.to_string(), r.5.to_string())
    }

    /// The fixture's tracks as reference beets 2.14 reads them: the pregap
    /// kept, "[data track]" placeholders, data tracks and videos left out
    /// by default, ignored media skipped (still counted in `mediums`).
    #[test]
    fn release_tracks_as_beets() {
        const ID: &str = "11111111-1111-1111-1111-111111111111";
        let a = fixture(ID, &ParseOpts::default());
        assert_eq!((a.media.as_str(), a.mediums), ("Media", 2));
        assert_eq!(
            rows(&a),
            [
                (1, 1, 0, 4, "Hidden Intro", "0"),
                (2, 1, 1, 4, "Alpha", "1"),
                (3, 1, 2, 4, "Beta", "2"),
                (4, 1, 3, 4, "Gamma", "3"),
                (5, 2, 2, 1, "Interview", "2"),
            ]
            .map(row)
        );
        assert_eq!(a.tracks[4].media, "DVD-Video");
        assert_eq!((a.year, a.country.as_str()), (2004, "GB"));
        assert!(a.nulls.contains("albumdisambig"));

        let all = ParseOpts {
            ignore_data_tracks: false,
            ignore_video_tracks: false,
            ..Default::default()
        };
        assert_eq!(
            rows(&fixture(ID, &all)),
            [
                (1, 1, 0, 5, "Hidden Intro", "0"),
                (2, 1, 1, 5, "Alpha", "1"),
                (3, 1, 2, 5, "Beta", "2"),
                (4, 1, 3, 5, "Gamma", "3"),
                (5, 1, 5, 5, "Data Extra", "5"),
                (6, 2, 1, 2, "Alpha (video)", "1"),
                (7, 2, 2, 2, "Interview", "2"),
            ]
            .map(row)
        );

        let no_dvd = ParseOpts {
            ignored_media: vec!["DVD-Video".into()],
            ..Default::default()
        };
        let a = fixture(ID, &no_dvd);
        assert_eq!((a.media.as_str(), a.mediums, a.tracks.len()), ("CD", 2, 4));

        // No release date: the release group's; no label info: None.
        let d = fixture(
            "22222222-2222-2222-2222-222222222222",
            &ParseOpts::default(),
        );
        assert_eq!((d.year, d.month, d.original_year), (2003, 0, 2003));
        assert!(d.nulls.contains("label") && d.nulls.contains("catalognum"));
        assert!(!d.nulls.contains("country"));
        assert_eq!(d.albumdisambig, "deluxe edition");
    }

    #[test]
    fn preferred_countries_pick_the_release_event() {
        let j: J = serde_json::from_str(
            r#"{"id": "r", "title": "T", "country": "US", "date": "2001-02-03",
                "release-events": [
                  {"date": "2001-02-03", "area": {"iso-3166-1-codes": ["US"]}},
                  {"date": "2002", "area": {"iso-3166-1-codes": ["GB"]}}],
                "media": []}"#,
        )
        .unwrap();
        let with = |c: &[&str]| {
            let a = parse_release(
                &j,
                &ParseOpts {
                    preferred_countries: c.iter().map(|s| s.to_string()).collect(),
                    ..Default::default()
                },
            );
            (a.country, a.year, a.month)
        };
        assert_eq!(with(&[]), ("US".into(), 2001, 2));
        assert_eq!(with(&["GB", "US"]), ("GB".into(), 2002, 0));
        assert_eq!(with(&["JP"]), ("US".into(), 2001, 2));
    }

    #[test]
    fn ids() {
        assert_eq!(
            extract_id("https://musicbrainz.org/release/0FA1E0C7-1C4A-4F2B-9A5E-ABCDEF012345")
                .as_deref(),
            Some("0fa1e0c7-1c4a-4f2b-9a5e-abcdef012345")
        );
        assert_eq!(extract_id("nope"), None);
        assert_eq!(escape("AC/DC: (live)"), "AC\\/DC\\: \\(live\\)");
    }
}
