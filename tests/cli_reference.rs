//! The same steps run with reference beets (when the approved wrapper is
//! installed) and compared stream by stream, with each tool's temp root
//! replaced by `<ROOT>`.

mod common;

use common::{Env, Out, RefEnv};
use std::path::Path;

fn streams(out: &Out, root: &Path) -> (String, String) {
    (
        common::normalise(&out.stdout, root),
        common::normalise(&out.stderr, root),
    )
}

/// Compare one command's streams and exit status.
fn compare(env: &Env, refenv: &RefEnv, args: &[&str], ours: &Out, theirs: &Out) {
    let (our_out, our_err) = streams(ours, env.tmp.path());
    let (ref_out, ref_err) = streams(theirs, refenv.tmp.path());
    assert_eq!(our_out, ref_out, "stdout of `beet {}`", args.join(" "));
    assert_eq!(our_err, ref_err, "stderr of `beet {}`", args.join(" "));
    assert_eq!(
        ours.status.success(),
        theirs.status.success(),
        "exit status of `beet {}`",
        args.join(" ")
    );
}

/// Run `args` with both tools after comparing the outcome; returns both
/// outputs (for follow-up steps).
fn both(env: &Env, refenv: &RefEnv, args: &[&str]) -> (Out, Out) {
    let ours = env.run(args);
    let theirs = refenv.run(args);
    compare(env, refenv, args, &ours, &theirs);
    (ours, theirs)
}

/// Each tool imports its own corpus.
fn import_both(env: &Env, refenv: &RefEnv) {
    let ours = env.import();
    let theirs = refenv.import();
    compare(
        env,
        refenv,
        &["import", "-A", "-q", "<corpus>"],
        &ours,
        &theirs,
    );
}

fn corpora() -> Option<(Env, RefEnv)> {
    let env = Env::new()?;
    let refenv = RefEnv::new("")?;
    for e in [&env.src, &refenv.src] {
        common::make_mp3(
            &e.join("Artist A/Album A/01 Alpha.mp3"),
            "Artist A",
            "Album A",
            "Alpha",
            1,
            "2001",
        );
        common::make_mp3(
            &e.join("Artist A/Album A/02 Beta.mp3"),
            "Artist A",
            "Album A",
            "Beta",
            2,
            "2001",
        );
        common::make_mp3(
            &e.join("Artist B/Album B/01 Gamma.mp3"),
            "Artist B",
            "Album B",
            "Gamma",
            1,
            "2010",
        );
    }
    Some((env, refenv))
}

#[test]
fn listings_queries_and_stats_match_reference_beets() {
    let Some((env, refenv)) = corpora() else {
        return;
    };
    import_both(&env, &refenv);

    for args in [
        vec!["ls", "-f", "$artist|$album|$year|$track|$title"],
        vec!["ls", "-a", "-f", "$albumartist|$album|$albumtotal|$year"],
        vec!["ls", "-p"],
        vec!["ls", "-f", "$title", "artist:Artist A"],
        vec!["ls", "-f", "$title", "title::^A"],
        vec!["ls", "-f", "$title", "year:2005.."],
        vec!["ls", "-f", "$title", "^year:2001"],
        vec!["ls", "-f", "$title", "year:2001,", "album:Album B"],
        vec!["ls", "-f", "$title", "title-"],
        vec!["ls", "-a", "-f", "$album", "year-"],
        vec!["stats"],
        vec!["stats", "year:2010"],
        vec!["stats", "-e"],
    ] {
        both(&env, &refenv, &args);
    }
}

#[test]
fn modify_write_config_and_config_defaults_match_reference_beets() {
    let Some((env, refenv)) = corpora() else {
        return;
    };
    import_both(&env, &refenv);

    both(
        &env,
        &refenv,
        &["modify", "-y", "-M", "title:Alpha", "title=Omega"],
    );
    both(&env, &refenv, &["ls", "-f", "$title"]);
    both(&env, &refenv, &["write"]);
    both(&env, &refenv, &["ls", "-p"]);
    both(&env, &refenv, &["config"]);
    both(&env, &refenv, &["config", "-d"]);
    both(&env, &refenv, &["rm", "-f", "title:Omega"]);
    both(&env, &refenv, &["ls", "-f", "$title"]);
}
