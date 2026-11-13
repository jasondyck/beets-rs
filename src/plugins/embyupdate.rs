//! A port of beets 2.14's `embyupdate` plugin (`beetsplug/embyupdate.py`):
//! after a command that changed the library, ask an Emby server to refresh
//! its library (`emby:` host/port/username/password/userid/apikey).

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::requests::{HttpError, Request as HttpRequest, Session};
use crate::plugins::{Plugin, PluginLog};
use anyhow::Result;
use md5::{Digest as _, Md5};
use serde_json::Value as J;
use sha1::Sha1;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub struct EmbyUpdate;

impl Plugin for EmbyUpdate {
    fn name(&self) -> &'static str {
        "embyupdate"
    }
}

/// beets' `api_url`: `host` (with `http://` when it has no scheme), `port`
/// and `endpoint`, with `format=json` added to the query.
pub fn api_url(host: &str, port: i64, endpoint: &str) -> String {
    let hostname = if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("http://{host}")
    };
    let base = format!("{hostname}:{port}");
    let joined = urljoin(&base, endpoint);
    let (scheme, rest) = joined.split_once("://").unwrap_or(("http", &joined));
    let (authority, path_query) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (path, query) = match path_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_query, ""),
    };
    let mut params: Vec<(String, Vec<String>)> = vec![];
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = percent_decode(k);
        let v = percent_decode(v);
        match params.iter_mut().find(|(pk, _)| *pk == k) {
            Some((_, values)) => values.push(v),
            None => params.push((k, vec![v])),
        }
    }
    match params.iter_mut().find(|(k, _)| k == "format") {
        Some((_, values)) => *values = vec!["json".into()],
        None => params.push(("format".into(), vec!["json".into()])),
    }
    let query = params
        .iter()
        .flat_map(|(k, vs)| {
            vs.iter()
                .map(|v| format!("{}={}", quote_plus(k), quote_plus(v)))
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{scheme}://{authority}{path}?{query}")
}

/// `urllib.parse.quote_plus`.
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b' ' => out.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Python's `urllib.parse.urljoin` for the absolute-path endpoints beets
/// uses (an endpoint with a scheme replaces the base).
fn urljoin(base: &str, endpoint: &str) -> String {
    if endpoint.contains("://") {
        return endpoint.to_string();
    }
    if endpoint.starts_with('/') {
        let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
        let authority = rest.split('/').next().unwrap_or(rest);
        return format!("{scheme}://{authority}{endpoint}");
    }
    let mut base = base.trim_end_matches('/').to_string();
    base.push('/');
    base.push_str(endpoint);
    base
}

/// beets' `password_data`: the username and its SHA-1 and MD5 hex digests.
pub fn password_data(username: &str, password: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("username".into(), username.to_string());
    out.insert(
        "password".into(),
        hex::encode(Sha1::digest(password.as_bytes())),
    );
    out.insert(
        "passwordMd5".into(),
        hex::encode(Md5::digest(password.as_bytes())),
    );
    out
}

/// beets' `create_headers`.
pub fn create_headers(user_id: &str, token: Option<&str>) -> Vec<(String, String)> {
    let mut out = vec![(
        "x-emby-authorization".to_string(),
        format!(
            "MediaBrowser UserId=\"{user_id}\", Client=\"other\", Device=\"beets\", \
             DeviceId=\"beets\", Version=\"0.0.0\""
        ),
    )];
    if let Some(token) = token {
        out.push(("x-mediabrowser-token".to_string(), token.to_string()));
    }
    out
}

fn form_body(data: &BTreeMap<String, String>) -> String {
    data.iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn header_refs(headers: &[(String, String)]) -> Vec<(&str, &str)> {
    headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

/// beets' `get_token`.
pub fn get_token(
    session: &Session,
    host: &str,
    port: i64,
    headers: &[(String, String)],
    auth_data: &BTreeMap<String, String>,
) -> std::result::Result<Option<String>, HttpError> {
    let url = api_url(host, port, "/Users/AuthenticateByName");
    let body = form_body(auth_data);
    let resp = session.request(
        "POST",
        &url,
        HttpRequest {
            headers: &header_refs(headers),
            form: Some(&body),
            ..HttpRequest::default()
        },
    )?;
    let json = resp.json()?;
    Ok(json
        .get("AccessToken")
        .and_then(J::as_str)
        .map(String::from))
}

/// beets' `get_user`: the public users named `username`.
pub fn get_user(
    session: &Session,
    host: &str,
    port: i64,
    username: &str,
) -> std::result::Result<Vec<J>, HttpError> {
    let url = api_url(host, port, "/Users/Public");
    let resp = session.get(&url, HttpRequest::default())?;
    let json = resp.json()?;
    Ok(json
        .as_array()
        .into_iter()
        .flatten()
        .filter(|u| u.get("Name").and_then(J::as_str) == Some(username))
        .cloned()
        .collect())
}

struct State {
    cfg: Config,
    pending: Cell<bool>,
    log: PluginLog,
    session: Session,
}

pub fn load(cfg: &Config) -> EmbyUpdate {
    let log = PluginLog::new("emby");
    // Event listeners log at WARNING, as beets' `register_listener` does.
    log.set_base_level(crate::plugins::WARNING);
    crate::events::register(Rc::new(State {
        cfg: cfg.clone(),
        pending: Cell::new(false),
        log,
        session: Session::plain("python-requests/2.32.3"),
    }));
    EmbyUpdate
}

impl State {
    fn str(&self, key: &str) -> String {
        self.cfg.str(&format!("emby.{key}"))
    }

    /// beets' `update`: authenticate if needed and trigger the refresh.
    fn update(&self) -> Result<()> {
        self.log.info("Updating Emby library...");
        let host = self.str("host");
        let port = self.cfg.f64("emby.port") as i64;
        let username = self.str("username");
        let password = self.str("password");
        let mut userid = self.str("userid");
        let mut token = self.str("apikey");

        // Check if at least an apikey or password is given.
        if password.is_empty() && token.is_empty() {
            self.log.warning("Provide at least Emby password or apikey.");
            return Ok(());
        }

        if userid.is_empty() {
            let users = get_user(&self.session, &host, port, &username)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let Some(user) = users.first() else {
                self.log
                    .warning(&format!("User {username} could not be found."));
                return Ok(());
            };
            userid = user
                .get("Id")
                .and_then(J::as_str)
                .unwrap_or_default()
                .to_string();
        }

        if token.is_empty() {
            let auth_data = password_data(&username, &password);
            let headers = create_headers(&userid, None);
            token = get_token(&self.session, &host, port, &headers, &auth_data)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .unwrap_or_default();
            if token.is_empty() {
                self.log
                    .warning(&format!("Could not get token for user {username}"));
                return Ok(());
            }
        }

        // Recreate headers with a token.
        let headers = create_headers(&userid, Some(&token));
        let url = api_url(&host, port, "/Library/Refresh");
        let resp = self.session.request(
            "POST",
            &url,
            HttpRequest {
                headers: &header_refs(&headers),
                ..HttpRequest::default()
            },
        );
        match resp {
            Ok(r) if r.status == 204 => self.log.info("Update triggered."),
            Ok(_) => self.log.warning("Update could not be triggered"),
            Err(e) => self.log.warning(&format!("Update could not be triggered: {e}")),
        }
        Ok(())
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "database_change" | "cli_exit")
    }

    fn on(&self, ev: &Event) {
        match ev.name {
            "database_change" => self.pending.set(true),
            "cli_exit" => {
                if self.pending.replace(false)
                    && let Err(e) = self.update()
                {
                    self.log.error(&format!("{e:#}"));
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "embyupdate",
        order: 62,
        defaults: r#"emby:
    host: http://localhost
    port: 8096
    username: null
    password: null
    userid: null
    apikey: null
"#,
        config_keys: &["emby"],
        redacted: &["emby.username", "emby.password", "emby.userid", "emby.apikey"],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

#[cfg(test)]
#[path = "embyupdate_tests.rs"]
mod tests;
