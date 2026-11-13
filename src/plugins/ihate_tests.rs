//! Ported from beets' test/plugins/test_ihate.py.

use super::*;
use crate::config::Config;
use crate::model::Item;
use crate::plugins::Plugin;

fn item() -> Item {
    let mut i = Item::new_item();
    i.set_list("genres", &["TestGenre".to_string()]);
    i.set_str("album", "TestAlbum");
    i.set_str("artist", "TestArtist");
    i
}

fn cfg(warn: &[&str], skip: &[&str]) -> Config {
    let mut c = Config::defaults();
    c.set("plugins", serde_yaml::from_str("[ihate]").unwrap());
    let list = |xs: &[&str]| {
        serde_yaml::Value::Sequence(
            xs.iter()
                .map(|x| serde_yaml::Value::String((*x).into()))
                .collect(),
        )
    };
    if !warn.is_empty() {
        c.set("ihate.warn", list(warn));
    }
    if !skip.is_empty() {
        c.set("ihate.skip", list(skip));
    }
    c
}

/// The upstream test's cases, as `do_i_hate_this(task, patterns)`.
#[test]
fn hate() {
    let it = item();
    let cases: &[(&[&str], bool)] = &[
        (&[], false),
        (&["artist:bad_artist", "artist:TestArtist"], true),
        (&["album:test", "artist:testartist"], true),
        (&["album:notthis genres:testgenre"], false),
        (
            &[
                "album:notthis genres:testgenre",
                "artist:testartist album:notthis",
            ],
            false,
        ),
        (
            &[
                "album:testalbum genres:testgenre",
                "artist:testartist album:notthis",
            ],
            true,
        ),
    ];
    for (patterns, expected) in cases {
        let patterns: Vec<String> = patterns.iter().map(ToString::to_string).collect();
        assert_eq!(
            do_i_hate_this(std::slice::from_ref(&it), false, &patterns),
            *expected,
            "{patterns:?}"
        );
    }
}

#[test]
fn skip_and_warn_queries_apply_to_an_applied_match() {
    let it = item();
    let cfg = cfg(&["artist:other"], &["artist:TestArtist"]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ctx = crate::plugins::PluginCtx {
        cfg: &cfg,
        lib: &lib,
        write: false,
    };
    let plugin = load(ctx.cfg);
    let info = crate::plugins::ChoiceInfo {
        imported_items: std::slice::from_ref(&it),
        album: false,
        apply: true,
        desc: "TestArtist - TestAlbum".into(),
    };
    assert!(plugin.import_task_choice_skip(&ctx, &info));
}

#[test]
fn warn_only_does_not_skip() {
    let it = item();
    let cfg = cfg(&["artist:TestArtist"], &[]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ctx = crate::plugins::PluginCtx {
        cfg: &cfg,
        lib: &lib,
        write: false,
    };
    let plugin = load(ctx.cfg);
    let info = crate::plugins::ChoiceInfo {
        imported_items: std::slice::from_ref(&it),
        album: false,
        apply: true,
        desc: "TestArtist - TestAlbum".into(),
    };
    assert!(!plugin.import_task_choice_skip(&ctx, &info));
}

#[test]
fn an_as_is_choice_is_never_hated() {
    let it = item();
    let cfg = cfg(&[], &["artist:TestArtist"]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ctx = crate::plugins::PluginCtx {
        cfg: &cfg,
        lib: &lib,
        write: false,
    };
    let plugin = load(ctx.cfg);
    let info = crate::plugins::ChoiceInfo {
        imported_items: std::slice::from_ref(&it),
        album: false,
        apply: false,
        desc: "TestArtist - TestAlbum".into(),
    };
    assert!(!plugin.import_task_choice_skip(&ctx, &info));
}
