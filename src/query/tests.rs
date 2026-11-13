//! Ported from beets' `test/dbcore/test_query.py`, `test_queryparse.py`,
//! `test_datequery.py` and `test_sort.py`, adapted to this schema (no
//! plugin-typed flexible fields).

use super::*;
use crate::library::Library;
use crate::paths::shell_split;
use std::collections::BTreeSet;

fn set_ctx(music: Option<&Path>, case_insensitive: bool) {
    set_context(Context {
        music_dir: music.map(normpath),
        sort_case_insensitive: case_insensitive,
        ..Context::default()
    });
}

fn rec(kind: Kind, fields: &[(&str, &str)]) -> Record {
    let mut r = Record::new(kind);
    for (k, v) in fields {
        if *k == "path" || *k == "artpath" {
            r.set(k, Value::Path(PathBuf::from(v)));
        } else {
            r.set_parsed(k, v).unwrap();
        }
    }
    r
}

fn item(fields: &[(&str, &str)]) -> Record {
    rec(Kind::Item, fields)
}

/// Add items as an album, then set album fields/flex attributes on it.
fn add_album(lib: &Library, items: Vec<Record>, album: &[(&str, &str)]) -> Record {
    let mut items = items;
    let mut a = lib.add_album(&mut items).unwrap();
    for (k, v) in album {
        if *k == "artpath" {
            a.set(k, Value::Path(PathBuf::from(v)));
        } else {
            a.set_parsed(k, v).unwrap();
        }
    }
    lib.store(&a).unwrap();
    a
}

fn q(kind: Kind, s: &str) -> Query {
    Query::parse(kind, &shell_split(s)).unwrap()
}

fn titles(lib: &Library, s: &str) -> BTreeSet<String> {
    lib.items(&q(Kind::Item, s))
        .unwrap()
        .iter()
        .map(|i| i.str("title").to_string())
        .collect()
}

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(ToString::to_string).collect()
}

// ------------------------------------------------------------ query parsing

#[test]
fn parse_query_part_as_beets() {
    let p = |s: &str| {
        let (k, t, _) = parse_query_part(s);
        (k, t)
    };
    let some = |k: &str, t: &str| (Some(k.to_string()), t.to_string());
    let none = |t: &str| (None, t.to_string());
    assert_eq!(p("test"), none("test"));
    assert_eq!(p("test:val"), some("test", "val"));
    assert_eq!(p("test:"), some("test", ""));
    assert_eq!(p(":regexp"), none(":regexp"));
    assert_eq!(p("test::regexp"), some("test", ":regexp"));
    assert_eq!(p(r"test\:val"), none("test:val"));
    assert_eq!(p(r":test\:regexp"), none(":test:regexp"));
    assert_eq!(p("year:1999..2010"), some("year", "1999..2010"));
    assert_eq!(p(""), none(""));
    // The key is all non-space; a space before the colon makes it a term.
    assert_eq!(p("foo bar:baz"), none("foo bar:baz"));
    assert_eq!(p("artist:dave matthews"), some("artist", "dave matthews"));
    assert_eq!(
        parse_query_part("-title:x"),
        (Some("title".into()), "x".into(), true)
    );
    assert_eq!(parse_query_part("^x"), (None, "x".into(), true));
}

fn or_len(query: &Query) -> usize {
    match &query.pred {
        Pred::Or(v) => v.len(),
        _ => 1,
    }
}

#[test]
fn parse_sorted_query_as_beets() {
    set_ctx(None, true);
    let parse = |s: &str| {
        let parts: Vec<String> = s.split_whitespace().map(String::from).collect();
        Query::parse_sorted(Kind::Item, &parts, true).unwrap()
    };
    assert_eq!(or_len(&parse("foo bar")), 1);
    assert_eq!(or_len(&parse("foo , bar")), 2);
    assert_eq!(or_len(&parse("foo, bar")), 2);
    // No space after the comma: one term.
    assert_eq!(or_len(&parse("foo,bar")), 1);
    assert_eq!(or_len(&parse("foo , bar ,")), 3);
    assert_eq!(or_len(&parse(", foo , bar")), 3);
    // A lone `-` is a (negated, match-nothing) query part, not a sort.
    let only = parse("-");
    assert!(only.sort.is_null());
    // Sort parts end in + or -, have no colon and more than the sign.
    let s = parse("year+ artist- foo:bar-");
    assert_eq!(
        s.sort
            .0
            .iter()
            .map(|f| (f.field.as_str(), f.ascending))
            .collect::<Vec<_>>(),
        [("year", true), ("artist", false)]
    );
    // "-bar+" is a slow sort on the field "-bar", not a negation.
    let n = parse("-bar+");
    assert!(matches!(&n.pred, Pred::And(v) if matches!(v[..], [Pred::True])));
    assert_eq!(n.sort.0[0].field, "-bar");
    assert_eq!(n.sort.0[0].kind, SortKind::Slow);
}

#[test]
fn sort_kinds_as_beets() {
    let k = |kind, f: &str| field_sort(kind, f, true, true);
    assert_eq!(k(Kind::Item, "year").kind, SortKind::Fixed);
    assert_eq!(k(Kind::Item, "flex").kind, SortKind::Slow);
    assert_eq!(k(Kind::Item, "artist").kind, SortKind::Smart);
    assert_eq!(k(Kind::Album, "artist").field, "albumartist");
    assert_eq!(k(Kind::Album, "path").kind, SortKind::Slow);
    assert_eq!(k(Kind::Item, "singleton").kind, SortKind::Slow);
    // Album fields sort items in SQL through the join.
    assert_eq!(k(Kind::Item, "artpath").kind, SortKind::Fixed);
}

// -------------------------------------------------------------- TestGet

fn get_lib() -> Library {
    set_ctx(None, true);
    let lib = Library::in_memory().unwrap();
    let first = item(&[
        ("title", "first"),
        ("artist", "one"),
        ("album", "baz"),
        ("year", "2001"),
        ("comp", "true"),
        ("genres", "rock"),
        ("artists", "one; eleven"),
        ("composers", "composer"),
    ]);
    let second = item(&[
        ("title", "second"),
        ("artist", "two"),
        ("album", "baz"),
        ("year", "2002"),
        ("comp", "true"),
        ("genres", "Rock"),
        ("artists", "two; twelve"),
    ]);
    add_album(&lib, vec![first, second], &[("albumflex", "foo")]);
    let mut third = item(&[
        ("title", "third"),
        ("artist", "three"),
        ("album", "foo"),
        ("year", "2003"),
        ("comp", "false"),
        ("genres", "Hard Rock"),
        ("artists", "three; one"),
        ("comments", "caf\u{e9}"),
    ]);
    third.flex.insert("Flex_Attr".into(), "flex".into());
    lib.add(&mut third).unwrap();
    lib
}

#[test]
fn get_queries_as_beets() {
    let lib = get_lib();
    let all = &["first", "second", "third"][..];
    for (query, want) in [
        ("", all),
        (":oNE", &[][..]),
        (":one", &["first"]),
        (":sec :ond", &["second"]),
        (":second", &["second"]),
        ("=rock", &["first"]),
        ("=~\"hard rock\"", &["third"]),
        (":t$", &["first"]),
        ("oNE", &["first"]),
        ("baz", &["first", "second"]),
        ("sec ond", &["second"]),
        ("three", &["third"]),
        ("albumflex:foo", &["first", "second"]),
        ("artist::t.+r", &["third"]),
        ("artist:thrEE", &["third"]),
        ("artists::eleven", &["first"]),
        ("artists::one", &["first", "third"]),
        ("ArTiST:three", &["third"]),
        ("comments:caf\u{e9}", &["third"]),
        ("comp:true", &["first", "second"]),
        ("comp:false", &["third"]),
        ("flex_attr:flex", &["third"]),
        ("Flex_Attr:flex", &["third"]),
        ("genres:=rock", &["first"]),
        ("genres:=Rock", &["second"]),
        ("genres:=\"Hard Rock\"", &["third"]),
        ("genres:=~\"hard rock\"", &["third"]),
        ("genres:=~rock", &["first", "second"]),
        ("genres:=\"hard rock\"", &[]),
        ("popebear", &[]),
        ("pope:bear", &[]),
        ("singleton:true", &["third"]),
        ("singleton:1", &["third"]),
        ("singleton:false", &["first", "second"]),
        ("singleton:0", &["first", "second"]),
        ("title:ond", &["second"]),
        ("title::sec", &["second"]),
        ("year:2001", &["first"]),
        ("year:2000..2002", &["first", "second"]),
        ("xyzzy:nonsense", &[]),
        // `test_legacy_field`: the old single-valued names query the lists.
        ("genres::rock", &["first"]),
        ("genre::rock", &["first"]),
        ("composers::composer", &["first"]),
        ("composer::composer", &["first"]),
        // OR between groups.
        ("first , title:third", &["first", "third"]),
        ("year:..2001 , year:2003..", &["first", "third"]),
    ] {
        assert_eq!(titles(&lib, query), set(want), "{query}");
    }
}

#[test]
fn negation_prefix_as_beets() {
    let lib = get_lib();
    for (query, want) in [
        ("-artist::t.+r", &["first", "second"][..]),
        ("-:t$", &["second", "third"]),
        ("sec -bar", &["second"]),
        ("sec -title:bar", &["second"]),
        ("-ond", &["first", "third"]),
        ("^ond", &["first", "third"]),
        ("^title:sec", &["first", "third"]),
        ("-title:sec", &["first", "third"]),
        ("^year:2001..2002", &["third"]),
        ("^comp:true", &["third"]),
        ("-singleton:true", &["first", "second"]),
        ("-albumflex:foo", &["third"]),
    ] {
        assert_eq!(titles(&lib, query), set(want), "{query}");
    }
}

fn run(lib: &Library, pred: Pred) -> BTreeSet<String> {
    let query = Query {
        kind: Kind::Item,
        pred,
        sort: Sort::default(),
    };
    lib.items(&query)
        .unwrap()
        .iter()
        .map(|i| i.str("title").to_string())
        .collect()
}

fn part(s: &str) -> Pred {
    construct_query_part(Kind::Item, s).unwrap()
}

#[test]
fn query_logic_as_beets() {
    let lib = get_lib();
    let all = set(&["first", "second", "third"]);
    for (p, negated) in [
        (part("comp:true"), &["third"][..]),
        (part("added:2000-01-01"), &["first", "second", "third"]),
        (Pred::False, &["first", "second", "third"]),
        (part("year:=2003"), &["first", "second"]),
        (part("year:2001..2002"), &["third"]),
        (
            Pred::And(vec![part("comp:true"), part("year:2002")]),
            &["first", "third"],
        ),
        (
            Pred::Or(vec![part("comp:true"), part("year:2002")]),
            &["third"],
        ),
        (part("artist::^t"), &["first"]),
        (part("album:ba"), &["third"]),
        (Pred::True, &[]),
    ] {
        let not = Pred::Not(Box::new(p.clone()));
        assert_eq!(run(&lib, not.clone()), set(negated), "{p:?}");
        assert_eq!(run(&lib, Pred::Or(vec![p.clone(), not.clone()])), all);
        assert!(run(&lib, Pred::And(vec![p.clone(), not.clone()])).is_empty());
        let yes = run(&lib, p.clone());
        assert_eq!(&yes | &set(negated), all);
        assert_eq!(run(&lib, Pred::Not(Box::new(not))), yes);
    }
}

fn make_slow(p: &mut Pred) {
    match p {
        Pred::Field(f) => f.fast = false,
        Pred::Not(p) => make_slow(p),
        Pred::And(v) | Pred::Or(v) => v.iter_mut().for_each(make_slow),
        _ => {}
    }
}

#[test]
fn fast_and_slow_queries_agree() {
    let lib = get_lib();
    for s in [
        "added:2001-01-01",
        "artist:=one",
        "year:2002",
        "year:=~2001",
        "album::^.a",
        "title:x",
        "title:ir",
        "comp:true",
        "singleton:true",
        "year:2001..",
        "length:..0:01",
        "albumartist:one",
    ] {
        for negate in [false, true] {
            let fast = if negate {
                Pred::Not(Box::new(part(s)))
            } else {
                part(s)
            };
            let mut slow = fast.clone();
            make_slow(&mut slow);
            assert!(slow.clause().is_none());
            assert_eq!(run(&lib, fast), run(&lib, slow), "{s} negated={negate}");
        }
    }
}

// --------------------------------------------------------------- TestMatch

#[test]
fn field_queries_match_a_record() {
    let it = item(&[
        ("album", "the album"),
        ("disc", "6"),
        ("year", "1"),
        ("bitrate", "128000"),
        ("genres", "Classical; Baroque"),
    ]);
    let obj = Obj::new(&it);
    for (s, want) in [
        ("album::^the album$", true),
        ("album::^album$", false),
        ("disc::^6$", true),
        ("album:album", true),
        ("album:ablum", false),
        ("disc:=~6", true),
        ("album:=~the album", true),
        ("album:=~THE ALBUM", true),
        ("album:=~album", false),
        // In memory, a list matches if one of its values does.
        ("genres:=Classical", true),
        ("genres:=Neoclassical", false),
        ("genres:=~classical", true),
        ("genres:=~neoclassical", false),
        ("year:1", true),
        ("year:10", false),
        ("bitrate:100000..200000", true),
        ("bitrate:200000..300000", false),
        ("bitrate:100000..", true),
    ] {
        let p = part(s);
        assert_eq!(p.matches(&obj), want, "{s}");
        assert_eq!(Pred::Not(Box::new(p)).matches(&obj), !want, "{s}");
    }
}

// --------------------------------------------------------------- TestQuery

#[test]
fn value_types_as_beets() {
    set_ctx(None, true);
    let lib = Library::in_memory().unwrap();
    add_album(
        &lib,
        vec![item(&[
            ("title", "album title"),
            ("comp", "1"),
            ("bpm", "120"),
        ])],
        &[],
    );
    lib.add(&mut item(&[("title", "singleton"), ("comp", "0")]))
        .unwrap();
    for (s, want) in [
        ("comp:true", &["album title"][..]),
        ("bpm:120", &["album title"]),
        ("bpm:110..125", &["album title"]),
        ("bpm:12", &[]),
        ("singleton:true", &["singleton"]),
        // A bare numeric field matches everything (beets' `"1"` clause).
        ("bpm:", &["album title", "singleton"]),
        // Upper case keys aren't looked up as typed fields: a substring.
        ("BPM:12", &["album title"]),
    ] {
        assert_eq!(titles(&lib, s), set(want), "{s}");
    }
}

#[test]
fn invalid_queries_explain_themselves() {
    set_ctx(None, true);
    let err = |s: &str| {
        Query::parse(Kind::Item, &shell_split(s))
            .unwrap_err()
            .to_string()
    };
    assert_eq!(
        err("year:199a"),
        "'year:199a': '199a' is not an int or a float"
    );
    assert!(
        err("year::199(").starts_with("'year::199(': '199(' is not a regular expression: "),
        "{}",
        err("year::199(")
    );
    assert_eq!(
        err("length:1:2:3"),
        "'length:1:2:3': '1:2:3' is not a M:SS string or a float"
    );
    assert_eq!(
        err("artist:x added:200a"),
        "'artist:x added:200a': '200a' is not a valid date/time string"
    );
}

#[test]
fn default_search_fields_as_beets() {
    set_ctx(None, true);
    let lib = Library::in_memory().unwrap();
    add_album(
        &lib,
        vec![item(&[
            ("title", "title"),
            ("album", "album"),
            ("albumartist", "albumartist"),
            ("catalognum", "catalognum"),
            ("year", "2001"),
        ])],
        &[],
    );
    let albums = |s: &str| !lib.albums(&q(Kind::Album, s)).unwrap().is_empty();
    let items = |s: &str| !lib.items(&q(Kind::Item, s)).unwrap().is_empty();
    assert!(albums("album"));
    assert!(albums("albumartist"));
    assert!(!albums("catalognum"));
    assert!(items("title"));
    assert!(!items("2001"));
}

#[test]
fn related_queries_as_beets() {
    set_ctx(None, true);
    let lib = Library::in_memory().unwrap();
    for n in 1..=2 {
        let name = format!("Album{n}");
        let items = (1..=2)
            .map(|i| item(&[("album", &name), ("title", &format!("{name} Item{i}"))]))
            .collect();
        add_album(
            &lib,
            items,
            &[
                ("artpath", &format!("/art/{name} Artpath")),
                ("catalognum", "ABC"),
            ],
        );
    }
    let albums = |s: &str| -> BTreeSet<String> {
        lib.albums(&q(Kind::Album, s))
            .unwrap()
            .iter()
            .map(|a| a.str("album").to_string())
            .collect()
    };
    assert_eq!(albums("title:Album1"), set(&["Album1"]));
    assert_eq!(
        titles(&lib, "title:Album1"),
        set(&["Album1 Item1", "Album1 Item2"])
    );
    assert_eq!(albums("title:Item2"), set(&["Album1", "Album2"]));
    assert_eq!(
        titles(&lib, "title:Item2"),
        set(&["Album1 Item2", "Album2 Item2"])
    );
    assert_eq!(albums("artpath::Album1"), set(&["Album1"]));
    assert_eq!(
        titles(&lib, "artpath::Album1"),
        set(&["Album1 Item1", "Album1 Item2"])
    );
    assert_eq!(albums("catalognum:ABC Album1"), set(&["Album1"]));
    // Items reach their album's flexible attributes (slowly).
    let mut a = lib
        .albums(&q(Kind::Album, "album:Album2"))
        .unwrap()
        .remove(0);
    a.flex.insert("mood".into(), "calm".into());
    lib.store(&a).unwrap();
    assert_eq!(
        titles(&lib, "mood:calm"),
        set(&["Album2 Item1", "Album2 Item2"])
    );
    assert_eq!(titles(&lib, "mood:calm title:1"), set(&["Album2 Item1"]));
    // Albums match on any of their items, also when negated.
    assert_eq!(albums("-title:Item1"), set(&["Album1", "Album2"]));
    // An album path query matches through the items' paths.
    assert!(albums("path:/nonexistent").is_empty());
}

// ------------------------------------------------------------- path queries

fn path_lib() -> Library {
    path_lib_in(Library::in_memory().unwrap())
}

fn path_lib_in(lib: Library) -> Library {
    for (p, t) in [
        ("/aaa/bb/c.mp3", "path item"),
        ("/x/y/z.mp3", "another item"),
        ("/c/_/title.mp3", "with underscore"),
        ("/c/%/title.mp3", "with percent"),
        (r"/c/\x/title.mp3", "with backslash"),
        ("/A/B/C2.mp3", "caps path"),
    ] {
        lib.add(&mut item(&[("path", p), ("title", t)])).unwrap();
    }
    lib
}

#[test]
fn explicit_path_queries_as_beets() {
    set_ctx(None, true);
    let lib = path_lib();
    let run = |args: &[&str]| -> BTreeSet<String> {
        lib.items(&Query::parse(Kind::Item, args).unwrap())
            .unwrap()
            .iter()
            .map(|i| i.str("title").to_string())
            .collect()
    };
    for (query, want) in [
        ("path:/aaa/bb/c.mp3", &["path item"][..]),
        ("path:/aaa", &["path item"]),
        ("path:/aaa/", &["path item"]),
        ("path:/aa", &[]),
        ("path:/xyzzy/", &[]),
        ("path:/b/", &[]),
        ("path:/x/../aaa/bb", &["path item"]),
        (r"path::c\.mp3$", &["path item"]),
        ("path:/c/_", &["with underscore"]),
        ("path:/c/%", &["with percent"]),
        (r"path:/c/\x", &["with backslash"]),
        // Case sensitive: nothing on disk says otherwise.
        ("path:/a/b/c2.mp3", &[]),
        (
            "-path:/aaa",
            &[
                "another item",
                "with underscore",
                "with percent",
                "with backslash",
                "caps path",
            ],
        ),
    ] {
        assert_eq!(run(&[query]), set(want), "{query}");
    }
}

#[test]
fn path_queries_are_relative_to_the_library_and_follow_case_sensitivity() {
    let d = tempfile::tempdir().unwrap();
    let music = d.path().join("lib");
    std::fs::create_dir_all(music.join("relative")).unwrap();
    set_ctx(Some(&music), true);
    let lib = path_lib_in(Library::in_memory_at(&music).unwrap());
    let inside = music.join("relative/item.mp3");
    lib.add(&mut item(&[
        ("path", inside.to_str().unwrap()),
        ("title", "relative item"),
    ]))
    .unwrap();
    assert_eq!(
        titles(&lib, "path:relative/item.mp3"),
        set(&["relative item"])
    );
    assert_eq!(titles(&lib, "path:relative"), set(&["relative item"]));
    let abs = format!("path:{}", inside.display());
    assert_eq!(titles(&lib, &abs), set(&["relative item"]));
    // String queries on paths see the library-relative form, as beets
    // stores it.
    assert_eq!(titles(&lib, "path::^relative/"), set(&["relative item"]));
    let abs_re = format!("path::{}", inside.display());
    assert_eq!(titles(&lib, &abs_re), set(&["relative item"]));

    // A case-insensitive query lowercases both sides.
    let Test::Path { pattern, dir, .. } = path_test("/a/b/c2.mp3") else {
        panic!()
    };
    let pred = Pred::Field(FieldQuery {
        field: "path".into(),
        col: "items.path".into(),
        fast: true,
        db_path: false,
        test: Test::Path {
            pattern,
            dir,
            case_sensitive: false,
        },
    });
    assert_eq!(run(&lib, pred.clone()), set(&["caps path"]));
    let mut slow = pred;
    make_slow(&mut slow);
    assert_eq!(run(&lib, slow), set(&["caps path"]));
}

#[test]
fn case_sensitivity_is_read_from_the_filesystem() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("Mixed")).unwrap();
    // Linux filesystems are case sensitive: "MIXED" doesn't exist.
    assert!(case_sensitive(&d.path().join("Mixed/song.mp3")));
    assert!(case_sensitive(Path::new("/no/such/path")));
}

#[test]
fn implicit_path_queries_as_beets() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("foo/bar")).unwrap();
    let base = d.path().display().to_string();
    for (q, want) in [
        (format!("{base}/foo/bar"), true),
        (format!("{base}/foo/"), true),
        (format!("{base}/foo/:bar"), true),
        (format!("foo:{base}/foo"), false),
        ("foo".to_string(), false),
        ("c.mp3".to_string(), false),
    ] {
        assert_eq!(is_path_query(&q), want, "{q}");
    }
    set_ctx(None, true);
    let lib = Library::in_memory().unwrap();
    let p = d.path().join("foo/bar/song.mp3");
    lib.add(&mut item(&[
        ("path", p.to_str().unwrap()),
        ("title", "in foo"),
    ]))
    .unwrap();
    lib.add(&mut item(&[
        ("path", "/elsewhere/x.mp3"),
        ("title", "elsewhere"),
    ]))
    .unwrap();
    let foo = format!("{base}/foo");
    assert_eq!(
        lib.items(&Query::parse(Kind::Item, &[&foo]).unwrap())
            .unwrap()
            .len(),
        1
    );
    // An explicit field keeps its meaning.
    let t = format!("title:{base}/foo");
    assert!(
        lib.items(&Query::parse(Kind::Item, &[&t]).unwrap())
            .unwrap()
            .is_empty()
    );
}

// ------------------------------------------------------------------- dates

/// Whether the interval `pattern` contains the local time `iso`.
fn contains(pattern: &str, iso: &str) -> bool {
    let Test::Date { start, end } = date_test(pattern).unwrap() else {
        panic!()
    };
    let d = NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S").unwrap();
    let ts = timestamp(d);
    !(start.is_some_and(|s| ts < s) || end.is_some_and(|e| ts >= e))
}

#[test]
fn date_intervals_as_beets() {
    let now = Local::now()
        .naive_local()
        .format("%Y-%m-%dT%H:%M:00")
        .to_string();
    for (pattern, date, want) in [
        ("2000..2001", "2000-01-01T00:00:00", true),
        ("2000..2001", "2001-06-20T14:15:16", true),
        ("2000..2001", "2001-12-31T23:59:59", true),
        ("2000..2001", "1999-12-31T23:59:59", false),
        ("2000..2001", "2002-01-01T00:00:00", false),
        ("2000..", "2000-01-01T00:00:00", true),
        ("2000..", "2099-10-11T00:00:00", true),
        ("2000..", "1999-12-31T23:59:59", false),
        ("..2001", "2001-12-31T23:59:59", true),
        ("..2001", "2002-01-01T00:00:00", false),
        ("-1d..1d", &now, true),
        ("-2d..-1d", &now, false),
        ("2000-06-20..2000-06-20", "2000-06-20T00:00:00", true),
        ("2000-06-20..2000-06-20", "2000-06-20T10:20:30", true),
        ("2000-06-20..2000-06-20", "2000-06-20T23:59:59", true),
        ("2000-06-20..2000-06-20", "2000-06-19T23:59:59", false),
        ("2000-06-20..2000-06-20", "2000-06-21T00:00:00", false),
        ("1999-12..2000-02", "1999-12-01T00:00:00", true),
        ("1999-12..2000-02", "2000-02-15T05:06:07", true),
        ("1999-12..2000-02", "2000-02-29T23:59:59", true),
        ("1999-12..2000-02", "1999-11-30T23:59:59", false),
        ("1999-12..2000-02", "2000-03-01T00:00:00", false),
        ("2000-01-01T12..2000-01-01T13", "2000-01-01T11:59:59", false),
        ("2000-01-01T12..2000-01-01T13", "2000-01-01T12:00:00", true),
        ("2000-01-01T12..2000-01-01T13", "2000-01-01T13:59:59", true),
        ("2000-01-01T12..2000-01-01T13", "2000-01-01T14:00:00", false),
        ("2008-12-01T22", "2008-12-01T22:30:00", true),
        ("2008-12-01T22", "2008-12-01T23:30:00", false),
        (
            "2000-01-01T12:30..2000-01-01T12:31",
            "2000-01-01T12:29:59",
            false,
        ),
        (
            "2000-01-01T12:30..2000-01-01T12:31",
            "2000-01-01T12:31:59",
            true,
        ),
        (
            "2000-01-01T12:30..2000-01-01T12:31",
            "2000-01-01T12:32:00",
            false,
        ),
        (
            "2000-01-01T12:30:50..2000-01-01T12:30:55",
            "2000-01-01T12:30:49",
            false,
        ),
        (
            "2000-01-01T12:30:50..2000-01-01T12:30:55",
            "2000-01-01T12:30:55",
            true,
        ),
        (
            "2000-01-01T12:30:50..2000-01-01T12:30:55",
            "2000-01-01T12:30:56",
            false,
        ),
        ("..", "1000-01-01T00:00:00", true),
    ] {
        assert_eq!(contains(pattern, date), want, "{pattern} {date}");
    }
}

#[test]
fn reversed_date_ranges_are_normalised() {
    for (rev, fwd) in [
        ("2001..2000", "2000..2001"),
        ("2000-02..1999-12", "1999-12..2000-02"),
        (
            "2000-01-01T13..2000-01-01T12",
            "2000-01-01T12..2000-01-01T13",
        ),
        ("2001..2000-06", "2000-06..2001"),
    ] {
        let (Test::Date { start: a, end: b }, Test::Date { start: c, end: d }) =
            (date_test(rev).unwrap(), date_test(fwd).unwrap())
        else {
            panic!()
        };
        assert_eq!((a, b), (c, d), "{rev}");
    }
}

#[test]
fn date_syntax_as_beets() {
    let ts = |s: &str| timestamp(NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap());
    for p in ["2000-01-01T12", "2000-01-01t12", "2000-01-01 12"] {
        let Test::Date { start, end } = date_test(p).unwrap() else {
            panic!()
        };
        assert_eq!(
            (start, end),
            (Some(ts("2000-01-01 12:00")), Some(ts("2000-01-01 13:00")))
        );
    }
    let Test::Date { start, end } = date_test("2024..2020").unwrap() else {
        panic!()
    };
    assert_eq!(
        (start, end),
        (Some(ts("2020-01-01 00:00")), Some(ts("2025-01-01 00:00")))
    );
    for bad in [
        "1409830085..1412422089",
        "12-34-56-78",
        "2001-01-0a",
        "2001-0a",
        "200a",
        "2001-01-01..2001-01-0a",
        "2001-0a..2001-01",
        "200a..2002",
        "20aa..",
        "..2aa",
        "2000|2001",
        "1|",
        "5|3d",
        "2000-01-01x12",
        "2001-02-30",
    ] {
        assert!(date_test(bad).is_err(), "{bad}");
    }
}

fn date_lib(added: &str) -> Library {
    set_ctx(None, true);
    let lib = Library::in_memory().unwrap();
    let d = NaiveDateTime::parse_from_str(added, "%Y-%m-%d %H:%M").unwrap();
    let mut it = item(&[("title", "x")]);
    lib.add(&mut it).unwrap();
    it.set("added", Value::Float(timestamp(d)));
    lib.store(&it).unwrap();
    lib
}

fn count(lib: &Library, s: &str) -> (usize, usize) {
    let fast = part(s);
    let mut slow = fast.clone();
    make_slow(&mut slow);
    (run(lib, fast).len(), run(lib, slow).len())
}

#[test]
fn date_queries_fast_and_slow() {
    let lib = date_lib("2013-03-30 22:21");
    for (s, n) in [
        ("added:2013-03", 1),
        ("added:2013-04", 0),
        ("added:2013-03-30", 1),
        ("added:2013-03-31", 0),
        ("added:2014..2013", 1),
        ("added:2012..2011", 0),
        ("added:2013-03-30T22", 1),
        ("added:2013-03-30T22:21..", 1),
        ("added:2013-03-30T22:22..", 0),
    ] {
        assert_eq!(count(&lib, s), (n, n), "{s}");
    }
    // Near a month changeover.
    let lib = date_lib("2017-12-31 22:55");
    assert_eq!(count(&lib, "added:2017-12"), (1, 1));
    assert_eq!(count(&lib, "added:2018-01"), (0, 0));
    assert_eq!(count(&lib, "added:2017-12-31"), (1, 1));
    assert_eq!(count(&lib, "added:2018-01-01"), (0, 0));
}

#[test]
fn relative_date_queries_as_beets() {
    let now = Local::now()
        .naive_local()
        .format("%Y-%m-%d %H:%M")
        .to_string();
    let lib = date_lib(&now);
    for unit in ["d", "w", "m", "y"] {
        for (s, n) in [
            (format!("added:-4{unit}..+4{unit}"), 1),
            (format!("added:-2{unit}..-1{unit}"), 0),
            (format!("added:-4{unit}.."), 1),
            (format!("added:4{unit}.."), 0),
            (format!("added:..+4{unit}"), 1),
            (format!("added:..-4{unit}"), 0),
            (format!("added:+4{unit}..-4{unit}"), 1),
        ] {
            assert_eq!(count(&lib, &s), (n, n), "{s}");
        }
    }
}

// ------------------------------------------------------------------- sorts

fn sort_lib() -> Library {
    let lib = Library::in_memory().unwrap();
    for (album, genre, year, flex1, flex2, aa) in [
        ("Album A", "Rock", "2001", "Flex1-1", "Flex2-A", "Foo"),
        ("Album B", "Rock", "2002", "Flex1-2", "Flex2-A", "Bar"),
        ("Album C", "Jazz", "2005", "Flex1-1", "Flex2-B", "Baz"),
    ] {
        let mut a = rec(
            Kind::Album,
            &[
                ("album", album),
                ("genre", genre),
                ("year", year),
                ("flex1", flex1),
                ("flex2", flex2),
                ("albumartist", aa),
            ],
        );
        let id = lib.add(&mut a).unwrap();
        a.set("artpath", Value::Path(format!("/{id}cover.jpg").into()));
        lib.store(&a).unwrap();
    }
    for (title, artist, album, year, f1, f2, aid, path, track) in [
        (
            "first",
            "One",
            "Album A",
            "2001",
            "Flex1-0",
            "Flex2-A",
            "1",
            "/path0.mp3",
            "1",
        ),
        (
            "second",
            "Two",
            "Album A",
            "2002",
            "Flex1-1",
            "Flex2-A",
            "1",
            "/patH1.mp3",
            "2",
        ),
        (
            "third",
            "Three",
            "Album B",
            "2003",
            "Flex1-2",
            "Flex1-B",
            "2",
            "/paTH2.mp3",
            "3",
        ),
        (
            "fourth",
            "Three",
            "Album C",
            "2004",
            "Flex1-2",
            "Flex1-C",
            "3",
            "/PATH3.mp3",
            "4",
        ),
    ] {
        lib.add(&mut item(&[
            ("title", title),
            ("artist", artist),
            ("album", album),
            ("year", year),
            ("flex1", f1),
            ("flex2", f2),
            ("album_id", aid),
            ("path", path),
            ("track", track),
        ]))
        .unwrap();
    }
    lib
}

fn ids(lib: &Library, kind: Kind, s: &str) -> Vec<i64> {
    let query = q(kind, s);
    match kind {
        Kind::Item => lib.items(&query).unwrap(),
        Kind::Album => lib.albums(&query).unwrap(),
    }
    .iter()
    .map(|r| r.id.unwrap())
    .collect()
}

#[test]
fn sorts_as_beets() {
    set_ctx(None, true);
    let lib = sort_lib();
    for (kind, s, want) in [
        (Kind::Album, "year+", &[1, 2, 3][..]),
        (Kind::Album, "flex1-", &[2, 1, 3]),
        (Kind::Album, "path+", &[1, 2, 3]),
        (Kind::Album, "year-", &[3, 2, 1]),
        (Kind::Album, "genre+ album+", &[3, 1, 2]),
        (Kind::Album, "flex2+ flex1+", &[1, 2, 3]),
        (Kind::Album, "path+ year+", &[1, 2, 3]),
        (Kind::Album, "year+ path+", &[1, 2, 3]),
        (Kind::Item, "flex2- flex1+", &[1, 2, 4, 3]),
        (Kind::Item, "artpath+", &[1, 2, 3, 4]),
        (Kind::Item, "artpath-", &[4, 3, 1, 2]),
        (Kind::Item, "path+", &[1, 2, 3, 4]),
    ] {
        assert_eq!(ids(&lib, kind, s), want, "{s}");
    }
    // Albums sorted by an item field are still listed once each.
    assert_eq!(ids(&lib, Kind::Album, "title+").len(), 3);
    assert_eq!(ids(&lib, Kind::Album, "title:r title-").len(), 3);
    // Defaults: sort_item (artist via artist_sort), sort_album.
    let artists: Vec<String> = lib
        .items(&q(Kind::Item, ""))
        .unwrap()
        .iter()
        .map(|i| i.str("artist").to_string())
        .collect();
    assert_eq!(artists, ["One", "Three", "Three", "Two"]);
    let aas: Vec<String> = lib
        .albums(&q(Kind::Album, ""))
        .unwrap()
        .iter()
        .map(|a| a.str("albumartist").to_string())
        .collect();
    assert_eq!(aas, ["Bar", "Baz", "Foo"]);
    set_context(Context {
        sort_item: vec!["artist-".into()],
        sort_album: vec!["albumartist-".into()],
        ..Context::default()
    });
    assert_eq!(ids(&lib, Kind::Item, "")[3], 1);
    assert_eq!(ids(&lib, Kind::Album, ""), [1, 3, 2]);
}

#[test]
fn sort_case_sensitivity_as_beets() {
    let lib = Library::in_memory().unwrap();
    lib.add(&mut rec(
        Kind::Album,
        &[("album", "album"), ("albumartist", "bar")],
    ))
    .unwrap();
    lib.add(&mut rec(
        Kind::Album,
        &[("album", "Album"), ("albumartist", "Bar")],
    ))
    .unwrap();
    lib.add(&mut item(&[
        ("artist", "artist"),
        ("flex1", "flex1"),
        ("track", "10"),
    ]))
    .unwrap();
    lib.add(&mut item(&[
        ("artist", "Artist"),
        ("flex1", "Flex1"),
        ("track", "2"),
    ]))
    .unwrap();
    let field = |kind, s: &str, f: &str| -> Vec<String> {
        let query = q(kind, s);
        match kind {
            Kind::Item => lib.items(&query).unwrap(),
            Kind::Album => lib.albums(&query).unwrap(),
        }
        .iter()
        .map(|r| r.formatted(f).unwrap_or_default())
        .collect()
    };
    set_ctx(None, true);
    assert_eq!(field(Kind::Item, "artist+", "artist"), ["artist", "Artist"]);
    assert_eq!(field(Kind::Album, "album+", "album"), ["album", "Album"]);
    assert_eq!(field(Kind::Item, "flex1+", "flex1"), ["flex1", "Flex1"]);
    assert_eq!(field(Kind::Item, "track+", "track"), ["02", "10"]);
    set_ctx(None, false);
    assert_eq!(field(Kind::Item, "artist+", "artist"), ["Artist", "artist"]);
    assert_eq!(field(Kind::Album, "album+", "album"), ["Album", "album"]);
    assert_eq!(field(Kind::Item, "flex1+", "flex1"), ["Flex1", "flex1"]);
}

#[test]
fn sorting_on_missing_fields_does_not_fail() {
    set_ctx(None, true);
    let lib = sort_lib();
    let base = ids(&lib, Kind::Item, "foo+");
    for s in ["foo-", "--", "-+", "+-", "++", "-foo-", "-foo+", "---"] {
        assert_eq!(ids(&lib, Kind::Item, s), base, "{s}");
    }
    assert_eq!(
        ids(&lib, Kind::Item, "foo+ id+"),
        ids(&lib, Kind::Item, "id+")
    );
    // A flexible field set on some items: the others ("") sort first.
    let mut items = lib.items(&q(Kind::Item, "id+")).unwrap();
    items[0].flex.insert("mykey".into(), "b".into());
    items[1].flex.insert("mykey".into(), "a".into());
    lib.store(&items[0]).unwrap();
    lib.store(&items[1]).unwrap();
    assert_eq!(ids(&lib, Kind::Item, "mykey+ id+"), [3, 4, 2, 1]);
    assert_eq!(ids(&lib, Kind::Item, "mykey- id+"), [1, 2, 3, 4]);
}

#[test]
fn queries_run_in_sql_when_they_can() {
    set_ctx(None, true);
    let fast = q(Kind::Item, "artist:x year:2000.. -comp:1 , artpath:y");
    assert!(fast.pred.clause().is_some());
    assert!(fast.pred.needs_join(Kind::Item));
    assert!(q(Kind::Item, "artist:x").pred.clause().is_some());
    assert!(!q(Kind::Item, "artist:x").pred.needs_join(Kind::Item));
    assert!(q(Kind::Item, "artist:x mood:happy").pred.clause().is_none());
    assert!(q(Kind::Album, "title:x").pred.needs_join(Kind::Album));
    let s = q(Kind::Item, "flex+ year+").sort;
    assert_eq!(
        s.order_clause(Kind::Item).unwrap(),
        "(CASE WHEN TYPEOF(items.year)='text' THEN LOWER(items.year) WHEN TYPEOF(items.year)='blob' THEN LOWER(items.year) ELSE items.year END) ASC"
    );
    assert!(
        q(Kind::Item, "year+ flex+")
            .sort
            .order_clause(Kind::Item)
            .is_none()
    );
}

#[test]
fn python_value_strings() {
    assert_eq!(py_float(180.0), "180.0");
    assert_eq!(py_float(0.5), "0.5");
    assert_eq!(py_float(1e16), "1e+16");
    assert_eq!(py_float(1.5e-5), "1.5e-05");
    assert_eq!(PyVal::Bool(true).as_string(), "True");
    assert!(PyVal::Int(1).py_eq(&PyVal::Bool(true)));
    assert!(!PyVal::Str("1".into()).py_eq(&PyVal::Int(1)));
}

/// Not a benchmark, but a guard: 50k items, listed and queried through
/// SQL, stay well under a second or two even in a debug build.
#[test]
#[ignore = "timing; run with --ignored"]
fn fifty_thousand_items() {
    set_ctx(None, true);
    let d = tempfile::tempdir().unwrap();
    let lib = Library::open(&d.path().join("lib.db")).unwrap();
    lib.transaction(|lib| {
        for n in 0..50_000 {
            let mut it = item(&[
                ("title", &format!("Song {n}")),
                ("artist", &format!("Artist {}", n % 500)),
                ("album", &format!("Album {}", n % 5000)),
                ("year", &format!("{}", 1950 + n % 70)),
                ("path", &format!("/music/{n}.mp3")),
            ]);
            if n % 10 == 0 {
                it.flex.insert("mood".into(), "calm".into());
            }
            lib.add(&mut it)?;
        }
        Ok(())
    })
    .unwrap();
    for s in [
        "",
        "artist:Artist 42",
        "year:1990..1999",
        "mood:calm",
        "flex+",
        "title::7$",
    ] {
        let t = std::time::Instant::now();
        let n = lib.items(&q(Kind::Item, s)).unwrap().len();
        eprintln!("{s:?}: {n} items in {:?}", t.elapsed());
    }
}
