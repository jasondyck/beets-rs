//! beets' test/plugins/test_plexupdate.py, against fake plex.tv and Plex
//! servers.

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Env {
    /// A token file in a fresh directory.
    dir: tempfile::TempDir,
    /// The local Plex server: sections and refresh.
    local: crate::plugins::requests::testserver::Server,
    /// plex.tv: pins.
    tv: crate::plugins::requests::testserver::Server,
}

fn sections() -> Reply {
    Reply::json(json!({
        "MediaContainer": {"Directory": [
            {"key": "3", "title": "Movies"},
            {"key": "2", "title": "Music"}
        ]}
    }))
}

fn env() -> Env {
    let local = serve(|r| match r.target.split('?').next().unwrap() {
        "/library/sections" => sections(),
        "/library/sections/2/refresh" => Reply::json(json!({})),
        "/library/sections/9/refresh" => Reply::status(401),
        _ => Reply::status(404),
    });
    let tv = serve(|_| Reply::json(json!({})));
    Env {
        dir: tempfile::tempdir().unwrap(),
        local,
        tv,
    }
}

impl Env {
    fn token_path(&self) -> PathBuf {
        self.dir.path().join("plex_token.json")
    }

    fn api(&self, token: Option<&str>) -> PlexAPI {
        let port: u16 = self.local.base.rsplit(':').next().unwrap().parse().unwrap();
        let mut api = PlexAPI::new(
            self.token_path(),
            "127.0.0.1",
            port,
            "Music",
            false,
            true,
            token.map(String::from),
        );
        api.base_url = format!("{}/", self.local.base);
        api.api_root = format!("{}/api/v2", self.tv.base);
        api.open_browser = false;
        api
    }
}

#[test]
fn get_named_music_section() {
    let env = env();
    let mut api = env.api(None);
    api.library_name = "Movies".into();
    assert_eq!(api.get_music_section().unwrap().as_deref(), Some("3"));
    api.library_name = "Music".into();
    assert_eq!(api.get_music_section().unwrap().as_deref(), Some("2"));
    api.library_name = "Nope".into();
    assert_eq!(api.get_music_section().unwrap(), None);
}

#[test]
fn update_library_refreshes_the_section() {
    let env = env();
    let api = env.api(None);
    api.update_library().unwrap();
    let targets: Vec<String> = env
        .local
        .requests()
        .iter()
        .map(|r| r.target.clone())
        .collect();
    assert_eq!(targets[0], "/library/sections");
    assert_eq!(targets[1], "/library/sections/2/refresh");
}

#[test]
fn unauthorized_is_explained() {
    let env = env();
    let local = serve(|r| match r.target.split('?').next().unwrap() {
        "/library/sections" => {
            Reply::json(json!({"MediaContainer": {"Directory": [{"key": "9", "title": "Music"}]}}))
        }
        _ => Reply::status(401),
    });
    let mut api = env.api(None);
    api.base_url = format!("{}/", local.base);
    let err = api.update_library().unwrap_err().to_string();
    assert!(err.contains("reauthenticate"), "{err}");
    assert!(err.contains("401"), "{err}");
}

#[test]
fn a_missing_section_is_an_error() {
    let env = env();
    let mut api = env.api(None);
    api.library_name = "Other Library".into();
    let err = api.update_library().unwrap_err().to_string();
    assert_eq!(
        err,
        "No music library named 'Other Library' found on the Plex server. \
         Check the `plex.library_name` setting."
    );
}

#[test]
fn the_session_saves_and_loads_a_token() {
    let env = env();
    let api = env.api(None);
    let session = api.session();
    session.save_token(&[("X-Plex-Token", json!("TOKEN123"))]);
    assert_eq!(session.token().as_deref(), Some("TOKEN123"));
    let data = session.load_token();
    assert_eq!(data["X-Plex-Token"], "TOKEN123");
    // The client identifier is generated once and persisted.
    let id = session.client_id();
    assert_eq!(id.len(), 32);
    assert_eq!(session.load_token()["client_identifier"], id);
    assert_eq!(session.client_id(), id);
}

#[test]
fn requests_attach_the_token() {
    let env = env();
    let api = env.api(None);
    api.session()
        .save_token(&[("X-Plex-Token", json!("TOKEN123"))]);
    api.session()
        .get(&format!("{}/library/sections", env.local.base))
        .unwrap();
    let requests = env.local.requests();
    assert_eq!(requests[0].header("X-Plex-Token"), Some("TOKEN123"));
    assert_eq!(
        requests[0].header("X-Plex-Client-Identifier"),
        Some(api.session().client_id().as_str())
    );
    assert_eq!(requests[0].header("X-Plex-Product"), Some("beets"));
}

#[test]
fn a_configured_token_wins() {
    let env = env();
    let api = env.api(Some("CONFIG_TOKEN"));
    api.session()
        .save_token(&[("X-Plex-Token", json!("TOKEN123"))]);
    api.session()
        .get(&format!("{}/library/sections", env.local.base))
        .unwrap();
    let requests = env.local.requests();
    assert_eq!(requests[0].header("X-Plex-Token"), Some("CONFIG_TOKEN"));
}

#[test]
fn the_pin_flow_saves_the_account_token() {
    let env = env();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let tv = serve(
        move |r| match (r.method.as_str(), r.target.split('?').next().unwrap()) {
            ("POST", "/api/v2/pins") => {
                Reply::json(json!({"id": 1, "code": "ABCD", "expiresIn": 1800}))
            }
            ("GET", "/api/v2/pins/1") => {
                if calls2.fetch_add(1, Ordering::SeqCst) == 0 {
                    Reply::json(json!({"id": 1, "code": "ABCD"}))
                } else {
                    Reply::json(json!({"id": 1, "code": "ABCD", "authToken": "TOKEN123"}))
                }
            }
            _ => Reply::status(404),
        },
    );
    let mut api = env.api(None);
    api.api_root = format!("{}/api/v2", tv.base);
    api.session()
        .poll_interval
        .set(std::time::Duration::from_millis(5));
    api.ui_authenticate_flow(&PluginLog::new("plexupdate"))
        .unwrap();
    let session = api.session();
    assert_eq!(session.token().as_deref(), Some("TOKEN123"));
    let data = session.load_token();
    assert_eq!(data["X-Plex-Token"], "TOKEN123");
    assert_eq!(data["client_identifier"], session.client_id());
    // The pin request carries the identifying headers.
    let requests = tv.requests();
    assert_eq!(
        requests[0].header("X-Plex-Client-Identifier"),
        Some(session.client_id().as_str())
    );
}

#[test]
fn the_pin_flow_times_out() {
    let env = env();
    let tv = serve(|_| Reply::json(json!({"id": 1, "code": "ABCD"})));
    let session = PlexSession::new(env.token_path(), None, &format!("{}/api/v2", tv.base));
    session
        .poll_interval
        .set(std::time::Duration::from_millis(5));
    assert_eq!(
        session.wait_for_login(1, std::time::Duration::from_millis(60)),
        None
    );
    assert_eq!(session.token(), None);
}

#[test]
fn a_plex_tv_error_is_a_user_error() {
    let env = env();
    let tv = serve(|_| Reply::status(500));
    let mut api = env.api(None);
    api.api_root = format!("{}/api/v2", tv.base);
    let err = api
        .ui_authenticate_flow(&PluginLog::new("plexupdate"))
        .unwrap_err();
    assert!(
        err.to_string().starts_with("Plex login flow failed:"),
        "{err}"
    );
}

#[test]
fn the_plugin_updates_on_exit_after_a_change() {
    let env = env();
    let port: u16 = env.local.base.rsplit(':').next().unwrap().parse().unwrap();
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[plexupdate]").unwrap());
    cfg.set("plex.host", "127.0.0.1".into());
    cfg.set("plex.port", serde_yaml::Value::Number(port.into()));
    cfg.set(
        "plex.tokenfile",
        env.token_path().to_string_lossy().to_string().into(),
    );
    cfg.config_dir = env.dir.path().to_path_buf();
    load(&cfg);
    crate::events::send("database_change", &[]);
    assert!(env.local.requests().is_empty());
    let ((), lines) = crate::logging::capture(|| crate::events::send("cli_exit", &[]));
    let targets: Vec<String> = env
        .local
        .requests()
        .iter()
        .map(|r| r.target.clone())
        .collect();
    assert_eq!(
        targets,
        ["/library/sections", "/library/sections/2/refresh"]
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "plexupdate: Updating Plex library..."),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "plexupdate: Started library update successfully."),
        "{lines:?}"
    );
}

#[test]
fn no_change_no_update() {
    let env = env();
    let port: u16 = env.local.base.rsplit(':').next().unwrap().parse().unwrap();
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[plexupdate]").unwrap());
    cfg.set("plex.host", "127.0.0.1".into());
    cfg.set("plex.port", serde_yaml::Value::Number(port.into()));
    cfg.config_dir = env.dir.path().to_path_buf();
    load(&cfg);
    crate::events::send("cli_exit", &[]);
    assert!(env.local.requests().is_empty());
}
