mod artresizer;
mod autotag;
mod commands;
mod config;
mod diff;
mod display;
mod duplicates;
mod events;
mod fileops;
mod formatted;
mod hooks;
mod importer;
mod inline;
mod layout;
mod library;
mod logging;
mod mb;
mod media;
mod mediafile;
mod migrate;
mod model;
mod paths;
mod pipeline;
mod plugin_registry;
mod plugins;
mod props;
mod pyyaml;
mod query;
mod state;
mod template;
mod ui;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgAction, FromArgMatches, Parser, Subcommand};
use std::path::PathBuf;

/// A Rust port of the beets music library manager.
///
/// Global options go before the command, as in beets.
#[derive(Parser, Debug)]
#[command(
    name = "beet",
    about,
    disable_version_flag = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Library database file to use
    #[arg(short = 'l', long = "library")]
    pub library: Option<PathBuf>,
    /// Destination music directory
    #[arg(short = 'd', long = "directory")]
    pub directory: Option<PathBuf>,
    /// Log more details (use twice for even more)
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    pub verbose: u8,
    /// Path to configuration file (repeatable; each goes over the last)
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: Vec<PathBuf>,
    /// A comma-separated list of plugins to load
    #[arg(short = 'p', long = "plugins", value_name = "PLUGINS")]
    pub plugins: Option<String>,
    /// A comma-separated list of plugins to disable
    #[arg(short = 'P', long = "disable-plugins", value_name = "PLUGINS")]
    pub disable_plugins: Option<String>,
    /// Same as the version command
    #[arg(long = "version", hide = true)]
    pub version: bool,
    /// The command; with none, show help (as beets does)
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

/// beets' comma-separated list options (`-p`, `-P`): empty entries dropped.
pub fn comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Import new music
    #[command(visible_alias = "imp", visible_alias = "im")]
    Import {
        /// Copy files into the library directory
        #[arg(short = 'c', long = "copy", overrides_with = "nocopy")]
        copy: bool,
        /// Don't copy files
        #[arg(short = 'C', long = "nocopy")]
        nocopy: bool,
        /// Move files into the library directory (overrides -c)
        #[arg(short = 'm', long = "move", overrides_with = "nomove")]
        mv: bool,
        /// Don't move files into the library (overrides -m)
        #[arg(short = 'M', long = "nomove")]
        nomove: bool,
        /// Write new metadata to files' tags
        #[arg(short = 'w', long = "write", overrides_with = "nowrite")]
        write: bool,
        /// Don't write metadata
        #[arg(short = 'W', long = "nowrite")]
        nowrite: bool,
        /// Never prompt: apply strong matches, otherwise use quiet_fallback
        #[arg(short = 'q', long)]
        quiet: bool,
        /// Decision in quiet mode when there's no strong match
        #[arg(long = "quiet-fallback", value_parser = ["skip", "asis"])]
        quiet_fallback: Option<String>,
        /// File to log untaggable albums to for later review
        #[arg(short = 'l', long = "log", value_name = "FILE")]
        log: Option<PathBuf>,
        /// Always confirm matches, even strong ones
        #[arg(short = 't', long)]
        timid: bool,
        /// Infer tags for imported files (default)
        #[arg(short = 'a', long = "autotag", overrides_with = "noautotag")]
        autotag: bool,
        /// Don't autotag; import files as-is
        #[arg(short = 'A', long = "noautotag")]
        noautotag: bool,
        /// Import individual tracks instead of albums
        #[arg(short = 's', long)]
        singletons: bool,
        /// Split each directory into albums by its tracks' artist and album tags
        #[arg(short = 'g', long = "group-albums")]
        group_albums: bool,
        /// Restrict matching to this MusicBrainz release/recording ID (repeatable)
        #[arg(short = 'S', long = "search-id")]
        search_id: Vec<String>,
        /// Import an entire tree as a single album
        #[arg(long)]
        flat: bool,
        /// Just print the files to import
        #[arg(long)]
        pretend: bool,
        /// Set the given fields to the supplied values (repeatable)
        #[arg(long = "set", value_name = "FIELD=VALUE")]
        set: Vec<String>,
        /// Skip directories imported by earlier incremental imports
        #[arg(short = 'i', long, overrides_with = "noincremental")]
        incremental: bool,
        /// Import everything, even previously imported directories
        #[arg(short = 'I', long)]
        noincremental: bool,
        /// Don't record skipped albums during an incremental import
        #[arg(
            short = 'R',
            long = "incremental-skip-later",
            overrides_with = "noincremental_skip_later"
        )]
        incremental_skip_later: bool,
        /// Record skipped albums during an incremental import
        #[arg(short = 'r', long = "noincremental-skip-later")]
        noincremental_skip_later: bool,
        /// Resume an interrupted import without asking
        #[arg(short = 'p', long, overrides_with = "noresume")]
        resume: bool,
        /// Start over instead of resuming an interrupted import
        #[arg(short = 'P', long)]
        noresume: bool,
        /// Retag items matching a library query instead of importing paths
        #[arg(short = 'L', long)]
        library: bool,
        /// Erase existing metadata before applying new metadata
        #[arg(long = "from-scratch")]
        from_scratch: bool,
        /// Paths to import (with -L: a library query)
        paths: Vec<PathBuf>,
    },
    /// Query the library
    #[command(visible_alias = "ls")]
    List {
        /// Show matching albums instead of tracks
        #[arg(short = 'a', long)]
        album: bool,
        /// Print paths for matched items or albums
        #[arg(short = 'p', long)]
        path: bool,
        /// Print with a custom format template
        #[arg(short = 'f', long)]
        format: Option<String>,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        query: Vec<String>,
    },
    /// Change metadata fields: `beet modify QUERY field=value field!`
    #[command(visible_alias = "mod")]
    Modify {
        /// Match albums instead of tracks
        #[arg(short = 'a', long)]
        album: bool,
        /// Move files in the library directory
        #[arg(short = 'm', long = "move", overrides_with = "nomove")]
        mv: bool,
        /// Don't move files in library
        #[arg(short = 'M', long = "nomove", overrides_with = "mv")]
        nomove: bool,
        /// Write new metadata to files' tags (default)
        #[arg(short = 'w', long = "write", overrides_with = "nowrite")]
        write: bool,
        /// Don't write metadata (opposite of -w)
        #[arg(short = 'W', long = "nowrite", overrides_with = "write")]
        nowrite: bool,
        /// Print with custom format
        #[arg(short = 'f', long)]
        format: Option<String>,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
        /// When modifying albums, don't also change item data
        #[arg(short = 'I', long = "noinherit")]
        noinherit: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        /// Query terms, `field=value` assignments and `field!` deletions
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove matching items from the library
    #[command(visible_alias = "rm")]
    Remove {
        #[arg(short = 'a', long)]
        album: bool,
        /// Also remove files from disk
        #[arg(short = 'd', long)]
        delete: bool,
        /// Do not ask when removing items
        #[arg(short = 'f', long)]
        force: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Move or copy items to their library paths (or another directory)
    #[command(visible_alias = "mv")]
    Move {
        #[arg(short = 'a', long)]
        album: bool,
        /// Destination directory
        #[arg(short = 'd', long = "dest", value_name = "DIR")]
        dest: Option<PathBuf>,
        /// Copy instead of moving
        #[arg(short = 'c', long)]
        copy: bool,
        /// Show how files would be moved, but don't touch anything
        #[arg(short = 'p', long)]
        pretend: bool,
        /// Always confirm all actions
        #[arg(short = 't', long)]
        timid: bool,
        /// Copy without changing the database path
        #[arg(short = 'e', long)]
        export: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Re-read tags from files, dropping items whose files are gone
    #[command(visible_alias = "upd", visible_alias = "up")]
    Update {
        /// Match albums instead of tracks
        #[arg(short = 'a', long)]
        album: bool,
        /// Print with custom format
        #[arg(short = 'f', long)]
        format: Option<String>,
        /// Move files in the library directory
        #[arg(short = 'm', long = "move", overrides_with = "nomove")]
        mv: bool,
        /// Don't move files in library
        #[arg(short = 'M', long = "nomove", overrides_with = "mv")]
        nomove: bool,
        /// Show all changes but do nothing
        #[arg(short = 'p', long)]
        pretend: bool,
        /// Field to update (repeatable; default: all)
        #[arg(short = 'F', long = "field", value_name = "FIELD")]
        fields: Vec<String>,
        /// Field to exclude from updates (repeatable)
        #[arg(short = 'e', long = "exclude-field", value_name = "FIELD")]
        exclude_fields: Vec<String>,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Write tag information to files
    Write {
        /// Show all changes but do nothing
        #[arg(short = 'p', long)]
        pretend: bool,
        /// Write tags even if the existing tags match the database
        #[arg(short = 'f', long)]
        force: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Show library statistics
    Stats {
        /// Exact size and time
        #[arg(short = 'e', long)]
        exact: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// List available fields
    Fields,
    /// Copy a beets library database (opened read-only) into this library
    #[command(name = "migrate-beets")]
    MigrateBeets {
        /// Take beets' library directory from this beets config file
        #[arg(long = "beets-config", value_name = "PATH")]
        beets_config: Option<PathBuf>,
        /// beets' library directory (relative paths in its database are under it)
        #[arg(short = 'd', long, value_name = "DIR")]
        directory: Option<PathBuf>,
        /// Replace what this library holds (backed up first)
        #[arg(short = 'f', long)]
        force: bool,
        /// The beets library database (library.db)
        database: PathBuf,
    },
    /// Output version information
    Version,
    /// Give detailed help on a specific sub-command
    #[command(visible_alias = "?")]
    Help {
        /// The command to explain
        command: Option<String>,
    },
    /// Show the effective configuration
    Config {
        /// Show files that configuration was loaded from
        #[arg(short = 'p', long)]
        paths: bool,
        /// Edit user configuration with $VISUAL (or $EDITOR)
        #[arg(short = 'e', long)]
        edit: bool,
        /// Include the default configuration
        #[arg(short = 'd', long)]
        defaults: bool,
        /// Do not redact sensitive fields
        #[arg(short = 'c', long)]
        clear: bool,
    },
}

/// One option a subcommand knows (as optparse would see it).
struct OptInfo {
    short: Option<char>,
    long: Option<String>,
    takes_value: bool,
}

fn options_of(cmd: &clap::Command) -> Vec<OptInfo> {
    let mut out: Vec<OptInfo> = cmd
        .get_arguments()
        .filter_map(|a| {
            if a.get_index().is_some() {
                return None;
            }
            let short = a.get_short();
            let long = a.get_long().map(str::to_string);
            if short.is_none() && long.is_none() {
                return None;
            }
            let takes_value = matches!(a.get_action(), ArgAction::Set | ArgAction::Append);
            Some(OptInfo {
                short,
                long,
                takes_value,
            })
        })
        .collect();
    // clap adds `-h/--help` when it builds the command, which happens after
    // this inspection; every command has it.
    if !out.iter().any(|o| o.long.as_deref() == Some("help")) {
        out.push(OptInfo {
            short: Some('h'),
            long: Some("help".into()),
            takes_value: false,
        });
    }
    out
}

enum PreError {
    Unknown(String),
    Ambiguous(String, Vec<String>),
    /// An option that needs a value at the end of the command line.
    Missing(String),
}

/// beets' `OptionParser.error` text for an option error.
fn option_error(err: PreError) -> String {
    match err {
        PreError::Unknown(opt) => format!("no such option: {opt}"),
        PreError::Ambiguous(opt, possibilities) => {
            format!("ambiguous option: {opt} ({}?)", possibilities.join(", "))
        }
        PreError::Missing(opt) => format!("{opt} option requires 1 argument"),
    }
}

fn root_usage() -> String {
    "Usage: \n  beet COMMAND [ARGS...]\n  beet help COMMAND".to_string()
}

fn command_usage(name: &str) -> String {
    if name == "list" {
        "Usage: beet [options]\nExample: beet list -f '$album: $title' artist:beatles".to_string()
    } else {
        format!("Usage: beet {name} [options]")
    }
}

fn exit_option_error(command: Option<&str>, err: PreError) -> ! {
    let (usage, prog) = match command {
        Some(name) => (command_usage(name), format!("beet {name}")),
        None => (root_usage(), "beet".to_string()),
    };
    eprintln!("{usage}\n\n{prog}: error: {}", option_error(err));
    std::process::exit(2);
}

/// Match a long option by exact name or optparse-style unambiguous prefix;
/// return its full name.
fn match_long(name: &str, opts: &[OptInfo]) -> Result<String, PreError> {
    let names: Vec<&String> = opts.iter().filter_map(|o| o.long.as_ref()).collect();
    if let Some(exact) = names.iter().find(|n| n.as_str() == name) {
        return Ok((*exact).clone());
    }
    let possibilities: Vec<String> = names
        .iter()
        .filter(|n| n.starts_with(name))
        .map(|n| format!("--{n}"))
        .collect();
    match possibilities.len() {
        1 => Ok(possibilities[0].trim_start_matches("--").to_string()),
        0 => Err(PreError::Unknown(format!("--{name}"))),
        _ => {
            let mut sorted = possibilities;
            sorted.sort();
            Err(PreError::Ambiguous(format!("--{name}"), sorted))
        }
    }
}

/// Validate/expand one option token (and its value, if separate), as
/// optparse parses it: short options cluster left to right, a value-taking
/// option takes the rest of the cluster or the next argument (even one
/// starting with `-`), and long options may be abbreviated.
fn process_option(
    args: &[std::ffi::OsString],
    i: &mut usize,
    opts: &[OptInfo],
    out: &mut Vec<std::ffi::OsString>,
    command: Option<&str>,
) {
    let token = args[*i].to_string_lossy().into_owned();
    let fail = |e: PreError| -> ! { exit_option_error(command, e) };
    if let Some(body) = token.strip_prefix("--") {
        let (name, inline) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        let full = match_long(name, opts).unwrap_or_else(|e| fail(e));
        let info = opts
            .iter()
            .find(|o| o.long.as_deref() == Some(&full))
            .unwrap();
        let mut new = std::ffi::OsString::from(format!("--{full}"));
        if info.takes_value {
            match inline {
                Some(v) => {
                    new.push("=");
                    new.push(v);
                    *i += 1;
                }
                None if *i + 1 < args.len() => {
                    new.push("=");
                    new.push(&args[*i + 1]);
                    *i += 2;
                }
                None => fail(PreError::Missing(format!("--{full}"))),
            }
        } else {
            *i += 1;
        }
        out.push(new);
        return;
    }
    // Short options (or `-` alone, which is positional).
    let cluster: Vec<char> = token[1..].chars().collect();
    let mut new = std::ffi::OsString::from("-");
    let mut j = 0;
    while j < cluster.len() {
        let c = cluster[j];
        let Some(info) = opts.iter().find(|o| o.short == Some(c)) else {
            fail(PreError::Unknown(format!("-{c}")));
        };
        new.push(c.to_string());
        if info.takes_value {
            let rest: String = cluster[j + 1..].iter().collect();
            if !rest.is_empty() {
                new.push(rest);
                j = cluster.len();
            } else if *i + 1 < args.len() {
                // The value is the next argument, even an empty one or one
                // starting with `-`: use the long `--option=value` form so
                // clap sees it as a value.
                // Any flags before this option in the cluster stay as
                // their own token; the value-taking option becomes
                // `--option=value`.
                let prefix: String = cluster[..j].iter().collect();
                if !prefix.is_empty() {
                    out.push(std::ffi::OsString::from(format!("-{prefix}")));
                }
                new = std::ffi::OsString::from("-");
                match &info.long {
                    Some(long) => {
                        let mut tok = std::ffi::OsString::from(format!("--{long}="));
                        tok.push(&args[*i + 1]);
                        out.push(tok);
                    }
                    None => {
                        // clap doesn't take attached short values: pass
                        // `-n value` as two arguments.
                        out.push(std::ffi::OsString::from(format!("-{c}")));
                        out.push(args[*i + 1].clone());
                    }
                }
                *i += 1;
                j = cluster.len();
            } else {
                fail(PreError::Missing(format!("-{c}")));
            }
        } else {
            j += 1;
        }
    }
    *i += 1;
    if !new.is_empty() && new != "-" {
        out.push(new);
    }
}

/// The plugins enabled by the configuration and the global options, for
/// knowing whether a plugin command is there at all (beets registers plugin
/// commands only for enabled plugins). A configuration that doesn't load
/// enables none; the real command reports the error later.
fn enabled_plugins(raw: &[std::ffi::OsString], root_opts: &[OptInfo]) -> Vec<String> {
    let mut overlays: Vec<PathBuf> = vec![];
    let mut plugins: Option<std::ffi::OsString> = None;
    let mut disabled: Option<std::ffi::OsString> = None;
    let mut i = 1;
    while i < raw.len() {
        let s = raw[i].to_string_lossy().into_owned();
        if s == "--" || s == "-" || !s.starts_with('-') {
            break;
        }
        // A global option; its value may be attached or the next argument.
        let mut inline: Option<std::ffi::OsString> = None;
        let info = if let Some(body) = s.strip_prefix("--") {
            let (name, val) = match body.split_once('=') {
                Some((n, v)) => (n, Some(std::ffi::OsString::from(v))),
                None => (body, None),
            };
            let Ok(full) = match_long(name, root_opts) else {
                break;
            };
            inline = val;
            root_opts.iter().find(|o| o.long.as_deref() == Some(&full))
        } else {
            let chars: Vec<char> = s[1..].chars().collect();
            let c = chars.first().copied().unwrap_or(' ');
            let rest: String = chars[1..].iter().collect();
            if !rest.is_empty() {
                inline = Some(std::ffi::OsString::from(rest));
            }
            root_opts.iter().find(|o| o.short == Some(c))
        };
        let Some(info) = info else { break };
        let mut value = inline;
        if value.is_none() && info.takes_value && i + 1 < raw.len() {
            value = Some(raw[i + 1].clone());
            i += 1;
        }
        if let Some(v) = value {
            match info.long.as_deref() {
                Some("config") => overlays.push(PathBuf::from(v)),
                Some("plugins") => plugins = Some(v),
                Some("disable_plugins") => disabled = Some(v),
                _ => {}
            }
        }
        i += 1;
    }
    let Ok(mut cfg) = crate::config::Config::load(&overlays) else {
        return vec![];
    };
    let seq = |s: std::ffi::OsString| {
        serde_yaml::Value::Sequence(
            crate::comma_list(&s.to_string_lossy())
                .into_iter()
                .map(serde_yaml::Value::String)
                .collect(),
        )
    };
    if let Some(p) = plugins {
        cfg.set("plugins", seq(p));
    }
    if let Some(d) = disabled {
        cfg.set("disabled_plugins", seq(d));
    }
    crate::plugins::names(&cfg)
}

/// beets' optparse checks, before clap parses: unknown (and abbreviated
/// long) options, with optparse's messages and exit code 2; `--` ends
/// option parsing (and may precede the command). Returns the possibly
/// rewritten arguments.
fn precheck(raw: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
    // The full command line, including the registry's plugin commands
    // (which clap only learns at run time).
    let root = cli();
    let root_opts = options_of(&root);
    let enabled = enabled_plugins(raw, &root_opts);
    let mut out: Vec<std::ffi::OsString> = vec![raw[0].clone()];
    let mut command: Option<String> = None;
    let mut sub_opts: Vec<OptInfo> = vec![];
    let mut i = 1;
    while i < raw.len() {
        let s = raw[i].to_string_lossy().into_owned();
        match &command {
            None => {
                if s == "--" {
                    i += 1;
                    if i < raw.len() {
                        let name = raw[i].to_string_lossy().into_owned();
                        match resolve_command(&root, &name) {
                            Some((canonical, sub)) => {
                                out.push(std::ffi::OsString::from(&canonical));
                                i += 1;
                                if plugin_enabled(&canonical, &enabled) {
                                    sub_opts = options_of(sub);
                                    command = Some(canonical);
                                } else {
                                    out.extend(raw[i..].iter().cloned());
                                    break;
                                }
                            }
                            None => {
                                eprintln!("error: unknown command '{name}'");
                                std::process::exit(1);
                            }
                        }
                    }
                    continue;
                }
                if s == "-" || !s.starts_with('-') {
                    match resolve_command(&root, &s) {
                        Some((canonical, sub)) => {
                            out.push(std::ffi::OsString::from(&canonical));
                            i += 1;
                            if plugin_enabled(&canonical, &enabled) {
                                sub_opts = options_of(sub);
                                command = Some(canonical);
                            } else {
                                // A disabled plugin's command: clap still
                                // knows it and the command reports it.
                                out.extend(raw[i..].iter().cloned());
                                break;
                            }
                        }
                        None => {
                            eprintln!("error: unknown command '{s}'");
                            std::process::exit(1);
                        }
                    }
                    continue;
                }
                process_option(raw, &mut i, &root_opts, &mut out, None);
            }
            Some(name) => {
                if s == "--" {
                    out.extend(raw[i..].iter().cloned());
                    break;
                }
                if s == "-" || !s.starts_with('-') {
                    out.push(raw[i].clone());
                    i += 1;
                    continue;
                }
                process_option(raw, &mut i, &sub_opts, &mut out, Some(name));
            }
        }
    }
    out
}

/// Whether a resolved command is a core command or an enabled plugin's.
fn plugin_enabled(canonical: &str, enabled: &[String]) -> bool {
    match crate::plugin_registry::command(canonical) {
        Some(cmd) => enabled.iter().any(|p| p == cmd.plugin),
        None => true,
    }
}

/// The canonical name and clap command for a command name or alias.
fn resolve_command<'a>(root: &'a clap::Command, name: &str) -> Option<(String, &'a clap::Command)> {
    let sub = root.get_subcommands().find(|c| {
        c.get_name() == name
            || c.get_visible_aliases().any(|a| a == name)
            || c.get_all_aliases().any(|a| a == name)
    })?;
    Some((sub.get_name().to_string(), sub))
}

/// Parse the command line; unknown commands get beets' error message.
/// The parsed command line: a core command (beets' own), or one of the
/// registry's plugin commands, which clap only learns at run time.
pub enum Parsed {
    Cli(Cli),
    Plugin {
        command: &'static plugin_registry::PluginCommand,
        globals: commands::Globals,
        matches: clap::ArgMatches,
    },
}

/// `beet`'s clap command: the core subcommands from `Cli`, the plugins'
/// from the registry (between the core ones and `version`/`help`/`config`,
/// as the old central enum ordered them), each built at run time.
pub fn cli() -> clap::Command {
    use clap::CommandFactory;
    let base = Cli::command();
    let mut cmd = clap::Command::new("beet")
        .disable_version_flag(true)
        .disable_help_subcommand(true);
    if let Some(about) = base.get_about() {
        cmd = cmd.about(about.clone());
    }
    if let Some(about) = base.get_long_about() {
        cmd = cmd.long_about(about.clone());
    }
    for arg in base.get_arguments() {
        cmd = cmd.arg(arg.clone());
    }
    // clap orders `--help`'s command list by each subcommand's
    // `display_order`; the derived ones carry theirs, so renumber everything
    // in insertion order.
    let mut tail = vec![];
    let mut order = 0;
    for sub in base.get_subcommands() {
        if matches!(sub.get_name(), "version" | "help" | "config") {
            tail.push(sub.clone());
        } else {
            cmd = cmd.subcommand(sub.clone().display_order(order));
            order += 1;
        }
    }
    for command in plugin_registry::commands() {
        cmd = cmd.subcommand((command.build)().display_order(order));
        order += 1;
    }
    for sub in tail {
        cmd = cmd.subcommand(sub.display_order(order));
        order += 1;
    }
    cmd
}

pub fn parse_args<I, T>(args: I) -> Result<Parsed, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = cli().try_get_matches_from(args)?;
    if let Some((name, sub)) = matches.subcommand()
        && let Some(command) = plugin_registry::command(name)
    {
        return Ok(Parsed::Plugin {
            command,
            globals: commands::Globals::from_matches(&matches),
            matches: sub.clone(),
        });
    }
    Ok(Parsed::Cli(Cli::from_arg_matches(&matches)?))
}

/// The command name in a clap "unrecognized subcommand" error.
pub fn unknown_command(e: &clap::Error) -> Option<String> {
    if e.kind() != ErrorKind::InvalidSubcommand {
        return None;
    }
    match e.get(ContextKind::InvalidSubcommand) {
        Some(ContextValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn main() {
    // A closed pipe (`beet ls | head`) ends the program quietly, as beets
    // does on EPIPE, instead of panicking in `println!`.
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of a signal before any other
    // threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let parsed = match parse_args(precheck(&raw)) {
        Ok(parsed) => parsed,
        Err(e) => match unknown_command(&e) {
            Some(name) => {
                eprintln!("error: unknown command '{name}'");
                std::process::exit(1);
            }
            None => e.exit(),
        },
    };
    let result = match parsed {
        Parsed::Cli(cli) => commands::run(cli),
        Parsed::Plugin {
            command,
            globals,
            matches,
        } => commands::run_plugin(command, globals, &matches),
    };
    if let Err(e) = result {
        // As beets: `invalid query: 'QUERY': EXPLANATION`.
        if let Some(q) = e.downcast_ref::<query::InvalidQueryError>() {
            eprintln!("invalid query: {q}");
            std::process::exit(1);
        }
        if let Some(c) = e.downcast_ref::<config::ConfigError>() {
            eprintln!("configuration error: {c}");
            std::process::exit(1);
        }
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `f` with a deeper stack: clap's derive builds the whole command
    /// tree on the stack, more than a test thread's 2 MiB in debug builds.
    fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let args: Vec<String> = std::iter::once("beet")
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        on_big_stack(move || {
            parse_args(args).map(|p| match p {
                Parsed::Cli(cli) => cli,
                Parsed::Plugin { .. } => panic!("expected a core command"),
            })
        })
    }

    #[test]
    fn cli_definition_is_valid() {
        on_big_stack(|| cli().debug_assert());
    }

    #[test]
    fn global_options_go_before_the_command() {
        let cli = parse(&[
            "-vv",
            "-l",
            "lib.db",
            "-d",
            "/music",
            "-c",
            "c.yaml",
            "-p",
            "fetchart,,lyrics",
            "-P",
            "lyrics",
            "ls",
            "-a",
        ])
        .unwrap();
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.library, Some(PathBuf::from("lib.db")));
        assert_eq!(cli.directory, Some(PathBuf::from("/music")));
        assert_eq!(cli.config, [PathBuf::from("c.yaml")]);
        assert_eq!(
            comma_list(cli.plugins.as_deref().unwrap()),
            ["fetchart", "lyrics"]
        );
        assert_eq!(
            comma_list(cli.disable_plugins.as_deref().unwrap()),
            ["lyrics"]
        );
        assert!(matches!(cli.cmd, Some(Cmd::List { album: true, .. })));
        assert_eq!(parse(&["--verbose", "-v", "stats"]).unwrap().verbose, 2);
        // As in beets, they aren't accepted after the command.
        assert!(parse(&["ls", "-v"]).is_err());
        assert!(parse(&["version", "-P", "lyrics"]).is_err());
        assert!(comma_list("").is_empty());
    }

    #[test]
    fn beets_command_aliases() {
        let cmd = |args: &[&str]| parse(args).unwrap().cmd.unwrap();
        assert!(matches!(cmd(&["ls"]), Cmd::List { .. }));
        assert!(matches!(cmd(&["rm", "x"]), Cmd::Remove { .. }));
        assert!(matches!(cmd(&["mv"]), Cmd::Move { .. }));
        assert!(matches!(cmd(&["upd"]), Cmd::Update { .. }));
        assert!(matches!(cmd(&["up"]), Cmd::Update { .. }));
        assert!(matches!(cmd(&["mod", "x=y"]), Cmd::Modify { .. }));
        assert!(matches!(cmd(&["imp", "dir"]), Cmd::Import { .. }));
        assert!(matches!(cmd(&["im", "dir"]), Cmd::Import { .. }));
        assert!(matches!(cmd(&["?", "ls"]), Cmd::Help { command: Some(c) } if c == "ls"));
        assert!(matches!(cmd(&["help"]), Cmd::Help { command: None }));
        assert!(matches!(cmd(&["version"]), Cmd::Version));
        // Commands beets gives no alias don't get one.
        for name in ["stat", "wr", "conf", "fetch", "upgrade"] {
            assert!(parse(&[name]).is_err(), "{name}");
        }
    }

    #[test]
    fn modify_flags_last_one_wins() {
        let flags = |args: &[&str]| match parse(args).unwrap().cmd.unwrap() {
            Cmd::Modify {
                mv,
                nomove,
                write,
                nowrite,
                noinherit,
                limit,
                format,
                ..
            } => (mv, nomove, write, nowrite, noinherit, limit, format),
            _ => unreachable!(),
        };
        assert_eq!(
            flags(&["mod", "-m", "-M", "-W", "-w", "x=y"]),
            (false, true, true, false, false, None, None)
        );
        assert_eq!(
            flags(&["mod", "-I", "-l", "3", "-f", "$title", "x=y"]),
            (
                false,
                false,
                false,
                false,
                true,
                Some(3),
                Some("$title".into())
            )
        );
        // No arguments at all reach modify, which reports beets' error.
        assert!(parse(&["modify"]).is_ok());
    }

    #[test]
    fn no_command_version_flag_and_unknown_commands() {
        let cli = parse(&[]).unwrap();
        assert!(cli.cmd.is_none());
        assert!(parse(&["--version"]).unwrap().version);
        let e = parse(&["frob", "x"]).unwrap_err();
        assert_eq!(unknown_command(&e).as_deref(), Some("frob"));
        // Other errors keep clap's message.
        assert_eq!(
            unknown_command(&parse(&["ls", "--bogus"]).unwrap_err()),
            None
        );
        // `beet help CMD` is `beet CMD --help`.
        let e = parse(&["mv", "--help"]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::DisplayHelp);
        assert!(e.to_string().contains("Usage: beet move"));
    }
}
