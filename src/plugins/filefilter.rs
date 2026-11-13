//! A port of beets 2.14's `filefilter` plugin (`beetsplug/filefilter.py`):
//! only files whose full path matches `path` (or, for album imports,
//! `album_path`; for singleton imports, `singleton_path`) are imported; a
//! task left with no files is dropped. Python regular expressions,
//! matched at the start of the path (`re.match`).

use crate::config::Config;
use anyhow::{Result, anyhow};
use fancy_regex::Regex;
use std::path::Path;
use std::sync::Arc;

pub struct FileFilterPlugin;

impl crate::plugins::Plugin for FileFilterPlugin {
    fn name(&self) -> &'static str {
        "filefilter"
    }

    /// `import_task_created_event`: keep only the files the pattern
    /// allows; a task with none left is dropped.
    fn import_task_created(
        &self,
        ctx: &crate::plugins::PluginCtx,
        items: &mut Vec<crate::model::Item>,
        singletons: bool,
    ) -> Result<bool> {
        let Some(filter) = for_import(ctx.cfg) else {
            return Ok(true);
        };
        let before = items.len();
        items.retain(|i| filter.allows(i.path(), singletons));
        if before > 0 && items.is_empty() {
            return Ok(false);
        }
        Ok(true)
    }
}

/// The plugin's compiled patterns.
#[derive(Debug)]
pub struct FileFilter {
    album: Regex,
    singleton: Regex,
}

fn compile(pat: &str) -> Result<Regex> {
    // `re.match`: anchored at the start only.
    Regex::new(&format!(r"\A(?:{pat})")).map_err(|e| anyhow!("{pat}: {e}"))
}

impl FileFilter {
    /// The plugin's `__init__`: `path` for both kinds of task, unless
    /// `album_path` / `singleton_path` is set.
    pub fn new(cfg: &Config) -> Result<FileFilter> {
        let path = cfg.str("filefilter.path");
        let base = compile(&path)?;
        let album = match cfg.get("filefilter.album_path") {
            Some(_) => compile(&cfg.str("filefilter.album_path"))?,
            None => base.clone(),
        };
        let singleton = match cfg.get("filefilter.singleton_path") {
            Some(_) => compile(&cfg.str("filefilter.singleton_path"))?,
            None => base,
        };
        Ok(FileFilter { album, singleton })
    }

    /// `file_filter`: the singleton pattern when `import.singletons` is
    /// set, else the album one.
    pub fn allows(&self, path: &Path, singletons: bool) -> bool {
        let re = if singletons {
            &self.singleton
        } else {
            &self.album
        };
        re.is_match(path.to_string_lossy().as_ref())
            .unwrap_or(false)
    }
}

/// The filter to apply while importing, if the plugin is enabled.
pub fn for_import(cfg: &Config) -> Option<Arc<FileFilter>> {
    if !crate::plugins::enabled(cfg, "filefilter") {
        return None;
    }
    FileFilter::new(cfg).ok().map(Arc::new)
}

#[cfg(test)]
#[path = "filefilter_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "filefilter",
        order: 6,
        defaults: r#"filefilter:
    path: .*
"#,
        config_keys: &["filefilter"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    FileFilter::new(cfg)?;
    Ok(Box::new(FileFilterPlugin))
}
