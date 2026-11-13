//! The mpdupdate plugin: MPD's line protocol and the event wiring.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// A minimal MPD server: greet, then answer each command; records them.
fn server(password: Option<&str>, update_fails: bool) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(vec![]));
    let seen2 = seen.clone();
    let password = password.map(String::from);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let _ = writer.write_all(b"OK MPD 0.23.5\n");
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let cmd = line.trim_end().to_string();
                seen2.lock().unwrap().push(cmd.clone());
                if cmd == "close" {
                    break;
                } else if let Some(rest) = cmd.strip_prefix("password ") {
                    let got = rest.trim_matches('"');
                    if password.as_deref() == Some(got) {
                        let _ = writer.write_all(b"OK\n");
                    } else {
                        let _ = writer.write_all(b"ACK [5@0] {} incorrect password\n");
                    }
                } else if cmd == "update" {
                    if update_fails {
                        let _ = writer.write_all(b"ACK [5@0] {} unknown command\n");
                    } else {
                        let _ = writer.write_all(b"updating_db: 1\nOK\n");
                    }
                }
            }
        }
    });
    (port, seen)
}

fn cfg(port: u16) -> Config {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mpdupdate]").unwrap());
    cfg.set("mpd.host", "127.0.0.1".into());
    cfg.set("mpd.port", serde_yaml::Value::Number(port.into()));
    cfg.set("mpd.password", "".into());
    cfg
}

fn log() -> PluginLog {
    PluginLog::new("mpdupdate")
}

/// Wait for the server thread to record exactly `want` (it runs alongside
/// the client).
fn wait_for(seen: &Arc<Mutex<Vec<String>>>, want: &[&str]) {
    for _ in 0..200 {
        if *seen.lock().unwrap() == want {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("server saw {:?}, wanted {want:?}", seen.lock().unwrap());
}

#[test]
fn sends_update() {
    let (port, seen) = server(None, false);
    update_mpd(&cfg(port), &log());
    wait_for(&seen, &["update", "close"]);
}

#[test]
fn authenticates_first() {
    let (port, seen) = server(Some("seekrit"), false);
    let mut cfg = cfg(port);
    cfg.set("mpd.password", "seekrit".into());
    update_mpd(&cfg, &log());
    wait_for(&seen, &["password \"seekrit\"", "update", "close"]);
}

#[test]
fn logs_authentication_failure() {
    let (port, seen) = server(Some("seekrit"), false);
    let mut cfg = cfg(port);
    cfg.set("mpd.password", "wrong".into());
    let ((), lines) = crate::logging::capture(|| update_mpd(&cfg, &log()));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Authentication failed: b'ACK")),
        "{lines:?}"
    );
    // No update is sent, and the connection is closed.
    wait_for(&seen, &["password \"wrong\"", "close"]);
}

#[test]
fn logs_a_failed_update() {
    let (port, seen) = server(None, true);
    let ((), lines) = crate::logging::capture(|| update_mpd(&cfg(port), &log()));
    assert!(
        lines.iter().any(|l| l.contains("Update failed: b'ACK")),
        "{lines:?}"
    );
    wait_for(&seen, &["update", "close"]);
}

#[test]
fn connection_failure_is_a_warning() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let ((), lines) = crate::logging::capture(|| update_mpd(&cfg(port), &log()));
    assert!(
        lines.iter().any(|l| l.contains("MPD connection failed")),
        "{lines:?}"
    );
}

#[test]
fn the_older_plugin_section_overrides_mpd() {
    let (port, _seen) = server(None, false);
    let mut cfg = cfg(port);
    // Broken values in `mpd`; the `mpdupdate` ones win.
    cfg.set("mpd.host", "192.0.2.1".into());
    cfg.set("mpdupdate.host", "127.0.0.1".into());
    cfg.set("mpdupdate.port", serde_yaml::Value::Number(port.into()));
    let (host, got_port, password) = settings(&cfg);
    assert_eq!(
        (host.as_str(), got_port, password.as_str()),
        ("127.0.0.1", port, "")
    );
}

#[test]
fn a_library_change_updates_on_exit() {
    let (port, seen) = server(None, false);
    let cfg = cfg(port);
    load(&cfg);
    // Nothing happens on the change itself...
    crate::events::send("database_change", &[]);
    assert!(seen.lock().unwrap().is_empty());
    // ...the update is sent when the command exits.
    crate::events::send("cli_exit", &[]);
    wait_for(&seen, &["update", "close"]);
}

#[test]
fn no_change_no_update_on_exit() {
    let (port, seen) = server(None, false);
    let cfg = cfg(port);
    load(&cfg);
    crate::events::send("cli_exit", &[]);
    assert!(seen.lock().unwrap().is_empty());
}
