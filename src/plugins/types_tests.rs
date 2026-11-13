//! beets' test/plugins/test_types_plugin.py, the plugins' declared types,
//! and queries on album computed fields.

use super::*;
use crate::library::Library;
use crate::model::{Item, Kind, Record, Value};
use crate::query::Query;
use crate::template::Template;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    lib: Library,
}

impl Env {
    /// A library with `plugins` enabled and these `types:`.
    fn new(plugins: &str, types: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str(plugins).unwrap());
        cfg.set("types", serde_yaml::from_str(types).unwrap());
        configure(&cfg).unwrap();
        crate::formatted::configure(&cfg).unwrap();
        crate::model::set_time_format(&cfg.str("time_format"));
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env { _dir: dir, lib }
    }

    fn add_item(&self, artist: &str) -> Item {
        let mut it = Item::new_item();
        it.set_path(format!("/nonexistent/{artist}.mp3"));
        it.set_str("artist", artist);
        self.lib.add(&mut it).unwrap();
        it
    }

    /// `modify` (no write, no move) of the items or albums matching `q`.
    fn modify(&self, kind: Kind, q: &[&str], sets: &[(&str, &str)]) {
        let q: Vec<String> = q.iter().map(|s| s.to_string()).collect();
        let query = Query::parse(kind, &q).unwrap();
        let objs = match kind {
            Kind::Item => self.lib.items(&query).unwrap(),
            Kind::Album => self.lib.albums(&query).unwrap(),
        };
        for mut o in objs {
            for (k, v) in sets {
                o.set_parsed(k, v).unwrap();
            }
            self.lib.store(&o).unwrap();
        }
    }

    /// `ls [-a] -f FMT QUERY`, lines joined.
    fn list(&self, kind: Kind, q: &str, fmt: &str) -> String {
        let parts: Vec<String> = crate::paths::shell_split(q);
        let query = Query::parse(kind, &parts).unwrap();
        let objs = match kind {
            Kind::Item => self.lib.items(&query).unwrap(),
            Kind::Album => self.lib.albums(&query).unwrap(),
        };
        let t = Template::parse(fmt);
        objs.iter()
            .map(|o| crate::commands::format_record(&t, o, Some(&self.lib)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn reload(&self, rec: &Record) -> Record {
        match rec.kind {
            Kind::Item => self.lib.get_item(rec.id.unwrap()).unwrap().unwrap(),
            Kind::Album => self.lib.get_album(rec.id.unwrap()).unwrap().unwrap(),
        }
    }
}

const ITEM_FMT: &str = "$artist - $album - $title";

#[test]
fn integer_modify_and_query() {
    let env = Env::new("[types]", "{myint: int}");
    let item = env.add_item("aaa");
    // Do not match unset values.
    assert_eq!(env.list(Kind::Item, "myint:1..3", ITEM_FMT), "");
    env.modify(Kind::Item, &[], &[("myint", "2")]);
    assert_eq!(env.reload(&item).get("myint"), Some(Value::Int(2)));
    assert!(env.list(Kind::Item, "myint:1..3", ITEM_FMT).contains("aaa"));
}

#[test]
fn album_integer_modify_and_query() {
    let env = Env::new("[types]", "{myint: int}");
    let mut item = Item::new_item();
    item.set_path("/nonexistent/a.mp3");
    item.set_str("albumartist", "aaa");
    let album = env.lib.add_album(&mut [item]).unwrap();
    let fmt = "$albumartist - $album - $title";
    assert_eq!(env.list(Kind::Album, "myint:1..3", fmt), "");
    env.modify(Kind::Album, &[], &[("myint", "2")]);
    assert_eq!(env.reload(&album).get("myint"), Some(Value::Int(2)));
    assert!(env.list(Kind::Album, "myint:1..3", fmt).contains("aaa"));
}

#[test]
fn float_modify_and_query() {
    let env = Env::new("[types]", "{myfloat: float}");
    let item = env.add_item("aaa");
    assert_eq!(env.list(Kind::Item, "myfloat:10..0", ITEM_FMT), "");
    env.modify(Kind::Item, &[], &[("myfloat", "-9.1")]);
    assert_eq!(env.reload(&item).get("myfloat"), Some(Value::Float(-9.1)));
    assert!(
        env.list(Kind::Item, "myfloat:-10..0", ITEM_FMT)
            .contains("aaa")
    );
}

#[test]
fn bool_modify_and_query() {
    let env = Env::new("[types]", "{mybool: bool}");
    let t = env.add_item("true");
    let f = env.add_item("false");
    env.add_item("unset");
    // Do not match unset values.
    assert_eq!(
        env.list(Kind::Item, "mybool:true , mybool:false", ITEM_FMT),
        ""
    );
    env.modify(Kind::Item, &["artist:true"], &[("mybool", "1")]);
    assert_eq!(env.reload(&t).get("mybool"), Some(Value::Bool(true)));
    env.modify(Kind::Item, &["artist:false"], &[("mybool", "false")]);
    assert_eq!(env.reload(&f).get("mybool"), Some(Value::Bool(false)));
    assert_eq!(
        env.list(Kind::Item, "mybool:true", "$artist $mybool"),
        "true True"
    );
    assert_eq!(
        env.list(Kind::Item, "mybool:false", "$artist $mybool"),
        "false False"
    );
}

fn mktime(y: i32, m: u32, d: u32) -> f64 {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(y, m, d, 0, 0, 0)
        .earliest()
        .unwrap()
        .timestamp() as f64
}

#[test]
fn date_modify_and_query() {
    let env = Env::new("[types]", "{mydate: date}");
    crate::model::set_time_format("%Y-%m-%d");
    let old = env.add_item("prince");
    let new = env.add_item("britney");
    assert_eq!(env.list(Kind::Item, "mydate:..2000", ITEM_FMT), "");
    env.modify(Kind::Item, &["artist:prince"], &[("mydate", "1999-01-01")]);
    assert_eq!(
        env.reload(&old).get("mydate"),
        Some(Value::Float(mktime(1999, 1, 1)))
    );
    env.modify(Kind::Item, &["artist:britney"], &[("mydate", "1999-12-30")]);
    assert_eq!(
        env.reload(&new).get("mydate"),
        Some(Value::Float(mktime(1999, 12, 30)))
    );
    assert_eq!(
        env.list(Kind::Item, "mydate:..1999-07", "$artist $mydate"),
        "prince 1999-01-01"
    );
    assert_eq!(
        env.list(Kind::Item, "mydate:1999-12-30", "$artist $mydate"),
        "britney 1999-12-30"
    );
    crate::model::set_time_format(crate::model::TIME_FORMAT);
}

#[test]
fn unknown_type_error() {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[types]").unwrap());
    cfg.set(
        "types",
        serde_yaml::from_str("{flex: unkown type}").unwrap(),
    );
    let err = configure(&cfg).unwrap_err();
    assert_eq!(
        err.to_string(),
        "configuration error: unknown type 'unkown type' for the 'flex' field"
    );
    // Without the plugin, the settings are ignored.
    cfg.set("plugins", serde_yaml::from_str("[]").unwrap());
    assert!(configure(&cfg).is_ok());
}

#[test]
fn template_if_def() {
    let env = Env::new("[types]", "{playcount: int, rating: float, starred: bool}");
    env.add_item("prince");
    env.modify(Kind::Item, &["artist:prince"], &[("playcount", "10")]);
    env.modify(Kind::Item, &["artist:prince"], &[("rating", "5.0")]);
    env.modify(Kind::Item, &["artist:prince"], &[("starred", "yes")]);
    env.add_item("britney");
    let t = |q: &str, fmt: &str| env.list(Kind::Item, q, fmt);
    let int_t = "%ifdef{playcount,Play count: $playcount,Not played}";
    assert_eq!(t("artist:prince", int_t), "Play count: 10");
    assert_eq!(t("artist:britney", int_t), "Not played");
    let float_t = "%ifdef{rating,Rating: $rating,Not rated}";
    assert_eq!(t("artist:prince", float_t), "Rating: 5.0");
    assert_eq!(t("artist:britney", float_t), "Not rated");
    let bool_t = "%ifdef{starred,Starred: $starred,Not starred}";
    assert_eq!(t("artist:prince", bool_t), "Starred: True");
    assert_eq!(t("artist:britney", bool_t), "Not starred");
}

// ------------------------------------------------------------ beyond beets'

#[test]
fn stored_as_beets_writes_them() {
    let env = Env::new("[types]", "{i: int, f: float, b: bool, d: date}");
    let item = env.add_item("a");
    env.modify(
        Kind::Item,
        &[],
        &[
            ("i", "abc"),
            ("f", "3.14159"),
            ("b", "yes"),
            ("d", "garbage"),
        ],
    );
    let r = env.reload(&item);
    let flex = |k: &str| r.flex[k].clone();
    assert_eq!(
        [flex("i"), flex("f"), flex("b"), flex("d")],
        ["0", "3.14159", "1", "0.0"]
    );
    assert_eq!(env.list(Kind::Item, "", "$i|$f|$b"), "0|3.1|True");
}

#[test]
fn unset_typed_values_sort_as_null() {
    let env = Env::new("[types]", "{n: int}");
    for (artist, n) in [
        ("a", Some("2")),
        ("b", Some("0")),
        ("c", None),
        ("d", Some("10")),
    ] {
        env.add_item(artist);
        if let Some(n) = n {
            env.modify(Kind::Item, &[&format!("artist:{artist}")], &[("n", n)]);
        }
    }
    // Numbers, not strings; the unset one counts as 0 (a stable sort).
    assert_eq!(env.list(Kind::Item, "n+", "$artist"), "b\nc\na\nd");
    assert_eq!(env.list(Kind::Item, "n-", "$artist"), "d\na\nb\nc");
}

#[test]
fn plugins_declare_types_and_conflicts_are_errors() {
    let env = Env::new("[lyrics, missing]", "{}");
    assert_eq!(
        crate::model::flex_type(Kind::Item, "lyrics_instrumental"),
        Some(FieldType::Bool)
    );
    assert_eq!(
        crate::model::flex_type(Kind::Album, "missing"),
        Some(FieldType::Int)
    );
    assert_eq!(crate::model::flex_type(Kind::Item, "missing"), None);
    // lyrics_instrumental formats as a boolean.
    env.add_item("x");
    env.modify(Kind::Item, &[], &[("lyrics_instrumental", "1")]);
    assert_eq!(
        env.list(
            Kind::Item,
            "lyrics_instrumental:true",
            "$lyrics_instrumental"
        ),
        "True"
    );

    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[types, missing]").unwrap());
    cfg.set("types", serde_yaml::from_str("{missing: float}").unwrap());
    assert_eq!(
        configure(&cfg).unwrap_err().to_string(),
        "Plugin missing defines flexible field missing which has already been \
         defined with another type."
    );
    // The same type twice is fine.
    cfg.set("types", serde_yaml::from_str("{missing: int}").unwrap());
    assert!(configure(&cfg).is_ok());
}

/// An album of `n` items whose `tracktotal` is `total`.
fn album(env: &Env, name: &str, n: usize, total: i64) {
    let mut items: Vec<Item> = (0..n)
        .map(|i| {
            let mut it = Item::new_item();
            it.set_path(format!("/nonexistent/{name}{i}.mp3"));
            it.set_str("album", name);
            it.set_int("track", i as i64 + 1);
            it.set_int("tracktotal", total);
            it
        })
        .collect();
    env.lib.add_album(&mut items).unwrap();
}

#[test]
fn queries_on_album_computed_fields() {
    let env = Env::new("[missing]", "{}");
    album(&env, "full", 2, 2);
    album(&env, "one short", 2, 3);
    album(&env, "two short", 1, 3);
    let ls = |q: &str| env.list(Kind::Album, q, "$album");
    // `missing` is an integer (the plugin's album_types).
    assert_eq!(ls("missing:1.."), "one short\ntwo short");
    assert_eq!(ls("missing:2"), "two short");
    assert_eq!(ls("missing:0"), "full");
    // ... and a string getter for `=`.
    assert_eq!(ls("missing:=1"), "one short");
    // `albumtotal` is untyped: a substring.
    assert_eq!(ls("albumtotal:3"), "one short\ntwo short");
    assert_eq!(ls("albumtotal:2..3"), "");
    // Sorted by the getters.
    assert_eq!(ls("missing-"), "two short\none short\nfull");
    // Items reach them through their album (as strings: no item type).
    assert_eq!(env.list(Kind::Item, "missing:2", "$album"), "two short");
    assert_eq!(env.list(Kind::Item, "albumtotal:2", "$album"), "full\nfull");
}

#[test]
fn missing_is_no_field_without_the_plugin() {
    let env = Env::new("[]", "{}");
    album(&env, "short", 1, 3);
    assert_eq!(env.list(Kind::Album, "missing:2", "$album"), "");
    // `albumtotal` stays a getter.
    assert_eq!(env.list(Kind::Album, "albumtotal:3", "$album"), "short");
}

/// test/dbcore/test_sort.py `test_field_present_in_some_items`.
#[test]
fn sort_by_int_field_present_in_some_items() {
    let env = Env::new("[types]", "{myint: int}");
    for a in ["a", "b", "c", "d"] {
        env.add_item(a);
    }
    env.modify(Kind::Item, &["id:1"], &[("myint", "2")]);
    env.modify(Kind::Item, &["id:2"], &[("myint", "10")]);
    assert_eq!(env.list(Kind::Item, "myint+ id+", "$id"), "3\n4\n1\n2");
    assert_eq!(env.list(Kind::Item, "myint- id+", "$id"), "2\n1\n3\n4");
}

/// The plugins' `item_types`/`album_types` come from their registry
/// entries (beets' classes), not a central list.
#[test]
fn plugin_types_come_from_the_registry() {
    let cfg = |plugins: &str| {
        let mut c = Config::defaults();
        c.set("plugins", serde_yaml::from_str(plugins).unwrap());
        c
    };
    let [items, albums] = flex_types(&cfg("[acousticbrainz]")).unwrap();
    assert_eq!(items["average_loudness"], FieldType::Float(6));
    assert!(albums.is_empty());
    let [items, albums] = flex_types(&cfg("[missing]")).unwrap();
    assert!(items.is_empty());
    assert_eq!(albums["missing"], FieldType::Int);
    let [items, _] = flex_types(&cfg("[mpdstats]")).unwrap();
    assert_eq!(items["rating"], FieldType::Float(1));
    assert_eq!(items["last_played"], FieldType::Date);
    // `types:` is still the config-driven special case.
    let mut c = cfg("[types]");
    c.set("types", serde_yaml::from_str("{rating: float}").unwrap());
    let [items, _] = flex_types(&c).unwrap();
    assert_eq!(items["rating"], FieldType::Float(1));
}

