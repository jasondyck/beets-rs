//! beets' `loadext` plugin: load SQLite extensions from `pluginpath`-like
//! config values. beets-rs' bundled SQLite has extension loading disabled,
//! so (as beets does on such a build) it only warns when enabled.

use crate::config::Config;
use crate::plugins::{Plugin, PluginLog};

pub struct LoadExt;

impl LoadExt {
    pub fn new(_cfg: &Config) -> Self {
        PluginLog::new("loadext").warning(
            "loadext is enabled but the current SQLite installation does not support extensions",
        );
        LoadExt
    }
}

impl Plugin for LoadExt {
    fn name(&self) -> &'static str {
        "loadext"
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "loadext",
        order: 76,
        defaults: r#""#,
        config_keys: &["loadext"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(LoadExt::new(cfg)))
}
