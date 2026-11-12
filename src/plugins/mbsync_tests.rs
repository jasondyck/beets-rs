//! beets' test/plugins/test_mbsync.py (with a fake metadata source where
//! beets patches `metadata_plugins.album_for_id` / `track_for_id`), and
//! the track mapping.

use super::*;
use crate::model::Album;
use std::cell::RefCell;
use tempfile::TempDir;

/// Answers every lookup with the same album / track, and counts them.
#[derive(Default)]
struct Fake {
    album: Option<AlbumInfo>,
    track: Option<TrackInfo>,
    asked: RefCell<Vec<String>>,
}

impl Source for Fake {
    fn album_for_id(&self, id: &str) -> Result<Option<AlbumInfo>> {
        self.asked.borrow_mut().push(format!("album {id}"));
        Ok(self.album.clone())
    }
    fn track_for_id(&self, id: &str) -> Result<Option<TrackInfo>> {
        self.asked.borrow_mut().push(format!("track {id}"));
        Ok(self.track.clone())
    }
}

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set(
            "plugins",
            serde_yaml::from_str("[musicbrainz, mbsync]").unwrap(),
        );
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env {
            _dir: dir,
            cfg,
            lib,
        }
    }

    fn item(&self, pairs: &[(&str, &str)]) -> Item {
        let mut it = Item::new_item();
        it.set_path(format!("/nonexistent/{}.mp3", pairs.len()));
        for (k, v) in pairs {
            it.set_parsed(k, v).unwrap();
        }
        it
    }

    fn add_album(&self, pairs: &[(&str, &str)]) -> Item {
        let mut items = [self.item(pairs)];
        self.lib.add_album(&mut items).unwrap();
        let [it] = items;
        it
    }

    fn add(&self, pairs: &[(&str, &str)]) -> Item {
        let mut it = self.item(pairs);
        self.lib.add(&mut it).unwrap();
        it
    }

    /// `beet mbsync` (not writing: the files don't exist), its log.
    fn run(&self, source: &Fake, opts: CmdOpts) -> Vec<String> {
        let opts = CmdOpts {
            write: Some(false),
            ..opts
        };
        let (r, log) =
            crate::logging::capture(|| Sync::new(&self.cfg, &self.lib, &opts, source).run(&opts));
        r.unwrap();
        log
    }

    fn reload(&self, it: &Item) -> Item {
        self.lib.get_item(it.id.unwrap()).unwrap().unwrap()
    }

    fn album_of(&self, it: &Item) -> Album {
        self.lib
            .get_album(self.reload(it).int("album_id"))
            .unwrap()
            .unwrap()
    }
}

fn track(id: &str, title: &str) -> TrackInfo {
    TrackInfo {
        track_id: id.into(),
        title: title.into(),
        ..Default::default()
    }
}

/// beets' patched lookups: "new album" with one track, and a singleton.
fn fake() -> Fake {
    Fake {
        album: Some(AlbumInfo {
            album_id: "album id".into(),
            album: "new album".into(),
            tracks: vec![track("track id", "new title")],
            ..Default::default()
        }),
        track: Some(track("singleton id", "new title")),
        ..Default::default()
    }
}

#[test]
fn update_library() {
    let env = Env::new();
    let album_item = env.add_album(&[
        ("album", "old album"),
        ("mb_albumid", "album id"),
        ("mb_trackid", "track id"),
    ]);
    let singleton = env.add(&[("title", "old title"), ("mb_trackid", "singleton id")]);
    let source = fake();
    env.run(&source, CmdOpts::default());

    assert_eq!(env.reload(&singleton).str("title"), "new title");
    let a = env.reload(&album_item);
    assert_eq!(a.str("title"), "new title");
    assert_eq!(a.str("mb_trackid"), "track id");
    assert_eq!(env.album_of(&album_item).str("album"), "new album");
    assert_eq!(
        *source.asked.borrow(),
        ["track singleton id", "album album id"]
    );
}

#[test]
fn custom_format() {
    let mut env = Env::new();
    for (album, id) in [("no id", ""), ("invalid id", "a1b2c3d4")] {
        env.add_album(&[
            ("artist", "albumartist"),
            ("album", album),
            ("mb_albumid", id),
        ]);
    }
    for (title, id) in [("no id", ""), ("invalid id", "a1b2c3d4")] {
        env.add(&[("artist", "artist"), ("title", title), ("mb_trackid", id)]);
    }
    // `-f`: both formats (as `command` sets them).
    let f = "'%if{$album,$album,$title}'";
    env.cfg.set("format_item", Y::String(f.into()));
    env.cfg.set("format_album", Y::String(f.into()));
    let log = env.run(&Fake::default(), CmdOpts::default());
    assert!(
        log.contains(&"mbsync: Skipping album with no mb_albumid: 'no id'".to_string()),
        "{log:?}"
    );
    assert!(
        log.contains(&"mbsync: Skipping singleton with no mb_trackid: 'no id'".to_string()),
        "{log:?}"
    );
    assert!(
        log.contains(&"mbsync: Release ID a1b2c3d4 not found for album 'invalid id'".to_string()),
        "{log:?}"
    );
    assert!(
        log.contains(
            &"mbsync: Recording ID not found: a1b2c3d4 for track 'invalid id'".to_string()
        ),
        "{log:?}"
    );
}

#[test]
fn update_library_from_scratch_set() {
    let mut env = Env::new();
    env.cfg.set("import.from_scratch", Y::Bool(true));
    let album_item = env.add_album(&[
        ("album", "old album"),
        ("mb_albumid", "album id"),
        ("mb_trackid", "track id"),
        ("lyrics", "Hello"),
    ]);
    let singleton = env.add(&[
        ("title", "old title"),
        ("mb_trackid", "singleton id"),
        ("lyrics", "Hello"),
    ]);
    env.run(&fake(), CmdOpts::default());
    assert_eq!(env.reload(&singleton).str("lyrics"), "Hello");
    assert_eq!(env.reload(&album_item).str("lyrics"), "Hello");
}

// ------------------------------------------------------------ beyond beets'

#[test]
fn pretend_changes_nothing() {
    let env = Env::new();
    let album_item = env.add_album(&[
        ("album", "old album"),
        ("mb_albumid", "album id"),
        ("mb_trackid", "track id"),
    ]);
    let singleton = env.add(&[("title", "old title"), ("mb_trackid", "singleton id")]);
    env.run(
        &fake(),
        CmdOpts {
            pretend: true,
            ..Default::default()
        },
    );
    assert_eq!(env.reload(&singleton).str("title"), "old title");
    assert_eq!(env.reload(&album_item).str("title"), "");
    assert_eq!(env.album_of(&album_item).str("album"), "old album");
}

#[test]
fn only_musicbrainz_data_answers() {
    let mut env = Env::new();
    env.add(&[
        ("title", "t"),
        ("mb_trackid", "singleton id"),
        ("data_source", "Discogs"),
    ]);
    env.add_album(&[("mb_albumid", "album id"), ("album", "a")]);
    env.cfg
        .set("plugins", serde_yaml::from_str("[mbsync]").unwrap());
    let source = fake();
    let log = env.run(&source, CmdOpts::default());
    assert!(source.asked.borrow().is_empty());
    assert_eq!(
        log,
        [
            "mbsync: Recording ID not found: singleton id for track  -  - t",
            "mbsync: Release ID album id not found for album  - a",
        ]
    );
}

#[test]
fn album_takes_its_fields_from_a_changed_item_and_passes_them_on() {
    let env = Env::new();
    let mut items: Vec<Item> = ["t1", "t2"]
        .iter()
        .enumerate()
        .map(|(n, id)| {
            env.item(&[
                ("album", "old"),
                ("mb_albumid", "album id"),
                ("mb_trackid", id),
                ("track", &(n + 1).to_string()),
            ])
        })
        .collect();
    env.lib.add_album(&mut items).unwrap();
    // Only the first track is on the release.
    let source = Fake {
        album: Some(AlbumInfo {
            album_id: "album id".into(),
            album: "new".into(),
            tracks: vec![track("t1", "One")],
            ..Default::default()
        }),
        ..Default::default()
    };
    env.run(&source, CmdOpts::default());
    let [a, b] = [env.reload(&items[0]), env.reload(&items[1])];
    assert_eq!((a.str("title"), a.str("album")), ("One", "new"));
    // The unmatched item gets the album's new fields through the album.
    assert_eq!((b.str("title"), b.str("album")), ("", "new"));
    assert_eq!(env.album_of(&items[0]).str("album"), "new");
}

#[test]
fn items_map_by_release_track_then_recording() {
    let env = Env::new();
    let t = |rec: &str, rt: &str, disc: usize, idx: usize| TrackInfo {
        track_id: rec.into(),
        release_track_id: rt.into(),
        medium: disc,
        medium_index: idx,
        ..Default::default()
    };
    let info = AlbumInfo {
        tracks: vec![
            t("r1", "rt1", 1, 1),
            t("r2", "rt2", 1, 2),
            t("r2", "rt3", 2, 1),
        ],
        ..Default::default()
    };
    let items = [
        env.item(&[("mb_releasetrackid", "rt3"), ("mb_trackid", "r1")]),
        env.item(&[("mb_trackid", "r1")]),
        // r2 twice: by disc and track number.
        env.item(&[("mb_trackid", "r2"), ("disc", "1"), ("track", "2")]),
        env.item(&[("mb_trackid", "r2"), ("disc", "3"), ("track", "9")]),
        env.item(&[("mb_releasetrackid", "nope"), ("mb_trackid", "zzz")]),
    ];
    assert_eq!(map_items(&items, &info), [(0, 2), (1, 0), (2, 1)]);
}
