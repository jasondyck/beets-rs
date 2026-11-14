//! A port of beets 2.14's `replace` plugin (`beetsplug/replace.py`):
//! `beet replace [-l N] QUERY NEW_FILE` puts another audio file in place of
//! a library item's file, keeping the item's tags in the database (the new
//! file keeps its own tags; beets rewrites nothing).
//!
//! The item is chosen from a numbered menu when the query matches several,
//! and the replacement needs a confirmation. The new file takes the old
//! file's path (with the new file's extension), and the item's stored path
//! follows it.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::query::Query;
use anyhow::{Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The plugin: a command only.
pub struct ReplacePlugin;

impl crate::plugins::Plugin for ReplacePlugin {
    fn name(&self) -> &'static str {
        "replace"
    }
}

/// `beet replace`'s arguments: `args[-1]` is the new file, the rest is the
/// query (beets parses them from `args`, so a query term can look like an
/// option).
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub limit: Option<usize>,
    pub args: Vec<String>,
}

/// `beet replace`, printing to stdout.
pub fn command(lib: &Library, opts: &CmdOpts) -> Result<()> {
    run(lib, opts, &mut std::io::stdout().lock())
}

/// `beet replace`, writing to `out`.
pub fn run(lib: &Library, opts: &CmdOpts, out: &mut impl Write) -> Result<()> {
    let Some((new_file, query)) = opts.args.split_last() else {
        bail!("Usage: beet replace <query> <new_file_path>");
    };
    if query.is_empty() {
        bail!("Usage: beet replace <query> <new_file_path>");
    }
    let new_file = Path::new(new_file);

    file_check(new_file)?;

    let mut item_list = lib.items(&Query::parse(Kind::Item, query)?)?;
    item_list.truncate(opts.limit.unwrap_or(usize::MAX));

    if item_list.is_empty() {
        bail!("No matching songs found.");
    }

    let Some(song) = select_song(&item_list, lib, out)? else {
        writeln!(out, "Operation cancelled.")?;
        return Ok(());
    };

    if !confirm_replacement(new_file, &song, out)? {
        writeln!(out, "Aborting replacement.")?;
        return Ok(());
    }

    replace_file(new_file, &song, lib, out)
}

/// `util.displayable_path`: a path as the user sees it.
fn displayable(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `displayable_path(item)`: beets passes the item where a path is
/// expected, and a model is iterable (its field names), so the menu shows
/// the item's keys joined by "; ". `Item.keys()` includes the album's
/// fields (beets' `with_album=True`): its `artpath` and any album flexible
/// attributes.
fn item_display(item: &Item, lib: &Library) -> String {
    let mut keys = item.keys();
    if item.int("album_id") != 0
        && let Ok(Some(album)) = lib.get_album(item.int("album_id"))
    {
        for k in album.keys() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys.join("; ")
}

/// `file_check`: the new file exists and is audio mediafile can read.
pub fn file_check(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("'{}' is not a valid file.", displayable(path));
    }
    // `mediafile.MediaFile(path)`; its `FileTypeError` when the format isn't
    // recognized (beets lets an `UnreadableFileError` crash the command).
    if crate::mediafile::MediaFile::open(path).is_err() {
        bail!("{}: File is not in a recognized format", displayable(path));
    }
    Ok(())
}

/// Python's `int()` for the menu: surrounding whitespace, a sign and
/// underscores between digits.
fn py_int(s: &str) -> Option<i128> {
    let t = s.trim();
    let (sign, digits) = match t.strip_prefix('-') {
        Some(d) => (-1i128, d),
        None => match t.strip_prefix('+') {
            Some(d) => (1, d),
            None => (1, t),
        },
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let mut n: i128 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)?;
        n = n.checked_mul(10)?.checked_add(i128::from(d))?;
    }
    Some(sign * n)
}

/// `select_song`: a numbered menu and a choice (None cancels).
pub fn select_song(items: &[Item], lib: &Library, out: &mut impl Write) -> Result<Option<Item>> {
    writeln!(out, "\nMatching songs:")?;
    for (i, item) in items.iter().enumerate() {
        writeln!(out, "{}. {}", i + 1, item_display(item, lib))?;
    }

    loop {
        let prompt = format!(
            "Which song would you like to replace? [1-{}] (0 to cancel): ",
            items.len()
        );
        let Some(line) = crate::ui::read_line(&prompt) else {
            return Ok(None);
        };
        match py_int(&line) {
            None => writeln!(out, "Invalid input. Please type in a number.")?,
            Some(0) => return Ok(None),
            Some(i) if i >= 1 && i <= items.len() as i128 => {
                return Ok(Some(items[i as usize - 1].clone()));
            }
            Some(_) => writeln!(
                out,
                "Invalid choice. Please enter a number between 1 and {}.",
                items.len()
            )?,
        }
    }
}

/// `confirm_replacement`: "y"/"yes" (any case) replaces; anything else not.
pub fn confirm_replacement(new_file: &Path, song: &Item, out: &mut impl Write) -> Result<bool> {
    let original = song.path().to_path_buf();
    if !original.exists() {
        bail!("The original song file was not found.");
    }
    writeln!(
        out,
        "\nReplacing: {} -> {}",
        displayable(new_file),
        displayable(&original)
    )?;
    let Some(line) = crate::ui::read_line("Are you sure you want to replace this track? (y/N): ")
    else {
        return Ok(false);
    };
    let decision = line.trim().to_lowercase();
    Ok(decision == "yes" || decision == "y")
}

/// The last component's suffix as Python's `Path.suffix` gives it (a
/// leading dot doesn't start one).
fn suffix(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lead = name.len() - name.trim_start_matches('.').len();
    match name[lead..].rfind('.') {
        Some(i) => name[lead + i..].to_string(),
        None => String::new(),
    }
}

/// Python's `Path.with_suffix`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lead = name.len() - name.trim_start_matches('.').len();
    let base = match name[lead..].rfind('.') {
        Some(i) => &name[..lead + i],
        None => name.as_str(),
    };
    path.with_file_name(format!("{base}{suffix}"))
}

/// `replace_file`: move the new file to the old file's path (with the new
/// file's extension), remove the old file if the extension changed, and
/// store the item's new path.
pub fn replace_file(
    new_file: &Path,
    song: &Item,
    lib: &Library,
    out: &mut impl Write,
) -> Result<()> {
    let original = song.path().to_path_buf();
    let dest = with_suffix(&original, &suffix(new_file));

    if let Err(e) = crate::fileops::move_file(new_file, &dest) {
        bail!("Error replacing file: {e:#}");
    }

    if suffix(new_file) != suffix(&original)
        && original.exists()
        && let Err(e) = std::fs::remove_file(&original)
    {
        bail!("Could not delete original file: {e}");
    }

    let mut song = song.clone();
    song.set_path(&dest);
    lib.store(&song)?;

    writeln!(out, "Replacement successful.")?;
    Ok(())
}

#[cfg(test)]
#[path = "replace_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "replace",
        order: 47,
        defaults: "",
        config_keys: &[],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "replace",
                build: build_replace,
                run: run_replace,
            },
        ],
    }
}

fn registry_make(_cfg: &Config) -> Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(ReplacePlugin))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ReplaceArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    // QUERY NEW_FILE_PATH (beets reads the last one as the file).
    #[arg(allow_hyphen_values = true, value_name = "ARGS")]
    args: Vec<String>,
}

pub fn build_replace() -> clap::Command {
    use clap::Args as _;
    ReplaceArgs::augment_args(
        clap::Command::new("replace").about("replace audio file while keeping tags"),
    )
}

pub fn run_replace(
    _cfg: &Config,
    lib: &Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> Result<()> {
    use clap::FromArgMatches as _;
    let a = ReplaceArgs::from_arg_matches(m)?;
    command(
        lib,
        &CmdOpts {
            limit: a.limit,
            args: a.args,
        },
    )
}
