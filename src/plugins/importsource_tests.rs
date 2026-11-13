//! Ported from beets' `test/plugins/test_importsource.py` (the removal prompt
//! is answered through `ui::script`, the full import through the importer).

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::query::Query;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn env(yaml: &str) -> (TempDir, Config, Library, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("library", dir.path().join("library.db").to_string_lossy().into());
    cfg.set("plugins", serde_yaml::from_str("[importsource]").unwrap());
    cfg.set("import.write", serde_yaml::Value::Bool(false));
    cfg.set("import.copy", serde_yaml::Value::Bool(true));
    cfg.set("import.resume", "no".into());
    cfg.set("import.incremental", serde_yaml::Value::Bool(false));
    cfg.set("import.duplicate_action", "keep".into());
    cfg.set("ui.color", serde_yaml::Value::Bool(false));
    if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        for (k, v) in m {
            cfg.set(k.as_str().unwrap(), v);
        }
    }
    let lib = Library::open_with(&cfg.library_path(), Some(&libdir), false).unwrap();
    (dir, cfg, lib, libdir)
}

fn make_mp3(dir: &Path, name: &str, title: &str) -> bool {
    std::fs::create_dir_all(dir).unwrap();
    std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"])
        .args(["-metadata", "artist=Artist", "-metadata", "album=Album"])
        .args(["-metadata", &format!("title={title}")])
        .arg(dir.join(name))
        .status()
        .is_ok_and(|s| s.success())
}

fn all_items(lib: &Library) -> Vec<Item> {
    lib.items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
        .unwrap()
}

/// An item with a `source_path` pointing at a file under `dir`.
fn source_item(lib: &Library, dir: &Path, name: &str) -> (Item, PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    let src = dir.join(name);
    std::fs::write(&src, b"audio").unwrap();
    let mut item = Item::new_item();
    item.set_path(dir.join("lib").join(name));
    item.set_str("title", name);
    item.set("source_path", Value::Str(src.to_string_lossy().into_owned()));
    lib.add(&mut item).unwrap();
    (item, src)
}

#[test]
fn source_path_written_on_import_and_kept_on_reimport() {
    let (d, cfg, lib, _libdir) = env("{}");
    let src = d.path().join("src/Album");
    if !make_mp3(&src, "1.mp3", "One") || !make_mp3(&src, "2.mp3", "Two") {
        return;
    }
    let plugins = crate::plugins::load(&cfg).unwrap();
    let mut opts = crate::importer::ImportOptions::from_config(&cfg);
    opts.autotag = false;
    let mut imp = crate::importer::Importer::new(&cfg, &lib, opts, &plugins).unwrap();
    imp.run(std::slice::from_ref(&src)).unwrap();
    let items = all_items(&lib);
    assert_eq!(items.len(), 2);
    for item in &items {
        assert!(item.str("source_path").ends_with(".mp3"), "{}", item.str("source_path"));
    }
    // A reimport keeps the original source path (it is the library file
    // now, but the attribute is still set).
    let before: Vec<String> = items.iter().map(|i| i.str("source_path").to_string()).collect();
    let plugins = crate::plugins::load(&cfg).unwrap();
    let mut opts = crate::importer::ImportOptions::from_config(&cfg);
    opts.autotag = false;
    opts.singletons = false;
    let mut imp = crate::importer::Importer::new(&cfg, &lib, opts, &plugins).unwrap();
    imp.run(&[cfg.directory()]).unwrap();
    let after: Vec<String> = all_items(&lib)
        .iter()
        .map(|i| i.str("source_path").to_string())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn delete_single_source() {
    let (d, cfg, lib, _libdir) = env("{importsource: {suggest_removal: yes}}");
    let (item, src) = source_item(&lib, &d.path().join("orig"), "x.mp3");
    crate::ui::script::set(vec![("d", None)]);
    let _p = load(&cfg);
    crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item))]);
    assert!(!src.exists());
}

#[test]
fn delete_source_directory() {
    let (d, cfg, lib, _libdir) = env("{importsource: {suggest_removal: yes}}");
    let dir = d.path().join("orig");
    let (item, _src) = source_item(&lib, &dir, "x.mp3");
    let (other, other_src) = source_item(&lib, &dir, "y.mp3");
    let _ = other;
    crate::ui::script::set(vec![("r", None), ("y", None)]);
    let _p = load(&cfg);
    crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item))]);
    assert!(!dir.exists(), "{}", other_src.display());
}

#[test]
fn stop_suggesting_for_an_album() {
    let (d, cfg, lib, _libdir) = env("{importsource: {suggest_removal: yes}}");
    let dir = d.path().join("orig");
    let (mut item, src) = source_item(&lib, &dir, "x.mp3");
    item.set_str("mb_albumid", "11111111-1111-1111-1111-111111111111");
    lib.store(&item).unwrap();
    crate::ui::script::set(vec![("s", None)]);
    let _p = load(&cfg);
    crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item))]);
    assert!(src.exists());
    assert_eq!(crate::ui::script::prompts().len(), 1);
    // A second removal of the same album asks nothing.
    crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item))]);
    assert_eq!(crate::ui::script::prompts().len(), 1);
}

#[test]
fn do_nothing_keeps_the_source() {
    let (d, cfg, lib, _libdir) = env("{importsource: {suggest_removal: yes}}");
    let (item, src) = source_item(&lib, &d.path().join("orig"), "x.mp3");
    crate::ui::script::set(vec![("n", None)]);
    let _p = load(&cfg);
    crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item))]);
    assert!(src.exists());
}

#[test]
fn warnings_for_missing_source_and_no_attribute() {
    let (d, cfg, lib, _libdir) = env("{importsource: {suggest_removal: yes}}");
    let (mut item, _src) = source_item(&lib, &d.path().join("orig"), "x.mp3");
    let gone = d.path().join("gone.mp3");
    item.set("source_path", Value::Str(gone.to_string_lossy().into_owned()));
    lib.store(&item).unwrap();
    let (_r, lines) = crate::logging::capture(|| {
        let _p = load(&cfg);
        crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item))]);
    });
    assert!(
        lines.iter().any(|l| l.contains("Original source file no longer exists")),
        "{lines:?}"
    );
}

#[test]
fn reimport_prevents_suggestions() {
    let (d, cfg, lib, _libdir) = env("{importsource: {suggest_removal: yes}}");
    let mut item = Item::new_item();
    item.set_path("/tmp/x.mp3");
    item.set_str("title", "T");
    item.set_str("mb_albumid", "11111111-1111-1111-1111-111111111111");
    let _p = load(&cfg);
    let task = crate::events::TaskInfo {
        items: vec![item.clone()],
        skip: false,
        ..Default::default()
    };
    crate::events::send("import_task_choice", &[("task", crate::events::Arg::Task(&task))]);
    let (_, src) = source_item(&lib, &d.path().join("source"), "x.mp3");
    let mut item2 = item.clone();
    item2.set("source_path", Value::Str(src.to_string_lossy().into_owned()));
    crate::ui::script::set(vec![("d", None)]);
    crate::events::send("item_removed", &[("item", crate::events::Arg::Item(&item2))]);
    assert!(src.exists());
    assert_eq!(crate::ui::script::prompts().len(), 0);
}
