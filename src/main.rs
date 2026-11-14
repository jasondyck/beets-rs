//! The `beet` binary: a Rust port of the beets music library manager.
//!
//! This file is the thin entry point: parse the command line (the clap
//! definition, help formatting and precheck live in [`cli`]), load the
//! plugins and library, and dispatch to [`commands`].

mod artresizer;
mod asf;
mod audioprops;
mod autotag;
mod cli;
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
mod langdetect;
mod layout;
mod library;
mod logging;
mod mb;
mod media;
mod mediafile;
mod metadata_plugins;
mod migrate;
mod model;
mod paths;
mod pipeline;
mod plugin_registry;
mod plugins;
mod pyyaml;
mod query;
mod state;
mod subcommand_help;
mod template;
mod ui;

use clap::error::ErrorKind;

use cli::precheck;
/// The CLI definition and help/precheck code (moved from this file).
pub use cli::{
    Cli, Cmd, HelpCommand, Parsed, canonical_command, cli, comma_list, help_commands, parse_args,
    root_help_text, subcommand_help_text, unknown_command,
};

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
    let (args, root_help, enabled, global, command) = precheck(&raw);
    // As beets, the enabled plugins load (and may warn) before help is
    // printed; unlike beets, without opening the library.
    let load_plugins = |cfg: &Option<crate::config::Config>| {
        if let Some(cfg) = cfg {
            let _ = crate::plugins::load(cfg);
        }
    };
    if root_help {
        load_plugins(&global);
        print!("{}", root_help_text(&enabled));
        return;
    }
    let parsed = match parse_args(args, &enabled) {
        Ok(parsed) => parsed,
        Err(e) => match unknown_command(&e) {
            Some(name) => {
                eprintln!("error: unknown command '{name}'");
                std::process::exit(1);
            }
            None => {
                if e.kind() == ErrorKind::DisplayHelp
                    && let Some(text) = command.as_deref().and_then(subcommand_help_text)
                {
                    load_plugins(&global);
                    print!("{text}");
                    return;
                }
                e.exit()
            }
        },
    };
    let result = match parsed {
        Parsed::Cli(cli) => commands::run(cli),
        Parsed::Plugin {
            command,
            globals,
            matches,
        } => commands::run_plugin(command, &globals, &matches),
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
