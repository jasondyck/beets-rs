//! Matching items against MusicBrainz candidates, following beets' autotag
//! design: weighted penalty distances, optimal item↔track assignment and a
//! strong/medium/low recommendation.

use crate::config::Config;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo, extract_id};
use crate::model::{Item, Value};
use anyhow::Result;
use deunicode::deunicode;
use pathfinding::prelude::{Matrix, kuhn_munkres_min};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

// ---------------------------------------------------------------- distance

#[derive(Debug, Clone, Default)]
pub struct Distance {
    penalties: BTreeMap<String, Vec<f64>>,
    weights: HashMap<String, f64>,
}

impl Distance {
    pub fn new(cfg: &Config) -> Self {
        let weights = cfg
            .mapping("match.distance_weights")
            .into_iter()
            .map(|(k, v)| (k, v.parse().unwrap_or(0.0)))
            .collect();
        Distance {
            penalties: BTreeMap::new(),
            weights,
        }
    }

    fn weight(&self, key: &str) -> f64 {
        *self.weights.get(key).unwrap_or(&0.0)
    }

    pub fn add(&mut self, key: &str, penalty: f64) {
        self.penalties
            .entry(key.to_string())
            .or_default()
            .push(penalty.clamp(0.0, 1.0));
    }

    pub fn add_string(&mut self, key: &str, a: &str, b: &str) {
        self.add(key, string_dist(a, b));
    }

    /// Penalty by position in a preference list: the first option is free,
    /// later ones cost more, and a value matching none costs 1.0. Options are
    /// case-insensitive regexes, so `GB|UK` works as in beets.
    pub fn add_priority(&mut self, key: &str, value: &str, options: &[String]) {
        let unit = 1.0 / options.len().max(1) as f64;
        for (i, opt) in options.iter().enumerate() {
            let hit = Regex::new(&format!("(?i)^(?:{opt})$")).is_ok_and(|re| re.is_match(value));
            if hit {
                self.add(key, i as f64 * unit);
                return;
            }
        }
        self.add(key, 1.0);
    }

    pub fn add_bool(&mut self, key: &str, mismatch: bool) {
        self.add(key, if mismatch { 1.0 } else { 0.0 });
    }

    fn raw(&self) -> f64 {
        self.penalties
            .iter()
            .map(|(k, p)| self.weight(k) * p.iter().sum::<f64>())
            .sum()
    }

    fn max(&self) -> f64 {
        self.penalties
            .iter()
            .map(|(k, p)| self.weight(k) * p.len() as f64)
            .sum()
    }

    /// Normalized distance in [0, 1].
    pub fn distance(&self) -> f64 {
        let max = self.max();
        if max == 0.0 { 0.0 } else { self.raw() / max }
    }

    /// beets' `generic_penalty_keys`: the keys with a penalty, without
    /// their `album_`/`track_` prefix, underscores as spaces.
    pub fn generic_penalty_keys(&self) -> Vec<String> {
        self.penalties
            .iter()
            .filter(|(_, p)| p.iter().any(|v| *v > 0.0))
            .map(|(k, _)| {
                k.replace("album_", "")
                    .replace("track_", "")
                    .replace('_', " ")
            })
            .collect()
    }

    pub fn has(&self, key: &str) -> bool {
        self.penalties
            .get(key)
            .is_some_and(|p| p.iter().any(|v| *v > 0.0))
    }
}

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^the ",
        r"[\[\(]?(ep|single)[\]\)]?",
        r"[\[\(]?(featuring|feat|ft)[\. :].+",
        r"\(.*?\)",
        r"\[.*?\]",
        r"(, )?(pt\.|part) .+",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});
static NONWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

fn normalize(s: &str) -> String {
    let mut s = deunicode(s).to_lowercase();
    // "Beatles, The" -> "the beatles"
    for suffix in [", the", ", a", ", an"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = format!("{} {}", &suffix[2..], stripped);
        }
    }
    s.replace('&', "and")
}

fn lev(a: &str, b: &str) -> f64 {
    let a = NONWORD.replace_all(a, "");
    let b = NONWORD.replace_all(b, "");
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    1.0 - strsim::normalized_levenshtein(&a, &b)
}

/// beets-style string distance in [0, 1]: case/diacritic/punctuation
/// insensitive, with reduced weight for "noise" like "(Remastered)" or "feat. X".
pub fn string_dist(a: &str, b: &str) -> f64 {
    let (a, b) = (normalize(a), normalize(b));
    if a == b {
        return 0.0;
    }
    let (mut sa, mut sb) = (a.clone(), b.clone());
    for p in PATTERNS.iter() {
        sa = p.replace_all(&sa, "").into_owned();
        sb = p.replace_all(&sb, "").into_owned();
    }
    let base = lev(&sa, &sb);
    let full = lev(&a, &b);
    (base + 0.3 * (full - base).max(0.0)).min(1.0)
}

// ------------------------------------------------------------ current state

/// Most common non-empty value of a field across items, and whether all items agree.
fn likely(items: &[Item], field: &str) -> (String, bool) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for i in items {
        *counts
            .entry(i.formatted(field).unwrap_or_default())
            .or_default() += 1;
    }
    let consensus = counts.len() == 1;
    let best = counts
        .into_iter()
        .filter(|(v, _)| !v.is_empty() && v != "0000" && v != "0")
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(v, _)| v)
        .unwrap_or_default();
    (best, consensus)
}

#[derive(Debug, Clone, Default)]
pub struct Current {
    pub artist: String,
    pub album: String,
    pub year: i64,
    pub mb_albumid: String,
    pub va: bool,
}

pub fn current_metadata(items: &[Item]) -> Current {
    let (albumartist, _) = likely(items, "albumartist");
    let (artist, artist_consensus) = likely(items, "artist");
    let (album, _) = likely(items, "album");
    let (year, _) = likely(items, "year");
    let (mbid, mbid_consensus) = likely(items, "mb_albumid");
    let va = items.len() > 1 && !artist_consensus && albumartist.is_empty()
        || items.iter().any(|i| i.bool("comp"));
    Current {
        artist: if albumartist.is_empty() {
            artist
        } else {
            albumartist
        },
        album,
        year: year.parse().unwrap_or(0),
        mb_albumid: if mbid_consensus { mbid } else { String::new() },
        va,
    }
}

// --------------------------------------------------------- track distances

pub fn track_distance(cfg: &Config, item: &Item, track: &TrackInfo, incl_artist: bool) -> Distance {
    let mut d = Distance::new(cfg);
    if let Some(len) = track.length {
        let grace = cfg.f64("match.track_length_grace");
        let max = cfg.f64("match.track_length_max").max(1.0);
        let diff = ((item.float("length") - len).abs() - grace).max(0.0);
        d.add("track_length", diff / max);
    }
    d.add_string("track_title", item.str("title"), &track.title);
    if incl_artist && !track.artist.is_empty() {
        let a = item.str("artist");
        if !(a.is_empty()
            || ["various artists", "various", "va", "unknown"].contains(&a.to_lowercase().as_str()))
        {
            d.add_string("track_artist", a, &track.artist);
        }
    }
    // Recordings from a singleton search have no position on a release.
    let idx = item.int("track") as usize;
    if idx > 0 && track.index > 0 {
        d.add_bool(
            "track_index",
            idx != track.index && idx != track.medium_index,
        );
    }
    let id = item.str("mb_trackid");
    if !id.is_empty() && !track.track_id.is_empty() {
        d.add_bool("track_id", id != track.track_id);
    }
    d
}

/// Optimal assignment of items to tracks. Returns (item index, track index) pairs.
pub fn assign(cfg: &Config, items: &[Item], tracks: &[TrackInfo], va: bool) -> Vec<(usize, usize)> {
    if items.is_empty() || tracks.is_empty() {
        return vec![];
    }
    let n = items.len().max(tracks.len());
    const SCALE: f64 = 1_000_000.0;
    let mut m = Matrix::new(n, n, SCALE as i64);
    for (i, item) in items.iter().enumerate() {
        for (t, track) in tracks.iter().enumerate() {
            m[(i, t)] = (track_distance(cfg, item, track, va).distance() * SCALE) as i64;
        }
    }
    let (_, cols) = kuhn_munkres_min(&m);
    cols.into_iter()
        .enumerate()
        .filter(|(i, t)| *i < items.len() && *t < tracks.len())
        .collect()
}

// --------------------------------------------------------- album matching

#[derive(Debug, Clone)]
pub struct AlbumMatch {
    pub distance: Distance,
    pub info: AlbumInfo,
    pub mapping: Vec<(usize, usize)>,
    pub extra_items: Vec<usize>,
    pub extra_tracks: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Recommendation {
    None,
    Low,
    Medium,
    Strong,
}

pub fn album_distance(
    cfg: &Config,
    items: &[Item],
    info: &AlbumInfo,
    mapping: &[(usize, usize)],
) -> Distance {
    let cur = current_metadata(items);
    let mut d = Distance::new(cfg);
    // Artist names are meaningless to compare when both sides are compilations.
    if !(info.va && cur.va) {
        d.add_string("artist", &cur.artist, &info.artist);
    }
    d.add_string("album", &cur.album, &info.album);
    if cur.year > 0 && info.year > 0 {
        let years = [info.year, info.original_year];
        d.add_bool("year", !years.contains(&cur.year));
    }
    if cfg.bool("match.preferred.original_year") && info.original_year > 0 && info.year > 0 {
        // Prefer the original release over later reissues.
        let this_year = chrono::Datelike::year(&chrono::Local::now()) as i64;
        let span = (this_year - info.original_year).max(1) as f64;
        d.add("year", (info.year - info.original_year).abs() as f64 / span);
    }
    let countries = cfg.list("match.preferred.countries");
    let media = cfg.list("match.preferred.media");
    let likely_str = |f: &str| likely(items, f).0;
    let cur_media = likely_str("media");
    if !cur_media.is_empty() && !info.media.is_empty() {
        d.add_string("media", &cur_media, &info.media);
    } else if !media.is_empty() {
        let first = info.media.split('/').next().unwrap_or("");
        d.add_priority("media", first, &media);
    }
    if likely_str("country").is_empty() && !countries.is_empty() {
        d.add_priority("country", &info.country, &countries);
    }
    for (key, field, value) in [
        ("country", "country", &info.country),
        ("label", "label", &info.label),
        ("catalognum", "catalognum", &info.catalognum),
        ("albumdisambig", "albumdisambig", &info.albumdisambig),
    ] {
        let cur_v = likely_str(field);
        if !cur_v.is_empty() && !value.is_empty() {
            d.add_string(key, &cur_v, value);
        }
    }
    if !cur.mb_albumid.is_empty() {
        d.add_bool("album_id", cur.mb_albumid != info.album_id);
    }
    for &(i, t) in mapping {
        d.add(
            "tracks",
            track_distance(cfg, &items[i], &info.tracks[t], info.va).distance(),
        );
    }
    let matched_tracks: Vec<usize> = mapping.iter().map(|(_, t)| *t).collect();
    for t in 0..info.tracks.len() {
        if !matched_tracks.contains(&t) {
            d.add("missing_tracks", 1.0);
        }
    }
    for _ in mapping.len()..items.len() {
        d.add("unmatched_tracks", 1.0);
    }
    d
}

pub fn match_album(cfg: &Config, items: &[Item], info: AlbumInfo) -> AlbumMatch {
    let mapping = assign(cfg, items, &info.tracks, info.va);
    let distance = album_distance(cfg, items, &info, &mapping);
    let extra_items = (0..items.len())
        .filter(|i| !mapping.iter().any(|(mi, _)| mi == i))
        .collect();
    let extra_tracks = (0..info.tracks.len())
        .filter(|t| !mapping.iter().any(|(_, mt)| mt == t))
        .collect();
    AlbumMatch {
        distance,
        info,
        mapping,
        extra_items,
        extra_tracks,
    }
}

fn rec_from_str(s: &str) -> Recommendation {
    match s {
        "strong" => Recommendation::Strong,
        "medium" => Recommendation::Medium,
        "low" => Recommendation::Low,
        _ => Recommendation::None,
    }
}

pub fn recommend(cfg: &Config, dists: &[&Distance]) -> Recommendation {
    let Some(best) = dists.first() else {
        return Recommendation::None;
    };
    let d = best.distance();
    // As beets' `_recommendation`: strong is strictly below its threshold,
    // and several close candidates give no recommendation at all.
    let mut rec = if d < cfg.f64("match.strong_rec_thresh") {
        Recommendation::Strong
    } else if d <= cfg.f64("match.medium_rec_thresh") {
        Recommendation::Medium
    } else if dists.len() == 1 || dists[1].distance() - d >= cfg.f64("match.rec_gap_thresh") {
        Recommendation::Low
    } else {
        return Recommendation::None;
    };
    for (key, max) in cfg.mapping("match.max_rec") {
        if best.has(&key) {
            rec = rec.min(rec_from_str(&max));
        }
    }
    rec
}

/// Search MusicBrainz for candidates for a group of items.
pub fn album_candidates(
    cfg: &Config,
    mb: &MusicBrainz,
    items: &[Item],
    search: Option<(&str, &str)>,
    ids: &[String],
) -> Result<(Vec<AlbumMatch>, Recommendation)> {
    let cur = current_metadata(items);
    let mut release_ids: Vec<String> = ids.iter().filter_map(|i| extract_id(i)).collect();
    if release_ids.is_empty() {
        if !cur.mb_albumid.is_empty() && search.is_none() {
            release_ids.push(cur.mb_albumid.clone());
        }
        let (artist, album) = match search {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (
                if cur.va {
                    String::new()
                } else {
                    cur.artist.clone()
                },
                cur.album.clone(),
            ),
        };
        for id in mb.search_releases(&artist, &album, items.len())? {
            if !release_ids.contains(&id) {
                release_ids.push(id);
            }
        }
        // Retry without the track count, which often filters out the right release.
        if release_ids.is_empty() {
            release_ids = mb.search_releases(&artist, &album, 0)?;
        }
    }
    let mut matches = vec![];
    for id in release_ids {
        match mb.release(&id) {
            Ok(info) => matches.push(match_album(cfg, items, info)),
            Err(e) => crate::logging::emit(format!("warning: {e:#}")),
        }
    }
    matches.sort_by(|a, b| {
        a.distance
            .distance()
            .partial_cmp(&b.distance.distance())
            .unwrap()
    });
    let dists: Vec<&Distance> = matches.iter().map(|m| &m.distance).collect();
    let rec = recommend(cfg, &dists);
    Ok((matches, rec))
}

// -------------------------------------------------------- singleton matching

#[derive(Debug, Clone)]
pub struct TrackMatch {
    pub distance: Distance,
    pub info: TrackInfo,
}

pub fn item_candidates(
    cfg: &Config,
    mb: &MusicBrainz,
    item: &Item,
    search: Option<(&str, &str)>,
    ids: &[String],
) -> Result<(Vec<TrackMatch>, Recommendation)> {
    let mut infos = vec![];
    let explicit: Vec<String> = ids.iter().filter_map(|i| extract_id(i)).collect();
    if !explicit.is_empty() {
        for id in explicit {
            infos.push(mb.recording(&id)?);
        }
    } else {
        if !item.str("mb_trackid").is_empty()
            && search.is_none()
            && let Ok(t) = mb.recording(item.str("mb_trackid"))
        {
            infos.push(t);
        }
        let (artist, title) = search.unwrap_or((item.str("artist"), item.str("title")));
        for t in mb.search_recordings(artist, title)? {
            if !infos.iter().any(|i| i.track_id == t.track_id) {
                infos.push(t);
            }
        }
    }
    let mut matches: Vec<TrackMatch> = infos
        .into_iter()
        .map(|info| TrackMatch {
            distance: track_distance(cfg, item, &info, true),
            info,
        })
        .collect();
    matches.sort_by(|a, b| {
        a.distance
            .distance()
            .partial_cmp(&b.distance.distance())
            .unwrap()
    });
    let dists: Vec<&Distance> = matches.iter().map(|m| &m.distance).collect();
    Ok((matches.clone(), recommend(cfg, &dists)))
}

// ------------------------------------------------------------- applying

/// beets' `overwrite_null`: which fields a match may clear. A value the
/// match doesn't have (empty, or 0) leaves the item's value alone unless
/// its field is listed, by the match's name for it (`album_id`, `year`)
/// or the item's (`mb_albumid`).
struct Setter<'a> {
    item: &'a mut Item,
    nullable: &'a [String],
}

impl Setter<'_> {
    fn may_clear(&self, key: &str, field: &str) -> bool {
        self.nullable.iter().any(|n| n == key || n == field)
    }

    fn str(&mut self, key: &str, field: &str, v: &str) {
        if !v.is_empty() || self.may_clear(key, field) {
            self.item.set_str(field, v);
        }
    }

    fn int(&mut self, key: &str, field: &str, v: i64) {
        if v != 0 || self.may_clear(key, field) {
            self.item.set_int(field, v);
        }
    }
}

fn overwrite_null(cfg: &Config, kind: &str) -> Vec<String> {
    cfg.list(&format!("overwrite_null.{kind}"))
}

pub fn apply_track(cfg: &Config, item: &mut Item, t: &TrackInfo) {
    let nullable = overwrite_null(cfg, "track");
    let mut s = Setter {
        item,
        nullable: &nullable,
    };
    s.str("title", "title", &t.title);
    s.str("artist", "artist", &t.artist);
    s.str("artist_sort", "artist_sort", &t.artist_sort);
    s.str("artist_credit", "artist_credit", &t.artist_credit);
    s.str("track_id", "mb_trackid", &t.track_id);
    s.str("artist_id", "mb_artistid", &t.artist_id);
    s.str("release_track_id", "mb_releasetrackid", &t.release_track_id);
    s.str("isrc", "isrc", &t.isrc);
    item.set_str("data_source", "MusicBrainz");
    sync_list_fields(item, TRACK_LIST_FIELDS);
}

/// beets' `SYNCHRONISED_LIST_FIELDS` for a track: the single value and
/// the list it heads.
const TRACK_LIST_FIELDS: &[(&str, &str)] = &[
    ("artist", "artists"),
    ("mb_artistid", "mb_artistids"),
    ("artist_sort", "artists_sort"),
    ("artist_credit", "artists_credit"),
];

/// The same for an album.
const ALBUM_LIST_FIELDS: &[(&str, &str)] = &[
    ("albumartist", "albumartists"),
    ("mb_albumartistid", "mb_albumartistids"),
    ("albumartist_sort", "albumartists_sort"),
    ("albumartist_credit", "albumartists_credit"),
    ("albumtype", "albumtypes"),
];

/// beets' `correct_list_fields`: a single value heads its list (MediaFile
/// writes only the list, so the single value is otherwise lost). Our
/// MusicBrainz data has no lists of its own yet, so a set value becomes
/// the whole list.
fn sync_list_fields(item: &mut Item, pairs: &[(&str, &str)]) {
    for (single, list) in pairs {
        let v = item.str(single).to_string();
        if !v.is_empty() {
            item.set_list(list, &[v]);
        }
    }
}

pub fn apply_album(cfg: &Config, items: &mut [Item], m: &AlbumMatch) {
    let info = &m.info;
    let per_disc = cfg.bool("per_disc_numbering");
    let original_date = cfg.bool("original_date");
    let va_name = cfg.str("va_name");
    let album_null = overwrite_null(cfg, "album");
    let track_null = overwrite_null(cfg, "track");
    for &(i, t) in &m.mapping {
        let track = &info.tracks[t];
        let item = &mut items[i];
        apply_track(cfg, item, track);
        {
            // A track without its own artist gets the album's.
            let mut s = Setter {
                item,
                nullable: &track_null,
            };
            if track.artist.is_empty() {
                s.str("artist", "artist", &info.artist);
                s.str("artist_sort", "artist_sort", &info.artist_sort);
                s.str("artist_credit", "artist_credit", &info.artist_credit);
                s.str("artist_id", "mb_artistid", &info.artist_id);
            }
            s.str("disctitle", "disctitle", &track.disctitle);
            s.str("media", "media", &track.media);
        }
        let mut s = Setter {
            item,
            nullable: &album_null,
        };
        s.str("album", "album", &info.album);
        let albumartist = if info.va && !va_name.is_empty() {
            va_name.as_str()
        } else {
            info.artist.as_str()
        };
        s.str("artist", "albumartist", albumartist);
        s.str("artist_sort", "albumartist_sort", &info.artist_sort);
        s.str("artist_credit", "albumartist_credit", &info.artist_credit);
        s.str("album_id", "mb_albumid", &info.album_id);
        s.str("artist_id", "mb_albumartistid", &info.artist_id);
        s.str(
            "releasegroup_id",
            "mb_releasegroupid",
            &info.releasegroup_id,
        );
        let (y, mo, d) = if original_date && info.original_year > 0 {
            (info.original_year, info.original_month, info.original_day)
        } else {
            (info.year, info.month, info.day)
        };
        if y != 0 {
            // As beets: with a year, a missing month and day are 0.
            for (f, v) in [("year", y), ("month", mo), ("day", d)] {
                s.item.set_int(f, v);
            }
        } else {
            s.int("year", "year", y);
            s.int("month", "month", mo);
            s.int("day", "day", d);
        }
        s.int("original_year", "original_year", info.original_year);
        s.int("original_month", "original_month", info.original_month);
        s.int("original_day", "original_day", info.original_day);
        s.int("mediums", "disctotal", info.mediums as i64);
        for (f, v) in [
            ("albumtype", &info.albumtype),
            ("albumstatus", &info.albumstatus),
            ("label", &info.label),
            ("catalognum", &info.catalognum),
            ("country", &info.country),
            ("albumdisambig", &info.albumdisambig),
            ("releasegroupdisambig", &info.releasegroupdisambig),
        ] {
            s.str(f, f, v);
        }
        let item = s.item;
        if per_disc {
            item.set_int("track", track.medium_index as i64);
            item.set_int("tracktotal", track.medium_total as i64);
        } else {
            item.set_int("track", track.index as i64);
            item.set_int("tracktotal", info.tracks.len() as i64);
        }
        item.set_int("disc", track.medium as i64);
        item.set("comp", Value::Bool(info.va));
        sync_list_fields(item, TRACK_LIST_FIELDS);
        sync_list_fields(item, ALBUM_LIST_FIELDS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_distance() {
        assert_eq!(string_dist("The Beatles", "beatles, the"), 0.0);
        assert_eq!(string_dist("Björk", "Bjork"), 0.0);
        assert_eq!(string_dist("Simon & Garfunkel", "Simon and Garfunkel"), 0.0);
        assert!(string_dist("Help!", "Help") < 0.01);
        let remaster = string_dist("Something", "Something (Remastered 2009)");
        assert!(remaster > 0.0 && remaster <= 0.3, "{remaster}");
        assert!(string_dist("Yesterday", "Tomorrow") > 0.5);
        assert_eq!(string_dist("", ""), 0.0);
    }

    fn item(title: &str, track: i64, len: f64) -> Item {
        let mut i = Item::new_item();
        i.set_str("title", title);
        i.set_str("artist", "Band");
        i.set_str("album", "Record");
        i.set_int("track", track);
        i.set("length", Value::Float(len));
        i
    }

    fn info() -> AlbumInfo {
        let t = |title: &str, idx: usize, len: f64| TrackInfo {
            title: title.into(),
            index: idx,
            medium: 1,
            medium_index: idx,
            medium_total: 3,
            length: Some(len),
            track_id: format!("rec{idx}"),
            ..Default::default()
        };
        AlbumInfo {
            album_id: "rel".into(),
            album: "Record".into(),
            artist: "Band".into(),
            year: 2000,
            mediums: 1,
            tracks: vec![
                t("Alpha", 1, 200.0),
                t("Beta", 2, 180.0),
                t("Gamma", 3, 240.0),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn assigns_shuffled_items_and_recommends_strong() {
        let cfg = Config::defaults();
        let items = vec![
            item("Gamma", 3, 241.0),
            item("Alpha", 1, 199.0),
            item("Beta", 2, 181.0),
        ];
        let m = match_album(&cfg, &items, info());
        let mut mapping = m.mapping.clone();
        mapping.sort();
        assert_eq!(mapping, vec![(0, 2), (1, 0), (2, 1)]);
        assert!(m.distance.distance() < 0.01, "{}", m.distance.distance());
        assert_eq!(recommend(&cfg, &[&m.distance]), Recommendation::Strong);

        let mut items = items;
        apply_album(&cfg, &mut items, &m);
        assert_eq!(items[0].int("track"), 3);
        assert_eq!(items[0].str("mb_trackid"), "rec3");
        assert_eq!(items[0].int("year"), 2000);
        assert_eq!(items[0].str("albumartist"), "Band");
        // The list fields MediaFile writes follow the single values.
        assert_eq!(items[0].list("albumartists"), ["Band"]);
        assert_eq!(items[0].list("artists"), [items[0].str("artist")]);
    }

    #[test]
    fn overwrite_null() {
        let mut it = item("Alpha", 1, 200.0);
        it.set_str("label", "Old Label");
        it.set_int("original_year", 1990);
        it.set_str("isrc", "OLD");
        let mut items = vec![it];
        let m = AlbumMatch {
            distance: Default::default(),
            info: info(),
            mapping: vec![(0, 0)],
            extra_items: vec![],
            extra_tracks: vec![],
        };
        // Missing values leave existing ones alone by default...
        let cfg = Config::defaults();
        apply_album(&cfg, &mut items, &m);
        assert_eq!(items[0].str("label"), "Old Label");
        assert_eq!(items[0].int("original_year"), 1990);
        assert_eq!(items[0].str("isrc"), "OLD");
        assert_eq!(items[0].int("year"), 2000);
        // ... and clear them when listed (by the match's name or the item's).
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(
            &file,
            "overwrite_null:\n  album: [label, original_year]\n  track: [isrc]\n",
        )
        .unwrap();
        let cfg = Config::load_file(&file).unwrap();
        apply_album(&cfg, &mut items, &m);
        assert_eq!(items[0].str("label"), "");
        assert_eq!(items[0].int("original_year"), 0);
        assert_eq!(items[0].str("isrc"), "");
    }

    #[test]
    fn missing_tracks_cap_recommendation() {
        let cfg = Config::defaults();
        let items = vec![item("Alpha", 1, 200.0), item("Beta", 2, 180.0)];
        let m = match_album(&cfg, &items, info());
        assert_eq!(m.extra_tracks, vec![2]);
        assert!(
            m.distance
                .generic_penalty_keys()
                .contains(&"missing tracks".to_string())
        );
        assert!(recommend(&cfg, &[&m.distance]) <= Recommendation::Medium);
    }

    #[test]
    fn preferences_break_ties() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(
            &file,
            "match:\n  preferred:\n    countries: ['GB|UK', US]\n    media: [CD]\n    original_year: yes\n",
        )
        .unwrap();
        let cfg = Config::load_file(&file).unwrap();
        let items = vec![
            item("Alpha", 1, 200.0),
            item("Beta", 2, 180.0),
            item("Gamma", 3, 240.0),
        ];
        let mk = |country: &str, media: &str, year: i64| {
            let mut i = info();
            i.country = country.into();
            i.media = media.into();
            i.year = year;
            i.original_year = 2000;
            match_album(&cfg, &items, i).distance.distance()
        };
        let uk_cd = mk("UK", "CD", 2000);
        let us_cd = mk("US", "CD", 2000);
        let jp_cd = mk("JP", "CD", 2000);
        let uk_vinyl = mk("UK", "12\" Vinyl", 2000);
        let uk_reissue = mk("UK", "CD", 2016);
        assert!(uk_cd < us_cd && us_cd < jp_cd, "{uk_cd} {us_cd} {jp_cd}");
        assert!(uk_cd < uk_vinyl);
        assert!(uk_cd < uk_reissue);
        // Preferences nudge but don't sink an otherwise perfect match.
        let strong = cfg.f64("match.strong_rec_thresh");
        assert_eq!(uk_cd, 0.0);
        assert!(jp_cd <= strong, "{jp_cd}");
    }

    #[test]
    fn bad_match_is_low() {
        let cfg = Config::defaults();
        let mut items = vec![item("Foo", 1, 30.0), item("Bar", 2, 500.0)];
        for i in &mut items {
            i.set_str("artist", "Someone Else");
            i.set_str("album", "Other");
        }
        let m = match_album(&cfg, &items, info());
        assert!(m.distance.distance() > 0.4, "{}", m.distance.distance());
        assert_eq!(recommend(&cfg, &[&m.distance]), Recommendation::Low);
    }

    #[test]
    fn close_weak_candidates_give_no_recommendation() {
        let cfg = Config::defaults();
        let mut items = vec![item("Foo", 1, 30.0), item("Bar", 2, 500.0)];
        for i in &mut items {
            i.set_str("artist", "Someone Else");
            i.set_str("album", "Other");
        }
        let a = match_album(&cfg, &items, info());
        let b = match_album(&cfg, &items, info());
        // Two equally weak candidates: beets has no recommendation.
        assert_eq!(
            recommend(&cfg, &[&a.distance, &b.distance]),
            Recommendation::None
        );
    }
}
