//! beets' `test/plugins/test_spotify.py`, with a fake Spotify API on
//! 127.0.0.1 (a free port) where beets uses `responses`.

use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use crate::model::{Item, Value};
use serde_json::json;

fn urls(server: &Server) -> Urls {
    Urls {
        oauth_token: format!("{}/api/token", server.base),
        open_track: "https://open.spotify.com/track/".into(),
        search: format!("{}/v1/search", server.base),
        album: format!("{}/v1/albums/", server.base),
        track: format!("{}/v1/tracks", server.base),
        audio_features: format!("{}/v1/audio-features", server.base),
    }
}

fn plugin(server: &Server) -> Spotify {
    let mut p = Spotify::with(Conf {
        urls: urls(server),
        mode: "list".into(),
        tiebreak: "popularity".into(),
        show_failures: false,
        region_filter: None,
        regex: vec![],
        client_id: "id".into(),
        client_secret: "secret".into(),
        tokenfile: PathBuf::from("/nonexistent/spotify_token.json"),
        search_limit: 5,
        search_query_ascii: false,
    });
    p.access_token = Some("token".into());
    p.retry_wait = 0;
    p
}

fn empty_server() -> Server {
    serve(|_| Reply::json(json!({"tracks": {"items": []}})))
}

#[test]
fn mode_is_validated() {
    let s = empty_server();
    let mut p = plugin(&s);
    assert!(!p.parse_opts(Some("fail"), false));
    assert!(p.parse_opts(Some("list"), false));
}

#[test]
fn empty_query_returns_none() {
    let s = empty_server();
    let mut p = plugin(&s);
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::defaults();
    let lib = crate::library::Library::open_with(&dir.path().join("l.db"), None, false).unwrap();
    assert!(p
        .match_library_tracks(&cfg, &lib, &["1=2".to_string()])
        .unwrap()
        .is_none());
}

fn one_item() -> Item {
    let mut item = Item::new_item();
    item.set_path("/file.mp3");
    item.set_str("mb_trackid", "01234");
    item.set_str("album", "Despicable Me 2");
    item.set_str("albumartist", "Pharrell Williams");
    item.set_str("title", "Happy");
    item.set("length", Value::Float(10.0));
    item
}

#[test]
fn missing_track_request_is_a_failure() {
    let s = serve(|_| Reply::json(json!({"tracks": {"items": []}})));
    let mut p = plugin(&s);
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::defaults();
    let lib = crate::library::Library::open_with(&dir.path().join("l.db"), None, false).unwrap();
    let mut item = one_item();
    lib.add(&mut item).unwrap();
    let results = p
        .match_library_tracks(&cfg, &lib, &[])
        .unwrap()
        .unwrap();
    assert!(results.is_empty());
    let q = &s.requests()[0].target;
    assert!(q.starts_with("/v1/search?"), "{q}");
    assert!(q.contains("q=Happy+artist%3A%27Pharrell+Williams%27+album%3A%27Despicable+Me+2%27"), "{q}");
    assert!(q.contains("type=track"));
}

#[test]
fn track_request_lists_the_found_track() {
    let found = json!({"tracks": {"items": [
        {"id": "6NPVjNh8Jhru9xOmyQigds", "name": "Happy", "popularity": 80}
    ]}});
    let s = serve(move |_| Reply::json(found.clone()));
    let mut p = plugin(&s);
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::defaults();
    let lib = crate::library::Library::open_with(&dir.path().join("l.db"), None, false).unwrap();
    let mut item = one_item();
    lib.add(&mut item).unwrap();
    let results = p
        .match_library_tracks(&cfg, &lib, &["Happy".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "6NPVjNh8Jhru9xOmyQigds");
    p.output_match_results(&results);
}

#[test]
fn japanese_query_keeps_non_ascii_unless_ascii_is_configured() {
    let s = serve(|_| Reply::json(json!({"tracks": {"items": []}})));
    let mut p = plugin(&s);
    p.search_response("track", "思想犯 artist:'ヨルシカ'").unwrap();
    let q = &s.requests()[0].target;
    assert!(q.contains("%E6%80%9D%E6%83%B3%E7%8A%AF"), "{q}");
    p.conf.search_query_ascii = true;
    p.search_response("track", "思想犯 artist:'ヨルシカ'").unwrap();
    let q = &s.requests()[1].target;
    assert!(!q.contains("%E6%80%9D"), "{q}");
}

fn album_json() -> J {
    json!({
        "name": "Album",
        "album_type": "album",
        "release_date": "2017-01-02",
        "release_date_precision": "day",
        "artists": [
            {"id": "6m8MRXIVKb6wQaPlBIDMr1", "name": "Project Skylate"},
            {"id": "4kkAIoQmNT5xEoNH5BuQLe", "name": "Sugar Shrill"}
        ],
        "label": "Label",
        "external_urls": {"spotify": "https://open.spotify.com/album/0yhKyyjyKXWUieJ4w1IAEa"},
        "tracks": {"items": [
            {
                "id": "6sjZfVJworBX6TqyjkxIJ1",
                "name": "Song",
                "duration_ms": 100000,
                "track_number": 1,
                "disc_number": 1,
                "external_urls": {"spotify": "https://open.spotify.com/track/6sjZfVJworBX6TqyjkxIJ1"},
                "artists": [{"id": "12345", "name": "Foo"}, {"id": "67890", "name": "Bar"}],
                "album": {"id": "0yhKyyjyKXWUieJ4w1IAEa", "name": "Album"}
            }
        ], "next": null}
    })
}

#[test]
fn multiartist_album_and_track() {
    let album = album_json();
    let track = album["tracks"]["items"][0].clone();
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/v1/albums/0yhKyyjyKXWUieJ4w1IAEa" {
            Reply::json(album.clone())
        } else if path == "/v1/tracks/6sjZfVJworBX6TqyjkxIJ1" {
            Reply::json(track.clone())
        } else {
            Reply::status(404)
        }
    });
    let p = plugin(&s);
    let ctx = crate::metadata_plugins::test_ctx::ctx();
    let info = p
        .album_for_id(&ctx, "0yhKyyjyKXWUieJ4w1IAEa")
        .unwrap()
        .unwrap();
    assert_eq!(info.artist, "Project Skylate, Sugar Shrill");
    assert_eq!(info.artists, ["Project Skylate", "Sugar Shrill"]);
    assert_eq!(info.artist_id, "6m8MRXIVKb6wQaPlBIDMr1");
    assert_eq!(info.artists_ids, ["6m8MRXIVKb6wQaPlBIDMr1", "4kkAIoQmNT5xEoNH5BuQLe"]);
    assert_eq!((info.year, info.month, info.day), (2017, 1, 2));
    assert_eq!(info.tracks.len(), 1);
    assert_eq!(info.tracks[0].artist, "Foo, Bar");
    assert_eq!(info.tracks[0].artists, ["Foo", "Bar"]);
    assert_eq!(info.tracks[0].artist_id, "12345");
    assert_eq!(info.tracks[0].artists_ids, ["12345", "67890"]);
    assert_eq!(info.tracks[0].length, Some(100.0));
    let data = info.to_data();
    assert_eq!(data["spotify_album_id"], crate::hooks::V::Str("0yhKyyjyKXWUieJ4w1IAEa".into()));

    let track = p
        .track_for_id(&ctx, "6sjZfVJworBX6TqyjkxIJ1")
        .unwrap()
        .unwrap();
    assert_eq!(track.artist, "Foo, Bar");
    assert_eq!(track.artists, ["Foo", "Bar"]);
    assert_eq!(track.medium_total, 1);
    assert_eq!(track.index, 1);
}

#[test]
fn invalid_release_date_precision_is_logged() {
    let mut album = album_json();
    album["release_date_precision"] = json!("hour");
    let s = serve(move |_| Reply::json(album.clone()));
    let p = plugin(&s);
    let ctx = crate::metadata_plugins::test_ctx::ctx();
    assert!(p.album_for_id(&ctx, "0yhKyyjyKXWUieJ4w1IAEa").unwrap().is_none());
}

#[test]
fn track_details_are_fetched_in_chunks_of_50() {
    let s = serve(|seen: &Seen| {
        let query = seen.target.split('?').nth(1).unwrap_or("");
        let ids: Vec<String> = query
            .split('&')
            .find_map(|p| p.strip_prefix("ids="))
            .unwrap_or("")
            .replace("%2C", ",")
            .split(',')
            .map(std::string::ToString::to_string)
            .collect();
        Reply::json(json!({
            "tracks": ids.iter().map(|id| json!({
                "id": id, "popularity": 50, "external_ids": {}
            })).collect::<Vec<_>>()
        }))
    });
    let mut p = plugin(&s);
    let ids: Vec<String> = (0..51).map(|i| format!("track-{i}")).collect();
    let details = p.get_track_details_by_id(&ids).unwrap();
    assert_eq!(details.len(), 51);
    let seen = s.requests();
    assert_eq!(seen.len(), 2);
    let count = |t: &str| {
        t.split("ids=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .replace("%2C", ",")
            .split(',')
            .count()
    };
    assert_eq!(count(&seen[0].target), 50);
    assert_eq!(count(&seen[1].target), 1);
}

#[test]
fn fetch_info_uses_the_batch_endpoints() {
    let s = serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/v1/tracks" {
            Reply::json(json!({
                "tracks": [
                    {"id": "id-1", "popularity": 10, "external_ids": {"isrc": "isrc-1", "ean": "ean-1"}},
                    {"id": "id-2", "popularity": 20, "external_ids": {"isrc": "isrc-2"}},
                    {"id": "id-3", "popularity": 30, "external_ids": {"isrc": "isrc-3"}}
                ]
            }))
        } else if path == "/v1/audio-features" {
            Reply::json(json!({
                "audio_features": [
                    {"id": "id-1", "tempo": 100.1, "energy": 0.4, "mode": 1, "time_signature": 4},
                    {"id": "id-2", "tempo": 110.2, "energy": 0.5},
                    {"id": "id-3", "tempo": 120.3, "energy": 0.6}
                ]
            }))
        } else {
            Reply::status(404)
        }
    });
    let mut p = plugin(&s);
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[spotify]").unwrap());
    crate::plugins::types::configure(&cfg).unwrap();
    let lib = crate::library::Library::open_with(&dir.path().join("l.db"), None, false).unwrap();
    let mut items = vec![];
    for idx in 1..=3 {
        let mut item = Item::new_item();
        item.set_path(format!("/file{idx}.mp3"));
        item.set_str("title", format!("Track {idx}"));
        item.set_str("artist", "Artist");
        item.set("length", Value::Float(10.0));
        item.set("spotify_track_id", Value::Str(format!("id-{idx}")));
        lib.add(&mut item).unwrap();
        items.push(item);
    }
    p.fetch_info(&cfg, &lib, &items, false, true).unwrap();
    let seen = s.requests();
    let paths: Vec<&str> = seen
        .iter()
        .map(|r| r.target.split('?').next().unwrap())
        .collect();
    assert_eq!(paths.iter().filter(|p| **p == "/v1/tracks").count(), 1);
    assert_eq!(paths.iter().filter(|p| **p == "/v1/audio-features").count(), 1);
    let stored = lib
        .items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
        .unwrap();
    let by_title = |t: &str| {
        stored
            .iter()
            .find(|i| i.str("title") == t)
            .unwrap()
            .clone()
    };
    assert_eq!(by_title("Track 1").get("spotify_track_popularity"), Some(Value::Int(10)));
    assert_eq!(by_title("Track 2").get("spotify_tempo"), Some(Value::Float(110.2)));
    assert_eq!(by_title("Track 1").get("spotify_mode"), Some(Value::Int(1)));
    assert_eq!(by_title("Track 1").get("spotify_time_signature"), Some(Value::Int(4)));
    assert_eq!(by_title("Track 1").str("ean"), "ean-1");
    assert_eq!(by_title("Track 3").str("isrc"), "isrc-3");
}

#[test]
fn audio_features_403_disables_them() {
    let s = serve(|seen: &Seen| {
        if seen.target.split('?').next() == Some("/v1/audio-features") {
            Reply::status(403)
        } else {
            Reply::json(json!({"tracks": [{"id": "id-1", "popularity": 1}]}))
        }
    });
    let mut p = plugin(&s);
    let ids = vec!["id-1".to_string()];
    let details = p.get_track_details_by_id(&ids).unwrap();
    assert_eq!(details.len(), 1);
    assert!(p.audio_features_available);
    let features = p.track_audio_features_batch(&ids);
    assert!(features.is_empty());
    assert!(!p.audio_features_available);
    // No request once disabled.
    let before = s.requests().len();
    assert!(p.track_audio_features_batch(&ids).is_empty());
    assert_eq!(s.requests().len(), before);
}

#[test]
fn reauthentication_on_401() {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let calls2 = calls.clone();
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        calls2.lock().unwrap().push(seen.target.clone());
        if path == "/api/token" {
            Reply::json(json!({"access_token": "new-token"}))
        } else if seen.header("Authorization") == Some("Bearer new-token") {
            Reply::json(json!({"tracks": {"items": [{"id": "id-1"}]}}))
        } else {
            Reply::status(401)
        }
    });
    let mut p = plugin(&s);
    p.conf.tokenfile = std::env::temp_dir().join(format!(
        "beets-rs-spotify-token-{}.json",
        std::process::id()
    ));
    let results = p.search_response("track", "q").unwrap();
    assert_eq!(results.len(), 1);
    let paths: Vec<String> = calls.lock().unwrap().clone();
    assert_eq!(paths.iter().filter(|p| p.starts_with("/api/token")).count(), 1);
    let _ = std::fs::remove_file(&p.conf.tokenfile);
}
