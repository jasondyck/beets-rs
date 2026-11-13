//! beets' `fish` plugin: generate Fish shell completions for `beet`.

use crate::config::Config;
use crate::library::Library;
use crate::model::{ALBUM_FIELDS, ITEM_FIELDS, Kind, Record};
use crate::plugins::Plugin;
use crate::query::Query;
use anyhow::{Context, Result};
use clap::CommandFactory;
use std::collections::BTreeSet;

pub struct FishPlugin;

impl FishPlugin {
    pub fn new() -> Self {
        FishPlugin
    }
}

impl Default for FishPlugin {
    fn default() -> Self {
        FishPlugin::new()
    }
}

impl Plugin for FishPlugin {
    fn name(&self) -> &'static str {
        "fish"
    }
}

pub struct CmdOpts {
    pub no_fields: bool,
    pub extra_values: Vec<String>,
    pub output: String,
}

const HEAD: &str = r#"
function __fish_beet_needs_command
    set cmd (commandline -opc)
    if test (count $cmd) -eq 1
        return 0
    end
    return 1
end

function __fish_beet_using_command
    set cmd (commandline -opc)
    set needle (count $cmd)
    if test $needle -gt 1
        if begin test $argv[1] = $cmd[2];
            and not contains -- $cmd[$needle] $FIELDS; end
                return 0
        end
    end
    return 1
end

function __fish_beet_use_extra
    set cmd (commandline -opc)
    set needle (count $cmd)
    if test $argv[2]  = $cmd[$needle]
        return 0
    end
    return 1
end
"#;

fn wrap(word: &str) -> String {
    if word.contains('"') && word.contains('\'') {
        format!("\"{}\"", word.replace('"', "\\\""))
    } else if word.contains('\'') {
        format!("\"{word}\"")
    } else {
        format!("'{word}'")
    }
}

fn escape(name: &str) -> String {
    if name == "?" { "\\?".into() } else { name.into() }
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text(s: &str) -> String {
    s.to_string()
}

pub fn command(_cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let path = crate::config::expand_tilde(&opts.output);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut fields: BTreeSet<String> = ITEM_FIELDS
        .iter()
        .map(|(n, _)| n.to_string())
        .chain(ALBUM_FIELDS.iter().map(|(n, _)| n.to_string()))
        .collect();
    for f in ["singleton", "missing", "atypes", "album_artist_no_feat"] {
        fields.insert(f.to_string());
    }
    let mut out = format!("{HEAD}\n");
    let cli = crate::Cli::command();
    let mut cmds: Vec<(String, String)> = vec![];
    for sub in cli.get_subcommands() {
        let help = sub
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        cmds.push((sub.get_name().to_string(), help.clone()));
        for a in sub.get_all_aliases() {
            cmds.push((a.to_string(), help.clone()));
        }
    }
    cmds.sort_by(|a, b| a.0.cmp(&b.0));
    out.push_str(&format!(
        "set CMDS {}\n\n",
        cmds.iter().map(|c| c.0.as_str()).collect::<Vec<_>>().join(" ")
    ));
    if !opts.no_fields {
        out.push_str(&format!(
            "set FIELDS {}\n\n",
            fields.iter().map(|f| format!("{f}:")).collect::<Vec<_>>().join(" ")
        ));
    }
    let mut extra_sets = vec![];
    for field in &opts.extra_values {
        let mut values: BTreeSet<String> = BTreeSet::new();
        let recs: Vec<Record> = if ALBUM_FIELDS.iter().any(|(n, _)| n == field)
            && !ITEM_FIELDS.iter().any(|(n, _)| n == field)
        {
            lib.albums(&Query::parse::<&str>(Kind::Album, &[])?)?
        } else {
            lib.items(&Query::parse::<&str>(Kind::Item, &[])?)?
        };
        for r in &recs {
            values.insert(text(&r.formatted(field).unwrap_or_default()));
        }
        out.push_str(&format!(
            "set  {}S {}\n\n",
            field.to_uppercase(),
            values.into_iter().collect::<Vec<_>>().join(" ")
        ));
        extra_sets.push(field.clone());
    }
    out.push_str("\n# ====== setup basic beet completion ======\n\n");
    for (short, long, desc) in [
        ("f", "format-item", "print with custom format"),
        ("f", "format-album", "print with custom format"),
        ("l", "library", "library database file to use"),
        ("d", "directory", "destination music directory"),
        ("v", "verbose", "print debugging information"),
        ("c", "config", "path to configuration file"),
        ("h", "help", "print this help message and exit"),
    ] {
        out.push_str(&format!(
            "complete -c beet -n '__fish_beet_needs_command' -s {short} -l {long} -f -d '{desc}'\n"
        ));
    }
    out.push_str("\n# ====== setup field completion for subcommands ======\n");
    for (name, help) in &cmds {
        let n = escape(name);
        out.push_str(&format!("\n# ------ fieldsetups for {n} -------\n"));
        out.push_str(&format!(
            "complete -c beet -n '__fish_beet_needs_command' -a {n} -f -d {}\n",
            wrap(&clean(help))
        ));
        if !opts.no_fields {
            out.push_str(&format!(
                "complete -c beet -n '__fish_beet_using_command {n}' -a '$FIELDS' -d 'fieldname'\n"
            ));
        }
        for f in &extra_sets {
            out.push_str(&format!(
                "complete -c beet -n '__fish_beet_use_extra {n} {f}:' -f -A -a ${}S -d {}\n",
                f.to_uppercase(),
                wrap(f)
            ));
        }
    }
    out.push_str("\n# ====== completions for commands ======\n");
    for sub in cli.get_subcommands() {
        let mut names = vec![sub.get_name().to_string()];
        names.extend(sub.get_all_aliases().map(ToString::to_string));
        for name in names {
            let n = escape(&name);
            out.push_str(&format!("\n\n\n# ====== completions for {n} =====\n"));
            for arg in sub.get_arguments().filter(|a| !a.is_hide_set()) {
                let long = arg
                    .get_long()
                    .map(|l| format!(" -l {l}"))
                    .unwrap_or_default();
                let short = arg
                    .get_short()
                    .map(|s| format!(" -s {s}"))
                    .unwrap_or_default();
                let help = arg
                    .get_help()
                    .map(|h| format!(" -d {}", wrap(&clean(&h.to_string()))))
                    .unwrap_or_default();
                let args = if arg.get_action().takes_values() {
                    " -r "
                } else {
                    ""
                };
                out.push_str(&format!(
                    "complete -c beet -n '__fish_beet_using_command {n}' {args}{short}{long}{help}\n"
                ));
            }
            out.push_str(&format!(
                "complete -c beet -n '__fish_beet_using_command {n}' -s h -l help -d 'print help'\n"
            ));
        }
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The default output path (beets' `~/.config/fish/completions/beet.fish`).
#[cfg(test)]
#[path = "fish_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "fish",
        order: 41,
        // beets' plugin adds no settings.
        defaults: r#""#,
        config_keys: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            order: 370,
            plugin: "fish",
            build: build_fish,
            run: run_fish,
        }],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(FishPlugin::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct FishArgs {
    /// Omit album/track field completions
    #[arg(short = 'f', long = "noFields")]
    no_fields: bool,
    /// Include specified field values in completions (repeatable)
    #[arg(short = 'e', long = "extravalues", value_name = "FIELD")]
    extra_values: Vec<String>,
    /// Where to save the script
    #[arg(short = 'o', long, default_value = "~/.config/fish/completions/beet.fish")]
    output: String,
}

pub fn build_fish() -> clap::Command {
    use clap::Args as _;
    FishArgs::augment_args(
        clap::Command::new("fish").about("Generate Fish shell tab completions (plugin: fish)"),
    )
}

pub fn run_fish(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = FishArgs::from_arg_matches(m)?;
    crate::plugins::fish::command(
        cfg,
        lib,
        crate::plugins::fish::CmdOpts {
            no_fields: a.no_fields,
            extra_values: a.extra_values,
            output: a.output,
        },
    )
}
