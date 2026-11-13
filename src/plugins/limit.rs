//! beets' `limit` plugin: the `<N` query prefix and the `lslimit`
//! command.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Kind, Record};
use crate::plugins::Plugin;
use crate::query::Query;
use crate::template::Template;
use anyhow::Result;

pub struct LimitPlugin;

impl LimitPlugin {
    /// beets' `__init__` deprecates the plugin in favour of `-l`.
    pub fn new() -> Self {
        crate::plugins::PluginLog::new("limit").warning(
            "LimitPlugin ('limit') is deprecated and will be removed in version 3.0.0. \
             Use 'beet ls -l <number>' instead.",
        );
        LimitPlugin
    }
}

impl Plugin for LimitPlugin {
    fn name(&self) -> &'static str {
        "limit"
    }
}

pub struct CmdOpts {
    pub album: bool,
    pub head: Option<usize>,
    pub limit: Option<usize>,
    pub tail: Option<usize>,
    pub format: Option<String>,
    pub path: bool,
    pub query: Vec<String>,
}

fn format_one(cfg: &Config, lib: &Library, rec: &Record, fmt: &Option<String>, path: bool) -> String {
    if path {
        return if rec.kind == Kind::Album {
            rec.path_field("path").display().to_string()
        } else {
            rec.path().display().to_string()
        };
    }
    let template = match fmt {
        Some(f) => f.clone(),
        None if rec.kind == Kind::Album => cfg.str("format_album"),
        None => cfg.str("format_item"),
    };
    crate::commands::format_record(&Template::parse(&template), rec, Some(lib))
}

pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    if opts.head.is_some() && opts.tail.is_some() {
        anyhow::bail!("Only use one of --head and --tail");
    }
    if opts.head.unwrap_or(0) > 0 || opts.tail.unwrap_or(0) > 0 {
        // Both options are usize, so a negative value never reaches here.
    }
    let q = Query::parse(if opts.album { Kind::Album } else { Kind::Item }, &opts.query)?;
    let mut recs: Vec<Record> = if opts.album {
        lib.albums(&q)?
    } else {
        lib.items(&q)?
    };
    // beets passes `-l` to `lib.items`, then takes the head/tail of that.
    if let Some(n) = opts.limit {
        recs.truncate(n);
    }
    if let Some(n) = opts.head {
        recs.truncate(n);
    } else if let Some(n) = opts.tail {
        let keep = recs.len().saturating_sub(n);
        recs.drain(..keep);
    }
    for r in recs {
        println!("{}", format_one(cfg, lib, &r, &opts.format, opts.path));
    }
    Ok(())
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "limit",
        order: 45,
        // beets' plugin adds no settings.
        defaults: r#""#,
        config_keys: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            order: 480,
            plugin: "limit",
            build: build_lslimit,
            run: run_lslimit,
        }],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(LimitPlugin::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct LslimitArgs {
    /// Show matching albums instead of tracks
    #[arg(short = 'a', long)]
    album: bool,
    /// Print paths for matched items or albums
    #[arg(short = 'p', long)]
    path: bool,
    /// Print with a custom format template
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Only the first N results
    #[arg(long)]
    head: Option<usize>,
    /// Only the last N results
    #[arg(long)]
    tail: Option<usize>,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_lslimit() -> clap::Command {
    use clap::Args as _;
    LslimitArgs::augment_args(
        clap::Command::new("lslimit").about("Query with optional head or tail (plugin: limit)"),
    )
}

pub fn run_lslimit(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = LslimitArgs::from_arg_matches(m)?;
    crate::plugins::limit::command(
        cfg,
        lib,
        crate::plugins::limit::CmdOpts {
            album: a.album,
            head: a.head,
            limit: a.limit,
            tail: a.tail,
            format: a.format,
            path: a.path,
            query: a.query,
        },
    )
}
