//! Plugin framework. Plugins are compiled in and enabled through the
//! beets-style `plugins:` config list. They hook into import and can add
//! their own subcommands (wired up in `main.rs`/`commands.rs`).

pub mod embedart;
pub mod fetchart;
pub mod lyrics;

use crate::config::Config;
use crate::fileops::FileOp;
use crate::library::Library;
use crate::model::{Album, Item};
use anyhow::{Result, bail};
use std::path::Path;
use std::time::Duration;

pub const USER_AGENT: &str = concat!(
    "beets-rs/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/beetbox/beets )"
);

pub fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
}

/// What a plugin gets to work with.
#[allow(dead_code)] // `cfg` is for plugins that need their own settings at hook time
pub struct PluginCtx<'a> {
    pub cfg: &'a Config,
    pub lib: &'a Library,
    /// Whether tag changes should be written to files (import.write).
    pub write: bool,
}

pub trait Plugin {
    fn name(&self) -> &'static str;

    /// Called after an album and its items are stored in the library.
    /// `source_dir` is where the files came from; `op` is how they got here.
    fn album_imported(
        &self,
        _ctx: &PluginCtx,
        _album: &mut Album,
        _items: &mut [Item],
        _source_dir: &Path,
        _op: FileOp,
    ) -> Result<()> {
        Ok(())
    }

    /// Called after a singleton is stored in the library.
    fn item_imported(&self, _ctx: &PluginCtx, _item: &mut Item) -> Result<()> {
        Ok(())
    }
}

pub const AVAILABLE: &[&str] = &["embedart", "fetchart", "lyrics"];

/// The plugins to load: `plugins` (or `-p`) minus `disabled_plugins` (`-P`),
/// as beets' `get_plugin_names`.
pub fn names(cfg: &Config) -> Vec<String> {
    let disabled = cfg.list("disabled_plugins");
    cfg.list("plugins")
        .into_iter()
        .filter(|p| !disabled.contains(p))
        .collect()
}

pub fn load(cfg: &Config) -> Result<Vec<Box<dyn Plugin>>> {
    let mut out: Vec<Box<dyn Plugin>> = vec![];
    let names = names(cfg);
    let mut sorted = names.clone();
    sorted.sort();
    crate::logging::debug!("Loading plugins: {}", sorted.join(", "));
    for name in names {
        match name.as_str() {
            "fetchart" => out.push(Box::new(fetchart::FetchArt::new(cfg))),
            "lyrics" => out.push(Box::new(lyrics::Lyrics::new(cfg))),
            "embedart" => out.push(Box::new(embedart::EmbedArt::new(cfg))),
            other => bail!(
                "unknown plugin {other:?} (available: {})",
                AVAILABLE.join(", ")
            ),
        }
    }
    // embedart embeds whatever fetchart found, so it must run after it
    // regardless of the order they're listed in.
    out.sort_by_key(|p| p.name() == "embedart");
    Ok(out)
}

pub fn enabled(cfg: &Config, name: &str) -> bool {
    names(cfg).iter().any(|p| p == name)
}

/// Run a hook on every plugin; failures are warnings, never fatal to import.
pub fn each(plugins: &[Box<dyn Plugin>], mut f: impl FnMut(&dyn Plugin) -> Result<()>) {
    for p in plugins {
        if let Err(e) = f(p.as_ref()) {
            eprintln!("warning: {}: {e:#}", p.name());
        }
    }
}
