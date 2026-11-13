//! The kodiupdate plugin: the JSON-RPC request, instances and events.

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use serde_json::json;

fn cfg(base: &str) -> Config {
    let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[kodiupdate]").unwrap());
    cfg.set(
        "kodi",
        serde_yaml::from_str(&format!(
            "[{{host: 127.0.0.1, port: {port}, user: kodi, pwd: secret}}]"
        ))
        .unwrap(),
    );
    cfg
}

fn log() -> PluginLog {
    PluginLog::new("kodi")
}

#[test]
fn posts_the_scan_request() {
    let server = serve(|_| Reply::json(json!({"result": "OK"})));
    let cfg = cfg(&server.base);
    let ((), lines) = crate::logging::capture(|| update(&cfg, &log()));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/jsonrpc");
    assert_eq!(
        requests[0].header("Authorization"),
        Some("Basic a29kaTpzZWNyZXQ=")
    );
    assert_eq!(requests[0].header("Content-Type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["method"], "AudioLibrary.Scan");
    assert_eq!(body["id"], 1);
    assert_eq!(body["jsonrpc"], "2.0");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Kodi update triggered for 127.0.0.1:")),
        "{lines:?}"
    );
}

#[test]
fn a_result_other_than_ok_is_a_warning() {
    let server = serve(|_| Reply::json(json!({"result": "Failed"})));
    let ((), lines) = crate::logging::capture(|| update(&cfg(&server.base), &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Kodi update failed: JSON response was")),
        "{lines:?}"
    );
}

#[test]
fn an_http_error_is_a_warning() {
    let server = serve(|_| Reply::status(500));
    let ((), lines) = crate::logging::capture(|| update(&cfg(&server.base), &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Kodi update failed: 500 Server Error")),
        "{lines:?}"
    );
}

#[test]
fn several_instances() {
    let server = serve(|_| Reply::json(json!({"result": "OK"})));
    let port: u16 = server.base.rsplit(':').next().unwrap().parse().unwrap();
    let mut cfg = cfg(&server.base);
    cfg.set(
        "kodi",
        serde_yaml::from_str(&format!(
            "[{{host: 127.0.0.1, port: {port}, user: a, pwd: a}}, \
             {{host: 127.0.0.1, port: {port}, user: b, pwd: b}}]"
        ))
        .unwrap(),
    );
    update(&cfg, &log());
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_single_mapping_works() {
    let server = serve(|_| Reply::json(json!({"result": "OK"})));
    let port: u16 = server.base.rsplit(':').next().unwrap().parse().unwrap();
    let mut cfg = cfg(&server.base);
    cfg.set(
        "kodi",
        serde_yaml::from_str(&format!(
            "{{host: 127.0.0.1, port: {port}, user: kodi, pwd: secret}}"
        ))
        .unwrap(),
    );
    update(&cfg, &log());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_change_triggers_an_update_on_exit() {
    let server = serve(|_| Reply::json(json!({"result": "OK"})));
    let cfg = cfg(&server.base);
    load(&cfg);
    crate::events::send("database_change", &[]);
    assert!(server.requests().is_empty());
    crate::events::send("cli_exit", &[]);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn no_change_no_update() {
    let server = serve(|_| Reply::json(json!({"result": "OK"})));
    load(&cfg(&server.base));
    crate::events::send("cli_exit", &[]);
    assert!(server.requests().is_empty());
}
