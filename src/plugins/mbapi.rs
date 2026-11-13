//! A port of beets' `beetsplug/_utils/musicbrainz.py` `MusicBrainzAPI`, as
//! far as the plugins here need it: JSON requests to the MusicBrainz WS/2
//! API with its rate limit, the `client` parameter, optional HTTP digest
//! authentication (`musicbrainz.user`/`musicbrainz.pass`, which mbcollection
//! needs), and the response normalization that turns hyphenated MusicBrainz
//! keys into underscores and groups `relations` by target type.
//!
//! Unlike `crate::mb` (the autotagger's client), this one is not tied to the
//! metadata source plugin: the collection and ListenBrainz plugins use it
//! whenever they are enabled.

use super::requests::{HttpError, Request, Response, Session, format_url};
use crate::config::Config;
use serde_json::{Map, Value as J};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// beets' `__version__`, which the `client` parameter carries.
pub const BEETS_VERSION: &str = "2.14.1";

/// `f"beets-{beets.__version__}"`.
pub fn client_name() -> String {
    format!("beets-{BEETS_VERSION}")
}

pub struct MusicBrainzApi {
    /// `<host>/ws/2`.
    pub api_root: String,
    session: Session,
    /// `requests_ratelimiter`'s `per_second`, as a minimum interval.
    interval: Duration,
    last: Mutex<Option<Instant>>,
    /// The `musicbrainz.user` and `musicbrainz.pass` for digest auth.
    auth: Option<(String, String)>,
}

#[allow(dead_code)] // the full ported API; the plugins here use a subset
impl MusicBrainzApi {
    /// `MusicBrainzAPI.__post_init__`: the musicbrainz settings, read at
    /// construction time. Digest authentication is used when both
    /// `musicbrainz.user` and `musicbrainz.pass` are set.
    pub fn new(cfg: &Config) -> Self {
        let host = cfg.str("musicbrainz.host");
        let (api_host, interval) = if host == "musicbrainz.org" {
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
        let user = cfg.str("musicbrainz.user");
        let pass = cfg.str("musicbrainz.pass");
        MusicBrainzApi {
            api_root: format!("{api_host}/ws/2"),
            session: Session::new(),
            interval,
            last: Mutex::new(None),
            auth: (!user.is_empty()).then_some((user, pass)),
        }
    }

    fn throttle(&self) {
        let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < self.interval {
                std::thread::sleep(self.interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    /// One request, with `fmt=json` (and, authenticated, `client` added),
    /// retried once with digest credentials when the server asks for them.
    fn send(
        &self,
        method: &str,
        url: &str,
        params: &[(&str, String)],
        auth: bool,
    ) -> Result<Response, HttpError> {
        let mut params = params.to_vec();
        params.push(("fmt", "json".to_string()));
        if auth {
            params.push(("client", client_name()));
        }
        let url = format_url(url, &params);
        self.throttle();
        let result = self.session.request(method, &url, Request::default());
        if let (
            Err(HttpError::Status {
                code: 401, headers, ..
            }),
            Some((user, pass)),
        ) = (&result, &self.auth)
        {
            let challenge = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("www-authenticate"))
                .map(|(_, v)| v.as_str())
                .filter(|v| v.starts_with("Digest "));
            // requests' digest auth signs the path and query, not the
            // absolute URL.
            let uri = match url.split_once("://") {
                Some((_, rest)) => match rest.find('/') {
                    Some(i) => &rest[i..],
                    None => "/",
                },
                None => url.as_str(),
            };
            if let Some(challenge) = challenge
                && let Some(header) = digest_authorization(method, uri, challenge, user, pass)
            {
                self.throttle();
                let request = Request {
                    headers: &[("Authorization", &header)],
                    ..Default::default()
                };
                return self.session.request(method, &url, request);
            }
        }
        result
    }

    /// beets' `RequestHandler.request`: an HTTP error status is an error,
    /// a 401 carries MusicBrainz' authentication hint.
    fn request(
        &self,
        method: &str,
        url: &str,
        params: &[(&str, String)],
    ) -> Result<Response, HttpError> {
        self.send(method, url, params, true).map_err(|e| match e {
            HttpError::Status { code: 401, .. } => HttpError::Other(
                "HTTP Error: 401 Unauthorized. Check your musicbrainz.user \
                 and musicbrainz.pass configuration"
                    .into(),
            ),
            other => other,
        })
    }

    /// `get_json`: the normalized response body.
    pub fn get_json(&self, url: &str, params: &[(&str, String)]) -> Result<J, HttpError> {
        let resp = self.request("GET", url, params)?;
        resp.json().map(|v| normalize(&v))
    }

    pub fn put(&self, url: &str) -> Result<Response, HttpError> {
        self.request("PUT", url, &[])
    }

    pub fn delete(&self, url: &str) -> Result<Response, HttpError> {
        self.request("DELETE", url, &[])
    }

    /// `_get_resource`: an endpoint under `api_root`, with `inc` includes.
    pub fn get_resource(
        &self,
        resource: &str,
        includes: &[&str],
        params: &[(&str, String)],
    ) -> Result<J, HttpError> {
        let mut params = params.to_vec();
        if !includes.is_empty() {
            params.push(("inc", includes.join("+")));
        }
        self.get_json(&format!("{}/{}", self.api_root, resource), &params)
    }

    /// `_lookup`.
    pub fn lookup(&self, entity: &str, id: &str, includes: &[&str]) -> Result<J, HttpError> {
        self.get_resource(&format!("{entity}/{id}"), includes, &[])
    }

    /// `_browse`: the `<entity>s` list of a browse response.
    pub fn browse(
        &self,
        entity: &str,
        includes: &[&str],
        params: &[(&str, String)],
    ) -> Result<Vec<J>, HttpError> {
        let normalised = entity.replace('-', "_");
        let data = self.get_resource(entity, includes, params)?;
        Ok(match data.get(format!("{normalised}s")) {
            Some(J::Array(a)) => a.clone(),
            _ => vec![],
        })
    }

    /// `search`: the entities matching `filters`, each formatted as
    /// `field:(term)` (`term` alone for an empty field), lower-cased,
    /// stripped and Lucene-escaped, joined with spaces.
    pub fn search(&self, entity: &str, filters: &[(&str, String)]) -> Result<Vec<J>, HttpError> {
        let query = search_query(filters);
        crate::logging::debug!(
            "Searching for MusicBrainz {entity}s with: {}",
            crate::hooks::py_repr(&query)
        );
        let normalised = entity.replace('-', "_");
        let data = self.get_resource(entity, &[], &[("query", query)])?;
        Ok(match data.get(format!("{normalised}s")) {
            Some(J::Array(a)) => a.clone(),
            _ => vec![],
        })
    }

    /// `get_base_recording_with_releases`.
    pub fn recording_with_releases(&self, id: &str) -> Result<J, HttpError> {
        self.lookup("recording", id, &["releases"])
    }
}

/// `MusicBrainzAPI.search`'s query: each filter as `field:(term)` (the
/// term alone for an empty field), lower-cased, stripped and escaped;
/// empty terms are left out.
pub fn search_query(filters: &[(&str, String)]) -> String {
    filters
        .iter()
        .filter_map(|(field, term)| {
            let term = term.to_lowercase();
            let term = term.trim();
            if term.is_empty() {
                return None;
            }
            let term = crate::mb::escape(term);
            Some(if field.is_empty() {
                term
            } else {
                format!("{field}:({term})")
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `MusicBrainzAPI._normalize_data`: keys with hyphens get underscores, and
/// a `relations` list becomes `<target-type>_relations` groups (target type
/// hyphen-to-underscore, the `target-type` key dropped).
pub fn normalize(v: &J) -> J {
    match v {
        J::Array(items) => J::Array(items.iter().map(normalize).collect()),
        J::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "relations" {
                    if let J::Array(rels) = v {
                        let mut groups: BTreeMap<String, Vec<J>> = BTreeMap::new();
                        for rel in rels {
                            let target = rel.get("target-type").and_then(J::as_str).unwrap_or("");
                            let mut rel = rel.clone();
                            if let J::Object(m) = &mut rel {
                                m.remove("target-type");
                            }
                            groups
                                .entry(target.replace('-', "_"))
                                .or_default()
                                .push(normalize(&rel));
                        }
                        for (target, list) in groups {
                            out.insert(format!("{target}_relations"), J::Array(list));
                        }
                    }
                } else {
                    out.insert(k.replace('-', "_"), normalize(v));
                }
            }
            J::Object(out)
        }
        other => other.clone(),
    }
}

fn md5_hex(s: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The `Digest` credentials for a 401 challenge (requests' `HTTPDigestAuth`
/// with MD5, which is what MusicBrainz expects).
pub fn digest_authorization(
    method: &str,
    uri: &str,
    challenge: &str,
    user: &str,
    pass: &str,
) -> Option<String> {
    let fields = parse_challenge(challenge.strip_prefix("Digest ")?);
    let realm = fields.get("realm")?;
    let nonce = fields.get("nonce")?;
    let qop = fields
        .get("qop")
        .and_then(|q| q.split(',').map(str::trim).find(|q| *q == "auth"));
    let ha1 = md5_hex(&format!("{user}:{realm}:{pass}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let mut header = format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\""
    );
    if let Some(qop) = qop {
        let nc = "00000001";
        let cnonce = cnonce();
        let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"));
        header.push_str(&format!(
            "{response}\", algorithm=MD5, qop={qop}, nc={nc}, cnonce=\"{cnonce}\""
        ));
    } else {
        let response = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
        header.push_str(&format!("{response}\", algorithm=MD5"));
    }
    if let Some(opaque) = fields.get("opaque") {
        header.push_str(&format!(", opaque=\"{opaque}\""));
    }
    Some(header)
}

fn cnonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:032x}")
}

/// `key=value` pairs of a challenge, quoted values unquoted.
fn parse_challenge(s: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' {
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        let key = s[start..i].trim().to_ascii_lowercase();
        i += 1;
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let v = s[start..i].to_string();
            i += 1;
            v
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            s[start..i].trim().to_string()
        };
        out.insert(key, value);
    }
    out
}

#[cfg(test)]
#[path = "mbapi_tests.rs"]
mod tests;
