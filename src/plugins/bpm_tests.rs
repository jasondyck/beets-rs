//! Tests for the `bpm` plugin: the tap average (beets' `bpm()`) and the
//! command's item update.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use serde_yaml::Value as Y;
use tempfile::TempDir;

fn strokes(v: &[(&str, f64)]) -> std::vec::IntoIter<Stroke> {
    v.iter()
        .map(|(s, t)| ((*s).to_string(), *t))
        .collect::<Vec<_>>()
        .into_iter()
}

#[test]
fn averages_the_intervals() {
    // 0.5 s between keystrokes: 120 bpm.
    assert_eq!(bpm(3, &mut strokes(&[("", 0.0), ("", 0.5), ("", 1.0)])), 120.0);
    // Mixed intervals average their rates: 60 and 120 bpm → 90 bpm.
    assert_eq!(bpm(3, &mut strokes(&[("", 0.0), ("", 1.0), ("", 1.5)])), 90.0);
}

#[test]
fn a_nonempty_line_breaks_off() {
    assert_eq!(
        bpm(5, &mut strokes(&[("", 0.0), ("", 0.5), ("x", 0.75), ("", 1.0)])),
        120.0
    );
}

#[test]
fn stops_at_max_strokes() {
    // Only two strokes are counted, so one interval.
    assert_eq!(
        bpm(2, &mut strokes(&[("", 0.0), ("", 0.25), ("", 0.5)])),
        240.0
    );
}

#[test]
fn end_of_input_ends_the_measure() {
    assert_eq!(bpm(3, &mut strokes(&[("", 0.0), ("", 0.4)])), 150.0);
}

fn env() -> (TempDir, Config, Library) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    // Tags are not written: the item's file doesn't exist.
    cfg.set("import.write", Y::Bool(false));
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib)
}

fn add(lib: &Library, bpm_value: Option<i64>) -> Item {
    let mut it = Item::new_item();
    it.set_str("title", "Song");
    if let Some(b) = bpm_value {
        it.set_int("bpm", b);
    }
    lib.add(&mut it).unwrap();
    it
}

fn tap(v: &[f64]) -> impl Iterator<Item = Stroke> {
    strokes(&v.iter().map(|t| ("", *t)).collect::<Vec<_>>())
}

#[test]
fn sets_the_measured_bpm() {
    let (_d, cfg, lib) = env();
    let mut item = add(&lib, None);
    get_bpm(&cfg, &lib, std::slice::from_mut(&mut item), false, &mut tap(&[0.0, 0.5, 1.0])).unwrap();
    assert_eq!(item.int("bpm"), 120);
    let stored = lib.get_item(item.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.int("bpm"), 120);
}

#[test]
fn truncates_like_int() {
    let (_d, cfg, lib) = env();
    let mut item = add(&lib, None);
    // 0.51 s intervals: 117.6 bpm → 117.
    get_bpm(&cfg, &lib, std::slice::from_mut(&mut item), false, &mut tap(&[0.0, 0.51, 1.02])).unwrap();
    assert_eq!(item.int("bpm"), 117);
}

/// `bpm.overwrite` defaults to yes, as beets' `overwrite: True`.
#[test]
fn overwrites_by_default() {
    let (_d, cfg, lib) = env();
    let mut item = add(&lib, Some(99));
    get_bpm(&cfg, &lib, std::slice::from_mut(&mut item), false, &mut tap(&[0.0, 0.5, 1.0])).unwrap();
    assert_eq!(item.int("bpm"), 120);
    let stored = lib.get_item(item.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.int("bpm"), 120);
}

#[test]
fn keeps_the_old_bpm_without_overwrite() {
    let (_d, mut cfg, lib) = env();
    cfg.set("bpm.overwrite", Y::Bool(false));
    let mut item = add(&lib, Some(99));
    get_bpm(&cfg, &lib, std::slice::from_mut(&mut item), false, &mut tap(&[0.0, 0.5, 1.0])).unwrap();
    assert_eq!(item.int("bpm"), 99);
    let stored = lib.get_item(item.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.int("bpm"), 99);
}

#[test]
fn more_than_one_item_is_an_error() {
    let (_d, cfg, lib) = env();
    let mut items = vec![add(&lib, None), add(&lib, None)];
    let err = get_bpm(&cfg, &lib, &mut items, false, &mut tap(&[0.0, 0.5]))
        .unwrap_err()
        .to_string();
    assert_eq!(err, "Can only get bpm of one song at time");
}
