//! beets' test/plugins/test_missing.py, with a fake MusicBrainz on
//! 127.0.0.1 (a free port) where beets uses `requests_mock`.

use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use serde_json::json;
use serde_yaml::Value as Y;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new(plugins: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str(plugins).unwrap());
        cfg.set("musicbrainz.ratelimit", Y::from(1000));
        // `$missing` is there when the plugin is enabled (per thread).
        crate::formatted::configure(&cfg).unwrap();
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env {
            _dir: dir,
            cfg,
            lib,
        }
    }

    /// Point the MusicBrainz client at `server`.
    fn mb(&mut self, server: &Server) {
        let host = server.base.trim_start_matches("http://").to_string();
        self.cfg.set("musicbrainz.host", Y::String(host));
    }

    /// An album of items with these (field, value) pairs each.
    fn album(&self, items: &[&[(&str, &str)]]) -> Album {
        let mut its: Vec<Item> = items
            .iter()
            .enumerate()
            .map(|(n, pairs)| {
                let mut it = Item::new_item();
                it.set_path(format!("/nonexistent/{n}-{}.mp3", self.lib_len()));
                for (k, v) in *pairs {
                    it.set_parsed(k, v).unwrap();
                }
                it
            })
            .collect();
        self.lib.add_album(&mut its).unwrap()
    }

    fn lib_len(&self) -> usize {
        self.lib
            .items(&Query::parse(Kind::Item, &[] as &[String]).unwrap())
            .unwrap()
            .len()
    }

    fn run(&self, opts: CmdOpts) -> String {
        let mut out = vec![];
        run(&self.cfg, &self.lib, opts, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }
}

const ARTIST_MBID: &str = "6a7c3c3e-2f0b-4a8e-9d55-0c3a5f6b1e2d";

/// beets' album in these tests (`Album(...)` added to the library).
fn library_album(env: &Env, album: &str, artist: &str, rg: &str) {
    env.album(&[&[
        ("album", album),
        ("albumartist", artist),
        ("mb_albumartistid", ARTIST_MBID),
        ("mb_albumid", "album"),
        ("mb_releasegroupid", rg),
    ]]);
}

/// A fake MusicBrainz answering release-group browses with `groups`.
fn rg_server(groups: serde_json::Value) -> Server {
    serve(move |_: &Seen| Reply::json(json!({"release-groups": groups})))
}

fn albums_opts(extra: impl FnOnce(&mut CmdOpts)) -> CmdOpts {
    let mut o = CmdOpts {
        album: true,
        ..Default::default()
    };
    extra(&mut o);
    o
}

#[test]
fn missing_artist_albums() {
    for (rg, expected) in [
        (
            json!({"id": "other", "title": "Other Album"}),
            "Artist - Other Album\n",
        ),
        (json!({"id": "release_group_in_lib", "title": "Album"}), ""),
    ] {
        let mut env = Env::new("[missing]");
        library_album(&env, "Album", "Artist", "release_group_in_lib");
        let server = rg_server(json!([rg]));
        env.mb(&server);
        assert_eq!(env.run(albums_opts(|_| {})), expected);
        let seen = server.requests();
        assert_eq!(seen.len(), 1);
        assert!(
            seen[0].target.starts_with(&format!(
                "/ws/2/release-group?artist={ARTIST_MBID}&type=album&fmt=json"
            )),
            "{}",
            seen[0].target
        );
    }
}

#[test]
fn missing_albums_custom_format() {
    let mut env = Env::new("[missing]");
    library_album(&env, "Album", "Artist", "release_group_in_lib");
    let server = rg_server(json!([
        {"id": "other", "title": "Other Album", "primary-type": "Album"}
    ]));
    env.mb(&server);
    let out = env.run(albums_opts(|o| {
        o.format = Some("$mb_releasegroupid | $albumtype | $album".into());
    }));
    assert_eq!(out, "other | album | Other Album\n");
}

#[test]
fn release_types_filters_and_commas() {
    let mut env = Env::new("[missing]");
    library_album(&env, "album", "artist", "album_id");
    let server = rg_server(json!([
        {"id": "album2_id", "title": "title 2"},
        {"id": "compilation_id", "title": "compilation"}
    ]));
    env.mb(&server);
    let out = env.run(albums_opts(|o| {
        o.release_types = vec!["compilation".into()];
    }));
    assert!(out.contains("artist - compilation"), "{out}");
    let out = env.run(albums_opts(|o| {
        o.release_types = vec!["compilation,album".into(), "ep".into()];
    }));
    assert!(out.contains("artist - compilation"), "{out}");
    assert!(out.contains("artist - title 2"), "{out}");
    let seen = server.requests();
    assert!(
        seen[0].target.contains("&type=compilation&"),
        "{}",
        seen[0].target
    );
    assert!(
        seen[1].target.contains("&type=compilation%7Calbum%7Cep&"),
        "{}",
        seen[1].target
    );
}

#[test]
fn empty_release_types_config_sends_empty_type() {
    let mut env = Env::new("[missing]");
    library_album(&env, "album", "artist", "album_id");
    env.cfg
        .set("missing.release_types", serde_yaml::from_str("[]").unwrap());
    let server = rg_server(json!([]));
    env.mb(&server);
    assert_eq!(env.run(albums_opts(|_| {})), "");
    assert!(server.requests()[0].target.contains("&type=&"));
}

#[test]
fn missing_albums_total() {
    let mut env = Env::new("[missing]");
    library_album(&env, "album", "artist", "album_id");
    let server = rg_server(json!([
        {"id": "album_id", "title": "album"},
        {"id": "other_id", "title": "other"}
    ]));
    env.mb(&server);
    assert_eq!(env.run(albums_opts(|o| o.total = true)), "1\n");
}

#[test]
fn albums_without_artist_mbid_are_skipped() {
    let mut env = Env::new("[missing]");
    env.album(&[&[
        ("album", "a"),
        ("albumartist", "x"),
        ("mb_albumartistid", "nope"),
    ]]);
    let server = rg_server(json!([{"id": "z", "title": "z"}]));
    env.mb(&server);
    assert_eq!(env.run(albums_opts(|o| o.total = true)), "0\n");
    assert!(server.requests().is_empty());
}

/// test_missing_tracks: two of three tracks in the library.
fn tracks_env(plugins: &str) -> Env {
    let env = Env::new(plugins);
    let common = |id: &'static str| -> Vec<(&'static str, &'static str)> {
        let mut v = vec![
            ("album", "album"),
            ("mb_albumid", "11111111-1111-1111-1111-111111111111"),
            ("mb_releasegroupid", "album_id"),
            ("mb_albumartistid", ARTIST_MBID),
            ("albumartist", "artist"),
            ("tracktotal", "3"),
        ];
        if !id.is_empty() {
            v.push(("mb_trackid", id));
        }
        v
    };
    let a = common("track_1");
    let b = common("");
    env.album(&[&a, &b]);
    env
}

#[test]
fn missing_tracks_total_and_count() {
    let env = tracks_env("[missing]");
    let total = env.run(CmdOpts {
        total: true,
        ..Default::default()
    });
    assert_eq!(total, "1\n");
    let count = env.run(CmdOpts {
        count: true,
        ..Default::default()
    });
    assert!(count.contains("artist - album: 1"), "{count}");
}

fn fixture_server() -> Server {
    serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or_default();
        match path.strip_prefix("/ws/2/release/") {
            Some(id) => {
                let file = format!(
                    "{}/tests/fixtures/mb/release-{id}.json",
                    env!("CARGO_MANIFEST_DIR")
                );
                match std::fs::read_to_string(file) {
                    Ok(body) => Reply::json(serde_json::from_str(&body).unwrap()),
                    Err(_) => Reply::status(404),
                }
            }
            None => Reply::status(404),
        }
    })
}

#[test]
fn missing_tracks_listed_from_musicbrainz() {
    let mut env = tracks_env("[missing, musicbrainz]");
    let server = fixture_server();
    env.mb(&server);
    // The fixture's first recording is in the library, as `track_1` isn't.
    let mut items = env
        .lib
        .items(&Query::parse(Kind::Item, &["mb_trackid:track_1".to_string()]).unwrap())
        .unwrap();
    items[0].set_str("mb_trackid", "00000000-0000-0000-0001-000000000001");
    env.lib.store(&items[0]).unwrap();
    let out = env.run(CmdOpts {
        format: Some("$title|$track|$tracktotal|$disc|$disctotal|$length|$album_id".into()),
        ..Default::default()
    });
    assert_eq!(
        out,
        "Hidden Intro|01|05|01|02|0:30|1\n\
         Beta|03|05|01|02|3:00|1\n\
         Gamma|04|05|01|02|4:00|1\n\
         Interview|05|05|02|02|10:00|1\n"
    );
    // The default format.
    let out = env.run(CmdOpts::default());
    assert!(
        out.starts_with("Fixture Artist - Fixture Album - Hidden Intro\n"),
        "{out}"
    );
}

#[test]
fn missing_tracks_need_the_source_plugin() {
    // Without the musicbrainz plugin (or with another data source), no
    // lookup: nothing to list.
    let mut env = tracks_env("[missing]");
    let server = fixture_server();
    env.mb(&server);
    assert_eq!(env.run(CmdOpts::default()), "");
    let mut env = tracks_env("[missing, musicbrainz]");
    env.mb(&server);
    let mut album = env
        .lib
        .albums(&Query::parse(Kind::Album, &[] as &[String]).unwrap())
        .unwrap()
        .remove(0);
    album.flex.insert("data_source".into(), "Discogs".into());
    env.lib.store(&album).unwrap();
    assert_eq!(env.run(CmdOpts::default()), "");
    assert!(server.requests().is_empty());
}

#[test]
fn complete_albums_are_not_looked_up() {
    let mut env = Env::new("[missing, musicbrainz]");
    env.album(&[&[
        ("album", "a"),
        ("tracktotal", "1"),
        ("mb_albumid", "11111111-1111-1111-1111-111111111111"),
    ]]);
    let server = fixture_server();
    env.mb(&server);
    assert_eq!(env.run(CmdOpts::default()), "");
    assert!(server.requests().is_empty());
}

#[test]
fn missing_template_field() {
    let env = tracks_env("[missing]");
    let album = env
        .lib
        .albums(&Query::parse(Kind::Album, &[] as &[String]).unwrap())
        .unwrap()
        .remove(0);
    let t = Template::parse("$missing/$albumtotal");
    assert_eq!(render(&t, &album, Some(&env.lib)), "1/3");
    let item = env.lib.album_items(album.id.unwrap()).unwrap().remove(0);
    let t = Template::parse("$missing");
    assert_eq!(render(&t, &item, Some(&env.lib)), "1");
    let keys = Formatted::new(&album, Some(&env.lib)).all_keys();
    assert!(keys.iter().any(|k| k == "missing"));
    // Without the plugin, no such field.
    crate::formatted::configure(&Config::defaults()).unwrap();
    assert_eq!(render(&t, &item, Some(&env.lib)), "$missing");
}
