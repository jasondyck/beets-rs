//! stats, `config`, `-l/--library`, album modifies and `write`.

mod common;

use common::{Env, make_mp3};

fn corpus() -> Option<Env> {
    let env = Env::new()?;
    env.add_album("Artist A", "Album A", "2001", &["Alpha", "Beta"]);
    env.add_album("Artist B", "Album B", "2010", &["Gamma"]);
    env.import();
    Some(env)
}

#[test]
fn stats_and_stats_with_a_query() {
    let Some(env) = corpus() else { return };
    let out = env.run(&["stats"]);
    out.assert_ok("stats");
    let text = out.stdout.clone();
    for line in ["Tracks: 3", "Artists: 2", "Albums: 2", "Album artists: 2"] {
        assert!(
            text.lines().any(|l| l == line),
            "missing {line:?} in\n{text}"
        );
    }
    assert!(
        text.lines().any(|l| l.starts_with("Total time: ")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l.starts_with("Approximate total size: ")),
        "{text}"
    );

    let out = env.run(&["stats", "year:2010"]);
    out.assert_ok("stats query");
    for line in ["Tracks: 1", "Artists: 1", "Albums: 1"] {
        assert!(
            out.stdout.lines().any(|l| l == line),
            "missing {line:?} in\n{}",
            out.stdout
        );
    }

    // Exact sizes.
    let out = env.run(&["stats", "-e"]);
    out.assert_ok("stats -e");
    assert!(
        out.stdout.lines().any(|l| l.starts_with("Total size: ")),
        "{}",
        out.stdout
    );
}

#[test]
fn config_prints_the_defaults_with_comments() {
    let Some(env) = corpus() else { return };
    let out = env.run(&["config", "-d"]);
    out.assert_ok("config -d");
    let text = &out.stdout;
    assert!(
        text.contains("# --------------- Main ---------------"),
        "{text}"
    );
    assert!(text.contains("# --------------- Plugins ---------------"));
    assert!(text.contains(&format!(
        "directory: {}",
        env.tmp.path().join("lib").display()
    )));
    assert!(text.contains("statefile: state.pickle"));
    assert!(text.contains("plugins: []"));
    assert!(text.contains("sort_case_insensitive: yes"), "a default key");
    assert!(text.contains("library: "));

    // Without -d: the user's keys and the enabled plugins' defaults only,
    // starting with the command line's config list as beets prints it.
    let out = env.run(&["config"]);
    out.assert_ok("config");
    assert!(out.stdout.starts_with("configs: []\n"), "{}", out.stdout);
    assert!(out.stdout.contains(&format!(
        "directory: {}",
        env.tmp.path().join("lib").display()
    )));
    assert!(!out.stdout.contains("strong_rec_thresh"), "no -d defaults");
}

#[test]
fn library_override_uses_another_database() {
    let Some(env) = corpus() else { return };
    assert_eq!(env.ls(&["-f", "$title"]).len(), 3);
    let other = env.tmp.path().join("other-library.db");
    let other_arg = other.to_str().unwrap().to_string();

    // A fresh database, empty.
    let out = env.run(&["-l", &other_arg, "ls"]);
    out.assert_ok("ls -l other");
    assert_eq!(out.stdout_clean(), Vec::<&str>::new());

    // Import a different album into it; the main library is untouched.
    let src2 = env.tmp.path().join("src2");
    make_mp3(
        &src2.join("Other/Else/01 Only.mp3"),
        "Other",
        "Else",
        "Only",
        1,
        "2020",
    );
    let out = env.run(&[
        "-l",
        &other_arg,
        "import",
        "-A",
        "-q",
        src2.to_str().unwrap(),
    ]);
    out.assert_ok("import -l other");
    let out = env.run(&["--library", &other_arg, "ls", "-f", "$title"]);
    out.assert_ok("ls --library other");
    assert_eq!(out.stdout, "Only\n");
    assert_eq!(env.ls(&["-f", "$title"]).len(), 3, "main library unchanged");
}

#[test]
fn modify_albums_inherits_to_items_unless_noinherit() {
    let Some(env) = corpus() else { return };
    let out = env.run(&["modify", "-y", "-a", "album=Renamed", "album:Album A"]);
    out.assert_ok("modify -a");
    assert!(
        out.stdout.starts_with("Modifying 1 albums.\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("  album: Album A -> Renamed\n"),
        "{}",
        out.stdout
    );
    assert_eq!(
        env.ls(&["-f", "$album", "album:Renamed"]),
        ["Renamed", "Renamed"],
        "the items inherit the album change"
    );
    assert_eq!(env.ls(&["-a", "-f", "$album"]), ["Renamed", "Album B"]);

    // -I keeps the change on the album only.
    let out = env.run(&["modify", "-y", "-a", "-I", "year=1999", "album:Album B"]);
    out.assert_ok("modify -a -I");
    assert_eq!(
        env.ls(&["-a", "-f", "$album|$year", "album:Album B"]),
        ["Album B|1999"]
    );
    assert_eq!(
        env.ls(&["-f", "$year", "album:Album B"]),
        ["2010"],
        "the item keeps its year"
    );
}

#[test]
fn write_puts_database_tags_into_files() {
    let Some(env) = Env::with_plugins("info") else {
        return;
    };
    env.add_album("Artist", "Album", "2000", &["Alpha"]);
    env.import();
    let path = env.lib.join("Artist/Album/01 Alpha.mp3");
    assert!(path.exists());

    env.run(&["modify", "-y", "-M", "title:Alpha", "title=Omega"])
        .assert_ok("modify");
    // `info` reads the file, not the database.
    let out = env.run(&["info", "-i", "title", path.to_str().unwrap()]);
    out.assert_ok("info before write");
    assert!(out.stdout.contains("title: Alpha\n"), "{}", out.stdout);

    let out = env.run(&["write"]);
    out.assert_ok("write");
    assert!(
        out.stdout.contains("  title: Alpha -> Omega\n"),
        "{}",
        out.stdout
    );
    let out = env.run(&["info", "-i", "title", path.to_str().unwrap()]);
    out.assert_ok("info after write");
    assert!(out.stdout.contains("title: Omega\n"), "{}", out.stdout);
}
