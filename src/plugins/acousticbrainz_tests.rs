//! Ported from beets' test/plugins/test_acousticbrainz.py.

use super::*;
use crate::config::Config;
use crate::library::Library;
use serde_json::json;

fn mapping(data: J) -> std::collections::BTreeSet<(String, String)> {
    let mut warn = |_: &str| {};
    map_data_to_scheme(&data, scheme(), &mut warn)
        .into_iter()
        .map(|(k, v)| {
            let v = match v {
                J::String(s) => s,
                other => other.to_string(),
            };
            (k, v)
        })
        .collect()
}

fn expected(pairs: &[(&str, &str)]) -> std::collections::BTreeSet<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn basic() {
    let data = json!({"key 1": "value 1", "key 2": "value 2"});
    let scheme: &[(&str, Node)] = &[("key 1", Field("attribute 1")), ("key 2", Field("attribute 2"))];
    let mut warn = |_: &str| {};
    let got: std::collections::BTreeSet<_> = map_data_to_scheme(&data, scheme, &mut warn)
        .into_iter()
        .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
        .collect();
    assert_eq!(got, expected(&[("attribute 1", "value 1"), ("attribute 2", "value 2")]));
}

#[test]
fn recurse() {
    let data = json!({"key": "value", "group": {"subkey": "subvalue", "subgroup": {"subsubkey": "subsubvalue"}}});
    let scheme: &[(&str, Node)] = &[
        ("key", Field("attribute 1")),
        (
            "group",
            Group(&[
                ("subkey", Field("attribute 2")),
                ("subgroup", Group(&[("subsubkey", Field("attribute 3"))])),
            ]),
        ),
    ];
    let mut warn = |_: &str| {};
    let got: std::collections::BTreeSet<_> = map_data_to_scheme(&data, scheme, &mut warn)
        .into_iter()
        .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        got,
        expected(&[
            ("attribute 1", "value"),
            ("attribute 2", "subvalue"),
            ("attribute 3", "subsubvalue")
        ])
    );
}

#[test]
fn composite() {
    let data = json!({"key 1": "part 1", "key 2": "part 2"});
    let scheme: &[(&str, Node)] = &[
        ("key 1", Composite("attribute", 0)),
        ("key 2", Composite("attribute", 1)),
    ];
    let mut warn = |_: &str| {};
    let got: std::collections::BTreeSet<_> = map_data_to_scheme(&data, scheme, &mut warn)
        .into_iter()
        .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
        .collect();
    assert_eq!(got, expected(&[("attribute", "part 1 part 2")]));
}

#[test]
fn realistic() {
    let data = json!({
        "tonal": {
            "chords_key": "A",
            "chords_scale": "minor",
            "chords_changes_rate": 0.0445116683841,
            "chords_number_rate": 0.00194468453992,
            "key_key": "A",
            "key_scale": "minor",
            "key_strength": 0.636936545372
        },
        "rhythm": {"bpm": 162.532119751},
        "lowlevel": {"average_loudness": 0.815025985241},
        "highlevel": {
            "gender": {"value": "female"},
            "genre_rosamerica": {"value": "roc"},
            "danceability": {"all": {"danceable": 0.143928021193}},
            "mood_acoustic": {"all": {"acoustic": 0.415711194277}},
            "mood_aggressive": {"all": {"aggressive": 0.0779221653938}},
            "mood_electronic": {"all": {"electronic": 0.339881360531}},
            "mood_happy": {"all": {"happy": 0.0894767045975}},
            "mood_party": {"all": {"party": 0.234383180737}},
            "mood_relaxed": {"all": {"relaxed": 0.123632438481}},
            "mood_sad": {"all": {"sad": 0.299694597721}},
            "moods_mirex": {"value": "Cluster3"},
            "ismir04_rhythm": {"value": "VienneseWaltz"},
            "tonal_atonal": {"all": {"tonal": 0.874250173569}},
            "timbre": {"value": "bright"},
            "voice_instrumental": {"value": "instrumental"}
        }
    });
    let got = mapping(data);
    assert_eq!(
        got,
        expected(&[
            ("chords_key", "A"),
            ("average_loudness", "0.815025985241"),
            ("mood_acoustic", "0.415711194277"),
            ("chords_changes_rate", "0.0445116683841"),
            ("tonal", "0.874250173569"),
            ("mood_sad", "0.299694597721"),
            ("bpm", "162.532119751"),
            ("gender", "female"),
            ("initial_key", "A minor"),
            ("chords_number_rate", "0.00194468453992"),
            ("mood_relaxed", "0.123632438481"),
            ("chords_scale", "minor"),
            ("voice_instrumental", "instrumental"),
            ("key_strength", "0.636936545372"),
            ("genre_rosamerica", "roc"),
            ("mood_party", "0.234383180737"),
            ("mood_aggressive", "0.0779221653938"),
            ("danceable", "0.143928021193"),
            ("rhythm", "VienneseWaltz"),
            ("mood_electronic", "0.339881360531"),
            ("mood_happy", "0.0894767045975"),
            ("moods_mirex", "Cluster3"),
            ("timbre", "bright"),
        ])
    );
}

fn env(base_url: &str) -> (Config, Library) {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[acousticbrainz]").unwrap());
    cfg.set("acousticbrainz.base_url", base_url.into());
    let lib = Library::in_memory().unwrap();
    (cfg, lib)
}

#[test]
fn fetch_stores_attributes_from_the_two_levels() {
    let server = crate::plugins::requests::testserver::serve(|seen| {
        if seen.target.ends_with("/low-level") {
            crate::plugins::requests::testserver::Reply::json(json!({
                "lowlevel": {"average_loudness": 0.5},
                "rhythm": {"bpm": 120.0},
            }))
        } else {
            crate::plugins::requests::testserver::Reply::json(json!({
                "highlevel": {"gender": {"value": "female"}},
            }))
        }
    });
    let (cfg, lib) = env(&server.base);
    let mut item = crate::model::Item::new_item();
    item.set_path("/tmp/x.mp3");
    item.set_str("mb_trackid", "11111111-1111-1111-1111-111111111111");
    lib.add(&mut item).unwrap();
    command(&cfg, &lib, false, None, &[]).unwrap();
    let got = lib.get_item(item.id.unwrap()).unwrap().unwrap();
    assert_eq!(got.str("gender"), "female");
    assert_eq!(got.get("bpm").and_then(|v| v.as_f64()), Some(120.0));
    assert_eq!(got.get("average_loudness").and_then(|v| v.as_f64()), Some(0.5));
}

#[test]
fn not_found_is_logged() {
    let server = crate::plugins::requests::testserver::serve(|_| {
        crate::plugins::requests::testserver::Reply::status(404)
    });
    let (cfg, lib) = env(&server.base);
    let mut item = crate::model::Item::new_item();
    item.set_path("/tmp/x.mp3");
    item.set_str("mb_trackid", "11111111-1111-1111-1111-111111111111");
    lib.add(&mut item).unwrap();
    let (_r, lines) = crate::logging::capture(|| {
        command(&cfg, &lib, false, None, &[]).unwrap();
    });
    assert!(
        lines.iter().any(|l| l.contains("recording ID 11111111-1111-1111-1111-111111111111 not found")),
        "{lines:?}"
    );
}

#[test]
fn a_base_url_needs_an_http_scheme() {
    let (cfg, _lib) = env("ftp://example.org");
    assert!(load(&cfg).is_err());
}

#[test]
fn float_fields_format_with_six_digits() {
    let (cfg, _lib) = env("http://127.0.0.1:9");
    crate::plugins::types::configure(&cfg).unwrap();
    let mut item = crate::model::Item::new_item();
    item.set_parsed("average_loudness", "0.5").unwrap();
    assert_eq!(
        item.formatted("average_loudness").as_deref(),
        Some("0.500000")
    );
    // The plugin's declared type, from its registry entry.
    assert_eq!(
        crate::model::flex_type(crate::model::Kind::Item, "average_loudness"),
        Some(crate::model::FieldType::Float(6))
    );
}

