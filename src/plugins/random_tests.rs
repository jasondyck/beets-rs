//! beets' `test/plugins/test_random.py`, adapted: the seeded stream is this
//! port's own, so the statistical checks use fixed seeds.

use super::*;
use crate::model::{Item, Value};
use serde_yaml::Value as Y;
use tempfile::TempDir;

fn item(artist: &str, length: f64) -> Record {
    let mut it = Item::new_item();
    it.set("artist", Value::Str(artist.into()));
    it.set("length", Value::Float(length));
    it
}

fn library() -> (TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
    (dir, lib)
}

fn stats(data: &[f64]) -> (f64, f64, f64) {
    let mean = data.iter().sum::<f64>() / data.len() as f64;
    let stdev =
        (data.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / (data.len() - 1) as f64).sqrt();
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (q, r) = (data.len() / 2, data.len() % 2);
    let median = if r == 1 {
        sorted[q]
    } else {
        (sorted[q - 1] + sorted[q]) / 2.0
    };
    (mean, stdev, median)
}

/// The upstream statistical test: one artist-1 item among eight artist-2
/// items ends up near the start when balanced by artist, in the middle when
/// balanced by track.
#[test]
fn equal_permutation_is_field_balanced() {
    let mut items = vec![item("Artist 1", 180.0), item("Artist 2", 240.0)];
    for _ in 0..8 {
        items.push(item("Artist 2", 240.0));
    }
    let experiment = |field: &str, seed: u64| {
        let mut rng = Rng::from_seed(seed);
        let mut positions = vec![];
        for _ in 0..500 {
            let mut shuffled = equal_chance_permutation(items.clone(), field, &mut rng);
            // Track the artist-1 item (the first one).
            let pos = shuffled
                .iter()
                .position(|o| o.str("artist") == "Artist 1")
                .unwrap();
            positions.push(pos as f64);
            shuffled.clear();
        }
        stats(&positions)
    };
    let (_, stdev1, median1) = experiment("artist", 12345);
    let (_, stdev2, median2) = experiment("track", 12345);
    assert!((median1 - 0.0).abs() <= 1.0, "median1 {median1}");
    assert!((median2 - 5.0).abs() <= 1.0, "median2 {median2}");
    assert!(stdev2 > stdev1, "{stdev2} > {stdev1}");
}

#[test]
fn equal_permutation_edges() {
    let mut rng = Rng::from_seed(1);
    assert!(equal_chance_permutation(vec![], "artist", &mut rng).is_empty());
    let one = equal_chance_permutation(vec![item("Artist 1", 1.0)], "artist", &mut rng);
    assert_eq!(one.len(), 1);
    // A missing field drops every item.
    assert!(
        equal_chance_permutation(vec![item("Artist 1", 1.0)], "nonexistent", &mut rng).is_empty()
    );
    // Same-value items all come back.
    let same = equal_chance_permutation(
        vec![item("Artist 1", 1.0), item("Artist 1", 1.0)],
        "artist",
        &mut rng,
    );
    assert_eq!(same.len(), 2);
}

#[test]
fn random_selection_by_count() {
    let items = vec![
        item("Artist 1", 180.0),
        item("Artist 2", 240.0),
        item("Artist 2", 300.0),
    ];
    let mut rng = Rng::from_seed(2);
    let selected = random_objs(items.clone(), "artist", 2, None, false, &mut rng);
    assert_eq!(selected.len(), 2);
    assert_eq!(
        random_objs(vec![], "artist", 1, None, false, &mut rng).len(),
        0
    );
    let all = random_objs(items.clone(), "artist", 3, None, false, &mut rng);
    assert_eq!(all.len(), 3);
}

#[test]
fn random_selection_by_time() {
    let items = vec![
        item("Artist 1", 180.0),
        item("Artist 2", 240.0),
        item("Artist 2", 300.0),
    ];
    let mut rng = Rng::from_seed(3);
    for _ in 0..20 {
        let selected = random_objs(items.clone(), "artist", 1, Some(6.0), false, &mut rng);
        let total: f64 = selected.iter().map(|o| o.float("length") / 60.0).sum();
        assert!(total <= 6.0 + 1e-9, "{total}");
    }
}

#[test]
fn equal_chance_selection_keeps_both_artists() {
    let mut items = vec![item("Artist 1", 180.0), item("Artist 2", 240.0)];
    for _ in 0..5 {
        items.push(item("Artist 1", 180.0));
    }
    let mut rng = Rng::from_seed(4);
    let selected = random_objs(items, "artist", 10, None, true, &mut rng);
    let mut artists: Vec<&str> = selected.iter().map(|o| o.str("artist")).collect();
    artists.sort_unstable();
    artists.dedup();
    assert!(artists.len() >= 2);
}

/// `beet random` prints one line per chosen object.
#[test]
fn command_prints_chosen_objects() {
    let (_dir, lib) = library();
    let mut its: Vec<Item> = (1..=3)
        .map(|n| {
            let mut it = Item::new_item();
            it.set("title", Value::Str(format!("T{n}")));
            it
        })
        .collect();
    lib.add_album(&mut its).unwrap();
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[random]").unwrap());
    cfg.set("format_item", Y::String("$title".into()));
    let opts = CmdOpts {
        number: 2,
        field: "albumartist".into(),
        ..Default::default()
    };
    // The selection itself is random; two of the three titles show up.
    let mut picks = std::collections::HashSet::new();
    for _ in 0..50 {
        let mut out = vec![];
        run(&cfg, &lib, &opts, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().count(), 2);
        picks.extend(out.lines().map(String::from));
        if picks.len() == 3 {
            break;
        }
    }
    assert_eq!(picks.len(), 3, "{picks:?}");
}
