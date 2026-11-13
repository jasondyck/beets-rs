//! beets' `test/plugins/test_discogs.py`: artist credits (ANV, featured,
//! roles), the tracklist's positions/mediums/index tracks, release dates,
//! media/genres, the search query, and the API client (a fake Discogs API
//! on 127.0.0.1 where the tests were pure parsing).

use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use serde_json::{Value as J, json};

/// `_artist`.
fn artist(name: &str, extra: J) -> J {
    let mut base = json!({
        "id": 1,
        "name": name,
        "join": "",
        "role": "",
        "anv": "",
        "tracks": "",
        "resource_url": ""
    });
    let obj = base.as_object_mut().unwrap();
    for (k, v) in extra.as_object().unwrap() {
        obj.insert(k.clone(), v.clone());
    }
    base
}

fn audio_track(title: &str, position: &str, duration: &str) -> J {
    json!({
        "type_": "track",
        "position": position,
        "title": title,
        "duration": duration
    })
}

fn index_track(title: &str, sub_tracks: Vec<J>) -> J {
    json!({
        "type_": "index",
        "position": "",
        "title": title,
        "duration": "",
        "sub_tracks": sub_tracks
    })
}

fn heading_track(title: &str) -> J {
    json!({
        "type_": "heading",
        "position": "",
        "title": title,
        "duration": ""
    })
}

fn release(tracks: Vec<J>) -> J {
    json!({
        "id": 11111111,
        "uri": "https://www.discogs.com/release/111111111",
        "title": "ALBUM TITLE",
        "year": 3001,
        "artists": [artist("ARTIST NAME", json!({"join": ","}))],
        "formats": [{"descriptions": ["FORMAT DESC 1", "FORMAT DESC 2"], "name": "FORMAT", "qty": 1}],
        "genres": ["STYLE1", "STYLE2"],
        "styles": ["GENRE1", "GENRE2"],
        "labels": [{"name": "LABEL NAME", "catno": "CATALOG NUMBER"}],
        "tracklist": tracks
    })
}

fn release_from_positions(positions: &[&str]) -> J {
    let tracks = positions
        .iter()
        .enumerate()
        .map(|(i, p)| audio_track(&format!("TITLE{}", i + 1), p, ""))
        .collect();
    release(tracks)
}

fn conf(base: &str) -> Conf {
    let mut conf = Conf::from_config(&crate::config::Config::defaults());
    conf.urls.api = base.to_string();
    conf
}

fn plugin_at(base: &str) -> Discogs {
    let client = DiscogsClient::with_token(base, "test", "token");
    Discogs::with(conf(base), client, "Various Artists")
}

/// A plugin with no server needed (pure parsing).
fn plugin() -> Discogs {
    plugin_at("http://127.0.0.1:1")
}

fn empty_server() -> Server {
    serve(|_| Reply::status(404))
}

#[test]
fn track_index_positions() {
    for (position, medium, index, subindex) in [
        ("1", None, Some("1"), None),
        ("A12", Some("A"), Some("12"), None),
        ("12-34", Some("12-"), Some("34"), None),
        ("CD1-1", Some("CD1-"), Some("1"), None),
        ("A1", Some("A"), Some("1"), None),
        ("1.12", None, Some("1"), Some("12")),
        ("12.a", None, Some("12"), Some("A")),
        ("12.34", None, Some("12"), Some("34")),
        ("1ab", None, Some("1"), Some("AB")),
        ("IV", Some("IV"), None, None),
        ("", None, None, None),
    ] {
        let got = get_track_index(position);
        assert_eq!(
            got,
            (
                medium.map(String::from),
                index.map(String::from),
                subindex.map(String::from)
            ),
            "{position}"
        );
    }
}

#[test]
fn release_dates() {
    for (released, expected) in [
        (Some("2000-08-13"), (Some(2000), Some(8), Some(13))),
        (Some("2000-08"), (Some(2000), Some(8), None)),
        (Some("2000"), (Some(2000), None, None)),
        (Some("2000-00-00"), (Some(2000), None, None)),
        (Some("2000-08-00"), (Some(2000), Some(8), None)),
        (Some("2000-8-1"), (Some(2000), Some(8), Some(1))),
        (Some("  2000-08-13  "), (Some(2000), Some(8), Some(13))),
        (Some(""), (Some(3001), None, None)),
        (None, (Some(3001), None, None)),
        (Some("0000-00-00"), (Some(3001), None, None)),
        (Some("13 Aug 2000"), (Some(3001), None, None)),
    ] {
        assert_eq!(parse_release_date(released, Some(3001)), expected, "{released:?}");
    }
}

#[test]
fn media_and_albumtype_from_formats() {
    assert_eq!(Discogs::get_media_and_albumtype(None), (None, None));
    let formats = vec![json!({
        "descriptions": ["7\"", "Single", "45 RPM"],
        "name": "Vinyl",
        "qty": 1
    })];
    assert_eq!(
        Discogs::get_media_and_albumtype(Some(&formats)),
        (Some("Vinyl".into()), Some("7\", Single, 45 RPM".into()))
    );
}

#[test]
fn media_and_medium_numbers() {
    let p = plugin();
    let info = p.get_album_info(&release_from_positions(&["1", "2"])).unwrap();
    assert_eq!(info.media, "FORMAT");
    assert_eq!(info.tracks[0].media, "FORMAT");
    assert_eq!(info.mediums, 1);
    assert_eq!((info.tracks[0].medium, info.tracks[0].medium_total), (1, 2));
    assert_eq!((info.tracks[1].medium, info.tracks[1].medium_total), (1, 2));
    assert_eq!((info.tracks[0].medium_index, info.tracks[0].index), (1, 1));
    assert_eq!((info.tracks[1].medium_index, info.tracks[1].index), (2, 2));

    let info = p.get_album_info(&release_from_positions(&["1-1", "2-1"])).unwrap();
    assert_eq!(info.mediums, 2);
    assert_eq!((info.tracks[0].medium, info.tracks[0].medium_total), (1, 1));
    assert_eq!((info.tracks[1].medium, info.tracks[1].medium_total), (2, 1));

    let info = p
        .get_album_info(&release_from_positions(&["A1", "B1", "C1"]))
        .unwrap();
    assert_eq!(info.mediums, 2);
    assert_eq!(
        (info.tracks[0].medium, info.tracks[0].medium_total, info.tracks[0].medium_index),
        (1, 2, 1)
    );
    assert_eq!(
        (info.tracks[1].medium, info.tracks[1].medium_total, info.tracks[1].medium_index),
        (1, 2, 2)
    );
    assert_eq!(
        (info.tracks[2].medium, info.tracks[2].medium_total, info.tracks[2].medium_index),
        (2, 1, 1)
    );

    let info = p
        .get_album_info(&release_from_positions(&["1-1", "1-2", "2-1", "3-1"]))
        .unwrap();
    assert_eq!(info.mediums, 3);
    assert_eq!(
        info.tracks.iter().map(|t| (t.medium_index, t.index, t.medium_total)).collect::<Vec<_>>(),
        [(1, 1, 2), (2, 2, 2), (1, 3, 1), (1, 4, 1)]
    );
}

#[test]
fn minimal_release() {
    let data = json!({
        "id": 123,
        "uri": "https://www.discogs.com/release/123456-something",
        "tracklist": [audio_track("A", "1", "01:01")],
        "artists": [artist("ARTIST NAME", json!({"id": 321}))],
        "title": "TITLE"
    });
    let info = plugin().get_album_info(&data).unwrap();
    assert_eq!(info.artist, "ARTIST NAME");
    assert_eq!(info.album, "TITLE");
    assert_eq!(info.tracks.len(), 1);
    assert_eq!(info.tracks[0].length, Some(61.0));
    assert_eq!(info.tracks[0].track_id, "123-1");
    assert!(info.nulls.contains("label"));
}

#[test]
fn genres_and_styles() {
    let p = plugin();
    let data = release_from_positions(&["1", "2"]);
    let info = p.get_album_info(&data).unwrap();
    let d = info.to_data();
    assert_eq!(d["style"], crate::hooks::V::Str("STYLE1, STYLE2".into()));
    assert_eq!(
        d["genres"],
        crate::hooks::V::List(vec!["GENRE1".into(), "GENRE2".into()])
    );
}

#[test]
fn append_style_to_genre() {
    let base = "http://127.0.0.1:1";
    let mut c = conf(base);
    c.append_style_genre = true;
    let p = Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists");
    let info = p.get_album_info(&release_from_positions(&["1", "2"])).unwrap();
    assert_eq!(
        info.to_data()["genres"],
        crate::hooks::V::List(
            ["GENRE1", "GENRE2", "STYLE1", "STYLE2"]
                .map(String::from)
                .to_vec()
        )
    );
    let mut data = release_from_positions(&["1", "2"]);
    data["genres"] = json!([]);
    let info = p.get_album_info(&data).unwrap();
    let d = info.to_data();
    assert_eq!(d["style"], crate::hooks::V::None);
    assert_eq!(
        d["genres"],
        crate::hooks::V::List(vec!["GENRE1".into(), "GENRE2".into()])
    );
}

fn disambiguation_release() -> J {
    json!({
        "id": 123,
        "uri": "https://www.discogs.com/release/123456-something",
        "tracklist": [{
            "title": "track",
            "position": "A",
            "type_": "track",
            "duration": "5:44",
            "artists": [artist("TEST ARTIST (5)", json!({"id": 11146}))]
        }],
        "artists": [
            artist("ARTIST NAME (2)", json!({"id": 321, "join": "&"})),
            artist("OTHER ARTIST (5)", json!({"id": 322}))
        ],
        "title": "title",
        "labels": [{"name": "LABEL NAME (5)", "catno": "catalog number"}]
    })
}

#[test]
fn strip_disambiguation() {
    let p = plugin();
    let info = p.get_album_info(&disambiguation_release()).unwrap();
    assert_eq!(info.artist, "ARTIST NAME & OTHER ARTIST");
    assert_eq!(info.artists, ["ARTIST NAME", "OTHER ARTIST"]);
    assert_eq!(info.artists_ids, ["321", "322"]);
    assert_eq!(info.tracks[0].artist, "TEST ARTIST");
    assert_eq!(info.tracks[0].artists, ["TEST ARTIST"]);
    assert_eq!(info.tracks[0].artist_id, "11146");
    assert_eq!(info.label, "LABEL NAME");
}

#[test]
fn dont_strip_disambiguation() {
    let base = "http://127.0.0.1:1";
    let mut c = conf(base);
    c.strip_disambiguation = false;
    let p = Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists");
    let info = p.get_album_info(&disambiguation_release()).unwrap();
    assert_eq!(info.artist, "ARTIST NAME (2) & OTHER ARTIST (5)");
    assert_eq!(info.artists, ["ARTIST NAME (2)", "OTHER ARTIST (5)"]);
    assert_eq!(info.tracks[0].artist, "TEST ARTIST (5)");
    assert_eq!(info.label, "LABEL NAME (5)");
}

#[test]
fn tracklist_positions_and_counts() {
    let p = plugin();
    for (positions, mediums, tracks) in [
        (vec!["1", "2", "3"], 1, 3),
        (vec!["A1", "A2", "B1", "B2"], 1, 4),
        (vec!["A1", "A2", "B1", "C1"], 2, 4),
        (vec!["1-1", "1-2", "2-1", "3-1"], 3, 4),
        (vec!["I", "II", "III", "IV"], 1, 4),
        (vec!["1", "2.1", "2.2", "3"], 1, 3),
        (vec!["A1", "A2.1", "A2.2", "A3"], 1, 3),
        (vec!["A1", "A2a", "A2b", "A3"], 1, 3),
        (vec!["A1", "A2.a", "A2.b", "A3"], 1, 3),
        (vec!["1.1", "1.2", "2.1", "2.2"], 1, 2),
        (vec!["1", "2", "Video 1"], 2, 3),
    ] {
        let info = p.get_album_info(&release_from_positions(&positions)).unwrap();
        assert_eq!(info.mediums, mediums, "{positions:?}");
        assert_eq!(info.tracks.len(), tracks, "{positions:?}");
    }
}

fn index_plugin(index_tracks: bool) -> Discogs {
    let base = "http://127.0.0.1:1";
    let mut c = conf(base);
    c.index_tracks = index_tracks;
    Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists")
}

#[test]
fn index_tracks_and_disc_titles() {
    for (index_tracks, tracks, mediums, expected) in [
        (
            false,
            vec![
                heading_track("MEDIUM TITLE"),
                audio_track("TITLE ONE", "1.1", ""),
                audio_track("TITLE TWO", "1.2", ""),
            ],
            1,
            vec![("TITLE ONE / TITLE TWO", Some("MEDIUM TITLE"))],
        ),
        (
            false,
            vec![
                audio_track("TITLE ONE", "1", ""),
                index_track(
                    "TRACK GROUP TITLE",
                    vec![
                        audio_track("SUBTITLE ONE", "2.1", "01:01"),
                        audio_track("SUBTITLE TWO", "2.2", "02:02"),
                    ],
                ),
                audio_track("TITLE THREE", "3", ""),
            ],
            1,
            vec![
                ("TITLE ONE", None),
                ("TRACK GROUP TITLE", None),
                ("TITLE THREE", None),
            ],
        ),
        (
            false,
            vec![
                audio_track("TITLE ONE", "1", ""),
                index_track(
                    "TRACK GROUP TITLE",
                    vec![
                        audio_track("SUBTITLE ONE", "2", "01:01"),
                        audio_track("SUBTITLE TWO", "3", "02:02"),
                    ],
                ),
                audio_track("TITLE FOUR", "4", ""),
            ],
            1,
            vec![
                ("TITLE ONE", None),
                ("SUBTITLE ONE", None),
                ("SUBTITLE TWO", None),
                ("TITLE FOUR", None),
            ],
        ),
        (
            true,
            vec![
                audio_track("TITLE ONE", "1", ""),
                index_track(
                    "TRACK GROUP TITLE",
                    vec![
                        audio_track("SUBTITLE ONE", "2", "01:01"),
                        audio_track("SUBTITLE TWO", "3", "02:02"),
                    ],
                ),
                audio_track("TITLE FOUR", "4", ""),
            ],
            1,
            vec![
                ("TITLE ONE", None),
                ("TRACK GROUP TITLE: SUBTITLE ONE", None),
                ("TRACK GROUP TITLE: SUBTITLE TWO", None),
                ("TITLE FOUR", None),
            ],
        ),
        (
            false,
            vec![
                heading_track("MEDIUM TITLE CD1"),
                audio_track("TITLE ONE", "1-1", ""),
                audio_track("TITLE TWO", "1-2", ""),
                heading_track("MEDIUM TITLE CD2"),
                audio_track("TITLE THREE", "2-1", ""),
            ],
            2,
            vec![
                ("TITLE ONE", Some("MEDIUM TITLE CD1")),
                ("TITLE TWO", Some("MEDIUM TITLE CD1")),
                ("TITLE THREE", Some("MEDIUM TITLE CD2")),
            ],
        ),
    ] {
        let p = index_plugin(index_tracks);
        let info = p.get_album_info(&release(tracks)).unwrap();
        assert_eq!(info.mediums, mediums);
        let got: Vec<(&str, Option<&str>)> = info
            .tracks
            .iter()
            .map(|t| {
                (
                    t.title.as_str(),
                    (!t.nulls.contains("disctitle")).then_some(t.disctitle.as_str()),
                )
            })
            .collect();
        assert_eq!(got, expected, "index_tracks={index_tracks}");
    }
}

#[test]
fn index_track_inherits_artists() {
    let track_artist = "TRACK ARTIST";
    let group_artist = "GROUP ARTIST";
    let tracks = vec![index_track(
        "TRACK GROUP TITLE",
        vec![
            {
                let mut t = audio_track("TRACK ONE", "2", "");
                t["artists"] = json!([artist(track_artist, json!({}))]);
                t
            },
            audio_track("SUBTITLE TWO", "3", ""),
        ],
    )
    .to_owned()];
    let mut tracks = tracks;
    tracks[0]["artists"] = json!([artist(group_artist, json!({}))]);
    let info = plugin().get_album_info(&release(tracks)).unwrap();
    assert_eq!(info.mediums, 1);
    let artists: std::collections::BTreeSet<String> =
        info.tracks.iter().map(|t| t.artist.clone()).collect();
    assert_eq!(
        artists,
        [track_artist.to_string(), group_artist.to_string()]
            .into_iter()
            .collect()
    );
}

#[test]
fn coalesce_logical_index_as_audio_track() {
    let mut index = index_track(
        "TRACK GROUP TITLE",
        vec![
            audio_track("SUBTITLE ONE", "2.1", ""),
            audio_track("SUBTITLE TWO", "2.2", ""),
        ],
    );
    index["duration"] = json!("03:03");
    let p = plugin();
    let clean = p.coalesce_tracks(&[index]).unwrap();
    assert_eq!(clean.len(), 1);
    assert_eq!(clean[0]["title"], "TRACK GROUP TITLE");
    assert_eq!(clean[0]["position"], "2");
    assert_eq!(clean[0]["duration"], "03:03");
}

fn anv_release() -> J {
    json!({
        "id": 123,
        "uri": "https://www.discogs.com/release/123456-something",
        "tracklist": [{
            "title": "track",
            "position": "A",
            "type_": "track",
            "duration": "5:44",
            "artists": [
                artist("ARTIST", json!({"id": 11146, "anv": "ART", "join": "Feat."})),
                artist("PERFORMER", json!({"id": 787}))
            ],
            "extraartists": [artist("PERFORMER", json!({"id": 787, "role": "Featuring"}))]
        }],
        "artists": [
            artist("DRUMMER", json!({"id": 445, "anv": "DRUM", "join": ", "})),
            artist("ARTIST (4)", json!({"id": 321, "anv": "ARTY", "join": "&"})),
            artist("SOLOIST", json!({"id": 446, "anv": "SOLO"}))
        ],
        "title": "title"
    })
}

fn anv_plugin(anv_artist: bool, anv_album_artist: bool, anv_credit: bool) -> Discogs {
    let base = "http://127.0.0.1:1";
    let mut c = conf(base);
    c.anv_artist = anv_artist;
    c.anv_album_artist = anv_album_artist;
    c.anv_artist_credit = anv_credit;
    Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists")
}

#[test]
fn anv_track_artist_fields() {
    for (use_anv, artist, artists) in [
        (false, "ARTIST Feat. PERFORMER", vec!["ARTIST", "PERFORMER"]),
        (true, "ART Feat. PERFORMER", vec!["ART", "PERFORMER"]),
    ] {
        let info = anv_plugin(use_anv, false, false)
            .get_album_info(&anv_release())
            .unwrap();
        assert_eq!(info.tracks[0].artist, artist);
        assert_eq!(info.tracks[0].artists, artists);
    }
}

#[test]
fn anv_credit_fields() {
    for (use_credit, album_credit, album_credits, track_credit, track_credits) in [
        (
            false,
            "DRUMMER, ARTIST & SOLOIST",
            vec!["DRUMMER", "ARTIST", "SOLOIST"],
            "ARTIST Feat. PERFORMER",
            vec!["ARTIST", "PERFORMER"],
        ),
        (
            true,
            "DRUM, ARTY & SOLO",
            vec!["DRUM", "ARTY", "SOLO"],
            "ART Feat. PERFORMER",
            vec!["ART", "PERFORMER"],
        ),
    ] {
        let info = anv_plugin(false, false, use_credit)
            .get_album_info(&anv_release())
            .unwrap();
        assert_eq!(info.artist_credit, album_credit);
        assert_eq!(info.artists_credit, album_credits);
        assert_eq!(info.tracks[0].artist_credit, track_credit);
        assert_eq!(info.tracks[0].artists_credit, track_credits);
    }
}

#[test]
fn anv_album_artist_fields() {
    for (use_anv, artist, artists) in [
        (false, "DRUMMER, ARTIST & SOLOIST", vec!["DRUMMER", "ARTIST", "SOLOIST"]),
        (true, "DRUM, ARTY & SOLO", vec!["DRUM", "ARTY", "SOLO"]),
    ] {
        let info = anv_plugin(false, use_anv, false)
            .get_album_info(&anv_release())
            .unwrap();
        assert_eq!(info.artist, artist);
        assert_eq!(info.artists, artists);
    }
}

#[test]
fn anv_album_artist_only_for_the_album() {
    let data = json!({
        "id": 123,
        "uri": "https://www.discogs.com/release/123456-something",
        "tracklist": [{
            "title": "track", "position": "A", "type_": "track", "duration": "5:44"
        }],
        "artists": [artist("ARTIST (4)", json!({"id": 321, "anv": "VARIATION"}))],
        "title": "title"
    });
    let info = anv_plugin(true, false, false).get_album_info(&data).unwrap();
    assert_eq!(info.artist, "ARTIST");
    assert_eq!(info.artists, ["ARTIST"]);
    assert_eq!(info.artist_id, "321");
    assert_eq!(info.tracks[0].artist, "VARIATION");
    assert_eq!(info.tracks[0].artists, ["VARIATION"]);
    assert_eq!(info.tracks[0].artist_credit, "ARTIST");
}

#[test]
fn featured_artists() {
    let track = json!({
        "type_": "track",
        "title": "track",
        "position": "1",
        "duration": "5:00",
        "artists": [
            artist("NEW ARTIST", json!({"id": 11146, "join": "&"})),
            artist("VOCALIST", json!({"id": 344, "join": "feat."}))
        ],
        "extraartists": [
            artist("SOLOIST", json!({"id": 3, "role": "Featuring"})),
            artist("PERFORMER (1)", json!({"id": 5, "role": "Other Role, Featuring"})),
            artist("RANDOM", json!({"id": 8, "role": "Written-By"})),
            artist("MUSICIAN", json!({"id": 10, "role": "Featuring [Uncredited]"}))
        ]
    });
    let p = plugin();
    let state = p.artist_state(vec![Artist::from_data(&artist("ARTIST", json!({})))], false);
    let (t, _, _) = p.get_track_info(&track, 1, &[], &state);
    assert_eq!(
        t.artist,
        "NEW ARTIST & VOCALIST feat. SOLOIST, PERFORMER, MUSICIAN"
    );
    assert_eq!(
        t.artists,
        ["NEW ARTIST", "VOCALIST", "SOLOIST", "PERFORMER", "MUSICIAN"]
    );
    assert_eq!(t.artists_ids, ["11146", "344", "3", "5", "10"]);
    assert_eq!(t.composers, Some(vec!["RANDOM".to_string()]));
}

#[test]
fn extraartist_roles() {
    let track = json!({
        "type_": "track",
        "title": "track",
        "position": "1",
        "duration": "5:00",
        "artists": [artist("TRACK ARTIST", json!({"id": 11}))],
        "extraartists": [
            artist("LYRICIST", json!({"id": 2, "role": "Lyrics By"})),
            artist("ARRANGER", json!({"id": 3, "role": "Arranged By"})),
            artist("REMIXER", json!({"id": 5, "role": "Remixed By"})),
            artist("COMPOSER", json!({"id": 6, "role": "Written-By"}))
        ]
    });
    let p = plugin();
    let state = p.artist_state(vec![Artist::from_data(&artist("ARTIST", json!({})))], false);
    let (t, _, _) = p.get_track_info(&track, 1, &[], &state);
    assert_eq!(t.artist, "TRACK ARTIST");
    assert_eq!(t.artists, ["TRACK ARTIST"]);
    assert_eq!(t.lyricists, Some(vec!["LYRICIST".to_string()]));
    assert_eq!(t.arrangers, Some(vec!["ARRANGER".to_string()]));
    assert_eq!(t.remixers, Some(vec!["REMIXER".to_string()]));
    assert_eq!(t.composers, Some(vec!["COMPOSER".to_string()]));
}

#[test]
fn various_artists_uses_the_configured_name() {
    let p = Discogs::with(
        conf("http://127.0.0.1:1"),
        DiscogsClient::with_token("http://127.0.0.1:1", "t", "token"),
        "VARIOUS ARTISTS",
    );
    let state = p.artist_state(vec![Artist::from_data(&artist("Various", json!({})))], false);
    let info = state.info();
    assert_eq!(info.artist, "VARIOUS ARTISTS");
    assert_eq!(info.artist_id, "1");
    assert_eq!(info.artists, ["VARIOUS ARTISTS"]);
    assert_eq!(info.artists_ids, ["1"]);
    assert_eq!(info.artist_credit, "VARIOUS ARTISTS");
    assert_eq!(info.arrangers, None);
    assert_eq!(info.composers, None);
}

fn item_with(fields: &[(&str, &str)]) -> crate::model::Item {
    let mut item = crate::model::Item::new_item();
    for (k, v) in fields {
        item.set_str(k, *v);
    }
    item
}

#[test]
fn search_query_filters() {
    let p = plugin();
    let (query, filters) = p.search_query(&[crate::model::Item::new_item()], "Artist", "Album", false);
    assert!(query.contains("Album"));
    assert_eq!(filters, [("type".to_string(), "release".to_string())]);
}

#[test]
fn search_query_extra_tags() {
    let base = "http://127.0.0.1:1";
    let mut c = conf(base);
    c.extra_tags = vec!["label".into(), "catalognum".into()];
    let p = Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists");
    let items = [
        item_with(&[("catalognum", "ABC 123"), ("label", "abc")]),
        item_with(&[("catalognum", "ABC 123"), ("label", "abc")]),
        item_with(&[("catalognum", "ABC 123"), ("label", "def")]),
    ];
    let (_, filters) = p.search_query(&items, "Artist", "Album", false);
    let get = |k: &str| {
        filters
            .iter()
            .find(|(f, _)| f == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("type"), Some("release"));
    assert_eq!(get("label"), Some("abc"));
    assert_eq!(get("catno"), Some("ABC123"));
}

#[test]
fn search_query_media_normalization() {
    for (media, expected) in [("Digital Media", "File"), ("WEB", "File"), ("Vinyl", "Vinyl")] {
        let base = "http://127.0.0.1:1";
        let mut c = conf(base);
        c.extra_tags = vec!["media".into()];
        let p = Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists");
        let (_, filters) = p.search_query(&[item_with(&[("media", media)])], "Artist", "Album", false);
        let format = filters.iter().find(|(k, _)| k == "format").map(|(_, v)| v.as_str());
        assert_eq!(format, Some(expected), "{media}");
    }
    // Normalized before the plurality: "File" wins 3 to 3 (ties keep the
    // first seen value's bucket).
    let base = "http://127.0.0.1:1";
    let mut c = conf(base);
    c.extra_tags = vec!["media".into()];
    let p = Discogs::with(c, DiscogsClient::with_token(base, "t", "token"), "Various Artists");
    let items = [
        item_with(&[("media", "Digital Media")]),
        item_with(&[("media", "Digital Media")]),
        item_with(&[("media", "WEB")]),
        item_with(&[("media", "WEB")]),
        item_with(&[("media", "Vinyl")]),
        item_with(&[("media", "Vinyl")]),
        item_with(&[("media", "Vinyl")]),
    ];
    let (_, filters) = p.search_query(&items, "Artist", "Album", false);
    assert_eq!(
        filters.iter().find(|(k, _)| k == "format").map(|(_, v)| v.as_str()),
        Some("File")
    );
}

#[test]
fn search_response_retries_a_malformed_body() {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(0));
    let calls2 = calls.clone();
    let s = serve(move |_| {
        let mut n = calls2.lock().unwrap();
        *n += 1;
        let n = *n;
        if n == 1 {
            Reply {
                status: 200,
                content_type: "application/json",
                body: b"invalid JSON".to_vec(),
            }
        } else {
            Reply::json(json!({"results": [{"id": 123}]}))
        }
    });
    let p = plugin_at(&s.base);
    let results = p.get_search_response("Album", &[("type".into(), "release".into())]).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(*calls.lock().unwrap(), 2);
    // The search sets per_page and page.
    let target = &s.requests()[1].target;
    assert!(target.contains("per_page=5"), "{target}");
    assert!(target.contains("type=release"), "{target}");
}

#[test]
fn search_response_raises_after_the_retry() {
    let s = serve(|_| Reply {
        status: 200,
        content_type: "application/json",
        body: b"invalid JSON".to_vec(),
    });
    let p = plugin_at(&s.base);
    assert!(matches!(
        p.get_search_response("Album", &[]),
        Err(DiscogsError::Malformed(_))
    ));
    assert_eq!(s.requests().len(), 2);
}

#[test]
fn album_for_id_fetches_and_refreshes() {
    let s = serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/releases/123" {
            Reply::json(release_from_positions(&["1", "2"]))
        } else if path == "/masters/9" {
            Reply::json(json!({"id": 9, "year": 1990}))
        } else {
            Reply::status(404)
        }
    });
    let p = plugin_at(&s.base);
    let ctx = crate::metadata_plugins::test_ctx::ctx();
    let info = p
        .album_for_id(&ctx, "https://www.discogs.com/release/123")
        .unwrap()
        .unwrap();
    assert_eq!(info.album_id, "11111111");
    assert_eq!(info.tracks.len(), 2);
    assert_eq!((info.year, info.month, info.day), (3001, 0, 0));
    assert_eq!(info.original_year, 3001);
    // An id that isn't a release id: no request.
    assert!(p.album_for_id(&ctx, "nope").unwrap().is_none());
    // A 404 is no album.
    assert!(p.album_for_id(&ctx, "999").unwrap().is_none());
}

#[test]
fn master_release_year_is_the_original_date() {
    let s = serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/releases/123" {
            let mut data = release_from_positions(&["1"]);
            data["master_id"] = json!(9);
            data["released"] = json!("2000-08-13");
            Reply::json(data)
        } else if path == "/masters/9" {
            Reply::json(json!({"id": 9, "year": 1990}))
        } else {
            Reply::status(404)
        }
    });
    let p = plugin_at(&s.base);
    let info = p.get_album_info(&json!({"id": 123})).unwrap();
    assert_eq!((info.year, info.month, info.day), (2000, 8, 13));
    assert_eq!((info.original_year, info.original_month, info.original_day), (1990, 0, 0));
    assert_eq!(info.releasegroup_id, "9");
}

#[test]
fn track_for_id_finds_the_invented_track_id() {
    let s = serve(|seen: &Seen| {
        if seen.target.split('?').next() == Some("/releases/11111111") {
            Reply::json(release_from_positions(&["1", "2"]))
        } else {
            Reply::status(404)
        }
    });
    let p = plugin_at(&s.base);
    let ctx = crate::metadata_plugins::test_ctx::ctx();
    let track = p.track_for_id(&ctx, "11111111-2").unwrap().unwrap();
    assert_eq!(track.title, "TITLE2");
    assert_eq!(track.track_id, "11111111-2");
}

#[test]
fn candidates_search_then_look_up() {
    let s = serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if path == "/database/search" {
            Reply::json(json!({"results": [{"id": 11111111, "title": "ALBUM TITLE"}]}))
        } else if path == "/releases/11111111" {
            Reply::json(release_from_positions(&["1", "2"]))
        } else {
            Reply::status(404)
        }
    });
    let p = plugin_at(&s.base);
    let ctx = crate::metadata_plugins::test_ctx::ctx();
    let albums = p
        .candidates(&ctx, &[], "Artist", "Album", false)
        .unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].album, "ALBUM TITLE");
    let target = &s.requests()[0].target;
    assert!(target.starts_with("/database/search?"), "{target}");
    assert!(target.contains("q=Artist+Album"), "{target}");
}

#[test]
fn malformed_search_is_logged_not_raised() {
    let s = serve(|_| Reply {
        status: 200,
        content_type: "application/json",
        body: b"invalid JSON".to_vec(),
    });
    let p = plugin_at(&s.base);
    let ctx = crate::metadata_plugins::test_ctx::ctx();
    assert!(p.candidates(&ctx, &[], "Artist", "Album", false).unwrap().is_empty());
}

#[test]
fn album_info_without_artists_is_refreshed() {
    let s = serve(|seen: &Seen| {
        if seen.target.split('?').next() == Some("/releases/11111111") {
            Reply::json(release_from_positions(&["1"]))
        } else {
            Reply::status(404)
        }
    });
    let _ = empty_server();
    let p = plugin_at(&s.base);
    let info = p.get_album_info(&json!({"id": 11111111, "title": "T"})).unwrap();
    assert_eq!(info.tracks.len(), 1);
    assert_eq!(s.requests().len(), 1);
}
