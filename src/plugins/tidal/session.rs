//! A port of beets' `beetsplug/tidal/session.py`: an OAuth2 session with
//! the PKCE authorization-code flow, automatic token refresh and token
//! persistence, plus the API base URL prefixing (`openapi.tidal.com/v2`).

use super::Urls;
use crate::plugins::osrandom::random_string;
use crate::plugins::requests::{self, HttpError, Request, Session};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where the session keeps its token and what it prefixes URLs with.
pub struct TidalSession {
    client_id: String,
    pub token_path: PathBuf,
    urls: Urls,
    token: Option<J>,
    /// The PKCE verifier and state of the authorization URL (kept for the
    /// token request that follows).
    pub(crate) verifier: Option<String>,
    pub(crate) state: Option<String>,
    http: Session,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

impl TidalSession {
    /// `TidalSession.__init__`: load the saved token (if any).
    pub fn new(client_id: &str, token_path: &Path, urls: Urls) -> Self {
        let token = load_token(token_path);
        TidalSession {
            client_id: client_id.to_string(),
            token_path: token_path.to_path_buf(),
            urls,
            token,
            verifier: None,
            state: None,
            http: Session::new_without_429_retry(),
        }
    }

    /// `load_token`: the token file's JSON, or None (also for an unreadable
    /// or invalid file, which beets' JSON load would raise on).
    #[allow(dead_code)]
    pub fn token(&self) -> Option<&J> {
        self.token.as_ref()
    }

    /// `save_token`: pretty-printed JSON (Python's `indent=2`).
    pub fn save_token(&self, token: &J) -> Result<()> {
        let text = serde_json::to_string_pretty(token)?;
        std::fs::write(&self.token_path, text)
            .with_context(|| format!("saving tidal token in {}", self.token_path.display()))?;
        Ok(())
    }

    /// `OAuth2Session.authorization_url`: the login URL and a generated
    /// state. The PKCE verifier (43 chars, `[A-Za-z0-9-._~]`) is kept for
    /// [`fetch_token`](Self::fetch_token).
    pub fn authorization_url(&mut self) -> (String, String) {
        let verifier = random_string(43, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~");
        let state = random_string(30, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789");
        self.verifier = Some(verifier);
        self.state = Some(state.clone());
        let challenge = code_challenge(self.verifier.as_deref().unwrap_or_default());
        let q: Vec<(&str, &str)> = vec![
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", "https://localhost"),
            ("scope", "search.read"),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ];
        let url = format!("{}?{}", self.urls.auth_url, query(&q));
        (url, state)
    }

    /// `OAuth2Session.fetch_token(..., authorization_response=...)` for the
    /// authorization-code grant: the code from the pasted redirect URL, then
    /// the token request (with the PKCE verifier and client id in the body).
    pub fn fetch_token(&mut self, redirect_url: &str) -> Result<()> {
        let Some(code) = query_param(redirect_url, "code") else {
            bail!("Please supply either code or authorization_response parameters.");
        };
        if let Some(expected) = self.state.as_deref()
            && query_param(redirect_url, "state").as_deref() != Some(expected)
        {
            bail!("(mismatching_state) CSRF Warning! State not equal in request and response.");
        }
        let Some(verifier) = self.verifier.clone() else {
            bail!("Code verifier is not found, authorization URL must be generated before");
        };
        let body = query(&[
            ("grant_type", "authorization_code"),
            ("client_id", &self.client_id),
            ("code_verifier", &verifier),
            ("code", &code),
            ("redirect_uri", "https://localhost"),
        ]);
        self.token_request(&body)
    }

    /// POST a form to the token endpoint, store the returned token (adding
    /// `expires_at`, as requests-oauthlib does) and save it.
    fn token_request(&mut self, form: &str) -> Result<()> {
        let req = Request {
            form: Some(form),
            ..Default::default()
        };
        let resp = self
            .http
            .request("POST", &self.urls.token_url, req)
            .map_err(|e| http_error(&e))?;
        let mut token = resp.json().map_err(|e| http_error(&e))?;
        if let Some(expires_in) = token.get("expires_in").and_then(serde_json::Value::as_f64) {
            token["expires_at"] = json!(now() + expires_in);
        }
        self.token = Some(token.clone());
        self.save_token(&token)
    }

    /// `OAuth2Session.refresh_token`, with beets' `auto_refresh_kwargs`.
    fn refresh(&mut self) -> Result<()> {
        let Some(refresh_token) = self
            .token
            .as_ref()
            .and_then(|t| t.get("refresh_token"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            return Ok(());
        };
        let body = query(&[
            ("grant_type", "refresh_token"),
            ("client_id", &self.client_id),
            ("scope", "search.read"),
            ("refresh_token", &refresh_token),
        ]);
        self.token_request(&body)
    }

    /// Whether the access token is missing or past `expires_at`.
    fn token_expired(&self) -> bool {
        match self.token.as_ref().and_then(|t| t.get("expires_at")) {
            Some(expires_at) => expires_at.as_f64().is_some_and(|e| e <= now()),
            None => false,
        }
    }

    /// A GET to the Tidal API (`TidalSession.request`): a relative path gets
    /// the API base URL, the access token rides in the Authorization header,
    /// and an expired token is refreshed first.
    pub fn get_json(&mut self, url: &str, params: &[(&str, String)]) -> Result<J> {
        if self.token_expired() {
            self.refresh()?;
        }
        let mut full = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("{}{url}", self.urls.api_base)
        };
        if !params.is_empty() {
            full = format!("{full}?{}", query_owned(params));
        }
        let access = self
            .token
            .as_ref()
            .and_then(|t| t.get("access_token"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let auth = format!("Bearer {access}");
        let resp = loop {
            let headers = [("Authorization", auth.as_str())];
            let req = Request {
                headers: &headers,
                ..Default::default()
            };
            match self.http.get(&full, req) {
                Ok(r) => break r,
                // beets' `TidalSession.request`: a rate limit is retried
                // after its `Retry-After` seconds (tidal's own retry policy
                // excludes 429, so its urllib3 adapter doesn't retry it).
                Err(HttpError::Status {
                    code: 429,
                    headers,
                    ..
                }) => {
                    let retry = headers
                        .iter()
                        .rev()
                        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                        .and_then(|(_, v)| v.trim().parse::<u64>().ok());
                    match retry {
                        Some(secs) if secs > 0 => {
                            crate::logging::debug!(
                                "Rate limit exceeded. Retrying after {secs} seconds."
                            );
                            std::thread::sleep(std::time::Duration::from_secs(secs));
                        }
                        _ => bail!(
                            "Rate limit handling failed: Retry-After header is missing or invalid"
                        ),
                    }
                }
                Err(e) => return Err(http_error(&e)),
            }
        };
        resp.json().map_err(|e| http_error(&e))
    }
}

fn load_token(path: &Path) -> Option<J> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn http_error(e: &HttpError) -> anyhow::Error {
    anyhow!(e.to_string())
}

/// `urlparse(...).query`, then the parameter's value.
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let q = url.split_once('?')?.1.split('#').next().unwrap_or_default();
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=')?;
        if unquote_plus(k) == name {
            return Some(unquote_plus(v));
        }
    }
    None
}

fn unquote_plus(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `urllib.parse.urlencode` for the pairs (spaces as `+`).
pub fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", requests::quote_plus(k), requests::quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn query_owned(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", requests::quote_plus(k), requests::quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// URL-safe base64 without padding (`base64.urlsafe_b64encode` minus `=`).
fn base64url(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            }
        }
    }
    out
}

/// The PKCE S256 challenge: `BASE64URL(SHA256(verifier))`.
pub fn code_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// beets' `_tokenfile`: `tidal.tokenfile` in the app directory unless it
/// is absolute.
pub fn token_path(cfg: &crate::config::Config) -> PathBuf {
    let raw = cfg.str("tidal.tokenfile");
    let path = crate::config::expand_tilde(&raw);
    if path.is_absolute() {
        path
    } else {
        cfg.config_dir.join(path)
    }
}
