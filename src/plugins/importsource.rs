//! A port of beets 2.14's `importsource` plugin (`beetsplug/importsource.py`):
//! an imported item's `source_path` flexible attribute remembers where it
//! came from; with `suggest_removal`, removing it asks whether to delete the
//! source file (or its directory).

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::model::{Item, Value};
use crate::plugins::{ImportTask, Plugin, PluginCtx, PluginLog};
use crate::query::Query;
use crate::ui::{self, Choice};
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ImportSourcePlugin;

impl Plugin for ImportSourcePlugin {
    fn name(&self) -> &'static str {
        "importsource"
    }

    /// beets' `import_stage`: remember the source path, before the files
    /// are placed (a reimported item keeps its old one).
    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        for item in task.items.iter_mut() {
            if !item.str("source_path").is_empty() {
                self_stage_log().info(&format!(
                    "Preserving source_path of reimported item {}",
                    item.id.unwrap_or(0)
                ));
                continue;
            }
            let path = item.path().to_path_buf();
            item.set("source_path", Value::Str(path.to_string_lossy().into_owned()));
            ctx.lib.store(item)?;
        }
        Ok(())
    }
}

fn self_stage_log() -> PluginLog {
    let log = PluginLog::new("importsource");
    log.set_base_level(crate::plugins::WARNING);
    log
}

struct State {
    cfg: Config,
    log: PluginLog,
    /// `mb_albumid`s whose removals we shouldn't suggest (a reimport).
    stop: std::sync::Mutex<HashSet<String>>,
}

pub fn load(cfg: &Config) -> ImportSourcePlugin {
    let log = PluginLog::new("importsource");
    log.set_base_level(crate::plugins::WARNING);
    let state = Arc::new(State {
        cfg: cfg.clone(),
        log,
        stop: std::sync::Mutex::default(),
    });
    // As beets, the removal-suggestion listeners exist only when enabled.
    if cfg.bool("importsource.suggest_removal") {
        crate::events::register(state);
    }
    ImportSourcePlugin
}

/// A path's items' `source_path` (as stored, a text path).
pub fn source_path(item: &Item) -> Option<PathBuf> {
    let p = item.str("source_path");
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// `os.access(path, os.W_OK)` / `X_OK` for the source (and its directory).
fn writable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let ok = |p: &Path, bit: u32| {
        std::fs::metadata(p)
            .is_ok_and(|m| m.permissions().mode() & bit != 0)
    };
    ok(p, 0o222)
        && p.parent()
            .is_some_and(|d| ok(d, 0o222 | 0o111))
}

impl State {
    /// `suggest_removal`: the prompt beets shows at `item_removed`.
    fn suggest_removal(&self, item: &Item) {
        let album_id = item.str("mb_albumid").to_string();
        if self.stop.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&album_id) {
            return;
        }
        let Some(source) = source_path(item) else {
            self.log.warning(&format!(
                "Item without source_path (probably imported before plugin usage): {}",
                item.path().display()
            ));
            return;
        };
        if !source.is_file() {
            self.log.warning(&format!(
                "Original source file no longer exists or is not accessible: {}",
                source.display()
            ));
            return;
        }
        if !writable(&source) {
            self.log.warning(&format!(
                "Original source file cannot be deleted (insufficient permissions): {}",
                source.display()
            ));
            return;
        }
        println!(
            "The item:\n{}\nis originated from:\n{}\nWhat would you like to do?",
            ui::colorize("text_warning", &item.path().display().to_string()),
            ui::colorize("text_warning", &source.display().to_string())
        );
        let choices = [
            "Delete the item's source",
            "Recursively delete the source's directory",
            "do Nothing",
            "do nothing and Stop suggesting to delete items from this album",
        ];
        let Some(choice) = ui::options(&choices, None, None) else {
            return;
        };
        match choice {
            Choice::Letter('d') => {
                self.log
                    .info(&format!("Deleting the item's source file: {}", source.display()));
                let _ = std::fs::remove_file(&source);
            }
            Choice::Letter('r') => self.remove_directory(item, &source),
            Choice::Letter('s') => {
                self.stop.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(album_id);
            }
            _ => self.log.info("Doing nothing"),
        }
    }

    /// The "recursively delete" branch (beets' `PathQuery` over
    /// `source_path`).
    fn remove_directory(&self, _item: &Item, source: &Path) {
        let Some(dir) = source.parent() else { return };
        self.log.info(&format!(
            "Searching for other items with a source_path attr containing: {}",
            dir.display()
        ));
        let all = crate::library::Library::open_with(
            &self.cfg.library_path(),
            Some(&self.cfg.directory()),
            false,
        )
        .and_then(|lib| lib.items(&Query::parse::<&str>(crate::model::Kind::Item, &[])?))
        .unwrap_or_default();
        println!("Doing so will delete the following items' sources as well:");
        for other in &all {
            if let Some(p) = source_path(other)
                && p.parent() == Some(dir)
            {
                println!(
                    "{}",
                    ui::colorize("text_warning", &other.path().display().to_string())
                );
            }
        }
        println!("Would you like to continue?");
        let Some(choice) = ui::options(
            &["Yes", "delete None", "delete just the File"],
            None,
            Some(Choice::Letter('y')),
        ) else {
            return;
        };
        match choice {
            Choice::Letter('y') => {
                self.log.info(&format!(
                    "Deleting the item's source directory: {}",
                    dir.display()
                ));
                let _ = std::fs::remove_dir_all(dir);
            }
            Choice::Letter('n') => self.log.info("doing nothing - aborting hook function"),
            Choice::Letter('f') => {
                self.log.info(&format!(
                    "removing just the item's original source: {}",
                    source.display()
                ));
                let _ = std::fs::remove_file(source);
            }
            _ => {}
        }
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "item_removed" | "import_task_choice")
    }

    fn on(&self, ev: &Event) {
        match ev.name {
            "item_removed" => {
                if let Some(item) = ev.item() {
                    self.suggest_removal(item);
                }
            }
            "import_task_choice" => {
                // A reimport replaces the item at the same path; don't offer
                // to delete its source afterwards.
                let Some(task) = ev.task() else { return };
                if task.skip {
                    return;
                }
                for item in &task.items {
                    let id = item.str("mb_albumid");
                    if !id.is_empty() {
                        self.stop.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "importsource",
        order: 63,
        defaults: r#"importsource:
    suggest_removal: false
"#,
        config_keys: &["importsource"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

#[cfg(test)]
#[path = "importsource_tests.rs"]
mod tests;
