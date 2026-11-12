//! A port of beets 2.14's `importadded` plugin
//! (`beetsplug/importadded.py`): an imported item's `added` (and, with
//! `preserve_mtimes`, its file's mtime and `mtime`) becomes the mtime its
//! source file had before the import; an album's `added` the earliest of
//! its items'. Reimported albums and items keep theirs. With
//! `preserve_write_mtimes`, a written file gets its item's `added` back as
//! its mtime.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::model::Value;
use crate::plugins::PluginLog;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct ImportAddedPlugin;

impl crate::plugins::Plugin for ImportAddedPlugin {
    fn name(&self) -> &'static str {
        "importadded"
    }
}

struct State {
    cfg: Config,
    log: PluginLog,
    /// A library path to the mtime of the file it came from.
    item_mtime: RefCell<HashMap<PathBuf, f64>>,
    /// Ids of new items that replaced library items.
    reimported_item_ids: RefCell<Vec<i64>>,
    /// Directories of the albums a reimport replaced.
    replaced_album_paths: RefCell<Vec<PathBuf>>,
}

pub fn load(cfg: &Config) -> ImportAddedPlugin {
    let log = PluginLog::new("importadded");
    log.set_base_level(crate::plugins::WARNING);
    crate::events::register(Rc::new(State {
        cfg: cfg.clone(),
        log,
        item_mtime: RefCell::default(),
        reimported_item_ids: RefCell::default(),
        replaced_album_paths: RefCell::default(),
    }));
    ImportAddedPlugin
}

/// A file's mtime as Python's `os.stat().st_mtime`.
pub fn st_mtime(path: &Path) -> Option<f64> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(m.mtime() as f64 + m.mtime_nsec() as f64 * 1e-9)
}

/// `os.utime(path, (atime, mtime))`, keeping the access time.
pub fn set_mtime(path: &Path, mtime: f64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(mtime.max(0.0));
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(t));
    } else if let Ok(f) = std::fs::File::open(path) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(t));
    }
}

impl State {
    fn preserve(&self, key: &str) -> bool {
        self.cfg.bool(&format!("importadded.{key}"))
    }

    /// `record_import_mtime`.
    fn record(&self, source: &Path, dest: &Path) {
        let Some(mtime) = st_mtime(source) else {
            return;
        };
        self.item_mtime
            .borrow_mut()
            .insert(dest.to_path_buf(), mtime);
        self.log.debug(&format!(
            "Recorded mtime {} for item '{}' imported from '{}'",
            crate::hooks::py_float(mtime),
            dest.display(),
            source.display()
        ));
    }

    /// `update_album_times`.
    fn album_imported(&self, ev: &Event) {
        let (Some(album), Some(lib)) = (ev.album(), ev.lib()) else {
            return;
        };
        let id = album.id.unwrap_or(0);
        let mut items = lib.album_items(id).unwrap_or_default();
        let dir = items
            .first()
            .and_then(|i| i.path().parent())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        if self.replaced_album_paths.borrow().contains(&dir) {
            self.log.debug(&format!(
                "Album '{}' is reimported, skipping import of added dates for the album and its items.",
                dir.display()
            ));
            return;
        }
        let mut mtimes = vec![];
        for item in &mut items {
            let mtime = self.item_mtime.borrow_mut().remove(item.path());
            if let Some(mtime) = mtime.filter(|m| *m != 0.0) {
                mtimes.push(mtime);
                if self.preserve("preserve_mtimes") {
                    set_mtime(item.path(), mtime);
                    item.set("mtime", Value::Float(mtime.trunc()));
                    let _ = lib.store(item);
                }
            }
        }
        let Some(min) = mtimes.into_iter().reduce(f64::min) else {
            // beets fails here (`min()` of nothing).
            self.log.error(&format!(
                "no recorded mtimes for album '{}'",
                album.str("album")
            ));
            return;
        };
        // Fresh: the items may have been stored above.
        let Ok(Some(old)) = lib.get_album(id) else {
            return;
        };
        let mut new = old.clone();
        new.set("added", Value::Float(min));
        self.log.debug(&format!(
            "Import of album '{}', selected album.added={} from item file mtimes.",
            new.str("album"),
            crate::hooks::py_float(min)
        ));
        // `album.store()`: `added` is inherited by the items.
        let _ = crate::commands::store_album_inherit(lib, &old, &new);
    }

    /// `update_item_times`.
    fn item_imported(&self, ev: &Event) {
        let (Some(item), Some(lib)) = (ev.item(), ev.lib()) else {
            return;
        };
        if self
            .reimported_item_ids
            .borrow()
            .contains(&item.id.unwrap_or(-1))
        {
            self.log.debug(&format!(
                "Item '{}' is reimported, skipping import of added date.",
                item.path().display()
            ));
            return;
        }
        let mtime = self.item_mtime.borrow_mut().remove(item.path());
        let Some(mtime) = mtime.filter(|m| *m != 0.0) else {
            return;
        };
        let Ok(Some(mut item)) = lib.get_item(item.id.unwrap_or(0)) else {
            return;
        };
        item.set("added", Value::Float(mtime));
        if self.preserve("preserve_mtimes") {
            set_mtime(item.path(), mtime);
            item.set("mtime", Value::Float(mtime.trunc()));
        }
        self.log.debug(&format!(
            "Import of item '{}', selected item.added={}",
            item.path().display(),
            crate::hooks::py_float(mtime)
        ));
        let _ = lib.store(&item);
    }

    /// `update_after_write_time`.
    fn after_write(&self, ev: &Event) {
        let (Some(item), Some(path)) = (ev.item(), ev.path("path")) else {
            return;
        };
        let added = item.float("added");
        if added == 0.0 {
            return;
        }
        if self.preserve("preserve_write_mtimes") {
            // The caller reads the file's mtime back as the item's.
            set_mtime(path, added);
        }
        self.log.debug(&format!(
            "Write of item '{}', selected item.added={}",
            item.path().display(),
            crate::hooks::py_float(added)
        ));
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        matches!(
            event,
            "import_task_created"
                | "import_task_files"
                | "before_item_moved"
                | "item_copied"
                | "item_linked"
                | "item_hardlinked"
                | "item_reflinked"
                | "album_imported"
                | "item_imported"
                | "after_write"
        )
    }

    fn on(&self, ev: &Event) {
        match ev.name {
            "import_task_created" => {
                // `record_if_inplace`.
                let Some(task) = ev.task() else { return };
                if task.in_place {
                    self.log
                        .debug("In place import detected, recording mtimes from source paths");
                    for item in &task.items {
                        self.record(item.path(), item.path());
                    }
                }
            }
            "import_task_files" => {
                // `record_reimported`.
                let Some(task) = ev.task() else { return };
                *self.reimported_item_ids.borrow_mut() = task.reimported_item_ids.clone();
                *self.replaced_album_paths.borrow_mut() = task.replaced_album_paths.clone();
            }
            "album_imported" => self.album_imported(ev),
            "item_imported" => self.item_imported(ev),
            "after_write" => self.after_write(ev),
            _ => {
                if let (Some(src), Some(dest)) = (ev.path("source"), ev.path("destination")) {
                    self.record(src, dest);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "importadded_tests.rs"]
mod tests;
