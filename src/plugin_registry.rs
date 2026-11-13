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
    /// beets' `item_types`: the flexible attribute types this plugin
    /// declares for items.
    pub item_types: &'static [(&'static str, crate::model::FieldType)],
    /// beets' `album_types`: the flexible attribute types for albums.
    pub album_types: &'static [(&'static str, crate::model::FieldType)],
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

/// Every plugin command, sorted by name as beets' `beet --help` lists them
/// (the registry's link order is not deterministic).
pub fn commands() -> Vec<&'static PluginCommand> {
    let mut out: Vec<(String, &'static PluginCommand)> = defs()
        .iter()
        .flat_map(|d| d.commands.iter())
        .map(|c| ((c.build)().get_name().to_string(), c))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter().map(|(_, c)| c).collect()
}

/// The command whose canonical name or alias is `name`.
pub fn command(name: &str) -> Option<&'static PluginCommand> {
    let canonical =
        |cmd: &clap::Command| cmd.get_name() == name || cmd.get_aliases().any(|a| a == name);
    commands().into_iter().find(|c| canonical(&(c.build)()))
}

/// What a plugin's defaults may need from the effective configuration
/// (beets computes some of them at load time).
pub struct DefaultsCtx {
    /// `config["directory"].as_filename()`: the resolved music directory.
    pub directory: String,
}

impl DefaultsCtx {
    /// The context of a raw configuration tree (the user sources over the
    /// core defaults).
    pub fn from_tree(tree: &Y) -> Self {
        let raw = match tree.get("directory") {
            Some(Y::String(s)) => s.clone(),
            Some(Y::Number(n)) => n.to_string(),
            _ => "~/Music".to_string(),
        };
        DefaultsCtx {
            directory: crate::config::expand_tilde(&raw)
                .to_string_lossy()
                .into_owned(),
        }
    }
}

/// The tokens plugin defaults may use for beets' computed values:
/// `CPU_COUNT` (int), `MUSIC_DIRECTORY` (`config["directory"]`),
/// `MPD_HOST`/`MPD_PORT` (the environment, else the fallbacks beets uses).
fn substitute(value: &mut Y, ctx: &DefaultsCtx) {
    match value {
        Y::String(s) => {
            *value = match s.as_str() {
                "CPU_COUNT" => Y::Number(crate::plugins::replaygain::cpu_count().into()),
                "MUSIC_DIRECTORY" => Y::String(ctx.directory.clone()),
                "MPD_HOST" => {
                    Y::String(std::env::var("MPD_HOST").unwrap_or_else(|_| "localhost".into()))
                }
                "MPD_PORT" => {
                    let port = std::env::var("MPD_PORT")
                        .ok()
                        .and_then(|p| p.parse::<u64>().ok())
                        .unwrap_or(6600);
                    Y::Number(port.into())
                }
                _ => return,
            };
        }
        Y::Sequence(items) => {
            for item in items {
                substitute(item, ctx);
            }
        }
        Y::Mapping(map) => {
            for (_, v) in map.iter_mut() {
                substitute(v, ctx);
            }
        }
        _ => {}
    }
}

/// Parse one plugin's `defaults` YAML, with beets' computed values filled
/// in.
fn parsed_defaults(def: &PluginDef, ctx: &DefaultsCtx) -> Option<Y> {
    if def.defaults.trim().is_empty() {
        return None;
    }
    let mut y: Y = serde_yaml::from_str(&crate::config::yaml11_bools(def.defaults))
        .unwrap_or_else(|e| panic!("{}: invalid default config: {e}", def.name));
    substitute(&mut y, ctx);
    Some(y)
}

/// Deep-merge `over` under `base`: keys `base` already has win (beets'
/// `config.add` appends a lower-priority source, so the first plugin to set
/// a key keeps it); new keys are appended in `over`'s order.
pub fn merge_first(base: &mut Y, over: &Y) {
    match (&mut *base, over) {
        (Y::Mapping(b), Y::Mapping(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) => merge_first(existing, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => {
            if b.is_null() {
                *b = o.clone();
            }
        }
    }
}

/// The defaults the enabled plugins add (`beets`' `config.add` in each
/// `__init__`), merged in `names` order: a shared section such as `mpd`
/// gets each plugin's keys, and a key two plugins set keeps the first
/// plugin's value. `beets` can only load plugins that are enabled, so
/// another plugin's defaults never show up.
pub fn defaults_for(names: &[String], ctx: &DefaultsCtx) -> Y {
    let mut out = Y::Mapping(Mapping::new());
    for name in names {
        let Some(def) = find(name) else { continue };
        if let Some(y) = parsed_defaults(def, ctx) {
            merge_first(&mut out, &y);
        }
    }
    out
}

/// Every bundled plugin's default settings, merged in registry order. (The
/// effective configuration uses [`defaults_for`] with the enabled plugins.)
pub fn default_tree(ctx: &DefaultsCtx) -> Y {
    let mut out = Y::Mapping(Mapping::new());
    for def in defs() {
        if let Some(y) = parsed_defaults(def, ctx) {
            merge_first(&mut out, &y);
        }
    }
    out
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
    fn orders_are_deterministic() {
        let names: Vec<&str> = defs().iter().map(|d| d.name).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| {
            let def = find(n).unwrap();
            (def.order, def.name)
        });
        assert_eq!(names, sorted);
    }

    /// Computed defaults: `MUSIC_DIRECTORY` is the effective music
    /// directory, `MPD_HOST`/`MPD_PORT` the environment (beets' fallbacks
    /// otherwise).
    #[test]
    fn computed_defaults() {
        let ctx = DefaultsCtx {
            directory: "/srv/music".into(),
        };
        let y = defaults_for(&["mpdstats".to_string()], &ctx);
        let mpd = &y["mpd"];
        assert_eq!(mpd["music_directory"], "/srv/music");
        assert_eq!(
            mpd["host"],
            std::env::var("MPD_HOST").unwrap_or_else(|_| "localhost".into())
        );
        let port = std::env::var("MPD_PORT")
            .ok()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(6600);
        assert_eq!(mpd["port"], port);
        // A shared section keeps the first plugin's key order and values.
        let y = defaults_for(&["mpdupdate".to_string(), "mpdstats".to_string()], &ctx);
        let keys: Vec<&str> = y["mpd"]
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap())
            .collect();
        assert_eq!(keys[..3], ["host", "port", "password"]);
        assert!(keys.contains(&"music_directory"));
    }
}
