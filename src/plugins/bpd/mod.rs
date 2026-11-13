//! A port of beets 2.14's `bpd` plugin (`beetsplug/bpd/`): an
//! MPD-protocol music player server over the beets library (`beet bpd
//! [HOST] [PORT] [CONTROL_PORT]`), with MPD's command language, command
//! lists, `idle` events, password authentication, the path (VFS) commands
//! and the library searches.
//!
//! **No playback.** beets plays audio with GStreamer (`gstplayer.py`);
//! this port implements the server, its state and the protocol but no
//! audio output: `play`/`next`/`pause`/`stop`/`seek`/`setvol` change the
//! server's state (and the clients see it in `status`), tracks don't
//! finish on their own (no `play_finished` events), and `status`'s
//! `elapsed` is always 0. `decoders` lists the formats beets-rs reads,
//! and the single output is named "beets-rs".

pub mod player;
pub mod vfs;

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind, Value};
use anyhow::Result;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::plugins::bpd::player::Player;

pub const PROTOCOL_VERSION: &str = "0.16.0";

pub const CLIST_BEGIN: &str = "command_list_begin";
pub const CLIST_VERBOSE_BEGIN: &str = "command_list_ok_begin";
pub const CLIST_END: &str = "command_list_end";
pub const RESP_OK: &str = "OK";
pub const RESP_CLIST_VERBOSE: &str = "list_OK";
pub const RESP_ERR: &str = "ACK";

#[allow(dead_code)] // the full MPD error list, as beets has it
pub const ERROR_NOT_LIST: i32 = 1;
pub const ERROR_ARG: i32 = 2;
pub const ERROR_PASSWORD: i32 = 3;
pub const ERROR_PERMISSION: i32 = 4;
pub const ERROR_UNKNOWN: i32 = 5;
pub const ERROR_NO_EXIST: i32 = 50;
#[allow(dead_code)]
pub const ERROR_PLAYLIST_MAX: i32 = 51;
pub const ERROR_SYSTEM: i32 = 52;
#[allow(dead_code)]
pub const ERROR_PLAYLIST_LOAD: i32 = 53;
#[allow(dead_code)]
pub const ERROR_UPDATE_ALREADY: i32 = 54;
#[allow(dead_code)]
pub const ERROR_PLAYER_SYNC: i32 = 55;
#[allow(dead_code)]
pub const ERROR_EXIST: i32 = 56;

pub const VOLUME_MIN: i64 = 0;
pub const VOLUME_MAX: i64 = 100;

/// Commands that are available when unauthenticated.
pub const SAFE_COMMANDS: &[&str] = &["close", "commands", "notcommands", "password", "ping"];

/// The subsystems `idle` can watch (the last ones are always silent: their
/// commands are unsupported).
pub const SUBSYSTEMS: &[&str] = &[
    "update",
    "player",
    "mixer",
    "options",
    "playlist",
    "database",
    "stored_playlist",
    "output",
    "subscription",
    "sticker",
    "message",
    "partition",
];

/// Every command this server implements, sorted (beets' `cmd_*` methods).
pub const COMMANDS: &[&str] = &[
    "add",
    "addid",
    "clear",
    "clearerror",
    "close",
    "commands",
    "consume",
    "count",
    "crash",
    "crossfade",
    "currentsong",
    "decoders",
    "delete",
    "deleteid",
    "disableoutput",
    "enableoutput",
    "find",
    "idle",
    "kill",
    "list",
    "listall",
    "listallinfo",
    "listplaylist",
    "listplaylistinfo",
    "listplaylists",
    "lsinfo",
    "load",
    "mixrampdb",
    "mixrampdelay",
    "move",
    "moveid",
    "next",
    "notcommands",
    "outputs",
    "password",
    "pause",
    "ping",
    "play",
    "playid",
    "playlistadd",
    "playlistclear",
    "playlistdelete",
    "playlistid",
    "playlistinfo",
    "playlistmove",
    "plchanges",
    "plchangesposid",
    "previous",
    "random",
    "rename",
    "repeat",
    "replay_gain_mode",
    "replay_gain_status",
    "rm",
    "save",
    "search",
    "seek",
    "seekid",
    "setvol",
    "single",
    "stats",
    "status",
    "stop",
    "swap",
    "swapid",
    "tagtypes",
    "update",
    "urlhandlers",
    "volume",
];

/// The MPD tag names and the beets fields they search (`tagtype_map`).
pub const TAGTYPE_MAP: &[(&str, &str)] = &[
    ("Artist", "artist"),
    ("ArtistSort", "artist_sort"),
    ("Album", "album"),
    ("Title", "title"),
    ("Track", "track"),
    ("AlbumArtist", "albumartist"),
    ("AlbumArtistSort", "albumartist_sort"),
    ("Label", "label"),
    ("Genre", "genres"),
    ("Date", "year"),
    ("OriginalDate", "original_year"),
    ("Composer", "composers"),
    ("Disc", "disc"),
    ("Comment", "comments"),
    ("MUSICBRAINZ_TRACKID", "mb_trackid"),
    ("MUSICBRAINZ_ALBUMID", "mb_albumid"),
    ("MUSICBRAINZ_ARTISTID", "mb_artistid"),
    ("MUSICBRAINZ_ALBUMARTISTID", "mb_albumartistid"),
    ("MUSICBRAINZ_RELEASETRACKID", "mb_releasetrackid"),
];

// -------------------------------------------------------------- errors

/// A `BPDError`: an error to send to the client as an `ACK` line.
#[derive(Debug, Clone, PartialEq)]
pub struct BpdError {
    pub code: i32,
    pub message: String,
    pub cmd_name: String,
    pub index: usize,
}

impl BpdError {
    pub fn new(code: i32, message: impl Into<String>) -> BpdError {
        BpdError {
            code,
            message: message.into(),
            cmd_name: String::new(),
            index: 0,
        }
    }

    /// beets' `Invalid type for argument`-style static errors.
    pub fn arg_type() -> BpdError {
        BpdError::new(ERROR_ARG, "invalid type for argument")
    }

    pub fn arg_index() -> BpdError {
        BpdError::new(ERROR_ARG, "argument out of range")
    }

    pub fn arg_not_found() -> BpdError {
        BpdError::new(ERROR_NO_EXIST, "argument not found")
    }

    /// The `ACK [code@index] {command} message` response line.
    pub fn response(&self) -> String {
        format!(
            "{RESP_ERR} [{}@{}] {{{}}} {}",
            self.code, self.index, self.cmd_name, self.message
        )
    }
}

/// What running a command did.
#[derive(Debug)]
pub enum Outcome {
    /// Lines to send, then `OK`.
    Lines(Vec<String>),
    /// The connection should close (beets' `BPDCloseError`).
    Close,
    /// The client entered the idle state, watching these subsystems.
    Idle(HashSet<String>),
    /// The server should stop (`kill`).
    Shutdown,
}

/// `cast_arg`: an int argument (`bool`/`intbool` included).
fn cast_int(v: &str) -> Result<i64, BpdError> {
    v.trim()
        .parse::<i64>()
        .map_err(|_| BpdError::arg_type())
}

fn cast_float(v: &str) -> Result<f64, BpdError> {
    match v.trim().parse::<f64>() {
        Ok(f) => Ok(f),
        Err(_) => Err(BpdError::arg_type()),
    }
}

/// beets' `cast_arg("intbool", ...)`: an int, then a bool.
fn cast_intbool(v: &str) -> Result<bool, BpdError> {
    Ok(cast_int(v)? != 0)
}

// -------------------------------------------------------------- commands

/// A command from a client: its name and parsed arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
}

impl Command {
    /// beets' `Command.__init__`: the name is the first word, the rest is
    /// parsed with MPD's quoting (`"..."` with `\"` and `\\` escapes).
    pub fn parse(s: &str) -> Command {
        let s = s.trim_start();
        let (name, args) = match s.find(|c: char| c.is_ascii_whitespace()) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        Command {
            name: name.to_string(),
            args: parse_args(args),
        }
    }
}

/// `arg_re = r#""((?:\\"|[^"])+)"|([^ \t"]+)"#` over the argument string.
fn parse_args(s: &str) -> Vec<String> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#""((?:\\"|[^"])+)"|([^ \t"]+)"#).unwrap()
    });
    let mut out = vec![];
    for caps in RE.captures_iter(s) {
        if let Some(quoted) = caps.get(1) {
            let arg = quoted.as_str().replace("\\\"", "\"").replace("\\\\", "\\");
            out.push(arg);
        } else if let Some(unquoted) = caps.get(2) {
            out.push(unquoted.as_str().to_string());
        }
    }
    out
}

/// How many arguments a command takes: (min, max); `None` for unlimited.
/// beets' handlers' signatures, including their optional arguments.
fn arity(name: &str) -> Option<(usize, Option<usize>)> {
    let r = match name {
        "ping" | "kill" | "close" | "commands" | "notcommands" | "status" | "clearerror"
        | "replay_gain_status" | "clear" | "currentsong" | "next" | "previous" | "stop"
        | "crash" | "stats" | "decoders" | "tagtypes" | "listplaylists" | "outputs" => (0, Some(0)),
        "password" | "random" | "repeat" | "consume" | "single" | "setvol" | "volume"
        | "crossfade" | "mixrampdb" | "mixrampdelay" | "replay_gain_mode" | "delete"
        | "deleteid" | "plchanges" | "plchangesposid" | "add" | "addid" | "listplaylist"
        | "listplaylistinfo" | "load" | "playlistclear" | "rm" | "save" | "enableoutput"
        | "disableoutput" => (1, Some(1)),
        "move" | "moveid" | "swap" | "swapid" | "seek" | "seekid" | "count" | "playlistadd"
        | "playlistdelete" | "rename" => (2, Some(2)),
        "playlistmove" => (3, Some(3)),
        "pause" | "play" | "playid" | "playlistinfo" | "playlistid" | "update" | "lsinfo"
        | "listall" | "listallinfo" => (0, Some(1)),
        "idle" | "search" | "find" | "list" => (0, None),
        _ => return None,
    };
    Some(r)
}

// -------------------------------------------------------------- the state

/// The server's state, shared by its connections.
pub struct State<'a> {
    pub lib: &'a Library,
    pub tree: vfs::Node,
    pub updated_time: i64,
    pub startup_time: Instant,
    /// The connected clients (their ids and queued events).
    clients: Vec<Client>,
    pub password: String,
    pub random: bool,
    pub repeat: bool,
    pub consume: bool,
    pub single: bool,
    pub volume: i64,
    pub crossfade: i64,
    pub mixrampdb: f64,
    pub mixrampdelay: f64,
    pub replay_gain_mode: String,
    pub playlist: Vec<Item>,
    pub playlist_version: i64,
    pub current_index: i64,
    pub paused: bool,
    pub error: Option<String>,
    pub random_obj: crate::plugins::random::Rng,
    pub player: Box<dyn Player>,
}

impl<'a> State<'a> {
    fn new(cfg: &'a Config, lib: &'a Library, password: String) -> Result<State<'a>> {
        let _ = cfg;
        Ok(State {
            lib,
            tree: vfs::libtree(lib)
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            updated_time: crate::library::now() as i64,
            startup_time: Instant::now(),
            password,
            random: false,
            repeat: false,
            consume: false,
            single: false,
            volume: VOLUME_MAX,
            crossfade: 0,
            mixrampdb: 0.0,
            mixrampdelay: f64::NAN,
            replay_gain_mode: "off".into(),
            clients: vec![],
            playlist: vec![],
            playlist_version: 0,
            current_index: -1,
            paused: false,
            error: None,
            random_obj: crate::plugins::random::Rng::new(),
            player: Box::new(player::NullPlayer::new()),
        })
    }

    fn send_event(&mut self, event: &str) {
        for c in &mut self.clients {
            c.pending.insert(event.to_string());
        }
    }

    fn item_id(&self, item: &Item) -> i64 {
        item.id.unwrap_or(0)
    }

    /// beets' `_id_to_index`.
    fn id_to_index(&self, track_id: &str) -> Result<usize, BpdError> {
        let track_id = cast_int(track_id)?;
        for (index, track) in self.playlist.iter().enumerate() {
            if self.item_id(track) == track_id {
                return Ok(index);
            }
        }
        Err(BpdError::arg_not_found())
    }

    /// beets' `_random_idx`: a random index other than the current one.
    fn random_idx(&mut self) -> i64 {
        let len = self.playlist.len();
        if len < 2 {
            return len as i64 - 1;
        }
        let mut new_index = (self.random_obj.next_below(len)) as i64;
        while new_index == self.current_index {
            new_index = (self.random_obj.next_below(len)) as i64;
        }
        new_index
    }

    fn succ_idx(&mut self) -> i64 {
        if self.repeat && self.single {
            return self.current_index;
        }
        if self.random {
            return self.random_idx();
        }
        self.current_index + 1
    }

    fn prev_idx(&mut self) -> i64 {
        if self.repeat && self.single {
            return self.current_index;
        }
        if self.random {
            return self.random_idx();
        }
        self.current_index - 1
    }

    fn cmd_stop(&mut self) {
        self.current_index = -1;
        self.paused = false;
        let _ = self.player.stop();
        self.send_event("player");
    }

    fn cmd_play(&mut self, index: i64) -> Result<(), BpdError> {
        if index < -1 || index >= self.playlist.len() as i64 {
            return Err(BpdError::arg_index());
        }
        let new_index = index != -1 && index != self.current_index;
        let was_paused = self.paused;
        if index == -1 {
            if self.playlist.is_empty() {
                self.cmd_stop();
                return Ok(());
            }
            if self.current_index == -1 {
                self.current_index = 0;
            }
        } else {
            self.current_index = index;
        }
        self.paused = false;
        if self.current_index > -1 {
            let path = self.playlist[self.current_index as usize].path().to_path_buf();
            if was_paused && !new_index {
                // Just unpause.
                let _ = self.player.play();
            } else {
                let _ = self.player.play_file(&path);
            }
        }
        self.send_event("player");
        Ok(())
    }

    /// Command dispatch: run one command on client `index`.
    pub fn run_command(
        &mut self,
        index: usize,
        cmd: &Command,
    ) -> std::result::Result<Outcome, BpdError> {
        let mut authenticated = self.clients[index].conn.authenticated;
        let outcome = self.run_command_as(cmd, &mut authenticated);
        self.clients[index].conn.authenticated = authenticated;
        outcome
    }

    /// [`State::run_command`] with the client's authentication flag.
    fn run_command_as(
        &mut self,
        cmd: &Command,
        authenticated: &mut bool,
    ) -> std::result::Result<Outcome, BpdError> {
        let name = cmd.name.as_str();
        let Some((min, max)) = arity(name).filter(|_| COMMANDS.contains(&name)) else {
            return Err(BpdError::new(
                ERROR_UNKNOWN,
                format!("unknown command \"{name}\""),
            ));
        };
        let n = cmd.args.len();
        if n < min || max.is_some_and(|m| n > m) {
            return Err(BpdError::new(
                ERROR_ARG,
                format!("wrong number of arguments for \"{name}\""),
            ));
        }
        // Permission.
        if !self.password.is_empty() && !*authenticated && !SAFE_COMMANDS.contains(&name) {
            return Err(BpdError::new(ERROR_PERMISSION, "insufficient privileges"));
        }
        match name {
            "ping" => Ok(Outcome::Lines(vec![])),
            "close" => Ok(Outcome::Close),
            "kill" => Ok(Outcome::Shutdown),
            "crash" => {
                // A programming error: reported as a system error.
                crate::logging::emit("bpd: crash requested by client".to_string());
                Err(BpdError::new(ERROR_SYSTEM, "server error"))
            }
            "password" => {
                if cmd.args[0] == self.password {
                    *authenticated = true;
                    Ok(Outcome::Lines(vec![]))
                } else {
                    *authenticated = false;
                    Err(BpdError::new(ERROR_PASSWORD, "incorrect password"))
                }
            }
            "commands" => {
                let mut lines = vec![];
                if !self.password.is_empty() && !*authenticated {
                    for c in SAFE_COMMANDS {
                        lines.push(format!("command: {c}"));
                    }
                } else {
                    for c in COMMANDS {
                        lines.push(format!("command: {c}"));
                    }
                }
                Ok(Outcome::Lines(lines))
            }
            "notcommands" => {
                let mut lines = vec![];
                if !self.password.is_empty() && !*authenticated {
                    for c in COMMANDS {
                        if !SAFE_COMMANDS.contains(c) {
                            lines.push(format!("command: {c}"));
                        }
                    }
                }
                Ok(Outcome::Lines(lines))
            }
            "idle" => {
                let watched: Vec<String> = if cmd.args.is_empty() {
                    SUBSYSTEMS.iter().map(|s| s.to_string()).collect()
                } else {
                    cmd.args.clone()
                };
                for system in &watched {
                    if !SUBSYSTEMS.contains(&system.as_str()) {
                        return Err(BpdError::new(
                            ERROR_ARG,
                            format!("Unrecognised idle event: {system}"),
                        ));
                    }
                }
                Ok(Outcome::Idle(watched.into_iter().collect()))
            }
            "status" => Ok(Outcome::Lines(self.status())),
            "clearerror" => {
                self.error = None;
                Ok(Outcome::Lines(vec![]))
            }
            "random" | "repeat" | "consume" | "single" => {
                let state = cast_intbool(&cmd.args[0])?;
                match name {
                    "random" => self.random = state,
                    "repeat" => self.repeat = state,
                    "consume" => self.consume = state,
                    _ => self.single = state,
                }
                self.send_event("options");
                Ok(Outcome::Lines(vec![]))
            }
            "setvol" => {
                self.set_volume(cast_int(&cmd.args[0])?)?;
                Ok(Outcome::Lines(vec![]))
            }
            "volume" => {
                let delta = cast_int(&cmd.args[0])?;
                self.set_volume(self.volume + delta)?;
                Ok(Outcome::Lines(vec![]))
            }
            "crossfade" => {
                let crossfade = cast_int(&cmd.args[0])?;
                if crossfade < 0 {
                    return Err(BpdError::new(
                        ERROR_ARG,
                        "crossfade time must be nonnegative",
                    ));
                }
                crate::logging::emit("bpd: crossfade is not implemented in bpd".to_string());
                self.crossfade = crossfade;
                self.send_event("options");
                Ok(Outcome::Lines(vec![]))
            }
            "mixrampdb" => {
                let db = cast_float(&cmd.args[0])?;
                if db > 0.0 {
                    return Err(BpdError::new(
                        ERROR_ARG,
                        "mixrampdb time must be negative",
                    ));
                }
                crate::logging::emit("bpd: mixramp is not implemented in bpd".to_string());
                self.mixrampdb = db;
                self.send_event("options");
                Ok(Outcome::Lines(vec![]))
            }
            "mixrampdelay" => {
                let delay = cast_float(&cmd.args[0])?;
                if delay < 0.0 {
                    return Err(BpdError::new(
                        ERROR_ARG,
                        "mixrampdelay time must be nonnegative",
                    ));
                }
                crate::logging::emit("bpd: mixramp is not implemented in bpd".to_string());
                self.mixrampdelay = delay;
                self.send_event("options");
                Ok(Outcome::Lines(vec![]))
            }
            "replay_gain_mode" => {
                let mode = cmd.args[0].as_str();
                if !["off", "track", "album", "auto"].contains(&mode) {
                    return Err(BpdError::new(ERROR_ARG, "Unrecognised replay gain mode"));
                }
                crate::logging::emit("bpd: replay gain is not implemented in bpd".to_string());
                self.replay_gain_mode = mode.to_string();
                self.send_event("options");
                Ok(Outcome::Lines(vec![]))
            }
            "replay_gain_status" => Ok(Outcome::Lines(vec![format!(
                "replay_gain_mode: {}",
                self.replay_gain_mode
            )])),
            "clear" => {
                self.playlist.clear();
                self.playlist_version += 1;
                self.cmd_stop();
                self.send_event("playlist");
                Ok(Outcome::Lines(vec![]))
            }
            "delete" => {
                let index = py_index(cast_int(&cmd.args[0])?, self.playlist.len())?;
                self.playlist.remove(index);
                self.playlist_version += 1;
                if self.current_index == index as i64 {
                    self.cmd_stop();
                } else if (index as i64) < self.current_index {
                    self.current_index -= 1;
                }
                self.send_event("playlist");
                Ok(Outcome::Lines(vec![]))
            }
            "deleteid" => {
                let index = self.id_to_index(&cmd.args[0])?;
                self.delete_index(index)?;
                Ok(Outcome::Lines(vec![]))
            }
            "move" | "moveid" => {
                let (idx_from, idx_to) = if name == "moveid" {
                    (
                        self.id_to_index(&cmd.args[0])? as i64,
                        cast_int(&cmd.args[1])?,
                    )
                } else {
                    (cast_int(&cmd.args[0])?, cast_int(&cmd.args[1])?)
                };
                self.move_track(idx_from, idx_to)?;
                Ok(Outcome::Lines(vec![]))
            }
            "swap" | "swapid" => {
                let (i, j) = if name == "swapid" {
                    (
                        self.id_to_index(&cmd.args[0])? as i64,
                        self.id_to_index(&cmd.args[1])? as i64,
                    )
                } else {
                    (cast_int(&cmd.args[0])?, cast_int(&cmd.args[1])?)
                };
                self.swap_tracks(i, j)?;
                Ok(Outcome::Lines(vec![]))
            }
            "urlhandlers" => Ok(Outcome::Lines(vec![])),
            "playlistinfo" => {
                let index = cmd.args.first().map(String::as_str);
                Ok(Outcome::Lines(self.playlistinfo(index)?))
            }
            "playlistid" => {
                let index = match cmd.args.first() {
                    Some(id) => Some(self.id_to_index(id)? as i64),
                    None => None,
                };
                Ok(Outcome::Lines(
                    self.playlistinfo(index.map(|i| i.to_string()).as_deref())?,
                ))
            }
            "plchanges" => Ok(Outcome::Lines(self.playlistinfo(None)?)),
            "plchangesposid" => {
                let mut lines = vec![];
                for (idx, track) in self.playlist.iter().enumerate() {
                    lines.push(format!("cpos: {idx}"));
                    lines.push(format!("Id: {}", self.item_id(track)));
                }
                Ok(Outcome::Lines(lines))
            }
            "currentsong" => Ok(Outcome::Lines(self.currentsong())),
            "next" => {
                self.next()?;
                Ok(Outcome::Lines(vec![]))
            }
            "previous" => {
                self.previous()?;
                Ok(Outcome::Lines(vec![]))
            }
            "pause" => {
                match cmd.args.first() {
                    Some(state) => self.paused = cast_intbool(state)?,
                    None => self.paused = !self.paused,
                }
                if self.paused {
                    let _ = self.player.pause();
                } else if self.player.playing() {
                    let _ = self.player.play();
                }
                self.send_event("player");
                Ok(Outcome::Lines(vec![]))
            }
            "play" => {
                let index = match cmd.args.first() {
                    Some(i) => cast_int(i)?,
                    None => -1,
                };
                self.cmd_play(index)?;
                Ok(Outcome::Lines(vec![]))
            }
            "playid" => {
                let track_id = match cmd.args.first() {
                    Some(i) => cast_int(i)?,
                    None => 0,
                };
                let index = if track_id == -1 {
                    -1
                } else {
                    self.id_to_index(&track_id.to_string())? as i64
                };
                self.cmd_play(index)?;
                Ok(Outcome::Lines(vec![]))
            }
            "stop" => {
                self.cmd_stop();
                Ok(Outcome::Lines(vec![]))
            }
            "seek" | "seekid" => {
                let index = if name == "seekid" {
                    self.id_to_index(&cmd.args[0])? as i64
                } else {
                    cast_int(&cmd.args[0])?
                };
                cast_float(&cmd.args[1])?;
                if index < 0 || index >= self.playlist.len() as i64 {
                    return Err(BpdError::arg_index());
                }
                let pos = cast_float(&cmd.args[1])?;
                self.current_index = index;
                let _ = self.player.seek(pos);
                self.send_event("player");
                Ok(Outcome::Lines(vec![]))
            }
            // Additions to the MPD protocol.
            "update" => {
                self.tree = vfs::libtree(self.lib)
                    .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))?;
                self.updated_time = crate::library::now() as i64;
                self.send_event("update");
                self.send_event("database");
                Ok(Outcome::Lines(vec![]))
            }
            "lsinfo" => {
                let path = cmd.args.first().map(String::as_str).unwrap_or("/");
                Ok(Outcome::Lines(self.lsinfo(path)?))
            }
            "listall" | "listallinfo" => {
                let path = cmd.args.first().map(String::as_str).unwrap_or("/");
                Ok(Outcome::Lines(self.listall(path, name == "listallinfo")?))
            }
            "add" | "addid" => {
                let path = &cmd.args[0];
                let items = self.path_items(path)?;
                let mut lines = vec![];
                for item in items {
                    if name == "addid" {
                        lines.push(format!("Id: {}", self.item_id(&item)));
                    }
                    self.playlist.push(item);
                }
                self.playlist_version += 1;
                self.send_event("playlist");
                Ok(Outcome::Lines(lines))
            }
            "stats" => Ok(Outcome::Lines(self.stats()?)),
            "decoders" => Ok(Outcome::Lines(decoders())),
            "tagtypes" => Ok(Outcome::Lines(
                TAGTYPE_MAP
                    .iter()
                    .map(|(tag, _)| format!("tagtype: {tag}"))
                    .collect(),
            )),
            "search" | "find" => Ok(Outcome::Lines(self.search(&cmd.args, name == "search")?)),
            "list" => Ok(Outcome::Lines(self.list_values(&cmd.args)?)),
            "count" => Ok(Outcome::Lines(self.count(&cmd.args[0], &cmd.args[1])?)),
            // Stored playlists: unsupported, as beets' dummy implementations.
            "listplaylist" | "listplaylistinfo" => Err(BpdError::new(
                ERROR_NO_EXIST,
                "No such playlist",
            )),
            "listplaylists" | "playlistadd" | "playlistclear" | "playlistdelete"
            | "playlistmove" | "rename" | "rm" | "save" => Err(BpdError::new(
                ERROR_UNKNOWN,
                "Stored playlists are disabled",
            )),
            "load" => Err(BpdError::new(ERROR_NO_EXIST, "Stored playlists are disabled")),
            "outputs" => Ok(Outcome::Lines(vec![
                "outputid: 0".into(),
                "outputname: beets-rs".into(),
                "outputenabled: 1".into(),
            ])),
            "enableoutput" => {
                if cast_int(&cmd.args[0])? != 0 {
                    return Err(BpdError::arg_index());
                }
                Ok(Outcome::Lines(vec![]))
            }
            "disableoutput" => {
                if cast_int(&cmd.args[0])? == 0 {
                    return Err(BpdError::new(ERROR_ARG, "cannot disable this output"));
                }
                Err(BpdError::arg_index())
            }
            other => Err(BpdError::new(
                ERROR_UNKNOWN,
                format!("unknown command \"{other}\""),
            )),
        }
    }

    fn set_volume(&mut self, vol: i64) -> Result<(), BpdError> {
        if !(VOLUME_MIN..=VOLUME_MAX).contains(&vol) {
            return Err(BpdError::new(ERROR_ARG, "volume out of range"));
        }
        self.volume = vol;
        self.player.set_volume(vol as f64 / 100.0);
        self.send_event("mixer");
        Ok(())
    }

    fn delete_index(&mut self, index: usize) -> Result<(), BpdError> {
        self.playlist.remove(index);
        self.playlist_version += 1;
        if self.current_index == index as i64 {
            self.cmd_stop();
        } else if (index as i64) < self.current_index {
            self.current_index -= 1;
        }
        self.send_event("playlist");
        Ok(())
    }

    /// beets' `cmd_move`: Python's `pop`/`insert` semantics.
    fn move_track(&mut self, idx_from: i64, idx_to: i64) -> Result<(), BpdError> {
        let from = py_index(idx_from, self.playlist.len())?;
        let track = self.playlist.remove(from);
        let len = self.playlist.len() as i64;
        let at = if idx_to < 0 {
            (idx_to + len).clamp(0, len)
        } else {
            idx_to.clamp(0, len)
        };
        self.playlist.insert(at as usize, track);
        if idx_from == self.current_index {
            self.current_index = at;
        } else if idx_from < self.current_index && idx_to >= self.current_index {
            self.current_index -= 1;
        } else if idx_from > self.current_index && idx_to <= self.current_index {
            self.current_index += 1;
        }
        self.playlist_version += 1;
        self.send_event("playlist");
        Ok(())
    }

    fn swap_tracks(&mut self, i: i64, j: i64) -> Result<(), BpdError> {
        let len = self.playlist.len();
        let i = py_index(i, len)?;
        let j = py_index(j, len)?;
        self.playlist.swap(i, j);
        if self.current_index == i as i64 {
            self.current_index = j as i64;
        } else if self.current_index == j as i64 {
            self.current_index = i as i64;
        }
        self.playlist_version += 1;
        self.send_event("playlist");
        Ok(())
    }

    fn next(&mut self) -> Result<(), BpdError> {
        let old_index = self.current_index;
        self.current_index = self.succ_idx();
        if self.consume && old_index >= 0 {
            let old = py_index(old_index, self.playlist.len())?;
            self.playlist.remove(old);
            if self.current_index > old_index {
                self.current_index -= 1;
            }
            self.playlist_version += 1;
            self.send_event("playlist");
        }
        if self.current_index >= self.playlist.len() as i64 {
            if self.repeat {
                self.current_index = -1;
                return self.cmd_play(-1);
            }
            self.cmd_stop();
            return Ok(());
        }
        if self.single && !self.repeat {
            self.cmd_stop();
            return Ok(());
        }
        self.cmd_play(self.current_index)
    }

    fn previous(&mut self) -> Result<(), BpdError> {
        let old_index = self.current_index;
        self.current_index = self.prev_idx();
        if self.consume && old_index >= 0 {
            let old = py_index(old_index, self.playlist.len())?;
            self.playlist.remove(old);
        }
        if self.current_index < 0 {
            if self.repeat {
                self.current_index = self.playlist.len() as i64 - 1;
            } else {
                self.current_index = 0;
            }
        }
        self.cmd_play(self.current_index)
    }

    fn status(&mut self) -> Vec<String> {
        let mut lines = vec![
            format!("repeat: {}", self.repeat as i32),
            format!("random: {}", self.random as i32),
            format!("consume: {}", self.consume as i32),
            format!("single: {}", self.single as i32),
            format!("playlist: {}", self.playlist_version),
            format!("playlistlength: {}", self.playlist.len()),
            format!("mixrampdb: {}", py_float(self.mixrampdb)),
        ];
        if self.volume > 0 {
            lines.push(format!("volume: {}", self.volume));
        }
        if !self.mixrampdelay.is_nan() {
            lines.push(format!("mixrampdelay: {}", py_float(self.mixrampdelay)));
        }
        if self.crossfade > 0 {
            lines.push(format!("xfade: {}", self.crossfade));
        }
        let state = if self.current_index == -1 {
            "stop"
        } else if self.paused {
            "pause"
        } else {
            "play"
        };
        lines.push(format!("state: {state}"));
        if self.current_index != -1
            && let Some(item) = self.playlist.get(self.current_index as usize)
        {
            let current_id = self.item_id(item);
            lines.push(format!("song: {}", self.current_index));
            lines.push(format!("songid: {current_id}"));
            if let Some(next) = self.playlist.get(self.current_index as usize + 1) {
                lines.push(format!("nextsong: {}", self.current_index + 1));
                lines.push(format!("nextsongid: {}", self.item_id(next)));
            }
            // The player's audio properties (the null player reports 0 and
            // falls back on the item's length).
            let (pos, player_total) = self.player.time();
            let total = if player_total > 0.0 {
                player_total
            } else {
                item.float("length")
            };
            lines.push(format!(
                "bitrate: {}",
                py_float(item.int("bitrate") as f64 / 1000.0)
            ));
            lines.push(format!(
                "audio: {}:{}:{}",
                item.int("samplerate"),
                item.int("bitdepth"),
                item.int("channels")
            ));
            lines.push(format!("time: {}:{}", pos as i64, total as i64));
            lines.push(format!("elapsed: {pos:.3}"));
            lines.push(format!("duration: {total:.3}"));
        }
        if let Some(error) = &self.error {
            lines.push(format!("error: {error}"));
        }
        lines
    }

    fn currentsong(&self) -> Vec<String> {
        match self.playlist.get(self.current_index.max(0) as usize) {
            Some(track) if self.current_index != -1 => self.item_info(track),
            _ => vec![],
        }
    }

    /// beets' `_item_info`.
    fn item_info(&self, item: &Item) -> Vec<String> {
        let mut lines = vec![
            format!("file: {}", vfs::item_path(self.lib, item)),
            format!("Time: {}", item.float("length") as i64),
            format!("duration: {:.3}", item.float("length")),
            format!("Id: {}", self.item_id(item)),
        ];
        if let Ok(pos) = self.id_to_index(&self.item_id(item).to_string()) {
            lines.push(format!("Pos: {pos}"));
        }
        for (tagtype, field) in TAGTYPE_MAP {
            lines.push(format!("{tagtype}: {}", item_field_str(item, field)));
        }
        lines
    }

    fn playlistinfo(&self, index: Option<&str>) -> Result<Vec<String>, BpdError> {
        let mut lines = vec![];
        match index {
            None => {
                for track in &self.playlist {
                    lines.extend(self.item_info(track));
                }
            }
            Some(index) => match parse_range(index, true)? {
                RangeSpec::Single(i) => {
                    let pos = py_index(i, self.playlist.len())?;
                    lines.extend(self.item_info(&self.playlist[pos]));
                }
                RangeSpec::Range(start, stop) => {
                    for i in start..stop {
                        let pos = py_index(i, self.playlist.len())?;
                        lines.extend(self.item_info(&self.playlist[pos]));
                    }
                }
            },
        }
        Ok(lines)
    }

    /// beets' `_resolve_path`: the node at `path` (a directory).
    fn resolve_path(&self, path: &str) -> Result<vfs::Node, BpdError> {
        let components = vfs::components(path);
        let mut node = &self.tree;
        for component in components {
            if node.files.contains_key(&component) {
                return Err(BpdError::arg_not_found());
            }
            match node.dirs.get(&component) {
                Some(child) => node = child,
                None => return Err(BpdError::arg_not_found()),
            }
        }
        Ok(node.clone())
    }

    /// beets' `_resolve_path` when the path names a file: its item id.
    fn resolve_file(&self, path: &str) -> Result<Option<i64>, BpdError> {
        let components = vfs::components(path);
        let Some((file, dirs)) = components.split_last() else {
            return Ok(None);
        };
        let mut node = &self.tree;
        for d in dirs {
            match node.dirs.get(d) {
                Some(child) => node = child,
                None => return Err(BpdError::arg_not_found()),
            }
        }
        Ok(node.files.get(file).copied())
    }

    /// `_all_items` of whatever `path` names (a file gives one item).
    fn path_items(&self, path: &str) -> Result<Vec<Item>, BpdError> {
        if let Some(id) = self.resolve_file(path)? {
            return self.get_item(id).map(|i| i.into_iter().collect());
        }
        self.all_items(&self.resolve_path(path)?)
    }

    fn get_item(&self, id: i64) -> Result<Option<Item>, BpdError> {
        self.lib
            .get_item(id)
            .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))
    }

    /// beets' `cmd_lsinfo`: a file isn't a directory.
    fn lsinfo(&self, path: &str) -> Result<Vec<String>, BpdError> {
        if self.resolve_file(path)?.is_some() {
            return Err(BpdError::new(ERROR_ARG, "this is not a directory"));
        }
        let node = self.resolve_path(path)?;
        let mut lines = vec![];
        for id in node.files.values() {
            if let Some(item) = self.get_item(*id)? {
                lines.extend(self.item_info(&item));
            }
        }
        for name in node.dirs.keys() {
            let mut dirpath = vfs::path_join(path, name);
            if let Some(stripped) = dirpath.strip_prefix('/') {
                dirpath = stripped.to_string();
            }
            lines.push(format!("directory: {dirpath}"));
        }
        Ok(lines)
    }

    /// beets' `_listall`: the paths (or info) of everything under `path`.
    fn listall(&self, path: &str, info: bool) -> Result<Vec<String>, BpdError> {
        if let Some(id) = self.resolve_file(path)? {
            if info {
                return Ok(match self.get_item(id)? {
                    Some(item) => self.item_info(&item),
                    None => vec![],
                });
            }
            return Ok(vec![format!("file: {path}")]);
        }
        let node = self.resolve_path(path)?;
        self.listall_node(path, &node, info)
    }

    fn listall_node(
        &self,
        basepath: &str,
        node: &vfs::Node,
        info: bool,
    ) -> Result<Vec<String>, BpdError> {
        let mut lines = vec![];
        for (name, id) in node.files.iter() {
            let newpath = vfs::path_join(basepath, name);
            if info {
                if let Some(item) = self.get_item(*id)? {
                    lines.extend(self.item_info(&item));
                }
            } else {
                lines.push(format!("file: {newpath}"));
            }
        }
        for (name, subdir) in node.dirs.iter() {
            let newpath = vfs::path_join(basepath, name);
            lines.push(format!("directory: {newpath}"));
            lines.extend(self.listall_node(&newpath, subdir, info)?);
        }
        Ok(lines)
    }

    /// beets' `_all_items`: every item under a directory node.
    fn all_items(&self, node: &vfs::Node) -> Result<Vec<Item>, BpdError> {
        let mut out = vec![];
        for id in node.files.values() {
            if let Some(item) = self.get_item(*id)? {
                out.push(item);
            }
        }
        for subdir in node.dirs.values() {
            out.extend(self.all_items(subdir)?);
        }
        Ok(out)
    }

    fn stats(&self) -> Result<Vec<String>, BpdError> {
        let rows = self
            .lib
            .raw_query(
                "SELECT COUNT(DISTINCT artist), COUNT(DISTINCT album), COUNT(id), SUM(length) \
                 FROM items",
                &[],
            )
            .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))?;
        let row = rows.first().cloned().unwrap_or_default();
        let get = |i: usize| row.get(i).cloned().unwrap_or(rusqlite::types::Value::Null);
        let sql_num = |v: rusqlite::types::Value| -> String {
            match v {
                rusqlite::types::Value::Integer(i) => i.to_string(),
                // `int()` of the sum.
                rusqlite::types::Value::Real(f) => (f as i64).to_string(),
                rusqlite::types::Value::Text(s) => s,
                _ => "0".into(),
            }
        };
        let uptime = self.startup_time.elapsed().as_secs() as i64;
        Ok(vec![
            format!("artists: {}", sql_num(get(0))),
            format!("albums: {}", sql_num(get(1))),
            format!("songs: {}", sql_num(get(2))),
            format!("uptime: {uptime}"),
            "playtime: 0".into(),
            format!("db_playtime: {}", sql_num(get(3))),
            format!("db_update: {}", self.updated_time),
        ])
    }

    /// `_tagtype_lookup`: the canonical name and beets field for a tag.
    fn tagtype_lookup(tag: &str) -> Result<(&'static str, &'static str), BpdError> {
        for (name, field) in TAGTYPE_MAP {
            if name.eq_ignore_ascii_case(tag) {
                return Ok((name, field));
            }
        }
        Err(BpdError::new(ERROR_UNKNOWN, "no such tagtype"))
    }

    /// `_metadata_query`: the query for tag/value pairs.
    fn metadata_query(
        &self,
        kv: &[String],
        substring: bool,
        allow_any: bool,
    ) -> Result<crate::query::Query, BpdError> {
        let bad = |_| BpdError::new(ERROR_UNKNOWN, "no such tagtype");
        if kv.is_empty() {
            let mut q = crate::query::Query::field_match(Kind::Item, "id", "", false)
                .map_err(bad)?;
            q.pred = crate::query::Pred::True;
            return Ok(q);
        }
        let mut preds = vec![];
        for pair in kv.chunks(2) {
            if pair.len() < 2 {
                return Err(BpdError::new(
                    ERROR_ARG,
                    "Incorrect number of filter arguments",
                ));
            }
            let (tag, value) = (&pair[0], &pair[1]);
            if tag.eq_ignore_ascii_case("any") {
                if !allow_any {
                    return Err(BpdError::new(ERROR_UNKNOWN, "no such tagtype"));
                }
                preds.push(
                    crate::query::Query::any_tag_field_match(Kind::Item, value, substring)
                        .map_err(bad)?
                        .pred,
                );
            } else {
                let (_, field) = Self::tagtype_lookup(tag)?;
                preds.push(
                    crate::query::Query::field_match(Kind::Item, field, value, substring)
                        .map_err(bad)?
                        .pred,
                );
            }
        }
        let mut q =
            crate::query::Query::field_match(Kind::Item, "id", "", false).map_err(bad)?;
        q.pred = match preds.len() {
            1 => preds.pop().unwrap(),
            _ => crate::query::Pred::And(preds),
        };
        Ok(q)
    }

    fn search(&self, kv: &[String], substring: bool) -> Result<Vec<String>, BpdError> {
        let query = self.metadata_query(kv, substring, substring)?;
        let items = self
            .lib
            .items(&query)
            .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))?;
        let mut lines = vec![];
        for item in &items {
            lines.extend(self.item_info(item));
        }
        Ok(lines)
    }

    fn list_values(&self, args: &[String]) -> Result<Vec<String>, BpdError> {
        let (show_tag, show_key) = Self::tagtype_lookup(&args[0])?;
        let kv: Vec<String> = if args.len() == 2 {
            if show_tag != "Album" {
                return Err(BpdError::new(
                    ERROR_ARG,
                    "should be \"Album\" for 3 arguments",
                ));
            }
            // MPD's short form: the albums belonging to an artist.
            vec!["Artist".to_string(), args[1].clone()]
        } else {
            args[1..].to_vec()
        };
        if !kv.len().is_multiple_of(2) {
            return Err(BpdError::new(
                ERROR_ARG,
                "Incorrect number of filter arguments",
            ));
        }
        let query = self.metadata_query(&kv, false, false)?;
        let (clause, params) = query
            .pred
            .clause()
            .ok_or_else(|| BpdError::new(ERROR_SYSTEM, "server error"))?;
        let sql = format!(
            "SELECT DISTINCT {show_key} FROM items WHERE {clause} ORDER BY {show_key}"
        );
        let rows = self
            .lib
            .raw_query(&sql, &params)
            .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))?;
        let mut lines = vec![];
        for row in rows {
            let Some(value) = row.first() else { continue };
            let text = match value {
                rusqlite::types::Value::Null => continue,
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Real(f) => py_float(*f),
                rusqlite::types::Value::Text(s) => s.clone(),
                rusqlite::types::Value::Blob(b) => String::from_utf8_lossy(b).into_owned(),
            };
            if text.is_empty() {
                continue;
            }
            lines.push(format!("{show_tag}: {text}"));
        }
        Ok(lines)
    }

    fn count(&self, tag: &str, value: &str) -> Result<Vec<String>, BpdError> {
        let (_, field) = Self::tagtype_lookup(tag)?;
        let query = crate::query::Query::field_match(Kind::Item, field, value, false)
            .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))?;
        let items = self
            .lib
            .items(&query)
            .map_err(|_| BpdError::new(ERROR_SYSTEM, "server error"))?;
        let mut playtime = 0.0;
        for item in &items {
            playtime += item.float("length");
        }
        Ok(vec![
            format!("songs: {}", items.len()),
            format!("playtime: {}", playtime as i64),
        ])
    }
}

/// Python list indexing (`-1` is the last element): the index, or an
/// `argument out of range` error.
fn py_index(index: i64, len: usize) -> Result<usize, BpdError> {
    let i = if index < 0 { index + len as i64 } else { index };
    if i < 0 || i >= len as i64 {
        return Err(BpdError::arg_index());
    }
    Ok(i as usize)
}

/// A parsed MPD range.
enum RangeSpec {
    Single(i64),
    /// START:STOP, where STOP is excluded (and may be far past the end).
    Range(i64, i64),
}

/// beets' `_parse_range`: `START:STOP` (stop excluded), or a single number.
fn parse_range(items: &str, accept_single: bool) -> Result<RangeSpec, BpdError> {
    match items.split_once(':') {
        Some((start, stop)) => {
            let start = if start.is_empty() {
                0
            } else {
                cast_int(start)?
            };
            let stop = if stop.is_empty() {
                i64::MAX
            } else {
                cast_int(stop)?
            };
            Ok(RangeSpec::Range(start, stop))
        }
        None => {
            if accept_single {
                Ok(RangeSpec::Single(cast_int(items)?))
            } else {
                Err(BpdError::new(ERROR_ARG, "bad range syntax"))
            }
        }
    }
}

/// Python's `str()` of a float: `2.0` is "2.0".
fn py_float(f: f64) -> String {
    crate::pyyaml::py_float_repr(f)
}

/// The raw value of an item field as Python's `str()` has it (beets'
/// `_item_info` prints `getattr(item, field)`).
fn item_field_str(item: &Item, field: &str) -> String {
    if matches!(
        item.field_type(field),
        Some(crate::model::FieldType::List(_))
    ) {
        return item.list(field).join("; ");
    }
    match item.get(field) {
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Float(f)) => py_float(f),
        Some(Value::Bool(b)) => if b { "True" } else { "False" }.to_string(),
        Some(Value::Str(s)) => s,
        Some(Value::Path(p)) => p.to_string_lossy().into_owned(),
        Some(Value::Null) | None => String::new(),
    }
}

/// beets' `cmd_decoders`: the formats this port reads (no GStreamer, so a
/// fixed list rather than the plugin's own).
fn decoders() -> Vec<String> {
    let mut lines = vec![];
    for (plugin, exts, mimes) in [
        (
            "mp3",
            &["mp3"][..],
            &["audio/mpeg", "audio/mp3"][..],
        ),
        ("flac", &["flac"][..], &["audio/flac", "audio/x-flac"][..]),
        ("vorbis", &["ogg", "oga"][..], &["audio/ogg"][..]),
        ("opus", &["opus"][..], &["audio/ogg", "audio/opus"][..]),
        ("aac", &["m4a", "mp4", "aac"][..], &["audio/mp4", "audio/aac"][..]),
        ("wav", &["wav"][..], &["audio/x-wav"][..]),
        ("aiff", &["aif", "aiff"][..], &["audio/x-aiff"][..]),
    ] {
        lines.push(format!("plugin: {plugin}"));
        for e in exts {
            lines.push(format!("suffix: {e}"));
        }
        for m in mimes {
            lines.push(format!("mime_type: {m}"));
        }
    }
    lines
}

// ------------------------------------------------------------ connections

/// A client's per-connection state.
#[derive(Debug, Default)]
pub struct Conn {
    pub authenticated: bool,
}

/// A buffered client stream; reads never block (the socket is
/// non-blocking) so one thread serves every client.
struct LineReader {
    stream: TcpStream,
    buf: Vec<u8>,
    eof: bool,
}

impl LineReader {
    fn new(stream: TcpStream) -> std::io::Result<LineReader> {
        stream.set_nonblocking(true)?;
        Ok(LineReader {
            stream,
            buf: vec![],
            eof: false,
        })
    }

    /// The next complete line (without its newline); `None` when there's
    /// nothing to read yet, or at the end of the stream (`at_eof`).
    fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                return Ok(Some(line));
            }
            if self.eof {
                return Ok(None);
            }
            let mut chunk = [0u8; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    fn at_eof(&self) -> bool {
        self.eof
    }
}

/// An MPD client connection.
struct Client {
    reader: LineReader,
    writer: TcpStream,
    conn: Conn,
    idle: HashSet<String>,
    pending: HashSet<String>,
    /// The command list being assembled, and whether it's verbose.
    clist: Option<(Vec<Command>, bool)>,
    closed: bool,
}

impl Client {
    fn send(&mut self, lines: &[String]) {
        for line in lines {
            let _ = writeln!(self.writer, "{line}");
        }
        let _ = self.writer.flush();
    }

    fn send_line(&mut self, line: &str) {
        let _ = writeln!(self.writer, "{line}");
        let _ = self.writer.flush();
    }
}

/// A control-socket connection (debugging, player notifications).
struct ControlClient {
    reader: LineReader,
    writer: TcpStream,
}

/// Why running a command didn't produce output lines.
enum Failure {
    Bpd(BpdError),
    Close,
    Idle(HashSet<String>),
    Shutdown,
}

/// The server: its state, listeners and connections. It runs in one
/// thread (as beets' asyncio loop does): `poll_once` accepts clients,
/// reads their commands and delivers idle events; `run` loops over it.
pub struct Server<'a> {
    state: State<'a>,
    mpd: Option<TcpListener>,
    ctrl: Option<TcpListener>,
    controls: Vec<ControlClient>,
    shutdown: bool,
}

impl<'a> Server<'a> {
    pub fn new(
        cfg: &'a Config,
        lib: &'a Library,
        password: String,
        volume: i64,
    ) -> Result<Server<'a>> {
        let log = crate::plugins::PluginLog::new("bpd");
        let player = player::default_player(&log);
        Server::with_player(cfg, lib, password, volume, player)
    }

    /// A server with a specific player (the tests' null player).
    pub fn with_player(
        cfg: &'a Config,
        lib: &'a Library,
        password: String,
        volume: i64,
        player: Box<dyn Player>,
    ) -> Result<Server<'a>> {
        let mut state = State::new(cfg, lib, password)?;
        state.volume = volume;
        state.player = player;
        state.player.set_volume(volume as f64 / 100.0);
        Ok(Server {
            state,
            mpd: None,
            ctrl: None,
            controls: vec![],
            shutdown: false,
        })
    }

    /// Bind the MPD and control sockets (`port` 0 asks the OS for one).
    pub fn bind(&mut self, host: &str, port: u16, ctrl_host: &str, ctrl_port: u16) -> Result<()> {
        let mpd = TcpListener::bind((host, port))?;
        let ctrl = TcpListener::bind((ctrl_host, ctrl_port))?;
        mpd.set_nonblocking(true)?;
        ctrl.set_nonblocking(true)?;
        self.mpd = Some(mpd);
        self.ctrl = Some(ctrl);
        Ok(())
    }

    /// The bound ports: (MPD, control).
    pub fn ports(&self) -> Option<(u16, u16)> {
        Some((
            self.mpd.as_ref()?.local_addr().ok()?.port(),
            self.ctrl.as_ref()?.local_addr().ok()?.port(),
        ))
    }

    #[cfg(test)]
    pub fn is_stopped(&self) -> bool {
        self.shutdown
    }

    /// Serve until stopped (Ctrl-C in the real command; `stop` in tests).
    pub fn run(&mut self) {
        while !self.shutdown {
            self.poll_once();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// One round of the event loop: accept, read commands, send events.
    pub fn poll_once(&mut self) {
        self.accept();
        self.read_clients();
        self.read_controls();
        self.dispatch_events();
        self.play_finished();
    }

    /// A track ended (`play_finished`): advance the playlist and tell the
    /// control connections (beets' `Server.play_finished`).
    fn play_finished(&mut self) {
        if !self.state.player.take_finished() {
            return;
        }
        let _ = self.state.next();
        for control in &mut self.controls {
            let _ = writeln!(control.writer, "play_finished");
            let _ = control.writer.flush();
        }
    }

    fn accept(&mut self) {
        if let Some(listener) = &self.mpd {
            while let Ok((stream, _)) = listener.accept() {
                let Ok(writer) = stream.try_clone() else { continue };
                let Ok(reader) = LineReader::new(stream) else {
                    continue;
                };
                let mut client = Client {
                    reader,
                    writer,
                    conn: Conn::default(),
                    idle: HashSet::new(),
                    pending: HashSet::new(),
                    clist: None,
                    closed: false,
                };
                client.send_line(&format!("OK MPD {PROTOCOL_VERSION}"));
                self.state.clients.push(client);
            }
        }
        if let Some(listener) = &self.ctrl {
            while let Ok((stream, _)) = listener.accept() {
                let Ok(writer) = stream.try_clone() else { continue };
                let Ok(reader) = LineReader::new(stream) else {
                    continue;
                };
                self.controls.push(ControlClient { reader, writer });
            }
        }
    }

    /// Read and run every available command of every client.
    fn read_clients(&mut self) {
        for i in 0..self.state.clients.len() {
            // Lines are handled one at a time, and a command may queue
            // events for other clients (handled in `dispatch_events`).
            loop {
                let line = match self.state.clients[i].reader.next_line() {
                    Ok(Some(l)) => l,
                    Ok(None) => {
                        if self.state.clients[i].reader.at_eof() {
                            self.state.clients[i].closed = true;
                        }
                        break;
                    }
                    Err(_) => {
                        self.state.clients[i].closed = true;
                        break;
                    }
                };
                if !self.handle_line(i, line) {
                    break;
                }
            }
        }
        self.state.clients.retain(|c| !c.closed);
    }

    /// Handle one line from client `i`; false stops reading it this round.
    fn handle_line(&mut self, i: usize, line: String) -> bool {
        let line = line.trim().to_string();
        let client = &mut self.state.clients[i];
        if !client.idle.is_empty() {
            if line == "noidle" {
                // Leaving idle: report what's queued, then OK.
                let events = events_for(&mut client.pending, &client.idle);
                for event in &events {
                    client.send_line(&format!("changed: {event}"));
                }
                client.idle.clear();
                client.send_line(RESP_OK);
                return true;
            }
            let mut err = BpdError::new(ERROR_UNKNOWN, format!("Got command while idle: {line}"));
            err.index = 0;
            client.send_line(&err.response());
            client.closed = true;
            return false;
        }
        if line.is_empty() {
            let mut err = BpdError::new(ERROR_UNKNOWN, "No command given");
            err.index = 0;
            client.send_line(&err.response());
            client.closed = true;
            return false;
        }
        if line == "noidle" {
            // When not idle, this command sends no response.
            return true;
        }
        if let Some((commands, verbose)) = &mut self.state.clients[i].clist {
            if line == CLIST_END {
                let (commands, verbose) = (std::mem::take(commands), *verbose);
                self.state.clients[i].clist = None;
                return self.run_command_list(i, &commands, verbose);
            }
            commands.push(Command::parse(&line));
            return true;
        }
        if line == CLIST_BEGIN || line == CLIST_VERBOSE_BEGIN {
            self.state.clients[i].clist = Some((vec![], line == CLIST_VERBOSE_BEGIN));
            return true;
        }
        let command = Command::parse(&line);
        match self.do_command(i, &command, 0) {
            Ok(lines) => {
                let mut out = lines;
                out.push(RESP_OK.to_string());
                self.state.clients[i].send(&out);
                true
            }
            Err(Failure::Bpd(e)) => {
                let response = e.response();
                self.state.clients[i].send_line(&response);
                true
            }
            Err(Failure::Close) => {
                self.state.clients[i].closed = true;
                false
            }
            Err(Failure::Idle(subsystems)) => {
                self.state.clients[i].idle = subsystems;
                // A client that goes idle reports anything already queued.
                self.send_idle_events(i);
                true
            }
            Err(Failure::Shutdown) => {
                self.state.clients[i].send_line(RESP_OK);
                self.state.clients[i].closed = true;
                self.shutdown = true;
                false
            }
        }
    }

    /// Run one command on client `i`; `Ok` is its output lines.
    fn do_command(
        &mut self,
        i: usize,
        command: &Command,
        index: usize,
    ) -> std::result::Result<Vec<String>, Failure> {
        match self.state.run_command(i, command) {
            Ok(Outcome::Lines(lines)) => Ok(lines),
            Ok(Outcome::Close) => Err(Failure::Close),
            Ok(Outcome::Idle(subsystems)) => Err(Failure::Idle(subsystems)),
            Ok(Outcome::Shutdown) => Err(Failure::Shutdown),
            Err(mut e) => {
                e.cmd_name = command.name.clone();
                e.index = index;
                Err(Failure::Bpd(e))
            }
        }
    }

    /// A command list: every command in order; the first error ends it.
    fn run_command_list(&mut self, i: usize, commands: &[Command], verbose: bool) -> bool {
        for (index, command) in commands.iter().enumerate() {
            match self.do_command(i, command, index) {
                Ok(lines) => {
                    self.state.clients[i].send(&lines);
                    if verbose {
                        self.state.clients[i].send_line(RESP_CLIST_VERBOSE);
                    }
                }
                Err(Failure::Bpd(e)) => {
                    let response = e.response();
                    self.state.clients[i].send_line(&response);
                    return true;
                }
                Err(Failure::Close) => {
                    self.state.clients[i].closed = true;
                    return false;
                }
                Err(Failure::Shutdown) => {
                    self.shutdown = true;
                    self.state.clients[i].closed = true;
                    return false;
                }
                Err(Failure::Idle(_)) => {
                    let mut e = BpdError::new(ERROR_SYSTEM, "server error");
                    e.cmd_name = command.name.clone();
                    e.index = index;
                    let response = e.response();
                    self.state.clients[i].send_line(&response);
                    return true;
                }
            }
        }
        self.state.clients[i].send_line(RESP_OK);
        true
    }

    /// Send client `i` the events it's watching for, if any.
    fn send_idle_events(&mut self, i: usize) {
        let idle = self.state.clients[i].idle.clone();
        let events = events_for(&mut self.state.clients[i].pending, &idle);
        if events.is_empty() {
            return;
        }
        for event in &events {
            self.state.clients[i].send_line(&format!("changed: {event}"));
        }
        self.state.clients[i].idle.clear();
        self.state.clients[i].send_line(RESP_OK);
    }

    fn dispatch_events(&mut self) {
        for i in 0..self.state.clients.len() {
            if !self.state.clients[i].idle.is_empty() {
                self.send_idle_events(i);
            }
        }
    }

    /// The control socket: `ctrl_*` commands for debugging and player
    /// notifications.
    fn read_controls(&mut self) {
        for i in 0..self.controls.len() {
            loop {
                match self.controls[i].reader.next_line() {
                    Ok(Some(line)) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            break;
                        }
                        let command = Command::parse(&line);
                        if let Some(reply) = self.control_command(&command) {
                            let _ = writeln!(self.controls[i].writer, "{reply}");
                            let _ = self.controls[i].writer.flush();
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
        let closed: Vec<usize> = (0..self.controls.len())
            .filter(|&i| self.controls[i].reader.at_eof())
            .collect();
        for i in closed.into_iter().rev() {
            self.controls.remove(i);
        }
    }

    fn control_command(&mut self, command: &Command) -> Option<String> {
        match command.name.as_str() {
            "play_finished" => {
                if !command.args.is_empty() {
                    return Some(format!(
                        "ERROR: wrong number of arguments for \"{}\"",
                        command.name
                    ));
                }
                None
            }
            "nickname" => {
                if command.args.len() != 2 {
                    return Some(format!(
                        "ERROR: wrong number of arguments for \"{}\"",
                        command.name
                    ));
                }
                Some(format!("ERROR: no such client: {}", command.args[0]))
            }
            "profile" => Some(
                "ERROR: memory profiling needs guppy, which beets-rs doesn't have".to_string(),
            ),
            other => Some(format!("ERROR: unknown command \"{other}\"")),
        }
    }
}

/// The queued events a client is watching for, removed from its queue
/// (sorted, for a stable `changed:` order).
fn events_for(pending: &mut HashSet<String>, idle: &HashSet<String>) -> Vec<String> {
    let mut events: Vec<String> = pending.intersection(idle).cloned().collect();
    events.sort();
    for event in &events {
        pending.remove(event);
    }
    events
}

// --------------------------------------------------------------- the plugin

pub struct BpdPlugin;

impl crate::plugins::Plugin for BpdPlugin {
    fn name(&self) -> &'static str {
        "bpd"
    }
}

/// `beet bpd`'s arguments: HOST, PORT, CONTROL_PORT.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    pub args: Vec<String>,
}

/// `beet bpd`: run the MPD-compatible server until interrupted.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let log = crate::plugins::PluginLog::new("bpd");
    let mut args = opts.args.clone();
    let host = if args.is_empty() {
        cfg.str("bpd.host")
    } else {
        args.remove(0)
    };
    let port: u16 = if args.is_empty() {
        cfg.get("bpd.port")
            .and_then(|v| v.as_f64())
            .unwrap_or(6600.0) as u16
    } else {
        args.remove(0).trim().parse()?
    };
    let ctrl_port: u16 = if args.is_empty() {
        cfg.get("bpd.control_port")
            .and_then(|v| v.as_f64())
            .unwrap_or(6601.0) as u16
    } else {
        args.remove(0).trim().parse()?
    };
    if !args.is_empty() {
        anyhow::bail!("too many arguments");
    }
    let password = cfg.str("bpd.password");
    let volume = cfg
        .get("bpd.volume")
        .and_then(|v| v.as_f64())
        .unwrap_or(VOLUME_MAX as f64) as i64;
    log.info("Starting server...");
    let mut server = Server::new(cfg, lib, password, volume)?;
    server.bind(&host, port, &host, ctrl_port)?;
    let (mpd_port, ctrl) = server.ports().unwrap_or((port, ctrl_port));
    log.info(&format!(
        "Server ready and listening on {host}:{mpd_port} (player: {})",
        server.state.player.name()
    ));
    log.debug(&format!(
        "Listening for control signals on {host}:{ctrl}"
    ));
    server.run();
    Ok(())
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "bpd",
        order: 83,
        defaults: r#"bpd:
    host: ''
    port: 6600
    control_port: 6601
    password: ''
    volume: 100
"#,
        config_keys: &["bpd"],
        redacted: &["bpd.password"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 550,
                plugin: "bpd",
                build: build_bpd,
                run: run_bpd,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(BpdPlugin))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct BpdArgs {
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_bpd() -> clap::Command {
    use clap::Args as _;
    BpdArgs::augment_args(clap::Command::new("bpd").about(
        "Run an MPD-compatible music player server (plugin: bpd)",
    ))
}

pub fn run_bpd(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = BpdArgs::from_arg_matches(m)?;
    crate::plugins::bpd::command(
        cfg,
        lib,
        &crate::plugins::bpd::CmdOpts { args: a.args },
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
