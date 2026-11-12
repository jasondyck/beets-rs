//! Ported from beets' test/plugins/test_zero.py, plus the write-event
//! details MediaFile gives.

use super::*;
use crate::mediafile::{Got, MediaFile};
use crate::model::{Item, Value};
use crate::ui::script;
use serde_yaml::Value as Y;
use std::path::{Path, PathBuf};

struct Env {
    _dir: tempfile::TempDir,
    cfg: Config,
    lib: Library,
    music: PathBuf,
}

fn set_deep(cfg: &mut Config, prefix: &str, y: Y) {
    match y {
        Y::Mapping(m) => {
            for (k, v) in m {
                let k = k.as_str().unwrap();
                let key = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{prefix}.{k}")
                };
                set_deep(cfg, &key, v);
            }
        }
        v => cfg.set(prefix, v),
    }
}

impl Env {
    /// None without ffmpeg.
    fn new() -> Option<Env> {
        media::clear_write_listeners();
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str("[zero]").unwrap());
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        let ok = std::process::Command::new("ffmpeg")
            .args(["-version"])
            .output()
            .is_ok_and(|o| o.status.success());
        ok.then_some(Env {
            _dir: dir,
            cfg,
            lib,
            music,
        })
    }

    /// An MP3 in the library with these fields, written to its file
    /// (before the plugin loads).
    fn add_item(&self, fields: &[(&str, Value)]) -> Item {
        let n = std::fs::read_dir(&self.music).unwrap().count();
        let p = self.music.join(format!("{n}.mp3"));
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.2"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok);
        let mut it = media::read(&p).unwrap();
        it.set_str("title", "Title");
        for (k, v) in fields {
            it.set(k, v.clone());
        }
        media::write_item(&mut it).unwrap();
        self.lib.add(&mut it).unwrap();
        it
    }

    /// Load the plugin with this `zero:` config.
    fn configure(&mut self, yaml: &str) -> Zero {
        media::clear_write_listeners();
        set_deep(&mut self.cfg, "zero", serde_yaml::from_str(yaml).unwrap());
        Zero::new(&self.cfg).unwrap()
    }

    fn run_zero(&self, query: &[&str]) {
        let plugins: Vec<Box<dyn Plugin>> = vec![Box::new(Zero::new(&self.cfg).unwrap())];
        let q: Vec<String> = query.iter().map(|s| s.to_string()).collect();
        command(&self.cfg, &self.lib, &plugins, None, &q).unwrap();
    }

    fn get(&self, id: i64) -> Item {
        self.lib.get_item(id).unwrap().unwrap()
    }
}

fn tag(p: &Path, f: &str) -> Got {
    MediaFile::open(p).unwrap().get(f).unwrap()
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

fn i(v: i64) -> Value {
    Value::Int(v)
}

#[test]
fn no_patterns() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[
        ("comments", s("test comment")),
        ("month", i(1)),
        ("year", i(2000)),
    ]);
    env.configure("{fields: [comments, month]}");
    media::write(&item).unwrap();
    let p = item.path();
    assert_eq!(tag(p, "comments"), Got::Str(None));
    assert_eq!(tag(p, "month"), Got::Int(None));
    assert_eq!(tag(p, "title"), Got::Str(Some("Title".into())));
    assert_eq!(tag(p, "year"), Got::Int(Some(2000)));
}

#[test]
fn pattern_match() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("comments", s("encoded by encoder"))]);
    env.configure("{fields: [comments], comments: [encoded by]}");
    media::write(&item).unwrap();
    assert_eq!(tag(item.path(), "comments"), Got::Str(None));
}

#[test]
fn pattern_nomatch() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("comments", s("recorded at place"))]);
    env.configure("{fields: [comments], comments: [encoded_by]}");
    media::write(&item).unwrap();
    assert_eq!(
        tag(item.path(), "comments"),
        Got::Str(Some("recorded at place".into()))
    );
}

#[test]
fn do_not_change_database() {
    let Some(mut env) = Env::new() else { return };
    let mut item = env.add_item(&[("year", i(2000))]);
    env.configure("{fields: [year]}");
    media::write_item(&mut item).unwrap();
    assert_eq!(item.int("year"), 2000);
    assert_eq!(tag(item.path(), "year"), Got::Int(None));
}

#[test]
fn change_database() {
    let Some(mut env) = Env::new() else { return };
    let mut item = env.add_item(&[("year", i(2000))]);
    env.configure("{fields: [year], update_database: true}");
    media::write_item(&mut item).unwrap();
    assert_eq!(item.int("year"), 0);
}

#[test]
fn album_art() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[]);
    let img = env.music.join("c.png");
    crate::artresizer::tests::write_image(&img, &crate::artresizer::tests::picture(8, 8, 1), 90);
    media::embed_art(item.path(), &std::fs::read(&img).unwrap()).unwrap();
    assert!(media::has_art(item.path()).unwrap());
    env.configure("{fields: [images]}");
    media::write(&item).unwrap();
    assert!(!media::has_art(item.path()).unwrap());
}

#[test]
fn auto_false() {
    let Some(mut env) = Env::new() else { return };
    let mut item = env.add_item(&[("year", i(2000))]);
    env.configure("{fields: [year], update_database: true, auto: false}");
    media::write_item(&mut item).unwrap();
    assert_eq!(item.int("year"), 2000);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2000)));
}

fn subcommand_item(env: &Env) -> Item {
    env.add_item(&[
        ("year", i(2016)),
        ("day", i(13)),
        ("month", i(3)),
        ("comments", s("test comment")),
    ])
}

#[test]
fn subcommand_update_database_true() {
    let Some(mut env) = Env::new() else { return };
    let item = subcommand_item(&env);
    env.configure("{fields: [comments], update_database: true, auto: false}");
    script::set(vec![("y", None)]);
    env.run_zero(&[]);
    let back = env.get(item.id.unwrap());
    assert_eq!(back.int("year"), 2016);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
    assert_eq!(tag(item.path(), "comments"), Got::Str(None));
    assert_eq!(back.str("comments"), "");
}

#[test]
fn subcommand_update_database_false() {
    let Some(mut env) = Env::new() else { return };
    let item = subcommand_item(&env);
    env.configure("{fields: [comments], update_database: false, auto: false}");
    script::set(vec![("y", None)]);
    env.run_zero(&[]);
    let back = env.get(item.id.unwrap());
    assert_eq!(back.int("year"), 2016);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
    assert_eq!(back.str("comments"), "test comment");
    assert_eq!(tag(item.path(), "comments"), Got::Str(None));
}

#[test]
fn subcommand_query_include() {
    let Some(mut env) = Env::new() else { return };
    let item = subcommand_item(&env);
    env.configure("{fields: [comments], update_database: false, auto: false}");
    env.run_zero(&["year: 2016"]);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
    assert_eq!(tag(item.path(), "comments"), Got::Str(None));
}

#[test]
fn subcommand_query_exclude() {
    let Some(mut env) = Env::new() else { return };
    let item = subcommand_item(&env);
    env.configure("{fields: [comments], update_database: false, auto: false}");
    env.run_zero(&["year: 0000"]);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
    assert_eq!(
        tag(item.path(), "comments"),
        Got::Str(Some("test comment".into()))
    );
}

#[test]
fn no_fields() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("year", i(2016))]);
    env.configure("{fields: []}");
    script::set(vec![("y", None)]);
    env.run_zero(&[]);
    assert_eq!(env.get(item.id.unwrap()).int("year"), 2016);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
}

#[test]
fn whitelist_and_blacklist() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("year", i(2016))]);
    let z = env.configure("{fields: [year], keep_fields: [comments]}");
    assert!(z.core.fields_to_progs.is_empty());
    script::set(vec![("y", None)]);
    env.run_zero(&[]);
    assert_eq!(env.get(item.id.unwrap()).int("year"), 2016);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
}

#[test]
fn keep_fields() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("year", i(2016)), ("comments", s("test comment"))]);
    let z = env.configure("{fields: null, keep_fields: [year], update_database: true}");
    let mut tags = WriteTags::for_item(&item);
    z.core.write(&mut tags);
    let (nulls, _) = tags.into_parts();
    assert!(nulls.contains("comments"));
    assert!(!nulls.contains("year"));
}

#[test]
fn keep_fields_removes_preserved_tags() {
    let mut cfg = Config::defaults();
    set_deep(
        &mut cfg,
        "zero",
        serde_yaml::from_str("{keep_fields: [year], fields: null, update_database: true}").unwrap(),
    );
    let core = ZeroCore::new(&cfg, &PluginLog::new("zero")).unwrap();
    assert!(!core.fields_to_progs.iter().any(|(f, _)| f == "id"));
    assert!(!core.fields_to_progs.iter().any(|(f, _)| f == "year"));
}

#[test]
fn fields_removes_preserved_tags() {
    let mut cfg = Config::defaults();
    set_deep(
        &mut cfg,
        "zero",
        serde_yaml::from_str("{fields: [year id], update_database: true}").unwrap(),
    );
    let core = ZeroCore::new(&cfg, &PluginLog::new("zero")).unwrap();
    assert!(!core.fields_to_progs.iter().any(|(f, _)| f == "id"));
    // A sequence item isn't split: "year id" is no field at all.
    assert!(core.fields_to_progs.is_empty());
    cfg.set("zero.fields", Y::String("year id".into()));
    let core = ZeroCore::new(&cfg, &PluginLog::new("zero")).unwrap();
    let names: Vec<&str> = core
        .fields_to_progs
        .iter()
        .map(|(f, _)| f.as_str())
        .collect();
    assert_eq!(names, ["year"]);
}

#[test]
fn omit_single_disc_with_tags_single() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[
        ("disctotal", i(1)),
        ("disc", i(1)),
        ("comments", s("test comment")),
    ]);
    env.configure("{omit_single_disc: true, fields: [comments]}");
    media::write(&item).unwrap();
    assert_eq!(tag(item.path(), "comments"), Got::Str(None));
    assert_eq!(tag(item.path(), "disc"), Got::Int(None));
    assert_eq!(tag(item.path(), "disctotal"), Got::Int(None));
}

#[test]
fn omit_single_disc_with_tags_multi() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[
        ("disctotal", i(4)),
        ("disc", i(1)),
        ("comments", s("test comment")),
    ]);
    env.configure("{omit_single_disc: true, fields: [comments]}");
    media::write(&item).unwrap();
    assert_eq!(tag(item.path(), "comments"), Got::Str(None));
    assert_eq!(tag(item.path(), "disc"), Got::Int(Some(1)));
    assert_eq!(tag(item.path(), "disctotal"), Got::Int(Some(4)));
}

#[test]
fn omit_single_disc_only_change_single() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("disctotal", i(1)), ("disc", i(1))]);
    env.configure("{omit_single_disc: true}");
    media::write(&item).unwrap();
    assert_eq!(tag(item.path(), "disc"), Got::Int(None));
    assert_eq!(tag(item.path(), "disctotal"), Got::Int(None));
}

#[test]
fn omit_single_disc_only_change_multi() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[("disctotal", i(4)), ("disc", i(1))]);
    env.configure("{omit_single_disc: true}");
    media::write(&item).unwrap();
    assert_eq!(tag(item.path(), "disc"), Got::Int(Some(1)));
    assert_eq!(tag(item.path(), "disctotal"), Got::Int(Some(4)));
}

#[test]
fn empty_query_n_response_no_changes() {
    let Some(mut env) = Env::new() else { return };
    let item = subcommand_item(&env);
    env.configure("{fields: [comments], update_database: true, auto: false}");
    script::set(vec![("n", None)]);
    env.run_zero(&[]);
    let back = env.get(item.id.unwrap());
    assert_eq!(back.int("year"), 2016);
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2016)));
    assert_eq!(
        tag(item.path(), "comments"),
        Got::Str(Some("test comment".into()))
    );
    assert_eq!(back.str("comments"), "test comment");
}

#[test]
fn keep_fields_images_preserves_art() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[]);
    let img = env.music.join("c.png");
    crate::artresizer::tests::write_image(&img, &crate::artresizer::tests::picture(8, 8, 1), 90);
    media::embed_art(item.path(), &std::fs::read(&img).unwrap()).unwrap();
    env.configure("{fields: null, keep_fields: [images]}");
    media::write(&item).unwrap();
    assert!(media::has_art(item.path()).unwrap());
    assert_eq!(tag(item.path(), "title"), Got::Str(None));
}

// Beyond beets' tests.

#[test]
fn patterns_match_python_str_of_value() {
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[
        ("year", i(1999)),
        ("genres", s("Rock; Pop")),
        ("comp", Value::Bool(true)),
    ]);
    env.configure(
        "{fields: [year, genres, comp], year: ['^19'], genres: [\"'pop'\"], comp: [^true$]}",
    );
    media::write(&item).unwrap();
    assert_eq!(tag(item.path(), "year"), Got::Int(None));
    assert_eq!(tag(item.path(), "genres"), Got::List(None));
    assert_eq!(tag(item.path(), "comp"), Got::Bool(None));
}

#[test]
fn single_fields_sorting_before_their_lists_stay() {
    // MediaFile deletes `genre` (and `date`), then writes `genres` (and
    // the year, month and day) again; `catalognums` sorts after
    // `catalognum` and stays deleted.
    let Some(mut env) = Env::new() else { return };
    let item = env.add_item(&[
        ("genres", s("Rock")),
        ("year", i(2001)),
        ("catalognum", s("CAT1")),
    ]);
    env.configure("{fields: [genre, date, catalognums]}");
    media::write(&item).unwrap();
    assert_eq!(
        tag(item.path(), "genres"),
        Got::List(Some(vec!["Rock".into()]))
    );
    assert_eq!(tag(item.path(), "year"), Got::Int(Some(2001)));
    assert_eq!(tag(item.path(), "catalognum"), Got::Str(None));
}

#[test]
fn invalid_fields_and_regex() {
    let mut cfg = Config::defaults();
    set_deep(
        &mut cfg,
        "zero",
        serde_yaml::from_str("{fields: [bogus, id, path, comments], comments: []}").unwrap(),
    );
    // An explicit empty pattern list never matches: the field isn't added.
    let core = ZeroCore::new(&cfg, &PluginLog::new("zero")).unwrap();
    assert!(core.fields_to_progs.is_empty());
    cfg.set("zero.comments", serde_yaml::from_str("['(']").unwrap());
    assert!(ZeroCore::new(&cfg, &PluginLog::new("zero")).is_err());
}

#[test]
fn as_is_choice_warns_once() {
    let mut cfg = Config::defaults();
    cfg.set("zero.fields", Y::String("comments".into()));
    let z = Zero::new(&cfg).unwrap();
    z.import_task_choice(false);
    assert!(!z.warned.get());
    z.import_task_choice(true);
    assert!(z.warned.get());
    media::clear_write_listeners();
}
