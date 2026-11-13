//! MusicBrainz metadata source: a port of beets 2.14's `musicbrainz`
//! plugin (`beetsplug/musicbrainz.py`) and its web service helper
//! (`beetsplug/_utils/musicbrainz.py`): release and recording search, the
//! lookups with beets' includes, and the release/recording JSON as beets'
//! `AlbumInfo`/`TrackInfo`.

use crate::config::Config;
use crate::hooks::{Data, V};
use crate::model::Item;
use anyhow::{Context, Result};
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
const BASE_URL: &str = "https://musicbrainz.org/";

/// beets' `RELEASE_INCLUDES`.
const RELEASE_INCLUDES: &[&str] = &[
    "artists",
    "media",
    "recordings",
    "release-groups",
    "labels",
    "artist-credits",
    "aliases",
    "recording-level-rels",
    "work-rels",
    "work-level-rels",
    "artist-rels",
    "isrcs",
    "url-rels",
    "release-rels",
    "genres",
    "tags",
];
/// beets' `RECORDING_INCLUDES`.
const RECORDING_INCLUDES: &[&str] = &[
    "artists",
    "aliases",
    "isrcs",
    "work-rels",
    "work-level-rels",
    "artist-rels",
];
/// beets' `BROWSE_INCLUDES`, for releases too big to come with their
/// recordings' relations.
const BROWSE_INCLUDES: &[&str] = &[
    "artist-credits",
    "aliases",
    "work-rels",
    "artist-rels",
    "recording-rels",
    "release-rels",
];
const BROWSE_CHUNKSIZE: usize = 100;
const BROWSE_MAXTRACKS: usize = 500;

/// beets' `FIELDS_TO_MB_KEYS`: `musicbrainz.extra_tags` and the search
/// fields they fill.
const FIELDS_TO_MB_KEYS: &[(&str, &str)] = &[
    ("barcode", "barcode"),
    ("catalognum", "catno"),
    ("country", "country"),
    ("label", "label"),
    ("media", "format"),
    ("year", "date"),
    ("tracks", "tracks"),
    ("alias", "alias"),
];

/// `musicbrainz.external_ids` sources, in beets' order.
const EXTERNAL_SOURCES: &[&str] = &["discogs", "bandcamp", "spotify", "deezer", "tidal"];

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub title: String,
    pub track_id: String,
    pub release_track_id: String,
    pub artist: String,
    pub artist_id: String,
    pub artist_sort: String,
    pub artist_credit: String,
    pub artists: Vec<String>,
    pub artists_ids: Vec<String>,
    pub artists_sort: Vec<String>,
    pub artists_credit: Vec<String>,
    pub length: Option<f64>,
    /// 1-based position across the whole release; 0 (None) for a
    /// recording on its own, as are the other positional fields.
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
    /// The last work related to the recording, and its composers and
    /// lyricists (beets' `_parse_work_relations`).
    pub work: Option<String>,
    pub mb_workid: Option<String>,
    pub composers: Option<Vec<String>>,
    pub composers_ids: Vec<String>,
    pub composer_sort: Option<String>,
    pub lyricists: Option<Vec<String>>,
    pub lyricists_ids: Vec<String>,
    /// The recording's arrangers and remixers.
    pub arrangers: Option<Vec<String>>,
    pub arrangers_ids: Vec<String>,
    pub remixers: Option<Vec<String>>,
    pub remixers_ids: Vec<String>,
    /// Fields beets would have as None (MusicBrainz gave nothing), so that
    /// a disambiguation string or `match.required` sees them as beets does.
    pub nulls: BTreeSet<&'static str>,
    /// Fields beets' info dict would not have at all (the data source's
    /// constructor does not set them).
    pub omit: BTreeSet<&'static str>,
    /// Extra information fields (mbpseudo's custom tags, and the fields the
    /// other metadata source plugins set: `bpm`, `initial_key`, `genres`,
    /// `deezer_track_id`, ...). In [`TrackInfo::to_data`] they override or
    /// add to the fields above.
    pub extra: Vec<(String, V)>,
}

#[derive(Debug, Clone, Default)]
pub struct AlbumInfo {
    pub album_id: String,
    pub album: String,
    pub artist: String,
    pub artist_id: String,
    pub artist_sort: String,
    pub artist_credit: String,
    pub artists: Vec<String>,
    pub artists_ids: Vec<String>,
    pub artists_sort: Vec<String>,
    pub artists_credit: Vec<String>,
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
    pub albumtypes: Vec<String>,
    pub albumstatus: String,
    pub albumdisambig: String,
    pub releasegroup_id: String,
    pub release_group_title: Option<String>,
    pub releasegroupdisambig: String,
    pub media: String,
    pub mediums: usize,
    pub tracks: Vec<TrackInfo>,
    pub barcode: String,
    pub asin: String,
    pub script: String,
    pub language: String,
    /// `musicbrainz.genres`: the release group's and release's genres (or
    /// tags), most counted first.
    pub genres: Option<Vec<String>>,
    /// `musicbrainz.external_ids`: (`discogs_album_id`, id), in the order
    /// beets finds them.
    pub external_ids: Vec<(String, Option<String>)>,
    pub data_source: String,
    pub data_url: String,
    /// Fields beets would have as None (see [`TrackInfo::nulls`]).
    pub nulls: BTreeSet<&'static str>,
    /// Fields beets' info dict would not have at all (see
    /// [`TrackInfo::omit`]).
    pub omit: BTreeSet<&'static str>,
    /// Extra information fields (see [`TrackInfo::extra`]):
    /// `spotify_album_id`, ...
    pub extra: Vec<(String, V)>,
}

/// A string field: None if listed in `nulls`.
fn nullable(nulls: &BTreeSet<&'static str>, name: &str, v: &str) -> V {
    if nulls.contains(name) {
        V::None
    } else {
        V::Str(v.to_string())
    }
}

fn list(v: &[String]) -> V {
    V::List(v.to_vec())
}

impl AlbumInfo {
    /// The info as beets' `dict(album_info)` (without `tracks`).
    pub fn to_data(&self) -> Data {
        let n = &self.nulls;
        let s = |k: &str, v: &str| nullable(n, k, v);
        let mut d: Data = [
            ("album", V::Str(self.album.clone())),
            ("album_id", V::Str(self.album_id.clone())),
            ("albumdisambig", s("albumdisambig", &self.albumdisambig)),
            ("albumstatus", s("albumstatus", &self.albumstatus)),
            ("albumtype", s("albumtype", &self.albumtype)),
            ("albumtypes", list(&self.albumtypes)),
            ("artist", V::Str(self.artist.clone())),
            ("artist_credit", s("artist_credit", &self.artist_credit)),
            ("artist_id", s("artist_id", &self.artist_id)),
            ("artist_sort", s("artist_sort", &self.artist_sort)),
            ("artists", list(&self.artists)),
            (
                "artists_credit",
                if n.contains("artists_credit") {
                    V::None
                } else {
                    list(&self.artists_credit)
                },
            ),
            ("artists_ids", list(&self.artists_ids)),
            (
                "artists_sort",
                if n.contains("artists_sort") {
                    V::None
                } else {
                    list(&self.artists_sort)
                },
            ),
            ("asin", s("asin", &self.asin)),
            ("barcode", s("barcode", &self.barcode)),
            ("catalognum", s("catalognum", &self.catalognum)),
            ("country", s("country", &self.country)),
            ("data_source", s("data_source", &self.data_source)),
            ("data_url", s("data_url", &self.data_url)),
            ("day", V::int_or_none(self.day)),
            ("discogs_albumid", V::None),
            ("discogs_artistid", V::None),
            ("discogs_labelid", V::None),
            ("genres", V::opt_list(self.genres.clone())),
            ("label", s("label", &self.label)),
            ("language", s("language", &self.language)),
            ("media", s("media", &self.media)),
            (
                "mediums",
                if n.contains("mediums") {
                    V::None
                } else {
                    V::Int(self.mediums as i64)
                },
            ),
            ("month", V::int_or_none(self.month)),
            ("original_day", V::int_or_none(self.original_day)),
            ("original_month", V::int_or_none(self.original_month)),
            ("original_year", V::int_or_none(self.original_year)),
            (
                "release_group_title",
                V::opt_str(self.release_group_title.clone()),
            ),
            (
                "releasegroup_id",
                s("releasegroup_id", &self.releasegroup_id),
            ),
            (
                "releasegroupdisambig",
                s("releasegroupdisambig", &self.releasegroupdisambig),
            ),
            ("script", s("script", &self.script)),
            ("style", V::None),
            ("va", V::Bool(self.va)),
            ("year", V::int_or_none(self.year)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .filter(|(k, _)| !self.omit.contains(k.as_str()))
        .collect();
        for (k, v) in &self.external_ids {
            d.insert(k.clone(), V::opt_str(v.clone()));
        }
        for (k, v) in &self.extra {
            d.insert(k.clone(), v.clone());
        }
        d
    }

    /// beets' `info[name]`, as `str()` shows it: None if the info has no
    /// such field, `Some(None)` if the field is None.
    pub fn field(&self, name: &str) -> Option<Option<String>> {
        if name == "tracks" {
            return Some(Some(format!("<{} tracks>", self.tracks.len())));
        }
        let v = self.to_data().remove(name)?;
        Some((v != V::None).then(|| v.py_str()))
    }
}

impl TrackInfo {
    /// The info as beets' `dict(track_info)`.
    pub fn to_data(&self) -> Data {
        let n = &self.nulls;
        let s = |k: &str, v: &str| nullable(n, k, v);
        let positioned = self.index != 0;
        // A positional value, None when the source gave none (a field in
        // `nulls`, or no position at all).
        let pos = |k: &str, v: usize| {
            if n.contains(k) {
                V::None
            } else if positioned {
                V::Int(v as i64)
            } else {
                V::None
            }
        };
        [
            ("album", V::None),
            ("arrangers", V::opt_list(self.arrangers.clone())),
            ("arrangers_ids", list(&self.arrangers_ids)),
            ("artist", V::Str(self.artist.clone())),
            ("artist_credit", s("artist_credit", &self.artist_credit)),
            ("artist_id", s("artist_id", &self.artist_id)),
            ("artist_sort", s("artist_sort", &self.artist_sort)),
            ("artists", list(&self.artists)),
            (
                "artists_credit",
                if n.contains("artists_credit") {
                    V::None
                } else {
                    list(&self.artists_credit)
                },
            ),
            ("artists_ids", list(&self.artists_ids)),
            (
                "artists_sort",
                if n.contains("artists_sort") {
                    V::None
                } else {
                    list(&self.artists_sort)
                },
            ),
            ("bpm", V::None),
            ("composer_sort", V::opt_str(self.composer_sort.clone())),
            ("composers", V::opt_list(self.composers.clone())),
            ("composers_ids", list(&self.composers_ids)),
            ("data_source", s("data_source", &self.data_source)),
            ("data_url", s("data_url", &self.data_url)),
            ("disctitle", s("disctitle", &self.disctitle)),
            ("genres", V::None),
            ("index", pos("index", self.index)),
            ("initial_key", V::None),
            ("isrc", s("isrc", &self.isrc)),
            ("length", self.length.map_or(V::None, V::Float)),
            ("lyricists", V::opt_list(self.lyricists.clone())),
            ("lyricists_ids", list(&self.lyricists_ids)),
            ("mb_workid", V::opt_str(self.mb_workid.clone())),
            ("media", s("media", &self.media)),
            ("medium", pos("medium", self.medium)),
            ("medium_index", pos("medium_index", self.medium_index)),
            ("medium_total", pos("medium_total", self.medium_total)),
            (
                "release_track_id",
                if self.release_track_id.is_empty() {
                    V::None
                } else {
                    V::Str(self.release_track_id.clone())
                },
            ),
            ("remixers", V::opt_list(self.remixers.clone())),
            ("remixers_ids", list(&self.remixers_ids)),
            ("title", V::Str(self.title.clone())),
            ("track_alt", V::opt_str(self.track_alt.clone())),
            ("track_id", V::Str(self.track_id.clone())),
            ("trackdisambig", s("trackdisambig", &self.trackdisambig)),
            ("work", V::opt_str(self.work.clone())),
            ("work_disambig", V::None),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .filter(|(k, _)| !self.omit.contains(k.as_str()))
        .chain(self.extra.iter().cloned())
        .collect()
    }

    /// beets' `info[name]` for a track (see [`AlbumInfo::field`]).
    pub fn field(&self, name: &str) -> Option<Option<String>> {
        let v = self.to_data().remove(name)?;
        Some((v != V::None).then(|| v.py_str()))
    }
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
                    && al.get("primary").and_then(J::as_bool) == Some(true)
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
/// `ignore_data_tracks`, `ignore_video_tracks`, for the release event
/// `match.preferred.countries`, `va_name`, and the plugin's `genres`,
/// `genres_tag` and `external_ids`.
#[derive(Debug, Clone)]
pub struct ParseOpts {
    pub aliases: AliasPrefs,
    pub ignored_media: Vec<String>,
    pub ignore_data_tracks: bool,
    pub ignore_video_tracks: bool,
    pub preferred_countries: Vec<String>,
    pub va_name: String,
    /// `musicbrainz.genres`: the JSON field to read them from ("genres",
    /// or "tags" with `genres_tag: tag`), or None.
    pub genres_field: Option<&'static str>,
    /// `musicbrainz.external_ids`: the sources wanted.
    pub external_ids: Vec<String>,
    /// mbpseudo's `scripts`: pseudo-release scripts wanted.
    pub pseudo_scripts: Vec<String>,
    /// mbpseudo's `custom_tags_only`: translate only into custom tags.
    pub custom_tags_only: bool,
    /// mbpseudo's `album_custom_tags` / `track_custom_tags`.
    pub album_custom_tags: Vec<(String, String)>,
    pub track_custom_tags: Vec<(String, String)>,
    /// The data source the parsed infos report: "MusicBrainz", or
    /// `"MusicBrainzPseudoRelease"` while the mbpseudo plugin is enabled
    /// (beets' `MusicBrainzPseudoReleasePlugin.data_source`).
    pub data_source: String,
}

impl Default for ParseOpts {
    fn default() -> Self {
        ParseOpts {
            aliases: AliasPrefs::default(),
            ignored_media: vec![],
            ignore_data_tracks: true,
            ignore_video_tracks: true,
            preferred_countries: vec![],
            va_name: "Various Artists".into(),
            genres_field: None,
            external_ids: vec![],
            pseudo_scripts: vec![],
            custom_tags_only: false,
            album_custom_tags: vec![],
            track_custom_tags: vec![],
            data_source: "MusicBrainz".into(),
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
            va_name: cfg.str("va_name"),
            genres_field: cfg.bool("musicbrainz.genres").then(|| {
                if cfg.str("musicbrainz.genres_tag") == "tag" {
                    "tags"
                } else {
                    "genres"
                }
            }),
            external_ids: EXTERNAL_SOURCES
                .iter()
                .filter(|src| cfg.bool(&format!("musicbrainz.external_ids.{src}")))
                .map(ToString::to_string)
                .collect(),
            pseudo_scripts: if crate::plugins::enabled(cfg, "mbpseudo") {
                cfg.list("mbpseudo.scripts")
            } else {
                vec![]
            },
            custom_tags_only: crate::plugins::enabled(cfg, "mbpseudo")
                && cfg.bool("mbpseudo.custom_tags_only"),
            album_custom_tags: if crate::plugins::enabled(cfg, "mbpseudo") {
                cfg.mapping("mbpseudo.album_custom_tags")
            } else {
                vec![]
            },
            track_custom_tags: if crate::plugins::enabled(cfg, "mbpseudo") {
                cfg.mapping("mbpseudo.track_custom_tags")
            } else {
                vec![]
            },
            data_source: if crate::plugins::enabled(cfg, "mbpseudo") {
                "MusicBrainzPseudoRelease".into()
            } else {
                "MusicBrainz".into()
            },
        }
    }
}

/// The script of a release (or a release relation's release).
fn release_script(v: &J) -> Option<&str> {
    v.get("text-representation")?.get("script")?.as_str()
}

/// beets' mbpseudo `_wanted_pseudo_release_id`/`_intercept_mb_release`: the
/// id of a forward `transl-tracklisting` release relation whose script is
/// one of `scripts`, or None.
pub fn wanted_pseudo_release(release: &J, scripts: &[String]) -> Option<String> {
    if scripts.is_empty() {
        return None;
    }
    if let Some(script) = release_script(release)
        && scripts.iter().any(|s| s == script)
    {
        return None;
    }
    let album_id = release.get("id").and_then(|v| v.as_str())?;
    for rel in relations(release, "release") {
        if rel.get("type").and_then(|v| v.as_str()) != Some("transl-tracklisting")
            || rel.get("direction").and_then(|v| v.as_str()) != Some("forward")
        {
            continue;
        }
        let Some(target) = rel.get("release") else {
            continue;
        };
        if let (Some(id), Some(script)) = (
            target.get("id").and_then(|v| v.as_str()),
            release_script(target),
        ) && scripts.iter().any(|s| s == script)
        {
            crate::logging::debug!(
                "mbpseudo: Adding pseudo-release {id} for main release {album_id}"
            );
            return Some(id.to_string());
        }
    }
    None
}

/// beets' mbpseudo `_add_custom_tags`: the pseudo-release's fields as the
/// official release's custom info fields.
pub fn add_custom_tags(official: &mut AlbumInfo, pseudo: &AlbumInfo, opts: &ParseOpts) {
    let pdata = pseudo.to_data();
    for (tag, pseudo_key) in &opts.album_custom_tags {
        if let Some(v) = pdata.get(pseudo_key) {
            official.extra.push((tag.clone(), v.clone()));
        }
    }
    for (track, pseudo_track) in official.tracks.iter_mut().zip(&pseudo.tracks) {
        let tdata = pseudo_track.to_data();
        for (tag, pseudo_key) in &opts.track_custom_tags {
            if let Some(v) = tdata.get(pseudo_key) {
                track.extra.push((tag.clone(), v.clone()));
            }
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

fn arr<'j>(v: &'j J, key: &str) -> &'j [J] {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// beets' normalised `<target-type>_relations`: an entity's relations to
/// one kind of target, in order.
fn relations<'j>(v: &'j J, target: &str) -> Vec<&'j J> {
    arr(v, "relations")
        .iter()
        .filter(|r| r.get("target-type").and_then(|t| t.as_str()) == Some(target))
        .collect()
}

/// Lucene-escape a search term (beets' `LUCENE_SPECIAL_CHAR_PAT`).
pub(crate) fn escape(term: &str) -> String {
    let mut out = String::new();
    for c in term.chars() {
        if "-+&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// beets' `_get_date`: a partial `YYYY-MM-DD` as numbers, 0 for each
/// missing or unparsable part.
fn parse_date(d: &str) -> (i64, i64, i64) {
    let mut it = d.split('-').map(|p| p.parse::<i64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// beets' `ArtistInfo`, from `_parse_artist_credits`.
#[derive(Debug, Default)]
struct Credits {
    artist: String,
    artist_id: String,
    artist_sort: String,
    artist_credit: String,
    artists: Vec<String>,
    artists_ids: Vec<String>,
    artists_sort: Vec<String>,
    artists_credit: Vec<String>,
}

/// beets' `_parse_artist_credits`: names (or their preferred aliases)
/// joined with the join phrases, and the parallel lists.
fn parse_credits(credits: &[J], prefs: &AliasPrefs) -> Credits {
    let mut c = Credits::default();
    for el in credits {
        let artist = el.get("artist").unwrap_or(&J::Null);
        c.artists_ids.push(s(artist, "id"));
        let alias = prefs.preferred(artist);
        let named = alias.unwrap_or(artist);
        let credited = match alias {
            Some(al) if prefs.as_credits => al,
            _ => el,
        };
        let join = s(el, "joinphrase");
        for (name, joined, multi) in [
            (s(named, "name"), &mut c.artist, &mut c.artists),
            (
                s(named, "sort-name"),
                &mut c.artist_sort,
                &mut c.artists_sort,
            ),
            (
                s(credited, "name"),
                &mut c.artist_credit,
                &mut c.artists_credit,
            ),
        ] {
            joined.push_str(&name);
            joined.push_str(&join);
            multi.push(name);
        }
    }
    c.artist_id = c.artists_ids.first().cloned().unwrap_or_default();
    c
}

/// beets' `_parse_work_relations` and `_parse_artist_relations` into a
/// track.
fn parse_relations(info: &mut TrackInfo, r: &J) {
    let works = relations(r, "work");
    let (mut lyricists, mut lyricists_ids) = (vec![], vec![]);
    let (mut composers, mut composers_ids, mut composer_sort) = (vec![], vec![], vec![]);
    for wr in works.iter().filter(|w| s(w, "type") == "performance") {
        let work = wr.get("work").unwrap_or(&J::Null);
        for ar in relations(work, "artist") {
            let a = ar.get("artist").unwrap_or(&J::Null);
            match s(ar, "type").as_str() {
                "lyricist" => {
                    lyricists.push(s(a, "name"));
                    lyricists_ids.push(s(a, "id"));
                }
                "composer" => {
                    composers.push(s(a, "name"));
                    composers_ids.push(s(a, "id"));
                    composer_sort.push(s(a, "sort-name"));
                }
                _ => {}
            }
        }
    }
    // As beets (a TODO there): the last work, of any relation type.
    if let Some(last) = works.last() {
        let work = last.get("work").unwrap_or(&J::Null);
        info.work = Some(s(work, "title"));
        info.mb_workid = Some(s(work, "id"));
    }
    let some = |v: Vec<String>| (!v.is_empty()).then_some(v);
    info.lyricists = some(lyricists);
    info.lyricists_ids = lyricists_ids;
    info.composers = some(composers);
    info.composers_ids = composers_ids;
    info.composer_sort = (!composer_sort.is_empty()).then(|| composer_sort.join(", "));

    let (mut arrangers, mut arrangers_ids, mut remixers, mut remixers_ids) =
        (vec![], vec![], vec![], vec![]);
    for ar in relations(r, "artist") {
        let a = ar.get("artist").unwrap_or(&J::Null);
        match s(ar, "type").as_str() {
            "arranger" => {
                arrangers.push(s(a, "name"));
                arrangers_ids.push(s(a, "id"));
            }
            "remixer" => {
                remixers.push(s(a, "name"));
                remixers_ids.push(s(a, "id"));
            }
            _ => {}
        }
    }
    info.arrangers = some(arrangers);
    info.arrangers_ids = arrangers_ids;
    info.remixers = some(remixers);
    info.remixers_ids = remixers_ids;
}

/// beets' `track_info`: a recording as a track. Positional fields (index,
/// medium, ...) are None until the release sets them.
pub fn parse_recording(r: &J, opts: &ParseOpts) -> TrackInfo {
    parse_recording_with(r, opts, None, None, None)
}

/// [`parse_recording`] with a release track's title, credits and length
/// in place of the recording's (beets copies the recording and overrides
/// those keys before `track_info`).
fn parse_recording_with(
    r: &J,
    opts: &ParseOpts,
    title: Option<String>,
    credits: Option<&[J]>,
    length: Option<f64>,
) -> TrackInfo {
    let prefs = &opts.aliases;
    let c = parse_credits(credits.unwrap_or(arr(r, "artist-credit")), prefs);
    let isrcs: Vec<&str> = arr(r, "isrcs").iter().filter_map(|i| i.as_str()).collect();
    let mut nulls: BTreeSet<&'static str> = ["disctitle", "media"].into();
    if isrcs.is_empty() {
        nulls.insert("isrc");
    }
    let trackdisambig = s(r, "disambiguation");
    if trackdisambig.is_empty() {
        nulls.insert("trackdisambig");
    }
    let length = length.or_else(|| r.get("length").and_then(J::as_f64));
    let id = s(r, "id");
    let mut info = TrackInfo {
        title: title.unwrap_or_else(|| prefs.title(r)),
        data_url: format!("{BASE_URL}recording/{id}"),
        track_id: id,
        artist: c.artist,
        artist_id: c.artist_id,
        artist_sort: c.artist_sort,
        artist_credit: c.artist_credit,
        artists: c.artists,
        artists_ids: c.artists_ids,
        artists_sort: c.artists_sort,
        artists_credit: c.artists_credit,
        length: length.filter(|l| *l != 0.0).map(|ms| ms / 1000.0),
        isrc: isrcs.join(";"),
        trackdisambig,
        data_source: opts.data_source.clone(),
        nulls,
        ..Default::default()
    };
    parse_relations(&mut info, r);
    info
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
                .filter(|m| !m.is_empty())
                .map(|m| m.split(" (").next().unwrap_or("").to_string())
        })
        .collect();
    if bases.len() == 1 {
        bases.into_iter().next().flatten()
    } else {
        Some("Media".into())
    }
}

/// beets' `get_tracks_from_medium`: the pregap, the tracks and (unless
/// ignored) the data tracks, without "[data track]" placeholders or (with
/// `ignore_video_tracks`) videos.
fn medium_tracks(medium: &J, opts: &ParseOpts) -> Vec<TrackInfo> {
    let mut all: Vec<&J> = vec![];
    if let Some(p) = medium.get("pregap").filter(|p| !p.is_null()) {
        all.push(p);
    }
    all.extend(arr(medium, "tracks"));
    if !opts.ignore_data_tracks {
        all.extend(arr(medium, "data-tracks"));
    }
    let valid: Vec<&J> = all
        .into_iter()
        .filter(|t| {
            let rec = t.get("recording").unwrap_or(&J::Null);
            s(rec, "title") != "[data track]"
                && !(opts.ignore_video_tracks
                    && rec.get("video").and_then(J::as_bool) == Some(true))
        })
        .collect();
    let format = opt_s(medium, "format");
    let disctitle = opt_s(medium, "title");
    let position = medium.get("position").and_then(J::as_u64).unwrap_or(0) as usize;
    let total = valid.len();
    let prefs = &opts.aliases;
    valid
        .iter()
        .map(|t| {
            let rec = t.get("recording").unwrap_or(&J::Null);
            // The release's track title/credit/length win over the
            // recording's, unless the recording has a preferred alias.
            let title =
                Some(s(t, "title")).filter(|t| !t.is_empty() && prefs.preferred(rec).is_none());
            let credits = Some(arr(t, "artist-credit")).filter(|a| !a.is_empty());
            let length = t.get("length").and_then(J::as_f64).filter(|l| *l != 0.0);
            let mut info = parse_recording_with(rec, opts, title, credits, length);
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
                Some(d) => info.disctitle.clone_from(d),
                None => {
                    info.nulls.insert("disctitle");
                }
            }
            match &format {
                Some(f) => info.media.clone_from(f),
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
    let events = arr(j, "release-events");
    for country in preferred {
        for e in events {
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

/// beets' `_parse_genres`: the release group's and the release's genres
/// (or tags), counts summed, most counted first (ties in first-seen order).
fn parse_genres(j: &J, field: &str) -> Option<Vec<String>> {
    let rg = j.get("release-group").unwrap_or(&J::Null);
    let mut counts: Vec<(String, i64)> = vec![];
    for g in arr(rg, field).iter().chain(arr(j, field)) {
        let name = s(g, "name");
        let count = g.get("count").and_then(J::as_i64).unwrap_or(0);
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += count,
            None => counts.push((name, count)),
        }
    }
    if counts.is_empty() {
        return None;
    }
    counts.sort_by_key(|(_, c)| -c);
    Some(counts.into_iter().map(|(n, _)| n).collect())
}

/// beets' `_parse_external_ids`: for each wanted source, the last URL
/// relation on its site (`<source>.com` in the URL), as its id.
fn parse_external_ids(j: &J, wanted: &[String]) -> Vec<(String, Option<String>)> {
    let urls = relations(j, "url");
    let mut out: Vec<(String, Option<String>)> = vec![];
    for source in wanted {
        let site = format!("{source}.com");
        let found = urls
            .iter()
            .rev()
            .filter_map(|r| {
                r.get("url")
                    .and_then(|u| u.get("resource"))
                    .and_then(|u| u.as_str())
            })
            .find(|u| u.contains(&site));
        if let Some(url) = found {
            crate::logging::debug!(
                "Found link to {} release via MusicBrainz",
                capitalize(source)
            );
            out.push((
                format!("{source}_album_id"),
                crate::metadata_plugins::extract_id(source, url),
            ));
        }
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f
            .to_uppercase()
            .chain(c.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// beets' `album_info` for a release.
pub fn parse_release(j: &J, opts: &ParseOpts) -> AlbumInfo {
    let prefs = &opts.aliases;
    let c = parse_credits(arr(j, "artist-credit"), prefs);
    let rg = j.get("release-group").unwrap_or(&J::Null);
    let (oy, om, od) = parse_date(&s(rg, "first-release-date"));
    let mut nulls = BTreeSet::new();
    let mut null_if = |name: &'static str, v: Option<String>| match v {
        Some(v) => v,
        None => {
            nulls.insert(name);
            String::new()
        }
    };
    // beets' `_parse_label_infos`: the first label info.
    let label_info = arr(j, "label-info").first();
    let label = label_info
        .and_then(|li| li.get("label"))
        .and_then(|l| opt_s(l, "name"))
        .filter(|n| n != "[no label]");
    let catalognum = label_info.and_then(|li| opt_s(li, "catalog-number"));
    let label = null_if("label", label);
    let catalognum = null_if("catalognum", catalognum);

    let media_list = arr(j, "media");
    let mediums = media_list.len();
    let mut tracks = vec![];
    for medium in media_list {
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
        opt_s(rg, "disambiguation").filter(|d| !d.is_empty()),
    );
    // beets' `_parse_release_group`: the primary type, then the secondary
    // ones, lower-cased.
    let primary = opt_s(rg, "primary-type")
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase());
    let mut albumtypes: Vec<String> = primary.iter().cloned().collect();
    albumtypes.extend(
        arr(rg, "secondary-types")
            .iter()
            .filter_map(|t| t.as_str())
            .map(str::to_lowercase),
    );
    let albumtype = null_if("albumtype", primary);
    let albumstatus = null_if("albumstatus", opt_s(j, "status"));
    let text = j.get("text-representation").unwrap_or(&J::Null);
    let script = null_if("script", opt_s(text, "script"));
    let language = null_if("language", opt_s(text, "language"));
    let barcode = null_if("barcode", opt_s(j, "barcode"));
    let asin = null_if("asin", opt_s(j, "asin"));
    let id = s(j, "id");
    let mut info = AlbumInfo {
        data_url: format!("{BASE_URL}release/{id}"),
        album_id: id,
        album: prefs.title(j),
        va: c.artist_id == VA_ARTIST_ID,
        artist: c.artist,
        artist_id: c.artist_id,
        artist_sort: c.artist_sort,
        artist_credit: c.artist_credit,
        artists: c.artists,
        artists_ids: c.artists_ids,
        artists_sort: c.artists_sort,
        artists_credit: c.artists_credit,
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
        albumtypes,
        albumstatus,
        albumdisambig,
        releasegroup_id: s(rg, "id"),
        release_group_title: Some(prefs.title(rg)),
        releasegroupdisambig,
        media,
        mediums,
        tracks,
        barcode,
        asin,
        script,
        language,
        genres: opts.genres_field.and_then(|f| parse_genres(j, f)),
        external_ids: parse_external_ids(j, &opts.external_ids),
        data_source: opts.data_source.clone(),
        nulls,
        omit: Default::default(),
        extra: vec![],
    };
    if info.va {
        let va = &opts.va_name;
        info.artist.clone_from(va);
        info.artist_sort.clone_from(va);
        info.artists = vec![va.clone()];
        info.artists_sort = vec![va.clone()];
        info.artist_credit.clone_from(va);
        info.artists_credit = vec![va.clone()];
    }
    info
}

/// beets' `_merge_pseudo_and_actual_album`: a pseudo-release (a
/// translated track listing) with the release facts of the actual one.
fn merge_pseudo_and_actual(mut pseudo: AlbumInfo, actual: AlbumInfo) -> AlbumInfo {
    for f in ["media", "country", "catalognum", "label", "barcode", "asin"] {
        pseudo.nulls.remove(f);
        if actual.nulls.contains(f) {
            pseudo.nulls.insert(f);
        }
    }
    pseudo.media = actual.media;
    pseudo.mediums = actual.mediums;
    pseudo.country = actual.country;
    pseudo.catalognum = actual.catalognum;
    pseudo.year = actual.year;
    pseudo.month = actual.month;
    pseudo.day = actual.day;
    pseudo.original_year = actual.original_year;
    pseudo.original_month = actual.original_month;
    pseudo.original_day = actual.original_day;
    pseudo.label = actual.label;
    pseudo.barcode = actual.barcode;
    pseudo.asin = actual.asin;
    pseudo.genres = actual.genres;
    pseudo
}

/// Pull an MBID out of a bare id or a URL (beets' `extract_release_id`
/// for MusicBrainz: as given, not lower-cased).
pub fn extract_id(s: &str) -> Option<String> {
    crate::metadata_plugins::extract_id("musicbrainz", s)
}

/// beets' `plurality`: the most common value, ties to the first seen.
fn plurality(values: impl IntoIterator<Item = String>) -> String {
    let mut counts: Vec<(String, usize)> = vec![];
    for v in values {
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some((_, c)) => *c += 1,
            None => counts.push((v, 1)),
        }
    }
    let mut best: Option<(String, usize)> = None;
    for (v, c) in counts {
        if best.as_ref().is_none_or(|(_, bc)| c > *bc) {
            best = Some((v, c));
        }
    }
    best.map(|(v, _)| v).unwrap_or_default()
}

/// Python's `str()` of an item's field, as beets' `item.get(tag)`.
fn item_value_str(item: &Item, field: &str) -> String {
    match item.get(field) {
        Some(crate::model::Value::Float(f)) => crate::hooks::py_float(f),
        Some(crate::model::Value::Null) | None => "None".into(),
        Some(v) => v.raw(),
    }
}

/// beets' search query: each filter as `field:(term)`, the term
/// lower-cased, stripped and escaped (empty ones left out), joined by
/// spaces.
pub(crate) fn search_query(filters: &[(&str, String)]) -> String {
    filters
        .iter()
        .filter_map(|(field, term)| {
            let term = term.to_lowercase();
            let term = term.trim();
            (!term.is_empty()).then(|| format!("{field}:({})", escape(term)))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What beets' plugin says when it loads: the deprecated
/// `searchlimit` (which beets 2.14 reads from `musicbrainz.match`). Once
/// per run.
pub fn warn_deprecated_config(cfg: &Config) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if cfg.get("musicbrainz.match.searchlimit").is_some() {
        ONCE.call_once(|| {
            crate::logging::emit(
                "musicbrainz: 'musicbrainz.searchlimit' configuration option is deprecated \
                 and will be removed in version 3.0.0. Use 'musicbrainz.search_limit' instead."
                    .to_string(),
            );
        });
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
    /// `musicbrainz.extra_tags`: (item field, search field), deduplicated.
    extra_tags: Vec<(&'static str, &'static str)>,
    opts: ParseOpts,
}

/// Not found (404), which beets treats as no candidate rather than an error.
#[derive(Debug)]
pub struct NotFound;

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not found on MusicBrainz")
    }
}

impl std::error::Error for NotFound {}

impl MusicBrainz {
    pub fn new(cfg: &Config) -> Self {
        // beets' `MusicBrainzAPI`: musicbrainz.org always over https at one
        // request a second; any other host per `https` and the rate limit.
        let host = cfg.str("musicbrainz.host");
        let (base, interval) = if host == "musicbrainz.org" {
            (
                "https://musicbrainz.org".to_string(),
                Duration::from_secs(1),
            )
        } else {
            let scheme = if cfg.bool("musicbrainz.https") {
                "https"
            } else {
                "http"
            };
            let rate = cfg.f64("musicbrainz.ratelimit").max(0.0);
            let per = cfg.f64("musicbrainz.ratelimit_interval").max(0.0);
            let interval = if rate > 0.0 {
                Duration::from_secs_f64(per / rate)
            } else {
                Duration::ZERO
            };
            (format!("{scheme}://{host}"), interval)
        };
        // `search_limit`, or the deprecated `match.searchlimit` (read, as
        // beets 2.14 does, under the plugin's own config).
        let search_limit = if cfg.get("musicbrainz.match.searchlimit").is_some() {
            cfg.f64("musicbrainz.match.searchlimit")
        } else {
            cfg.f64("musicbrainz.search_limit")
        };
        let mut extra_tags: Vec<(&'static str, &'static str)> = vec![];
        for t in cfg.list("musicbrainz.extra_tags") {
            if let Some(&(tag, key)) = FIELDS_TO_MB_KEYS.iter().find(|(f, _)| *f == t)
                && !extra_tags.iter().any(|(f, _)| *f == tag)
            {
                extra_tags.push((tag, key));
            }
        }
        if !extra_tags.is_empty() {
            let shown: Vec<String> = extra_tags
                .iter()
                .map(|(t, k)| format!("'{t}': '{k}'"))
                .collect();
            crate::logging::debug!(
                "musicbrainz: Additional search terms: {{{}}}",
                shown.join(", ")
            );
        }
        MusicBrainz {
            agent: ureq::AgentBuilder::new()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build(),
            base: format!("{base}/ws/2"),
            interval,
            last: Mutex::new(None),
            search_limit: search_limit.max(0.0) as usize,
            extra_tags,
            opts: ParseOpts::from_config(cfg),
        }
    }

    fn throttle(&self) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            let mut req = self.agent.get(&url);
            for (k, v) in query {
                req = req.query(k, v);
            }
            req = req.query("fmt", "json");
            match req.call() {
                Ok(resp) => return resp.into_json().context("parsing MusicBrainz response"),
                Err(ureq::Error::Status(503 | 429, _)) if attempt < 4 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(attempt));
                }
                Err(ureq::Error::Status(404, _)) => return Err(NotFound.into()),
                Err(e) => return Err(e).with_context(|| format!("MusicBrainz request {path}")),
            }
        }
    }

    /// beets' `MusicBrainzAPI.get_work`: a work lookup with its includes
    /// (the JSON keeps MusicBrainz' hyphenated keys).
    pub fn get_work(&self, id: &str, includes: &[&str]) -> Result<J> {
        let inc = includes.join("+");
        self.get(&format!("work/{id}"), &[("inc", &inc)])
    }

    /// beets' `MusicBrainzAPI.search`: each filter as `field:(term)`, the
    /// term lower-cased, stripped and escaped (empty ones left out),
    /// joined by spaces. Returns the results' ids.
    fn search(&self, entity: &str, filters: &[(&str, String)]) -> Result<Vec<String>> {
        let query = search_query(filters);
        if query.is_empty() {
            crate::logging::debug!("musicbrainz: Skipping search with empty query and filters");
            return Ok(vec![]);
        }
        crate::logging::debug!(
            "Searching for MusicBrainz {entity}s with: {}",
            crate::hooks::py_repr(&query)
        );
        let limit = self.search_limit.to_string();
        let res = self.get(entity, &[("query", &query), ("limit", &limit)])?;
        let ids: Vec<String> = arr(&res, &format!("{entity}s"))
            .iter()
            .map(|r| s(r, "id"))
            .collect();
        crate::logging::debug!("musicbrainz: Found {} result(s)", ids.len());
        Ok(ids)
    }

    /// beets' `get_album_criteria` and `candidates`: search releases by
    /// album and artist (the Various Artists id when the album is likely
    /// VA) and `extra_tags`; returns their ids.
    pub fn search_releases(
        &self,
        items: &[Item],
        artist: &str,
        album: &str,
        va_likely: bool,
    ) -> Result<Vec<String>> {
        let mut criteria: Vec<(&str, String)> = vec![("release", album.to_string())];
        if va_likely {
            criteria.push(("arid", VA_ARTIST_ID.into()));
        } else {
            criteria.push(("artist", artist.to_string()));
        }
        for &(tag, key) in &self.extra_tags {
            let value = match tag {
                "tracks" => items.len().to_string(),
                "alias" => album.to_string(),
                _ => {
                    let v = plurality(items.iter().map(|i| item_value_str(i, tag)));
                    if tag == "catalognum" {
                        v.replace(' ', "")
                    } else {
                        v
                    }
                }
            };
            match criteria.iter_mut().find(|(k, _)| *k == key) {
                Some(c) => c.1 = value,
                None => criteria.push((key, value)),
            }
        }
        self.search("release", &criteria)
    }

    /// beets' `album_for_id`: the release, None if it's not on
    /// MusicBrainz or the id isn't an MBID; a pseudo-release gets the
    /// release facts of the release it translates.
    pub fn album_for_id(&self, album_id: &str) -> Result<Option<AlbumInfo>> {
        Ok(self.album_for_id_variants(album_id)?.into_iter().next())
    }

    /// The candidates a release id yields: the release itself, or (mbpseudo)
    /// the translated pseudo-release first and the official one after it.
    pub fn album_for_id_variants(&self, album_id: &str) -> Result<Vec<AlbumInfo>> {
        crate::logging::debug!("musicbrainz: Requesting MusicBrainz release {album_id}");
        let Some(id) = extract_id(album_id) else {
            crate::logging::debug!("musicbrainz: Invalid MBID ({album_id}).");
            return Ok(vec![]);
        };
        let release = match self.get_release(&id) {
            Ok(r) => r,
            Err(e) if e.is::<NotFound>() => {
                crate::logging::debug!("musicbrainz: Release {id} not found on MusicBrainz.");
                return Ok(vec![]);
            }
            Err(e) => return Err(e),
        };
        let album = parse_release(&release, &self.opts);
        if s(&release, "status") == "Pseudo-Release"
            && let Some(rel) = relations(&release, "release")
                .into_iter()
                .find(|r| s(r, "type") == "transl-tracklisting" && s(r, "direction") == "backward")
        {
            let actual_id = rel.get("release").map(|r| s(r, "id")).unwrap_or_default();
            let actual = parse_release(&self.get_release(&actual_id)?, &self.opts);
            let mut info = merge_pseudo_and_actual(album, actual);
            for l in info_listeners() {
                l.album_info(&mut info);
            }
            return Ok(vec![info]);
        }
        // mbpseudo: an official release whose translation has a wanted
        // script also offers the translated (pseudo) track listing.
        if !self.opts.pseudo_scripts.is_empty()
            && let Some(pseudo_id) = wanted_pseudo_release(&release, &self.opts.pseudo_scripts)
        {
            let pseudo = parse_release(&self.get_release(&pseudo_id)?, &self.opts);
            if self.opts.custom_tags_only {
                let mut official = album;
                add_custom_tags(&mut official, &pseudo, &self.opts);
                return Ok(vec![official]);
            }
            let merged = merge_pseudo_and_actual(pseudo, album.clone());
            let mut infos = vec![merged, album];
            for info in &mut infos {
                for l in info_listeners() {
                    l.album_info(info);
                }
            }
            return Ok(infos);
        }
        let mut infos = vec![album];
        for info in &mut infos {
            for l in info_listeners() {
                l.album_info(info);
            }
        }
        Ok(infos)
    }

    /// A release lookup with beets' includes; a release with too many
    /// tracks for MusicBrainz to send their relations gets its recordings
    /// browsed (beets' `_ensure_complete_recordings`).
    fn get_release(&self, id: &str) -> Result<J> {
        let inc = RELEASE_INCLUDES.join("+");
        let mut release = self.get(&format!("release/{id}"), &[("inc", &inc)])?;
        let track_count: usize = arr(&release, "media")
            .iter()
            .map(|m| arr(m, "tracks").len())
            .sum();
        if track_count > BROWSE_MAXTRACKS {
            crate::logging::debug!("musicbrainz: Album {id} has too many tracks");
            let inc = BROWSE_INCLUDES.join("+");
            let mut recordings: Vec<J> = vec![];
            for offset in (0..track_count).step_by(BROWSE_CHUNKSIZE) {
                crate::logging::debug!("musicbrainz: Retrieving tracks starting at {offset}");
                let res = self.get(
                    "recording",
                    &[
                        ("release", id),
                        ("limit", &BROWSE_CHUNKSIZE.to_string()),
                        ("inc", &inc),
                        ("offset", &offset.to_string()),
                    ],
                )?;
                recordings.extend(arr(&res, "recordings").iter().cloned());
            }
            if let Some(media) = release.get_mut("media").and_then(|m| m.as_array_mut()) {
                for medium in media {
                    let Some(tracks) = medium.get_mut("tracks").and_then(|t| t.as_array_mut())
                    else {
                        continue;
                    };
                    for t in tracks {
                        let rid = t.get("recording").map(|r| s(r, "id")).unwrap_or_default();
                        if let Some(r) = recordings.iter().find(|r| s(r, "id") == rid) {
                            t["recording"] = r.clone();
                        }
                    }
                }
            }
        }
        Ok(release)
    }

    /// beets' `item_candidates`: search recordings by artist and title
    /// (and title as an alias), then look each one up.
    pub fn search_recordings(&self, artist: &str, title: &str) -> Result<Vec<TrackInfo>> {
        let criteria = [
            ("artist", artist.to_string()),
            ("recording", title.to_string()),
            ("alias", title.to_string()),
        ];
        let mut out = vec![];
        for id in self.search("recording", &criteria)? {
            if let Some(t) = self.track_for_id(&id)? {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// beets' `MusicBrainzAPI.browse_release_groups(artist=..., type=...)`:
    /// the artist's release groups of those types (`|`-separated; empty
    /// sends an empty `type`), as MusicBrainz returns them (one page).
    pub fn browse_release_groups(&self, artist: &str, types: &str) -> Result<Vec<J>> {
        let res = self.get("release-group", &[("artist", artist), ("type", types)])?;
        Ok(arr(&res, "release-groups").to_vec())
    }

    /// beets' `track_for_id`: the recording, None if not found.
    pub fn track_for_id(&self, track_id: &str) -> Result<Option<TrackInfo>> {
        let mut info = self.track_for_id_raw(track_id)?;
        if let Some(info) = info.as_mut() {
            for l in info_listeners() {
                l.track_info(info);
            }
        }
        Ok(info)
    }

    fn track_for_id_raw(&self, track_id: &str) -> Result<Option<TrackInfo>> {
        let Some(id) = extract_id(track_id) else {
            crate::logging::debug!("musicbrainz: Invalid MBID ({track_id}).");
            return Ok(None);
        };
        let inc = RECORDING_INCLUDES.join("+");
        match self.get(&format!("recording/{id}"), &[("inc", &inc)]) {
            Ok(j) => Ok(Some(parse_recording(&j, &self.opts))),
            Err(e) if e.is::<NotFound>() => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ------------------------------------------------- info received events

/// A listener for beets' `albuminfo_received` and `trackinfo_received`
/// events: each release or recording fetched from MusicBrainz (for a
/// search, an id lookup or the `missing` plugin), before it's matched.
pub trait InfoListener: Send + Sync {
    fn album_info(&self, _info: &mut AlbumInfo) {}
    fn track_info(&self, _info: &mut TrackInfo) {}
}

type InfoListeners = Vec<std::sync::Arc<dyn InfoListener>>;

#[cfg(not(test))]
static INFO_LISTENERS: std::sync::RwLock<InfoListeners> = std::sync::RwLock::new(Vec::new());

// Tests run in parallel: each test thread has its own listeners.
#[cfg(test)]
thread_local! {
    static INFO_LISTENERS: std::cell::RefCell<InfoListeners> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Register an info listener (a plugin, when it loads).
pub fn add_info_listener(l: std::sync::Arc<dyn InfoListener>) {
    #[cfg(not(test))]
    INFO_LISTENERS.write().unwrap().push(l);
    #[cfg(test)]
    INFO_LISTENERS.with(|v| v.borrow_mut().push(l));
}

fn info_listeners() -> InfoListeners {
    #[cfg(not(test))]
    return INFO_LISTENERS.read().unwrap().clone();
    #[cfg(test)]
    INFO_LISTENERS.with(|v| v.borrow().clone())
}

#[cfg(test)]
pub fn clear_info_listeners() {
    INFO_LISTENERS.with(|v| v.borrow_mut().clear());
}

#[cfg(test)]
pub fn info_listener_count() -> usize {
    INFO_LISTENERS.with(|v| v.borrow().len())
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
                languages: langs.iter().map(ToString::to_string).collect(),
                ignored_types: ignored.iter().map(ToString::to_string).collect(),
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
                    preferred_countries: c.iter().map(ToString::to_string).collect(),
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
            Some("0FA1E0C7-1C4A-4F2B-9A5E-ABCDEF012345")
        );
        assert_eq!(extract_id("nope"), None);
        assert_eq!(escape("AC/DC: (live)"), "AC\\/DC\\: \\(live\\)");
    }

    fn to_json(v: &V) -> J {
        match v {
            V::None => J::Null,
            V::Str(s) => J::from(s.as_str()),
            V::Int(i) => J::from(*i),
            V::Float(f) => J::from(*f),
            V::Bool(b) => J::from(*b),
            V::List(l) => J::from(l.clone()),
        }
    }

    /// Field-by-field differences between ours and reference beets'.
    fn diffs(what: &str, ours: &Data, theirs: &J, out: &mut Vec<String>) {
        let theirs = theirs.as_object().unwrap();
        for (k, v) in theirs {
            let o = ours.get(k).map(to_json);
            if o.as_ref() != Some(v) {
                out.push(format!("{what}.{k}: ours {o:?}, beets {v}"));
            }
        }
        for k in ours.keys().filter(|k| !theirs.contains_key(*k)) {
            out.push(format!("{what}.{k}: ours {:?}, beets has none", ours[k]));
        }
    }

    /// Every fixture release under every config of
    /// tools/diffbeets/mbexpected.py, parsed and applied as reference
    /// beets 2.14 does: `dict(album_info)`, each `dict(track_info)`, the
    /// album's `item_data` and each track's `merge_with_album`.
    #[test]
    fn fixtures_parse_and_apply_as_beets() {
        use crate::hooks::{self, ApplyOpts};
        let dir = format!("{}/tests/fixtures/mb", env!("CARGO_MANIFEST_DIR"));
        let mut checked = 0;
        let mut out = vec![];
        for entry in std::fs::read_dir(format!("{dir}/expected")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let id = name.split('.').next().unwrap();
            let want: J = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let tmp = tempfile::tempdir().unwrap();
            let file = tmp.path().join("config.yaml");
            std::fs::write(&file, want["config"].as_str().unwrap()).unwrap();
            let cfg = Config::load_file(&file).unwrap();
            let release: J = serde_json::from_str(
                &std::fs::read_to_string(format!("{dir}/release-{id}.json")).unwrap(),
            )
            .unwrap();
            let info = parse_release(&release, &ParseOpts::from_config(&cfg));
            let o = ApplyOpts::from_config(&cfg);
            diffs(
                &format!("{name} album"),
                &info.to_data(),
                &want["album"],
                &mut out,
            );
            diffs(
                &format!("{name} album_item_data"),
                &hooks::album_item_data(&info.to_data(), &o),
                &want["album_item_data"],
                &mut out,
            );
            let (tracks, merged) = (
                want["tracks"].as_array().unwrap(),
                want["merged"].as_array().unwrap(),
            );
            assert_eq!(info.tracks.len(), tracks.len(), "{name}: tracks");
            for (n, t) in info.tracks.iter().enumerate() {
                diffs(
                    &format!("{name} track {n}"),
                    &t.to_data(),
                    &tracks[n],
                    &mut out,
                );
                diffs(
                    &format!("{name} merged {n}"),
                    &hooks::merge_with_album(t, &info, &o),
                    &merged[n],
                    &mut out,
                );
            }
            checked += 1;
        }
        assert!(checked >= 12, "expected files: {checked}");
        assert!(
            out.is_empty(),
            "differences from beets:\n{}",
            out.join("\n")
        );
    }

    #[test]
    fn search_filters_as_beets() {
        // beets' `format_search_term`: lower-cased, stripped, escaped.
        let q = search_query(&[
            ("release", " AC/DC: Live ".into()),
            ("arid", VA_ARTIST_ID.into()),
            ("catno", "".into()),
        ]);
        assert_eq!(
            q,
            "release:(ac\\/dc\\: live) arid:(89ad4ac3\\-39f7\\-470e\\-963a\\-56509c546377)"
        );
        let mut a = crate::model::Record::new_item();
        a.set_str("catalognum", "SMP 001");
        let b = a.clone();
        let mut c = a.clone();
        c.set_str("catalognum", "X");
        assert_eq!(
            plurality([&a, &b, &c].map(|i| item_value_str(i, "catalognum"))),
            "SMP 001"
        );
        assert_eq!(item_value_str(&a, "year"), "0");
        assert_eq!(plurality(["b".to_string(), "a".into()]), "b");
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "musicbrainz",
        order: 17,
        defaults: r#"musicbrainz:
    host: musicbrainz.org
    https: no
    ratelimit: 1
    ratelimit_interval: 1
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    search_query_ascii: no
    genres: no
    genres_tag: genre
    external_ids:
        discogs: no
        bandcamp: no
        spotify: no
        deezer: no
        tidal: no
    extra_tags: []
    aliases_as_credits: no
"#,
        config_keys: &["musicbrainz"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: None,
        fatal: false,
        commands: &[],
    }
}
