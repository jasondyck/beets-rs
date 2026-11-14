//! Ported from beets' `test/plugins/test_keyfinder.py`: the external
//! program is a script in a temp dir here.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use serde_yaml::Value as Y;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A fake KeyFinder program: prints `key` on stdout, its arguments into
/// `<program>.args`, and exits with `status`.
fn program(dir: &Path, name: &str, key: &str, status: i32) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(
        &p,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '{key}\\n'\nexit {status}\n"
        ),
    )
    .unwrap();
    crate::osutil::set_mode(&p, 0o755);
    p
}

fn env() -> (TempDir, Config, Library) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("import.write", Y::Bool(false));
    let bin = program(dir.path(), "KeyFinder", "dbm", 0);
    cfg.set("keyfinder.bin", bin.to_string_lossy().into());
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib)
}

fn add(lib: &Library, dir: &Path, key: Option<&str>) -> Item {
    let file = dir.join("file.mp3");
    std::fs::write(&file, b"x").unwrap();
    let mut it = Item::new_item();
    it.set_str("title", "Song");
    it.set_path(&file);
    if let Some(k) = key {
        it.set_str("initial_key", k);
    }
    lib.add(&mut it).unwrap();
    it
}

fn keyfinder(cfg: &Config, items: &mut [Item]) -> Vec<bool> {
    KeyFinderPlugin::new().find_key(cfg, items, false).unwrap()
}

#[test]
fn adds_the_key() {
    let (dir, cfg, lib) = env();
    let mut items = vec![add(&lib, dir.path(), None)];
    assert_eq!(keyfinder(&cfg, &mut items), vec![true]);
    // The MusicalKey type normalizes it, as beets' `dbm` → `C#m`.
    assert_eq!(items[0].str("initial_key"), "C#m");
    // The GUI program gets -f before the path.
    let args = std::fs::read_to_string(dir.path().join("KeyFinder.args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(args[0], "-f");
    assert_eq!(args[1], items[0].path().to_string_lossy());
}

#[test]
fn keyfinder_cli_gets_no_flag() {
    let (dir, mut cfg, lib) = env();
    let bin = program(dir.path(), "keyfinder-cli", "Am", 0);
    cfg.set("keyfinder.bin", bin.to_string_lossy().into());
    let mut items = vec![add(&lib, dir.path(), None)];
    keyfinder(&cfg, &mut items);
    assert_eq!(items[0].str("initial_key"), "Am");
    let args = std::fs::read_to_string(dir.path().join("keyfinder-cli.args")).unwrap();
    assert_eq!(args.trim(), items[0].path().to_string_lossy());
}

#[test]
fn keeps_an_existing_key() {
    let (dir, cfg, lib) = env();
    let mut items = vec![add(&lib, dir.path(), Some("F"))];
    assert_eq!(keyfinder(&cfg, &mut items), vec![false]);
    assert_eq!(items[0].str("initial_key"), "F");
}

#[test]
fn overwrite_replaces_the_key() {
    let (dir, mut cfg, lib) = env();
    cfg.set("keyfinder.overwrite", Y::Bool(true));
    let mut items = vec![add(&lib, dir.path(), Some("F"))];
    keyfinder(&cfg, &mut items);
    assert_eq!(items[0].str("initial_key"), "C#m");
}

#[test]
fn no_output_is_logged_and_skipped() {
    let (dir, mut cfg, lib) = env();
    let bin = program(dir.path(), "silent", "", 0);
    cfg.set("keyfinder.bin", bin.to_string_lossy().into());
    let mut items = vec![add(&lib, dir.path(), None)];
    assert_eq!(keyfinder(&cfg, &mut items), vec![false]);
    assert_eq!(items[0].str("initial_key"), "");
}

#[test]
fn a_failing_program_is_logged_and_skipped() {
    let (dir, mut cfg, lib) = env();
    let bin = program(dir.path(), "failing", "C", 1);
    cfg.set("keyfinder.bin", bin.to_string_lossy().into());
    let mut items = vec![add(&lib, dir.path(), None)];
    assert_eq!(keyfinder(&cfg, &mut items), vec![false]);
    assert_eq!(items[0].str("initial_key"), "");
}

#[test]
fn a_missing_program_is_logged_and_skipped() {
    let (dir, mut cfg, lib) = env();
    cfg.set(
        "keyfinder.bin",
        dir.path().join("nope").to_string_lossy().into(),
    );
    let mut items = vec![add(&lib, dir.path(), None)];
    assert_eq!(keyfinder(&cfg, &mut items), vec![false]);
    assert_eq!(items[0].str("initial_key"), "");
}

#[test]
fn only_the_last_word_of_the_output_is_the_key() {
    let (dir, mut cfg, lib) = env();
    let bin = program(dir.path(), "chatty", "detected key: G#m", 0);
    cfg.set("keyfinder.bin", bin.to_string_lossy().into());
    let mut items = vec![add(&lib, dir.path(), None)];
    keyfinder(&cfg, &mut items);
    assert_eq!(items[0].str("initial_key"), "G#m");
}
