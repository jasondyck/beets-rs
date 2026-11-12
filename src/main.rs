mod artresizer;
mod autotag;
mod commands;
mod config;
mod diff;
mod display;
mod duplicates;
mod fileops;
mod formatted;
mod hooks;
mod importer;
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
mod plugins;
mod props;
mod query;
mod state;
mod template;
mod ui;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgAction, Parser, Subcommand};
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
    /// Download album art (plugin: fetchart)
    Fetchart {
        /// Re-download art when already present
        #[arg(short = 'f', long)]
        force: bool,
        /// Quiet mode: do not output albums that already have artwork
        #[arg(short = 'q', long)]
        quiet: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Embed image files into file metadata (plugin: embedart)
    Embedart {
        /// The image file to embed
        #[arg(short = 'f', long = "file", value_name = "PATH")]
        file: Option<PathBuf>,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
        /// The URL of the image file to embed
        #[arg(short = 'u', long, value_name = "URL")]
        url: Option<String>,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Extract an image from file metadata (plugin: embedart)
    Extractart {
        /// Image output file
        #[arg(short = 'o', value_name = "OUTPATH")]
        outpath: Option<PathBuf>,
        /// Image filename to create for all matched albums
        #[arg(short = 'n', value_name = "FILENAME")]
        filename: Option<String>,
        /// Associate the extracted images with the album
        #[arg(short = 'a')]
        associate: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Remove images from file metadata (plugin: embedart)
    Clearart {
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Fetch song lyrics (plugin: lyrics)
    Lyrics {
        /// Print lyrics to console
        #[arg(short = 'p', long)]
        print: bool,
        /// Write lyrics to given directory as ReST files
        #[arg(short = 'r', long = "write-rest", value_name = "dir")]
        rest_directory: Option<String>,
        /// Always re-download lyrics
        #[arg(short = 'f', long)]
        force: bool,
        /// Skip items that already have synced lyrics
        #[arg(long, overrides_with = "no_keep_synced")]
        keep_synced: bool,
        /// Do not skip items that already have synced lyrics
        #[arg(long, overrides_with = "keep_synced")]
        no_keep_synced: bool,
        /// Do not fetch missing lyrics
        #[arg(short = 'l', long)]
        local: bool,
        /// Limit query results
        #[arg(long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Fetch genres (plugin: lastgenre)
    Lastgenre {
        /// Show actions but do nothing
        #[arg(short = 'p', long)]
        pretend: bool,
        /// Modify existing genres
        #[arg(short = 'f', long, overrides_with = "no_force")]
        force: bool,
        /// Don't modify existing genres
        #[arg(short = 'F', long, overrides_with = "force")]
        no_force: bool,
        /// Combine with existing genres when modifying
        #[arg(short = 'k', long, overrides_with = "no_keep_existing")]
        keep_existing: bool,
        /// Don't combine with existing genres when modifying
        #[arg(short = 'K', long, overrides_with = "keep_existing")]
        no_keep_existing: bool,
        /// Genre source: artist, album, or track
        #[arg(short = 's', long, value_name = "SOURCE")]
        source: Option<String>,
        /// Match items instead of albums
        #[arg(short = 'A', long, overrides_with = "albums")]
        items: bool,
        /// Match albums instead of items (default)
        #[arg(short = 'a', long, overrides_with = "items")]
        albums: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Show file metadata (plugin: info)
    Info {
        /// Match albums instead of tracks
        #[arg(short = 'a', long)]
        album: bool,
        /// Show library fields instead of tags
        #[arg(short = 'l', long)]
        library: bool,
        /// Summarize the tags of all files
        #[arg(short = 's', long)]
        summarize: bool,
        /// Comma separated list of keys to show
        #[arg(short = 'i', long = "include-keys", value_name = "INCLUDED_KEYS")]
        included_keys: Vec<String>,
        /// Show only the keys
        #[arg(short = 'k', long = "keys-only")]
        keys_only: bool,
        /// Print with custom format
        #[arg(short = 'f', long)]
        format: Option<String>,
        /// Limit query results
        #[arg(long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List duplicate tracks or albums (plugin: duplicates)
    #[command(visible_alias = "dup")]
    Duplicates {
        /// Show duplicate counts
        #[arg(short = 'c', long)]
        count: bool,
        /// Report duplicates based on arbitrary command
        #[arg(short = 'C', long, value_name = "PROG", allow_hyphen_values = true)]
        checksum: Option<String>,
        /// Delete items from library and disk
        #[arg(short = 'd', long)]
        delete: bool,
        /// Show all versions of duplicate tracks or albums
        #[arg(short = 'F', long)]
        full: bool,
        /// Report duplicates only if all attributes are set
        #[arg(short = 's', long)]
        strict: bool,
        /// Report duplicates based on keys (use multiple times)
        #[arg(short = 'k', long = "key", value_name = "KEY")]
        keys: Vec<String>,
        /// Merge duplicate items
        #[arg(short = 'M', long)]
        merge: bool,
        /// Move items to dest
        #[arg(short = 'm', long = "move", value_name = "DEST")]
        move_to: Option<String>,
        /// Copy items to dest
        #[arg(short = 'o', long = "copy", value_name = "DEST")]
        copy_to: Option<String>,
        /// Tag matched items with 'k=v' attribute
        #[arg(short = 't', long)]
        tag: Option<String>,
        /// Remove items from library
        #[arg(short = 'r', long)]
        remove: bool,
        /// Match albums instead of tracks
        #[arg(short = 'a', long)]
        album: bool,
        /// Print paths for matched items or albums
        #[arg(short = 'p', long)]
        path: bool,
        /// Print with custom format
        #[arg(short = 'f', long)]
        format: Option<String>,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// List missing tracks (plugin: missing)
    #[command(visible_alias = "miss")]
    Missing {
        /// Count missing tracks per album
        #[arg(short = 'c', long)]
        count: bool,
        /// Count total of missing tracks
        #[arg(short = 't', long)]
        total: bool,
        /// Show missing album releases for artist instead of tracks;
        /// Defaults to only releases of type 'album'
        #[arg(short = 'a', long)]
        album: bool,
        /// Comma-separated list of release types for missing albums (valid:
        /// nat, album, single, ep, broadcast, other, compilation, soundtrack,
        /// spokenword, interview, audiobook, live, remix, dj-mix,
        /// mixtape/street)
        #[arg(long = "release-types", value_name = "RELEASE_TYPES")]
        release_types: Vec<String>,
        /// Print with custom format
        #[arg(short = 'f', long)]
        format: Option<String>,
        /// Limit query results
        #[arg(short = 'l', long)]
        limit: Option<usize>,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
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

/// Parse the command line; unknown commands get beets' error message.
pub fn parse_cli<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Cli::try_parse_from(args)
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
    let cli = match parse_cli(std::env::args_os()) {
        Ok(cli) => cli,
        Err(e) => match unknown_command(&e) {
            Some(name) => {
                eprintln!("error: unknown command '{name}'");
                std::process::exit(1);
            }
            None => e.exit(),
        },
    };
    if let Err(e) = commands::run(cli) {
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
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        parse_cli(std::iter::once("beet").chain(args.iter().copied()))
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
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
