//! A port of beets 2.14's `play` plugin (`beetsplug/play.py`): `beet play`
//! sends the items or albums matching a query to a player, as a temporary
//! `.m3u` playlist (or, with `raw`, as file arguments), and the importer
//! prompt gets a "plaY" choice.
//!
//! Running the player is `interactive_open`'s `exec` (beets replaces
//! itself with the player; so do we) — the tests only look at the command
//! and the playlist that would be handed over.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Kind, Record};
use crate::plugins::PluginLog;
use crate::query::Query;
use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Indicate where arguments should be inserted into the command string.
/// If this is missing, they're placed at the end.
pub const ARGS_MARKER: &str = "$args";

/// Indicate where the playlist file (with absolute path) should be
/// inserted into the command string. If this is missing, it's placed at
/// the end, but before arguments.
pub const PLS_MARKER: &str = "$playlist";

pub struct PlayPlugin;

impl Default for PlayPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayPlugin {
    pub fn new() -> PlayPlugin {
        PlayPlugin
    }
}

impl crate::plugins::Plugin for PlayPlugin {
    fn name(&self) -> &'static str {
        "play"
    }

    /// beets' `before_choose_candidate` listener: the importer gets a
    /// "plaY" choice.
    fn prompt_choices(&self, _task: &crate::plugins::ChoiceTask) -> Vec<crate::plugins::PromptChoice> {
        vec![crate::plugins::PromptChoice {
            short: 'y',
            long: "plaY",
        }]
    }

    fn run_prompt_choice(
        &self,
        short: char,
        task: &mut crate::plugins::ChoiceTask,
    ) -> Result<bool> {
        if short != 'y' {
            return Ok(false);
        }
        let paths: Vec<PathBuf> = task.items.iter().map(|i| i.path().to_path_buf()).collect();
        let count = task.items.len();
        let opts = CmdOpts::default();
        let planned = build_plan(
            task.cfg,
            &opts,
            count,
            paths,
            "track",
            &mut std::io::stdout(),
            &mut || None,
        )?;
        if let Some(plan) = planned {
            // `play(..., keep_open=True)`: a subprocess, not an exec.
            run(&plan, true)?;
        }
        // The callback returns None: beets shows the prompt again.
        Ok(false)
    }
}

/// `beet play`'s options.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    pub album: bool,
    /// `-A/--args`.
    pub args: Option<String>,
    pub randomize: bool,
    /// `-y/--yes`: skip the warning threshold.
    pub yes: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// beets' `util.open_anything`: the platform's file dispatcher.
pub fn open_anything() -> String {
    if cfg!(target_os = "macos") {
        "open".into()
    } else if cfg!(windows) {
        "cmd /c start \"\"".into()
    } else {
        "xdg-open".into()
    }
}

/// What would be played: the command and the arguments to give it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub command: String,
    pub open_args: Vec<PathBuf>,
    /// "track" or "album", for the "Playing N ..." line.
    pub item_type: &'static str,
    pub count: usize,
}

/// Which paths the selection gives, in beets' order.
fn paths_for(
    cfg: &Config,
    lib: &Library,
    album: bool,
    selection: &[Record],
) -> Result<Vec<PathBuf>> {
    let use_folders = cfg.bool("play.use_folders");
    let mut paths = vec![];
    if album {
        let sort = crate::query::sort_from_strings(
            Kind::Item,
            &crate::query::context().sort_album,
            true,
        );
        for a in selection {
            if use_folders {
                let items = lib.album_items(a.id.unwrap_or(0))?;
                if let Some(first) = items.first() {
                    paths.push(first.path().parent().unwrap_or(Path::new("")).to_path_buf());
                }
            } else {
                let q = Query {
                    kind: Kind::Item,
                    pred: Query::match_field(
                        Kind::Item,
                        "album_id",
                        &a.id.unwrap_or(0).to_string(),
                    )?
                    .pred,
                    sort: sort.clone(),
                };
                paths.extend(lib.items(&q)?.into_iter().map(|i| i.path().to_path_buf()));
            }
        }
    } else {
        paths.extend(selection.iter().map(|i| i.path().to_path_buf()));
    }
    Ok(paths)
}

/// beets' `_make_relative`: each path relative to `start`; one with no
/// relative form (another drive on Windows) stays absolute, with a
/// warning.
fn make_relative(
    paths: Vec<PathBuf>,
    start: &Path,
    log: &PluginLog,
) -> Vec<PathBuf> {
    let mut absolute: Vec<PathBuf> = vec![];
    let mut relative = vec![];
    for path in paths {
        match pathdiff(&path, start) {
            Some(r) => relative.push(r),
            None => {
                absolute.push(path.clone());
                relative.push(path);
            }
        }
    }
    if let Some(first) = absolute.first() {
        log.warning(&format!(
            "{} path(s) have no relative form from relative_to '{}' and stay absolute, \
             starting with '{}'",
            absolute.len(),
            start.to_string_lossy(),
            first.to_string_lossy()
        ));
    }
    relative
}

/// `os.path.relpath(path, start)`: None when there is none (a relative
/// and an absolute path).
fn pathdiff(path: &Path, start: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if path.is_absolute() != start.is_absolute() {
        return None;
    }
    let a: Vec<Component> = path.components().collect();
    let b: Vec<Component> = start.components().collect();
    let mut i = 0;
    while i < a.len() && i < b.len() && a[i] == b[i] {
        i += 1;
    }
    let mut out = PathBuf::new();
    for _ in i..b.len() {
        out.push("..");
    }
    for c in &a[i..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

/// beets' `_play_command` up to running the player: the paths, the
/// playlist and the command string. `ask` answers the warning threshold
/// prompt (beets' `ui.input_options(("Continue", "Abort"))`).
pub fn plan(
    cfg: &Config,
    lib: &Library,
    opts: &CmdOpts,
    out: &mut impl Write,
    ask: &mut dyn FnMut() -> Option<char>,
) -> Result<Option<Plan>> {
    let kind = if opts.album { Kind::Album } else { Kind::Item };
    let query = Query::parse(kind, &opts.query)?;
    let selection: Vec<Record> = if opts.album {
        let mut a = lib.albums(&query)?;
        a.truncate(opts.limit.unwrap_or(usize::MAX));
        a
    } else {
        let mut i = lib.items(&query)?;
        i.truncate(opts.limit.unwrap_or(usize::MAX));
        i
    };
    let item_type = if opts.album { "album" } else { "track" };
    let paths = paths_for(cfg, lib, opts.album, &selection)?;
    build_plan(cfg, opts, selection.len(), paths, item_type, out, ask)
}

/// The rest of beets' `_play_command`, once the selection is known.
fn build_plan(
    cfg: &Config,
    opts: &CmdOpts,
    count: usize,
    mut paths: Vec<PathBuf>,
    item_type: &'static str,
    out: &mut impl Write,
    ask: &mut dyn FnMut() -> Option<char>,
) -> Result<Option<Plan>> {
    let log = PluginLog::new("play");

    let relative_to = cfg.str("play.relative_to");
    if !relative_to.is_empty() {
        let start = crate::query::normpath(Path::new(&relative_to));
        paths = make_relative(paths, &start, &log);
    }

    if count == 0 {
        // beets prints the warning in the "text_warning" colour.
        let warning = crate::ui::colorize("text_warning", &format!("No {item_type} to play."));
        writeln!(out, "{warning}")?;
        return Ok(None);
    }

    if opts.randomize {
        crate::plugins::random::Rng::new().shuffle(&mut paths);
    }

    let raw = cfg.bool("play.raw");
    let mut open_args = if raw {
        paths.clone()
    } else {
        vec![crate::plugins::play::create_tmp_playlist(cfg, &paths)?]
    };
    let command_str_cfg = cfg.str("play.command");
    let mut command = command_str(&command_str_cfg, opts.args.as_deref());

    if command.contains(PLS_MARKER) {
        if !raw {
            let pls: Vec<String> = open_args
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            command = command.replace(PLS_MARKER, &pls.concat());
            log.debug(&format!("command altered by PLS_MARKER to: {command}"));
            open_args = vec![];
        } else {
            let one = command.replacen(PLS_MARKER, " ", 1);
            command = if one.contains(PLS_MARKER) {
                one.replace(PLS_MARKER, " ")
            } else {
                one
            };
        }
    }

    let warning_threshold = cfg
        .get("play.warning_threshold")
        .and_then(serde_yaml::Value::as_f64)
        .unwrap_or(0.0);
    if !opts.yes && warning_threshold != 0.0 && count as f64 > warning_threshold {
        let mut plural = item_type.to_string();
        if count > 1 {
            plural.push('s');
        }
        let warning = crate::ui::colorize(
            "text_warning",
            &format!("You are about to queue {count} {plural}."),
        );
        writeln!(out, "{warning}")?;
        // "Continue" is the upper-case-letter choice: 'c'; "Abort" is 'a'.
        if ask() == Some('a') {
            return Ok(None);
        }
    }

    Ok(Some(Plan {
        command,
        open_args,
        item_type,
        count,
    }))
}

/// beets' `_command_str`: the configured command (or the platform's
/// opener), with `$args` substituted or appended.
pub fn command_str(configured: &str, args: Option<&str>) -> String {
    if configured.is_empty() {
        return open_anything();
    }
    if let Some(args) = args {
        if configured.contains(ARGS_MARKER) {
            return configured.replace(ARGS_MARKER, args);
        }
        return format!("{configured} {args}");
    }
    // Don't include the marker in the command.
    configured.replace(&format!(" {ARGS_MARKER}"), "")
}

/// beets' `_create_tmp_playlist`: a temporary `.m3u` with one path per
/// line.
pub fn create_tmp_playlist(cfg: &Config, paths: &[PathBuf]) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("beets").join("beetsplug_play");
    std::fs::create_dir_all(&dir)?;
    let name = unique_name();
    let file = dir.join(name);
    let mut out = std::fs::File::create(&file)?;
    if cfg.bool("play.bom") {
        out.write_all(b"\xef\xbb\xbf")?;
    }
    for p in paths {
        use std::os::unix::ffi::OsStrExt;
        out.write_all(p.as_os_str().as_bytes())?;
        out.write_all(b"\n")?;
    }
    Ok(file)
}

/// `tempfile.mkstemp`'s unguessable name (in the same directory).
fn unique_name() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}{:08x}.m3u", std::process::id(), nanos ^ n as u32)
}

/// beets' `play`'s "Playing N ..." line.
fn print_playing(plan: &Plan, out: &mut impl Write) -> Result<()> {
    let plural = if plan.count > 1 { "s" } else { "" };
    writeln!(out, "Playing {} {}{plural}.", plan.count, plan.item_type)?;
    Ok(())
}

/// beets' `play`: print what plays, then hand the paths to the player.
pub fn play(plan: &Plan, out: &mut impl Write) -> Result<()> {
    print_playing(plan, out)?;
    run(plan, false)
}

/// Run the player: `keep_open` waits for it (the importer's "plaY");
/// otherwise beets `exec`s it, and so do we.
fn run(plan: &Plan, keep_open: bool) -> Result<()> {
    let words = crate::plugins::duplicates::shlex_split(&plan.command)?;
    let (program, rest) = words.split_first().expect("a command has a program");
    let mut cmd = std::process::Command::new(program);
    cmd.args(rest).args(&plan.open_args);
    let play_error = |e: std::io::Error| anyhow::anyhow!("Could not play the query: {e}");
    if keep_open {
        return match cmd.status() {
            Ok(_) => Ok(()),
            Err(e) => Err(play_error(e)),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(play_error(cmd.exec()))
    }
    #[cfg(not(unix))]
    {
        match cmd.status() {
            Ok(_) => Ok(()),
            Err(e) => Err(play_error(e)),
        }
    }
}

/// `beet play`: send the matching items or albums to the player.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let mut stdout = std::io::stdout();
    // beets' `ui.input_options(("Continue", "Abort"))`: not required, the
    // first option is the default.
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
    let planned = plan(cfg, lib, opts, &mut stdout, &mut ask)?;
    if let Some(plan) = planned {
        play(&plan, &mut std::io::stdout().lock())?;
    }
    Ok(())
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "play",
        order: 87,
        defaults: r#"play:
    command:
    use_folders: no
    relative_to:
    raw: no
    warning_threshold: 100
    bom: no
"#,
        config_keys: &["play"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "play",
                build: build_play,
                run: run_play,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(PlayPlugin::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct PlayArgs {
    /// Query albums instead of tracks
    #[arg(short = 'a', long)]
    album: bool,
    /// Add additional arguments to the command
    #[arg(short = 'A', long, value_name = "ARGS")]
    args: Option<String>,
    /// Randomize the order of playlist entries
    #[arg(short = 'R', long)]
    randomize: bool,
    /// Skip the warning threshold
    #[arg(short = 'y', long)]
    yes: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_play() -> clap::Command {
    use clap::Args as _;
    PlayArgs::augment_args(clap::Command::new("play").about(
        "send music to a player as a playlist",
    ))
}

pub fn run_play(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = PlayArgs::from_arg_matches(m)?;
    crate::plugins::play::command(
        cfg,
        lib,
        &crate::plugins::play::CmdOpts {
            album: a.album,
            args: a.args,
            randomize: a.randomize,
            yes: a.yes,
            limit: a.limit,
            query: a.query,
        },
    )
}

#[cfg(test)]
#[path = "play_tests.rs"]
mod tests;
