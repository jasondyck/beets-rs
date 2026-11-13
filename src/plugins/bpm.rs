//! A port of beets 2.14's `bpm` plugin (`beetsplug/bpm.py`): `beet bpm`
//! measures a track's tempo by pressing Enter to the rhythm (or gets the
//! average of the intervals between keystrokes).

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::PluginLog;
use crate::query::Query;
use anyhow::{Result, bail};
use std::io::BufRead;

pub struct BpmPlugin {
    log: PluginLog,
}

impl Default for BpmPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl BpmPlugin {
    pub fn new() -> BpmPlugin {
        BpmPlugin {
            log: PluginLog::new("bpm"),
        }
    }
}

impl crate::plugins::Plugin for BpmPlugin {
    fn name(&self) -> &'static str {
        "bpm"
    }
}

/// One keystroke: the line read and when it was read (seconds).
pub type Stroke = (String, f64);

/// beets' `bpm`: the average BPM of the intervals between empty lines,
/// counting from the second stroke; a non-empty line breaks off.
pub fn bpm(max_strokes: usize, strokes: &mut impl Iterator<Item = Stroke>) -> f64 {
    let mut t0: Option<f64> = None;
    let mut dt = vec![];
    for _ in 0..max_strokes {
        let Some((s, t1)) = strokes.next() else {
            break;
        };
        if s.is_empty() {
            if let Some(t0) = t0 {
                dt.push(t1 - t0);
            }
            t0 = Some(t1);
        } else {
            break;
        }
    }
    let bpms: Vec<f64> = dt.iter().map(|d| 1.0 / d * 60.0).collect();
    // Python's ZeroDivisionError for a single stroke, and for a zero
    // interval.
    if bpms.is_empty() {
        return f64::NAN;
    }
    bpms.iter().sum::<f64>() / bpms.len() as f64
}

/// The real keystrokes: each line of stdin, timed.
pub struct StdinStrokes;

impl Iterator for StdinStrokes {
    type Item = Stroke;

    fn next(&mut self) -> Option<Stroke> {
        let mut line = String::new();
        let t = std::time::Instant::now();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                // `input()` strips the trailing newline only.
                let s = line
                    .strip_suffix('\n')
                    .map(|l| l.strip_suffix('\r').unwrap_or(l))
                    .unwrap_or(&line);
                Some((s.to_string(), t.elapsed().as_secs_f64()))
            }
        }
    }
}

/// beets' `get_bpm` (and `command`): measure the one matching item's BPM.
pub fn get_bpm(
    cfg: &Config,
    lib: &Library,
    items: &mut [Item],
    write: bool,
    strokes: &mut impl Iterator<Item = Stroke>,
) -> Result<()> {
    let plugin = BpmPlugin::new();
    let log = &plugin.log;
    let overwrite = cfg.bool("bpm.overwrite");
    if items.len() > 1 {
        bail!("Can only get bpm of one song at time");
    }
    if items.is_empty() {
        // beets indexes an empty list (an IndexError).
        bail!("list index out of range");
    }
    let item = &mut items[0];
    if item.int("bpm") != 0 {
        log.info(&format!("Found bpm {}", item.int("bpm")));
        if !overwrite {
            return Ok(());
        }
    }

    let max_strokes = cfg.get("bpm.max_strokes").and_then(|v| v.as_f64()).unwrap_or(3.0);
    log.info(&format!(
        "Press Enter {} times to the rhythm or Ctrl-D to exit",
        py_int(max_strokes)
    ));
    let new_bpm = bpm(max_strokes.max(0.0) as usize, strokes);
    if new_bpm.is_nan() || new_bpm.is_infinite() {
        bail!("float division by zero");
    }
    // `int(new_bpm)` truncates.
    item.set_int("bpm", new_bpm as i64);
    if write {
        crate::media::write_item(item)?;
    }
    lib.store(item)?;
    log.info(&format!("Added new bpm {}", item.int("bpm")));
    Ok(())
}

/// Python's `int()` of a number as a config value: `3.0` is `3`.
fn py_int(f: f64) -> String {
    if f.fract() == 0.0 {
        (f as i64).to_string()
    } else {
        f.to_string()
    }
}

/// `beet bpm`: the tempo of the one item matching the query.
pub fn command(cfg: &Config, lib: &Library, limit: Option<usize>, q: &[String]) -> Result<()> {
    let mut items = lib.items(&Query::parse(Kind::Item, q)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    let write = cfg.bool("import.write");
    get_bpm(cfg, lib, &mut items, write, &mut StdinStrokes)
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "bpm",
        order: 84,
        defaults: r#"bpm:
    max_strokes: 3
    overwrite: yes
"#,
        config_keys: &["bpm"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "bpm",
                build: build_bpm,
                run: run_bpm,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(BpmPlugin::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct BpmArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_bpm() -> clap::Command {
    use clap::Args as _;
    BpmArgs::augment_args(clap::Command::new("bpm").about(
        "determine bpm of a song by pressing a key to the rhythm",
    ))
}

pub fn run_bpm(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = BpmArgs::from_arg_matches(m)?;
    crate::plugins::bpm::command(cfg, lib, a.limit, &a.query)
}

#[cfg(test)]
#[path = "bpm_tests.rs"]
mod tests;
