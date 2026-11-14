//! `test/plugins/test_bpsync.py` doesn't exist upstream (the plugin is
//! deprecated), so these port its behaviour from `beetsplug/bpsync.py`:
//! the skip messages and the re-fetch of Beatport singletons and albums
//! through the `beatport` source, against its fake API on 127.0.0.1 (the
//! client built with [`BeatportClient::with_api_base`], as beets' own tests
//! do for the beatport plugin).

use super::*;
use crate::model::Album;
use crate::plugins::beatport::{BeatportClient, Conf, Urls};
use crate::plugins::requests::testserver::{Reply, Seen, serve};
use serde_json::{Value as J, json};
use std::path::PathBuf;
use tempfile::TempDir;

/// The release and its tracks (the recorded API responses of beets'
/// `test/plugins/test_beatport.py`), served by the fake API.
fn release_response() -> J {
    json!({
        "id": 1742984,
        "type": "release",
        "name": "Charade",
        "slug": "charade",
        "releaseDate": "2016-04-11",
        "catalogNumber": "GR089",
        "label": {"id": 24539, "name": "Gravitas Recordings", "type": "label"},
        "artists": [{"id": 326158, "name": "Supersillyus", "type": "artist"}],
        "genres": [{"id": 9, "name": "Breaks", "slug": "breaks", "type": "genre"}]
    })
}

fn track_response(id: i64, number: i64, name: &str, bpm: i64, key: &str) -> J {
    json!({
        "id": id,
        "type": "track",
        "name": name,
        "trackNumber": number,
        "mixName": "Original Mix",
        "title": format!("{name} (Original Mix)"),
        "slug": name.to_lowercase().replace(' ', "-"),
        "releaseDate": "2016-04-11",
        "lengthMs": 425421,
        "bpm": bpm,
        "key": {"shortName": key},
        "artists": [{"id": 326158, "name": "Supersillyus", "type": "artist"}],
        "genres": [{"id": 9, "name": "Breaks", "type": "genre"}],
        "subGenres": [{"id": 209, "name": "Glitch Hop", "type": "subgenre"}]
    })
}

fn tracks_response() -> Vec<J> {
    vec![
        track_response(7817567, 1, "Mirage a Trois", 90, "Gmin"),
        track_response(7817568, 2, "Aeon Bahamut", 100, "Gmaj"),
    ]
}

/// The fake Beatport API; returns the server and a `Beatport` client for it.
fn fake_api() -> (crate::plugins::requests::testserver::Server, Beatport) {
    let release = release_response();
    let tracks = tracks_response();
    let server = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        let query = seen.target.split_once('?').map_or("", |(_, q)| q);
        match path {
            "/catalog/3/releases" => {
                let id = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("id="))
                    .unwrap_or_default();
                let found: Vec<J> = [release.clone()]
                    .into_iter()
                    .filter(|r| r["id"].as_i64() == id.parse::<i64>().ok())
                    .collect();
                Reply::json(json!({"results": found}))
            }
            "/catalog/3/tracks" if query.contains("releaseId=") => {
                Reply::json(json!({"results": tracks.clone()}))
            }
            "/catalog/3/tracks" => {
                let id = query
                    .split('&')
                    .find_map(|p| p.strip_prefix("id="))
                    .unwrap_or_default();
                let found: Vec<J> = tracks
                    .iter()
                    .filter(|t| t["id"].as_i64() == id.parse::<i64>().ok())
                    .cloned()
                    .collect();
                Reply::json(json!({"results": found}))
            }
            _ => Reply::status(404),
        }
    });
    let mut beatport = Beatport::with(Conf {
        urls: Urls {
            api_base: server.base.clone(),
        },
        apikey: "key".into(),
        apisecret: "secret".into(),
        tokenfile: PathBuf::from("/nonexistent"),
    });
    beatport.client = Some(
        BeatportClient::with_api_base("key", "secret", &server.base)
            .with_token("token", "tokensecret"),
    );
    (server, beatport)
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
        std::fs::create_dir_all(&music).unwrap();
        let mut cfg = Config::load_in(dir.path(), &[]).unwrap();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str("[bpsync]").unwrap());
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env {
            _dir: dir,
            cfg,
            lib,
        }
    }

    fn item(pairs: &[(&str, &str)]) -> Item {
        let mut it = Item::new_item();
        it.set_path(format!("/nonexistent/{}.mp3", pairs.len()));
        for (k, v) in pairs {
            it.set_parsed(k, v).unwrap();
        }
        it
    }

    fn add(&self, pairs: &[(&str, &str)]) -> Item {
        let mut it = Self::item(pairs);
        self.lib.add(&mut it).unwrap();
        it
    }

    fn add_album(&self, items: &[&[(&str, &str)]]) -> Vec<Item> {
        let mut items: Vec<Item> = items.iter().map(|p| Self::item(p)).collect();
        self.lib.add_album(&mut items).unwrap();
        items
    }

    /// `beet bpsync`'s work (not writing: the files don't exist), its log.
    fn run(&self, source: &dyn Source, opts: CmdOpts) -> Vec<String> {
        let opts = CmdOpts {
            nowrite: true,
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
        self.lib.get_album(self.reload(it).int("album_id")).unwrap().unwrap()
    }
}

fn beatport_track<'a>(pairs: &'a [(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut v = vec![("data_source", "Beatport")];
    v.extend_from_slice(pairs);
    v
}

#[test]
fn load_logs_the_deprecation() {
    let cfg = crate::config::Config::defaults();
    let (_r, lines) = crate::logging::capture(|| load(&cfg));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("The 'bpsync' plugin is deprecated")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("The 'beatport' plugin is deprecated")),
        "{lines:?}"
    );
}

#[test]
fn singletons_are_refetched_from_beatport() {
    let env = Env::new();
    let item = env.add(&beatport_track(&[
        ("artist", "Old Artist"),
        ("title", "Old Title"),
        ("mb_trackid", "7817567"),
    ]));
    let (_server, beatport) = fake_api();
    let source = BeatportSource::new(&env.cfg, &beatport);
    let log = env.run(&source, CmdOpts::default());
    let it = env.reload(&item);
    assert_eq!(it.str("title"), "Mirage a Trois");
    assert_eq!(it.str("artist"), "Supersillyus");
    assert_eq!(it.int("bpm"), 90);
    assert_eq!(it.str("initial_key"), "Gmin");
    assert_eq!(it.flex.get("data_source").map(String::as_str), Some("Beatport"));
    assert!(it.str("genres").contains("Glitch Hop"), "{:?}", it.str("genres"));
    // `show_model_changes` printed the changes; nothing was logged.
    assert!(log.is_empty(), "{log:?}");
}

#[test]
fn singletons_without_beatport_ids_are_skipped() {
    let env = Env::new();
    env.add(&[("artist", "A"), ("title", "No ID")]);
    env.add(&[("artist", "A"), ("title", "Invalid"), ("mb_trackid", "a1b2c3")]);
    env.add(&[
        ("artist", "A"),
        ("title", "Not Beatport"),
        ("mb_trackid", "7817567"),
        ("data_source", "MusicBrainz"),
    ]);
    let (server, beatport) = fake_api();
    let source = BeatportSource::new(&env.cfg, &beatport);
    let log = env.run(&source, CmdOpts::default());
    assert_eq!(
        log,
        [
            "bpsync: Skipping singleton with no mb_trackid: A -  - No ID",
            "bpsync: Skipping non-Beatport singleton: A -  - Invalid",
            "bpsync: Skipping non-Beatport singleton: A -  - Not Beatport",
        ]
    );
    // No lookup was made for any of them.
    assert!(server.requests().is_empty());
}

#[test]
fn albums_are_refetched_from_beatport() {
    let env = Env::new();
    let items = env.add_album(&[
        &beatport_track(&[
            ("album", "Old Album"),
            ("artist", "Old Artist"),
            ("title", "One"),
            ("track", "1"),
            ("mb_albumid", "1742984"),
            ("mb_trackid", "7817567"),
        ]),
        &beatport_track(&[
            ("album", "Old Album"),
            ("artist", "Old Artist"),
            ("title", "Two"),
            ("track", "2"),
            ("mb_albumid", "1742984"),
            ("mb_trackid", "7817568"),
        ]),
    ]);
    let (_server, beatport) = fake_api();
    let source = BeatportSource::new(&env.cfg, &beatport);
    let log = env.run(&source, CmdOpts::default());
    let [a, b] = [env.reload(&items[0]), env.reload(&items[1])];
    assert_eq!((a.str("title"), a.int("track")), ("Mirage a Trois", 1));
    assert_eq!((b.str("title"), b.int("track")), ("Aeon Bahamut", 2));
    assert_eq!(a.str("album"), "Charade");
    assert_eq!(a.str("catalognum"), "GR089");
    let album = env.album_of(&items[0]);
    assert_eq!(album.str("album"), "Charade");
    assert_eq!(album.str("albumartist"), "Supersillyus");
    assert!(
        log.iter().any(|l| l == "bpsync: applying changes to  - Old Album"),
        "{log:?}"
    );
}

#[test]
fn albums_that_are_not_beatport_releases_are_skipped() {
    let env = Env::new();
    // No mb_albumid.
    env.add_album(&[&[("album", "No ID"), ("artist", "A")]]);
    // A MusicBrainz id (not numeric).
    env.add_album(&[&[
        ("album", "MB release"),
        ("artist", "A"),
        ("mb_albumid", "33333333-3333-3333-3333-333333333333"),
    ]]);
    // A numeric id but a non-Beatport album and items.
    env.add_album(&[&[
        ("album", "Not Beatport"),
        ("artist", "A"),
        ("mb_albumid", "1742984"),
        ("mb_trackid", "7817567"),
    ]]);
    // Beatport album, but an item without a Beatport track id.
    env.add_album(&[
        &beatport_track(&[
            ("album", "Mixed"),
            ("artist", "A"),
            ("mb_albumid", "1742984"),
            ("mb_trackid", "7817567"),
            ("track", "1"),
        ]),
        &beatport_track(&[
            ("album", "Mixed"),
            ("artist", "A"),
            ("mb_albumid", "1742984"),
            ("mb_trackid", "nope"),
            ("track", "2"),
        ]),
    ]);
    let (server, beatport) = fake_api();
    let source = BeatportSource::new(&env.cfg, &beatport);
    let log = env.run(&source, CmdOpts::default());
    assert_eq!(
        log,
        [
            "bpsync: Skipping album with invalid Beatport ID:  - MB release",
            "bpsync: Skipping non-Beatport release:  - Mixed",
            "bpsync: Skipping album with no mb_albumid:  - No ID",
            "bpsync: Skipping non-Beatport release:  - Not Beatport",
        ]
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_release_not_found_is_logged() {
    let env = Env::new();
    // A numeric id no release: the fake API returns no results.
    env.add_album(&[&beatport_track(&[
        ("album", "Gone"),
        ("artist", "A"),
        ("mb_albumid", "1"),
        ("mb_trackid", "7817567"),
    ])]);
    let (_server, beatport) = fake_api();
    let source = BeatportSource::new(&env.cfg, &beatport);
    let log = env.run(&source, CmdOpts::default());
    assert_eq!(log, ["bpsync: Release ID 1 not found for album  - Gone"]);
}

#[test]
fn changed_items_are_moved_in_the_library() {
    let env = Env::new();
    let music = env.cfg.str("directory");
    let music_dir = std::path::Path::new(&music);
    std::fs::create_dir_all(&music).unwrap();
    let old = format!("{music}/old.mp3");
    std::fs::write(&old, b"").unwrap();
    let mut it = Env::item(&beatport_track(&[
        ("artist", "Supersillyus"),
        ("title", "Old Title"),
        ("mb_trackid", "7817567"),
    ]));
    it.set_path(&old);
    env.lib.add(&mut it).unwrap();
    let mut cfg = env.cfg.clone();
    cfg.set("paths.default", Y::String("$artist/$title".into()));
    let (_server, beatport) = fake_api();
    let opts = CmdOpts {
        nowrite: true,
        mv: Some(true),
        ..Default::default()
    };
    let source = BeatportSource::new(&cfg, &beatport);
    crate::logging::capture(|| Sync::new(&cfg, &env.lib, &opts, &source).run(&opts))
        .0
        .unwrap();
    let new = env.reload(&it).path().display().to_string();
    assert_eq!(
        new,
        music_dir
            .join("Non-Album/Supersillyus/Mirage a Trois.mp3")
            .display()
            .to_string()
    );
    assert!(std::path::Path::new(&new).exists(), "{new}");
    assert!(!std::path::Path::new(&old).exists());
}

#[test]
fn pretend_shows_but_changes_nothing() {
    let env = Env::new();
    let item = env.add(&beatport_track(&[
        ("artist", "Old Artist"),
        ("title", "Old Title"),
        ("mb_trackid", "7817567"),
    ]));
    let (_server, beatport) = fake_api();
    let source = BeatportSource::new(&env.cfg, &beatport);
    env.run(
        &source,
        CmdOpts {
            pretend: true,
            ..Default::default()
        },
    );
    let it = env.reload(&item);
    assert_eq!((it.str("artist"), it.str("title")), ("Old Artist", "Old Title"));
}

/// `beet bpsync` as the command line runs it: the source is built from the
/// configuration (token file) and `BEETSRS_BEATPORT_API_URL`.
#[test]
fn command_builds_the_beatport_source_from_the_config() {
    use crate::plugins::requests::testserver::{Reply, serve};
    use serde_json::json;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    let release = release_response();
    let tracks = tracks_response();
    let server = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        match path {
            "/catalog/3/releases" => Reply::json(json!({"results": [release.clone()]})),
            "/catalog/3/tracks" => Reply::json(json!({"results": tracks.clone()})),
            _ => Reply::status(404),
        }
    });
    let env = Env::new();
    std::fs::write(
        env._dir.path().join("beatport_token.json"),
        r#"{"token": "t", "secret": "s"}"#,
    )
    .unwrap();
    assert_eq!(env.cfg.str("beatport.tokenfile"), "beatport_token.json");
    assert!(
        env.cfg
            .config_dir
            .join(env.cfg.str("beatport.tokenfile"))
            .exists(),
        "token file at {:?} (tokenfile={:?})",
        env.cfg.config_dir.join(env.cfg.str("beatport.tokenfile")),
        env.cfg.str("beatport.tokenfile")
    );
    let item = env.add(&beatport_track(&[
        ("album", "Old Album"),
        ("mb_albumid", "1742984"),
        ("mb_trackid", "7817567"),
        ("track", "1"),
    ]));
    // SAFETY: the lock keeps this the only bpsync test building a source
    // from a config, and no other test reads this variable.
    unsafe { std::env::set_var("BEETSRS_BEATPORT_API_URL", &server.base) };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        command(
            &env.cfg,
            &env.lib,
            &CmdOpts {
                nowrite: true,
                ..Default::default()
            },
        )
        .unwrap();
    }));
    // SAFETY: still under the lock, as the set above.
    unsafe { std::env::remove_var("BEETSRS_BEATPORT_API_URL") };
    result.unwrap();
    let it = env.reload(&item);
    // A singleton's `TrackMatch` has the track's fields, not the release's.
    assert_eq!(
        (it.str("title"), it.str("album")),
        ("Mirage a Trois", "Old Album")
    );
}
