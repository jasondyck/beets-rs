//! `ls`, `ls -a`, `-f` templates and the query language.

mod common;

use common::Env;

fn corpus() -> Option<Env> {
    let env = Env::new()?;
    env.add_album("Artist A", "Album A", "2001", &["Alpha", "Beta", "Gamma"]);
    env.add_album("Artist B", "Album B", "2010", &["Delta"]);
    env.import();
    Some(env)
}

#[test]
fn templates_sorts_and_computed_fields() {
    let Some(env) = corpus() else { return };
    assert_eq!(
        env.ls(&["-f", "$artist|$album|$year|$track|$title"]),
        [
            "Artist A|Album A|2001|01|Alpha",
            "Artist A|Album A|2001|02|Beta",
            "Artist A|Album A|2001|03|Gamma",
            "Artist B|Album B|2010|01|Delta",
        ]
    );
    assert_eq!(
        env.ls(&["-a", "-f", "$albumartist|$album|$year|$albumtotal"]),
        ["Artist A|Album A|2001|0", "Artist B|Album B|2010|0"]
    );
    // An album field reached from an item, and the `%if`/%upper functions.
    assert_eq!(
        env.ls(&["-f", "$title|$album|%upper{$artist}"]),
        [
            "Alpha|Album A|ARTIST A",
            "Beta|Album A|ARTIST A",
            "Gamma|Album A|ARTIST A",
            "Delta|Album B|ARTIST B",
        ]
    );
    // Explicit sorts (the default is artist+, album+, disc+, track+).
    assert_eq!(
        env.ls(&["-f", "$title", "title-"]),
        ["Gamma", "Delta", "Beta", "Alpha"]
    );
    assert_eq!(
        env.ls(&["-f", "$title", "year-", "track+"]),
        ["Delta", "Alpha", "Beta", "Gamma"]
    );
    assert_eq!(
        env.ls(&["-a", "-f", "$album", "year-"]),
        ["Album B", "Album A"]
    );
}

#[test]
fn query_fields_regexes_ranges_and_logic() {
    let Some(env) = corpus() else { return };
    let titles = |q: &[&str]| {
        let mut args = vec!["-f", "$title"];
        args.extend_from_slice(q);
        env.ls(&args)
    };
    // Substring and exact (`=`) / case-insensitive (`=~`) matches.
    assert_eq!(titles(&["artist:Artist A"]), ["Alpha", "Beta", "Gamma"]);
    assert_eq!(titles(&["title:=Beta"]), ["Beta"]);
    assert_eq!(titles(&["title:=~beta"]), ["Beta"]);
    assert!(titles(&["title:=beta"]).is_empty(), "= is case-sensitive");
    // Regexes (`:` after the field, or alone).
    assert_eq!(titles(&["title::^A"]), ["Alpha"]);
    assert_eq!(titles(&["title::a$"]), ["Alpha", "Beta", "Gamma", "Delta"]);
    // Numeric ranges on typed fields.
    assert_eq!(titles(&["track:2..3"]), ["Beta", "Gamma"]);
    assert_eq!(titles(&["year:2005.."]), ["Delta"]);
    assert_eq!(titles(&["year:..2001"]), ["Alpha", "Beta", "Gamma"]);
    // Negation (`^`) and OR (a trailing comma ends a group).
    assert_eq!(titles(&["^year:2001"]), ["Delta"]);
    assert_eq!(
        titles(&["year:2001,", "album:Album B"]),
        ["Alpha", "Beta", "Gamma", "Delta"]
    );
    // Album queries see the album's own and their items' fields.
    assert_eq!(
        env.ls(&["-a", "-f", "$album", "album:Album A"]),
        ["Album A"]
    );
    assert_eq!(env.ls(&["-a", "-f", "$album", "year:2010"]), ["Album B"]);
    assert_eq!(env.ls(&["-a", "-f", "$album", "title:Delta"]), ["Album B"]);
}
