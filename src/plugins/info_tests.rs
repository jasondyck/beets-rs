//! beets' `test/plugins/test_info.py`, and the plugin's formatting.

use super::*;
use crate::media;
use crate::model::Item;
use tempfile::TempDir;

/// MediaFile's `readable_fields()` (beets 2.14, mediafile 0.17) and
/// `art`, less `images`.
const PY_FIELDS: &str = "acoustid_fingerprint acoustid_id album albumartist albumartist_credit \
    albumartist_sort albumartists albumartists_credit albumartists_sort albumdisambig \
    albumstatus albumtype albumtypes arranger arrangers art artist artist_credit artist_sort \
    artists artists_credit artists_sort asin barcode bitdepth bitrate bitrate_mode bpm \
    catalognum catalognums channels comments comp composer composer_sort composers copyright \
    country date day disc disctitle disctotal encoder encoder_info encoder_settings format \
    genre genres grouping initial_key isrc label language languages length lyricist lyricists \
    lyrics mb_albumartistid mb_albumartistids mb_albumid mb_artistid mb_artistids \
    mb_releasegroupid mb_releasetrackid mb_trackid mb_workid media month original_date \
    original_day original_month original_year r128_album_gain r128_track_gain remixers \
    rg_album_gain rg_album_peak rg_track_gain rg_track_peak samplerate script subtitle \
    synced_lyrics title track tracktotal url year";

#[test]
fn tag_fields_are_mediafiles() {
    let want: Vec<&str> = PY_FIELDS.split_whitespace().collect();
    assert_eq!(tag_fields(), want);
}

struct Env {
    dir: TempDir,
    lib: Library,
}

/// An MP3 made with ffmpeg (None without ffmpeg).
fn mp3(path: &Path, ext: &str) -> Option<()> {
    let p = path.with_extension(ext);
    std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=d=1",
            "-ac",
            "1",
        ])
        .arg(&p)
        .status()
        .ok()?
        .success()
        .then_some(())
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env { dir, lib }
    }

    /// beets' `create_mediafile_fixture`: a new file outside the library.
    fn mediafile_fixture(&self, name: &str) -> Option<PathBuf> {
        let p = self.dir.path().join(name);
        mp3(&p, "mp3")?;
        Some(p)
    }

    /// beets' `add_item_fixtures`: items in the library with files, titled
    /// "tïtle N", artist "the artist", track N+2... as beets' helper.
    fn add_items(&self, count: usize) -> Option<Vec<Item>> {
        let mut out = vec![];
        for i in 0..count {
            let p = self.dir.path().join("music").join(format!("item{i}.mp3"));
            mp3(&p, "mp3")?;
            let mut it = media::read(&p).unwrap();
            it.set_str("title", format!("tïtle {i}"));
            it.set_str("artist", "the artist");
            it.set_str("album", "the album");
            it.set_int("track", (i + 2) as i64);
            media::write(&it).unwrap();
            self.lib.add(&mut it).unwrap();
            out.push(it);
        }
        Some(out)
    }

    fn run(&self, opts: &CmdOpts) -> String {
        let mut out = vec![];
        run(&self.lib, opts, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn info(&self, args: &[&str]) -> String {
        self.run(&CmdOpts {
            args: args.iter().map(ToString::to_string).collect(),
            ..Default::default()
        })
    }
}

fn set_tags(path: &Path, f: impl FnOnce(&mut Item)) {
    let mut it = media::read(path).unwrap();
    f(&mut it);
    media::write(&it).unwrap();
}

#[test]
fn path() {
    let env = Env::new();
    let Some(path) = env.mediafile_fixture("min.mp3") else {
        return;
    };
    set_tags(&path, |it| {
        it.set_str("albumartist", "AAA");
        it.set_str("disctitle", "DDD");
        it.set_list("genres", &["a", "b", "c"]);
        it.set_list::<&str>("composers", &[]);
    });
    let out = env.info(&[path.to_str().unwrap()]);
    assert!(out.contains(path.to_str().unwrap()), "{out}");
    assert!(out.contains("albumartist: AAA"), "{out}");
    assert!(out.contains("disctitle: DDD"), "{out}");
    assert!(out.contains("genres: a; b; c"), "{out}");
    assert!(!out.contains("composer:"), "{out}");
}

#[test]
fn item_query() {
    let env = Env::new();
    let Some(mut items) = env.add_items(2) else {
        return;
    };
    let item2 = items.pop().unwrap();
    let mut item1 = items.pop().unwrap();
    item1.set_str("album", "xxxx");
    media::write(&item1).unwrap();
    item1.set_str("album", "yyyy");
    env.lib.store(&item1).unwrap();

    let out = env.info(&["album:yyyy"]);
    assert!(out.contains(item1.path().to_str().unwrap()), "{out}");
    assert!(out.contains("album: xxxx"), "{out}");
    assert!(!out.contains(item2.path().to_str().unwrap()), "{out}");
}

#[test]
fn item_library_query() {
    let env = Env::new();
    let Some(mut items) = env.add_items(1) else {
        return;
    };
    let mut item = items.pop().unwrap();
    item.set_str("album", "xxxx");
    env.lib.store(&item).unwrap();

    let out = env.run(&CmdOpts {
        library: true,
        args: vec!["album:xxxx".into()],
        ..Default::default()
    });
    assert!(out.contains(item.path().to_str().unwrap()), "{out}");
    assert!(out.contains("album: xxxx"), "{out}");
}

/// The shared setup of the two `collect_item_and_path` tests.
fn item_and_path(multi: bool) -> Option<(Env, PathBuf)> {
    let env = Env::new();
    let path = env.mediafile_fixture("min.mp3")?;
    let mut item = env.add_items(1)?.pop().unwrap();
    item.set_str("album", "AAA");
    item.set_int("tracktotal", 5);
    item.set_str("title", "TTT");
    if multi {
        item.set_list("albumartists", &["Artist A", "Artist B"]);
        item.set_list("artists", &["Artist A", "Artist Z"]);
    }
    media::write(&item).unwrap();
    env.lib.store(&item).unwrap();
    set_tags(&path, |it| {
        it.set_str("album", "AAA");
        it.set_int("tracktotal", 5);
        it.set_str("title", "SSS");
        if multi {
            it.set_list("albumartists", &["Artist C", "Artist D"]);
            it.set_list("artists", &["Artist A", "Artist Z"]);
        }
    });
    Some((env, path))
}

fn summarize(env: &Env, path: &Path) -> String {
    env.run(&CmdOpts {
        summarize: true,
        args: vec!["album:AAA".into(), path.to_str().unwrap().into()],
        ..Default::default()
    })
}

#[test]
fn collect_item_and_path() {
    let Some((env, path)) = item_and_path(false) else {
        return;
    };
    let out = summarize(&env, &path);
    assert!(out.contains("album: AAA"), "{out}");
    assert!(out.contains("tracktotal: 5"), "{out}");
    assert!(out.contains("title: [various]"), "{out}");
}

#[test]
fn collect_item_and_path_with_multi_values() {
    let Some((env, path)) = item_and_path(true) else {
        return;
    };
    let out = summarize(&env, &path);
    assert!(out.contains("album: AAA"), "{out}");
    assert!(out.contains("tracktotal: 5"), "{out}");
    assert!(out.contains("title: [various]"), "{out}");
    assert!(out.contains("albumartists: [various]"), "{out}");
    assert!(out.contains("artists: Artist A; Artist Z"), "{out}");
}

#[test]
fn custom_format() {
    let env = Env::new();
    if env.add_items(1).is_none() {
        return;
    }
    let out = env.run(&CmdOpts {
        library: true,
        format: Some("$track. $title - $artist ($length)".into()),
        ..Default::default()
    });
    assert_eq!(out, "02. tïtle 0 - the artist (0:01)\n");
}

// ------------------------------------------------------------ beyond beets'

fn data(pairs: &[(&str, Option<PyVal>)]) -> Data {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn render_aligns_sorts_and_skips_missing() {
    let d = data(&[
        ("title", Some(PyVal::Str("T".into()))),
        ("bitrate_mode", Some(PyVal::Str(String::new()))),
        ("composer", None),
        ("art", Some(PyVal::Bool(false))),
        ("length", Some(PyVal::Float(1.0))),
        ("genres", Some(PyVal::List(vec!["a".into(), "b".into()]))),
    ]);
    assert_eq!(
        render_data(&d, Some("/x.mp3")),
        [
            "/x.mp3",
            "         art: False",
            "bitrate_mode: ",
            "      genres: a; b",
            "      length: 1.0",
            "       title: T",
        ]
    );
    // Keys only: every key, values or not.
    assert_eq!(
        render_keys(&d, None),
        [
            "    art",
            "    bitrate_mode",
            "    composer",
            "    genres",
            "    length",
            "    title"
        ]
    );
    // Nothing to show: not even the path.
    assert!(render_data(&data(&[("composer", None)]), Some("/x.mp3")).is_empty());
}

#[test]
fn summary_merges_as_python() {
    let mut s = Data::new();
    update_summary(
        &mut s,
        data(&[
            ("a", Some(PyVal::Int(1))),
            ("b", Some(PyVal::Str("x".into()))),
            ("c", None),
            ("d", None),
        ]),
    );
    update_summary(
        &mut s,
        data(&[
            ("a", Some(PyVal::Float(1.0))),
            ("b", Some(PyVal::Str("y".into()))),
            ("c", Some(PyVal::Str("z".into()))),
            ("d", None),
            ("e", Some(PyVal::Bool(true))),
        ]),
    );
    assert_eq!(
        render_data(&s, None),
        ["a: 1", "b: [various]", "c: [various]", "e: True"]
    );
}

#[test]
fn included_keys_and_dates() {
    let env = Env::new();
    let Some(path) = env.mediafile_fixture("d.mp3") else {
        return;
    };
    set_tags(&path, |it| {
        it.set_int("year", 2001);
        it.set_str("title", "T");
    });
    let out = env.run(&CmdOpts {
        included_keys: vec!["title,date,nope,images".into(), "path,art".into()],
        args: vec![path.to_str().unwrap().into()],
        ..Default::default()
    });
    let want = format!(
        "{}\n  art: False\n date: 2001-01-01\ntitle: T\n",
        path.display()
    );
    assert_eq!(out, want);
}

#[test]
fn unreadable_file_is_skipped() {
    let env = Env::new();
    let Some(good) = env.mediafile_fixture("good.mp3") else {
        return;
    };
    let bad = env.dir.path().join("bad.mp3");
    std::fs::write(&bad, b"").unwrap();
    let out = env.run(&CmdOpts {
        included_keys: vec!["format".into()],
        args: vec![bad.to_str().unwrap().into(), good.to_str().unwrap().into()],
        ..Default::default()
    });
    assert_eq!(out, format!("{}\nformat: MP3\n", good.display()));
}

#[test]
fn mono_wavpack_channels_is_true() {
    let env = Env::new();
    let p = env.dir.path().join("m.wv");
    if mp3(&p, "wv").is_none() {
        return;
    }
    let out = env.run(&CmdOpts {
        included_keys: vec!["channels".into()],
        args: vec![p.to_str().unwrap().into()],
        ..Default::default()
    });
    assert!(out.ends_with("channels: True\n"), "{out}");
}

#[test]
fn no_args_shows_nothing_from_files() {
    let env = Env::new();
    if env.add_items(1).is_none() {
        return;
    }
    assert_eq!(env.info(&[]), "");
}
