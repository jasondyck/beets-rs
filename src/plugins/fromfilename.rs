//! Port of beets 2.14's `fromfilename` plugin: when a task's items have
//! empty titles, take the title (and the track number and artist, if the
//! file names have them) from the file names. beets does it on
//! `import_task_start`, so only while autotagging, before the lookup.

use crate::config::Config;
use crate::model::Item;
use crate::plugins::PluginLog;
use fancy_regex::Regex;
use std::collections::HashMap;

/// beets' `PATTERNS`, most specific first.
const PATTERNS: &[&str] = &[
    r"^(?P<track>\d+)\.?\s*-\s*(?P<artist>.+?)\s*-\s*(?P<title>.+?)(\s*-\s*(?P<tag>.*))?$",
    r"^(?P<artist>.+?)\s*-\s*(?P<title>.+?)(\s*-\s*(?P<tag>.*))?$",
    r"^(?P<track>\d+)\.?[\s_-]+(?P<title>.+)$",
    r"^(?P<title>.+) by (?P<artist>.+)$",
    r"^(track ?)?(?P<track>\d+).*$",
    r"^(?P<title>.+)$",
];

fn bad_title(title: &str) -> bool {
    title.is_empty()
}

/// The named groups of each item's match (None for an unmatched group),
/// or None unless every name matches.
type Groups = HashMap<String, Option<String>>;

fn all_matches(names: &[String], pattern: &str) -> Option<Vec<Groups>> {
    let re = Regex::new(&format!("(?i){pattern}")).unwrap();
    let group_names: Vec<String> = re.capture_names().flatten().map(String::from).collect();
    let mut out = vec![];
    for name in names {
        let caps = re.captures(name).ok().flatten()?;
        if group_names.is_empty() {
            return None;
        }
        out.push(
            group_names
                .iter()
                .map(|g| (g.clone(), caps.name(g).map(|m| m.as_str().to_string())))
                .collect(),
        );
    }
    Some(out)
}

fn equal_fields(d: &[Groups], field: &str) -> bool {
    let first = d[0].get(field);
    d.iter().all(|m| m.get(field) == first)
}

fn apply_matches(items: &mut [Item], d: &[Groups], log: &PluginLog) {
    let keys = &d[0];
    if keys.contains_key("tag") && !equal_fields(d, "tag") {
        return;
    }
    let title_field = if keys.contains_key("artist") {
        let (artist, title_field) = if equal_fields(d, "artist") {
            (keys["artist"].clone(), "title")
        } else if equal_fields(d, "title") {
            (keys["title"].clone(), "artist")
        } else {
            return;
        };
        for item in items.iter_mut() {
            if item.str("artist").is_empty() {
                item.set_str("artist", artist.clone().unwrap_or_default());
                log.info(&format!("Artist replaced with: {}", item.str("artist")));
            }
        }
        Some(title_field)
    } else if keys.contains_key("title") {
        Some("title")
    } else {
        None
    };
    for (item, m) in items.iter_mut().zip(d) {
        if let Some(tf) = title_field
            && bad_title(item.str("title"))
        {
            // `str(...)` of the group: "None" when it didn't take part.
            let v = m
                .get(tf)
                .cloned()
                .flatten()
                .unwrap_or_else(|| "None".into());
            item.set_str("title", v);
            log.info(&format!("Title replaced with: {}", item.str("title")));
        }
        if let Some(Some(t)) = m.get("track")
            && item.int("track") == 0
        {
            item.set_int("track", t.parse().unwrap_or(0));
            log.info(&format!("Track replaced with: {}", item.int("track")));
        }
    }
}

/// The file name without directory and extension (`os.path.splitext`).
fn stem(item: &Item) -> String {
    let name = item
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // splitext: the last dot, unless the name is only dots before it.
    match name.rfind('.') {
        Some(k) if name[..k].chars().any(|c| c != '.') => name[..k].to_string(),
        _ => name,
    }
}

/// `filename_task`: the `import_task_start` listener. `log`: log what
/// changes (the importer's pass; the lookup's copy is quiet).
pub fn task_start(cfg: &Config, items: &mut [Item], log: bool) {
    if !crate::plugins::enabled(cfg, "fromfilename") || items.is_empty() {
        return;
    }
    if !items.iter().any(|i| bad_title(i.str("title"))) {
        return;
    }
    let logger = PluginLog::new("fromfilename");
    logger.set_base_level(if log {
        crate::plugins::WARNING
    } else {
        u8::MAX
    });
    let names: Vec<String> = items.iter().map(stem).collect();
    for pattern in PATTERNS {
        logger.debug(&format!("Trying pattern: {pattern}"));
        if let Some(d) = all_matches(&names, pattern) {
            apply_matches(items, &d, &logger);
        }
    }
}

pub struct FromFilenamePlugin;

impl super::Plugin for FromFilenamePlugin {
    fn name(&self) -> &'static str {
        "fromfilename"
    }
}

#[cfg(test)]
mod tests {
    //! Ported from beets' test/plugins/test_fromfilename.py.
    use super::*;

    #[test]
    fn fromfilename() {
        let mut cfg = Config::defaults();
        cfg.set("plugins", serde_yaml::from_str("[fromfilename]").unwrap());
        type Song = (&'static str, i64, &'static str, &'static str);
        let cases: &[(Song, Song)] = &[
            (
                (
                    "/tmp/01 - The Artist - Song One.m4a",
                    1,
                    "The Artist",
                    "Song One",
                ),
                (
                    "/tmp/02. - The Artist - Song Two.m4a",
                    2,
                    "The Artist",
                    "Song Two",
                ),
            ),
            (
                (
                    "/tmp/01-The_Artist-Song_One.m4a",
                    1,
                    "The_Artist",
                    "Song_One",
                ),
                (
                    "/tmp/02.-The_Artist-Song_Two.m4a",
                    2,
                    "The_Artist",
                    "Song_Two",
                ),
            ),
            (
                ("/tmp/01 - Song_One.m4a", 1, "", "Song_One"),
                ("/tmp/02. - Song_Two.m4a", 2, "", "Song_Two"),
            ),
            (
                (
                    "/tmp/Song One by The Artist.m4a",
                    0,
                    "The Artist",
                    "Song One",
                ),
                (
                    "/tmp/Song Two by The Artist.m4a",
                    0,
                    "The Artist",
                    "Song Two",
                ),
            ),
            (("/tmp/01.m4a", 1, "", "01"), ("/tmp/02.m4a", 2, "", "02")),
            (
                ("/tmp/Song One.m4a", 0, "", "Song One"),
                ("/tmp/Song Two.m4a", 0, "", "Song Two"),
            ),
            (
                ("/tmp/track01.m4a", 1, "", "track01"),
                ("/tmp/track 2.m4a", 2, "", "track 2"),
            ),
        ];
        for (s1, s2) in cases {
            let mut items: Vec<Item> = [s1, s2]
                .iter()
                .map(|s| {
                    let mut i = Item::new_item();
                    i.set_path(s.0);
                    i
                })
                .collect();
            task_start(&cfg, &mut items, false);
            for (i, s) in items.iter().zip([s1, s2]) {
                assert_eq!(
                    (i.int("track"), i.str("artist"), i.str("title")),
                    (s.1, s.2, s.3),
                    "{}",
                    s.0
                );
            }
        }
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "fromfilename",
        order: 32,
        defaults: r#""#,
        config_keys: &["fromfilename"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(FromFilenamePlugin))
}
