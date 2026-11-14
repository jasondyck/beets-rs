//! beets' `test/plugins/test_listenbrainz.py`, against fake servers.

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use serde_json::{Value as J, json};
use std::io::{Cursor, Write};

fn cfg() -> Config {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[listenbrainz]").unwrap());
    cfg.set("listenbrainz.token", "test_token".into());
    cfg.set("listenbrainz.username", "test_user".into());
    cfg
}

fn plugin(root: &str) -> ListenBrainzPlugin {
    let mut p = ListenBrainzPlugin::new(&cfg()).unwrap();
    p.root = root.to_string();
    p
}

fn mb_cfg(server: &str) -> Config {
    let mut cfg = cfg();
    cfg.set(
        "musicbrainz.host",
        server.trim_start_matches("http://").to_string().into(),
    );
    cfg.set("musicbrainz.https", serde_yaml::Value::Bool(false));
    cfg.set(
        "musicbrainz.ratelimit",
        serde_yaml::Value::Number(1000.into()),
    );
    cfg.set(
        "musicbrainz.ratelimit_interval",
        serde_yaml::Value::Number(1.into()),
    );
    cfg
}

fn zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Cursor::new(vec![]);
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(content.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn write_zip(dir: &std::path::Path, name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, zip_bytes(files)).unwrap();
    path
}

fn listen(ts: i64, title: &str, release: &str) -> J {
    json!({
        "listened_at": ts,
        "track_metadata": {"track_name": title, "artist_name": "A", "release_name": release}
    })
}

#[test]
fn get_mb_recording_id() {
    for (results, want) in [
        (json!([{"id": "id1"}]), Some("id1".to_string())),
        (json!([]), None),
    ] {
        let server = serve(move |_| Reply::json(json!({"recordings": results.clone()})));
        let p = ListenBrainzPlugin::new(&mb_cfg(&server.base)).unwrap();
        let track = json!({"track_metadata": {"track_name": "S", "release_name": "A"}});
        assert_eq!(p.get_mb_recording_id(&track).unwrap(), want);
        let target = &server.requests()[0].target;
        assert!(target.starts_with("/ws/2/recording?"), "{target}");
        assert!(target.contains("query=s+release%3A%28a%29"), "{target}");
    }
}

#[test]
fn get_track_info() {
    let server = serve(|r| {
        assert!(r.target.contains("/ws/2/recording/id1"));
        assert!(r.target.contains("inc=releases"));
        Reply::json(json!({
            "title": "T",
            "artist-credit": [],
            "releases": [{"title": "Al", "date": "2023-01"}]
        }))
    });
    let p = ListenBrainzPlugin::new(&mb_cfg(&server.base)).unwrap();
    let info = p.get_track_info(&[json!({"identifier": "id1"})]).unwrap();
    assert_eq!(
        info,
        vec![json!({
            "identifier": "id1",
            "title": "T",
            "artist": J::Null,
            "album": "Al",
            "year": "2023",
        })]
    );
}

#[test]
fn aggregate_listens_counts_by_mbid() {
    let track = |mbid: &str, artist: &str, name: &str, album: &str| Track {
        mbid: Some(mbid.into()),
        artist: artist.into(),
        name: name.into(),
        album: Some(album.into()),
        playcount: 1,
    };
    let tracks = vec![
        track("m1", "A", "S", "Al"),
        track("m1", "A", "S", "Al"),
        track("m1", "A", "S", "Al"),
        track("m2", "B", "T", "Bl"),
    ];
    let out = ListenBrainzPlugin::aggregate_listens(&tracks);
    let by_mbid: std::collections::BTreeMap<String, i64> = out
        .iter()
        .map(|t| (t.mbid.clone().unwrap(), t.playcount))
        .collect();
    assert_eq!(by_mbid["m1"], 3);
    assert_eq!(by_mbid["m2"], 1);
    assert_eq!(out.len(), 2);
    // The first entry's data is kept.
    assert_eq!(out[0].artist, "A");
}

#[test]
fn aggregate_falls_back_to_artist_title_album() {
    let track = |mbid: Option<&str>| Track {
        mbid: mbid.map(String::from),
        artist: "A".into(),
        name: "S".into(),
        album: Some("Al".into()),
        playcount: 1,
    };
    let out = ListenBrainzPlugin::aggregate_listens(&[track(None), track(Some("")), track(None)]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].playcount, 3);
}

#[test]
fn get_tracks_from_listens() {
    let p = plugin("http://127.0.0.1:9/1/");
    // No release name: skipped.
    let listens = vec![json!({
        "listened_at": 1,
        "track_metadata": {"track_name": "No album"}
    })];
    assert!(p.get_tracks_from_listens(&listens).is_empty());
    // `additional_info.recording_mbid` wins over the mapping.
    let listens = vec![json!({
        "listened_at": 1,
        "track_metadata": {
            "track_name": " Song ",
            "artist_name": " Artist ",
            "release_name": " Album ",
            "additional_info": {"recording_mbid": "rec-mbid-123"},
            "mbid_mapping": {"recording_mbid": "rec-mbid-456"}
        }
    })];
    let tracks = p.get_tracks_from_listens(&listens);
    assert_eq!(tracks[0].mbid.as_deref(), Some("rec-mbid-123"));
    assert_eq!(tracks[0].name, "Song");
    assert_eq!(tracks[0].artist, "Artist");
    assert_eq!(tracks[0].album.as_deref(), Some("Album"));
    assert_eq!(tracks[0].playcount, 1);
    // The mapping is the fallback.
    let listens = vec![json!({
        "listened_at": 1,
        "track_metadata": {
            "track_name": "Song", "artist_name": "Artist", "release_name": "Album",
            "mbid_mapping": {"recording_mbid": "rec-mbid-123"}
        }
    })];
    assert_eq!(
        p.get_tracks_from_listens(&listens)[0].mbid.as_deref(),
        Some("rec-mbid-123")
    );
    // A null mapping is fine.
    let listens = vec![json!({
        "listened_at": 1,
        "track_metadata": {
            "track_name": "Song", "artist_name": "Artist", "release_name": "Album",
            "mbid_mapping": null
        }
    })];
    assert_eq!(p.get_tracks_from_listens(&listens)[0].mbid, None);
}

#[test]
fn get_listens_paginates() {
    let server = serve(|r| {
        if r.target.contains("max_ts=") {
            Reply::json(json!({"payload": {"listens": [
                listen(50, "T5", "R"), listen(49, "T6", "R"), listen(48, "T7", "R")
            ]}}))
        } else {
            Reply::json(json!({"payload": {"listens": [
                listen(100, "T0", "R"), listen(99, "T1", "R"), listen(98, "T2", "R"),
                listen(97, "T3", "R"), listen(96, "T4", "R")
            ]}}))
        }
    });
    let p = plugin(&format!("{}/1/", server.base));
    let listens = p.get_listens(None, None, Some(5), None).unwrap().unwrap();
    assert_eq!(listens.len(), 8);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].target.contains("count=5"));
    assert!(!requests[0].target.contains("max_ts="));
    assert!(
        requests[1].target.contains("max_ts=95"),
        "{}",
        requests[1].target
    );
    assert_eq!(
        requests[0].header("Authorization").unwrap(),
        "Token test_token"
    );
}

#[test]
fn get_listens_stops_on_empty_page() {
    let server = serve(|_| Reply::json(json!({"payload": {"listens": []}})));
    let p = plugin(&format!("{}/1/", server.base));
    assert_eq!(p.get_listens(None, None, None, None).unwrap(), Some(vec![]));
}

#[test]
fn get_listens_returns_none_on_api_error() {
    let server = serve(|_| Reply::status(404));
    let p = plugin(&format!("{}/1/", server.base));
    assert_eq!(p.get_listens(None, None, None, None).unwrap(), None);
}

#[test]
fn get_listens_rejects_both_min_and_max_ts() {
    let p = plugin("http://127.0.0.1:9/1/");
    let err = p.get_listens(Some(1), Some(2), None, None).unwrap_err();
    assert!(err.to_string().contains("mutually exclusive"));
}

#[test]
fn get_listens_clamps_count_to_1000() {
    let server = serve(|_| Reply::json(json!({"payload": {"listens": []}})));
    let p = plugin(&format!("{}/1/", server.base));
    p.get_listens(None, None, Some(5000), None).unwrap();
    assert!(server.requests()[0].target.contains("count=1000"));
}

#[test]
fn get_listens_respects_max_total() {
    let server = serve(|r| {
        let count: usize = r
            .target
            .split("count=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .and_then(|s| s.parse().ok())
            .unwrap();
        let listens: Vec<J> = (0..count)
            .map(|i| listen(100 - i as i64, "T", "R"))
            .collect();
        Reply::json(json!({"payload": {"listens": listens}}))
    });
    let p = plugin(&format!("{}/1/", server.base));
    let listens = p.get_listens(None, None, None, Some(7)).unwrap().unwrap();
    assert_eq!(listens.len(), 7);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].target.contains("count=7"));
}

#[test]
fn import_file_returns_listens() {
    let d = tempfile::tempdir().unwrap();
    let listen = json!({"listened_at": 1778946700, "track_metadata": {"track_name": "S"}});
    let path = write_zip(
        d.path(),
        "export.zip",
        &[("listens/2026.jsonl", &format!("{listen}\n"))],
    );
    let p = plugin("http://127.0.0.1:9/1/");
    let result = p
        .import_listenbrainz_data_export(path.to_str().unwrap())
        .unwrap();
    assert_eq!(result, vec![listen]);
}

#[test]
fn import_file_ignores_non_listens_entries() {
    let d = tempfile::tempdir().unwrap();
    let listen = json!({"listened_at": 1778946700, "track_metadata": {}});
    let path = write_zip(
        d.path(),
        "export.zip",
        &[
            ("listens/2026.jsonl", &format!("{listen}\n")),
            ("other/data.jsonl", "{\"should\": \"be ignored\"}\n"),
            ("listens/readme.txt", "not a jsonl file\n"),
        ],
    );
    let p = plugin("http://127.0.0.1:9/1/");
    assert_eq!(
        p.import_listenbrainz_data_export(path.to_str().unwrap())
            .unwrap(),
        vec![listen]
    );
}

#[test]
fn import_file_skips_empty_lines() {
    let d = tempfile::tempdir().unwrap();
    let listen = json!({"listened_at": 1778946700, "track_metadata": {}});
    let content = format!("\n{listen}\n\n");
    let path = write_zip(d.path(), "export.zip", &[("listens/2026.jsonl", &content)]);
    let p = plugin("http://127.0.0.1:9/1/");
    assert_eq!(
        p.import_listenbrainz_data_export(path.to_str().unwrap())
            .unwrap(),
        vec![listen]
    );
}

#[test]
fn import_file_skips_invalid_json() {
    let d = tempfile::tempdir().unwrap();
    let listen = json!({"listened_at": 1778946700, "track_metadata": {}});
    let content = format!("not valid json\n{listen}\n");
    let path = write_zip(d.path(), "export.zip", &[("listens/2026.jsonl", &content)]);
    let p = plugin("http://127.0.0.1:9/1/");
    assert_eq!(
        p.import_listenbrainz_data_export(path.to_str().unwrap())
            .unwrap(),
        vec![listen]
    );
}

#[test]
fn import_file_raises_on_missing_file() {
    let p = plugin("http://127.0.0.1:9/1/");
    let err = p
        .import_listenbrainz_data_export("/nonexistent/export.zip")
        .unwrap_err();
    assert!(err.to_string().contains("unreadable export file"), "{err}");
}

#[test]
fn a_missing_setting_fails_the_load() {
    // Without the section at all: `config["listenbrainz"]["token"].get()`
    // raises in beets, and the plugin fails to load.
    let cfg = Config::defaults();
    assert!(ListenBrainzPlugin::new(&cfg).is_err());
}

// ------------------------------------------------- playlist helpers (5.25 follow-up)

/// The reference's `get_listenbrainz_playlists` output for this response
/// (run against beets 2.14.1; see the tests' expectations).
fn createdfor() -> J {
    json!({"playlists": [
        {"playlist": {"creator": "listenbrainz",
                      "title": "Weekly Exploration - week of 2024-03-08",
                      "identifier": "https://listenbrainz.org/playlist/expl-3"}},
        {"playlist": {"creator": "listenbrainz",
                      "title": "Weekly Jams - week of 2024-03-01",
                      "identifier": "https://listenbrainz.org/playlist/jams-2"}},
        {"playlist": {"creator": "listenbrainz",
                      "title": "Weekly Jams - week of 2024-02-23",
                      "identifier": "https://listenbrainz.org/playlist/jams-1"}},
        {"playlist": {"creator": "listenbrainz",
                      "title": "Weekly Jams - week of 2024-03-08",
                      "identifier": "https://listenbrainz.org/playlist/jams-3"}},
        {"playlist": {"creator": "someone",
                      "title": "Not ListenBrainz - week of 2024-03-08",
                      "identifier": "https://listenbrainz.org/playlist/x"}},
        {"playlist": {"creator": "listenbrainz",
                      "title": "Weekly Jams - no date here",
                      "identifier": "https://listenbrainz.org/playlist/y"}},
    ]})
}

#[test]
fn get_playlists_createdfor() {
    let server = serve(|_| Reply::json(json!({"playlists": []})));
    let p = plugin(&server.base);
    assert_eq!(
        p.get_playlists_createdfor("alice"),
        Some(json!({"playlists": []}))
    );
    let seen = &server.requests()[0];
    assert_eq!(seen.target, "/user/alice/playlists/createdfor");
    assert_eq!(seen.header("Authorization"), Some("Token test_token"));
    // Any request error is None (and the plugin logs it at debug).
    let server = serve(|_| Reply::status(500));
    let p = plugin(&server.base);
    assert_eq!(p.get_playlists_createdfor("alice"), None);
}

#[test]
fn get_listenbrainz_playlists() {
    let server = serve(|_| Reply::json(createdfor()));
    let p = plugin(&server.base);
    assert_eq!(
        p.get_listenbrainz_playlists().unwrap(),
        vec![
            json!({"type": "Exploration", "date": "2024-03-08", "identifier": "expl-3"}),
            json!({"type": "Jams", "date": "2024-03-08", "identifier": "jams-3"}),
            json!({"type": "Jams", "date": "2024-03-01", "identifier": "jams-2"}),
            json!({"type": "Jams", "date": "2024-02-23", "identifier": "jams-1"}),
        ]
    );
    // No response, an empty one, or one without playlists: nothing.
    let server = serve(|_| Reply::status(404));
    assert!(
        plugin(&server.base)
            .get_listenbrainz_playlists()
            .unwrap()
            .is_empty()
    );
    for empty in [json!({}), json!([]), J::Null] {
        let server = serve(move |_| Reply::json(empty.clone()));
        assert!(
            plugin(&server.base)
                .get_listenbrainz_playlists()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn get_listenbrainz_playlists_errors_on_malformed_entries() {
    // beets raises on these (ValueError/AttributeError); the port returns
    // an error.
    for bad in [
        json!({"playlists": [{"playlist": {"creator": "listenbrainz",
            "title": "Weekly Jams - week of not-a-date",
            "identifier": "https://listenbrainz.org/playlist/x"}}]}),
        json!({"playlists": [{"playlist": {"creator": "listenbrainz",
            "title": "Weekly Jams - week of 2024-03-01"}}]}),
        json!({"playlists": [{"title": "no playlist object"}]}),
        json!({"playlists": J::Null}),
    ] {
        let shown = bad.clone();
        let server = serve(move |_| Reply::json(bad.clone()));
        assert!(
            plugin(&server.base).get_listenbrainz_playlists().is_err(),
            "{shown}"
        );
    }
}

#[test]
fn get_playlist() {
    let server = serve(|r| {
        assert_eq!(r.target, "/playlist/expl-3");
        Reply::json(json!({"playlist": {"track": []}}))
    });
    let p = plugin(&server.base);
    assert_eq!(
        p.get_playlist("expl-3"),
        Some(json!({"playlist": {"track": []}}))
    );
    let server = serve(|_| Reply::status(404));
    assert_eq!(plugin(&server.base).get_playlist("missing"), None);
}

/// A playlist with a string identifier, a list identifier and a track with
/// no creator.
fn playlist() -> J {
    json!({"playlist": {"track": [
        {"identifier": "https://musicbrainz.org/recording/rec1",
         "creator": "C1", "title": "T1"},
        {"identifier": ["https://musicbrainz.org/recording/rec2",
                        "https://other/ignored"],
         "title": "T2"},
    ]}})
}

/// A fake MusicBrainz lookup by recording ID.
fn mb_recordings() -> crate::plugins::requests::testserver::Server {
    serve(|r| {
        if r.target.contains("/ws/2/recording/rec1") {
            Reply::json(json!({
                "title": "Rec One",
                "artist-credit": [{"artist": {"name": "Artist One"}}],
                "releases": [{"title": "Album One", "date": "2020-05-01"}],
            }))
        } else {
            assert!(r.target.contains("/ws/2/recording/rec2"), "{}", r.target);
            Reply::json(json!({"title": "Rec Two", "artist-credit": [], "releases": []}))
        }
    })
}

fn plugin_with_mb(mb: &str, lb: &str) -> ListenBrainzPlugin {
    let mut p = ListenBrainzPlugin::new(&mb_cfg(mb)).unwrap();
    p.root = lb.to_string();
    p
}

#[test]
fn get_tracks_from_playlist() {
    let mb = mb_recordings();
    let lb = serve(|r| {
        assert_eq!(r.target, "/playlist/pl-1");
        Reply::json(playlist())
    });
    let p = plugin_with_mb(&mb.base, &lb.base);
    assert_eq!(p.get_playlist("pl-1"), Some(playlist()));
    let tracks = p.get_tracks_from_playlist(&playlist()).unwrap();
    // The reference's output for the same data, in this order.
    assert_eq!(
        tracks,
        vec![
            json!({"identifier": "rec1", "title": "Rec One",
                   "artist": "Artist One", "album": "Album One", "year": "2020"}),
            json!({"identifier": "rec2", "title": "Rec Two",
                   "artist": J::Null, "album": J::Null, "year": J::Null}),
        ]
    );
}

#[test]
fn get_tracks_from_playlist_errors_on_missing_data() {
    let mb = mb_recordings();
    let p = plugin_with_mb(&mb.base, "http://127.0.0.1:1");
    for bad in [
        json!({"playlist": {}}),
        json!({}),
        json!({"playlist": {"track": [{"title": "no identifier"}]}}),
        json!({"playlist": {"track": [{"identifier": [], "title": "empty list"}]}}),
    ] {
        assert!(p.get_tracks_from_playlist(&bad).is_err(), "{bad}");
    }
}

/// LB responses for the weekly helpers: the playlist list, then two Jams
/// and one Exploration playlist.
fn weekly_lb() -> crate::plugins::requests::testserver::Server {
    serve(|r| match r.target.split('?').next().unwrap() {
        "/user/test_user/playlists/createdfor" => Reply::json(createdfor()),
        "/playlist/jams-3" => Reply::json(json!({"playlist": {"track": [
            {"identifier": "https://musicbrainz.org/recording/rec2"},
        ]}})),
        "/playlist/jams-2" => Reply::json(json!({"playlist": {"track": [
            {"identifier": "https://musicbrainz.org/recording/rec1"},
        ]}})),
        "/playlist/expl-3" => Reply::json(playlist()),
        _ => Reply::status(404),
    })
}

#[test]
fn get_weekly_playlist() {
    let mb = mb_recordings();
    let lb = weekly_lb();
    let p = plugin_with_mb(&mb.base, &lb.base);
    // The newest Jams; the second newest; an Exploration.
    assert_eq!(
        p.get_weekly_playlist("Jams", true).unwrap(),
        vec![json!({"identifier": "rec2", "title": "Rec Two",
                    "artist": J::Null, "album": J::Null, "year": J::Null})]
    );
    assert_eq!(
        p.get_weekly_playlist("Jams", false).unwrap(),
        vec![json!({"identifier": "rec1", "title": "Rec One",
                    "artist": "Artist One", "album": "Album One", "year": "2020"})]
    );
    assert_eq!(p.get_weekly_playlist("Exploration", true).unwrap().len(), 2);
    // No playlist of that type: beets' IndexError.
    assert!(p.get_weekly_playlist("Nope", true).is_err());
    // A selected playlist that can't be fetched: no tracks (as beets).
    let lb = serve(|r| match r.target.split('?').next().unwrap() {
        "/user/test_user/playlists/createdfor" => Reply::json(createdfor()),
        _ => Reply::status(404),
    });
    let p = plugin_with_mb(&mb.base, &lb.base);
    assert!(p.get_weekly_playlist("Jams", true).unwrap().is_empty());
    // Asking for the second newest when there is none: beets' IndexError.
    let lb = serve(|r| match r.target.split('?').next().unwrap() {
        "/user/test_user/playlists/createdfor" => Reply::json(json!({"playlists": [
            {"playlist": {"creator": "listenbrainz",
                          "title": "Weekly Jams - week of 2024-03-08",
                          "identifier": "https://listenbrainz.org/playlist/jams-3"}},
        ]})),
        "/playlist/jams-3" => Reply::json(json!({"playlist": {"track": [
            {"identifier": "https://musicbrainz.org/recording/rec2"},
        ]}})),
        _ => Reply::status(404),
    });
    let p = plugin_with_mb(&mb.base, &lb.base);
    assert!(p.get_weekly_playlist("Jams", false).is_err());
    assert!(p.get_weekly_playlist("Jams", true).is_ok());
}
