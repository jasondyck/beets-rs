//! Ported from beets' `test/plugins/test_embyupdate.py`.

use super::*;
use crate::plugins::requests::testserver::serve;
use serde_json::json;

#[test]
fn api_url_hosts() {
    assert_eq!(
        api_url("localhost", 8096, "/Library/Refresh"),
        "http://localhost:8096/Library/Refresh?format=json"
    );
    assert_eq!(
        api_url("http://localhost", 8096, "/Library/Refresh"),
        "http://localhost:8096/Library/Refresh?format=json"
    );
    assert_eq!(
        api_url("https://localhost", 8096, "/Library/Refresh"),
        "https://localhost:8096/Library/Refresh?format=json"
    );
    // Existing query parameters are kept; `format` is set.
    assert_eq!(
        api_url("localhost", 80, "/x?a=b&format=xml"),
        "http://localhost:80/x?a=b&format=json"
    );
}

#[test]
fn password_data_hashes() {
    let d = password_data("username", "password");
    assert_eq!(d["username"], "username");
    assert_eq!(d["password"], "5baa61e4c9b93f3f0682250b6cf8331b7ee68fd8");
    assert_eq!(d["passwordMd5"], "5f4dcc3b5aa765d61d8327deb882cf99");
}

#[test]
fn headers_with_and_without_token() {
    let id = "e8837bc1-ad67-520e-8cd2-f629e3155721";
    assert_eq!(
        create_headers(id, None),
        vec![(
            "x-emby-authorization".to_string(),
            "MediaBrowser UserId=\"e8837bc1-ad67-520e-8cd2-f629e3155721\", \
             Client=\"other\", Device=\"beets\", DeviceId=\"beets\", Version=\"0.0.0\""
                .to_string()
        )]
    );
    let with = create_headers(id, Some("abc123"));
    assert_eq!(with.len(), 2);
    assert_eq!(with[1], ("x-mediabrowser-token".into(), "abc123".into()));
}

#[test]
fn get_token_and_user_from_the_api() {
    let user = json!({
        "Name": "username",
        "Id": "2ec276a2642e54a19b612b9418a8bd3b",
        "HasPassword": true,
    });
    let server = serve(move |seen| {
        if seen.target.starts_with("/Users/AuthenticateByName") {
            crate::plugins::requests::testserver::Reply::json(json!({
                "User": user,
                "AccessToken": "4b19180cf02748f7b95c7e8e76562fc8",
            }))
        } else if seen.target.starts_with("/Users/Public") {
            crate::plugins::requests::testserver::Reply::json(json!([user]))
        } else {
            crate::plugins::requests::testserver::Reply::status(404)
        }
    });
    let (host, port) = {
        let hp = server.base.trim_start_matches("http://");
        let (h, p) = hp.split_once(':').unwrap();
        (h.to_string(), p.parse::<i64>().unwrap())
    };
    let session = Session::plain("test");
    let auth = password_data("username", "password");
    let headers = create_headers("e8837bc1-ad67-520e-8cd2-f629e3155721", None);
    assert_eq!(
        get_token(&session, &host, port, &headers, &auth).unwrap(),
        Some("4b19180cf02748f7b95c7e8e76562fc8".to_string())
    );
    let users = get_user(&session, &host, port, "username").unwrap();
    assert_eq!(
        users[0].get("Id").and_then(J::as_str),
        Some("2ec276a2642e54a19b612b9418a8bd3b")
    );
    assert!(get_user(&session, &host, port, "other").unwrap().is_empty());
}
