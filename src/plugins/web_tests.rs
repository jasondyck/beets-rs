//! beets' `test/plugins/test_web.py` (driving `App::handle` as beets' tests
//! drive Flask's test client), plus checks of the JSON layout, headers,
//! CORS, the reverse proxy, ranges, and the HTTP server on 127.0.0.1:0.

use super::*;
use serde_json::{Value as J, json};
use serde_yaml::Value as Y;
use std::path::PathBuf;
use tempfile::TempDir;

/// beets' `_common.RSRC / "full.mp3"` (title "full").
fn rsrc_full_mp3() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/web/full.mp3")
}

struct Env {
    dir: TempDir,
    cfg: Config,
    lib: Library,
    settings: Settings,
    path1: PathBuf,
    path4: PathBuf,
}

impl Env {
    /// `WebPluginMixin.setup_web_app`: items #1–#3 and albums #1–#2.
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str("[web]").unwrap());
        crate::query::configure(&cfg);
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        let item = |title: &str, path: &str, album_id: i64, flex: &[(&str, &str)]| {
            let mut i = Item::new_item();
            i.set_str("title", title);
            i.set_path(path);
            i.set_int("album_id", album_id);
            for (k, v) in flex {
                i.set_str(k, *v);
            }
            i
        };
        let path1 = PathBuf::from("/path_1");
        let mut i1 = item("title", "/path_1", 2, &[("artist", "AAA Singers")]);
        lib.add(&mut i1).unwrap();
        let mut i2 = item(
            "another title",
            "/somewhere/a",
            0,
            &[("artist", "AAA Singers")],
        );
        lib.add(&mut i2).unwrap();
        let mut i3 = item("and a third", "/somewhere/abc", 2, &[("testattr", "ABC")]);
        lib.add(&mut i3).unwrap();
        let mut a1 = Album::new_album();
        a1.set_str("album", "album");
        a1.set_str("albumtest", "xyz");
        lib.add(&mut a1).unwrap();
        let path4 = PathBuf::from("/somewhere2/art_path_2");
        let mut a2 = Album::new_album();
        a2.set_str("album", "other album");
        a2.set("artpath", Value::Path(path4.clone()));
        lib.add(&mut a2).unwrap();
        // beets' `Album.store` passes the album's fields on to its items.
        for mut i in [i1, i3] {
            i.set_str("album", "other album");
            lib.store(&i).unwrap();
        }
        Env {
            dir,
            cfg,
            lib,
            settings: Settings::default(),
            path1,
            path4,
        }
    }

    fn app(&self) -> App<'_> {
        App {
            cfg: &self.cfg,
            lib: &self.lib,
            settings: self.settings.clone(),
        }
    }

    fn request(&self, req: &Request) -> Response {
        self.app().handle(req)
    }

    fn get(&self, url: &str) -> Response {
        self.request(&Request::new("GET", url))
    }

    fn delete(&self, url: &str) -> Response {
        self.request(&Request::new("DELETE", url))
    }

    fn patch(&self, url: &str, body: &J) -> Response {
        let mut r = Request::new("PATCH", url);
        r.headers
            .push(("Content-Type".into(), "application/json".into()));
        r.body = body.to_string().into_bytes();
        self.request(&r)
    }

    /// The status and the JSON body.
    fn get_json(&self, url: &str) -> (u16, J) {
        json_of(self.get(url))
    }

    fn add(&self, mut rec: Record) -> i64 {
        self.lib.add(&mut rec).unwrap()
    }

    fn item(&self, title: &str, flex: &[(&str, &str)]) -> i64 {
        let mut i = Item::new_item();
        i.set_str("title", title);
        for (k, v) in flex {
            i.set_str(k, *v);
        }
        self.add(i)
    }

    fn album(&self, name: &str, flex: &[(&str, &str)]) -> i64 {
        let mut a = Album::new_album();
        a.set_str("album", name);
        for (k, v) in flex {
            a.set_str(k, *v);
        }
        self.add(a)
    }

    /// `Item.from_path` of a copy of full.mp3 at `name` in the temp dir.
    fn mp3_item(&self, name: &str) -> (PathBuf, i64) {
        let ipath = self.dir.path().join(name);
        std::fs::copy(rsrc_full_mp3(), &ipath).unwrap();
        let item = crate::media::read(&ipath).unwrap();
        (ipath, self.add(item))
    }
}

fn json_of(r: Response) -> (u16, J) {
    let status = r.status;
    let body = r.into_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(J::Null))
}

fn titles(v: &J) -> Vec<String> {
    let mut t: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect();
    t.sort();
    t
}

fn albums_of(v: &J) -> Vec<String> {
    let mut t: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["album"].as_str().unwrap().to_string())
        .collect();
    t.sort();
    t
}

// ------------------------------------------------------- TestWebPlugin

#[test]
fn config_include_paths_true() {
    let mut env = Env::new();
    env.settings.include_paths = true;
    let (status, j) = env.get_json("/item/1");
    assert_eq!(status, 200);
    assert_eq!(j["path"], json!(env.path1.display().to_string()));
}

#[test]
fn config_include_artpaths_true() {
    let mut env = Env::new();
    env.settings.include_paths = true;
    let (status, j) = env.get_json("/album/2");
    assert_eq!(status, 200);
    assert_eq!(j["artpath"], json!(env.path4.display().to_string()));
}

#[test]
fn config_include_paths_false() {
    let env = Env::new();
    let (status, j) = env.get_json("/item/1");
    assert_eq!(status, 200);
    assert!(j.get("path").is_none());
}

#[test]
fn config_include_artpaths_false() {
    let env = Env::new();
    let (status, j) = env.get_json("/album/2");
    assert_eq!(status, 200);
    assert!(j.get("artpath").is_none());
}

#[test]
fn get_all_items() {
    let (status, j) = Env::new().get_json("/item/");
    assert_eq!(status, 200);
    assert_eq!(j["items"].as_array().unwrap().len(), 3);
}

#[test]
fn get_unique_item_artist() {
    let (status, j) = Env::new().get_json("/item/values/artist");
    assert_eq!(status, 200);
    assert_eq!(j["values"], json!(["", "AAA Singers"]));
}

#[test]
fn get_single_item_by_id() {
    let (status, j) = Env::new().get_json("/item/1");
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(1));
    assert_eq!(j["title"], json!("title"));
}

#[test]
fn get_multiple_items_by_id() {
    let (status, j) = Env::new().get_json("/item/1,2");
    assert_eq!(status, 200);
    assert_eq!(j["items"].as_array().unwrap().len(), 2);
    assert_eq!(titles(&j["items"]), ["another title", "title"]);
}

#[test]
fn get_single_item_not_found() {
    assert_eq!(Env::new().get("/item/4").status, 404);
}

#[test]
fn get_single_item_by_path() {
    let env = Env::new();
    let data_path = rsrc_full_mp3();
    env.add(crate::media::read(&data_path).unwrap());
    let (status, j) = env.get_json(&format!("/item/path/{}", data_path.display()));
    assert_eq!(status, 200);
    assert_eq!(j["title"], json!("full"));
}

#[test]
fn get_single_item_by_path_not_found_if_not_in_library() {
    let env = Env::new();
    // A valid file, but not in the library.
    let r = env.get(&format!("/item/path/{}", rsrc_full_mp3().display()));
    assert_eq!(r.status, 404);
}

#[test]
fn get_item_empty_query() {
    let (status, j) = Env::new().get_json("/item/query/");
    assert_eq!(status, 200);
    assert_eq!(j["items"].as_array().unwrap().len(), 3);
}

#[test]
fn get_simple_item_query() {
    let (status, j) = Env::new().get_json("/item/query/another");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["title"], json!("another title"));
}

#[test]
fn query_item_string() {
    // testattr:ABC
    let (status, j) = Env::new().get_json("/item/query/testattr%3aABC");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["title"], json!("and a third"));
}

#[test]
fn query_item_regex() {
    // testattr::[A-C]+
    let (status, j) = Env::new().get_json("/item/query/testattr%3a%3a[A-C]%2b");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["title"], json!("and a third"));
}

#[test]
fn query_item_regex_backslash() {
    // testattr::\w+
    let (status, j) = Env::new().get_json("/item/query/testattr%3a%3a%5cw%2b");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["title"], json!("and a third"));
}

#[test]
fn query_item_path() {
    // Path queries match from the root to a directory or file, so this is
    // one item; separators in the query are `\`.
    let (status, j) = Env::new().get_json("/item/query/path:\\somewhere\\a");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["title"], json!("another title"));
}

#[test]
fn get_all_albums() {
    let (status, j) = Env::new().get_json("/album/");
    assert_eq!(status, 200);
    assert_eq!(albums_of(&j["albums"]), ["album", "other album"]);
}

#[test]
fn get_single_album_by_id() {
    let (status, j) = Env::new().get_json("/album/2");
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(2));
    assert_eq!(j["album"], json!("other album"));
}

#[test]
fn get_multiple_albums_by_id() {
    let (status, j) = Env::new().get_json("/album/1,2");
    assert_eq!(status, 200);
    assert_eq!(albums_of(&j["albums"]), ["album", "other album"]);
}

#[test]
fn get_album_empty_query() {
    let (status, j) = Env::new().get_json("/album/query/");
    assert_eq!(status, 200);
    assert_eq!(j["albums"].as_array().unwrap().len(), 2);
}

#[test]
fn get_simple_album_query() {
    let (status, j) = Env::new().get_json("/album/query/other");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["album"], json!("other album"));
    assert_eq!(j["results"][0]["id"], json!(2));
}

#[test]
fn get_album_details() {
    let (status, j) = Env::new().get_json("/album/2?expand");
    assert_eq!(status, 200);
    let items = j["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["album"], json!("other album"));
    assert_eq!(items[1]["album"], json!("other album"));
    assert_eq!(titles(&j["items"]), ["and a third", "title"]);
}

#[test]
fn query_album_string() {
    // albumtest:xy
    let (status, j) = Env::new().get_json("/album/query/albumtest%3axy");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["album"], json!("album"));
}

#[test]
fn query_album_artpath_regex() {
    // artpath::art_
    let (status, j) = Env::new().get_json("/album/query/artpath%3a%3aart_");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["album"], json!("other album"));
}

#[test]
fn query_album_regex_backslash() {
    // albumtest::\w+
    let (status, j) = Env::new().get_json("/album/query/albumtest%3a%3a%5cw%2b");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(j["results"][0]["album"], json!("album"));
}

#[test]
fn get_stats() {
    let (status, j) = Env::new().get_json("/stats");
    assert_eq!(status, 200);
    assert_eq!(j["items"], json!(3));
    assert_eq!(j["albums"], json!(2));
}

#[test]
fn delete_item_id() {
    let mut env = Env::new();
    env.settings.readonly = false;
    let id = env.item("test_delete_item_id", &[("test_delete_item_id", "1")]);
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(env.delete(&format!("/item/{id}")).status, 200);
    assert_eq!(env.get(&format!("/item/{id}")).status, 404);
}

#[test]
fn delete_item_without_file() {
    let mut env = Env::new();
    env.settings.readonly = false;
    let (ipath, id) = env.mp3_item("testfile1.mp3");
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(env.delete(&format!("/item/{id}")).status, 200);
    assert_eq!(env.get(&format!("/item/{id}")).status, 404);
    assert!(ipath.exists(), "the file stays");
}

#[test]
fn delete_item_with_file() {
    let mut env = Env::new();
    env.settings.readonly = false;
    let (ipath, id) = env.mp3_item("testfile2.mp3");
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    let (status, j) = json_of(env.delete(&format!("/item/{id}?delete")));
    assert_eq!(status, 200);
    assert_eq!(j, json!({"deleted": true}));
    assert_eq!(env.get(&format!("/item/{id}")).status, 404);
    assert!(!ipath.exists(), "the file is gone");
}

#[test]
fn delete_item_query() {
    let mut env = Env::new();
    env.settings.readonly = false;
    env.item("test_delete_item_query", &[("test_delete_item_query", "1")]);
    let (status, j) = env.get_json("/item/query/test_delete_item_query");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(env.delete("/item/query/test_delete_item_query").status, 200);
    let (status, j) = env.get_json("/item/query/test_delete_item_query");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 0);
}

#[test]
fn delete_item_all_fails() {
    // DELETE isn't a method of the list of all items.
    let mut env = Env::new();
    env.settings.readonly = false;
    assert_eq!(env.delete("/item/").status, 405);
    assert_eq!(env.lib.counts().unwrap().0, 3);
}

#[test]
fn delete_item_id_readonly() {
    let env = Env::new();
    let id = env.item("test_delete_item_id_ro", &[("test_delete_item_id_ro", "1")]);
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(env.delete(&format!("/item/{id}")).status, 405);
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
}

#[test]
fn delete_item_query_readonly() {
    let env = Env::new();
    env.item("test_delete_item_q_ro", &[("test_delete_item_q_ro", "1")]);
    let (status, j) = env.get_json("/item/query/test_delete_item_q_ro");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(env.delete("/item/query/test_delete_item_q_ro").status, 405);
    let (status, j) = env.get_json("/item/query/test_delete_item_q_ro");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
}

#[test]
fn delete_album_id() {
    let mut env = Env::new();
    env.settings.readonly = false;
    let id = env.album("test_delete_album_id", &[("test_delete_album_id", "1")]);
    let (status, j) = env.get_json(&format!("/album/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(env.delete(&format!("/album/{id}")).status, 200);
    assert_eq!(env.get(&format!("/album/{id}")).status, 404);
}

#[test]
fn delete_album_query() {
    let mut env = Env::new();
    env.settings.readonly = false;
    env.album(
        "test_delete_album_query",
        &[("test_delete_album_query", "1")],
    );
    let (status, j) = env.get_json("/album/query/test_delete_album_query");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        env.delete("/album/query/test_delete_album_query").status,
        200
    );
    let (status, j) = env.get_json("/album/query/test_delete_album_query");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 0);
}

#[test]
fn delete_album_all_fails() {
    let mut env = Env::new();
    env.settings.readonly = false;
    assert_eq!(env.delete("/album/").status, 405);
    assert_eq!(env.lib.counts().unwrap().1, 2);
}

#[test]
fn delete_album_id_readonly() {
    let env = Env::new();
    let id = env.album(
        "test_delete_album_id_ro",
        &[("test_delete_album_id_ro", "1")],
    );
    let (status, j) = env.get_json(&format!("/album/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(env.delete(&format!("/album/{id}")).status, 405);
    let (status, j) = env.get_json(&format!("/album/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
}

#[test]
fn delete_album_query_readonly() {
    let env = Env::new();
    env.album(
        "test_delete_album_query_ro",
        &[("test_delete_album_query_ro", "1")],
    );
    let (status, j) = env.get_json("/album/query/test_delete_album_query_ro");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        env.delete("/album/query/test_delete_album_query_ro").status,
        405
    );
    let (status, j) = env.get_json("/album/query/test_delete_album_query_ro");
    assert_eq!(status, 200);
    assert_eq!(j["results"].as_array().unwrap().len(), 1);
}

#[test]
fn patch_item_id() {
    // PATCH is only implemented for items, not albums.
    let mut env = Env::new();
    env.settings.readonly = false;
    let id = env.item(
        "test_patch_item_id",
        &[("test_patch_f1", "1"), ("test_patch_f2", "Old")],
    );
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(j["test_patch_f1"], json!("1"));
    assert_eq!(j["test_patch_f2"], json!("Old"));

    let (status, j) = json_of(env.patch(&format!("/item/{id}"), &json!({"test_patch_f2": "New"})));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(j["test_patch_f1"], json!("1"));
    assert_eq!(j["test_patch_f2"], json!("New"));

    // The update really happened.
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(j["test_patch_f1"], json!("1"));
    assert_eq!(j["test_patch_f2"], json!("New"));
}

#[test]
fn patch_item_id_readonly() {
    let env = Env::new();
    let id = env.item(
        "test_patch_item_id_ro",
        &[("test_patch_f1", "2"), ("test_patch_f2", "Old")],
    );
    let (status, j) = env.get_json(&format!("/item/{id}"));
    assert_eq!(status, 200);
    assert_eq!(j["id"], json!(id));
    assert_eq!(j["test_patch_f1"], json!("2"));
    assert_eq!(j["test_patch_f2"], json!("Old"));
    let r = env.patch(&format!("/item/{id}"), &json!({"test_patch_f2": "New"}));
    assert_eq!(r.status, 405);
}

#[test]
fn get_item_file() {
    let env = Env::new();
    let (_, id) = env.mp3_item("testfile2.mp3");
    let r = env.get(&format!("/item/{id}/file"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("audio/mpeg"));
    assert_eq!(
        r.header("Content-Disposition"),
        Some("attachment; filename=testfile2.mp3")
    );
    assert_eq!(r.into_bytes(), std::fs::read(rsrc_full_mp3()).unwrap());
}

#[test]
fn file_etag_and_conditional_get() {
    let env = Env::new();
    let (_, id) = env.mp3_item("testfile2.mp3");
    let url = format!("/item/{id}/file");
    let r = env.get(&url);
    let etag = r.header("ETag").unwrap().to_string();
    let last_modified = r.header("Last-Modified").unwrap().to_string();
    // Werkzeug's `set_etag`: a quoted `"{mtime}-{size}-{adler32}"`.
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert_eq!(etag.matches('-').count(), 2, "{etag}");
    let conditional = |headers: &[(&str, &str)]| {
        let mut q = Request::new("GET", &url);
        for (k, v) in headers {
            q.headers.push(((*k).into(), (*v).into()));
        }
        env.request(&q)
    };
    let r = conditional(&[("If-None-Match", etag.as_str())]);
    assert_eq!(r.status, 304);
    assert!(r.into_bytes().is_empty());
    // Weak comparison, lists and `*`.
    assert_eq!(
        conditional(&[("If-None-Match", &format!("W/{etag}"))]).status,
        304
    );
    assert_eq!(
        conditional(&[("If-None-Match", &format!("\"x\", {etag}"))]).status,
        304
    );
    assert_eq!(conditional(&[("If-None-Match", "*")]).status, 304);
    assert_eq!(conditional(&[("If-None-Match", "\"other\"")]).status, 200);
    // `If-Modified-Since`, when no `If-None-Match` decides it.
    assert_eq!(
        conditional(&[("If-Modified-Since", last_modified.as_str())]).status,
        304
    );
    assert_eq!(
        conditional(&[("If-Modified-Since", "Thu, 01 Jan 1970 00:00:00 GMT")]).status,
        200
    );
    // A range request is answered as one, as Werkzeug does.
    assert_eq!(
        conditional(&[("If-None-Match", etag.as_str()), ("Range", "bytes=0-1")]).status,
        206
    );
}

#[test]
fn adler32_matches_zlib() {
    assert_eq!(adler32(b""), 1);
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
}

#[test]
fn item_file_relative_path_outside_main_context() {
    // A file inside the library directory (stored relative to it).
    let env = Env::new();
    let path = env.cfg.directory().join("x.mp3");
    std::fs::write(&path, b"data").unwrap();
    let mut i = Item::new_item();
    i.set_path(&path);
    let id = env.add(i);
    let r = env.get(&format!("/item/{id}/file"));
    assert_eq!(r.status, 200);
    assert_eq!(r.into_bytes(), b"data");
}

#[test]
fn album_art_relative_path_outside_main_context() {
    let env = Env::new();
    let path = env.cfg.directory().join("cover.jpg");
    std::fs::write(&path, b"art").unwrap();
    let mut a = Album::new_album();
    a.set_str("album", "x");
    a.set("artpath", Value::Path(path));
    let id = env.add(a);
    let r = env.get(&format!("/album/{id}/art"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("image/jpeg"));
    assert_eq!(
        r.header("Content-Disposition"),
        Some("inline; filename=cover.jpg")
    );
    assert_eq!(r.into_bytes(), b"art");
}

// ---------------------------------------------------------- TestWebXSS

/// The Underscore.js templates in index.html escape user data (`<%- %>`)
/// rather than interpolating it raw (`<%= %>`; GHSA-3gxm-wfjx-m847).
#[test]
fn templates_use_escaping_syntax() {
    let env = Env::new();
    let html = String::from_utf8(env.get("/").into_bytes()).unwrap();
    let templates: Vec<String> =
        regex::Regex::new(r#"(?s)<script type="text/template"[^>]*>(.*?)</script>"#)
            .unwrap()
            .captures_iter(&html)
            .map(|c| c[1].to_string())
            .collect();
    let all = templates.join("\n");
    let user_data_fields = [
        "title",
        "artist",
        "album",
        "year",
        "track",
        "tracktotal",
        "disc",
        "disctotal",
        "length",
        "format",
        "bitrate",
        "mb_trackid",
        "id",
        "lyrics",
        "comments",
    ];
    let fields = |re: &str| -> Vec<String> {
        regex::Regex::new(re)
            .unwrap()
            .captures_iter(&all)
            .map(|c| c[1].to_string())
            .filter(|f| user_data_fields.contains(&f.as_str()))
            .collect()
    };
    let vulnerable = fields(r"<%=\s*(\w+)\s*%>");
    assert!(vulnerable.is_empty(), "non-escaping tags: {vulnerable:?}");
    assert!(!fields(r"<%-\s*(\w+)\s*%>").is_empty());
}

// ------------------------------------------------------ beyond beets' tests

#[test]
fn index_links_static_files_and_they_are_served() {
    let env = Env::new();
    let r = env.get("/");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("text/html; charset=utf-8"));
    let html = String::from_utf8(r.into_bytes()).unwrap();
    assert!(html.contains(r#"<script src="/static/beets.js"></script>"#));
    assert!(html.contains(r#"href="/static/beets.css""#));
    assert!(!html.contains("{{"));
    assert!(html.ends_with("</html>"), "Jinja drops the final newline");
    for (name, ct) in [
        ("beets.js", "text/javascript; charset=utf-8"),
        ("jquery.js", "text/javascript; charset=utf-8"),
        ("beets.css", "text/css; charset=utf-8"),
    ] {
        let r = env.get(&format!("/static/{name}"));
        assert_eq!(r.status, 200, "{name}");
        assert_eq!(r.header("Content-Type"), Some(ct));
    }
    assert_eq!(env.get("/static/nope.js").status, 404);
    assert_eq!(env.get("/static/").status, 404);
}

#[test]
fn json_layout_matches_flask_and_json_dumps() {
    let env = Env::new();
    // `jsonify`: compact, sorted keys, a final newline.
    let body = String::from_utf8(env.get("/stats").into_bytes()).unwrap();
    assert_eq!(body, "{\"albums\":2,\"items\":3}\n");
    let body = String::from_utf8(env.get("/item/values/title?sort_key=id").into_bytes()).unwrap();
    assert_eq!(
        body,
        "{\"values\":[\"title\",\"another title\",\"and a third\"]}\n"
    );
    let body = String::from_utf8(env.get("/artist/").into_bytes()).unwrap();
    assert_eq!(body, "{\"artist_names\":[\"\"]}\n");
    // `json_generator`: `json.dumps` defaults inside, no newline.
    let body = String::from_utf8(env.get("/album/query/albumtest:xyz").into_bytes()).unwrap();
    assert!(body.starts_with("{\"results\":[{\"added\": "), "{body}");
    assert!(body.contains(", \"albumtest\": \"xyz\", "), "{body}");
    assert!(body.ends_with("}]}"), "{body}");
    // Debug mode indents `jsonify`'s output.
    let mut env = env;
    env.settings.debug = true;
    let body = String::from_utf8(env.get("/stats").into_bytes()).unwrap();
    assert_eq!(body, "{\n  \"albums\": 2,\n  \"items\": 3\n}\n");
}

#[test]
fn item_json_has_typed_values_album_fields_and_size() {
    let env = Env::new();
    let (_, j) = env.get_json("/item/1");
    // Fixed fields with their Python types.
    assert_eq!(j["album_id"], json!(2));
    assert_eq!(j["track"], json!(0));
    assert_eq!(j["comp"], json!(false));
    assert_eq!(j["length"], json!(0.0));
    assert_eq!(j["genres"], json!([]));
    assert_eq!(j["rg_track_gain"], J::Null);
    assert_eq!(j["initial_key"], J::Null);
    // The album's fields the item lacks (beets' `Item.keys()`), its art
    // path as base64 bytes, as `_rep` leaves it.
    assert_eq!(j["artpath"], json!("L3NvbWV3aGVyZTIvYXJ0X3BhdGhfMg=="));
    assert_eq!(j["size"], json!(0));
    // A singleton: no album, so a null `album_id` and no album fields.
    let (_, j) = env.get_json("/item/2");
    assert_eq!(j["album_id"], J::Null);
    assert!(j.get("artpath").is_none());
    // An album's flexible attributes reach its items.
    let (_, j) = env.get_json("/album/1");
    assert_eq!(j["albumtest"], json!("xyz"));
    let (_, j) = env.get_json("/item/query/testattr:ABC");
    assert_eq!(j["results"][0]["size"], json!(0));
    // `?expand` is for a single album only.
    let (_, j) = env.get_json("/album/1,2?expand");
    assert!(j["albums"][1].get("items").is_none());
}

#[test]
fn include_paths_shows_unset_art_as_none() {
    let mut env = Env::new();
    env.settings.include_paths = true;
    let (_, j) = env.get_json("/album/1");
    assert_eq!(j["artpath"], json!("None"));
}

#[test]
fn non_ascii_is_escaped_as_python_does() {
    let mut j = String::new();
    py_json_str(&mut j, "é\u{1F600}\"\\\n\u{1}");
    assert_eq!(j, "\"\\u00e9\\ud83d\\ude00\\\"\\\\\\n\\u0001\"");
    assert_eq!(Json::Float(1e16).dumps(Layout::Default), "1e+16");
    assert_eq!(Json::Float(2.5).dumps(Layout::Default), "2.5");
    assert_eq!(
        Json::List(vec![]).dumps(Layout::Indent),
        "[]",
        "empty containers stay on one line"
    );
    assert_eq!(b64encode(b"ab"), "YWI=");
    assert_eq!(b64encode(b"abc"), "YWJj");
    assert_eq!(b64encode(b"a"), "YQ==");
}

#[test]
fn unique_values_of_unknown_and_computed_fields() {
    let env = Env::new();
    assert_eq!(env.get("/item/values/nope").status, 404);
    assert_eq!(env.get("/item/values/title?sort_key=nope").status, 404);
    // A computed field has no column: an SQL error in beets.
    assert_eq!(env.get("/item/values/singleton").status, 500);
    // Paths are bytes, which `jsonify` can't serialize.
    assert_eq!(env.get("/item/values/path").status, 500);
    let (status, j) = env.get_json("/album/values/album");
    assert_eq!(status, 200);
    assert_eq!(j["values"], json!(["album", "other album"]));
}

#[test]
fn routing_methods_redirects_and_errors() {
    let env = Env::new();
    // Flask's automatic OPTIONS.
    let r = env.request(&Request::new("OPTIONS", "/item/1"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Allow"), Some("DELETE, GET, HEAD, OPTIONS, PATCH"));
    // A method the route doesn't have.
    let r = env.delete("/item/");
    assert_eq!(r.status, 405);
    assert_eq!(r.header("Allow"), Some("GET, HEAD, OPTIONS"));
    let r = env.patch("/album/1", &json!({}));
    assert_eq!(r.status, 405);
    assert_eq!(r.header("Allow"), Some("DELETE, GET, HEAD, OPTIONS"));
    // `strict_slashes`
    let mut req = Request::new("GET", "/item?expand");
    req.headers.push(("Host".into(), "h:1".into()));
    let r = env.request(&req);
    assert_eq!(r.status, 308);
    assert_eq!(r.header("Location"), Some("http://h:1/item/?expand"));
    assert_eq!(env.get("/album/query").status, 308);
    // No such things.
    assert_eq!(env.get("/nope").status, 404);
    assert_eq!(env.get("/stats/").status, 404);
    assert_eq!(env.get("/item/abc").status, 404);
    assert_eq!(env.get("/item/x/file").status, 404);
    assert_eq!(env.get("/item/99/file").status, 404);
    assert_eq!(env.get("/album/1/art").status, 404, "no art");
    // A missing file is an error (as `send_file`'s).
    assert_eq!(env.get("/item/1/file").status, 500);
    let body = String::from_utf8(env.get("/item/99/file").into_bytes()).unwrap();
    assert_eq!(
        body,
        "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n\
         <p>Item with id 99 not found</p>\n"
    );
    // HEAD is GET (the server leaves the body out).
    assert_eq!(env.request(&Request::new("HEAD", "/stats")).status, 200);
}

#[test]
fn patch_needs_json_and_writes_fixed_fields() {
    let mut env = Env::new();
    env.settings.readonly = false;
    let mut r = Request::new("PATCH", "/item/2");
    r.body = b"{}".to_vec();
    assert_eq!(env.request(&r).status, 415);
    let mut r = Request::new("PATCH", "/item/2");
    r.headers
        .push(("Content-Type".into(), "application/json".into()));
    r.body = b"{nope".to_vec();
    assert_eq!(env.request(&r).status, 400);
    // No such item: nothing to patch.
    assert_eq!(env.patch("/item/99", &json!({"title": "x"})).status, 405);
    // Typed fields take the JSON's values; several items give a list.
    let (status, j) = json_of(env.patch(
        "/item/1,2",
        &json!({"year": 1999, "genres": ["Rock", "Pop"], "comp": true}),
    ));
    assert_eq!(status, 200);
    assert_eq!(j["items"][1]["year"], json!(1999));
    let (_, j) = env.get_json("/item/2");
    assert_eq!(j["year"], json!(1999));
    assert_eq!(j["genres"], json!(["Rock", "Pop"]));
    assert_eq!(j["comp"], json!(true));
    // By query: the patched items, under "items".
    let (status, j) = json_of(env.patch("/item/query/title:another", &json!({"bpm": "120"})));
    assert_eq!(status, 200);
    assert_eq!(j["items"][0]["bpm"], json!(120));
}

#[test]
fn patch_writes_tags_to_the_file() {
    let mut env = Env::new();
    env.settings.readonly = false;
    let (ipath, id) = env.mp3_item("w.mp3");
    let r = env.patch(&format!("/item/{id}"), &json!({"title": "patched"}));
    assert_eq!(r.status, 200);
    assert_eq!(crate::media::read(&ipath).unwrap().str("title"), "patched");
}

#[test]
fn deleting_an_album_removes_its_items() {
    let mut env = Env::new();
    env.settings.readonly = false;
    assert_eq!(env.delete("/album/2").status, 200);
    assert_eq!(env.lib.counts().unwrap(), (1, 1));
}

#[test]
fn file_names_and_ranges() {
    let env = Env::new();
    let dir = env.cfg.directory();
    let add = |name: &str| {
        let p = dir.join(name);
        std::fs::write(&p, b"0123456789").unwrap();
        let mut i = Item::new_item();
        i.set_path(&p);
        env.add(i)
    };
    let latin = add("café.mp3");
    let other = add("Привет \"x\".flac");
    let plain = add("a b.ogg");
    let disp = |id: i64| {
        env.get(&format!("/item/{id}/file"))
            .header("Content-Disposition")
            .unwrap()
            .to_string()
    };
    assert_eq!(
        disp(latin),
        "attachment; filename=cafe.mp3; filename*=UTF-8''caf%C3%A9.mp3"
    );
    assert_eq!(disp(other), r#"attachment; filename="Privet \"x\".flac""#);
    assert_eq!(disp(plain), r#"attachment; filename="a b.ogg""#);
    let range = |h: &str| {
        let mut r = Request::new("GET", &format!("/item/{plain}/file"));
        r.headers.push(("Range".into(), h.into()));
        let resp = env.request(&r);
        let cr = resp.header("Content-Range").map(String::from);
        (resp.status, cr, resp.into_bytes())
    };
    assert_eq!(
        range("bytes=2-4"),
        (206, Some("bytes 2-4/10".into()), b"234".to_vec())
    );
    assert_eq!(
        range("bytes=7-"),
        (206, Some("bytes 7-9/10".into()), b"789".to_vec())
    );
    assert_eq!(
        range("bytes=-2"),
        (206, Some("bytes 8-9/10".into()), b"89".to_vec())
    );
    assert_eq!(range("bytes=20-").0, 416);
    assert_eq!(range("items=1-2").0, 200);
}

fn cors_req(method: &str, origin: Option<&str>) -> Request {
    let mut r = Request::new(method, "/stats");
    if let Some(o) = origin {
        r.headers.push(("Origin".into(), o.into()));
    }
    r
}

#[test]
fn cors_headers_as_flask_cors() {
    let mut env = Env::new();
    env.settings.cors = Some(Cors {
        origin: "http://example.com".into(),
        supports_credentials: false,
    });
    let r = env.request(&cors_req("GET", Some("http://EXAMPLE.com")));
    assert_eq!(
        r.header("Access-Control-Allow-Origin"),
        Some("http://EXAMPLE.com")
    );
    assert_eq!(r.header("Vary"), None);
    let r = env.request(&cors_req("GET", Some("http://other.com")));
    assert_eq!(r.header("Access-Control-Allow-Origin"), None);
    // No Origin: the configured one (flask-cors' `always_send`).
    let r = env.request(&cors_req("GET", None));
    assert_eq!(
        r.header("Access-Control-Allow-Origin"),
        Some("http://example.com")
    );
    // A preflight request.
    let mut req = cors_req("OPTIONS", Some("http://example.com"));
    req.headers
        .push(("Access-Control-Request-Method".into(), "patch".into()));
    req.headers.push((
        "Access-Control-Request-Headers".into(),
        "content-type, x-other".into(),
    ));
    let r = env.request(&req);
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("Access-Control-Allow-Methods"),
        Some("DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT")
    );
    assert_eq!(
        r.header("Access-Control-Allow-Headers"),
        Some("content-type")
    );
    // `*`: any origin, echoed back, with `Vary: Origin`; credentials.
    env.settings.cors = Some(Cors {
        origin: "*".into(),
        supports_credentials: true,
    });
    let r = env.request(&cors_req("GET", Some("http://a.b")));
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("http://a.b"));
    assert_eq!(r.header("Access-Control-Allow-Credentials"), Some("true"));
    assert_eq!(r.header("Vary"), Some("Origin"));
    let r = env.request(&cors_req("GET", None));
    assert_eq!(r.header("Access-Control-Allow-Origin"), None);
    env.settings.cors.as_mut().unwrap().supports_credentials = false;
    let r = env.request(&cors_req("GET", None));
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    // Errors get the headers too.
    let mut req = cors_req("GET", Some("http://a.b"));
    req.url = "/nope".into();
    assert!(
        env.request(&req)
            .header("Access-Control-Allow-Origin")
            .is_some()
    );
    // Without `cors`, none.
    env.settings.cors = None;
    let r = env.request(&cors_req("GET", Some("http://a.b")));
    assert_eq!(r.header("Access-Control-Allow-Origin"), None);
}

#[test]
fn reverse_proxy_strips_the_script_name() {
    let mut env = Env::new();
    let req = |url: &str| {
        let mut r = Request::new("GET", url);
        r.headers.push(("X-Script-Name".into(), "/beets".into()));
        r.headers.push(("X-Scheme".into(), "https".into()));
        r.headers.push(("Host".into(), "h".into()));
        r
    };
    assert_eq!(env.request(&req("/beets/stats")).status, 404);
    env.settings.reverse_proxy = true;
    assert_eq!(env.request(&req("/beets/stats")).status, 200);
    let html = String::from_utf8(env.request(&req("/beets/")).into_bytes()).unwrap();
    assert!(html.contains(r#"src="/beets/static/beets.js""#));
    let r = env.request(&req("/beets/item"));
    assert_eq!(r.header("Location"), Some("https://h/beets/item/"));
}

#[test]
fn settings_from_config() {
    let mut cfg = Config::defaults();
    let s = Settings::from_config(&cfg, false);
    assert!(s.readonly && !s.include_paths && !s.reverse_proxy && s.cors.is_none());
    assert_eq!(cfg.str("web.host"), "127.0.0.1");
    assert_eq!(cfg.str("web.port"), "8337");
    cfg.set("web.cors", Y::String("*".into()));
    cfg.set("web.cors_supports_credentials", Y::Bool(true));
    cfg.set("web.readonly", Y::Bool(false));
    let s = Settings::from_config(&cfg, true);
    let c = s.cors.unwrap();
    assert_eq!(c.origin, "*");
    assert!(c.supports_credentials && !s.readonly && s.debug);
}

/// The app behind the HTTP server, on 127.0.0.1 and a free port.
#[test]
fn serves_over_http() {
    let env = Env::new();
    let db = env.dir.path().join("l.db");
    let music = env.cfg.directory();
    let cfg = env.cfg.clone();
    let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    let s = server.clone();
    let handle = std::thread::spawn(move || {
        crate::query::configure(&cfg);
        let lib = Library::open_with(&db, Some(&music), false).unwrap();
        let app = App {
            cfg: &cfg,
            lib: &lib,
            settings: Settings::default(),
        };
        serve(&app, &s);
    });
    let base = format!("http://127.0.0.1:{port}");
    let r = ureq::get(&format!("{base}/stats")).call().unwrap();
    assert_eq!(r.content_type(), "application/json");
    assert_eq!(r.into_string().unwrap(), "{\"albums\":2,\"items\":3}\n");
    let r = ureq::get(&format!("{base}/item/query/another"))
        .call()
        .unwrap();
    let j: J = r.into_json().unwrap();
    assert_eq!(j["results"][0]["title"], json!("another title"));
    match ureq::delete(&format!("{base}/item/1")).call() {
        Err(ureq::Error::Status(405, _)) => {}
        other => panic!("{other:?}"),
    }
    let r = ureq::head(&format!("{base}/stats")).call().unwrap();
    assert_eq!(r.into_string().unwrap(), "");
    server.unblock();
    handle.join().unwrap();
}
