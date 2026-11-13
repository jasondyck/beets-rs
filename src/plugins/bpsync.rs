//! A port of beets 2.14's deprecated `bpsync` plugin
//! (`beetsplug/bpsync.py`): update metadata from Beatport. The plugin is
//! dead in beets (it wraps the deprecated `beatport` metadata source, which
//! needs OAuth credentials and isn't ported yet); beets-rs logs the same
//! deprecation warnings at load and its `bpsync` command reports that the
//! Beatport source is missing.

use crate::config::Config;
use crate::library::Library;
use crate::plugins::{Plugin, PluginLog};

pub struct BPSyncPlugin;

impl Plugin for BPSyncPlugin {
    fn name(&self) -> &'static str {
        "bpsync"
    }
}

pub fn load(_cfg: &Config) -> BPSyncPlugin {
    let log = PluginLog::new("bpsync");
    log.warning("The 'bpsync' plugin is deprecated and will be removed in version 3.0.0.");
    let beatport = PluginLog::new("beatport");
    beatport.warning("The 'beatport' plugin is deprecated and will be removed in version 3.0.0.");
    BPSyncPlugin
}

/// The command's options (beets' `BPSyncCLIOpts`).
#[allow(dead_code)]
pub struct CmdOpts {
    pub pretend: bool,
    pub mv: Option<bool>,
    pub nowrite: bool,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> anyhow::Result<()> {
    let _ = (cfg, lib, opts);
    anyhow::bail!("bpsync needs the beatport metadata source, which isn't ported")
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct BpsyncArgs {
    /// show all changes but do nothing
    #[arg(short = 'p', long)]
    pretend: bool,
    /// move files in the library directory
    #[arg(short = 'm', long = "move", overrides_with = "nomove")]
    mv: bool,
    /// don't move files in library
    #[arg(short = 'M', long = "nomove", overrides_with = "mv")]
    nomove: bool,
    /// don't write updated metadata to files
    #[arg(short = 'W', long = "nowrite")]
    nowrite: bool,
    /// Print with custom format
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_bpsync() -> clap::Command {
    use clap::Args as _;
    BpsyncArgs::augment_args(clap::Command::new("bpsync").about("update metadata from Beatport"))
}

pub fn run_bpsync(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = BpsyncArgs::from_arg_matches(m)?;
    command(
        cfg,
        lib,
        CmdOpts {
            pretend: a.pretend,
            mv: if a.mv {
                Some(true)
            } else if a.nomove {
                Some(false)
            } else {
                None
            },
            nowrite: a.nowrite,
            format: a.format,
            limit: a.limit,
            query: a.query,
        },
    )
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "bpsync",
        order: 67,
        defaults: r#""#,
        config_keys: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            order: 390,
            plugin: "bpsync",
            build: build_bpsync,
            run: run_bpsync,
        }],
    }
}

fn registry_make(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

#[cfg(test)]
#[path = "bpsync_tests.rs"]
mod tests;
