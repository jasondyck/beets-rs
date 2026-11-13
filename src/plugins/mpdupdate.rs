//! A port of beets 2.14's `mpdupdate` plugin (`beetsplug/mpdupdate.py`):
//! after a command that changed the library, ask the MPD server to update
//! its database, with MPD's plain-text protocol over TCP or a Unix socket.
//!
//! Settings live in the `mpd` section (`host`, `port`, `password`), whose
//! defaults come from `$MPD_HOST` and `$MPD_PORT`; for backwards
//! compatibility `mpdupdate.host` and the like override them.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::PluginLog;
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;

pub struct MpdUpdatePlugin;

impl crate::plugins::Plugin for MpdUpdatePlugin {
    fn name(&self) -> &'static str {
        "mpdupdate"
    }
}

/// beets' `BufferedSocket`: send commands and read replies line by line.
enum Sock {
    Tcp(TcpStream),
    Unix(UnixStream),
}

struct BufferedSocket {
    sock: Sock,
    buf: Vec<u8>,
}

impl BufferedSocket {
    /// Connect to `host:port`, or to a Unix socket when `host` starts with
    /// `/`, `~` or `@` (`@` for the abstract namespace).
    fn connect(host: &str, port: u16) -> std::io::Result<Self> {
        let sock = match host.chars().next() {
            Some('/') | Some('~') => {
                let path = if let Some(rest) = host.strip_prefix('~') {
                    dirs::home_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                        + rest
                } else {
                    host.to_string()
                };
                Sock::Unix(UnixStream::connect(path)?)
            }
            Some('@') => {
                use std::os::linux::net::SocketAddrExt;
                let addr =
                    std::os::unix::net::SocketAddr::from_abstract_name(&host.as_bytes()[1..])?;
                Sock::Unix(UnixStream::connect_addr(&addr)?)
            }
            _ => {
                let addr = format!("{host}:{port}");
                let stream = TcpStream::connect(addr)?;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                Sock::Tcp(stream)
            }
        };
        Ok(BufferedSocket { sock, buf: vec![] })
    }

    fn readline(&mut self) -> Vec<u8> {
        let sep = b'\n';
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == sep) {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return line;
            }
            let mut chunk = [0u8; 1024];
            let n = match &mut self.sock {
                Sock::Tcp(s) => s.read(&mut chunk),
                Sock::Unix(s) => s.read(&mut chunk),
            };
            match n {
                Ok(0) | Err(_) => return vec![],
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    fn send(&mut self, data: &[u8]) {
        let _ = match &mut self.sock {
            Sock::Tcp(s) => s.write_all(data),
            Sock::Unix(s) => s.write_all(data),
        };
    }
}

/// The `mpd` settings, with `mpdupdate.*` as the older way to set them.
pub fn settings(cfg: &Config) -> (String, u16, String) {
    let get = |key: &str| {
        let older = format!("mpdupdate.{key}");
        if cfg.get(&older).is_some() {
            cfg.str(&older)
        } else {
            cfg.str(&format!("mpd.{key}"))
        }
    };
    // `mpd.host`/`port` already default to `$MPD_HOST`/`$MPD_PORT` (from the
    // registry's computed defaults).
    let host = get("host");
    let port = get("port").parse::<u16>().unwrap_or(6600);
    let password = get("password");
    (host, port, password)
}

/// `MPDUpdatePlugin.update_mpd`.
pub fn update_mpd(cfg: &Config, log: &PluginLog) {
    let (host, port, password) = settings(cfg);
    log.info("Updating MPD database...");

    let mut s = match BufferedSocket::connect(&host, port) {
        Ok(s) => s,
        Err(e) => {
            log.warning(&format!("MPD connection failed: {e}"));
            return;
        }
    };

    let resp = s.readline();
    if !find(&resp, b"OK MPD") {
        log.warning(&format!("MPD connection failed: {}", bytes_repr(&resp)));
        return;
    }

    if !password.is_empty() {
        s.send(format!("password \"{password}\"\n").as_bytes());
        let resp = s.readline();
        if !find(&resp, b"OK") {
            log.warning(&format!("Authentication failed: {}", bytes_repr(&resp)));
            s.send(b"close\n");
            return;
        }
    }

    s.send(b"update\n");
    let resp = s.readline();
    if !find(&resp, b"updating_db") {
        log.warning(&format!("Update failed: {}", bytes_repr(&resp)));
    }

    s.send(b"close\n");
    log.info("Database updated.");
}

/// Python's `repr` of bytes (as the log messages show the reply).
fn bytes_repr(b: &[u8]) -> String {
    let mut out = String::from("b'");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(c as char),
            _ => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push('\'');
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

struct State {
    cfg: Config,
    /// `cli_exit` is registered on the first library change (beets dedupes
    /// the listener, so one is enough).
    registered: Cell<bool>,
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        event == "database_change"
    }

    fn on(&self, _ev: &Event) {
        if !self.registered.replace(true) {
            crate::events::register(Rc::new(UpdateListener {
                cfg: self.cfg.clone(),
                log: PluginLog::new("mpdupdate"),
            }));
        }
    }
}

struct UpdateListener {
    cfg: Config,
    log: PluginLog,
}

impl Listener for UpdateListener {
    fn wants(&self, event: &str) -> bool {
        event == "cli_exit"
    }

    fn on(&self, _ev: &Event) {
        update_mpd(&self.cfg, &self.log);
    }
}

/// The plugin's `__init__`: read its settings and listen for changes.
pub fn load(cfg: &Config) -> MpdUpdatePlugin {
    crate::events::register(Rc::new(State {
        cfg: cfg.clone(),
        registered: Cell::new(false),
    }));
    MpdUpdatePlugin
}

#[cfg(test)]
#[path = "mpdupdate_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "mpdupdate",
        order: 42,
        defaults: r#"mpd:
    host: MPD_HOST
    port: MPD_PORT
    password: ''
"#,
        config_keys: &["mpd"],
        item_types: &[],
        album_types: &[],
        redacted: &["mpd.password"],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}
