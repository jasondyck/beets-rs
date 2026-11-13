//! Tests for the `absubmit` plugin: SHA-1, the extractor and the
//! submission against a fake AcousticBrainz server (beets' tests replace
//! the extractor with a fake binary the same way).

use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::Item;
use crate::plugins::requests::testserver::{Reply, serve};
use serde_yaml::Value as Y;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn sha1_matches_hashlib() {
    assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        sha1_hex(b"The quick brown fox jumps over the lazy dog"),
        "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
    );
    // A message over one block: the padding and length bytes.
    let long = vec![b'a'; 1000];
    assert_eq!(sha1_hex(&long), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
}

/// A fake extractor: writes `json` to the output file it's given.
fn extractor(dir: &Path, json: &str) -> PathBuf {
    let p = dir.join("streaming_extractor_music");
    std::fs::write(
        &p,
        format!("#!/bin/sh\nprintf '%s' '{json}' > \"$2\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// A fake extractor that fails.
fn failing_extractor(dir: &Path) -> PathBuf {
    let p = dir.join("failing_extractor");
    std::fs::write(&p, "#!/bin/sh\nexit 2\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

fn env(extractor_path: &Path, base_url: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    let libdir = dir.path().join("lib");
    std::fs::create_dir_all(&libdir).unwrap();
    cfg.set("directory", libdir.to_string_lossy().into());
    cfg.set(
        "absubmit.extractor",
        extractor_path.to_string_lossy().into(),
    );
    cfg.set("absubmit.base_url", Y::String(base_url.into()));
    cfg.set("format_item", Y::String("$title".into()));
    let lib = Library::in_memory().unwrap();
    Env {
        _dir: dir,
        cfg,
        lib,
    }
}

fn add(lib: &Library, dir: &Path, mbid: Option<&str>, probe: Option<f64>) -> Item {
    let file = dir.join("file.mp3");
    std::fs::write(&file, b"x").unwrap();
    let mut it = Item::new_item();
    it.set_str("title", "Song");
    it.set_path(&file);
    if let Some(id) = mbid {
        it.set_str("mb_trackid", id);
    }
    if let Some(p) = probe {
        it.set(PROBE_FIELD, crate::model::Value::Float(p));
    }
    lib.add(&mut it).unwrap();
    it
}

const ANALYSIS: &str = r#"{"metadata": {"version": {"essentia": "2.1"}}, "lowlevel": {"average_loudness": 0.8}}"#;

#[test]
fn extracts_and_submits() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), Some("mbid-1"), None);
    plugin
        .analyze_submit(&e.cfg, &e.lib, &item, false, false)
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/mbid-1/low-level");
    assert_eq!(
        requests[0].header("Content-Type"),
        Some("application/json")
    );
    let body: J = serde_json::from_slice(&requests[0].body).unwrap();
    let sha = sha1_hex(&std::fs::read(&ex).unwrap());
    assert_eq!(
        body["metadata"]["version"]["essentia_build_sha"],
        J::String(sha)
    );
    // The extractor's own data is kept.
    assert_eq!(body["metadata"]["version"]["essentia"], "2.1");
    assert_eq!(body["lowlevel"]["average_loudness"], 0.8);
}

#[test]
fn a_failing_submission_logs_the_message() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let server = serve(|_r| Reply {
        status: 400,
        content_type: "application/json",
        body: br#"{"message": "invalid analysis"}"#.to_vec(),
    });
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), Some("mbid-2"), None);
    // beets logs the error and carries on.
    plugin
        .analyze_submit(&e.cfg, &e.lib, &item, false, false)
        .unwrap();
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_broken_analysis_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), "not json");
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), Some("mbid-3"), None);
    assert!(
        plugin
            .get_analysis(&e.cfg, &e.lib, &item, false, false)
            .unwrap()
            .is_none()
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_failing_extractor_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let ex = failing_extractor(d.path());
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), Some("mbid-4"), None);
    assert!(
        plugin
            .get_analysis(&e.cfg, &e.lib, &item, false, false)
            .unwrap()
            .is_none()
    );
    assert!(server.requests().is_empty());
}

#[test]
fn an_item_without_an_mbid_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let ex = failing_extractor(d.path());
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), None, None);
    assert!(
        plugin
            .get_analysis(&e.cfg, &e.lib, &item, false, false)
            .unwrap()
            .is_none()
    );
    assert!(server.requests().is_empty());
}

#[test]
fn an_existing_analysis_is_not_redone() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), Some("mbid-5"), Some(0.5));
    assert!(
        plugin
            .get_analysis(&e.cfg, &e.lib, &item, false, false)
            .unwrap()
            .is_none()
    );
    // `-f` redoes it.
    assert!(
        plugin
            .get_analysis(&e.cfg, &e.lib, &item, true, false)
            .unwrap()
            .is_some()
    );
}

#[test]
fn pretend_does_not_run_the_extractor() {
    let d = tempfile::tempdir().unwrap();
    let ex = failing_extractor(d.path());
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    let item = add(&e.lib, d.path(), Some("mbid-6"), None);
    assert!(
        plugin
            .get_analysis(&e.cfg, &e.lib, &item, false, true)
            .unwrap()
            .is_none()
    );
    assert!(server.requests().is_empty());
}

#[test]
fn the_extractor_must_exist() {
    let mut cfg = Config::defaults();
    cfg.set("absubmit.extractor", Y::String("/nonexistent/extractor".into()));
    assert_eq!(
        ABSubmit::new(&cfg).err().expect("extractor is missing").to_string(),
        "Extractor command does not exist: /nonexistent/extractor."
    );
}

#[test]
fn the_base_url_must_be_http() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let mut cfg = Config::defaults();
    cfg.set("absubmit.extractor", ex.to_string_lossy().into());
    cfg.set("absubmit.base_url", Y::String("acousticbrainz.org".into()));
    assert_eq!(
        ABSubmit::new(&cfg).err().expect("base_url is not http").to_string(),
        "AcousticBrainz server base URL must start with an HTTP scheme"
    );
}

#[test]
fn a_base_url_gets_a_trailing_slash() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let mut cfg = Config::defaults();
    cfg.set("absubmit.extractor", ex.to_string_lossy().into());
    cfg.set("absubmit.base_url", Y::String("http://example.com/api".into()));
    let plugin = ABSubmit::new(&cfg).unwrap();
    assert_eq!(plugin.url, "http://example.com/api/{mbid}/low-level");
}

#[test]
fn a_missing_base_url_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let e = env(&ex, "");
    let plugin = ABSubmit::new(&e.cfg).unwrap();
    assert_eq!(
        plugin.check_configured().expect_err("no base_url").to_string(),
        "This plugin is deprecated since AcousticBrainz no longer accepts new submissions. \
         See the base_url configuration option."
    );
}

#[test]
fn the_command_lists_items_by_limit() {
    let d = tempfile::tempdir().unwrap();
    let ex = extractor(d.path(), ANALYSIS);
    let server = serve(|_r| Reply::json(serde_json::json!({})));
    let e = env(&ex, &server.base);
    let a = add(&e.lib, d.path(), Some("mbid-7"), None);
    let b = add(&e.lib, d.path(), Some("mbid-8"), None);
    let opts = CmdOpts {
        limit: Some(1),
        query: vec!["title:Song".into()],
        ..Default::default()
    };
    command(&e.cfg, &e.lib, &opts).unwrap();
    // The default item sort lists a before b; only the first is submitted.
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, vec![format!("/{}/low-level", a.str("mb_trackid"))]);
    assert_ne!(a.str("mb_trackid"), b.str("mb_trackid"));
}
