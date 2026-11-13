//! A port of beets 2.14's `playlist` plugin (`beetsplug/playlist.py`):
//! `playlist:` queries match the files listed in an M3U playlist, and with
//! `auto: yes` the plugin updates playlists when `rm`/`mv` move or remove
//! files (at the end of the command, through beets' `cli_exit` event).
//!
//! Only the query part reads its settings from the query context; the
//! auto-update keeps the changes in a process-wide map, as beets' plugin
//! instance does.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::PluginLog;
use crate::query::Context;
use anyhow::Result;
use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The plugin: a query (`playlist:`) and the auto-update listeners.
pub struct PlaylistPlugin;

impl crate::plugins::Plugin for PlaylistPlugin {
    fn name(&self) -> &'static str {
        "playlist"
    }
}

/// The plugin's listeners: beets' `item_moved`, `item_removed` and
/// `cli_exit` events, when `auto` is set.
struct AutoUpdate {
    cfg: Config,
}

impl Listener for AutoUpdate {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "item_moved" | "item_removed" | "cli_exit")
    }

    fn on(&self, ev: &Event) {
        match ev.name {
            "item_moved" => {
                if let (Some(source), Some(destination)) =
                    (ev.path("source"), ev.path("destination"))
                {
                    item_moved(&self.cfg, source, destination);
                }
            }
            "item_removed" => {
                if let Some(item) = ev.item() {
                    item_removed(&self.cfg, item.path());
                }
            }
            "cli_exit" => {
                if let Some(lib) = ev.lib() {
                    cli_exit(&self.cfg, lib);
                }
            }
            _ => {}
        }
    }
}

/// The plugin's `__init__`: register the auto-update listeners.
pub fn load(cfg: &Config) -> PlaylistPlugin {
    crate::events::register(std::rc::Rc::new(AutoUpdate { cfg: cfg.clone() }));
    PlaylistPlugin
}

/// `is_m3u_file`.
pub fn is_m3u_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
        .unwrap_or(false)
}

/// `os.path.abspath` (what beets' `PlaylistQuery` uses for the
/// `playlist_dir` candidate).
fn abspath(p: &Path) -> PathBuf {
    if p.is_absolute() {
        crate::library::lexical_normpath(p)
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(p)
            .components()
            .collect()
    }
}

/// `line.rstrip()` of Python's bytes.
fn rstrip_ws(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(0);
    &line[..end]
}

fn bytes_path(p: &Path) -> Vec<u8> {
    p.as_os_str().as_bytes().to_vec()
}

fn path_from_bytes(b: &[u8]) -> PathBuf {
    PathBuf::from(std::ffi::OsStr::from_bytes(b))
}

/// `PlaylistQuery`: the paths an M3U playlist lists (as `InQuery("path",
/// paths)` would have them).
pub fn query_paths(pattern: &str, ctx: &Context) -> Vec<Vec<u8>> {
    // The full path to the playlist candidates.
    let candidates = [
        PathBuf::from(pattern),
        abspath(&ctx.playlist_dir.join(format!("{pattern}.m3u"))),
    ];
    let mut paths: Vec<Vec<u8>> = vec![];
    for candidate in candidates {
        if !is_m3u_file(&candidate) {
            continue;
        }
        let Ok(data) = std::fs::read(&candidate) else {
            paths.clear();
            continue;
        };
        let relative_to: PathBuf = match &ctx.playlist_relative_to {
            Some(base) => base.clone(),
            None => candidate.parent().map(PathBuf::from).unwrap_or_default(),
        };
        for line in data.split_inclusive(|&b| b == b'\n') {
            if line.first() == Some(&b'#') {
                // Comments, and the extm3u extension line.
                continue;
            }
            let rel = rstrip_ws(line);
            let joined = relative_to.join(path_from_bytes(rel));
            paths.push(bytes_path(&crate::query::normpath(&joined)));
        }
        break;
    }
    paths
}

// ------------------------------------------------------------ auto update

/// `source -> destination`, or `None` when the item was deleted.
type Changes = HashMap<Vec<u8>, Option<Vec<u8>>>;
static CHANGES: Mutex<Option<Changes>> = Mutex::new(None);

fn change_map() -> std::sync::MutexGuard<'static, Option<Changes>> {
    CHANGES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn auto(cfg: &Config) -> bool {
    crate::plugins::enabled(cfg, "playlist") && cfg.bool("playlist.auto")
}

fn norm_bytes(p: &Path) -> Vec<u8> {
    bytes_path(&crate::query::normpath(p))
}

/// beets' `item_moved` listener.
pub fn item_moved(cfg: &Config, source: &Path, destination: &Path) {
    if !auto(cfg) {
        return;
    }
    change_map()
        .get_or_insert_with(HashMap::new)
        .insert(norm_bytes(source), Some(norm_bytes(destination)));
}

/// beets' `item_removed` listener: only a file that is already gone is a
/// deletion (beets sends the event before `rm -d` deletes it).
pub fn item_removed(cfg: &Config, path: &Path) {
    if !auto(cfg) || path.exists() {
        return;
    }
    change_map()
        .get_or_insert_with(HashMap::new)
        .insert(norm_bytes(path), None);
}

/// `os.path.relpath(path, start)` for absolute paths.
fn relpath(path: &Path, start: &Path) -> PathBuf {
    let path = crate::library::lexical_normpath(path);
    let start = crate::library::lexical_normpath(start);
    let p: Vec<_> = path.components().collect();
    let s: Vec<_> = start.components().collect();
    let common = p.iter().zip(&s).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..s.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn playlist_dir(cfg: &Config) -> PathBuf {
    crate::config::expand_tilde(&cfg.str("playlist.playlist_dir"))
}

/// `find_playlists`: the M3U files in the playlist directory.
fn find_playlists(cfg: &Config, log: &PluginLog) -> Vec<PathBuf> {
    let dir = playlist_dir(cfg);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        log.warning(&format!(
            "Unable to open playlist directory {}",
            dir.display()
        ));
        return vec![];
    };
    let mut out = vec![];
    for e in entries.flatten() {
        let name = e.file_name();
        if is_m3u_file(Path::new(&name)) {
            out.push(dir.join(name));
        }
    }
    out
}

/// `update_playlist`: rewrite the playlist's lines with the moved paths.
fn update_playlist(filename: &Path, base_dir: &Path, log: &PluginLog) -> Result<()> {
    let data = std::fs::read(filename)?;
    let changes = change_map();
    let changes = changes.clone().unwrap_or_default();
    let mut out: Vec<u8> = vec![];
    let mut changed = 0;
    let mut deletions = 0;
    for line in data.split_inclusive(|&b| b == b'\n') {
        let original = {
            let mut end = line.len();
            while end > 0 && (line[end - 1] == b'\r' || line[end - 1] == b'\n') {
                end -= 1;
            }
            &line[..end]
        };
        // Is the path in the playlist relative or absolute?
        let original_path = path_from_bytes(original);
        let is_relative = !original_path.is_absolute();
        let lookup = if is_relative {
            base_dir.join(&original_path)
        } else {
            original_path.clone()
        };
        match changes.get(&norm_bytes(&lookup)) {
            None => out.extend_from_slice(line),
            Some(None) => {
                // The item has been deleted.
                deletions += 1;
            }
            Some(Some(new_path)) => {
                changed += 1;
                let new_path = if is_relative {
                    relpath(&path_from_bytes(new_path), base_dir)
                } else {
                    path_from_bytes(new_path)
                };
                // `line.replace(original_path, new_path)`.
                out.extend_from_slice(new_path.as_os_str().as_bytes());
                out.extend_from_slice(&line[original.len()..]);
            }
        }
    }
    if changed != 0 || deletions != 0 {
        log.info(&format!(
            "Updated playlist {} ({} changes, {} deletions)",
            filename.display(),
            changed,
            deletions
        ));
        std::fs::write(filename, &out)?;
    }
    Ok(())
}

/// beets' `cli_exit`: update every playlist in the playlist directory.
pub fn cli_exit(cfg: &Config, _lib: &crate::library::Library) {
    if !auto(cfg) {
        return;
    }
    let log = PluginLog::new("playlist");
    let relative_to = cfg.str("playlist.relative_to");
    let base: Option<PathBuf> = match relative_to.as_str() {
        "playlist" => None,
        "library" => Some(cfg.directory()),
        other => Some(crate::config::expand_tilde(other)),
    };
    for playlist in find_playlists(cfg, &log) {
        log.info(&format!("Updating playlist: {}", playlist.display()));
        let base_dir = match &base {
            Some(b) => b.clone(),
            None => playlist.parent().map(PathBuf::from).unwrap_or_default(),
        };
        if let Err(e) = update_playlist(&playlist, &base_dir, &log) {
            log.error(&format!(
                "Failed to update playlist: {} ({e})",
                playlist.display()
            ));
        }
    }
    // A fresh command starts with no changes.
    *change_map() = None;
}

#[cfg(test)]
#[path = "playlist_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "playlist",
        order: 36,
        defaults: r#"playlist:
    auto: no
    playlist_dir: .
    relative_to: library
    forward_slash: no
"#,
        config_keys: &["playlist"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}
