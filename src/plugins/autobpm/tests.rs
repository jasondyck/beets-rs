//! Tests for the `autobpm` plugin. beets measures its fixture track
//! (`bpm.mp3`, 117 BPM) with librosa; this port's librosa pipeline is
//! checked against the tempi librosa 1.0.1 gives for generated click
//! tracks (107/99/117/152 for 90/100/120/150 BPM) and for that fixture.

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use serde_yaml::Value as Y;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A click track: a 1 kHz beep for 40 ms every `period` seconds.
fn click_track(dir: &Path, name: &str, bpm: f64) -> PathBuf {
    let period = 60.0 / bpm;
    let path = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "aevalsrc='if(lt(mod(t,{period}),0.04),0.9*sin(2*PI*1000*t),0)':s=44100:d=8"
        ))
        .args(["-ac", "1"])
        .arg(&path)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());
    path
}

fn measured(path: &Path) -> f64 {
    let samples = librosa::load(path).unwrap();
    librosa::beat_track_tempo(&samples, librosa::SAMPLE_RATE)
}

#[test]
fn measures_steady_tempi_like_librosa() {
    let dir = tempfile::tempdir().unwrap();
    // librosa 1.0.1's own estimates for these click tracks.
    for (bpm, expected) in [(90.0, 89), (100.0, 99), (120.0, 117), (150.0, 152)] {
        let path = click_track(dir.path(), &format!("{bpm}.wav"), bpm);
        let got = py_round(measured(&path));
        assert_eq!(got, expected, "{bpm} BPM click track");
    }
}

#[test]
fn measures_the_beets_fixture_like_librosa() {
    // beets' test/rsrc/bpm.mp3, which librosa measures at 117.45 → 117.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/bpm.mp3");
    assert_eq!(py_round(measured(&path)), 117);
}

#[test]
fn resamples_like_resampy_kaiser_fast() {
    // resampy 0.4.3, filter="kaiser_fast", for this input.
    let x: Vec<f32> = vec![
        0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.4, 0.3, 0.2, 0.1, -0.1, -0.2, -0.3, -0.4, -0.5, -0.4,
        -0.3, -0.2, -0.1, 0.0,
    ];
    let up = librosa::resample_kaiser_fast(&x, 8000, 22050);
    assert_eq!(up.len(), 55);
    let expected_up = [
        0.004948531f32, 0.027862223f32, 0.06240097f32, 0.10456053f32,
        0.14849749f32, 0.18889292f32, 0.22325069f32, 0.2530408f32,
        0.28291714f32, 0.3180654f32, 0.36078846f32, 0.4081081f32,
        0.4518889f32, 0.48174897f32, 0.48945838f32, 0.47261143f32,
    ];
    for (a, b) in up.iter().zip(expected_up.iter()) {
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }
    let down = librosa::resample_kaiser_fast(&x, 44100, 22050);
    assert_eq!(down.len(), 10);
    let expected_down = [
        0.019752286f32, 0.19042566f32, 0.41764447f32, 0.4180927f32,
        0.19343287f32, -0.063790016f32, -0.3196724f32, -0.45639962f32,
        -0.31583956f32, -0.08387089f32,
    ];
    for (a, b) in down.iter().zip(expected_down.iter()) {
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }
}

#[test]
fn silent_audio_has_no_tempo() {
    let samples = vec![0f32; 22050 * 2];
    assert_eq!(librosa::beat_track_tempo(&samples, librosa::SAMPLE_RATE), 0.0);
    assert!(librosa::onset_envelope(&samples, librosa::SAMPLE_RATE, 512, 2048)
        .iter()
        .all(|&v| v == 0.0));
}

#[test]
fn rounds_like_python() {
    assert_eq!(py_round(117.4), 117);
    assert_eq!(py_round(117.5), 118);
    assert_eq!(py_round(118.5), 118);
    assert_eq!(py_round(0.5), 0);
    assert_eq!(py_round(-0.5), 0);
    assert_eq!(py_round(2.5), 2);
}

#[test]
fn a_missing_file_is_an_error() {
    let err = librosa::load(Path::new("/nonexistent/file.mp3"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("No such file"), "{err}");
}

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set("import.write", Y::Bool(false));
    cfg.set("format_item", Y::String("$title".into()));
    let lib = Library::in_memory().unwrap();
    Env {
        _dir: dir,
        cfg,
        lib,
    }
}

fn add(env: &Env, path: &Path, bpm: Option<i64>) -> Item {
    let mut it = Item::new_item();
    it.set_str("title", "Song");
    it.set_path(path);
    if let Some(b) = bpm {
        it.set_int("bpm", b);
    }
    env.lib.add(&mut it).unwrap();
    it
}

fn run(env: &Env, items: &mut [Item], force: bool, quiet: bool) -> Result<()> {
    AutoBPMPlugin::new(&env.cfg).calculate_bpm(&env.cfg, &env.lib, items, false, force, quiet)
}

#[test]
fn sets_the_bpm() {
    let dir = tempfile::tempdir().unwrap();
    let path = click_track(dir.path(), "120.wav", 120.0);
    let env = env();
    let mut item = add(&env, &path, None);
    run(&env, std::slice::from_mut(&mut item), false, false).unwrap();
    assert_eq!(item.int("bpm"), 117);
    let stored = env.lib.get_item(item.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.int("bpm"), 117);
}

#[test]
fn keeps_an_existing_bpm_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    let path = click_track(dir.path(), "120.wav", 120.0);
    let env = env();
    let mut item = add(&env, &path, Some(10));
    run(&env, std::slice::from_mut(&mut item), false, false).unwrap();
    assert_eq!(item.int("bpm"), 10);
    run(&env, std::slice::from_mut(&mut item), true, false).unwrap();
    assert_eq!(item.int("bpm"), 117);
}

#[test]
fn the_deprecated_overwrite_option_is_force() {
    let mut cfg = Config::defaults();
    cfg.set("autobpm.overwrite", Y::Bool(true));
    assert!(AutoBPMPlugin::new(&cfg).force);
    let mut cfg = Config::defaults();
    cfg.set("autobpm.force", Y::Bool(true));
    assert!(AutoBPMPlugin::new(&cfg).force);
}

#[test]
fn a_missing_file_is_logged_and_skipped() {
    let env = env();
    let mut item = add(&env, Path::new("/nonexistent/file.mp3"), None);
    run(&env, std::slice::from_mut(&mut item), false, false).unwrap();
    assert_eq!(item.int("bpm"), 0);
}

#[test]
fn the_command_honours_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let a = click_track(dir.path(), "a.wav", 120.0);
    let b = click_track(dir.path(), "b.wav", 120.0);
    let env = env();
    add(&env, &a, None);
    add(&env, &b, None);
    let opts = CmdOpts {
        limit: Some(1),
        query: vec!["title:Song".into()],
        ..Default::default()
    };
    command(&env.cfg, &env.lib, &opts).unwrap();
    let items = env
        .lib
        .items(&Query::parse(Kind::Item, &[] as &[&str]).unwrap())
        .unwrap();
    assert_eq!(items.iter().filter(|i| i.int("bpm") != 0).count(), 1);
}
