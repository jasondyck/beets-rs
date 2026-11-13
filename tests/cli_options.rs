//! Unknown options as beets' optparse reports them, `--`, and long-option
//! abbreviations (2.9).

mod common;

use common::Env;

fn list_usage() -> &'static str {
    "Usage: beet [options]\nExample: beet list -f '$album: $title' artist:beatles"
}

#[test]
fn unknown_options_use_beets_messages_and_exit_2() {
    let Some(env) = Env::new() else { return };
    let cases: &[(&[&str], &str, &str)] = &[
        (&["rm", "-y", "foo"], "beet remove", "-y"),
        (&["ls", "-y"], "beet list", "-y"),
        (&["ls", "-aQ"], "beet list", "-Q"),
        (&["ls", "-Qa"], "beet list", "-Q"),
        // optparse walks a short cluster left to right: `-a` is known, `-r`
        // is not.
        (&["ls", "-artist:x"], "beet list", "-r"),
        (&["ls", "-1"], "beet list", "-1"),
        (&["ls", "---x"], "beet list", "---x"),
        (&["ls", "--bogus"], "beet list", "--bogus"),
        (&["ls", "--bogus=x"], "beet list", "--bogus"),
        (&["ls", "--formatt"], "beet list", "--formatt"),
        (&["mv", "-z", "x"], "beet move", "-z"),
        (&["modify", "-y", "foo=1", "-q"], "beet modify", "-q"),
        (&["stats", "-Q"], "beet stats", "-Q"),
        (&["config", "-Q"], "beet config", "-Q"),
        (&["fields", "--bogus"], "beet fields", "--bogus"),
    ];
    for (args, prog, opt) in cases {
        let out = env.run(args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`beet {}` should exit 2:\n{}",
            args.join(" "),
            out.stderr
        );
        assert_eq!(out.stdout, "", "`beet {}` stdout", args.join(" "));
        let usage = if *prog == "beet list" {
            list_usage().to_string()
        } else {
            format!("Usage: beet {} [options]", prog.trim_start_matches("beet "))
        };
        assert_eq!(
            out.stderr,
            format!("{usage}\n\n{prog}: error: no such option: {opt}\n"),
            "`beet {}`",
            args.join(" ")
        );
    }

    // A missing value is optparse's other option error.
    for (args, opt) in [
        (["ls", "-f"], "-f"),
        (["ls", "--form"], "--format"),
        (["rm", "--limit"], "--limit"),
    ] {
        let out = env.run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            out.stderr.ends_with(&format!(
                "beet list: error: {opt} option requires 1 argument\n"
            )) || out.stderr.ends_with(&format!(
                "beet remove: error: {opt} option requires 1 argument\n"
            )),
            "{}",
            out.stderr
        );
    }

    // Ambiguous long abbreviations list the candidates.
    let out = env.run(&["ls", "--="]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stderr.ends_with(
            "beet list: error: ambiguous option: -- \
             (--album, --format, --help, --limit, --path?)\n"
        ),
        "{}",
        out.stderr
    );

    // Unknown global options use the root usage.
    let out = env.run(&["-Q", "ls"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        out.stderr,
        "Usage: \n  beet COMMAND [ARGS...]\n  beet help COMMAND\n\n\
         beet: error: no such option: -Q\n"
    );

    // Unknown commands keep beets' exit 1.
    let out = env.run(&["frob", "-y"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stderr, "error: unknown command 'frob'\n");
}

#[test]
fn dashes_queries_and_abbreviations_still_work() {
    let Some(env) = Env::new() else { return };
    // After `--`, anything goes (including things that look like options).
    for args in [
        &["ls", "--", "-artist:x"][..],
        &["ls", "--", "-1"],
        &["ls", "--", "--bogus"],
        &["ls", "--", "-a", "-f", "$title"],
        &["--", "ls", "-a"],
        &["ls", "-f", "-a"],
        &["ls", "-f", ""],
        &["ls", "--a"],
        &["ls", "--li", "0"],
        &["ls", "-ap"],
    ] {
        let out = env.run(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "`beet {}` should be accepted:\n{}",
            args.join(" "),
            out.stderr
        );
    }

    // Long-option abbreviations expand (`--a` is `--album`, `--li` is
    // `--limit`) and `-l/--limit` works for list.
    env.add_album("Artist", "Album", "2000", &["One", "Two"]);
    env.import();
    assert_eq!(env.ls(&["-l", "1"]).len(), 1);
    assert_eq!(env.ls(&["--lim", "1"]).len(), 1);
    assert_eq!(env.ls(&["-a", "-f", "$album"]), ["Album"]);
    assert_eq!(env.ls(&["--a", "-f", "$album"]), ["Album"]);
}

#[test]
fn plugin_commands_get_the_same_option_checks() {
    let Some(env) = Env::new() else { return };
    // A command from a disabled plugin is unknown to beets; when the plugin
    // is enabled, its options are checked like the core ones.
    let out = env.run(&["-p", "fetchart", "fetchart", "-y"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        out.stderr,
        "Usage: beet fetchart [options]\n\n\
         beet fetchart: error: no such option: -y\n"
    );
    let out = env.run(&["-p", "fetchart,info", "info", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        out.stderr,
        "Usage: beet info [options]\n\n\
         beet info: error: no such option: --bogus\n"
    );
    // A valid plugin command still runs.
    let out = env.run(&["-p", "info", "info", "-i", "title"]);
    out.assert_ok("info");
}
