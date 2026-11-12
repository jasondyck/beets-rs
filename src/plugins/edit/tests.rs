//! Ported from beets' test/plugins/test_edit.py (the importer cases are in
//! importer.rs' tests and diffbeets' import phase). The editor is a shell
//! script editing the file, as upstream's tests mock it.

use super::*;
use crate::ui::script;
use serde_yaml::Value as Y;
use std::os::unix::fs::PermissionsExt;

const TRACK_COUNT: usize = 10;

struct Env {
    dir: tempfile::TempDir,
    cfg: Config,
    lib: Library,
    album_id: i64,
}

impl Env {
    /// An album of 10 tracks as beets' `add_album_fixture` (no files:
    /// tags aren't written).
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.set("plugins", serde_yaml::from_str("[edit]").unwrap());
        cfg.set("import.write", Y::Bool(false));
        cfg.set("import.copy", Y::Bool(false));
        cfg.set(
            "directory",
            Y::String(dir.path().join("lib").display().to_string()),
        );
        let lib = Library::in_memory_at(&dir.path().join("lib")).unwrap();
        let mut items: Vec<Item> = (0..TRACK_COUNT)
            .map(|i| {
                let mut it = Item::new_item();
                it.set_path(dir.path().join(format!("{i}.mp3")));
                it.set_str("title", format!("t\u{ef}tle {i}"));
                it.set_str("artist", "the \u{e4}rtist");
                it.set_str("album", "\u{e4}lbum");
                it.set_str("albumartist", "the album artist");
                it.set_int("track", i as i64 + 1);
                it
            })
            .collect();
        let album = lib.add_album(&mut items).unwrap();
        Env {
            dir,
            cfg,
            lib,
            album_id: album.id.unwrap(),
        }
    }

    /// The editor: a script with this body (`$1` is the file).
    fn editor(&mut self, body: &str) {
        let p = self.dir.path().join("editor.sh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        self.cfg.set("editor", Y::String(p.display().to_string()));
    }

    /// `ModifyFileMocker(replacements=...)`.
    fn replace(&mut self, old: &str, new: &str) {
        self.editor(&format!("sed -i -e 's/{old}/{new}/g' \"$1\""));
    }

    fn run(&self, answers: &[&str], args: &[&str], album: bool) {
        script::set(answers.iter().map(|a| (*a, None)).collect());
        let q: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        command(&self.cfg, &self.lib, album, &[], false, &q).unwrap();
    }

    fn items(&self) -> Vec<Item> {
        self.lib.album_items(self.album_id).unwrap()
    }

    fn album(&self) -> Record {
        self.lib.get_album(self.album_id).unwrap().unwrap()
    }
}

fn titles(env: &Env) -> Vec<String> {
    env.items()
        .iter()
        .map(|i| i.str("title").to_string())
        .collect()
}

#[test]
fn title_edit_discard() {
    let mut env = Env::new();
    env.replace("t\u{ef}tle", "modified t\u{ef}tle");
    env.run(&["c"], &[], false);
    assert!(titles(&env).iter().all(|t| t.starts_with("t\u{ef}tle")));
}

#[test]
fn title_edit_apply() {
    let mut env = Env::new();
    env.replace("t\u{ef}tle", "modified t\u{ef}tle");
    env.run(&["a"], &[], false);
    assert!(
        titles(&env)
            .iter()
            .all(|t| t.starts_with("modified t\u{ef}tle"))
    );
    assert!(env.items().iter().all(|i| i.str("album") == "\u{e4}lbum"));
}

#[test]
fn single_title_edit_apply() {
    let mut env = Env::new();
    env.replace("t\u{ef}tle 9", "modified t\u{ef}tle 9");
    env.run(&["a"], &[], false);
    let t = titles(&env);
    assert_eq!(t.iter().filter(|t| t.starts_with("modified")).count(), 1);
    assert_eq!(t[9], "modified t\u{ef}tle 9");
}

#[test]
fn title_edit_keep_editing_then_apply() {
    let mut env = Env::new();
    env.replace("t\u{ef}tle", "modified t\u{ef}tle");
    env.run(&["e", "a"], &[], false);
    assert!(
        titles(&env)
            .iter()
            .all(|t| t.contains("modified t\u{ef}tle"))
    );
}

#[test]
fn title_edit_keep_editing_then_cancel() {
    let mut env = Env::new();
    env.replace("t\u{ef}tle", "modified t\u{ef}tle");
    env.run(&["e", "c"], &[], false);
    assert!(titles(&env).iter().all(|t| t.starts_with("t\u{ef}tle")));
}

#[test]
fn noedit() {
    let mut env = Env::new();
    env.editor("true");
    env.run(&[], &[], false);
    assert!(titles(&env).iter().all(|t| t.starts_with("t\u{ef}tle")));
}

#[test]
fn album_edit_apply() {
    // Editing items' album doesn't change the album.
    let mut env = Env::new();
    env.replace("\u{e4}lbum", "modified \u{e4}lbum");
    env.run(&["a"], &[], false);
    assert!(
        env.items()
            .iter()
            .all(|i| i.str("album") == "modified \u{e4}lbum")
    );
    assert_eq!(env.album().str("album"), "\u{e4}lbum");
}

#[test]
fn single_edit_add_field() {
    let mut env = Env::new();
    env.editor("sed -i -e 's/^id: 2$/id: 2\\nfoo: bar/' \"$1\"");
    env.run(&["a"], &[], false);
    let it = env.lib.get_item(2).unwrap().unwrap();
    assert_eq!(it.str("foo"), "bar");
}

#[test]
fn a_album_edit_apply() {
    let mut env = Env::new();
    env.replace("\u{e4}lbum", "modified \u{e4}lbum");
    env.run(&["a"], &[], true);
    assert_eq!(env.album().str("album"), "modified \u{e4}lbum");
    assert!(
        env.items()
            .iter()
            .all(|i| i.str("album") == "modified \u{e4}lbum")
    );
}

#[test]
fn a_albumartist_edit_apply() {
    let mut env = Env::new();
    env.replace("album artist", "modified album artist");
    env.run(&["a"], &[], true);
    assert_eq!(env.album().str("albumartist"), "the modified album artist");
    assert!(
        env.items()
            .iter()
            .all(|i| i.str("albumartist") == "the modified album artist")
    );
}

#[test]
fn a_album_edit_preserves_missing_artpath() {
    let mut env = Env::new();
    env.cfg
        .set("edit.albumfields", Y::String("album artpath".into()));
    env.replace("\u{e4}lbum", "modified \u{e4}lbum");
    env.run(&["a"], &[], true);
    let a = env.album();
    assert_eq!(a.str("album"), "modified \u{e4}lbum");
    assert!(a.path_field("artpath").as_os_str().is_empty());
}

#[test]
fn malformed_yaml() {
    let mut env = Env::new();
    env.editor("printf '!MALFORMED' > \"$1\"");
    env.run(&["n"], &[], false);
    assert!(titles(&env).iter().all(|t| t.starts_with("t\u{ef}tle")));
}

#[test]
fn invalid_yaml() {
    let mut env = Env::new();
    env.editor("printf 'wellformed: yes, but invalid' > \"$1\"");
    env.run(&[], &[], false);
    assert!(titles(&env).iter().all(|t| t.starts_with("t\u{ef}tle")));
}

// ApplyDataMatchingTest

fn three_items() -> Vec<Item> {
    (1..=3)
        .map(|i| {
            let mut it = Item::new_item();
            it.id = Some(i);
            it.set_str("title", format!("Title {i}"));
            it.set_int("track", i);
            it.set_str("artist", "Artist");
            it
        })
        .collect()
}

fn doc(id: Option<i64>, title: &str, track: i64) -> Doc {
    let mut d = vec![];
    if let Some(id) = id {
        d.push(("id".to_string(), PyVal::Int(id as i128)));
    }
    d.push(("title".to_string(), PyVal::Str(title.to_string())));
    d.push(("track".to_string(), PyVal::Int(track as i128)));
    d
}

fn old_docs(items: &[Item]) -> Vec<Doc> {
    items
        .iter()
        .map(|i| doc(i.id, i.str("title"), i.int("track")))
        .collect()
}

#[test]
fn matches_by_id_when_documents_are_reordered() {
    let ed = Editor::new(&Config::defaults());
    let mut items = three_items();
    let old = old_docs(&items);
    let new = vec![
        doc(Some(3), "Title 3", 3),
        doc(Some(1), "Title 1", 1),
        doc(Some(2), "Modified Title 2", 2),
    ];
    ed.apply_data(&mut items, &old, &new);
    let t: Vec<&str> = items.iter().map(|i| i.str("title")).collect();
    assert_eq!(t, ["Title 1", "Modified Title 2", "Title 3"]);
    let tr: Vec<i64> = items.iter().map(|i| i.int("track")).collect();
    assert_eq!(tr, [1, 2, 3]);
}

#[test]
fn ignores_document_with_missing_id() {
    let ed = Editor::new(&Config::defaults());
    let mut items = three_items();
    let old = old_docs(&items);
    let new = vec![
        doc(None, "Header-like doc", 99),
        doc(Some(2), "Modified Title 2", 2),
        doc(Some(3), "Title 3", 3),
    ];
    ed.apply_data(&mut items, &old, &new);
    assert_eq!(items[0].str("title"), "Title 1");
    assert_eq!(items[0].int("track"), 1);
}

#[test]
fn ignores_documents_with_duplicate_id() {
    let ed = Editor::new(&Config::defaults());
    let mut items = three_items();
    let old = old_docs(&items);
    let new = vec![
        doc(Some(1), "Title 1", 1),
        doc(Some(1), "Modified Title 2", 2),
        doc(Some(3), "Title 3", 3),
    ];
    ed.apply_data(&mut items, &old, &new);
    assert_eq!(items[0].str("title"), "Title 1");
    assert_eq!(items[0].int("track"), 1);
    assert_eq!(items[1].str("title"), "Title 2");
    assert_eq!(items[1].int("track"), 2);
}

// AlbumHeaderFieldsTest

#[test]
fn keeps_flexible_fields_but_drops_item_only_fields() {
    let mut cfg = Config::defaults();
    cfg.set(
        "edit.albumfields",
        Y::String("album mood track title".into()),
    );
    let ed = Editor::new(&cfg);
    let mut it = Item::new_item();
    it.id = Some(1);
    it.set_str("title", "Title 1");
    it.set_int("track", 1);
    it.set_str("album", "Album");
    it.set_str("mood", "Happy");
    let mut items = vec![it];
    let noop = |_: usize, _: &mut Vec<Item>| {};
    let task = ChoiceTask {
        cfg: &cfg,
        is_album: true,
        items: &mut items,
        candidates: 0,
        selected: None,
        apply_candidate: &noop,
    };
    let header = ed.album_header(&task).unwrap();
    assert_eq!(
        header,
        vec![
            ("album".to_string(), PyVal::Str("Album".into())),
            ("mood".to_string(), PyVal::Str("Happy".into())),
        ]
    );
}

// The YAML: beets' `dump`/`load` round trip and validation.

#[test]
fn dump_and_load() {
    let docs = vec![doc(Some(1), "yes", 1), doc(Some(2), "Two", 2)];
    let s = dump(&docs);
    assert_eq!(
        s,
        "id: 1\ntitle: 'yes'\ntrack: 1\n---\nid: 2\ntitle: Two\ntrack: 2\n"
    );
    assert_eq!(load(&s).unwrap(), docs);
    assert_eq!(
        load("- 1\n").unwrap_err(),
        "each entry must be a dictionary; found list"
    );
    assert!(
        load("a: [1\n")
            .unwrap_err()
            .starts_with("invalid YAML: while parsing")
    );
}

#[test]
fn typed_values() {
    let mut it = Item::new_item();
    it.set_int("track", 3);
    it.set("comp", Value::Bool(true));
    it.set_list("genres", &["Rock", "Pop"]);
    let d = flatten(&it, None, None);
    let get = |k: &str| d.iter().find(|(n, _)| n == k).unwrap().1.clone();
    assert_eq!(get("track"), PyVal::Int(3));
    // From the database a boolean is an int: edited as text.
    assert_eq!(get("comp"), PyVal::Str("True".into()));
    assert_eq!(
        get("genres"),
        PyVal::List(vec![PyVal::Str("Rock".into()), PyVal::Str("Pop".into())])
    );
    // Applying: native values for the field's type, anything else parsed.
    let mut it2 = it.clone();
    apply_doc(
        &mut it2,
        &vec![
            ("track".into(), PyVal::Str("7".into())),
            ("year".into(), PyVal::Bool(true)),
            ("title".into(), PyVal::Bool(true)),
            ("comp".into(), PyVal::Str("False".into())),
            (
                "genres".into(),
                PyVal::List(vec![PyVal::Str("Jazz".into())]),
            ),
            ("rg_track_gain".into(), PyVal::Int(2)),
        ],
    );
    assert_eq!(it2.int("track"), 7);
    assert_eq!(it2.int("year"), 1);
    assert_eq!(it2.str("title"), "True");
    assert!(!it2.bool("comp"));
    assert_eq!(it2.list("genres"), ["Jazz"]);
    assert_eq!(it2.float("rg_track_gain"), 2.0);
}
