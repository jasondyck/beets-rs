//! Ported from beets' test/plugins/test_badfiles.py (the interactive prompt
//! needs stdin, so its automatic actions are tested instead).

use super::*;
use crate::autotag::{AlbumMatch, Recommendation, TrackMatch};
use crate::config::Config;
use crate::importer::{CandidateSource, ImportOptions, Importer};
use crate::library::Library;
use crate::model::Item;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    cfg.set("plugins", serde_yaml::from_str("[badfiles]").unwrap());
    cfg.set("import.write", serde_yaml::Value::Bool(false));
    cfg.set("import.resume", "no".into());
    cfg.set("import.incremental", serde_yaml::Value::Bool(false));
    cfg.set("import.duplicate_action", "keep".into());
    cfg.set("ui.color", serde_yaml::Value::Bool(false));
    apply_cfg(&mut cfg, yaml_cfg);
    let lib = Library::in_memory().unwrap();
    (dir, cfg, lib)
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
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

fn item_at(path: &Path) -> Item {
    let mut i = Item::new_item();
    i.set_path(path);
    i
}

#[test]
fn quiet_import_skips_prompt() {
    let (_d, cfg, _lib) = env("{}");
    let b = load(&cfg);
    // No automatic action (the default is ask) and quiet: continue.
    assert_eq!(
        b.before_choice(&[vec!["bad: error".into()]], true).unwrap(),
        BeforeChoice::Continue
    );
}

#[test]
fn automatic_error_actions() {
    let failed = vec![vec![
        "x.mp3: checker exited with status 1".to_string(),
        "  error: broken".to_string(),
    ]];
    for (action, expected) in [
        ("skip", BeforeChoice::Skip),
        ("continue", BeforeChoice::Continue),
        ("abort", BeforeChoice::Abort),
    ] {
        let (_d, cfg, _lib) = env(&format!(
            "{{badfiles: {{import_action_on_error: {action}}}}}"
        ));
        let b = load(&cfg);
        assert_eq!(
            b.before_choice(&failed, false).unwrap(),
            expected,
            "{action}"
        );
    }
}

#[test]
fn warnings_and_errors_take_their_own_actions() {
    let warning = vec![vec![
        "x.mp3: checker found 1 errors or warnings".to_string(),
    ]];
    let only_warning = vec![vec!["  warning: only a warning".to_string()]];
    let (_d, cfg, _lib) =
        env("{badfiles: {import_action_on_warning: skip, import_action_on_error: abort}}");
    let b = load(&cfg);
    assert_eq!(
        b.before_choice(&only_warning, false).unwrap(),
        BeforeChoice::Skip
    );
    // The counts line names both "errors" and "warnings", so it counts as
    // an error (beets' substring check).
    assert_eq!(
        b.before_choice(&warning, false).unwrap(),
        BeforeChoice::Abort
    );
    // Errors take precedence over warnings.
    let mut both = warning.clone();
    both.push(vec![
        "x.mp3: checker exited with status 1".to_string(),
        "  error: broken".to_string(),
    ]);
    assert_eq!(b.before_choice(&both, false).unwrap(), BeforeChoice::Abort);
    // Nothing failed: carry on.
    assert_eq!(b.before_choice(&[], false).unwrap(), BeforeChoice::Continue);
}

#[test]
fn check_item_runs_the_configured_checker() {
    let (d, _cfg, _lib) = env("{}");
    let checker = script(d.path(), "checker", "echo WARNING: broken; exit 1");
    let (_d2, cfg, _lib) = env(&format!(
        "{{badfiles: {{commands: {{mp3: '{}'}}}}}}",
        checker.display()
    ));
    let mp3 = d.path().join("x.mp3");
    if !make_mp3(d.path(), "x.mp3", &[("artist", "A"), ("title", "T")]) {
        return;
    }
    let b = load(&cfg);
    let mut out = vec![];
    let lines = b.check_item(&item_at(&mp3), false, &mut out);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("checker exited with status 1"),
        "{lines:?}"
    );
    assert!(lines[1].contains("WARNING: broken"), "{lines:?}");
}

#[test]
fn check_item_verbose_success() {
    let (d, cfg, _lib) = env(&format!(
        "{{badfiles: {{commands: {{mp3: '{}'}}}}}}",
        "/bin/true"
    ));
    let mp3 = d.path().join("x.mp3");
    if !make_mp3(d.path(), "x.mp3", &[("artist", "A"), ("title", "T")]) {
        return;
    }
    let b = load(&cfg);
    let mut out = vec![];
    assert!(b.check_item(&item_at(&mp3), false, &mut out).is_empty());
    let lines = b.check_item(&item_at(&mp3), true, &mut out);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with(": ok"), "{lines:?}");
}

#[test]
fn check_item_missing_file() {
    let (d, cfg, _lib) = env(&format!(
        "{{badfiles: {{commands: {{mp3: '{}'}}}}}}",
        "/bin/true"
    ));
    let missing = d.path().join("missing.mp3");
    let b = load(&cfg);
    let mut out = vec![];
    let lines = b.check_item(&item_at(&missing), false, &mut out);
    assert!(lines.is_empty());
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("file does not exist"), "{text}");
}

struct NoCandidates;

impl CandidateSource for NoCandidates {
    fn album(&self, _items: &[Item]) -> anyhow::Result<(Vec<AlbumMatch>, Recommendation)> {
        Ok((vec![], Recommendation::None))
    }
    fn item(&self, _item: &Item) -> anyhow::Result<(Vec<TrackMatch>, Recommendation)> {
        Ok((vec![], Recommendation::None))
    }
}

/// The upstream on-import test, with the automatic abort action instead of
/// answering "aBort" at the prompt.
#[test]
fn abort_on_import_error() {
    let (d, cfg, lib) = env("{}");
    let checker = script(d.path(), "checker", "echo 'error: broken'; exit 1");
    let mut cfg = cfg;
    cfg.set("badfiles.check_on_import", serde_yaml::Value::Bool(true));
    cfg.set(
        "badfiles.commands",
        serde_yaml::from_str(&format!("{{mp3: '{}'}}", checker.display())).unwrap(),
    );
    cfg.set("badfiles.import_action_on_error", "abort".into());
    let src = d.path().join("src/album");
    if !make_mp3(
        &src,
        "1.mp3",
        &[("artist", "A"), ("album", "Al"), ("title", "T")],
    ) {
        return;
    }
    let plugins = crate::plugins::load(&cfg).unwrap();
    let opts = ImportOptions::from_config(&cfg);
    let mut imp = Importer::new(&cfg, &lib, opts, &plugins)
        .unwrap()
        .with_source(Arc::new(NoCandidates));
    imp.run(std::slice::from_ref(&src)).unwrap();
    let items: Vec<Item> = lib
        .items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
        .unwrap();
    assert!(items.is_empty());
}
