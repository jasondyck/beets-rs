//! Ported from beets' `test/plugins/test_mbsubmit.py`, plus the command.

use super::*;
use crate::autotag::Recommendation;
use crate::model::Value;
use crate::plugins::{ChoiceTask, Plugin};
use serde_yaml::Value as Y;
use tempfile::TempDir;

fn cfg() -> Config {
    Config::defaults()
}

fn item(disc: i64, track: i64, title: &str) -> Item {
    let mut it = Item::new_item();
    it.set_int("disc", disc);
    it.set_int("track", track);
    it.set_str("title", title);
    it.set_str("artist", "A");
    it.set("length", Value::Float(1.0));
    it
}

fn render(plugin: &MBSubmitPlugin, items: &[Item]) -> String {
    let mut out = vec![];
    plugin.print_tracks(items, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// Upstream's `_overlapping_multidisc_items`.
fn overlapping_multidisc_items() -> Vec<Item> {
    vec![
        item(2, 1, "D2T1"),
        item(1, 2, "D1T2"),
        item(2, 2, "D2T2"),
        item(1, 1, "D1T1"),
    ]
}

#[test]
fn sorts_by_disc_then_track() {
    let plugin = MBSubmitPlugin::new(&cfg()).unwrap();
    assert_eq!(
        render(&plugin, &overlapping_multidisc_items()),
        "01. D1T1 - A (0:01)\n\
         02. D1T2 - A (0:01)\n\
         01. D2T1 - A (0:01)\n\
         02. D2T2 - A (0:01)\n"
    );
}

#[test]
fn format_is_configurable() {
    let mut cfg = cfg();
    cfg.set("mbsubmit.format", Y::String("$title|$track".into()));
    let plugin = MBSubmitPlugin::new(&cfg).unwrap();
    assert_eq!(
        render(&plugin, &[item(1, 2, "Two")]),
        "Two|02\n"
    );
}

#[test]
fn threshold_must_be_a_recommendation() {
    let mut cfg = cfg();
    cfg.set("mbsubmit.threshold", Y::String("great".into()));
    assert_eq!(
        MBSubmitPlugin::new(&cfg)
            .err()
            .expect("threshold is invalid")
            .to_string(),
        "mbsubmit.threshold: must be one of [\"none\", \"low\", \"medium\", \"strong\"], not \"great\""
    );
}

fn choices(plugin: &MBSubmitPlugin, rec: Option<Recommendation>) -> Vec<char> {
    let cfg = cfg();
    let mut items = vec![item(1, 1, "T")];
    let noop = |_: usize, _: &mut Vec<Item>| {};
    let task = ChoiceTask {
        cfg: &cfg,
        is_album: true,
        items: &mut items,
        paths: &[],
        rec,
        candidates: 1,
        selected: Some(0),
        apply_candidate: &noop,
    };
    plugin
        .prompt_choices(&task)
        .into_iter()
        .map(|c| c.short)
        .collect()
}

#[test]
fn choices_offered_up_to_the_threshold() {
    let plugin = MBSubmitPlugin::new(&cfg()).unwrap();
    // The default threshold is medium.
    assert_eq!(choices(&plugin, Some(Recommendation::None)), vec!['p', 'o']);
    assert_eq!(choices(&plugin, Some(Recommendation::Medium)), vec!['p', 'o']);
    assert!(choices(&plugin, Some(Recommendation::Strong)).is_empty());
    // "No match at all" offers nothing (task.rec is None).
    assert!(choices(&plugin, None).is_empty());
}

#[test]
fn threshold_config_shifts_the_choices() {
    let mut cfg = cfg();
    cfg.set("mbsubmit.threshold", Y::String("none".into()));
    let plugin = MBSubmitPlugin::new(&cfg).unwrap();
    assert_eq!(choices(&plugin, Some(Recommendation::None)), vec!['p', 'o']);
    assert!(choices(&plugin, Some(Recommendation::Low)).is_empty());
}

fn library() -> (TempDir, Config, Library) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib)
}

fn add(lib: &Library, artist: &str, track: i64) -> Item {
    let mut it = item(1, track, &format!("Track {track}"));
    it.set_str("artist", artist);
    lib.add(&mut it).unwrap();
    it
}

#[test]
fn command_prints_the_matching_tracks() {
    let (_d, cfg, lib) = library();
    add(&lib, "A", 1);
    add(&lib, "B", 2);
    let plugin = MBSubmitPlugin::new(&cfg).unwrap();
    let items = lib
        .items(&Query::parse(Kind::Item, &["artist:B"]).unwrap())
        .unwrap();
    assert_eq!(render(&plugin, &items), "02. Track 2 - B (0:01)\n");
}
