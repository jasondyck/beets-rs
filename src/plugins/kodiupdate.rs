//! A port of beets 2.14's `kodiupdate` plugin (`beetsplug/kodiupdate.py`):
//! after a command that changed the library, ask each configured Kodi
//! instance (`kodi:` settings, one or a list) to rescan its audio library
//! through its JSON-RPC API.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::requests::{Request, Session};
use crate::plugins::{PluginLog, USER_AGENT};
use serde_json::{Value as J, json};
use std::cell::Cell;
use std::rc::Rc;

pub struct KodiUpdate;

impl crate::plugins::Plugin for KodiUpdate {
    fn name(&self) -> &'static str {
        "kodi"
    }
}

/// One configured Kodi server.
pub struct Instance {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pwd: String,
}

/// `self.config.get()`, accepting one mapping or a list of them.
pub fn instances(cfg: &Config) -> Vec<Instance> {
    fn scalar(v: &serde_yaml::Value) -> String {
        match v {
            serde_yaml::Value::String(s) => s.clone(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }
    let value = cfg.get("kodi");
    let maps: Vec<&serde_yaml::Value> = match value {
        Some(serde_yaml::Value::Sequence(seq)) => seq.iter().collect(),
        Some(v) if v.is_mapping() => vec![v],
        _ => return vec![],
    };
    maps.into_iter()
        .map(|m| {
            let get = |k: &str| {
                m.get(serde_yaml::Value::String(k.to_string()))
                    .map(scalar)
                    .unwrap_or_default()
            };
            Instance {
                host: get("host"),
                port: get("port").parse().unwrap_or(8080),
                user: get("user"),
                pwd: get("pwd"),
            }
        })
        .collect()
}

/// `update_kodi`: POST the `AudioLibrary.Scan` request.
pub fn update_kodi(i: &Instance) -> Result<J, String> {
    let url = format!("http://{}:{}/jsonrpc", i.host, i.port);
    let auth = format!(
        "Basic {}",
        base64(format!("{}:{}", i.user, i.pwd).as_bytes())
    );
    let payload = json!({"jsonrpc": "2.0", "method": "AudioLibrary.Scan", "id": 1});
    let session = Session::plain(USER_AGENT);
    let resp = session
        .request(
            "POST",
            &url,
            Request {
                headers: &[
                    ("Content-Type", "application/json"),
                    ("Authorization", &auth),
                ],
                json: Some(&payload),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    if resp.status >= 400 {
        let kind = if resp.status < 500 {
            "Client"
        } else {
            "Server"
        };
        return Err(format!(
            "{} {} Error: {} for url: {url}",
            resp.status,
            kind,
            crate::plugins::requests::reason(resp.status)
        ));
    }
    resp.json().map_err(|e| e.to_string())
}

/// `KodiUpdate.update`.
pub fn update(cfg: &Config, log: &PluginLog) {
    log.info("Requesting a Kodi library update...");
    for instance in instances(cfg) {
        match update_kodi(&instance) {
            Ok(json) => {
                if json.get("result") != Some(&J::String("OK".into())) {
                    log.warning(&format!("Kodi update failed: JSON response was {json:?}"));
                    continue;
                }
                log.info(&format!(
                    "Kodi update triggered for {}:{}",
                    instance.host, instance.port
                ));
            }
            Err(e) => log.warning(&format!("Kodi update failed: {e}")),
        }
    }
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i < chunk.len() + 1 {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

struct State {
    cfg: Config,
    registered: Cell<bool>,
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        event == "database_change"
    }

    fn on(&self, _ev: &Event) {
        if !self.registered.replace(true) {
            crate::events::register(Rc::new(ExitListener {
                cfg: self.cfg.clone(),
                log: PluginLog::new("kodi"),
            }));
        }
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
        update(&self.cfg, &self.log);
    }
}

/// The plugin's `__init__`.
pub fn load(cfg: &Config) -> KodiUpdate {
    crate::events::register(Rc::new(State {
        cfg: cfg.clone(),
        registered: Cell::new(false),
    }));
    KodiUpdate
}

#[cfg(test)]
#[path = "kodiupdate_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "kodiupdate",
        order: 38,
        defaults: r#"kodi: [{host: localhost, port: 8080, user: kodi, pwd: kodi}]
"#,
        config_keys: &["kodi"],
        redacted: &["kodi.user", "kodi.pwd"],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}
