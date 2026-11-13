//! A port of beets 2.14's `keyfinder` plugin (`beetsplug/keyfinder.py`):
//! `beet keyfinder` (and, with `keyfinder.auto`, the import) runs the
//! KeyFinder GUI program (or `keyfinder-cli`) on each item's file and
//! stores its output as `initial_key`.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::PluginLog;
use crate::query::Query;
use anyhow::Result;
use std::path::Path;

pub struct KeyFinderPlugin {
    log: PluginLog,
}

impl Default for KeyFinderPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyFinderPlugin {
    pub fn new() -> KeyFinderPlugin {
        KeyFinderPlugin {
            log: PluginLog::new("keyfinder"),
        }
    }
}

impl crate::plugins::Plugin for KeyFinderPlugin {
    fn name(&self) -> &'static str {
        "keyfinder"
    }

    /// beets' `import_stages` (only with `keyfinder.auto`): the plugin
    /// computes keys during the import, without writing tags (the
    /// importer writes them).
    fn import_stage(
        &self,
        ctx: &crate::plugins::PluginCtx,
        task: &mut crate::plugins::ImportTask,
    ) -> Result<()> {
        if !ctx.cfg.bool("keyfinder.auto") {
            return Ok(());
        }
        let mut items = task.items.to_vec();
        let changed = self.find_key(ctx.cfg, &mut items, false)?;
        for (i, it) in items.into_iter().enumerate() {
            if changed[i] {
                task.items[i] = it;
            }
        }
        Ok(())
    }
}

impl KeyFinderPlugin {
    /// The command line beets builds: the configured binary, plus `-f`
    /// before the path unless it is `keyfinder-cli`.
    fn command(bin: &str) -> Vec<String> {
        let mut command = vec![bin.to_string()];
        let base = Path::new(bin)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !base.contains("keyfinder-cli") {
            // The KeyFinder GUI program needs the -f flag before the path.
            command.push("-f".into());
        }
        command
    }

    /// beets' `find_key`. Returns, per item, whether it changed (so the
    /// caller can store it).
    pub fn find_key(&self, cfg: &Config, items: &mut [Item], write: bool) -> Result<Vec<bool>> {
        let overwrite = cfg.bool("keyfinder.overwrite");
        let command = Self::command(&cfg.str("keyfinder.bin"));
        let mut changed = vec![false; items.len()];
        for (i, item) in items.iter_mut().enumerate() {
            if !item.str("initial_key").is_empty() && !overwrite {
                continue;
            }
            let path = item.path();
            let output = match run_output(&command, path) {
                Ok(out) => out,
                Err(e) => {
                    self.log.error(&format!("execution failed: {e}"));
                    continue;
                }
            };
            // `output.rsplit(None, 1)[-1]`: the last whitespace-separated
            // word.
            let Some(key_raw) = output
                .split(|b: &u8| b.is_ascii_whitespace())
                .rfind(|w| !w.is_empty())
            else {
                self.log
                    .error(&format!("no key returned for path: {}", display(path)));
                continue;
            };
            let Ok(key) = std::str::from_utf8(key_raw) else {
                self.log.error("output is invalid UTF-8");
                continue;
            };
            let key = key.to_string();
            let shown = display(path);
            item.set_str("initial_key", &key);
            self.log
                .info(&format!("added computed initial key {key} for {shown}"));
            if write {
                crate::media::write_item(item)?;
            }
            changed[i] = true;
        }
        Ok(changed)
    }
}

/// `util.displayable_path`.
fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Python's `repr()` of a list of strings, for the CalledProcessError
/// message.
fn py_list_repr(items: &[String]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|s| {
            let escaped = s.replace('\\', "\\\\").replace('\'', "\\'");
            format!("'{escaped}'")
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// beets' `util.command_output`: the command's stdout, with the error
/// messages Python's `OSError` and `CalledProcessError` print.
fn run_output(command: &[String], target: &Path) -> std::result::Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt;
    let args: Vec<std::ffi::OsString> = command[1..]
        .iter()
        .map(std::ffi::OsString::from)
        .chain(std::iter::once(std::ffi::OsString::from(
            std::ffi::OsStr::from_bytes(target.as_os_str().as_bytes()),
        )))
        .collect();
    match std::process::Command::new(&command[0]).args(&args).output() {
        Ok(out) if out.status.success() => Ok(out.stdout),
        Ok(out) => {
            let mut full = vec![command[0].clone()];
            full.extend(args.iter().map(|a| a.to_string_lossy().into_owned()));
            Err(format!(
                "Command {} returned non-zero exit status {:?}.",
                py_list_repr(&full),
                out.status.code().unwrap_or(-1)
            ))
        }
        Err(e) => {
            let errno = e.raw_os_error().unwrap_or(0);
            let strerror = std::io::Error::from_raw_os_error(errno).to_string();
            // "[Errno 2] No such file or directory: 'KeyFinder'"
            let msg = strerror
                .split(" (os error")
                .next()
                .unwrap_or(&strerror)
                .to_string();
            Err(format!("[Errno {errno}] {msg}: '{}'", command[0]))
        }
    }
}

/// `beet keyfinder`: compute the keys of the matching items.
pub fn command(cfg: &Config, lib: &Library, limit: Option<usize>, q: &[String]) -> Result<()> {
    let plugin = KeyFinderPlugin::new();
    let mut items = lib.items(&Query::parse(Kind::Item, q)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    let write = cfg.bool("import.write");
    let changed = plugin.find_key(cfg, &mut items, write)?;
    for (it, c) in items.iter().zip(changed) {
        if c {
            lib.store(it)?;
        }
    }
    Ok(())
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "keyfinder",
        order: 85,
        defaults: r#"keyfinder:
    bin: KeyFinder
    auto: yes
    overwrite: no
"#,
        config_keys: &["keyfinder"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 570,
                plugin: "keyfinder",
                build: build_keyfinder,
                run: run_keyfinder,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(KeyFinderPlugin::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct KeyfinderArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_keyfinder() -> clap::Command {
    use clap::Args as _;
    KeyfinderArgs::augment_args(clap::Command::new("keyfinder").about(
        "Detect and add initial key from audio (plugin: keyfinder)",
    ))
}

pub fn run_keyfinder(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = KeyfinderArgs::from_arg_matches(m)?;
    crate::plugins::keyfinder::command(cfg, lib, a.limit, &a.query)
}

#[cfg(test)]
#[path = "keyfinder_tests.rs"]
mod tests;
