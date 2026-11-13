//! `_utils/playcount.py`: matching play-count entries to library items.

use super::*;
use crate::model::{Item, Kind};
use crate::plugins::PluginLog;
use crate::query::Query;

fn log() -> PluginLog {
    PluginLog::new("test")
}

fn add(lib: &Library, title: &str, artist: &str, album: &str, mbid: &str) -> i64 {
    let mut item = Item::new_item();
    item.set_str("title", title);
    item.set_str("artist", artist);
    item.set_str("album", album);
    item.set_path(std::path::PathBuf::from(format!("/lib/{title}.mp3")));
    if !mbid.is_empty() {
        item.set_str("mb_trackid", mbid);
    }
    lib.add(&mut item).unwrap();
    item.id.unwrap()
}

fn titles(items: &[Item]) -> Vec<String> {
    items.iter().map(|i| i.str("title").to_string()).collect()
}

fn all(lib: &Library) -> Vec<Item> {
    lib.items(&Query::parse(Kind::Item, &[] as &[String]).unwrap())
        .unwrap()
}

#[test]
fn mbid_match_wins() {
    let lib = Library::in_memory().unwrap();
    add(&lib, "Song", "A", "Al", "m1");
    add(&lib, "Other", "B", "Bl", "m2");
    let track = Track {
        mbid: Some("m2".into()),
        name: "Song".into(),
        artist: "A".into(),
        album: Some("Al".into()),
        playcount: 5,
    };
    let found = get_items(&lib, &track, &log()).unwrap();
    assert_eq!(titles(&found), ["Other"]);
}

#[test]
fn exact_then_substring_match() {
    let lib = Library::in_memory().unwrap();
    add(&lib, "Song", "The Artist", "Album", "");
    add(&lib, "Song (Live)", "The Artist", "Other Album", "");
    // Exact artist/title (case-insensitively).
    let exact = Track {
        mbid: Some("nope".into()),
        name: "song".into(),
        artist: "the artist".into(),
        album: Some("album".into()),
        playcount: 1,
    };
    assert_eq!(titles(&get_items(&lib, &exact, &log()).unwrap()), ["Song"]);
    // Without an album, both exact titles using this artist match.
    let no_album = Track {
        mbid: None,
        name: "Song".into(),
        artist: "The Artist".into(),
        album: None,
        playcount: 1,
    };
    assert_eq!(
        titles(&get_items(&lib, &no_album, &log()).unwrap()),
        ["Song"]
    );
    // Substring fallback.
    let sub = Track {
        mbid: Some("".into()),
        name: "ong (Li".into(),
        artist: "rtis".into(),
        album: None,
        playcount: 1,
    };
    assert_eq!(
        titles(&get_items(&lib, &sub, &log()).unwrap()),
        ["Song (Live)"]
    );
    // The typographic apostrophe is tried too.
    let typo = Track {
        name: "Don't".into(),
        artist: "A".into(),
        ..Default::default()
    };
    add(&lib, "Don\u{2019}t", "A", "", "");
    assert_eq!(
        titles(&get_items(&lib, &typo, &log()).unwrap()),
        ["Don\u{2019}t"]
    );
    // No match.
    let none = Track {
        name: "Nothing".into(),
        artist: "Nobody".into(),
        ..Default::default()
    };
    assert!(get_items(&lib, &none, &log()).unwrap().is_empty());
}

#[test]
fn process_track_stores_the_count() {
    let lib = Library::in_memory().unwrap();
    crate::model::set_flex_types(
        [
            (
                "lastfm_play_count".to_string(),
                crate::model::FieldType::Int,
            ),
            (
                "listenbrainz_play_count".to_string(),
                crate::model::FieldType::Int,
            ),
        ]
        .into_iter()
        .collect(),
        Default::default(),
    );
    let id = add(&lib, "Song", "A", "Al", "m1");
    let track = Track {
        mbid: Some("m1".into()),
        name: "Song".into(),
        artist: "A".into(),
        album: None,
        playcount: 7,
    };
    assert!(process_track(&lib, &track, &log(), "lastfm").unwrap());
    let items = all(&lib);
    assert_eq!(items[0].id, Some(id));
    assert_eq!(
        items[0].get("lastfm_play_count"),
        Some(crate::model::Value::Int(7))
    );
    assert_eq!(items[0].get("listenbrainz_play_count"), None);
    // The count is replaced, not added.
    process_track(&lib, &track, &log(), "lastfm").unwrap();
    assert_eq!(
        all(&lib)[0].get("lastfm_play_count"),
        Some(crate::model::Value::Int(7))
    );
    assert!(
        !process_track(
            &lib,
            &Track {
                name: "Nope".into(),
                artist: "Nope".into(),
                ..Default::default()
            },
            &log(),
            "lastfm"
        )
        .unwrap()
    );
    assert_eq!(
        update_play_counts(&lib, &[track], &log(), "listenbrainz").unwrap(),
        (1, 0)
    );
    assert_eq!(
        all(&lib)[0].get("listenbrainz_play_count"),
        Some(crate::model::Value::Int(7))
    );
}
