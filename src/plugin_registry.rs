//! The plugin registry: a plugin declares its name, defaults, constructor
//! and CLI commands in its own file with [`inventory`], and this module
//! gathers them. Adding a plugin touches only its own files (plus its
//! README row and diffbeets cases): `plugins::load`, the config defaults
//! and the `beet` command line are built from here.

use crate::config::Config;
use crate::library::Library;
use crate::plugins::Plugin;
use anyhow::Result;
use serde_yaml::{Mapping, Value as Y};

/// A plugin instance's constructor.
type Make = fn(&Config) -> Result<Box<dyn Plugin>>;
/// A plugin command's implementation.
type Run = fn(&Config, &Library, &[Box<dyn Plugin>], &clap::ArgMatches) -> Result<()>;

/// One of a plugin's subcommands.
pub struct PluginCommand {
    /// Position among the plugin commands on `beet --help` (core commands
    /// come first, `version`/`help`/`config` last).
    pub order: u16,
    /// The plugin the command belongs to (the `plugins:` name, checked
    /// when the command runs).
    pub plugin: &'static str,
    /// The clap command (`Args::augment_args(Command::new(name))`).
    pub build: fn() -> clap::Command,
    /// Run it with the parsed subcommand matches.
    pub run: Run,
}

/// A bundled plugin, as its file declares it.
pub struct PluginDef {
    /// The name in the `plugins:` setting.
    pub name: &'static str,
    /// Order in the "unknown plugin" message's list (the plugins in
    /// `beetsplug/`, as beets' bundled set).
    pub order: u16,
    /// The YAML settings the plugin's `__init__` would `config.add`.
    pub defaults: &'static str,
    /// The top-level keys `defaults` sets (beets copies them into the
    /// effective configuration while the plugin is enabled).
    pub config_keys: &'static [&'static str],
    /// Dotted defaults keys `beet config` shows as REDACTED while the
    /// plugin is enabled (beets' `redact` flags).
    pub redacted: &'static [&'static str],
    /// The plugin instance's constructor. None for `musicbrainz`, which is
    /// the autotagger's own metadata source rather than a `Plugin`.
    pub make: Option<Make>,
    /// A setup error ends the command (beets reads the plugin's settings on
    /// `pluginload`), rather than the plugin being left out.
    pub fatal: bool,
    /// Its `beet` subcommands.
    pub commands: &'static [PluginCommand],
}

inventory::collect!(PluginDef);

/// Every registered plugin, in the order of beets' bundled plugin list.
pub fn defs() -> Vec<&'static PluginDef> {
    let mut defs: Vec<&'static PluginDef> = inventory::iter::<PluginDef>.into_iter().collect();
    defs.sort_by_key(|d| d.order);
    defs
}

/// The registration for `name`.
pub fn find(name: &str) -> Option<&'static PluginDef> {
    inventory::iter::<PluginDef>
        .into_iter()
        .find(|d| d.name == name)
}

/// Every plugin command, in `beet --help` order.
pub fn commands() -> Vec<&'static PluginCommand> {
    let mut out: Vec<&'static PluginCommand> =
        defs().iter().flat_map(|d| d.commands.iter()).collect();
    out.sort_by_key(|c| c.order);
    out
}

/// The command whose canonical name or alias is `name`.
pub fn command(name: &str) -> Option<&'static PluginCommand> {
    let canonical =
        |cmd: &clap::Command| cmd.get_name() == name || cmd.get_aliases().any(|a| a == name);
    commands().into_iter().find(|c| canonical(&(c.build)()))
}

/// Every plugin's default settings, merged as beets' `config.add` calls
/// would leave them (regardless of whether the plugin is enabled; the
/// effective configuration only copies the enabled ones).
pub fn default_tree() -> Y {
    let mut out = Mapping::new();
    for def in defs() {
        if def.defaults.trim().is_empty() {
            continue;
        }
        let text = def.defaults.replace(
            "CPU_COUNT",
            &crate::plugins::replaygain::cpu_count().to_string(),
        );
        let y: Y = serde_yaml::from_str(&crate::config::yaml11_bools(&text))
            .unwrap_or_else(|e| panic!("{}: invalid default config: {e}", def.name));
        if let Y::Mapping(m) = y {
            for (k, v) in m {
                out.insert(k, v);
            }
        }
    }
    Y::Mapping(out)
}
