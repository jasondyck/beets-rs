//! The Last.fm client, a port of beets 2.14's `beetsplug/lastgenre/client.py`
//! together with the parts of pylast it uses: `LastFMNetwork` with beets'
//! API key, `get_album`/`get_artist`/`get_track` and `get_top_tags`, i.e.
//! an unauthenticated POST of `artist`, `album`/`track`, `api_key` and
//! `method` (`album.getTopTags` etc.) to `https://ws.audioscrobbler.com/2.0/`,
//! answered with Last.fm's XML.

use super::{AliasPatterns, IgnorePatterns, is_ignored, normalize_genre};
use crate::plugins::PluginLog;
use crate::plugins::requests::{HttpError, Request, Session, quote_plus};
use crate::template::py_repr;
use std::cell::RefCell;
use std::collections::HashMap;

/// beets' `plugins.LASTFM_KEY`.
pub const LASTFM_KEY: &str = "2dc3914abf35f0d9c92d97d8f8e42b43";

/// pylast's `ws_server` for `LastFMNetwork`.
pub const LASTFM_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

/// The kinds of lookup (beets' `FETCH_METHODS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Artist,
    AlbumArtist,
}

impl Kind {
    /// The name beets uses for the lookup (in cache keys and logs).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Artist => "artist",
            Kind::AlbumArtist => "album_artist",
        }
    }

    /// The arguments beets derives from the object.
    pub fn args(self, obj: &crate::model::Record) -> Vec<String> {
        let s = |f: &str| obj.str(f).to_string();
        match self {
            Kind::Track => vec![s("artist"), s("title")],
            Kind::Album => vec![s("albumartist"), s("album")],
            Kind::Artist => vec![s("artist")],
            Kind::AlbumArtist => vec![s("albumartist")],
        }
    }

    /// pylast's API method and the parameter names of the arguments.
    fn method(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Kind::Track => ("track.getTopTags", &["artist", "track"]),
            Kind::Album => ("album.getTopTags", &["artist", "album"]),
            Kind::Artist | Kind::AlbumArtist => ("artist.getTopTags", &["artist"]),
        }
    }
}

/// pylast's errors as `fetch_genres` tells them apart.
#[derive(Debug, PartialEq)]
pub enum LastFmError {
    /// `WSError`, `MalformedResponseError` or `NetworkError`, as `str()`.
    Pylast(String),
    /// Anything else ("error in pylast library").
    Other(String),
}

/// A `TopItem`: a tag name and its weight.
#[derive(Debug, PartialEq)]
pub struct TopTag {
    pub name: String,
    pub weight: f64,
}

pub struct LastFmClient {
    pub api_url: String,
    pub api_key: String,
    pub min_weight: i64,
    pub ignore_patterns: IgnorePatterns,
    pub alias_patterns: AliasPatterns,
    /// Keys are `entity.arg1-arg2-...`; values the lowercase tags.
    pub genre_cache: RefCell<HashMap<String, Vec<String>>>,
    session: Session,
}

/// pylast's `_number`: an int, else a float, else 0.
fn py_number(s: &str) -> f64 {
    if let Ok(i) = s.parse::<i64>() {
        return i as f64;
    }
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .unwrap_or(0.0)
}

/// pylast's `_extract`: the stripped text of the first `name` element
/// under `node`, if it has any.
fn extract(node: roxmltree::Node, name: &str) -> Option<String> {
    let n = node.descendants().find(|n| n.has_tag_name(name))?;
    let first = n.first_child()?;
    let text: String = if first.is_text() {
        first.text().unwrap_or_default().to_string()
    } else {
        return None;
    };
    Some(text.trim().to_string())
}

/// pylast's `_check_response_for_errors` and `get_top_tags`' parsing.
pub fn parse_top_tags(body: &str) -> Result<Vec<TopTag>, LastFmError> {
    let text = body.replace("opensearch:", "");
    let doc = roxmltree::Document::parse(&text).map_err(|e| {
        LastFmError::Pylast(format!(
            "Malformed response from Last.fm. Underlying error: {e}"
        ))
    })?;
    let Some(lfm) = doc.descendants().find(|n| n.has_tag_name("lfm")) else {
        return Err(LastFmError::Other("list index out of range".into()));
    };
    if lfm.attribute("status").unwrap_or_default() != "ok" {
        let Some(e) = doc.descendants().find(|n| n.has_tag_name("error")) else {
            return Err(LastFmError::Other("list index out of range".into()));
        };
        let details = e
            .first_child()
            .and_then(|c| c.text())
            .unwrap_or_default()
            .trim()
            .to_string();
        return Err(LastFmError::Pylast(details));
    }
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("tag"))
        .filter_map(|t| {
            Some(TopTag {
                name: extract(t, "name")?,
                weight: extract(t, "count").map_or(0.0, |c| py_number(&c)),
            })
        })
        .collect())
}

impl LastFmClient {
    pub fn new(
        min_weight: i64,
        ignore_patterns: IgnorePatterns,
        alias_patterns: AliasPatterns,
    ) -> Self {
        LastFmClient {
            api_url: LASTFM_API_URL.to_string(),
            api_key: LASTFM_KEY.to_string(),
            min_weight,
            ignore_patterns,
            alias_patterns,
            genre_cache: RefCell::new(HashMap::new()),
            // pylast's own HTTP client: no retries and no rate limit.
            session: Session::plain(crate::plugins::USER_AGENT),
        }
    }

    /// pylast's `_Request.execute` for `get_top_tags`.
    pub fn get_top_tags(&self, kind: Kind, args: &[String]) -> Result<Vec<TopTag>, LastFmError> {
        let (method, names) = kind.method();
        let mut params: Vec<(&str, &str)> = names
            .iter()
            .zip(args)
            .map(|(n, a)| (*n, a.as_str()))
            .collect();
        params.push(("api_key", &self.api_key));
        params.push(("method", method));
        let data = params
            .iter()
            .map(|(k, v)| format!("{k}={}", quote_plus(v)))
            .collect::<Vec<_>>()
            .join("&");
        let resp = self
            .session
            .request(
                "POST",
                &self.api_url,
                Request {
                    headers: &[("Accept-Charset", "utf-8")],
                    form: Some(&data),
                    allow_redirects: true,
                    ..Default::default()
                },
            )
            .map_err(|e| match e {
                HttpError::Request(m) => LastFmError::Pylast(format!("NetworkError: {m}")),
                other => LastFmError::Pylast(format!("NetworkError: {other}")),
            })?;
        if [500, 502, 503, 504].contains(&resp.status) {
            return Err(LastFmError::Pylast(format!(
                "Connection to the API failed with HTTP code {}",
                resp.status
            )));
        }
        parse_top_tags(&resp.text(true))
    }

    /// beets' `fetch_genres`: the entity's top tags (lowercase) of at
    /// least `min_weight`; errors are logged and give none.
    pub fn fetch_genres(&self, log: &PluginLog, kind: Kind, args: &[String]) -> Vec<String> {
        let res = match self.get_top_tags(kind, args) {
            Ok(r) => r,
            Err(LastFmError::Pylast(e)) => {
                log.debug(&format!("last.fm error: {e}"));
                return vec![];
            }
            Err(LastFmError::Other(e)) => {
                log.error(&format!("error in pylast library: {e}"));
                return vec![];
            }
        };
        filter_weight(res, self.min_weight)
    }

    /// beets' `_last_lookup`: the (cached) genres of the entity, aliased,
    /// without ignored ones. `\u{2010}` (hyphen) becomes "-" for the lookup.
    pub fn last_lookup(&self, log: &PluginLog, kind: Kind, args: &[String]) -> Vec<String> {
        self.last_lookup_with(log, kind, args, |a| self.fetch_genres(log, kind, a))
    }

    pub fn last_lookup_with(
        &self,
        log: &PluginLog,
        kind: Kind,
        args: &[String],
        fetch: impl FnOnce(&[String]) -> Vec<String>,
    ) -> Vec<String> {
        if args.iter().any(String::is_empty) {
            return vec![];
        }
        let replaced: Vec<String> = args.iter().map(|a| a.replace('\u{2010}', "-")).collect();
        let key = format!("{}.{}", kind.name(), replaced.join("-"));
        let cached = self.genre_cache.borrow().get(&key).cloned();
        let genres = match cached {
            Some(g) => g,
            None => {
                let g = fetch(&replaced);
                self.genre_cache.borrow_mut().insert(key, g.clone());
                g
            }
        };
        super::extra_debug(
            log,
            &format!(
                "last.fm (unfiltered) {} tags: {}",
                kind.name(),
                py_list(&genres)
            ),
        );
        genres
            .iter()
            .map(|g| normalize_genre(log, &self.alias_patterns, g))
            .filter(|n| !n.is_empty() && !is_ignored(log, &self.ignore_patterns, n, &args[0]))
            .collect()
    }

    /// beets' `fetch`: `args` if given, else the object's.
    pub fn fetch(
        &self,
        log: &PluginLog,
        kind: Kind,
        obj: &crate::model::Record,
        args: &[String],
    ) -> Vec<String> {
        let args = if args.is_empty() {
            kind.args(obj)
        } else {
            args.to_vec()
        };
        self.last_lookup(log, kind, &args)
    }
}

/// The tags' names, lowercase, without those under `min_weight` (if set).
pub fn filter_weight(tags: Vec<TopTag>, min_weight: i64) -> Vec<String> {
    tags.into_iter()
        .filter(|t| min_weight == 0 || t.weight.trunc() as i64 >= min_weight)
        .map(|t| t.name.to_lowercase())
        .collect()
}

/// A Python list of strings' `repr`.
pub fn py_list<S: AsRef<str>>(v: &[S]) -> String {
    let parts: Vec<String> = v.iter().map(|s| py_repr(s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}
