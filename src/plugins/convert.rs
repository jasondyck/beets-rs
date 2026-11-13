//! Port of beets 2.14's `convert` plugin (`beetsplug/convert.py`):
//! transcodes items into another directory (`beet convert`), or on import
//! (`auto`, replacing the imported file; `auto_keep`, a converted copy in
//! `dest`).
//!
//! The command template of the target format (`formats`, or the top-level
//! `command`/`extension` shortcuts) is split as `shlex.split` does and
//! `$source`/`$dest` are substituted in each argument as Python's
//! `Template.safe_substitute`. Conversions run in a pool of `threads`
//! worker threads (beets' `pipeline`); everything that needs the library
//! (destinations, the `no_convert` query, storing `--keep-new` items) runs
//! on the main thread.

use crate::config::{Config, ConfigError};
use crate::fileops;
use crate::library::Library;
use crate::model::{Album, Item, Kind, Value};
use crate::paths::PathFormatter;
use crate::plugins::{DEBUG, INFO, ImportTask, Plugin, PluginCtx, PluginLog};
use crate::query::{Obj, Query, normpath};
use anyhow::{Result, anyhow, bail};
use serde_yaml::Value as Y;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Some convenient alternate names for formats.
const ALIASES: &[(&str, &str)] = &[("windows media", "wma"), ("vorbis", "ogg")];

const LOSSLESS_FORMATS: &[&str] = &["ape", "flac", "alac", "wave", "aiff"];

/// Python's `shlex.split` (POSIX mode, no comments).
pub fn shlex_split(s: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_token = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            '\\' => {
                let Some(n) = chars.next() else {
                    bail!("No escaped character");
                };
                cur.push(n);
                in_token = true;
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(n) => cur.push(n),
                        None => bail!("No closing quotation"),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(n @ ('"' | '\\')) => cur.push(n),
                            Some(n) => {
                                cur.push('\\');
                                cur.push(n);
                            }
                            None => bail!("No closing quotation"),
                        },
                        Some(n) => cur.push(n),
                        None => bail!("No closing quotation"),
                    }
                }
            }
            c => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        out.push(cur);
    }
    Ok(out)
}

/// Python's `string.Template(s).safe_substitute(vars)`.
pub fn safe_substitute(s: &str, vars: &[(&str, &str)]) -> String {
    let is_start = |c: u8| c == b'_' || c.is_ascii_alphabetic();
    let is_cont = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
    let lookup = |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] != b'$' {
            i += 1;
            continue;
        }
        out.push_str(&s[last..i]);
        let rest = &b[i + 1..];
        if rest.first() == Some(&b'$') {
            out.push('$');
            i += 2;
        } else if rest.first().is_some_and(|c| is_start(*c)) {
            let n = 1 + rest[1..].iter().take_while(|c| is_cont(**c)).count();
            let name = &s[i + 1..i + 1 + n];
            match lookup(name) {
                Some(v) => out.push_str(v),
                None => out.push_str(&s[i..i + 1 + n]),
            }
            i += 1 + n;
        } else if rest.first() == Some(&b'{')
            && rest.get(1).is_some_and(|c| is_start(*c))
            && let Some(close) = rest.iter().position(|c| *c == b'}')
            && rest[2..close].iter().all(|c| is_cont(*c))
        {
            let name = &s[i + 2..i + 1 + close];
            match lookup(name) {
                Some(v) => out.push_str(v),
                None => out.push_str(&s[i..i + 2 + close]),
            }
            i += 2 + close;
        } else {
            out.push('$');
            i += 1;
        }
        last = i;
    }
    out.push_str(&s[last..]);
    out
}

/// `replace_ext`: the path with its extension replaced by `ext`.
fn replace_ext(path: &Path, ext: &str) -> PathBuf {
    let s = path.to_string_lossy();
    let name_start = s.rfind('/').map(|i| i + 1).unwrap_or(0);
    let name = &s[name_start..];
    let lead = name.len() - name.trim_start_matches('.').len();
    let stem = match name[lead..].rfind('.') {
        Some(i) => &s[..name_start + lead + i],
        None => &s[..],
    };
    PathBuf::from(format!("{stem}.{ext}"))
}

/// `os.path.relpath(path, start)` for absolute, normalized paths.
fn relpath(path: &Path, start: &Path) -> PathBuf {
    let p: Vec<_> = path.components().collect();
    let s: Vec<_> = start.components().collect();
    let common = p.iter().zip(&s).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..s.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// The command template and extension of the target format.
#[derive(Clone, Debug)]
pub struct FormatCommand {
    pub command: String,
    pub ext: String,
}

/// Why the format has no command (raised when it's needed, as beets'
/// cached property).
#[derive(Clone, Debug)]
enum CommandError {
    Config(String),
    User(String),
}

fn y_str(y: &Y) -> Option<String> {
    match y {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(if *b { "True" } else { "False" }.into()),
        _ => None,
    }
}

/// The `command` property: the format's command and extension.
fn format_command(cfg: &Config, fmt: &str) -> std::result::Result<FormatCommand, CommandError> {
    let fmt = ALIASES
        .iter()
        .find(|(a, _)| *a == fmt)
        .map(|(_, f)| *f)
        .unwrap_or(fmt);
    let key = format!("convert.formats.{fmt}");
    let (mut command, mut ext) = match cfg.get(&key) {
        None | Some(Y::Null) => return Err(CommandError::Config(format!("{key} not found"))),
        Some(Y::Mapping(m)) => {
            let Some(c) = m.get("command").and_then(y_str) else {
                return Err(CommandError::User(format!(
                    "convert: format {fmt} needs the \"command\" field"
                )));
            };
            let e = m
                .get("extension")
                .and_then(y_str)
                .unwrap_or_else(|| fmt.to_string());
            (c, e)
        }
        Some(y) => match y_str(y) {
            Some(c) => (c, fmt.to_string()),
            None => return Err(CommandError::Config(format!("{key}: must be a string"))),
        },
    };
    // Convenience and backwards-compatibility shortcuts.
    if let Some(c) = cfg.get("convert.command").and_then(y_str) {
        command = c;
    } else if let Some(o) = cfg.get("convert.opts").and_then(y_str) {
        command = format!("ffmpeg -i $source -y {o} $dest");
    }
    if let Some(e) = cfg.get("convert.extension").and_then(y_str) {
        ext = e;
    }
    Ok(FormatCommand { command, ext })
}

fn opt_int(cfg: &Config, key: &str) -> Option<i64> {
    match cfg.get(key) {
        Some(Y::Number(n)) => n.as_i64().or(n.as_f64().map(|f| f as i64)),
        Some(Y::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

fn opt_str(cfg: &Config, key: &str) -> Option<String> {
    cfg.get(key).and_then(y_str)
}

/// The command-line options (`self.config.set(vars(opts))`): each one
/// given overrides its setting.
#[derive(Clone, Debug, Default)]
pub struct CmdOpts {
    pub pretend: bool,
    pub threads: Option<usize>,
    pub refresh: bool,
    pub keep_new: bool,
    pub dest: Option<String>,
    pub format: Option<String>,
    pub yes: bool,
    pub link: bool,
    pub hardlink: bool,
    pub playlist: Option<String>,
    pub force: bool,
    pub album: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// The plugin's settings, resolved once (beets' cached properties).
#[derive(Clone, Debug)]
pub struct Settings {
    pub dest: Option<String>,
    pub pretend: bool,
    pub link: bool,
    pub hardlink: bool,
    pub threads: usize,
    pub fmt: String,
    pub id3v23: bool,
    pub write_metadata: bool,
    pub max_bitrate: Option<i64>,
    pub tmpdir: Option<String>,
    pub quiet: bool,
    pub embed: bool,
    pub no_convert: String,
    pub never_convert_lossy_files: bool,
    pub copy_album_art: bool,
    pub album_art_maxwidth: u32,
    pub delete_originals: bool,
    pub playlist: Option<String>,
    pub force: bool,
    pub refresh: bool,
    pub keep_new: bool,
    command: std::result::Result<FormatCommand, CommandError>,
}

impl Settings {
    pub fn new(cfg: &Config) -> Result<Settings> {
        Self::with_opts(cfg, None)
    }

    pub fn with_opts(cfg: &Config, opts: Option<&CmdOpts>) -> Result<Settings> {
        let mut cfg = cfg.clone();
        if let Some(o) = opts {
            // Flags only turn settings on; values replace them.
            let mut set = |k: &str, v: Y| cfg.set(&format!("convert.{k}"), v);
            if o.pretend {
                set("pretend", Y::Bool(true));
            }
            if let Some(t) = o.threads {
                set("threads", Y::Number((t as u64).into()));
            }
            if o.refresh {
                set("refresh", Y::Bool(true));
            }
            if o.keep_new {
                set("keep_new", Y::Bool(true));
            }
            if let Some(d) = &o.dest {
                set("dest", Y::String(d.clone()));
            }
            if let Some(f) = &o.format {
                set("format", Y::String(f.clone()));
            }
            if o.link {
                set("link", Y::Bool(true));
            }
            if o.hardlink {
                set("hardlink", Y::Bool(true));
            }
            if let Some(p) = &o.playlist {
                set("playlist", Y::String(p.clone()));
            }
            if o.force {
                set("force", Y::Bool(true));
            }
        }
        let cfg = &cfg;
        let b = |k: &str| cfg.bool(&format!("convert.{k}"));
        let fmt = opt_str(cfg, "convert.format")
            .unwrap_or_else(|| "mp3".into())
            .to_lowercase();
        let id3v23 = match cfg.get("convert.id3v23") {
            Some(Y::Bool(v)) => *v,
            Some(Y::String(s)) if s == "inherit" => crate::media::id3v23(),
            None | Some(Y::Null) => crate::media::id3v23(),
            Some(_) => {
                return Err(ConfigError(
                    "convert.id3v23 must be one of [True, False, 'inherit'], not ...".into(),
                )
                .into());
            }
        };
        let hardlink = b("hardlink");
        let threads = match opt_int(cfg, "convert.threads") {
            Some(t) => t.max(0) as usize,
            None => crate::plugins::replaygain::cpu_count(),
        };
        let command = format_command(cfg, &fmt);
        Ok(Settings {
            dest: opt_str(cfg, "convert.dest").filter(|d| !d.is_empty()),
            pretend: b("pretend"),
            link: !hardlink && b("link"),
            hardlink,
            threads,
            fmt,
            id3v23,
            write_metadata: b("write_metadata"),
            max_bitrate: opt_int(cfg, "convert.max_bitrate"),
            tmpdir: opt_str(cfg, "convert.tmpdir").filter(|d| !d.is_empty()),
            quiet: b("quiet"),
            embed: b("embed"),
            no_convert: opt_str(cfg, "convert.no_convert").unwrap_or_default(),
            never_convert_lossy_files: b("never_convert_lossy_files"),
            copy_album_art: b("copy_album_art"),
            album_art_maxwidth: opt_int(cfg, "convert.album_art_maxwidth")
                .unwrap_or(0)
                .max(0) as u32,
            delete_originals: b("delete_originals"),
            playlist: opt_str(cfg, "convert.playlist"),
            force: b("force"),
            refresh: b("refresh"),
            keep_new: b("keep_new"),
            command,
        })
    }

    /// The `command` property (an error when the format has none).
    pub fn command(&self) -> Result<&FormatCommand> {
        match &self.command {
            Ok(c) => Ok(c),
            Err(CommandError::Config(m)) => Err(ConfigError(m.clone()).into()),
            Err(CommandError::User(m)) => Err(anyhow!("{m}")),
        }
    }

    /// The `dest` property.
    pub fn dest(&self) -> Result<PathBuf> {
        match &self.dest {
            Some(d) => Ok(normpath(Path::new(d))),
            None => bail!("no convert destination set"),
        }
    }
}

/// beets' `parse_query_string` for the `no_convert` setting.
fn parse_no_convert(s: &str) -> Result<Option<Query>> {
    if s.is_empty() {
        return Ok(None);
    }
    let parts = shlex_split(s)?;
    Ok(Some(Query::parse(Kind::Item, &parts)?))
}

/// What decides whether an item is transcoded (`should_transcode`).
pub struct Selector<'a> {
    s: &'a Settings,
    lib: &'a Library,
    no_convert: Option<Query>,
}

impl<'a> Selector<'a> {
    pub fn new(s: &'a Settings, lib: &'a Library) -> Result<Self> {
        Ok(Selector {
            s,
            lib,
            no_convert: parse_no_convert(&s.no_convert)?,
        })
    }

    fn album(&self, item: &Item) -> Option<Album> {
        match item.get("album_id") {
            Some(Value::Int(id)) => self.lib.get_album(id).ok().flatten(),
            _ => None,
        }
    }

    /// `in_no_convert`.
    pub fn in_no_convert(&self, item: &Item) -> bool {
        let Some(q) = &self.no_convert else {
            return false;
        };
        let album = self.album(item);
        q.matches_obj(&Obj {
            rec: item,
            album: album.as_ref(),
            album_dir: None,
            album_counts: None,
            lib: None,
        })
    }

    /// `should_transcode`: the item's bitrate is too high or its format
    /// isn't the target; `force` ignores `no_convert`,
    /// `never_convert_lossy_files` and `max_bitrate`.
    pub fn should_transcode(&self, item: &Item) -> bool {
        if self.s.force {
            return true;
        }
        let format = item.str("format").to_lowercase();
        if self.in_no_convert(item)
            || (self.s.never_convert_lossy_files && !LOSSLESS_FORMATS.contains(&format.as_str()))
        {
            return false;
        }
        if let Some(maxbr) = self.s.max_bitrate
            && item.int("bitrate") >= 1000 * maxbr
        {
            return true;
        }
        self.s.fmt != format
    }
}

/// One item's conversion, prepared on the main thread.
struct Job {
    item: Item,
    dest: PathBuf,
    transcode: bool,
    /// The album's art file (`album.art_filepath`).
    art: Option<PathBuf>,
}

/// The converter run by the worker threads (no library access).
struct Worker<'a> {
    s: &'a Settings,
    base_level: u8,
}

impl Worker<'_> {
    fn log(&self) -> PluginLog {
        let log = PluginLog::new("convert");
        log.set_base_level(self.base_level);
        log
    }

    /// `encode`: run the command on `source`, writing `dest`. `Ok(false)`
    /// when the command failed (beets' `CalledProcessError`); an error when
    /// it couldn't be run.
    fn encode(&self, log: &PluginLog, command: &str, source: &Path, dest: &Path) -> Result<bool> {
        encode(self.s, log, command, source, dest)
    }

    /// `convert_item`. Returns the item to store (`--keep-new`).
    fn convert_item(&self, keep_new: bool, job: Job) -> Result<Option<Item>> {
        let log = self.log();
        let s = self.s;
        let Job {
            mut item,
            mut dest,
            transcode,
            art,
        } = job;
        let FormatCommand { command, ext } = s.command()?.clone();
        // Ensure that desired item is readable before processing it.
        if let Err(e) = crate::mediafile::MediaFile::open(item.path()) {
            log.error(&format!("Could not open file to convert: {e:#}"));
            return Ok(None);
        }
        let item_path = item.path().to_path_buf();
        let (original, converted) = if keep_new {
            let converted = if transcode {
                replace_ext(&item_path, &ext)
            } else {
                item_path.clone()
            };
            (dest.clone(), converted)
        } else {
            if transcode {
                dest = replace_ext(&dest, &ext);
            }
            (item_path.clone(), dest.clone())
        };

        if dest.exists() {
            let dest_newer = || {
                let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                m(&item_path) <= m(&dest)
            };
            if !s.refresh || keep_new || dest_newer() {
                log.info(&format!(
                    "Skipping {} (destination file exists)",
                    item_path.display()
                ));
                if keep_new && s.refresh {
                    log.debug("Skipping refresh: not supported with keep_new");
                }
                return Ok(None);
            }
            if s.pretend {
                log.info(&format!(
                    "Pretend to remove {} (original file modified)",
                    dest.display()
                ));
            } else {
                log.info(&format!(
                    "Removing {} (original file modified)",
                    dest.display()
                ));
                std::fs::remove_file(&dest)?;
            }
        }

        if !s.pretend
            && let Some(parent) = dest.parent()
        {
            std::fs::create_dir_all(parent)?;
        }

        if keep_new {
            if s.pretend {
                log.info(&format!(
                    "Pretend to move {} to {}",
                    item_path.display(),
                    original.display()
                ));
            } else {
                log.info(&format!("Moving to {}", original.display()));
                fileops::move_file(&item_path, &original)?;
            }
        }

        let linked;
        if transcode {
            linked = false;
            if !self.encode(&log, &command, &original, &converted)? {
                return Ok(None);
            }
        } else {
            linked = s.link || s.hardlink;
            if s.pretend {
                let msg = if s.hardlink {
                    "ln"
                } else if s.link {
                    "ln -s"
                } else {
                    "cp"
                };
                log.info(&format!(
                    "{msg} {} {}",
                    original.display(),
                    converted.display()
                ));
            } else {
                let msg = if s.hardlink {
                    "Hardlinking"
                } else if s.link {
                    "Linking"
                } else {
                    "Copying"
                };
                log.info(&format!("{msg} {}", item_path.display()));
                if s.hardlink {
                    fileops::hardlink_file(&original, &converted)?;
                } else if s.link {
                    fileops::link_file(&original, &converted)?;
                } else {
                    fileops::copy_file(&original, &converted)?;
                }
            }
        }

        if s.pretend {
            return Ok(None);
        }

        // Write tags from the database to the file if requested.
        if s.write_metadata
            && let Err(e) = crate::media::write_with(&item, &converted, s.id3v23)
        {
            crate::logging::emit(format!("error writing {}: {e:#}", converted.display()));
        }

        let mut store = None;
        if keep_new {
            // Read the transcoded file again to get its new bitrate,
            // duration, etc.
            item.set_path(&converted);
            match crate::media::read(&converted) {
                Ok(fresh) => {
                    for f in crate::media::media_fields().chain(["mtime"]) {
                        if let Some(v) = fresh.get(f) {
                            item.set(f, v);
                        }
                    }
                }
                Err(e) => log.error(&format!("{e:#}")),
            }
            store = Some(item.clone());
        }

        if s.embed
            && !linked
            && let Some(art) = art
        {
            let maxwidth = get_art_resize(s, &log, &art);
            log.debug(&format!("embedding album art from {}", art.display()));
            embed_item(&log, &item, &art, maxwidth, &converted, s.id3v23);
        }
        Ok(store)
    }
}

/// `encode`, shared by the command and `auto`.
fn encode(
    s: &Settings,
    log: &PluginLog,
    command: &str,
    source: &Path,
    dest: &Path,
) -> Result<bool> {
    let (pretend, quiet) = (s.pretend, s.quiet);
    if !quiet && !pretend {
        log.info(&format!("Encoding {}", source.display()));
    }
    let source_s = source.to_string_lossy();
    let dest_s = dest.to_string_lossy();
    let args: Vec<String> = shlex_split(command)?
        .iter()
        .map(|a| safe_substitute(a, &[("source", &source_s), ("dest", &dest_s)]))
        .collect();
    if pretend {
        log.info(&args.join(" "));
        return Ok(true);
    }
    let Some((prog, rest)) = args.split_first() else {
        bail!("convert: couldn't invoke '': empty command");
    };
    let out = std::process::Command::new(prog)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            log.info(&format!(
                "Encoding {} failed. Cleaning up...",
                source.display()
            ));
            let mut output = o.stdout;
            output.extend(o.stderr);
            log.debug(&format!(
                "Command [{}] exited with status {}: {}",
                args.iter()
                    .map(|a| crate::template::py_repr(a))
                    .collect::<Vec<_>>()
                    .join(", "),
                o.status.code().unwrap_or(-1),
                py_bytes_repr(&output)
            ));
            let _ = std::fs::remove_file(dest);
            if let Some(dir) = dest.parent() {
                prune_dir(dir);
            }
            return Ok(false);
        }
        Err(e) => {
            bail!(
                "convert: couldn't invoke {}: {}",
                crate::template::py_repr(&args.join(" ")),
                crate::plugins::art::py_os_error(&e, Path::new(prog))
            );
        }
    }
    if !quiet && !pretend {
        log.info(&format!("Finished encoding {}", source.display()));
    }
    Ok(true)
}

/// Python's `repr()` of bytes (printable ASCII kept).
fn py_bytes_repr(b: &[u8]) -> String {
    let mut out = String::from("b'");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(c as char),
            _ => write!(out, "\\x{c:02x}").unwrap(),
        }
    }
    out.push('\'');
    out
}

/// `util.prune_dirs(dir)` without a root: remove `dir` if it holds
/// nothing but clutter (`.DS_Store`, `Thumbs.db`).
fn prune_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let only_clutter = entries
        .flatten()
        .all(|e| matches!(e.file_name().to_str(), Some(".DS_Store" | "Thumbs.db")));
    if only_clutter {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// `_get_art_resize`: the width to resize the album art to, if it's
/// wider than `album_art_maxwidth`.
fn get_art_resize(s: &Settings, log: &PluginLog, artpath: &Path) -> Option<u32> {
    if s.album_art_maxwidth == 0 {
        return None;
    }
    let maxwidth = s.album_art_maxwidth;
    let size = crate::artresizer::get_size(artpath);
    log.debug(&format!(
        "image size: {}",
        match size {
            Some((w, h)) => format!("({w}, {h})"),
            None => "None".into(),
        }
    ));
    match size {
        Some((w, _)) => (w > maxwidth).then_some(maxwidth),
        None => {
            log.warning("Could not get size of image (please see documentation for dependencies).");
            None
        }
    }
}

/// `art.embed_item(log, item, path, maxwidth, itempath=converted,
/// id3v23=...)`: the item's tags and the image into `itempath`.
fn embed_item(
    log: &PluginLog,
    item: &Item,
    imagepath: &Path,
    maxwidth: Option<u32>,
    itempath: &Path,
    id3v23: bool,
) {
    let mut path = imagepath.to_path_buf();
    let mut resized = None;
    if let Some(w) = maxwidth {
        path = crate::plugins::art::resize_image(log, imagepath, w, 0);
        if path != imagepath {
            resized = Some(path.clone());
        }
    }
    log.debug(&format!("embedding {}", path.display()));
    let data = std::fs::read(&path);
    if let Some(r) = resized {
        let _ = std::fs::remove_file(r);
    }
    let data = match data {
        Ok(d) => d,
        Err(e) => {
            log.warning(&format!(
                "could not read image file: {}",
                crate::plugins::art::py_os_error(&e, &path)
            ));
            return;
        }
    };
    let mime = crate::plugins::art::image_mime_type(&data);
    if !matches!(mime, Some("image/jpeg" | "image/png")) {
        log.info(&format!(
            "not embedding image of unsupported type: {}",
            mime.unwrap_or("None")
        ));
        return;
    }
    if let Err(e) = crate::media::write_with_images_at(item, itempath, id3v23, Some(&data)) {
        crate::logging::emit(format!("error writing {}: {e:#}", itempath.display()));
    }
}

/// The main-thread side of a conversion run: destinations and the pool.
pub struct Converter<'a> {
    cfg: &'a Config,
    lib: &'a Library,
    s: &'a Settings,
    sel: Selector<'a>,
    log: &'a PluginLog,
    formatter: std::cell::OnceCell<PathFormatter>,
}

impl<'a> Converter<'a> {
    pub fn new(
        cfg: &'a Config,
        lib: &'a Library,
        s: &'a Settings,
        log: &'a PluginLog,
    ) -> Result<Self> {
        Ok(Converter {
            cfg,
            lib,
            s,
            sel: Selector::new(s, lib)?,
            log,
            formatter: std::cell::OnceCell::new(),
        })
    }

    fn formatter(&self) -> Result<&PathFormatter> {
        if let Some(f) = self.formatter.get() {
            return Ok(f);
        }
        let dest = self.s.dest()?;
        let f = PathFormatter::with_formats(self.cfg, self.cfg.mapping("convert.paths"), dest)?;
        Ok(self.formatter.get_or_init(|| f))
    }

    /// `get_item_destination`.
    pub fn item_destination(&self, item: &Item) -> Result<PathBuf> {
        let ext = if self.sel.should_transcode(item) && !self.s.keep_new {
            Some(self.s.command()?.ext.clone())
        } else {
            None
        };
        let f = self.formatter()?;
        Ok(f.destination_ext(item, None, Some(self.lib), ext.as_deref()))
    }

    /// `_parallel_convert`: `convert_item` for every item, in `threads`
    /// threads.
    pub fn parallel_convert(&self, items: Vec<Item>, keep_new: bool) -> Result<()> {
        let threads = self.s.threads;
        if threads == 0 || items.is_empty() {
            return Ok(());
        }
        // beets resolves these on the first item, in this order.
        self.s.command()?;
        let mut jobs = std::collections::VecDeque::new();
        for item in items {
            let dest = self.item_destination(&item)?;
            let transcode = self.sel.should_transcode(&item);
            let art = self
                .sel
                .album(&item)
                .map(|a| a.path_field("artpath").to_path_buf())
                .filter(|p| !p.as_os_str().is_empty());
            jobs.push_back(Job {
                item,
                dest,
                transcode,
                art,
            });
        }
        let queue = Mutex::new(jobs);
        let abort = AtomicBool::new(false);
        let worker = Worker {
            s: self.s,
            base_level: self.log.base_level(),
        };
        let mut first_err = None;
        std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel();
            for _ in 0..threads {
                let tx = tx.clone();
                let (queue, abort, worker) = (&queue, &abort, &worker);
                scope.spawn(move || {
                    while !abort.load(Ordering::Relaxed) {
                        let Some(job) = queue.lock().unwrap().pop_front() else {
                            break;
                        };
                        let r = worker.convert_item(keep_new, job);
                        if r.is_err() {
                            abort.store(true, Ordering::Relaxed);
                        }
                        if tx.send(r).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            for r in rx {
                match r {
                    Ok(Some(item)) => {
                        if let Err(e) = self.lib.store(&item) {
                            self.log.error(&format!("{e:#}"));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                }
            }
        });
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// `copy_album_art`: copy (or resize) the album's art next to its
    /// first converted item.
    pub fn copy_album_art(&self, album: &Album) -> Result<()> {
        let artpath = album.path_field("artpath").to_path_buf();
        if artpath.as_os_str().is_empty() {
            return Ok(());
        }
        let log = self.log;
        if !artpath.is_file() {
            log.info(&format!(
                "Skipping {} (source file not found)",
                artpath.display()
            ));
            return Ok(());
        }
        let Some(album_item) = self
            .lib
            .album_items(album.id.unwrap_or_default())?
            .into_iter()
            .next()
        else {
            return Ok(());
        };
        let item_dest = self.item_destination(&album_item)?;
        let item_dir = item_dest.parent().unwrap_or(Path::new("/"));
        let dest =
            crate::paths::art_destination(self.cfg, album, &artpath, item_dir, Some(self.lib))?;
        if artpath == dest {
            return Ok(());
        }
        let pretend = self.s.pretend;
        if !pretend && let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if dest.exists() {
            log.info(&format!(
                "Skipping {} (destination file exists)",
                artpath.display()
            ));
            return Ok(());
        }
        if let Some(maxwidth) = get_art_resize(self.s, log, &artpath) {
            log.info(&format!(
                "Resizing cover art from {} to {}",
                artpath.display(),
                dest.display()
            ));
            if !pretend
                && let Err(e) = crate::artresizer::resize_to(maxwidth, &artpath, &dest, 0, 0)
            {
                crate::logging::emit(format!(
                    "artresizer: convert failed for {}",
                    artpath.display()
                ));
                crate::logging::debug!("artresizer: {e}");
            }
        } else {
            let (link, hardlink) = (self.s.link, self.s.hardlink);
            if pretend {
                let msg = if hardlink {
                    "ln"
                } else if link {
                    "ln -s"
                } else {
                    "cp"
                };
                log.info(&format!("{msg} {} {}", artpath.display(), dest.display()));
            } else {
                let msg = if hardlink {
                    "Hardlinking"
                } else if link {
                    "Linking"
                } else {
                    "Copying"
                };
                log.info(&format!(
                    "{msg} cover art from {} to {}",
                    artpath.display(),
                    dest.display()
                ));
                if hardlink {
                    fileops::hardlink_file(&artpath, &dest)?;
                } else if link {
                    fileops::link_file(&artpath, &dest)?;
                } else {
                    fileops::copy_file(&artpath, &dest)?;
                }
            }
        }
        Ok(())
    }
}

/// `beet convert`.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let s = Settings::with_opts(cfg, Some(&opts))?;
    let log = PluginLog::new("convert");
    log.set_base_level(INFO);
    let pretend = s.pretend;
    let limit = opts.limit.unwrap_or(usize::MAX);
    let mut albums = vec![];
    let items: Vec<Item> = if opts.album {
        albums = lib.albums(&Query::parse(Kind::Album, &opts.query)?)?;
        albums.truncate(limit);
        let mut items = vec![];
        for a in &albums {
            items.extend(lib.album_items(a.id.unwrap_or_default())?);
        }
        if !pretend {
            let t = crate::template::Template::parse(&cfg.str("format_album"));
            for a in &albums {
                println!("{}", crate::commands::format_record(&t, a, Some(lib)));
            }
        }
        items
    } else {
        let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
        items.truncate(limit);
        if !pretend {
            let t = crate::template::Template::parse(&cfg.str("format_item"));
            for i in &items {
                println!("{}", crate::commands::format_record(&t, i, Some(lib)));
            }
        }
        items
    };
    if items.is_empty() {
        log.error("Empty query result.");
        return Ok(());
    }
    if !(pretend || opts.yes || crate::ui::input_yn("Convert? (Y/n)", false)) {
        return Ok(());
    }
    let conv = Converter::new(cfg, lib, &s, &log)?;

    if opts.album && s.copy_album_art {
        for album in &albums {
            conv.copy_album_art(album)?;
        }
    }

    // A playlist of the converted files, relative to its own directory.
    let mut playlist = None;
    if let Some(pl) = &s.playlist {
        let pl_path = normpath(&s.dest()?.join(pl));
        let pl_dir = pl_path.parent().unwrap_or(Path::new("/")).to_path_buf();
        let mut entries = vec![];
        for item in &items {
            let mut item_path = conv.item_destination(item)?;
            if !s.keep_new && conv.sel.should_transcode(item) {
                item_path = replace_ext(&item_path, &s.command()?.ext);
            }
            entries.push(relpath(&item_path, &pl_dir));
        }
        playlist = Some((pl_path, entries));
    }

    conv.parallel_convert(items, s.keep_new)?;

    if let Some((pl_path, entries)) = playlist {
        log.info(&format!("Creating playlist file {}", pl_path.display()));
        if !pretend {
            write_m3u(&pl_path, &entries)?;
        }
    }
    Ok(())
}

/// `M3UFile.write` with `#EXTM3U`.
fn write_m3u(path: &Path, entries: &[PathBuf]) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let mut out = b"#EXTM3U\n".to_vec();
    for e in entries {
        out.extend_from_slice(e.as_os_str().as_bytes());
        out.push(b'\n');
    }
    out.push(b'\n');
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)?;
    Ok(())
}

pub struct ConvertPlugin {
    /// Temporary transcoded files (`_temp_files`), deleted once imported.
    temp_files: Mutex<Vec<PathBuf>>,
}

impl ConvertPlugin {
    pub fn new() -> Self {
        ConvertPlugin {
            temp_files: Mutex::new(vec![]),
        }
    }

    /// `auto_convert`: `convert_on_import` for each item, transcoding a
    /// newly imported item into a temporary file and pointing the item at
    /// it. The encodes run in parallel (beets' `par_map`: a thread per
    /// CPU); the library is updated on this thread.
    fn auto_convert(
        &self,
        s: &Settings,
        sel: &Selector,
        log: &PluginLog,
        lib: &Library,
        items: &mut [Item],
    ) -> Result<()> {
        let todo: Vec<usize> = (0..items.len())
            .filter(|&i| sel.should_transcode(&items[i]))
            .collect();
        if todo.is_empty() {
            return Ok(());
        }
        let FormatCommand { command, ext } = s.command()?.clone();
        let tmpdir = match &s.tmpdir {
            Some(d) => PathBuf::from(d),
            None => std::env::temp_dir(),
        };
        let mut jobs = vec![];
        for &i in &todo {
            let dest = mkstemp(&tmpdir, &ext)?;
            // Delete the transcode later.
            self.temp_files.lock().unwrap().push(dest.clone());
            jobs.push((i, items[i].path().to_path_buf(), dest));
        }
        let base_level = log.base_level();
        let results = par_map(
            crate::plugins::replaygain::cpu_count(),
            &jobs,
            |(_, src, dest)| {
                let log = PluginLog::new("convert");
                log.set_base_level(base_level);
                encode(s, &log, &command, src, dest)
            },
        );
        for ((i, source, dest), r) in jobs.into_iter().zip(results) {
            if !r? {
                continue;
            }
            let item = &mut items[i];
            // Point the newly imported entry at the converted file.
            item.set_path(&dest);
            crate::media::write_with(item, &dest, crate::media::id3v23())?;
            let fresh = crate::media::read(&dest)?;
            for f in crate::media::media_fields().chain(["mtime"]) {
                if let Some(v) = fresh.get(f) {
                    item.set(f, v);
                }
            }
            lib.store(item)?;
            if s.delete_originals {
                log.log(
                    if s.quiet { DEBUG } else { INFO },
                    &format!("Removing original file {}", source.display()),
                );
                if let Err(e) = std::fs::remove_file(&source) {
                    log.error(&format!("{e}"));
                }
            }
        }
        Ok(())
    }
}

/// `tempfile.mkstemp(".EXT", dir=tmpdir)`: a new empty file.
fn mkstemp(tmpdir: &Path, ext: &str) -> Result<PathBuf> {
    loop {
        let name = crate::artresizer::py_temp_name();
        let name = name.file_name().unwrap_or_default().to_string_lossy();
        let p = tmpdir.join(format!("{name}.{ext}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(anyhow!(crate::plugins::art::py_os_error(&e, &p))),
        }
    }
}

/// `util.par_map`: `f` over `jobs` in up to `threads` threads; the
/// results in order.
fn par_map<J: Sync, R: Send>(threads: usize, jobs: &[J], f: impl Fn(&J) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = jobs.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, jobs.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else {
                        break;
                    };
                    *results[i].lock().unwrap() = Some(f(job));
                }
            });
        }
    });
    results
        .into_iter()
        .map(|r| r.into_inner().unwrap().expect("every job ran"))
        .collect()
}

impl Default for ConvertPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for ConvertPlugin {
    fn name(&self) -> &'static str {
        "convert"
    }

    /// beets' early import stages `auto_convert` and `auto_convert_keep`.
    fn early_import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        let auto = ctx.cfg.bool("convert.auto");
        let auto_keep = ctx.cfg.bool("convert.auto_keep");
        if !auto && !auto_keep {
            return Ok(());
        }
        let s = Settings::new(ctx.cfg)?;
        // beets converts in other threads (`par_map`, the pipeline), where
        // the plugin's import-time WARNING level (thread-local) doesn't
        // apply: the messages show at INFO.
        let log = PluginLog::new("convert");
        log.set_base_level(INFO);
        if auto {
            let sel = Selector::new(&s, ctx.lib)?;
            self.auto_convert(&s, &sel, &log, ctx.lib, task.items)?;
        }
        if auto_keep {
            let conv = Converter::new(ctx.cfg, ctx.lib, &s, &log)?;
            let items: Vec<Item> = task
                .items
                .iter()
                .filter(|i| conv.sel.should_transcode(i))
                .cloned()
                .collect();
            conv.parallel_convert(items, false)?;
        }
        Ok(())
    }

    /// `_cleanup` (`import_task_files`): delete the temporary transcodes
    /// the task's files were placed from. Unlike beets, one the library
    /// still uses (imported without copying, or symlinked) is kept.
    fn import_task_files(&self, _ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        let mut temp = self.temp_files.lock().unwrap();
        let keep_linked = task.op == fileops::FileOp::Link;
        temp.retain(|path| {
            let in_task = task.items.iter().any(|i| i.path() == path);
            if in_task {
                // Still the library's file (no copy/move).
                return false;
            }
            if keep_linked {
                return false;
            }
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
            false
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "convert/tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "convert",
        order: 20,
        defaults: r#"convert:
    dest:
    pretend: no
    link: no
    hardlink: no
    threads: CPU_COUNT
    format: mp3
    id3v23: inherit
    write_metadata: yes
    formats:
        aac:
            command: ffmpeg -i $source -y -vn -acodec aac -aq 1 $dest
            extension: m4a
        alac:
            command: ffmpeg -i $source -y -vn -acodec alac $dest
            extension: m4a
        flac: ffmpeg -i $source -y -vn -acodec flac $dest
        mp3: ffmpeg -i $source -y -vn -aq 2 $dest
        opus: ffmpeg -i $source -y -vn -acodec libopus -ab 96k $dest
        ogg: ffmpeg -i $source -y -vn -acodec libvorbis -aq 3 $dest
        wma: ffmpeg -i $source -y -vn -acodec wmav2 -vn $dest
    max_bitrate:
    auto: no
    auto_keep: no
    tmpdir:
    quiet: no
    embed: yes
    paths: {}
    no_convert: ''
    never_convert_lossy_files: no
    copy_album_art: no
    album_art_maxwidth: 0
    delete_originals: no
    playlist:
    force: no
    refresh: no
    keep_new: no
"#,
        config_keys: &["convert"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "convert",
                build: build_convert,
                run: run_convert,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(ConvertPlugin::new()))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ConvertArgs {
    /// Show actions but do nothing
    #[arg(short = 'p', long)]
    pretend: bool,
    /// Change the number of threads, defaults to maximum available
    /// processors
    #[arg(short = 't', long)]
    threads: Option<usize>,
    /// Reconvert if original file is newer than converted file
    #[arg(short = 'r', long)]
    refresh: bool,
    /// Keep only the converted and move the old files
    #[arg(short = 'k', long = "keep-new")]
    keep_new: bool,
    /// Set the destination directory
    #[arg(short = 'd', long)]
    dest: Option<String>,
    /// Set the target format of the tracks
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Do not ask for confirmation
    #[arg(short = 'y', long)]
    yes: bool,
    /// Symlink files that do not need transcoding.
    #[arg(short = 'l', long)]
    link: bool,
    /// Hardlink files that do not need transcoding. Overrides --link.
    #[arg(short = 'H', long)]
    hardlink: bool,
    /// Create an m3u8 playlist file containing the converted files. The
    /// playlist file will be saved below the destination directory, thus
    /// PLAYLIST could be a file name or a relative path. To ensure a
    /// working playlist when transferred to a different computer, or
    /// opened from an external drive, relative paths pointing to media
    /// files will be used.
    #[arg(short = 'm', long)]
    playlist: Option<String>,
    /// Force transcoding. Ignores no_convert, never_convert_lossy_files,
    /// and max_bitrate
    #[arg(short = 'F', long)]
    force: bool,
    /// Match albums instead of tracks
    #[arg(short = 'a', long)]
    album: bool,
    /// Limit query results
    #[arg(long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_convert() -> clap::Command {
    use clap::Args as _;
    ConvertArgs::augment_args(
        clap::Command::new("convert").about("convert to external location"),
    )
}

pub fn run_convert(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ConvertArgs::from_arg_matches(m)?;
    crate::plugins::convert::command(
        cfg,
        lib,
        crate::plugins::convert::CmdOpts {
            pretend: a.pretend,
            threads: a.threads,
            refresh: a.refresh,
            keep_new: a.keep_new,
            dest: a.dest,
            format: a.format,
            yes: a.yes,
            link: a.link,
            hardlink: a.hardlink,
            playlist: a.playlist,
            force: a.force,
            album: a.album,
            limit: a.limit,
            query: a.query,
        },
    )
}
