//! A port of beets 2.14's `mpdstats` plugin (`beetsplug/mpdstats.py`): a
//! small MPD client that records play/skip counts, the last played time and
//! (with `rating`) a rating computed from them.
//!
//! The MPD protocol is spoken directly (a line-based text protocol):
//! `status`, `currentsong`, `idle` and `password`.

use crate::config::Config;
use crate::library::Library;
use crate::model::{FieldType, Item, Kind, Value};
use crate::plugins::{Plugin, PluginLog};
use crate::query::Query;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// If we lose the connection, how many times to retry, and the pause.
const RETRIES: usize = 10;
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Redundant play events closer than this are ignored.
const DUPLICATE_PLAY_THRESHOLD: f64 = 10.0;

/// `is_url`: an http(s) stream rather than a file.
pub fn is_url(path: &str) -> bool {
    path.split_once("://")
        .map(|(scheme, _)| scheme == "http" || scheme == "https")
        .unwrap_or(false)
}

/// The client the statistics code talks to (beets' `MPDClientWrapper`).
pub trait Client {
    /// `currentsong`: (path prefixed with `music_directory`, song id).
    fn currentsong(&mut self) -> (Option<String>, Option<String>);
    /// `status`.
    fn status(&mut self) -> BTreeMap<String, String>;
    /// `idle`: the changed subsystems.
    fn events(&mut self) -> Vec<String>;
}

/// beets' `MPDClientWrapper`: the real MPD connection.
pub struct MPDClientWrapper {
    pub music_directory: String,
    pub strip_path: String,
    log: PluginLog,
    host: String,
    port: u16,
    password: String,
    stream: Option<BufReader<TcpStream>>,
}

impl MPDClientWrapper {
    pub fn new(
        cfg: &Config,
        log: PluginLog,
        host: Option<String>,
        port: Option<u16>,
        password: Option<String>,
    ) -> MPDClientWrapper {
        let mut strip_path = cfg.str("mpd.strip_path");
        if !strip_path.ends_with('/') {
            strip_path.push('/');
        }
        MPDClientWrapper {
            music_directory: {
                let d = cfg.str("mpd.music_directory");
                if d.is_empty() {
                    cfg.directory().to_string_lossy().into_owned()
                } else {
                    d
                }
            },
            strip_path,
            log,
            host: host.unwrap_or_else(|| cfg.str("mpd.host")),
            port: port.unwrap_or_else(|| cfg.f64("mpd.port") as u16),
            password: password.unwrap_or_else(|| cfg.str("mpd.password")),
            stream: None,
        }
    }

    fn connect(&mut self) -> std::io::Result<()> {
        let stream = TcpStream::connect((self.host.as_str(), self.port))?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if !line.starts_with("OK MPD") {
            return Err(std::io::Error::other("not an MPD server"));
        }
        if !self.password.is_empty() {
            let response = command_on(&mut reader, &format!("password {}", self.password))?;
            if !response.contains_key("OK") {
                return Err(std::io::Error::other("MPD authentication failed"));
            }
        }
        self.stream = Some(reader);
        Ok(())
    }

    fn get(&mut self, command: &str) -> BTreeMap<String, String> {
        for attempt in 0..=RETRIES {
            if self.stream.is_none() && self.connect().is_err() {
                self.log.error("cannot connect to MPD");
            }
            if let Some(reader) = self.stream.as_mut() {
                match command_on(reader, command) {
                    Ok(map) => return map,
                    Err(e) => {
                        self.log.error(&format!("{e}"));
                        self.stream = None;
                    }
                }
            }
            if attempt < RETRIES {
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
        BTreeMap::new()
    }
}

/// Send a command; its response lines as a map (`OK` for the final line).
fn command_on(
    reader: &mut BufReader<TcpStream>,
    command: &str,
) -> std::io::Result<BTreeMap<String, String>> {
    {
        let stream = reader.get_mut();
        stream.write_all(command.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
    let mut out = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(std::io::Error::other("MPD connection closed"));
        }
        let line = line.trim_end();
        if line == "OK" {
            out.insert("OK".into(), String::new());
            return Ok(out);
        }
        if let Some(rest) = line.strip_prefix("ACK ") {
            return Err(std::io::Error::other(rest.to_string()));
        }
        if let Some((k, v)) = line.split_once(": ") {
            out.insert(k.to_string(), v.to_string());
        }
    }
}

impl Client for MPDClientWrapper {
    fn currentsong(&mut self) -> (Option<String>, Option<String>) {
        let entry = self.get("currentsong");
        let mut file = entry.get("file").cloned();
        let id = entry.get("id").cloned();
        if let Some(f) = &file
            && !is_url(f)
        {
            let mut f = f.clone();
            if f.starts_with(self.strip_path.as_str()) {
                f = f[self.strip_path.len()..].to_string();
            }
            file = Some(format!("{}/{}", self.music_directory.trim_end_matches('/'), f));
        }
        self.log.debug(&format!("returning: {file:?}"));
        (file, id)
    }

    fn status(&mut self) -> BTreeMap<String, String> {
        self.get("status")
    }

    fn events(&mut self) -> Vec<String> {
        let map = self.get("idle");
        map.keys()
            .filter(|k| *k != "OK")
            .cloned()
            .collect()
    }
}

/// What a playing song records (beets' `NowPlaying`).
#[derive(Clone)]
pub struct NowPlaying {
    pub started: f64,
    pub elapsed_at_start: i64,
    pub duration: i64,
    pub path: String,
    pub id: String,
    pub beets_item: Option<Item>,
}

/// beets' `MPDStats`.
pub struct MPDStats<'a, C: Client> {
    pub lib: &'a Library,
    pub do_rating: bool,
    pub rating_mix: f64,
    pub played_ratio_threshold: f64,
    pub log: PluginLog,
    pub mpd: C,
    pub now_playing: Option<NowPlaying>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn normpath(p: &str) -> String {
    crate::query::normpath(std::path::Path::new(p))
        .to_string_lossy()
        .into_owned()
}

impl<'a, C: Client> MPDStats<'a, C> {
    pub fn new(lib: &'a Library, log: PluginLog, mpd: C, cfg: &Config) -> Self {
        MPDStats {
            lib,
            do_rating: cfg.bool("mpd.rating"),
            rating_mix: cfg.f64("mpd.rating_mix"),
            played_ratio_threshold: cfg.f64("mpd.played_ratio_threshold"),
            log,
            mpd,
            now_playing: None,
        }
    }

    /// beets' `rating`.
    pub fn rating(&self, play_count: i64, skip_count: i64, rating: f64, skipped: bool) -> f64 {
        let rolling = if skipped {
            rating - rating / 2.0
        } else {
            rating + (1.0 - rating) / 2.0
        };
        let stable = (play_count as f64 + 1.0) / (play_count + skip_count + 2) as f64;
        self.rating_mix * stable + (1.0 - self.rating_mix) * rolling
    }

    /// beets' `get_item`.
    pub fn get_item(&self, path: &str) -> Option<Item> {
        let want = normpath(path);
        let items = self
            .lib
            .items(&Query::parse::<&str>(Kind::Item, &[]).ok()?)
            .ok()?;
        if let Some(item) = items
            .into_iter()
            .find(|i| normpath(&i.path().to_string_lossy()) == want)
        {
            return Some(item);
        }
        self.log
            .info(&format!("item not found: {path}"));
        None
    }

    /// beets' `update_item`.
    pub fn update_item(
        &self,
        item: &Option<Item>,
        attribute: &str,
        value: Option<Value>,
        increment: Option<f64>,
    ) {
        let Some(item) = item else { return };
        let mut item = match self.lib.get_item(item.id.unwrap_or(0)) {
            Ok(Some(i)) => i,
            _ => return,
        };
        let value = match increment {
            Some(inc) => {
                let current = item
                    .get(attribute)
                    .and_then(|v| v.as_f64())
                    .or_else(|| self.lib_flex_f64(&item, attribute))
                    .unwrap_or(0.0);
                Some(Value::Float(current + inc))
            }
            None => value,
        };
        if let Some(value) = value {
            item.set(attribute, value);
            let _ = self.lib.store(&item);
            self.log.debug(&format!(
                "updated: {attribute} = {} [{}]",
                item.get(attribute).map(|v| v.raw()).unwrap_or_default(),
                item.path().display()
            ));
        }
    }

    fn lib_flex_f64(&self, item: &Item, attribute: &str) -> Option<f64> {
        item.str(attribute).trim().parse().ok()
    }

    /// beets' `update_rating`.
    pub fn update_rating(&self, item: &Option<Item>, skipped: bool) {
        let Some(it) = item else { return };
        let mut it = match self.lib.get_item(it.id.unwrap_or(0)) {
            Ok(Some(i)) => i,
            _ => return,
        };
        let num = |name: &str, default: f64| -> f64 {
            it.get(name)
                .and_then(|v| v.as_f64())
                .or_else(|| it.str(name).trim().parse().ok())
                .unwrap_or(default)
        };
        let rating = self.rating(
            num("play_count", 0.0) as i64,
            num("skip_count", 0.0) as i64,
            num("rating", 0.5),
            skipped,
        );
        it.set("rating", Value::Float(rating));
        let _ = self.lib.store(&it);
    }

    /// beets' `handle_song_change`: skipped or played?
    pub fn handle_song_change(&mut self, song: &NowPlaying) -> bool {
        let elapsed = song.elapsed_at_start as f64 + (now() - song.started);
        let skipped = elapsed / song.duration as f64 >= 0.0
            && elapsed / (song.duration.max(1) as f64) < self.played_ratio_threshold;
        if skipped {
            self.handle_skipped(song);
        } else {
            self.handle_played(song);
        }
        if self.do_rating {
            self.update_rating(&song.beets_item, skipped);
        }
        skipped
    }

    pub fn handle_played(&self, song: &NowPlaying) {
        self.update_item(
            &song.beets_item,
            "play_count",
            None,
            Some(1.0),
        );
        self.log.info(&format!("played {}", song.path));
    }

    pub fn handle_skipped(&self, song: &NowPlaying) {
        self.update_item(&song.beets_item, "skip_count", None, Some(1.0));
        self.log.info(&format!("skipped {}", song.path));
    }

    pub fn on_stop(&mut self, status: &BTreeMap<String, String>) {
        self.log.info("stop");
        if let Some(np) = self.now_playing.take()
            && status.get("songid") != Some(&np.id)
        {
            self.handle_song_change(&np);
        }
    }

    pub fn on_pause(&mut self, _status: &BTreeMap<String, String>) {
        self.log.info("pause");
        self.now_playing = None;
    }

    pub fn on_play(&mut self, status: &BTreeMap<String, String>) {
        let (path, songid) = self.mpd.currentsong();
        let (Some(path), Some(songid)) = (path, songid) else {
            return;
        };
        let time = status.get("time").cloned().unwrap_or_default();
        let (played, duration) = match time.split_once(':') {
            Some((a, b)) => (a.parse::<i64>().unwrap_or(0), b.parse::<i64>().unwrap_or(0)),
            None => (0, 0),
        };
        if let Some(np) = &self.now_playing {
            if np.path != path {
                let np = np.clone();
                self.handle_song_change(&np);
            } else {
                let diff = (now() - np.started).abs();
                if diff <= DUPLICATE_PLAY_THRESHOLD {
                    return;
                }
                if played == 0 {
                    let np = np.clone();
                    self.handle_song_change(&np);
                }
            }
        }
        if is_url(&path) {
            self.log.info(&format!("playing stream {path}"));
            self.now_playing = None;
            return;
        }
        self.log.info(&format!("playing {path}"));
        let beets_item = self.get_item(&path);
        self.now_playing = Some(NowPlaying {
            started: now(),
            elapsed_at_start: played,
            duration,
            path,
            id: songid,
            beets_item: beets_item.clone(),
        });
        self.update_item(
            &beets_item,
            "last_played",
            Some(Value::Float(now().trunc())),
            None,
        );
    }

    /// beets' `run`.
    pub fn run(&mut self) {
        let mut events = vec!["player".to_string()];
        loop {
            if events.iter().any(|e| e == "player") {
                let status = self.mpd.status();
                match status.get("state").map(String::as_str) {
                    Some("play") => self.on_play(&status),
                    Some("pause") => self.on_pause(&status),
                    _ => self.on_stop(&status),
                }
            }
            events = self.mpd.events();
        }
    }
}

/// Item attribute types (beets' `MPDStatsPlugin.item_types`).
pub const ITEM_TYPES: &[(&str, FieldType)] = &[
    ("play_count", FieldType::Int),
    ("skip_count", FieldType::Int),
    ("last_played", FieldType::Date),
    ("rating", FieldType::Float),
];

pub struct MPDStatsPlugin;

impl Plugin for MPDStatsPlugin {
    fn name(&self) -> &'static str {
        "mpdstats"
    }
}

/// `beet mpdstats [--host H] [--port P] [--password PW]`.
pub fn config_with_overrides(
    cfg: &Config,
    host: Option<&str>,
    port: Option<u16>,
    password: Option<&str>,
) -> Config {
    let mut cfg = cfg.clone();
    if let Some(h) = host {
        cfg.set("mpd.host", serde_yaml::Value::String(h.to_string()));
    }
    if let Some(p) = port {
        cfg.set("mpd.port", serde_yaml::Value::Number(p.into()));
    }
    if let Some(p) = password {
        cfg.set("mpd.password", serde_yaml::Value::String(p.to_string()));
    }
    cfg
}

pub fn command(
    cfg: &Config,
    lib: &Library,
    host: Option<String>,
    port: Option<u16>,
    password: Option<String>,
) -> anyhow::Result<()> {
    let log = PluginLog::new("mpdstats");
    let cfg = config_with_overrides(cfg, host.as_deref(), port, password.as_deref());
    let log2 = PluginLog::new("mpdstats");
    let client = MPDClientWrapper::new(&cfg, log2, host, port, password);
    let mut stats = MPDStats::new(lib, log, client, &cfg);
    stats.run();
    Ok(())
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct MpdstatsArgs {
    /// set the hostname of the server to connect to
    #[arg(long)]
    host: Option<String>,
    /// set the port of the MPD server to connect to
    #[arg(long)]
    port: Option<u16>,
    /// set the password of the MPD server to connect to
    #[arg(long)]
    password: Option<String>,
}

pub fn build_mpdstats() -> clap::Command {
    use clap::Args as _;
    MpdstatsArgs::augment_args(
        clap::Command::new("mpdstats").about("run a MPD client to gather play statistics"),
    )
}

pub fn run_mpdstats(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = MpdstatsArgs::from_arg_matches(m)?;
    command(cfg, lib, a.host, a.port, a.password)
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "mpdstats",
        order: 68,
        defaults: r#"mpd:
    music_directory:
    strip_path: ''
    rating: true
    rating_mix: 0.75
    host: localhost
    port: 6600
    password: ''
    played_ratio_threshold: 0.85
"#,
        config_keys: &["mpd"],
        redacted: &["mpd.password"],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            order: 400,
            plugin: "mpdstats",
            build: build_mpdstats,
            run: run_mpdstats,
        }],
    }
}

fn registry_make(
    _cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(MPDStatsPlugin))
}

#[cfg(test)]
#[path = "mpdstats_tests.rs"]
mod tests;
