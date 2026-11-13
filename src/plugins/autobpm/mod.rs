//! A port of beets 2.14's `autobpm` plugin (`beetsplug/autobpm.py`):
//! `beet autobpm` (and, with `autobpm.auto`, the import) estimates each
//! item's tempo and stores it as `bpm`.
//!
//! beets measures the tempo with librosa (`librosa.beat.beat_track`); this
//! port reimplements the librosa pipeline it goes through (see
//! [`librosa`]): a symphonia decode to mono 22.05 kHz floats with
//! resampy's `kaiser_fast` resampler, `onset_strength` with
//! `aggregate=np.median`, and the tempogram tempo estimate with librosa's
//! log-normal prior. `beat_track_kwargs` applies only to the beat tracker
//! librosa runs after the tempo, which isn't ported (beets only uses the
//! tempo), so the options are accepted and ignored with a warning.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::PluginLog;
use crate::query::Query;
pub mod librosa;

use anyhow::Result;

pub struct AutoBPMPlugin {
    force: bool,
    quiet: bool,
    log: PluginLog,
}

impl AutoBPMPlugin {
    /// The plugin's `__init__`: the config, the deprecated `overwrite`
    /// option (beets' `deprecate_for_user` turns it into `force`), the
    /// warning for unsupported `beat_track_kwargs`.
    pub fn new(cfg: &Config) -> AutoBPMPlugin {
        let log = PluginLog::new("autobpm");
        let mut force = cfg.bool("autobpm.force");
        if cfg.get("autobpm.overwrite").is_some() {
            log.warning(
                "'autobpm.overwrite' configuration option is deprecated, use 'autobpm.force' \
                 as a replacement.",
            );
            force = cfg.bool("autobpm.overwrite");
        }
        if !cfg.mapping("autobpm.beat_track_kwargs").is_empty() {
            log.warning(
                "autobpm.beat_track_kwargs is not supported by beets-rs (no librosa); \
                 the options are ignored.",
            );
        }
        AutoBPMPlugin {
            force,
            quiet: cfg.bool("autobpm.quiet"),
            log,
        }
    }

    /// beets' `calculate_bpm`.
    #[allow(clippy::too_many_arguments)]
    pub fn calculate_bpm(
        &self,
        cfg: &Config,
        lib: &Library,
        items: &mut [Item],
        write: bool,
        force: bool,
        quiet: bool,
    ) -> Result<()> {
        for item in items.iter_mut() {
            let path = item.path();
            let shown = path.to_string_lossy().into_owned();
            if item.int("bpm") != 0 {
                if !quiet {
                    self.log.info(&format!(
                        "BPM for {shown} already exists: {}",
                        item.int("bpm")
                    ));
                }
                if !force {
                    continue;
                }
            }
            let samples = match librosa::load(path) {
                Ok(s) => s,
                Err(e) => {
                    self.log.error(&format!("Failed to load {shown}: {e}"));
                    continue;
                }
            };
            let tempo = librosa::beat_track_tempo(&samples, librosa::SAMPLE_RATE);
            let _ = tempo;
            let bpm = py_round(tempo);
            item.set_int("bpm", bpm);
            self.log
                .info(&format!("Computed BPM for {shown}: {bpm}"));
            if write {
                crate::media::write_item(item)?;
            }
            lib.store(item)?;
        }
        let _ = cfg;
        Ok(())
    }

    /// beets' `imported`: the import stage.
    pub fn imported(&self, cfg: &Config, lib: &Library, items: &mut [Item]) -> Result<()> {
        self.calculate_bpm(cfg, lib, items, false, self.force, self.quiet)
    }
}

impl crate::plugins::Plugin for AutoBPMPlugin {
    fn name(&self) -> &'static str {
        "autobpm"
    }

    fn import_stage(
        &self,
        ctx: &crate::plugins::PluginCtx,
        task: &mut crate::plugins::ImportTask,
    ) -> Result<()> {
        if !ctx.cfg.bool("autobpm.auto") {
            return Ok(());
        }
        // beets runs import stages with the plugin's logger at WARNING.
        self.log.at_level(crate::plugins::WARNING, || {
            self.imported(ctx.cfg, ctx.lib, task.items)
        })?;
        // `calculate_bpm` stores the items; the importer reloads them.
        Ok(())
    }
}

/// `round()`: Python rounds halves to even.
fn py_round(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        // Banker's rounding.
        let t = x.trunc();
        if (t as i64) % 2 == 0 {
            t as i64
        } else {
            t as i64 + x.signum() as i64
        }
    } else {
        r as i64
    }
}

/// `beet autobpm`'s options.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    /// `-f/--force`: overwrite existing BPM values.
    pub force: bool,
    /// `-q/--quiet`: no message when an item already has a BPM.
    pub quiet: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// `beet autobpm`.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let plugin = AutoBPMPlugin::new(cfg);
    let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
    items.truncate(opts.limit.unwrap_or(usize::MAX));
    let force = plugin.force || opts.force;
    let quiet = plugin.quiet || opts.quiet;
    let write = cfg.bool("import.write");
    plugin.calculate_bpm(cfg, lib, &mut items, write, force, quiet)
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "autobpm",
        order: 82,
        defaults: r#"autobpm:
    auto: yes
    force: no
    quiet: no
    beat_track_kwargs: {}
"#,
        config_keys: &["autobpm"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "autobpm",
                build: build_autobpm,
                run: run_autobpm,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(AutoBPMPlugin::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct AutobpmArgs {
    /// Overwrite existing BPM values
    #[arg(short = 'f', long)]
    force: bool,
    /// Suppress message when item already has BPM
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_autobpm() -> clap::Command {
    use clap::Args as _;
    AutobpmArgs::augment_args(clap::Command::new("autobpm").about(
        "detect and add bpm from audio using Librosa",
    ))
}

pub fn run_autobpm(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = AutobpmArgs::from_arg_matches(m)?;
    crate::plugins::autobpm::command(
        cfg,
        lib,
        &crate::plugins::autobpm::CmdOpts {
            force: a.force,
            quiet: a.quiet,
            limit: a.limit,
            query: a.query,
        },
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
