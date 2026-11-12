//! beets' test/plugins/test_lyrics.py, with web services served by a fake
//! server on 127.0.0.1 (port 0) and beets' page fixtures.

use super::backends::{self, Backend, SearchResult};
use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lyrics")
        .join(name);
    std::fs::read_to_string(p).unwrap()
}

fn conf_with(yaml: &str) -> Conf {
    let mut cfg = Config::defaults();
    let y: Y = serde_yaml::from_str(yaml).unwrap();
    if let Y::Mapping(m) = y {
        for (k, v) in m {
            cfg.set(&format!("lyrics.{}", k.as_str().unwrap()), v);
        }
    }
    Conf::new(&cfg)
}

/// The plugin with `yaml` settings over the defaults, its services all at
/// `base` (when given).
fn plugin(yaml: &str, base: Option<&str>) -> LyricsPlugin {
    let mut conf = conf_with(yaml);
    let mut urls = Urls::default();
    if let Some(b) = base {
        urls = Urls {
            lrclib: format!("{b}/api"),
            google: format!("{b}/customsearch/v1"),
            genius: format!("{b}/search"),
            tekstowo: b.to_string(),
            musixmatch: format!("{b}/lyrics"),
            translate: format!("{b}/translate"),
        };
        conf.lrcmux_url = format!("{b}/lrcmux/");
    }
    LyricsPlugin::with_conf(conf, urls)
}

fn item(pairs: &[(&str, &str)]) -> Item {
    let mut i = Item::new_item();
    for (k, v) in pairs {
        i.set_parsed(k, v).unwrap();
    }
    i
}

// ---------------------------------------------------------------- utils

#[test]
fn search_empty() {
    for (artist, title) in [
        ("Various Artists", "Title"),
        ("Artist", ""),
        ("", "Title"),
        (" ", ""),
        ("", " "),
        ("", ""),
    ] {
        let i = item(&[("artist", artist), ("title", title)]);
        assert!(search_pairs(&i).is_empty(), "{artist:?} {title:?}");
    }
}

#[test]
fn search_pairs_artists() {
    for (artist, sort, extra) in [
        ("Alice ft. Bob", "", vec!["Alice"]),
        ("Alice feat Bob", "", vec!["Alice"]),
        ("Alice feat. Bob", "", vec!["Alice"]),
        ("Alice feats Bob", "", vec![]),
        ("Alice featuring Bob", "", vec!["Alice"]),
        ("Alice & Bob", "", vec!["Alice"]),
        ("Alice and Bob", "", vec!["Alice"]),
        ("Alice", "", vec![]),
        ("Alice", "Alice", vec![]),
        ("Alice", "alice", vec![]),
        ("Alice", "alice ", vec![]),
        ("Alice", "Alice A", vec!["Alice A"]),
        ("CHVRCHΞS", "CHVRCHES", vec!["CHVRCHES"]),
        ("横山克", "Masaru Yokoyama", vec!["Masaru Yokoyama"]),
    ] {
        let i = item(&[("artist", artist), ("artist_sort", sort), ("title", "song")]);
        let artists: Vec<String> = search_pairs(&i).into_iter().map(|(a, _)| a).collect();
        let mut expected = vec![artist];
        expected.extend(extra);
        assert_eq!(artists, expected, "{artist}");
    }
}

#[test]
fn search_pairs_titles() {
    for (title, extra) in [
        ("1/2", vec![]),
        ("1 / 2", vec!["1", "2"]),
        ("Song (live)", vec!["Song"]),
        ("Song (live) (new)", vec!["Song"]),
        ("Song (live (new))", vec!["Song"]),
        ("Song ft. B", vec!["Song"]),
        ("Song featuring B", vec!["Song"]),
        ("Song and B", vec![]),
        ("Song: B", vec!["Song"]),
    ] {
        let i = item(&[("artist", "A"), ("title", title)]);
        let mut titles: Vec<String> = vec![];
        for (_, ts) in search_pairs(&i) {
            for t in ts {
                if !titles.contains(&t) {
                    titles.push(t);
                }
            }
        }
        let mut expected = vec![title];
        expected.extend(extra);
        assert_eq!(titles, expected, "{title}");
    }
}

#[test]
fn slugs() {
    for (text, expected) in [
        ("test", "test"),
        ("Mørdag", "mordag"),
        ("l'été c'est fait pour jouer", "l-ete-c-est-fait-pour-jouer"),
        ("\u{e7}afe au lait (boisson)", "cafe-au-lait-boisson"),
        (
            "Multiple  spaces -- and symbols! -- merged",
            "multiple-spaces-and-symbols-merged",
        ),
        ("\u{200b}no-width-space", "no-width-space"),
        ("El\u{2d}p", "el-p"),
        ("\u{200b}blackbear", "blackbear"),
        ("\u{200d}", ""),
        ("\u{2010}", ""),
    ] {
        assert_eq!(slug(text), expected, "{text:?}");
    }
}

#[test]
fn sanitized_sources() {
    let s = |v: &[&str]| {
        sanitize_choices(
            &v.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            backends::NAMES,
        )
    };
    assert_eq!(
        s(&["genius", "nope", "genius", "lrclib"]),
        ["genius", "lrclib"]
    );
    assert_eq!(
        s(&["tekstowo", "*"]),
        [
            "tekstowo",
            "lrclib",
            "google",
            "genius",
            "musixmatch",
            "lrcmux"
        ]
    );
}

// ---------------------------------------------------------- SearchBackend

#[test]
fn check_match() {
    for (thresh, target, artist, matches) in [
        (0.11, "Target Artist", "Target Artist", true),
        (0.11, "Target Artist", "Target Artis", true),
        (0.11, "Target Artist", "Target Arti", false),
        (0.11, "Psychonaut", "Psychonaut (BEL)", true),
        (0.11, "beets song", "beats song", true),
        (0.10, "beets song", "beats song", false),
        (
            0.11,
            "Lucid Dreams (Forget Me)",
            "Lucid Dreams (Remix) ft. Lil Uzi Vert",
            false,
        ),
        (
            0.12,
            "Lucid Dreams (Forget Me)",
            "Lucid Dreams (Remix) ft. Lil Uzi Vert",
            true,
        ),
    ] {
        let p = plugin(&format!("dist_thresh: {thresh}"), None);
        let r = SearchResult {
            artist: artist.into(),
            title: String::new(),
            url: String::new(),
        };
        assert_eq!(
            Backend::Genius.check_match(&p, target, "", &r),
            matches,
            "{thresh} {target} {artist}"
        );
    }
}

// ----------------------------------------------------------- the plugin

#[test]
fn error_handling() {
    let s = serve(|r| {
        if r.target.contains("bad") {
            Reply::status(502)
        } else {
            Reply::text("invalid")
        }
    });
    let p = plugin("sources: [lrclib]", Some(&s.base));
    let (found, log) = crate::logging::capture(|| p.get_lyrics("bad", "", "", 0));
    assert!(found.is_none());
    let last = log.last().unwrap();
    assert!(
        last.starts_with("lyrics: LRCLib: Request error: ") && last.contains("502"),
        "{last}"
    );
    let (found, log) = crate::logging::capture(|| p.get_lyrics("x", "", "", 0));
    assert!(found.is_none());
    assert_eq!(
        log.last().unwrap(),
        "lyrics: LRCLib: Could not decode response JSON data"
    );
}

fn lib_item(lib: &Library, lyrics: &str) -> Item {
    let mut i = item(&[
        ("artist", "the artist"),
        ("album", "the album"),
        ("title", "the title"),
        ("lyrics", lyrics),
    ]);
    i.set_path("/nonexistent/x.mp3");
    lib.add(&mut i).unwrap();
    i
}

#[test]
fn overwrite_config() {
    let cases: &[(&str, &str, Option<&str>, &str)] = &[
        ("{}", "old", Some("new"), "old"),
        ("force: yes", "old", Some("new"), "new"),
        ("{force: yes, local: yes}", "old", Some("new"), "old"),
        ("{force: yes, fallback: null}", "old", None, "old"),
        ("{force: yes, fallback: ''}", "old", None, ""),
        ("{force: yes, fallback: default}", "old", None, "default"),
        (
            "{force: yes, synced: yes}",
            "[00:00.00] old synced",
            Some("new plain"),
            "[00:00.00] old synced",
        ),
        (
            "{force: yes, synced: yes}",
            "[00:00.00] old synced",
            Some("[00:00.00] new synced"),
            "[00:00.00] new synced",
        ),
        (
            "{force: yes, synced: no}",
            "[00:00.00] old synced",
            Some("new plain"),
            "new plain",
        ),
        (
            "{force: yes, keep_synced: yes}",
            "[00:00.00] old synced",
            Some("new"),
            "[00:00.00] old synced",
        ),
    ];
    for (yaml, old, found, expected) in cases {
        let lib = Library::in_memory().unwrap();
        let p = plugin(yaml, None);
        let mut i = lib_item(&lib, old);
        let found = found.map(|f| Lyrics::new(f, None, None));
        crate::logging::capture(|| {
            p.add_item_lyrics_with(&lib, &mut i, false, |_| found)
                .unwrap()
        });
        assert_eq!(i.str("lyrics"), *expected, "{yaml} {old}");
    }
}

#[test]
fn set_additional_lyrics_info() {
    let lib = Library::in_memory().unwrap();
    let p = plugin("{}", None);
    let mut i = lib_item(&lib, "");
    i.set_str("lyrics_translation_language", "EN");
    lib.store(&i).unwrap();
    let found = Lyrics::new(
        "sing in the rain every hour of the day",
        Some("lrclib"),
        Some("https://lrclib.net/api/1"),
    );
    crate::logging::capture(|| p.add_item_lyrics_with(&lib, &mut i, false, |_| Some(found)))
        .0
        .unwrap();
    let i = lib.get_item(i.id.unwrap()).unwrap().unwrap();
    assert_eq!(i.str("lyrics_url"), "https://lrclib.net/api/1");
    assert_eq!(i.str("lyrics_instrumental"), "0");
    assert_eq!(i.str("lyrics_backend"), "lrclib");
    // No language detection (as beets without langdetect), and the old
    // translation language is cleared.
    assert!(!i.flex.contains_key("lyrics_language"));
    assert!(!i.flex.contains_key("lyrics_translation_language"));
}

#[test]
fn store_instrumental_lyrics_info() {
    let lib = Library::in_memory().unwrap();
    let p = plugin("{}", None);
    let mut i = lib_item(&lib, "");
    let found = Lyrics::new(
        "[Instrumental]",
        Some("lrclib"),
        Some("https://lrclib.net/api/1"),
    );
    crate::logging::capture(|| p.add_item_lyrics_with(&lib, &mut i, false, |_| Some(found)))
        .0
        .unwrap();
    let i = lib.get_item(i.id.unwrap()).unwrap().unwrap();
    assert_eq!(i.str("lyrics"), "");
    assert_eq!(i.str("lyrics_instrumental"), "1");
    assert_eq!(i.str("lyrics_backend"), "lrclib");
    assert_eq!(i.str("lyrics_url"), "https://lrclib.net/api/1");
}

#[test]
fn log_messages() {
    let lib = Library::in_memory().unwrap();
    let p = plugin("{}", None);
    let mut i = lib_item(&lib, "words");
    let (_, log) = crate::logging::capture(|| p.add_item_lyrics(&lib, &mut i, false));
    assert_eq!(
        log,
        ["lyrics: LyricsPlugin: 🔵 Lyrics already present: the artist - the album - the title"]
    );
    // Import stages log at WARNING (and so only errors) without -v.
    let mut i = lib_item(&lib, "");
    let ctx = PluginCtx {
        cfg: &Config::defaults(),
        lib: &lib,
        write: false,
    };
    let s = serve(|_| Reply::json(json!([])));
    let p = plugin("sources: [lrclib, google]", Some(&s.base));
    let (_, log) = crate::logging::capture(|| p.imported(&ctx, std::slice::from_mut(&mut i)));
    assert_eq!(
        log,
        ["lyrics: LyricsPlugin: Disabling Google source: no API key configured."]
    );
}

#[test]
fn imported_skips_auto_ignored_items() {
    let lib = Library::in_memory().unwrap();
    let s = serve(|r| {
        if r.target.starts_with("/api/search") {
            Reply::json(json!([{"id": 1, "instrumental": false, "duration": 0,
                "plainLyrics": "found", "syncedLyrics": null}]))
        } else {
            Reply::status(404)
        }
    });
    let p = plugin(
        "{sources: [lrclib], auto_ignore: 'album:Greatest Hits'}",
        Some(&s.base),
    );
    let mut items: Vec<Item> = [
        ("Old Song", "Greatest Hits"),
        ("Come Together", "Abbey Road"),
    ]
    .iter()
    .map(|(t, a)| {
        let mut i = item(&[("artist", "The Beatles"), ("title", t), ("album", a)]);
        i.set_path("/nonexistent/x.mp3");
        lib.add(&mut i).unwrap();
        i
    })
    .collect();
    let ctx = PluginCtx {
        cfg: &Config::defaults(),
        lib: &lib,
        write: false,
    };
    p.album_imported(
        &ctx,
        &mut Album::new_album(),
        &mut items,
        Path::new("/"),
        FileOp::Copy,
        false,
    )
    .unwrap();
    assert_eq!(items[0].str("lyrics"), "");
    assert_eq!(items[1].str("lyrics"), "found");
    let searched: Vec<String> = s.requests().iter().map(|r| r.target.clone()).collect();
    assert!(
        searched.iter().all(|t| t.contains("Come+Together")),
        "{searched:?}"
    );
}

// ---------------------------------------------------------------- Google

#[test]
fn google_disabled_without_api_key() {
    let p = plugin("sources: [google]", None);
    let (backends, _) = crate::logging::capture(|| p.backends().to_vec());
    assert!(backends.is_empty());
    let p = plugin("{sources: [google], google_API_key: test}", None);
    assert_eq!(p.backends(), [Backend::Google]);
}

#[test]
fn google_mocked_source_ok() {
    let result = Backend::Google
        .scrape(&fixture("examplecom-beetssong.html"))
        .unwrap()
        .to_lowercase();
    assert!(
        result.contains("via plugins, beets becomes a panacea"),
        "{result}"
    );
}

#[test]
fn google_make_search_result() {
    for (url_title, artist, title) in [
        ("Artist - beets song Lyrics", "Artist", "beets song"),
        (
            "www.azlyrics.com | Beats song by Artist",
            "Artist",
            "Beats song",
        ),
        (
            "lyric.com | seets bong lyrics by Artist",
            "Artist",
            "seets bong",
        ),
        ("foo", "", "foo"),
        (
            "Artist - Beets Song lyrics | AZLyrics",
            "Artist",
            "Beets Song",
        ),
        ("Letra de Artist - Beets Song", "Artist", "Beets Song"),
        ("Letra de Artist - Beets ...", "Artist", "Beets"),
        ("Artist Beets Song", "Artist", "Beets Song"),
        ("BeetsSong - Artist", "Artist", "BeetsSong"),
        ("Artist - BeetsSong", "Artist", "BeetsSong"),
        ("Beets Song", "", "Beets Song"),
        ("Beets Song Artist", "Artist", "Beets Song"),
        (
            "BeetsSong (feat. Other & Another) - Artist",
            "Artist",
            "BeetsSong (feat. Other & Another)",
        ),
        (
            "Beets song lyrics by Artist - original song full text. Official Beets song lyrics, 2024 version | LyricsMode.com",
            "Artist",
            "Beets song",
        ),
    ] {
        let r = backends::google_search_result(
            "Artist",
            "Beets song",
            &json!({"title": url_title, "link": "http://doesntmatter.com"}),
        );
        assert_eq!(
            (r.artist.as_str(), r.title.as_str()),
            (artist, title),
            "{url_title}"
        );
    }
}

/// A fake server whose handler knows its own base URL.
fn serve_self(handler: impl Fn(&str, &Seen) -> Reply + Send + Sync + 'static) -> Server {
    let base: Arc<OnceLock<String>> = Arc::default();
    let b = base.clone();
    let s = serve(move |r| handler(b.get().unwrap(), r));
    base.set(s.base.clone()).unwrap();
    s
}

#[test]
fn google_fetches_matching_pages() {
    let page = fixture("examplecom-beetssong.html");
    let s = serve_self(move |base, r| {
        if r.target.starts_with("/customsearch") {
            Reply::json(json!({"items": [
                {"title": "Other - Something else", "link": format!("{base}/other")},
                {"title": "Artist - Beets Song Lyrics", "link": format!("{base}/beetssong")},
            ]}))
        } else {
            Reply::text(&page)
        }
    });
    let p = plugin("{sources: [google], google_API_key: test}", Some(&s.base));
    let found = crate::logging::capture(|| p.get_lyrics("Artist", "Beets song", "", 186))
        .0
        .unwrap();
    assert_eq!(found.backend.as_deref(), Some("google"));
    assert_eq!(found.url.unwrap(), format!("{}/beetssong", s.base));
    assert!(
        found
            .text
            .to_lowercase()
            .contains("beets becomes a panacea")
    );
    let q = &s.requests()[0].target;
    assert!(
        q.contains("key=test") && q.contains("q=Artist+Beets+song"),
        "{q}"
    );
    assert!(
        q.contains("siteSearch=www.musixmatch.com&siteSearchFilter=e"),
        "{q}"
    );
    // Only the matching result's page was fetched.
    assert_eq!(s.requests().len(), 2);
}

// ---------------------------------------------------------------- Genius

#[test]
fn genius_scrape() {
    for (file, lines) in [
        ("genius-2pacalleyezonmelyrics.html", 131),
        ("genius-Ttngchinchillalyrics.html", 29),
        ("genius-sample.html", 0),
    ] {
        let result = Backend::Genius.scrape(&fixture(file)).unwrap_or_default();
        assert_eq!(text::splitlines(&result).len(), lines, "{file}");
    }
}

#[test]
fn genius_search_and_errors() {
    let page = fixture("genius-Ttngchinchillalyrics.html");
    let s = serve_self(move |base, r| {
        if r.target.starts_with("/search?q=Bad") {
            Reply {
                status: 401,
                content_type: "application/json",
                body: br#"{"meta": {"status": 401, "message": "Invalid token"}}"#.to_vec(),
            }
        } else if r.target.starts_with("/search") {
            Reply::json(json!({"response": {"hits": [
                {"result": {"artist_names": "TTNG", "title": "Chinchilla",
                            "url": format!("{base}/chinchilla")}}
            ]}}))
        } else {
            Reply::text(&page)
        }
    });
    let p = plugin("sources: [genius]", Some(&s.base));
    let found = crate::logging::capture(|| p.get_lyrics("TTNG", "Chinchilla", "", 0))
        .0
        .unwrap();
    assert_eq!(found.backend.as_deref(), Some("genius"));
    assert_eq!(found.url.unwrap(), format!("{}/chinchilla", s.base));
    assert_eq!(text::splitlines(&found.text).len(), 29);
    let req = &s.requests()[0];
    assert_eq!(req.target, "/search?q=TTNG+Chinchilla");
    assert!(
        req.header("Authorization")
            .unwrap()
            .starts_with("Bearer Ryq93pUGm8bM6eUWwD_M3NOFFDAtp2yEE7W")
    );
    // A non-matching result is skipped.
    assert!(
        crate::logging::capture(|| p.get_lyrics("Someone", "Else", "", 0))
            .0
            .is_none()
    );
    // API errors are logged.
    let (found, log) = crate::logging::capture(|| p.get_lyrics("Bad", "x", "", 0));
    assert!(found.is_none());
    assert_eq!(
        log.last().unwrap(),
        "lyrics: Genius: Request error: Invalid token Status: 401"
    );
}

// -------------------------------------------------------------- Tekstowo

#[test]
fn tekstowo_scrape() {
    for (file, expecting) in [
        ("tekstowo-piosenka24kgoldncityofangels1.html", true),
        (
            "tekstowo-piosenkabeethovenbeethovenpianosonata17tempestthe3rdmovement.html",
            false,
        ),
    ] {
        let r = Backend::Tekstowo.scrape(&fixture(file));
        assert_eq!(r.is_some_and(|t| !t.is_empty()), expecting, "{file}");
    }
}

#[test]
fn tekstowo_search() {
    assert_eq!(
        backends::tekstowo_search_url("https://www.tekstowo.pl", "city of angels", "24kgoldn"),
        "https://www.tekstowo.pl/szukaj,City+Of+Angels+24Kgoldn.html"
    );
    let html = r#"<html><body>
        <div class="flex-group"><a href="/piosenka,24kgoldn,city_of_angels_1.html"
            title="24kGoldn - City of Angels">24kGoldn - City of Angels</a></div>
        <div class="flex-group extra"><a href="/x.html" title="A - B">A - B</a></div>
        <div class="flex-group"><a href="/y.html" title="No separator">y</a></div>
        </body></html>"#;
    assert_eq!(
        backends::tekstowo_results("https://www.tekstowo.pl", html),
        [SearchResult {
            artist: "24kGoldn".into(),
            title: "City of Angels".into(),
            url: "https://www.tekstowo.pl/piosenka,24kgoldn,city_of_angels_1.html".into(),
        }]
    );
}

// ---------------------------------------------------------------- LRCLib

const LYRICS_DURATION: f64 = 950.0;
const ITEM_DURATION: i64 = 999;
const SYNCED: &str = "[00:00.00] synced";

fn lyrics_match(overrides: J) -> J {
    let mut m = json!({
        "id": 1,
        "instrumental": false,
        "duration": LYRICS_DURATION,
        "syncedLyrics": SYNCED,
        "plainLyrics": "plain",
    });
    for (k, v) in overrides.as_object().unwrap() {
        m[k] = v.clone();
    }
    m
}

fn lrclib_server(search: J) -> Server {
    serve(move |r| {
        if r.target.starts_with("/api/search") {
            Reply::json(search.clone())
        } else {
            Reply::status(404)
        }
    })
}

fn lrclib_fetch(yaml: &str, search: J) -> Option<Lyrics> {
    let s = lrclib_server(search);
    let p = plugin(yaml, Some(&s.base));
    Backend::LrcLib
        .fetch(&p, "la", "la", "la", ITEM_DURATION)
        .unwrap()
}

#[test]
fn lrclib_synced_config_option() {
    for (yaml, expected) in [("synced: yes", SYNCED), ("synced: no", "plain")] {
        let l = lrclib_fetch(yaml, json!([lyrics_match(json!({}))])).unwrap();
        assert_eq!(l.text, expected);
        assert_eq!(l.backend.as_deref(), Some("lrclib"));
    }
}

#[test]
fn lrclib_null_plain_lyrics_falls_back_to_synced() {
    let l = lrclib_fetch(
        "synced: no",
        json!([lyrics_match(json!({"plainLyrics": null}))]),
    )
    .unwrap();
    assert_eq!(l.text, "synced");
}

#[test]
fn lrclib_fetch_lyrics() {
    let m = |o: J| lyrics_match(o);
    let cases: Vec<(&str, J, Option<&str>)> = vec![
        ("handle non-matching lyrics", json!([]), None),
        ("synced when available", json!([m(json!({}))]), Some(SYNCED)),
        (
            "synced with 3-decimal millisecond timestamp",
            json!([m(
                json!({"syncedLyrics": "[00:01.234] 3-decimal synced lyrics"})
            )]),
            Some("[00:01.234] 3-decimal synced lyrics"),
        ),
        (
            "none: duration too short",
            json!([m(json!({"duration": 1}))]),
            None,
        ),
        (
            "instrumental track",
            json!([m(json!({"instrumental": true}))]),
            Some(""),
        ),
        (
            "plain by default",
            json!([m(json!({"syncedLyrics": null}))]),
            Some("plain"),
        ),
        (
            "none: no lyrics text despite instrumental being False",
            json!([m(json!({"plainLyrics": null, "syncedLyrics": null}))]),
            None,
        ),
        (
            "prefer synced lyrics even if plain duration is closer",
            json!([
                m(json!({"duration": ITEM_DURATION, "syncedLyrics": null,
                         "plainLyrics": "plain with closer duration"})),
                m(json!({"syncedLyrics": SYNCED, "plainLyrics": "plain 2"})),
            ]),
            Some(SYNCED),
        ),
        (
            "ignore synced with invalid duration",
            json!([
                m(json!({"duration": ITEM_DURATION, "syncedLyrics": null,
                         "plainLyrics": "valid plain"})),
                m(json!({"duration": 1, "syncedLyrics": "synced with invalid duration"})),
            ]),
            Some("valid plain"),
        ),
        (
            "ignore synced with a timestamp longer than duration",
            json!([m(
                json!({"duration": 59, "syncedLyrics": "[01:00.00] invalid synced"})
            )]),
            None,
        ),
        (
            "prefer match with synced lyrics",
            json!([m(json!({"syncedLyrics": null})), m(json!({}))]),
            Some(SYNCED),
        ),
    ];
    for (name, search, expected) in cases {
        let l = lrclib_fetch("synced: yes", search);
        assert_eq!(l.as_ref().map(|l| l.text.as_str()), expected, "{name}");
        if let Some(l) = l {
            assert!(l.url.unwrap().ends_with("/api/get/1"), "{name}");
        }
    }
}

#[test]
fn lrclib_get_fallback_and_params() {
    let s = serve(|r| {
        if r.target.starts_with("/api/search") {
            Reply::json(json!([]))
        } else {
            Reply::json(json!({"id": 7, "instrumental": false, "duration": 200.0,
                "plainLyrics": "from get", "syncedLyrics": null}))
        }
    });
    let p = plugin("{}", Some(&s.base));
    let l = Backend::LrcLib
        .fetch(&p, "A & B", "Song", "The Album", 201)
        .unwrap()
        .unwrap();
    assert_eq!(l.text, "from get");
    assert_eq!(l.url.unwrap(), format!("{}/api/get/7", s.base));
    let targets: Vec<String> = s.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        targets,
        [
            "/api/search?artist_name=A+%26+B&track_name=Song",
            "/api/get?artist_name=A+%26+B&track_name=Song&duration=201&album_name=The+Album",
        ]
    );
}

// ---------------------------------------------------------------- LRCMux

#[test]
fn lrcmux_fetch_lyrics() {
    for (yaml, text, format, level, sources) in [
        ("synced: yes", SYNCED, "lrc", "line", None),
        ("synced: no", "plain", "txt", "none", None),
        (
            "{synced: yes, lrcmux: {sources: [ytmusic]}}",
            SYNCED,
            "lrc",
            "line",
            Some("ytmusic"),
        ),
    ] {
        let body = text.to_string();
        let s = serve(move |_| Reply::text(&format!("\n{body}\n")));
        let mut p = plugin(yaml, Some(&s.base));
        if sources.is_some() {
            p.conf.lrcmux_sources = vec!["ytmusic".into()];
        }
        let l = Backend::LrcMux
            .fetch(&p, "la", "la", "la", 0)
            .unwrap()
            .unwrap();
        assert_eq!(l.text, text);
        assert_eq!(l.backend.as_deref(), Some("lrcmux"));
        let url = l.url.unwrap();
        assert!(
            url.starts_with(&format!("{}/lrcmux/get?artist=la", s.base)),
            "{url}"
        );
        assert!(url.contains(&format!("format={format}")), "{url}");
        assert!(url.contains(&format!("level={level}")), "{url}");
        if let Some(src) = sources {
            assert!(url.contains(&format!("sources={src}")), "{url}");
        }
        assert_eq!(s.requests()[0].target, url.strip_prefix(&s.base).unwrap());
    }
}

#[test]
fn lrcmux_not_found_and_empty() {
    let s = serve(|r| {
        if r.target.contains("title=missing") {
            Reply::status(404)
        } else {
            Reply::text("  ")
        }
    });
    let p = plugin("{}", Some(&s.base));
    assert!(
        Backend::LrcMux
            .fetch(&p, "la", "missing", "", 0)
            .unwrap()
            .is_none()
    );
    assert!(
        Backend::LrcMux
            .fetch(&p, "la", "la", "", 0)
            .unwrap()
            .is_none()
    );
}

// ------------------------------------------------------------ MusiXmatch

#[test]
fn musixmatch_fetch() {
    let s = serve(|r| {
        if r.target.contains("Blocked") {
            Reply::text("We detected that your IP is blocked")
        } else {
            Reply::text(
                "<head>x</head><p class=\"mxm-lyrics__content x\">line one\\nline two</p>\n",
            )
        }
    });
    let p = plugin("{}", Some(&s.base));
    let l = Backend::MusiXmatch
        .fetch(&p, "The Artist", "Song", "", 0)
        .unwrap()
        .unwrap();
    assert_eq!(l.url.unwrap(), format!("{}/lyrics/The-Artist/Song", s.base));
    // As beets: the text after the first "<p ..." is kept, and so is the
    // newline after "</p>".
    assert_eq!(l.text, "x</head>\nline one\nline two\n");
    let (r, log) = crate::logging::capture(|| Backend::MusiXmatch.fetch(&p, "Blocked", "x", "", 0));
    assert!(r.unwrap().is_none());
    assert_eq!(log, ["lyrics: MusiXmatch: Failed: Blocked IP address"]);
}

// ----------------------------------------------------------- Translation

fn bing_server() -> Server {
    serve(|r| {
        let body = String::from_utf8_lossy(&r.body);
        let translations = if body.contains("Refrain") {
            " | [Refrain : Doja Cat] | Difficile pour moi de te laisser partir (Te laisser partir, te laisser partir) | Mon corps ne me laissait pas le cacher (Cachez-le) | [Chorus] | Quoi qu\u{2019}il arrive, je ne plierais pas (Ne plierait pas, ne plierais pas) | Chevauchant à travers le tonnerre, la foudre"
        } else if body.contains("00:00.00") {
            " | [00:00.00] Quelques paroles synchronisées | [00:01.00] Quelques paroles plus synchronisées"
        } else {
            " | Quelques paroles synchronisées | Quelques paroles plus synchronisées"
        };
        Reply::json(json!([{
            "detectedLanguage": {"language": "en", "score": 1.0},
            "translations": [{"text": translations, "to": "fr"}],
        }]))
    })
}

fn dedent(s: &str) -> String {
    let lines: Vec<&str> = s.split('\n').collect();
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                ""
            } else {
                &l[indent..]
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn translate() {
    let s = bing_server();
    let p = plugin("{}", Some(&s.base));
    // beets' test needs langdetect (to know the lyrics are English); with
    // no `from_languages` the language needn't be known.
    let bing = Translator::new("123", "fr", &[]);
    let cases = [
        (
            "
            [Refrain: Doja Cat]
            Hard for me to let you go (Let you go, let you go)
            My body wouldn't let me hide it (Hide it)
            [Chorus]
            No matter what, I wouldn't fold (Wouldn't fold, wouldn't fold)
            Ridin' through the thunder, lightnin'",
            Lyrics::default(),
            "
            [Refrain: Doja Cat] / [Refrain : Doja Cat]
            Hard for me to let you go (Let you go, let you go) / Difficile pour moi de te laisser partir (Te laisser partir, te laisser partir)
            My body wouldn't let me hide it (Hide it) / Mon corps ne me laissait pas le cacher (Cachez-le)
            [Chorus]
            No matter what, I wouldn't fold (Wouldn't fold, wouldn't fold) / Quoi qu\u{2019}il arrive, je ne plierais pas (Ne plierait pas, ne plierais pas)
            Ridin' through the thunder, lightnin' / Chevauchant à travers le tonnerre, la foudre",
        ),
        (
            "
            [00:00.00] Some synced lyrics
            [00:00.50]
            [00:01.00] Some more synced lyrics
            ",
            Lyrics::default(),
            "
            [00:00.00] Some synced lyrics / Quelques paroles synchronisées
            [00:00.50]
            [00:01.00] Some more synced lyrics / Quelques paroles plus synchronisées",
        ),
        (
            "Some lyrics",
            Lyrics {
                text: "Some lyrics / Some translation".into(),
                language: Some("EN".into()),
                translation_language: Some("FR".into()),
                ..Default::default()
            },
            "Some lyrics / Some translation",
        ),
    ];
    for (new, old, expected) in cases {
        let (out, _) = crate::logging::capture(|| {
            bing.translate(&p, Lyrics::new(dedent(new), None, None), &old)
        });
        assert_eq!(out.full_text(), dedent(expected));
    }
    let req = &s.requests()[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.target, "/translate?api-version=3.0&to=FR");
    assert_eq!(req.header("Ocp-Apim-Subscription-Key"), Some("123"));
    // With `from_languages`, the (undetected) language must be listed.
    let bing = Translator::new("123", "fr", &["en".to_string()]);
    let (out, log) = crate::logging::capture(|| {
        bing.translate(&p, Lyrics::new("Hello", None, None), &Lyrics::default())
    });
    assert_eq!(out.full_text(), "Hello");
    assert_eq!(
        log,
        ["lyrics: Translator: 🔵 Configuration ['EN'] does not permit translating from None"]
    );
}

#[test]
fn translator_needs_key_and_language() {
    assert!(conf_with("translate: {api_key: k}").translator.is_none());
    let t = conf_with("translate: {api_key: k, to_language: de, from_languages: [en, fr]}")
        .translator
        .unwrap();
    assert_eq!((t.to_language.as_str(), t.from_languages.len()), ("DE", 2));
}

// ------------------------------------------------------ the command

#[test]
fn command_options_and_rest_directory() {
    let lib = Library::in_memory().unwrap();
    let d = tempfile::tempdir().unwrap();
    let mut i = lib_item(&lib, "hello");
    i.set_str("albumartist", "Some Artist");
    lib.store(&i).unwrap();
    let mut conf = conf_with("{}");
    conf.rest_directory = Some(d.path().join("rest").display().to_string());
    let p = LyricsPlugin::with_conf(conf, Urls::default());
    crate::logging::capture(|| run_command(&p, &lib, false, &CmdOpts::default()))
        .0
        .unwrap();
    assert!(d.path().join("rest/artists/some-artist.rst").exists());
    assert_eq!(expanduser("~/x"), dirs::home_dir().unwrap().join("x"));
    assert_eq!(expanduser("a/~b"), PathBuf::from("a/~b"));
}

#[test]
fn sylt_written_to_mp3() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("x.mp3");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.3"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return;
    }
    let lib = Library::in_memory().unwrap();
    let mut i = crate::media::read(&path).unwrap();
    i.set_str("artist", "a");
    i.set_str("title", "t");
    lib.add(&mut i).unwrap();
    let p = plugin("{}", None);
    let lrc = "[00:01.00] hello\n[00:02.00] world";
    crate::logging::capture(|| {
        p.add_item_lyrics_with(&lib, &mut i, true, |_| Some(Lyrics::new(lrc, None, None)))
    })
    .0
    .unwrap();
    assert_eq!(i.str("lyrics"), lrc);
    let mf = crate::mediafile::MediaFile::open(&path).unwrap();
    assert_eq!(
        mf.synced_lyrics().unwrap(),
        [("hello".to_string(), 1000), ("world".to_string(), 2000)]
    );
    assert_eq!(crate::media::read(&path).unwrap().str("lyrics"), lrc);
    // Plain lyrics remove the SYLT frame.
    let mut conf = conf_with("{force: yes}");
    conf.force = true;
    let p = LyricsPlugin::with_conf(conf, Urls::default());
    crate::logging::capture(|| {
        p.add_item_lyrics_with(&lib, &mut i, true, |_| {
            Some(Lyrics::new("plain", None, None))
        })
    })
    .0
    .unwrap();
    let mf = crate::mediafile::MediaFile::open(&path).unwrap();
    assert_eq!(mf.synced_lyrics(), None);
    // mutagen reads what we wrote.
    let py = dirs::home_dir()
        .unwrap()
        .join(".cache/beets-reference/venv/bin/python");
    if py.exists() {
        i.set_str("lyrics", "");
        lib.store(&i).unwrap();
        crate::logging::capture(|| {
            p.add_item_lyrics_with(&lib, &mut i, true, |_| Some(Lyrics::new(lrc, None, None)))
        })
        .0
        .unwrap();
        let out = std::process::Command::new(py)
            .args([
                "-I",
                "-c",
                "import sys, mutagen; f = mutagen.File(sys.argv[1]); \
                 print(f.tags['SYLT::XXX'].text, f.tags['SYLT::XXX'].format, f.tags['SYLT::XXX'].type)",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "[('hello', 1000), ('world', 2000)] 2 1"
        );
    }
}
