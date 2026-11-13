//! Ported from beets' test/plugins/test_play.py: the player is never run
//! here; the command line and the playlist it would get are checked.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use serde_yaml::Value as Y;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("play.command", Y::String("echo".into()));
    let lib = Library::in_memory().unwrap();
    Env {
        _dir: dir,
        cfg,
        lib,
    }
}

impl Env {
    fn add(&self, title: &str, album: &str, track: i64) -> Item {
        let mut it = Item::new_item();
        it.set_str("title", title);
        it.set_str("album", album);
        it.set_str("artist", "Artist");
        it.set_str("albumartist", "Artist");
        it.set_int("track", track);
        let path = self
            .cfg
            .directory()
            .join("Artist")
            .join(album)
            .join(format!("{track:02} {title}.mp3"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
        it.set_path(&path);
        self.lib.add(&mut it).unwrap();
        it
    }

    fn plan(&self, opts: &CmdOpts) -> (Option<Plan>, String) {
        let mut out = vec![];
        let plan = plan(&self.cfg, &self.lib, opts, &mut out, &mut || Some('c')).unwrap();
        (plan, String::from_utf8(out).unwrap())
    }

    fn plan_with(&self, opts: &CmdOpts, answer: char) -> (Option<Plan>, String) {
        let mut out = vec![];
        let plan = plan(&self.cfg, &self.lib, opts, &mut out, &mut || Some(answer)).unwrap();
        (plan, String::from_utf8(out).unwrap())
    }
}

fn query(q: &[&str]) -> Vec<String> {
    q.iter().map(std::string::ToString::to_string).collect()
}

fn opts(q: &[&str]) -> CmdOpts {
    CmdOpts {
        query: query(q),
        ..Default::default()
    }
}

fn playlist_lines(plan: &Plan) -> Vec<String> {
    let p = &plan.open_args[0];
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn basic() {
    let e = env();
    let item = e.add("aNiceTitle", "a nice älbum", 1);
    let (plan, out) = e.plan(&opts(&["title:aNiceTitle"]));
    let plan = plan.unwrap();
    assert_eq!(plan.command, "echo");
    assert_eq!(plan.count, 1);
    assert_eq!(
        playlist_lines(&plan),
        vec![item.path().to_string_lossy().into_owned()]
    );
    // `play` prints the count.
    let mut printed = vec![];
    print_playing(&plan, &mut printed).unwrap();
    assert_eq!(String::from_utf8(printed).unwrap(), "Playing 1 track.\n");
    assert!(out.is_empty());
}

#[test]
fn album_option() {
    let e = env();
    let a = e.add("One", "a nice älbum", 1);
    let b = e.add("Two", "a nice älbum", 2);
    e.lib
        .add_album(&mut [a.clone(), b.clone()])
        .unwrap();
    let mut o = opts(&["nice"]);
    o.album = true;
    let (plan, _) = e.plan(&o);
    let plan = plan.unwrap();
    assert_eq!(plan.item_type, "album");
    assert_eq!(
        playlist_lines(&plan),
        vec![
            a.path().to_string_lossy().into_owned(),
            b.path().to_string_lossy().into_owned(),
        ]
    );
}

#[test]
fn args_option() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    let mut o = opts(&["title:aNiceTitle"]);
    o.args = Some("foo".into());
    assert_eq!(e.plan(&o).0.unwrap().command, "echo foo");
}

#[test]
fn args_option_in_middle() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    let mut e = e;
    e.cfg
        .set("play.command", Y::String("echo $args other".into()));
    let mut o = opts(&["title:aNiceTitle"]);
    o.args = Some("foo".into());
    assert_eq!(e.plan(&o).0.unwrap().command, "echo foo other");
    // Without -A the marker is dropped, with its leading space.
    assert_eq!(e.plan(&opts(&["title:aNiceTitle"])).0.unwrap().command, "echo other");
}

#[test]
fn relative_to() {
    let e = env();
    let item = e.add("aNiceTitle", "a nice älbum", 1);
    let mut e = e;
    let start = item.path().parent().unwrap().parent().unwrap();
    e.cfg
        .set("play.relative_to", Y::String(start.to_string_lossy().into()));
    let (plan, _) = e.plan(&opts(&["title:aNiceTitle"]));
    assert_eq!(
        playlist_lines(&plan.unwrap()),
        vec![
            item.path()
                .strip_prefix(start)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        ]
    );
}

#[test]
fn use_folders() {
    let e = env();
    let item = e.add("aNiceTitle", "a nice älbum", 1);
    e.lib.add_album(&mut [item.clone()]).unwrap();
    let mut e = e;
    // No command: the platform's opener.
    e.cfg.set("play.command", Y::Null);
    e.cfg.set("play.use_folders", Y::Bool(true));
    let mut o = opts(&["nice"]);
    o.album = true;
    let plan = e.plan(&o).0.unwrap();
    assert_eq!(plan.command, open_anything());
    assert_eq!(
        playlist_lines(&plan),
        vec![item
            .path()
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned()]
    );
}

#[test]
fn raw() {
    let e = env();
    let item = e.add("aNiceTitle", "a nice älbum", 1);
    let mut e = e;
    e.cfg.set("play.raw", Y::Bool(true));
    let (plan, _) = e.plan(&opts(&["nice"]));
    let plan = plan.unwrap();
    assert_eq!(plan.command, "echo");
    assert_eq!(plan.open_args, vec![item.path().to_path_buf()]);
}

#[test]
fn playlist_marker() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    let mut e = e;
    e.cfg.set(
        "play.command",
        Y::String("echo --some params --playlist=$playlist --some-more params".into()),
    );
    let plan = e.plan(&opts(&["nice"])).0.unwrap();
    // The playlist path goes into the command; no separate arguments.
    assert!(plan.command.starts_with("echo --some params --playlist="));
    assert!(plan.command.ends_with(" --some-more params"));
    assert_eq!(plan.command.matches("$playlist").count(), 0);
    assert!(plan.open_args.is_empty());
}

#[test]
fn playlist_marker_with_raw() {
    let e = env();
    let item = e.add("aNiceTitle", "a nice älbum", 1);
    let mut e = e;
    e.cfg.set("play.raw", Y::Bool(true));
    e.cfg
        .set("play.command", Y::String("echo -p $playlist -q".into()));
    let plan = e.plan(&opts(&["nice"])).0.unwrap();
    // beets replaces the marker with a space and keeps the paths.
    assert_eq!(plan.command, "echo -p   -q");
    assert_eq!(plan.open_args, vec![item.path().to_path_buf()]);
}

#[test]
fn not_found() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    let (plan, out) = e.plan(&opts(&["not found"]));
    assert!(plan.is_none());
    assert_eq!(out, "No track to play.\n");
}

#[test]
fn warning_threshold() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    e.add("another NiceTitle", "a nice älbum", 2);
    let mut e = e;
    e.cfg.set("play.warning_threshold", Y::Number(1.into()));
    let (plan, out) = e.plan_with(&opts(&["nice"]), 'a');
    assert!(plan.is_none());
    assert_eq!(out, "You are about to queue 2 tracks.\n");
}

#[test]
fn warning_threshold_continue() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    e.add("another NiceTitle", "a nice älbum", 2);
    let mut e = e;
    e.cfg.set("play.warning_threshold", Y::Number(1.into()));
    let (plan, out) = e.plan_with(&opts(&["nice"]), 'c');
    assert!(plan.is_some());
    assert_eq!(out, "You are about to queue 2 tracks.\n");
}

#[test]
fn warning_threshold_bypassed_with_yes() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    e.add("another NiceTitle", "a nice älbum", 2);
    let mut e = e;
    e.cfg.set("play.warning_threshold", Y::Number(1.into()));
    let mut o = opts(&["NiceTitle"]);
    o.yes = true;
    let (plan, out) = e.plan_with(&o, 'a');
    let plan = plan.unwrap();
    assert_eq!(out, "");
    assert_eq!(playlist_lines(&plan).len(), 2);
}

#[test]
fn randomize() {
    let e = env();
    let items: Vec<Item> = (1..=100)
        .map(|t| e.add(&format!("randomize {t:03}"), "randomize_test", t))
        .collect();
    let baseline: Vec<String> = items
        .iter()
        .map(|i| i.path().to_string_lossy().into_owned())
        .collect();
    let mut o = opts(&["album:randomize_test"]);
    o.randomize = true;
    let lines = playlist_lines(&e.plan(&o).0.unwrap());
    let mut sorted_baseline = baseline.clone();
    sorted_baseline.sort();
    let mut sorted_lines = lines.clone();
    sorted_lines.sort();
    assert_eq!(sorted_lines, sorted_baseline);
    assert_ne!(lines, baseline);
}

#[test]
fn bom() {
    let e = env();
    e.add("aNiceTitle", "a nice älbum", 1);
    let mut e = e;
    e.cfg.set("play.bom", Y::Bool(true));
    let plan = e.plan(&opts(&["nice"])).0.unwrap();
    let bytes = std::fs::read(&plan.open_args[0]).unwrap();
    assert!(bytes.starts_with(b"\xef\xbb\xbf"));
}

#[test]
fn command_str_markers() {
    assert_eq!(command_str("", None), open_anything());
    assert_eq!(command_str("mpv $args --no-video", Some("--x")), "mpv --x --no-video");
    assert_eq!(command_str("mpv $args --no-video", None), "mpv --no-video");
    assert_eq!(command_str("mpv --no-video $args", None), "mpv --no-video");
    assert_eq!(command_str("mpv", Some("--x")), "mpv --x");
}

#[test]
fn play_prints_the_plural() {
    let plan = Plan {
        command: "echo".into(),
        open_args: vec![],
        item_type: "album",
        count: 2,
    };
    let mut printed = vec![];
    print_playing(&plan, &mut printed).unwrap();
    assert_eq!(String::from_utf8(printed).unwrap(), "Playing 2 albums.\n");
}
