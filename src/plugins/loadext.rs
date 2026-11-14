//! beets' `loadext` plugin: load SQLite extensions (for the ICU
//! collation, typically) when the library opens. The bundled SQLite is
//! built with extension loading (`SQLITE_ENABLE_LOAD_EXTENSION`), so the
//! plugin loads the configured files for real, as beets does.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::{Plugin, PluginLog};
use std::path::PathBuf;
use std::sync::Arc;

pub struct LoadExt;

struct State {
    cfg: Config,
    log: PluginLog,
}

impl LoadExt {
    pub fn new(cfg: &Config) -> Self {
        let state = Arc::new(State {
            cfg: cfg.clone(),
            log: PluginLog::new("loadext"),
        });
        // beets raises an event listener's logger to WARNING (DEBUG with
        // -vv, "loading extension").
        state.log.set_base_level(crate::plugins::WARNING);
        crate::events::register(state);
        LoadExt
    }
}

impl Plugin for LoadExt {
    fn name(&self) -> &'static str {
        "loadext"
    }
}

impl State {
    /// A `loadext` entry as confuse's `Filename` reads it: `~` expanded,
    /// a relative path taken from the configuration directory.
    fn extension_path(&self, entry: &str) -> PathBuf {
        let p = crate::config::expand_tilde(entry);
        if p.is_absolute() {
            p
        } else {
            self.cfg.config_dir.join(p)
        }
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        event == "library_opened"
    }

    fn on(&self, ev: &Event) {
        let Some(lib) = ev.lib() else { return };
        for entry in self.cfg.list("loadext") {
            let ext = self.extension_path(&entry);
            self.log
                .debug(&format!("loading extension {}", ext.display()));
            if let Err(e) = lib.load_extension(&ext) {
                self.log.error(&format!(
                    "failed to load extension {}: {e}",
                    ext.display()
                ));
            }
        }
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "loadext",
        order: 76,
        defaults: r#""#,
        config_keys: &["loadext"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(LoadExt::new(cfg)))
}

#[cfg(test)]
#[path = "loadext_tests.rs"]
mod tests;
