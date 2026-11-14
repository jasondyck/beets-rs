//! Ported from beets' `test/plugins/test_replace.py`, plus the command's
//! file moves (upstream only tests the helpers; the command itself runs
//! here with scripted answers and real generated audio).

use super::*;
use crate::library::Library;
use crate::model::{Item, Kind};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A temp library directory with a real (generated) audio file helper.
struct Env {
    _dir: TempDir,
    dir: PathBuf,
    lib: Library,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lib")).unwrap();
        let lib = Library::in_memory().unwrap();
        Env {
            dir: dir.path().to_path_buf(),
            _dir: dir,
            lib,
        }
    }

    /// An audio file at `name` under the temp dir, with these tags.
    fn audio(&self, name: &str, title: &str) -> Option<PathBuf> {
        let p = self.dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.5"])
            .args(["-metadata", "artist=Artist"])
            .args(["-metadata", "album=Album"])
            .args(["-metadata", &format!("title={title}")])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(p)
    }

    /// An item whose stored path is `path` and whose title is `title`.
    fn item(&self, title: &str, path: &Path) -> Item {
        let mut it = Item::new_item();
        it.set_str("title", title);
        it.set_str("artist", "Artist");
        it.set_str("album", "Album");
        it.set_path(path);
        self.lib.add(&mut it).unwrap();
        it
    }

    /// Run the command: the printed output and its result.
    fn run(&self, opts: &CmdOpts) -> (String, Result<()>) {
        let mut out = vec![];
        let result = run(&self.lib, opts, &mut out);
        (String::from_utf8(out).unwrap(), result)
    }
}

fn opts(args: &[&str]) -> CmdOpts {
    CmdOpts {
        limit: None,
        args: args.iter().map(|a| (*a).to_string()).collect(),
    }
}

fn answers(a: &[&str]) {
    crate::ui::script::set(a.iter().map(|s| (*s, None)).collect());
}

#[test]
fn command_callback_accepts_cli_arguments() {
    let e = Env::new();
    let (_, result) = e.run(&opts(&[]));
    let err = result.unwrap_err().to_string();
    assert!(err.starts_with("Usage: beet replace"), "{err}");
    // One argument isn't enough either.
    let (_, result) = e.run(&opts(&["query"]));
    assert!(result.unwrap_err().to_string().starts_with("Usage: beet replace"));
}

#[test]
fn path_is_dir() {
    let e = Env::new();
    let d = e.dir.join("fakeDir");
    std::fs::create_dir(&d).unwrap();
    let err = file_check(&d).unwrap_err().to_string();
    assert!(err.ends_with("' is not a valid file."), "{err}");
    assert!(err.contains("fakeDir"), "{err}");
}

#[test]
fn path_is_unsupported_file() {
    let e = Env::new();
    let p = e.dir.join("fakefile.txt");
    std::fs::write(&p, "test").unwrap();
    let err = file_check(&p).unwrap_err().to_string();
    assert_eq!(err, format!("{}: File is not in a recognized format", p.display()));
}

#[test]
fn path_is_supported_file() {
    let e = Env::new();
    let Some(p) = e.audio("full.mp3", "Full") else {
        return;
    };
    file_check(&p).unwrap();
}

#[test]
fn select_song_valid_choice() {
    let e = Env::new();
    let songs: Vec<Item> = ["A", "B", "C"]
        .iter()
        .enumerate()
        .map(|(n, t)| e.item(t, &e.dir.join(format!("{n}.mp3"))))
        .collect();
    answers(&["2"]);
    let mut out = vec![];
    let selected = select_song(&songs, &e.lib, &mut out).unwrap().unwrap();
    assert_eq!(selected.str("title"), "B");
    let out = String::from_utf8(out).unwrap();
    assert!(out.starts_with("\nMatching songs:\n"), "{out}");
    assert!(out.contains("\n1. "), "{out}");
    assert!(out.contains("\n2. "), "{out}");
    assert!(out.contains("\n3. "), "{out}");
    // The menu shows the item's keys, joined by "; " as beets'
    // `displayable_path(item)` does.
    assert!(out.contains("\n2. id; "), "{out}");
    assert_eq!(
        crate::ui::script::prompts(),
        vec!["Which song would you like to replace? [1-3] (0 to cancel): "]
    );
}

#[test]
fn select_song_cancel() {
    let e = Env::new();
    let songs: Vec<Item> = ["A", "B", "C"]
        .iter()
        .map(|t| e.item(t, &e.dir.join(format!("{t}.mp3"))))
        .collect();
    answers(&["0"]);
    let mut out = vec![];
    assert!(select_song(&songs, &e.lib, &mut out).unwrap().is_none());
}

#[test]
fn select_song_invalid_then_valid() {
    let e = Env::new();
    let songs: Vec<Item> = ["A", "B", "C"]
        .iter()
        .map(|t| e.item(t, &e.dir.join(format!("{t}.mp3"))))
        .collect();
    answers(&["invalid", "4", "3"]);
    let mut out = vec![];
    let selected = select_song(&songs, &e.lib, &mut out).unwrap().unwrap();
    assert_eq!(selected.str("title"), "C");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("Invalid input. Please type in a number."), "{out}");
    assert!(
        out.contains("Invalid choice. Please enter a number between 1 and 3."),
        "{out}"
    );
    // -1 is a number out of range, not invalid input.
    answers(&["-1", "1"]);
    let mut out = vec![];
    select_song(&songs, &e.lib, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("Invalid choice. Please enter a number between 1 and 3."),
        "{out}"
    );
}

#[test]
fn confirm_replacement_file_not_exist() {
    let e = Env::new();
    let song = e.item("A", &e.dir.join("missing.mp3"));
    let mut out = vec![];
    let err = confirm_replacement(Path::new("test"), &song, &mut out)
        .unwrap_err()
        .to_string();
    assert_eq!(err, "The original song file was not found.");
}

#[test]
fn confirm_replacement_yes() {
    let e = Env::new();
    let Some(p) = e.audio("a.mp3", "A") else {
        return;
    };
    let song = e.item("A", &p);
    answers(&["YES    "]);
    let mut out = vec![];
    assert!(confirm_replacement(Path::new("test"), &song, &mut out).unwrap());
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.starts_with(&format!("\nReplacing: test -> {}", p.display())),
        "{out}"
    );
    assert_eq!(
        crate::ui::script::prompts(),
        vec!["Are you sure you want to replace this track? (y/N): "]
    );
}

#[test]
fn confirm_replacement_no() {
    let e = Env::new();
    let Some(p) = e.audio("a.mp3", "A") else {
        return;
    };
    let song = e.item("A", &p);
    for answer in ["test123", "", "n", "yes please"] {
        answers(&[answer]);
        let mut out = vec![];
        assert!(!confirm_replacement(Path::new("test"), &song, &mut out).unwrap());
    }
}

#[test]
fn same_suffix_replaces_the_file_in_place() {
    let e = Env::new();
    let Some(old) = e.audio("lib/old.mp3", "Old Title") else {
        return;
    };
    let Some(new) = e.audio("new.mp3", "New Title") else {
        return;
    };
    let song = e.item("Old Title", &old);
    answers(&["1", "y"]);
    let (out, result) = e.run(&opts(&["Old", new.to_str().unwrap()]));
    result.unwrap();
    assert!(out.ends_with("Replacement successful.\n"), "{out}");
    // The new file took the old path; its own tags are kept.
    assert!(!new.exists());
    let tags = crate::mediafile::MediaFile::open(&old).unwrap();
    assert_eq!(
        tags.get("title"),
        Some(crate::mediafile::Got::Str(Some("New Title".into())))
    );
    // The database keeps the item's tags and its path.
    let items = e.lib.items(&Query::parse::<&str>(Kind::Item, &[]).unwrap()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].str("title"), "Old Title");
    assert_eq!(items[0].path(), old.as_path());
    assert_eq!(items[0].id, song.id);
}

#[test]
fn new_suffix_changes_the_path_and_removes_the_old_file() {
    let e = Env::new();
    let Some(old) = e.audio("lib/old.mp3", "Old Title") else {
        return;
    };
    let Some(new) = e.audio("new.flac", "Flac Title") else {
        return;
    };
    e.item("Old Title", &old);
    answers(&["1", "y"]);
    let (out, result) = e.run(&opts(&["Old", new.to_str().unwrap()]));
    result.unwrap();
    assert!(out.ends_with("Replacement successful.\n"), "{out}");
    let dest = e.dir.join("lib/old.flac");
    assert!(!new.exists());
    assert!(!old.exists(), "the old file is removed");
    assert!(dest.exists());
    let items = e.lib.items(&Query::parse::<&str>(Kind::Item, &[]).unwrap()).unwrap();
    assert_eq!(items[0].path(), dest.as_path());
    assert_eq!(items[0].str("title"), "Old Title");
}

#[test]
fn abort_and_cancel_do_not_move_anything() {
    let e = Env::new();
    let Some(old) = e.audio("lib/old.mp3", "Old Title") else {
        return;
    };
    let Some(new) = e.audio("new.mp3", "New Title") else {
        return;
    };
    e.item("Old Title", &old);
    answers(&["1", "no"]);
    let (out, result) = e.run(&opts(&["Old", new.to_str().unwrap()]));
    result.unwrap();
    assert!(out.ends_with("Aborting replacement.\n"), "{out}");
    assert!(new.exists());
    let tags = crate::mediafile::MediaFile::open(&old).unwrap();
    assert_eq!(
        tags.get("title"),
        Some(crate::mediafile::Got::Str(Some("Old Title".into())))
    );
    answers(&["0"]);
    let (out, result) = e.run(&opts(&["Old", new.to_str().unwrap()]));
    result.unwrap();
    assert!(out.ends_with("Operation cancelled.\n"), "{out}");
    assert!(new.exists());
}

#[test]
fn no_match_and_limit() {
    let e = Env::new();
    let Some(a) = e.audio("lib/a.mp3", "One") else {
        return;
    };
    let Some(b) = e.audio("lib/b.mp3", "Two") else {
        return;
    };
    let Some(new) = e.audio("new.mp3", "New Title") else {
        return;
    };
    e.item("One", &a);
    e.item("Two", &b);
    let (_, result) = e.run(&opts(&["Nothing", new.to_str().unwrap()]));
    assert_eq!(result.unwrap_err().to_string(), "No matching songs found.");

    // `-l 1` shows only the first of the matching songs.
    answers(&["0"]);
    let mut o = opts(&["Artist", new.to_str().unwrap()]);
    o.limit = Some(1);
    let (out, result) = e.run(&o);
    result.unwrap();
    let menu: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with('1') || l.starts_with('2'))
        .collect();
    assert_eq!(menu.len(), 1, "{out}");
    assert!(menu[0].starts_with("1. "), "{out}");
    assert_eq!(
        crate::ui::script::prompts(),
        vec!["Which song would you like to replace? [1-1] (0 to cancel): "]
    );
}

#[test]
fn python_int() {
    assert_eq!(py_int("2"), Some(2));
    assert_eq!(py_int("  2 "), Some(2));
    assert_eq!(py_int("+2"), Some(2));
    assert_eq!(py_int("-1"), Some(-1));
    assert_eq!(py_int("1_0"), Some(10));
    assert_eq!(py_int("0x2"), None);
    assert_eq!(py_int(""), None);
    assert_eq!(py_int("_1"), None);
    assert_eq!(py_int("1_"), None);
    assert_eq!(py_int("1__0"), None);
}
