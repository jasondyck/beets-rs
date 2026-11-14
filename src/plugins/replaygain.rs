//! A port of beets 2.14's `replaygain` plugin (`beetsplug/replaygain.py`):
//! ReplayGain (`rg_*`) or, for the formats in `r128`, EBU R128 (`r128_*`)
//! track and album gains, computed on import (`auto`) or with
//! `beet replaygain [-a] [-f] [-w/-W] [-t N] [-l N]`.
//!
//! Backends: `ffmpeg` (its `ebur128` filter), `command` (mp3gain,
//! mp3rgain or aacgain) and `metaflac`, as beets runs them. beets'
//! `gstreamer` and `audiotools` backends need Python libraries; selecting
//! them fails to load the plugin with beets' error ("python-gi not found",
//! "audiotools not found").
//!
//! Analyses run in a pool of `threads` worker threads (beets'
//! `ThreadPool`; `-t 0` runs them in turn); results are stored by the
//! main thread as they come in, as the pool's callbacks.

use crate::config::Config;
use crate::hooks::py_float;
use crate::library::Library;
use crate::model::{Album, Item, Kind, Value};
use crate::plugins::{ImportTask, Plugin, PluginCtx, PluginLog};
use crate::query::Query;
use crate::template::Template;
use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

const NAME: &str = "replaygain";

/// The backends beets 2.14 has, in its order (for the error message).
pub const BACKENDS: &[&str] = &["command", "metaflac", "gstreamer", "audiotools", "ffmpeg"];

// ------------------------------------------------------------------ errors

/// beets' `ReplayGainError` (local to a track or album) and
/// `FatalReplayGainError`.
#[derive(Debug, Clone, PartialEq)]
pub enum RgError {
    Local(String),
    Fatal(String),
}

impl std::fmt::Display for RgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RgError::Local(s) | RgError::Fatal(s) => f.write_str(s),
        }
    }
}

type RgResult<T> = std::result::Result<T, RgError>;

fn local(s: impl Into<String>) -> RgError {
    RgError::Local(s.into())
}

fn fatal(s: impl Into<String>) -> RgError {
    RgError::Fatal(s.into())
}

/// A command's output (beets' `CommandOutput`).
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// How running a command went: not found (`OSError`), or its output.
enum Run {
    NotFound(String),
    Done(Output),
}

/// beets' `command_output`: run `args`, failing on a non-zero status.
/// Err(None) when the program can't be run (Python's `OSError`).
fn command_output(args: &[String]) -> std::result::Result<Output, Option<(i32, Vec<u8>)>> {
    let out = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| None)?;
    if !out.status.success() {
        let code = crate::osutil::returncode_or(&out.status, 1);
        let mut both = out.stdout;
        both.extend(out.stderr);
        return Err(Some((code, both)));
    }
    Ok(Output {
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// beets' `call`: the output, or a `ReplayGainError` on a non-zero status.
fn call(args: &[String], log: &PluginLog) -> RgResult<Run> {
    match command_output(args) {
        Ok(o) => Ok(Run::Done(o)),
        Err(None) => Ok(Run::NotFound(args[0].clone())),
        Err(Some((code, output))) => {
            log.debug(&String::from_utf8_lossy(&output));
            Err(local(format!("{} exited with status {code}", args[0])))
        }
    }
}

/// `call` where a missing program is an error too (beets lets the
/// `OSError` through; it ends the command).
fn call_found(args: &[String], log: &PluginLog) -> RgResult<Output> {
    match call(args, log)? {
        Run::Done(o) => Ok(o),
        Run::NotFound(p) => Err(fatal(format!(
            "[Errno 2] No such file or directory: {}",
            crate::hooks::py_repr(&p)
        ))),
    }
}

pub fn db_to_lufs(db: f64) -> f64 {
    db - 107.0
}

/// Python 3.12+'s `sum()` of floats (Neumaier's compensated summation),
/// so album gains match beets' to the last bit.
pub fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = 0.0f64;
    let mut c = 0.0f64;
    for x in values {
        let t = sum + x;
        if sum.abs() >= x.abs() {
            c += (sum - t) + x;
        } else {
            c += (x - t) + sum;
        }
        sum = t;
    }
    if c != 0.0 && c.is_finite() {
        sum += c;
    }
    sum
}

/// Python's `float()` of a byte string (surrounding whitespace allowed).
fn py_parse_float(s: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(s).ok()?.trim();
    if s.is_empty() || s.contains('_') && (s.starts_with('_') || s.ends_with('_')) {
        return None;
    }
    s.replace('_', "").parse().ok()
}

/// Python's `repr()` of a byte string (for the debug and error messages).
fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            c if c as char == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote);
    out
}

/// Python's `bytes.splitlines()`.
fn split_lines(b: &[u8]) -> Vec<&[u8]> {
    let mut out = vec![];
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                out.push(&b[start..i]);
                start = i + 1;
            }
            b'\r' => {
                out.push(&b[start..i]);
                if b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < b.len() {
        out.push(&b[start..]);
    }
    out
}

// ------------------------------------------------------------------- tasks

/// A gain (in LU to the reference level) and peak (part of full scale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain {
    pub gain: f64,
    pub peak: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeakMethod {
    True,
    Sample,
}

impl PeakMethod {
    pub fn name(self) -> &'static str {
        match self {
            PeakMethod::True => "true",
            PeakMethod::Sample => "sample",
        }
    }
}

/// One file of a task: what the backends need of an item.
#[derive(Debug, Clone)]
pub struct TaskItem {
    pub path: PathBuf,
    pub format: String,
    /// `str(item)` (the `format_item` template), for messages.
    pub name: String,
}

/// beets' `RgTask`/`R128Task`: one calculation (a track, a disc or an
/// album), its parameters and results.
#[derive(Debug, Clone)]
pub struct GainTask {
    pub items: Vec<Item>,
    pub files: Vec<TaskItem>,
    /// The album (`str(album)`), for album tasks.
    pub album: Option<String>,
    pub target_level: f64,
    pub peak_method: Option<PeakMethod>,
    pub r128: bool,
    pub backend_name: &'static str,
    pub album_gain: Option<Gain>,
    pub track_gains: Option<Vec<Gain>>,
}

fn opt_float(item: &Item, field: &str) -> Option<f64> {
    match item.get(field) {
        Some(Value::Float(f)) => Some(f),
        Some(Value::Int(i)) => Some(i as f64),
        _ => None,
    }
}

fn float_str(item: &Item, field: &str) -> String {
    opt_float(item, field).map_or_else(|| "None".into(), py_float)
}

impl GainTask {
    fn store_track_gain(&self, item: &mut Item, g: Gain, log: &PluginLog) {
        if self.r128 {
            item.set("rg_track_gain", Value::Null);
            item.set("rg_track_peak", Value::Null);
            item.set("r128_track_gain", Value::Float(g.gain));
            log.debug(&format!(
                "applied r128 track gain {} LU",
                float_str(item, "r128_track_gain")
            ));
        } else {
            item.set("r128_track_gain", Value::Null);
            item.set("rg_track_gain", Value::Float(g.gain));
            item.set("rg_track_peak", Value::Float(g.peak));
            log.debug(&format!(
                "applied track gain {} LU, peak {} of FS",
                float_str(item, "rg_track_gain"),
                float_str(item, "rg_track_peak")
            ));
        }
    }

    fn store_album_gain(&self, item: &mut Item, g: Gain, log: &PluginLog) {
        if self.r128 {
            item.set("rg_album_gain", Value::Null);
            item.set("rg_album_peak", Value::Null);
            item.set("r128_album_gain", Value::Float(g.gain));
            log.debug(&format!(
                "applied r128 album gain {} LU",
                float_str(item, "r128_album_gain")
            ));
        } else {
            item.set("r128_album_gain", Value::Null);
            item.set("rg_album_gain", Value::Float(g.gain));
            item.set("rg_album_peak", Value::Float(g.peak));
            log.debug(&format!(
                "applied album gain {} LU, peak {} of FS",
                float_str(item, "rg_album_gain"),
                float_str(item, "rg_album_peak")
            ));
        }
    }

    /// beets' `RgTask.store`: the gains into the items (and the database,
    /// if given), and their files with `write`.
    pub fn store(&mut self, lib: Option<&Library>, write: bool, log: &PluginLog) -> RgResult<()> {
        let save = |item: &mut Item| {
            if let Some(lib) = lib
                && let Err(e) = lib.store(item)
            {
                crate::logging::emit(format!("{e:#}"));
            }
        };
        let finish = |item: &mut Item, name: &str| {
            if write && let Err(e) = crate::media::write_item(item) {
                // beets' `try_write`: a failure is logged, not fatal.
                crate::logging::emit(format!("{e:#}"));
            }
            log.debug(&format!("done analyzing {name}"));
        };
        let mut items = std::mem::take(&mut self.items);
        let r = if self.album.is_some() {
            match (&self.album_gain, &self.track_gains) {
                (Some(ag), Some(tg)) if tg.len() == items.len() => {
                    let ag = *ag;
                    for (i, (item, g)) in items.iter_mut().zip(tg.clone()).enumerate() {
                        self.store_track_gain(item, g, log);
                        save(item);
                        self.store_album_gain(item, ag, log);
                        save(item);
                        finish(item, &self.files[i].name);
                    }
                    Ok(())
                }
                _ => Err(local(format!(
                    "ReplayGain backend `{}` failed for some tracks in album {}",
                    self.backend_name,
                    self.album.as_deref().unwrap_or_default()
                ))),
            }
        } else {
            match &self.track_gains {
                Some(tg) if tg.len() == 1 => {
                    let item = &mut items[0];
                    self.store_track_gain(item, tg[0], log);
                    save(item);
                    finish(item, &self.files[0].name);
                    Ok(())
                }
                _ => Err(local(format!(
                    "ReplayGain backend `{}` failed for track {}",
                    self.backend_name, self.files[0].name
                ))),
            }
        };
        self.items = items;
        r
    }
}

// ---------------------------------------------------------------- backends

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Sets `task.track_gains`.
    fn compute_track_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()>;
    /// Sets `task.album_gain` and `task.track_gains`.
    fn compute_album_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()>;
}

/// beets' `FfmpegBackend`: ffmpeg's `ebur128` filter.
pub struct FfmpegBackend {
    pub ffmpeg_path: String,
}

impl FfmpegBackend {
    pub fn new(log: &PluginLog) -> RgResult<Self> {
        let b = FfmpegBackend {
            ffmpeg_path: "ffmpeg".into(),
        };
        let out = match call(&[b.ffmpeg_path.clone(), "-version".into()], log)? {
            Run::Done(o) => o,
            Run::NotFound(_) => {
                return Err(fatal(format!("could not find ffmpeg at {}", b.ffmpeg_path)));
            }
        };
        if !ffmpeg_supports_ebur128(&out.stdout) {
            return Err(fatal(
                "Installed FFmpeg version does not support ReplayGain.calculation. Either \
                 libavfilter version 6.67.100 or above orthe --enable-libebur128 configuration \
                 option is required.",
            ));
        }
        Ok(b)
    }

    fn construct_cmd(&self, path: &Path, peak: Option<PeakMethod>) -> Vec<String> {
        vec![
            self.ffmpeg_path.clone(),
            "-nostats".into(),
            "-hide_banner".into(),
            "-i".into(),
            path.display().to_string(),
            "-map".into(),
            "a:0".into(),
            "-filter".into(),
            format!("ebur128=peak={}", peak.map_or("none", |p| p.name())),
            "-f".into(),
            "null".into(),
            "-".into(),
        ]
    }

    /// beets' `_analyse_item`: the gain and the number of gating blocks
    /// over the threshold (0 unless `count_blocks`).
    fn analyse_item(
        &self,
        file: &TaskItem,
        target_level: f64,
        peak: Option<PeakMethod>,
        count_blocks: bool,
        log: &PluginLog,
    ) -> RgResult<(Gain, usize)> {
        log.debug(&format!("analyzing {}", file.name));
        let cmd = self.construct_cmd(&file.path, peak);
        log.debug(&format!("executing {}", cmd.join(" ")));
        let out = call_found(&cmd, log)?;
        let a = parse_ffmpeg_output(&out.stderr, target_level, peak, count_blocks)?;
        if let Some(threshold) = a.threshold {
            log.debug(&format!(
                "{}: {} blocks over {} LUFS",
                file.name,
                a.blocks,
                py_float(threshold)
            ));
        }
        log.debug(&format!(
            "{}: gain {} LU, peak {}",
            file.name,
            py_float(a.gain.gain),
            py_float(a.gain.peak)
        ));
        Ok((a.gain, a.blocks))
    }
}

/// Whether `ffmpeg -version`'s output says it has the ebur128 filter.
pub fn ffmpeg_supports_ebur128(stdout: &[u8]) -> bool {
    let mut ok = false;
    for line in split_lines(stdout) {
        if line.starts_with(b"configuration:")
            && line
                .windows(b"--enable-libebur128".len())
                .any(|w| w == b"--enable-libebur128")
        {
            ok = true;
        }
        if line.starts_with(b"libavfilter") {
            let s = String::from_utf8_lossy(line);
            let rest = s.split_once(' ').map_or("", |(_, r)| r);
            let ver = rest.split_once('/').map_or(rest, |(v, _)| v);
            let parts: Option<Vec<i64>> = ver.split('.').map(|p| p.trim().parse().ok()).collect();
            if let Some(parts) = parts
                && parts.as_slice() >= [6, 67, 100].as_slice()
            {
                ok = true;
            }
        }
    }
    ok
}

/// What `parse_ffmpeg_output` read.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub gain: Gain,
    pub blocks: usize,
    /// The gating threshold, when counting blocks.
    pub threshold: Option<f64>,
}

/// beets' `_find_line`.
fn find_line(output: &[&[u8]], search: &[u8], start: isize, step: isize) -> RgResult<usize> {
    let end = if step > 0 { output.len() as isize } else { -1 };
    let mut i = start;
    while (step > 0 && i < end) || (step < 0 && i > end) {
        if i >= 0 && (i as usize) < output.len() && output[i as usize].starts_with(search) {
            return Ok(i as usize);
        }
        i += step;
    }
    Err(local(format!(
        "ffmpeg output: missing {} after line {start}",
        bytes_repr(search)
    )))
}

/// beets' `_parse_float`: the number in `key: value unit`.
fn parse_float(line: &[u8]) -> RgResult<f64> {
    let Some(colon) = line.iter().position(|&c| c == b':') else {
        return Err(local(format!(
            "ffmpeg output: expected key value pair, found {}",
            bytes_repr(line)
        )));
    };
    let mut value = &line[colon + 1..];
    while let Some((&c, rest)) = value.split_first() {
        if c.is_ascii_whitespace() || c == 0x0b || c == 0x0c {
            value = rest;
        } else {
            break;
        }
    }
    let value = value.split(|&c| c == b' ').next().unwrap_or_default();
    py_parse_float(value).ok_or_else(|| {
        local(format!(
            "ffmpeg output: expected float value, found {}",
            bytes_repr(value)
        ))
    })
}

/// The gain, peak and gating blocks in the stderr of ffmpeg's `ebur128`
/// filter (beets' `_analyse_item`).
pub fn parse_ffmpeg_output(
    stderr: &[u8],
    target_level: f64,
    peak_method: Option<PeakMethod>,
    count_blocks: bool,
) -> RgResult<Analysis> {
    let output = split_lines(stderr);
    let last = output.len() as isize - 1;
    let target_lufs = db_to_lufs(target_level);
    let peak = match peak_method {
        None => 0.0,
        Some(m) => {
            let name = m.name();
            let head = format!("  {}{} peak:", name[..1].to_uppercase(), &name[1..]);
            let line_peak = find_line(&output, head.as_bytes(), last, -1)?;
            let p = parse_float(output[find_line(&output, b"    Peak:", line_peak as isize, 1)?])?;
            // TPFS -> part of FS
            10f64.powf(p / 20.0)
        }
    };
    let line_il = find_line(&output, b"  Integrated loudness:", last, -1)?;
    let i = parse_float(output[find_line(&output, b"    I:", line_il as isize, 1)?])?;
    let gain = target_lufs - i;
    let mut blocks = 0;
    let mut threshold = None;
    if count_blocks {
        let t = parse_float(output[find_line(&output, b"    Threshold:", line_il as isize, 1)?])?;
        for line in &output {
            if !line.starts_with(b"[Parsed_ebur128") || line.ends_with(b"Summary:") {
                continue;
            }
            let Some(pos) = line.windows(2).position(|w| w == b"M:") else {
                continue;
            };
            let mut l = b"M: ".to_vec();
            l.extend_from_slice(&line[pos + 2..]);
            if parse_float(&l)? >= t {
                blocks += 1;
            }
        }
        threshold = Some(t);
    }
    Ok(Analysis {
        gain: Gain { gain, peak },
        blocks,
        threshold,
    })
}

/// beets' `compute_album_gain` arithmetic: the album's gain is that of all
/// its tracks' gating blocks together, its peak the highest track peak.
pub fn album_gain(tracks: &[(Gain, usize)], target_level: f64) -> Gain {
    let target_lufs = db_to_lufs(target_level);
    let peak = tracks
        .iter()
        .map(|(g, _)| g.peak)
        .fold(None, |m: Option<f64>, p| match m {
            Some(m) if m >= p => Some(m),
            _ => Some(p),
        })
        .unwrap_or(0.0);
    let n_blocks: usize = tracks.iter().map(|(_, n)| n).sum();
    let gain = if n_blocks > 0 {
        let sum_powers = py_sum(tracks.iter().map(|(g, n)| {
            let loudness = target_lufs - g.gain;
            let power = 10f64.powf((loudness + 0.691) / 10.0);
            *n as f64 * power
        }));
        -0.691 + 10.0 * (sum_powers / n_blocks as f64).log10()
    } else {
        -70.0
    };
    Gain {
        gain: target_lufs - gain,
        peak,
    }
}

impl Backend for FfmpegBackend {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn compute_track_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()> {
        let mut gains = vec![];
        for f in &task.files {
            gains.push(
                self.analyse_item(f, task.target_level, task.peak_method, false, log)?
                    .0,
            );
        }
        task.track_gains = Some(gains);
        Ok(())
    }

    fn compute_album_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()> {
        let mut results = vec![];
        for f in &task.files {
            results.push(self.analyse_item(f, task.target_level, task.peak_method, true, log)?);
        }
        let ag = album_gain(&results, task.target_level);
        log.debug(&format!(
            "{}: gain {} LU, peak {}",
            task.album.as_deref().unwrap_or("None"),
            py_float(ag.gain),
            py_float(ag.peak)
        ));
        task.album_gain = Some(ag);
        task.track_gains = Some(results.into_iter().map(|(g, _)| g).collect());
        Ok(())
    }
}

/// beets' `CommandBackend`: mp3gain, mp3rgain or aacgain.
pub struct CommandBackend {
    pub command: String,
    pub cmd_name: &'static str,
    pub noclip: bool,
}

/// The tools and the formats they handle, in beets' order.
const TOOLS: &[(&str, &[&str])] = &[
    ("mp3rgain", &["AAC", "MP3"]),
    ("aacgain", &["AAC", "MP3"]),
    ("mp3gain", &["MP3"]),
];

/// `shutil.which`.
fn which(cmd: &str) -> Option<PathBuf> {
    crate::osutil::which(cmd)
}

impl CommandBackend {
    pub fn new(cfg: &Config) -> RgResult<Self> {
        let command = cfg.str("replaygain.command");
        let name = Path::new(&command)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(&(cmd_name, _)) = TOOLS.iter().find(|(t, _)| *t == name) else {
            return Err(fatal(format!(
                "replaygain.command must be one of {{'mp3rgain', 'aacgain', 'mp3gain'}}, not {}",
                crate::hooks::py_repr(&name)
            )));
        };
        let Some(exec) = which(&command) else {
            return Err(fatal(format!("replaygain command not found: {command}")));
        };
        let noclip = match cfg.get("replaygain.noclip") {
            None => true,
            Some(_) => cfg.bool("replaygain.noclip"),
        };
        Ok(CommandBackend {
            command: exec.display().to_string(),
            cmd_name,
            noclip,
        })
    }

    fn format_supported(&self, f: &TaskItem) -> bool {
        TOOLS
            .iter()
            .find(|(t, _)| *t == self.cmd_name)
            .is_some_and(|(_, fmts)| fmts.contains(&f.format.as_str()))
    }

    /// beets' `compute_gain`: the files' gains (and with `is_album`, the
    /// album's last).
    fn compute_gain(
        &self,
        files: &[&TaskItem],
        target_level: f64,
        is_album: bool,
        log: &PluginLog,
    ) -> RgResult<Vec<Gain>> {
        if files.is_empty() {
            log.debug("no supported tracks to analyze");
            return Ok(vec![]);
        }
        let mut cmd = vec![
            self.command.clone(),
            "-o".into(),
            "-s".into(),
            "s".into(),
            if self.noclip { "-k" } else { "-c" }.into(),
            "-d".into(),
            ((target_level - 89.0).trunc() as i64).to_string(),
        ];
        cmd.extend(files.iter().map(|f| f.path.display().to_string()));
        log.debug(&format!("analyzing {} files", files.len()));
        log.debug(&format!("executing {}", cmd.join(" ")));
        let out = call_found(&cmd, log)?;
        log.debug("analysis finished");
        parse_tool_output(&out.stdout, files.len() + usize::from(is_album), log)
    }
}

/// beets' `parse_tool_output`: the gains in mp3gain's tab-separated
/// output (`-o`), one line per file after the header.
pub fn parse_tool_output(text: &[u8], num_lines: usize, log: &PluginLog) -> RgResult<Vec<Gain>> {
    let mut out = vec![];
    for line in text.split(|&c| c == b'\n').skip(1).take(num_lines) {
        let parts: Vec<&[u8]> = line.split(|&c| c == b'\t').collect();
        let bad = || {
            log.debug(&format!("bad tool output: {}", bytes_repr(text)));
            local("mp3gain failed")
        };
        if parts.len() != 6 || parts[0] == b"File" {
            return Err(bad());
        }
        let num = |b: &[u8]| {
            py_parse_float(b).ok_or_else(|| {
                local(format!(
                    "could not convert string to float: {}",
                    crate::hooks::py_repr(&String::from_utf8_lossy(b))
                ))
            })
        };
        let gain = num(parts[2])?;
        let peak = num(parts[3])? / f64::from(1u32 << 15);
        out.push(Gain { gain, peak });
    }
    Ok(out)
}

impl Backend for CommandBackend {
    fn name(&self) -> &'static str {
        "command"
    }

    fn compute_track_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()> {
        let files: Vec<&TaskItem> = task
            .files
            .iter()
            .filter(|f| self.format_supported(f))
            .collect();
        task.track_gains = Some(self.compute_gain(&files, task.target_level, false, log)?);
        Ok(())
    }

    fn compute_album_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()> {
        let files: Vec<&TaskItem> = task
            .files
            .iter()
            .filter(|f| self.format_supported(f))
            .collect();
        if files.len() != task.files.len() {
            log.debug("tracks are of unsupported format");
            task.album_gain = None;
            task.track_gains = None;
            return Ok(());
        }
        let mut out = self.compute_gain(&files, task.target_level, true, log)?;
        task.album_gain = out.pop();
        task.track_gains = Some(out);
        Ok(())
    }
}

/// beets' `MetaflacBackend`: `metaflac --scan-replay-gain`.
pub struct MetaflacBackend {
    pub command: String,
}

impl MetaflacBackend {
    pub fn new(cfg: &Config, log: &PluginLog) -> RgResult<Self> {
        let command = match cfg.get("replaygain.metaflac") {
            None => "metaflac".to_string(),
            Some(_) => cfg.str("replaygain.metaflac"),
        };
        let Some(path) = which(&command) else {
            return Err(fatal(format!(
                "replaygain metaflac command not found: {}",
                crate::hooks::py_repr(&command)
            )));
        };
        let b = MetaflacBackend {
            command: path.display().to_string(),
        };
        let help = call_found(&[b.command.clone(), "--help".into()], log)?;
        if !help
            .stdout
            .windows(b"--scan-replay-gain".len())
            .any(|w| w == b"--scan-replay-gain")
        {
            return Err(fatal(
                "your metaflac does not support --scan-replay-gain (FLAC tools 1.3.2 or newer is \
                 required)",
            ));
        }
        Ok(b)
    }

    fn read_gain(
        &self,
        files: &[&TaskItem],
        target_level: f64,
        log: &PluginLog,
    ) -> RgResult<Vec<(Gain, Gain)>> {
        let mut cmd = vec![self.command.clone(), "--scan-replay-gain".into()];
        cmd.extend(files.iter().map(|f| f.path.display().to_string()));
        let out = call_found(&cmd, log)?;
        let text = String::from_utf8_lossy(&out.stdout);
        let offset = target_level - 89.0;
        let mut results = vec![];
        for gains in parse_metaflac_gains(&text)?.into_iter().take(files.len()) {
            let [ag, ap, tg, tp] = gains;
            results.push((
                Gain {
                    gain: ag + offset,
                    peak: ap,
                },
                Gain {
                    gain: tg + offset,
                    peak: tp,
                },
            ));
        }
        if results.len() < files.len() {
            return Err(local(format!(
                "metaflac output: no gains for {}",
                files[results.len()].name
            )));
        }
        Ok(results)
    }
}

/// beets' `_parse_gains`: each line is
/// `<path>: <album gain> <album peak> <track gain> <track peak>`.
pub fn parse_metaflac_gains(text: &str) -> RgResult<Vec<[f64; 4]>> {
    let mut out = vec![];
    for line in text.lines() {
        let Some((_, gains)) = line.rsplit_once(": ") else {
            return Err(local(format!("metaflac output: unexpected line {line:?}")));
        };
        let v: Vec<f64> = gains
            .split_whitespace()
            .map(|g| {
                py_parse_float(g.as_bytes()).ok_or_else(|| {
                    local(format!(
                        "could not convert string to float: {}",
                        crate::hooks::py_repr(g)
                    ))
                })
            })
            .collect::<RgResult<_>>()?;
        if v.len() < 4 {
            return Err(local(format!("metaflac output: unexpected line {line:?}")));
        }
        out.push([v[0], v[1], v[2], v[3]]);
    }
    Ok(out)
}

impl Backend for MetaflacBackend {
    fn name(&self) -> &'static str {
        "metaflac"
    }

    fn compute_track_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()> {
        let files: Vec<&TaskItem> = task.files.iter().filter(|f| f.format == "FLAC").collect();
        if files.is_empty() {
            task.track_gains = None;
            return Ok(());
        }
        let r = self.read_gain(&files, task.target_level, log)?;
        task.track_gains = Some(r.into_iter().map(|(_, t)| t).collect());
        Ok(())
    }

    fn compute_album_gain(&self, task: &mut GainTask, log: &PluginLog) -> RgResult<()> {
        if task.files.is_empty() || !task.files.iter().all(|f| f.format == "FLAC") {
            task.album_gain = None;
            task.track_gains = None;
            return Ok(());
        }
        let files: Vec<&TaskItem> = task.files.iter().collect();
        let r = self.read_gain(&files, task.target_level, log)?;
        task.album_gain = Some(r[0].0);
        task.track_gains = Some(r.into_iter().map(|(_, t)| t).collect());
        Ok(())
    }
}

/// The backend `name`, set up (beets' `BACKENDS[name](config, log)`).
fn make_backend(cfg: &Config, name: &str, log: &PluginLog) -> RgResult<Arc<dyn Backend>> {
    Ok(match name {
        "ffmpeg" => Arc::new(FfmpegBackend::new(log)?),
        "command" => Arc::new(CommandBackend::new(cfg)?),
        "metaflac" => Arc::new(MetaflacBackend::new(cfg, log)?),
        "gstreamer" => return Err(fatal("Failed to load GStreamer: python-gi not found")),
        "audiotools" => return Err(fatal("Failed to load audiotools: audiotools not found")),
        _ => unreachable!("checked against BACKENDS"),
    })
}

// -------------------------------------------------------------------- pool

#[derive(Clone, Copy)]
enum Compute {
    Track,
    Album,
}

type Job = (GainTask, Compute);
type Done = (GainTask, RgResult<()>);

/// beets' `ThreadPool` of analyses: workers compute, the main thread
/// stores the results (the pool's callbacks).
struct Pool {
    queue: Arc<JobQueue>,
    done: mpsc::Receiver<Done>,
    workers: Vec<std::thread::JoinHandle<()>>,
    pending: usize,
}

/// The jobs still to start. Workers pop under the lock and wait on the
/// condition variable (which releases it), so none holds a lock while idle.
struct JobQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct QueueState {
    jobs: VecDeque<Job>,
    closed: bool,
}

impl JobQueue {
    fn new() -> Self {
        JobQueue {
            state: Mutex::new(QueueState {
                jobs: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// Queue a job; false once the pool is closed (the caller then runs it
    /// itself, as for a pool that was never opened).
    fn push(&self, job: Job) -> bool {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return false;
        }
        state.jobs.push_back(job);
        drop(state);
        self.ready.notify_one();
        true
    }

    /// No more jobs will be pushed; workers drain what is queued and stop.
    fn close(&self) {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed = true;
        self.ready.notify_all();
    }

    fn pop(&self) -> Option<Job> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(job) = state.jobs.pop_front() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Pool {
    fn new(threads: usize, backend: &Arc<dyn Backend>, level: u8) -> Pool {
        let (dtx, drx) = mpsc::channel::<Done>();
        let queue = Arc::new(JobQueue::new());
        let workers = (0..threads)
            .map(|_| {
                let queue = Arc::clone(&queue);
                let dtx = dtx.clone();
                let backend = Arc::clone(backend);
                std::thread::spawn(move || {
                    let log = PluginLog::new(NAME);
                    log.set_base_level(level);
                    while let Some((mut task, kind)) = queue.pop() {
                        let r = compute(backend.as_ref(), &mut task, kind, &log);
                        if dtx.send((task, r)).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        Pool {
            queue,
            done: drx,
            workers,
            pending: 0,
        }
    }
}

fn compute(
    backend: &dyn Backend,
    task: &mut GainTask,
    kind: Compute,
    log: &PluginLog,
) -> RgResult<()> {
    match kind {
        Compute::Track => backend.compute_track_gain(task, log),
        Compute::Album => backend.compute_album_gain(task, log),
    }
}

// ------------------------------------------------------------------ plugin

/// beets' `album.items()`: in the default item sort.
fn album_items(lib: &Library, album: &Album) -> Result<Vec<Item>> {
    let q = format!("album_id:{}", album.id.unwrap_or(0));
    lib.items(&Query::parse(Kind::Item, &[q])?)
}

/// The plugin, set up from its settings (beets' `ReplayGainPlugin`).
pub struct ReplayGain {
    pub force_on_import: bool,
    pub backend_name: String,
    pub peak_method: PeakMethod,
    pub auto: bool,
    pub r128_whitelist: Vec<String>,
    pub targetlevel: f64,
    pub r128_targetlevel: f64,
    pub per_disc: bool,
    pub threads: usize,
    pub parallel_on_import: bool,
    backend: Arc<dyn Backend>,
    log: PluginLog,
    format_item: Template,
    format_album: Template,
    pool: std::cell::RefCell<Option<Pool>>,
    /// A fatal error from a worker, ending the run.
    fatal: std::cell::RefCell<Option<String>>,
}

/// The `threads` default: `os.cpu_count()`.
pub fn cpu_count() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

impl ReplayGain {
    pub fn new(cfg: &Config) -> Result<Self> {
        let log = PluginLog::new(NAME);
        let backend_name = cfg.str("replaygain.backend");
        if !BACKENDS.contains(&backend_name.as_str()) {
            bail!(
                "Selected ReplayGain backend {backend_name} is not supported. Please select one \
                 of: {}",
                BACKENDS.join(", ")
            );
        }
        let peak = cfg.str("replaygain.peak");
        let peak_method = match peak.as_str() {
            "true" => PeakMethod::True,
            "sample" => PeakMethod::Sample,
            _ => bail!(
                "Selected ReplayGain peak method {peak} is not supported. Please select one of: \
                 true, sample"
            ),
        };
        let backend = make_backend(cfg, &backend_name, &log)
            .map_err(|e| anyhow!("replaygain initialization failed: {e}"))?;
        let threads = match cfg.get("replaygain.threads") {
            None | Some(serde_yaml::Value::Null) => cpu_count(),
            Some(_) => cfg.f64("replaygain.threads").max(0.0) as usize,
        };
        Ok(ReplayGain {
            force_on_import: cfg.bool("replaygain.overwrite"),
            backend_name,
            peak_method,
            auto: cfg.bool("replaygain.auto"),
            r128_whitelist: cfg.list("replaygain.r128"),
            targetlevel: cfg.f64("replaygain.targetlevel"),
            r128_targetlevel: cfg.f64("replaygain.r128_targetlevel"),
            per_disc: cfg.bool("replaygain.per_disc"),
            threads,
            parallel_on_import: cfg.bool("replaygain.parallel_on_import"),
            backend,
            log,
            format_item: Template::parse(&cfg.str("format_item")),
            format_album: Template::parse(&cfg.str("format_album")),
            pool: std::cell::RefCell::default(),
            fatal: std::cell::RefCell::default(),
        })
    }

    fn should_use_r128(&self, item: &Item) -> bool {
        self.r128_whitelist.iter().any(|f| f == item.str("format"))
    }

    fn track_requires_gain(&self, item: &Item) -> bool {
        if self.should_use_r128(item) {
            opt_float(item, "r128_track_gain").is_none()
        } else {
            opt_float(item, "rg_track_gain").is_none() || opt_float(item, "rg_track_peak").is_none()
        }
    }

    fn album_requires_gain(&self, items: &[Item]) -> bool {
        items.iter().any(|item| {
            if self.should_use_r128(item) {
                opt_float(item, "r128_track_gain").is_none()
                    || opt_float(item, "r128_album_gain").is_none()
            } else {
                opt_float(item, "rg_album_gain").is_none()
                    || opt_float(item, "rg_album_peak").is_none()
            }
        })
    }

    fn item_name(&self, item: &Item, lib: Option<&Library>) -> String {
        crate::commands::format_record(&self.format_item, item, lib)
    }

    fn album_name(&self, album: &Album, lib: Option<&Library>) -> String {
        crate::commands::format_record(&self.format_album, album, lib)
    }

    fn create_task(
        &self,
        items: Vec<Item>,
        use_r128: bool,
        album: Option<String>,
        lib: Option<&Library>,
    ) -> GainTask {
        let files = items
            .iter()
            .map(|i| TaskItem {
                path: i.path().to_path_buf(),
                format: i.str("format").to_string(),
                name: self.item_name(i, lib),
            })
            .collect();
        GainTask {
            items,
            files,
            album,
            target_level: if use_r128 {
                self.r128_targetlevel
            } else {
                self.targetlevel
            },
            peak_method: (!use_r128).then_some(self.peak_method),
            r128: use_r128,
            backend_name: self.backend.name(),
            album_gain: None,
            track_gains: None,
        }
    }

    /// beets' `handle_album`: `items` are the album's.
    pub fn handle_album(
        &self,
        lib: Option<&Library>,
        album: &Album,
        items: Vec<Item>,
        write: bool,
        force: bool,
    ) -> Result<()> {
        let name = self.album_name(album, lib);
        if !force && !self.album_requires_gain(&items) {
            self.log.info(&format!("Skipping album {name}"));
            return Ok(());
        }
        let Some(first) = items.first() else {
            return Ok(());
        };
        let use_r128 = self.should_use_r128(first);
        if items.iter().any(|i| self.should_use_r128(i) != use_r128) {
            self.log.error(&format!(
                "Cannot calculate gain for album {name} (incompatible formats)"
            ));
            return Ok(());
        }
        self.log.info(&format!("analyzing {name}"));
        let mut discs: Vec<(i64, Vec<Item>)> = vec![];
        if self.per_disc {
            for item in items {
                let d = item.int("disc");
                match discs.iter_mut().find(|(n, _)| *n == d) {
                    Some((_, v)) => v.push(item),
                    None => discs.push((d, vec![item])),
                }
            }
        } else {
            discs.push((1, items));
        }
        for (_, items) in discs {
            let task = self.create_task(items, use_r128, Some(name.clone()), lib);
            self.apply(task, Compute::Album, lib, write)?;
        }
        Ok(())
    }

    /// beets' `handle_track`.
    pub fn handle_track(
        &self,
        lib: Option<&Library>,
        item: Item,
        write: bool,
        force: bool,
    ) -> Result<()> {
        if !force && !self.track_requires_gain(&item) {
            self.log
                .info(&format!("Skipping track {}", self.item_name(&item, lib)));
            return Ok(());
        }
        let use_r128 = self.should_use_r128(&item);
        let task = self.create_task(vec![item], use_r128, None, lib);
        self.apply(task, Compute::Track, lib, write)
    }

    /// beets' `_apply`: in the pool if it's open, else now.
    fn apply(
        &self,
        mut task: GainTask,
        kind: Compute,
        lib: Option<&Library>,
        write: bool,
    ) -> Result<()> {
        if let Some(e) = self.fatal.borrow().as_ref() {
            bail!("Fatal replay gain error: {e}");
        }
        let sent = {
            let mut pool = self.pool.borrow_mut();
            match pool.as_mut() {
                Some(p) => {
                    let queued = p.queue.push((task.clone(), kind));
                    if queued {
                        p.pending += 1;
                    }
                    queued
                }
                None => false,
            }
        };
        if sent {
            self.drain(lib, write, false);
            return Ok(());
        }
        let r = compute(self.backend.as_ref(), &mut task, kind, &self.log)
            .and_then(|()| task.store(lib, write, &self.log));
        match r {
            Ok(()) => Ok(()),
            Err(RgError::Local(e)) => {
                self.log.info(&format!("ReplayGain error: {e}"));
                Ok(())
            }
            Err(RgError::Fatal(e)) => bail!("Fatal replay gain error: {e}"),
        }
    }

    /// Store the pool's finished results (all of them, waiting, if `wait`).
    fn drain(&self, lib: Option<&Library>, write: bool, wait: bool) {
        loop {
            let got = {
                let pool = self.pool.borrow();
                let Some(p) = pool.as_ref() else { return };
                if p.pending == 0 {
                    return;
                }
                if wait {
                    p.done.recv().ok()
                } else {
                    p.done.try_recv().ok()
                }
            };
            let Some((mut task, r)) = got else { return };
            if let Some(p) = self.pool.borrow_mut().as_mut() {
                p.pending -= 1;
            }
            match r.and_then(|()| task.store(lib, write, &self.log)) {
                Ok(()) => {}
                // `handle_exc`: non-fatal errors are logged.
                Err(RgError::Local(e)) => self.log.info(&e),
                Err(RgError::Fatal(e)) => {
                    // The exception watcher ends the pool.
                    self.fatal.borrow_mut().get_or_insert(e);
                    self.terminate_pool();
                    return;
                }
            }
        }
    }

    fn open_pool(&self, threads: usize) {
        let mut pool = self.pool.borrow_mut();
        if threads > 0 && pool.is_none() {
            *pool = Some(Pool::new(threads, &self.backend, self.log.base_level()));
        }
    }

    /// Wait for the pool's work and store it (beets' `close_pool`).
    fn close_pool(&self, lib: Option<&Library>, write: bool) -> Result<()> {
        self.drain(lib, write, true);
        if let Some(mut p) = self.pool.borrow_mut().take() {
            p.queue.close();
            for w in p.workers.drain(..) {
                w.join().ok();
            }
        }
        match self.fatal.borrow_mut().take() {
            Some(e) => bail!("Fatal replay gain error: {e}"),
            None => Ok(()),
        }
    }

    /// beets' `terminate_pool`: drop the work not started.
    fn terminate_pool(&self) {
        if let Some(p) = self.pool.borrow_mut().take() {
            p.queue.close();
            // Workers finish what they're on; their results are dropped.
            drop(p.done);
        }
    }

    /// beets' `imported` stage.
    fn imported(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if !self.auto {
            return Ok(());
        }
        let parallel = self.parallel_on_import && self.threads > 0;
        let lib = Some(ctx.lib);
        self.log
            .at_level(crate::plugins::WARNING, || -> Result<()> {
                if parallel {
                    self.open_pool(self.threads);
                }
                let r = match task.album.as_deref() {
                    Some(album) => {
                        let items = match album.id {
                            Some(_) => album_items(ctx.lib, album)?,
                            None => task.items.to_vec(),
                        };
                        self.handle_album(lib, album, items, false, self.force_on_import)
                    }
                    None => match task.items.first() {
                        Some(item) => {
                            self.handle_track(lib, item.clone(), false, self.force_on_import)
                        }
                        None => Ok(()),
                    },
                };
                let c = self.close_pool(lib, false);
                r.and(c)
            })
    }
}

impl Plugin for ReplayGain {
    fn name(&self) -> &'static str {
        NAME
    }

    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        self.imported(ctx, task)
    }
}

/// `beet replaygain`'s options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub album: bool,
    pub force: bool,
    pub threads: Option<usize>,
    /// `-w`/`-W`, if given.
    pub write: Option<bool>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

impl ReplayGain {
    /// beets' `command_func`.
    pub fn command(&self, cfg: &Config, library: &Library, opts: &CmdOpts) -> Result<()> {
        let write = opts.write.unwrap_or_else(|| cfg.bool("import.write"));
        // `-t 0` runs the analyses in turn.
        if opts.threads != Some(0) {
            self.open_pool(opts.threads.unwrap_or(self.threads));
        }
        let lib = Some(library);
        let r = (|| -> Result<()> {
            if opts.album {
                let mut albums = library.albums(&Query::parse(Kind::Album, &opts.query)?)?;
                albums.truncate(opts.limit.unwrap_or(usize::MAX));
                self.log.info(&format!(
                    "Analyzing {} albums ~ {} backend...",
                    albums.len(),
                    self.backend_name
                ));
                for album in &albums {
                    let items = album_items(library, album)?;
                    self.handle_album(lib, album, items, write, opts.force)?;
                }
            } else {
                let mut items = library.items(&Query::parse(Kind::Item, &opts.query)?)?;
                items.truncate(opts.limit.unwrap_or(usize::MAX));
                self.log.info(&format!(
                    "Analyzing {} tracks ~ {} backend...",
                    items.len(),
                    self.backend_name
                ));
                for item in items {
                    self.handle_track(lib, item, write, opts.force)?;
                }
            }
            Ok(())
        })();
        let c = self.close_pool(lib, write);
        r.and(c)
    }
}

#[cfg(all(test, unix))]
#[path = "replaygain_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "replaygain",
        order: 19,
        defaults: r#"replaygain:
    overwrite: no
    auto: yes
    backend: command
    threads: CPU_COUNT
    parallel_on_import: no
    per_disc: no
    peak: 'true'
    targetlevel: 89
    r128: [Opus]
    r128_targetlevel: 84
    command: ''
    noclip: yes
"#,
        config_keys: &["replaygain"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "replaygain",
                build: build_replaygain,
                run: run_replaygain,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(ReplayGain::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ReplaygainArgs {
    /// Match albums instead of tracks
    #[arg(short = 'a', long)]
    album: bool,
    /// Change the number of threads, defaults to maximum available
    /// processors
    #[arg(short = 't', long)]
    threads: Option<usize>,
    /// Analyze all files, including those that already have ReplayGain
    /// metadata
    #[arg(short = 'f', long)]
    force: bool,
    /// Write new metadata to files' tags
    #[arg(short = 'w', long, overrides_with = "nowrite")]
    write: bool,
    /// Don't write metadata (opposite of -w)
    #[arg(short = 'W', long, overrides_with = "write")]
    nowrite: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_replaygain() -> clap::Command {
    use clap::Args as _;
    ReplaygainArgs::augment_args(
        clap::Command::new("replaygain").about("analyze for ReplayGain"),
    )
}

pub fn run_replaygain(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ReplaygainArgs::from_arg_matches(m)?;
    if !plugins.iter().any(|p| p.name() == "replaygain") {
        // It failed to load (and said why).
        anyhow::bail!("unknown command 'replaygain'");
    }
    // Set up again (it said what it had to when it loaded).
    let rg = crate::logging::capture(|| crate::plugins::replaygain::ReplayGain::new(cfg)).0?;
    let write = match (a.write, a.nowrite) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    };
    rg.command(
        cfg,
        lib,
        &crate::plugins::replaygain::CmdOpts {
            album: a.album,
            force: a.force,
            threads: a.threads,
            write,
            limit: a.limit,
            query: a.query,
        },
    )
}
