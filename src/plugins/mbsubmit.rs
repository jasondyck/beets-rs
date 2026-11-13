//! A port of beets 2.14's `mbsubmit` plugin (`beetsplug/mbsubmit.py`):
//! `beet mbsubmit` prints the matched items as a MusicBrainz track parser
//! listing [1], and the importer prompt offers "Print tracks" and "Open
//! files with Picard" for matches the recommendation isn't good enough
//! for.
//!
//! [1] <https://wiki.musicbrainz.org/History:How_To_Parse_Track_Listings>

use crate::autotag::Recommendation;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::{ChoiceTask, PluginLog, PromptChoice};
use crate::query::Query;
use crate::template::Template;
use anyhow::{Result, bail};
use std::io::Write;

pub struct MBSubmitPlugin {
    format: String,
    threshold: Recommendation,
    picard_path: String,
    log: PluginLog,
}

/// beets' `Recommendation` names, as `mbsubmit.threshold` chooses them.
fn rec_choices() -> [(&'static str, Recommendation); 4] {
    [
        ("none", Recommendation::None),
        ("low", Recommendation::Low),
        ("medium", Recommendation::Medium),
        ("strong", Recommendation::Strong),
    ]
}

impl MBSubmitPlugin {
    /// The plugin's `__init__`: the config, with `threshold` validated as
    /// beets' `as_choice` does.
    pub fn new(cfg: &Config) -> Result<MBSubmitPlugin> {
        let value = cfg.str("mbsubmit.threshold");
        let choices = rec_choices();
        let names: Vec<&str> = choices.iter().map(|(n, _)| *n).collect();
        let Some((_, threshold)) = choices.iter().find(|(n, _)| *n == value) else {
            bail!("mbsubmit.threshold: must be one of {names:?}, not {value:?}");
        };
        Ok(MBSubmitPlugin {
            format: cfg.str("mbsubmit.format"),
            threshold: *threshold,
            picard_path: cfg.str("mbsubmit.picard_path"),
            log: PluginLog::new("mbsubmit"),
        })
    }

    /// beets' `_item_sort_key`: disc then track, missing values as 0.
    fn sort_key(item: &Item) -> (i64, i64) {
        (item.int("disc"), item.int("track"))
    }

    fn sorted(items: &[Item]) -> Vec<&Item> {
        let mut v: Vec<&Item> = items.iter().collect();
        // A stable sort, as Python's `sorted`.
        v.sort_by_key(|i| Self::sort_key(i));
        v
    }

    /// The item's `format(item, fmt)`.
    fn format_item(&self, item: &Item) -> String {
        crate::commands::format_record(&Template::parse(&self.format), item, None)
    }

    /// beets' `print_tracks`: the prompt choice's callback.
    fn print_tracks(&self, items: &[Item], out: &mut impl Write) -> Result<()> {
        for item in Self::sorted(items) {
            writeln!(out, "{}", self.format_item(item))?;
        }
        Ok(())
    }

    /// beets' `picard`: launch Picard on the task's files.
    fn picard(&self, paths: &[std::path::PathBuf]) -> Result<()> {
        match std::process::Command::new(&self.picard_path)
            .args(paths)
            .spawn()
        {
            Ok(_) => self.log.info(&format!(
                "launched picard from\n{}",
                self.picard_path
            )),
            // beets catches OSError and logs "Could not open picard, got
            // error:\n{exc}".
            Err(e) => self.log.error(&format!("Could not open picard, got error:\n{e}")),
        }
        Ok(())
    }

    /// beets' `_mbsubmit`: print the items for the track parser.
    pub fn mbsubmit(&self, items: &[Item], out: &mut impl Write) -> Result<()> {
        self.print_tracks(items, out)
    }
}

impl crate::plugins::Plugin for MBSubmitPlugin {
    fn name(&self) -> &'static str {
        "mbsubmit"
    }

    /// beets' `before_choose_candidate` listener: the choices appear when
    /// the recommendation is at most the threshold.
    fn prompt_choices(&self, task: &ChoiceTask) -> Vec<PromptChoice> {
        match task.rec {
            Some(rec) if rec <= self.threshold => vec![
                PromptChoice {
                    short: 'p',
                    long: "Print tracks",
                },
                PromptChoice {
                    short: 'o',
                    long: "Open files with Picard",
                },
            ],
            _ => vec![],
        }
    }

    fn run_prompt_choice(&self, short: char, task: &mut ChoiceTask) -> Result<bool> {
        match short {
            'p' => {
                let mut out = std::io::stdout().lock();
                self.print_tracks(task.items, &mut out)?;
                out.flush().ok();
            }
            'o' => self.picard(task.paths)?,
            _ => {}
        }
        // Both callbacks return None, so beets shows the prompt again.
        Ok(false)
    }
}

/// `beet mbsubmit`: print the tracks matching the query.
pub fn command(cfg: &Config, lib: &Library, limit: Option<usize>, q: &[String]) -> Result<()> {
    let plugin = MBSubmitPlugin::new(cfg)?;
    let mut items = lib.items(&Query::parse(Kind::Item, q)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    let mut out = std::io::stdout().lock();
    plugin.mbsubmit(&items, &mut out)?;
    out.flush().ok();
    Ok(())
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "mbsubmit",
        order: 86,
        defaults: r#"mbsubmit:
    format: $track. $title - $artist ($length)
    threshold: medium
    picard_path: picard
"#,
        config_keys: &["mbsubmit"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "mbsubmit",
                build: build_mbsubmit,
                run: run_mbsubmit,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(MBSubmitPlugin::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct MbsubmitArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_mbsubmit() -> clap::Command {
    use clap::Args as _;
    MbsubmitArgs::augment_args(clap::Command::new("mbsubmit").about(
        "Submit Tracks to MusicBrainz",
    ))
}

pub fn run_mbsubmit(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = MbsubmitArgs::from_arg_matches(m)?;
    crate::plugins::mbsubmit::command(cfg, lib, a.limit, &a.query)
}

#[cfg(test)]
#[path = "mbsubmit_tests.rs"]
mod tests;
