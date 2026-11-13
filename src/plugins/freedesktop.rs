//! A port of beets 2.14's `freedesktop` plugin (`beetsplug/freedesktop.py`):
//! a deprecated command that points at the `thumbnails` plugin, whose
//! `--dolphin` option replaces it.

use crate::config::Config;
use crate::library::Library;
use crate::plugins::Plugin;

pub struct FreedesktopPlugin;

impl Plugin for FreedesktopPlugin {
    fn name(&self) -> &'static str {
        "freedesktop"
    }
}

/// beets' deprecation message (two lines).
pub const MESSAGE: [&str; 2] = [
    "This plugin is deprecated. Its functionality is superseded by the 'thumbnails' plugin",
    "'thumbnails --dolphin' replaces freedesktop. See doc & changelog for more information",
];

/// `beet freedesktop`: beets' deprecation message.
pub fn command(_cfg: &Config, _lib: &Library) -> anyhow::Result<()> {
    for line in MESSAGE {
        println!("{line}");
    }
    Ok(())
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct FreedesktopArgs {}

pub fn build_freedesktop() -> clap::Command {
    use clap::Args as _;
    FreedesktopArgs::augment_args(
        clap::Command::new("freedesktop").about("Print a message to redirect to thumbnails --dolphin"),
    )
}

pub fn run_freedesktop(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let _a = FreedesktopArgs::from_arg_matches(m)?;
    command(cfg, lib)
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "freedesktop",
        order: 60,
        defaults: r#""#,
        config_keys: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            order: 350,
            plugin: "freedesktop",
            build: build_freedesktop,
            run: run_freedesktop,
        }],
    }
}

fn registry_make(
    _cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(FreedesktopPlugin))
}

#[cfg(test)]
#[path = "freedesktop_tests.rs"]
mod tests;
