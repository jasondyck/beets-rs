//! Ported from beets' test/plugins/test_mpdstats.py (with a mock client in
//! place of beets' patched `MPDClientWrapper`).

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Value};

struct Mock {
    /// statuses, one per `status` call.
    statuses: Vec<BTreeMap<String, String>>,
    /// events, one per `events` call.
    events: Vec<Vec<String>>,
    currentsong: (Option<String>, Option<String>),
}

impl Client for Mock {
    fn currentsong(&mut self) -> (Option<String>, Option<String>) {
        self.currentsong.clone()
    }
    fn status(&mut self) -> BTreeMap<String, String> {
        if self.statuses.is_empty() {
            return BTreeMap::new();
        }
        self.statuses.remove(0)
    }
    fn events(&mut self) -> Vec<String> {
        if self.events.is_empty() {
            return vec![];
        }
        self.events.remove(0)
    }
}

fn status(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn item(path: &str, title: &str) -> Item {
    let mut i = Item::new_item();
    i.set_path(path);
    i.set_str("title", title);
    i
}

fn env() -> (Config, Library) {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mpdstats]").unwrap());
    let lib = Library::in_memory().unwrap();
    (cfg, lib)
}

fn mock(statuses: Vec<BTreeMap<String, String>>, events: Vec<Vec<String>>) -> Mock {
    Mock {
        statuses,
        events,
        currentsong: (None, None),
    }
}

#[test]
fn update_rating_ignores_missing_items() {
    let (cfg, lib) = env();
    let stats = MPDStats::new(&lib, PluginLog::new("mpdstats"), mock(vec![], vec![]), &cfg);
    stats.update_rating(&None, true).unwrap();
    assert!(stats.now_playing.is_none());
}

#[test]
fn update_rating_uses_play_and_skip_counts() {
    let (cfg, lib) = env();
    let mut it = item("/foo/bar.flac", "title");
    lib.add(&mut it).unwrap();
    let stats = MPDStats::new(&lib, PluginLog::new("mpdstats"), mock(vec![], vec![]), &cfg);
    let before = stats.get_item("/foo/bar.flac").unwrap();
    assert_eq!(before.str("title"), "title");
    stats.update_rating(&Some(it.clone()), true).unwrap();
    let after = lib.get_item(it.id.unwrap()).unwrap().unwrap();
    let rating = after.str("rating").parse::<f64>().unwrap();
    // rating 0.5, skipped: rolling 0.25, stable 0.5 -> 0.4375
    assert!((rating - 0.4375).abs() < 1e-9, "{rating}");
}

#[test]
fn get_item_normalizes_paths_and_logs_misses() {
    let (cfg, lib) = env();
    let mut it = item("/foo/bar.flac", "title");
    lib.add(&mut it).unwrap();
    let stats = MPDStats::new(&lib, PluginLog::new("mpdstats"), mock(vec![], vec![]), &cfg);
    assert!(stats.get_item("/foo/bar.flac").is_some());
    let (_, lines) = crate::logging::capture(|| {
        assert!(stats.get_item("/some/non-existing/path").is_none());
    });
    assert!(
        lines.iter().any(|l| l.contains("item not found:")),
        "{lines:?}"
    );
}

#[test]
fn play_pause_stop_update_the_statistics() {
    let (cfg, lib) = env();
    let mut it = item("/foo/bar.flac", "title");
    it.set_str("mb_trackid", "");
    lib.add(&mut it).unwrap();
    let m = Mock {
        statuses: vec![
            status(&[("state", "pause")]),
            status(&[("state", "play"), ("songid", "1"), ("time", "0:1")]),
            status(&[("state", "stop")]),
        ],
        events: vec![vec!["player".into()], vec!["player".into()], vec!["player".into()]],
        currentsong: (Some("/foo/bar.flac".into()), Some("1".into())),
    };
    let mut stats = MPDStats::new(&lib, PluginLog::new("mpdstats"), m, &cfg);
    let (_, lines) = crate::logging::capture(|| {
        // As `run` does, one status per "player" event.
        let s = stats.mpd.status();
        stats.on_pause(&s).unwrap();
        let s = stats.mpd.status();
        stats.on_play(&s).unwrap();
        let s = stats.mpd.status();
        stats.on_stop(&s).unwrap();
        let _ = stats.mpd.events();
    });
    assert!(lines.iter().any(|l| l == "mpdstats: pause"), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("playing /foo/bar.flac")), "{lines:?}");
    assert!(lines.iter().any(|l| l == "mpdstats: stop"), "{lines:?}");
}

#[test]
fn command_options_override_the_settings() {
    let cfg = Config::defaults();
    let cfg = config_with_overrides(&cfg, Some("somehost"), Some(5000), Some("secret"));
    assert_eq!(cfg.str("mpd.host"), "somehost");
    assert_eq!(cfg.f64("mpd.port"), 5000.0);
    assert_eq!(cfg.str("mpd.password"), "secret");
}

#[test]
fn a_rating_is_computed_from_counts() {
    let (cfg, lib) = env();
    let stats = MPDStats::new(&lib, PluginLog::new("mpdstats"), mock(vec![], vec![]), &cfg);
    // (play+1)/(play+skip+2) with mix 0.75.
    let r = stats.rating(4, 0, 0.5, false);
    assert!((r - (0.75 * 5.0 / 6.0 + 0.25 * 0.75)).abs() < 1e-9, "{r}");
    let r = stats.rating(0, 3, 0.5, true);
    assert!((r - (0.75 * 1.0 / 5.0 + 0.25 * 0.25)).abs() < 1e-9, "{r}");
    let _ = Value::Float(0.0);
}
