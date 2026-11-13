//! Matching items against MusicBrainz candidates: a port of beets'
//! `autotag/distance.py` (weighted penalty distances, `string_dist`) and
//! `autotag/match.py` (item↔track assignment, candidate filtering by
//! `match.required`/`match.ignored`, the strong/medium/low recommendation).

use crate::config::Config;
use crate::hooks;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo};
use crate::model::Item;
use anyhow::Result;
use deunicode::deunicode;
use pathfinding::prelude::{Matrix, kuhn_munkres_min};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

// ---------------------------------------------------------------- distance

/// Artist names meaning "various artists" (beets' `VA_ARTISTS`).
const VA_ARTISTS: [&str; 5] = ["", "various artists", "various", "va", "unknown"];

/// The plugins that are beets `MetadataSourcePlugin`s.
const METADATA_SOURCES: &[&str] = &["musicbrainz", "chroma", "tidal"];

/// What the distance functions read from the config, gathered once per
/// lookup (beets caches the same values).
#[derive(Debug, Clone)]
pub struct Ctx {
    weights: Arc<HashMap<String, f64>>,
    length_grace: f64,
    length_max: f64,
    preferred_media: Vec<Regex>,
    preferred_countries: Vec<Regex>,
    original_year: bool,
    /// The MusicBrainz source's `data_source_mismatch_penalty`.
    source_penalty: f64,
    /// How many metadata source plugins are enabled (beets'
    /// `find_metadata_source_plugins()`).
    metadata_sources: usize,
    required: Vec<String>,
    ignored: Vec<String>,
    max_rec: Vec<(String, Recommendation)>,
    strong: f64,
    medium: f64,
    gap: f64,
    timid: bool,
    /// The current year (beets' `datetime.date.today().year`).
    this_year: i64,
}

/// A preference pattern as beets compiles it: case-insensitive, matched at
/// the start of the value (Python's `re.match`).
fn pref_regex(pat: &str) -> Option<Regex> {
    Regex::new(&format!("(?i)^(?:{pat})")).ok()
}

impl Ctx {
    pub fn new(cfg: &Config) -> Self {
        let weights = cfg
            .mapping("match.distance_weights")
            .into_iter()
            .map(|(k, v)| (k, v.parse().unwrap_or(0.0)))
            .collect();
        // The MusicBrainz plugin's penalty: `source_weight` (deprecated)
        // over `data_source_mismatch_penalty`.
        let source_penalty = if cfg.get("musicbrainz.source_weight").is_some() {
            cfg.f64("musicbrainz.source_weight")
        } else {
            cfg.f64("musicbrainz.data_source_mismatch_penalty")
        };
        Ctx {
            weights: Arc::new(weights),
            length_grace: cfg.f64("match.track_length_grace"),
            length_max: cfg.f64("match.track_length_max"),
            preferred_media: cfg
                .list("match.preferred.media")
                .iter()
                .filter_map(|p| pref_regex(&format!(r"(\d+x)?({p})")))
                .collect(),
            preferred_countries: cfg
                .list("match.preferred.countries")
                .iter()
                .filter_map(|p| pref_regex(p))
                .collect(),
            original_year: cfg.bool("match.preferred.original_year"),
            source_penalty,
            metadata_sources: METADATA_SOURCES
                .iter()
                .filter(|p| crate::plugins::enabled(cfg, p))
                .count(),
            required: cfg.list("match.required"),
            ignored: cfg.list("match.ignored"),
            max_rec: cfg
                .mapping("match.max_rec")
                .into_iter()
                .map(|(k, v)| (k, rec_from_str(&v)))
                .collect(),
            strong: cfg.f64("match.strong_rec_thresh"),
            medium: cfg.f64("match.medium_rec_thresh"),
            gap: cfg.f64("match.rec_gap_thresh"),
            timid: cfg.bool("import.timid"),
            this_year: chrono::Datelike::year(&chrono::Local::now()) as i64,
        }
    }

    fn dist(&self) -> Distance {
        Distance {
            penalties: vec![],
            weights: self.weights.clone(),
            tracks: vec![],
        }
    }
}

/// beets' `Distance`: penalties by key, in the order they were first added,
/// each weighted by `match.distance_weights`.
#[derive(Debug, Clone, Default)]
pub struct Distance {
    penalties: Vec<(String, Vec<f64>)>,
    weights: Arc<HashMap<String, f64>>,
    /// An album match's per-track distances (beets' `Distance.tracks`).
    pub tracks: Vec<Distance>,
}

impl Distance {
    #[cfg(test)]
    pub fn new(cfg: &Config) -> Self {
        Ctx::new(cfg).dist()
    }

    fn weight(&self, key: &str) -> f64 {
        *self.weights.get(key).unwrap_or(&0.0)
    }

    pub fn add(&mut self, key: &str, dist: f64) {
        debug_assert!((0.0..=1.0).contains(&dist), "{key}: {dist}");
        match self.penalties.iter_mut().find(|(k, _)| k == key) {
            Some((_, p)) => p.push(dist),
            None => self.penalties.push((key.to_string(), vec![dist])),
        }
    }

    pub fn add_string(&mut self, key: &str, a: Option<&str>, b: Option<&str>) {
        self.add(key, string_dist(a, b));
    }

    /// 0.0 if `value` equals any of `options`, else 1.0.
    pub fn add_equality(&mut self, key: &str, value: &str, options: &[&str]) {
        self.add_expr(key, !options.contains(&value));
    }

    pub fn add_expr(&mut self, key: &str, expr: bool) {
        self.add(key, if expr { 1.0 } else { 0.0 });
    }

    /// 1.0 for each unit of difference, or one 0.0 when there's none.
    pub fn add_number(&mut self, key: &str, a: i64, b: i64) {
        let diff = (a - b).unsigned_abs();
        if diff == 0 {
            self.add(key, 0.0);
        }
        for _ in 0..diff {
            self.add(key, 1.0);
        }
    }

    /// By position in a preference list: the first option is free, later
    /// ones cost more, and matching none costs 1.0.
    pub fn add_priority(&mut self, key: &str, value: &str, options: &[Regex]) {
        let unit = 1.0 / options.len().max(1) as f64;
        let dist = options
            .iter()
            .position(|re| re.is_match(value))
            .map_or(1.0, |i| i as f64 * unit);
        self.add(key, dist);
    }

    /// `a` as a ratio of `b`, `a` bound to [0, b].
    pub fn add_ratio(&mut self, key: &str, a: f64, b: f64) {
        let n = a.min(b).max(0.0);
        self.add(key, if b != 0.0 { n / b } else { 0.0 });
    }

    /// beets' `add_data_source`: a mismatch costs when the old data came
    /// from somewhere, or whenever more than one metadata source is
    /// enabled (chroma alongside musicbrainz, say).
    fn add_data_source(&mut self, ctx: &Ctx, before: &str, after: &str) {
        if before != after && (!before.is_empty() || ctx.metadata_sources > 1) {
            let penalty = if after == "MusicBrainz" {
                ctx.source_penalty
            } else {
                0.5
            };
            self.add("data_source", penalty);
        }
    }

    fn raw(&self) -> f64 {
        self.penalties
            .iter()
            .map(|(k, p)| p.iter().sum::<f64>() * self.weight(k))
            .sum()
    }

    fn max(&self) -> f64 {
        self.penalties
            .iter()
            .map(|(k, p)| p.len() as f64 * self.weight(k))
            .sum()
    }

    /// Normalized distance in [0, 1].
    pub fn distance(&self) -> f64 {
        let max = self.max();
        if max != 0.0 { self.raw() / max } else { 0.0 }
    }

    /// The weighted share of one penalty (beets' `dist[key]`).
    pub fn get(&self, key: &str) -> f64 {
        let Some((_, p)) = self.penalties.iter().find(|(k, _)| k == key) else {
            return 0.0;
        };
        let max = self.max();
        if max != 0.0 {
            p.iter().sum::<f64>() * self.weight(key) / max
        } else {
            0.0
        }
    }

    /// beets' `items()`: (key, weighted distance) for the penalties that
    /// count, largest first, then by key.
    pub fn items(&self) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = self
            .penalties
            .iter()
            .map(|(k, _)| (k.clone(), self.get(k)))
            .filter(|(_, d)| *d != 0.0)
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// beets' `generic_penalty_keys`: the keys that count, in the order
    /// they were added, without their `album_`/`track_` prefix and with
    /// underscores as spaces.
    pub fn generic_penalty_keys(&self) -> Vec<String> {
        self.penalties
            .iter()
            .filter(|(k, _)| self.get(k) != 0.0)
            .map(|(k, _)| {
                k.replace("album_", "")
                    .replace("track_", "")
                    .replace('_', " ")
            })
            .collect()
    }

    /// Whether the penalty `key` counts (beets' `key in dist.keys()`).
    pub fn has(&self, key: &str) -> bool {
        self.get(key) != 0.0
    }
}

// ------------------------------------------------------- string distance

/// beets' `SD_PATTERNS`: parts of a string that count for less.
static SD_PATTERNS: LazyLock<Vec<(Regex, f64)>> = LazyLock::new(|| {
    [
        (r"^the ", 0.1),
        (r"[\[\(]?(ep|single)[\]\)]?", 0.0),
        (r"[\[\(]?\b(featuring|feat|ft)\b[\. :].+", 0.1),
        (r"\(.*?\)", 0.3),
        (r"\[.*?\]", 0.3),
        (r"(, )?(pt\.|part) .+", 0.2),
    ]
    .iter()
    .map(|(p, w)| (Regex::new(p).unwrap(), *w))
    .collect()
});
static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]").unwrap());

/// beets' `_string_dist_basic`: edit distance over the ASCII letters and
/// digits, normalized by the longer string.
fn string_dist_basic(a: &str, b: &str) -> f64 {
    let a = deunicode(a).to_lowercase();
    let b = deunicode(b).to_lowercase();
    let a = NON_ALNUM.replace_all(&a, "");
    let b = NON_ALNUM.replace_all(&b, "");
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let max = a.chars().count().max(b.chars().count());
    strsim::levenshtein(&a, &b) as f64 / max as f64
}

/// beets' `string_dist`: an edit distance with tweaks for "Beatles, The",
/// "&", and parts such as "(Remastered)" or "feat. X" that count for less.
/// None is a missing value (a distance of 1 from anything but None).
pub fn string_dist(a: Option<&str>, b: Option<&str>) -> f64 {
    let (a, b) = match (a, b) {
        (None, None) => return 0.0,
        (Some(a), Some(b)) => (a, b),
        _ => return 1.0,
    };
    let mut a = a.to_lowercase();
    let mut b = b.to_lowercase();
    for word in ["the", "a", "an"] {
        for s in [&mut a, &mut b] {
            let suffix = format!(", {word}");
            if let Some(rest) = s.strip_suffix(&suffix) {
                *s = format!("{word} {rest}");
            }
        }
    }
    a = a.replace('&', "and");
    b = b.replace('&', "and");
    let mut base = string_dist_basic(&a, &b);
    let mut penalty = 0.0;
    for (re, weight) in SD_PATTERNS.iter() {
        let ca = re.replace_all(&a, "").into_owned();
        let cb = re.replace_all(&b, "").into_owned();
        if ca != a || cb != b {
            let case = string_dist_basic(&ca, &cb);
            let delta = (base - case).max(0.0);
            if delta == 0.0 {
                continue;
            }
            a = ca;
            b = cb;
            base = case;
            penalty += weight * delta;
        }
    }
    base + penalty
}

// ------------------------------------------------------------ current state

/// beets' `plurality`: the most common value, ties going to the first seen.
fn plurality(values: impl IntoIterator<Item = String>) -> String {
    let mut counts: Vec<(String, usize)> = vec![];
    for v in values {
        match counts.iter_mut().find(|(k, _)| *k == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    let mut best: Option<(String, usize)> = None;
    for (v, n) in counts {
        if best.as_ref().is_none_or(|(_, b)| n > *b) {
            best = Some((v, n));
        }
    }
    best.map(|(v, _)| v).unwrap_or_default()
}

/// beets' `Likelies` (`get_most_common_tags`): each field's most common
/// value across the items, plus what the album lookup needs.
#[derive(Debug, Clone, Default)]
pub struct Current {
    pub artist: String,
    pub album: String,
    pub year: i64,
    pub disctotal: i64,
    pub mb_albumid: String,
    pub label: String,
    pub catalognum: String,
    pub country: String,
    pub media: String,
    pub albumdisambig: String,
    pub data_source: String,
    /// Every item has the same `mb_albumid` (beets' `id_consensus`).
    pub id_consensus: bool,
    /// beets' `va_likely`: the items' artists differ, or one is a compilation.
    pub va: bool,
}

pub fn current_metadata(items: &[Item]) -> Current {
    let raw = |f: &str| {
        plurality(
            items
                .iter()
                .map(|i| i.get(f).map(|v| v.raw()).unwrap_or_default()),
        )
    };
    let distinct = |f: &str| {
        let mut seen: Vec<&str> = items.iter().map(|i| i.str(f)).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    };
    let num = |f: &str| raw(f).parse::<f64>().map_or(0, |v| v as i64);
    let albumartist = raw("albumartist");
    let artist = if distinct("albumartist") == 1 && !albumartist.is_empty() {
        albumartist.clone()
    } else {
        raw("artist")
    };
    Current {
        artist,
        album: raw("album"),
        year: num("year"),
        disctotal: num("disctotal"),
        mb_albumid: raw("mb_albumid"),
        label: raw("label"),
        catalognum: raw("catalognum"),
        country: raw("country"),
        media: raw("media"),
        albumdisambig: raw("albumdisambig"),
        data_source: raw("data_source"),
        id_consensus: distinct("mb_albumid") == 1,
        va: distinct("artist") > 1 || items.iter().any(|i| i.bool("comp")),
    }
}

// --------------------------------------------------------- track distances

/// beets' `track_distance`. `incl_artist`: compare the track artist too
/// (for various-artist releases and singletons).
pub fn track_distance(ctx: &Ctx, item: &Item, track: &TrackInfo, incl_artist: bool) -> Distance {
    let mut d = ctx.dist();
    if let Some(len) = track.length.filter(|l| *l != 0.0) {
        let diff = (item.float("length") - len).abs() - ctx.length_grace;
        d.add_ratio("track_length", diff, ctx.length_max);
    }
    d.add_string("track_title", Some(item.str("title")), Some(&track.title));
    if incl_artist
        && !track.artist.is_empty()
        && !VA_ARTISTS.contains(&item.str("artist").to_lowercase().as_str())
    {
        d.add_string(
            "track_artist",
            Some(item.str("artist")),
            Some(&track.artist),
        );
    }
    let n = item.int("track");
    if track.index != 0 && n != 0 {
        // Per-disc or per-release numbering both count.
        d.add_expr(
            "track_index",
            n != track.medium_index as i64 && n != track.index as i64,
        );
    }
    let id = item.str("mb_trackid");
    if !id.is_empty() {
        d.add_expr("track_id", id != track.track_id);
    }
    if track.medium != 0 && item.int("disc") != 0 {
        d.add_expr("medium", item.int("disc") != track.medium as i64);
    }
    let before = item.get("data_source").map(|v| v.raw()).unwrap_or_default();
    d.add_data_source(ctx, &before, &track.data_source);
    d
}

/// Optimal assignment of items to tracks (beets' `assign_items`), by the
/// tracks' distances without the artist. Returns (item, track) index
/// pairs in track order.
pub fn assign(ctx: &Ctx, items: &[Item], tracks: &[TrackInfo]) -> Vec<(usize, usize)> {
    if items.is_empty() || tracks.is_empty() {
        return vec![];
    }
    let n = items.len().max(tracks.len());
    const SCALE: f64 = 1e12;
    let mut m = Matrix::new(n, n, SCALE as i64);
    for (i, item) in items.iter().enumerate() {
        for (t, track) in tracks.iter().enumerate() {
            m[(i, t)] = (track_distance(ctx, item, track, false).distance() * SCALE) as i64;
        }
    }
    let (_, cols) = kuhn_munkres_min(&m);
    let mut pairs: Vec<(usize, usize)> = cols
        .into_iter()
        .enumerate()
        .filter(|(i, t)| *i < items.len() && *t < tracks.len())
        .collect();
    pairs.sort_by_key(|&(_, t)| t);
    pairs
}

// --------------------------------------------------------- album matching

#[derive(Debug, Clone)]
pub struct AlbumMatch {
    pub distance: Distance,
    pub info: AlbumInfo,
    /// (item, track) index pairs, in track order.
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

/// beets' `distance`: how far the album `info`, with the items matched to
/// its tracks as `mapping`, is from what the items say (`cur`).
pub fn album_distance(
    ctx: &Ctx,
    cur: &Current,
    items: &[Item],
    info: &AlbumInfo,
    mapping: &[(usize, usize)],
    unmatched: usize,
) -> Distance {
    let mut d = ctx.dist();
    if !info.va {
        d.add_string("artist", Some(&cur.artist), Some(&info.artist));
    }
    d.add_string("album", Some(&cur.album), Some(&info.album));
    if !info.media.is_empty() {
        if !ctx.preferred_media.is_empty() {
            d.add_priority("media", &info.media, &ctx.preferred_media);
        } else if !cur.media.is_empty() {
            d.add_equality("media", &info.media, &[&cur.media]);
        }
    }
    if cur.disctotal != 0 && info.mediums != 0 {
        d.add_number("mediums", cur.disctotal, info.mediums as i64);
    }
    let this_year = ctx.this_year;
    if info.year != 0 && ctx.original_year {
        // 1889 (the first gramophone discs) when the original is unknown.
        let original = if info.original_year != 0 {
            info.original_year
        } else {
            1889
        };
        let diff = (info.year - original).abs() as f64;
        d.add_ratio("year", diff, (this_year - original).abs() as f64);
    } else if cur.year != 0 && info.year != 0 {
        if cur.year == info.year || cur.year == info.original_year {
            d.add("year", 0.0);
        } else if info.original_year != 0 {
            let diff = (cur.year - info.year).abs() as f64;
            d.add_ratio("year", diff, (this_year - info.original_year).abs() as f64);
        } else {
            d.add("year", 1.0);
        }
    }
    if !info.country.is_empty() && !ctx.preferred_countries.is_empty() {
        d.add_priority("country", &info.country, &ctx.preferred_countries);
    } else if !cur.country.is_empty() && !info.country.is_empty() {
        d.add_string("country", Some(&cur.country), Some(&info.country));
    }
    for (key, mine, theirs) in [
        ("label", &cur.label, &info.label),
        ("catalognum", &cur.catalognum, &info.catalognum),
        ("albumdisambig", &cur.albumdisambig, &info.albumdisambig),
    ] {
        if !mine.is_empty() && !theirs.is_empty() {
            d.add_string(key, Some(mine), Some(theirs));
        }
    }
    if !cur.mb_albumid.is_empty() {
        d.add_equality("album_id", &cur.mb_albumid, &[&info.album_id]);
    }
    for &(i, t) in mapping {
        let td = track_distance(ctx, &items[i], &info.tracks[t], info.va);
        d.add("tracks", td.distance());
        d.tracks.push(td);
    }
    for _ in mapping.len()..info.tracks.len() {
        d.add("missing_tracks", 1.0);
    }
    for _ in 0..unmatched {
        d.add("unmatched_tracks", 1.0);
    }
    d.add_data_source(ctx, &cur.data_source, &info.data_source);
    d
}

pub fn match_album(ctx: &Ctx, cur: &Current, items: &[Item], info: AlbumInfo) -> AlbumMatch {
    let mapping = assign(ctx, items, &info.tracks);
    let extra_items: Vec<usize> = (0..items.len())
        .filter(|i| !mapping.iter().any(|(mi, _)| mi == i))
        .collect();
    let extra_tracks = (0..info.tracks.len())
        .filter(|t| !mapping.iter().any(|(_, mt)| mt == t))
        .collect();
    let distance = album_distance(ctx, cur, items, &info, &mapping, extra_items.len());
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

/// beets' `_recommendation` for candidates sorted by distance; `max_rec`
/// caps it for each penalty the best one has, its tracks' included.
pub fn recommend(ctx: &Ctx, dists: &[&Distance]) -> Recommendation {
    let Some(best) = dists.first() else {
        return Recommendation::None;
    };
    let d = best.distance();
    let mut rec = if d < ctx.strong {
        Recommendation::Strong
    } else if d <= ctx.medium {
        Recommendation::Medium
    } else if dists.len() == 1 || dists[1].distance() - d >= ctx.gap {
        Recommendation::Low
    } else {
        return Recommendation::None;
    };
    for (key, max) in &ctx.max_rec {
        if best.has(key) || best.tracks.iter().any(|t| t.has(key)) {
            rec = rec.min(*max);
        }
    }
    rec
}

/// beets' `_add_candidate`: match a release to the items, unless it has no
/// tracks, was seen already, lacks a `match.required` field or has a
/// `match.ignored` penalty.
fn add_candidate(
    ctx: &Ctx,
    cur: &Current,
    items: &[Item],
    out: &mut Vec<AlbumMatch>,
    info: AlbumInfo,
) {
    crate::logging::debug!(
        "Candidate: {} - {} ({}) from {}",
        info.artist,
        info.album,
        info.album_id,
        info.data_source
    );
    if info.tracks.is_empty() {
        crate::logging::debug!("No tracks.");
        return;
    }
    if !info.album_id.is_empty() && out.iter().any(|m| m.info.album_id == info.album_id) {
        crate::logging::debug!("Duplicate.");
        return;
    }
    for tag in &ctx.required {
        if !matches!(info.field(tag), Some(Some(_))) {
            crate::logging::debug!("Ignored. Missing required tag: {tag}");
            return;
        }
    }
    let m = match_album(ctx, cur, items, info);
    let penalties = m.distance.items();
    for tag in &ctx.ignored {
        if penalties.iter().any(|(k, _)| k == tag) {
            crate::logging::debug!("Ignored. Penalty: {tag}");
            return;
        }
    }
    crate::logging::debug!("Success. Distance: {:.2}", m.distance.distance());
    out.push(m);
}

fn sort_by_distance<T>(v: &mut [T], d: impl Fn(&T) -> f64) {
    v.sort_by(|a, b| d(a).total_cmp(&d(b)));
}

/// beets' `maybe_handle_plugin_error`: a metadata source's failure is logged
/// and counts as no result, unless `raise_on_error`.
fn metadata_error(cfg: &Config, source: &str, method: &str, e: anyhow::Error) -> Result<()> {
    if cfg.bool("raise_on_error") {
        return Err(e);
    }
    crate::logging::emit(format!("Error in '{source}.{method}': {e:#}"));
    Ok(())
}

fn metadata_single<T>(
    cfg: &Config,
    source: &str,
    method: &str,
    r: Result<Option<T>>,
) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => {
            metadata_error(cfg, source, method, e)?;
            Ok(None)
        }
    }
}

fn metadata_list<T>(cfg: &Config, source: &str, method: &str, r: Result<Vec<T>>) -> Result<Vec<T>> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => {
            metadata_error(cfg, source, method, e)?;
            Ok(vec![])
        }
    }
}

/// beets' `tag_album`: the release candidates for a group of items and a
/// recommendation. With `ids`, only those releases; otherwise the items'
/// own release (when they agree on one), returned alone if it's a strong
/// match, then a search (by `search`, if both parts are given, else by the
/// items' artist and album).
pub fn album_candidates(
    cfg: &Config,
    mb: &MusicBrainz,
    items: &[Item],
    search: Option<(&str, &str)>,
    ids: &[String],
) -> Result<(Vec<AlbumMatch>, Recommendation)> {
    // As beets with no metadata source plugin: nothing to search.
    let mb_on = crate::plugins::enabled(cfg, "musicbrainz");
    let tidal_on = crate::plugins::enabled(cfg, "tidal");
    if !mb_on && !tidal_on {
        return Ok((vec![], Recommendation::None));
    }
    let ctx = Ctx::new(cfg);
    let cur = current_metadata(items);
    let rec_of = |c: &[AlbumMatch]| {
        let dists: Vec<&Distance> = c.iter().map(|m| &m.distance).collect();
        recommend(&ctx, &dists)
    };
    let mut cands = vec![];
    // beets' `albums_for_ids`: each lookup's error is logged, not fatal.
    let add_ids = |cands: &mut Vec<AlbumMatch>, ids: &[String]| -> Result<()> {
        for id in ids {
            if mb_on {
                match mb.album_for_id_variants(id) {
                    Ok(infos) => {
                        for info in infos {
                            add_candidate(&ctx, &cur, items, cands, info);
                        }
                    }
                    Err(e) => crate::logging::emit(format!("warning: {e:#}")),
                }
            }
            if tidal_on {
                // The tidal plugin's `album_for_id` (beets' `albums_for_ids`
                // asks every metadata source; each extracts the IDs it
                // knows, here `tidal.com/album/<n>` URLs and numeric IDs).
                if let Some(info) = metadata_single(
                    cfg,
                    "Tidal",
                    "album_for_id",
                    crate::plugins::tidal::album_for_id(cfg, id),
                )? {
                    add_candidate(&ctx, &cur, items, cands, info);
                }
            }
        }
        Ok(())
    };
    if !ids.is_empty() {
        add_ids(&mut cands, ids)?;
    } else {
        if !cur.mb_albumid.is_empty() && cur.id_consensus {
            add_ids(&mut cands, std::slice::from_ref(&cur.mb_albumid))?;
        }
        if !cands.is_empty() && !ctx.timid && rec_of(&cands) == Recommendation::Strong {
            crate::logging::debug!("ID match.");
            let rec = rec_of(&cands);
            return Ok((cands, rec));
        }
        let (artist, album) = match search {
            Some((a, b)) if !a.is_empty() && !b.is_empty() => (a.to_string(), b.to_string()),
            _ => (cur.artist.clone(), cur.album.clone()),
        };
        let va_likely = cur.va || VA_ARTISTS.contains(&artist.to_lowercase().as_str());
        if mb_on {
            let release_ids = mb.search_releases(items, &artist, &album, va_likely)?;
            add_ids(&mut cands, &release_ids)?;
            // The other metadata source's candidates (beets queries every
            // source's `candidates`).
            for info in crate::plugins::chroma::album_candidates(cfg, mb, items) {
                add_candidate(&ctx, &cur, items, &mut cands, info);
            }
        }
        if tidal_on {
            for info in metadata_list(
                cfg,
                "Tidal",
                "candidates",
                crate::plugins::tidal::album_candidates(cfg, items),
            )? {
                add_candidate(&ctx, &cur, items, &mut cands, info);
            }
        }
    }
    sort_by_distance(&mut cands, |m| m.distance.distance());
    let rec = rec_of(&cands);
    Ok((cands, rec))
}

// -------------------------------------------------------- singleton matching

#[derive(Debug, Clone)]
pub struct TrackMatch {
    pub distance: Distance,
    pub info: TrackInfo,
}

/// beets' `tag_item`: the recording candidates for one item. The item's
/// recording (or the `ids` given) first, returned alone if it's a strong
/// match (or if `ids` were given); then a search by `search`, each part
/// falling back to the item's artist and title.
pub fn item_candidates(
    cfg: &Config,
    mb: &MusicBrainz,
    item: &Item,
    search: Option<(&str, &str)>,
    ids: &[String],
) -> Result<(Vec<TrackMatch>, Recommendation)> {
    let mb_on = crate::plugins::enabled(cfg, "musicbrainz");
    let tidal_on = crate::plugins::enabled(cfg, "tidal");
    if !mb_on && !tidal_on {
        return Ok((vec![], Recommendation::None));
    }
    let ctx = Ctx::new(cfg);
    let mut cands: Vec<TrackMatch> = vec![];
    let push = |cands: &mut Vec<TrackMatch>, info: TrackInfo| {
        let m = TrackMatch {
            distance: track_distance(&ctx, item, &info, true),
            info,
        };
        // Keyed by recording, as beets' candidates dict: a repeat replaces.
        match cands
            .iter_mut()
            .find(|c| c.info.track_id == m.info.track_id)
        {
            Some(c) => *c = m,
            None => cands.push(m),
        }
    };
    let rec_of = |c: &mut Vec<TrackMatch>| {
        sort_by_distance(c, |m| m.distance.distance());
        let dists: Vec<&Distance> = c.iter().map(|m| &m.distance).collect();
        recommend(&ctx, &dists)
    };
    let trackids: Vec<String> = if ids.is_empty() {
        [item.str("mb_trackid").to_string()]
            .into_iter()
            .filter(|i| !i.is_empty())
            .collect()
    } else {
        ids.to_vec()
    };
    if !trackids.is_empty() {
        for id in &trackids {
            if mb_on {
                match mb.track_for_id(id) {
                    Ok(Some(t)) => push(&mut cands, t),
                    Ok(None) => {}
                    Err(e) => crate::logging::emit(format!("warning: {e:#}")),
                }
            }
            if tidal_on
                && let Some(t) = metadata_single(
                    cfg,
                    "Tidal",
                    "track_for_id",
                    crate::plugins::tidal::track_for_id(cfg, id),
                )?
            {
                push(&mut cands, t);
            }
        }
        let rec = rec_of(&mut cands);
        if rec == Recommendation::Strong && !ctx.timid {
            crate::logging::debug!("Track ID match.");
            return Ok((cands, rec));
        }
        if !ids.is_empty() {
            return Ok(if cands.is_empty() {
                (cands, Recommendation::None)
            } else {
                (cands, rec)
            });
        }
    }
    let (sa, st) = search.unwrap_or_default();
    let artist = if sa.is_empty() {
        item.str("artist")
    } else {
        sa
    };
    let title = if st.is_empty() { item.str("title") } else { st };
    if mb_on {
        for t in mb.search_recordings(artist, title)? {
            push(&mut cands, t);
        }
        for t in crate::plugins::chroma::item_candidates(cfg, mb, item) {
            push(&mut cands, t);
        }
    }
    if tidal_on {
        for t in metadata_list(
            cfg,
            "Tidal",
            "item_candidates",
            crate::plugins::tidal::item_candidates(cfg, item),
        )? {
            push(&mut cands, t);
        }
    }
    let rec = rec_of(&mut cands);
    Ok((cands, rec))
}

// ------------------------------------------------------------- applying

/// beets' `TrackMatch.apply_metadata` (without `from_scratch`, which the
/// importer handles): `item.update(info.item_data)`.
pub fn apply_track(cfg: &Config, item: &mut Item, t: &TrackInfo) {
    let o = hooks::ApplyOpts::from_config(cfg);
    hooks::update(item, &hooks::track_item_data(&t.to_data(), &o));
}

/// beets' `AlbumMatch.apply_metadata`: each matched item updated with its
/// track merged with the album (`merge_with_album`).
pub fn apply_album(cfg: &Config, items: &mut [Item], m: &AlbumMatch) {
    let o = hooks::ApplyOpts::from_config(cfg);
    for &(i, t) in &m.mapping {
        let data = hooks::merge_with_album(&m.info.tracks[t], &m.info, &o);
        hooks::update(&mut items[i], &data);
    }
}

/// beets' `AlbumMatch.apply_album_metadata`: the new album updated with
/// the album's item data (fields that aren't album fields, such as
/// `media`, become flexible attributes), and what changed inherited by
/// its items, as beets' `Album.store` does: album fields the items share,
/// and flexible attributes (so the items get the album's `media`).
pub fn apply_album_metadata(
    cfg: &Config,
    album: &mut crate::model::Album,
    items: &mut [Item],
    info: &AlbumInfo,
) {
    use crate::model::{Kind, field_type};
    let o = hooks::ApplyOpts::from_config(cfg);
    for key in hooks::update(album, &hooks::album_item_data(&info.to_data(), &o)) {
        let fixed = field_type(Kind::Album, &key).is_some();
        if fixed && (key == "artpath" || field_type(Kind::Item, &key).is_none()) {
            continue;
        }
        if let Some(v) = album.get(&key) {
            for item in items.iter_mut() {
                item.set(&key, v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Value;

    /// The defaults with `settings` (YAML) over them.
    fn config(settings: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(&file, settings).unwrap();
        Config::load_file(&file).unwrap()
    }

    fn sd(a: &str, b: &str) -> f64 {
        string_dist(Some(a), Some(b))
    }

    // ------------------------------------------- upstream test_distance.py

    #[test]
    fn matching_string_distances() {
        for (a, b) in [
            ("Some String", "Some String"),
            ("Some String", "Some.String!"),
            ("Some String", "sOME sTring"),
            ("My Song (EP)", "My Song"),
            ("The Song Title", "Song Title, The"),
            ("A Song Title", "Song Title, A"),
            ("An Album Title", "Album Title, An"),
            ("", ""),
            ("Untitled", "[Untitled]"),
            ("And", "&"),
            ("\u{e9}\u{e1}\u{f1}", "ean"),
        ] {
            assert_eq!(sd(a, b), 0.0, "{a:?} {b:?}");
        }
        assert_ne!(sd("Some String", "Totally Different"), 0.0);
    }

    #[test]
    fn featuring_pattern_does_not_match_mid_word() {
        for (a, b) in [
            ("Draft Beer", "Draft Whiskey"),
            ("Left Field", "Left Symphony"),
            ("Gift Ideas", "Gift Cards"),
            ("Craft Beer", "Craft Wine"),
        ] {
            assert!(sd(a, b) > 0.3, "{a} {b}");
        }
    }

    #[test]
    fn string_distance_relative_weights() {
        for (s1, s2, reference) in [
            ("XXX Band Name", "The Band Name", "Band Name"),
            ("One .Two.", "One (Two)", "One"),
            ("One .Two.", "One [Two]", "One"),
            ("My Song blah Someone", "My Song feat Someone", "My Song"),
        ] {
            assert!(sd(s2, reference) < sd(s1, reference), "{s1} {s2}");
        }
        // Just make sure these don't crash.
        sd("The ", "");
        sd("(EP)", "(EP)");
        sd(", An", "");
    }

    /// Values from reference beets 2.14's `string_dist`.
    #[test]
    fn string_distances_as_beets() {
        for (a, b, want) in [
            ("Some String", "Totally Different", 0.8125),
            ("Draft Beer", "Draft Whiskey", 0.5),
            ("XXX Band Name", "Band Name", 0.2727272727272727),
            ("The Band Name", "Band Name", 0.02727272727272727),
            ("One .Two.", "One", 0.5),
            ("One (Two)", "One", 0.15),
            ("My Song feat Someone", "My Song", 0.06470588235294118),
            ("My Song blah Someone", "My Song", 0.6470588235294118),
            (
                "Something",
                "Something (Remastered 2009)",
                0.1826086956521739,
            ),
            ("Yesterday", "Tomorrow", 0.8888888888888888),
            ("Help!", "Help", 0.0),
            ("Alpha", "Alpha (video)", 0.15),
            ("Gamma", "Gamma (Remastered)", 0.19999999999999998),
            (
                "Fixture Album",
                "Fixture Album (Deluxe)",
                0.09999999999999999,
            ),
            ("Song, Pt. 2", "Song", 0.08571428571428572),
            ("Bj\u{f6}rk & Friends", "Bjork and friends", 0.0),
            ("Single Ladies", "Ladies", 0.0),
            ("abc", "bcd", 0.6666666666666666),
            ("The Beatles", "beatles, the", 0.0),
            ("Live [Remastered] (Deluxe)", "Live", 0.24),
        ] {
            assert_eq!(sd(a, b), want, "{a:?} {b:?}");
        }
        assert_eq!(string_dist(Some("abc"), None), 1.0);
        assert_eq!(string_dist(None, None), 0.0);
    }

    /// The weights of upstream's `TestDistance` fixture.
    fn test_ctx() -> Ctx {
        Ctx::new(&config(
            "match:\n  distance_weights:\n    data_source: 2.0\n    album: 4.0\n    medium: 2.0\n",
        ))
    }

    fn penalties<'d>(d: &'d Distance, key: &str) -> &'d [f64] {
        &d.penalties.iter().find(|(k, _)| k == key).unwrap().1
    }

    #[test]
    fn distance_add_methods() {
        let mut d = test_ctx().dist();
        d.add("add", 1.0);
        assert_eq!(d.penalties, vec![("add".to_string(), vec![1.0])]);

        d.add_equality("equality", "ghi", &["abc", "def", "ghi"]);
        d.add_equality("equality", "xyz", &["abc", "def", "ghi"]);
        assert_eq!(penalties(&d, "equality"), [0.0, 1.0]);

        d.add_expr("expr", true);
        d.add_expr("expr", false);
        assert_eq!(penalties(&d, "expr"), [1.0, 0.0]);

        for (a, b) in [(1, 1), (1, 2), (2, 1), (-1, 2)] {
            d.add_number("number", a, b);
        }
        assert_eq!(penalties(&d, "number"), [0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);

        let re =
            |opts: &[&str]| -> Vec<Regex> { opts.iter().map(|o| pref_regex(o).unwrap()).collect() };
        d.add_priority("priority", "abc", &re(&["abc"]));
        d.add_priority("priority", "def", &re(&["abc", "def"]));
        d.add_priority("priority", "gh", &re(&["ab", "cd", "ef", "GH"]));
        d.add_priority("priority", "xyz", &re(&["abc", "def"]));
        assert_eq!(penalties(&d, "priority"), [0.0, 0.5, 0.75, 1.0]);

        for (a, b) in [(25.0, 100.0), (10.0, 5.0), (-5.0, 5.0), (5.0, 0.0)] {
            d.add_ratio("ratio", a, b);
        }
        assert_eq!(penalties(&d, "ratio"), [0.25, 1.0, 0.0, 0.0]);

        d.add_string("string", Some("abc"), Some("bcd"));
        d.add_string("string", Some("abc"), None);
        d.add_string("string", None, None);
        assert_eq!(penalties(&d, "string"), [2.0 / 3.0, 1.0, 0.0]);
    }

    #[test]
    fn distance_totals_and_shares() {
        let mut d = test_ctx().dist();
        d.add("album", 0.5);
        d.add("media", 0.25);
        d.add("media", 0.75);
        assert_eq!(d.distance(), 0.5);
        assert_eq!(d.max(), 6.0);
        assert_eq!(d.raw(), 3.0);
        assert_eq!(d.get("album"), 1.0 / 3.0);
        assert_eq!(d.get("media"), 1.0 / 6.0);
    }

    #[test]
    fn distance_items_and_keys() {
        let ctx = test_ctx();
        let mut d = ctx.dist();
        d.add("data_source", 0.0);
        d.add("album", 0.5);
        d.add("medium", 0.25);
        d.add("medium", 0.75);
        assert_eq!(
            d.items(),
            [("album".to_string(), 0.2), ("medium".to_string(), 0.2)]
        );
        assert_eq!(d.distance(), 0.4);

        let mut d = ctx.dist();
        d.add("album", 0.1875);
        d.add("medium", 0.75);
        assert_eq!(
            d.items(),
            [("medium".to_string(), 0.25), ("album".to_string(), 0.125)]
        );
        // Equal shares sort by key.
        let mut d = ctx.dist();
        d.add("album", 0.375);
        d.add("medium", 0.75);
        assert_eq!(
            d.items(),
            [("album".to_string(), 0.25), ("medium".to_string(), 0.25)]
        );
    }

    #[test]
    fn generic_penalty_keys_are_in_insertion_order_without_zeros() {
        let mut d = test_ctx().dist();
        d.add("medium", 0.25);
        d.add("album", 0.5);
        d.add("label", 0.0);
        d.add("track_title", 0.5);
        d.add("missing_tracks", 1.0);
        assert_eq!(
            d.generic_penalty_keys(),
            ["medium", "album", "title", "missing tracks"]
        );
    }

    fn item(title: &str, artist: &str) -> Item {
        let mut i = Item::new_item();
        i.set_str("title", title);
        i.set_str("artist", artist);
        i
    }

    #[test]
    fn track_distance_penalties() {
        let ctx = Ctx::new(&Config::defaults());
        let info = TrackInfo {
            title: "title".into(),
            artist: "artist".into(),
            ..Default::default()
        };
        for (title, artist, penalty) in [
            ("title", "artist", false),
            ("title", "Various Artists", false),
            ("title", "different artist", true),
            ("different title", "artist", true),
        ] {
            let d = track_distance(&ctx, &item(title, artist), &info, true);
            assert_eq!(d.distance() != 0.0, penalty, "{title} {artist}");
        }
    }

    /// Upstream's `TestDataSourceDistance`, for the cases that apply with
    /// MusicBrainz as the only source.
    #[test]
    fn data_source_distance() {
        for (item_source, info_source, settings, want) in [
            ("MusicBrainz", "MusicBrainz", "", 0.0),
            ("Discogs", "MusicBrainz", "", 0.125),
            ("", "MusicBrainz", "", 0.0),
            ("Discogs", "Discogs", "", 0.0),
            (
                "Discogs",
                "MusicBrainz",
                "musicbrainz:\n  data_source_mismatch_penalty: 1.0\n",
                0.25,
            ),
            (
                "Discogs",
                "MusicBrainz",
                "musicbrainz:\n  source_weight: 1.0\n",
                0.25,
            ),
            (
                "Discogs",
                "MusicBrainz",
                "musicbrainz:\n  data_source_mismatch_penalty: 0.0\n",
                0.0,
            ),
        ] {
            for (weight, scaled) in [(1.0, want), (5.0, want * 2.5), (0.0, 0.0)] {
                let cfg = config(&format!(
                    "{settings}match:\n  distance_weights:\n    data_source: {weight}\n"
                ));
                let ctx = Ctx::new(&cfg);
                let mut it = item("", "");
                if !item_source.is_empty() {
                    it.set_str("data_source", item_source);
                }
                let info = TrackInfo {
                    data_source: info_source.into(),
                    ..Default::default()
                };
                let d = track_distance(&ctx, &it, &info, false).distance();
                assert_eq!(d, scaled, "{item_source} {info_source} {settings} {weight}");
            }
        }
    }

    fn album_items() -> Vec<Item> {
        [("one", 1), ("two", 2), ("three", 3)]
            .iter()
            .map(|(t, n)| {
                let mut i = item(t, "artist");
                i.set_str("album", "album");
                i.set_int("track", *n);
                i.set("length", Value::Float(1.0));
                i
            })
            .collect()
    }

    fn album_info(items: &[Item]) -> AlbumInfo {
        AlbumInfo {
            artist: "artist".into(),
            album: "album".into(),
            tracks: items
                .iter()
                .map(|i| TrackInfo {
                    title: i.str("title").into(),
                    artist: i.str("artist").into(),
                    index: i.int("track") as usize,
                    length: Some(1.0),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Upstream's `get_dist`: items and tracks paired in order.
    fn get_dist(items: &[Item], info: &AlbumInfo) -> f64 {
        let ctx = Ctx::new(&Config::defaults());
        let n = items.len().min(info.tracks.len());
        let mapping: Vec<(usize, usize)> = (0..n).map(|i| (i, i)).collect();
        let unmatched = items.len().saturating_sub(info.tracks.len());
        album_distance(
            &ctx,
            &current_metadata(items),
            items,
            info,
            &mapping,
            unmatched,
        )
        .distance()
    }

    #[test]
    fn album_distances_as_beets() {
        let items = album_items();
        let info = album_info(&items);
        assert_eq!(get_dist(&items, &info), 0.0);

        let mut i = album_info(&items);
        i.tracks.pop();
        assert_eq!(get_dist(&items, &i), 0.05660377358490566);

        let mut i = album_info(&items);
        i.tracks.push(TrackInfo {
            index: 4,
            title: "four".into(),
            artist: "artist".into(),
            length: Some(1.0),
            ..Default::default()
        });
        assert_eq!(get_dist(&items, &i), 0.06976744186046512);

        let mut i = album_info(&items);
        let t = i.tracks[1].title.clone();
        i.tracks[1].title = i.tracks[2].title.clone();
        i.tracks[2].title = t;
        assert_eq!(get_dist(&items, &i), 0.13333333333333336);

        // The album artist counts unless it's a VA release.
        let mut i = album_info(&items);
        i.artist = "another artist".into();
        assert_eq!(get_dist(&items, &i), 0.1346153846153846);
        i.va = true;
        assert_eq!(get_dist(&items, &i), 0.0);
        // Some VA releases have no track artists.
        for t in &mut i.tracks {
            t.artist.clear();
        }
        assert_eq!(get_dist(&items, &i), 0.0);

        let mut i = album_info(&items);
        i.va = true;
        i.tracks[0].artist = "another artist".into();
        assert_eq!(get_dist(&items, &i), 0.029914529914529912);

        // Per-disc numbering counts as matching.
        let mut i = album_info(&items);
        i.tracks[0].medium_index = 1;
        i.tracks[1].medium_index = 2;
        i.tracks[2].medium_index = 1;
        assert_eq!(get_dist(&items, &i), 0.0);
    }

    /// Items and a release that differ on every album field; the numbers
    /// are reference beets 2.14's for the same objects (in 2026).
    fn rich() -> (Vec<Item>, AlbumInfo) {
        let items = [("Alpha", 203.0), ("Beta", 171.5), ("Gamma", 260.0)]
            .iter()
            .enumerate()
            .map(|(n, (t, l))| {
                let mut i = item(t, "Fixture Artist");
                for (k, v) in [
                    ("albumartist", "Fixture Artist"),
                    ("album", "Fixture Album"),
                    ("label", "Fixture Rec."),
                    ("catalognum", "FX 1"),
                    ("country", "US"),
                    ("media", "Vinyl"),
                    ("albumdisambig", "first press"),
                    ("mb_albumid", "11111111-1111-1111-1111-111111111111"),
                    ("mb_trackid", &format!("r000000{}", n + 1)),
                    ("data_source", "Discogs"),
                ] {
                    i.set_str(k, v);
                }
                for (k, v) in [
                    ("track", n as i64 + 1),
                    ("disc", 1),
                    ("disctotal", 2),
                    ("year", 2001),
                ] {
                    i.set_int(k, v);
                }
                i.set("length", Value::Float(*l));
                i
            })
            .collect();
        let tracks = [
            ("Alpha", 200.0),
            ("Beta", 180.0),
            ("Gamma (Live)", 240.0),
            ("Delta", 100.0),
        ]
        .iter()
        .enumerate()
        .map(|(n, (t, l))| TrackInfo {
            title: t.to_string(),
            artist: "Fixture Artist".into(),
            index: n + 1,
            medium: 1,
            medium_index: n + 1,
            length: Some(*l),
            track_id: format!("r000000{}", if n == 1 { 9 } else { n + 1 }),
            data_source: "MusicBrainz".into(),
            ..Default::default()
        })
        .collect();
        let info = AlbumInfo {
            artist: "Fixture Artist".into(),
            album: "Fixture Album".into(),
            album_id: "22222222-2222-2222-2222-222222222222".into(),
            year: 2004,
            original_year: 1999,
            country: "GB".into(),
            label: "Fixture Records".into(),
            catalognum: "FX-001".into(),
            albumdisambig: "deluxe".into(),
            media: "CD".into(),
            mediums: 1,
            data_source: "MusicBrainz".into(),
            tracks,
            ..Default::default()
        };
        (items, info)
    }

    fn rich_match(settings: &str) -> (Ctx, AlbumMatch) {
        let mut ctx = Ctx::new(&config(settings));
        ctx.this_year = 2026;
        let (items, info) = rich();
        let m = match_album(&ctx, &current_metadata(&items), &items, info);
        (ctx, m)
    }

    fn items_of(d: &Distance) -> Vec<(&str, f64)> {
        let mut v: Vec<(&str, f64)> = vec![];
        for (k, x) in d.items() {
            let k = d
                .penalties
                .iter()
                .find(|(p, _)| *p == k)
                .unwrap()
                .0
                .as_str();
            v.push((k, x));
        }
        v
    }

    #[test]
    fn every_album_penalty_as_beets() {
        let (ctx, m) = rich_match("");
        let d = &m.distance;
        assert_eq!(d.distance(), 0.4658315802894117);
        assert_eq!(
            d.generic_penalty_keys(),
            [
                "media",
                "mediums",
                "year",
                "country",
                "label",
                "catalognum",
                "albumdisambig",
                "id",
                "tracks",
                "missing tracks",
                "data source"
            ]
        );
        assert_eq!(
            items_of(d),
            [
                ("album_id", 0.20080321285140562),
                ("tracks", 0.05201759418626888),
                ("data_source", 0.040160642570281124),
                ("media", 0.040160642570281124),
                ("mediums", 0.040160642570281124),
                ("missing_tracks", 0.03614457831325302),
                ("country", 0.020080321285140562),
                ("albumdisambig", 0.01807228915662651),
                ("catalognum", 0.008032128514056226),
                ("label", 0.005737234652897304),
                ("year", 0.004462293618920125),
            ]
        );
        let tracks: Vec<f64> = d.tracks.iter().map(Distance::distance).collect();
        assert_eq!(
            tracks,
            [0.07142857142857142, 0.42857142857142855, 0.1476190476190476]
        );
        assert_eq!(recommend(&ctx, &[d]), Recommendation::Low);
    }

    #[test]
    fn preferred_media_countries_and_original_year_as_beets() {
        let (_, m) = rich_match(
            "match:\n  preferred:\n    countries: ['US', 'GB|UK']\n    media: ['CD', 'Digital Media|File']\n    original_year: yes\n",
        );
        assert_eq!(m.distance.distance(), 0.4186056394891736);
        assert_eq!(
            items_of(&m.distance),
            [
                ("album_id", 0.20080321285140562),
                ("tracks", 0.05201759418626888),
                ("data_source", 0.040160642570281124),
                ("mediums", 0.040160642570281124),
                ("missing_tracks", 0.03614457831325302),
                ("albumdisambig", 0.01807228915662651),
                ("country", 0.010040160642570281),
                ("catalognum", 0.008032128514056226),
                ("year", 0.007437156031533542),
                ("label", 0.005737234652897304),
            ]
        );
        // "2xCD" counts as CD; a later preference costs more; none, 1.0.
        let ctx = Ctx::new(&config(
            "match:\n  preferred:\n    media: ['CD', 'Vinyl']\n",
        ));
        for (media, want) in [
            ("2xCD", 0.0),
            ("CD", 0.0),
            ("12\" Vinyl", 1.0),
            ("Vinyl", 0.5),
        ] {
            let mut d = ctx.dist();
            d.add_priority("media", media, &ctx.preferred_media);
            assert_eq!(d.penalties[0].1, [want], "{media}");
        }
    }

    #[test]
    fn max_rec_caps_by_album_and_track_penalties() {
        // The track IDs differ on one track only: a track-level penalty.
        let (_, m) = rich_match("");
        let ctx = Ctx::new(&config(
            "match:\n  strong_rec_thresh: 0.9\n  max_rec:\n    track_id: low\n",
        ));
        assert!(!m.distance.has("track_id"));
        assert_eq!(recommend(&ctx, &[&m.distance]), Recommendation::Low);
        let ctx = Ctx::new(&config("match:\n  strong_rec_thresh: 0.9\n"));
        // The default caps: missing tracks.
        assert_eq!(recommend(&ctx, &[&m.distance]), Recommendation::Medium);
        let ctx = Ctx::new(&config(
            "match:\n  strong_rec_thresh: 0.9\n  max_rec:\n    missing_tracks: strong\n",
        ));
        assert_eq!(recommend(&ctx, &[&m.distance]), Recommendation::Strong);
    }

    #[test]
    fn required_and_ignored_drop_candidates() {
        let (items, info) = rich();
        let cur = current_metadata(&items);
        let run = |settings: &str, info: AlbumInfo| {
            let ctx = Ctx::new(&config(settings));
            let mut out = vec![];
            add_candidate(&ctx, &cur, &items, &mut out, info);
            out.len()
        };
        assert_eq!(run("", info.clone()), 1);
        assert_eq!(run("match:\n  ignored: missing_tracks\n", info.clone()), 0);
        assert_eq!(
            run("match:\n  ignored: [unmatched_tracks]\n", info.clone()),
            1
        );
        assert_eq!(run("match:\n  required: year label\n", info.clone()), 1);
        let mut no_label = info.clone();
        no_label.nulls.insert("label");
        no_label.label.clear();
        assert_eq!(run("match:\n  required: year label\n", no_label), 0);
        let mut no_year = info.clone();
        no_year.year = 0;
        assert_eq!(run("match:\n  required: [year]\n", no_year), 0);
        // No tracks: never a candidate.
        let mut empty = info.clone();
        empty.tracks.clear();
        assert_eq!(run("", empty), 0);
        // The same release twice counts once.
        let ctx = Ctx::new(&Config::defaults());
        let mut out = vec![];
        add_candidate(&ctx, &cur, &items, &mut out, info.clone());
        add_candidate(&ctx, &cur, &items, &mut out, info);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn likelies_are_pluralities_ties_to_the_first() {
        let mut items: Vec<Item> = ["b", "a", "a", "b", ""]
            .iter()
            .map(|l| {
                let mut i = item("t", "x");
                i.set_str("label", *l);
                i
            })
            .collect();
        assert_eq!(current_metadata(&items).label, "b");
        // Empty values count.
        for i in &mut items[..3] {
            i.set_str("label", "");
        }
        assert_eq!(current_metadata(&items).label, "");
        // The album artist is the artist only when all items agree on it.
        items[0].set_str("albumartist", "AA");
        let cur = current_metadata(&items);
        assert_eq!((cur.artist.as_str(), cur.va), ("x", false));
        for i in &mut items {
            i.set_str("albumartist", "AA");
        }
        items[1].set_str("artist", "y");
        let cur = current_metadata(&items);
        assert_eq!((cur.artist.as_str(), cur.va), ("AA", true));
    }

    // ---------------------------------------------- upstream test_match.py

    fn titled(titles: &[&str]) -> Vec<Item> {
        titles.iter().map(|t| item(t, "")).collect()
    }

    fn tracks(titles: &[&str]) -> Vec<TrackInfo> {
        titles
            .iter()
            .map(|t| TrackInfo {
                title: t.to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn assigns_tracks_as_beets() {
        let ctx = Ctx::new(&Config::defaults());
        let (a, b, c) = ("one", "two", "three");
        for (items, tracks_, want, extra_items, extra_tracks) in [
            (
                vec![a, c, b],
                vec![a, b, c],
                vec![(a, a), (b, b), (c, c)],
                vec![],
                vec![],
            ),
            (
                vec![b, c],
                vec![a, b, c],
                vec![(b, b), (c, c)],
                vec![],
                vec![a],
            ),
            (
                vec![a, c],
                vec![a, b, c],
                vec![(a, a), (c, c)],
                vec![],
                vec![b],
            ),
            (
                vec![a, b],
                vec![a, b, c],
                vec![(a, a), (b, b)],
                vec![],
                vec![c],
            ),
            (
                vec![a, c, b],
                vec![a, c],
                vec![(a, a), (c, c)],
                vec![b],
                vec![],
            ),
        ] {
            let its = titled(&items);
            let info = AlbumInfo {
                tracks: tracks(&tracks_),
                ..Default::default()
            };
            let m = match_album(&ctx, &current_metadata(&its), &its, info);
            let mut got: Vec<(&str, &str)> = m
                .mapping
                .iter()
                .map(|&(i, t)| (its[i].str("title"), m.info.tracks[t].title.as_str()))
                .collect();
            got.sort();
            let mut want = want;
            want.sort();
            assert_eq!(got, want);
            let ei: Vec<&str> = m.extra_items.iter().map(|&i| its[i].str("title")).collect();
            let et: Vec<&str> = m
                .extra_tracks
                .iter()
                .map(|&t| m.info.tracks[t].title.as_str())
                .collect();
            assert_eq!((ei, et), (extra_items, extra_tracks));
        }
    }

    #[test]
    fn order_works_when_track_names_are_entirely_wrong() {
        let ctx = Ctx::new(&Config::defaults());
        let lengths = [
            241.37243007106997,
            342.27781704375036,
            245.95070222338137,
            472.87662515485437,
            279.1759535763187,
            270.33333768012,
            247.83435613222923,
            216.54504531525072,
            225.72775379800484,
            317.7643606963552,
            243.57001238834192,
            186.45916150485752,
        ];
        let items: Vec<Item> = lengths
            .iter()
            .enumerate()
            .map(|(n, l)| {
                let mut i = item(
                    &format!("ben harper - Burn to Shine {}", n + 1),
                    "ben harper",
                );
                i.set_str("album", "burn to shine");
                i.set_int("track", n as i64 + 1);
                i.set("length", Value::Float(*l));
                i
            })
            .collect();
        let infos = [
            ("Alone", 238.893),
            ("The Woman in You", 341.44),
            ("Less", 245.6),
            ("Two Hands of a Prayer", 470.493),
            ("Please Bleed", 277.866),
            ("Suzie Blue", 269.306),
            ("Steal My Kisses", 245.36),
            ("Burn to Shine", 214.906),
            ("Show Me a Little Shame", 224.0929999999999),
            ("Forgiven", 317.2),
            ("Beloved One", 243.733),
            ("In the Lord's Arms", 186.133),
        ];
        let tracks: Vec<TrackInfo> = infos
            .iter()
            .enumerate()
            .map(|(n, (t, l))| TrackInfo {
                title: t.to_string(),
                length: Some(*l),
                index: n + 1,
                ..Default::default()
            })
            .collect();
        let pairs = assign(&ctx, &items, &tracks);
        assert_eq!(pairs, (0..12).map(|i| (i, i)).collect::<Vec<_>>());
    }

    // ------------------------------------------------------------ applying

    fn track_item(title: &str, track: i64, len: f64) -> Item {
        let mut i = item(title, "Band");
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
            // What MusicBrainz didn't give is None, as parsed.
            nulls: ["isrc", "trackdisambig", "disctitle", "media"].into(),
            ..Default::default()
        };
        AlbumInfo {
            album_id: "rel".into(),
            album: "Record".into(),
            artist: "Band".into(),
            year: 2000,
            mediums: 1,
            nulls: ["label", "catalognum", "albumdisambig", "asin"].into(),
            tracks: vec![
                t("Alpha", 1, 200.0),
                t("Beta", 2, 180.0),
                t("Gamma", 3, 240.0),
            ],
            ..Default::default()
        }
    }

    fn matched(cfg: &Config, items: &[Item], info: AlbumInfo) -> AlbumMatch {
        match_album(&Ctx::new(cfg), &current_metadata(items), items, info)
    }

    #[test]
    fn assigns_shuffled_items_and_recommends_strong() {
        let cfg = Config::defaults();
        let items = vec![
            track_item("Gamma", 3, 241.0),
            track_item("Alpha", 1, 199.0),
            track_item("Beta", 2, 181.0),
        ];
        let m = matched(&cfg, &items, info());
        assert_eq!(m.mapping, vec![(1, 0), (2, 1), (0, 2)]);
        assert_eq!(m.distance.distance(), 0.0);
        assert_eq!(
            recommend(&Ctx::new(&cfg), &[&m.distance]),
            Recommendation::Strong
        );

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
        let mut it = track_item("Alpha", 1, 200.0);
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
        // ... and clear them when listed (by the match's name for them).
        let cfg = config("overwrite_null:\n  album: [label, original_year]\n  track: [isrc]\n");
        apply_album(&cfg, &mut items, &m);
        assert_eq!(items[0].str("label"), "");
        assert_eq!(items[0].int("original_year"), 0);
        assert_eq!(items[0].str("isrc"), "");
    }

    #[test]
    fn missing_tracks_cap_recommendation() {
        let cfg = Config::defaults();
        let items = vec![track_item("Alpha", 1, 200.0), track_item("Beta", 2, 180.0)];
        let m = matched(&cfg, &items, info());
        assert_eq!(m.extra_tracks, vec![2]);
        assert!(
            m.distance
                .generic_penalty_keys()
                .contains(&"missing tracks".to_string())
        );
        assert!(recommend(&Ctx::new(&cfg), &[&m.distance]) <= Recommendation::Medium);
    }

    #[test]
    fn close_weak_candidates_give_no_recommendation() {
        let cfg = Config::defaults();
        let mut items = vec![track_item("Foo", 1, 30.0), track_item("Bar", 2, 500.0)];
        for i in &mut items {
            i.set_str("artist", "Someone Else");
            i.set_str("album", "Other");
        }
        let a = matched(&cfg, &items, info());
        let b = matched(&cfg, &items, info());
        assert!(a.distance.distance() > 0.4, "{}", a.distance.distance());
        let ctx = Ctx::new(&cfg);
        assert_eq!(recommend(&ctx, &[&a.distance]), Recommendation::Low);
        // Two equally weak candidates: beets has no recommendation.
        assert_eq!(
            recommend(&ctx, &[&a.distance, &b.distance]),
            Recommendation::None
        );
    }
}
