//! Ported from beets' `test/plugins/test_convert.py` (with a shell command
//! standing in for `convert_stub.py`: it copies the file and appends a
//! tag), plus the helpers' Python semantics (`shlex.split`,
//! `Template.safe_substitute`).

use super::*;
use crate::fileops::FileOp;
use std::path::PathBuf;
use tempfile::TempDir;

fn ffmpeg_ok() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A tiny audio file made by ffmpeg.
fn make(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    let ok = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.1"])
        .args([
            "-metadata",
            "title=Tag Title",
            "-metadata",
            "artist=Tag Artist",
        ])
        .arg(&p)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    p
}

/// `tagged_copy_cmd`: a conversion command that copies the file and
/// appends `tag` to the copy.
fn tagged_copy_cmd(tag: &str) -> String {
    format!("sh -c 'cat \"$0\" > \"$1\" && printf %s {tag} >> \"$1\"' $source $dest")
}

fn file_endswith(path: &Path, tag: &str) -> bool {
    assert!(path.is_file(), "{} is not a file", path.display());
    std::fs::read(path).unwrap().ends_with(tag.as_bytes())
}

fn y(s: &str) -> Y {
    serde_yaml::from_str(s).unwrap()
}

struct Env {
    dir: TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.set(
            "directory",
            Y::String(dir.path().join("lib").display().to_string()),
        );
        cfg.set("plugins", y("[convert]"));
        cfg.set(
            "convert.dest",
            Y::String(dir.path().join("convert_dest").display().to_string()),
        );
        cfg.set("convert.threads", y("1"));
        let lib = Library::in_memory_at(&dir.path().join("lib")).unwrap();
        Env { dir, cfg, lib }
    }

    fn dest(&self) -> PathBuf {
        self.dir.path().join("convert_dest")
    }

    fn add_item(&self, ext: &str, album: bool) -> Item {
        let n = self
            .lib
            .items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
            .unwrap()
            .len();
        let p = make(&self.dir.path().join("lib"), &format!("item{n}.{ext}"));
        let mut item = crate::media::read(&p).unwrap();
        if album {
            self.lib.add_album(std::slice::from_mut(&mut item)).unwrap();
        } else {
            self.lib.add(&mut item).unwrap();
        }
        item
    }

    /// `TestConvertCli`'s setup: an Ogg album and its formats.
    fn cli() -> Env {
        let mut e = Env::new();
        e.cfg.set("convert.paths", y("{default: converted}"));
        e.cfg.set("convert.format", y("mp3"));
        let mut formats = serde_yaml::Mapping::new();
        formats.insert(y("mp3"), Y::String(tagged_copy_cmd("mp3")));
        formats.insert(y("ogg"), Y::String(tagged_copy_cmd("ogg")));
        let mut opus = serde_yaml::Mapping::new();
        opus.insert(y("command"), Y::String(tagged_copy_cmd("opus")));
        opus.insert(y("extension"), y("ops"));
        formats.insert(y("opus"), Y::Mapping(opus));
        e.cfg.set("convert.formats", Y::Mapping(formats));
        // The stub's output is Ogg data with the tag appended: Mutagen
        // keeps those trailing bytes when beets writes tags, lofty doesn't.
        e.cfg.set("convert.write_metadata", Y::Bool(false));
        e.add_item("ogg", true);
        e
    }

    fn converted_mp3(&self) -> PathBuf {
        self.dest().join("converted.mp3")
    }

    fn item(&self) -> Item {
        self.lib.get_item(1).unwrap().unwrap()
    }

    /// `run_convert_path(item, *args)`, answering the prompt with `answer`.
    fn run(&self, item: &Item, answer: Option<&str>, opts: CmdOpts) -> Vec<String> {
        let mut opts = opts;
        // (beets' tests use `path:`, which needs the global library context.)
        opts.query = vec![format!("id:{}", item.id.unwrap())];
        crate::ui::script::set(answer.map(|a| (a, None)).into_iter().collect());
        let (r, lines) = crate::logging::capture(|| command(&self.cfg, &self.lib, &opts));
        r.unwrap();
        lines
    }
}

fn yes() -> CmdOpts {
    CmdOpts {
        yes: true,
        ..Default::default()
    }
}

#[test]
fn test_convert() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(&e.item(), Some("y"), CmdOpts::default());
    assert!(file_endswith(&e.converted_mp3(), "mp3"));
    assert_eq!(crate::ui::script::prompts(), vec!["Convert? (Y/n) "]);
}

#[test]
fn test_convert_with_auto_confirmation() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(&e.item(), None, yes());
    assert!(file_endswith(&e.converted_mp3(), "mp3"));
}

#[test]
fn test_reject_confirmation() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(&e.item(), Some("n"), CmdOpts::default());
    assert!(!e.converted_mp3().exists());
}

#[test]
fn test_convert_keep_new() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    assert_eq!(e.item().path().extension().unwrap(), "ogg");
    e.run(
        &e.item(),
        Some("y"),
        CmdOpts {
            keep_new: true,
            ..Default::default()
        },
    );
    assert_eq!(e.item().path().extension().unwrap(), "mp3");
    // The original moved to the destination.
    assert!(e.dest().join("converted.ogg").is_file());
}

#[test]
fn test_format_option() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(
        &e.item(),
        Some("y"),
        CmdOpts {
            format: Some("opus".into()),
            ..Default::default()
        },
    );
    assert!(file_endswith(&e.dest().join("converted.ops"), "opus"));
}

#[test]
fn test_embed_album_art() {
    if !ffmpeg_ok() {
        return;
    }
    // A real transcode, so the converted file takes tags.
    let mut e = Env::cli();
    e.cfg.set(
        "convert.formats.mp3",
        y("ffmpeg -i $source -y -vn -aq 2 $dest"),
    );
    e.cfg.set("convert.write_metadata", Y::Bool(true));
    e.cfg.set("convert.embed", Y::Bool(true));
    let image = e.dir.path().join("image.jpg");
    crate::artresizer::tests::write_image(&image, &crate::artresizer::tests::picture(2, 3, 1), 90);
    let mut album = e.lib.get_album(1).unwrap().unwrap();
    album.set("artpath", Value::Path(image.clone()));
    e.lib.store(&album).unwrap();
    e.run(&e.item(), Some("y"), CmdOpts::default());
    let data = crate::media::get_art(&e.converted_mp3()).unwrap().unwrap();
    assert_eq!(data, std::fs::read(&image).unwrap());
}

#[test]
fn test_copy_album_art_missing_source() {
    if !ffmpeg_ok() {
        return;
    }
    let mut e = Env::cli();
    e.cfg.set("convert.copy_album_art", Y::Bool(true));
    let mut album = e.lib.get_album(1).unwrap().unwrap();
    album.set("artpath", Value::Path(e.dir.path().join("nonexistent.jpg")));
    e.lib.store(&album).unwrap();
    crate::ui::script::set(vec![]);
    let (r, lines) = crate::logging::capture(|| {
        command(
            &e.cfg,
            &e.lib,
            &CmdOpts {
                album: true,
                yes: true,
                ..Default::default()
            },
        )
    });
    r.unwrap();
    assert!(
        lines.iter().any(|l| l.contains("source file not found")),
        "{lines:?}"
    );
    assert!(file_endswith(&e.converted_mp3(), "mp3"));
}

#[test]
fn test_copy_album_art_resized() {
    if !ffmpeg_ok() {
        return;
    }
    let mut e = Env::cli();
    e.cfg.set("convert.copy_album_art", Y::Bool(true));
    e.cfg.set("convert.album_art_maxwidth", y("10"));
    e.cfg.set("art_filename", y("cover"));
    let image = e.dir.path().join("image.jpg");
    crate::artresizer::tests::write_image(
        &image,
        &crate::artresizer::tests::picture(40, 20, 1),
        90,
    );
    let mut album = e.lib.get_album(1).unwrap().unwrap();
    album.set("artpath", Value::Path(image.clone()));
    e.lib.store(&album).unwrap();
    let opts = CmdOpts {
        album: true,
        yes: true,
        ..Default::default()
    };
    let lines = crate::logging::capture(|| command(&e.cfg, &e.lib, &opts)).1;
    let dest = e.dest().join("cover.jpg");
    assert!(
        lines.contains(&format!(
            "convert: Resizing cover art from {} to {}",
            image.display(),
            dest.display()
        )),
        "{lines:?}"
    );
    assert_eq!(crate::artresizer::get_size(&dest), Some((10, 5)));
}

#[test]
fn test_skip_existing() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    let converted = e.converted_mp3();
    std::fs::create_dir_all(converted.parent().unwrap()).unwrap();
    std::fs::write(&converted, "XXX").unwrap();
    e.run(&e.item(), None, yes());
    assert_eq!(std::fs::read_to_string(&converted).unwrap(), "XXX");
}

#[test]
fn test_refresh_reconverts_older_destination() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    let converted = e.converted_mp3();
    std::fs::create_dir_all(converted.parent().unwrap()).unwrap();
    std::fs::write(&converted, "XXX").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&converted)
        .unwrap()
        .set_modified(old)
        .unwrap();
    e.run(
        &e.item(),
        None,
        CmdOpts {
            yes: true,
            refresh: true,
            ..Default::default()
        },
    );
    assert!(file_endswith(&converted, "mp3"));
}

#[test]
fn test_pretend() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    let lines = e.run(
        &e.item(),
        None,
        CmdOpts {
            pretend: true,
            ..Default::default()
        },
    );
    assert!(!e.converted_mp3().exists());
    // Nothing listed (the worker thread's log goes straight to stderr).
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn encode_pretend_logs_the_split_command() {
    let mut cfg = Config::defaults();
    cfg.set("convert.pretend", Y::Bool(true));
    let s = Settings::new(&cfg).unwrap();
    let log = PluginLog::new("convert");
    let (r, lines) = crate::logging::capture(|| {
        encode(
            &s,
            &log,
            &tagged_copy_cmd("mp3"),
            Path::new("/a/b c.ogg"),
            Path::new("/d/e.mp3"),
        )
    });
    assert!(r.unwrap());
    assert_eq!(
        lines,
        vec!["convert: sh -c cat \"$0\" > \"$1\" && printf %s mp3 >> \"$1\" /a/b c.ogg /d/e.mp3"]
    );
}

#[test]
fn test_empty_query() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    crate::ui::script::set(vec![]);
    let opts = CmdOpts {
        query: vec!["An impossible query".into()],
        ..Default::default()
    };
    let lines = crate::logging::capture(|| command(&e.cfg, &e.lib, &opts)).1;
    assert_eq!(lines, vec!["convert: Empty query result."]);
}

/// `test_transcode_selection`.
#[test]
fn test_transcode_selection() {
    if !ffmpeg_ok() {
        return;
    }
    let cases: &[(Option<i64>, &str, bool, bool)] = &[
        (Some(5000), "mp3", false, true),
        (Some(5), "mp3", false, true),
        (None, "mp3", false, true),
        (Some(5000), "ogg", false, false),
        (Some(5000), "ogg", true, true),
        (Some(5), "ogg", false, true),
        (None, "ogg", false, false),
    ];
    for &(max_bitrate, fmt, force, should_transcode) in cases {
        let mut e = Env::cli();
        if let Some(m) = max_bitrate {
            e.cfg.set("convert.max_bitrate", y(&m.to_string()));
        }
        e.cfg.set("convert.format", Y::String(fmt.into()));
        e.run(
            &e.item(),
            Some("y"),
            CmdOpts {
                force,
                ..Default::default()
            },
        );
        let converted = e.dest().join(format!("converted.{fmt}"));
        assert_eq!(
            file_endswith(&converted, fmt),
            should_transcode,
            "{max_bitrate:?} {fmt} {force}"
        );
    }
}

#[test]
fn test_playlist() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(
        &e.item(),
        Some("y"),
        CmdOpts {
            playlist: Some("playlist.m3u8".into()),
            ..Default::default()
        },
    );
    assert!(e.dest().join("playlist.m3u8").exists());
}

#[test]
fn test_playlist_pretend() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(
        &e.item(),
        None,
        CmdOpts {
            playlist: Some("playlist.m3u8".into()),
            pretend: true,
            ..Default::default()
        },
    );
    assert!(!e.dest().join("playlist.m3u8").exists());
}

#[test]
fn test_force_overrides() {
    if !ffmpeg_ok() {
        return;
    }
    for (key, value) in [
        ("no_convert", "format:ogg"),
        ("never_convert_lossy_files", "yes"),
    ] {
        let mut e = Env::cli();
        let item = e.add_item("ogg", false);
        e.cfg.set(&format!("convert.{key}"), y(value));
        e.run(
            &item,
            Some("y"),
            CmdOpts {
                format: Some("opus".into()),
                force: true,
                ..Default::default()
            },
        );
        assert!(
            file_endswith(&e.dest().join("converted.ops"), "opus"),
            "{key}"
        );
    }
}

#[test]
fn test_playlist_entry() {
    if !ffmpeg_ok() {
        return;
    }
    let cases: &[(Option<&str>, bool, Option<&str>, &str)] = &[
        (None, false, None, "converted.mp3"),
        (Some("opus"), false, None, "converted.ops"),
        (None, false, Some("format:ogg"), "converted.ogg"),
        (None, true, None, "converted.ogg"),
    ];
    for &(format, keep_new, no_convert, expected) in cases {
        let mut e = Env::cli();
        if let Some(nc) = no_convert {
            e.cfg.set("convert.no_convert", Y::String(nc.into()));
        }
        e.run(
            &e.item(),
            Some("y"),
            CmdOpts {
                format: format.map(String::from),
                keep_new,
                playlist: Some("playlist.m3u8".into()),
                ..Default::default()
            },
        );
        let text = std::fs::read_to_string(e.dest().join("playlist.m3u8")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#EXTM3U");
        assert_eq!(lines[1], expected);
    }
}

#[test]
fn playlist_entries_relative_to_playlist_dir() {
    if !ffmpeg_ok() {
        return;
    }
    let e = Env::cli();
    e.run(
        &e.item(),
        Some("y"),
        CmdOpts {
            playlist: Some("sub/dir/pl.m3u".into()),
            ..Default::default()
        },
    );
    let text = std::fs::read_to_string(e.dest().join("sub/dir/pl.m3u")).unwrap();
    assert_eq!(text, "#EXTM3U\n../../converted.mp3\n\n");
}

/// `TestNeverConvertLossyFiles.test_transcode`.
#[test]
fn test_never_convert_lossy_files() {
    if !ffmpeg_ok() {
        return;
    }
    let cases = [
        ("flac", true, "mp3", true),
        ("flac", false, "mp3", true),
        ("ogg", false, "mp3", true),
        ("ogg", true, "ogg", false),
    ];
    for (source_ext, never, expected_ext, should_convert) in cases {
        let mut e = Env::new();
        let item = e.add_item(source_ext, false);
        e.cfg.set("convert.paths", y("{default: converted}"));
        e.cfg.set("convert.format", y("mp3"));
        e.cfg
            .set("convert.formats.mp3", Y::String(tagged_copy_cmd("mp3")));
        e.cfg
            .set("convert.never_convert_lossy_files", Y::Bool(never));
        e.cfg.set("convert.write_metadata", Y::Bool(false));
        e.run(&item, Some("y"), CmdOpts::default());
        let converted = e.dest().join(format!("converted.{expected_ext}"));
        assert_eq!(
            file_endswith(&converted, "mp3"),
            should_convert,
            "{source_ext} {never}"
        );
    }
}

/// `TestNoConvert.test_no_convert_skip`.
#[test]
fn test_no_convert_skip() {
    let lib = Library::in_memory().unwrap();
    let mut item = Item::new_item();
    item.set_str("format", "ogg");
    item.set_int("bitrate", 256);
    for (value, should_skip) in [
        ("", false),
        ("bitrate:320", false),
        ("bitrate:320 format:ogg", false),
        ("bitrate:320 , format:ogg", true),
    ] {
        let mut cfg = Config::defaults();
        cfg.set("convert.no_convert", Y::String(value.into()));
        let s = Settings::new(&cfg).unwrap();
        let sel = Selector::new(&s, &lib).unwrap();
        assert_eq!(sel.in_no_convert(&item), should_skip, "{value}");
    }
}

#[test]
fn command_errors() {
    if !ffmpeg_ok() {
        return;
    }
    let mut e = Env::cli();
    let r = command(
        &e.cfg,
        &e.lib,
        &CmdOpts {
            yes: true,
            format: Some("xyz".into()),
            ..Default::default()
        },
    );
    let err = r.unwrap_err();
    assert_eq!(
        err.downcast_ref::<ConfigError>().map(|c| c.0.as_str()),
        Some("convert.formats.xyz not found")
    );
    e.cfg.set("convert.formats.foo", y("{extension: x}"));
    let r = command(
        &e.cfg,
        &e.lib,
        &CmdOpts {
            yes: true,
            format: Some("foo".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        r.unwrap_err().to_string(),
        "convert: format foo needs the \"command\" field"
    );
    e.cfg.set("convert.dest", Y::Null);
    let r = command(&e.cfg, &e.lib, &yes());
    assert_eq!(r.unwrap_err().to_string(), "no convert destination set");
    // An encoder that can't be run.
    e.cfg
        .set("convert.dest", Y::String(e.dest().display().to_string()));
    e.cfg.set(
        "convert.formats.mp3",
        y("no-such-encoder-xyz $source $dest"),
    );
    let r = command(&e.cfg, &e.lib, &yes());
    let msg = r.unwrap_err().to_string();
    assert!(
        msg.starts_with("convert: couldn't invoke 'no-such-encoder-xyz ")
            && msg.ends_with(": [Errno 2] No such file or directory: 'no-such-encoder-xyz'"),
        "{msg}"
    );
}

#[test]
fn link_and_hardlink_untranscoded() {
    if !ffmpeg_ok() {
        return;
    }
    let mut e = Env::cli();
    e.cfg.set("convert.format", y("ogg"));
    e.run(
        &e.item(),
        None,
        CmdOpts {
            yes: true,
            link: true,
            ..Default::default()
        },
    );
    let out = e.dest().join("converted.ogg");
    assert_eq!(
        std::fs::read_link(&out).unwrap(),
        crate::osutil::syspath(e.item().path())
    );
    std::fs::remove_file(&out).unwrap();
    // -H overrides -l.
    e.run(
        &e.item(),
        None,
        CmdOpts {
            yes: true,
            link: true,
            hardlink: true,
            ..Default::default()
        },
    );
    assert!(!std::fs::symlink_metadata(&out).unwrap().is_symlink());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(&out).unwrap().ino(),
            std::fs::metadata(e.item().path()).unwrap().ino()
        );
    }
}

#[test]
fn tags_written_from_library() {
    if !ffmpeg_ok() {
        return;
    }
    let mut e = Env::cli();
    e.cfg.set(
        "convert.formats.mp3",
        y("ffmpeg -i $source -y -vn -aq 2 $dest"),
    );
    e.cfg.set("convert.write_metadata", Y::Bool(true));
    let mut item = e.item();
    item.set_str("title", "Library Title");
    e.lib.store(&item).unwrap();
    e.run(&item, None, yes());
    let read = crate::media::read(&e.converted_mp3()).unwrap();
    assert_eq!(read.str("title"), "Library Title");
    assert_eq!(read.str("format"), "MP3");
    // write_metadata: no leaves ffmpeg's tags (none: it doesn't map Ogg
    // stream tags).
    std::fs::remove_file(e.converted_mp3()).unwrap();
    e.cfg.set("convert.write_metadata", Y::Bool(false));
    e.cfg.set("convert.embed", Y::Bool(false));
    e.run(&item, None, yes());
    let read = crate::media::read(&e.converted_mp3()).unwrap();
    assert_eq!(read.str("title"), "");
}

#[test]
fn threads_zero_converts_nothing_and_many_threads_convert_all() {
    if !ffmpeg_ok() {
        return;
    }
    let mut e = Env::cli();
    e.cfg.set("convert.paths", y("{default: $title-$id}"));
    for _ in 0..4 {
        e.add_item("ogg", false);
    }
    let all = CmdOpts {
        yes: true,
        threads: Some(0),
        ..Default::default()
    };
    command(&e.cfg, &e.lib, &all.clone()).unwrap();
    assert!(!e.dest().exists());
    command(
        &e.cfg,
        &e.lib,
        &CmdOpts {
            threads: Some(3),
            ..all
        },
    )
    .unwrap();
    for id in 1..=5 {
        assert!(file_endswith(
            &e.dest().join(format!("Tag Title-{id}.mp3")),
            "mp3"
        ));
    }
}

// --------------------------------------------------------------- import

/// `run_asis_importer` reduced to what convert sees: the item added, the
/// early import stages, the file copied into the library, then
/// `import_task_files`.
fn asis_import(e: &Env, plugin: &ConvertPlugin, src: &Path, op: FileOp) -> Item {
    let mut item = crate::media::read(src).unwrap();
    e.lib.add(&mut item).unwrap();
    let ctx = PluginCtx {
        cfg: &e.cfg,
        lib: &e.lib,
        write: true,
    };
    let mut items = vec![item];
    let paths = [src.parent().unwrap().to_path_buf()];
    let mut task = ImportTask {
        album: None,
        items: &mut items,
        paths: &paths,
        asis: true,
        op,
    };
    plugin.early_import_stage(&ctx, &mut task).unwrap();
    let mut item = e.lib.get_item(items[0].id.unwrap()).unwrap().unwrap();
    // `manipulate_files`.
    if op != FileOp::None {
        let ext = item
            .path()
            .extension()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let dest = e.dir.path().join("lib").join(format!("imported.{ext}"));
        fileops::transfer(op, item.path(), &dest).unwrap();
        item.set_path(&dest);
        e.lib.store(&item).unwrap();
    }
    let mut items = vec![item];
    let mut task = ImportTask {
        album: None,
        items: &mut items,
        paths: &paths,
        asis: true,
        op,
    };
    plugin.import_task_files(&ctx, &mut task).unwrap();
    items.remove(0)
}

fn import_env() -> (Env, PathBuf) {
    let mut e = Env::new();
    e.cfg
        .set("convert.command", Y::String(tagged_copy_cmd("convert")));
    // Enforce running convert.
    e.cfg.set("convert.max_bitrate", y("1"));
    e.cfg.set("convert.auto", Y::Bool(true));
    e.cfg.set("convert.quiet", Y::Bool(false));
    let tmp = e.dir.path().join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    e.cfg
        .set("convert.tmpdir", Y::String(tmp.display().to_string()));
    let src = make(&e.dir.path().join("import"), "a.mp3");
    (e, src)
}

fn tmp_files(e: &Env) -> usize {
    std::fs::read_dir(e.dir.path().join("tmp"))
        .map_or(0, Iterator::count)
}

#[test]
fn test_import_converted() {
    if !ffmpeg_ok() {
        return;
    }
    let (e, src) = import_env();
    let plugin = ConvertPlugin::new();
    let item = asis_import(&e, &plugin, &src, FileOp::Copy);
    assert!(file_endswith(item.path(), "convert"));
    // The temporary transcode is gone, the original kept.
    assert_eq!(tmp_files(&e), 0);
    assert!(src.is_file());
}

#[test]
fn test_import_original_on_convert_error() {
    if !ffmpeg_ok() {
        return;
    }
    let (mut e, src) = import_env();
    // `false` exits with non-zero code.
    e.cfg.set("convert.command", y("'false'"));
    let plugin = ConvertPlugin::new();
    let item = asis_import(&e, &plugin, &src, FileOp::Copy);
    assert!(item.path().is_file());
    assert!(!file_endswith(item.path(), "convert"));
    assert_eq!(tmp_files(&e), 0);
}

#[test]
fn test_delete_originals() {
    if !ffmpeg_ok() {
        return;
    }
    let (mut e, src) = import_env();
    e.cfg.set("convert.delete_originals", Y::Bool(true));
    let plugin = ConvertPlugin::new();
    let item = asis_import(&e, &plugin, &src, FileOp::Copy);
    assert!(!src.exists());
    assert!(file_endswith(item.path(), "convert"));
}

#[test]
fn import_without_copying_keeps_the_transcode() {
    if !ffmpeg_ok() {
        return;
    }
    let (e, src) = import_env();
    let plugin = ConvertPlugin::new();
    let item = asis_import(&e, &plugin, &src, FileOp::None);
    assert!(item.path().starts_with(e.dir.path().join("tmp")));
    assert!(file_endswith(item.path(), "convert"));
}

#[test]
fn auto_keep_converts_a_copy_into_dest() {
    if !ffmpeg_ok() {
        return;
    }
    let (mut e, src) = import_env();
    e.cfg.set("convert.auto", Y::Bool(false));
    e.cfg.set("convert.auto_keep", Y::Bool(true));
    e.cfg.set("convert.paths", y("{default: kept}"));
    let plugin = ConvertPlugin::new();
    let item = asis_import(&e, &plugin, &src, FileOp::Copy);
    assert!(!file_endswith(item.path(), "convert"));
    assert!(file_endswith(&e.dest().join("kept.mp3"), "convert"));
}

// ------------------------------------------------------------ helpers

#[test]
fn shlex_split_as_python() {
    let s = |x: &str| shlex_split(x).unwrap();
    assert_eq!(s("a b  c"), ["a", "b", "c"]);
    assert_eq!(s("a 'b c' \"d e\""), ["a", "b c", "d e"]);
    assert_eq!(s(r#"a\ b "x\"y\z" 'q\'"#), ["a b", "x\"y\\z", "q\\"]);
    assert_eq!(s("a'b'\"c\"d ''"), ["abcd", ""]);
    assert_eq!(s("a #b"), ["a", "#b"]);
    assert!(shlex_split("'a").is_err());
    assert!(shlex_split("a\\").is_err());
}

#[test]
fn safe_substitute_as_python() {
    let v = [("source", "S"), ("dest", "D")];
    assert_eq!(safe_substitute("$source", &v), "S");
    assert_eq!(safe_substitute("${dest}.x", &v), "D.x");
    assert_eq!(
        safe_substitute("$sourcex $$source $0 $ ${a", &v),
        "$sourcex $source $0 $ ${a"
    );
    assert_eq!(safe_substitute("$other-$dest", &v), "$other-D");
}

#[test]
fn format_command_aliases_and_shortcuts() {
    let mut cfg = Config::defaults();
    let c = format_command(&cfg, "vorbis").unwrap();
    assert_eq!(
        c.command,
        "ffmpeg -i $source -y -vn -acodec libvorbis -aq 3 $dest"
    );
    assert_eq!(c.ext, "ogg");
    let c = format_command(&cfg, "alac").unwrap();
    assert_eq!(c.ext, "m4a");
    cfg.set("convert.opts", y("-x"));
    assert_eq!(
        format_command(&cfg, "mp3").unwrap().command,
        "ffmpeg -i $source -y -x $dest"
    );
    cfg.set("convert.command", y("cmd $source $dest"));
    cfg.set("convert.extension", y("ext"));
    let c = format_command(&cfg, "mp3").unwrap();
    assert_eq!(
        (c.command.as_str(), c.ext.as_str()),
        ("cmd $source $dest", "ext")
    );
}

#[test]
fn replace_ext_and_relpath() {
    assert_eq!(
        replace_ext(Path::new("/a/b.c/d.flac"), "mp3"),
        Path::new("/a/b.c/d.mp3")
    );
    assert_eq!(
        replace_ext(Path::new("/a/b.c/d"), "mp3"),
        Path::new("/a/b.c/d.mp3")
    );
    assert_eq!(
        replace_ext(Path::new("/a/.hidden"), "mp3"),
        Path::new("/a/.hidden.mp3")
    );
    assert_eq!(
        relpath(Path::new("/a/b/c.mp3"), Path::new("/a/x/y")),
        Path::new("../../b/c.mp3")
    );
    assert_eq!(
        relpath(Path::new("/a/b.mp3"), Path::new("/a")),
        Path::new("b.mp3")
    );
}
