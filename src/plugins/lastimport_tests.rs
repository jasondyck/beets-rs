//! The lastimport plugin: last.fm's XML response and the command's loop.

use super::*;
use crate::model::{Item, Kind};
use crate::plugins::requests::testserver::{Reply, serve};
use crate::query::Query;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn cfg(base: &str) -> Config {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[lastimport]").unwrap());
    cfg.set("lastfm.user", "testuser".into());
    cfg.set("lastfm.api_key", "KEY".into());
    cfg.set("lastimport.per_page", serde_yaml::Value::Number(2.into()));
    cfg.set(
        "lastimport.retry_limit",
        serde_yaml::Value::Number(3.into()),
    );
    let _ = base;
    cfg
}

fn log() -> PluginLog {
    PluginLog::new("lastimport")
}

/// The plugin's declared flexible attribute type (run by beets' plugin
/// loading before any command; `crate::plugins::types::configure`).
fn declare_types() {
    crate::model::set_flex_types(
        [(
            "lastfm_play_count".to_string(),
            crate::model::FieldType::Int,
        )]
        .into_iter()
        .collect(),
        std::collections::BTreeMap::default(),
    );
}

fn xml(tracks: &[(&str, &str, &str, i64)], total_pages: i64) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><lfm status=\"ok\">\
         <toptracks page=\"1\" totalPages=\"{total_pages}\">"
    );
    for (title, artist, mbid, playcount) in tracks {
        out.push_str(&format!(
            "<track><name>{title}</name><playcount>{playcount}</playcount>\
             <mbid>{mbid}</mbid><artist><name>{artist}</name></artist></track>"
        ));
    }
    out.push_str("</toptracks></lfm>");
    out
}

fn add(lib: &Library, title: &str, artist: &str, album: &str, mbid: &str) {
    let mut item = Item::new_item();
    item.set_str("title", title);
    item.set_str("artist", artist);
    item.set_str("album", album);
    item.set_path(std::path::PathBuf::from(format!("/lib/{title}.mp3")));
    if !mbid.is_empty() {
        item.set_str("mb_trackid", mbid);
    }
    lib.add(&mut item).unwrap();
}

fn items(lib: &Library) -> Vec<Item> {
    lib.items(&Query::parse(Kind::Item, &[] as &[String]).unwrap())
        .unwrap()
}

#[test]
fn fetch_tracks_sends_pylasts_parameters() {
    let server = serve(|_| Reply::text(&xml(&[("Song", "Artist", "m1", 5)], 3)));
    let (tracks, total) =
        fetch_tracks_from(&format!("{}/2.0/", server.base), &cfg(""), 2, 500).unwrap();
    assert_eq!(total, 3);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].name, "Song");
    assert_eq!(tracks[0].artist, "Artist");
    assert_eq!(tracks[0].mbid.as_deref(), Some("m1"));
    assert_eq!(tracks[0].playcount, 5);
    assert_eq!(tracks[0].album, None);
    let target = &server.requests()[0].target;
    assert!(target.starts_with("/2.0/?"), "{target}");
    assert!(target.contains("method=user.getTopTracks"), "{target}");
    assert!(target.contains("api_key=KEY"), "{target}");
    assert!(target.contains("user=testuser"), "{target}");
    assert!(target.contains("period=overall"), "{target}");
    assert!(target.contains("page=2"), "{target}");
    assert!(target.contains("limit=500"), "{target}");
}

#[test]
fn an_error_response_is_an_error() {
    let server = serve(|_| {
        Reply::text(
            "<?xml version=\"1.0\"?><lfm status=\"failed\">\
             <error code=\"6\">User not found</error></lfm>",
        )
    });
    let err = fetch_tracks_from(&format!("{}/2.0/", server.base), &cfg(""), 1, 2).unwrap_err();
    assert_eq!(err.to_string(), "User not found");
}

#[test]
fn import_lastfm_stores_play_counts() {
    declare_types();
    let server = serve(|r| {
        if r.target.contains("page=2") {
            Reply::text(&xml(&[("Third", "Artist", "m3", 2)], 2))
        } else {
            Reply::text(&xml(
                &[("First", "Artist", "m1", 5), ("Second", "Artist", "", 1)],
                2,
            ))
        }
    });
    let cfg = cfg(&server.base);
    let lib = Library::in_memory().unwrap();
    add(&lib, "First", "Artist", "Album", "m1");
    add(&lib, "Second", "Artist", "Album", "");
    // Nothing matches this one.
    let (result, lines) = crate::logging::capture(|| {
        import_lastfm_from(&format!("{}/2.0/", server.base), &cfg, &lib, &log())
    });
    result.unwrap();
    let all = items(&lib);
    let count = |title: &str| {
        all.iter()
            .find(|i| i.str("title") == title)
            .unwrap()
            .get("lastfm_play_count")
    };
    assert_eq!(count("First"), Some(crate::model::Value::Int(5)));
    assert_eq!(count("Second"), Some(crate::model::Value::Int(1)));
    let want = [
        "lastimport: Fetching last.fm library for @testuser",
        "lastimport: Querying page #1...",
        "lastimport: Querying page #2/2...",
        "lastimport: ... done!",
        "lastimport: finished processing 2 song pages",
        "lastimport: 1 unknown play-counts",
        "lastimport: 2 play-counts imported",
    ];
    for w in want {
        assert!(lines.iter().any(|l| l == w), "missing {w:?} in {lines:?}");
    }
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn an_empty_user_is_an_error() {
    let mut cfg = cfg("");
    cfg.set("lastfm.user", "".into());
    let lib = Library::in_memory().unwrap();
    let err = import_lastfm(&cfg, &lib, &log()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "You must specify a user name for lastimport"
    );
}

#[test]
fn no_data_is_an_error() {
    let server = serve(|_| Reply::text(&xml(&[], 0)));
    let lib = Library::in_memory().unwrap();
    let err =
        import_lastfm_from(&format!("{}/2.0/", server.base), &cfg(""), &lib, &log()).unwrap_err();
    assert_eq!(err.to_string(), "Last.fm reported no data.");
}

#[test]
fn an_empty_page_is_retried() {
    declare_types();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let server = serve(move |_| {
        let n = calls2.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Reply::text(&xml(&[], 1))
        } else {
            Reply::text(&xml(&[("First", "Artist", "m1", 4)], 1))
        }
    });
    let cfg = cfg(&server.base);
    let lib = Library::in_memory().unwrap();
    add(&lib, "First", "Artist", "Album", "m1");
    let (result, lines) = crate::logging::capture(|| {
        import_lastfm_from(&format!("{}/2.0/", server.base), &cfg, &lib, &log())
    });
    result.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        lines
            .iter()
            .any(|l| l == "lastimport: Retrying page #1... (1/3 retry)"),
        "{lines:?}"
    );
    assert_eq!(
        items(&lib)[0].get("lastfm_play_count"),
        Some(crate::model::Value::Int(4))
    );
}
