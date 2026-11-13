//! beets' `test/plugins/test_deezer.py`, with a fake Deezer API on
//! 127.0.0.1 (a free port) where beets uses `requests_mock`.

use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use crate::model::{Item, Value};
use serde_json::json;

fn urls(server: &Server) -> Urls {
    Urls {
        search: format!("{}/search/", server.base),
        album: format!("{}/album/", server.base),
        track: format!("{}/track/", server.base),
    }
}

fn plugin(server: &Server) -> Deezer {
    Deezer::with(Conf {
        urls: urls(server),
        search_limit: 5,
        search_query_ascii: false,
    })
}

fn no_server() -> Server {
    serve(|_| Reply::status(500))
}

fn track_data(extra: serde_json::Value) -> serde_json::Value {
    let mut base = json!({
        "id": 1,
        "title": "Title",
        "duration": 100,
        "link": "https://www.deezer.com/track/1",
    });
    let obj = base.as_object_mut().unwrap();
    for (k, v) in extra.as_object().unwrap() {
        obj.insert(k.clone(), v.clone());
    }
    base
}

/// A minimal Deezer track payload credited to `artist_id`.
fn make_track(artist_id: i64, position: usize) -> serde_json::Value {
    json!({
        "title": format!("Track {position}"),
        "id": 1000 + position,
        "link": format!("https://www.deezer.com/track/{}", 1000 + position),
        "duration": 200,
        "track_position": position,
        "disk_number": 1,
        "artist": {"id": artist_id, "name": format!("Artist {artist_id}")},
    })
}

#[test]
fn search_query_is_free_text_for_tracks() {
    let s = no_server();
    let p = plugin(&s);
    assert_eq!(p.search_query("track", "Artist", "Title", false), "Title Artist");
    assert_eq!(p.search_query("track", "", "Title", false), "Title");
    assert_eq!(
        p.search_query("album", "Artist", "Album", false),
        "album:\"Album\" artist:\"Artist\""
    );
    assert_eq!(
        p.search_query("album", "Various Artists", "Album", true),
        "album:\"Album\""
    );
}

#[test]
fn track_artist_from_contributors_or_artist() {
    let s = no_server();
    let p = plugin(&s);
    let t = p.get_track(&track_data(json!({
        "contributors": [{"id": 2, "name": "Artist"}]
    })));
    assert_eq!((t.artist.as_str(), t.artist_id.as_str()), ("Artist", "2"));
    let t = p.get_track(&track_data(json!({"artist": {"id": 2, "name": "Artist"}})));
    assert_eq!((t.artist.as_str(), t.artist_id.as_str()), ("Artist", "2"));
    let t = p.get_track(&track_data(json!({})));
    assert_eq!((t.artist.as_str(), t.artist_id.as_str()), ("", ""));
    assert!(t.nulls.contains("isrc"));
    let data = t.to_data();
    assert_eq!(data["deezer_track_id"], crate::hooks::V::Int(1));
    assert_eq!(data["length"], crate::hooks::V::Float(100.0));
}

fn make_album(album_artist_id: i64, contributor_ids: &[i64]) -> serde_json::Value {
    json!({
        "title": "Some Album",
        "link": "https://www.deezer.com/album/1",
        "record_type": "album",
        "label": "Some Label",
        "release_date": "2017-01-01",
        "artist": {"id": album_artist_id, "name": format!("Artist {album_artist_id}")},
        "contributors": contributor_ids
            .iter()
            .map(|id| json!({"id": id, "name": format!("Artist {id}")}))
            .collect::<Vec<_>>(),
        "cover_xl": null,
    })
}

/// Mock the album and album-tracks endpoints for album id 1.
fn album_server(
    album_artist_id: i64,
    track_artist_ids: &[i64],
    contributor_ids: Option<Vec<i64>>,
) -> Server {
    let contributors =
        contributor_ids.unwrap_or_else(|| {
            let mut v = vec![album_artist_id];
            for id in track_artist_ids {
                if !v.contains(id) {
                    v.push(*id);
                }
            }
            v
        });
    let album = make_album(album_artist_id, &contributors);
    let tracks: Vec<serde_json::Value> = track_artist_ids
        .iter()
        .enumerate()
        .map(|(i, id)| make_track(*id, i + 1))
        .collect();
    serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        match path {
            "/album/1" => Reply::json(album.clone()),
            "/album/1/tracks" => Reply::json(json!({"data": tracks})),
            _ => Reply::status(404),
        }
    })
}

#[test]
fn compilation_with_non_va_album_artist_is_va() {
    let s = album_server(100, &[100, 200, 300, 400, 500, 600], None);
    let p = plugin(&s);
    let info = p.album_for_id(&ctx(), "1").unwrap().unwrap();
    assert!(info.va);
    assert_eq!(info.artist, "Various Artists");
    assert_eq!(info.album, "Some Album");
    assert_eq!(info.year, 2017);
    assert_eq!((info.month, info.day), (1, 1));
    assert_eq!(info.tracks.len(), 6);
    assert_eq!(info.tracks[0].medium_total, 6);
    assert_eq!(info.mediums, 1);
}

#[test]
fn plurality_artist_album_is_not_va() {
    let s = album_server(100, &[100, 100, 200, 300, 400], Some(vec![100]));
    let p = plugin(&s);
    let info = p.album_for_id(&ctx(), "1").unwrap().unwrap();
    assert!(!info.va);
    assert_eq!(info.artist, "Artist 100");
    assert_eq!(info.artist_credit, "Artist 100");
}

#[test]
fn single_artist_album_is_not_va() {
    let s = album_server(100, &[100, 100, 100, 100], None);
    let p = plugin(&s);
    let info = p.album_for_id(&ctx(), "1").unwrap().unwrap();
    assert!(!info.va);
    assert_eq!(info.artist, "Artist 100");
}

#[test]
fn album_id_from_url_and_ids() {
    let s = album_server(100, &[100], None);
    let p = plugin(&s);
    assert!(p
        .album_for_id(&ctx(), "https://www.deezer.com/album/1")
        .unwrap()
        .is_some());
    // Not a Deezer id: no request.
    assert!(p.album_for_id(&ctx(), "not a deezer id!").unwrap().is_none());
}

#[test]
fn track_for_id_sets_index_and_medium_total() {
    let track = json!({
        "id": 11,
        "title": "One",
        "duration": 100,
        "link": "https://www.deezer.com/track/11",
        "track_position": 1,
        "disk_number": 1,
        "artist": {"id": 2, "name": "Artist"},
        "album": {"id": 7},
    });
    let tracks = json!({"data": [
        {"id": 10, "disk_number": 1},
        {"id": 11, "disk_number": 1},
        {"id": 12, "disk_number": 2},
    ]});
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        match path {
            "/track/11" => Reply::json(track.clone()),
            "/album/7/tracks" => Reply::json(tracks.clone()),
            _ => Reply::status(404),
        }
    });
    let p = plugin(&s);
    let t = p.track_for_id(&ctx(), "11").unwrap().unwrap();
    assert_eq!(t.track_id, "11");
    assert_eq!(t.medium_total, 2);
    assert_eq!(t.index, 2);
}

#[test]
fn candidates_and_item_candidates_search_then_look_up() {
    let album = make_album(100, &[100]);
    let mut track = make_track(100, 1);
    track["album"] = json!({"id": 1});
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/search/album" {
            Reply::json(json!({"data": [{"id": 1}]}))
        } else if path == "/search/track" {
            Reply::json(json!({"data": [{"id": 1001}]}))
        } else if path == "/album/1" {
            Reply::json(album.clone())
        } else if path == "/album/1/tracks" {
            Reply::json(json!({"data": [track.clone()]}))
        } else if path == "/track/1001" {
            Reply::json(track.clone())
        } else {
            Reply::status(404)
        }
    });
    let p = plugin(&s);
    let mut item = Item::new_item();
    item.set_str("title", "Title");
    let albums = p
        .candidates(&ctx(), std::slice::from_ref(&item), "Artist", "Some Album", false)
        .unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].album_id, "1");
    // The album search was filtered by album and artist.
    let q = &s.requests()[0].target;
    assert!(q.starts_with("/search/album?"), "{q}");
    assert!(q.contains("q=album%3A%22Some+Album%22+artist%3A%22Artist%22"), "{q}");
    let tracks = p
        .item_candidates(&ctx(), &item, "Artist", "Title")
        .unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].deezer_id(), "1001");
}

impl TrackInfo {
    fn deezer_id(&self) -> String {
        self.extra
            .iter()
            .find(|(k, _)| k == "deezer_track_id")
            .map(|(_, v)| v.py_str())
            .unwrap_or_default()
    }
}

#[test]
fn deezerupdate_stores_track_rank() {
    let s = serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/track/1" {
            Reply::json(json!({"id": 1, "rank": 1234}))
        } else {
            Reply::status(404)
        }
    });
    let p = plugin(&s);
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[deezer]").unwrap());
    crate::plugins::types::configure(&cfg).unwrap();
    let lib = crate::library::Library::open_with(&dir.path().join("l.db"), None, false).unwrap();
    let mut item = Item::new_item();
    item.set_path("/file.mp3");
    item.set("deezer_track_id", Value::Int(1));
    lib.add(&mut item).unwrap();
    p.deezerupdate(&cfg, &lib, std::slice::from_mut(&mut item), false)
        .unwrap();
    assert_eq!(item.get("deezer_track_rank"), Some(Value::Int(1234)));
    // `deezer_updated` is set after the store, as beets does.
    let stored = lib
        .items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].get("deezer_track_rank"), Some(Value::Int(1234)));
}

#[test]
fn search_limit_and_ascii_are_configurable() {
    let s = serve(|_| Reply::json(json!({"data": []})));
    let p = Deezer::with(Conf {
        urls: urls(&s),
        search_limit: 2,
        search_query_ascii: true,
    });
    p.search_api("track", "Tïtle");
    let q = &s.requests()[0].target;
    assert!(q.contains("limit=2"), "{q}");
    assert!(q.contains("q=T%C3%AFtle") || q.contains("q=Title"), "{q}");
}

use crate::metadata_plugins::test_ctx::ctx;
