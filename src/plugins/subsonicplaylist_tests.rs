//! Tests for the Subsonic playlist plugin (beets' plugin has no tests): the
//! REST calls against a fake server and the tag/delete logic.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::query::Query;

fn track(artist: &str, album: &str, title: &str) -> Item {
    let mut i = Item::new_item();
    i.set_str("artist", artist);
    i.set_str("album", album);
    i.set_str("title", title);
    i
}

#[test]
fn filter_to_be_removed_keeps_matching_items() {
    let items = vec![track("A", "Al", "T"), track("B", "Bl", "U")];
    let keys: HashSet<TrackKey> = [("A".into(), "Al".into(), "T".into())].into();
    let gone = filter_to_be_removed(&items, &keys);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].str("title"), "U");
}

fn env(base_url: &str, yaml: &str) -> (Config, Library) {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[subsonicplaylist]").unwrap());
    cfg.set("subsonicplaylist.base_url", base_url.into());
    cfg.set("subsonicplaylist.username", "u".into());
    cfg.set("subsonicplaylist.password", "p".into());
    if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        for (k, v) in m {
            cfg.set(&format!("subsonicplaylist.{}", k.as_str().unwrap()), v);
        }
    }
    let lib = Library::in_memory().unwrap();
    (cfg, lib)
}

fn serve_playlists() -> crate::plugins::requests::testserver::Server {
    crate::plugins::requests::testserver::serve(|seen| {
        use crate::plugins::requests::testserver::Reply;
        if seen.target.contains("getPlaylists") {
            Reply::text(
                "<subsonic-response status=\"ok\"><playlists>\
                 <playlist name=\"Named\" id=\"2\"/></playlists></subsonic-response>",
            )
        } else if seen.target.contains("getPlaylist") {
            let (name, id) = if seen.target.contains("id=2") {
                ("Named", "2")
            } else {
                ("P", "1")
            };
            Reply::text(&format!(
                "<subsonic-response status=\"ok\"><playlist name=\"{name}\" id=\"{id}\">\
                 <entry artist=\"A\" album=\"Al\" title=\"T\"/>\
                 </playlist></subsonic-response>"
            ))
        } else {
            Reply::status(404)
        }
    })
}

fn all(lib: &Library) -> Vec<Item> {
    lib.items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
        .unwrap()
}

#[test]
fn tags_items_from_playlists() {
    let server = serve_playlists();
    let (cfg, lib) = env(&server.base, "playlist_ids: ['1']\nplaylist_names: ['Named']");
    let mut item = track("A", "Al", "T");
    lib.add(&mut item).unwrap();
    command(&cfg, &lib, false).unwrap();
    let got = all(&lib);
    assert_eq!(got[0].str("subsonic_playlist"), ";P;Named;");
    // The request had the Subsonic auth parameters (u/t/s/v/c).
    let seen = server.requests();
    assert!(!seen.is_empty());
    assert!(seen[0].target.contains("u=u"), "{}", seen[0].target);
    assert!(seen[0].target.contains("t="), "{}", seen[0].target);
    assert!(seen[0].target.contains("s="), "{}", seen[0].target);
    assert!(seen[0].target.contains("c=beets"), "{}", seen[0].target);
}

#[test]
fn delete_clears_tags_not_in_any_playlist() {
    let server = serve_playlists();
    let (cfg, lib) = env(&server.base, "playlist_ids: ['1']");
    let mut item = track("A", "Al", "T");
    item.set_str("subsonic_playlist", ";Old;");
    lib.add(&mut item).unwrap();
    let mut other = track("B", "Bl", "U");
    other.set_str("subsonic_playlist", ";Old;");
    lib.add(&mut other).unwrap();
    command(&cfg, &lib, true).unwrap();
    let got = all(&lib);
    let tagged: Vec<&str> = got.iter().map(|i| i.str("subsonic_playlist")).collect();
    assert!(tagged.contains(&";P;"), "{tagged:?}");
    assert!(tagged.contains(&""), "{tagged:?}");
}

#[test]
fn undiscovered_tracks_are_logged() {
    let server = serve_playlists();
    let (cfg, lib) = env(&server.base, "playlist_ids: ['1']");
    let (_r, lines) = crate::logging::capture(|| command(&cfg, &lib, false).unwrap());
    assert!(
        lines.iter().any(|l| l.contains("track not found")),
        "{lines:?}"
    );
}
