//! Ported from beets' test/plugins/test_rewrite.py and
//! test_advancedrewrite.py: the rewritten values as templates show them.

use super::*;
use crate::formatted::{FormatEnv, Formatted};
use crate::library::Library;
use crate::model::Value;
use crate::template::Template;
use std::rc::Rc;

struct Env {
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new(plugin: &str, rules: &str) -> Env {
        let mut cfg = Config::defaults();
        cfg.set("plugins", Y::Sequence(vec![Y::String(plugin.into())]));
        cfg.set(plugin, serde_yaml::from_str(rules).unwrap());
        Env {
            cfg,
            lib: Library::in_memory().unwrap(),
        }
    }

    fn item(&self, fields: &[(&str, &str)]) -> Record {
        let mut it = Record::new_item();
        it.set_path(format!("/m/{}.mp3", fields.len()));
        for (k, v) in fields {
            if is_list(k) {
                let l: Vec<&str> = v.split("; ").collect();
                it.set_list(k, &l);
            } else {
                let _ = it.set_parsed(k, v);
            }
        }
        self.lib.add(&mut it).unwrap();
        it
    }

    fn render(&self, rec: &Record, t: &str) -> String {
        let env = Rc::new(FormatEnv::new(&self.cfg).unwrap());
        Template::parse(t).render(&Formatted::with_env(rec, Some(&self.lib), env))
    }
}

#[test]
fn artist_rewrite_applies_to_artist_albumartist_and_album_fields() {
    let e = Env::new("rewrite", "{'artist .*jimi hendrix.*': Jimi Hendrix}");
    let mut it = e.item(&[
        ("artist", "The Jimi Hendrix Experience"),
        ("albumartist", "The Jimi Hendrix Experience"),
    ]);
    let album = e.lib.add_album(std::slice::from_mut(&mut it)).unwrap();
    assert_eq!(
        e.render(&it, "$artist|$albumartist"),
        "Jimi Hendrix|Jimi Hendrix"
    );
    assert_eq!(e.render(&album, "$albumartist"), "Jimi Hendrix");
    // Stored values are untouched.
    assert_eq!(it.str("artist"), "The Jimi Hendrix Experience");
}

#[test]
fn rewrite_all_matching_rules() {
    let e = Env::new(
        "rewrite",
        "{'artist .*hendrix.*': hendrix catalog, 'artist .*catalog.*': Experience catalog}",
    );
    let it = e.item(&[("artist", "The Jimi Hendrix Experience")]);
    assert_eq!(e.render(&it, "$artist"), "Experience catalog");
}

#[test]
fn rewrite_is_case_insensitive_and_leaves_non_matches_unchanged() {
    let e = Env::new(
        "rewrite",
        "{'artist odd eye circle': LOONA / ODD EYE CIRCLE}",
    );
    let a = e.item(&[("artist", "ODD EYE CIRCLE")]);
    let b = e.item(&[("artist", "ARTMS")]);
    assert_eq!(e.render(&a, "$artist"), "LOONA / ODD EYE CIRCLE");
    assert_eq!(e.render(&b, "$artist"), "ARTMS");
}

#[test]
fn rewrite_applied_to_all_list_values() {
    let e = Env::new(
        "rewrite",
        "{'genres rock': Classic Rock, 'genres pop': Pop}",
    );
    let it = e.item(&[("genres", "rock; pop; techno")]);
    assert_eq!(e.render(&it, "$genres"), "Classic Rock; Pop; techno");
}

#[test]
fn invalid_rewrite_spec_and_field() {
    let e = Env::new("rewrite", "{artist: Jimi Hendrix}");
    assert_eq!(
        Rewriter::rewrite(&e.cfg).unwrap_err().to_string(),
        "invalid rewrite specification"
    );
    let e = Env::new("rewrite", "{'not_a_field rock': Classic Rock}");
    assert_eq!(
        Rewriter::rewrite(&e.cfg).unwrap_err().to_string(),
        "invalid field name (not_a_field) in rewriter"
    );
}

#[test]
fn advanced_simple_rewrite_example() {
    let e = Env::new(
        "advancedrewrite",
        "[{'artist ODD EYE CIRCLE': 이달의 소녀 오드아이써클}]",
    );
    let it = e.item(&[
        ("artist", "ODD EYE CIRCLE"),
        ("albumartist", "ODD EYE CIRCLE"),
    ]);
    assert_eq!(e.render(&it, "$artist"), "이달의 소녀 오드아이써클");
}

#[test]
fn advanced_list_field() {
    let e = Env::new("advancedrewrite", "[{'genres rock': techno}]");
    let it = e.item(&[("genres", "rock; pop")]);
    assert_eq!(e.render(&it, "$genres"), "techno; pop");
}

#[test]
fn advanced_rewrite_example() {
    let e = Env::new(
        "advancedrewrite",
        "[{match: 'mb_artistid:dec0f331-cb08-4c8e-9c9f-aeb1f0f6d88c year:..2022', replacements: {artist: 이달의 소녀 오드아이써클, artist_sort: LOONA / ODD EYE CIRCLE}}]",
    );
    let common = [
        ("artist", "ODD EYE CIRCLE"),
        ("artist_sort", "ODD EYE CIRCLE"),
        ("mb_artistid", "dec0f331-cb08-4c8e-9c9f-aeb1f0f6d88c"),
    ];
    let a = e.item(&[&common[..], &[("year", "2017")]].concat());
    let b = e.item(&[&common[..], &[("year", "2023")]].concat());
    assert_eq!(
        e.render(&a, "$artist|$artist_sort|$albumartist_sort"),
        "이달의 소녀 오드아이써클|LOONA / ODD EYE CIRCLE|LOONA / ODD EYE CIRCLE"
    );
    assert_eq!(e.render(&b, "$artist"), "ODD EYE CIRCLE");
}

#[test]
fn advanced_rewrite_with_multi_valued_field() {
    let e = Env::new(
        "advancedrewrite",
        "[{match: 'artist:배유빈 feat. 김미현', replacements: {artists: [유빈, 미미]}}]",
    );
    let it = e.item(&[
        ("artist", "배유빈 feat. 김미현"),
        ("artists", "배유빈; 김미현"),
    ]);
    assert_eq!(e.render(&it, "$artists"), "유빈; 미미");
}

#[test]
fn advanced_config_errors() {
    let e = Env::new("advancedrewrite", "[{match: 'artist:A', replacements: {}}]");
    assert_eq!(
        Rewriter::advanced(&e.cfg).unwrap_err().to_string(),
        "Advanced rewrites must have at least one replacement"
    );
    let e = Env::new(
        "advancedrewrite",
        "[{match: \"artist:'A & B'\", replacements: {artist: [C, D]}}]",
    );
    assert_eq!(
        Rewriter::advanced(&e.cfg).unwrap_err().to_string(),
        "Field artist is not a multi-valued field but a list was given: C, D"
    );
    let e = Env::new("advancedrewrite", "[{'artist A': B, 'album C': D}]");
    assert!(
        Rewriter::advanced(&e.cfg)
            .unwrap_err()
            .to_string()
            .starts_with("Simple rewrites must have only one rule")
    );
}

#[test]
fn advanced_combined_rewrite_example() {
    let e = Env::new(
        "advancedrewrite",
        "[{'artist A': B}, {match: \"album:'C'\", replacements: {artist: D}}]",
    );
    let a = e.item(&[("artist", "A"), ("albumartist", "A")]);
    assert_eq!(e.render(&a, "$artist"), "B");
    let c = e.item(&[("artist", "C"), ("albumartist", "C"), ("album", "C")]);
    assert_eq!(e.render(&c, "$artist"), "D");
    let _ = Value::Null;
}
