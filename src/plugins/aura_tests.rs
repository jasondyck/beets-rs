//! Ported from beets' `test/plugins/test_aura.py`: the JSON:API documents
//! for the collection endpoints (with filters, sorts and includes), plus
//! the single resources, images and audio files.

use super::*;
use crate::model::MULTI;
use crate::plugins::web::Request;
use serde_json::Value as V;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    let lib = Library::in_memory().unwrap();
    Env {
        _dir: dir,
        cfg,
        lib,
    }
}

impl Env {
    /// `helper.add_item_fixture`.
    fn add_item(
        &self,
        album: &str,
        title: &str,
        artist: &str,
        albumartist: &str,
        genres: &[&str],
        track: i64,
    ) -> Record {
        let mut it = Record::new_item();
        it.set_str("album", album);
        it.set_str("title", title);
        it.set_str("artist", artist);
        it.set_str("albumartist", albumartist);
        it.set_list("genres", genres);
        it.set_int("track", track);
        let path = self
            .cfg
            .directory()
            .join("Artist")
            .join(album)
            .join(format!("{track:02} {title}.mp3"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
        it.set_path(&path);
        it
    }

    fn get(&self, url: &str) -> (u16, String, String) {
        self.get_with(url, &[])
    }

    fn get_with(&self, url: &str, headers: &[(&str, &str)]) -> (u16, String, String) {
        let app = App {
            cfg: &self.cfg,
            lib: &self.lib,
        };
        let req = Request {
            method: "GET".into(),
            url: url.into(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            ..Default::default()
        };
        let resp = app.handle(&req);
        let content_type = resp
            .headers
            .iter()
            .find(|(k, _)| k == "Content-Type")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let body = resp.into_bytes();
        (200, content_type, String::from_utf8_lossy(&body).into_owned())
    }

    fn json(&self, url: &str) -> V {
        let (_, content_type, body) = self.get(url);
        assert_eq!(content_type, "application/json");
        serde_json::from_str(&body).unwrap()
    }
}

/// The fixtures: an "Album" with an item, and an "Other Album" with
/// another one (to prove filtering works).
fn fixtures() -> (Env, Record, Record) {
    let e = env();
    let mut item = e.add_item(
        "Album",
        "Title",
        "Artist",
        "Album Artist",
        &["rock", "pop"],
        2,
    );
    e.lib.add(&mut item).unwrap();
    let album = e.lib.add_album(&mut [item.clone()]).unwrap();
    let mut other = e.add_item(
        "Other Album",
        "Other Title",
        "Other Artist",
        "Other Album Artist",
        &[],
        1,
    );
    e.lib.add(&mut other).unwrap();
    e.lib.add_album(&mut [other.clone()]).unwrap();
    (e, item, album)
}

fn track_document(item: &Record, album: &Record) -> V {
    serde_json::json!({
        "type": "track",
        "id": item.id.unwrap().to_string(),
        "attributes": {
            "album": item.str("album"),
            "albumartist": item.str("albumartist"),
            "artist": item.str("artist"),
            "genre": ["rock", "pop"],
            "genres": ["rock", "pop"],
            "size": std::fs::metadata(item.path()).unwrap().len(),
            "title": item.str("title"),
            "track": item.int("track"),
        },
        "relationships": {
            "albums": {"data": [{"id": album.id.unwrap().to_string(), "type": "album"}]},
            "artists": {"data": [{"id": item.str("artist"), "type": "artist"}]},
        },
    })
}

fn artist_document(item: &Record) -> V {
    serde_json::json!({
        "type": "artist",
        "id": item.str("artist"),
        "attributes": {"name": item.str("artist")},
        "relationships": {
            "tracks": {"data": [{"id": item.id.unwrap().to_string(), "type": "track"}]},
        },
    })
}

fn album_document(album: &Record, item: &Record) -> V {
    serde_json::json!({
        "type": "album",
        "id": album.id.unwrap().to_string(),
        "attributes": {
            "artist": album.str("albumartist"),
            "title": album.str("album"),
            "genre": ["rock", "pop"],
            "genres": ["rock", "pop"],
        },
        "relationships": {
            "tracks": {"data": [{"id": item.id.unwrap().to_string(), "type": "track"}]},
        },
    })
}

#[test]
fn tracks() {
    let (e, item, album) = fixtures();
    // Filter by an int field; sort by a field with no value.
    let data = e.json("/aura/tracks?filter%5Btrack%5D=2&include=tracks,artists,albums&sort=mb_trackid");
    assert_eq!(
        data,
        serde_json::json!({
            "data": [track_document(&item, &album)],
            "included": [
                artist_document(&item),
                album_document(&album, &item),
            ],
        })
    );
}

#[test]
fn artists() {
    let (e, item, _album) = fixtures();
    let data = e.json("/aura/artists?filter%5Bartist%5D=Artist&include=tracks,artists,albums");
    assert_eq!(
        data,
        serde_json::json!({
            "data": [artist_document(&item)],
            "included": [track_document(&item, &_album)],
        })
    );
}

#[test]
fn albums() {
    let (e, item, album) = fixtures();
    // Filter by a list field (beets' MULTI_VALUE_DELIMITER).
    let filter = format!("/aura/albums?filter%5Bgenres%5D={}&include=tracks,artists,albums",
        crate::plugins::requests::quote(&format!("rock{MULTI}pop"), ""));
    let data = e.json(&filter);
    assert_eq!(
        data,
        serde_json::json!({
            "data": [album_document(&album, &item)],
            "included": [track_document(&item, &album)],
        })
    );
}

#[test]
fn single_resources() {
    let (e, item, album) = fixtures();
    let id = item.id.unwrap();
    assert_eq!(
        e.json(&format!("/aura/tracks/{id}")),
        serde_json::json!({"data": track_document(&item, &album)})
    );
    let album_id = album.id.unwrap();
    assert_eq!(
        e.json(&format!("/aura/albums/{album_id}")),
        serde_json::json!({"data": album_document(&album, &item)})
    );
    assert_eq!(
        e.json("/aura/artists/Artist"),
        serde_json::json!({"data": artist_document(&item)})
    );
}

#[test]
fn server_info() {
    let (e, _i, _a) = fixtures();
    let data = e.json("/aura/server");
    assert_eq!(data["data"]["type"], "server");
    assert_eq!(data["data"]["id"], "0");
    assert_eq!(data["data"]["attributes"]["server"], "beets-aura");
    assert_eq!(data["data"]["attributes"]["auth-required"], false);
    assert_eq!(
        data["data"]["attributes"]["features"],
        serde_json::json!(["albums", "artists", "images"])
    );
}

#[test]
fn errors_are_json_api_documents() {
    let (e, _i, _a) = fixtures();
    assert_eq!(
        e.json("/aura/tracks/99"),
        serde_json::json!({"errors": [{
            "detail": "There is no track with an id of 99 in the library.",
            "status": "404 Not Found",
            "title": "No track with the requested id.",
        }]})
    );
    assert_eq!(
        e.json("/aura/albums/99")["errors"][0]["status"],
        "404 Not Found"
    );
    assert_eq!(
        e.json("/aura/artists/Nope")["errors"][0]["title"],
        "No artist with the requested id."
    );
    assert_eq!(
        e.json("/aura/images/album-1-cover.jpg")["errors"][0]["title"],
        "No image with the requested id."
    );
}

#[test]
fn unknown_routes_are_html_404s() {
    let (e, _i, _a) = fixtures();
    let (_, content_type, _) = e.get("/aura/nope");
    assert_eq!(content_type, "text/html; charset=utf-8");
    // The blueprint's prefix is required.
    let (_, content_type, _) = e.get("/tracks");
    assert_eq!(content_type, "text/html; charset=utf-8");
    // A non-numeric id doesn't match Flask's int converter.
    let (_, content_type, _) = e.get("/aura/tracks/abc");
    assert_eq!(content_type, "text/html; charset=utf-8");
}

#[test]
fn pagination_and_links() {
    let (e, item, _a) = fixtures();
    let data = e.json("/aura/tracks?page=0&limit=1");
    assert_eq!(
        data["links"]["next"],
        "http://localhost/aura/tracks?page=1&limit=1"
    );
    assert_eq!(data["data"][0]["id"], item.id.unwrap().to_string());
    // The second page still has a next link, as in beets.
    let data = e.json("/aura/tracks?page=1&limit=1");
    assert_eq!(
        data["links"]["next"],
        "http://localhost/aura/tracks?page=2&limit=1"
    );
    let data = e.json("/aura/tracks?page=5&limit=1");
    assert!(data.get("links").is_none());
    assert_eq!(data["data"], serde_json::json!([]));
    // With no arguments the next link is appended with a "?".
    let mut e = e;
    e.cfg.set("aura.page_limit", serde_yaml::Value::Number(1.into()));
    let data = e.json("/aura/tracks");
    assert_eq!(data["links"]["next"], "http://localhost/aura/tracks?page=1");
}

#[test]
fn filter_attributes_are_translated() {
    let (e, item, _a) = fixtures();
    // `genre` is AURA's name for beets' `genres`; the exact match compares
    // the stored (delimited) value, as beets' fast MatchQuery does.
    let data = e.json("/aura/tracks?filter%5Bgenre%5D=rock");
    assert_eq!(data["data"], serde_json::json!([]));
    let data = e.json(&format!(
        "/aura/tracks?filter%5Bgenre%5D={}",
        crate::plugins::requests::quote(&format!("rock{MULTI}pop"), "")
    ));
    assert_eq!(data["data"][0]["id"], item.id.unwrap().to_string());
    // An unknown attribute is used as a (flexible) beets field.
    let data = e.json("/aura/tracks?filter%5Bnope%5D=rock");
    assert_eq!(data["data"], serde_json::json!([]));
    // `filter[` must start the key, as beets' regex requires.
    let data = e.json("/aura/tracks?xfilter%5Btrack%5D=2");
    assert_eq!(data["data"].as_array().unwrap().len(), 2);
}

#[test]
fn sorts_run_slowly_and_case_insensitively() {
    let (e, _i, _a) = fixtures();
    let data = e.json("/aura/tracks?sort=-title");
    assert_eq!(data["data"][0]["attributes"]["title"], "Title");
    let data = e.json("/aura/tracks?sort=title");
    assert_eq!(data["data"][0]["attributes"]["title"], "Other Title");
    let data = e.json("/aura/tracks?sort=%2Ctitle%2C");
    assert_eq!(data["data"][0]["attributes"]["title"], "Other Title");
}

#[test]
fn audio_files() {
    let (e, item, _a) = fixtures();
    let id = item.id.unwrap();
    // No Accept header: nothing is acceptable (as werkzeug has it).
    let (status, content_type, body) = e.get(&format!("/aura/tracks/{id}/audio"));
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/json");
    assert!(body.contains("406 Not Acceptable"));
    // A matching Accept header sends the file.
    let (_, content_type, _) = e.get_with(
        &format!("/aura/tracks/{id}/audio"),
        &[("Accept", "audio/*")],
    );
    assert_eq!(content_type, "audio/mpeg");
    // Parameters in the Accept item don't match.
    let (_, _, body) = e.get_with(
        &format!("/aura/tracks/{id}/audio"),
        &[("Accept", "audio/mpeg; bitrate=999")],
    );
    assert!(body.contains("406 Not Acceptable"));
}

#[test]
fn image_documents_and_files() {
    let (e, _i, album) = fixtures();
    let album_id = album.id.unwrap();
    let art = e.cfg.directory().join("Artist/Album/cover.jpg");
    std::fs::write(&art, b"jpegdata").unwrap();
    let mut album = album;
    album.set("artpath", crate::model::Value::Path(art.clone()));
    e.lib.store(&album).unwrap();
    let id = format!("album-{album_id}-cover.jpg");
    let data = e.json(&format!("/aura/images/{id}"));
    assert_eq!(data["data"]["type"], "image");
    assert_eq!(data["data"]["attributes"]["role"], "cover");
    assert_eq!(data["data"]["attributes"]["mimetype"], "image/jpeg");
    assert_eq!(data["data"]["attributes"]["size"], 8);
    // Not an image Pillow could open either: no width/height.
    assert!(data["data"]["attributes"].get("width").is_none());
    assert!(data["data"]["attributes"].get("height").is_none());
    assert_eq!(
        data["data"]["relationships"]["albums"]["data"][0]["id"],
        album_id.to_string()
    );
    // The album document has the image relationship.
    let data = e.json(&format!("/aura/albums/{album_id}"));
    assert_eq!(
        data["data"]["relationships"]["images"]["data"][0]["id"],
        id.clone()
    );
    let (_, content_type, body) = e.get(&format!("/aura/images/{id}/file"));
    assert_eq!(content_type, "image/jpeg");
    assert_eq!(body, "jpegdata");
    // A traversal attempt is not an image (a name with a slash doesn't
    // match the route: a 404 page; ".." is refused).
    assert!(
        e.json("/aura/images/album-1-..")["errors"][0]["status"] == "404 Not Found"
    );
    let (_, content_type, _) = e.get("/aura/images/album-1-..%2Fcover.jpg");
    assert_eq!(content_type, "text/html; charset=utf-8");
    // A real image gets Pillow's dimensions (from the header).
    let png = e.cfg.directory().join("Artist/Album/cover.png");
    crate::artresizer::tests::write_image(
        &png,
        &crate::artresizer::tests::picture(30, 20, 0),
        0,
    );
    let mut album = e.lib.get_album(album_id).unwrap().unwrap();
    album.set("artpath", crate::model::Value::Path(png));
    e.lib.store(&album).unwrap();
    let data = e.json(&format!("/aura/images/album-{album_id}-cover.png"));
    assert_eq!(data["data"]["attributes"]["width"], 30);
    assert_eq!(data["data"]["attributes"]["height"], 20);
}

#[test]
fn safe_filenames() {
    assert!(safe_filename("cover.jpg"));
    assert!(!safe_filename("a/b.jpg"));
    assert!(!safe_filename(".."));
    assert!(!safe_filename("."));
}
