//! Tests for the `ipfs` plugin: a fake `ipfs` executable stands in for the
//! daemon (beets' tests mock `util.command_output`).

use super::*;
use crate::config::Config;
use crate::model::Item;
use std::os::unix::fs::PermissionsExt;

fn fake_ipfs(dir: &Path) -> PathBuf {
    let p = dir.join("ipfs");
    std::fs::write(
        &p,
        "#!/bin/sh\nif [ \"$1\" = add ]; then\n  echo QmItemOne\n  echo QmItemTwo\n  echo QmAlbumHash\nfi\nif [ \"$1\" = get ]; then\n  mkdir -p \"$2\"\n  echo audio > \"$2/download.mp3\"\nfi\n",
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn env() -> (tempfile::TempDir, Config, Library) {
    let d = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = d.path().join("cfg");
    cfg.set("directory", d.path().join("lib").to_string_lossy().into());
    cfg.set(
        "library",
        d.path().join("library.db").to_string_lossy().into(),
    );
    cfg.set("plugins", serde_yaml::from_str("[ipfs]").unwrap());
    let lib = Library::open_with(&cfg.library_path(), Some(&d.path().join("lib")), false).unwrap();
    (d, cfg, lib)
}

fn album_with(lib: &Library, d: &Path) -> Album {
    let mut items: Vec<Item> = ["01.mp3", "02.mp3"]
        .iter()
        .map(|n| {
            let mut i = Item::new_item();
            i.set_path(d.join(n));
            i.set_str("title", *n);
            i.set_str("artist", "A");
            i.set_str("album", "Al");
            i
        })
        .collect();
    lib.add_album(&mut items).unwrap()
}

#[test]
fn add_stores_item_and_album_hashes() {
    let (d, cfg, lib) = env();
    let album_dir = d.path().join("orig");
    std::fs::create_dir_all(&album_dir).unwrap();
    let mut album = album_with(&lib, &album_dir);
    set_test_binary(Some(fake_ipfs(d.path())));
    assert!(ipfs_add(&cfg, &lib, &mut album).unwrap());
    set_test_binary(None);
    assert_eq!(album.str("ipfs"), "QmAlbumHash");
    let items = lib.album_items(album.id.unwrap()).unwrap();
    assert_eq!(items[0].str("ipfs"), "QmItemOne");
    assert_eq!(items[1].str("ipfs"), "QmItemTwo");
    // Already added: no second run.
    set_test_binary(Some(d.path().join("does-not-exist")));
    assert!(!ipfs_add(&cfg, &lib, &mut album).unwrap());
    set_test_binary(None);
}

#[test]
fn added_albums_get_ipfs_paths() {
    let (d, _cfg, lib) = env();
    let album_dir = d.path().join("orig");
    std::fs::create_dir_all(&album_dir).unwrap();
    let mut album = album_with(&lib, &album_dir);
    album.set_str("ipfs", "QmAlbumHash");
    lib.store(&album).unwrap();
    for mut item in lib.album_items(album.id.unwrap()).unwrap() {
        item.set_str("ipfs", "QmItem");
        lib.store(&item).unwrap();
    }
    let tmp = d.path().join("published.db");
    let tmplib = ipfs_added_albums(&lib, &tmp).unwrap();
    let albums = tmplib
        .albums(&Query::parse::<&str>(Kind::Album, &[]).unwrap())
        .unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].str("ipfs"), "QmAlbumHash");
    let items = tmplib.album_items(albums[0].id.unwrap()).unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert_eq!(
            item.path(),
            Path::new(&format!(
                "/ipfs/QmAlbumHash/{}",
                item.path().file_name().unwrap().to_string_lossy()
            ))
        );
    }
    drop(tmplib);
}

#[test]
fn list_reads_the_joined_remote_library() {
    let (d, cfg, lib) = env();
    let remotes = d.path().join("remotes");
    std::fs::create_dir_all(&remotes).unwrap();
    {
        let rlib = Library::open_with(&remotes.join("joined.db"), None, false).unwrap();
        let mut item = Item::new_item();
        item.set_path("/ipfs/QmRemote/01.mp3");
        item.set_str("album", "Remote Album");
        item.set_str("albumartist", "Remote Artist");
        let mut album = rlib.add_album(std::slice::from_mut(&mut item)).unwrap();
        album.set_str("ipfs", "QmRemote");
        rlib.store(&album).unwrap();
    }
    ipfs_list(&cfg, &lib, &[], None).unwrap();
}

#[test]
fn play_plans_the_remote_albums_through_the_play_plugin() {
    let (d, mut cfg, _lib) = env();
    let remotes = d.path().join("remotes");
    std::fs::create_dir_all(&remotes).unwrap();
    {
        let rlib = Library::open_with(&remotes.join("joined.db"), None, false).unwrap();
        let mut item = Item::new_item();
        item.set_path("/ipfs/QmRemote/01.mp3");
        item.set_str("album", "Remote Album");
        item.set_str("albumartist", "Remote Artist");
        let mut album = rlib.add_album(std::slice::from_mut(&mut item)).unwrap();
        album.set_str("ipfs", "QmRemote");
        rlib.store(&album).unwrap();
    }
    cfg.set("plugins", serde_yaml::from_str("[ipfs, play]").unwrap());
    cfg.set("play.command", "echo".into());
    cfg.set("play.raw", serde_yaml::Value::Bool(true));
    // Relative paths would hide the /ipfs/... paths; ipfs_play unsets this.
    cfg.set(
        "play.relative_to",
        serde_yaml::Value::String(d.path().to_string_lossy().into_owned()),
    );
    let mut out = vec![];
    let plan = ipfs_plan(
        &cfg,
        &CmdOpts {
            play: true,
            ..CmdOpts::default()
        },
        &mut out,
        &mut || Some('c'),
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.command, "echo");
    assert_eq!(plan.item_type, "album");
    assert_eq!(plan.count, 1);
    assert_eq!(
        plan.open_args,
        vec![PathBuf::from("/ipfs/QmRemote/01.mp3")]
    );
}

#[test]
fn publish_adds_the_temporary_database() {
    let (d, cfg, lib) = env();
    let album_dir = d.path().join("orig");
    std::fs::create_dir_all(&album_dir).unwrap();
    let mut album = album_with(&lib, &album_dir);
    album.set_str("ipfs", "QmAlbumHash");
    lib.store(&album).unwrap();
    for mut item in lib.album_items(album.id.unwrap()).unwrap() {
        item.set_str("ipfs", "QmItem");
        lib.store(&item).unwrap();
    }
    set_test_binary(Some(fake_ipfs(d.path())));
    ipfs_publish(&cfg, &lib).unwrap();
    set_test_binary(None);
}

#[test]
fn a_missing_ipfs_binary_is_logged() {
    let (d, cfg, lib) = env();
    let album_dir = d.path().join("orig");
    std::fs::create_dir_all(&album_dir).unwrap();
    let mut album = album_with(&lib, &album_dir);
    set_test_binary(Some(d.path().join("no-such-ipfs")));
    let (_r, lines) = crate::logging::capture(|| {
        assert!(!ipfs_add(&cfg, &lib, &mut album).unwrap());
    });
    set_test_binary(None);
    assert!(
        lines.iter().any(|l| l.contains("Failed to add")),
        "{lines:?}"
    );
}
