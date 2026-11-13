//! Ported from beets' test/plugins/test_mbpseudo.py (the release
//! interception and custom tags; the `PseudoAlbumInfo` view has no direct
//! counterpart: the merged album info plays its role).

use super::*;
use crate::mb::{ParseOpts, TrackInfo, add_custom_tags, wanted_pseudo_release};

fn official_release() -> serde_json::Value {
    serde_json::json!({
        "id": "official",
        "title": "百花繚乱",
        "status": "Official",
        "text-representation": {"script": "Jpan", "language": "ja"},
        "relations": [{
            "type": "transl-tracklisting",
            "direction": "forward",
            "target-type": "release",
            "release": {"id": "pseudo", "text-representation": {"script": "Latn", "language": "en"}}
        }]
    })
}

#[test]
fn intercepts_a_wanted_script() {
    let release = official_release();
    assert_eq!(
        wanted_pseudo_release(&release, &["Latn".to_string()]),
        Some("pseudo".to_string())
    );
}

#[test]
fn does_not_intercept_a_desired_script_release() {
    let mut release = official_release();
    release["text-representation"]["script"] = serde_json::json!("Latn");
    assert_eq!(wanted_pseudo_release(&release, &["Latn".to_string()]), None);
}

#[test]
fn interception_skips_when_relation_values_dont_match() {
    for key in ["type", "direction"] {
        let mut release = official_release();
        release["relations"][0][key] = serde_json::json!("other");
        assert_eq!(wanted_pseudo_release(&release, &["Latn".to_string()]), None);
    }
    let mut release = official_release();
    release["relations"][0]["release"]["text-representation"]["script"] =
        serde_json::json!("Null");
    assert_eq!(wanted_pseudo_release(&release, &["Latn".to_string()]), None);
}

#[test]
fn no_scripts_means_no_interception() {
    assert_eq!(wanted_pseudo_release(&official_release(), &[]), None);
}

#[test]
fn custom_tags_map_pseudo_fields() {
    let mut official = crate::mb::AlbumInfo {
        tracks: vec![TrackInfo::default()],
        album: "百花繚乱".into(),
        artist: "Art".into(),
        ..Default::default()
    };
    let pseudo = crate::mb::AlbumInfo {
        tracks: vec![TrackInfo {
            title: "In Bloom".into(),
            artist: "Pseudo Art".into(),
            ..Default::default()
        }],
        album: "In Bloom".into(),
        artist: "Pseudo Art".into(),
        ..Default::default()
    };
    let opts = ParseOpts {
        album_custom_tags: vec![
            ("album_transl".into(), "album".into()),
            ("album_artist_transl".into(), "artist".into()),
        ],
        track_custom_tags: vec![
            ("title_transl".into(), "title".into()),
            ("artist_transl".into(), "artist".into()),
        ],
        ..Default::default()
    };
    add_custom_tags(&mut official, &pseudo, &opts);
    let data = official.to_data();
    assert_eq!(data["album_transl"].py_str(), "In Bloom");
    assert_eq!(data["album_artist_transl"].py_str(), "Pseudo Art");
    let tdata = official.tracks[0].to_data();
    assert_eq!(tdata["title_transl"].py_str(), "In Bloom");
    assert_eq!(tdata["artist_transl"].py_str(), "Pseudo Art");
}

#[test]
fn the_musicbrainz_plugin_cannot_be_enabled_too() {
    let cfg = crate::config::Config::defaults();
    assert!(load(&cfg).is_err());
    let mut cfg = crate::config::Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mbpseudo]").unwrap());
    assert!(load(&cfg).is_ok());
}
