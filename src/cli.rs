//! The beets-compatible command line: the clap definition, the optparse
//! help formatter, the option precheck and plugin-command registration.

use crate::commands;
use crate::plugin_registry;
use crate::subcommand_help;
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
    /// Format for printing items
    #[arg(long = "format-item", value_name = "FORMAT_ITEM")]
    pub format_item: Option<String>,
    /// Format for printing albums
    #[arg(long = "format-album", value_name = "FORMAT_ALBUM")]
    pub format_album: Option<String>,
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
#[must_use]
pub fn comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// import new music
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
    /// query the library
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
    /// change metadata fields
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
    /// remove matching items from the library
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
    /// move or copy items
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
    /// update the library
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
    /// write tag information to files
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
    /// show statistics about the library or a query
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
    /// show fields available for queries and format strings
    Fields,
    /// copy a beets library database (opened read-only) into this library
    #[command(name = "migrate-beets", hide = true)]
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
    /// output version information
    Version,
    /// give detailed help on a specific sub-command
    #[command(visible_alias = "?")]
    Help {
        /// The command to explain
        command: Option<String>,
    },
    /// show or edit the user configuration
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

/// How optparse checks an option's value (`type="int"`, `type="choice"`, ...).
#[derive(Clone, Debug, PartialEq)]
enum ValueKind {
    Any,
    Int,
    Float,
    Choice(Vec<String>),
}

/// One option a subcommand knows (as optparse would see it).
struct OptInfo {
    short: Option<char>,
    long: Option<String>,
    takes_value: bool,
    kind: ValueKind,
}

/// The value checks the option's clap parser implies, as optparse would
/// make them: enumeration choices and integer/float conversions.
#[must_use]
fn value_kind(arg: &clap::Arg) -> ValueKind {
    let choices: Vec<String> = arg
        .get_possible_values()
        .into_iter()
        .map(|v| v.get_name().to_string())
        .collect();
    if !choices.is_empty() {
        return ValueKind::Choice(choices);
    }
    let id = arg.get_value_parser().type_id();
    let is = |t: std::any::TypeId| id == t;
    if [
        std::any::TypeId::of::<i8>(),
        std::any::TypeId::of::<i16>(),
        std::any::TypeId::of::<i32>(),
        std::any::TypeId::of::<i64>(),
        std::any::TypeId::of::<i128>(),
        std::any::TypeId::of::<isize>(),
        std::any::TypeId::of::<u8>(),
        std::any::TypeId::of::<u16>(),
        std::any::TypeId::of::<u32>(),
        std::any::TypeId::of::<u64>(),
        std::any::TypeId::of::<u128>(),
        std::any::TypeId::of::<usize>(),
    ]
    .iter()
    .any(|t| is(*t))
    {
        ValueKind::Int
    } else if is(std::any::TypeId::of::<f32>()) || is(std::any::TypeId::of::<f64>()) {
        ValueKind::Float
    } else {
        ValueKind::Any
    }
}

#[must_use]
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
                kind: if takes_value {
                    value_kind(a)
                } else {
                    ValueKind::Any
                },
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
            kind: ValueKind::Any,
        });
    }
    out
}

enum PreError {
    Unknown(String),
    Ambiguous(String, Vec<String>),
    /// An option that needs a value at the end of the command line.
    Missing(String),
    /// A value optparse's type check rejects (`option -l: invalid ...`).
    ValueError(String),
    /// A command-level check of a parsed value (beets raises `UserError`,
    /// which prints without a usage block and exits 1).
    User(String),
}

/// optparse's value checks (`type="int"`, `type="float"`, `type="choice"`),
/// with its wording, plus the non-negative check of beets' `add_limit_option`
/// callback (a `UserError`, not an optparse error).
fn check_value(info: &OptInfo, label: &str, value: &str) -> Result<(), PreError> {
    match &info.kind {
        ValueKind::Any => Ok(()),
        ValueKind::Choice(choices) => {
            if choices.iter().any(|c| c == value) {
                return Ok(());
            }
            let quoted: Vec<String> = choices.iter().map(|c| format!("'{c}'")).collect();
            Err(PreError::ValueError(format!(
                "option {label}: invalid choice: '{value}' (choose from {})",
                quoted.join(", ")
            )))
        }
        ValueKind::Int => {
            // `int()`: surrounding whitespace and a leading sign are fine.
            let trimmed = value.trim();
            let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err(PreError::ValueError(format!(
                    "option {label}: invalid integer value: '{value}'"
                )));
            }
            if info.long.as_deref() == Some("limit") && trimmed.starts_with('-') {
                // beets' `_set_limit` (ui/__init__.py): negative limits are a
                // UserError, printed without the usage block, exit 1.
                return Err(PreError::User(format!(
                    "{label} argument must be a non-negative integer"
                )));
            }
            if digits != "0" && digits.starts_with('0') {
                // Python's int() accepts these; clap's integers do too.
            }
            Ok(())
        }
        ValueKind::Float => match value.trim().parse::<f64>() {
            Ok(_) => Ok(()),
            Err(_) => Err(PreError::ValueError(format!(
                "option {label}: invalid floating-point value: '{value}'"
            ))),
        },
    }
}

/// The value as optparse's `int`/`float` conversion sees it (it strips
/// surrounding whitespace).
fn normalize_value(info: &OptInfo, value: &std::ffi::OsStr) -> std::ffi::OsString {
    match info.kind {
        ValueKind::Int | ValueKind::Float => {
            std::ffi::OsString::from(value.to_string_lossy().trim())
        }
        _ => value.to_os_string(),
    }
}

impl PreError {
    #[cfg(test)]
    fn message(&self) -> String {
        option_error(self)
    }
}

/// beets' `OptionParser.error` text for an option error.
fn option_error(err: &PreError) -> String {
    match err {
        PreError::Unknown(opt) => format!("no such option: {opt}"),
        PreError::Ambiguous(opt, possibilities) => {
            format!("ambiguous option: {opt} ({}?)", possibilities.join(", "))
        }
        PreError::Missing(opt) => format!("{opt} option requires 1 argument"),
        PreError::ValueError(message) | PreError::User(message) => message.clone(),
    }
}

fn root_usage() -> String {
    "Usage: \n  beet COMMAND [ARGS...]\n  beet help COMMAND".to_string()
}

/// One row of `beet --help`'s command list.
pub struct HelpCommand {
    name: String,
    aliases: Vec<String>,
    help: String,
}

/// The command list beets prints: the core commands and the enabled
/// plugins', hidden ones (and disabled plugin commands) left out.
#[must_use]
pub fn help_commands(enabled_plugins: &[String]) -> Vec<HelpCommand> {
    use clap::CommandFactory;
    let row = |cmd: &clap::Command| {
        if cmd.is_hide_set() {
            return None;
        }
        Some(HelpCommand {
            name: cmd.get_name().to_string(),
            aliases: cmd.get_visible_aliases().map(str::to_string).collect(),
            help: cmd.get_about().map(ToString::to_string).unwrap_or_default(),
        })
    };
    let mut out: Vec<HelpCommand> = Cli::command().get_subcommands().filter_map(row).collect();
    for command in plugin_registry::commands() {
        if !enabled_plugins.iter().any(|p| p == command.plugin) {
            continue;
        }
        out.extend(row(&(command.build)()));
    }
    // beets sorts the list by name (`ui/__init__.py`'s format_help).
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// optparse's output width: `$COLUMNS`, or 80, less two columns.
#[must_use]
fn optparse_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|w| w.trim().parse::<usize>().ok())
        .map_or(80 - 2, |w| w.saturating_sub(2))
}

/// optparse's `max_help_position`: the help starts at column 24 at most.
#[must_use]
fn max_help_position(width: usize) -> usize {
    (width.saturating_sub(20)).clamp(4, 24)
}

/// textwrap.wrap's hyphen splitting: a word breaks after a hyphen between
/// word characters ("already-" + "imported"), as optparse's help wraps.
#[must_use]
fn split_hyphens(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut parts = vec![];
    let mut part = String::new();
    for (i, c) in chars.iter().enumerate() {
        part.push(*c);
        let breaks = *c == '-'
            && i > 0
            && i + 1 < chars.len()
            && word_char(chars[i - 1])
            && word_char(chars[i + 1]);
        if breaks {
            parts.push(std::mem::take(&mut part));
        }
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// `textwrap.wrap(text, width)` for optparse's help, whose arguments collapse
/// whitespace; long words and hyphenated words are broken as textwrap does by
/// default.
#[must_use]
fn wrap_help(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = vec![];
    let mut line = String::new();
    let push = |frag: &str, space: bool, line: &mut String, lines: &mut Vec<String>| {
        let sep = usize::from(space && !line.is_empty());
        if line.chars().count() + sep + frag.chars().count() <= width {
            if sep == 1 {
                line.push(' ');
            }
            line.push_str(frag);
            return;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(line));
        }
        let mut frag = frag;
        while frag.chars().count() > width {
            let split = frag
                .char_indices()
                .nth(width)
                .map_or(frag.len(), |(i, _)| i);
            lines.push(frag[..split].to_string());
            frag = &frag[split..];
        }
        line.push_str(frag);
    };
    for word in text.split_whitespace() {
        for (i, frag) in split_hyphens(word).iter().enumerate() {
            push(frag, i == 0, &mut line, &mut lines);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// One optparse help table (`Commands:`/`Options:`): two-space indent, names
/// padded to `help_position`, the help wrapped in the rest of the width.
#[must_use]
fn render_table(rows: &[(String, String)], width: usize, help_position: usize) -> String {
    let indent = 2;
    let name_width = help_position.saturating_sub(indent + 2);
    let help_width = width.saturating_sub(help_position);
    let mut out = String::new();
    for (name, help) in rows {
        let indent_first = if name.chars().count() > name_width {
            out.push_str(&format!("{}{}\n", " ".repeat(indent), name));
            help_position
        } else {
            out.push_str(&format!(
                "{}{:<name_width$}  ",
                " ".repeat(indent),
                name,
                name_width = name_width
            ));
            0
        };
        let lines = wrap_help(help, help_width);
        out.push_str(&" ".repeat(indent_first));
        out.push_str(lines.first().map_or("", String::as_str));
        out.push('\n');
        for line in lines.iter().skip(1) {
            out.push_str(&" ".repeat(help_position));
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The root options as optparse shows them (`-h`, then the metavar-bearing
/// long form), in beets' order.
const ROOT_OPTIONS: &[(&str, &str)] = &[
    ("--format-item=FORMAT_ITEM", "print with custom format"),
    ("--format-album=FORMAT_ALBUM", "print with custom format"),
    (
        "-l LIBRARY, --library=LIBRARY",
        "library database file to use",
    ),
    (
        "-d DIRECTORY, --directory=DIRECTORY",
        "destination music directory",
    ),
    (
        "-v, --verbose",
        "log more details (use twice for even more)",
    ),
    ("-c CONFIGS, --config=CONFIGS", "path to configuration file"),
    (
        "-p PLUGINS, --plugins=PLUGINS",
        "a comma-separated list of plugins to load",
    ),
    (
        "-P DISABLED_PLUGINS, --disable-plugins=DISABLED_PLUGINS",
        "a comma-separated list of plugins to disable",
    ),
    ("-h, --help", "show this help message and exit"),
];

/// `beet help CMD`/`beet CMD -h`: beets' optparse help for a command (usage,
/// blank line, `Options:` and the option table), as the reference's parser
/// shows it. None for commands beets doesn't have (ours: `migrate-beets`).
#[must_use]
pub fn subcommand_help_text(name: &str) -> Option<String> {
    subcommand_help_text_at(name, optparse_width())
}

#[must_use]
fn subcommand_help_text_at(name: &str, width: usize) -> Option<String> {
    let help = subcommand_help::find(name)?;
    let rows: Vec<(String, String)> = help
        .options
        .iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    // optparse fits the help column to its longest option (plus the
    // two-space indent and two spaces), up to `max_help_position`.
    let max_pos = max_help_position(width);
    let help_position = rows
        .iter()
        .map(|(name, _)| name.chars().count() + 4)
        .max()
        .unwrap_or(0)
        .min(max_pos);
    Some(format!(
        "{}\n\nOptions:\n{}",
        help.usage,
        render_table(&rows, width, help_position)
    ))
}

/// The canonical name of a command (by name or alias) among the enabled
/// ones, as beets' `_subcommand_for_name` resolves it.
#[must_use]
pub fn canonical_command(name: &str, enabled_plugins: &[String]) -> Option<String> {
    resolve_command(&cli(enabled_plugins), name).map(|(canonical, _)| canonical)
}

/// `beet --help`/`beet help`, byte for byte as beets' optparse prints it.
#[must_use]
pub fn root_help_text(enabled_plugins: &[String]) -> String {
    root_help_text_at(enabled_plugins, optparse_width())
}

/// The same at a given optparse width (for tests).
#[must_use]
fn root_help_text_at(enabled_plugins: &[String], width: usize) -> String {
    let rows: Vec<(String, String)> = ROOT_OPTIONS
        .iter()
        .map(|(name, help)| (name.to_string(), help.to_string()))
        .collect();
    let mut out = format!(
        "{}\n\nOptions:\n{}",
        root_usage(),
        render_table(&rows, width, max_help_position(width))
    );
    let commands: Vec<(String, String)> = help_commands(enabled_plugins)
        .into_iter()
        .map(|c| {
            let name = if c.aliases.is_empty() {
                c.name
            } else {
                format!("{} ({})", c.name, c.aliases.join(", "))
            };
            (name, c.help)
        })
        .collect();
    // The commands' help column is fitted to the longest command name (up
    // to optparse's maximum), unlike the options' fixed column.
    let max_pos = max_help_position(width);
    let help_position = commands
        .iter()
        .map(|(name, _)| name.chars().count() + 4)
        .filter(|p| *p <= max_pos)
        .max()
        .unwrap_or(0);
    out.push_str(&format!(
        "\nCommands:\n{}",
        render_table(&commands, width, help_position)
    ));
    out
}

#[must_use]
fn command_usage(name: &str) -> String {
    if name == "list" {
        "Usage: beet [options]\nExample: beet list -f '$album: $title' artist:beatles".to_string()
    } else {
        format!("Usage: beet {name} [options]")
    }
}

fn exit_option_error(command: Option<&str>, err: PreError) -> ! {
    if let PreError::User(message) = err {
        // beets logs the UserError as `error: ...` and exits 1.
        eprintln!("error: {message}");
        std::process::exit(1);
    }
    let (usage, prog) = match command {
        Some(name) => (command_usage(name), format!("beet {name}")),
        None => (root_usage(), "beet".to_string()),
    };
    eprintln!("{usage}\n\n{prog}: error: {}", option_error(&err));
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
/// starting with `-`), and long options may be abbreviated. Returns whether
/// this was the help flag (`-h`/`--help`), which optparse acts on at once.
fn process_option(
    args: &[std::ffi::OsString],
    i: &mut usize,
    opts: &[OptInfo],
    out: &mut Vec<std::ffi::OsString>,
    command: Option<&str>,
) -> bool {
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
            .expect("BUG: match_long resolved the name of one of opts' long options");
        let mut new = std::ffi::OsString::from(format!("--{full}"));
        if info.takes_value {
            let label = format!("--{full}");
            match inline {
                Some(v) => {
                    check_value(info, &label, &v).unwrap_or_else(|e| fail(e));
                    new.push("=");
                    new.push(normalize_value(info, v.as_ref()));
                    *i += 1;
                }
                None if *i + 1 < args.len() => {
                    check_value(info, &label, &args[*i + 1].to_string_lossy())
                        .unwrap_or_else(|e| fail(e));
                    new.push("=");
                    new.push(normalize_value(info, &args[*i + 1]));
                    *i += 2;
                }
                None => fail(PreError::Missing(label)),
            }
        } else {
            *i += 1;
        }
        out.push(new);
        return full == "help";
    }
    // Short options (or `-` alone, which is positional).
    let cluster: Vec<char> = token[1..].chars().collect();
    let mut new = std::ffi::OsString::from("-");
    let mut helped = false;
    let mut j = 0;
    while j < cluster.len() {
        let c = cluster[j];
        let Some(info) = opts.iter().find(|o| o.short == Some(c)) else {
            fail(PreError::Unknown(format!("-{c}")));
        };
        if c == 'h' && !info.takes_value {
            helped = true;
        }
        new.push(c.to_string());
        if info.takes_value {
            let label = format!("-{c}");
            let rest: String = cluster[j + 1..].iter().collect();
            if !rest.is_empty() {
                check_value(info, &label, &rest).unwrap_or_else(|e| fail(e));
                new.push(normalize_value(info, rest.as_ref()));
                j = cluster.len();
            } else if *i + 1 < args.len() {
                check_value(info, &label, &args[*i + 1].to_string_lossy())
                    .unwrap_or_else(|e| fail(e));
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
                        tok.push(normalize_value(info, &args[*i + 1]));
                        out.push(tok);
                    }
                    None => {
                        // clap doesn't take attached short values: pass
                        // `-n value` as two arguments.
                        out.push(std::ffi::OsString::from(format!("-{c}")));
                        out.push(normalize_value(info, &args[*i + 1]));
                    }
                }
                *i += 1;
                j = cluster.len();
            } else {
                fail(PreError::Missing(label));
            }
        } else {
            j += 1;
        }
    }
    *i += 1;
    if !new.is_empty() && new != "-" {
        out.push(new);
    }
    helped
}

/// The configuration as the global options leave it, for knowing whether a
/// plugin command is there at all (beets registers plugin commands only for
/// enabled plugins) and for the plugins' load-time warnings before help. A
/// configuration that doesn't load gives the default one.
fn global_config(
    raw: &[std::ffi::OsString],
    root_opts: &[OptInfo],
) -> Option<crate::config::Config> {
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
                Some("disable-plugins") => disabled = Some(v),
                _ => {}
            }
        }
        i += 1;
    }
    let mut cfg = crate::config::Config::load(&overlays).ok()?;
    let seq = |s: std::ffi::OsString| {
        serde_yaml::Value::Sequence(
            comma_list(&s.to_string_lossy())
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
    Some(cfg)
}

/// beets' optparse checks, before clap parses: unknown (and abbreviated
/// long) options, with optparse's messages and exit code 2; `--` ends
/// option parsing (and may precede the command). Returns the possibly
/// rewritten arguments; whether a root-level `-h`/`--help` was given (beets
/// then runs the help command, whatever follows); the plugins enabled; their
/// configuration (for loading them before help); and the command, once seen.
pub(crate) fn precheck(
    raw: &[std::ffi::OsString],
) -> (
    Vec<std::ffi::OsString>,
    bool,
    Vec<String>,
    Option<crate::config::Config>,
    Option<String>,
) {
    // The full command line, including the enabled plugins' commands
    // (which clap only learns at run time).
    let root_opts = options_of(&cli(&[]));
    let global = global_config(raw, &root_opts);
    let enabled = global
        .as_ref()
        .map(crate::plugins::names)
        .unwrap_or_default();
    let root = cli(&enabled);
    let mut out: Vec<std::ffi::OsString> = vec![raw[0].clone()];
    let mut command: Option<String> = None;
    let mut sub_opts: Vec<OptInfo> = vec![];
    let mut root_help = false;
    let mut i = 1;
    while i < raw.len() {
        let s = raw[i].to_string_lossy().into_owned();
        match &command {
            None => {
                if root_help && (s == "--" || s == "-" || !s.starts_with('-')) {
                    // As beets: the global options force the help command, so
                    // nothing after them (not even a command) is parsed.
                    break;
                }
                if s == "--" {
                    i += 1;
                    if i < raw.len() {
                        let name = raw[i].to_string_lossy().into_owned();
                        match resolve_command(&root, &name) {
                            Some((canonical, sub)) => {
                                out.push(std::ffi::OsString::from(&canonical));
                                i += 1;
                                sub_opts = options_of(sub);
                                command = Some(canonical);
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
                            sub_opts = options_of(sub);
                            command = Some(canonical);
                        }
                        None => {
                            eprintln!("error: unknown command '{s}'");
                            std::process::exit(1);
                        }
                    }
                    continue;
                }
                root_help |= process_option(raw, &mut i, &root_opts, &mut out, None);
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
                if process_option(raw, &mut i, &sub_opts, &mut out, Some(name)) {
                    // optparse's help action prints and exits, whatever
                    // follows.
                    break;
                }
            }
        }
    }
    (out, root_help, enabled, global, command)
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

/// `beet`'s clap command: the core subcommands from `Cli` and the enabled
/// plugins' from the registry (between the core ones and
/// `version`/`help`/`config`, as the old central enum ordered them), each
/// built at run time. As in beets, a disabled plugin's commands aren't
/// registered at all (`beet web` is then an unknown command).
#[must_use]
pub fn cli(enabled_plugins: &[String]) -> clap::Command {
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
        if !enabled_plugins.iter().any(|p| p == command.plugin) {
            continue;
        }
        cmd = cmd.subcommand((command.build)().display_order(order));
        order += 1;
    }
    for sub in tail {
        cmd = cmd.subcommand(sub.display_order(order));
        order += 1;
    }
    cmd
}

pub fn parse_args<I, T>(args: I, enabled_plugins: &[String]) -> Result<Parsed, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = cli(enabled_plugins).try_get_matches_from(args)?;
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
#[must_use]
pub fn unknown_command(e: &clap::Error) -> Option<String> {
    if e.kind() != ErrorKind::InvalidSubcommand {
        return None;
    }
    match e.get(ContextKind::InvalidSubcommand) {
        Some(ContextValue::String(s)) => Some(s.clone()),
        _ => None,
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
            parse_args(args, &[]).map(|p| match p {
                Parsed::Cli(cli) => cli,
                Parsed::Plugin { .. } => panic!("expected a core command"),
            })
        })
    }

    #[test]
    fn cli_definition_is_valid() {
        on_big_stack(|| cli(&[]).debug_assert());
    }

    /// As in beets, a plugin's commands are registered only while the plugin
    /// is enabled; otherwise the command is unknown.
    #[test]
    fn plugin_commands_only_for_enabled_plugins() {
        let parse_with = |args: &[&str], enabled: &[&str]| {
            let args: Vec<String> = std::iter::once("beet")
                .chain(args.iter().copied())
                .map(String::from)
                .collect();
            let enabled: Vec<String> = enabled.iter().map(ToString::to_string).collect();
            on_big_stack(move || parse_args(args, &enabled))
        };
        let Err(disabled) = parse_with(&["fetchart"], &[]) else {
            panic!("a disabled plugin's command parsed");
        };
        assert_eq!(unknown_command(&disabled).as_deref(), Some("fetchart"));
        assert!(parse_with(&["fetchart"], &["fetchart"]).is_ok());
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

    /// beets' own `beet --help`, captured from reference beets 2.14.1 (the
    /// `--format-*` options, optparse's layout and the sorted command list;
    /// `migrate-beets` is ours and stays out, like beets' hidden commands).
    const REFERENCE_CORE_HELP: &str = "\
Usage: 
  beet COMMAND [ARGS...]
  beet help COMMAND

Options:
  --format-item=FORMAT_ITEM
                        print with custom format
  --format-album=FORMAT_ALBUM
                        print with custom format
  -l LIBRARY, --library=LIBRARY
                        library database file to use
  -d DIRECTORY, --directory=DIRECTORY
                        destination music directory
  -v, --verbose         log more details (use twice for even more)
  -c CONFIGS, --config=CONFIGS
                        path to configuration file
  -p PLUGINS, --plugins=PLUGINS
                        a comma-separated list of plugins to load
  -P DISABLED_PLUGINS, --disable-plugins=DISABLED_PLUGINS
                        a comma-separated list of plugins to disable
  -h, --help            show this help message and exit

Commands:
  config            show or edit the user configuration
  fields            show fields available for queries and format strings
  help (?)          give detailed help on a specific sub-command
  import (imp, im)  import new music
  list (ls)         query the library
  modify (mod)      change metadata fields
  move (mv)         move or copy items
  remove (rm)       remove matching items from the library
  stats             show statistics about the library or a query
  update (upd, up)  update the library
  version           output version information
  write             write tag information to files
";

    #[test]
    fn root_help_matches_beets() {
        assert_eq!(root_help_text_at(&[], 78), REFERENCE_CORE_HELP);
    }

    /// beets' `beet help list` at the default width (optparse's layout and
    /// the command's own example usage line).
    #[test]
    fn subcommand_help_matches_beets() {
        assert_eq!(
            subcommand_help_text_at("list", 78).unwrap(),
            "\
Usage: beet [options]
Example: beet list -f '$album: $title' artist:beatles

Options:
  -h, --help            show this help message and exit
  -a, --album           match albums instead of tracks
  -p PATH, --path=PATH  print paths for matched items or albums
  -f FORMAT, --format=FORMAT
                        print with custom format
  -l LIMIT, --limit=LIMIT
                        limit query results
"
        );
        // With one option the help column shrinks to it, as optparse fits it.
        assert_eq!(
            subcommand_help_text_at("fields", 78).unwrap(),
            "\
Usage: beet fields [options]

Options:
  -h, --help  show this help message and exit
"
        );
        // A long option goes on its own line, its help at the max column.
        assert_eq!(
            subcommand_help_text_at("move", 78).unwrap(),
            "\
Usage: beet move [options]

Options:
  -h, --help            show this help message and exit
  -d DIR, --dest=DIR    destination directory
  -c, --copy            copy instead of moving
  -p, --pretend         show how files would be moved, but don't touch
                        anything
  -t, --timid           always confirm all actions
  -e, --export          copy without changing the database path
  -a, --album           match albums instead of tracks
  -l LIMIT, --limit=LIMIT
                        limit query results
"
        );
        assert!(subcommand_help_text_at("migrate-beets", 78).is_none());
    }

    /// textwrap's hyphen breaking in optparse's help (a narrow terminal).
    #[test]
    fn subcommand_help_wraps_at_hyphens() {
        let help = subcommand_help_text_at("import", 38).unwrap();
        assert!(help.contains("  -i, --incremental\n                  skip already-\n                  imported directories\n"));
        assert!(help.contains("  -I, --noincremental\n                  do not skip already-\n                  imported directories\n"));
        let help = subcommand_help_text_at("import", 78).unwrap();
        assert!(help.contains("  -i, --incremental     skip already-imported directories\n"));
    }

    /// Every command beets has help for: the ones beets-rs implements too
    /// (the plugin registry's; `migrate-beets` is ours alone).
    #[test]
    fn every_command_has_beets_help() {
        use clap::CommandFactory;
        for sub in Cli::command().get_subcommands() {
            let name = sub.get_name();
            if name == "migrate-beets" {
                continue;
            }
            assert!(
                subcommand_help::find(name).is_some(),
                "no beets help text for {name}"
            );
        }
        for command in plugin_registry::commands() {
            let name = (command.build)().get_name().to_string();
            assert!(
                subcommand_help::find(&name).is_some(),
                "no beets help text for the {name} plugin command"
            );
        }
    }

    #[test]
    fn root_help_lists_enabled_plugin_commands_sorted() {
        let help = root_help_text_at(&["fetchart".into(), "embedart".into()], 78);
        let commands: Vec<&str> = help
            .lines()
            .skip_while(|l| *l != "Commands:")
            .skip(1)
            .filter(|l| l.starts_with("  ") && !l.starts_with("    "))
            .map(|l| l.trim_start().split("  ").next().unwrap())
            .collect();
        let names: Vec<&str> = commands
            .iter()
            .map(|name| name.split(' ').next().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(names.contains(&"clearart"));
        assert!(names.contains(&"extractart"));
        assert!(names.contains(&"fetchart"));
        // A plugin that isn't enabled has no command, as in beets.
        assert!(!help.contains("lyrics"));
        assert!(!help.contains("migrate-beets"));
    }

    #[test]
    fn command_aliases_as_beets_shows_them() {
        assert_eq!(
            help_commands(&[])
                .iter()
                .find(|c| c.name == "import")
                .unwrap()
                .aliases,
            ["imp", "im"]
        );
        assert_eq!(
            help_commands(&[])
                .iter()
                .find(|c| c.name == "help")
                .unwrap()
                .aliases,
            ["?"]
        );
    }

    fn opt(long: &str, kind: ValueKind) -> OptInfo {
        OptInfo {
            short: None,
            long: Some(long.into()),
            takes_value: true,
            kind,
        }
    }

    #[test]
    fn optparse_value_errors() {
        let int = opt("limit", ValueKind::Int);
        assert_eq!(
            check_value(&int, "-l", "a").unwrap_err().message(),
            "option -l: invalid integer value: 'a'"
        );
        assert_eq!(
            check_value(&int, "--limit", "").unwrap_err().message(),
            "option --limit: invalid integer value: ''"
        );
        // Python's int(): a sign is fine, so -1 reaches beets' limit check.
        assert!(check_value(&int, "-l", "+2").is_ok());
        assert!(check_value(&int, "-l", " 3 ").is_ok());
        assert_eq!(
            check_value(&int, "-l", "-1").unwrap_err().message(),
            "-l argument must be a non-negative integer"
        );
        assert!(matches!(
            check_value(&int, "--limit=-1", "-1"),
            Err(PreError::User(_))
        ));
        // Other integer options take negatives.
        let threads = opt("threads", ValueKind::Int);
        assert!(check_value(&threads, "-t", "-1").is_ok());
        let choice = opt(
            "format",
            ValueKind::Choice(vec!["json".into(), "csv".into()]),
        );
        assert!(check_value(&choice, "-f", "json").is_ok());
        assert_eq!(
            check_value(&choice, "-f", "yaml").unwrap_err().message(),
            "option -f: invalid choice: 'yaml' (choose from 'json', 'csv')"
        );
        let float = opt("threshold", ValueKind::Float);
        assert!(check_value(&float, "--threshold", "0.5").is_ok());
        assert_eq!(
            check_value(&float, "--threshold", "x")
                .unwrap_err()
                .message(),
            "option --threshold: invalid floating-point value: 'x'"
        );
    }

    #[test]
    fn root_help_columns_follow_optparse() {
        // At a narrow width optparse puts long option strings on their own
        // line, with the help indented to `max_help_position`.
        let help = root_help_text_at(&[], 50);
        assert!(help.contains("  --format-item=FORMAT_ITEM\n"));
        assert!(help.contains("                    print with custom format\n"));
        assert!(help.contains("  -v, --verbose         log more details"));
    }
}
