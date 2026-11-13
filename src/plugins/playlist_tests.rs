//! beets' `test/plugins/test_playlist.py`: `playlist:` queries in the three
//! `relative_to` modes, and the auto-update listeners.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind, Value};
use crate::query::Query;
use serde_yaml::Value as Y;
use std::sync::Mutex;
use tempfile::TempDir;

/// The auto-update state is process-wide, as beets' plugin instance is.
static LOCK: Mutex<()> = Mutex::new(());

struct Env {
    _dir: TempDir,
    music: PathBuf,
    playlists: PathBuf,
    cfg: Config,
    lib: Library,
}

fn env(relative_to: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    let playlists = dir.path().join("playlists");
    std::fs::create_dir_all(&playlists).unwrap();
    let mut cfg = Config::defaults();
    cfg.set("directory", Y::String(music.display().to_string()));
    cfg.set("plugins", serde_yaml::from_str("[playlist]").unwrap());
    cfg.set(
        "playlist.playlist_dir",
        Y::String(playlists.display().to_string()),
    );
    cfg.set("playlist.relative_to", Y::String(relative_to.into()));
    crate::query::configure(&cfg);
    let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
    let mut e = Env {
        _dir: dir,
        music,
        playlists,
        cfg,
        lib,
    };
    for (p, title, _album) in [
        ("a/b/c.mp3", "some item", "some album"),
        ("d/e/f.mp3", "another item", "another album"),
        ("x/y/z.mp3", "yet another item", "yet another album"),
    ] {
        let mut it = Item::new_item();
        it.set_path(e.music.join(p));
        it.set("title", Value::Str(title.into()));
        e.lib.add_album(&mut [it]).unwrap();
    }
    let _ = &mut e;
    e
}

impl Env {
    fn path(&self, p: &str) -> PathBuf {
        self.music.join(p)
    }

    fn write_absolute(&self) {
        let lines = [
            self.path("a/b/c.mp3"),
            self.path("d/e/f.mp3"),
            self.path("nonexisting.mp3"),
        ];
        write_lines(&self.playlists.join("absolute.m3u"), &lines);
    }

    fn write_relative(&self) {
        let lines = [
            PathBuf::from("a/b/c.mp3"),
            PathBuf::from("d/e/f.mp3"),
            PathBuf::from("nonexisting.mp3"),
        ];
        write_lines(&self.playlists.join("relative.m3u"), &lines);
    }

    fn write_relative_to_playlist(&self) {
        let pl = &self.playlists;
        let lines = [
            relpath(&self.path("a/b/c.mp3"), pl),
            relpath(&self.path("d/e/f.mp3"), pl),
            relpath(&self.path("nonexisting.mp3"), pl),
        ];
        write_lines(&self.playlists.join("relative.m3u"), &lines);
    }

    fn titles(&self, q: &str) -> Vec<String> {
        let query = Query::parse(Kind::Item, &[q]).unwrap();
        let mut t: Vec<String> = self
            .lib
            .items(&query)
            .unwrap()
            .iter()
            .map(|i| i.str("title").to_string())
            .collect();
        t.sort_unstable();
        t
    }

    fn item(&self, p: &str) -> Item {
        let query = Query::path(Kind::Item, "path", &self.path(p).display().to_string());
        self.lib.items(&query).unwrap().pop().unwrap()
    }
}

fn write_lines(path: &Path, lines: &[PathBuf]) {
    let text: Vec<String> = lines.iter().map(|p| p.display().to_string()).collect();
    std::fs::write(path, text.join("\n")).unwrap();
}

#[test]
fn query_with_absolute_paths_in_playlist() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = env("library");
    e.write_absolute();
    assert_eq!(e.titles("playlist:absolute"), ["another item", "some item"]);
    assert_eq!(
        e.titles(&format!(
            "playlist:{}",
            e.playlists.join("absolute.m3u").display()
        )),
        ["another item", "some item"]
    );
}

#[test]
fn query_with_relative_paths_in_playlist() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = env("library");
    e.write_relative();
    assert_eq!(e.titles("playlist:relative"), ["another item", "some item"]);
    assert_eq!(
        e.titles(&format!(
            "playlist:{}",
            e.playlists.join("relative.m3u").display()
        )),
        ["another item", "some item"]
    );
}

#[test]
fn query_relative_to_a_directory() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut e = env("library");
    e.cfg.set(
        "playlist.relative_to",
        Y::String(e.music.display().to_string()),
    );
    crate::query::configure(&e.cfg);
    e.write_relative();
    assert_eq!(e.titles("playlist:relative"), ["another item", "some item"]);
}

#[test]
fn query_relative_to_the_playlist() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = env("playlist");
    e.write_relative_to_playlist();
    assert_eq!(e.titles("playlist:relative"), ["another item", "some item"]);
}

#[test]
fn nonexisting_playlists_match_nothing() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = env("library");
    assert!(e.titles("playlist:nonexisting").is_empty());
    assert!(e.titles("playlist:/does/not/exist.m3u").is_empty());
}

fn update_env(relative_to: &str) -> Env {
    let mut e = env(relative_to);
    e.cfg.set("playlist.auto", Y::Bool(true));
    e.cfg
        .set("playlist.relative_to", Y::String(relative_to.into()));
    crate::query::configure(&e.cfg);
    e.write_absolute();
    e.write_relative();
    e
}

#[test]
fn item_moved_updates_playlists() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = update_env("library");
    let item = e.item("d/e/f.mp3");
    item_moved(&e.cfg, item.path(), &e.path("g/h/i.mp3"));
    // An item that isn't in a playlist.
    let item = e.item("x/y/z.mp3");
    item_moved(&e.cfg, item.path(), &e.path("u/v/w.mp3"));
    cli_exit(&e.cfg, &e.lib);

    let expected = [
        e.path("a/b/c.mp3"),
        e.path("g/h/i.mp3"),
        e.path("nonexisting.mp3"),
    ];
    assert_eq!(read_paths(&e.playlists.join("absolute.m3u")), expected);
    assert_eq!(
        read_paths(&e.playlists.join("relative.m3u")),
        [
            PathBuf::from("a/b/c.mp3"),
            PathBuf::from("g/h/i.mp3"),
            PathBuf::from("nonexisting.mp3"),
        ]
    );
}

#[test]
fn item_removed_updates_playlists() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = update_env("library");
    let item = e.item("d/e/f.mp3");
    item_removed(&e.cfg, item.path());
    let item = e.item("x/y/z.mp3");
    item_removed(&e.cfg, item.path());
    cli_exit(&e.cfg, &e.lib);

    let expected = [e.path("a/b/c.mp3"), e.path("nonexisting.mp3")];
    assert_eq!(read_paths(&e.playlists.join("absolute.m3u")), expected);
    assert_eq!(
        read_paths(&e.playlists.join("relative.m3u")),
        [PathBuf::from("a/b/c.mp3"), PathBuf::from("nonexisting.mp3")]
    );
}

#[test]
fn auto_off_keeps_playlists() {
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let e = env("library");
    e.write_absolute();
    let item = e.item("d/e/f.mp3");
    item_moved(&e.cfg, item.path(), &e.path("g/h/i.mp3"));
    cli_exit(&e.cfg, &e.lib);
    assert_eq!(
        read_paths(&e.playlists.join("absolute.m3u")),
        [
            e.path("a/b/c.mp3"),
            e.path("d/e/f.mp3"),
            e.path("nonexisting.mp3")
        ]
    );
}

fn read_paths(path: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect()
}
