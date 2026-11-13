//! beets' `bareasc` plugin: the `#` query prefix and the `bareasc`
//! command (matches and prints unidecoded text).

use crate::config::Config;
use crate::plugins::Plugin;
use deunicode::deunicode;

pub struct BareascPlugin;

impl BareascPlugin {
    pub fn new(_cfg: &Config) -> Self {
        BareascPlugin
    }
}

impl Plugin for BareascPlugin {
    fn name(&self) -> &'static str {
        "bareasc"
    }
}

/// Python `unidecode`'s text. `deunicode`'s table differs for the vulgar
/// fractions (unidecode separates them from preceding digits, and the
/// Unicode Number Forms have a trailing space too) and for `™`.
pub fn unidecode(s: &str) -> String {
    const SPECIAL: &[(char, &str)] = &[
        ('½', " 1/2"),
        ('¼', " 1/4"),
        ('¾', " 3/4"),
        ('™', "(tm)"),
        ('⅐', " 1/7 "),
        ('⅑', " 1/9 "),
        ('⅒', " 1/10 "),
        ('⅓', " 1/3 "),
        ('⅔', " 2/3 "),
        ('⅕', " 1/5 "),
        ('⅖', " 2/5 "),
        ('⅗', " 3/5 "),
        ('⅘', " 4/5 "),
        ('⅙', " 1/6 "),
        ('⅚', " 5/6 "),
        ('⅛', " 1/8 "),
        ('⅜', " 3/8 "),
        ('⅝', " 5/8 "),
        ('⅞', " 7/8 "),
        ('⅟', " 1/"),
    ];
    if !s.chars().any(|c| SPECIAL.iter().any(|(p, _)| *p == c)) {
        return deunicode(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match SPECIAL.iter().find(|(p, _)| *p == c) {
            Some((_, repl)) => out.push_str(repl),
            None => out.push_str(&deunicode(&c.to_string())),
        }
    }
    out
}

fn is_lower(s: &str) -> bool {
    s.chars().any(char::is_lowercase) && !s.chars().any(char::is_uppercase)
}

/// `BareascQuery.string_match`: smartcase, then a bare-ASCII substring.
pub fn matches(pattern: &str, val: &str) -> bool {
    let p = unidecode(pattern);
    let v = unidecode(val);
    if is_lower(pattern) {
        v.to_lowercase().contains(&p.to_lowercase())
    } else {
        v.contains(&p)
    }
}

pub struct CmdOpts {
    pub album: bool,
    pub path: bool,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

pub fn command(cfg: &Config, lib: &crate::library::Library, opts: CmdOpts) -> anyhow::Result<()> {
    use crate::model::{Kind, Record};
    use crate::template::Template;
    let kind = if opts.album { Kind::Album } else { Kind::Item };
    let q = crate::query::Query::parse(kind, &opts.query)?;
    let mut recs: Vec<Record> = if opts.album {
        lib.albums(&q)?
    } else {
        lib.items(&q)?
    };
    if let Some(n) = opts.limit {
        recs.truncate(n);
    }
    for r in recs {
        let line = if opts.path {
            r.path().display().to_string()
        } else {
            let fmt = match &opts.format {
                Some(f) => f.clone(),
                None if opts.album => cfg.str("format_album"),
                None => cfg.str("format_item"),
            };
            crate::commands::format_record(&Template::parse(&fmt), &r, Some(lib))
        };
        println!("{}", unidecode(&line));
    }
    Ok(())
}

#[cfg(test)]
#[path = "bareasc_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "bareasc",
        order: 70,
        defaults: r#"bareasc:
    prefix: '#'
"#,
        config_keys: &["bareasc"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "bareasc",
            build: build_bareasc,
            run: run_bareasc,
        }],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(BareascPlugin::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct BareascArgs {
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
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_bareasc() -> clap::Command {
    use clap::Args as _;
    BareascArgs::augment_args(clap::Command::new("bareasc").about(
        "unidecode version of beet list command",
    ))
}

pub fn run_bareasc(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = BareascArgs::from_arg_matches(m)?;
    crate::plugins::bareasc::command(
        cfg,
        lib,
        crate::plugins::bareasc::CmdOpts {
            album: a.album,
            path: a.path,
            format: a.format,
            limit: a.limit,
            query: a.query,
        },
    )
}
