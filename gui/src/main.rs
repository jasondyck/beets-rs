//! beet-gui: a GPUI desktop front end for beets-rs.
//!
//! Browsing reads the library directly (on a worker thread). Everything
//! that changes it — editing, moving, removing, importing, plugin commands
//! — runs the real `beet` command line (this same executable started with
//! `--beet`), so behaviour matches the CLI exactly and its prompts are
//! answered from the UI.
//!
//! Usage: `beet-gui [-c CONFIG]… [-l LIBRARY] [-d DIRECTORY]`
//! (`BEETSRSDIR` is honoured, as by `beet`).

mod app;
mod backend;
mod catalog;
mod runner;

use std::path::PathBuf;

use gpui_kit::*;

fn parse_opts() -> Result<backend::Opts, String> {
    let mut opts = backend::Opts::default();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy().into_owned();
        let mut value = |name: &str| {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-c" | "--config" => opts.config.push(value(&arg)?),
            "-l" | "--library" => opts.library = Some(value(&arg)?),
            "-d" | "--directory" => opts.directory = Some(value(&arg)?),
            "-h" | "--help" => {
                println!(
                    "usage: beet-gui [-c CONFIG]… [-l LIBRARY] [-d DIRECTORY]\n\n\
                     A desktop front end for beets-rs. `beet-gui --beet ARGS…` runs the\n\
                     command line instead."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(opts)
}

fn main() {
    // Multi-call: the GUI runs `beet` commands as `beet-gui --beet ARGS…`.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == runner::BEET_FLAG)
    {
        let args = std::iter::once("beet".into()).chain(std::env::args_os().skip(2));
        beets_rs::run_cli_with(args);
        return;
    }
    let opts = match parse_opts() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("beet-gui: {e}");
            std::process::exit(2);
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            app::bind_keys(cx);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("beets".into()),
                    ..TitlebarOptions::default()
                }),
                window_bounds: Some(WindowBounds::centered(size(px(1400.), px(880.)), cx)),
                window_min_size: Some(size(px(900.), px(560.))),
                app_id: Some("beet-gui".into()),
                ..WindowOptions::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| app::BeetsApp::new(opts, window, cx))
            })
            .expect("failed to open the window");
            cx.activate(true);
        });
}
