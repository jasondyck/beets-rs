//! A port of beets 2.14's `spotify` plugin (`beetsplug/spotify.py`):
//! release and track search on the Spotify Web API as autotag candidates,
//! lookups by Spotify id or URL, `beet spotify` (playlist URLs or a
//! `spotify:trackset:` URI) and `beet spotifysync` (track popularity,
//! ISRC and audio features).
//!
//! The API's base URLs are the plugin's, except in tests (which build the
//! plugin with [`Spotify::with`] and a fake server's URLs).

use super::requests::{HttpError, Request, Session, format_url};
use super::{PluginLog, USER_AGENT};
use crate::config::Config;
use crate::hooks::V;
use crate::mb::{AlbumInfo, TrackInfo};
use crate::metadata_plugins::{MetadataSource, SourceCtx, extract_id};
use crate::model::{FieldType, Item, Kind};
use anyhow::{Result, anyhow};
use serde_json::Value as J;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `DEFAULT_WAITING_TIME`.
const DEFAULT_WAITING_TIME: u64 = 5;

/// Spotify's item fields and their types (`SpotifyPlugin.item_types`).
pub const ITEM_TYPES: &[(&str, FieldType)] = &[
    ("spotify_track_popularity", FieldType::Int),
    ("spotify_acousticness", FieldType::Float),
    ("spotify_danceability", FieldType::Float),
    ("spotify_energy", FieldType::Float),
    ("spotify_instrumentalness", FieldType::Float),
    ("spotify_key", FieldType::Float),
    ("spotify_liveness", FieldType::Float),
    ("spotify_loudness", FieldType::Float),
    ("spotify_mode", FieldType::Int),
    ("spotify_speechiness", FieldType::Float),
    ("spotify_tempo", FieldType::Float),
    ("spotify_time_signature", FieldType::Int),
    ("spotify_valence", FieldType::Float),
    ("spotify_updated", FieldType::Date),
];

/// `spotify_audio_features`: API field -> item field.
const AUDIO_FEATURES: &[(&str, &str)] = &[
    ("acousticness", "spotify_acousticness"),
    ("danceability", "spotify_danceability"),
    ("energy", "spotify_energy"),
    ("instrumentalness", "spotify_instrumentalness"),
    ("key", "spotify_key"),
    ("liveness", "spotify_liveness"),
    ("loudness", "spotify_loudness"),
    ("mode", "spotify_mode"),
    ("speechiness", "spotify_speechiness"),
    ("tempo", "spotify_tempo"),
    ("time_signature", "spotify_time_signature"),
    ("valence", "spotify_valence"),
];

/// The API's base URLs (`SpotifyPlugin.oauth_token_url` etc.).
#[derive(Debug, Clone)]
pub struct Urls {
    pub oauth_token: String,
    pub open_track: String,
    pub search: String,
    pub album: String,
    pub track: String,
    pub audio_features: String,
}

impl Default for Urls {
    fn default() -> Self {
        // `BEETSRS_SPOTIFY_API_URL` (a fake server in tests and for
        // debugging) replaces the API host; the web URLs stay.
        let base = std::env::var("BEETSRS_SPOTIFY_API_URL").ok();
        let mut urls = Urls {
            oauth_token: "https://accounts.spotify.com/api/token".into(),
            open_track: "https://open.spotify.com/track/".into(),
            search: "https://api.spotify.com/v1/search".into(),
            album: "https://api.spotify.com/v1/albums/".into(),
            track: "https://api.spotify.com/v1/tracks".into(),
            audio_features: "https://api.spotify.com/v1/audio-features".into(),
        };
        if let Some(base) = base {
            urls.oauth_token = format!("{base}/api/token");
            urls.search = format!("{base}/v1/search");
            urls.album = format!("{base}/v1/albums/");
            urls.track = format!("{base}/v1/tracks");
            urls.audio_features = format!("{base}/v1/audio-features");
        }
        urls
    }
}

/// A `regex` setting: `field`, `search`, `replace`.
#[derive(Debug, Clone, Default)]
pub struct RegexSpec {
    pub field: String,
    pub search: String,
    pub replace: String,
}

#[derive(Debug, Clone)]
pub struct Conf {
    pub urls: Urls,
    pub mode: String,
    pub tiebreak: String,
    pub show_failures: bool,
    pub region_filter: Option<String>,
    pub regex: Vec<RegexSpec>,
    pub client_id: String,
    pub client_secret: String,
    pub tokenfile: PathBuf,
    pub search_limit: usize,
    pub search_query_ascii: bool,
}

impl Conf {
    pub fn from_config(cfg: &Config) -> Self {
        let regex = match cfg.get("spotify.regex") {
            Some(serde_yaml::Value::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| {
                    let map = v.as_mapping()?;
                    let get = |k: &str| {
                        map.get(serde_yaml::Value::String(k.into()))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string()
                    };
                    Some(RegexSpec {
                        field: get("field"),
                        search: get("search"),
                        replace: get("replace"),
                    })
                })
                .collect(),
            _ => vec![],
        };
        let region_filter = match cfg.get("spotify.region_filter") {
            Some(serde_yaml::Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        Conf {
            urls: Urls::default(),
            mode: cfg.str("spotify.mode"),
            tiebreak: cfg.str("spotify.tiebreak"),
            show_failures: cfg.bool("spotify.show_failures"),
            region_filter,
            regex,
            client_id: cfg.str("spotify.client_id"),
            client_secret: cfg.str("spotify.client_secret"),
            // confuse's `Filename(in_app_dir=True)`.
            tokenfile: cfg.config_dir.join(cfg.str("spotify.tokenfile")),
            search_limit: cfg.f64("spotify.search_limit").max(0.0) as usize,
            search_query_ascii: cfg.bool("spotify.search_query_ascii"),
        }
    }
}

/// `APIError` / `AudioFeaturesUnavailableError`.
#[derive(Debug)]
pub enum ApiError {
    Error(String),
    /// The audio features API answered 403 (deprecated).
    AudioFeaturesUnavailable,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Error(m) => f.write_str(m),
            ApiError::AudioFeaturesUnavailable => f.write_str(
                "Audio features API returned 403 (deprecated or unavailable)",
            ),
        }
    }
}

impl std::error::Error for ApiError {}

/// `time.time()`.
fn time_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

/// base64 (standard alphabet, padded).
fn b64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub struct Spotify {
    pub conf: Conf,
    pub log: PluginLog,
    pub access_token: Option<String>,
    /// `audio_features_available`: turned off on the first 403.
    pub audio_features_available: bool,
    session: Session,
    /// Seconds to wait before a retry (beets sleeps 5/6; tests don't).
    pub retry_wait: u64,
}

impl Spotify {
    /// Whether the plugin has a token to use (a saved one, or credentials
    /// it can fetch one with): beets' loader authenticates when the plugin
    /// loads, so `metadata_plugins::sources` only rebuilds it then.
    pub fn is_configured(cfg: &Config) -> bool {
        cfg.config_dir.join(cfg.str("spotify.tokenfile")).exists()
    }

    /// The plugin's setup: the saved OAuth token, or a new one.
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut plugin = Self::with(Conf::from_config(cfg));
        plugin.setup()?;
        Ok(plugin)
    }

    pub fn with(conf: Conf) -> Self {
        Spotify {
            conf,
            log: PluginLog::new("spotify"),
            access_token: None,
            audio_features_available: true,
            session: Session::plain(USER_AGENT),
            retry_wait: DEFAULT_WAITING_TIME + 1,
        }
    }

    pub fn setup(&mut self) -> Result<()> {
        match std::fs::read_to_string(&self.conf.tokenfile) {
            Ok(text) => {
                let data: J = serde_json::from_str(&text)
                    .map_err(|e| anyhow!("{} could not be read: {e}", self.conf.tokenfile.display()))?;
                self.access_token = data
                    .get("access_token")
                    .and_then(|t| t.as_str())
                    .map(String::from);
            }
            Err(_) => self.authenticate()?,
        }
        Ok(())
    }

    /// `_authenticate`: the Client Credentials Flow.
    pub fn authenticate(&mut self) -> Result<()> {
        let auth = format!(
            "Basic {}",
            b64(format!("{}:{}", self.conf.client_id, self.conf.client_secret).as_bytes())
        );
        let resp = self
            .session
            .request(
                "POST",
                &self.conf.urls.oauth_token,
                Request {
                    headers: &[("Authorization", &auth)],
                    form: Some("grant_type=client_credentials"),
                    ..Default::default()
                },
            )
            .map_err(|e| anyhow!("Spotify authorization failed: {e}"))?;
        let body = resp.json().map_err(|e| {
            anyhow!(
                "Spotify authorization failed: {}\n{}",
                e,
                resp.text(false)
            )
        })?;
        let token = body
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("Spotify authorization failed"))?
            .to_string();
        self.log
            .debug(&format!("Spotify access token: {token}"));
        if let Some(dir) = self.conf.tokenfile.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(
            &self.conf.tokenfile,
            format!("{{\"access_token\": {token:?}}}"),
        )
        .map_err(|e| {
            anyhow!(
                "{} could not be written: {e}",
                self.conf.tokenfile.display()
            )
        })?;
        self.access_token = Some(token);
        Ok(())
    }

    fn token(&self) -> String {
        self.access_token.clone().unwrap_or_default()
    }

    /// A GET whose error statuses are errors (`raise_for_status`): the
    /// session's plain form hands them back as responses.
    fn get_checked(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<super::requests::Response, HttpError> {
        let resp = self.session.get_any(
            url,
            Request {
                headers,
                ..Default::default()
            },
        )?;
        if resp.status >= 400 {
            return Err(HttpError::Status {
                code: resp.status,
                url: url.to_string(),
                body: resp.body.clone(),
                headers: resp.headers.clone(),
            });
        }
        Ok(resp)
    }

    /// `_handle_response`: a request with the access token, reauthenticating
    /// on 401, retrying 429/503 (up to `max_retries`).
    pub fn handle_response(
        &mut self,
        method: &str,
        url: &str,
        params: &[(&str, String)],
    ) -> Result<J, ApiError> {
        self.handle_response_retry(method, url, params, 0, 3)
    }

    fn handle_response_retry(
        &mut self,
        method: &str,
        url: &str,
        params: &[(&str, String)],
        retry_count: u32,
        max_retries: u32,
    ) -> Result<J, ApiError> {
        if retry_count > max_retries {
            return Err(ApiError::Error("Maximum retries reached.".into()));
        }
        let auth = format!("Bearer {}", self.token());
        let full = format_url(url, params);
        let sent = if method.eq_ignore_ascii_case("get") {
            self.get_checked(&full, &[("Authorization", &auth)])
        } else {
            self.session
                .request(
                    method,
                    &full,
                    Request {
                        headers: &[("Authorization", &auth)],
                        ..Default::default()
                    },
                )
                .and_then(|r| {
                    if r.status >= 400 {
                        Err(HttpError::Status {
                            code: r.status,
                            url: full.clone(),
                            body: r.body.clone(),
                            headers: r.headers.clone(),
                        })
                    } else {
                        Ok(r)
                    }
                })
        };
        match sent.and_then(|r| r.json()) {
            Ok(j) => Ok(j),
            Err(HttpError::Status { code, .. }) => match code {
                401 => {
                    self.log.debug("Spotify access token has expired. Reauthenticating.");
                    self.authenticate()
                        .map_err(|e| ApiError::Error(format!("{e:#}")))?;
                    self.handle_response_retry(
                        method,
                        url,
                        params,
                        retry_count + 1,
                        max_retries,
                    )
                }
                404 => {
                    Err(ApiError::Error(format!(
                        "API Error: {code}\nURL: {url}\nparams: {params:?}"
                    )))
                }
                403 => {
                    if url.starts_with(&self.conf.urls.audio_features) {
                        return Err(ApiError::AudioFeaturesUnavailable);
                    }
                    Err(ApiError::Error(format!(
                        "API Error: {code}\nURL: {url}\nparams: {params:?}"
                    )))
                }
                429 => {
                    let seconds = DEFAULT_WAITING_TIME;
                    self.log
                        .debug(&format!("Too many API requests. Retrying after {seconds} seconds."));
                    std::thread::sleep(Duration::from_secs(self.retry_wait));
                    self.handle_response_retry(
                        method,
                        url,
                        params,
                        retry_count + 1,
                        max_retries,
                    )
                }
                503 => {
                    self.log.debug(
                        "Service Unavailable. Retrying after 5 seconds.",
                    );
                    std::thread::sleep(Duration::from_secs(self.retry_wait));
                    self.handle_response_retry(
                        method,
                        url,
                        params,
                        retry_count + 1,
                        max_retries,
                    )
                }
                502 => {
                    self.log.error("Bad Gateway.");
                    Err(ApiError::Error("Bad Gateway.".into()))
                }
                _ => {
                    Err(ApiError::Error(format!(
                        "Spotify API error:\n{code}\nURL:\n{url}\nparams:\n{params:?}"
                    )))
                }
            },
            Err(HttpError::Request(m)) => {
                let msg = if m.contains("timed out") {
                    self.log.error("ReadTimeout.");
                    "Request timed out."
                } else {
                    self.log.error(&format!("Network error: {m}"));
                    "Network error."
                };
                Err(ApiError::Error(msg.into()))
            }
            Err(e) => {
                Err(ApiError::Error(e.to_string()))
            }
        }
    }

    /// `_multi_artist_credit`: the artist names and ids.
    fn multi_artist_credit(artists: &[J]) -> (Vec<String>, Vec<String>) {
        let names = artists
            .iter()
            .map(|a| {
                a.get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let ids = artists
            .iter()
            .map(|a| match a.get("id") {
                Some(J::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            })
            .collect();
        (names, ids)
    }

    /// `album_for_id`: a Spotify album object (with its track list).
    pub fn album_from_data(&mut self, id: &str, data: &J) -> Option<AlbumInfo> {
        if data.get("name").and_then(|n| n.as_str()) == Some("") {
            self.log
                .debug(&format!("Album removed from Spotify: {id}"));
            return None;
        }
        let artists = data
            .get("artists")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        let (artists_names, artists_ids) = Self::multi_artist_credit(&artists);
        let artist = artists_names.join(", ");
        let date_parts: Vec<i64> = data
            .get("release_date")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .split('-')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let precision = data
            .get("release_date_precision")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();
        let (year, month, day) = match precision.as_str() {
            "day" => (
                date_parts.first().copied().unwrap_or(0),
                date_parts.get(1).copied(),
                date_parts.get(2).copied(),
            ),
            "month" => (date_parts[0], date_parts.get(1).copied(), None),
            "year" => (date_parts[0], None, None),
            _ => {
                self.log.error(&format!(
                    "Invalid `release_date_precision` returned by Spotify API: '{precision}'"
                ));
                return None;
            }
        };
        let tracks = data.get("tracks")?;
        let mut tracks_items = tracks
            .get("items")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        let mut next = tracks.get("next").and_then(|n| n.as_str()).map(String::from);
        while let Some(url) = next {
            let page = match self.handle_response("get", &url, &[]) {
                Ok(p) => p,
                Err(e) => {
                    self.log.error(&format!("{e}"));
                    return None;
                }
            };
            if let Some(items) = page.get("items").and_then(|i| i.as_array()) {
                tracks_items.extend(items.clone());
            }
            next = page.get("next").and_then(|n| n.as_str()).map(String::from);
        }
        let mut out_tracks = vec![];
        let mut medium_totals: Vec<(usize, usize)> = vec![];
        for (i, track_data) in tracks_items.iter().enumerate() {
            let mut track = self.get_track(track_data);
            track.index = i + 1;
            match medium_totals.iter_mut().find(|(m, _)| *m == track.medium) {
                Some((_, n)) => *n += 1,
                None => medium_totals.push((track.medium, 1)),
            }
            out_tracks.push(track);
        }
        for track in &mut out_tracks {
            track.medium_total = medium_totals
                .iter()
                .find(|(m, _)| *m == track.medium)
                .map(|(_, n)| *n)
                .unwrap_or(0);
        }
        let mut extra = vec![("spotify_album_id".to_string(), V::Str(id.to_string()))];
        if let Some(first) = artists_ids.first() {
            extra.push(("spotify_artist_id".to_string(), V::Str(first.clone())));
        }
        let va = artists.len() == 1 && artist.to_lowercase() == "various artists";
        Some(AlbumInfo {
            album: data
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
            album_id: id.to_string(),
            artist,
            artist_id: artists_ids.first().cloned().unwrap_or_default(),
            artists: artists_names,
            artists_ids,
            tracks: out_tracks,
            albumtype: data
                .get("album_type")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            va,
            year,
            month: month.unwrap_or(0),
            day: day.unwrap_or(0),
            label: data
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or_default()
                .to_string(),
            mediums: medium_totals
                .iter()
                .filter(|(m, _)| *m != 0)
                .map(|(m, _)| *m)
                .max()
                .unwrap_or(0),
            data_source: self.data_source().to_string(),
            data_url: data
                .get("external_urls")
                .and_then(|u| u.get("spotify"))
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string(),
            extra,
            ..Default::default()
        })
    }

    /// `_get_track`: a (simplified) Spotify track object as a `TrackInfo`.
    pub fn get_track(&self, data: &J) -> TrackInfo {
        let artists = data
            .get("artists")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        let (names, ids) = Self::multi_artist_credit(&artists);
        let album = data
            .get("album")
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .map(String::from);
        let track_id = data
            .get("id")
            .and_then(|i| i.as_str().map(String::from).or_else(|| Some(i.to_string())))
            .unwrap_or_default();
        let mut extra = vec![("spotify_track_id".to_string(), V::Str(track_id.clone()))];
        if let Some(first) = ids.first() {
            extra.push(("spotify_artist_id".to_string(), V::Str(first.clone())));
        }
        if let Some(album) = &album {
            extra.push(("album".to_string(), V::Str(album.clone())));
        }
        TrackInfo {
            title: data
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
            track_id,
            artist: names.join(", "),
            artist_id: ids.first().cloned().unwrap_or_default(),
            artists: names,
            artists_ids: ids,
            length: data
                .get("duration_ms")
                .and_then(|d| d.as_f64())
                .map(|d| d / 1000.0),
            index: data
                .get("track_number")
                .and_then(|n| n.as_i64())
                .unwrap_or(0) as usize,
            medium: data
                .get("disc_number")
                .and_then(|n| n.as_i64())
                .unwrap_or(0) as usize,
            medium_index: data
                .get("track_number")
                .and_then(|n| n.as_i64())
                .unwrap_or(0) as usize,
            data_source: self.data_source().to_string(),
            data_url: data
                .get("external_urls")
                .and_then(|u| u.get("spotify"))
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string(),
            extra,
            ..Default::default()
        }
    }

    /// `track_for_id`: a track object, with its album's track count and
    /// position.
    pub fn track_from_data(&mut self, data: &J) -> Option<TrackInfo> {
        let mut track = self.get_track(data);
        let album_id = data
            .get("album")
            .and_then(|a| a.get("id"))
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string();
        let album_data = match self.handle_response(
            "get",
            &format!("{}{album_id}", self.conf.urls.album),
            &[],
        ) {
            Ok(a) => a,
            Err(e) => {
                self.log.error(&format!("{e}"));
                return None;
            }
        };
        let mut medium_total = 0;
        let items = album_data
            .get("tracks")
            .and_then(|t| t.get("items"))
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        for (i, t) in items.iter().enumerate() {
            let disc = t.get("disc_number").and_then(|n| n.as_i64()).unwrap_or(0) as usize;
            if disc == track.medium {
                medium_total += 1;
                if t.get("id").and_then(|i| i.as_str()) == Some(track.track_id.as_str()) {
                    track.index = i + 1;
                }
            }
        }
        track.medium_total = medium_total;
        Some(track)
    }

    /// `get_search_query_with_filters`.
    pub fn search_query(&self, query_type: &str, artist: &str, name: &str, va_likely: bool) -> String {
        let mut query = if query_type == "album" {
            format!("album:\"{name}\"")
        } else {
            name.to_string()
        };
        if query_type == "track" || !va_likely {
            query.push_str(&format!(" artist:\"{artist}\""));
        }
        query
    }

    /// `get_search_response`: the search's result items (one token refresh
    /// on 401).
    pub fn search_response(&mut self, query_type: &str, query: &str) -> Result<Vec<J>, ApiError> {
        let mut query = query.to_string();
        if self.conf.search_query_ascii {
            query = deunicode::deunicode(&query);
        }
        if query.is_empty() {
            self.log.debug("Skipping search with empty query and filters");
            return Ok(vec![]);
        }
        let limit = self.conf.search_limit;
        self.log
            .debug(&format!("Searching for '{query}' with {{}}"));
        let url = format_url(
            &self.conf.urls.search,
            &[
                ("q", query.replace('"', "'")),
                ("type", query_type.to_string()),
                ("limit", limit.to_string()),
            ],
        );
        for _ in 0..2 {
            let auth = format!("Bearer {}", self.token());
            match self.get_checked(&url, &[("Authorization", &auth)]) {
                Ok(resp) => {
                    let data = resp.json().unwrap_or(J::Null);
                    let results = data
                        .get(format!("{query_type}s"))
                        .and_then(|v| v.get("items"))
                        .and_then(|i| i.as_array())
                        .cloned()
                        .unwrap_or_default();
                    self.log
                        .debug(&format!("Found {} result(s)", results.len()));
                    return Ok(results);
                }
                Err(HttpError::Status { code: 401, .. }) => {
                    self.authenticate()
                        .map_err(|e| ApiError::Error(format!("{e:#}")))?;
                    continue;
                }
                Err(e) => {
                    self.log.error(&format!("Error searching Spotify: {e}"));
                    return Ok(vec![]);
                }
            }
        }
        Ok(vec![])
    }

    // ------------------------------------------------------- commands

    /// `_parse_opts`: validate the `-m` mode.
    pub fn parse_opts(&mut self, mode: Option<&str>, show_failures: bool) -> bool {
        if let Some(m) = mode {
            self.conf.mode = m.to_string();
        }
        if show_failures {
            self.conf.show_failures = true;
        }
        if !["list", "open"].contains(&self.conf.mode.as_str()) {
            self.log
                .warning(&format!("{} is not a valid mode", self.conf.mode));
            return false;
        }
        true
    }

    /// `_match_library_tracks`: search Spotify for each library item
    /// matching the query; None when the query matches no items.
    pub fn match_library_tracks(
        &mut self,
        cfg: &Config,
        lib: &crate::library::Library,
        keywords: &[String],
    ) -> Result<Option<Vec<J>>> {
        let items = lib.items(&crate::query::Query::parse(Kind::Item, keywords)?)?;
        if items.is_empty() {
            self.log
                .debug("Your beets query returned no items, skipping Spotify.");
            return Ok(None);
        }
        self.log
            .info(&format!("Processing {} tracks...", items.len()));
        let mut results = vec![];
        let mut failures: Vec<String> = vec![];
        for mut item in items {
            for spec in self.conf.regex.clone() {
                if spec.field.is_empty() || spec.search.is_empty() || spec.replace.is_empty() {
                    continue;
                }
                let value = item.str(&spec.field);
                if let Ok(re) = regex::Regex::new(&spec.search) {
                    let new = re.replace_all(value, spec.replace.as_str()).into_owned();
                    item.set_str(&spec.field, new);
                }
            }
            let artist = {
                let a = item.str("artist");
                if a.is_empty() {
                    item.str("albumartist").to_string()
                } else {
                    a.to_string()
                }
            };
            let album = item.str("album").to_string();
            let query_string = item.str("title").to_string();
            let mut query = query_string.clone();
            if !artist.is_empty() {
                query.push_str(&format!(" artist:'{artist}'"));
            }
            if !album.is_empty() {
                query.push_str(&format!(" album:'{album}'"));
            }
            let found = match self.search_response("track", &query) {
                Ok(f) => f,
                Err(e) => {
                    self.log.error(&format!("Error searching Spotify: {e}"));
                    vec![]
                }
            };
            if found.is_empty() {
                failures.push(query);
                continue;
            }
            let mut found: Vec<J> = found;
            if let Some(region) = &self.conf.region_filter {
                found.retain(|t| {
                    t.get("available_markets")
                        .and_then(|m| m.as_array())
                        .is_some_and(|m| m.iter().any(|v| v.as_str() == Some(region.as_str())))
                });
            }
            let chosen = if found.len() == 1 || self.conf.tiebreak == "first" {
                self.log
                    .debug(&format!("Spotify track(s) found, count: {}", found.len()));
                found[0].clone()
            } else {
                self.log
                    .debug(&format!("Most popular track chosen, count: {}", found.len()));
                let mut best = found[0].clone();
                let mut best_pop = popularity(&best);
                for t in &found[1..] {
                    let p = popularity(t);
                    if p > best_pop {
                        best = t.clone();
                        best_pop = p;
                    }
                }
                best
            };
            results.push(chosen);
        }
        let failure_count = failures.len();
        if failure_count > 0 {
            if self.conf.show_failures {
                self.log.info(&format!(
                    "{failure_count} track(s) did not match a Spotify ID:"
                ));
                for track in &failures {
                    self.log.info(&format!("track: {track}"));
                }
                self.log.info("");
            } else {
                self.log.warning(&format!(
                    "{failure_count} track(s) did not match a Spotify ID:\nuse --show-failures to display"
                ));
            }
        }
        let _ = cfg;
        Ok(Some(results))
    }

    /// `_output_match_results`: the track URLs, or a trackset URI (opened).
    pub fn output_match_results(&self, results: &[J]) {
        if results.is_empty() {
            self.log
                .warning("No Spotify tracks found from beets query");
            return;
        }
        let ids: Vec<String> = results
            .iter()
            .map(|t| t.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string())
            .collect();
        if self.conf.mode == "open" {
            self.log
                .info("Attempting to open Spotify with playlist");
            let url = format!("spotify:trackset:Playlist:{}", ids.join(","));
            // beets' `webbrowser.open`; a failure is ignored.
            let _ = std::process::Command::new("xdg-open")
                .arg(&url)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        } else {
            for id in ids {
                println!("{}{id}", self.conf.urls.open_track);
            }
        }
    }

    /// `get_track_details_by_id`: popularity and external ids, in batches of
    /// 50.
    pub fn get_track_details_by_id(&mut self, track_ids: &[String]) -> Result<Vec<(String, J)>, ApiError> {
        let mut out = vec![];
        let url = self.conf.urls.track.clone();
        for chunk in track_ids.chunks(50) {
            let data = self.handle_response("get", &url, &[("ids", chunk.join(","))])?;
            let tracks = data
                .get("tracks")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            for (idx, track) in tracks.iter().enumerate() {
                if track.is_null() {
                    continue;
                }
                let id = track
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| chunk.get(idx).cloned().unwrap_or_default());
                out.push((id, track.clone()));
            }
        }
        Ok(out)
    }

    /// `track_audio_features_batch`: audio features, in batches of 100.
    pub fn track_audio_features_batch(
        &mut self,
        track_ids: &[String],
    ) -> Vec<(String, J)> {
        if !self.audio_features_available {
            return vec![];
        }
        let mut out = vec![];
        for chunk in track_ids.chunks(100) {
            let features_url = self.conf.urls.audio_features.clone();
            let data = match self.handle_response("get", &features_url, &[("ids", chunk.join(","))]) {
                Ok(d) => d,
                Err(ApiError::AudioFeaturesUnavailable) => {
                    self.disable_audio_features();
                    break;
                }
                Err(e) => {
                    self.log.debug(&format!("Spotify API error: {e}"));
                    continue;
                }
            };
            let features = data
                .get("audio_features")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            for (idx, feature) in features.iter().enumerate() {
                if feature.is_null() {
                    continue;
                }
                let id = feature
                    .get("id")
                    .and_then(|i| i.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| chunk.get(idx).cloned().unwrap_or_default());
                out.push((id, feature.clone()));
            }
        }
        out
    }

    fn disable_audio_features(&mut self) {
        if self.audio_features_available {
            self.audio_features_available = false;
            self.log.warning(
                "Audio features API is unavailable (403 error). Skipping audio features for remaining tracks.",
            );
        }
    }

    /// `_fetch_info`: popularity, external ids and audio features for each
    /// item's Spotify track.
    pub fn fetch_info(
        &mut self,
        cfg: &Config,
        lib: &crate::library::Library,
        items: &[Item],
        write: bool,
        force: bool,
    ) -> Result<()> {
        self.log.debug(&format!("Total {} tracks", items.len()));
        let mut to_update: Vec<(usize, String)> = vec![];
        for (index, item) in items.iter().enumerate() {
            self.log.info(&format!(
                "Processing {}/{} tracks - {} ",
                index + 1,
                items.len(),
                format_record(cfg, item)
            ));
            if !force && item.get("spotify_track_popularity").is_some() {
                self.log
                    .debug(&format!("Popularity already present for: {}", format_record(cfg, item)));
                continue;
            }
            let Some(id) = item.get("spotify_track_id").filter(|v| !v.raw().is_empty()) else {
                self.log
                    .debug(&format!("No track_id present for: {}", format_record(cfg, item)));
                continue;
            };
            to_update.push((index, id.raw()));
        }
        if to_update.is_empty() {
            return Ok(());
        }
        let mut unique: Vec<String> = vec![];
        for (_, id) in &to_update {
            if !unique.contains(id) {
                unique.push(id.clone());
            }
        }
        let details = self
            .get_track_details_by_id(&unique)
            .map_err(|e| anyhow!("{e}"))?;
        let features = self.track_audio_features_batch(&unique);
        for (index, spotify_track_id) in &to_update {
            let mut item = items[*index].clone();
            if let Some((_, track)) = details.iter().find(|(id, _)| id == spotify_track_id) {
                let external = track.get("external_ids").cloned().unwrap_or(J::Null);
                if let Some(p) = track.get("popularity").and_then(|p| p.as_i64()) {
                    item.set("spotify_track_popularity", crate::model::Value::Int(p));
                }
                if let Some(isrc) = external.get("isrc").and_then(|v| v.as_str()) {
                    item.set_str("isrc", isrc);
                }
                for key in ["ean", "upc"] {
                    if let Some(v) = external.get(key).and_then(|v| v.as_str()) {
                        item.set_str(key, v);
                    }
                }
            }
            if self.audio_features_available {
                match features.iter().find(|(id, _)| id == spotify_track_id) {
                    None => self
                        .log
                        .info(&format!("No audio features found for: {}", format_record(cfg, &item))),
                    Some((_, feature)) => {
                        for (api_field, item_field) in AUDIO_FEATURES {
                            let Some(value) = feature.get(*api_field) else {
                                continue;
                            };
                            let Some(f) = value.as_f64() else {
                                continue;
                            };
                            // The value's type, as beets' `item_types` take
                            // it (key is a float, mode and time_signature
                            // ints).
                            let v = match item.field_type(item_field) {
                                Some(crate::model::FieldType::Int) => {
                                    crate::model::Value::Int(f as i64)
                                }
                                _ => crate::model::Value::Float(f),
                            };
                            item.set(item_field, v);
                        }
                    }
                }
            }
            item.set("spotify_updated", crate::model::Value::Float(time_time()));
            if write {
                let mut written = item.clone();
                if let Err(e) = crate::media::write_item(&mut written) {
                    crate::logging::emit(format!("{e:#}"));
                }
            }
            lib.store(&item)?;
        }
        Ok(())
    }
}

impl MetadataSource for Spotify {
    fn data_source(&self) -> &'static str {
        "Spotify"
    }

    fn album_for_id(&self, _ctx: &SourceCtx, album_id: &str) -> Result<Option<AlbumInfo>> {
        let Some(id) = extract_id("spotify", album_id) else {
            return Ok(None);
        };
        let mut me = Spotify {
            conf: self.conf.clone(),
            log: PluginLog::new("spotify"),
            access_token: self.access_token.clone(),
            audio_features_available: self.audio_features_available,
            session: Session::plain(USER_AGENT),
            retry_wait: self.retry_wait,
        };
        let data = match me.handle_response("get", &format!("{}{id}", me.conf.urls.album), &[]) {
            Ok(d) => d,
            Err(e) => {
                me.log.error(&format!("{e}"));
                return Ok(None);
            }
        };
        Ok(me.album_from_data(&id, &data))
    }

    fn track_for_id(&self, _ctx: &SourceCtx, track_id: &str) -> Result<Option<TrackInfo>> {
        let Some(id) = extract_id("spotify", track_id) else {
            self.log
                .debug(&format!("Invalid Spotify ID: {track_id}"));
            return Ok(None);
        };
        let mut me = Spotify {
            conf: self.conf.clone(),
            log: PluginLog::new("spotify"),
            access_token: self.access_token.clone(),
            audio_features_available: self.audio_features_available,
            session: Session::plain(USER_AGENT),
            retry_wait: self.retry_wait,
        };
        let data = match me.handle_response("get", &format!("{}/{}", me.conf.urls.track, id), &[]) {
            Ok(d) => d,
            Err(e) => {
                me.log.error(&format!("{e}"));
                return Ok(None);
            }
        };
        Ok(me.track_from_data(&data))
    }

    fn candidates(
        &self,
        _ctx: &SourceCtx,
        _items: &[Item],
        artist: &str,
        album: &str,
        va_likely: bool,
    ) -> Result<Vec<AlbumInfo>> {
        let query = self.search_query("album", artist, album, va_likely);
        let mut me = self.reopen();
        let results = match me.search_response("album", &query) {
            Ok(r) => r,
            Err(e) => {
                me.log.error(&format!("Error searching Spotify: {e}"));
                return Ok(vec![]);
            }
        };
        let mut out = vec![];
        for result in results {
            let Some(id) = result.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            if let Ok(Some(info)) = me.album_for_id(&SourceCtx { cfg: _ctx.cfg, mb: _ctx.mb }, id) {
                out.push(info);
            }
        }
        Ok(out)
    }

    fn item_candidates(
        &self,
        _ctx: &SourceCtx,
        _item: &Item,
        artist: &str,
        title: &str,
    ) -> Result<Vec<TrackInfo>> {
        let query = self.search_query("track", artist, title, false);
        let mut me = self.reopen();
        let results = match me.search_response("track", &query) {
            Ok(r) => r,
            Err(e) => {
                me.log.error(&format!("Error searching Spotify: {e}"));
                return Ok(vec![]);
            }
        };
        let mut out = vec![];
        for result in results {
            let Some(id) = result.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            if let Ok(Some(info)) = me.track_for_id(&SourceCtx { cfg: _ctx.cfg, mb: _ctx.mb }, id) {
                out.push(info);
            }
        }
        Ok(out)
    }
}

impl Spotify {
    /// A copy that requests can mutate (the token may be refreshed).
    fn reopen(&self) -> Spotify {
        Spotify {
            conf: self.conf.clone(),
            log: PluginLog::new("spotify"),
            access_token: self.access_token.clone(),
            audio_features_available: self.audio_features_available,
            session: Session::plain(USER_AGENT),
            retry_wait: self.retry_wait,
        }
    }
}

/// `track["popularity"]` as an integer (missing: 0).
fn popularity(track: &J) -> i64 {
    track.get("popularity").and_then(|p| p.as_i64()).unwrap_or(0)
}

/// `{item}`: the `format_item` template.
fn format_record(cfg: &Config, item: &Item) -> String {
    let t = crate::template::Template::parse(&cfg.str("format_item"));
    crate::commands::format_record(&t, item, None)
}

impl super::Plugin for Spotify {
    fn name(&self) -> &'static str {
        "spotify"
    }
}

// ---------------------------------------------------------------- commands

/// `beet spotify`.
pub fn command(cfg: &Config, lib: &crate::library::Library, opts: CmdOpts) -> Result<()> {
    let mut plugin = Spotify::new(cfg)?;
    if !plugin.parse_opts(opts.mode.as_deref(), opts.show_failures) {
        return Ok(());
    }
    if let Some(results) = plugin.match_library_tracks(cfg, lib, &opts.query)? {
        plugin.output_match_results(&results);
    }
    Ok(())
}

/// `beet spotifysync`.
pub fn sync_command(cfg: &Config, lib: &crate::library::Library, opts: SyncOpts) -> Result<()> {
    let mut plugin = Spotify::new(cfg)?;
    let mut items = lib.items(&crate::query::Query::parse(Kind::Item, &opts.query)?)?;
    if let Some(limit) = opts.limit {
        items.truncate(limit);
    }
    plugin.fetch_info(cfg, lib, &items, cfg.bool("import.write"), opts.force_refetch)
}

#[derive(Debug, Default)]
pub struct CmdOpts {
    pub mode: Option<String>,
    pub show_failures: bool,
    pub query: Vec<String>,
}

#[derive(Debug, Default)]
pub struct SyncOpts {
    pub force_refetch: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

// --------------------------------------------------------------- registry

fn registry_make(cfg: &Config) -> Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Spotify::new(cfg)?))
}

fn metadata_source_make(
    cfg: &Config,
) -> Result<Box<dyn crate::metadata_plugins::MetadataSource>> {
    Ok(Box::new(Spotify::new(cfg)?))
}

inventory::submit! {
    crate::metadata_plugins::MetadataSourceDef {
        // 5.19
        order: 92,
        plugin: "spotify",
        data_source: "Spotify",
        configured: Spotify::is_configured,
        make: metadata_source_make,
    }
}

// `beet spotify`'s options.
#[derive(clap::Args)]
pub struct SpotifyArgs {
    /// "open" to open Spotify with the playlist, "list" to print (default)
    #[arg(short = 'm', long)]
    mode: Option<String>,
    /// List tracks that did not match a Spotify ID
    #[arg(short = 'f', long = "show-failures")]
    show_failures: bool,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_spotify() -> clap::Command {
    use clap::Args as _;
    SpotifyArgs::augment_args(
        clap::Command::new("spotify").about("build a Spotify playlist"),
    )
}

pub fn run_spotify(
    cfg: &Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> Result<()> {
    use clap::FromArgMatches as _;
    let a = SpotifyArgs::from_arg_matches(m)?;
    command(
        cfg,
        lib,
        CmdOpts {
            mode: a.mode,
            show_failures: a.show_failures,
            query: a.query,
        },
    )
}

// `beet spotifysync`'s options (`-f`, `add_limit_option`, a query).
#[derive(clap::Args)]
pub struct SpotifysyncArgs {
    /// Re-download data when already present
    #[arg(short = 'f', long = "force")]
    force_refetch: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_spotifysync() -> clap::Command {
    use clap::Args as _;
    SpotifysyncArgs::augment_args(
        clap::Command::new("spotifysync")
            .about("fetch track attributes from Spotify"),
    )
}

pub fn run_spotifysync(
    cfg: &Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> Result<()> {
    use clap::FromArgMatches as _;
    let a = SpotifysyncArgs::from_arg_matches(m)?;
    sync_command(
        cfg,
        lib,
        SyncOpts {
            force_refetch: a.force_refetch,
            limit: a.limit,
            query: a.query,
        },
    )
}

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "spotify",
        // 5.19
        order: 92,
        defaults: r#"spotify:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    search_query_ascii: no
    mode: list
    tiebreak: popularity
    show_failures: no
    region_filter:
    regex: []
    client_id: 4e414367a1d14c75a5c5129a627fcab8
    client_secret: 4a9b5b7848e54e118a7523b1c7c3e1e5
    tokenfile: spotify_token.json
"#,
        config_keys: &["spotify"],
        redacted: &["spotify.client_id", "spotify.client_secret"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                // 5.19
                    plugin: "spotify",
                build: build_spotify,
                run: run_spotify,
            },
            crate::plugin_registry::PluginCommand {
                // 5.19
                    plugin: "spotify",
                build: build_spotifysync,
                run: run_spotifysync,
            },
        ],
    }
}

#[cfg(test)]
#[path = "spotify_tests.rs"]
mod tests;
