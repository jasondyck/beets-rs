//! beets' `test/plugins/test_beatport.py`: the release/track objects from
//! recorded API responses, and the infos built from them; plus the OAuth1
//! signing and the client against a fake API on 127.0.0.1.

use super::*;
use crate::plugins::oauth::{hmac_sha1, sha1};
use crate::plugins::requests::testserver::{Reply, Seen, serve};
use serde_json::json;

/// The release from the upstream test's recorded response.
fn release_response() -> J {
    json!({
        "id": 1742984,
        "type": "release",
        "name": "Charade",
        "slug": "charade",
        "releaseDate": "2016-04-11",
        "publishDate": "2016-04-11",
        "category": "Release",
        "catalogNumber": "GR089",
        "label": {"id": 24539, "name": "Gravitas Recordings", "type": "label"},
        "artists": [{"id": 326158, "name": "Supersillyus", "type": "artist"}],
        "genres": [{"id": 9, "name": "Breaks", "slug": "breaks", "type": "genre"}]
    })
}

/// One track of the upstream test's recorded track list.
fn track_response(id: i64, number: i64, name: &str, slug: &str, length_ms: i64, bpm: i64, key: &str) -> J {
    json!({
        "id": id,
        "type": "track",
        "name": name,
        "trackNumber": number,
        "mixName": "Original Mix",
        "title": format!("{name} (Original Mix)"),
        "slug": slug,
        "releaseDate": "2016-04-11",
        "lengthMs": length_ms,
        "bpm": bpm,
        "key": {"shortName": key},
        "artists": [{"id": 326158, "name": "Supersillyus", "type": "artist"}],
        "genres": [{"id": 9, "name": "Breaks", "type": "genre"}],
        "subGenres": [{"id": 209, "name": "Glitch Hop", "type": "subgenre"}]
    })
}

fn tracks_response() -> Vec<J> {
    vec![
        track_response(7817567, 1, "Mirage a Trois", "mirage-a-trois-original-mix", 425421, 90, "Gmin"),
        track_response(7817568, 2, "Aeon Bahamut", "aeon-bahamut-original-mix", 458000, 100, "Gmaj"),
        track_response(7817569, 3, "Trancendental Medication", "trancendental-medication-original-mix", 68571, 141, "Fmaj"),
        track_response(7817570, 4, "A List of Instructions for When I'm Human", "a-list-of-instructions-for-when-im-human-original-mix", 417913, 88, "Amin"),
        track_response(7817571, 5, "The Great Shenanigan", "the-great-shenanigan-original-mix", 589875, 123, "E&#9837;maj"),
        track_response(7817572, 6, "Charade", "charade-original-mix", 425423, 123, "Amaj"),
    ]
}

fn plugin() -> Beatport {
    Beatport::with(Conf {
        urls: Urls::default(),
        apikey: "key".into(),
        apisecret: "secret".into(),
        tokenfile: PathBuf::from("/nonexistent"),
    })
}

#[test]
fn sha1_and_hmac_match_the_reference_values() {
    // FIPS 180-1 test vectors.
    let hex = |b: [u8; 20]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(hex(sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        hex(sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
        "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
    );
    // RFC 2202 HMAC-SHA1 test cases 1 and 2.
    assert_eq!(
        hex(hmac_sha1(&[0x0b; 20], b"Hi There")),
        "b617318655057264e28bc0b6fb378c8ef146be00"
    );
    assert_eq!(
        hex(hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
        "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
    );
}

#[test]
fn release_and_track_objects() {
    let album = BeatportRelease::from_data(&release_response());
    assert_eq!(album.object.name, "Charade");
    assert_eq!(album.catalog_number.as_deref(), Some("GR089"));
    assert_eq!(album.label_name.as_deref(), Some("Gravitas Recordings"));
    assert_eq!(album.category.as_deref(), Some("Release"));
    assert_eq!(album.url.as_deref(), Some("https://beatport.com/release/charade/1742984"));
    assert_eq!(album.object.artists_str().as_deref(), Some("Supersillyus"));

    let tracks: Vec<BeatportTrack> = tracks_response().iter().map(BeatportTrack::from_data).collect();
    assert_eq!(tracks[0].object.name, "Mirage a Trois");
    assert_eq!(tracks[0].mix_name.as_deref(), Some("Original Mix"));
    assert_eq!(tracks[0].length as i64, 425);
    assert_eq!(
        tracks[0].url.as_deref(),
        Some("https://beatport.com/track/mirage-a-trois-original-mix/7817567")
    );
    assert_eq!(tracks[0].bpm, Some(90));
    assert_eq!(tracks[0].initial_key, "Gmin");
    assert_eq!(tracks[0].object.genres, ["Glitch Hop", "Breaks"]);
    assert_eq!(tracks[5].track_number, Some(6));
}

#[test]
fn empty_response_objects() {
    let track = json!({
        "id": 7817567,
        "name": "Mirage a Trois",
        "genres": [{"id": 9, "name": "Breaks"}],
        "subGenres": [{"id": 209, "name": "Glitch Hop"}]
    });
    let t = BeatportTrack::from_data(&track);
    assert_eq!(t.object.genres, ["Glitch Hop", "Breaks"]);
    // No subGenres: only the genre.
    let mut no_sub = track.clone();
    no_sub["subGenres"] = json!([]);
    assert_eq!(BeatportTrack::from_data(&no_sub).object.genres, ["Breaks"]);
    let mut no_genre = track.clone();
    no_genre["genres"] = json!([]);
    assert_eq!(BeatportTrack::from_data(&no_genre).object.genres, ["Glitch Hop"]);
}

#[test]
fn album_info_from_a_release() {
    let p = plugin();
    let mut release = BeatportRelease::from_data(&release_response());
    release.tracks = Some(tracks_response().iter().map(BeatportTrack::from_data).collect());
    let info = p.album_info(&release);
    assert_eq!(info.album, "Charade");
    assert_eq!(info.album_id, "1742984");
    assert_eq!(info.artist, "Supersillyus");
    assert_eq!(info.artist_id, "326158");
    assert_eq!(info.albumtype.as_str(), "Release");
    assert_eq!(info.catalognum, "GR089");
    assert_eq!(info.label, "Gravitas Recordings");
    assert_eq!(info.media, "Digital");
    assert_eq!((info.year, info.month, info.day), (2016, 4, 11));
    assert!(!info.va);
    assert_eq!(info.tracks.len(), 6);
    let data = info.to_data();
    assert_eq!(
        data["beatport_album_id"],
        crate::hooks::V::Str("1742984".into())
    );
    assert_eq!(
        data["genres"],
        crate::hooks::V::List(vec!["Breaks".into()])
    );
    let t = &info.tracks[5];
    assert_eq!(t.title, "Charade");
    assert_eq!(t.index, 6);
    assert_eq!(t.medium_index, 6);
    assert_eq!(t.to_data()["bpm"], crate::hooks::V::Int(123));
    assert_eq!(
        t.to_data()["initial_key"],
        crate::hooks::V::Str("Amaj".into())
    );
    assert_eq!(t.length, Some(425.423));
}

#[test]
fn non_original_mix_gets_its_name_in_parentheses() {
    let mut track = track_response(1, 1, "Song", "song", 1000, 120, "Amin");
    track["mixName"] = json!("Extended Mix");
    let p = plugin();
    let info = p.track_info(&BeatportTrack::from_data(&track));
    assert_eq!(info.title, "Song (Extended Mix)");
}

#[test]
fn more_than_three_artists_is_various_artists() {
    let mut data = release_response();
    data["artists"] = json!([
        {"id": 1, "name": "A"}, {"id": 2, "name": "B"},
        {"id": 3, "name": "C"}, {"id": 4, "name": "D"}
    ]);
    let mut release = BeatportRelease::from_data(&data);
    release.tracks = Some(vec![]);
    let info = plugin().album_info(&release);
    assert!(info.va);
    assert_eq!(info.artist, "Various Artists");
}

#[test]
fn deprecation_message_as_beets() {
    assert_eq!(
        deprecation_message(),
        "The 'beatport' plugin is deprecated and will be removed in version 3.0.0."
    );
}

fn client(base: &str) -> BeatportClient {
    BeatportClient::with_api_base("key", "secret", base).with_token("token", "tokensecret")
}

#[test]
fn client_fetches_a_release_with_oauth() {
    let release = release_response();
    let tracks = tracks_response();
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        match path {
            "/catalog/3/releases" => Reply::json(json!({"results": [release.clone()]})),
            "/catalog/3/tracks" => Reply::json(json!({"results": tracks.clone()})),
            _ => Reply::status(404),
        }
    });
    let c = client(&s.base);
    let release = c.get_release("1742984").unwrap().unwrap();
    assert_eq!(release.object.name, "Charade");
    assert_eq!(release.tracks.as_ref().unwrap().len(), 6);
    let seen = s.requests();
    assert_eq!(seen[0].target, "/catalog/3/releases?id=1742984");
    assert!(seen[1].target.starts_with("/catalog/3/tracks?releaseId=1742984"));
    let auth = seen[0].header("Authorization").unwrap();
    assert!(auth.starts_with("OAuth "), "{auth}");
    for part in [
        "oauth_consumer_key=\"key\"",
        "oauth_token=\"token\"",
        "oauth_signature_method=\"HMAC-SHA1\"",
        "oauth_version=\"1.0\"",
    ] {
        assert!(auth.contains(part), "{auth}");
    }
}

#[test]
fn client_search_returns_releases_and_tracks() {
    let release = release_response();
    let tracks = tracks_response();
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        if !seen.target.contains("fieldType:release") && path == "/catalog/3/search" {
            return Reply::json(json!({"results": [tracks[0].clone()]}));
        }
        match path {
            "/catalog/3/search" => Reply::json(json!({"results": [release.clone()]})),
            "/catalog/3/tracks" => Reply::json(json!({"results": tracks.clone()})),
            _ => Reply::status(404),
        }
    });
    let c = client(&s.base);
    let releases = c.search("supersillyus charade", "release").unwrap();
    assert_eq!(releases.len(), 1);
    assert!(matches!(&releases[0], SearchResult::Release(r) if r.tracks.as_ref().unwrap().len() == 6));
    let found = c.search("mirage", "track").unwrap();
    assert!(matches!(&found[0], SearchResult::Track(t) if t.object.name == "Mirage a Trois"));
    assert!(s.requests()[0].target.contains("perPage=5"));
    assert!(s.requests()[0].target.contains("facets=fieldType%3Arelease"));
}

#[test]
fn album_and_track_lookup_by_id() {
    let release = release_response();
    let tracks = tracks_response();
    let s = serve(move |seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or("");
        match path {
            "/catalog/3/releases" => Reply::json(json!({"results": [release.clone()]})),
            "/catalog/3/tracks" => {
                // `id=<track id>` returns one track; `releaseId=` all of
                // them.
                let by_id = seen
                    .target
                    .split('?')
                    .nth(1)
                    .unwrap_or("")
                    .split('&')
                    .find_map(|p| p.strip_prefix("id="));
                match by_id {
                    Some(id) => {
                        let found: Vec<J> = tracks
                            .iter()
                            .filter(|t| t["id"].as_i64().is_some_and(|v| v.to_string() == id))
                            .cloned()
                            .collect();
                        Reply::json(json!({"results": found}))
                    }
                    None => Reply::json(json!({"results": tracks.clone()})),
                }
            }
            _ => Reply::status(404),
        }
    });
    let mut p = plugin();
    p.conf.urls.api_base = s.base.clone();
    p.client = Some(client(&s.base));
    let info = p
        .album_for_id(
            &crate::metadata_plugins::test_ctx::ctx(),
            "https://beatport.com/release/charade/1742984",
        )
        .unwrap()
        .unwrap();
    assert_eq!(info.album_id, "1742984");
    let track = p
        .track_for_id(
            &crate::metadata_plugins::test_ctx::ctx(),
            "https://beatport.com/track/charade-original-mix/7817572",
        )
        .unwrap()
        .unwrap();
    assert_eq!(track.title, "Charade");
    assert_eq!(track.track_id, "7817572");
    // Not a track URL: no request.
    assert!(p
        .track_for_id(&crate::metadata_plugins::test_ctx::ctx(), "nope")
        .unwrap()
        .is_none());
}

#[test]
fn candidate_queries_are_normalized() {
    assert_eq!(normalize_query("A & B - CD1"), "A B ");
    assert_eq!(normalize_query("Album disc 2"), "Album ");
}
