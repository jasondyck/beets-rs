//! A port of beets 2.14's `beatport` plugin (`beetsplug/beatport.py`):
//! release and track search on the (OAuth1-signed) Beatport API as autotag
//! candidates, lookups by Beatport id or URL, `bpm`/`initial_key`/`genres`.
//!
//! beets 2.14 deprecates the plugin (`deprecate_for_user`), which we log
//! the same way. The API's base URL is the plugin's, except in tests
//! (which build the client with [`BeatportClient::with_api_base`] and a
//! fake server).

use super::oauth::{OAuth1, oauth_quote, parse_qsl};
use super::requests::{Session, urlencode};
use super::PluginLog;
use crate::config::Config;
use crate::hooks::V;
use crate::mb::{AlbumInfo, TrackInfo};
use crate::metadata_plugins::{MetadataSource, SourceCtx, extract_id, get_artist, opt_str};
use crate::model::Item;
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use serde_json::Value as J;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The plugin's `USER_AGENT` (`beets/<version> +https://beets.io/`).
const BEATPORT_USER_AGENT: &str = "beets/2.14.1 +https://beets.io/";

/// `BeatportAPIError`.
#[derive(Debug)]
pub struct BeatportAPIError(pub String);

impl std::fmt::Display for BeatportAPIError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BeatportAPIError {}

// ------------------------------------------------------------------ objects

/// `BeatportObject`: the fields every search result has.
#[derive(Debug, Clone, Default)]
pub struct BeatportObject {
    pub beatport_id: String,
    pub name: String,
    pub release_date: Option<NaiveDate>,
    /// `(artist id, artist name)`.
    pub artists: Option<Vec<(String, String)>>,
    pub genres: Vec<String>,
}

impl BeatportObject {
    pub fn from_data(data: &J) -> Self {
        let artists = data.get("artists").and_then(|a| a.as_array()).map(|a| {
            a.iter()
                .map(|x| {
                    (
                        x.get("id")
                            .map(json_id)
                            .unwrap_or_default(),
                        x.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        });
        let names = |key: &str| -> Vec<String> {
            data.get(key)
                .and_then(|v| v.as_array())
                .map(|v| {
                    v.iter()
                        .map(|x| {
                            x.get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut genres = names("subGenres");
        for g in names("genres") {
            if !genres.contains(&g) {
                genres.push(g);
            }
        }
        BeatportObject {
            beatport_id: data.get("id").map(json_id).unwrap_or_default(),
            name: data
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
            release_date: data
                .get("releaseDate")
                .and_then(|d| d.as_str())
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()),
            artists,
            genres,
        }
    }

    pub fn artists_str(&self) -> Option<String> {
        self.artists.as_ref().map(|a| {
            if a.len() < 4 {
                a.iter()
                    .map(|(_, n)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                "Various Artists".into()
            }
        })
    }
}

/// A JSON number or string as Python's `str()` of the value.
fn json_id(v: &J) -> String {
    match v {
        J::Number(n) => n
            .as_i64()
            .map(|i| i.to_string())
            .unwrap_or_else(|| n.to_string()),
        J::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct BeatportRelease {
    pub object: BeatportObject,
    pub catalog_number: Option<String>,
    pub label_name: Option<String>,
    pub category: Option<String>,
    pub url: Option<String>,
    pub tracks: Option<Vec<BeatportTrack>>,
}

impl BeatportRelease {
    pub fn from_data(data: &J) -> Self {
        let opt = |v: Option<&J>| -> Option<String> {
            match v {
                Some(J::String(s)) => Some(s.clone()),
                Some(J::Null) | None => None,
                Some(v) => Some(json_id(v)),
            }
        };
        let url = data.get("slug").and_then(|s| s.as_str()).map(|slug| {
            format!(
                "https://beatport.com/release/{slug}/{}",
                data.get("id").map(json_id).unwrap_or_default()
            )
        });
        BeatportRelease {
            object: BeatportObject::from_data(data),
            catalog_number: opt(data.get("catalogNumber")),
            label_name: data
                .get("label")
                .and_then(|l| l.get("name"))
                .and_then(|n| n.as_str())
                .map(String::from),
            category: opt(data.get("category")),
            url,
            tracks: None,
        }
    }
}

impl std::fmt::Display for BeatportRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<BeatportRelease: {} - {} ({})>",
            self.object.artists_str().unwrap_or_default(),
            self.object.name,
            self.catalog_number.clone().unwrap_or_default()
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct BeatportTrack {
    pub object: BeatportObject,
    pub mix_name: Option<String>,
    /// `length`, in seconds.
    pub length: f64,
    pub url: Option<String>,
    pub track_number: Option<i64>,
    pub bpm: Option<i64>,
    pub initial_key: String,
}

impl BeatportTrack {
    pub fn from_data(data: &J) -> Self {
        let length_ms = data.get("lengthMs").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mut length = length_ms / 1000.0;
        if length == 0.0 {
            // `data.get("length", "0:0").split(":")`
            if let Some((m, s)) = data
                .get("length")
                .and_then(|l| l.as_str())
                .unwrap_or("0:0")
                .split_once(':')
                && let (Ok(m), Ok(s)) = (m.parse::<i64>(), s.parse::<i64>())
            {
                length = (m * 60 + s) as f64;
            }
        }
        BeatportTrack {
            object: BeatportObject::from_data(data),
            mix_name: data.get("mixName").and_then(|m| m.as_str()).map(String::from),
            length,
            url: data.get("slug").and_then(|s| s.as_str()).map(|slug| {
                format!(
                    "https://beatport.com/track/{slug}/{}",
                    data.get("id").map(json_id).unwrap_or_default()
                )
            }),
            track_number: data.get("trackNumber").and_then(|n| n.as_i64()),
            bpm: data.get("bpm").and_then(|b| b.as_i64()),
            // `str((data.get("key") or {}).get("shortName"))`
            initial_key: match data.get("key").and_then(|k| k.get("shortName")) {
                Some(J::String(s)) => s.clone(),
                Some(v) => json_id(v),
                None => "None".to_string(),
            },
        }
    }
}

// ------------------------------------------------------------------- client

pub struct BeatportClient {
    pub api: OAuth1,
    pub api_base: String,
    session: Session,
}

impl BeatportClient {
    pub fn with_api_base(consumer_key: &str, consumer_secret: &str, base: &str) -> Self {
        BeatportClient {
            api: OAuth1::new(consumer_key, consumer_secret),
            api_base: base.to_string(),
            session: Session::plain(BEATPORT_USER_AGENT),
        }
    }

    pub fn with_token(mut self, token: &str, secret: &str) -> Self {
        self.api = self.api.with_token(token, secret);
        self
    }

    fn make_url(&self, endpoint: &str) -> String {
        if endpoint.starts_with('/') {
            format!("{}{endpoint}", self.api_base)
        } else {
            format!("{}/{}", self.api_base, endpoint)
        }
    }

    fn auth_header(&self, method: &str, url: &str, params: &[(String, String)]) -> String {
        self.api.header(method, url, params, None)
    }

    /// `get_authorize_url`: request a token, then the authorization URL.
    pub fn get_authorize_url(&mut self) -> Result<String> {
        let url = self.make_url("/identity/1/oauth/request-token");
        let header = self.api.header("POST", &url, &[], Some("oob"));
        let resp = self
            .session
            .request(
                "POST",
                &url,
                super::requests::Request {
                    headers: &[("Authorization", &header)],
                    ..Default::default()
                },
            )
            .map_err(|e| BeatportAPIError(e.to_string()))?;
        let pairs = parse_qsl(&resp.text(false));
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        self.api.token = get("oauth_token");
        self.api.token_secret = get("oauth_token_secret");
        let mut auth_url = self.make_url("/identity/1/oauth/authorize");
        if let Some(t) = &self.api.token {
            auth_url.push_str(&format!("?oauth_token={}", oauth_quote(t)));
        }
        Ok(auth_url)
    }

    /// `get_access_token`: the verifier data from the authorization page.
    pub fn get_access_token(&mut self, auth_data: &str) -> Result<(String, String)> {
        let mut params = parse_qsl(auth_data);
        if let Some(i) = params
            .iter()
            .position(|(k, _)| k == "oauth_token" || k == "oauth_verifier")
        {
            params.remove(i);
        }
        let url = self.make_url("/identity/1/oauth/access-token");
        let header = self.api.header("POST", &url, &params, None);
        let resp = self
            .session
            .request(
                "POST",
                &url,
                super::requests::Request {
                    headers: &[("Authorization", &header)],
                    ..Default::default()
                },
            )
            .map_err(|e| BeatportAPIError(e.to_string()))?;
        let pairs = parse_qsl(&resp.text(false));
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        Ok((get("oauth_token"), get("oauth_token_secret")))
    }

    /// `_get`: a GET request's `results` list; a non-2xx status is an
    /// error (`Error {status} for '{path}'`).
    pub fn get(&self, endpoint: &str, params: &[(String, String)]) -> Result<Vec<J>> {
        let url = self.make_url(endpoint);
        let header = self.auth_header("GET", &url, params);
        let owned: Vec<(&str, String)> =
            params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let full = if params.is_empty() {
            url.clone()
        } else {
            format!("{url}?{}", urlencode(&owned))
        };
        let resp = self
            .session
            .get(
                &full,
                super::requests::Request {
                    headers: &[("Authorization", &header)],
                    ..Default::default()
                },
            )
            .map_err(|e| BeatportAPIError(format!("Error connecting to Beatport API: {e}")))?;
        Ok(resp
            .json()
            .map_err(|e| BeatportAPIError(format!("Error connecting to Beatport API: {e}")))?
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// `search`: the matching releases (with track lists when `details`) or
    /// tracks.
    pub fn search(&self, query: &str, release_type: &str) -> Result<Vec<SearchResult>> {
        let results = self.get(
            "catalog/3/search",
            &[
                ("query".to_string(), query.to_string()),
                ("perPage".to_string(), "5".to_string()),
                ("facets".to_string(), format!("fieldType:{release_type}")),
            ],
        )?;
        let mut out = vec![];
        for item in &results {
            if release_type == "release" {
                let mut release = BeatportRelease::from_data(item);
                let id = release.object.beatport_id.clone();
                release.tracks = Some(self.get_release_tracks(&id)?);
                out.push(SearchResult::Release(release));
            } else {
                out.push(SearchResult::Track(BeatportTrack::from_data(item)));
            }
        }
        Ok(out)
    }

    pub fn get_release(&self, beatport_id: &str) -> Result<Option<BeatportRelease>> {
        let response = self.get(
            "/catalog/3/releases",
            &[("id".to_string(), beatport_id.to_string())],
        )?;
        let Some(item) = response.first() else {
            return Ok(None);
        };
        let mut release = BeatportRelease::from_data(item);
        release.tracks = Some(self.get_release_tracks(beatport_id)?);
        Ok(Some(release))
    }

    pub fn get_release_tracks(&self, beatport_id: &str) -> Result<Vec<BeatportTrack>> {
        let response = self.get(
            "/catalog/3/tracks",
            &[
                ("releaseId".to_string(), beatport_id.to_string()),
                ("perPage".to_string(), "100".to_string()),
            ],
        )?;
        Ok(response.iter().map(BeatportTrack::from_data).collect())
    }

    pub fn get_track(&self, beatport_id: &str) -> Result<Option<BeatportTrack>> {
        let response = self.get(
            "/catalog/3/tracks",
            &[("id".to_string(), beatport_id.to_string())],
        )?;
        Ok(response.first().map(BeatportTrack::from_data))
    }
}

#[derive(Debug, Clone)]
pub enum SearchResult {
    Release(BeatportRelease),
    Track(BeatportTrack),
}

// ---------------------------------------------------------------- the plugin

#[derive(Debug, Clone)]
pub struct Urls {
    pub api_base: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            api_base: std::env::var("BEETSRS_BEATPORT_API_URL")
                .unwrap_or_else(|_| "https://oauth-api.beatport.com".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Conf {
    pub urls: Urls,
    pub apikey: String,
    pub apisecret: String,
    pub tokenfile: PathBuf,
}

pub struct Beatport {
    pub conf: Conf,
    pub log: PluginLog,
    pub client: Option<BeatportClient>,
}

impl Beatport {
    /// Whether the plugin has a token to use: beets' plugin loader logs in
    /// and saves one when its setup runs, so without one the plugin isn't
    /// loaded (and `metadata_plugins::sources` doesn't try again).
    pub fn is_configured(cfg: &Config) -> bool {
        cfg.config_dir.join(cfg.str("beatport.tokenfile")).exists()
    }

    /// `setup`: the OAuth token from the token file, or the interactive
    /// login.
    pub fn new(cfg: &Config) -> Result<Self> {
        // `deprecate_for_user`, once per process (beets logs it when the
        // plugin loads).
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            PluginLog::new("beatport").warning(&deprecation_message());
        }
        let conf = Conf {
            urls: Urls::default(),
            apikey: cfg.str("beatport.apikey"),
            apisecret: cfg.str("beatport.apisecret"),
            // confuse's `Filename(in_app_dir=True)`.
            tokenfile: cfg.config_dir.join(cfg.str("beatport.tokenfile")),
        };
        let mut plugin = Self::with(conf);
        plugin.setup()?;
        Ok(plugin)
    }

    pub fn with(conf: Conf) -> Self {
        Beatport {
            conf,
            log: PluginLog::new("beatport"),
            client: None,
        }
    }

    pub fn setup(&mut self) -> Result<()> {
        let (token, secret) = match std::fs::read_to_string(&self.conf.tokenfile) {
            Ok(text) => {
                let data: J = serde_json::from_str(&text)
                    .map_err(|e| anyhow!("{} could not be read: {e}", self.conf.tokenfile.display()))?;
                (
                    data.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    data.get("secret").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                )
            }
            Err(_) => self.authenticate()?,
        };
        self.client = Some(
            BeatportClient::with_api_base(
                &self.conf.apikey,
                &self.conf.apisecret,
                &self.conf.urls.api_base,
            )
            .with_token(&token, &secret),
        );
        Ok(())
    }

    fn authenticate(&self) -> Result<(String, String)> {
        let mut auth_client = BeatportClient::with_api_base(
            &self.conf.apikey,
            &self.conf.apisecret,
            &self.conf.urls.api_base,
        );
        let url = auth_client
            .get_authorize_url()
            .map_err(|e| anyhow!("communication with Beatport failed: {e}"))?;
        println!("To authenticate with Beatport, visit:");
        println!("{url}");
        let data = match crate::ui::read_line_or_eof("Enter the string displayed in your browser:") {
            Some(d) => d,
            None => bail!("Beatport token request failed"),
        };
        let (token, secret) = auth_client
            .get_access_token(&data)
            .map_err(|e| anyhow!("Beatport token request failed: {e}"))?;
        self.log
            .debug(&format!("Beatport token {token}, secret {secret}"));
        if let Some(dir) = self.conf.tokenfile.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(
            &self.conf.tokenfile,
            format!("{{\"token\": {token:?}, \"secret\": {secret:?}}}"),
        )
        .map_err(|e| {
            anyhow!(
                "{} could not be written: {e}",
                self.conf.tokenfile.display()
            )
        })?;
        Ok((token, secret))
    }

    fn client(&self) -> Result<&BeatportClient> {
        self.client
            .as_ref()
            .ok_or_else(|| anyhow!("Beatport client not initialized. Call setup() first."))
    }

    /// `_get_artist`: the joined artist names and the first id.
    fn get_artists(&self, artists: Option<&Vec<(String, String)>>) -> (String, Option<String>) {
        let triples: Vec<(&str, &str, Option<&str>)> = artists
            .map(|a| {
                a.iter()
                    .map(|(id, name)| (id.as_str(), name.as_str(), None))
                    .collect()
            })
            .unwrap_or_default();
        get_artist(&triples, false)
    }

    /// `_get_album_info`.
    pub fn album_info(&self, release: &BeatportRelease) -> AlbumInfo {
        let va = release.object.artists.as_ref().is_some_and(|a| a.len() > 3);
        let (mut artist, artist_id) = self.get_artists(release.object.artists.as_ref());
        if va {
            artist = "Various Artists".to_string();
        }
        let tracks: Vec<TrackInfo> = release
            .tracks
            .as_ref()
            .map(|t| t.iter().map(|x| self.track_info(x)).collect())
            .unwrap_or_default();
        let mut nulls = BTreeSet::new();
        let mut extra = vec![(
            "beatport_album_id".to_string(),
            V::Str(release.object.beatport_id.clone()),
        )];
        if !release.object.genres.is_empty() {
            extra.push(("genres".to_string(), V::List(release.object.genres.clone())));
        }
        let date = release.object.release_date;
        AlbumInfo {
            album: release.object.name.clone(),
            album_id: release.object.beatport_id.clone(),
            artist,
            artist_id: artist_id.unwrap_or_default(),
            tracks,
            albumtype: release.category.clone().unwrap_or_default(),
            va,
            label: opt_str(&mut nulls, "label", release.label_name.clone()),
            catalognum: opt_str(&mut nulls, "catalognum", release.catalog_number.clone()),
            media: "Digital".to_string(),
            data_source: self.data_source().to_string(),
            data_url: release.url.clone().unwrap_or_default(),
            year: date.map(|d| chrono::Datelike::year(&d) as i64).unwrap_or(0),
            month: date.map(|d| chrono::Datelike::month(&d) as i64).unwrap_or(0),
            day: date.map(|d| chrono::Datelike::day(&d) as i64).unwrap_or(0),
            nulls,
            extra,
            ..Default::default()
        }
    }

    /// `_get_track_info`.
    pub fn track_info(&self, track: &BeatportTrack) -> TrackInfo {
        // `title = track.name`, then the mix name unless it's the
        // original.
        let mut title = track.object.name.clone();
        if let Some(mix) = &track.mix_name
            && mix != "Original Mix"
        {
            title.push_str(&format!(" ({mix})"));
        }
        let (artist, artist_id) = self.get_artists(track.object.artists.as_ref());
        let mut extra = vec![("initial_key".to_string(), V::Str(track.initial_key.clone()))];
        if let Some(bpm) = track.bpm {
            extra.push(("bpm".to_string(), V::Int(bpm)));
        }
        if !track.object.genres.is_empty() {
            extra.push(("genres".to_string(), V::List(track.object.genres.clone())));
        }
        TrackInfo {
            title,
            track_id: track.object.beatport_id.clone(),
            artist,
            artist_id: artist_id.unwrap_or_default(),
            length: Some(track.length),
            index: track.track_number.unwrap_or(0) as usize,
            medium_index: track.track_number.unwrap_or(0) as usize,
            data_source: self.data_source().to_string(),
            data_url: track.url.clone().unwrap_or_default(),
            extra,
            ..Default::default()
        }
    }
}

/// Strip non-word characters and medium information from a query, as the
/// plugin does before searching.
fn normalize_query(query: &str) -> String {
    let q = regex::Regex::new(r"\W+").unwrap();
    let q = q.replace_all(query, " ");
    let re = regex::Regex::new(r"(?i)\b(CD|disc)\s*\d+").unwrap();
    re.replace_all(&q, "").into_owned()
}

impl MetadataSource for Beatport {
    fn data_source(&self) -> &'static str {
        "Beatport"
    }

    /// `album_for_id`: the release lookup by id.
    fn album_for_id(&self, _ctx: &SourceCtx, album_id: &str) -> Result<Option<AlbumInfo>> {
        self.log.debug(&format!("Searching for release {album_id}"));
        let Some(release_id) = extract_id("beatport", album_id) else {
            self.log.debug("Not a valid Beatport release ID.");
            return Ok(None);
        };
        let client = self.client()?;
        match client.get_release(&release_id) {
            Ok(Some(release)) => Ok(Some(self.album_info(&release))),
            Ok(None) => Ok(None),
            Err(e) => {
                self.log.debug(&format!("API Error: {e} (query: {album_id})"));
                Ok(None)
            }
        }
    }

    /// `track_for_id`: the track lookup by id from a URL or a bare number.
    fn track_for_id(&self, _ctx: &SourceCtx, track_id: &str) -> Result<Option<TrackInfo>> {
        self.log.debug(&format!("Searching for track {track_id}"));
        let re = regex::Regex::new(r"(^|beatport\.com/track/.+/)(\d+)$").unwrap();
        let Some(caps) = re.captures(track_id) else {
            self.log.debug("Not a valid Beatport track ID.");
            return Ok(None);
        };
        let client = self.client()?;
        match client.get_track(&caps[2]) {
            Ok(Some(track)) => Ok(Some(self.track_info(&track))),
            Ok(None) => Ok(None),
            Err(e) => {
                self.log.debug(&format!("API Error: {e} (query: {track_id})"));
                Ok(None)
            }
        }
    }

    /// `candidates`: a release search, its API errors only logged.
    fn candidates(
        &self,
        _ctx: &SourceCtx,
        _items: &[Item],
        artist: &str,
        album: &str,
        va_likely: bool,
    ) -> Result<Vec<AlbumInfo>> {
        let query = if va_likely {
            album.to_string()
        } else {
            format!("{artist} {album}")
        };
        let query = normalize_query(&query);
        let client = self.client()?;
        let results = match client.search(&query, "release") {
            Ok(r) => r,
            Err(e) => {
                self.log.debug(&format!("API Error: {e} (query: {query})"));
                return Ok(vec![]);
            }
        };
        Ok(results
            .into_iter()
            .filter_map(|r| match r {
                SearchResult::Release(release) => Some(self.album_info(&release)),
                SearchResult::Track(_) => None,
            })
            .collect())
    }

    /// `item_candidates`: a track search.
    fn item_candidates(
        &self,
        _ctx: &SourceCtx,
        _item: &Item,
        artist: &str,
        title: &str,
    ) -> Result<Vec<TrackInfo>> {
        let query = format!("{artist} {title}");
        let client = self.client()?;
        let results = match client.search(&query, "track") {
            Ok(r) => r,
            Err(e) => {
                self.log.debug(&format!("API Error: {e} (query: {query})"));
                return Ok(vec![]);
            }
        };
        Ok(results
            .into_iter()
            .filter_map(|r| match r {
                SearchResult::Track(t) => Some(self.track_info(&t)),
                SearchResult::Release(_) => None,
            })
            .collect())
    }
}

impl super::Plugin for Beatport {
    fn name(&self) -> &'static str {
        "beatport"
    }
}

/// `deprecate_for_user`'s message.
pub fn deprecation_message() -> String {
    "The 'beatport' plugin is deprecated and will be removed in version 3.0.0.".to_string()
}

// --------------------------------------------------------------- registry

fn registry_make(cfg: &Config) -> Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Beatport::new(cfg)?))
}

fn metadata_source_make(
    cfg: &Config,
) -> Result<Box<dyn crate::metadata_plugins::MetadataSource>> {
    Ok(Box::new(Beatport::new(cfg)?))
}

inventory::submit! {
    crate::metadata_plugins::MetadataSourceDef {
        // 5.19
        order: 89,
        plugin: "beatport",
        data_source: "Beatport",
        configured: Beatport::is_configured,
        make: metadata_source_make,
    }
}

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "beatport",
        // 5.19
        order: 89,
        defaults: r#"beatport:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    apikey: 57713c3906af6f5def151b33601389176b37b429
    apisecret: b3fe08c93c80aefd749fe871a16cd2bb32e2b954
    tokenfile: beatport_token.json
"#,
        config_keys: &["beatport"],
        item_types: &[],
        album_types: &[],
        redacted: &["beatport.apikey", "beatport.apisecret"],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

#[cfg(test)]
#[path = "beatport_tests.rs"]
mod tests;
