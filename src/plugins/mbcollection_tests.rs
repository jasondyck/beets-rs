//! beets' test/plugins/test_mbcollection.py, against a fake MusicBrainz.

use super::*;
use crate::model::Album;
use crate::plugins::requests::testserver::{Reply, serve};
use serde_json::json;

const COLLECTION_ID: &str = "11111111-2222-3333-4444-555555555555";

fn cfg(server: &str) -> Config {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mbcollection]").unwrap());
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
    cfg.set("musicbrainz.user", "testuser".into());
    cfg.set("musicbrainz.pass", "testpass".into());
    cfg.set("mbcollection.collection", COLLECTION_ID.into());
    cfg
}

fn collections_response(collections: serde_json::Value) -> Reply {
    Reply::json(json!({"collections": collections}))
}

fn album(lib: &Library, mb_albumid: &str) {
    let mut a = Album::new_album();
    a.set_str("album", "Album");
    a.set_str("mb_albumid", mb_albumid);
    lib.add(&mut a).unwrap();
}

#[test]
fn no_collections_is_an_error() {
    let server = serve(|_| collections_response(json!([])));
    let plugin = MusicBrainzCollectionPlugin::new(&cfg(&server.base));
    match plugin.collection() {
        Err(Failure::User(m)) => assert_eq!(m, "no collections exist for user"),
        _ => panic!("expected a UserError"),
    }
}

#[test]
fn no_release_collections_is_an_error() {
    let server = serve(|_| collections_response(json!([{"id": "c1", "entity_type": "event"}])));
    let plugin = MusicBrainzCollectionPlugin::new(&cfg(&server.base));
    match plugin.collection() {
        Err(Failure::User(m)) => assert_eq!(m, "No release collection found."),
        _ => panic!("expected a UserError"),
    }
}

#[test]
fn an_unknown_configured_id_is_an_error() {
    let server = serve(|_| collections_response(json!([{"id": "c1", "entity_type": "release"}])));
    let plugin = MusicBrainzCollectionPlugin::new(&cfg(&server.base));
    match plugin.collection() {
        Err(Failure::User(m)) => {
            assert_eq!(m, format!("invalid collection ID: {COLLECTION_ID}"))
        }
        _ => panic!("expected a UserError"),
    }
}

#[test]
fn a_valid_collection() {
    let server = serve(|_| {
        collections_response(json!([{
            "id": COLLECTION_ID,
            "entity_type": "release",
            "release_count": 3
        }]))
    });
    let plugin = MusicBrainzCollectionPlugin::new(&cfg(&server.base));
    let collection = plugin.collection().ok().expect("a collection");
    assert_eq!(collection.id(), COLLECTION_ID);
    assert_eq!(collection.release_count(), 3);
    assert_eq!(
        collection.releases_url(),
        format!("{}/ws/2/collection/{COLLECTION_ID}/releases", server.base)
    );
}

#[test]
fn mbupdate_fetches_pages_and_batches_submissions() {
    let server = serve(|r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/ws/2/collection?") => collections_response(json!([{
            "id": COLLECTION_ID,
            "entity_type": "release",
            "release_count": 3
        }])),
        ("GET", t) if t.contains("/releases") && t.contains("offset=0") => {
            Reply::json(json!({"releases": [{"id": "in_collection1"}, {"id": "not_in_library"}]}))
        }
        ("GET", t) if t.contains("/releases") && t.contains("offset=2") => {
            Reply::json(json!({"releases": [{"id": "in_collection2"}]}))
        }
        ("PUT", _) | ("DELETE", _) => Reply::json(json!({})),
        _ => Reply::status(404),
    });
    let cfg = cfg(&server.base);
    let lib = Library::in_memory().unwrap();
    for id in [
        "in_collection1",
        "in_collection2",
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-000000000002",
    ] {
        album(&lib, id);
    }
    // Small chunks exercise pagination and batching.
    command_chunked(&cfg, &lib, Some(true), 2, 1).unwrap();

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        6,
        "{:?}",
        requests
            .iter()
            .map(|r| (&r.method, &r.target))
            .collect::<Vec<_>>()
    );
    let puts: Vec<&str> = requests
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| r.target.as_str())
        .collect();
    assert_eq!(puts.len(), 2);
    assert!(puts[0].contains("/releases/00000000-0000-0000-0000-000000000001"));
    assert!(puts[1].contains("/releases/00000000-0000-0000-0000-000000000002"));
    let deletes: Vec<&str> = requests
        .iter()
        .filter(|r| r.method == "DELETE")
        .map(|r| r.target.as_str())
        .collect();
    assert_eq!(deletes.len(), 1);
    assert!(deletes[0].contains("/releases/not_in_library"));
    // Adds come before the removal.
    let first_delete = requests.iter().position(|r| r.method == "DELETE").unwrap();
    let last_put = requests.iter().rposition(|r| r.method == "PUT").unwrap();
    assert!(last_put < first_delete);
}

#[test]
fn mbupdate_without_remove_deletes_nothing() {
    let server = serve(|r| match (r.method.as_str(), r.target.as_str()) {
        ("GET", t) if t.starts_with("/ws/2/collection?") => collections_response(json!([{
            "id": COLLECTION_ID,
            "entity_type": "release",
            "release_count": 0
        }])),
        ("PUT", _) => Reply::json(json!({})),
        _ => Reply::status(404),
    });
    let cfg = cfg(&server.base);
    let lib = Library::in_memory().unwrap();
    album(&lib, "00000000-0000-0000-0000-000000000001");
    command_chunked(&cfg, &lib, None, 2, 1).unwrap();
    let requests = server.requests();
    assert_eq!(requests.iter().filter(|r| r.method == "PUT").count(), 1);
    assert_eq!(requests.iter().filter(|r| r.method == "DELETE").count(), 0);
    // No release pages are fetched.
    assert!(requests.iter().all(|r| !r.target.contains("offset=")));
}

#[test]
fn mbupdate_logs_unauthorized_errors() {
    let server = serve(|_| Reply::status(401));
    let cfg = cfg(&server.base);
    let lib = Library::in_memory().unwrap();
    let (result, lines) = crate::logging::capture(|| command(&cfg, &lib, None));
    result.unwrap();
    assert!(
        lines.iter().any(|l| l
            == "mbcollection: Failed to update MusicBrainz collection: HTTP Error: 401 \
                Unauthorized. Check your musicbrainz.user and musicbrainz.pass configuration"),
        "{lines:?}"
    );
}

#[test]
fn an_invalid_collection_id_fails_the_command() {
    let server = serve(|_| collections_response(json!([{"id": "c1", "entity_type": "release"}])));
    let cfg = cfg(&server.base);
    let lib = Library::in_memory().unwrap();
    let err = command(&cfg, &lib, None).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("invalid collection ID: {COLLECTION_ID}")
    );
}

#[test]
fn uuids_are_lowercase_only() {
    assert!(is_uuid("00000000-0000-0000-0000-000000000001"));
    assert!(!is_uuid("00000000-0000-0000-0000-00000000000Z"));
    assert!(!is_uuid("00000000-0000-0000-0000-00000000001"));
    assert!(!is_uuid("in_collection1"));
    // beets' pattern is lowercase, so uppercase IDs are not added.
    assert!(!is_uuid("00000000-0000-0000-0000-00000000000A"));
}
