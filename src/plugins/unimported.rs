//! A port of beets 2.14's `unimported` plugin (`beetsplug/unimported.py`):
//! `beet unimported` lists the files in the library directory that are
//! neither items' files nor albums' art, except files with an
//! `ignore_extensions` extension and those under `ignore_subdirectories`.

use crate::config::Config;
use crate::library::Library;
use crate::model::Kind;
use crate::query::Query;
use anyhow::Result;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct UnimportedPlugin;

impl crate::plugins::Plugin for UnimportedPlugin {
    fn name(&self) -> &'static str {
        "unimported"
    }
}

/// `beet unimported`, printing to stdout.
pub fn command(cfg: &Config, lib: &Library) -> Result<()> {
    run(cfg, lib, &mut std::io::stdout().lock())
}

/// Python's `os.walk(top)`: (dir, files) top-down, in directory order.
fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<PathBuf>)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = vec![];
    let mut dirs = vec![];
    for e in rd.flatten() {
        let p = e.path();
        // `os.walk` doesn't follow symlinked directories.
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    out.push((dir.to_path_buf(), files));
    for d in dirs {
        walk(&d, out);
    }
}

pub fn run(cfg: &Config, lib: &Library, out: &mut impl Write) -> Result<()> {
    let libdir = lib
        .music_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cfg.directory());
    let exts: Vec<String> = cfg
        .list("unimported.ignore_extensions")
        .iter()
        .map(|x| format!(".{}", x.to_lowercase()))
        .collect();
    let ignore_dirs: Vec<PathBuf> = cfg
        .list("unimported.ignore_subdirectories")
        .iter()
        .map(|x| libdir.join(x))
        .collect();
    let all = |k| Query::parse(k, &[] as &[String]);
    let mut known: HashSet<PathBuf> = lib
        .items(&all(Kind::Item)?)?
        .iter()
        .map(|i| i.path().to_path_buf())
        .collect();
    for a in lib.albums(&all(Kind::Album)?)? {
        let art = a.path_field("artpath");
        if !art.as_os_str().is_empty() {
            known.insert(art.to_path_buf());
        }
    }
    let mut tree = vec![];
    walk(&libdir, &mut tree);
    for (root, files) in tree {
        // Not under an ignored directory (or one itself).
        if root.ancestors().any(|a| ignore_dirs.iter().any(|d| d == a)) {
            continue;
        }
        for f in files {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if exts.iter().any(|e| name.ends_with(e.as_str())) {
                continue;
            }
            if !known.contains(&f) {
                writeln!(out, "{}", f.display())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "unimported_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "unimported",
        order: 33,
        defaults: r#"unimported:
    ignore_extensions: []
    ignore_subdirectories: []
"#,
        config_keys: &["unimported"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "unimported",
            build: build_unimported,
            run: run_unimported,
        }],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(UnimportedPlugin))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct UnimportedArgs {}

pub fn build_unimported() -> clap::Command {
    use clap::Args as _;
    UnimportedArgs::augment_args(clap::Command::new("unimported").about(
        "list all files in the library folder which are not listed in the beets library database",
    ))
}

pub fn run_unimported(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let _a = UnimportedArgs::from_arg_matches(m)?;
    crate::plugins::unimported::command(cfg, lib)
}
