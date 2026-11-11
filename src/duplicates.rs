//! Duplicate detection helpers for the importer, as in beets:
//! `import.duplicate_keys`, the duplicate actions (including "Upgrade"),
//! and the old/new summaries shown at the duplicate prompt.

use crate::model::{Item, Record};
use crate::template::Template;
use std::collections::HashMap;

/// beets' `DuplicateAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateAction {
    Skip,
    Merge,
    Remove,
    Keep,
    Ask,
    Upgrade,
}

impl DuplicateAction {
    /// The prompt's options (beets' `strict_options`: all but Ask).
    pub const PROMPT: [&'static str; 5] =
        ["Skip new", "Merge all", "Remove old", "Keep all", "Upgrade"];

    /// `import.duplicate_action`; anything unknown asks.
    pub fn from_config(s: &str) -> Self {
        match s {
            "skip" => Self::Skip,
            "merge" => Self::Merge,
            "remove" => Self::Remove,
            "keep" => Self::Keep,
            "upgrade" => Self::Upgrade,
            _ => Self::Ask,
        }
    }

    /// The action for a prompt letter.
    pub fn from_letter(c: char) -> Self {
        match c {
            'm' => Self::Merge,
            'r' => Self::Remove,
            'k' => Self::Keep,
            'u' => Self::Upgrade,
            _ => Self::Skip,
        }
    }
}

/// The values a new album or item has for the duplicate keys: formatted
/// field values, `None` where the field doesn't exist (an unset flexible
/// attribute).
pub type KeyValues = Vec<(String, Option<String>)>;

/// Key values taken from one record.
pub fn key_values(rec: &Record, keys: &[String]) -> KeyValues {
    keys.iter().map(|k| (k.clone(), rec.formatted(k))).collect()
}

/// Key values for an album imported as-is: the most common value among its
/// items for each key (beets' likelies), with `albumartist` taken from the
/// most common track artist, as beets' `find_duplicates` does.
pub fn likely_key_values(items: &[Item], keys: &[String]) -> KeyValues {
    keys.iter()
        .map(|k| {
            let field = if k == "albumartist" { "artist" } else { k };
            (
                k.clone(),
                plurality(items.iter().map(|i| i.formatted(field))),
            )
        })
        .collect()
}

/// The most common value (the first seen among equally common ones).
fn plurality(values: impl Iterator<Item = Option<String>>) -> Option<String> {
    let mut counts: Vec<(Option<String>, usize)> = vec![];
    for v in values {
        match counts.iter_mut().find(|(c, _)| *c == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    let best = counts.iter().map(|(_, n)| *n).max()?;
    counts.into_iter().find(|(_, n)| *n == best)?.0
}

/// Whether `rec` has the same value for every key (beets'
/// `duplicates_query`: an exact match per field).
pub fn matches(rec: &Record, want: &KeyValues) -> bool {
    want.iter().all(|(k, v)| rec.formatted(k) == *v)
}

/// beets' `_item_dup_key`: an item's identity for per-track upgrades.
fn item_key(item: &Item, keys: &[String]) -> Vec<Option<String>> {
    keys.iter().map(|k| item.formatted(k)).collect()
}

/// beets' `resolve_upgrade`: which new items to keep (by index) and which
/// old items they supersede. New items with no old counterpart are kept;
/// ones with a counterpart only if their bitrate beats every old item with
/// the same key, which are then all superseded.
pub fn resolve_upgrade(new: &[Item], old: &[Item], keys: &[String]) -> (Vec<usize>, Vec<Item>) {
    let mut by_key: HashMap<Vec<Option<String>>, Vec<&Item>> = HashMap::new();
    for o in old {
        by_key.entry(item_key(o, keys)).or_default().push(o);
    }
    let mut kept = vec![];
    let mut superseded = vec![];
    for (n, item) in new.iter().enumerate() {
        match by_key.get(&item_key(item, keys)) {
            None => kept.push(n),
            Some(group) => {
                let best_old = group.iter().map(|o| o.int("bitrate")).max().unwrap_or(0);
                if item.int("bitrate") > best_old {
                    kept.push(n);
                    superseded.extend(group.iter().map(|o| (*o).clone()));
                }
            }
        }
    }
    (kept, superseded)
}

/// One duplicate found in the library: its album id (`None` for a
/// singleton) and its items.
pub struct Found {
    pub album_id: Option<i64>,
    pub items: Vec<Item>,
}

/// The outcome of an upgrade against the chosen duplicate.
#[derive(Debug, Default)]
pub struct Upgrade {
    /// Indices of the new items to import.
    pub kept: Vec<usize>,
    /// Old items to remove.
    pub superseded: Vec<Item>,
    /// The old album the kept items join if any of its items survive.
    pub album_id: Option<i64>,
}

/// beets' `resolve_upgrade_target`: resolve the upgrade against each
/// duplicate separately and use the one with the most superseded tracks
/// (then most kept tracks, then lowest album id); the others are left alone.
pub fn resolve_upgrade_target(new: &[Item], found: &[Found], keys: &[String]) -> Upgrade {
    let mut best: Option<((usize, usize, i64), Upgrade)> = None;
    for dup in found {
        let (kept, superseded) = resolve_upgrade(new, &dup.items, keys);
        let rank = (superseded.len(), kept.len(), -dup.album_id.unwrap_or(0));
        if best.as_ref().is_none_or(|(r, _)| rank > *r) {
            best = Some((
                rank,
                Upgrade {
                    kept,
                    superseded,
                    album_id: dup.album_id,
                },
            ));
        }
    }
    best.map(|(_, u)| u).unwrap_or_default()
}

/// beets' `human_bytes`.
fn human_bytes(size: u64) -> String {
    let mut size = size as f64;
    let mut unit = "B";
    for power in ["", "K", "M", "G", "T", "P", "E", "Z", "Y", "H"] {
        if size < 1024.0 {
            return format!("{size:.1} {power}{unit}");
        }
        size /= 1024.0;
        unit = "iB";
    }
    "big".into()
}

/// beets' `summarize_items`: "N items, FORMAT, NNNkbps, M:SS, SIZE" for the
/// duplicate prompt (no item count for a single track).
pub fn summarize_items(items: &[Item], singleton: bool) -> String {
    let mut parts = vec![];
    if !singleton {
        parts.push(format!("{} items", items.len()));
    }
    let mut counts: Vec<(String, usize)> = vec![];
    for i in items {
        let f = i.str("format").to_string();
        match counts.iter_mut().find(|(c, _)| *c == f) {
            Some((_, n)) => *n += 1,
            None => counts.push((f, 1)),
        }
    }
    if counts.len() == 1 {
        parts.push(counts.remove(0).0);
    } else {
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        parts.extend(counts.into_iter().map(|(f, n)| format!("{f} {n}")));
    }
    if let Some(first) = items.first() {
        let n = items.len() as f64;
        let bitrate = items.iter().map(|i| i.int("bitrate") as f64).sum::<f64>() / n;
        parts.push(format!("{}kbps", (bitrate / 1000.0) as i64));
        if first.str("format") == "FLAC" {
            let khz = first.int("samplerate") as f64 / 1000.0;
            parts.push(format!("{khz:.1}kHz/{} bit", first.int("bitdepth")));
        }
        let secs = items.iter().map(|i| i.float("length")).sum::<f64>() as i64;
        parts.push(format!("{}:{:02}", secs / 60, secs % 60));
        let size: u64 = items
            .iter()
            .map(|i| std::fs::metadata(i.path()).map(|m| m.len()).unwrap_or(0))
            .sum();
        parts.push(human_bytes(size));
    }
    parts.join(", ")
}

/// beets' `_report_item_summary`: "Old: <summary>" (or "New: ..."), then
/// with `duplicate_verbose_prompt` each item in the `format_item` format.
pub fn report(
    prefix: &str,
    items: &[Item],
    singleton: bool,
    verbose: Option<&Template>,
) -> Vec<String> {
    let mut lines = vec![format!("{prefix}: {}", summarize_items(items, singleton))];
    if let Some(fmt) = verbose {
        lines.extend(
            items
                .iter()
                .map(|i| format!("  {}", crate::commands::format_record(fmt, i, None))),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Value;

    fn item(artist: &str, title: &str, bitrate: i64) -> Item {
        let mut i = Item::new_item();
        i.set_str("artist", artist);
        i.set_str("title", title);
        i.set_int("bitrate", bitrate);
        i.set_str("format", "MP3");
        i.set("length", Value::Float(61.5));
        i
    }

    fn keys(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn keys_match_exactly_including_flex_fields() {
        let a = item("A", "T", 0);
        let want = key_values(&a, &keys("artist title"));
        assert!(matches(&item("A", "T", 1), &want));
        assert!(!matches(&item("a", "T", 1), &want));
        // A flex key: unset on both sides matches, set on one doesn't.
        let k = keys("artist mood");
        let want = key_values(&a, &k);
        assert!(matches(&item("A", "X", 0), &want));
        let mut b = item("A", "X", 0);
        b.set_str("mood", "sad");
        assert!(!matches(&b, &want));
        assert!(matches(&b, &key_values(&b, &k)));
    }

    #[test]
    fn as_is_albums_use_the_most_common_values_and_track_artist() {
        let mut items = vec![item("X", "1", 0), item("Y", "2", 0), item("Y", "3", 0)];
        for i in &mut items {
            i.set_str("album", "Alb");
            i.set_str("albumartist", "Z");
        }
        let kv = likely_key_values(&items, &keys("albumartist album"));
        assert_eq!(
            kv,
            [
                ("albumartist".into(), Some("Y".into())),
                ("album".into(), Some("Alb".into()))
            ]
        );
    }

    #[test]
    fn upgrade_keeps_better_and_unmatched_tracks() {
        let k = keys("artist title");
        let old = vec![item("A", "1", 128_000), item("A", "2", 320_000)];
        let new = vec![
            item("A", "1", 256_000),
            item("A", "2", 256_000),
            item("A", "3", 96_000),
        ];
        let (kept, sup) = resolve_upgrade(&new, &old, &k);
        assert_eq!(kept, [0, 2]);
        assert_eq!(sup.len(), 1);
        assert_eq!(sup[0].str("title"), "1");
    }

    #[test]
    fn upgrade_target_is_the_album_with_most_superseded_tracks() {
        let k = keys("artist title");
        let new = vec![item("A", "1", 320_000), item("A", "2", 320_000)];
        let found = [
            Found {
                album_id: Some(1),
                items: vec![item("A", "1", 128_000)],
            },
            Found {
                album_id: Some(2),
                items: vec![item("A", "1", 128_000), item("A", "2", 128_000)],
            },
            Found {
                album_id: Some(3),
                items: vec![item("A", "1", 128_000), item("A", "2", 128_000)],
            },
        ];
        let u = resolve_upgrade_target(&new, &found, &k);
        // Tie between 2 and 3 goes to the lower id.
        assert_eq!(
            (u.album_id, u.kept.len(), u.superseded.len()),
            (Some(2), 2, 2)
        );
        // Nothing better: nothing kept.
        let worse = vec![item("A", "1", 64_000)];
        assert!(resolve_upgrade_target(&worse, &found, &k).kept.is_empty());
    }

    #[test]
    fn summaries_match_beets() {
        let items = vec![item("A", "1", 128_000), item("A", "2", 192_000)];
        assert_eq!(
            summarize_items(&items, false),
            "2 items, MP3, 160kbps, 2:03, 0.0 B"
        );
        assert_eq!(
            summarize_items(&items[..1], true),
            "MP3, 128kbps, 1:01, 0.0 B"
        );
        let mut flac = item("A", "3", 900_000);
        flac.set_str("format", "FLAC");
        flac.set_int("samplerate", 44100);
        flac.set_int("bitdepth", 16);
        let mixed = vec![flac.clone(), items[0].clone(), items[1].clone()];
        assert!(summarize_items(&mixed, false).starts_with("3 items, MP3 2, FLAC 1, "));
        assert!(summarize_items(&[flac], true).contains("44.1kHz/16 bit"));
        assert_eq!(human_bytes(1536), "1.5 KiB");
        // The verbose prompt lists every item.
        assert_eq!(report("Old", &items, false, None).len(), 1);
        let fmt = Template::parse("$artist - $title");
        assert_eq!(
            report("New", &items, false, Some(&fmt)),
            [
                "New: 2 items, MP3, 160kbps, 2:03, 0.0 B",
                "  A - 1",
                "  A - 2"
            ]
        );
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
    }
}
