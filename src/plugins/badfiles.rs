//! A port of beets 2.14's `badfiles` plugin (`beetsplug/badfiles.py`):
//! `beet bad [-v] [-l N] [QUERY]` runs a checker on each item's file
//! (`mp3val` for MP3, `flac -wst` for FLAC, or the `commands` setting's
//! command for the extension, with the path appended); with
//! `check_on_import`, the importer checks each task's files and, before
//! the match prompt, shows what failed and aborts, skips or continues as
//! `import_action_on_error` / `import_action_on_warning` say (or asks).

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::{BeforeChoice, PluginCtx, PluginLog};
use crate::query::Query;
use crate::ui::colorize;
use anyhow::{Result, bail};
use serde_yaml::Value as Y;
use std::io::Write;
use std::path::Path;

pub struct BadFiles {
    cfg: Config,
    log: PluginLog,
}

pub fn load(cfg: &Config) -> BadFiles {
    BadFiles {
        cfg: cfg.clone(),
        log: PluginLog::new("badfiles"),
    }
}

/// A checker's result: (exit status, error count, output lines).
type Checked = (i32, usize, Vec<String>);

/// Python's `subprocess.list2cmdline`.
fn list2cmdline(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if !a.is_empty() && !a.contains([' ', '\t', '"']) {
                return a.clone();
            }
            let mut out = String::from('"');
            let mut backslashes = 0;
            for c in a.chars() {
                match c {
                    '\\' => backslashes += 1,
                    '"' => {
                        out.push_str(&"\\".repeat(backslashes * 2 + 1));
                        out.push('"');
                        backslashes = 0;
                    }
                    c => {
                        out.push_str(&"\\".repeat(backslashes));
                        backslashes = 0;
                        out.push(c);
                    }
                }
            }
            out.push_str(&"\\".repeat(backslashes * 2));
            out.push('"');
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A checker that couldn't be run (`CheckerCommandError`).
struct CheckerError {
    checker: String,
    path: String,
    not_found: bool,
    msg: String,
}

impl BadFiles {
    /// `run_command`: (status, errors, non-empty output lines).
    fn run_command(&self, cmd: &[String]) -> std::result::Result<Checked, CheckerError> {
        self.log
            .debug(&format!("running command: {}", list2cmdline(cmd)));
        let out = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(std::process::Stdio::null())
            .output();
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                return Err(CheckerError {
                    checker: cmd[0].clone(),
                    path: cmd.last().cloned().unwrap_or_default(),
                    not_found: e.kind() == std::io::ErrorKind::NotFound,
                    msg: e.to_string(),
                });
            }
        };
        // `stderr=STDOUT`: both streams, as one (stdout first here).
        let mut text = out.stdout;
        text.extend(out.stderr);
        let text = String::from_utf8_lossy(&text).into_owned();
        let lines: Vec<String> = text
            .split('\n')
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        Ok(match out.status.code() {
            Some(0) => (0, 0, lines),
            Some(n) => (n, 1, lines),
            None => {
                use std::os::unix::process::ExitStatusExt;
                (-out.status.signal().unwrap_or(0), 1, lines)
            }
        })
    }

    /// The checker for an extension: the `commands` setting's, else
    /// `mp3val` / `flac -wst` (the bool: the built-in mp3val, whose output
    /// is filtered to its warnings).
    fn checker(&self, ext: &str) -> Option<(Vec<String>, bool)> {
        let ext = ext.to_lowercase();
        if let Some(Y::Mapping(m)) = self.cfg.get("badfiles.commands")
            && let Some(cmd) = m.get(Y::String(ext.clone()))
        {
            let cmd = match cmd {
                Y::String(s) => s.clone(),
                other => serde_yaml::to_string(other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
            return Some((
                crate::plugins::duplicates::shlex_split(&cmd).unwrap_or_default(),
                false,
            ));
        }
        match ext.as_str() {
            "mp3" => Some((vec!["mp3val".into()], true)),
            "flac" => Some((vec!["flac".into(), "-wst".into()], false)),
            _ => None,
        }
    }

    /// `check_item`: the lines to show for the item's file (none when it
    /// passed, unless `verbose`).
    pub fn check_item(&self, item: &Item, verbose: bool, out: &mut impl Write) -> Vec<String> {
        let dpath = item.path().display().to_string();
        self.log.debug(&format!("checking path: {dpath}"));
        if !item.path().exists() {
            let _ = writeln!(
                out,
                "{}: file does not exist",
                colorize("text_error", &dpath)
            );
        }
        let ext = Path::new(&dpath)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some((mut cmd, is_mp3val)) = self.checker(&ext) else {
            self.log
                .error(&format!("no checker specified in the config for {ext}"));
            return vec![];
        };
        cmd.push(dpath.clone());
        let (status, mut errors, mut output) = match self.run_command(&cmd) {
            Ok(r) => r,
            Err(e) => {
                if e.not_found {
                    self.log.error(&format!(
                        "command not found: {} when validating file: {}",
                        e.checker, e.path
                    ));
                } else {
                    self.log
                        .error(&format!("error invoking {}: {}", e.checker, e.msg));
                }
                return vec![];
            }
        };
        if is_mp3val && status == 0 {
            // `check_mp3val`: only its warnings count.
            output.retain(|l| l.starts_with("WARNING:"));
            errors = output.len();
        }
        let mut lines = vec![];
        if status > 0 {
            lines.push(format!(
                "{}: checker exited with status {status}",
                colorize("text_error", &dpath)
            ));
            lines.extend(output.iter().map(|l| format!("  {l}")));
        } else if errors > 0 {
            lines.push(format!(
                "{}: checker found {errors} errors or warnings",
                colorize("text_warning", &dpath)
            ));
            lines.extend(output.iter().map(|l| format!("  {l}")));
        } else if verbose {
            lines.push(format!("{}: ok", colorize("text_success", &dpath)));
        }
        lines
    }

    /// The action setting: `ask`, `abort`, `skip` or `continue`.
    fn action(&self, which: &str) -> Result<String> {
        let key = format!("badfiles.import_action_on_{which}");
        let v = self.cfg.str(&key);
        if !["ask", "abort", "skip", "continue"].contains(&v.as_str()) {
            bail!(
                "{key}: must be one of ['ask', 'abort', 'skip', 'continue'], not {}",
                crate::hooks::py_repr(&v)
            );
        }
        Ok(v)
    }

    /// `handle_import_action`.
    fn handle(action: &str, failure: &str) -> BeforeChoice {
        let name = match action {
            "abort" => "Aborting",
            "skip" => "Skipping",
            _ => "Continuing",
        };
        println!(
            "{} due to import_action_on_{failure} configuration",
            colorize("text_warning", name)
        );
        match action {
            "abort" => BeforeChoice::Abort,
            "skip" => BeforeChoice::Skip,
            _ => BeforeChoice::Continue,
        }
    }

    /// `on_import_task_before_choice` for the failed checks.
    pub fn before_choice(&self, failed: &[Vec<String>], quiet: bool) -> Result<BeforeChoice> {
        if failed.is_empty() {
            return Ok(BeforeChoice::Continue);
        }
        let warning_action = self.action("warning")?;
        let error_action = self.action("error")?;
        println!(
            "{} one or more files failed checks:",
            colorize("text_warning", "BAD")
        );
        let (mut found_warning, mut found_error) = (false, false);
        for lines in failed {
            for line in lines {
                let low = line.to_lowercase();
                if low.contains("checker found 0 errors or warnings") {
                    continue;
                }
                found_warning |= low.contains("warning");
                found_error |= low.contains("error");
                println!("{line}");
            }
        }
        if found_error && error_action != "ask" {
            return Ok(Self::handle(&error_action, "error"));
        }
        if found_warning && warning_action != "ask" {
            return Ok(Self::handle(&warning_action, "warning"));
        }
        if quiet {
            return Ok(BeforeChoice::Continue);
        }
        println!();
        println!("What would you like to do?");
        let sel = crate::ui::options(
            &["aBort", "skip", "continue"],
            None,
            Some(crate::ui::Choice::Letter('b')),
        );
        Ok(match sel {
            Some(crate::ui::Choice::Letter('s')) => BeforeChoice::Skip,
            Some(crate::ui::Choice::Letter('c')) => BeforeChoice::Continue,
            _ => BeforeChoice::Abort,
        })
    }
}

impl crate::plugins::Plugin for BadFiles {
    fn name(&self) -> &'static str {
        "badfiles"
    }

    /// `import_task_start` (the checks) and `import_task_before_choice`.
    fn import_task_before_choice(&self, ctx: &PluginCtx, items: &[Item]) -> Result<BeforeChoice> {
        if !self.cfg.bool("badfiles.check_on_import") {
            return Ok(BeforeChoice::Continue);
        }
        let mut failed = vec![];
        let mut stdout = std::io::stdout();
        for item in items {
            let lines = self.check_item(item, false, &mut stdout);
            if !lines.is_empty() {
                failed.push(lines);
            }
        }
        self.before_choice(&failed, ctx.cfg.bool("import.quiet"))
    }
}

/// `beet bad`.
pub fn command(
    cfg: &Config,
    lib: &Library,
    verbose: bool,
    limit: Option<usize>,
    query: &[String],
) -> Result<()> {
    let b = load(cfg);
    let mut items = lib.items(&Query::parse(Kind::Item, query)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    let mut stdout = std::io::stdout();
    for item in &items {
        for line in b.check_item(item, verbose, &mut stdout) {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "badfiles_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "badfiles",
        order: 0,
        defaults: r#"badfiles:
    check_on_import: no
    import_action_on_warning: ask
    import_action_on_error: ask
"#,
        config_keys: &["badfiles"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "badfiles",
                build: build_bad,
                run: run_bad,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct BadArgs {
    /// View results for both the bad and uncorrupted files
    #[arg(short = 'v', long)]
    verbose: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_bad() -> clap::Command {
    use clap::Args as _;
    BadArgs::augment_args(
        clap::Command::new("bad").about("check for corrupt or missing files"),
    )
}

pub fn run_bad(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = BadArgs::from_arg_matches(m)?;
    crate::plugins::badfiles::command(cfg, lib, a.verbose, a.limit, &a.query)
}
