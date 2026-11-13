//! A port of beets 2.14's `permissions` plugin
//! (`beetsplug/permissions.py`): after an item or album is imported (and
//! when an album's art is set), set `permissions.file` on its files and
//! `permissions.dir` on their directories inside the library.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::PluginLog;
use serde_yaml::Value as Y;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct PermissionsPlugin;

impl crate::plugins::Plugin for PermissionsPlugin {
    fn name(&self) -> &'static str {
        "permissions"
    }
}

struct Perms {
    cfg: Config,
    log: PluginLog,
}

pub fn load(cfg: &Config) -> PermissionsPlugin {
    let log = PluginLog::new("permissions");
    log.set_base_level(crate::plugins::WARNING);
    crate::events::register(Rc::new(Perms {
        cfg: cfg.clone(),
        log,
    }));
    PermissionsPlugin
}

/// `convert_perm`: the setting as an octal number (an integer, as YAML
/// reads `644`, is read as its digits).
pub fn convert_perm(v: Option<&Y>) -> Option<u32> {
    let s = match v? {
        Y::Number(n) => n.to_string(),
        Y::String(s) => s.trim().to_string(),
        _ => return None,
    };
    let s = s.replace('_', "");
    let (neg, digits) = match s.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, s.strip_prefix('+').unwrap_or(&s)),
    };
    let digits = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
        .unwrap_or(digits);
    if neg {
        return None;
    }
    u32::from_str_radix(digits, 8).ok()
}

/// `check_permissions`: the file's mode bits are exactly `perm`.
fn check(path: &Path, perm: u32) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o7777 == perm)
}

/// `dirs_in_library`: the path's ancestors inside the library, from the
/// top down (the library directory itself excluded).
pub fn dirs_in_library(library: &Path, path: &Path) -> Vec<PathBuf> {
    if !path.starts_with(library) {
        return vec![];
    }
    let mut dirs: Vec<PathBuf> = path
        .ancestors()
        .skip(1)
        .filter(|p| p.starts_with(library) && *p != library)
        .map(Path::to_path_buf)
        .collect();
    dirs.reverse();
    dirs
}

impl Perms {
    /// `set_permissions`.
    fn set(&self, files: &[PathBuf], dirs: &[PathBuf]) {
        let perm = |k: &str| convert_perm(self.cfg.get(&format!("permissions.{k}")));
        let (Some(file_perm), Some(dir_perm)) = (perm("file"), perm("dir")) else {
            // beets fails here (`int(perm, 8)`).
            self.log.error("invalid permissions setting");
            return;
        };
        for p in files {
            self.log
                .debug(&format!("setting file permissions on {}", p.display()));
            self.apply(p, file_perm);
        }
        let mut dirs = dirs.to_vec();
        dirs.sort_by_key(|p| p.components().count());
        for p in &dirs {
            self.log
                .debug(&format!("setting directory permissions on {}", p.display()));
            self.apply(p, dir_perm);
        }
    }

    fn apply(&self, p: &Path, perm: u32) {
        if !check(p, perm) {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(perm));
        }
        // `assert_permissions`.
        if !check(p, perm) {
            self.log
                .warning(&format!("could not set permissions on {}", p.display()));
            let now = std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0);
            self.log.debug(&format!(
                "set permissions to {perm}, but permissions are now {now}"
            ));
        }
    }
}

impl Listener for Perms {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "item_imported" | "album_imported" | "art_set")
    }

    fn on(&self, ev: &Event) {
        let libdir = ev
            .lib()
            .and_then(|l| l.music_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| self.cfg.directory());
        match ev.name {
            "item_imported" => {
                if let Some(item) = ev.item() {
                    let p = item.path().to_path_buf();
                    let dirs = dirs_in_library(&libdir, &p);
                    self.set(&[p], &dirs);
                }
            }
            "album_imported" => {
                let (Some(album), Some(lib)) = (ev.album(), ev.lib()) else {
                    return;
                };
                let items = lib.album_items(album.id.unwrap_or(0)).unwrap_or_default();
                let mut files = vec![];
                let mut dirs: Vec<PathBuf> = vec![];
                for item in &items {
                    let p = item.path().to_path_buf();
                    for d in dirs_in_library(&libdir, &p) {
                        if !dirs.contains(&d) {
                            dirs.push(d);
                        }
                    }
                    files.push(p);
                }
                self.set(&files, &dirs);
            }
            "art_set" => {
                if let Some(album) = ev.album() {
                    let art = album.path_field("artpath");
                    if !art.as_os_str().is_empty() {
                        self.set(&[art.to_path_buf()], &[]);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "permissions_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "permissions",
        order: 18,
        defaults: r#"permissions:
    file: '644'
    dir: '755'
"#,
        config_keys: &["permissions"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}
