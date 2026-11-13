//! beets' test/plugins/test_parentwork.py, with a fake MusicBrainz on
//! 127.0.0.1 (a free port) where beets uses `requests_mock`.

use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use serde_json::json;
use serde_yaml::Value as Y;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

/// The three works of the upstream test: 1 -> 2 -> 3, the last with a
/// composer (and an end date).
fn server() -> Server {
    serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        match path {
            "/ws/2/work/1" => Reply::json(json!({
                "id": "1",
                "title": "work",
                "work-relations": [
                    {"type": "parts", "direction": "backward", "work": {"id": "2"}}
                ]
            })),
            "/ws/2/work/2" => Reply::json(json!({
                "id": "2",
                "title": "directparentwork",
                "work-relations": [
                    {"type": "parts", "direction": "backward", "work": {"id": "3"}}
                ]
            })),
            _ => Reply::json(json!({
                "id": "3",
                "title": "parentwork",
                "disambiguation": "the big one",
                "artist-relations": [
                    {"type": "composer", "end": "1712-03-22",
                     "artist": {"name": "random composer",
                                "sort-name": "composer, random"}}
                ]
            })),
        }
    })
}

fn env(server: &Server) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    let mut cfg = Config::defaults();
    cfg.set("directory", Y::String(music.display().to_string()));
    cfg.set("plugins", serde_yaml::from_str("[parentwork]").unwrap());
    cfg.set(
        "musicbrainz.host",
        Y::String(server.base.trim_start_matches("http://").to_string()),
    );
    cfg.set("musicbrainz.ratelimit", Y::from(1000));
    crate::formatted::configure(&cfg).unwrap();
    let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
    Env {
        _dir: dir,
        cfg,
        lib,
    }
}

fn item(lib: &Library, fields: &[(&str, &str)]) -> Item {
    let mut it = Item::new_item();
    it.set_path("/file");
    for (k, v) in fields {
        it.set(k, Value::Str(v.to_string()));
    }
    lib.add(&mut it).unwrap();
    it
}

fn run(env: &Env, force: Option<bool>) {
    command(
        &env.cfg,
        &env.lib,
        &CmdOpts {
            force,
            ..Default::default()
        },
    )
    .unwrap();
}

fn load(lib: &Library) -> Item {
    let q = Query::parse(Kind::Item, &[] as &[String]).unwrap();
    lib.items(&q).unwrap().pop().unwrap()
}

#[test]
fn normal_case() {
    let server = server();
    let env = env(&server);
    item(
        &env.lib,
        &[("mb_workid", "1"), ("parentwork_workid_current", "1")],
    );
    run(&env, None);
    let it = load(&env.lib);
    assert_eq!(it.str("mb_parentworkid"), "3");
    assert_eq!(it.str("parentwork"), "parentwork");
    assert_eq!(it.str("parentwork_disambig"), "the big one");
    assert_eq!(it.str("parent_composer"), "random composer");
    assert_eq!(it.str("parent_composer_sort"), "composer, random");
    assert_eq!(it.str("work_date"), "1712-03-22");
    assert_eq!(it.str("parentwork_date"), "1712-03-22");
    assert_eq!(it.str("parentwork_workid_current"), "1");
    assert_eq!(server.requests().len(), 3);
    assert!(
        server.requests()[0]
            .target
            .starts_with("/ws/2/work/1?inc=work-rels%2Bartist-rels"),
        "{}",
        server.requests()[0].target
    );
}

#[test]
fn force() {
    let server = server();
    let mut env = env(&server);
    env.cfg.set("parentwork.force", Y::Bool(true));
    item(
        &env.lib,
        &[
            ("mb_workid", "1"),
            ("mb_parentworkid", "XXX"),
            ("parentwork_workid_current", "1"),
            ("parentwork", "parentwork"),
        ],
    );
    run(&env, None);
    assert_eq!(load(&env.lib).str("mb_parentworkid"), "3");
}

#[test]
fn no_force() {
    let server = server();
    let env = env(&server);
    item(
        &env.lib,
        &[
            ("mb_workid", "1"),
            ("mb_parentworkid", "XXX"),
            ("parentwork_workid_current", "1"),
            ("parentwork", "parentwork"),
        ],
    );
    run(&env, None);
    assert_eq!(load(&env.lib).str("mb_parentworkid"), "XXX");
    assert!(server.requests().is_empty());
}

#[test]
fn work_changed_refetches() {
    let server = server();
    let env = env(&server);
    item(
        &env.lib,
        &[
            ("mb_workid", "1"),
            ("mb_parentworkid", "XXX"),
            ("parentwork_workid_current", "other"),
            ("parentwork", "parentwork"),
        ],
    );
    run(&env, None);
    assert_eq!(load(&env.lib).str("mb_parentworkid"), "3");
}

#[test]
fn no_work_id() {
    let server = server();
    let env = env(&server);
    item(&env.lib, &[("mb_trackid", "track")]);
    run(&env, Some(true));
    assert!(server.requests().is_empty());
    assert!(load(&env.lib).get("parentwork").is_none());
}

#[test]
fn error_fetching_leaves_the_item() {
    // A 404 makes the lookup fail; the item is left alone.
    let server = serve(|_: &Seen| Reply::status(404));
    let env = env(&server);
    item(&env.lib, &[("mb_workid", "1")]);
    run(&env, Some(true));
    assert!(load(&env.lib).get("parentwork").is_none());
}
