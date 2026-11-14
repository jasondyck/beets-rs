//! Ported from beets' `test/plugins/test_unimported.py`.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use serde_yaml::Value as Y;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn env() -> (TempDir, Config, Library, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("plugins", serde_yaml::from_str("[unimported]").unwrap());
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib, libdir)
}

fn make_file(libdir: &Path, rel: &str) -> PathBuf {
    let p = libdir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"").unwrap();
    // `rel` uses `/`; the reported paths use the platform's separators.
    p.components().collect()
}

fn add_item(lib: &Library, libdir: &Path) -> Item {
    let path = make_file(libdir, "artist/album/track.mp3");
    let mut item = Item::new_item();
    item.set_path(path);
    lib.add(&mut item).unwrap();
    item
}

fn unimported(cfg: &Config, lib: &Library) -> HashSet<String> {
    let mut out = vec![];
    run(cfg, lib, &mut out).unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

fn strings(xs: &[&str]) -> Y {
    Y::Sequence(xs.iter().map(|s| Y::String((*s).into())).collect())
}

#[test]
fn report_files_missing_from_the_library() {
    let (_d, cfg, lib, libdir) = env();
    let item = add_item(&lib, &libdir);
    let strays: HashSet<String> = [
        make_file(&libdir, "stray.txt"),
        make_file(&libdir, "sub/stray.txt"),
    ]
    .iter()
    .map(|p| p.display().to_string())
    .collect();
    assert!(item.path().exists());
    assert_eq!(unimported(&cfg, &lib), strays);
}

#[test]
fn do_not_report_album_art() {
    let (_d, cfg, lib, libdir) = env();
    let mut album = {
        let mut item = Item::new_item();
        item.set_path(make_file(&libdir, "artist/album/track.mp3"));
        lib.add_album(std::slice::from_mut(&mut item)).unwrap()
    };
    album.set(
        "artpath",
        crate::model::Value::Path(make_file(&libdir, "cover.jpg")),
    );
    lib.store(&album).unwrap();
    let stray = make_file(&libdir, "stray.txt");
    assert_eq!(
        unimported(&cfg, &lib),
        HashSet::from([stray.display().to_string()])
    );
}

#[test]
fn ignore_extension() {
    for (ignore, filename) in [
        ("mp3", "archive.mp3"),
        ("tar.gz", "archive.tar.gz"),
        ("mp3", "archive.MP3"),
    ] {
        let (_d, mut cfg, lib, libdir) = env();
        make_file(&libdir, filename);
        let reported = make_file(&libdir, "report.txt");
        cfg.set("unimported.ignore_extensions", strings(&[ignore]));
        assert_eq!(
            unimported(&cfg, &lib),
            HashSet::from([reported.display().to_string()]),
            "{ignore} {filename}"
        );
    }
}

#[test]
fn ignore_subdirectory() {
    let (_d, mut cfg, lib, libdir) = env();
    make_file(&libdir, "data/report.txt");
    make_file(&libdir, "data/nested/report.txt");
    let reported = make_file(&libdir, "music/report.txt");
    cfg.set("unimported.ignore_subdirectories", strings(&["data"]));
    assert_eq!(
        unimported(&cfg, &lib),
        HashSet::from([reported.display().to_string()])
    );
}

#[test]
fn do_not_ignore_partially_matching_subdirectory() {
    let (_d, mut cfg, lib, libdir) = env();
    let all: HashSet<String> = [
        make_file(&libdir, "music/report.txt"),
        make_file(&libdir, "data/report.txt"),
        make_file(&libdir, "data/nested/report.txt"),
    ]
    .iter()
    .map(|p| p.display().to_string())
    .collect();
    cfg.set("unimported.ignore_subdirectories", strings(&["dat"]));
    assert_eq!(unimported(&cfg, &lib), all);
}
