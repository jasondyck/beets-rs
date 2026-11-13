//! Ported from beets' test/plugins/test_permissions.py.

use super::*;
use crate::config::Config;
use crate::library::Library;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn yaml(v: &str) -> serde_yaml::Value {
    serde_yaml::from_str(v).unwrap()
}

/// Set `yaml_cfg`'s keys, merging mappings into the defaults (as beets'
/// layered config does) rather than replacing whole plugin sections.
fn apply_cfg(cfg: &mut Config, yaml_cfg: &str) {
    fn apply(cfg: &mut Config, prefix: &str, v: &serde_yaml::Value) {
        let is_map = cfg.get(prefix).is_some_and(serde_yaml::Value::is_mapping);
        if let (true, serde_yaml::Value::Mapping(m)) = (is_map, v) {
            for (k, val) in m {
                apply(cfg, &format!("{prefix}.{}", k.as_str().unwrap()), val);
            }
        } else {
            cfg.set(prefix, v.clone());
        }
    }
    if let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(yaml_cfg) {
        for (k, v) in m {
            apply(cfg, k.as_str().unwrap(), &v);
        }
    }
}

fn env(plugins: &str, yaml_cfg: &str) -> (TempDir, Config, Library, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("plugins", yaml(plugins));
    cfg.set("import.write", serde_yaml::Value::Bool(false));
    cfg.set("import.resume", "no".into());
    cfg.set("import.incremental", serde_yaml::Value::Bool(false));
    cfg.set("import.duplicate_action", "keep".into());
    cfg.set("ui.color", serde_yaml::Value::Bool(false));
    apply_cfg(&mut cfg, yaml_cfg);
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib, libdir)
}

fn make_mp3(dir: &Path, name: &str, tags: &[(&str, &str)]) -> bool {
    std::fs::create_dir_all(dir).unwrap();
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"]);
    for (k, v) in tags {
        cmd.args(["-metadata", &format!("{k}={v}")]);
    }
    cmd.arg(dir.join(name)).status().is_ok_and(|s| s.success())
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn import(cfg: &Config, lib: &Library, src: &Path) {
    let plugins = crate::plugins::load(cfg).unwrap();
    let mut opts = crate::importer::ImportOptions::from_config(cfg);
    opts.autotag = false;
    let mut imp = crate::importer::Importer::new(cfg, lib, opts, &plugins).unwrap();
    imp.run(&[src.to_path_buf()]).unwrap();
}

fn all_items(lib: &Library) -> Vec<crate::model::Item> {
    lib.items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
        .unwrap()
}

#[test]
fn dirs_in_library_cases() {
    let t = tempfile::tempdir().unwrap();
    let lib = t.path().join("library");
    let album = lib.join("album");
    let track = album.join("sibling.mp3");
    assert_eq!(
        dirs_in_library(&lib, &track),
        vec![album.clone()],
        "only the album directory, not the library"
    );
    // A sibling library that starts with the same prefix is ignored.
    let sibling = t.path().join("library1");
    let track = sibling.join("album1/sibling.mp3");
    assert!(dirs_in_library(&lib, &track).is_empty());
    // A path outside the library has no directories inside it.
    assert!(dirs_in_library(&lib, &t.path().join("elsewhere/x.mp3")).is_empty());
}

#[test]
fn convert_perm_from_string_and_int() {
    assert_eq!(convert_perm(Some(&yaml("'10'"))), Some(8));
    assert_eq!(convert_perm(Some(&yaml("10"))), Some(8));
    assert_eq!(convert_perm(Some(&yaml("'644'"))), Some(0o644));
    assert_eq!(convert_perm(Some(&yaml("755"))), Some(0o755));
}

#[test]
fn permissions_on_album_imported() {
    let (d, cfg, lib, _libdir) = env("[permissions]", "{permissions: {file: '777', dir: '777'}}");
    let src = d.path().join("src/album");
    if !make_mp3(
        &src,
        "track_1.mp3",
        &[("artist", "Artist"), ("album", "Album"), ("title", "One")],
    ) {
        return;
    }
    assert_ne!(mode(&src.join("track_1.mp3")), 0o777);
    import(&cfg, &lib, &src);
    let item = &all_items(&lib)[0];
    let mut paths = vec![item.path().to_path_buf()];
    paths.extend(dirs_in_library(&cfg.directory(), item.path()));
    assert!(paths.len() >= 2);
    for p in paths {
        assert_eq!(mode(&p), 0o777, "{}", p.display());
    }
}

#[test]
fn permissions_on_item_imported() {
    let (d, cfg, lib, _libdir) = env(
        "[permissions]",
        "{permissions: {file: '777', dir: '777'}, import: {singletons: yes}}",
    );
    let src = d.path().join("src");
    if !make_mp3(
        &src,
        "track_1.mp3",
        &[("artist", "Artist"), ("title", "One")],
    ) {
        return;
    }
    import(&cfg, &lib, &src);
    let item = &all_items(&lib)[0];
    let mut paths = vec![item.path().to_path_buf()];
    paths.extend(dirs_in_library(&cfg.directory(), item.path()));
    for p in paths {
        assert_eq!(mode(&p), 0o777, "{}", p.display());
    }
}

#[test]
fn permissions_on_set_art() {
    let (d, cfg, lib, _libdir) = env("[permissions]", "{permissions: {file: '777', dir: '777'}}");
    let src = d.path().join("src/album");
    if !make_mp3(
        &src,
        "track_1.mp3",
        &[("artist", "Artist"), ("album", "Album"), ("title", "One")],
    ) {
        return;
    }
    import(&cfg, &lib, &src);
    let album = &lib
        .albums(&crate::query::Query::parse::<&str>(crate::model::Kind::Album, &[]).unwrap())
        .unwrap()[0];
    let cover = d.path().join("cover.jpg");
    std::fs::write(&cover, b"\xff\xd8\xff\xe0not really a jpeg").unwrap();
    let plugins = crate::plugins::load(&cfg).unwrap();
    let ctx = crate::plugins::PluginCtx {
        cfg: &cfg,
        lib: &lib,
        write: false,
    };
    let mut album = album.clone();
    crate::plugins::art::set_art(&ctx, &mut album, &cover, true).unwrap();
    assert!(super::check(album.path_field("artpath"), 0o777));
    drop(plugins);
}
