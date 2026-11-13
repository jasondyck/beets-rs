//! Ported from beets' `test/plugins/test_ftintitle.py`.

use super::*;
use serde_yaml::Value as Y;

fn cfg_with(yaml: &str) -> Config {
    let mut c = Config::defaults();
    c.set("plugins", serde_yaml::from_str("[ftintitle]").unwrap());
    c.set("import.write", Y::Bool(false));
    let y: Y = serde_yaml::from_str(yaml).unwrap();
    if let Y::Mapping(m) = y {
        for (k, v) in m {
            c.set(&format!("ftintitle.{}", k.as_str().unwrap()), v);
        }
    }
    c
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(ToString::to_string).collect()
}

/// `test_ftintitle_functional`: (config, drop flag, (artist, title,
/// albumartist), (expected artist, expected title)).
#[test]
fn ftintitle_functional() {
    type Case<'a> = (
        &'a str,
        bool,
        (&'a str, &'a str, &'a str),
        (&'a str, &'a str),
    );
    let cases: &[Case] = &[
        (
            "{}",
            false,
            ("Alice", "Song 1", "Alice"),
            ("Alice", "Song 1"),
        ),
        (
            "{format: 'feat {0}'}",
            false,
            ("Alice ft. Bob", "Song 1", ""),
            ("Alice", "Song 1 feat Bob"),
        ),
        ("{}", false, ("Alice", "Song 1", ""), ("Alice", "Song 1")),
        (
            "{format: 'featuring {0}'}",
            false,
            ("Alice ft Bob", "Song 1", "George"),
            ("Alice", "Song 1 featuring Bob"),
        ),
        (
            "{}",
            false,
            ("Alice", "Song 1", "George"),
            ("Alice", "Song 1"),
        ),
        (
            "{}",
            true,
            ("Alice ft Bob", "Song 1", "Alice"),
            ("Alice", "Song 1"),
        ),
        (
            "{}",
            true,
            ("Alice", "Song 1", "Alice"),
            ("Alice", "Song 1"),
        ),
        (
            "{}",
            true,
            ("Alice ft Bob", "Song 1", "George"),
            ("Alice", "Song 1"),
        ),
        (
            "{}",
            true,
            ("Alice", "Song 1", "George"),
            ("Alice", "Song 1"),
        ),
        (
            "{format: 'feat. {}'}",
            false,
            ("Alice ft Bob", "Song 1", "Alice"),
            ("Alice", "Song 1 feat. Bob"),
        ),
        (
            "{format: 'featuring {}'}",
            false,
            ("Alice feat. Bob", "Song 1", "Alice"),
            ("Alice", "Song 1 featuring Bob"),
        ),
        (
            "{format: 'with {}'}",
            false,
            ("Alice feat Bob", "Song 1", "Alice"),
            ("Alice", "Song 1 with Bob"),
        ),
        (
            "{format: 'ft. {}'}",
            false,
            ("Alice (ft. Bob)", "Song 1", "Alice"),
            ("Alice", "Song 1 ft. Bob"),
        ),
        (
            "{format: 'feat. {}', keep_in_artist: true}",
            false,
            ("Alice ft Bob", "Song 1", "Alice"),
            ("Alice ft Bob", "Song 1 feat. Bob"),
        ),
        (
            "{format: 'feat. {}', keep_in_artist: true}",
            true,
            ("Alice ft Bob", "Song 1", "Alice"),
            ("Alice ft Bob", "Song 1"),
        ),
        (
            "{format: 'featuring {}', custom_words: [med]}",
            false,
            ("Alice med Bob", "Song 1", "Alice"),
            ("Alice", "Song 1 featuring Bob"),
        ),
        (
            "{format: 'featuring {}', keep_in_artist: true, custom_words: [med]}",
            false,
            ("Alice med Bob", "Song 1", "Alice"),
            ("Alice med Bob", "Song 1 featuring Bob"),
        ),
        (
            "{format: 'featuring {}', keep_in_artist: true, custom_words: [med]}",
            true,
            ("Alice med Bob", "Song 1", "Alice"),
            ("Alice med Bob", "Song 1"),
        ),
        (
            "{format: 'feat. {}', preserve_album_artist: true}",
            false,
            ("Alice feat. Bob", "Song 1", "Alice"),
            ("Alice", "Song 1 feat. Bob"),
        ),
        (
            "{format: 'feat. {}', preserve_album_artist: false}",
            false,
            ("Alice feat. Bob", "Song 1", "Alice"),
            ("Alice", "Song 1 feat. Bob"),
        ),
        (
            "{format: 'feat. {}', preserve_album_artist: true}",
            false,
            ("Alice feat. Bob", "Song 1", "Alice feat. Bob"),
            ("Alice feat. Bob", "Song 1"),
        ),
        (
            "{format: 'feat. {}', preserve_album_artist: false}",
            false,
            ("Alice feat. Bob", "Song 1", "Alice feat. Bob"),
            ("Alice", "Song 1 feat. Bob"),
        ),
        (
            "{format: 'ft. {}', bracket_keywords: [mix]}",
            false,
            ("Alice ft. Bob", "Song 1 (Club Mix)", "Alice"),
            ("Alice", "Song 1 ft. Bob (Club Mix)"),
        ),
        (
            "{format: 'ft. {}', bracket_keywords: [nomatch]}",
            false,
            ("Alice ft. Bob", "Song 1 (Club Remix)", "Alice"),
            ("Alice", "Song 1 (Club Remix) ft. Bob"),
        ),
    ];
    for (yaml, drop, (artist, title, aa), (want_artist, want_title)) in cases {
        let cfg = cfg_with(yaml);
        let lib = Library::in_memory().unwrap();
        let mut item = Item::new_item();
        item.set_path("/x.mp3");
        item.set_str("artist", *artist);
        item.set_str("artist_sort", *artist);
        item.set_str("title", *title);
        item.set_str("albumartist", *aa);
        let id = lib.add(&mut item).unwrap();
        command(&cfg, &lib, *drop, None, &[]).unwrap();
        let back = lib.get_item(id).unwrap().unwrap();
        let case = format!("{yaml} {drop} {artist:?}");
        assert_eq!(back.str("artist"), *want_artist, "{case}");
        assert_eq!(back.str("title"), *want_title, "{case}");
    }
}

fn track(artist: &str, title: &str) -> TrackInfo {
    TrackInfo {
        artist: artist.into(),
        title: title.into(),
        ..Default::default()
    }
}

#[test]
fn trackinfo_received_rewrites_artist_with_artist_credit() {
    let core = FtCore::new(&cfg_with("{auto: true}"));
    let mut info = TrackInfo {
        artist: "Alice feat. Bob".into(),
        artist_credit: "Alice".into(),
        artist_sort: "Alice feat. Bob".into(),
        artists: words(&["Alice", "Bob"]),
        artists_credit: words(&["Alice"]),
        title: "Song".into(),
        ..Default::default()
    };
    core.track_info(&mut info);
    assert_eq!(info.artist, "Alice");
    assert_eq!(info.artist_credit, "Alice");
    assert_eq!(info.artist_sort, "Alice");
    assert_eq!(info.artists, ["Alice", "Bob"]);
    assert_eq!(info.artists_credit, ["Alice"]);
    assert_eq!(info.title, "Song feat. Bob");
}

#[test]
fn trackinfo_received_rewrites_or_skips_simple_cases() {
    // auto disabled: the plugin doesn't listen.
    crate::mb::clear_info_listeners();
    FtInTitle::new(&cfg_with("{auto: false}"));
    assert!(crate::mb::info_listener_count() == 0);
    FtInTitle::new(&cfg_with("{auto: true}"));
    assert!(crate::mb::info_listener_count() == 1);
    crate::mb::clear_info_listeners();

    let core = FtCore::new(&cfg_with("{auto: true}"));
    for (artist, title, want_artist, want_title) in [
        ("Alice", "Song", "Alice", "Song"),
        (
            "Alice feat. Bob",
            "Song feat. Bob",
            "Alice",
            "Song feat. Bob",
        ),
    ] {
        let mut info = track(artist, title);
        core.track_info(&mut info);
        assert_eq!(
            (info.artist.as_str(), info.title.as_str()),
            (want_artist, want_title)
        );
    }
}

#[test]
fn ft_in_title_reports_no_change_for_info_drop_keep_noop() {
    let core = FtCore::new(&cfg_with("{drop: true, keep_in_artist: true}"));
    let mut info = track("Alice feat. Bob", "Song");
    assert!(!core.ft_in_title(&mut info, "", &listener_log()));
    assert_eq!(info.artist, "Alice feat. Bob");
    assert_eq!(info.title, "Song");
}

#[test]
fn trackinfo_received_preserves_artist_credit_when_disabled() {
    let core = FtCore::new(&cfg_with("{auto: true}"));
    let mut info = TrackInfo {
        artist: "Alice feat. Bob".into(),
        artist_credit: "Alice feat. Bobby".into(),
        title: "Song".into(),
        ..Default::default()
    };
    core.track_info(&mut info);
    assert_eq!(info.artist, "Alice");
    assert_eq!(info.artist_credit, "Alice feat. Bobby");
    assert_eq!(info.title, "Song feat. Bob");
}

#[test]
fn ft_in_title_reports_no_change_for_item_drop_keep_noop() {
    let core = FtCore::new(&cfg_with("{drop: true, keep_in_artist: true}"));
    let mut item = Item::new_item();
    item.set_path("/x.mp3");
    item.set_str("artist", "Alice feat. Bob");
    item.set_str("title", "Song");
    item.set_str("albumartist", "Alice");
    assert!(!core.ft_in_title(&mut item, "Alice", &listener_log()));
    assert_eq!(item.str("artist"), "Alice feat. Bob");
    assert_eq!(item.str("title"), "Song");
}

#[test]
fn imported_stage_rewrites_imported_items() {
    let cfg = cfg_with("{auto: true}");
    let lib = Library::in_memory().unwrap();
    let mut item = Item::new_item();
    item.set_path("/x.mp3");
    item.set_str("artist", "Alice feat. Bob");
    item.set_str("title", "Song");
    item.set_str("albumartist", "Alice");
    let id = lib.add(&mut item).unwrap();
    let plugin = FtInTitle::new(&cfg);
    crate::mb::clear_info_listeners();
    let ctx = PluginCtx {
        cfg: &cfg,
        lib: &lib,
        write: false,
    };
    let mut items = vec![item];
    let mut task = ImportTask {
        album: None,
        items: &mut items,
        paths: &[],
        asis: true,
        op: crate::fileops::FileOp::None,
    };
    plugin.import_stage(&ctx, &mut task).unwrap();
    let back = lib.get_item(id).unwrap().unwrap();
    assert_eq!(back.str("artist"), "Alice");
    assert_eq!(back.str("title"), "Song feat. Bob");
}

#[test]
fn albuminfo_received_rewrites_tracks_with_album_artist() {
    let core = FtCore::new(&cfg_with("{auto: true}"));
    let mut t = track("Alice & Bob", "Song");
    t.artist_sort = "Alice & Bob".into();
    let mut info = AlbumInfo {
        artist: "Alice".into(),
        album: "Album".into(),
        tracks: vec![t],
        ..Default::default()
    };
    core.album_info(&mut info);
    let t = &info.tracks[0];
    assert_eq!(t.artist, "Alice");
    assert_eq!(t.artist_sort, "Alice");
    assert_eq!(t.title, "Song feat. Bob");
}

#[test]
fn find_feat_part_cases() {
    for (artist, aa, want) in [
        ("Alice ft. Bob", "Alice", Some("Bob")),
        ("Alice feat Bob", "Alice", Some("Bob")),
        ("Alice featuring Bob", "Alice", Some("Bob")),
        ("Alice & Bob", "Alice", Some("Bob")),
        ("Alice and Bob", "Alice", Some("Bob")),
        ("Alice With Bob", "Alice", Some("Bob")),
        ("Alice (ft. Bob)", "Alice", Some("Bob")),
        ("Alice [ft. Bob]", "Bob", Some("Alice")),
        ("Alice defeat Bob", "Alice", None),
        ("Alice & Bob", "Bob", Some("Alice")),
        ("Alice ft. Bob", "Bob", Some("Alice")),
        ("Alice ft. Carol", "Bob", Some("Carol")),
    ] {
        assert_eq!(
            find_feat_part(artist, Some(aa), &[]).as_deref(),
            want,
            "{artist}"
        );
    }
}

#[test]
fn split_on_feat_cases() {
    for (given, l, r) in [
        ("Alice ft. Bob", "Alice", Some("Bob")),
        ("Alice feat Bob", "Alice", Some("Bob")),
        ("Alice feat. Bob", "Alice", Some("Bob")),
        ("Alice featuring Bob", "Alice", Some("Bob")),
        ("Alice & Bob", "Alice", Some("Bob")),
        ("Alice, Bob & Charlie", "Alice", Some("Bob & Charlie")),
        (
            "Alice, Bob & Charlie feat. Xavier",
            "Alice, Bob & Charlie",
            Some("Xavier"),
        ),
        ("Alice and Bob", "Alice", Some("Bob")),
        ("Alice With Bob", "Alice", Some("Bob")),
        ("Alice defeat Bob", "Alice defeat Bob", None),
        ("Alice & Bob feat Charlie", "Alice & Bob", Some("Charlie")),
        ("Alice & Bob ft. Charlie", "Alice & Bob", Some("Charlie")),
        (
            "Alice & Bob featuring Charlie",
            "Alice & Bob",
            Some("Charlie"),
        ),
        (
            "Alice and Bob feat Charlie",
            "Alice and Bob",
            Some("Charlie"),
        ),
        ("Alice (ft. Bob)", "Alice", Some("Bob")),
        ("Alice [& Bob]", "Alice", Some("Bob")),
    ] {
        let (a, b) = split_on_feat(given, true, &[]);
        assert_eq!((a.as_str(), b.as_deref()), (l, r), "{given}");
    }
}

#[test]
fn insert_ft_into_title_cases() {
    let none: Option<&[&str]> = None;
    let cases: &[(&str, Option<&[&str]>, &str)] = &[
        ("Song (Remix)", none, "Song ft. Bob (Remix)"),
        ("Song [Version]", none, "Song ft. Bob [Version]"),
        ("Song {Extended Mix}", none, "Song ft. Bob {Extended Mix}"),
        ("Song <Instrumental>", none, "Song ft. Bob <Instrumental>"),
        ("Song (Remix) (Live)", none, "Song ft. Bob (Remix) (Live)"),
        ("Song (Live) [Remix]", none, "Song ft. Bob (Live) [Remix]"),
        (
            "Song [Edit] (Remastered)",
            none,
            "Song ft. Bob [Edit] (Remastered)",
        ),
        ("Song", none, "Song ft. Bob"),
        ("Song (Arbitrary)", none, "Song (Arbitrary) ft. Bob"),
        ("Song (", none, "Song ( ft. Bob"),
        ("Song (Live", none, "Song (Live ft. Bob"),
        (
            "Song (Live) (Arbitrary)",
            none,
            "Song ft. Bob (Live) (Arbitrary)",
        ),
        (
            "Song (Arbitrary) (Remix)",
            none,
            "Song (Arbitrary) ft. Bob (Remix)",
        ),
        (
            "Song (Remix (Extended))",
            none,
            "Song ft. Bob (Remix (Extended))",
        ),
        (
            "Song [Arbitrary [Description]]",
            none,
            "Song [Arbitrary [Description]] ft. Bob",
        ),
        (
            "Song (Remix [Extended])",
            none,
            "Song ft. Bob (Remix [Extended])",
        ),
        (
            "Song [Arbitrary {Extended}]",
            none,
            "Song ft. Bob [Arbitrary {Extended}]",
        ),
        (
            "Song {Live <Arbitrary>}",
            none,
            "Song ft. Bob {Live <Arbitrary>}",
        ),
        (
            "Song <Remaster (Arbitrary)>",
            none,
            "Song ft. Bob <Remaster (Arbitrary)>",
        ),
        (
            "Song <Extended> [Live]",
            none,
            "Song ft. Bob <Extended> [Live]",
        ),
        (
            "Song (Version) <Live>",
            none,
            "Song ft. Bob (Version) <Live>",
        ),
        (
            "Song (Arbitrary [Description])",
            none,
            "Song (Arbitrary [Description]) ft. Bob",
        ),
        (
            "Song [Description (Arbitrary)]",
            none,
            "Song [Description (Arbitrary)] ft. Bob",
        ),
        ("Song (Live)", Some(&["live"]), "Song ft. Bob (Live)"),
        (
            "Song (Concert)",
            Some(&["concert"]),
            "Song ft. Bob (Concert)",
        ),
        ("Song (Remix)", Some(&["custom"]), "Song (Remix) ft. Bob"),
        ("Song (Custom)", Some(&["custom"]), "Song ft. Bob (Custom)"),
        ("Song", Some(&[]), "Song ft. Bob"),
        ("Song (", Some(&[]), "Song ( ft. Bob"),
        (
            "Song (Club Mix)",
            Some(&["club mix"]),
            "Song ft. Bob (Club Mix)",
        ),
        (
            "Song (Club Remix)",
            Some(&["club mix"]),
            "Song (Club Remix) ft. Bob",
        ),
    ];
    for (given, kw, want) in cases {
        let kw = kw.map(words);
        assert_eq!(
            insert_ft_into_title(given, "ft. Bob", kw.as_deref()),
            *want,
            "{given}"
        );
    }
}

#[test]
fn contains_feat_cases() {
    for (given, want) in [
        ("Alice ft. Bob", true),
        ("Alice feat. Bob", true),
        ("Alice feat Bob", true),
        ("Alice featuring Bob", true),
        ("Alice (ft. Bob)", true),
        ("Alice (feat. Bob)", true),
        ("Alice [ft. Bob]", true),
        ("Alice [feat. Bob]", true),
        ("Alice defeat Bob", false),
        ("Aliceft.Bob", false),
        ("Alice (defeat Bob)", false),
        ("Live and Let Go", false),
        ("Come With Me", false),
    ] {
        assert_eq!(contains_feat(given, &[]), want, "{given}");
    }
}

#[test]
fn custom_words_cases() {
    for (given, w, want) in [
        ("Alice ft. Bob", vec![], true),
        ("Alice defeat Bob", vec![], false),
        ("Aliceft.Bob", vec![], false),
        ("Live and Let Go", vec![], false),
        ("Alice x Bob", vec!["x"], true),
        ("Alice x Bob", vec!["X"], true),
        ("Alice och Xavier", vec!["x"], false),
        ("Alice ft. Xavier", vec!["x"], true),
        ("Alice med Carol", vec!["med"], true),
        ("Alice med Carol", vec![], false),
    ] {
        assert_eq!(contains_feat(given, &words(&w)), want, "{given} {w:?}");
    }
}

#[test]
fn album_template_value() {
    assert_eq!(album_artist_no_feat("Foo ft. Bar", &[]), "Foo");
    assert_eq!(album_artist_no_feat("Foobar", &[]), "Foobar");
    // Through the template environment.
    let cfg = cfg_with("{}");
    let env = std::rc::Rc::new(crate::formatted::FormatEnv::new(&cfg).unwrap());
    let mut album = crate::model::Record::new_album();
    album.set_str("albumartist", "Foo feat. Bar");
    let f = crate::formatted::Formatted::with_env(&album, None, env);
    assert_eq!(
        crate::template::Template::parse("$album_artist_no_feat").render(&f),
        "Foo"
    );
}
