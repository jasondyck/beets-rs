//! beets' test/plugins/test_replaygain.py (the ffmpeg, command and
//! metaflac backends), plus the output parsing on canned tool output.

use super::*;
use crate::media;
use crate::plugins::{DEBUG, ImportTask, PluginCtx};
use serde_yaml::Value as Y;
use tempfile::TempDir;

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn quiet_log() -> PluginLog {
    let log = PluginLog::new(NAME);
    log.set_base_level(100);
    log
}

// ------------------------------------------------------- ffmpeg's output

const FFMPEG_TAIL: &str = "\
[Parsed_ebur128_0 @ 0x1] t: 0.399977   TARGET:-23 LUFS    M:-120.7 S:-120.7     I: -70.0 LUFS       LRA:   0.0 LU  FTPK: -18.5 dBFS  TPK: -18.4 dBFS
[Parsed_ebur128_0 @ 0x1] t: 0.499977   TARGET:-23 LUFS    M: -22.2 S:-120.7     I: -22.2 LUFS       LRA:   0.0 LU  FTPK: -18.5 dBFS  TPK: -18.4 dBFS
[Parsed_ebur128_0 @ 0x1] t: 0.599977   TARGET:-23 LUFS    M: -22.1 S:-120.7     I: -22.2 LUFS       LRA:   0.0 LU  FTPK: -18.5 dBFS  TPK: -18.4 dBFS
[Parsed_ebur128_0 @ 0x1] t: 0.699977   TARGET:-23 LUFS    M: -40.0 S:-120.7     I: -22.2 LUFS       LRA:   0.0 LU  FTPK: -18.5 dBFS  TPK: -18.4 dBFS
[Parsed_ebur128_0 @ 0x1] Summary:

  Integrated loudness:
    I:         -22.2 LUFS
    Threshold: -32.2 LUFS

  Loudness range:
    LRA:         0.0 LU
    Threshold:   0.0 LUFS
    LRA low:     0.0 LUFS
    LRA high:    0.0 LUFS

  Sample peak:
    Peak:      -20.0 dBFS

  True peak:
    Peak:      -18.4 dBFS
[out#0/null @ 0x2] video:0KiB audio:86KiB subtitle:0KiB other streams:0KiB
";

#[test]
fn ffmpeg_output_true_peak() {
    let a =
        parse_ffmpeg_output(FFMPEG_TAIL.as_bytes(), 89.0, Some(PeakMethod::True), false).unwrap();
    // -18 LUFS target less -22.2 LUFS
    assert_eq!(a.gain.gain, -18.0 - -22.2);
    assert_eq!(a.gain.peak, 10f64.powf(-18.4 / 20.0));
    assert_eq!(a.blocks, 0);
    assert_eq!(a.threshold, None);
}

#[test]
fn ffmpeg_output_sample_peak_and_blocks() {
    let a =
        parse_ffmpeg_output(FFMPEG_TAIL.as_bytes(), 84.0, Some(PeakMethod::Sample), true).unwrap();
    assert_eq!(a.gain.gain, -23.0 - -22.2);
    assert_eq!(a.gain.peak, 10f64.powf(-20.0 / 20.0));
    // Blocks at or over the -32.2 LUFS threshold (not -120.7 or -40).
    assert_eq!(a.blocks, 2);
    assert_eq!(a.threshold, Some(-32.2));
}

#[test]
fn ffmpeg_output_no_peak() {
    let a = parse_ffmpeg_output(FFMPEG_TAIL.as_bytes(), 89.0, None, false).unwrap();
    assert_eq!(a.gain.peak, 0.0);
}

#[test]
fn ffmpeg_output_errors() {
    let e =
        parse_ffmpeg_output(b"nothing\nhere\n", 89.0, Some(PeakMethod::True), false).unwrap_err();
    assert_eq!(
        e,
        RgError::Local("ffmpeg output: missing b'  True peak:' after line 1".into())
    );
    let e = parse_ffmpeg_output(b"", 89.0, None, false).unwrap_err();
    assert_eq!(
        e,
        RgError::Local("ffmpeg output: missing b'  Integrated loudness:' after line -1".into())
    );
    let bad = b"  Integrated loudness:\n    I:         loud LUFS\n";
    let e = parse_ffmpeg_output(bad, 89.0, None, false).unwrap_err();
    assert_eq!(
        e,
        RgError::Local("ffmpeg output: expected float value, found b'loud'".into())
    );
}

#[test]
fn ffmpeg_version_check() {
    let new = b"ffmpeg version n9.0.2\nconfiguration: --prefix=/usr\nlibavfilter    11.  4.100 / 11.  4.100\n";
    assert!(ffmpeg_supports_ebur128(new));
    let old = b"configuration: --prefix=/usr\nlibavfilter     6. 66.100 /  6. 66.100\n";
    assert!(!ffmpeg_supports_ebur128(old));
    let lib = b"configuration: --prefix=/usr --enable-libebur128\nlibavfilter     5.  1.100 /  5.  1.100\n";
    assert!(ffmpeg_supports_ebur128(lib));
}

/// The album gain of two tracks, as reference beets computed it for a
/// sine and a noise MP3 (7 gating blocks each).
#[test]
fn album_gain_matches_beets() {
    let tracks = [
        (
            Gain {
                gain: 4.199999999999999,
                peak: 0.12022644346174131,
            },
            7,
        ),
        (
            Gain {
                gain: -4.4,
                peak: 0.5623413251903491,
            },
            7,
        ),
    ];
    let g = album_gain(&tracks, 89.0);
    assert_eq!(g.gain, -1.9512693082205956);
    assert_eq!(g.peak, 0.5623413251903491);
    // No blocks: -70 LUFS.
    let g = album_gain(&[(tracks[0].0, 0)], 89.0);
    assert_eq!(g.gain, -18.0 + 70.0);
}

#[test]
fn sum_is_pythons() {
    // Python 3.12+: sum([0.1] * 10) == 1.0
    assert_eq!(py_sum(std::iter::repeat_n(0.1, 10)), 1.0);
    assert_eq!(py_sum([1e100, 1.0, -1e100, 1.0]), 2.0);
}

// ------------------------------------------------- mp3gain's and metaflac's

#[test]
fn tool_output_parses() {
    let text = b"File\tMP3 gain\tdB gain\tMax Amplitude\tMax global_gain\tMin global_gain\n\
/a/1.mp3\t2\t3.050000\t16384.000000\t210\t100\n\
/a/2.mp3\t-1\t-1.500000\t32768.000000\t210\t100\n\
\"Album\"\t1\t0.750000\t32768.000000\t210\t100\n";
    let log = quiet_log();
    let g = parse_tool_output(text, 3, &log).unwrap();
    assert_eq!(
        g,
        [
            Gain {
                gain: 3.05,
                peak: 0.5
            },
            Gain {
                gain: -1.5,
                peak: 1.0
            },
            Gain {
                gain: 0.75,
                peak: 1.0
            },
        ]
    );
    // Fewer lines than files.
    assert_eq!(
        parse_tool_output(b"File\tMP3 gain\n", 1, &log),
        Err(RgError::Local("mp3gain failed".into()))
    );
}

#[test]
fn metaflac_backend_parses_gains() {
    let output = "01.flac: -1.234567 0.123456 1.987654 0.456789\n\
                  Artist: 02.flac: -1.234567 0.123456 -1.987654 0.987654\n";
    assert_eq!(
        parse_metaflac_gains(output).unwrap(),
        [
            [-1.234567, 0.123456, 1.987654, 0.456789],
            [-1.234567, 0.123456, -1.987654, 0.987654]
        ]
    );
}

fn task_of(formats: &[&str]) -> GainTask {
    GainTask {
        items: vec![],
        files: formats
            .iter()
            .map(|f| TaskItem {
                path: PathBuf::from("/nonexistent"),
                format: f.to_string(),
                name: "x".into(),
            })
            .collect(),
        album: Some("a".into()),
        target_level: 89.0,
        peak_method: None,
        r128: false,
        backend_name: "metaflac",
        album_gain: None,
        track_gains: None,
    }
}

#[test]
fn metaflac_backend_cannot_compute_album_gain_with_mixed_formats() {
    let b = MetaflacBackend {
        command: "/nonexistent/metaflac".into(),
    };
    let mut task = task_of(&["FLAC", "MP3", "FLAC"]);
    b.compute_album_gain(&mut task, &quiet_log()).unwrap();
    assert!(task.track_gains.is_none());
    assert!(task.album_gain.is_none());
}

// ------------------------------------------------------------- the plugin

struct Env {
    dir: TempDir,
    cfg: Config,
    lib: Library,
    music: PathBuf,
}

impl Env {
    fn new(rg: &str) -> Env {
        media::clear_write_listeners();
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str("[replaygain]").unwrap());
        cfg.set("import.write", Y::Bool(true));
        let y: Y = serde_yaml::from_str(rg).unwrap();
        for (k, v) in y.as_mapping().unwrap() {
            cfg.set(&format!("replaygain.{}", k.as_str().unwrap()), v.clone());
        }
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env {
            dir,
            cfg,
            lib,
            music,
        }
    }

    fn set(&mut self, key: &str, v: Y) {
        self.cfg.set(&format!("replaygain.{key}"), v);
    }

    fn plugin(&self) -> ReplayGain {
        ReplayGain::new(&self.cfg).unwrap()
    }

    /// An album of `discs` × `tracks` white-noise files (each louder than
    /// the last) of this extension, without gains.
    fn add_album(&self, tracks: usize, discs: usize, ext: &str) -> Album {
        let mut items = vec![];
        let n0 = std::fs::read_dir(&self.music).unwrap().count();
        for d in 1..=discs {
            for t in 1..=tracks {
                let n = n0 + items.len();
                let p = self.music.join(format!("{n}.{ext}"));
                let amp = 0.1 + 0.05 * (items.len() % 8) as f64;
                let ok = Command::new("ffmpeg")
                    .args(["-loglevel", "error", "-f", "lavfi", "-i"])
                    .arg(format!("anoisesrc=d=0.5:a={amp}:seed={n}"))
                    .arg(&p)
                    .status()
                    .is_ok_and(|s| s.success());
                assert!(ok, "ffmpeg made {}", p.display());
                let mut it = media::read(&p).unwrap();
                it.set_str("title", format!("t{n}"));
                it.set_str("album", format!("album {n0}"));
                it.set_int("track", t as i64);
                it.set_int("disc", d as i64);
                for f in RG_FIELDS {
                    it.set(f, Value::Null);
                }
                media::write_item(&mut it).unwrap();
                items.push(it);
            }
        }
        self.lib.add_album(&mut items).unwrap()
    }

    fn run(&self, args: &[&str]) {
        let mut opts = CmdOpts::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match *a {
                "-a" => opts.album = true,
                "-f" => opts.force = true,
                "-w" => opts.write = Some(true),
                "-W" => opts.write = Some(false),
                "-t" => opts.threads = it.next().map(|n| n.parse().unwrap()),
                q => opts.query.push(q.to_string()),
            }
        }
        let p = self.plugin();
        crate::logging::capture(|| p.command(&self.cfg, &self.lib, opts))
            .0
            .unwrap();
    }

    fn items(&self) -> Vec<Item> {
        self.lib
            .items(&Query::parse(Kind::Item, &[] as &[String]).unwrap())
            .unwrap()
    }

    fn reload(&self, item: &Item) -> Item {
        self.lib.get_item(item.id.unwrap()).unwrap().unwrap()
    }
}

const RG_FIELDS: [&str; 6] = [
    "rg_track_gain",
    "rg_track_peak",
    "rg_album_gain",
    "rg_album_peak",
    "r128_track_gain",
    "r128_album_gain",
];

fn tag(item: &Item, f: &str) -> Option<f64> {
    let it = media::read(item.path()).unwrap();
    opt_float(&it, f)
}

fn album_items(env: &Env, a: &Album) -> Vec<Item> {
    env.lib.album_items(a.id.unwrap()).unwrap()
}

macro_rules! need_ffmpeg {
    () => {
        if !have("ffmpeg", "-version") {
            eprintln!("skipped: no ffmpeg");
            return;
        }
    };
}

/// `ReplayGainCliTest` and `R128Test`, for the ffmpeg backend.
#[test]
fn cli_saves_track_gain() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(2, 1, "mp3");
    for item in env.items() {
        assert_eq!(opt_float(&item, "rg_track_peak"), None);
        assert_eq!(tag(&item, "rg_track_gain"), None);
    }
    env.run(&[]);
    for item in env.items() {
        let peak = opt_float(&item, "rg_track_peak").unwrap();
        let gain = opt_float(&item, "rg_track_gain").unwrap();
        assert!((tag(&item, "rg_track_peak").unwrap() - peak).abs() < 1e-6);
        assert!((tag(&item, "rg_track_gain").unwrap() - gain).abs() < 1e-2);
    }
}

#[test]
fn cli_skips_calculated_tracks() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    let a = env.add_album(1, 1, "mp3");
    env.run(&[]);
    let mut item = album_items(&env, &a).remove(0);
    assert!(opt_float(&item, "rg_track_gain").is_some());
    assert!(opt_float(&item, "rg_track_peak").is_some());
    assert_eq!(opt_float(&item, "r128_track_gain"), None);
    let gain = opt_float(&item, "rg_track_gain").unwrap() + 1.0;
    let peak = opt_float(&item, "rg_track_peak").unwrap() + 1.0;
    item.set("rg_track_gain", Value::Float(gain));
    item.set("rg_track_peak", Value::Float(peak));
    env.lib.store(&item).unwrap();
    env.run(&[]);
    let item = env.reload(&item);
    assert_eq!(opt_float(&item, "rg_track_gain"), Some(gain));
    assert_eq!(opt_float(&item, "rg_track_peak"), Some(peak));
}

#[test]
fn cli_saves_album_gain_to_file() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(2, 1, "mp3");
    env.run(&["-a"]);
    let gains: Vec<f64> = env
        .items()
        .iter()
        .map(|i| tag(i, "rg_album_gain").unwrap())
        .collect();
    let peaks: Vec<f64> = env
        .items()
        .iter()
        .map(|i| tag(i, "rg_album_peak").unwrap())
        .collect();
    assert_eq!(gains[0], gains[1]);
    assert_eq!(peaks[0], peaks[1]);
    assert_ne!(gains[0], 0.0);
    assert_ne!(peaks[0], 0.0);
    // The album's peak is its loudest track's.
    let track_peaks: Vec<f64> = env
        .items()
        .iter()
        .map(|i| opt_float(i, "rg_track_peak").unwrap())
        .collect();
    let max = track_peaks.iter().cloned().fold(f64::MIN, f64::max);
    assert_eq!(opt_float(&env.items()[0], "rg_album_peak"), Some(max));
}

#[test]
fn targetlevel_has_effect() {
    need_ffmpeg!();
    let mut env = Env::new("{backend: ffmpeg}");
    let a = env.add_album(1, 1, "mp3");
    let item = album_items(&env, &a).remove(0);
    let mut analyse = |level: i64| {
        env.set("targetlevel", Y::from(level));
        env.run(&["-f"]);
        opt_float(&env.reload(&item), "rg_track_gain").unwrap()
    };
    let g84 = analyse(84);
    let g89 = analyse(89);
    assert!((g89 - g84 - 5.0).abs() < 1e-9);
}

#[test]
fn per_disc() {
    need_ffmpeg!();
    let mut env = Env::new("{backend: ffmpeg}");
    let a = env.add_album(2, 3, "mp3");
    env.set("per_disc", Y::Bool(true));
    env.run(&["-a"]);
    let items = album_items(&env, &a);
    for item in &items {
        assert!(opt_float(item, "rg_track_gain").is_some());
        assert!(opt_float(item, "rg_album_gain").is_some());
    }
    // Each disc has its own album gain.
    let disc_gain = |d: i64| {
        items
            .iter()
            .filter(|i| i.int("disc") == d)
            .map(|i| opt_float(i, "rg_album_gain").unwrap())
            .collect::<Vec<_>>()
    };
    for d in 1..=3 {
        let g = disc_gain(d);
        assert_eq!(g[0], g[1]);
    }
    assert_ne!(disc_gain(1)[0], disc_gain(2)[0]);
}

#[test]
fn threads_give_the_same_gains() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(2, 1, "mp3");
    env.add_album(2, 1, "mp3");
    env.run(&["-a", "-t", "0"]);
    let serial: Vec<_> = env
        .items()
        .iter()
        .map(|i| opt_float(i, "rg_album_gain"))
        .collect();
    env.run(&["-a", "-f", "-t", "3"]);
    let pooled: Vec<_> = env
        .items()
        .iter()
        .map(|i| opt_float(i, "rg_album_gain"))
        .collect();
    assert_eq!(serial, pooled);
    assert!(serial.iter().all(Option::is_some));
}

#[test]
fn nowrite_leaves_files_alone() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(1, 1, "mp3");
    env.run(&["-W"]);
    let item = env.items().remove(0);
    assert!(opt_float(&item, "rg_track_gain").is_some());
    assert_eq!(tag(&item, "rg_track_gain"), None);
}

/// Wrong-typed and calculated tags (`R128Test`).
#[test]
fn r128_cli_does_not_skip_wrong_tag_type_and_clears_it() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    let a_rg = env.add_album(1, 1, "mp3");
    let a_r128 = env.add_album(1, 1, "opus");
    let mut item_rg = album_items(&env, &a_rg).remove(0);
    let mut item_r128 = album_items(&env, &a_r128).remove(0);
    item_rg.set("r128_track_gain", Value::Float(0.0));
    env.lib.store(&item_rg).unwrap();
    item_r128.set("rg_track_gain", Value::Float(0.0));
    item_r128.set("rg_track_peak", Value::Float(42.0));
    env.lib.store(&item_r128).unwrap();
    env.run(&[]);
    let item_rg = env.reload(&item_rg);
    let item_r128 = env.reload(&item_r128);
    assert!(opt_float(&item_rg, "rg_track_gain").is_some());
    assert!(opt_float(&item_rg, "rg_track_peak").is_some());
    assert_eq!(opt_float(&item_rg, "r128_track_gain"), None);
    assert!(opt_float(&item_r128, "r128_track_gain").is_some());
    assert_eq!(opt_float(&item_r128, "rg_track_gain"), None);
    assert_eq!(opt_float(&item_r128, "rg_track_peak"), None);
}

#[test]
fn r128_cli_writes_only_r128_tags() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(2, 1, "opus");
    env.run(&["-a"]);
    for item in env.items() {
        assert_eq!(tag(&item, "rg_track_gain"), None);
        assert_eq!(tag(&item, "rg_album_gain"), None);
        assert!(tag(&item, "r128_track_gain").is_some());
        assert!(tag(&item, "r128_album_gain").is_some());
    }
}

#[test]
fn r128_targetlevel_has_effect() {
    need_ffmpeg!();
    let mut env = Env::new("{backend: ffmpeg}");
    env.add_album(1, 1, "opus");
    let item = env.items().remove(0);
    let mut analyse = |level: i64| {
        env.set("r128_targetlevel", Y::from(level));
        env.run(&["-f"]);
        opt_float(&env.reload(&item), "r128_track_gain").unwrap()
    };
    assert_ne!(analyse(84), analyse(89));
}

#[test]
fn r128_cli_skips_calculated_tracks() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(1, 1, "opus");
    env.run(&[]);
    let mut item = env.items().remove(0);
    assert!(opt_float(&item, "r128_track_gain").is_some());
    assert_eq!(opt_float(&item, "rg_track_gain"), None);
    let g = opt_float(&item, "r128_track_gain").unwrap() + 1.0;
    item.set("r128_track_gain", Value::Float(g));
    env.lib.store(&item).unwrap();
    env.run(&[]);
    assert_eq!(opt_float(&env.reload(&item), "r128_track_gain"), Some(g));
}

#[test]
fn r128_for_other_formats() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg, r128: [MP3]}");
    env.add_album(1, 1, "mp3");
    env.run(&[]);
    let item = env.items().remove(0);
    assert!(opt_float(&item, "r128_track_gain").is_some());
    assert_eq!(opt_float(&item, "rg_track_gain"), None);
}

#[test]
fn incompatible_formats_are_skipped() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    let mp3 = env.add_album(1, 1, "mp3");
    let opus = env.add_album(1, 1, "opus");
    let mut o = album_items(&env, &opus).remove(0);
    o.set_int("album_id", mp3.id.unwrap());
    env.lib.store(&o).unwrap();
    env.run(&["-a"]);
    assert!(
        env.items()
            .iter()
            .all(|i| opt_float(i, "rg_album_gain").is_none())
    );
}

/// `ImportTest`: the import stage gives albums their gains.
#[test]
fn import_stage_computes_gains() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    for parallel in [false, true] {
        let mut cfg = env.cfg.clone();
        cfg.set("replaygain.parallel_on_import", Y::Bool(parallel));
        let p = ReplayGain::new(&cfg).unwrap();
        let mut album = env.add_album(2, 1, "mp3");
        let mut items = album_items(&env, &album);
        let ctx = PluginCtx {
            cfg: &cfg,
            lib: &env.lib,
            write: false,
        };
        let mut task = ImportTask {
            album: Some(&mut album),
            items: &mut items,
            paths: &[],
            asis: true,
            op: crate::fileops::FileOp::Copy,
        };
        p.import_stage(&ctx, &mut task).unwrap();
        for item in env.lib.album_items(album.id.unwrap()).unwrap() {
            assert!(opt_float(&item, "rg_track_gain").is_some());
            assert!(opt_float(&item, "rg_album_gain").is_some());
            // Import doesn't write; the importer does, if it writes.
            assert_eq!(tag(&item, "rg_track_gain"), None);
        }
    }
    // `auto: no`
    let mut cfg = env.cfg.clone();
    cfg.set("replaygain.auto", Y::Bool(false));
    let p = ReplayGain::new(&cfg).unwrap();
    let item = env.add_album(1, 1, "mp3");
    let mut items = album_items(&env, &item);
    let ctx = PluginCtx {
        cfg: &cfg,
        lib: &env.lib,
        write: false,
    };
    let mut task = ImportTask {
        album: None,
        items: &mut items,
        paths: &[],
        asis: true,
        op: crate::fileops::FileOp::Copy,
    };
    p.import_stage(&ctx, &mut task).unwrap();
    let first = env.lib.album_items(item.id.unwrap()).unwrap().remove(0);
    assert_eq!(opt_float(&first, "rg_track_gain"), None);
}

// ---------------------------------------------------- settings and errors

#[test]
fn unsupported_settings() {
    let env = Env::new("{backend: bogus}");
    let e = ReplayGain::new(&env.cfg).err().unwrap();
    assert_eq!(
        e.to_string(),
        "Selected ReplayGain backend bogus is not supported. Please select one of: command, \
         metaflac, gstreamer, audiotools, ffmpeg"
    );
    let env = Env::new("{backend: ffmpeg, peak: loud}");
    let e = ReplayGain::new(&env.cfg).err().unwrap();
    assert_eq!(
        e.to_string(),
        "Selected ReplayGain peak method loud is not supported. Please select one of: true, sample"
    );
    let env = Env::new("{backend: gstreamer}");
    let e = ReplayGain::new(&env.cfg).err().unwrap();
    assert_eq!(
        e.to_string(),
        "replaygain initialization failed: Failed to load GStreamer: python-gi not found"
    );
    let env = Env::new("{backend: audiotools}");
    let e = ReplayGain::new(&env.cfg).err().unwrap();
    assert_eq!(
        e.to_string(),
        "replaygain initialization failed: Failed to load audiotools: audiotools not found"
    );
}

#[test]
fn command_backend_setup_errors() {
    let env = Env::new("{}");
    let e = ReplayGain::new(&env.cfg).err().unwrap();
    assert_eq!(
        e.to_string(),
        "replaygain initialization failed: replaygain.command must be one of {'mp3rgain', \
         'aacgain', 'mp3gain'}, not ''"
    );
    let env = Env::new("{command: /nonexistent/mp3gain}");
    let e = ReplayGain::new(&env.cfg).err().unwrap();
    assert_eq!(
        e.to_string(),
        "replaygain initialization failed: replaygain command not found: /nonexistent/mp3gain"
    );
}

#[test]
fn defaults() {
    let cfg = Config::defaults();
    assert_eq!(cfg.str("replaygain.backend"), "command");
    assert_eq!(cfg.f64("replaygain.targetlevel"), 89.0);
    assert_eq!(cfg.f64("replaygain.r128_targetlevel"), 107.0 - 23.0);
    assert_eq!(cfg.list("replaygain.r128"), ["Opus"]);
    assert_eq!(cfg.str("replaygain.peak"), "true");
    assert_eq!(cfg.f64("replaygain.threads") as usize, cpu_count());
}

/// A fake mp3gain: logs its arguments and prints canned `-o` output for
/// each file (and an album line).
fn fake_mp3gain(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    let script = format!(
        "#!/bin/sh\n\
         echo \"$@\" >> {log}\n\
         printf 'File\\tMP3 gain\\tdB gain\\tMax Amplitude\\tMax global_gain\\tMin global_gain\\n'\n\
         n=0\n\
         for a in \"$@\"; do case \"$a\" in /*) n=$((n+1)); printf '%s\\t2\\t%s.5\\t16384.0\\t210\\t100\\n' \"$a\" \"$n\";; esac; done\n\
         printf '\"Album\"\\t1\\t-7.25\\t32768.0\\t210\\t100\\n'\n",
        log = dir.join("args.log").display()
    );
    std::fs::write(&p, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn command_backend_with_fake_tool() {
    need_ffmpeg!();
    let mut env = Env::new("{}");
    let tool = fake_mp3gain(env.dir.path(), "mp3gain");
    env.set("command", Y::String(tool.display().to_string()));
    env.set("targetlevel", Y::from(92.5));
    let a = env.add_album(2, 1, "mp3");
    env.run(&["-a", "-t", "0"]);
    let items = album_items(&env, &a);
    assert_eq!(opt_float(&items[0], "rg_track_gain"), Some(1.5));
    assert_eq!(opt_float(&items[1], "rg_track_gain"), Some(2.5));
    assert_eq!(opt_float(&items[0], "rg_track_peak"), Some(0.5));
    assert_eq!(opt_float(&items[1], "rg_album_gain"), Some(-7.25));
    assert_eq!(opt_float(&items[1], "rg_album_peak"), Some(1.0));
    let args = std::fs::read_to_string(env.dir.path().join("args.log")).unwrap();
    // -d is int(targetlevel - 89); -k is noclip.
    assert!(args.starts_with("-o -s s -k -d 3 /"), "{args}");

    // mp3gain doesn't do Opus: the track fails, the album is skipped.
    env.set("noclip", Y::Bool(false));
    let o = env.add_album(1, 1, "opus");
    env.set("r128", Y::Sequence(vec![]));
    env.run(&["-t", "0", "-f"]);
    env.run(&["-a", "-t", "0", "-f"]);
    let item = album_items(&env, &o).remove(0);
    assert_eq!(opt_float(&item, "rg_track_gain"), None);
    let args = std::fs::read_to_string(env.dir.path().join("args.log")).unwrap();
    assert!(args.lines().last().unwrap().contains(" -c -d 3 "), "{args}");
}

#[test]
fn metaflac_backend_runs() {
    need_ffmpeg!();
    if !have("metaflac", "--version") {
        eprintln!("skipped: no metaflac");
        return;
    }
    let env = Env::new("{backend: metaflac}");
    let a = env.add_album(2, 1, "flac");
    env.run(&["-a", "-t", "0"]);
    let items = album_items(&env, &a);
    for item in &items {
        assert!(opt_float(item, "rg_track_gain").is_some());
        assert!(opt_float(item, "rg_album_gain").is_some());
    }
    assert_eq!(
        opt_float(&items[0], "rg_album_gain"),
        opt_float(&items[1], "rg_album_gain")
    );
}

#[test]
fn debug_messages() {
    need_ffmpeg!();
    let env = Env::new("{backend: ffmpeg}");
    env.add_album(1, 1, "opus");
    let p = env.plugin();
    p.log.set_base_level(DEBUG);
    let opts = CmdOpts {
        album: true,
        threads: Some(0),
        ..Default::default()
    };
    let (r, lines) = crate::logging::capture(|| p.command(&env.cfg, &env.lib, opts));
    r.unwrap();
    let text = lines.join("\n");
    assert!(
        text.starts_with("replaygain: Analyzing 1 albums ~ ffmpeg backend..."),
        "{text}"
    );
    assert!(text.contains("ebur128=peak=none -f null -"), "{text}");
    assert!(text.contains(" blocks over "), "{text}");
    assert!(
        text.contains("replaygain: applied r128 album gain "),
        "{text}"
    );
    assert!(
        text.ends_with("replaygain: done analyzing  - album 0 - t0"),
        "{text}"
    );
}
