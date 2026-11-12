//! beets' test/plugins/test_lastgenre.py, with Last.fm served by a fake
//! server on 127.0.0.1 (port 0).

use super::client::{Kind, LastFmClient, LastFmError, TopTag, filter_weight, parse_top_tags};
use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use std::cell::RefCell;
use std::rc::Rc;

fn set_deep(cfg: &mut Config, prefix: &str, y: Y) {
    match y {
        Y::Mapping(m) if !prefix.ends_with("ignorelist") && !prefix.ends_with("aliases") => {
            for (k, v) in m {
                let key = format!("{prefix}.{}", py_str(&k));
                set_deep(cfg, &key, v);
            }
        }
        v => cfg.set(prefix, v),
    }
}

fn config(yaml: &str) -> Config {
    let mut cfg = Config::defaults();
    set_deep(&mut cfg, "lastgenre", serde_yaml::from_str(yaml).unwrap());
    cfg
}

fn plugin(yaml: &str) -> LastGenre {
    LastGenre::new(&config(yaml)).unwrap()
}

fn set(v: &[&str]) -> HashSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `_setup_config`: whitelist off, canonical off, count 1 by default.
fn setup(whitelist: &str, canonical: &str, count: i64, prefer_specific: bool) -> LastGenre {
    plugin(&format!(
        "{{whitelist: {whitelist}, canonical: {canonical}, count: {count}, prefer_specific: {prefer_specific}}}"
    ))
}

fn setup_wl(whitelist: &[&str], canonical: bool, count: i64) -> LastGenre {
    let mut p = setup("no", if canonical { "yes" } else { "no" }, count, false);
    p.whitelist = set(whitelist);
    p
}

/// Keys (artists or alias templates) with their patterns.
type Spec<'a> = &'a [(&'a str, &'a [&'a str])];

fn pats(v: Spec) -> IgnorePatterns {
    v.iter()
        .map(|(a, ps)| {
            (
                a.to_string(),
                ps.iter().map(|p| Pat::new(p).unwrap()).collect(),
            )
        })
        .collect()
}

#[test]
fn default() {
    let p = setup("no", "no", 1, false);
    assert_eq!(p.resolve_genres(&["delta blues"], ""), ["delta blues"]);
}

#[test]
fn c14n_only() {
    let p = setup("no", "yes", 99, false);
    assert_eq!(p.resolve_genres(&["delta blues"], ""), ["blues"]);
    assert_eq!(p.resolve_genres(&["iota blues"], ""), ["iota blues"]);
}

#[test]
fn whitelist_only() {
    let p = setup("yes", "no", 1, false);
    assert!(p.resolve_genres(&["iota blues"], "").is_empty());
}

#[test]
fn whitelist_c14n() {
    let p = setup("yes", "yes", 99, false);
    assert_eq!(
        p.resolve_genres(&["delta blues"], ""),
        ["delta blues", "blues"]
    );
}

#[test]
fn whitelist_custom() {
    let p = setup_wl(&["blues", "rock", "jazz"], false, 2);
    assert_eq!(p.resolve_genres(&["pop", "blues"], ""), ["blues"]);
    let p = setup_wl(&["rock"], true, 1);
    assert!(p.resolve_genres(&["delta blues"], "").is_empty());
}

#[test]
fn format_genres() {
    let p = setup("no", "no", 2, false);
    assert_eq!(
        p.format_genres(s(&["jazz", "pop", "rock", "blues"])),
        ["Jazz", "Pop", "Rock", "Blues"]
    );
}

#[test]
fn title_case_as_python() {
    assert_eq!(py_title("hip-hop"), "Hip-Hop");
    assert_eq!(py_title("r&b"), "R&B");
    assert_eq!(py_title("rock'n'roll"), "Rock'N'Roll");
    assert_eq!(py_title("80s dj's"), "80S Dj'S");
    assert_eq!(py_title("ǆa straße ROCK"), "ǅa Straße Rock");
}

#[test]
fn count_c14n() {
    let p = setup_wl(&["blues", "rock", "jazz"], true, 2);
    // 'blues' supersedes 'country blues' and takes the second slot.
    assert_eq!(
        p.resolve_genres(&["jazz", "pop", "country blues", "rock"], ""),
        ["jazz", "blues"]
    );
}

#[test]
fn c14n_whitelist() {
    let p = setup_wl(&["rock"], true, 1);
    assert!(p.resolve_genres(&["delta blues"], "").is_empty());
}

#[test]
fn empty_canonical_and_whitelist_raise_user_error() {
    for v in ["''", "null"] {
        let e = LastGenre::new(&config(&format!("{{canonical: {v}}}")))
            .err()
            .unwrap();
        assert!(e.to_string().contains("Invalid canonical setting"), "{e}");
        let e = LastGenre::new(&config(&format!("{{whitelist: {v}}}")))
            .err()
            .unwrap();
        assert!(e.to_string().contains("Invalid whitelist setting"), "{e}");
    }
}

#[test]
fn prefer_specific_loads_tree() {
    let p = setup("no", "no", 1, true);
    assert!(!p.c14n_branches.is_empty());
    assert!(!p.canonicalize);
}

#[test]
fn prefer_specific_without_canonical() {
    let p = setup("no", "no", 4, true);
    assert_eq!(
        p.resolve_genres(&["math rock", "post-rock"], ""),
        ["post-rock", "math rock"]
    );
}

#[test]
fn no_duplicate() {
    let p = setup("no", "no", 99, false);
    assert_eq!(p.resolve_genres(&["blues", "blues"], ""), ["blues"]);
}

fn item(pairs: &[(&str, &str)]) -> Item {
    let mut i = Item::new_item();
    for (k, v) in pairs {
        i.set_parsed(k, v).unwrap();
    }
    i
}

#[test]
fn fetch_va_genres_picks_most_popular_track_genre() {
    let mut p = plugin("{whitelist: no, source: track}");
    p.fetch_override = Some(Box::new(|kind, obj, _| {
        if kind != Kind::Track {
            return vec![];
        }
        match obj.str("artist") {
            "Artist C" => s(&["Jazz"]),
            _ => s(&["Rock"]),
        }
    }));
    let items: Vec<Item> = ["Artist A", "Artist B", "Artist C"]
        .iter()
        .map(|a| item(&[("artist", a)]))
        .collect();
    assert_eq!(p.fetch_va_genres(&items), ["Rock"]);
}

#[test]
fn fetch_va_genres_falls_back_to_artist_genre() {
    let mut p = plugin("{whitelist: no}");
    p.fetch_override = Some(Box::new(|kind, _, _| {
        if kind == Kind::Artist {
            s(&["Jazz"])
        } else {
            vec![]
        }
    }));
    assert_eq!(
        p.fetch_va_genres(&[item(&[("artist", "Artist A")])]),
        ["Jazz"]
    );
    p.fetch_override = Some(Box::new(|_, _, _| vec![]));
    assert!(
        p.fetch_va_genres(&[item(&[("artist", "Artist A")])])
            .is_empty()
    );
}

#[test]
fn fetch_genre_weights() {
    let tags = || {
        vec![
            TopTag {
                name: "Pop".into(),
                weight: 90.0,
            },
            TopTag {
                name: "Rap".into(),
                weight: 40.0,
            },
        ]
    };
    assert_eq!(filter_weight(tags(), 0), ["pop", "rap"]);
    assert_eq!(filter_weight(tags(), 50), ["pop"]);
}

#[test]
fn sort_by_depth_in_tree() {
    let p = setup("no", "yes", 1, false);
    assert_eq!(
        sort_by_depth(
            &["electronic", "ambient", "post-rock", "downtempo"],
            &p.c14n_branches
        ),
        ["post-rock", "downtempo", "ambient", "electronic"]
    );
    // Non-canonical tag ('chill out') present.
    assert_eq!(
        sort_by_depth(&["electronic", "ambient", "chill out"], &p.c14n_branches),
        ["ambient", "electronic"]
    );
}

// Ignorelist tests in resolve_genres and is_ignored.

#[test]
fn ignorelist_filters_genres_in_resolve() {
    let mut p = setup("no", "no", 1, false);
    p.ignore_patterns = pats(&[("the artist", &["^metal$"]), ("*", &["^rock$"])]);
    p.conf.count = 99;
    let r = p.resolve_genres(&["metal", "rock", "jazz"], "the artist");
    assert_eq!(r, ["jazz"]);
}

#[test]
fn ignorelist_stops_c14n_ancestry_walk() {
    let mut p = setup("no", "yes", 99, false);
    p.ignore_patterns = pats(&[("the artist", &["^delta blues$"])]);
    assert!(p.resolve_genres(&["delta blues"], "the artist").is_empty());
}

#[test]
fn ignorelist_c14n_no_whitelist_keeps_oldest_ancestor() {
    let mut p = setup("no", "yes", 99, false);
    p.ignore_patterns = pats(&[("*", &["^jazz$"])]);
    assert_eq!(p.resolve_genres(&["delta blues"], ""), ["blues"]);
}

#[test]
fn ignorelist_c14n_no_whitelist_drops_ignored_ancestor() {
    let mut p = setup("no", "yes", 99, false);
    p.ignore_patterns = pats(&[("*", &["^blues$"])]);
    assert!(p.resolve_genres(&["delta blues"], "").is_empty());
}

#[test]
fn filter_valid() {
    let mut p = setup("no", "no", 1, false);
    assert_eq!(
        p.filter_valid(&["rock", "jazz", "blues"], ""),
        ["rock", "jazz", "blues"]
    );
    assert_eq!(
        p.filter_valid(&["rock", "", "  ", "blues"], ""),
        ["rock", "blues"]
    );
    p.ignore_patterns = pats(&[("*", &["^metal$"])]);
    assert_eq!(
        p.filter_valid(&["rock", "metal", "jazz"], ""),
        ["rock", "jazz"]
    );
    p.ignore_patterns = pats(&[("the artist", &["^noise$"])]);
    assert_eq!(p.filter_valid(&["noise", "rock"], "the artist"), ["rock"]);
    assert_eq!(
        p.filter_valid(&["noise", "rock"], "other"),
        ["noise", "rock"]
    );

    let mut p = setup_wl(&["rock", "blues"], false, 1);
    assert_eq!(
        p.filter_valid(&["rock", "jazz", "blues"], ""),
        ["rock", "blues"]
    );
    p.whitelist = set(&["rock"]);
    assert_eq!(
        p.filter_valid(&["Rock", "ROCK", "rock"], ""),
        ["Rock", "ROCK", "rock"]
    );
    p.whitelist = set(&["rock", "metal", "jazz"]);
    p.ignore_patterns = pats(&[("*", &["^metal$"])]);
    assert_eq!(
        p.filter_valid(&["rock", "metal", "pop", "jazz"], ""),
        ["rock", "jazz"]
    );
}

// _get_genre.

type MockGenres = Vec<(Vec<&'static str>, Vec<&'static str>)>;

/// The client serves `mock`: lookup kind plus explicit arguments to
/// genres; anything else finds nothing.
fn mock_lastfm(p: &mut LastGenre, mock: MockGenres) {
    p.fetch_override = Some(Box::new(move |kind, _, args| {
        let mut key = vec![kind.name().to_string()];
        key.extend(args.iter().cloned());
        mock.iter()
            .find(|(k, _)| k.iter().map(|x| x.to_string()).collect::<Vec<_>>() == key)
            .map(|(_, v)| s(v))
            .unwrap_or_default()
    }));
}

/// `_common.item()`'s relevant fields.
fn common_item(genres: &[&str]) -> Item {
    let mut i = item(&[
        ("title", "the title"),
        ("artist", "the artist"),
        ("albumartist", "the album artist"),
        ("album", "the album"),
    ]);
    i.set_list("genres", genres);
    i.set_list("albumartists", &["Artist A", "Artist B"]);
    i
}

fn check_get_genre(conf: &str, genres: &[&str], mock: MockGenres, expected: (&[&str], &str)) {
    let mut p = plugin(conf);
    mock_lastfm(&mut p, mock);
    let (g, label) = p.get_genre(&common_item(genres), &[]);
    assert_eq!((g, label.as_str()), (s(expected.0), expected.1), "{conf}");
}

fn m(k: &[&'static str], v: &[&'static str]) -> (Vec<&'static str>, Vec<&'static str>) {
    (k.to_vec(), v.to_vec())
}

#[test]
fn get_genre() {
    let base = "force: yes, keep_existing: yes, canonical: no, prefer_specific: no";
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: yes, count: 10}}"),
        &["Blues"],
        vec![m(&["album"], &["Jazz"])],
        (&["Blues", "Jazz"], "keep + album, whitelist"),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: yes, count: 10}}"),
        &["original unknown", "Blues"],
        vec![m(&["album"], &["Jazz"])],
        (&["Blues", "Jazz"], "keep + album, whitelist"),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: yes}}"),
        &[],
        vec![m(&["album"], &["Jazz"])],
        (&["Jazz"], "album, whitelist"),
    );
    check_get_genre(
        &format!("{{{base}, source: artist, whitelist: yes, count: 10}}"),
        &["original unknown", "Blues"],
        vec![m(&["album"], &["Jazz"]), m(&["artist"], &["Pop"])],
        (&["Blues", "Pop"], "keep + artist, whitelist"),
    );
    let noforce = "force: no, keep_existing: no, source: album, whitelist: no, canonical: no, prefer_specific: no";
    check_get_genre(
        &format!("{{{noforce}}}"),
        &["any genre"],
        vec![m(&["album"], &["Jazz"])],
        (&["any genre"], "keep any, no-force"),
    );
    check_get_genre(
        &format!("{{{noforce}}}"),
        &[],
        vec![m(&["album"], &["Jazzin"])],
        (&["Jazzin"], "album, any"),
    );
    check_get_genre(
        "{force: no, keep_existing: no, source: artist, whitelist: yes, canonical: yes, cleanup_existing: yes, prefer_specific: no, count: 10}",
        &["Cosmic Disco"],
        vec![m(&["artist"], &[])],
        (&["Disco", "Electronic"], "keep + cleanup, whitelist"),
    );
    check_get_genre(
        &format!("{{{base}, source: track, whitelist: no, count: 10}}"),
        &["unknown genre"],
        vec![
            m(&["track"], &[]),
            m(&["album"], &[]),
            m(&["artist"], &["Jazz"]),
        ],
        (&["Unknown Genre", "Jazz"], "keep + artist, any"),
    );
    let empty = || vec![m(&["track"], &[]), m(&["album"], &[]), m(&["artist"], &[])];
    check_get_genre(
        &format!("{{{base}, source: track, whitelist: no, fallback: fallback genre}}"),
        &["any existing"],
        empty(),
        (&["any existing"], "original fallback"),
    );
    check_get_genre(
        &format!("{{{base}, source: track, whitelist: yes, fallback: fallback genre}}"),
        &["Jazz"],
        empty(),
        (&["Jazz"], "original fallback"),
    );
    check_get_genre(
        "{force: yes, keep_existing: no, source: track, whitelist: yes, fallback: fallback genre, canonical: no, prefer_specific: no}",
        &["Jazz"],
        empty(),
        (&["fallback genre"], "fallback"),
    );
    check_get_genre(
        &format!("{{{base}, source: track, whitelist: yes, fallback: fallback genre}}"),
        &[],
        empty(),
        (&["fallback genre"], "fallback"),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: yes, count: 5}}"),
        &["original unknown", "Blues", "Rock", "Folk", "Metal"],
        vec![m(&["album"], &["Jazz", "Bebop", "Hardbop"])],
        (
            &["Blues", "Rock", "Metal", "Jazz", "Bebop"],
            "keep + album, whitelist",
        ),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: yes, fallback: fallback genre}}"),
        &["not whitelisted original"],
        vec![m(&["album"], &[]), m(&["artist"], &["Jazz"])],
        (&["Jazz"], "keep + artist, whitelist"),
    );
    check_get_genre(
        "{force: yes, keep_existing: no, source: album, whitelist: yes, canonical: yes, prefer_specific: no, count: 10}",
        &[],
        vec![m(&["album"], &["acid techno"])],
        (&["Techno", "Electronic"], "album, whitelist"),
    );
    check_get_genre(
        "{force: yes, keep_existing: yes, source: album, whitelist: yes, canonical: yes, prefer_specific: no, count: 10, extended_debug: yes}",
        &["detroit techno"],
        vec![m(&["album"], &["acid house"])],
        (
            &[
                "Detroit Techno",
                "Techno",
                "Electronic",
                "Acid House",
                "House",
            ],
            "keep + album, whitelist",
        ),
    );
    let canon = "force: yes, keep_existing: yes, source: album, whitelist: yes, canonical: yes, prefer_specific: no, count: 10";
    check_get_genre(
        &format!("{{{canon}}}"),
        &["Cosmic Disco"],
        vec![m(&["album"], &["Detroit Techno"])],
        (
            &["Disco", "Electronic", "Detroit Techno", "Techno"],
            "keep + album, whitelist",
        ),
    );
    check_get_genre(
        &format!("{{{canon}}}"),
        &["Cosmic Disco"],
        vec![m(&["album"], &[]), m(&["artist"], &[])],
        (
            &["Disco", "Electronic"],
            "keep + original fallback, whitelist",
        ),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: no, count: 10}}"),
        &["Jazz; Funk; Soul"],
        vec![m(&["album"], &[]), m(&["artist"], &[])],
        (&["Jazz", "Funk", "Soul"], "original fallback"),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: yes}}"),
        &["Baroque", "Classical"],
        vec![m(&["album"], &[]), m(&["artist"], &[])],
        (&["Baroque", "Classical"], "original fallback"),
    );
    check_get_genre(
        &format!(
            "{{{base}, source: album, whitelist: yes, aliases: {{hip hop: [hip-hop, hiphop]}}}}"
        ),
        &["hip-hop", "hiphop"],
        vec![m(&["album"], &[]), m(&["artist"], &[])],
        (&["hip hop"], "original fallback"),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: no, count: 10}}"),
        &[],
        vec![m(&["album"], &[]), m(&["artist"], &["Jazz"])],
        (&["Jazz"], "artist, any"),
    );
    check_get_genre(
        &format!("{{{base}, source: album, whitelist: no, count: 10}}"),
        &[],
        vec![
            m(&["album"], &[]),
            m(&["artist"], &["Pop"]),
            m(&["album", "Artist A", "the album"], &["Rock"]),
            m(&["album", "Artist B", "the album"], &["Jazz"]),
        ],
        (&["Rock", "Jazz"], "multi-valued albumartist album, any"),
    );
}

fn test_album() -> Album {
    let mut a = Album::new_album();
    a.set_str("albumartist", "Artist Group");
    a.set_list("albumartists", &["Artist A", "Artist B"]);
    a.set_str("album", "the album");
    a
}

#[test]
fn get_genre_album() {
    let cases: Vec<(MockGenres, (&[&str], &str))> = vec![
        (
            vec![m(&["album"], &[]), m(&["album_artist"], &["Jazz"])],
            (&["Jazz"], "albumartist, any"),
        ),
        (
            vec![
                m(&["album"], &[]),
                m(&["album_artist"], &[]),
                m(&["album_artist", "Artist A"], &["Rock"]),
                m(&["album_artist", "Artist B"], &["Jazz"]),
            ],
            (&["Rock", "Jazz"], "multi-valued albumartist, any"),
        ),
        (
            vec![m(&["album"], &[]), m(&["album_artist"], &[])],
            (&[], "fallback unconfigured"),
        ),
    ];
    for (mock, expected) in cases {
        let mut p = plugin("{source: artist, whitelist: no, count: 10, force: yes}");
        mock_lastfm(&mut p, mock);
        let (g, label) = p.get_genre(&test_album(), &[]);
        assert_eq!((g, label.as_str()), (s(expected.0), expected.1));
    }
}

#[test]
fn ignorelist_patterns() {
    let cases: &[(&str, &str, &str, bool)] = &[
        ("{'*': [spoken word]}", "Any Artist", "spoken word", true),
        ("{'*': [spoken word]}", "Any Artist", "jazz", false),
        (
            "{'*': ['.*electronic.*']}",
            "Any Artist",
            "ambient electronic",
            true,
        ),
        ("{'*': ['.*electronic.*']}", "Any Artist", "jazz", false),
        ("{metallica: [metal]}", "Metallica", "metal", true),
        ("{metallica: [metal]}", "Iron Maiden", "metal", false),
        ("{metallica: [metal]}", "METALLICA", "METAL", true),
        ("{metallica: [metal]}", "Metallica", "heavy metal", false),
        (
            "{metallica: ['.*metal.*']}",
            "Metallica",
            "heavy metal",
            true,
        ),
        (
            "{metallica: ['^Heavy Metal$']}",
            "Metallica",
            "classic metal",
            false,
        ),
        (
            "{'*': [spoken word], metallica: [metal]}",
            "Metallica",
            "spoken word",
            true,
        ),
        (
            "{'*': [spoken word], metallica: [metal]}",
            "Metallica",
            "metal",
            true,
        ),
        (
            r"{fracture: ['^(heavy|black|power|death)?\s?(metal|rock)$|\w+-metal\d*$']}",
            "Fracture",
            "power metal",
            true,
        ),
        (
            "{amon tobin: ['d(rum)?[ n/]*b(ass)?']}",
            "Amon Tobin",
            "dnb",
            true,
        ),
        ("{}", "Any Artist", "any genre", false),
    ];
    for (ignorelist, artist, genre, expected) in cases {
        let p = plugin(&format!("{{ignorelist: {ignorelist}}}"));
        assert_eq!(
            is_ignored(&p.log, &p.ignore_patterns, genre, artist),
            *expected,
            "{ignorelist} {artist} {genre}"
        );
    }
}

#[test]
fn ignorelist_config_format() {
    let cases: &[(&str, Spec)] = &[
        ("{metallica: [metal]}", &[("metallica", &["metal"])]),
        ("{'*': [spoken word]}", &[("*", &["spoken word"])]),
        (
            "{metallica: [metal, '.*rock.*']}",
            &[("metallica", &["metal", ".*rock.*"])],
        ),
        (
            "{'*': [spoken word], metallica: [metal]}",
            &[("*", &["spoken word"]), ("metallica", &["metal"])],
        ),
        (
            "{artist: ['[invalid(regex']}",
            &[("artist", &[r"\[invalid\(regex"])],
        ),
        ("no", &[]),
        ("{}", &[]),
    ];
    for (conf, expected) in cases {
        let p = plugin(&format!("{{ignorelist: {conf}}}"));
        let got: HashMap<String, Vec<String>> = p
            .ignore_patterns
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().map(|p| p.pattern.clone()).collect()))
            .collect();
        let want: HashMap<String, Vec<String>> = expected
            .iter()
            .map(|(k, v)| (k.to_string(), s(v)))
            .collect();
        assert_eq!(got, want, "{conf}");
    }
    // The escaped pattern matches literally.
    let p = plugin("{ignorelist: {artist: ['[invalid(regex']}}");
    assert!(is_ignored(
        &p.log,
        &p.ignore_patterns,
        "[invalid(regex",
        "Artist"
    ));
}

#[test]
fn ignorelist_config_format_errors() {
    for (conf, msg) in [
        (
            "/path/to/ignorelist.txt",
            "lastgenre.ignorelist must be a dict, not str",
        ),
        ("42", "lastgenre.ignorelist must be a dict, not int"),
        (
            "[spoken word, comedy]",
            "lastgenre.ignorelist must be a dict, not list",
        ),
        (
            "{metallica: metal}",
            "lastgenre.ignorelist.metallica must be a list, not str",
        ),
        (
            "{metallica: [1]}",
            "lastgenre.ignorelist.metallica#0: must be a string",
        ),
    ] {
        let e = LastGenre::new(&config(&format!("{{ignorelist: {conf}}}")))
            .err()
            .unwrap();
        assert!(e.downcast_ref::<ConfigError>().is_some());
        assert_eq!(e.to_string(), msg);
    }
}

#[test]
fn ignorelist_multivalued_albumartist_fallback() {
    let mut p = plugin(
        "{ignorelist: {Artist A: [Metal], Artist Group: [Metal]}, whitelist: no, count: 10}",
    );
    p.conf.force = true;
    p.fetch_override = Some(Box::new(|kind, _, args| {
        if kind == Kind::AlbumArtist && !args.is_empty() {
            match args[0].as_str() {
                "Artist A" => s(&["Rock"]),
                _ => s(&["Metal", "Jazz"]),
            }
        } else {
            vec![]
        }
    }));
    let mut a = test_album();
    a.set_str("album", "Album Title");
    let (genres, label) = p.get_genre(&a, &[]);
    assert!(label.contains("multi-valued albumartist"), "{label}");
    assert!(genres.contains(&"Metal".to_string()), "{genres:?}");
}

// Aliases.

fn alias_pats(v: Spec) -> AliasPatterns {
    v.iter()
        .flat_map(|(t, ps)| ps.iter().map(|p| (Pat::new(p).unwrap(), t.to_lowercase())))
        .collect()
}

#[test]
fn normalize_genre_templates() {
    let log = PluginLog::new(NAME);
    let cases: &[(Spec, &str, &str)] = &[
        (&[("foo bar", &["foobar"])], "foobar", "foo bar"),
        (
            &[(r"\1 music", &[r"(fake)[ /-]*music"])],
            "fake-music",
            "fake music",
        ),
        (&[(r"\1-\2", &[r"(x)[ /-]*(y)"])], "x y", "x-y"),
        (&[("foo bar", &["foobar"])], "FOOBAR", "foo bar"),
        (&[("foo bar", &["foobar"])], "jazz", "jazz"),
        (&[], "something", "something"),
    ];
    for (aliases, genre, expected) in cases {
        assert_eq!(
            normalize_genre(&log, &alias_pats(aliases), genre),
            *expected
        );
    }
}

#[test]
fn normalize_genre_invalid_template_does_not_crash() {
    let log = PluginLog::new(NAME);
    let pats = vec![(
        Pat::new(r"(hip)[ /-]*hop").unwrap(),
        r"\g<2> hop".to_string(),
    )];
    let (out, logged) = crate::logging::capture(|| normalize_genre(&log, &pats, "hip-hop"));
    assert_eq!(out, "hip-hop");
    assert_eq!(
        logged,
        [
            r"lastgenre: invalid alias template \g<2> hop; skipping for genre hip-hop: invalid group reference 2 at position 3"
        ]
    );
}

#[test]
fn template_expansion_as_python() {
    let re = Regex::new("(a)(b)?").unwrap();
    let c = re.captures("a").unwrap().unwrap();
    assert_eq!(expand(&c, 2, r"\1-\2").unwrap(), "a-");
    assert_eq!(expand(&c, 2, r"x\-y").unwrap(), r"x\-y");
    assert_eq!(expand(&c, 2, r"\\").unwrap(), r"\");
    assert_eq!(expand(&c, 2, r"\g<1>z").unwrap(), "az");
    assert_eq!(
        expand(&c, 2, r"\3").unwrap_err(),
        "invalid group reference 3 at position 1"
    );
    assert_eq!(
        expand(&c, 2, r"\q").unwrap_err(),
        r"bad escape \q at position 0"
    );
    assert_eq!(
        expand(&c, 2, r"\g<x>").unwrap_err(),
        "unknown group name 'x'"
    );
    assert_eq!(py_re_escape("a b-c&d~e#f/g:h"), r"a\ b\-c\&d\~e\#f/g:h");
}

#[test]
fn aliases_config_format() {
    let p = plugin("{aliases: {hip hop: [hip-hop, hiphop]}}");
    assert_eq!(
        normalize_genre(&p.log, &p.alias_patterns, "hip-hop"),
        "hip hop"
    );
    assert_eq!(
        normalize_genre(&p.log, &p.alias_patterns, "hiphop"),
        "hip hop"
    );
}

#[test]
fn aliases_config_format_errors() {
    for (conf, msg) in [
        (
            "/path/to/aliases.txt",
            "lastgenre.aliases must be a dict or bool.",
        ),
        ("42", "lastgenre.aliases must be a dict or bool."),
        ("{hip hop: hip-hop}", "hip hop must be a list, not str"),
    ] {
        let e = LastGenre::new(&config(&format!("{{aliases: {conf}}}")))
            .err()
            .unwrap();
        assert_eq!(e.to_string(), msg);
    }
}

#[test]
fn normalize_before_whitelist_and_ignorelist() {
    let mut p = plugin("{aliases: {hip hop: [hip-hop, hiphop]}}");
    p.whitelist = set(&["hip hop"]);
    assert_eq!(p.resolve_genres(&["hip-hop"], ""), ["hip hop"]);

    let mut p = plugin("{aliases: {hip hop: [hip-hop]}}");
    p.ignore_patterns = pats(&[("*", &["hip hop"])]);
    assert!(p.resolve_genres(&["hip-hop"], "").is_empty());
}

#[test]
fn original_fallback_keeps_all_alias_normalized_genres() {
    let mut p = plugin(
        "{force: yes, keep_existing: yes, source: album, whitelist: yes, count: 1, aliases: {hip hop: [hip-hop, hiphop]}}",
    );
    p.whitelist = set(&["hip hop", "jazz"]);
    p.fetch_override = Some(Box::new(|_, _, _| vec![]));
    let (g, label) = p.get_genre(&common_item(&["hip-hop", "Jazz"]), &[]);
    assert_eq!(g, ["hip hop", "Jazz"]);
    assert_eq!(label, "original fallback");
}

#[test]
fn aliases_disabled() {
    let p = plugin("{aliases: no}");
    assert!(p.alias_patterns.is_empty());
    assert_eq!(
        normalize_genre(&p.log, &p.alias_patterns, "hip-hop"),
        "hip-hop"
    );
}

#[test]
fn default_alias_patterns() {
    let p = plugin("{}");
    for (input, expected) in [
        ("dnb", "drum and bass"),
        ("drum n bass", "drum and bass"),
        ("r&b", "r&b"),
        ("rnb", "r&b"),
        ("rock & roll", "rock and roll"),
        ("rock'n'roll", "rock and roll"),
        ("kpop", "k-pop"),
        ("k -pop", "k-pop"),
        ("j rock", "j-rock"),
        ("post rock", "post-rock"),
        ("lofi", "lo-fi"),
        ("lo fi", "lo-fi"),
        ("p funk", "p-funk"),
        ("synth funk", "synth funk"),
        ("synth pop", "synth-pop"),
        ("synth-wave", "synthwave"),
        ("avantgarde", "avant-garde"),
        ("avant gard", "avant-garde"),
        ("nu-jazz", "nu jazz"),
        ("nu-metal", "nu metal"),
        ("nu-soul", "nu soul"),
        ("nu-disco", "nu disco"),
        ("nu - disco", "nu disco"),
        ("electronic music", "electronic"),
        ("world", "world music"),
        ("chill", "chill"),
        ("chill out", "chillout"),
        ("chill-out", "chillout"),
        ("dark wave", "dark wave"),
        ("dark-wave", "dark wave"),
        ("blues rock", "blues rock"),
        ("blues-rock", "blues rock"),
        ("folk-rock", "folk rock"),
        ("downbeat", "downtempo"),
        ("shoegazer", "shoegaze"),
        ("shoegazing", "shoegaze"),
        ("hip-hop", "hip hop"),
        ("triphop", "trip hop"),
        ("punk", "punk rock"),
        ("alt", "alt"),
        ("alt rock", "alternative rock"),
        ("alternative", "alternative"),
        ("goth", "gothic rock"),
        ("goth rock", "gothic rock"),
        ("gothic rock", "gothic rock"),
        ("prog", "progressive rock"),
        ("prog rock", "progressive rock"),
        ("progressive rock", "progressive rock"),
        ("trad", "trad"),
        ("traditional", "traditional"),
        ("indie", "indie"),
        ("indie rock", "indie rock"),
        ("juke/footwork", "juke/footwork"),
        ("Funk / Soul", "funk / soul"),
        ("psy-trance", "psytrance"),
        ("psy rock", "psychedelic rock"),
        ("psych-pop", "psychedelic pop"),
        ("disney", "disney"),
        ("old-school death-metal", "old school death metal"),
        ("old-skool-hip-hop", "old school hip hop"),
        ("old school-jungle", "old school jungle"),
    ] {
        assert_eq!(
            normalize_genre(&p.log, &p.alias_patterns, input),
            expected,
            "{input}"
        );
    }
}

#[test]
fn client_normalizes_in_last_lookup() {
    let client = LastFmClient::new(
        0,
        pats(&[("*", &["hip hop"])]),
        vec![(Pat::new("hip-hop").unwrap(), "hip hop".into())],
    );
    client
        .genre_cache
        .borrow_mut()
        .insert("track.artist-title".into(), s(&["hip-hop"]));
    let log = PluginLog::new(NAME);
    let r = client.last_lookup_with(&log, Kind::Track, &s(&["artist", "title"]), |_| {
        panic!("cached")
    });
    assert!(r.is_empty());
}

#[test]
fn bundled_files() {
    let p = plugin("{canonical: yes}");
    assert_eq!(p.whitelist.len(), 1568);
    assert!(p.whitelist.contains("zydeco"));
    assert_eq!(
        find_parents("Acid Techno", &p.c14n_branches),
        ["acid techno", "techno", "electronic"]
    );
    assert_eq!(find_parents("Nonsense", &p.c14n_branches), ["Nonsense"]);
}

#[test]
fn whitelist_and_tree_from_files() {
    let dir = tempfile::tempdir().unwrap();
    let wl = dir.path().join("wl.txt");
    std::fs::write(&wl, "# comment\n  Rock \n\nJazz\n").unwrap();
    let tree = dir.path().join("tree.yaml");
    std::fs::write(&tree, "- rock:\n    - indie:\n        - shoegaze\n- jazz\n").unwrap();
    let p = plugin(&format!(
        "{{whitelist: '{}', canonical: '{}', count: 9}}",
        wl.display(),
        tree.display()
    ));
    assert_eq!(p.whitelist, set(&["rock", "jazz"]));
    assert_eq!(
        p.resolve_genres(&["shoegaze", "jazz"], ""),
        ["rock", "jazz"]
    );
    let e = LastGenre::new(&config("{whitelist: /nonexistent/wl.txt}"))
        .err()
        .unwrap();
    assert!(e.to_string().contains("/nonexistent/wl.txt"), "{e}");
}

#[test]
fn source_must_be_a_choice() {
    let p = plugin("{source: foo}");
    assert_eq!(
        p.conf.sources().unwrap_err().to_string(),
        "lastgenre.source: must be one of ['track', 'album', 'artist'], not 'foo'"
    );
}

// The Last.fm client against a fake server.

const TOP_TAGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<lfm status="ok">
<toptags artist="Artist" album="Album">
  <tag><name>Rock</name><count>100</count><url>https://www.last.fm/tag/rock</url></tag>
  <tag><name>Hip-Hop &amp; Rap</name><count>40</count><url>x</url></tag>
  <tag><name>seen live</name><count>5</count><url>x</url></tag>
  <tag><name>no count</name><url>x</url></tag>
</toptags>
</lfm>"#;

fn client_at(base: &str, min_weight: i64) -> LastFmClient {
    let mut c = LastFmClient::new(min_weight, HashMap::new(), vec![]);
    c.api_url = format!("{base}/2.0/");
    c
}

#[test]
fn parses_top_tags() {
    let tags = parse_top_tags(TOP_TAGS).unwrap();
    assert_eq!(tags.len(), 4);
    assert_eq!(tags[1].name, "Hip-Hop & Rap");
    assert_eq!(tags[1].weight, 40.0);
    assert_eq!(tags[3].weight, 0.0);
    assert_eq!(
        parse_top_tags(
            r#"<lfm status="failed"><error code="6">The artist you supplied could not be found</error></lfm>"#
        ),
        Err(LastFmError::Pylast(
            "The artist you supplied could not be found".into()
        ))
    );
    assert!(matches!(
        parse_top_tags("<html>oops"),
        Err(LastFmError::Pylast(e)) if e.starts_with("Malformed response from Last.fm. Underlying error: ")
    ));
}

#[test]
fn fetches_top_tags_like_pylast() {
    let server = serve(|r| {
        let body = String::from_utf8_lossy(&r.body).to_string();
        if body.contains("method=artist.getTopTags") && body.contains("artist=Missing") {
            Reply {
                status: 400,
                content_type: "text/xml",
                body: br#"<?xml version="1.0"?><lfm status="failed"><error code="6">The artist you supplied could not be found</error></lfm>"#.to_vec(),
            }
        } else if body.contains("artist=Broken") {
            Reply::status(503)
        } else {
            Reply {
                status: 200,
                content_type: "text/xml; charset=utf-8",
                body: TOP_TAGS.as_bytes().to_vec(),
            }
        }
    });
    let client = client_at(&server.base, 10);
    let log = PluginLog::new(NAME);
    let mut album = Album::new_album();
    album.set_str("albumartist", "AC/DC & Co");
    album.set_str("album", "Back in\u{2010}Black");
    assert_eq!(
        client.fetch(&log, Kind::Album, &album, &[]),
        ["rock", "hip-hop & rap"]
    );
    // Cached.
    client.fetch(&log, Kind::Album, &album, &[]);
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    let r = &seen[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.target, "/2.0/");
    assert_eq!(
        r.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        String::from_utf8_lossy(&r.body),
        format!(
            "artist=AC%2FDC+%26+Co&album=Back+in-Black&api_key={}&method=album.getTopTags",
            client::LASTFM_KEY
        )
    );

    let mut item = Item::new_item();
    item.set_str("artist", "Artist");
    item.set_str("title", "Song");
    client.fetch(&log, Kind::Track, &item, &[]);
    client.fetch(&log, Kind::Artist, &item, &[]);
    let bodies: Vec<String> = server
        .requests()
        .iter()
        .map(|r| String::from_utf8_lossy(&r.body).to_string())
        .collect();
    assert!(bodies[1].starts_with("artist=Artist&track=Song&api_key="));
    assert!(bodies[1].ends_with("&method=track.getTopTags"));
    assert!(bodies[2].starts_with("artist=Artist&api_key="));
    assert!(bodies[2].ends_with("&method=artist.getTopTags"));

    // Missing metadata: no lookup.
    item.set_str("title", "");
    assert!(client.fetch(&log, Kind::Track, &item, &[]).is_empty());
    assert_eq!(server.requests().len(), 3);

    // Errors are logged (at debug level) and find nothing.
    crate::logging::set_verbosity(0);
    let (r, logged) = crate::logging::capture(|| {
        log.at_level(DEBUG, || {
            let mut out = client.fetch(&log, Kind::Artist, &item, &s(&["Missing"]));
            out.extend(client.fetch(&log, Kind::Artist, &item, &s(&["Broken"])));
            out
        })
    });
    assert!(r.is_empty());
    assert_eq!(
        logged,
        [
            "lastgenre: last.fm error: The artist you supplied could not be found",
            "lastgenre: last.fm error: Connection to the API failed with HTTP code 503",
        ]
    );

    // A server that isn't there.
    let gone = client_at("http://127.0.0.1:1", 0);
    let (r, logged) = crate::logging::capture(|| {
        log.at_level(DEBUG, || gone.fetch(&log, Kind::Artist, &item, &[]))
    });
    assert!(r.is_empty());
    assert!(
        logged[0].starts_with("lastgenre: last.fm error: NetworkError: "),
        "{logged:?}"
    );
}

// The command and the import stage, with a library.

struct Env {
    _dir: tempfile::TempDir,
    lib: Library,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    std::fs::create_dir_all(&music).unwrap();
    let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
    Env { _dir: dir, lib }
}

fn add_album(lib: &Library, artists: &[&str], genres: &[&str]) -> Album {
    let mut items: Vec<Item> = artists
        .iter()
        .enumerate()
        .map(|(n, a)| {
            let mut i = item(&[
                ("artist", a),
                ("title", &format!("t{n}")),
                ("album", "Pretend Album"),
                ("albumartist", "Pretend Artist"),
            ]);
            i.set_path(format!("/nonexistent/{n}.mp3"));
            i.set_list("genres", genres);
            i
        })
        .collect();
    lib.add_album(&mut items).unwrap()
}

/// A plugin whose fake Last.fm knows "album" and "track" genres.
fn served_plugin(yaml: &str) -> LastGenre {
    let mut p = plugin(yaml);
    p.fetch_override = Some(Box::new(|kind, obj, _| match kind {
        Kind::Album => s(&["Jazz"]),
        Kind::Track if obj.str("artist") == "A" => s(&["Rock"]),
        Kind::Track => s(&["Blues"]),
        _ => vec![],
    }));
    p
}

#[test]
fn pretend_option_skips_library_updates() {
    let e = env();
    let album = add_album(&e.lib, &["A"], &["Original Genre"]);
    let mut p = served_plugin("{force: yes, whitelist: no}");
    p.conf.pretend = true;
    let opts = CmdOpts {
        pretend: true,
        album: true,
        ..Default::default()
    };
    run_command(&p, &e.lib, false, &opts).unwrap();
    let album = e.lib.get_album(album.id.unwrap()).unwrap().unwrap();
    assert_eq!(album.list("genres"), ["Original Genre"]);
    let items = e.lib.album_items(album.id.unwrap()).unwrap();
    assert_eq!(items[0].list("genres"), ["Original Genre"]);
}

#[test]
fn album_genres_are_inherited() {
    let e = env();
    let album = add_album(&e.lib, &["A", "B"], &[]);
    let p = served_plugin("{whitelist: no}");
    run_command(
        &p,
        &e.lib,
        false,
        &CmdOpts {
            album: true,
            ..Default::default()
        },
    )
    .unwrap();
    let id = album.id.unwrap();
    assert_eq!(
        e.lib.get_album(id).unwrap().unwrap().list("genres"),
        ["Jazz"]
    );
    for i in e.lib.album_items(id).unwrap() {
        assert_eq!(i.list("genres"), ["Jazz"]);
    }
}

#[test]
fn track_source_keeps_track_genres() {
    let e = env();
    let album = add_album(&e.lib, &["A", "B"], &[]);
    let p = served_plugin("{whitelist: no, source: track}");
    run_command(
        &p,
        &e.lib,
        false,
        &CmdOpts {
            album: true,
            ..Default::default()
        },
    )
    .unwrap();
    let id = album.id.unwrap();
    assert_eq!(
        e.lib.get_album(id).unwrap().unwrap().list("genres"),
        ["Jazz"]
    );
    let genres: Vec<Vec<String>> = e
        .lib
        .album_items(id)
        .unwrap()
        .iter()
        .map(|i| i.list("genres"))
        .collect();
    assert_eq!(genres, [s(&["Rock"]), s(&["Blues"])]);
}

#[test]
fn items_mode_and_no_force() {
    let e = env();
    let album = add_album(&e.lib, &["A", "B"], &[]);
    let id = album.id.unwrap();
    let mut first = e.lib.album_items(id).unwrap().remove(0);
    first.set_list("genres", &["Kept"]);
    e.lib.store(&first).unwrap();
    let p = served_plugin("{whitelist: no, source: track}");
    run_command(&p, &e.lib, false, &CmdOpts::default()).unwrap();
    let genres: Vec<Vec<String>> = e
        .lib
        .album_items(id)
        .unwrap()
        .iter()
        .map(|i| i.list("genres"))
        .collect();
    assert_eq!(genres, [s(&["Kept"]), s(&["Blues"])]);
    // The album itself was left alone.
    assert!(
        e.lib
            .get_album(id)
            .unwrap()
            .unwrap()
            .list("genres")
            .is_empty()
    );
}

#[test]
fn import_stage_updates_album_and_items() {
    let e = env();
    let album = add_album(&e.lib, &["A"], &[]);
    let p = served_plugin("{whitelist: no}");
    let cfg = Config::defaults();
    let ctx = PluginCtx {
        cfg: &cfg,
        lib: &e.lib,
        write: false,
    };
    let mut a = album.clone();
    let mut items = e.lib.album_items(album.id.unwrap()).unwrap();
    p.album_imported(
        &ctx,
        &mut a,
        &mut items,
        Path::new("/x"),
        FileOp::Copy,
        false,
    )
    .unwrap();
    assert_eq!(a.list("genres"), ["Jazz"]);
    assert_eq!(items[0].list("genres"), ["Jazz"]);
    assert_eq!(
        e.lib.album_items(album.id.unwrap()).unwrap()[0].list("genres"),
        ["Jazz"]
    );

    // `auto: no` turns the stage off.
    let mut p = served_plugin("{whitelist: no, auto: no, force: yes}");
    p.fetch_override = Some(Box::new(|_, _, _| s(&["Pop"])));
    p.album_imported(
        &ctx,
        &mut a,
        &mut items,
        Path::new("/x"),
        FileOp::Copy,
        false,
    )
    .unwrap();
    assert_eq!(a.list("genres"), ["Jazz"]);
}

#[test]
fn shared_state_mock_counts_lookups() {
    // VA albums look up each track's genre.
    let calls = Rc::new(RefCell::new(vec![]));
    let mut p = plugin("{whitelist: no, source: artist}");
    let c = calls.clone();
    p.fetch_override = Some(Box::new(move |kind, obj, _| {
        c.borrow_mut().push((kind, obj.str("artist").to_string()));
        if kind == Kind::Artist {
            s(&["Pop"])
        } else {
            vec![]
        }
    }));
    let mut a = Album::new_album();
    a.set_str("albumartist", "Various Artists");
    a.set_str("album", "Comp");
    let items = vec![item(&[("artist", "X")]), item(&[("artist", "Y")])];
    let (g, label) = p.get_genre(&a, &items);
    assert_eq!(g, ["Pop"]);
    assert_eq!(label, "most popular track, any");
    assert_eq!(
        *calls.borrow(),
        [
            (Kind::Artist, "X".to_string()),
            (Kind::Artist, "Y".to_string())
        ]
    );
}
