//! beets-rs: the library behind the `beet` command line and the GUI.
//!
//! (Kept at `src/main.rs`, which was the binary's root, so branches that
//! touch it still merge; the binary itself is `src/bin/beet.rs`.)
//!
//! [`run_cli`] is the command line's entry point: parse the command line (the clap
//! definition, help formatting and precheck live in [`cli`]), load the
//! plugins and library, and dispatch to [`commands`].

// Every module is public only so the GUI can reach the library; these two
// lints fire on public items and were not reported while this was a binary.
#![allow(clippy::new_without_default, clippy::should_implement_trait)]

pub mod artresizer;
pub mod asf;
pub mod audioprops;
pub mod autotag;
pub mod cli;
pub mod commands;
pub mod config;
pub mod diff;
pub mod display;
pub mod duplicates;
pub mod events;
pub mod fileops;
pub mod formatted;
pub mod hooks;
pub mod importer;
pub mod inline;
pub mod langdetect;
pub mod layout;
pub mod library;
pub mod logging;
pub mod mb;
pub mod media;
pub mod mediafile;
pub mod metadata_plugins;
pub mod migrate;
pub mod model;
pub mod osutil;
pub mod paths;
pub mod pipeline;
pub mod plugin_registry;
pub mod plugins;
pub mod pyyaml;
pub mod query;
pub mod state;
pub mod subcommand_help;
pub mod template;
pub mod ui;

use clap::error::ErrorKind;

use cli::precheck;
/// The CLI definition and help/precheck code (moved from this file).
pub use cli::{
    Cli, Cmd, HelpCommand, Parsed, canonical_command, cli, comma_list, help_commands, parse_args,
    root_help_text, subcommand_help_text, unknown_command,
};

/// Run the `beet` command line with the process's arguments.
pub fn run_cli() {
    run_cli_with(std::env::args_os());
}

/// [`run_cli`] with explicit arguments (the first is the program name), for
/// front ends that embed the command line, such as the GUI.
pub fn run_cli_with<I, T>(args: I)
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    // A closed pipe (`beet ls | head`) ends the program quietly, as beets
    // does on EPIPE, instead of panicking in `println!`.
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of a signal before any other
    // threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let raw: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
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
