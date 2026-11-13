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

/// Config a plugin writes while it loads, as beets' plugins do
/// (`config["importfeeds"]["relative_to"] = ...`): confuse's `set` puts
/// it above every other source, so `beet config` lists those keys first.
pub struct PluginLoadSet {
    /// The plugin the values belong to (only applied while it is enabled).
    pub plugin: &'static str,
    /// Dotted keys and their values.
    pub set: fn(&Config) -> Vec<(&'static str, Y)>,
}

inventory::collect!(PluginLoadSet);

/// Every plugin's load-time `config` assignments.
pub fn load_sets() -> Vec<&'static PluginLoadSet> {
    let mut sets: Vec<&'static PluginLoadSet> =
        inventory::iter::<PluginLoadSet>.into_iter().collect();
    sets.sort_by_key(|s| s.plugin);
    sets
}

/// Every registered plugin, in the order of beets' bundled plugin list.
/// `order` is the primary key; `name` breaks ties so the order is
/// deterministic (the registry's link order is not).
pub fn defs() -> Vec<&'static PluginDef> {
    let mut defs: Vec<&'static PluginDef> = inventory::iter::<PluginDef>.into_iter().collect();
    defs.sort_by_key(|d| (d.order, d.name));
    defs
}

/// The registration for `name`.
pub fn find(name: &str) -> Option<&'static PluginDef> {
    inventory::iter::<PluginDef>
        .into_iter()
        .find(|d| d.name == name)
}

/// Every plugin command, in `beet --help` order (`order`, then the
/// command's name to make ties deterministic).
pub fn commands() -> Vec<&'static PluginCommand> {
    let mut out: Vec<(u16, String, &'static PluginCommand)> = defs()
        .iter()
        .flat_map(|d| d.commands.iter())
        .map(|c| (c.order, (c.build)().get_name().to_string(), c))
        .collect();
    out.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    out.into_iter().map(|(_, _, c)| c).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// `order` values are picked by hand (one number per plugin file), so a
    /// collision must show up here rather than depending on link order.
    #[test]
    fn plugin_orders_are_unique() {
        let mut by_order: BTreeMap<u16, &'static str> = BTreeMap::new();
        for def in inventory::iter::<PluginDef> {
            if let Some(other) = by_order.insert(def.order, def.name) {
                panic!(
                    "plugins {other:?} and {:?} both have order {}; pick another",
                    def.name, def.order
                );
            }
        }
    }

    #[test]
    fn command_orders_are_unique() {
        let mut all: Vec<(u16, String)> = inventory::iter::<PluginDef>
            .into_iter()
            .flat_map(|d| d.commands.iter())
            .map(|c| (c.order, (c.build)().get_name().to_string()))
            .collect();
        all.sort();
        for pair in all.windows(2) {
            assert_ne!(
                pair[0].0, pair[1].0,
                "commands {:?} and {:?} both have order {}; pick another",
                pair[0].1, pair[1].1, pair[0].0
            );
        }
    }

    #[test]
    fn orders_are_deterministic() {
        let names: Vec<&str> = defs().iter().map(|d| d.name).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| {
            let def = find(n).unwrap();
            (def.order, def.name)
        });
        assert_eq!(names, sorted);
    }
}
