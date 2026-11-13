//! beets' `test/plugins/test_duplicates.py`, and the plugin's own rules.

use super::*;
use crate::model::Item;
use serde_yaml::Value as Y;
use tempfile::TempDir;

struct Env {
    dir: TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str("[duplicates]").unwrap());
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env { dir, cfg, lib }
    }

    fn set(&mut self, key: &str, yaml: &str) {
        self.cfg.set(key, serde_yaml::from_str(yaml).unwrap());
    }

    fn run(&self, opts: &CmdOpts) -> String {
        let mut out = vec![];
        run(&self.cfg, &self.lib, opts, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn add(&self, pairs: &[(&str, &str)], path: &str) -> Item {
        let mut it = Item::new_item();
        it.set_path(self.dir.path().join(path));
        for (k, v) in pairs {
            it.set_parsed(k, v).unwrap();
        }
        self.lib.add(&mut it).unwrap();
        it
    }

    fn items(&self) -> Vec<Item> {
        self.lib
            .items(&Query::parse(Kind::Item, &[] as &[String]).unwrap())
            .unwrap()
    }
}

/// beets' fixture: the same item, added `count` times.
const DUP: &[(&str, &str)] = &[
    ("album", "Pretend Album"),
    ("albumartist", "Pretend Artist"),
    ("artist", "Pretend Artist"),
    ("title", "Pretend Track"),
    ("genres", "Original Genre"),
    ("mb_trackid", "abc"),
    ("mb_albumid", "def"),
];

fn dups(count: usize) -> Env {
    let env = Env::new();
    for _ in 0..count {
        env.add(DUP, "pretend.mp3");
    }
    env
}

fn run_cmd(env: &Env, count: bool, path: bool) -> String {
    env.run(&CmdOpts {
        count,
        path,
        ..Default::default()
    })
    .trim()
    .to_string()
}

#[test]
fn duplicate() {
    let out = run_cmd(&dups(2), false, false);
    assert!(out.contains("Pretend Artist"), "{out}");
    assert!(out.contains("Pretend Album"), "{out}");
    assert!(out.contains("Pretend Track"), "{out}");
}

#[test]
fn duplicate_path() {
    let env = dups(2);
    let out = run_cmd(&env, false, true);
    assert!(
        out.contains(env.dir.path().join("pretend.mp3").to_str().unwrap()),
        "{out}"
    );
}

#[test]
fn duplicate_with_count() {
    let out = run_cmd(&dups(4), true, false);
    assert!(out.contains("Pretend Artist"), "{out}");
    assert!(out.contains("Pretend Album"), "{out}");
    assert!(out.contains("Pretend Track"), "{out}");
    assert!(out.ends_with('3'), "{out}");
}

#[test]
fn duplicate_path_with_count() {
    let env = dups(6);
    let out = run_cmd(&env, true, true);
    assert!(
        out.contains(env.dir.path().join("pretend.mp3").to_str().unwrap()),
        "{out}"
    );
    assert!(out.ends_with('5'), "{out}");
}

// ------------------------------------------------------------ beyond beets'

#[test]
fn groups_skip_empty_keys_and_strict_needs_all() {
    let env = Env::new();
    env.add(&[("title", "A"), ("label", "L")], "1.mp3");
    env.add(&[("title", "A")], "2.mp3");
    env.add(&[("title", "")], "3.mp3");
    env.add(&[("title", "")], "4.mp3");
    let keys = |strict: bool| CmdOpts {
        keys: vec!["title".into(), "label".into()],
        strict,
        full: true,
        format: Some("$path".into()),
        ..Default::default()
    };
    // Not strict: (A, L) and (A) differ; the two empty ones are skipped.
    assert_eq!(env.run(&keys(false)), "");
    let opts = CmdOpts {
        keys: vec!["title".into(), "nonexistent".into()],
        full: true,
        format: Some("$title".into()),
        ..Default::default()
    };
    assert_eq!(env.run(&opts), "A\nA\n");
    // Strict: nothing has every key.
    assert_eq!(env.run(&keys(true)), "");
}

#[test]
fn numbers_group_across_types_but_bytes_never_equal_str() {
    let a = KeyPart::of(&PyVal::Int(1)).unwrap();
    assert_eq!(a, KeyPart::of(&PyVal::Float(1.0)).unwrap());
    assert_eq!(a, KeyPart::of(&PyVal::Bool(true)).unwrap());
    assert_ne!(
        KeyPart::of(&PyVal::Str("x".into())).unwrap(),
        KeyPart::of(&PyVal::Bytes(b"x".to_vec())).unwrap()
    );
    // 0 and b"" are kept (only None and "" are dropped).
    assert!(KeyPart::of(&PyVal::Int(0)).is_some());
    assert!(KeyPart::of(&PyVal::Bytes(vec![])).is_some());
    assert!(KeyPart::of(&PyVal::Str(String::new())).is_none());
    assert!(KeyPart::of(&PyVal::None).is_none());
}

#[test]
fn order_by_completeness_and_tiebreak() {
    let mut env = Env::new();
    env.add(&[("title", "A")], "1.mp3");
    env.add(&[("title", "A"), ("label", "L"), ("isrc", "I")], "2.mp3");
    env.add(&[("title", "A"), ("label", "L")], "3.mp3");
    let opts = || CmdOpts {
        keys: vec!["title".into()],
        full: true,
        format: Some("$id".into()),
        ..Default::default()
    };
    assert_eq!(env.run(&opts()), "2\n3\n1\n");
    // tiebreak: by the fields, highest first; ties keep their order.
    env.set("duplicates.tiebreak", "{items: [id]}");
    assert_eq!(env.run(&opts()), "3\n2\n1\n");
    env.set("duplicates.tiebreak", "{items: [label]}");
    assert_eq!(env.run(&opts()), "2\n3\n1\n");
}

#[test]
fn merge_fills_the_head_from_the_rest() {
    let env = Env::new();
    env.add(&[("title", "A"), ("label", "L"), ("isrc", "I")], "1.mp3");
    env.add(&[("title", "A"), ("grouping", "G")], "2.mp3");
    env.add(
        &[("title", "A"), ("grouping", "H"), ("bpm", "120")],
        "3.mp3",
    );
    let out = env.run(&CmdOpts {
        keys: vec!["title".into()],
        merge: true,
        full: true,
        format: Some("$id $grouping $bpm".into()),
        ..Default::default()
    });
    // The head takes the first non-empty string; numbers are never empty.
    assert_eq!(out, "1 G 0\n2 G 0\n3 H 120\n");
    let head = env.lib.get_item(1).unwrap().unwrap();
    assert_eq!(head.str("grouping"), "G");
}

#[test]
fn tag_sets_a_field_on_each_duplicate() {
    let env = dups(3);
    env.run(&CmdOpts {
        tag: Some("dup=yes".into()),
        ..Default::default()
    });
    let tagged: Vec<Option<String>> = env
        .items()
        .iter()
        .map(|i| i.flex.get("dup").cloned())
        .collect();
    assert_eq!(
        tagged
            .iter()
            .filter(|t| t.as_deref() == Some("yes"))
            .count(),
        2
    );
    let mut out = vec![];
    let err = run(
        &env.cfg,
        &env.lib,
        &CmdOpts {
            tag: Some("a=b=c".into()),
            ..Default::default()
        },
        &mut out,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "duplicates: can't parse k=v tag: a=b=c");
    // The first duplicate was printed before the error.
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "Pretend Artist - Pretend Album - Pretend Track\n"
    );
}

#[test]
fn remove_drops_the_duplicates() {
    let env = dups(3);
    env.run(&CmdOpts {
        remove: true,
        ..Default::default()
    });
    assert_eq!(env.items().len(), 1);
}

#[test]
fn checksum_groups_by_program_output_and_caches_it() {
    let env = Env::new();
    for (name, body) in [("1.mp3", "same"), ("2.mp3", "same"), ("3.mp3", "other")] {
        std::fs::write(env.dir.path().join(name), body).unwrap();
        env.add(&[("title", name)], name);
    }
    let opts = || CmdOpts {
        checksum: Some(r#"sh -c 'cksum < "$0"' {file}"#.into()),
        full: true,
        format: Some("$title".into()),
        ..Default::default()
    };
    assert_eq!(env.run(&opts()), "1.mp3\n2.mp3\n");
    let cached: Vec<String> = env.items().iter().map(|i| i.flex["sh"].clone()).collect();
    assert!(cached[0].ends_with('\n') && cached[0] == cached[1] && cached[0] != cached[2]);
    // Cached now: the same groups without running the program.
    assert_eq!(env.run(&opts()), "1.mp3\n2.mp3\n");
    // A failing program leaves nothing to group by.
    let failing = CmdOpts {
        checksum: Some("false {file}".into()),
        ..Default::default()
    };
    assert_eq!(env.run(&failing), "");
}

#[test]
fn shlex_and_format_as_python() {
    assert_eq!(
        shlex_split(r#"sh -c 'cksum < "$0"' {file}"#).unwrap(),
        ["sh", "-c", r#"cksum < "$0""#, "{file}"]
    );
    assert_eq!(
        shlex_split(r#"a\ b "c \"d\" \x" # not-a-comment"#).unwrap(),
        ["a b", r#"c "d" \x"#, "#", "not-a-comment"]
    );
    assert_eq!(shlex_split("x ''").unwrap(), ["x", ""]);
    assert!(shlex_split("'open").is_err());
    assert_eq!(format_arg("{file}.{{x}}", "/a").unwrap(), "/a.{x}");
    assert!(format_arg("{other}", "/a").is_err());
}

#[test]
fn albums_group_by_mb_albumid() {
    let env = Env::new();
    for (n, album) in ["A", "B"].iter().enumerate() {
        let mut items = vec![];
        for t in 0..=n {
            let mut it = Item::new_item();
            it.set_path(env.dir.path().join(format!("{album}{t}.mp3")));
            it.set_str("album", *album);
            it.set_str("albumartist", "X");
            it.set_str("mb_albumid", "same");
            items.push(it);
        }
        env.lib.add_album(&mut items).unwrap();
    }
    let out = env.run(&CmdOpts {
        album: true,
        full: true,
        count: true,
        ..Default::default()
    });
    // The album with more tracks first.
    assert_eq!(out, "X - B: 2\nX - A: 2\n");
}

#[test]
fn a_copy_never_lands_on_its_own_source() {
    // beets' `move_file`: a copy whose source already has the `.1` name
    // gets a new unique path, not its own file.
    let env = Env::new();
    let dir = env.dir.path().join("music/Ar/Al");
    std::fs::create_dir_all(&dir).unwrap();
    let tags = [
        ("artist", "Ar"),
        ("albumartist", "Ar"),
        ("album", "Al"),
        ("title", "T"),
        ("track", "1"),
    ];
    for name in ["01 T.mp3", "01 T.1.mp3"] {
        std::fs::write(dir.join(name), name).unwrap();
        env.add(&tags, &format!("music/Ar/Al/{name}"));
    }
    let src = env
        .items()
        .into_iter()
        .find(|i| i.path().ends_with("01 T.1.mp3"))
        .unwrap();
    let mut cfg = env.cfg.clone();
    cfg.set(
        "paths",
        serde_yaml::from_str("{singleton: '$artist/$album/$track $title'}").unwrap(),
    );
    crate::commands::move_model(&cfg, &env.lib, &src, FileOp::Copy, None).unwrap();
    let copied = env.lib.get_item(src.id.unwrap()).unwrap().unwrap();
    assert!(
        copied.path().ends_with("01 T.2.mp3"),
        "{}",
        copied.path().display()
    );
    assert_eq!(std::fs::read(copied.path()).unwrap(), b"01 T.1.mp3");
    assert!(dir.join("01 T.1.mp3").exists());
}
