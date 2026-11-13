//! The MusicBrainz helper: normalization, the search query, the `client`
//! parameter and HTTP digest authentication (beets'
//! `test/plugins/test_mbcollection.py` and `_utils/musicbrainz.py`).

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use serde_json::json;

fn cfg(server: &str) -> Config {
    let mut cfg = Config::defaults();
    cfg.set(
        "musicbrainz.host",
        serde_yaml::Value::String(server.trim_start_matches("http://").to_string()),
    );
    cfg.set("musicbrainz.https", serde_yaml::Value::Bool(false));
    cfg.set(
        "musicbrainz.ratelimit",
        serde_yaml::Value::Number(1000.into()),
    );
    cfg.set(
        "musicbrainz.ratelimit_interval",
        serde_yaml::Value::Number(1.into()),
    );
    cfg
}

#[test]
fn normalize_underscores_keys_and_groups_relations() {
    let data = json!({
        "artist-credit": [{"artist": {"sort-name": "A"}}],
        "relations": [
            {"target-type": "work", "type": "performance", "work": {"id": "w"}},
            {"target-type": "artist-role", "type": "composer", "artist": {"id": "a"}},
            {"target-type": "work", "type": "medley", "work": {"id": "w2"}}
        ]
    });
    let out = normalize(&data);
    assert_eq!(out["artist_credit"][0]["artist"]["sort_name"], "A");
    assert_eq!(out["work_relations"].as_array().unwrap().len(), 2);
    // The relation's own `target-type` is dropped, the rest normalized.
    assert!(out["work_relations"][0].get("target-type").is_none());
    assert_eq!(out["artist_role_relations"][0]["type"], "composer");
    // Groups come in sorted target-type order.
    assert_eq!(out["artist_role_relations"][0]["artist"]["id"], "a");
    // No `target-type` key anywhere.
    assert!(out.get("target-type").is_none());
}

#[test]
fn search_query_is_lucene_escaped() {
    let q = super::search_query(&[
        ("", "  Song Name ".into()),
        ("release", "A:B".into()),
        ("empty", "  ".into()),
    ]);
    assert_eq!(q, "song name release:(a\\:b)");
}

#[test]
fn digest_authorization_md5() {
    // RFC 2617's example (without qop).
    let header = digest_authorization(
        "GET",
        "/dir/index.html",
        r#"Digest realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        "Mufasa",
        "Circle Of Life",
    )
    .unwrap();
    // HA1 = 939e7578ed9e3c518a452acee763bce9,
    // HA2 = 39aff3a2bab6126f332b942af96d3366,
    // response = md5("HA1:nonce:HA2").
    assert!(
        header.contains("response=\"670fd8c2df070c60b045671b8b24ff02\""),
        "{header}"
    );
    assert!(header.contains("username=\"Mufasa\""));
    assert!(header.contains("uri=\"/dir/index.html\""));
}

/// A raw HTTP server (the test server cannot set extra headers): answer
/// the first request with a digest challenge, the next with JSON, and
/// record each request's headers.
type SeenHeaders = std::sync::Arc<std::sync::Mutex<Vec<Vec<(String, String)>>>>;

fn digest_server() -> (String, SeenHeaders) {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut headers = vec![];
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            seen2.lock().unwrap().push(headers);
            let (head, body) = if n == 0 {
                (
                    "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"mb\", nonce=\"abc\", qop=\"auth\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                    String::new(),
                )
            } else {
                let body = "{\"collections\": []}";
                (
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ),
                    body.to_string(),
                )
            };
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    (base, seen)
}

#[test]
fn digest_request_retries_with_credentials() {
    let (base, seen) = digest_server();
    let mut cfg = cfg(&base);
    cfg.set("musicbrainz.user", "testuser".into());
    cfg.set("musicbrainz.pass", "testpass".into());
    let api = MusicBrainzApi::new(&cfg);
    let data = api
        .get_json(&format!("{}/collection", api.api_root), &[])
        .unwrap();
    assert_eq!(data["collections"], json!([]));
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let header = |name: &str| {
        requests[1]
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let auth = header("Authorization").unwrap();
    assert!(auth.starts_with("Digest username=\"testuser\""), "{auth}");
    assert!(auth.contains("realm=\"mb\""), "{auth}");
    assert!(auth.contains("qop=auth"), "{auth}");
    assert!(auth.contains("uri=\"/ws/2/collection"), "{auth}");
    // The first request had no credentials.
    assert!(
        !requests[0]
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("Authorization"))
    );
    // Both carry fmt and client.
    for headers in requests.iter() {
        let target = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let _ = target("Host");
    }
}

#[test]
fn unauthorized_has_musicbrainz_hint() {
    let server = serve(|_| Reply::status(401));
    let mut cfg = cfg(&server.base);
    cfg.set("musicbrainz.user", "u".into());
    cfg.set("musicbrainz.pass", "p".into());
    let api = MusicBrainzApi::new(&cfg);
    let err = api
        .get_json(&format!("{}/collection", api.api_root), &[])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "HTTP Error: 401 Unauthorized. Check your musicbrainz.user and musicbrainz.pass configuration"
    );
}

#[test]
fn browse_returns_the_entity_list() {
    let server = serve(|_| Reply::json(json!({"release-groups": [{"id": "rg"}]})));
    let api = MusicBrainzApi::new(&cfg(&server.base));
    let groups = api.browse("release-group", &[], &[]).unwrap();
    assert_eq!(groups[0]["id"], "rg");
}

#[test]
fn recording_with_releases_normalizes_artist_credit() {
    let server = serve(|_| {
        Reply::json(json!({
            "title": "T",
            "artist-credit": [{"artist": {"name": "A"}}],
            "releases": [{"title": "Al", "date": "2023-01"}]
        }))
    });
    let api = MusicBrainzApi::new(&cfg(&server.base));
    let rec = api.recording_with_releases("id1").unwrap();
    assert_eq!(rec["artist_credit"][0]["artist"]["name"], "A");
    assert!(server.requests()[0].target.contains("inc=releases"));
}
