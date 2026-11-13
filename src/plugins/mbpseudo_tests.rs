//! Ported from beets' `test/plugins/test_mbpseudo.py` (the release
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

#[test]
fn the_data_source_is_the_pseudo_plugin_while_enabled() {
    let mut cfg = crate::config::Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mbpseudo]").unwrap());
    assert_eq!(ParseOpts::from_config(&cfg).data_source, "MusicBrainzPseudoRelease");
    let plain = crate::config::Config::defaults();
    assert_eq!(ParseOpts::from_config(&plain).data_source, "MusicBrainz");
}

#[test]
fn album_matched_switches_the_data_source() {
    let mut cfg = crate::config::Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mbpseudo]").unwrap());
    load(&cfg).unwrap();
    let ctx = crate::autotag::Ctx::new(&cfg);
    let info = crate::mb::AlbumInfo {
        album_id: "pseudo".into(),
        album: "In Bloom".into(),
        data_source: "MusicBrainzPseudoRelease".into(),
        ..Default::default()
    };
    let m = crate::autotag::match_album(
        &ctx,
        &crate::autotag::Current::default(),
        &[],
        info,
    );
    assert_eq!(m.info.data_source, "MusicBrainz");
    // The pseudo-release is still what the match proposes.
    assert_eq!(m.info.album_id, "pseudo");
    assert_eq!(m.info.album, "In Bloom");
}

#[test]
fn album_matched_leaves_another_source_alone() {
    let mut cfg = crate::config::Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[mbpseudo]").unwrap());
    load(&cfg).unwrap();
    let cell = std::cell::RefCell::new(crate::autotag::AlbumMatch {
        distance: crate::autotag::Distance::default(),
        info: crate::mb::AlbumInfo {
            data_source: "elsewhere".into(),
            ..Default::default()
        },
        mapping: vec![],
        extra_items: vec![],
        extra_tracks: vec![],
    });
    crate::events::send(
        "album_matched",
        &[("match", crate::events::Arg::Match(&cell))],
    );
    assert_eq!(cell.borrow().info.data_source, "elsewhere");
}

