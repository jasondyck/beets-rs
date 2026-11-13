//! End-to-end: import -> ls -> modify -> mv -> write -> update -> rm.

mod common;

use common::Env;

fn corpus() -> Option<Env> {
    let env = Env::new()?;
    env.add_album("Artist One", "Album One", "2001", &["One", "Two"]);
    env.add_album("Artist Two", "Album Two", "2010", &["Three"]);
    Some(env)
}

#[test]
fn import_list_modify_move_write_update_remove() {
    let Some(env) = corpus() else { return };

    // Import copies the albums into the library and logs their source
    // directories, as beets does.
    let out = env.import();
    let mut imported: Vec<&str> = out.stderr_clean();
    imported.sort_unstable();
    let mut want = [
        env.src.join("Artist One/Album One"),
        env.src.join("Artist Two/Album Two"),
    ];
    want.sort();
    assert_eq!(
        imported,
        want.iter().map(|p| p.to_str().unwrap()).collect::<Vec<_>>()
    );

    let album1 = env.lib.join("Artist One/Album One");
    let album2 = env.lib.join("Artist Two/Album Two");
    assert!(album1.join("01 One.mp3").exists());
    assert!(album1.join("02 Two.mp3").exists());
    assert!(album2.join("01 Three.mp3").exists());
    assert_eq!(
        env.paths(),
        vec![
            album1.join("01 One.mp3"),
            album1.join("02 Two.mp3"),
            album2.join("01 Three.mp3"),
        ]
    );

    // Albums with a template, and album fields reachable from items.
    let albums = env.ls(&[
        "-a",
        "-f",
        "$albumartist|$album|$year|$albumtotal",
        "album:Album One",
    ]);
    assert_eq!(albums, ["Artist One|Album One|2001|0"]);
    let items = env.ls(&["-f", "$title|$album|$year|$track"]);
    assert_eq!(
        items,
        [
            "One|Album One|2001|01",
            "Two|Album One|2001|02",
            "Three|Album Two|2010|01"
        ]
    );

    // `modify -M` changes the database only: the path stays.
    let out = env.run(&["modify", "-y", "-M", "title:One", "title=Uno"]);
    out.assert_ok("modify");
    assert_eq!(
        out.stdout,
        "Modifying 1 items.\nArtist One - Album One - One\n  title: One -> Uno\n"
    );
    assert!(album1.join("01 One.mp3").exists(), "no move without -m");
    assert_eq!(env.ls(&["-f", "$title", "title:Uno"]), ["Uno"]);

    // `modify -m` moves the file to its new path.
    let out = env.run(&["modify", "-y", "-m", "title:Two", "title=Deux"]);
    out.assert_ok("modify -m");
    assert!(
        !album1.join("02 Two.mp3").exists(),
        "the old path is vacated"
    );
    let moved = album1.join("02 Deux.mp3");
    assert!(moved.exists(), "the file is at its new path");
    assert!(env.paths().contains(&moved));
    assert_eq!(
        env.ls(&["-f", "$title", "album:Album One"]),
        ["Uno", "Deux"]
    );

    // `mv` moves the item whose database title changed without `-m`, and
    // then everything is in place.
    let out = env.run(&["mv"]);
    out.assert_ok("mv");
    assert_eq!(out.stderr, "Moving 1 item (2 already in place).\n");
    assert!(album1.join("01 Uno.mp3").exists());
    assert!(!album1.join("01 One.mp3").exists());
    let out = env.run(&["mv"]);
    out.assert_ok("mv");
    assert_eq!(out.stderr, "Moving 0 items (3 already in place).\n");

    // `write` puts the database's tags into the files, listing the changes.
    let out = env.run(&["write"]);
    out.assert_ok("write");
    assert!(
        out.stdout.contains("  title: One -> Uno\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("  title: Two -> Deux\n"),
        "{}",
        out.stdout
    );

    // A file changed behind the library's back: `update` re-reads it and
    // applies the new tag.
    let three = album2.join("01 Three.mp3");
    common::make_mp3(&three, "Artist Two", "Album Two", "Tres", 1, "2010");
    common::touch_newer(&three);
    let out = env.run(&["update"]);
    out.assert_ok("update");
    assert!(
        out.stdout.contains("  title: Three -> Tres\n"),
        "{}",
        out.stdout
    );
    assert_eq!(env.ls(&["-f", "$title", "title:Tres"]), ["Tres"]);

    // `rm` drops the item but keeps the file.
    let uno = album1.join("01 Uno.mp3");
    let out = env.run(&["rm", "-f", "title:Uno"]);
    out.assert_ok("rm");
    assert_eq!(env.ls(&["-f", "$title"]), ["Deux", "Tres"]);
    assert!(env.ls(&["-p", "title:Uno"]).is_empty());
    assert!(uno.exists(), "rm without -d keeps the file");

    // `rm -d -a` deletes the album's files and prunes its directory.
    let out = env.run(&["rm", "-d", "-f", "-a", "album:Album Two"]);
    out.assert_ok("rm -d");
    assert!(!three.exists(), "the file is deleted");
    assert!(!album2.exists(), "the emptied album directory is pruned");
    assert_eq!(env.ls(&["-f", "$title"]), ["Deux"]);
}

#[test]
fn rm_d_singleton_deletes_file_and_leaves_other_albums() {
    let Some(env) = Env::new() else { return };
    env.add_album("Artist", "Album", "2000", &["Keep", "Drop"]);
    env.import();
    let drop = env.lib.join("Artist/Album/02 Drop.mp3");
    let keep = env.lib.join("Artist/Album/01 Keep.mp3");
    assert!(drop.exists());

    env.run(&["rm", "-d", "-f", "title:Drop"])
        .assert_ok("rm -d");
    assert!(!drop.exists());
    assert!(keep.exists(), "the other item's file stays");
    assert!(env.lib.join("Artist/Album").exists());
    assert_eq!(env.ls(&["-f", "$title"]), ["Keep"]);
}
