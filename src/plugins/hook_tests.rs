//! Ported from beets' `test/plugins/test_hook.py`.

use super::*;
use crate::config::Config;
#[cfg(unix)]
use crate::events::Arg;
use crate::events::Event;
#[cfg(unix)]
use crate::model::Item;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn env(hooks: &str) -> (TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.path().join("cfg");
    cfg.set("directory", dir.path().join("lib").to_string_lossy().into());
    cfg.set("plugins", serde_yaml::from_str("[hook]").unwrap());
    if !hooks.trim().is_empty() {
        cfg.set("hook.hooks", serde_yaml::from_str(hooks).unwrap());
    }
    (dir, cfg)
}

fn hook(event: &str, command: &str) -> String {
    format!(
        "- event: {event}\n  command: {}",
        serde_yaml::to_string(command).unwrap()
    )
}

#[cfg(unix)]
fn send_path(event: &'static str, path: &Path) {
    crate::events::send(event, &[("path", Arg::Path(path))]);
}

#[test]
fn hook_empty_command() {
    let (_d, cfg) = env(&hook("write", ""));
    let (_p, lines) = crate::logging::capture(|| {
        super::load(&cfg).unwrap();
        crate::events::send("write", &[]);
    });
    assert!(
        lines.iter().any(|l| l.contains("invalid command \"\"")),
        "{lines:?}"
    );
}

#[test]
#[cfg(unix)]
fn hook_non_zero_exit() {
    let (_d, cfg) = env(&hook("write", "sh -c \"exit 1\""));
    let (_p, lines) = crate::logging::capture(|| {
        super::load(&cfg).unwrap();
        crate::events::send("write", &[]);
    });
    assert!(
        lines
            .iter()
            .any(|l| l.contains("hook for write exited with status 1")),
        "{lines:?}"
    );
}

#[test]
fn hook_non_existent_command() {
    let (_d, cfg) = env(&hook("write", "non-existent-command"));
    let (_p, lines) = crate::logging::capture(|| {
        super::load(&cfg).unwrap();
        crate::events::send("write", &[]);
    });
    let text = lines.join("\n");
    assert!(text.contains("hook for write failed: "), "{text}");
    assert!(text.contains("Err"), "{text}");
    assert!(text.contains("file"), "{text}");
}

/// The command for each event touches a path; both events run.
#[test]
#[cfg(unix)]
fn hook_commands_run_per_event() {
    let (d, mut cfg) = env("");
    cfg.set(
        "hook.hooks",
        serde_yaml::from_str(&format!(
            "- event: write\n  command: touch {}/write\n- event: after_write\n  command: touch {}/after_write\n",
            d.path().display(),
            d.path().display()
        ))
        .unwrap(),
    );
    super::load(&cfg).unwrap();
    crate::events::send("write", &[]);
    crate::events::send("after_write", &[]);
    assert!(d.path().join("write").is_file());
    assert!(d.path().join("after_write").is_file());
}

#[test]
#[cfg(unix)]
fn hook_event_substitution() {
    let (d, mut cfg) = env("");
    cfg.set(
        "hook.hooks",
        serde_yaml::from_str(&format!(
            "- event: write\n  command: touch {}/{{event}}\n",
            d.path().display()
        ))
        .unwrap(),
    );
    super::load(&cfg).unwrap();
    crate::events::send("write", &[]);
    assert!(d.path().join("write").is_file());
}

#[test]
#[cfg(unix)]
fn hook_argument_substitution() {
    let (d, cfg) = env(&hook("after_write", "touch {path}"));
    let target = d.path().join("touched");
    super::load(&cfg).unwrap();
    send_path("after_write", &target);
    assert!(target.is_file());
}

#[test]
#[cfg(unix)]
fn hook_bytes_interpolation() {
    let (d, cfg) = env(&hook("after_write", "touch '{path}'"));
    // A non-ASCII path, as a bytes kwarg in beets.
    let target = d.path().join("bé ö");
    super::load(&cfg).unwrap();
    send_path("after_write", &target);
    assert!(target.is_file());
}

#[test]
#[cfg(unix)]
fn hook_item_and_task_arguments() {
    let (d, mut cfg) = env("");
    let out = d.path().join("titles");
    cfg.set(
        "hook.hooks",
        serde_yaml::from_str(&format!(
            "- event: write\n  command: \"sh -c 'echo {{item.title}}/{{item.album}} > {}'\"\n",
            out.display()
        ))
        .unwrap(),
    );
    let mut item = Item::new_item();
    item.set_str("title", "A Title");
    item.set_str("album", "An Album");
    let path = PathBuf::from("/tmp/x.mp3");
    super::load(&cfg).unwrap();
    crate::events::send(
        "write",
        &[("item", Arg::Item(&item)), ("path", Arg::Path(&path))],
    );
    let text = std::fs::read_to_string(&out).unwrap();
    assert_eq!(text.trim(), "A Title/An Album");
}

#[test]
fn hook_plugin_defined_event_loads_and_registers() {
    let event = "alternatives.item_updated";
    assert!(crate::events::ALL_EVENTS.contains(&event));
    let (_d, cfg) = env(&hook(event, "true"));
    super::load(&cfg).unwrap();
    let ev = Event {
        name: "alternatives.item_updated",
        args: &[],
    };
    // A listener for the custom event is registered (and only that one).
    let hooks = Hooks {
        hooks: vec![(event.into(), "true".into())],
        format_item: "$title".into(),
        format_album: "$album".into(),
        log: crate::plugins::PluginLog::new("hook"),
    };
    assert!(hooks.wants(ev.name));
    assert!(!hooks.wants("write"));
}

#[test]
fn hook_invalid_event_is_an_error() {
    let (_d, cfg) = env(&hook("no_such_event", "true"));
    let err = match super::load(&cfg) {
        Ok(_) => panic!("an invalid event must fail to load"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("no_such_event"), "{err}");
}
