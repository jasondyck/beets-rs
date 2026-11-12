//! Ported from beets' test/plugins/test_scrub.py (the as-is import runs
//! the `import_task_files` hook), plus per-format scrubbing as Mutagen does
//! it.

use super::*;
use crate::mediafile::Got;
use serde_yaml::Value as Y;
use std::path::PathBuf;

/// An audio file made by ffmpeg with `extra` arguments (None without
/// ffmpeg).
fn make(dir: &Path, name: &str, extra: &[&str]) -> Option<PathBuf> {
    let p = dir.join(name);
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.3"]);
    cmd.args(extra);
    cmd.args([
        "-metadata",
        "artist=Tag Artist",
        "-metadata",
        "album=Tag Album",
        "-metadata",
        "title=Tag Title",
    ]);
    cmd.arg(&p);
    cmd.status().is_ok_and(|s| s.success()).then_some(p)
}

/// The same with a JPEG cover (MP3, FLAC, M4A).
fn make_with_art(dir: &Path, name: &str) -> Option<PathBuf> {
    let img = dir.join("cover.jpg");
    if !img.exists() {
        crate::artresizer::tests::write_image(
            &img,
            &crate::artresizer::tests::picture(16, 16, 1),
            90,
        );
    }
    let img = img.display().to_string();
    let mut args = vec!["-i", img.as_str(), "-map", "0", "-map", "1", "-c:v", "copy"];
    if !name.ends_with(".mp3") {
        args.extend(["-disposition:v", "attached_pic"]);
    }
    make(dir, name, &args)
}

fn tag(p: &Path, f: &str) -> Got {
    MediaFile::open(p).unwrap().get(f).unwrap()
}

fn artist(p: &Path) -> Option<String> {
    match tag(p, "artist") {
        Got::Str(s) => s,
        _ => None,
    }
}

fn images(p: &Path) -> usize {
    MediaFile::open(p).unwrap().pictures().len()
}

/// `run_asis_importer(write=...)` with the plugin: the imported items
/// (read from the files) go through `import_task_files`.
fn asis_import(auto: bool, write: bool) -> Option<(tempfile::TempDir, Vec<PathBuf>)> {
    let dir = tempfile::tempdir().unwrap();
    let p = make(dir.path(), "a.mp3", &[])?;
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[scrub]").unwrap());
    cfg.set("scrub.auto", Y::Bool(auto));
    let lib = Library::in_memory_at(dir.path()).unwrap();
    let mut item = crate::media::read(&p).unwrap();
    lib.add(&mut item).unwrap();
    let ctx = PluginCtx {
        cfg: &cfg,
        lib: &lib,
        write,
    };
    let mut items = vec![item];
    let mut task = ImportTask {
        album: None,
        items: &mut items,
        paths: &[],
        asis: true,
        op: crate::fileops::FileOp::None,
    };
    Scrub::new(&cfg).import_task_files(&ctx, &mut task).unwrap();
    Some((dir, vec![p]))
}

#[test]
fn tags_not_scrubbed() {
    let Some((_d, paths)) = asis_import(false, true) else {
        return;
    };
    for p in paths {
        assert_eq!(artist(&p).as_deref(), Some("Tag Artist"));
        assert_eq!(tag(&p, "album"), Got::Str(Some("Tag Album".into())));
    }
}

#[test]
fn tags_restored() {
    let Some((_d, paths)) = asis_import(true, true) else {
        return;
    };
    for p in paths {
        assert_eq!(artist(&p).as_deref(), Some("Tag Artist"));
        assert_eq!(tag(&p, "album"), Got::Str(Some("Tag Album".into())));
    }
}

#[test]
fn tags_not_scrubbed_when_nowrite() {
    let Some((_d, paths)) = asis_import(true, false) else {
        return;
    };
    for p in paths {
        assert_eq!(artist(&p).as_deref(), Some("Tag Artist"));
        assert_eq!(tag(&p, "album"), Got::Str(Some("Tag Album".into())));
    }
}

#[test]
fn scrub_removes_every_tag_but_flac_pictures_and_aiff_wav() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mp3) = make_with_art(dir.path(), "a.mp3") else {
        return;
    };
    let flac = make_with_art(dir.path(), "b.flac").unwrap();
    let m4a = make_with_art(dir.path(), "c.m4a").unwrap();
    let others: Vec<PathBuf> = ["x.ogg", "x.opus", "x.wv"]
        .iter()
        .map(|n| make(dir.path(), n, &[]).unwrap())
        .collect();
    // AIFF and WAV with tags (ID3 chunks) as beets writes them.
    let mut kept = vec![];
    for n in ["x.aiff", "x.wav"] {
        let p = make(dir.path(), n, &[]).unwrap();
        let mut it = crate::media::read(&p).unwrap();
        it.set_str("artist", "Tag Artist");
        crate::media::write_item(&mut it).unwrap();
        kept.push(p);
    }
    for p in [&mp3, &flac, &m4a] {
        assert_eq!(images(p), 1, "{}", p.display());
    }
    for p in [&mp3, &flac, &m4a].into_iter().chain(&others).chain(&kept) {
        assert_eq!(artist(p).as_deref(), Some("Tag Artist"), "{}", p.display());
        scrub(p).unwrap();
    }
    for p in [&mp3, &flac, &m4a].into_iter().chain(&others) {
        assert_eq!(artist(p), None, "{}", p.display());
        assert_eq!(tag(p, "title"), Got::Str(None), "{}", p.display());
    }
    assert_eq!(images(&mp3), 0);
    assert_eq!(images(&m4a), 0);
    // Mutagen's `FLAC.delete` keeps the PICTURE blocks.
    assert_eq!(images(&flac), 1);
    for p in &kept {
        assert_eq!(artist(p).as_deref(), Some("Tag Artist"), "{}", p.display());
    }
    // ID3v1 goes too.
    let raw = std::fs::read(&mp3).unwrap();
    assert!(raw.len() < 128 || &raw[raw.len() - 128..raw.len() - 125] != b"TAG");
}

#[test]
fn scrub_item_restores_tags_and_art() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mp3) = make_with_art(dir.path(), "a.mp3") else {
        return;
    };
    let mut item = crate::media::read(&mp3).unwrap();
    item.set_str("title", "Library Title");
    let log = PluginLog::new("scrub");
    scrub_item(&mut item, true, &log);
    assert_eq!(artist(&mp3).as_deref(), Some("Tag Artist"));
    assert_eq!(tag(&mp3, "title"), Got::Str(Some("Library Title".into())));
    assert_eq!(images(&mp3), 1);
    // Without restoring: nothing left.
    scrub_item(&mut item, false, &log);
    assert_eq!(artist(&mp3), None);
    assert_eq!(images(&mp3), 0);
}

#[test]
fn command_with_query_and_nowrite() {
    let dir = tempfile::tempdir().unwrap();
    let Some(a) = make(dir.path(), "a.mp3", &[]) else {
        return;
    };
    let b = make(dir.path(), "b.mp3", &[]).unwrap();
    let lib = Library::in_memory_at(dir.path()).unwrap();
    for p in [&a, &b] {
        let mut it = crate::media::read(p).unwrap();
        lib.add(&mut it).unwrap();
    }
    command(&lib, false, Some(1), &[]).unwrap();
    let scrubbed = [&a, &b].iter().filter(|p| artist(p).is_none()).count();
    assert_eq!(scrubbed, 1);
    command(
        &lib,
        true,
        None,
        &["path:".to_string() + &b.display().to_string()],
    )
    .unwrap();
    assert_eq!(artist(&b).as_deref(), Some("Tag Artist"));
}
