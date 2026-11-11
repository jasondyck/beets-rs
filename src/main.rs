mod autotag;
mod commands;
mod config;
mod importer;
mod library;
mod mb;
mod media;
mod model;
mod paths;
mod plugins;
mod query;
mod state;
mod template;
mod ui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// A Rust port of the beets music library manager.
#[derive(Parser)]
#[command(name = "beet", version, about)]
pub struct Cli {
    /// Path to the configuration file
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,
    /// Library database file
    #[arg(short = 'l', long = "library")]
    pub library: Option<PathBuf>,
    /// Destination music directory
    #[arg(short = 'd', long = "directory")]
    pub directory: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
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
        #[arg(short = 'a', long)]
        album: bool,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
        /// Don't move files to match new metadata
        #[arg(short = 'M', long = "nomove")]
        nomove: bool,
        /// Don't write tags
        #[arg(short = 'W', long = "nowrite")]
        nowrite: bool,
        #[arg(required = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove matching items from the library
    #[command(visible_alias = "rm")]
    Remove {
        #[arg(short = 'a', long)]
        album: bool,
        /// Also delete files from disk
        #[arg(short = 'd', long)]
        delete: bool,
        /// Don't ask for confirmation
        #[arg(short = 'f', long)]
        force: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Move or copy items to their library paths (or another directory)
    #[command(visible_alias = "mv")]
    Move {
        #[arg(short = 'a', long)]
        album: bool,
        /// Destination directory instead of the library directory
        #[arg(short = 'd', long = "dest")]
        dest: Option<PathBuf>,
        /// Copy instead of move
        #[arg(short = 'c', long)]
        copy: bool,
        /// Show what would be moved
        #[arg(short = 'p', long)]
        pretend: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Re-read tags from files, dropping items whose files are gone
    #[command(visible_alias = "upd", visible_alias = "up")]
    Update {
        #[arg(short = 'p', long)]
        pretend: bool,
        /// Don't move files to match updated metadata
        #[arg(short = 'M', long = "nomove")]
        nomove: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Write library metadata to files' tags
    Write {
        #[arg(short = 'p', long)]
        pretend: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Show library statistics
    Stats {
        #[arg(short = 'e', long)]
        exact: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// List available fields
    Fields,
    /// Fetch album art (plugin: fetchart)
    Fetchart {
        /// Re-fetch even if the album already has art
        #[arg(short = 'f', long)]
        force: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Embed album art into files (plugin: embedart)
    Embedart {
        /// Embed this image instead of each album's art file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Extract embedded art to a file (plugin: embedart)
    Extractart {
        /// Output path without extension (default: ./cover)
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Base name for files written next to albums with -a (default: fetchart.art_filename)
        #[arg(short = 'n', long)]
        name: Option<String>,
        /// Extract each matching album's art into its folder and set it as the album art
        #[arg(short = 'a', long)]
        associate: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Remove embedded art from files (plugin: embedart)
    Clearart {
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Fetch lyrics (plugin: lyrics)
    Lyrics {
        /// Print the lyrics
        #[arg(short = 'p', long)]
        print: bool,
        /// Re-fetch even if the item already has lyrics
        #[arg(short = 'f', long)]
        force: bool,
        #[arg(allow_hyphen_values = true)]
        query: Vec<String>,
    },
    /// Show the effective configuration
    Config {
        /// Print the config file path
        #[arg(short = 'p', long)]
        paths: bool,
        /// Show the built-in defaults
        #[arg(short = 'd', long)]
        defaults: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = commands::run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
