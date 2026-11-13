//! A port of beets 2.14's `subsonicupdate` plugin
//! (`beetsplug/subsonicupdate.py`): with `subsonic.auto` (the default), any
//! library change or smart-playlist update makes `beet` ask the Subsonic
//! server to start a library scan when the command exits; the
//! `subsonicupdate` command does the same on demand.
//!
//! Settings: `subsonic.url`, `user`, `pass`, `auth` (`token`, the default,
//! or `pass` for older servers) and `auto`; the deprecated `host`, `port`
//! and `contextpath` build the URL when it is empty.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::requests::{Request, Session, format_url};
use crate::plugins::{PluginLog, USER_AGENT};
use serde_json::Value as J;
use std::cell::Cell;
use std::rc::Rc;

pub struct SubsonicUpdate;

impl crate::plugins::Plugin for SubsonicUpdate {
    fn name(&self) -> &'static str {
        "subsonic"
    }
}

/// `auto`: whether library changes trigger a scan.
pub fn auto(cfg: &Config) -> bool {
    cfg.bool("subsonic.auto")
}

/// A random 6-letter alphanumeric salt, as beets' `random.choice` loop.
fn salt() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut bytes = [0u8; 6];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
        .is_err()
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (t >> (8 * i)) as u8;
        }
    }
    bytes
        .iter()
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect()
}

/// `SubsonicUpdate.__create_token`.
pub fn create_token(cfg: &Config) -> (String, String) {
    let password = cfg.str("subsonic.pass");
    let salt = salt();
    let token = md5_hex(&format!("{password}{salt}"));
    (salt, token)
}

fn md5_hex(s: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `SubsonicUpdate.__format_url`: `<url>/rest/<endpoint>`, with the
/// deprecated `host`/`port`/`contextpath` settings when `url` is empty.
pub fn format_subsonic_url(cfg: &Config, endpoint: &str) -> String {
    let mut url = cfg.str("subsonic.url");
    if let Some(stripped) = url.strip_suffix('/') {
        url = stripped.to_string();
    }
    if url.is_empty() {
        let host = cfg.str("subsonic.host");
        let port = cfg.str("subsonic.port");
        let mut context_path = cfg.str("subsonic.contextpath");
        if context_path == "/" {
            context_path.clear();
        }
        url = format!("http://{host}:{port}{context_path}");
    }
    format!("{url}/rest/{endpoint}")
}

/// `SubsonicUpdate.start_scan`.
pub fn start_scan(cfg: &Config, log: &PluginLog) {
    let user = cfg.str("subsonic.user");
    let auth = cfg.str("subsonic.auth");
    let url = format_subsonic_url(cfg, "startScan");
    log.debug(&format!("URL is {url}"));
    log.debug(&format!("auth type is {auth}"));

    let mut payload: Vec<(&str, String)> =
        vec![("u", user), ("c", "beets".into()), ("f", "json".into())];
    match auth.as_str() {
        "token" => {
            let (salt, token) = create_token(cfg);
            payload.push(("t", token));
            payload.push(("s", salt));
            payload.push(("v", "1.13.0".into()));
        }
        "password" => {
            let pass = cfg.str("subsonic.pass");
            let encpass: String = pass.bytes().map(|b| format!("{b:02x}")).collect();
            payload.push(("p", format!("enc:{encpass}")));
            payload.push(("v", "1.12.0".into()));
        }
        _ => return,
    }
    let url = format_url(&url, &payload);
    if !url.contains("://") {
        log.error(&format!(
            "Error connecting to Subsonic server: Invalid URL '{url}': No scheme supplied"
        ));
        return;
    }
    let session = Session::plain(USER_AGENT);
    let response = match session.get_any(&url, Request::default()) {
        Ok(r) => r,
        Err(e) => {
            log.error(&format!("Error connecting to Subsonic server: {e}"));
            return;
        }
    };
    let status = response.status;
    let json: J = match response.json() {
        Ok(j) => j,
        Err(_) => {
            log.error(&format!(
                "Subsonic server returned a non-JSON response from {url} (HTTP {status})"
            ));
            return;
        }
    };
    let body = &json["subsonic-response"];
    if status == 200 && body["status"] == "ok" {
        let count = &body["scanStatus"]["count"];
        log.info(&format!("Updating Subsonic; scanning {count} tracks"));
    } else if status == 200 && body["status"] == "failed" {
        let msg = &body["error"]["message"];
        log.error(&format!("Error: {}", json_string(msg)));
    } else {
        log.error(&format!("Error: {}", json_string(&json)));
    }
}

/// Python's `str()` of the parsed response, as the error message shows it.
fn json_string(v: &J) -> String {
    match v {
        J::String(s) => s.clone(),
        other => other.to_string(),
    }
}

struct State {
    cfg: Config,
    registered: Cell<bool>,
}

impl State {
    fn register_exit(&self) {
        if !self.registered.replace(true) {
            crate::events::register(Rc::new(ExitListener {
                cfg: self.cfg.clone(),
                log: PluginLog::new("subsonic"),
            }));
        }
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "database_change" | "smartplaylist_update")
    }

    fn on(&self, _ev: &Event) {
        self.register_exit();
    }
}

struct ExitListener {
    cfg: Config,
    log: PluginLog,
}

impl Listener for ExitListener {
    fn wants(&self, event: &str) -> bool {
        event == "cli_exit"
    }

    fn on(&self, _ev: &Event) {
        start_scan(&self.cfg, &self.log);
    }
}

/// The plugin's `__init__`.
pub fn load(cfg: &Config) -> SubsonicUpdate {
    if auto(cfg) {
        crate::events::register(Rc::new(State {
            cfg: cfg.clone(),
            registered: Cell::new(false),
        }));
    }
    SubsonicUpdate
}

/// The `subsonicupdate` command: no arguments, then a scan.
pub fn command(cfg: &Config, args: &[String]) -> anyhow::Result<()> {
    if !args.is_empty() {
        anyhow::bail!("the 'subsonicupdate' command does not take arguments");
    }
    start_scan(cfg, &PluginLog::new("subsonic"));
    Ok(())
}

#[cfg(test)]
#[path = "subsonicupdate_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "subsonicupdate",
        order: 44,
        defaults: r#"subsonic:
    user: admin
    pass: admin
    url: http://localhost:4040
    auth: token
    auto: yes
"#,
        config_keys: &["subsonic"],
        item_types: &[],
        album_types: &[],
        redacted: &["subsonic.user", "subsonic.pass"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "subsonicupdate",
                build: build_subsonicupdate,
                run: run_subsonicupdate,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct SubsonicUpdateArgs {
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_subsonicupdate() -> clap::Command {
    use clap::Args as _;
    SubsonicUpdateArgs::augment_args(
        clap::Command::new("subsonicupdate")
            .about("Update the Subsonic library"),
    )
}

pub fn run_subsonicupdate(
    cfg: &crate::config::Config,
    _lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = SubsonicUpdateArgs::from_arg_matches(m)?;
    command(cfg, &a.args)
}
