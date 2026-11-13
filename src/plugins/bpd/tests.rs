//! Ported from beets' test/plugins/test_bpd.py: the MPD protocol over
//! this port's server (no playback). The server runs its event loop on the
//! test's thread (`poll_once`) while the client sits in a thread of its
//! own.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Value};
use serde_yaml::Value as Y;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};
use tempfile::TempDir;

// ------------------------------------------------------------ the client

/// An MPD response.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub lines: Vec<(String, String)>,
    /// `(code, index, command, message)` for an `ACK`.
    pub err: Option<(i32, usize, String, String)>,
    /// The `list_OK` delimiters of a verbose command list.
    pub list_ok: usize,
}

impl Response {
    fn ok(&self) -> bool {
        self.err.is_none()
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.lines
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of a repeated header.
    fn values(&self, key: &str) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn keys(&self) -> std::collections::BTreeSet<&str> {
        self.lines.iter().map(|(k, _)| k.as_str()).collect()
    }
}

struct Mpc {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Mpc {
    fn connect(port: u16) -> Mpc {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        Mpc { stream, buf: vec![] }
    }

    fn readline(&mut self) -> String {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                return line;
            }
            let mut chunk = [0u8; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => panic!("connection closed"),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) => panic!("read failed: {e}"),
            }
        }
    }

    fn hello(&mut self) -> String {
        self.readline()
    }

    fn write(&mut self, text: &str) {
        self.stream.write_all(text.as_bytes()).unwrap();
    }

    fn send(&mut self, command: &str) {
        self.write(&format!("{command}\n"));
    }

    /// Read one response (a command list's delimiters included).
    fn response(&mut self) -> Response {
        let mut lines = vec![];
        let mut list_ok = 0;
        loop {
            let line = self.readline();
            if line == RESP_OK {
                return Response {
                    lines,
                    err: None,
                    list_ok,
                };
            }
            if line == RESP_CLIST_VERBOSE {
                list_ok += 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix("ACK") {
                // "ACK [5@0] {name} message"
                let inner = rest.trim_start();
                let inner = inner.strip_prefix('[').expect("an ACK bracket");
                let (code_pos, inner) = inner.split_once(']').expect("a closing bracket");
                let (code, pos) = code_pos.split_once('@').expect("a code@pos");
                let inner = inner.trim_start();
                let inner = inner.strip_prefix('{').expect("a command name");
                let (cmd, msg) = inner.split_once('}').expect("a closing brace");
                return Response {
                    lines,
                    err: Some((
                        code.parse().unwrap(),
                        pos.parse().unwrap(),
                        cmd.to_string(),
                        msg.trim_start().to_string(),
                    )),
                    list_ok,
                };
            }
            match line.split_once(": ") {
                Some((k, v)) => lines.push((k.to_string(), v.to_string())),
                None => lines.push((line, String::new())),
            }
        }
    }

    fn command(&mut self, command: &str) -> Response {
        self.send(command);
        self.response()
    }
}

// ------------------------------------------------------------- the server

struct Env {
    #[allow(dead_code)]
    dir: TempDir,
    cfg: Config,
    lib: Library,
}

fn env(password: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("bpd.password", Y::String(password.into()));
    let lib = Library::open_with(&dir.path().join("lib.db"), Some(&libdir), false).unwrap();
    let mut items = vec![];
    for (track, title) in [(1, "Track One Title"), (2, "Track Two Title")] {
        let path = libdir
            .join("Artist Name")
            .join("Album Title")
            .join(format!("{track:02} {title}.mp3"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
        let mut item = Item::new_item();
        item.set_str("title", title);
        item.set_int("track", track);
        item.set_str("album", "Album Title");
        item.set_str("artist", "Artist Name");
        item.set_str("albumartist", "Artist Name");
        item.set("length", Value::Float(1.0));
        item.set_int("bitrate", 320_000);
        item.set_int("samplerate", 44_100);
        item.set_int("bitdepth", 16);
        item.set_int("channels", 2);
        item.set_path(&path);
        lib.add(&mut item).unwrap();
        items.push(item);
    }
    lib.add_album(&mut items).unwrap();
    Env { dir, cfg, lib }
}

/// Run `f` (the client side) while the server's event loop runs here.
fn serve<R: Send + 'static>(
    env: &Env,
    volume: i64,
    f: impl FnOnce(u16, u16) -> R + Send + 'static,
) -> R {
    let password = env.cfg.str("bpd.password");
    let mut server = Server::new(&env.cfg, &env.lib, password, volume).unwrap();
    server.bind("127.0.0.1", 0, "127.0.0.1", 0).unwrap();
    let (port, ctrl) = server.ports().unwrap();
    let handle = std::thread::spawn(move || f(port, ctrl));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !handle.is_finished() {
        server.poll_once();
        assert!(Instant::now() < deadline, "the test client timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    handle.join().unwrap()
}

/// An error response's `(code, index, command, message)`.
fn err(r: &Response) -> (i32, usize, String, String) {
    r.err.clone().expect("an ACK response")
}

fn client(env: &Env, f: impl FnOnce(&mut Mpc) + Send + 'static) {
    serve(env, 100, move |port, _| {
        let mut c = Mpc::connect(port);
        assert_eq!(c.hello(), "OK MPD 0.16.0");
        f(&mut c);
    });
}

// ---------------------------------------------------- command parsing

#[test]
fn parses_commands() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("command", "command", &[]),
        ("command hello", "command", &["hello"]),
        ("command hello there", "command", &["hello", "there"]),
        (r#"command "hello there""#, "command", &["hello there"]),
        (
            r#"command "hello there" sir"#,
            "command",
            &["hello there", "sir"],
        ),
        (r#"command "hello \" there""#, "command", &[r#"hello " there"#]),
        (r#"command "hello \\ there""#, "command", &[r"hello \ there"]),
    ];
    for (input, name, args) in cases {
        let c = Command::parse(input);
        assert_eq!(&c.name, name, "{input}");
        assert_eq!(c.args, args.to_vec(), "{input}");
    }
}

// ------------------------------------------------------------- protocol

#[test]
fn unknown_command() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("notacommand");
        let (code, pos, cmd, msg) = err(&r);
        assert_eq!(code, ERROR_UNKNOWN);
        assert_eq!(pos, 0);
        assert_eq!(cmd, "notacommand");
        assert_eq!(msg, "unknown command \"notacommand\"");
    });
}

#[test]
fn mistakes_in_arguments() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("ping extra argument");
        assert_eq!(err(&r).0, ERROR_ARG);
        let r = c.command("add");
        let (code, _, cmd, msg) = err(&r);
        assert_eq!(code, ERROR_ARG);
        assert_eq!(cmd, "add");
        assert_eq!(msg, "wrong number of arguments for \"add\"");
    });
}

#[test]
fn system_error() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("crash");
        let (code, _, cmd, msg) = err(&r);
        assert_eq!(code, ERROR_SYSTEM);
        assert_eq!(cmd, "crash");
        assert_eq!(msg, "server error");
    });
}

#[test]
fn empty_request() {
    let env = env("");
    client(&env, |c| {
        c.send("");
        let r = c.response();
        let (code, _, _, msg) = err(&r);
        assert_eq!(code, ERROR_UNKNOWN);
        assert_eq!(msg, "No command given");
    });
}

#[test]
fn ping_and_commands() {
    let env = env("");
    client(&env, |c| {
        assert!(c.command("ping").ok());
        let r = c.command("commands");
        let names = r.values("command");
        for cmd in [
            "clearerror", "status", "search", "find", "list", "lsinfo", "add", "idle",
            "close", "kill",
        ] {
            assert!(names.contains(&cmd), "missing {cmd}");
        }
        let r = c.command("notcommands");
        assert!(r.values("command").is_empty());
    });
}

#[test]
fn command_lists() {
    let env = env("");
    client(&env, |c| {
        c.write("command_list_begin\nping\nnotacommand\nping\ncommand_list_end\n");
        let r = c.response();
        let (code, pos, cmd, _) = err(&r);
        assert_eq!((code, pos), (ERROR_UNKNOWN, 1));
        assert_eq!(cmd, "notacommand");
        // The next command runs normally again.
        assert!(c.command("ping").ok());
        // A verbose list delimits each command.
        c.write("command_list_ok_begin\nping\nstatus\ncommand_list_end\n");
        let r = c.response();
        assert!(r.ok());
        assert_eq!(r.list_ok, 2);
        // A command list's error stops it.
        c.write("command_list_begin\nping\nadd\nping\ncommand_list_end\n");
        let r = c.response();
        let (_, pos, cmd, _) = err(&r);
        assert_eq!(pos, 1);
        assert_eq!(cmd, "add");
    });
}

#[test]
fn password_auth() {
    let env = env("abc123");
    client(&env, |c| {
        let r = c.command("status");
        assert_eq!(r.err.unwrap().0, ERROR_PERMISSION);
        // Only the safe commands are listed.
        let names: Vec<String> = c
            .command("commands")
            .values("command")
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(names.len(), SAFE_COMMANDS.len());
        let r = c.command("password wrong");
        assert_eq!(r.err.unwrap().0, ERROR_PASSWORD);
        assert!(c.command("password \"abc123\"").ok());
        assert!(c.command("status").ok());
    });
}

// ------------------------------------------------------------- playback

fn add_items(c: &mut Mpc, paths: &[&str]) {
    for path in paths {
        let r = c.command(&format!("add \"{path}\""));
        assert!(r.ok(), "{:?}", r.err);
    }
}

#[test]
fn playback_state() {
    let env = env("");
    client(&env, |c| {
        // Nothing playing.
        let r = c.command("status");
        assert_eq!(r.value("state"), Some("stop"));
        assert_eq!(r.value("playlistlength"), Some("0"));
        // Add both items.
        add_items(
            c,
            &[
                "Artist Name/Album Title/01 Track One Title.mp3",
                "Artist Name/Album Title/02 Track Two Title.mp3",
            ],
        );
        let r = c.command("status");
        assert_eq!(r.value("playlistlength"), Some("2"));
        // Play, then check the playing fields.
        assert!(c.command("play").ok());
        let r = c.command("status");
        assert_eq!(r.value("state"), Some("play"));
        assert_eq!(r.value("song"), Some("0"));
        assert_eq!(r.value("nextsong"), Some("1"));
        assert_eq!(r.value("bitrate"), Some("320.0"));
        assert_eq!(r.value("audio"), Some("44100:16:2"));
        assert_eq!(r.value("duration"), Some("1.000"));
        assert_eq!(r.value("elapsed"), Some("0.000"));
        // Currentsong has the item's info.
        let r = c.command("currentsong");
        assert_eq!(r.value("file"), Some("Artist Name/Album Title/01 Track One Title.mp3"));
        assert_eq!(r.value("Title"), Some("Track One Title"));
        assert_eq!(r.value("Track"), Some("1"));
        assert_eq!(r.value("Album"), Some("Album Title"));
        assert_eq!(r.value("Artist"), Some("Artist Name"));
        assert_eq!(r.value("Pos"), Some("0"));
        assert_eq!(r.value("Id"), Some("1"));
        // Pause, seek, stop.
        assert!(c.command("pause").ok());
        assert_eq!(c.command("status").value("state"), Some("pause"));
        assert!(c.command("pause 0").ok());
        assert_eq!(c.command("status").value("state"), Some("play"));
        assert!(c.command("seek 1 0.5").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("2"));
        assert!(c.command("stop").ok());
        assert_eq!(c.command("status").value("state"), Some("stop"));
        assert!(c.command("currentsong").lines.is_empty());
    });
}

#[test]
fn next_and_previous() {
    let env = env("");
    client(&env, |c| {
        add_items(
            c,
            &[
                "Artist Name/Album Title/01 Track One Title.mp3",
                "Artist Name/Album Title/02 Track Two Title.mp3",
            ],
        );
        assert!(c.command("play").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("1"));
        assert!(c.command("next").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("2"));
        // Off the end: stopped.
        assert!(c.command("next").ok());
        assert_eq!(c.command("status").value("state"), Some("stop"));
        // Previous from a stopped state starts at the first item.
        assert!(c.command("previous").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("1"));
        // And previous from the first item wraps to 0 without repeat.
        assert!(c.command("previous").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("1"));
    });
}

#[test]
fn repeat_single_consume_random() {
    let env = env("");
    client(&env, |c| {
        add_items(
            c,
            &[
                "Artist Name/Album Title/01 Track One Title.mp3",
                "Artist Name/Album Title/02 Track Two Title.mp3",
            ],
        );
        // repeat: next from the last item wraps around.
        assert!(c.command("repeat 1").ok());
        assert!(c.command("play 1").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("2"));
        assert!(c.command("next").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("1"));
        // single with repeat: next stays on the same item.
        assert!(c.command("single 1").ok());
        assert!(c.command("next").ok());
        assert_eq!(c.command("currentsong").value("Id"), Some("1"));
        assert_eq!(c.command("status").value("state"), Some("play"));
        // single without repeat: next stops.
        assert!(c.command("repeat 0").ok());
        assert!(c.command("next").ok());
        assert_eq!(c.command("status").value("state"), Some("stop"));
        assert!(c.command("single 0").ok());
        // consume: playing removes the old entry.
        assert!(c.command("consume 1").ok());
        assert!(c.command("play 0").ok());
        assert!(c.command("next").ok());
        let info = c.command("playlistinfo");
        assert_eq!(info.values("Id"), vec!["2"]);
        assert_eq!(c.command("status").value("consume"), Some("1"));
        // random mode changes options.
        assert!(c.command("random 1").ok());
        assert_eq!(c.command("status").value("random"), Some("1"));
    });
}

#[test]
fn options_and_volume() {
    let env = env("");
    client(&env, |c| {
        // No xfade until it's set.
        let r = c.command("status");
        assert!(r.value("xfade").is_none());
        assert!(c.command("crossfade 123").ok());
        assert_eq!(c.command("status").value("xfade"), Some("123"));
        assert_eq!(c.command("crossfade -2").err.unwrap().0, ERROR_ARG);
        assert_eq!(c.command("crossfade 0.5").err.unwrap().0, ERROR_ARG);
        // mixramp.
        assert!(c.command("mixrampdb -17").ok());
        assert!(c.command("mixrampdelay 2").ok());
        let r = c.command("status");
        assert_eq!(r.value("mixrampdb"), Some("-17.0"));
        assert_eq!(r.value("mixrampdelay"), Some("2.0"));
        // nan clears mixrampdelay.
        assert!(c.command("mixrampdelay nan").ok());
        assert!(c.command("status").value("mixrampdelay").is_none());
        assert_eq!(c.command("mixrampdelay -2").err.unwrap().0, ERROR_ARG);
        // Volume.
        assert!(c.command("setvol 67").ok());
        assert_eq!(c.command("status").value("volume"), Some("67"));
        assert!(c.command("volume 5").ok());
        assert!(c.command("volume -2").ok());
        assert_eq!(c.command("status").value("volume"), Some("70"));
        assert_eq!(c.command("setvol 101").err.unwrap().0, ERROR_ARG);
        // Replay gain.
        assert!(c.command("replay_gain_mode track").ok());
        let r = c.command("replay_gain_status");
        assert_eq!(r.value("replay_gain_mode"), Some("track"));
        assert_eq!(c.command("replay_gain_mode nope").err.unwrap().0, ERROR_ARG);
    });
}

// -------------------------------------------------------------- playlist

#[test]
fn playlist_commands() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("addid \"Artist Name/Album Title/01 Track One Title.mp3\"");
        assert!(r.ok());
        assert_eq!(r.value("Id"), Some("1"));
        add_items(c, &["Artist Name/Album Title/02 Track Two Title.mp3"]);
        // playlistinfo ranges.
        let r = c.command("playlistinfo");
        assert_eq!(r.values("Id"), vec!["1", "2"]);
        assert_eq!(c.command("playlistinfo 0").value("Id"), Some("1"));
        let r = c.command("playlistinfo 0:2");
        assert_eq!(r.values("Id"), vec!["1", "2"]);
        assert_eq!(c.command("playlistinfo 200").err.unwrap().0, ERROR_ARG);
        // playlistid.
        assert_eq!(
            c.command("playlistid 2").value("Title"),
            Some("Track Two Title")
        );
        let r = c.command("playlistid");
        assert_eq!(r.values("Track"), vec!["1", "2"]);
        // The path reported back is accepted again.
        let reported = c.command("playlistinfo").value("file").unwrap().to_string();
        assert!(c.command(&format!("add \"{reported}\"")).ok());
        // plchanges / plchangesposid.
        let r = c.command("plchanges 0");
        assert_eq!(r.values("Id").len(), 3);
        let r = c.command("plchangesposid 0");
        assert_eq!(r.values("cpos"), vec!["0", "1", "2"]);
        // clear, delete, deleteid, move, swap.
        assert!(c.command("clear").ok());
        assert_eq!(c.command("status").value("playlistlength"), Some("0"));
        add_items(
            c,
            &[
                "Artist Name/Album Title/01 Track One Title.mp3",
                "Artist Name/Album Title/02 Track Two Title.mp3",
            ],
        );
        assert!(c.command("delete 0").ok());
        assert_eq!(c.command("playlistinfo").values("Id"), vec!["2"]);
        assert_eq!(c.command("delete 5").err.unwrap().0, ERROR_ARG);
        assert!(c.command("clear").ok());
        add_items(
            c,
            &[
                "Artist Name/Album Title/01 Track One Title.mp3",
                "Artist Name/Album Title/02 Track Two Title.mp3",
            ],
        );
        assert!(c.command("deleteid 1").ok());
        assert_eq!(c.command("playlistinfo").values("Id"), vec!["2"]);
        assert_eq!(c.command("deleteid 99").err.unwrap().0, ERROR_NO_EXIST);
        assert!(c.command("clear").ok());
        add_items(
            c,
            &[
                "Artist Name/Album Title/01 Track One Title.mp3",
                "Artist Name/Album Title/02 Track Two Title.mp3",
            ],
        );
        assert!(c.command("swap 0 1").ok());
        assert_eq!(c.command("playlistinfo").values("Id"), vec!["2", "1"]);
        assert!(c.command("swapid 1 2").ok());
        assert_eq!(c.command("playlistinfo").values("Id"), vec!["1", "2"]);
        assert!(c.command("move 0 1").ok());
        assert_eq!(c.command("playlistinfo").values("Id"), vec!["2", "1"]);
        assert!(c.command("move 0 5").ok());
    });
}

// ---------------------------------------------------- stored playlists

#[test]
fn stored_playlists_are_disabled() {
    let env = env("");
    client(&env, |c| {
        assert_eq!(c.command("listplaylist x").err.unwrap().0, ERROR_NO_EXIST);
        assert_eq!(
            c.command("listplaylistinfo x").err.unwrap().0,
            ERROR_NO_EXIST
        );
        assert_eq!(c.command("listplaylists").err.unwrap().0, ERROR_UNKNOWN);
        assert_eq!(c.command("load x").err.unwrap().0, ERROR_NO_EXIST);
        for cmd in [
            "playlistclear x",
            "playlistdelete x 0",
            "playlistmove x 0 1",
            "rename x y",
            "rm x",
            "save x",
        ] {
            assert_eq!(c.command(cmd).err.unwrap().0, ERROR_UNKNOWN, "{cmd}");
        }
        assert_eq!(
            c.command("playlistadd x y").err.unwrap().0,
            ERROR_UNKNOWN
        );
    });
}

// ----------------------------------------------------------- the library

#[test]
fn search_find_list_count() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("search track 1");
        assert_eq!(r.value("Title"), Some("Track One Title"));
        let r = c.command("search any 1");
        assert_eq!(r.value("Title"), Some("Track One Title"));
        // find is exact.
        assert_eq!(c.command("find title \"Track One\"").lines, vec![]);
        let r = c.command("find title \"Track One Title\"");
        assert_eq!(r.value("Title"), Some("Track One Title"));
        assert_eq!(c.command("search nope x").err.unwrap().0, ERROR_UNKNOWN);
        // list.
        assert_eq!(c.command("list album").value("Album"), Some("Album Title"));
        assert_eq!(c.command("list track").values("Track"), vec!["1", "2"]);
        // The short form: an artist's albums.
        assert_eq!(
            c.command("list album \"Artist Name\"").value("Album"),
            Some("Album Title")
        );
        assert_eq!(
            c.command("list track \"Artist Name\"").err.unwrap().0,
            ERROR_ARG
        );
        assert_eq!(
            c.command("list album artist \"Artist Name\" track")
                .err
                .unwrap()
                .0,
            ERROR_ARG
        );
        // count.
        let r = c.command("count track 1");
        assert_eq!(r.value("songs"), Some("1"));
        assert_eq!(r.value("playtime"), Some("1"));
    });
}

#[test]
fn vfs_browsing() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("lsinfo");
        assert_eq!(r.value("directory"), Some("Artist Name"));
        let r = c.command("lsinfo \"Artist Name\"");
        assert_eq!(r.value("directory"), Some("Artist Name/Album Title"));
        let r = c.command("lsinfo \"Artist Name/Album Title\"");
        assert_eq!(
            r.values("Title"),
            vec!["Track One Title", "Track Two Title"]
        );
        // A file isn't a directory.
        assert_eq!(
            c.command("lsinfo \"Artist Name/Album Title/01 Track One Title.mp3\"")
                .err
                .unwrap()
                .3,
            "this is not a directory"
        );
        // listall and listallinfo.
        let r = c.command("listall");
        assert_eq!(
            r.values("file"),
            vec![
                "/Artist Name/Album Title/01 Track One Title.mp3",
                "/Artist Name/Album Title/02 Track Two Title.mp3",
            ]
        );
        assert_eq!(
            r.values("directory"),
            vec!["/Artist Name", "/Artist Name/Album Title"]
        );
        let r = c.command("listallinfo");
        assert_eq!(r.values("Title").len(), 2);
        // A single file through listall.
        let r = c.command("listall \"Artist Name/Album Title/01 Track One Title.mp3\"");
        assert_eq!(
            r.value("file"),
            Some("Artist Name/Album Title/01 Track One Title.mp3")
        );
        // add of a whole directory.
        assert!(c.command("add \"Artist Name/Album Title\"").ok());
        assert_eq!(c.command("status").value("playlistlength"), Some("2"));
        // A missing path.
        assert_eq!(c.command("lsinfo nope").err.unwrap().0, ERROR_NO_EXIST);
        assert_eq!(c.command("add nope").err.unwrap().0, ERROR_NO_EXIST);
        // update rebuilds the tree.
        assert!(c.command("update").ok());
    });
}

#[test]
fn stats_and_tagtypes() {
    let env = env("");
    client(&env, |c| {
        let r = c.command("stats");
        assert_eq!(
            r.keys(),
            ["artists", "albums", "songs", "uptime", "playtime", "db_playtime", "db_update"]
                .into_iter()
                .collect()
        );
        assert_eq!(r.value("artists"), Some("1"));
        assert_eq!(r.value("albums"), Some("1"));
        assert_eq!(r.value("songs"), Some("2"));
        assert_eq!(r.value("db_playtime"), Some("2"));
        let r = c.command("tagtypes");
        let tags = r.values("tagtype");
        assert!(tags.contains(&"Artist"));
        assert!(tags.contains(&"MUSICBRAINZ_TRACKID"));
        // Item info has every tagtype and the metadata headers.
        let r = c.command(&format!(
            "add \"{}\"",
            "Artist Name/Album Title/01 Track One Title.mp3"
        ));
        assert!(r.ok());
        let r = c.command("playlistinfo 0");
        let mut expected: std::collections::BTreeSet<&str> =
            ["Pos", "Time", "Id", "file", "duration"].into_iter().collect();
        for (tag, _) in TAGTYPE_MAP {
            expected.insert(tag);
        }
        assert_eq!(r.keys(), expected);
        // outputs and decoders.
        let r = c.command("outputs");
        assert_eq!(r.value("outputid"), Some("0"));
        assert_eq!(r.value("outputenabled"), Some("1"));
        let r = c.command("decoders");
        assert_eq!(r.values("plugin")[0], "mp3");
        assert!(r.values("suffix").contains(&"mp3"));
        assert!(r.values("mime_type").contains(&"audio/mpeg"));
        assert!(c.command("enableoutput 0").ok());
        assert_eq!(c.command("enableoutput 1").err.unwrap().0, ERROR_ARG);
        assert_eq!(c.command("disableoutput 0").err.unwrap().0, ERROR_ARG);
        assert_eq!(c.command("disableoutput 1").err.unwrap().0, ERROR_ARG);
    });
}

// ------------------------------------------------------------------ idle

#[test]
fn idle_and_events() {
    let env = env("");
    serve(&env, 100, move |port, _| {
        let mut idle = Mpc::connect(port);
        idle.hello();
        // An event already queued when idle starts is reported at once.
        assert!(idle.command("random 1").ok());
        let mut other = Mpc::connect(port);
        other.hello();
        // Queue an event from another client first.
        assert!(other.command("repeat 1").ok());
        let r = idle.command("idle options");
        assert_eq!(r.values("changed"), vec!["options"]);
        // noidle with nothing pending.
        idle.send("idle");
        std::thread::sleep(Duration::from_millis(20));
        let r = idle.command("noidle");
        assert!(r.ok());
        assert!(r.values("changed").is_empty());
        // A command while idle is an error and closes the connection.
        idle.send("idle");
        std::thread::sleep(Duration::from_millis(20));
        idle.send("status");
        let r = idle.response();
        assert_eq!(r.err.unwrap().0, ERROR_UNKNOWN);
        // noidle when not idle sends no response.
        let mut third = Mpc::connect(port);
        third.hello();
        third.send("noidle");
        let r = third.command("notacommand");
        assert_eq!(r.err.unwrap().0, ERROR_UNKNOWN);
    });
}

// -------------------------------------------------------------- control

#[test]
fn control_socket() {
    let env = env("");
    serve(&env, 100, move |port, ctrl| {
        let mut c = Mpc::connect(port);
        c.hello();
        assert!(c.command("ping").ok());
        let stream = TcpStream::connect(("127.0.0.1", ctrl)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = std::io::BufReader::new(stream);
        writer.write_all(b"notacommand\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ERROR: unknown command \"notacommand\"");
        writer.write_all(b"nickname a b\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ERROR: no such client: a");
        writer.write_all(b"play_finished\n").unwrap();
        // No reply; the connection still works.
        writer.write_all(b"nope\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("ERROR: unknown command"));
        assert!(c.command("ping").ok());
    });
}

#[test]
fn kill_stops_the_server() {
    let env = env("");
    let mut server = Server::new(&env.cfg, &env.lib, String::new(), 100).unwrap();
    server.bind("127.0.0.1", 0, "127.0.0.1", 0).unwrap();
    let port = server.ports().unwrap().0;
    let handle = std::thread::spawn(move || {
        let mut c = Mpc::connect(port);
        c.hello();
        assert!(c.command("kill").ok());
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !server.is_stopped() && Instant::now() < deadline {
        server.poll_once();
    }
    assert!(server.is_stopped());
    handle.join().unwrap();
}

#[test]
fn close_closes_the_connection() {
    let env = env("");
    client(&env, |c| {
        c.send("close");
        // The server closes without a response: the next read sees EOF.
        std::thread::sleep(Duration::from_millis(20));
        let mut chunk = [0u8; 16];
        let n = c.stream.read(&mut chunk).unwrap_or(0);
        assert_eq!(n, 0);
    });
}
