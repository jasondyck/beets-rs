//! A port of beets 2.14's `discogs` plugin (`beetsplug/discogs/`): release
//! search on the Discogs API as autotag candidates, lookups by Discogs
//! release id or URL, artist credits with name variations (ANV), the
//! tracklist's medium/side/subtrack handling, and index/heading tracks.
//!
//! beets uses the `discogs_client` library; this is a small client for the
//! same API (`/database/search`, `/releases/<id>`, `/masters/<id>`), with
//! the plugin's OAuth/token login. The API's base URL is the plugin's,
//! except in tests (which build the plugin with [`Discogs::with`] and a
//! fake server).

use super::oauth::{OAuth1, parse_qsl};
use super::requests::{HttpError, Request, Session, urlencode};
use super::{PluginLog, USER_AGENT};
use crate::config::Config;
use crate::hooks::V;
use crate::mb::{AlbumInfo, TrackInfo};
use crate::metadata_plugins::{MetadataSource, SourceCtx, extract_id, opt_str};
use crate::model::Item;
use anyhow::{Result, anyhow, bail};
use serde_json::Value as J;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

/// `MEDIA_FORMAT_ALIASES`.
const MEDIA_FORMAT_ALIASES: [&str; 2] = ["digital media", "web"];

/// `FIELDS_TO_DISCOGS_KEYS`.
const FIELDS_TO_DISCOGS_KEYS: [(&str, &str); 5] = [
    ("barcode", "barcode"),
    ("catalognum", "catno"),
    ("country", "country"),
    ("label", "label"),
    ("media", "format"),
];

/// The search's field filters can also use the release's year.
const FIELDS_TO_DISCOGS_KEYS_YEAR: (&str, &str) = ("year", "year");

// ---------------------------------------------------------------- errors

/// A client failure: `DiscogsAPIError` (a status code) or a malformed
/// response (`MalformedResponseError`, retried once in searches).
#[derive(Debug)]
pub enum DiscogsError {
    Status(u16, String),
    /// `MalformedResponseError`: the body wasn't JSON.
    Malformed(String),
    /// Connection problems (a `RequestException`).
    Request(String),
}

impl std::fmt::Display for DiscogsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscogsError::Status(code, url) => write!(f, "({code}, {url:?})"),
            DiscogsError::Malformed(m) | DiscogsError::Request(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DiscogsError {}

// ---------------------------------------------------------------- artists

/// A Discogs artist credit (`Artist` in the plugin's types).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub anv: String,
    pub join: String,
    pub role: String,
}

impl Artist {
    pub fn from_data(data: &J) -> Self {
        let s = |k: &str| {
            data.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Artist {
            id: match data.get("id") {
                Some(J::Number(n)) => n
                    .as_i64()
                    .map_or_else(|| n.to_string(), |i| i.to_string()),
                Some(J::String(s)) => s.clone(),
                _ => String::new(),
            },
            name: s("name"),
            anv: s("anv"),
            join: s("join"),
            role: s("role"),
        }
    }
}

/// `ArtistState.ValidArtist`: a normalized, render-ready artist.
#[derive(Debug, Clone, Default)]
pub struct ValidArtist {
    pub id: String,
    pub name: String,
    pub credit: String,
    pub join: String,
    pub role: String,
}

impl ValidArtist {
    /// `get_artist`: the display field with its trailing join token.
    fn get_artist(&self, property: &str) -> String {
        let value = match property {
            "credit" => &self.credit,
            _ => &self.name,
        };
        let join = match self.join.as_str() {
            "," => ", ".to_string(),
            "" => String::new(),
            j => format!(" {j} "),
        };
        format!("{value}{join}")
    }
}

/// `ArtistInfo`: what an `ArtistState` contributes to an info.
#[derive(Debug, Clone, Default)]
pub struct ArtistInfo {
    pub artist: String,
    pub artists: Vec<String>,
    pub artist_credit: String,
    pub artists_credit: Vec<String>,
    pub artist_id: String,
    pub artists_ids: Vec<String>,
    pub arrangers: Option<Vec<String>>,
    pub composers: Option<Vec<String>>,
    pub remixers: Option<Vec<String>>,
    pub lyricists: Option<Vec<String>>,
}

/// `ArtistState`: the plugin's policy for artist names, ANVs, "Various" and
/// featured artists.
#[derive(Debug, Clone, Default)]
pub struct ArtistState {
    pub raw_artists: Vec<Artist>,
    pub use_anv: bool,
    pub use_credit_anv: bool,
    pub featured_string: String,
    pub should_strip_disambiguation: bool,
    pub va_name: String,
}

/// `DISAMBIGUATION_RE = re.compile(r" \(\d+\)")`.
fn strip_disambiguation_re(text: &str) -> String {
    let re = regex::Regex::new(r" \(\d+\)").unwrap();
    re.replace_all(text, "").into_owned()
}

impl ArtistState {
    pub fn strip_disambiguation(&self, text: &str) -> String {
        if self.should_strip_disambiguation {
            strip_disambiguation_re(text)
        } else {
            text.to_string()
        }
    }

    /// `valid_artists`: normalized, filtered entries.
    pub fn valid_artists(&self) -> Vec<ValidArtist> {
        let mut out = vec![];
        for a in &self.raw_artists {
            let name = if a.name == "Various" {
                self.va_name.clone()
            } else {
                a.name.clone()
            };
            if name.is_empty() {
                continue;
            }
            let anv = if a.anv.is_empty() {
                name.clone()
            } else {
                a.anv.clone()
            };
            if anv.is_empty() {
                continue;
            }
            out.push(ValidArtist {
                id: a.id.clone(),
                name: self.strip_disambiguation(if self.use_anv { &anv } else { &name }),
                credit: self
                    .strip_disambiguation(if self.use_credit_anv { &anv } else { &name }),
                join: a.join.trim().to_string(),
                role: a.role.to_lowercase(),
            });
        }
        out
    }

    /// `main_artists`: unique by id, in order.
    pub fn main_artists(&self) -> Vec<ValidArtist> {
        let mut out: Vec<ValidArtist> = vec![];
        for a in self.valid_artists() {
            if (a.role.is_empty() || a.role.contains("featuring"))
                && !out.iter().any(|x| x.id == a.id) {
                    out.push(a);
                }
        }
        out
    }

    fn roles_containing(&self, needle: &str) -> Option<Vec<String>> {
        let names: Vec<String> = self
            .valid_artists()
            .into_iter()
            .filter(|a| a.role.contains(needle))
            .map(|a| a.name)
            .collect();
        (!names.is_empty()).then_some(names)
    }

    /// `join_artists`: the joined string, featured artists after the
    /// featured marker.
    pub fn join_artists(&self, property: &str) -> String {
        let main = self.main_artists();
        let non_featured: Vec<&ValidArtist> =
            main.iter().filter(|a| a.role.is_empty()).collect();
        let featured: Vec<&ValidArtist> = main
            .iter()
            .filter(|a| a.role.contains("featuring"))
            .collect();
        let mut artist: String = non_featured
            .iter()
            .map(|a| a.get_artist(property))
            .collect();
        if !featured.is_empty() {
            if !artist.contains("feat") {
                artist.push_str(&format!(" {} ", self.featured_string));
            }
            artist.push_str(
                &featured
                    .iter()
                    .map(|a| a.get_artist(property))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        artist
    }

    /// `info`: the values an info takes from this state.
    pub fn info(&self) -> ArtistInfo {
        let main = self.main_artists();
        ArtistInfo {
            artist: self.join_artists("name"),
            artists: main.iter().map(|a| a.name.clone()).collect(),
            artist_credit: self.join_artists("credit"),
            artists_credit: main.iter().map(|a| a.credit.clone()).collect(),
            artist_id: main.first().map(|a| a.id.clone()).unwrap_or_default(),
            artists_ids: main.iter().map(|a| a.id.clone()).collect(),
            arrangers: self.roles_containing("arrang"),
            composers: self.roles_containing("written-by"),
            remixers: self.roles_containing("remix"),
            lyricists: self.roles_containing("lyrics"),
        }
    }
}

// -------------------------------------------------------------- tracklists

/// A tracklist entry's `type_`.
fn track_type(track: &J) -> &str {
    track.get("type_").and_then(|t| t.as_str()).unwrap_or_default()
}

/// `get_track_index`: the medium, medium index and subtrack index of a
/// Discogs position ("1", "1-1", "A1", "A1.1", "A1a", "IV", ...).
pub fn get_track_index(position: &str) -> (Option<String>, Option<String>, Option<String>) {
    let p = position.to_uppercase();
    let b = p.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    // The regex is `(.*?)(\d*?)((?<=\w)\.[\w]+|(?<=\d)[A-Z]+)?` with
    // `fullmatch`: the shortest medium and digit run that leaves a whole
    // subtrack suffix (or nothing).
    for i in 0..=b.len() {
        // `medium_index` is a run of digits (possibly empty).
        let mut j = i;
        loop {
            let suffix = &p[j..];
            let alt1 = suffix.starts_with('.')
                && suffix.len() > 1
                && suffix[1..].bytes().all(is_word)
                && j >= 1
                && is_word(b[j - 1]);
            if alt1 {
                return (
                    group(&p[..i]),
                    group(&p[i..j]),
                    group(&suffix[1..]),
                );
            }
            let alt2 = !suffix.is_empty()
                && suffix.bytes().all(|c| c.is_ascii_uppercase())
                && j >= 1
                && b[j - 1].is_ascii_digit();
            if alt2 {
                return (group(&p[..i]), group(&p[i..j]), group(suffix));
            }
            if suffix.is_empty() {
                return (group(&p[..i]), group(&p[i..j]), None);
            }
            if j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            } else {
                break;
            }
        }
    }
    (None, None, None)
}

fn group(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// `get_track_length`: "M:SS" seconds, or None.
fn get_track_length(duration: &str) -> Option<f64> {
    let (m, s) = duration.split_once(':')?;
    let m: u32 = m.trim().parse().ok()?;
    let s: u32 = s.trim().parse().ok()?;
    if m > 59 || s > 60 {
        // `time.strptime` refuses out-of-range values.
        return None;
    }
    Some(f64::from(m * 60 + s))
}

/// `parse_release_date`: the year/month/day a release was issued on (the
/// release's own `year` fills in what `released` doesn't say).
pub fn parse_release_date(released: Option<&str>, year: Option<i64>) -> (Option<i64>, Option<i64>, Option<i64>) {
    let re = regex::Regex::new(r"^(\d{4})(?:-(\d{1,2})(?:-(\d{1,2}))?)?$").unwrap();
    let Some(caps) = released.map(str::trim).and_then(|r| re.captures(r)) else {
        return (year, None, None);
    };
    let num = |i: usize| {
        caps.get(i)
            .and_then(|m| m.as_str().parse::<i64>().ok())
            .unwrap_or(0)
    };
    let released_year = num(1);
    let month = num(2);
    let day = num(3);
    (
        if released_year != 0 {
            Some(released_year)
        } else {
            year
        },
        (month != 0).then_some(month),
        (day != 0).then_some(day),
    )
}

/// `TracklistState`: the tracks built from a tracklist, with each entry's
/// medium and medium index.
#[derive(Debug, Default)]
pub struct TracklistState {
    pub index: usize,
    pub index_tracks: HashMap<usize, String>,
    pub tracks: Vec<TrackInfo>,
    pub divisions: Vec<String>,
    pub next_divisions: Vec<String>,
    pub mediums: Vec<Option<String>>,
    pub medium_indices: Vec<Option<String>>,
}

// ------------------------------------------------------------------ client

/// A Discogs API client (`discogs_client.Client`): token or OAuth1 auth.
pub struct DiscogsClient {
    pub base: String,
    pub token: Option<String>,
    pub oauth: Option<OAuth1>,
    session: Session,
}

impl DiscogsClient {
    pub fn with_token(base: &str, user_agent: &str, token: &str) -> Self {
        DiscogsClient {
            base: base.to_string(),
            token: Some(token.to_string()),
            oauth: None,
            session: Session::plain(user_agent),
        }
    }

    pub fn with_oauth(base: &str, user_agent: &str, oauth: OAuth1) -> Self {
        DiscogsClient {
            base: base.to_string(),
            token: None,
            oauth: Some(oauth),
            session: Session::plain(user_agent),
        }
    }

    fn url(&self, endpoint: &str) -> String {
        format!("{}{endpoint}", self.base)
    }

    fn get(&self, endpoint: &str, params: &[(&str, String)]) -> Result<J, DiscogsError> {
        let url = self.url(endpoint);
        let auth = match (&self.token, &self.oauth) {
            (Some(token), _) => format!("Discogs token={token}"),
            (None, Some(oauth)) => {
                let owned: Vec<(String, String)> =
                    params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
                oauth.header("GET", &url, &owned, None)
            }
            _ => String::new(),
        };
        let full = if params.is_empty() {
            url.clone()
        } else {
            format!("{url}?{}", urlencode(params))
        };
        match self.session.get(
            &full,
            Request {
                headers: &[("Authorization", &auth)],
                ..Default::default()
            },
        ) {
            Ok(resp) => resp.json().map_err(|_| DiscogsError::Malformed(full)),
            Err(HttpError::Status { code, .. }) => Err(DiscogsError::Status(code, full)),
            Err(e) => Err(DiscogsError::Request(e.to_string())),
        }
    }

    /// `Release.refresh()`/`fetch()`: a release's data.
    pub fn release(&self, id: &str) -> Result<J, DiscogsError> {
        self.get(&format!("/releases/{id}"), &[])
    }

    /// `Master.fetch("year")`: a master release's year.
    pub fn master_year(&self, id: &str) -> Result<Option<i64>, DiscogsError> {
        let data = self.get(&format!("/masters/{id}"), &[])?;
        Ok(data.get("year").and_then(serde_json::Value::as_i64))
    }

    /// `client.search(query, **filters)`: the first page's result data.
    pub fn search(
        &self,
        query: &str,
        filters: &[(String, String)],
        limit: usize,
    ) -> Result<Vec<J>, DiscogsError> {
        let mut params: Vec<(&str, String)> = vec![("q", query.to_string())];
        for (k, v) in filters {
            params.push((k.as_str(), v.clone()));
        }
        params.push(("per_page", limit.to_string()));
        params.push(("page", "1".to_string()));
        let data = self.get("/database/search", &params)?;
        Ok(data
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

// ---------------------------------------------------------------- plugin

/// The API's base URL.
#[derive(Debug, Clone)]
pub struct Urls {
    pub api: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            api: std::env::var("BEETSRS_DISCOGS_API_URL")
                .unwrap_or_else(|_| "https://api.discogs.com".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Conf {
    pub urls: Urls,
    pub apikey: String,
    pub apisecret: String,
    pub tokenfile: PathBuf,
    pub user_token: String,
    pub separator: String,
    pub index_tracks: bool,
    pub append_style_genre: bool,
    pub strip_disambiguation: bool,
    pub featured_string: String,
    pub extra_tags: Vec<String>,
    pub search_limit: usize,
    pub search_query_ascii: bool,
    /// `anv`: whether to prefer the artist name variation for the artist,
    /// album artist and credit.
    pub anv_artist: bool,
    pub anv_album_artist: bool,
    pub anv_artist_credit: bool,
}

impl Conf {
    pub fn from_config(cfg: &Config) -> Self {
        Conf {
            urls: Urls::default(),
            apikey: cfg.str("discogs.apikey"),
            apisecret: cfg.str("discogs.apisecret"),
            tokenfile: cfg.config_dir.join(cfg.str("discogs.tokenfile")),
            user_token: cfg.str("discogs.user_token"),
            separator: cfg.str("discogs.separator"),
            index_tracks: cfg.bool("discogs.index_tracks"),
            append_style_genre: cfg.bool("discogs.append_style_genre"),
            strip_disambiguation: cfg.bool("discogs.strip_disambiguation"),
            featured_string: cfg.str("discogs.featured_string"),
            extra_tags: cfg.list("discogs.extra_tags"),
            search_limit: cfg.f64("discogs.search_limit").max(0.0) as usize,
            search_query_ascii: cfg.bool("discogs.search_query_ascii"),
            anv_artist: cfg.bool("discogs.anv.artist"),
            anv_album_artist: cfg.bool("discogs.anv.album_artist"),
            anv_artist_credit: cfg.bool("discogs.anv.artist_credit"),
        }
    }
}

pub struct Discogs {
    pub conf: Conf,
    pub log: PluginLog,
    client: RefCell<DiscogsClient>,
    /// `get_master_year`'s cache.
    master_years: RefCell<HashMap<String, Option<i64>>>,
    /// The `va_name` setting.
    pub va_name: String,
}

impl Discogs {
    /// Whether the plugin has credentials to use (a user token or a saved
    /// OAuth token): without them its setup would log in interactively, as
    /// beets' plugin loader does once.
    pub fn is_configured(cfg: &Config) -> bool {
        !cfg.str("discogs.user_token").is_empty()
            || cfg
                .config_dir
                .join(cfg.str("discogs.tokenfile"))
                .exists()
    }

    /// `setup`: a configured user token (bypassing OAuth), else the token
    /// file, else the interactive OAuth login.
    pub fn new(cfg: &Config) -> Result<Self> {
        let conf = Conf::from_config(cfg);
        let va_name = cfg.str("va_name");
        if !conf.user_token.is_empty() {
            let client = DiscogsClient::with_token(&conf.urls.api, USER_AGENT, &conf.user_token);
            return Ok(Self::with(conf, client, &va_name));
        }
        let (token, secret) = match std::fs::read_to_string(&conf.tokenfile) {
            Ok(text) => {
                let data: J = serde_json::from_str(&text).map_err(|e| {
                    anyhow!("{} could not be read: {e}", conf.tokenfile.display())
                })?;
                (
                    data.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    data.get("secret").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                )
            }
            Err(_) => authenticate(&conf)?,
        };
        let oauth = OAuth1::new(&conf.apikey, &conf.apisecret).with_token(&token, &secret);
        let client = DiscogsClient::with_oauth(&conf.urls.api, USER_AGENT, oauth);
        Ok(Self::with(conf, client, &va_name))
    }

    pub fn with(conf: Conf, client: DiscogsClient, va_name: &str) -> Self {
        Discogs {
            conf,
            log: PluginLog::new("discogs"),
            client: RefCell::new(client),
            master_years: RefCell::new(HashMap::new()),
            va_name: va_name.to_string(),
        }
    }

    /// `reset_auth`: delete the token file and log in again.
    fn reset_auth(&self) {
        let _ = std::fs::remove_file(&self.conf.tokenfile);
        if let Ok((token, secret)) = authenticate(&self.conf) {
            let oauth =
                OAuth1::new(&self.conf.apikey, &self.conf.apisecret).with_token(&token, &secret);
            *self.client.borrow_mut() =
                DiscogsClient::with_oauth(&self.conf.urls.api, USER_AGENT, oauth);
        }
    }

    /// `strip_disambiguation` for labels (the plugin's own, without an
    /// `ArtistState`).
    fn strip_label_disambiguation(&self, text: &str) -> String {
        if self.conf.strip_disambiguation {
            strip_disambiguation_re(text)
        } else {
            text.to_string()
        }
    }

    /// `get_media_and_albumtype`: the first format's media and descriptions.
    pub fn get_media_and_albumtype(formats: Option<&Vec<J>>) -> (Option<String>, Option<String>) {
        let Some(first) = formats.and_then(|f| f.first()) else {
            return (None, None);
        };
        let albumtype = first.get("descriptions").and_then(|d| d.as_array()).map(|d| {
            d.iter()
                .map(|x| x.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(", ")
        });
        let media = first
            .get("name")
            .and_then(|n| n.as_str())
            .map(String::from);
        (media, albumtype)
    }

    fn artist_state(&self, artists: Vec<Artist>, for_album_artist: bool) -> ArtistState {
        ArtistState {
            raw_artists: artists,
            use_anv: self.conf_anv(if for_album_artist {
                "album_artist"
            } else {
                "artist"
            }),
            use_credit_anv: self.conf_anv("artist_credit"),
            featured_string: self.conf.featured_string.clone(),
            should_strip_disambiguation: self.conf.strip_disambiguation,
            va_name: self.va_name.clone(),
        }
    }

    fn conf_anv(&self, key: &str) -> bool {
        match key {
            "artist" => self.conf.anv_artist,
            "album_artist" => self.conf.anv_album_artist,
            _ => self.conf.anv_artist_credit,
        }
    }

    /// `get_track_info`: a tracklist entry as a `TrackInfo`, and its medium
    /// and medium index.
    pub fn get_track_info(
        &self,
        track: &J,
        index: usize,
        divisions: &[String],
        albumartist: &ArtistState,
    ) -> (TrackInfo, Option<String>, Option<String>) {
        let mut title = track
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        if self.conf.index_tracks {
            let prefix = divisions.join(", ");
            if !prefix.is_empty() {
                title = format!("{prefix}: {title}");
            }
        }
        let position = track.get("position").and_then(|p| p.as_str()).unwrap_or_default();
        let (medium, medium_index, _) = get_track_index(position);
        let length = get_track_length(
            track.get("duration").and_then(|d| d.as_str()).unwrap_or_default(),
        );
        // `[*(track.get("artists") or albumartistinfo.raw_artists),
        //   *track.get("extraartists", [])]`
        let mut artists: Vec<Artist> = match track.get("artists").and_then(|a| a.as_array()) {
            Some(a) if !a.is_empty() => a.iter().map(Artist::from_data).collect(),
            _ => albumartist.raw_artists.clone(),
        };
        if let Some(extra) = track.get("extraartists").and_then(|a| a.as_array()) {
            artists.extend(extra.iter().map(Artist::from_data));
        }
        let state = self.artist_state(artists, false);
        let info = state.info();
        let track_info = TrackInfo {
            title,
            artist: info.artist,
            artist_id: info.artist_id,
            artists: info.artists,
            artists_ids: info.artists_ids,
            artist_credit: info.artist_credit,
            artists_credit: info.artists_credit,
            arrangers: info.arrangers,
            composers: info.composers,
            remixers: info.remixers,
            lyricists: info.lyricists,
            length,
            index,
            ..Default::default()
        };
        (track_info, medium, medium_index)
    }

    /// `TracklistState.build`.
    fn build_tracklist(&self, tracklist: &[J], albumartist: &ArtistState) -> TracklistState {
        let mut state = TracklistState::default();
        for track in tracklist {
            if track_type(track) == "track" {
                state.index += 1;
                if !state.next_divisions.is_empty() {
                    state.divisions.append(&mut state.next_divisions);
                    state.next_divisions.clear();
                }
                let mut info = self.get_track_info(track, state.index, &state.divisions, albumartist);
                info.0.track_alt = Some(
                    track
                        .get("position")
                        .and_then(|p| p.as_str())
                        .unwrap_or_default()
                        .to_string(),
                );
                state.tracks.push(info.0);
                state.mediums.push(info.1);
                state.medium_indices.push(info.2);
            } else {
                let title = track
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                state.next_divisions.push(title.clone());
                state.divisions.pop();
                state.index_tracks.insert(state.index + 1, title);
            }
        }
        state
    }

    /// `_subtrack_position`: the physical position of a flat audio subtrack.
    fn subtrack_position(track: &J) -> Option<String> {
        if track_type(track) != "track" {
            return None;
        }
        let position = track.get("position").and_then(|p| p.as_str()).unwrap_or_default();
        let (medium, medium_index, subindex) = get_track_index(position);
        subindex.map(|_| {
            format!(
                "{}{}",
                medium.unwrap_or_default(),
                medium_index.unwrap_or_default()
            )
        })
    }

    /// `_merge_subtracks`: one physical track from its flat subtracks.
    fn merge_subtracks(subtracks: &[J]) -> J {
        let mut merged = subtracks[0].clone();
        let titles: Vec<&str> = subtracks
            .iter()
            .map(|t| t.get("title").and_then(|x| x.as_str()).unwrap_or_default())
            .collect();
        if let Some(obj) = merged.as_object_mut() {
            obj.insert("title".into(), J::String(titles.join(" / ")));
        }
        merged
    }

    /// `_coalesce_index_track`: an index container's physical tracks.
    fn coalesce_index_track(&self, index_track: &J) -> Option<Vec<J>> {
        let subtracks: Vec<J> = index_track
            .get("sub_tracks")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        let first_position = subtracks
            .first()
            .and_then(|t| t.get("position"))
            .and_then(|p| p.as_str())
            .unwrap_or_default();
        let (medium, medium_index, subindex) = get_track_index(first_position);
        if subindex.is_some() {
            // A logical piece of one physical track: keep the index's
            // metadata.
            let mut merged = serde_json::Map::new();
            merged.insert("type_".into(), J::String("track".into()));
            merged.insert(
                "position".into(),
                J::String(format!(
                    "{}{}",
                    medium.unwrap_or_default(),
                    medium_index.unwrap_or_default()
                )),
            );
            merged.insert(
                "title".into(),
                index_track.get("title").cloned().unwrap_or(J::Null),
            );
            merged.insert(
                "duration".into(),
                index_track.get("duration").cloned().unwrap_or(J::Null),
            );
            for k in ["artists", "extraartists"] {
                if let Some(v) = index_track.get(k) {
                    merged.insert(k.into(), v.clone());
                }
            }
            return Some(vec![J::Object(merged)]);
        }
        let artists = index_track.get("artists").cloned();
        let title_prefix = if self.conf.index_tracks {
            format!(
                "{}: ",
                index_track
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
            )
        } else {
            String::new()
        };
        let mut out = vec![];
        for sub in &subtracks {
            let mut track = sub.clone();
            if let (Some(artists), None) = (
                artists.as_ref().filter(|a| !a.is_null()),
                track.get("artists").filter(|a| !a.is_null()),
            )
                && let Some(obj) = track.as_object_mut() {
                    obj.insert("artists".into(), artists.clone());
                }
            if !title_prefix.is_empty() {
                let title = track
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                if let Some(obj) = track.as_object_mut() {
                    obj.insert("title".into(), J::String(format!("{title_prefix}{title}")));
                }
            }
            out.push(track);
        }
        Some(out)
    }

    /// `_coalesce_tracks`: normalize each tracklist entry by its kind.
    fn coalesce_tracks(&self, raw: &[J]) -> Option<Vec<J>> {
        let mut out = vec![];
        let mut i = 0;
        while i < raw.len() {
            let position = Self::subtrack_position(&raw[i]);
            if position.is_some() {
                // Consecutive subtracks of one physical track.
                let mut group = vec![];
                let mut j = i;
                while j < raw.len() && Self::subtrack_position(&raw[j]) == position {
                    group.push(raw[j].clone());
                    j += 1;
                }
                let subtracks: Vec<J> = group
                    .iter()
                    .filter(|t| track_type(t) == "track")
                    .cloned()
                    .collect();
                if subtracks.is_empty() {
                    out.extend(group);
                } else {
                    out.push(Self::merge_subtracks(&subtracks));
                }
                i = j;
                continue;
            }
            match track_type(&raw[i]) {
                "track" | "heading" => out.push(raw[i].clone()),
                "index" => {
                    out.extend(self.coalesce_index_track(&raw[i])?);
                }
                _ => {}
            }
            i += 1;
        }
        Some(out)
    }

    /// `get_tracks`: the tracklist's tracks, with medium and index fix-ups.
    pub fn get_tracks(&self, tracklist: &[J], albumartist: &ArtistState) -> Vec<TrackInfo> {
        let clean = match self.coalesce_tracks(tracklist) {
            Some(c) => c,
            None => {
                self.log
                    .error("uncaught exception in _coalesce_tracks: Discogs index track does not contain subtracks");
                tracklist.to_vec()
            }
        };
        let mut t = self.build_tracklist(&clean, albumartist);
        let mut medium: Option<String> = None;
        let mut medium_count = 0usize;
        let mut index_count = 0usize;
        let mut side_count = 0usize;
        let mut sides_per_medium = 1usize;
        let all_some = t.mediums.iter().all(Option::is_some);
        if all_some {
            let mut m: Vec<String> = t
                .mediums
                .iter()
                .map(|x| x.clone().unwrap_or_default().to_lowercase())
                .collect();
            m.sort();
            m.dedup();
            let joined: String = m.concat();
            // `"".join(m) in ascii_lowercase`: a substring check (the
            // empty string counts).
            if "abcdefghijklmnopqrstuvwxyz".contains(&joined) {
                sides_per_medium = 2;
            }
        }
        for i in 0..t.tracks.len() {
            let medium_str = t.mediums[i].clone();
            let medium_index = t.medium_indices[i].clone();
            let medium_is_index = medium_str.is_some()
                && medium_index.is_none()
                && (medium_str.as_ref().map(|m| m.chars().count()) != Some(1)
                    || {
                        let c = medium_str.as_ref().unwrap().chars().next().unwrap() as i64;
                        c - 64 != side_count as i64 + 1
                    });
            if !medium_is_index && medium != medium_str {
                side_count += 1;
                if sides_per_medium == 2 {
                    if !side_count.is_multiple_of(sides_per_medium) {
                        index_count = 0;
                        medium_count += 1;
                    }
                } else {
                    medium_count += 1;
                    index_count = 0;
                }
                medium = medium_str;
            }
            index_count += 1;
            if medium_count == 0 {
                medium_count = 1;
            }
            t.tracks[i].medium = medium_count;
            t.tracks[i].medium_index = index_count;
        }
        // `disctitle` from index tracks: the one before a medium's first
        // track is the disc's title.
        let index_tracks = t.index_tracks.clone();
        let mut disctitle: Option<String> = None;
        for track in &mut t.tracks {
            if track.medium_index == 1 {
                disctitle = index_tracks.get(&track.index).cloned();
            }
            track.disctitle = disctitle.clone().unwrap_or_default();
            if disctitle.is_none() {
                track.nulls.insert("disctitle");
            }
        }
        t.tracks
    }

    /// `get_album_info`: a release's data as an `AlbumInfo`.
    pub fn get_album_info(&self, data: &J) -> Option<AlbumInfo> {
        let mut data = data.clone();
        // Reload the release when its fields aren't present (a search
        // result has only a summary).
        if data.get("artists").is_none() {
            let id = data.get("id").map(json_id).unwrap_or_default();
            match self.client.borrow().release(&id) {
                Ok(full) => data = full,
                Err(e) => {
                    self.log
                        .debug(&format!("Connection error in release lookup: {e}"));
                    return None;
                }
            }
        }
        let artist_data: Vec<Artist> = data
            .get("artists")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().map(Artist::from_data).collect())
            .unwrap_or_default();
        let albumartist = self.artist_state(artist_data.clone(), true);
        let album = {
            let title = data.get("title").and_then(|t| t.as_str()).unwrap_or_default();
            regex::Regex::new(" +")
                .unwrap()
                .replace_all(title, " ")
                .into_owned()
        };
        let album_id = json_id(data.get("id").unwrap_or(&J::Null));
        let tracklist: Vec<J> = data
            .get("tracklist")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mut tracks = self.get_tracks(&tracklist, &self.artist_state(artist_data, false));
        let va = albumartist.main_artists().first().is_some_and(|a| a.name == self.va_name);
        let (year, month, day) = parse_release_date(
            data.get("released").and_then(|r| r.as_str()),
            data.get("year").and_then(serde_json::Value::as_i64),
        );
        let mediums: Vec<usize> = tracks.iter().map(|t| t.medium).collect();
        let country = data.get("country").and_then(|c| c.as_str()).map(String::from);
        let data_url = data.get("uri").and_then(|u| u.as_str()).map(String::from);
        let mut styles: Vec<String> = string_list(data.get("genres"));
        let mut genres: Vec<String> = string_list(data.get("styles"));
        if self.conf.append_style_genre {
            genres.extend(styles.iter().cloned());
        }
        let discogs_albumid = data
            .get("uri")
            .and_then(|u| u.as_str())
            .and_then(|u| extract_id("discogs", u));
        let (media, albumtype) =
            Self::get_media_and_albumtype(data.get("formats").and_then(|f| f.as_array()));
        let mut label = None;
        let mut catalogno = None;
        let mut labelid = None;
        if let Some(labels) = data.get("labels").and_then(|l| l.as_array())
            && let Some(first) = labels.first() {
                label = first
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| self.strip_label_disambiguation(n));
                catalogno = first.get("catno").and_then(|c| c.as_str()).map(String::from);
                labelid = first.get("id").map(json_id);
            }
        let cover_art_url = data
            .get("images")
            .and_then(|i| i.as_array())
            .and_then(|i| i.first())
            .and_then(|i| i.get("uri"))
            .and_then(|u| u.as_str())
            .map(String::from);
        if catalogno.as_deref() == Some("none") {
            catalogno = None;
        }
        for track in &mut tracks {
            track.media = media.clone().unwrap_or_default();
            track.medium_total = mediums.iter().filter(|m| **m == track.medium).count();
            // Discogs has no track ids: invent them (#2336).
            track.track_id = format!("{album_id}-{}", track.track_alt.clone().unwrap_or_default());
            track.data_url = data_url.clone().unwrap_or_default();
            track.data_source = "Discogs".to_string();
        }
        let master_id = data.get("master_id").and_then(|m| {
            (!m.is_null()).then(|| json_id(m))
        });
        let (original_year, original_month, original_day) = match &master_id {
            Some(mid) => (self.get_master_year(mid), None, None),
            None => (year, month, day),
        };
        let mut nulls: BTreeSet<&'static str> = BTreeSet::new();
        let mut extra: Vec<(String, V)> = vec![];
        if !styles.is_empty() {
            styles.sort();
            extra.push((
                "style".to_string(),
                V::Str(styles.join(&self.conf.separator)),
            ));
        } else {
            extra.push(("style".to_string(), V::None));
        }
        genres.sort();
        extra.push(("genres".to_string(), V::List(genres)));
        for (k, v) in [
            ("discogs_albumid", discogs_albumid.clone()),
            ("discogs_labelid", labelid.clone()),
            (
                "discogs_artistid",
                albumartist.main_artists().first().map(|a| a.id.clone()),
            ),
            ("cover_art_url", cover_art_url.clone()),
        ] {
            extra.push((
                k.to_string(),
                match v {
                    Some(v) => V::Str(v),
                    None => V::None,
                },
            ));
        }
        let label = opt_str(&mut nulls, "label", label);
        let catalognum = opt_str(&mut nulls, "catalognum", catalogno);
        let country = opt_str(&mut nulls, "country", country);
        let albumtype = opt_str(&mut nulls, "albumtype", albumtype);
        let media = opt_str(&mut nulls, "media", media);
        let data_url = opt_str(&mut nulls, "data_url", data_url);
        Some(AlbumInfo {
            album,
            album_id,
            artist: albumartist.info().artist,
            artist_id: albumartist.info().artist_id,
            artist_credit: albumartist.info().artist_credit,
            artists: albumartist.info().artists,
            artists_ids: albumartist.info().artists_ids,
            artists_credit: albumartist.info().artists_credit,
            va,
            tracks,
            year: year.unwrap_or(0),
            month: month.unwrap_or(0),
            day: day.unwrap_or(0),
            original_year: original_year.unwrap_or(0),
            original_month: original_month.unwrap_or(0),
            original_day: original_day.unwrap_or(0),
            label,
            catalognum,
            country,
            albumtype,
            media,
            mediums: {
                let mut s = mediums.clone();
                s.sort();
                s.dedup();
                s.len()
            },
            releasegroup_id: master_id.unwrap_or_default(),
            data_source: "Discogs".to_string(),
            data_url,
            nulls,
            extra,
            ..Default::default()
        })
    }

    /// `get_master_year`, cached.
    fn get_master_year(&self, master_id: &str) -> Option<i64> {
        if let Some(v) = self.master_years.borrow().get(master_id) {
            return *v;
        }
        self.log
            .debug(&format!("Getting master release {master_id}"));
        let result = match self.client.borrow().master_year(master_id) {
            Ok(v) => v,
            Err(DiscogsError::Status(404, _)) => None,
            Err(DiscogsError::Status(code, url)) => {
                self.log.debug(&format!("API Error: {code} (query: {url:?})"));
                if code == 401 {
                    self.reset_auth();
                    return self.get_master_year(master_id);
                }
                None
            }
            Err(e) => {
                self.log
                    .debug(&format!("Connection error in master release lookup: {e}"));
                None
            }
        };
        self.master_years
            .borrow_mut()
            .insert(master_id.to_string(), result);
        result
    }

    /// `get_search_query_with_filters`.
    pub fn search_query(
        &self,
        items: &[Item],
        artist: &str,
        name: &str,
        va_likely: bool,
    ) -> (String, Vec<(String, String)>) {
        let mut query = if va_likely {
            name.to_string()
        } else {
            format!("{artist} {name}")
        };
        // Strip non-word characters (`(?u)\W+`).
        let re = regex::Regex::new(r"\W+").unwrap();
        query = re.replace_all(&query, " ").into_owned();
        let medium = regex::Regex::new(r"(?i)\b(CD|disc|vinyl)\s*\d+").unwrap();
        query = medium.replace_all(&query, "").into_owned();
        let mut filters: Vec<(String, String)> = vec![("type".into(), "release".into())];
        if items.is_empty() {
            return (query, filters);
        }
        let extra = self.extra_field_by_tag();
        for (tag, api_field) in extra {
            let mut values: Vec<Option<String>> = items
                .iter()
                .map(|i| i.get(&tag).map(|v| v.raw()))
                .collect();
            if tag == "media" {
                values = values
                    .into_iter()
                    .map(|v| {
                        v.map(|v| {
                            if MEDIA_FORMAT_ALIASES.contains(&v.to_lowercase().as_str()) {
                                "File".to_string()
                            } else {
                                v
                            }
                        })
                    })
                    .collect();
            }
            let Some(most_common) = plurality(values) else {
                continue;
            };
            let value = if tag == "catalognum" {
                most_common.replace(' ', "")
            } else {
                most_common
            };
            filters.push((api_field.to_string(), value));
        }
        (query, filters)
    }

    /// `extra_discogs_field_by_tag`: the configured extra tags' API fields.
    fn extra_field_by_tag(&self) -> Vec<(String, String)> {
        let mut out = vec![];
        for tag in &self.conf.extra_tags {
            if let Some((t, k)) = FIELDS_TO_DISCOGS_KEYS.iter().find(|(f, _)| f == tag) {
                out.push((t.to_string(), k.to_string()));
            } else if *tag == FIELDS_TO_DISCOGS_KEYS_YEAR.0 {
                out.push(("year".to_string(), "year".to_string()));
            }
        }
        out
    }

    /// `get_search_response`: a search's result data; a malformed response
    /// is retried once.
    pub fn get_search_response(
        &self,
        query: &str,
        filters: &[(String, String)],
    ) -> Result<Vec<J>, DiscogsError> {
        let limit = self.conf.search_limit;
        match self.client.borrow().search(query, filters, limit) {
            Ok(r) => Ok(r),
            Err(DiscogsError::Malformed(m)) => {
                self.log
                    .debug(&format!("{m}; retrying"));
                self.client.borrow().search(query, filters, limit)
            }
            Err(e) => Err(e),
        }
    }

    /// `_search_api`: normalize the query, then search.
    fn search_api(&self, query: &str, filters: &[(String, String)]) -> Vec<J> {
        let mut query = query.to_string();
        if self.conf.search_query_ascii {
            query = deunicode::deunicode(&query);
        }
        if query.is_empty() && filters.is_empty() {
            self.log.debug("Skipping search with empty query and filters");
            return vec![];
        }
        self.log
            .debug(&format!("Searching for '{query}' with {filters:?}"));
        match self.get_search_response(&query, filters) {
            Ok(r) => {
                self.log
                    .debug(&format!("Found {} result(s)", r.len()));
                r
            }
            Err(e) => {
                self.log.error(&format!("Error searching Discogs: {e}"));
                vec![]
            }
        }
    }
}

/// JSON values as Python `str()` would show an id.
fn json_id(v: &J) -> String {
    match v {
        J::Number(n) => n
            .as_i64()
            .map_or_else(|| n.to_string(), |i| i.to_string()),
        J::String(s) => s.clone(),
        J::Null => "None".to_string(),
        other => other.to_string(),
    }
}

fn string_list(v: Option<&J>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// beets' `util.plurality`: the most common value (None counts as a
/// value), ties to the first seen; None when no value is set at all.
fn plurality(values: Vec<Option<String>>) -> Option<String> {
    let mut counts: Vec<(Option<String>, usize)> = vec![];
    for v in values {
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some((_, c)) => *c += 1,
            None => counts.push((v, 1)),
        }
    }
    let mut best: Option<(Option<String>, usize)> = None;
    for (v, c) in counts {
        if best.as_ref().is_none_or(|(_, bc)| c > *bc) {
            best = Some((v, c));
        }
    }
    best.and_then(|(v, _)| v)
}

/// `authenticate`: the OAuth login, saving the token in the token file.
fn authenticate(conf: &Conf) -> Result<(String, String)> {
    let oauth = OAuth1::new(&conf.apikey, &conf.apisecret);
    // `Client(...).get_authorize_url()`
    let base = &conf.urls.api;
    let request_url = format!("{base}/oauth/request_token");
    let header = oauth.header("GET", &request_url, &[], Some("oob"));
    let session = Session::plain(USER_AGENT);
    let resp = session
        .get(
            &request_url,
            Request {
                headers: &[("Authorization", &header)],
                ..Default::default()
            },
        )
        .map_err(|e| anyhow!("communication with Discogs failed: {e}"))?;
    let params = parse_qsl(&resp.text(false));
    let get = |k: &str| {
        params
            .iter()
            .find(|(name, _)| name == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let token = get("oauth_token");
    let token_secret = get("oauth_token_secret");
    let auth_url = format!("{base}/oauth/authorize?oauth_token={token}");
    println!("To authenticate with Discogs, visit:");
    println!("{auth_url}");
    let Some(code) = crate::ui::read_line_or_eof("Enter the code:") else {
        bail!("Discogs authorization failed");
    };
    let mut oauth = oauth.with_token(&token, &token_secret);
    let access_url = format!("{base}/oauth/access_token");
    let verifier: Vec<(String, String)> = vec![("oauth_verifier".to_string(), code.clone())];
    let header = oauth.header("GET", &access_url, &verifier, None);
    let resp = session
        .get(
            &format!("{access_url}?oauth_verifier={code}"),
            Request {
                headers: &[("Authorization", &header)],
                ..Default::default()
            },
        )
        .map_err(|_| anyhow!("Discogs token request failed"))?;
    let params = parse_qsl(&resp.text(false));
    let get = |k: &str| {
        params
            .iter()
            .find(|(name, _)| name == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let (token, secret) = (get("oauth_token"), get("oauth_token_secret"));
    if let Some(dir) = conf.tokenfile.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(
        &conf.tokenfile,
        format!("{{\"token\": {token:?}, \"secret\": {secret:?}}}"),
    )
    .map_err(|e| {
        anyhow!(
            "{} could not be written: {e}",
            conf.tokenfile.display()
        )
    })?;
    oauth.token = Some(token.clone());
    Ok((token, secret))
}

impl MetadataSource for Discogs {
    fn data_source(&self) -> &'static str {
        "Discogs"
    }

    fn album_for_id(&self, _ctx: &SourceCtx, album_id: &str) -> Result<Option<AlbumInfo>> {
        let Some(discogs_id) = extract_id("discogs", album_id) else {
            return Ok(None);
        };
        match self.client.borrow().release(&discogs_id) {
            Ok(data) => Ok(self.get_album_info(&data)),
            Err(DiscogsError::Status(404, _)) => Ok(None),
            Err(DiscogsError::Status(code, url)) => {
                self.log.debug(&format!("API Error: {code} (query: {url:?})"));
                if code == 401 {
                    self.reset_auth();
                    return self.album_for_id(_ctx, album_id);
                }
                Ok(None)
            }
            Err(e) => {
                self.log
                    .debug(&format!("Connection error in album lookup: {e}"));
                Ok(None)
            }
        }
    }

    fn track_for_id(&self, ctx: &SourceCtx, track_id: &str) -> Result<Option<TrackInfo>> {
        if let Some(album) = self.album_for_id(ctx, track_id)? {
            for track in album.tracks {
                if track.track_id == track_id {
                    return Ok(Some(track));
                }
            }
        }
        Ok(None)
    }

    fn candidates(
        &self,
        _ctx: &SourceCtx,
        items: &[Item],
        artist: &str,
        album: &str,
        va_likely: bool,
    ) -> Result<Vec<AlbumInfo>> {
        let (query, filters) = self.search_query(items, artist, album, va_likely);
        let mut out = vec![];
        for result in self.search_api(&query, &filters) {
            if let Some(info) = self.get_album_info(&result) {
                out.push(info);
            }
        }
        Ok(out)
    }

    fn item_candidates(
        &self,
        ctx: &SourceCtx,
        item: &Item,
        artist: &str,
        title: &str,
    ) -> Result<Vec<TrackInfo>> {
        let albums = self.candidates(ctx, std::slice::from_ref(item), artist, title, false)?;
        let mut out = vec![];
        for album in albums {
            let closest = album
                .tracks
                .iter()
                .map(|t| (string_dist(&t.title, title), t.clone()))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((score, mut track)) = closest {
                if score > 0.3 {
                    continue;
                }
                track.artist.clone_from(&album.artist);
                track.artist_id.clone_from(&album.artist_id);
                track.extra.push(("album".to_string(), V::Str(album.album.clone())));
                out.push(track);
            }
        }
        Ok(out)
    }
}

/// beets' `string_dist`: the normalized Levenshtein distance (0 for equal
/// strings, 1 for wholly different ones).
pub fn string_dist(a: &str, b: &str) -> f64 {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    if a == b {
        return 0.0;
    }
    let la = a.chars().count();
    let lb = b.chars().count();
    if la == 0 || lb == 0 {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=lb).collect();
    let mut cur = vec![0usize; lb + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let dist = prev[lb] as f64;
    (dist / la.max(lb) as f64).min(1.0)
}

impl super::Plugin for Discogs {
    fn name(&self) -> &'static str {
        "discogs"
    }
}

// --------------------------------------------------------------- registry

fn registry_make(cfg: &Config) -> Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Discogs::new(cfg)?))
}

fn metadata_source_make(
    cfg: &Config,
) -> Result<Box<dyn crate::metadata_plugins::MetadataSource>> {
    Ok(Box::new(Discogs::new(cfg)?))
}

inventory::submit! {
    crate::metadata_plugins::MetadataSourceDef {
        // 5.19
        order: 91,
        plugin: "discogs",
        data_source: "Discogs",
        configured: Discogs::is_configured,
        make: metadata_source_make,
    }
}

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "discogs",
        // 5.19
        order: 91,
        defaults: r#"discogs:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    search_query_ascii: no
    apikey: rAzVUQYRaoFjeBjyWuWZ
    apisecret: plxtUTqoCzwxZpqdPysCwGuBSmZNdZVy
    tokenfile: discogs_token.json
    user_token: ''
    separator: ', '
    index_tracks: no
    append_style_genre: no
    strip_disambiguation: yes
    featured_string: Feat.
    extra_tags: []
    anv:
        artist_credit: yes
        artist: no
        album_artist: no
"#,
        config_keys: &["discogs"],
        item_types: &[],
        album_types: &[],
        redacted: &["discogs.apikey", "discogs.apisecret", "discogs.user_token"],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

#[cfg(test)]
#[path = "discogs_tests.rs"]
mod tests;
