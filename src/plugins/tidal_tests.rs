//! beets' `test/plugins/test_tidal.py`, against a fake Tidal API on
//! 127.0.0.1 (the reference tests patch the Python methods; here the
//! session's base URLs point at the local server), plus the session's
//! PKCE/token handling.

use super::*;
use crate::plugins::Plugin as _;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use crate::plugins::tidal::api::{TidalApi, merge};
use crate::plugins::tidal::session::{TidalSession, code_challenge};
use serde_json::json;
use std::collections::BTreeMap;
use tempfile::TempDir;

const CURRENT_TS: f64 = 150000000.0;

// ------------------------------------------------------------- fixtures

fn make_search_doc(resource_type: &str, ids: &[&str]) -> J {
    json!({
        "data": [{
            "id": "search-result",
            "type": "searchResults",
            "attributes": {"trackingId": "tracking-id"},
            "relationships": {
                resource_type: {
                    "data": ids.iter()
                        .map(|id| json!({"id": id, "type": resource_type}))
                        .collect::<Vec<_>>(),
                    "links": {},
                }
            }
        }]
    })
}

fn make_artwork(id: &str, href: &str) -> J {
    json!({
        "id": id,
        "type": "artworks",
        "attributes": {
            "mediaType": "IMAGE",
            "files": if href.is_empty() {
                json!([])
            } else {
                json!([{"href": href, "meta": {"width": 1280, "height": 1280}}])
            },
        },
    })
}

fn make_artist(id: &str, name: &str) -> J {
    json!({"id": id, "type": "artists",
           "attributes": {"name": name, "popularity": 0.5}})
}

fn make_track(
    id: &str,
    title: &str,
    duration: &str,
    isrc: &str,
    artist_ids: &[&str],
    version: Option<&str>,
) -> J {
    let mut attrs = json!({
        "title": title,
        "duration": duration,
        "explicit": false,
        "isrc": isrc,
        "key": "C",
        "keyScale": "MAJOR",
        "mediaTags": [],
        "popularity": 0.5,
    });
    if let Some(v) = version {
        attrs["version"] = json!(v);
    }
    json!({
        "id": id,
        "type": "tracks",
        "attributes": attrs,
        "relationships": {"artists": {
            "data": artist_ids.iter()
                .map(|a| json!({"id": a, "type": "artists"}))
                .collect::<Vec<_>>(),
            "links": {},
        }},
    })
}

type Lookups = (BTreeMap<String, J>, BTreeMap<String, J>);

fn make_album(
    id: &str,
    title: &str,
    tracks: &[J],
    artist_ids: &[&str],
    release_date: &str,
    version: Option<&str>,
    cover_art_id: Option<&str>,
) -> (J, Lookups) {
    let artists: BTreeMap<String, J> = artist_ids
        .iter()
        .map(|a| (a.to_string(), make_artist(a, &format!("Artist {a}"))))
        .collect();
    let track_lookup: BTreeMap<String, J> = tracks
        .iter()
        .map(|t| (s(t, "id"), t.clone()))
        .collect();
    let mut attrs = json!({
        "albumType": "ALBUM",
        "barcodeId": "123456",
        "duration": "PT45M",
        "explicit": false,
        "mediaTags": [],
        "numberOfItems": tracks.len(),
        "numberOfVolumes": 1,
        "popularity": 0.5,
        "title": title,
        "releaseDate": release_date,
    });
    if let Some(v) = version {
        attrs["version"] = json!(v);
    }
    let mut relationships = json!({
        "artists": {
            "data": artist_ids.iter()
                .map(|a| json!({"id": a, "type": "artists"}))
                .collect::<Vec<_>>(),
            "links": {},
        },
        "items": {
            "data": tracks.iter()
                .map(|t| json!({"id": s(t, "id"), "type": "tracks"}))
                .collect::<Vec<_>>(),
            "links": {},
        },
    });
    if let Some(ca) = cover_art_id {
        relationships["coverArt"] = json!({
            "data": [{"id": ca, "type": "artworks"}],
            "links": {},
        });
    }
    (
        json!({"id": id, "type": "albums", "attributes": attrs,
               "relationships": relationships}),
        (track_lookup, artists),
    )
}

fn refs(map: &BTreeMap<String, J>) -> BTreeMap<String, &J> {
    map.iter().map(|(k, v)| (k.clone(), v)).collect()
}

fn doc(data: Vec<J>, included: Vec<J>) -> J {
    json!({"data": data, "included": included})
}

// ------------------------------------------------------------------ env

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
    token_path: std::path::PathBuf,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.set(
            "directory",
            serde_yaml::Value::String(dir.path().join("music").display().to_string()),
        );
        cfg.set("plugins", serde_yaml::from_str("[tidal]").unwrap());
        let token_path = dir.path().join("tidal_token.json");
        cfg.set(
            "tidal.tokenfile",
            serde_yaml::Value::String(token_path.display().to_string()),
        );
        let lib = Library::in_memory().unwrap();
        Env {
            _dir: dir,
            cfg,
            lib,
            token_path,
        }
    }

}

/// Route requests by their path (the fake Tidal API).
fn api_server(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (Server, TempDir, Config) {
    let server = serve(handler);
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.set(
        "directory",
        serde_yaml::Value::String(dir.path().join("music").display().to_string()),
    );
    cfg.set("plugins", serde_yaml::from_str("[tidal]").unwrap());
    let token_path = dir.path().join("tidal_token.json");
    cfg.set(
        "tidal.tokenfile",
        serde_yaml::Value::String(token_path.display().to_string()),
    );
    set_test_urls(Some(Urls {
        api_base: format!("{}/v2", server.base),
        auth_url: format!("{}/authorize", server.base),
        token_url: format!("{}/token", server.base),
    }));
    (server, dir, cfg)
}

fn with_api<R>(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static, f: impl FnOnce(&Config) -> R) -> (R, Vec<Seen>) {
    let (server, _dir, cfg) = api_server(handler);
    let r = f(&cfg);
    (r, server.requests())
}

fn query_param_of(seen: &Seen, name: &str) -> Option<String> {
    let q = seen.target.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == name {
            return Some(v.replace('+', " "));
        }
    }
    None
}

// ------------------------------------------------------------------ API

#[test]
fn search_results_uses_query_filter() {
    let (seen, requests) = with_api(
        move |_| Reply::json(json!({"data": []})),
        |cfg| {
            let mut api = TidalApi::new(
                &cfg.str("tidal.client_id"),
                &session::token_path(cfg),
                urls(),
            );
            api.search_results("My Artist My Album", "INCLUDE", &["albums"], "US")
                .unwrap()
        },
    );
    assert_eq!(seen, json!({"data": []}));
    let r = &requests[0];
    assert_eq!(r.method, "GET");
    let path = r.target.split('?').next().unwrap();
    assert_eq!(path, "/v2/searchResults");
    assert_eq!(
        query_param_of(r, "filter%5Bquery%5D").as_deref(),
        Some("My Artist My Album")
    );
    assert_eq!(query_param_of(r, "explicitFilter").as_deref(), Some("INCLUDE"));
    assert_eq!(query_param_of(r, "countryCode").as_deref(), Some("US"));
    assert_eq!(query_param_of(r, "include").as_deref(), Some("albums"));
}

#[test]
fn pagination_merges_pages_and_dedupes_included() {
    let (albums, requests) = with_api(
        move |r| {
            if r.target.contains("page=2") {
                let (a, (tracks, artists)) = make_album(
                    "2",
                    "Second",
                    &[make_track("2", "T2", "PT3M", "I2", &["a1"], None)],
                    &["a1"],
                    "2024-01-15",
                    None,
                    None,
                );
                let mut included: Vec<J> = tracks.into_values().collect();
                included.extend(artists.into_values());
                Reply::json(doc(vec![a], included))
            } else {
                let (a, (tracks, artists)) = make_album(
                    "1",
                    "First",
                    &[make_track("1", "T1", "PT3M", "I1", &["a1"], None)],
                    &["a1"],
                    "2024-01-15",
                    None,
                    None,
                );
                let mut included: Vec<J> = tracks.into_values().collect();
                included.extend(artists.into_values());
                let mut d = doc(vec![a], included);
                d["links"] = json!({"next": "/v2/albums?page=2"});
                Reply::json(d)
            }
        },
        |cfg| {
            let mut tidal = Tidal::new(cfg);
            tidal.search_albums_by_ids(&["1".into(), "2".into()], &[]).unwrap()
        },
    );
    assert_eq!(albums.len(), 2);
    assert_eq!(requests.len(), 2);
}

#[test]
fn tracks_are_batched_in_twenties() {
    let (seen, requests) = with_api(
        move |r| {
            let ids: Vec<String> = r
                .target
                .split('&')
                .filter(|p| p.starts_with("filter%5Bid%5D="))
                .map(|p| p.split_once('=').unwrap().1.to_string())
                .collect();
            let data: Vec<J> = ids
                .iter()
                .map(|id| make_track(id, "T", "PT1M", "I", &["a1"], None))
                .collect();
            let artists = vec![make_artist("a1", "A")];
            Reply::json(doc(data, artists))
        },
        |cfg| {
            let ids: Vec<String> = (0..45).map(|i| i.to_string()).collect();
            let mut tidal = Tidal::new(cfg);
            tidal.search_tracks_by_ids(&ids, &[]).unwrap()
        },
    );
    assert_eq!(seen.len(), 45);
    assert_eq!(requests.len(), 3);
    for r in &requests {
        let n = r.target.matches("filter%5Bid%5D=").count();
        assert!(n <= 20, "{n}");
    }
}

// -------------------------------------------------------------- parsing

fn expected_album_data() -> BTreeMap<String, crate::hooks::V> {
    use crate::hooks::V as V;
    let pairs: Vec<(&str, V)> = vec![
        ("album", V::Str("My Album (Deluxe Edition)".into())),
        ("album_id", V::Str("1".into())),
        ("albumdisambig", V::None),
        ("albumstatus", V::None),
        ("albumtype", V::Str("album".into())),
        ("albumtypes", V::List(vec!["album".into()])),
        ("artist", V::Str("Artist 1001".into())),
        ("artist_credit", V::None),
        ("artist_id", V::Str("1001".into())),
        ("artist_sort", V::None),
        ("artists", V::List(vec!["Artist 1001".into()])),
        ("artists_credit", V::None),
        ("artists_ids", V::List(vec!["1001".into()])),
        ("artists_sort", V::None),
        ("asin", V::None),
        ("barcode", V::Str("123456".into())),
        ("catalognum", V::None),
        ("country", V::None),
        ("cover_art_url", V::None),
        ("data_source", V::Str("Tidal".into())),
        ("data_url", V::None),
        ("day", V::Int(15)),
        ("discogs_albumid", V::None),
        ("discogs_artistid", V::None),
        ("discogs_labelid", V::None),
        ("duration", V::Int(2700)),
        ("genres", V::None),
        ("label", V::None),
        ("language", V::None),
        ("media", V::None),
        ("mediums", V::None),
        ("month", V::Int(1)),
        ("original_day", V::None),
        ("original_month", V::None),
        ("original_year", V::None),
        ("release_group_title", V::None),
        ("releasegroup_id", V::None),
        ("releasegroupdisambig", V::None),
        ("script", V::None),
        ("style", V::None),
        ("tidal_album_id", V::Str("1".into())),
        ("tidal_album_popularity", V::Int(50)),
        ("tidal_artist_id", V::Str("1001".into())),
        ("tidal_updated", V::Float(CURRENT_TS)),
        ("va", V::Bool(false)),
        ("year", V::Int(2024)),
    ];
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[test]
fn parse_album() {
    let tracks = vec![
        make_track("101", "Track One", "PT3M", "ISRC1", &["1001"], None),
        make_track(
            "102",
            "Track Two",
            "PT4M",
            "ISRC2",
            &["1001"],
            Some("Remastered"),
        ),
    ];
    let (album, (track_lookup, artist_lookup)) = make_album(
        "1",
        "My Album",
        &tracks,
        &["1001"],
        "2024-01-15",
        Some("Deluxe Edition"),
        None,
    );
    let cfg = Env::new().cfg;
    let info = Tidal::new(&cfg).get_album_info(
        &album,
        &refs(&track_lookup),
        &refs(&artist_lookup),
        &refs(&BTreeMap::new()),
    );
    let opts = crate::hooks::ApplyOpts::from_config(&cfg);
    let mut got = crate::hooks::album_raw_data(&info.to_data(), &opts);
    got.remove("tidal_updated");
    let mut want = expected_album_data();
    want.remove("tidal_updated");
    assert_eq!(got, want);
    assert_eq!(info.tracks.len(), 2);

    use crate::hooks::V as V;
    let first = info.tracks[0].to_data();
    assert_eq!(first["title"], V::Str("Track One".into()));
    assert_eq!(first["artist"], V::Str("Artist 1001".into()));
    assert_eq!(first["length"], V::Float(180.0));
    assert_eq!(first["isrc"], V::Str("ISRC1".into()));
    assert_eq!(first["index"], V::Int(1));
    assert_eq!(first["track_id"], V::Str("101".into()));
    assert_eq!(first["tidal_track_id"], V::Str("101".into()));
    assert_eq!(first["tidal_artist_id"], V::Str("1001".into()));
    assert_eq!(first["tidal_track_popularity"], V::Int(50));
    assert_eq!(first["label"], V::None);
    assert_eq!(first["medium"], V::None);
    assert_eq!(first["medium_index"], V::None);
    assert_eq!(first["medium_total"], V::None);
    assert_eq!(first["artist_credit"], V::None);
    assert_eq!(first["artists_credit"], V::None);
    assert_eq!(first["artists_sort"], V::None);
    assert_eq!(first["data_url"], V::None);
    assert!(!first.contains_key("trackdisambig"));

    let second = info.tracks[1].to_data();
    assert_eq!(second["title"], V::Str("Track Two (Remastered)".into()));
    assert_eq!(second["length"], V::Float(240.0));
    assert_eq!(second["isrc"], V::Str("ISRC2".into()));
    assert_eq!(second["index"], V::Int(2));
}

#[test]
fn parse_album_title_version() {
    let track = make_track("101", "My Song", "PT3M", "ISRC001", &["1001"], None);
    let (album, (tracks, artists)) = make_album(
        "3",
        "My Album",
        &[track],
        &["1001"],
        "2024-01-15",
        Some("Deluxe Edition"),
        None,
    );
    let cfg = Env::new().cfg;
    let info = Tidal::new(&cfg).get_album_info(
        &album,
        &refs(&tracks),
        &refs(&artists),
        &refs(&BTreeMap::new()),
    );
    assert_eq!(info.album, "My Album (Deluxe Edition)");
}

#[test]
fn artwork_urls() {
    let cfg = Env::new().cfg;
    let tidal = Tidal::new(&cfg);
    let (track, artists) = {
        let track = make_track("t1", "Song", "PT3M", "ISRC001", &["a1"], None);
        let (album, (tracks, artists)) = make_album(
            "al1",
            "Album",
            &[track],
            &["a1"],
            "2024-01-15",
            None,
            Some("ca1"),
        );
        let artwork = make_artwork("ca1", "https://resources.tidal.com/images/ca1/1280x1280.jpg");
        let info = tidal.get_album_info(
            &album,
            &refs(&tracks),
            &refs(&artists),
            &refs(&BTreeMap::from([("ca1".to_string(), artwork)])),
        );
        assert_eq!(
            info.to_data()["cover_art_url"],
            crate::hooks::V::Str("https://resources.tidal.com/images/ca1/1280x1280.jpg".into())
        );
        (album, artists)
    };
    // No files in the artwork: no cover.
    let (album2, tracks2, artists2) = {
        let track = make_track("t1", "Song", "PT3M", "ISRC001", &["a1"], None);
        let (album, (tracks, artists)) = make_album(
            "al2",
            "Album",
            &[track],
            &["a1"],
            "2024-01-15",
            None,
            Some("ca1"),
        );
        (album, tracks, artists)
    };
    let info = tidal.get_album_info(
        &album2,
        &refs(&tracks2),
        &refs(&artists2),
        &refs(&BTreeMap::from([("ca1".to_string(), make_artwork("ca1", ""))])),
    );
    assert_eq!(info.to_data()["cover_art_url"], crate::hooks::V::None);
    // No coverArt relationship: no cover.
    let (album3, (tracks3, artists3)) = make_album(
        "al3",
        "Album",
        &[make_track("t1", "Song", "PT3M", "ISRC001", &["a1"], None)],
        &["a1"],
        "2024-01-15",
        None,
        None,
    );
    let info = tidal.get_album_info(
        &album3,
        &refs(&tracks3),
        &refs(&artists3),
        &refs(&BTreeMap::new()),
    );
    assert_eq!(info.to_data()["cover_art_url"], crate::hooks::V::None);
    let _ = (track, artists);
}

#[test]
fn parse_track_fields() {
    let cfg = Env::new().cfg;
    let tidal = Tidal::new(&cfg);
    let track = make_track("101", "My Track", "PT4M", "ISRC456", &["1001"], None);
    let artists = BTreeMap::from([("1001".to_string(), make_artist("1001", "My Artist"))]);
    let info = tidal.get_track_info(&track, &refs(&artists));
    assert_eq!(info.title, "My Track");
    assert_eq!(info.track_id, "101");
    assert_eq!(info.length, Some(240.0));
    assert_eq!(info.isrc, "ISRC456");
    assert_eq!(info.artist, "My Artist");
    use crate::hooks::V as V;
    assert_eq!(info.to_data()["tidal_track_id"], V::Str("101".into()));
    assert_eq!(info.to_data()["tidal_artist_id"], V::Str("1001".into()));
    assert_eq!(info.to_data()["tidal_track_popularity"], V::Int(50));

    let versioned = make_track(
        "102",
        "My Song",
        "PT3M",
        "ISRC002",
        &["1001"],
        Some("Remastered"),
    );
    let info = tidal.get_track_info(&versioned, &refs(&artists));
    assert_eq!(info.title, "My Song (Remastered)");
    assert_eq!(info.track_id, "102");
}

// --------------------------------------------------------------- lookups

#[test]
fn track_for_id_and_missing() {
    let track = make_track("490839595", "API Track", "PT3M", "ISRC001", &["1001"], None);
    let artist = make_artist("1001", "API Artist");
    let (result, requests) = with_api(
        move |r| {
            if r.target.contains("filter%5Bid%5D=490839595") {
                Reply::json(doc(vec![track.clone()], vec![artist.clone()]))
            } else {
                Reply::json(doc(vec![], vec![]))
            }
        },
        |cfg| {
            let mut tidal = Tidal::new(cfg);
            let found = tidal
                .track_for_id("https://tidal.com/track/490839595")
                .unwrap();
            let missing = tidal.track_for_id("does_not_exist").unwrap();
            (found, missing)
        },
    );
    let info = result.0.unwrap();
    assert_eq!(info.title, "API Track");
    assert_eq!(info.track_id, "490839595");
    assert_eq!(info.artist, "API Artist");
    assert!(result.1.is_none());
    // Only the URL lookup asked the API.
    assert_eq!(requests.len(), 1);
}

#[test]
fn album_for_id_and_missing() {
    let track = make_track("101", "Album Track", "PT3M30S", "ISRC001", &["1001"], None);
    let (album, (tracks, artists)) = make_album(
        "226495055",
        "API Album",
        &[track],
        &["1001"],
        "2024-01-15",
        None,
        None,
    );
    let included = {
        let mut v: Vec<J> = artists.into_values().collect();
        v.extend(tracks.values().cloned());
        v
    };
    let (result, _) = with_api(
        move |_| Reply::json(doc(vec![album.clone()], included.clone())),
        |cfg| {
            let mut tidal = Tidal::new(cfg);
            let found = tidal
                .album_for_id("https://tidal.com/album/226495055")
                .unwrap();
            let missing = tidal.album_for_id("does_not_exist").unwrap();
            (found, missing)
        },
    );
    let info = result.0.unwrap();
    assert_eq!(info.album, "API Album");
    assert_eq!(info.album_id, "226495055");
    assert_eq!(info.tracks.len(), 1);
    assert_eq!(info.tracks[0].title, "Album Track");
    assert!(result.1.is_none());
}

#[test]
fn candidates_by_barcode() {
    let track = make_track("101", "Album Track", "PT3M", "ISRC001", &["1001"], None);
    let (album, (tracks, artists)) = make_album(
        "1",
        "Barcode Album",
        &[track],
        &["1001"],
        "2024-01-15",
        None,
        None,
    );
    let included = {
        let mut v: Vec<J> = artists.into_values().collect();
        v.extend(tracks.values().cloned());
        v
    };
    let (found, requests) = with_api(
        move |_| Reply::json(doc(vec![album.clone()], included.clone())),
        |cfg| {
            let mut item = Item::new_item();
            item.set_str("barcode", "123456");
            Tidal::new(cfg)
                .candidates(&[item], "Artist", "Album", false)
                .unwrap()
        },
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].album, "Barcode Album");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].target.contains("filter%5BbarcodeId%5D=123456"));
}

#[test]
fn candidates_query_fallback() {
    let track = make_track("101", "Album Track", "PT3M", "ISRC001", &["1001"], None);
    let (album, (tracks, artists)) = make_album(
        "1",
        "Query Album",
        &[track],
        &["1001"],
        "2024-01-15",
        None,
        None,
    );
    let included = {
        let mut v: Vec<J> = artists.into_values().collect();
        v.extend(tracks.values().cloned());
        v
    };
    let (found, requests) = with_api(
        move |r| {
            if r.target.starts_with("/v2/searchResults") {
                Reply::json(make_search_doc("albums", &["1"]))
            } else {
                Reply::json(doc(vec![album.clone()], included.clone()))
            }
        },
        |cfg| {
            let mut item = Item::new_item();
            item.set_str("title", "My Song");
            item.set_str("artist", "My Artist");
            item.set_str("album", "My Album");
            Tidal::new(cfg)
                .candidates(&[item], "My Artist", "My Album", false)
                .unwrap()
        },
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].album, "Query Album");
    assert!(
        requests
            .iter()
            .any(|r| r.target.starts_with("/v2/searchResults"))
    );
}

#[test]
fn album_search_handles_empty_response() {
    let (found, requests) = with_api(
        move |_| Reply::json(json!({"data": []})),
        |cfg| Tidal::new(cfg).search_albums_by_query("missing album").unwrap(),
    );
    assert!(found.is_empty());
    assert_eq!(requests.len(), 1);
}

#[test]
fn item_candidates_by_isrc_and_query() {
    let track = make_track("490839595", "ISRC Track", "PT3M", "ISRC001", &["1001"], None);
    let artist = make_artist("1001", "ISRC Artist");
    let (found, requests) = with_api(
        move |r| {
            if r.target.contains("filter%5Bisrc%5D=ISRC001") {
                Reply::json(doc(vec![track.clone()], vec![artist.clone()]))
            } else {
                Reply::json(doc(vec![], vec![]))
            }
        },
        |cfg| {
            let mut item = Item::new_item();
            item.set_str("isrc", "ISRC001");
            Tidal::new(cfg)
                .item_candidates(&item, "Artist", "Title")
                .unwrap()
        },
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "ISRC Track");
    assert_eq!(requests.len(), 1);

    // Query fallback.
    let query_track = make_track("490839595", "Query Track", "PT3M", "ISRC002", &["1001"], None);
    let query_artist = make_artist("1001", "Query Artist");
    let (found, _) = with_api(
        move |r| {
            if r.target.starts_with("/v2/searchResults") {
                let mut d = make_search_doc("tracks", &["490839595"]);
                d["included"] = json!([query_track, query_artist]);
                Reply::json(d)
            } else {
                Reply::json(doc(vec![], vec![]))
            }
        },
        |cfg| {
            let mut item = Item::new_item();
            item.set_str("title", "Query Song");
            item.set_str("artist", "Query Artist");
            Tidal::new(cfg)
                .item_candidates(&item, "Query Artist", "Query Song")
                .unwrap()
        },
    );
    assert_eq!(found[0].title, "Query Track");
}

#[test]
fn track_search_handles_empty_response() {
    let (found, _) = with_api(
        move |_| Reply::json(json!({"data": []})),
        |cfg| Tidal::new(cfg).search_tracks_by_query("missing track").unwrap(),
    );
    assert!(found.is_empty());
}

// --------------------------------------------------------------- helpers

#[test]
fn static_helpers() {
    use crate::hooks::V as V;
    let with = |pairs: &[(&str, J)]| -> J {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), v.clone());
        }
        J::Object(m)
    };
    assert_eq!(
        parse_data_url(&with(&[(
            "externalLinks",
            json!([{"href": "https://tidal.com/b/123", "meta": ""}])
        )])),
        Some("https://tidal.com/b/123".into())
    );
    assert_eq!(parse_data_url(&with(&[])), None);
    assert_eq!(
        parse_label(&with(&[("copyright", json!({"text": "(P) 2024 Tidal"}))])),
        Some("Tidal".into())
    );
    assert_eq!(parse_label(&with(&[])), None);

    for (text, want) in [
        ("© 2017 Blackened Recordings", "Blackened Recordings"),
        ("(C) 2019 Ghosteen Ltd", "Ghosteen"),
        ("(P) 2007 Century Media Records Ltd.", "Century Media Records"),
        ("℗ 2024 Tidal", "Tidal"),
        ("Lightning Bolt", "Lightning Bolt"),
        (
            "Kim Gordon under exclusive license to Matador Records",
            "Matador Records",
        ),
        (
            "© 2011 Motown Records, a Division of UMG Recordings, Inc.",
            "Motown Records",
        ),
        (
            "© 2019 Atlantic Recording Corporation for the United States and \
             WEA International Inc. for the world outside of the United States",
            "Atlantic Recording Corporation",
        ),
        ("(P) 1992 Zomba Recording LLC", "Zomba Recording"),
    ] {
        assert_eq!(normalize_label(text), want, "{text}");
    }

    assert_eq!(
        parse_release_date(&json!({"releaseDate": "2024-01-15"})),
        Some((2024, 1, 15))
    );
    assert_eq!(parse_release_date(&json!({})), None);
    assert_eq!(parse_release_date(&json!({"releaseDate": "2024"})), None);

    let log = PluginLog::new("tidal");
    for (duration, want) in [
        ("PT30S", Some(30)),
        ("PT3M30S", Some(210)),
        ("PT4M", Some(240)),
        ("PT1H", Some(3600)),
        ("PT1H30M", Some(5400)),
    ] {
        assert_eq!(duration_to_seconds(duration, &log), want);
    }
    let (_r, lines) = crate::logging::capture(|| duration_to_seconds("invalid", &log));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Invalid ISO 8601 duration: invalid")),
        "{lines:?}"
    );

    assert_eq!(parse_popularity(&json!({"popularity": 0.5})), 50);
    assert_eq!(parse_popularity(&json!({"popularity": 1.0})), 100);
    assert_eq!(parse_popularity(&json!({"popularity": 0.0})), 0);
    // Python's round(): halves to even.
    assert_eq!(parse_popularity(&json!({"popularity": 0.125})), 12);
    assert_eq!(parse_popularity(&json!({"popularity": 0.135})), 14);

    assert_eq!(
        Tidal::extract_id("https://tidal.com/album/226495055"),
        Some("226495055".into())
    );
    assert_eq!(
        Tidal::extract_id("https://tidal.com/browse/track/490839595"),
        Some("490839595".into())
    );
    assert_eq!(Tidal::extract_id("226495055"), Some("226495055".into()));
    assert_eq!(Tidal::extract_id("does_not_exist"), None);

    // The artwork helper on its own.
    let (album, (_, artists)) = make_album(
        "al1",
        "Album",
        &[make_track("t1", "Song", "PT3M", "ISRC001", &["a1"], None)],
        &["a1"],
        "2024-01-15",
        None,
        Some("ca1"),
    );
    let _ = artists;
    assert_eq!(
        parse_artwork_url(
            &album,
            &refs(&BTreeMap::from([(
                "ca1".to_string(),
                make_artwork("ca1", "https://example.com/cover.jpg")
            )]))
        ),
        Some("https://example.com/cover.jpg".into())
    );
    assert_eq!(
        parse_artwork_url(
            &album,
            &refs(&BTreeMap::from([(
                "ca1".to_string(),
                make_artwork("ca1", "")
            )]))
        ),
        None
    );
    let (no_rel, _) = make_album(
        "al1",
        "Album",
        &[make_track("t1", "Song", "PT3M", "ISRC001", &["a1"], None)],
        &["a1"],
        "2024-01-15",
        None,
        None,
    );
    assert_eq!(parse_artwork_url(&no_rel, &BTreeMap::new()), None);
    let _ = V::None;
}

#[test]
fn merge_dedupes_included() {
    let mut a = json!({"data": [{"id": "1"}], "included": [{"type": "artists", "id": "a"}]});
    let b = json!({
        "data": [{"id": "2"}],
        "included": [{"type": "artists", "id": "a"}, {"type": "tracks", "id": "t"}],
        "links": {"next": "x"},
    });
    merge(&mut a, b);
    assert_eq!(a["data"].as_array().unwrap().len(), 2);
    assert_eq!(a["included"].as_array().unwrap().len(), 2);
    assert_eq!(a["links"]["next"], "x");
}

// -------------------------------------------------------------- session

#[test]
fn authorization_url_has_pkce_and_token_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("t.json");
    let urls = Urls {
        api_base: "http://api/v2".into(),
        auth_url: "http://auth/authorize".into(),
        token_url: "http://auth/token".into(),
    };
    let mut s = TidalSession::new("client-id", &token_path, urls);
    let (url, state) = s.authorization_url();
    assert!(url.starts_with("http://auth/authorize?"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=client-id"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains(&format!("state={state}")));
    assert!(url.contains("redirect_uri=https%3A%2F%2Flocalhost"));
    // The challenge is the URL-safe SHA-256 of a 43-char verifier.
    let challenge = query_param(&url, "code_challenge").unwrap();
    assert_eq!(challenge.len(), 43);
    assert!(!challenge.contains('='));
    let verifier = s.verifier.clone().unwrap();
    assert_eq!(verifier.len(), 43);
    assert_eq!(code_challenge(&verifier), challenge);

    // A hand-written token file round trips.
    let token = json!({"access_token": "a", "refresh_token": "r", "expires_at": 4102444800.0});
    s.save_token(&token).unwrap();
    let saved: J = serde_json::from_str(&std::fs::read_to_string(&token_path).unwrap()).unwrap();
    assert_eq!(saved, token);
    let s2 = TidalSession::new("client-id", &token_path, Urls::default());
    assert_eq!(s2.token(), Some(&token));
}

#[test]
fn verifier_and_state_are_random_and_well_formed() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("t.json");
    let urls = Urls::default();
    let mut a = TidalSession::new("client-id", &token_path, urls.clone());
    let mut b = TidalSession::new("client-id", &token_path, urls);
    let (url_a, state_a) = a.authorization_url();
    let (url_b, state_b) = b.authorization_url();
    let verifier_a = a.verifier.clone().unwrap();
    let verifier_b = b.verifier.clone().unwrap();
    assert_eq!(verifier_a.len(), 43);
    assert_eq!(verifier_b.len(), 43);
    assert_ne!(verifier_a, verifier_b);
    assert!(verifier_a != verifier_b);
    assert_ne!(state_a, state_b);
    let allowed = |s: &str| {
        s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c))
    };
    assert!(allowed(&verifier_a), "{verifier_a}");
    assert!(allowed(&verifier_b), "{verifier_b}");
    assert_ne!(query_param(&url_a, "code_challenge"), query_param(&url_b, "code_challenge"));
}

#[test]
fn rate_limit_retries_after_retry_after() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A raw server: the shared test server gives every reply the same
    // headers, and this needs a 429 with Retry-After first.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
            }
            let n = calls2.fetch_add(1, Ordering::SeqCst);
            let (status, extra, body) = if n < 1 {
                (429, "Retry-After: 1\r\n", "")
            } else {
                (200, "", "{\"data\": [], \"included\": []}")
            };
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("token.json");
    std::fs::write(
        &token_path,
        json!({"access_token": "a", "expires_at": 4102444800.0}).to_string(),
    )
    .unwrap();
    let urls = Urls {
        api_base: format!("{base}/v2"),
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
    };
    let mut s = TidalSession::new("client-id", &token_path, urls);
    let got = s.get_json("/tracks", &[]).unwrap();
    assert_eq!(got["data"], json!([]));
    assert!(calls.load(Ordering::SeqCst) >= 2, "{}", calls.load(Ordering::SeqCst));
}

#[test]
fn fetch_token_posts_code_and_saves_it() {
    let (token_path, requests) = {
        let (server, dir, _cfg) = api_server(|r| {
            if r.target.starts_with("/token") {
                Reply::json(json!({
                    "access_token": "access",
                    "refresh_token": "refresh",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                }))
            } else {
                Reply::status(404)
            }
        });
        let token_path = dir.path().join("token.json");
        let urls = Urls {
            api_base: format!("{}/v2", server.base),
            auth_url: format!("{}/authorize", server.base),
            token_url: format!("{}/token", server.base),
        };
        let mut s = TidalSession::new("client-id", &token_path, urls);
        let (url, state) = s.authorization_url();
        let redirect = format!("https://localhost/?code=abc&state={state}");
        let _ = url;
        s.fetch_token(&redirect).unwrap();
        let saved: J = serde_json::from_str(&std::fs::read_to_string(&token_path).unwrap()).unwrap();
        assert_eq!(saved["access_token"], "access");
        assert!(saved["expires_at"].as_f64().unwrap() > 0.0);
        (token_path, server.requests())
    };
    let _ = token_path;
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(r.method, "POST");
    let body = String::from_utf8(r.body.clone()).unwrap();
    assert!(body.contains("grant_type=authorization_code"), "{body}");
    assert!(body.contains("code=abc"), "{body}");
    assert!(body.contains("client_id=client-id"), "{body}");
    assert!(body.contains("code_verifier="), "{body}");
}

#[test]
fn expired_token_is_refreshed() {
    let (dir, server) = {
        let (server, dir, _cfg) = api_server(|r| {
            if r.target.starts_with("/token") {
                Reply::json(json!({
                    "access_token": "new",
                    "refresh_token": "refresh2",
                    "expires_in": 3600,
                }))
            } else {
                Reply::json(doc(vec![], vec![]))
            }
        });
        (dir, server)
    };
    let token_path = dir.path().join("token.json");
    std::fs::write(
        &token_path,
        json!({
            "access_token": "old",
            "refresh_token": "refresh",
            "expires_at": 1.0,
        })
        .to_string(),
    )
    .unwrap();
    let urls = Urls {
        api_base: format!("{}/v2", server.base),
        auth_url: format!("{}/authorize", server.base),
        token_url: format!("{}/token", server.base),
    };
    let mut s = TidalSession::new("client-id", &token_path, urls);
    s.get_json("/tracks", &[]).unwrap();
    let saved: J = serde_json::from_str(&std::fs::read_to_string(&token_path).unwrap()).unwrap();
    assert_eq!(saved["access_token"], "new");
    let requests = server.requests();
    assert!(requests.iter().any(|r| r.target.starts_with("/token")));
    let api = requests
        .iter()
        .find(|r| r.target.starts_with("/v2/tracks"))
        .unwrap();
    assert_eq!(api.header("authorization"), Some("Bearer new"));
}

#[test]
fn import_begin_requires_a_token_file() {
    let env = Env::new();
    let plugin = TidalPlugin::new(&env.cfg);
    let ctx = crate::plugins::PluginCtx {
        cfg: &env.cfg,
        lib: &env.lib,
        write: false,
    };
    let err = plugin.import_begin(&ctx).unwrap_err().to_string();
    assert_eq!(
        err,
        "Please login to TIDAL using `beet tidal --auth` or disable tidal plugin"
    );
    std::fs::write(&env.token_path, "{}").unwrap();
    plugin.import_begin(&ctx).unwrap();
}

// -------------------------------------------------------------- tidalsync

fn popularity_doc(track: J, artist: J) -> Reply {
    Reply::json(doc(vec![track], vec![artist]))
}

#[test]
fn sync_updates_popularity() {
    let track = make_track("490839595", "Test Track", "PT3M", "ISRC001", &["1001"], None);
    let artist = make_artist("1001", "Test Artist");
    let (result, requests) = with_api(
        move |_| popularity_doc(track.clone(), artist.clone()),
        |cfg| {
            let env = Env::new();
            let _ = &env;
            let lib = Library::in_memory().unwrap();
            let mut item = Item::new_item();
            item.set_str("tidal_track_id", "490839595");
            item.set_str("title", "Test Track");
            item.set_str("artist", "Test Artist");
            lib.add(&mut item).unwrap();
            let mut tidal = Tidal::new(cfg);
            let log = PluginLog::new("tidal");
            tidal
                .sync_item_popularity(&lib, &[item.clone()], false, false, &log)
                .unwrap();
            let reloaded = lib.get_item(item.id.unwrap()).unwrap().unwrap();
            (
                reloaded.flex.get("tidal_track_popularity").cloned(),
                reloaded.flex.get("tidal_updated").cloned(),
            )
        },
    );
    assert_eq!(result.0.as_deref(), Some("50"));
    assert!(
        result.1.as_deref().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) > 0.0
    );
    assert_eq!(requests.len(), 1);
}

#[test]
fn sync_skips_existing_unless_forced() {
    let _env_cfg = Env::new().cfg;
    let lib = Library::in_memory().unwrap();
    let mut item = Item::new_item();
    item.set_str("tidal_track_id", "490839595");
    item.set_int("tidal_track_popularity", 42);
    lib.add(&mut item).unwrap();
    let track = make_track("490839595", "Test Track", "PT3M", "ISRC001", &["1001"], None);
    let artist = make_artist("1001", "Test Artist");
    let (got, requests) = with_api(
        move |_| popularity_doc(track.clone(), artist.clone()),
        |cfg| {
            let mut tidal = Tidal::new(cfg);
            let log = PluginLog::new("tidal");
            tidal
                .sync_item_popularity(&lib, &[item.clone()], false, false, &log)
                .unwrap();
            let not_forced = lib
                .get_item(item.id.unwrap())
                .unwrap()
                .unwrap()
                .flex
                .get("tidal_track_popularity")
                .cloned();
            tidal
                .sync_item_popularity(&lib, &[item.clone()], false, true, &log)
                .unwrap();
            let forced = lib
                .get_item(item.id.unwrap())
                .unwrap()
                .unwrap()
                .flex
                .get("tidal_track_popularity")
                .cloned();
            (not_forced, forced)
        },
    );
    assert_eq!(got.0.as_deref(), Some("42"));
    assert_eq!(got.1.as_deref(), Some("50"));
    assert_eq!(requests.len(), 1);
}

#[test]
fn sync_skips_models_without_ids_and_on_misses() {
    let lib = Library::in_memory().unwrap();
    let mut no_id = Item::new_item();
    no_id.set_str("title", "No ID");
    lib.add(&mut no_id).unwrap();
    let mut missing = Item::new_item();
    missing.set_str("tidal_track_id", "490839595");
    lib.add(&mut missing).unwrap();
    let (result, requests) = with_api(
        move |_| Reply::json(doc(vec![], vec![])),
        |cfg| {
            let mut tidal = Tidal::new(cfg);
            let log = PluginLog::new("tidal");
            tidal
                .sync_item_popularity(&lib, &[no_id.clone(), missing.clone()], false, false, &log)
                .unwrap();
            let a = lib
                .get_item(no_id.id.unwrap())
                .unwrap()
                .unwrap()
                .flex
                .get("tidal_track_popularity")
                .cloned();
            let b = lib
                .get_item(missing.id.unwrap())
                .unwrap()
                .unwrap()
                .flex
                .get("tidal_track_popularity")
                .cloned();
            let c = lib
                .get_item(missing.id.unwrap())
                .unwrap()
                .unwrap()
                .flex
                .get("tidal_updated")
                .cloned();
            (a, b, c)
        },
    );
    assert_eq!(result, (None, None, None));
    // Only the item with an ID was looked up.
    assert_eq!(requests.len(), 1);
}

#[test]
fn sync_updates_album_popularity() {
    let track = make_track("490839595", "Test Track", "PT3M", "ISRC001", &["1001"], None);
    let artist = make_artist("1001", "Test Artist");
    let album_track = make_track("101", "Album Track", "PT3M", "ISRC001", &["1001"], None);
    let (album_data, (_, artists)) = make_album(
        "251380836",
        "Test Album",
        &[album_track],
        &["1001"],
        "2024-01-15",
        None,
        None,
    );
    let included: Vec<J> = artists.into_values().collect();
    let (result, _) = with_api(
        move |r| {
            if r.target.starts_with("/v2/tracks") {
                popularity_doc(track.clone(), artist.clone())
            } else {
                Reply::json(doc(vec![album_data.clone()], included.clone()))
            }
        },
        |cfg| {
            let lib = Library::in_memory().unwrap();
            let mut item = Item::new_item();
            item.set_str("tidal_track_id", "490839595");
            let mut items = [item];
            let mut album = lib.add_album(&mut items).unwrap();
            album.set_str("tidal_album_id", "251380836");
            lib.store(&album).unwrap();
            let mut tidal = Tidal::new(cfg);
            let log = PluginLog::new("tidal");
            tidal
                .sync_album_popularity(&lib, &[album.clone()], false, false, &log)
                .unwrap();
            let reloaded = lib.get_album(album.id.unwrap()).unwrap().unwrap();
            reloaded.flex.get("tidal_album_popularity").cloned()
        },
    );
    assert_eq!(result.as_deref(), Some("50"));
}

#[test]
fn tidalsync_command_defaults_to_items_and_respects_limit() {
    // A fresh API is built by the command; point this thread's URLs at a
    // server that answers every track lookup.
    let artist = make_artist("a1", "A");
    let (out, requests) = with_api(
        move |r| {
            let id = query_param_of(r, "filter%5Bid%5D").unwrap_or_default();
            let track = make_track(&id, "One", "PT3M", "ISRC1", &["a1"], None);
            popularity_doc(track, artist.clone())
        },
        |cfg| {
            let lib = Library::in_memory().unwrap();
            for i in 0..2 {
                let mut item = Item::new_item();
                item.set_str("tidal_track_id", i.to_string());
                item.set_str("data_source", "Tidal");
                lib.add(&mut item).unwrap();
            }
            let (_r, lines) = crate::logging::capture(|| {
                command_tidalsync(cfg, &lib, false, false, false, Some(1), &[]).unwrap()
            });
            let got: Vec<String> = lib
                .items(&Query::parse(Kind::Item, &["data_source:tidal".to_string()]).unwrap())
                .unwrap()
                .iter()
                .filter_map(|i| i.flex.get("tidal_track_popularity").cloned())
                .collect();
            (lines, got)
        },
    );
    assert!(
        out.0.iter().any(|l| l.contains("Syncing popularity for 1 items")),
        "{:?}",
        out.0
    );
    assert!(
        out.0
            .iter()
            .any(|l| l.contains("Successfully synchronised popularity for 1 items")),
        "{:?}",
        out.0
    );
    assert_eq!(out.1, vec!["50".to_string()]);
    assert_eq!(requests.len(), 1);
}

#[test]
fn tidalsync_command_empty_library() {
    let (_r, lines) = crate::logging::capture(|| {
        let env = Env::new();
        command_tidalsync(&env.cfg, &env.lib, false, false, false, None, &[]).unwrap()
    });
    assert!(
        lines.iter().any(|l| l.contains("Syncing popularity for 0 items")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Successfully synchronised popularity for 0 items")),
        "{lines:?}"
    );
}

// -------------------------------------------------------------- registry

#[test]
fn defaults_match_beets() {
    let def = crate::plugin_registry::find("tidal").unwrap();
    assert_eq!(def.name, "tidal");
    assert_eq!(def.redacted, &["tidal.client_id"]);
    assert!(def.defaults.contains("client_id: mcjmpl1bPATJXcBT"));
    assert!(def.defaults.contains("tokenfile: tidal_token.json"));
    assert!(def.defaults.contains("search_limit: 5"));
    assert!(def.defaults.contains("data_source_mismatch_penalty: 0.5"));
}

#[test]
fn types_are_declared() {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[tidal]").unwrap());
    let types = crate::plugins::types::flex_types(&cfg).unwrap();
    assert_eq!(
        types[0].get("tidal_track_popularity"),
        Some(&crate::model::FieldType::Int)
    );
    assert_eq!(
        types[1].get("tidal_album_popularity"),
        Some(&crate::model::FieldType::Int)
    );
    assert_eq!(
        types[0].get("tidal_updated"),
        Some(&crate::model::FieldType::Date)
    );
}
