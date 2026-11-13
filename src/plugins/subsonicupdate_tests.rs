//! beets' test/plugins/test_subsonicupdate.py, against a fake server.

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};

const SUCCESS: &str = r#"{"subsonic-response": {"status": "ok", "version": "1.15.0",
    "scanStatus": {"scanning": true, "count": 1000}}}"#;
const FAILED: &str = r#"{"subsonic-response": {"status": "failed", "version": "1.15.0",
    "error": {"code": 40, "message": "Wrong username or password."}}}"#;
const ERROR_BODY: &str = r#"{"timestamp": 1599185854498, "status": 404, "error": "Not Found",
    "message": "No message available", "path": "/rest/startScn"}"#;

fn cfg(url: &str) -> Config {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[subsonicupdate]").unwrap());
    cfg.set("subsonic.url", url.into());
    cfg.set("subsonic.user", "admin".into());
    cfg.set("subsonic.pass", "admin".into());
    cfg.set("subsonic.auth", "token".into());
    cfg.set("subsonic.auto", serde_yaml::Value::Bool(true));
    cfg
}

fn log() -> PluginLog {
    PluginLog::new("subsonic")
}

fn query(target: &str, key: &str) -> Option<String> {
    let (_, q) = target.split_once('?')?;
    q.split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

/// A port no one listens on.
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn start_scan_success() {
    let server = serve(|_| Reply::text(SUCCESS));
    let cfg = cfg(&server.base);
    let ((), lines) = crate::logging::capture(|| start_scan(&cfg, &log()));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let target = &requests[0].target;
    assert!(target.starts_with("/rest/startScan?"), "{target}");
    assert_eq!(query(target, "u").as_deref(), Some("admin"));
    assert_eq!(query(target, "v").as_deref(), Some("1.13.0"));
    assert_eq!(query(target, "c").as_deref(), Some("beets"));
    assert_eq!(query(target, "f").as_deref(), Some("json"));
    let salt = query(target, "s").unwrap();
    assert_eq!(salt.len(), 6);
    let token = query(target, "t").unwrap();
    assert_eq!(token, md5_hex(&format!("admin{salt}")));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Updating Subsonic; scanning 1000 tracks")),
        "{lines:?}"
    );
}

#[test]
fn start_scan_failed_bad_credentials() {
    let server = serve(|_| Reply::text(FAILED));
    let ((), lines) = crate::logging::capture(|| start_scan(&cfg(&server.base), &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Error: Wrong username or password.")),
        "{lines:?}"
    );
}

#[test]
fn start_scan_failed_not_found() {
    let server = serve(|_| Reply {
        status: 404,
        content_type: "application/json",
        body: ERROR_BODY.as_bytes().to_vec(),
    });
    let ((), lines) = crate::logging::capture(|| start_scan(&cfg(&server.base), &log()));
    assert!(lines.iter().any(|l| l.contains("Not Found")), "{lines:?}");
}

#[test]
fn start_scan_failed_unreachable() {
    let cfg = cfg(&format!("http://127.0.0.1:{}", closed_port()));
    let ((), lines) = crate::logging::capture(|| start_scan(&cfg, &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Error connecting to Subsonic server")),
        "{lines:?}"
    );
}

#[test]
fn url_with_context_path() {
    let server = serve(|_| Reply::text(SUCCESS));
    let cfg = cfg(&format!("{}/contextPath/", server.base));
    start_scan(&cfg, &log());
    assert_eq!(server.requests().len(), 1);
    assert!(
        server.requests()[0]
            .target
            .starts_with("/contextPath/rest/startScan?")
    );
}

#[test]
fn url_with_trailing_slash() {
    let server = serve(|_| Reply::text(SUCCESS));
    let cfg = cfg(&format!("{}/", server.base));
    start_scan(&cfg, &log());
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].target.starts_with("/rest/startScan?"));
}

#[test]
fn deprecated_host_port_contextpath_build_the_url() {
    let mut cfg = cfg("");
    cfg.set("subsonic.url", "".into());
    cfg.set("subsonic.host", "example.com".into());
    cfg.set("subsonic.port", serde_yaml::Value::Number(4040.into()));
    cfg.set("subsonic.contextpath", "/airsonic".into());
    assert_eq!(
        format_subsonic_url(&cfg, "startScan"),
        "http://example.com:4040/airsonic/rest/startScan"
    );
    cfg.set("subsonic.contextpath", "/".into());
    assert_eq!(
        format_subsonic_url(&cfg, "startScan"),
        "http://example.com:4040/rest/startScan"
    );
}

#[test]
fn url_with_missing_scheme_is_rejected() {
    let mut cfg = cfg("");
    cfg.set("subsonic.url", "localhost/airsonic".into());
    let ((), lines) = crate::logging::capture(|| start_scan(&cfg, &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Error connecting to Subsonic server")
                && l.contains("No scheme supplied")),
        "{lines:?}"
    );
}

#[test]
fn non_json_response() {
    let server = serve(|_| Reply {
        status: 503,
        content_type: "text/html",
        body: b"<html>server unavailable</html>".to_vec(),
    });
    let ((), lines) = crate::logging::capture(|| start_scan(&cfg(&server.base), &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Subsonic server returned a non-JSON response")),
        "{lines:?}"
    );
}

#[test]
fn password_auth_sends_the_hex_password() {
    let server = serve(|_| Reply::text(SUCCESS));
    let mut cfg = cfg(&server.base);
    cfg.set("subsonic.auth", "password".into());
    start_scan(&cfg, &log());
    let target = &server.requests()[0].target;
    assert_eq!(query(target, "p").as_deref(), Some("enc%3A61646d696e"));
    assert_eq!(query(target, "v").as_deref(), Some("1.12.0"));
    assert!(query(target, "t").is_none());
}

#[test]
fn an_unknown_auth_type_sends_nothing() {
    let server = serve(|_| Reply::text(SUCCESS));
    let mut cfg = cfg(&server.base);
    cfg.set("subsonic.auth", "other".into());
    start_scan(&cfg, &log());
    assert!(server.requests().is_empty());
}

#[test]
fn the_command_starts_a_scan() {
    let server = serve(|_| Reply::text(SUCCESS));
    command(&cfg(&server.base), &[]).unwrap();
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn the_command_rejects_arguments() {
    let server = serve(|_| Reply::text(SUCCESS));
    let err = command(&cfg(&server.base), &["startScan".into()]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the 'subsonicupdate' command does not take arguments"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_change_scans_on_exit() {
    let server = serve(|_| Reply::text(SUCCESS));
    let cfg = cfg(&server.base);
    load(&cfg);
    crate::events::send("database_change", &[]);
    assert!(server.requests().is_empty());
    crate::events::send("cli_exit", &[]);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_smartplaylist_update_scans_on_exit() {
    let server = serve(|_| Reply::text(SUCCESS));
    load(&cfg(&server.base));
    crate::events::send("smartplaylist_update", &[]);
    assert!(server.requests().is_empty());
    crate::events::send("cli_exit", &[]);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn no_change_no_scan() {
    let server = serve(|_| Reply::text(SUCCESS));
    load(&cfg(&server.base));
    crate::events::send("cli_exit", &[]);
    assert!(server.requests().is_empty());
}

#[test]
fn auto_disabled_does_not_scan() {
    let server = serve(|_| Reply::text(SUCCESS));
    let mut cfg = cfg(&server.base);
    cfg.set("subsonic.auto", serde_yaml::Value::Bool(false));
    load(&cfg);
    crate::events::send("database_change", &[]);
    crate::events::send("smartplaylist_update", &[]);
    crate::events::send("cli_exit", &[]);
    assert!(server.requests().is_empty());
}
