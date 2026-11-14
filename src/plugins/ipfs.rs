//! A port of beets 2.14's `ipfs` plugin (`beetsplug/ipfs.py`): store album
//! and item hashes of an IPFS daemon's `ipfs add` in the `ipfs` flexible
//! attribute, fetch albums back, publish the library and import remote
//! libraries.
//!
//! Needs the `ipfs` command on `PATH` (or the test-only binary override).
//! `-m/--play` plays from the joined remote library through the `play`
//! plugin's own plan/run code, as beets constructs its `PlayPlugin`.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Album, Item, Kind};
use crate::plugins::{ImportTask, Plugin, PluginCtx, PluginLog};
use crate::query::Query;
use crate::template::Template;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[cfg(test)]
thread_local! {
    static TEST_BINARY: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// The `ipfs` executable (tests point it at a fake).
pub fn binary() -> PathBuf {
    #[cfg(test)]
    if let Some(p) = TEST_BINARY.with(|b| b.borrow().clone()) {
        return p;
    }
    PathBuf::from("ipfs")
}

#[cfg(test)]
pub fn set_test_binary(p: Option<PathBuf>) {
    TEST_BINARY.with(|b| *b.borrow_mut() = p);
}

pub struct IPFSPlugin;

impl Plugin for IPFSPlugin {
    fn name(&self) -> &'static str {
        "ipfs"
    }

    /// `auto_add`: an album import adds the album's directory to IPFS.
    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if !ctx.cfg.bool("ipfs.auto") {
            return Ok(());
        }
        let Some(album) = task.album.as_deref_mut() else {
            return Ok(());
        };
        if ipfs_add(ctx.cfg, ctx.lib, album)? {
            ctx.lib.store(album)?;
        }
        Ok(())
    }
}

/// An album's directory (beets' `Album.item_dir`).
pub fn album_dir(lib: &Library, album: &Album) -> Option<PathBuf> {
    lib.album_items(album.id?)
        .ok()?
        .first()
        .and_then(|i| i.path().parent().map(Path::to_path_buf))
}

/// `util.command_output`: run the command, its stdout (an error status is
/// an error).
fn output(cmd: &[String]) -> std::io::Result<String> {
    let out = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "Command '{}' returned non-zero exit status {}",
            cmd.join(" "),
            out.status.code().unwrap_or(-1)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// beets' `ipfs_add`: hash the album directory, store the hashes.
pub fn ipfs_add(cfg: &Config, lib: &Library, album: &mut Album) -> Result<bool> {
    let log = PluginLog::new("ipfs");
    let Some(dir) = album_dir(lib, album) else {
        return Ok(false);
    };
    if !album.str("ipfs").is_empty() {
        log.debug(&format!("{} already added", dir.display()));
        return Ok(false);
    }
    log.info(&format!("Adding {} to ipfs", dir.display()));
    let mut cmd = vec![binary().to_string_lossy().into_owned(), "add".into()];
    if cfg.bool("ipfs.nocopy") {
        cmd.push("--nocopy".into());
    }
    cmd.extend(["-q".into(), "-r".into(), dir.to_string_lossy().into_owned()]);
    let text = match output(&cmd) {
        Ok(t) => t,
        Err(e) => {
            log.error(&format!("Failed to add {}, error: {e}", dir.display()));
            return Ok(false);
        }
    };
    let lines: Vec<&str> = text.split_whitespace().collect();
    let length = lines.len();
    if length > 0 {
        let items = lib.album_items(album.id.unwrap_or(0))?;
        for (n, line) in lines.iter().enumerate() {
            if n == length - 1 {
                log.info(&format!("album: {line}"));
                album.set_str("ipfs", (*line).to_string());
            } else if let Some(mut item) = items.get(n).cloned() {
                log.info(&format!("item: {line}"));
                item.set_str("ipfs", (*line).to_string());
                lib.store(&item)?;
            }
        }
    }
    Ok(true)
}

/// beets' `_remote_libs_path`.
fn remote_libs_path(lib_path: &Path) -> PathBuf {
    lib_path
        .parent()
        .unwrap_or(Path::new(""))
        .join("remotes")
}

/// beets' `remote_lib`: the joined remote library, if there is one.
fn remote_lib(cfg: &Config) -> Result<Library> {
    let path = remote_libs_path(&cfg.library_path()).join("joined.db");
    if !path.is_file() {
        bail!("no imported libraries yet");
    }
    Library::open_with(&path, None, false)
}

/// The album queries in the joined remote library.
fn query_remote(cfg: &Config, query: &[String], limit: Option<usize>) -> Result<Vec<Album>> {
    let rlib = remote_lib(cfg)?;
    let mut albums = rlib.albums(&Query::parse(Kind::Album, query)?)?;
    albums.truncate(limit.unwrap_or(usize::MAX));
    Ok(albums)
}

/// beets' `ipfs_get`: a hash (`Qm...`, 46 chars) or albums' hashes.
pub fn ipfs_get(cfg: &Config, lib: &Library, queries: &[String]) -> Result<()> {
    let Some(query) = queries.first() else {
        return Ok(());
    };
    if query.starts_with("Qm") && query.len() == 46 {
        ipfs_get_from_hash(cfg, lib, query)?;
    } else {
        for album in query_remote(cfg, queries, None)? {
            let hash = album.str("ipfs").to_string();
            if !hash.is_empty() {
                ipfs_get_from_hash(cfg, lib, &hash)?;
            }
        }
    }
    Ok(())
}

/// beets' `ipfs_get_from_hash`: fetch the files and import them.
fn ipfs_get_from_hash(cfg: &Config, lib: &Library, hash: &str) -> Result<()> {
    let log = PluginLog::new("ipfs");
    let cmd = vec![
        binary().to_string_lossy().into_owned(),
        "get".into(),
        hash.to_string(),
    ];
    if let Err(e) = output(&cmd) {
        log.error(&format!("Failed to get {hash} from ipfs.\n{e}"));
        return Ok(());
    }
    log.info(&format!("Getting {hash} from ipfs"));
    let mut opts = crate::importer::ImportOptions::from_config(cfg);
    opts.op = crate::fileops::FileOp::for_import(
        cfg,
        cfg.bool("import.copy"),
        cfg.bool("import.move"),
    );
    let plugins: Vec<Box<dyn Plugin>> = vec![];
    let mut imp = crate::importer::Importer::new(cfg, lib, opts, &plugins)?;
    imp.run(&[PathBuf::from(hash)])?;
    let _ = std::fs::remove_dir_all(hash);
    Ok(())
}

/// beets' `ipfs_publish`: a database of the IPFS-added albums, itself added
/// to IPFS.
pub fn ipfs_publish(cfg: &Config, lib: &Library) -> Result<()> {
    let log = PluginLog::new("ipfs");
    let tmp = std::env::temp_dir().join(format!("beets-ipfs-{}.db", std::process::id()));
    {
        let tmplib = ipfs_added_albums(lib, &tmp)?;
        drop(tmplib);
    }
    let mut cmd = vec![binary().to_string_lossy().into_owned(), "add".into()];
    if cfg.bool("ipfs.nocopy") {
        cmd.push("--nocopy".into());
    }
    cmd.extend(["-q".into(), tmp.to_string_lossy().into_owned()]);
    let result = output(&cmd);
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok(out) => log.info(&format!("hash of library: {}", out.trim_end())),
        Err(err) => {
            log.error(&format!("Failed to publish library. Error: {err}"));
        }
    }
    Ok(())
}

/// beets' `ipfs_import`: fetch a remote database and merge its albums into
/// `remotes/joined.db`.
pub fn ipfs_import(cfg: &Config, _lib: &Library, args: &[String]) -> Result<()> {
    let log = PluginLog::new("ipfs");
    let Some(hash) = args.first() else {
        bail!("no hash given");
    };
    let name = args.get(1).map_or(hash.as_str(), String::as_str);
    let remotes = remote_libs_path(&cfg.library_path());
    if !remotes.exists()
        && let Err(e) = std::fs::create_dir_all(&remotes)
    {
        log.error(&format!("Could not create {}. Error: {e}", remotes.display()));
        return Ok(());
    }
    let path = remotes.join(format!("{name}.db"));
    if !path.exists() {
        let cmd = vec![
            binary().to_string_lossy().into_owned(),
            "get".into(),
            hash.to_string(),
            "-o".into(),
            path.to_string_lossy().into_owned(),
        ];
        if output(&cmd).is_err() {
            log.error(&format!("Could not import {hash}"));
            return Ok(());
        }
    }
    let joined = remotes.join("joined.db");
    let jlib = Library::open_with(&joined, None, false)?;
    let nlib = Library::open_with(&path, None, false)?;
    for album in nlib.albums(&Query::parse::<&str>(Kind::Album, &[])?)? {
        if jlib
            .albums(&Query::parse::<&str>(Kind::Album, &[])?)?
            .iter()
            .any(|a| a.str("mb_albumid") == album.str("mb_albumid"))
        {
            continue;
        }
        let mut items: Vec<Item> = nlib.album_items(album.id.unwrap_or(0))?;
        for item in &mut items {
            item.id = None;
        }
        let mut added = jlib.add_album(&mut items)?;
        added.set_str("ipfs", album.str("ipfs").to_string());
        jlib.store(&added)?;
    }
    Ok(())
}

/// beets' `ipfs_list`.
pub fn ipfs_list(cfg: &Config, _lib: &Library, args: &[String], limit: Option<usize>) -> Result<()> {
    let fmt = Template::parse(&cfg.str("format_album"));
    for album in query_remote(cfg, args, limit)? {
        println!(
            "{} : {}",
            crate::commands::format_record(&fmt, &album, None),
            album.str("ipfs")
        );
    }
    Ok(())
}

/// beets' `ipfs_added_albums`: a library of only the IPFS-added albums,
/// with `/ipfs/<album hash>/<file>` paths.
pub fn ipfs_added_albums(lib: &Library, path: &Path) -> Result<Library> {
    let tmp = Library::open_with(path, None, false)?;
    for album in lib.albums(&Query::parse::<&str>(Kind::Album, &[])?)? {
        let hash = album.str("ipfs").to_string();
        if hash.is_empty() {
            continue;
        }
        let mut items: Vec<Item> = lib.album_items(album.id.unwrap_or(0))?;
        for item in &mut items {
            if item.str("ipfs").is_empty() {
                break;
            }
            let name = item
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            item.set_path(format!("/ipfs/{hash}/{name}"));
            item.id = None;
        }
        if items.is_empty() {
            continue;
        }
        let mut added = tmp.add_album(&mut items)?;
        added.set_str("ipfs", hash);
        tmp.store(&added)?;
    }
    Ok(tmp)
}

/// `beet ipfs [-a] [-g] [-p] [-i] [-l] [-m] [--limit N] [QUERY]`.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub add: bool,
    pub get: bool,
    pub publish: bool,
    pub import: bool,
    pub list: bool,
    pub play: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let log = PluginLog::new("ipfs");
    if opts.add {
        let mut albums = lib.albums(&Query::parse(Kind::Album, &opts.query)?)?;
        albums.truncate(opts.limit.unwrap_or(usize::MAX));
        for mut album in albums {
            if lib.album_items(album.id.unwrap_or(0))?.is_empty() {
                log.info(&format!(
                    "{} does not contain items, aborting",
                    album.str("album")
                ));
            }
            ipfs_add(cfg, lib, &mut album)?;
            lib.store(&album)?;
        }
    }
    if opts.get {
        ipfs_get(cfg, lib, &opts.query)?;
    }
    if opts.publish {
        ipfs_publish(cfg, lib)?;
    }
    if opts.import {
        ipfs_import(cfg, lib, &opts.query)?;
    }
    if opts.list {
        ipfs_list(cfg, lib, &opts.query, opts.limit)?;
    }
    if opts.play {
        ipfs_play(cfg, opts)?;
    }
    Ok(())
}

/// beets' `ipfs_play`: the joined remote library's albums, played by the
/// `play` plugin, with `play.relative_to` unset (beets sets it to None).
fn ipfs_play(cfg: &Config, opts: &CmdOpts) -> Result<()> {
    let mut out = std::io::stdout();
    let mut ask = || {
        match crate::ui::options(
            &["Continue", "Abort"],
            None,
            Some(crate::ui::Choice::Letter('c')),
        ) {
            Some(crate::ui::Choice::Letter(c)) => Some(c),
            _ => None,
        }
    };
    if let Some(plan) = ipfs_plan(cfg, opts, &mut out, &mut ask)? {
        crate::plugins::play::play(&plan, &mut out)?;
    }
    Ok(())
}

/// The plan `ipfs_play` would run (the `play` plugin's, over the joined
/// remote library).
pub fn ipfs_plan(
    cfg: &Config,
    opts: &CmdOpts,
    out: &mut impl std::io::Write,
    ask: &mut dyn FnMut() -> Option<char>,
) -> Result<Option<crate::plugins::play::Plan>> {
    let mut cfg = cfg.clone();
    cfg.set("play.relative_to", serde_yaml::Value::Null);
    let rlib = remote_lib(&cfg)?;
    let play_opts = crate::plugins::play::CmdOpts {
        album: true,
        limit: opts.limit,
        ..Default::default()
    };
    crate::plugins::play::plan(&cfg, &rlib, &play_opts, out, ask)
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct IpfsArgs {
    /// Add to ipfs
    #[arg(short = 'a', long)]
    add: bool,
    /// Get from ipfs
    #[arg(short = 'g', long)]
    get: bool,
    /// Publish local library to ipfs
    #[arg(short = 'p', long)]
    publish: bool,
    /// Import remote library from ipfs
    #[arg(short = 'i', long = "import")]
    import: bool,
    /// Query imported libraries
    #[arg(short = 'l', long = "list")]
    list: bool,
    /// Play music from remote libraries
    #[arg(short = 'm', long)]
    play: bool,
    /// Limit query results
    #[arg(long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_ipfs() -> clap::Command {
    use clap::Args as _;
    IpfsArgs::augment_args(clap::Command::new("ipfs").about("interact with ipfs"))
}

pub fn run_ipfs(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = IpfsArgs::from_arg_matches(m)?;
    command(
        cfg,
        lib,
        &CmdOpts {
            add: a.add,
            get: a.get,
            publish: a.publish,
            import: a.import,
            list: a.list,
            play: a.play,
            limit: a.limit,
            query: a.query,
        },
    )
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "ipfs",
        order: 66,
        defaults: r#"ipfs:
    auto: yes
    nocopy: no
"#,
        config_keys: &["ipfs"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "ipfs",
            build: build_ipfs,
            run: run_ipfs,
        }],
    }
}

fn registry_make(
    _cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(IPFSPlugin))
}

#[cfg(test)]
#[path = "ipfs_tests.rs"]
mod tests;
