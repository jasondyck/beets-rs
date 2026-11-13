//! Ported from beets' `test/plugins/test_importadded.py`, with as-is imports
//! (the plugin doesn't depend on autotagging).

use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

/// Config + library in a temp dir; `plugins` is the plugin list as YAML.
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

fn env(plugins: &str, yaml: &str) -> (TempDir, Config, Library, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("plugins", serde_yaml::from_str(plugins).unwrap());
    cfg.set("import.write", serde_yaml::Value::Bool(false));
    cfg.set("import.resume", "no".into());
    cfg.set("import.incremental", serde_yaml::Value::Bool(false));
    cfg.set("import.duplicate_action", "keep".into());
    cfg.set("ui.color", serde_yaml::Value::Bool(false));
    apply_cfg(&mut cfg, yaml);
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib, libdir)
}

/// A 1-second MP3, or None without ffmpeg.
fn make_mp3(path: &Path, tags: &[(&str, &str)]) -> bool {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"]);
    for (k, v) in tags {
        cmd.args(["-metadata", &format!("{k}={v}")]);
    }
    cmd.arg(path).status().is_ok_and(|s| s.success())
}

fn make_album(src: &Path, album: &str, titles: &[&str]) -> bool {
    titles.iter().enumerate().all(|(n, title)| {
        let track = (n + 1).to_string();
        make_mp3(
            &src.join(album).join(format!("{track}.mp3")),
            &[
                ("artist", "Artist"),
                ("album", album),
                ("title", title),
                ("track", &track),
            ],
        )
    })
}

fn set_mtime(path: &Path, t: SystemTime) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(t))
        .unwrap();
}

fn mtime(path: &Path) -> f64 {
    super::st_mtime(path).unwrap()
}

fn import(cfg: &Config, lib: &Library, src: &Path) -> crate::importer::Stats {
    let plugins = crate::plugins::load(cfg).unwrap();
    let mut opts = crate::importer::ImportOptions::from_config(cfg);
    opts.autotag = false;
    let mut imp = crate::importer::Importer::new(cfg, lib, opts, &plugins).unwrap();
    imp.run(&[src.to_path_buf()]).unwrap();
    imp.stats
}

fn all_items(lib: &Library) -> Vec<Item> {
    lib.items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
        .unwrap()
}

fn all_albums(lib: &Library) -> Vec<crate::model::Album> {
    lib.albums(&crate::query::Query::parse::<&str>(crate::model::Kind::Album, &[]).unwrap())
        .unwrap()
}

/// Different mtimes on the files to be imported, as the upstream test does:
/// returns the earliest.
fn modify_mtimes(paths: &[PathBuf], offset: i64) -> f64 {
    for (i, p) in paths.iter().enumerate() {
        let m = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(mtime(p));
        let delta = Duration::from_secs((offset.unsigned_abs()) * (i as u64 + 1));
        let t = if offset >= 0 { m + delta } else { m - delta };
        set_mtime(p, t);
    }
    let paths_and_mtimes: Vec<f64> = paths.iter().map(|p| mtime(p)).collect();
    paths_and_mtimes.into_iter().reduce(f64::min).unwrap_or(0.0)
}

fn album_paths(src: &Path, album: &str, n: usize) -> Vec<PathBuf> {
    (1..=n)
        .map(|i| src.join(album).join(format!("{i}.mp3")))
        .collect()
}

#[test]
fn import_album_with_added_dates() {
    let (_d, cfg, lib, _libdir) = env("[importadded]", "{}");
    let src = _d.path().join("src");
    if !make_album(&src, "Album", &["One", "Two"]) {
        return;
    }
    let min = modify_mtimes(&album_paths(&src, "Album", 2), -60_000);
    import(&cfg, &lib, &src.join("Album"));
    let album = &all_albums(&lib)[0];
    assert_eq!(album.float("added"), min);
    for item in all_items(&lib) {
        assert_eq!(item.float("added"), min);
    }
}

#[test]
fn import_album_inplace_with_added_dates() {
    let (_d, mut cfg, lib, _libdir) = env("[importadded]", "{}");
    let src = _d.path().join("src");
    if !make_album(&src, "Album", &["One", "Two"]) {
        return;
    }
    cfg.set("import.copy", serde_yaml::Value::Bool(false));
    let min = modify_mtimes(&album_paths(&src, "Album", 2), -60_000);
    let mut opts = crate::importer::ImportOptions::from_config(&cfg);
    opts.autotag = false;
    assert_eq!(opts.op, crate::fileops::FileOp::None);
    let plugins = crate::plugins::load(&cfg).unwrap();
    let mut imp = crate::importer::Importer::new(&cfg, &lib, opts, &plugins).unwrap();
    imp.run(&[src.join("Album")]).unwrap();
    let album = &all_albums(&lib)[0];
    assert_eq!(album.float("added"), min);
    for item in all_items(&lib) {
        assert_eq!(item.float("added"), min);
    }
}

#[test]
fn import_album_with_preserved_mtimes() {
    let (_d, cfg, lib, _libdir) = env(
        "[importadded]",
        "{import: {copy: yes}, importadded: {preserve_mtimes: yes}}",
    );
    let src = _d.path().join("src");
    if !make_album(&src, "Album", &["One", "Two"]) {
        return;
    }
    let paths = album_paths(&src, "Album", 2);
    let min = modify_mtimes(&paths, -60_000);
    let source_mtimes: Vec<f64> = paths.iter().map(|p| mtime(p)).collect();
    import(&cfg, &lib, &src.join("Album"));
    let album = &all_albums(&lib)[0];
    assert!((album.float("added") - min).abs() < 1e-3);
    for (item, src_mtime) in all_items(&lib).iter().zip(&source_mtimes) {
        assert!((item.float("added") - min).abs() < 1e-3);
        // beets stores the item's mtime as an int, so compare relatively
        // (the upstream test uses `approx(rel=1e-4)`).
        let m = item.float("mtime");
        assert!(
            (m - src_mtime).abs() <= src_mtime.abs() * 1e-4,
            "{m} vs {src_mtime}"
        );
        assert!((mtime(item.path()) - src_mtime).abs() < 1e-3);
    }
}

#[test]
fn reimported_album_skipped() {
    let (_d, mut cfg, lib, libdir) = env("[importadded]", "{}");
    let src = _d.path().join("src");
    if !make_album(&src, "Album", &["One", "Two"]) {
        return;
    }
    modify_mtimes(&album_paths(&src, "Album", 2), -60_000);
    import(&cfg, &lib, &src.join("Album"));
    let album_added = all_albums(&lib)[0].float("added");
    let before: Vec<(PathBuf, f64)> = all_items(&lib)
        .iter()
        .map(|i| (i.path().to_path_buf(), i.float("added")))
        .collect();
    // Newer mtimes on the library files, as if beets had modified them.
    let paths: Vec<PathBuf> = before.iter().map(|(p, _)| p.clone()).collect();
    modify_mtimes(&paths, 10_000);
    // Reimport the library directory in place.
    cfg.set("import.copy", serde_yaml::Value::Bool(false));
    let plugins = crate::plugins::load(&cfg).unwrap();
    let mut opts = crate::importer::ImportOptions::from_config(&cfg);
    opts.autotag = false;
    let mut imp = crate::importer::Importer::new(&cfg, &lib, opts, &plugins).unwrap();
    imp.run(std::slice::from_ref(&libdir)).unwrap();
    let album = &all_albums(&lib)[0];
    assert!((album.float("added") - album_added).abs() < 1e-3);
    for (path, added) in before {
        let now = all_items(&lib)
            .into_iter()
            .find(|i| i.path() == path)
            .unwrap()
            .float("added");
        assert!((now - added).abs() < 1e-3, "{}", path.display());
    }
}

#[test]
fn singletons_with_added_dates() {
    let (_d, cfg, lib, _libdir) = env("[importadded]", "{import: {copy: yes, singletons: yes}}");
    let src = _d.path().join("src");
    if !make_mp3(
        &src.join("one.mp3"),
        &[("artist", "Artist"), ("title", "One")],
    ) {
        return;
    }
    let source_mtime = mtime(&src.join("one.mp3"));
    import(&cfg, &lib, &src);
    let items = all_items(&lib);
    assert_eq!(items.len(), 1);
    assert!((items[0].float("added") - source_mtime).abs() < 1e-3);
}

#[test]
fn reimported_singletons_skipped() {
    let (_d, cfg, lib, _libdir) = env("[importadded]", "{import: {copy: yes, singletons: yes}}");
    let src = _d.path().join("src");
    if !make_mp3(
        &src.join("one.mp3"),
        &[("artist", "Artist"), ("title", "One")],
    ) {
        return;
    }
    import(&cfg, &lib, &src);
    let before: Vec<(PathBuf, f64)> = all_items(&lib)
        .iter()
        .map(|i| (i.path().to_path_buf(), i.float("added")))
        .collect();
    let paths: Vec<PathBuf> = before.iter().map(|(p, _)| p.clone()).collect();
    modify_mtimes(&paths, 10_000);
    let libdir = cfg.directory();
    import(&cfg, &lib, &libdir);
    for (path, added) in before {
        let now = all_items(&lib)
            .into_iter()
            .find(|i| i.path() == path)
            .unwrap()
            .float("added");
        assert!((now - added).abs() < 1e-3);
    }
}
