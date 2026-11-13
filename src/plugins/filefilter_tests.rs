//! Ported from beets' test/plugins/test_filefilter.py.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Album, Item};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

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

fn env(yaml_cfg: &str) -> (TempDir, Config, Library) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("plugins", serde_yaml::from_str("[filefilter]").unwrap());
    cfg.set("import.write", serde_yaml::Value::Bool(false));
    cfg.set("import.copy", serde_yaml::Value::Bool(false));
    cfg.set("import.resume", "no".into());
    cfg.set("import.incremental", serde_yaml::Value::Bool(false));
    cfg.set("import.duplicate_action", "keep".into());
    cfg.set("ui.color", serde_yaml::Value::Bool(false));
    apply_cfg(&mut cfg, yaml_cfg);
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib)
}

fn make_mp3(path: &Path, title: &str) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"])
        .args(["-metadata", "artist=Artist"])
        .args(["-metadata", "album=Album"])
        .args(["-metadata", &format!("title={title}")])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

fn import(cfg: &Config, lib: &Library, src: &Path, singletons: bool) {
    let plugins = crate::plugins::load(cfg).unwrap();
    let mut opts = crate::importer::ImportOptions::from_config(cfg);
    opts.autotag = false;
    opts.singletons = singletons;
    let mut imp = crate::importer::Importer::new(cfg, lib, opts, &plugins).unwrap();
    imp.run(&[src.to_path_buf()]).unwrap();
}

fn items(lib: &Library) -> Vec<Item> {
    lib.items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
        .unwrap()
}

fn albums(lib: &Library) -> Vec<Album> {
    lib.albums(&crate::query::Query::parse::<&str>(crate::model::Kind::Album, &[]).unwrap())
        .unwrap()
}

struct Corpus {
    _dir: TempDir,
    album_track: PathBuf,
    other_album_track: PathBuf,
    single_track: PathBuf,
    import_path: PathBuf,
}

fn corpus() -> Option<Corpus> {
    let dir = tempfile::tempdir().unwrap();
    let import_path = dir.path().join("src");
    let album_track = import_path.join("album/1.mp3");
    let other_album_track = import_path.join("other_album/1.mp3");
    let single_track = import_path.join("single.mp3");
    for (i, p) in [&album_track, &other_album_track, &single_track]
        .iter()
        .enumerate()
    {
        if !make_mp3(p, &format!("Title {i}")) {
            return None;
        }
    }
    Some(Corpus {
        _dir: dir,
        album_track,
        other_album_track,
        single_track,
        import_path,
    })
}

fn run_filter(c: &Corpus, yaml_cfg: &str, singletons: bool) -> (usize, Vec<PathBuf>) {
    let (_d, cfg, lib) = env(yaml_cfg);
    import(&cfg, &lib, &c.import_path, singletons);
    (
        albums(&lib).len(),
        items(&lib).iter().map(|i| i.path().to_path_buf()).collect(),
    )
}

fn check(c: &Corpus, yaml: &str, singletons: bool, albums_n: usize, paths: &[&PathBuf]) {
    let (n, got) = run_filter(c, yaml, singletons);
    assert_eq!(n, albums_n, "albums for {yaml}");
    let want: std::collections::HashSet<PathBuf> = paths.iter().map(|p| (*p).clone()).collect();
    let got: std::collections::HashSet<PathBuf> = got.into_iter().collect();
    assert_eq!(got, want, "items for {yaml}");
}

#[test]
fn import_default_imports_everything() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{}",
        false,
        3,
        &[&c.album_track, &c.other_album_track, &c.single_track],
    );
}

#[test]
fn import_nothing() {
    let Some(c) = corpus() else { return };
    check(&c, "{filefilter: {path: not_there}}", false, 0, &[]);
}

#[test]
fn global_config() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{filefilter: {path: '.*album.*'}}",
        false,
        2,
        &[&c.album_track, &c.other_album_track],
    );
}

#[test]
fn album_config() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{filefilter: {album_path: '.*other_album.*'}}",
        false,
        1,
        &[&c.other_album_track],
    );
}

#[test]
fn singleton_config_is_ignored_for_album_import() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{filefilter: {singleton_path: '.*other_album.*'}}",
        false,
        3,
        &[&c.album_track, &c.other_album_track, &c.single_track],
    );
}

#[test]
fn singleton_global_config() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{filefilter: {path: '.*album.*'}}",
        true,
        0,
        &[&c.album_track, &c.other_album_track],
    );
}

#[test]
fn singleton_album_config_is_ignored() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{filefilter: {album_path: '.*other_album.*'}}",
        true,
        0,
        &[&c.album_track, &c.other_album_track, &c.single_track],
    );
}

#[test]
fn singleton_singleton_config() {
    let Some(c) = corpus() else { return };
    check(
        &c,
        "{filefilter: {singleton_path: '.*other_album.*'}}",
        true,
        0,
        &[&c.other_album_track],
    );
}
