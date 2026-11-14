//! beets' `test/plugins/test_plexupdate.py`, against fake plex.tv and Plex
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

// --------------------------------------------------------- TLS (verify=False)

/// A self-signed certificate and its PKCS#8 private key, as DER, made with
/// openssl (None when openssl isn't installed).
fn self_signed(dir: &Path) -> Option<(Vec<u8>, Vec<u8>)> {
    let ok = |args: &[&str]| {
        std::process::Command::new("openssl")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let cert_pem = dir.join("cert.pem");
    let key_pem = dir.join("key.pem");
    let cert_der = dir.join("cert.der");
    let key_der = dir.join("key.der");
    let (c, k) = (cert_pem.to_str()?, key_pem.to_str()?);
    let (cd, kd) = (cert_der.to_str()?, key_der.to_str()?);
    if !ok(&[
        "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", k, "-out", c, "-days", "1",
        "-subj", "/CN=localhost",
    ]) {
        return None;
    }
    if !ok(&["x509", "-in", c, "-outform", "DER", "-out", cd]) {
        return None;
    }
    if !ok(&["pkcs8", "-topk8", "-nocrypt", "-in", k, "-outform", "DER", "-out", kd]) {
        return None;
    }
    Some((std::fs::read(cert_der).ok()?, std::fs::read(key_der).ok()?))
}

/// A local HTTPS server on 127.0.0.1 (a free port) answering each request
/// with `body` as JSON, and the port. Handshakes that fail (the verifying
/// client rejects the certificate) are ignored.
fn tls_server(cert: Vec<u8>, key: Vec<u8>, body: &'static str) -> u16 {
    use std::io::{Read as _, Write as _};
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    let certs = vec![CertificateDer::from(cert)];
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key));
    let config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap(),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let config = config.clone();
            std::thread::spawn(move || {
                let Ok(mut conn) = rustls::ServerConnection::new(config) else {
                    return;
                };
                loop {
                    if conn.wants_write() && conn.write_tls(&mut stream).is_err() {
                        return;
                    }
                    if !conn.is_handshaking() {
                        break;
                    }
                    let Ok(n) = conn.read_tls(&mut stream) else {
                        return;
                    };
                    if n == 0 || conn.process_new_packets().is_err() {
                        return;
                    }
                }
                // One HTTP request, then the response.
                let mut req = vec![];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = conn.read_tls(&mut stream) else {
                        return;
                    };
                    if n == 0 || conn.process_new_packets().is_err() {
                        return;
                    }
                    conn.reader().read_to_end(&mut req).ok();
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                conn.writer().write_all(resp.as_bytes()).ok();
                conn.complete_io(&mut stream).ok();
            });
        }
    });
    port
}

#[test]
fn ignore_cert_errors_accepts_a_self_signed_server() {
    let dir = tempfile::tempdir().unwrap();
    let Some((cert, key)) = self_signed(dir.path()) else {
        return;
    };
    let port = tls_server(cert, key, "{\"MediaContainer\": {\"Directory\": []}}");
    let token_path = dir.path().join("plex_token.json");
    let api = |verify| {
        let mut api = PlexAPI::new(
            token_path.clone(),
            "127.0.0.1",
            port,
            "Music",
            true,
            verify,
            None,
        );
        api.open_browser = false;
        api
    };
    // The bad certificate is refused by default...
    assert!(api(true).get_music_section().is_err());
    // ...and accepted with `plex.ignore_cert_errors`.
    assert_eq!(api(false).get_music_section().unwrap(), None);
}
