//! A port of beets 2.14's `chroma` plugin (`beetsplug/chroma.py`) and the
//! parts of pyacoustid 1.3 it uses: AcoustID fingerprints from `fpcalc`,
//! the AcoustID lookup and submission API, and Chromaprint fingerprint
//! comparison (`chromasearch`).
//!
//! As in beets, the plugin is a metadata source: with `auto`, each import
//! task's files are fingerprinted and looked up when the task starts
//! (`import_task_start`); the recordings and releases found become extra
//! album and item candidates, resolved through the MusicBrainz source (and
//! only while the `musicbrainz` plugin is enabled); applying a match stores
//! `acoustid_fingerprint` and `acoustid_id` on the items
//! (`import_task_apply`). beets 2.14 no longer calls metadata sources'
//! `track_distance`, so chroma's `track_id` penalty isn't applied there
//! either; being a second metadata source does change the `data_source`
//! penalty (see `autotag::Ctx`).

use super::{ImportTask, Plugin, PluginCtx, PluginLog, WARNING};
use crate::config::Config;
use crate::hooks::{py_float, py_repr};
use crate::library::Library;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo};
use crate::model::{Item, Kind};
use crate::query::Query;
use crate::template::Template;
use anyhow::{Result, bail};
use serde_json::Value as J;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// beets' AcoustID application key.
pub const API_KEY: &str = "1vOwZtEn";
const SCORE_THRESH: f64 = 0.5;
/// How many tracks must have an album in common.
const COMMON_REL_THRESH: f64 = 0.6;
const MAX_RECORDINGS: usize = 5;
const MAX_RELEASES: usize = 5;

/// pyacoustid's `API_BASE_URL`. `BEETSRS_ACOUSTID_API_URL` points it
/// elsewhere (a fake server, for testing).
const API_BASE_URL: &str = "http://api.acoustid.org/v2/";
/// pyacoustid's `MAX_AUDIO_LENGTH`: how much audio `fpcalc` reads.
const MAX_AUDIO_LENGTH: u32 = 120;
/// pyacoustid's `REQUEST_INTERVAL` (3 requests a second).
#[cfg(not(test))]
const REQUEST_INTERVAL: Duration = Duration::from_millis(330);
#[cfg(test)]
const REQUEST_INTERVAL: Duration = Duration::ZERO;
const TIMEOUT: Duration = Duration::from_secs(10);
/// pyacoustid's comparison settings.
const MAX_BIT_ERROR: u32 = 2;
const MAX_ALIGN_OFFSET: usize = 120;

/// What lookups found, by audio file path, as beets' module-level
/// `_matches` (recording IDs, release IDs), `_fingerprints` and
/// `_acoustids`. Filled when an import task starts, read by the candidate
/// searches and when a match is applied.
#[derive(Default)]
struct State {
    matches: HashMap<PathBuf, (Vec<String>, Vec<String>)>,
    fingerprints: HashMap<PathBuf, String>,
    acoustids: HashMap<PathBuf, String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// Tests' fake AcoustID server.
#[cfg(test)]
static TEST_API_BASE: Mutex<Option<String>> = Mutex::new(None);

fn api_base() -> String {
    #[cfg(test)]
    if let Some(b) = TEST_API_BASE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return b;
    }
    std::env::var("BEETSRS_ACOUSTID_API_URL").unwrap_or_else(|_| API_BASE_URL.to_string())
}

/// Python's `repr()` of a bytes path (`b'/a/b.mp3'`).
fn bytes_repr(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let b = p.as_os_str().as_bytes();
    let q = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::from("b");
    out.push(q as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(q as char);
    out
}

/// Python's `repr()` of a list of strings.
fn list_repr(v: &[String]) -> String {
    let parts: Vec<String> = v.iter().map(|s| py_repr(s)).collect();
    format!("[{}]", parts.join(", "))
}

// ------------------------------------------------------------ fingerprints

/// pyacoustid's `fingerprint_file(path, force_fpcalc=True)`: run `fpcalc`
/// (or `$FPCALC`) on the file; returns the duration and the (compressed,
/// base64) fingerprint, or pyacoustid's error message.
pub fn fingerprint_file(path: &Path) -> Result<(f64, String), String> {
    let fpcalc = std::env::var_os("FPCALC").unwrap_or_else(|| "fpcalc".into());
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let out = std::process::Command::new(fpcalc)
        .arg("-length")
        .arg(MAX_AUDIO_LENGTH.to_string())
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("fpcalc not found".into());
        }
        Err(e) => return Err(format!("fpcalc invocation failed: {e}")),
    };
    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        return Err(format!("fpcalc exited with status {code}"));
    }
    let (mut duration, mut fp) = (None, None);
    for line in out.stdout.split(|&b| b == b'\n' || b == b'\r') {
        let mut parts = line.splitn(2, |&b| b == b'=');
        let key = parts.next().unwrap_or_default();
        let value = parts.next();
        match (key, value) {
            (b"DURATION", Some(v)) => {
                duration = Some(
                    String::from_utf8_lossy(v)
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| "fpcalc duration not numeric".to_string())?,
                );
            }
            (b"FINGERPRINT", Some(v)) => fp = Some(String::from_utf8_lossy(v).into_owned()),
            _ => {}
        }
    }
    match (duration, fp) {
        (Some(d), Some(f)) => Ok((d, f)),
        _ => Err("missing fpcalc output".into()),
    }
}

// --------------------------------------------------------------- web API

/// pyacoustid's `_api_request`: POST the form parameters and parse the
/// JSON response (whatever the HTTP status), rate-limited to three
/// requests a second. Errors are pyacoustid's `WebServiceError` messages.
fn api_request(url: &str, params: &[(&str, String)]) -> Result<J, String> {
    {
        let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < REQUEST_INTERVAL {
                std::thread::sleep(REQUEST_INTERVAL - elapsed);
            }
        }
        *last = Some(Instant::now());
    }
    let agent = ureq::AgentBuilder::new()
        .user_agent(super::USER_AGENT)
        .timeout(TIMEOUT)
        .build();
    let body = super::requests::urlencode(params);
    let resp = agent
        .post(url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body);
    let resp = match resp {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => return Err(format!("HTTP request failed: {t}")),
    };
    let text = resp
        .into_string()
        .map_err(|e| format!("HTTP request failed: {e}"))?;
    serde_json::from_str(&text).map_err(|_| "response is not valid JSON".to_string())
}

/// pyacoustid's `lookup` with `meta="recordings releases"`.
fn lookup(fp: &str, duration: f64) -> Result<J, String> {
    api_request(
        &format!("{}lookup", api_base()),
        &[
            ("format", "json".into()),
            ("client", API_KEY.into()),
            ("duration", (duration as i64).to_string()),
            ("fingerprint", fp.into()),
            ("meta", "recordings releases".into()),
        ],
    )
}

/// pyacoustid's `submit`.
fn submit(userkey: &str, data: &[Vec<(&'static str, String)>]) -> Result<J, String> {
    let mut args: Vec<(String, String)> = vec![
        ("format".into(), "json".into()),
        ("client".into(), API_KEY.into()),
        ("user".into(), userkey.into()),
    ];
    for (i, d) in data.iter().enumerate() {
        for (k, v) in d {
            args.push((format!("{k}.{i}"), v.clone()));
        }
    }
    let params: Vec<(&str, String)> = args.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let response = api_request(&format!("{}submit", api_base()), &params)?;
    if response.get("status").and_then(J::as_str) != Some("ok") {
        let err = response.get("error");
        return match (
            err.and_then(|e| e.get("code")),
            err.and_then(|e| e.get("message")),
        ) {
            (Some(code), Some(message)) => {
                Err(format!("error {}: {}", py_json(code), py_json(message)))
            }
            _ => Err(format!("response: {}", py_json(&response))),
        };
    }
    Ok(response)
}

/// A JSON value as Python's `str()` shows it once parsed.
fn py_json(v: &J) -> String {
    match v {
        J::Null => "None".into(),
        J::Bool(b) => if *b { "True" } else { "False" }.into(),
        J::Number(n) => n.to_string(),
        J::String(s) => s.clone(),
        J::Array(a) => format!(
            "[{}]",
            a.iter().map(py_json_repr).collect::<Vec<_>>().join(", ")
        ),
        J::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(k), py_json_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_json_repr(v: &J) -> String {
    match v {
        J::String(s) => py_repr(s),
        v => py_json(v),
    }
}

// ------------------------------------------------------------- matching

/// beets' `releases_key`: sort releases by date (with
/// `match.preferred.original_year`), then by preferred country.
fn releases_key(
    release: &J,
    countries: &[regex::Regex],
    original_year: bool,
) -> (i64, i64, i64, usize) {
    let date = release
        .get("date")
        .filter(|d| d.as_object().is_some_and(|o| !o.is_empty()));
    let (year, month, day) = match date {
        Some(d) if original_year => {
            let part = |k: &str, def: i64| d.get(k).and_then(J::as_i64).unwrap_or(def);
            (part("year", 9999), part("month", 99), part("day", 99))
        }
        _ => (9999, 99, 99),
    };
    let mut country_key = 99;
    if let Some(c) = release
        .get("country")
        .and_then(J::as_str)
        .filter(|c| !c.is_empty())
        && let Some(i) = countries.iter().position(|p| p.is_match(c))
    {
        country_key = i;
    }
    (year, month, day, country_key)
}

/// beets' `acoustid_match`: fingerprint the file and look it up, recording
/// what was found.
fn acoustid_match(cfg: &Config, log: &PluginLog, path: &Path) {
    let (duration, fp) = match fingerprint_file(path) {
        Ok(r) => r,
        Err(e) => {
            log.error(&format!(
                "fingerprinting of {} failed: {e}",
                bytes_repr(path)
            ));
            return;
        }
    };
    state().fingerprints.insert(path.to_path_buf(), fp.clone());
    let res = match lookup(&fp, duration) {
        Ok(r) => r,
        Err(e) => {
            log.debug(&format!(
                "fingerprint matching {} failed: {e}",
                bytes_repr(path)
            ));
            return;
        }
    };
    log.debug(&format!("chroma: fingerprinted {}", bytes_repr(path)));

    let results = res.get("results").and_then(J::as_array);
    if res.get("status").and_then(J::as_str) != Some("ok") || results.is_none_or(|r| r.is_empty()) {
        log.debug("no match found");
        return;
    }
    let result = &results.unwrap()[0];
    if result.get("score").and_then(J::as_f64).unwrap_or(0.0) < SCORE_THRESH {
        log.debug("no results above threshold");
        return;
    }
    if let Some(id) = result.get("id").and_then(J::as_str) {
        state().acoustids.insert(path.to_path_buf(), id.to_string());
    }
    let recordings = match result.get("recordings").and_then(J::as_array) {
        Some(r) if !r.is_empty() => r,
        _ => {
            log.debug("no recordings found");
            return;
        }
    };
    let mut recording_ids = vec![];
    let mut releases: Vec<&J> = vec![];
    for recording in recordings {
        if let Some(id) = recording.get("id").and_then(J::as_str) {
            recording_ids.push(id.to_string());
        }
        if let Some(rels) = recording.get("releases").and_then(J::as_array) {
            releases.extend(rels);
        }
    }
    // Lookup results come in no particular order: sort them by the
    // `match.preferred` options (a stable sort, as Python's).
    let countries: Vec<regex::Regex> = cfg
        .list("match.preferred.countries")
        .iter()
        .filter_map(|p| regex::Regex::new(&format!("(?i)^(?:{p})")).ok())
        .collect();
    let original_year = cfg.bool("match.preferred.original_year");
    releases.sort_by_key(|r| releases_key(r, &countries, original_year));
    let release_ids: Vec<String> = releases
        .iter()
        .filter_map(|r| r.get("id").and_then(J::as_str).map(String::from))
        .collect();
    log.debug(&format!(
        "matched recordings {} on releases {}",
        list_repr(&recording_ids),
        list_repr(&release_ids)
    ));
    state()
        .matches
        .insert(path.to_path_buf(), (recording_ids, release_ids));
}

/// Whether chroma fingerprints import tasks as they start (`auto`).
pub fn auto(cfg: &Config) -> bool {
    super::enabled(cfg, "chroma") && cfg.bool("chroma.auto")
}

/// beets' `fingerprint_task`, the `import_task_start` listener (with
/// `auto`): fingerprint and look up each of the task's items, for the
/// candidate searches that follow.
pub fn fingerprint_task(cfg: &Config, items: &[Item]) {
    if !auto(cfg) {
        return;
    }
    let log = PluginLog::new("chroma");
    log.set_base_level(WARNING);
    for item in items {
        acoustid_match(cfg, &log, item.path());
    }
}

/// beets' `_all_releases`: the releases more than `COMMON_REL_THRESH` of
/// the items were found on, in the order first seen.
fn all_releases(items: &[Item]) -> Vec<String> {
    let st = state();
    let mut counts: Vec<(String, usize)> = vec![];
    for item in items {
        let Some((_, release_ids)) = st.matches.get(item.path()) else {
            continue;
        };
        for id in release_ids {
            match counts.iter_mut().find(|(r, _)| r == id) {
                Some((_, n)) => *n += 1,
                None => counts.push((id.clone(), 1)),
            }
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n as f64 / items.len() as f64 > COMMON_REL_THRESH)
        .map(|(r, _)| r)
        .collect()
}

/// The plugin's `candidates`: the releases the items share, looked up on
/// MusicBrainz. A lookup error drops them all, as beets' plugin error
/// handling does (`Error in 'Acoustid.candidates'`).
pub fn album_candidates(cfg: &Config, mb: &MusicBrainz, items: &[Item]) -> Vec<AlbumInfo> {
    if !super::enabled(cfg, "chroma") {
        return vec![];
    }
    let log = PluginLog::new("chroma");
    log.set_base_level(WARNING);
    if !super::enabled(cfg, "musicbrainz") {
        log.debug("musicbrainz plugin not enabled; acoustid matches will not produce candidates");
        return vec![];
    }
    let mut albums = vec![];
    for relid in all_releases(items).iter().take(MAX_RELEASES) {
        match mb.album_for_id(relid) {
            Ok(Some(a)) => albums.push(a),
            Ok(None) => {}
            Err(e) => {
                crate::logging::emit(format!("Error in 'Acoustid.candidates': {e:#}"));
                return vec![];
            }
        }
    }
    log.debug(&format!("acoustid album candidates: {}", albums.len()));
    albums
}

/// The plugin's `item_candidates`: the item's recordings, looked up on
/// MusicBrainz.
pub fn item_candidates(cfg: &Config, mb: &MusicBrainz, item: &Item) -> Vec<TrackInfo> {
    if !super::enabled(cfg, "chroma") {
        return vec![];
    }
    let Some(recording_ids) = state().matches.get(item.path()).map(|m| m.0.clone()) else {
        return vec![];
    };
    let log = PluginLog::new("chroma");
    log.set_base_level(WARNING);
    if !super::enabled(cfg, "musicbrainz") {
        log.debug("musicbrainz plugin not enabled; acoustid matches will not produce candidates");
        return vec![];
    }
    let mut tracks = vec![];
    for id in recording_ids.iter().take(MAX_RECORDINGS) {
        match mb.track_for_id(id) {
            Ok(Some(t)) => tracks.push(t),
            Ok(None) => {}
            Err(e) => {
                crate::logging::emit(format!("Error in 'Acoustid.item_candidates': {e:#}"));
                return vec![];
            }
        }
    }
    log.debug(&format!("acoustid item candidates: {}", tracks.len()));
    tracks
}

// ------------------------------------------------------------- the plugin

pub struct Chroma;

impl Plugin for Chroma {
    fn name(&self) -> &'static str {
        "chroma"
    }

    /// beets' `apply_acoustid_metadata`.
    fn import_task_apply(&self, _ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        let st = state();
        for item in task.items.iter_mut() {
            if let Some(fp) = st.fingerprints.get(item.path()) {
                item.set_str("acoustid_fingerprint", fp.clone());
            }
            if let Some(id) = st.acoustids.get(item.path()) {
                item.set_str("acoustid_id", id.clone());
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- commands

/// beets' `item.filepath`.
fn filepath(item: &Item) -> String {
    item.path().display().to_string()
}

/// beets' `fingerprint_item`: the item's fingerprint, computed (and
/// stored, and with `write` written to the file) if it has none. None if
/// it has no length or fingerprinting fails.
fn fingerprint_item(
    log: &PluginLog,
    lib: Option<&Library>,
    item: &mut Item,
    write: bool,
    quiet: bool,
) -> Option<String> {
    if item.float("length") == 0.0 {
        log.info(&format!("{}: no duration available", filepath(item)));
        return None;
    }
    let existing = item.str("acoustid_fingerprint");
    if !existing.is_empty() {
        if !quiet {
            if write {
                log.info(&format!("{}: fingerprint exists, skipping", filepath(item)));
            } else {
                log.info(&format!("{}: using existing fingerprint", filepath(item)));
            }
        }
        return Some(existing.to_string());
    }
    log.info(&format!("{}: fingerprinting", filepath(item)));
    let fp = match fingerprint_file(item.path()) {
        Ok((_, fp)) => fp,
        Err(e) => {
            log.info(&format!("fingerprint generation failed: {e}"));
            return None;
        }
    };
    item.set_str("acoustid_fingerprint", fp.clone());
    if write {
        log.info(&format!("{}: writing fingerprint", filepath(item)));
        // beets' `try_write`: a failure is logged, not fatal.
        if let Err(e) = crate::media::write_item(item) {
            crate::logging::emit(format!("{e:#}"));
        }
    }
    if let Some(lib) = lib
        && let Err(e) = lib.store(item)
    {
        crate::logging::emit(format!("{e:#}"));
    }
    Some(fp)
}

fn query_items(lib: &Library, q: &[String], limit: Option<usize>) -> Result<Vec<Item>> {
    let mut v = lib.items(&Query::parse(Kind::Item, q)?)?;
    v.truncate(limit.unwrap_or(usize::MAX));
    Ok(v)
}

/// beets' `ui.should_write()`.
fn should_write(cfg: &Config, opt: bool) -> bool {
    opt || cfg.bool("import.write")
}

/// `beet fingerprint`: compute fingerprints for items without them.
pub fn fingerprint_command(
    cfg: &Config,
    lib: &Library,
    limit: Option<usize>,
    q: &[String],
) -> Result<()> {
    let log = PluginLog::new("chroma");
    let write = should_write(cfg, false);
    for mut item in query_items(lib, q, limit)? {
        fingerprint_item(&log, Some(lib), &mut item, write, false);
    }
    Ok(())
}

/// `beet submit`: submit the items' fingerprints to AcoustID with the
/// user's `acoustid.apikey`.
pub fn submit_command(
    cfg: &Config,
    lib: &Library,
    limit: Option<usize>,
    q: &[String],
) -> Result<()> {
    let Some(apikey) = cfg
        .get("acoustid.apikey")
        .map(|_| cfg.str("acoustid.apikey"))
    else {
        bail!("no Acoustid user API key provided");
    };
    let log = PluginLog::new("chroma");
    let items = query_items(lib, q, limit)?;
    submit_items(&log, cfg, Some(lib), &apikey, items, 64);
    Ok(())
}

/// beets' `submit_items`: fingerprint the items and submit them in chunks.
fn submit_items(
    log: &PluginLog,
    cfg: &Config,
    lib: Option<&Library>,
    userkey: &str,
    items: Vec<Item>,
    chunksize: usize,
) {
    let mut data: Vec<Vec<(&'static str, String)>> = vec![];
    let submit_chunk = |data: &mut Vec<Vec<(&'static str, String)>>| {
        log.info(&format!("submitting {} fingerprints", data.len()));
        if let Err(e) = submit(userkey, data) {
            log.warning(&format!("acoustid submission error: {e}"));
        }
        data.clear();
    };
    let write = should_write(cfg, false);
    for mut item in items {
        let fp = fingerprint_item(log, lib, &mut item, write, false);
        // As requests' form encoding, a None value (no fingerprint) is
        // left out.
        let mut d: Vec<(&'static str, String)> =
            vec![("duration", (item.float("length") as i64).to_string())];
        if let Some(fp) = fp {
            d.push(("fingerprint", fp));
        }
        let mbid = item.str("mb_trackid");
        if !mbid.is_empty() {
            d.push(("mbid", mbid.to_string()));
            log.debug("submitting MBID");
        } else {
            d.extend([
                ("track", item.str("title").to_string()),
                ("artist", item.str("artist").to_string()),
                ("album", item.str("album").to_string()),
                ("albumartist", item.str("albumartist").to_string()),
                ("year", item.int("year").to_string()),
                ("trackno", item.int("track").to_string()),
                ("discno", item.int("disc").to_string()),
            ]);
            log.debug("submitting textual metadata");
        }
        data.push(d);
        if data.len() >= chunksize {
            submit_chunk(&mut data);
        }
    }
    if !data.is_empty() {
        submit_chunk(&mut data);
    }
}

/// `beet chromasearch`'s options.
#[derive(Debug, Default)]
pub struct SearchOpts {
    pub search: Option<String>,
    pub count: i64,
    pub full: bool,
    pub write: bool,
    pub path: bool,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// A fingerprint comparison (pyacoustid's `compare_fingerprints`, or a
/// stand-in in tests).
type Compare = dyn Fn(&str, &str) -> Result<f64>;

/// `beet chromasearch`: the library items whose fingerprints best match
/// the one given.
pub fn chromasearch_command(cfg: &Config, lib: &Library, opts: SearchOpts) -> Result<()> {
    let out = chromasearch(cfg, lib, opts, &|a, b| compare_fingerprints(a, b))?;
    for line in out {
        println!("{line}");
    }
    Ok(())
}

fn chromasearch(
    cfg: &Config,
    lib: &Library,
    opts: SearchOpts,
    compare: &Compare,
) -> Result<Vec<String>> {
    let Some(target) = opts.search.filter(|s| !s.is_empty()) else {
        bail!("no --search provided");
    };
    if opts.count <= 0 {
        bail!("--count must be > 0");
    }
    let fmt = match (&opts.format, opts.path) {
        (Some(f), _) => f.clone(),
        (None, true) => "$path".into(),
        (None, false) => cfg.str("format_item"),
    };
    let t = Template::parse(&fmt);
    let show = |item: &Item| crate::commands::format_record(&t, item, Some(lib));
    let log = PluginLog::new("chroma");
    let write = should_write(cfg, opts.write);
    let mut top = TopN::new(opts.count as usize);
    for mut item in query_items(lib, &opts.query, opts.limit)? {
        let Some(fp) = fingerprint_item(&log, Some(lib), &mut item, write, true) else {
            log.warning(&format!("{}: could not compute fingerprint", show(&item)));
            continue;
        };
        let score = compare(&target, &fp)?;
        if score == 1.0 && !opts.full {
            return Ok(vec![format!(
                "{}: {}",
                crate::ui::colorize("text_success", "Found exact match"),
                show(&item)
            )]);
        }
        if score > 0.0 {
            top.add(score, show(&item));
        }
    }
    Ok(top
        .sorted()
        .into_iter()
        .map(|(score, shown)| scored_item(score, &shown))
        .collect())
}

/// beets' `ScoredItem.__str__`.
fn scored_item(score: f64, shown: &str) -> String {
    let rounded: f64 = format!("{:.2}", score * 100.0).parse().unwrap_or(0.0);
    let percent = format!("{:>6}", format!("{}%", py_float(rounded)));
    let color = if score >= 0.95 {
        "text_success"
    } else if score >= 0.85 {
        "text_warning"
    } else {
        "text_error"
    };
    format!("[{}] {shown}", crate::ui::colorize(color, &percent))
}

/// beets' `TopN`: the `n` best scores, kept in a heap as Python's `heapq`
/// keeps it (so that ties come out in the same order).
struct TopN {
    n: usize,
    heap: Vec<(f64, String)>,
}

impl TopN {
    fn new(n: usize) -> Self {
        TopN { n, heap: vec![] }
    }

    fn add(&mut self, score: f64, shown: String) {
        if self.heap.len() < self.n {
            self.heap.push((score, shown));
            let last = self.heap.len() - 1;
            self.sift_down(0, last);
        } else if score > self.heap[0].0 {
            self.heap[0] = (score, shown);
            self.sift_up(0);
        }
    }

    /// `heapq._siftdown`.
    fn sift_down(&mut self, start: usize, mut pos: usize) {
        let new = self.heap[pos].clone();
        while pos > start {
            let parent = (pos - 1) >> 1;
            if new.0 < self.heap[parent].0 {
                self.heap[pos] = self.heap[parent].clone();
                pos = parent;
                continue;
            }
            break;
        }
        self.heap[pos] = new;
    }

    /// `heapq._siftup`.
    fn sift_up(&mut self, mut pos: usize) {
        let end = self.heap.len();
        let start = pos;
        let new = self.heap[pos].clone();
        let mut child = 2 * pos + 1;
        while child < end {
            let right = child + 1;
            if right < end && self.heap[child].0 >= self.heap[right].0 {
                child = right;
            }
            self.heap[pos] = self.heap[child].clone();
            pos = child;
            child = 2 * pos + 1;
        }
        self.heap[pos] = new;
        self.sift_down(start, pos);
    }

    /// `sorted(heap, reverse=True)`: stable, best first.
    fn sorted(mut self) -> Vec<(f64, String)> {
        self.heap.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.heap
    }
}

// ------------------------------------------------- fingerprint comparison

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for &c in s.trim().as_bytes() {
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `width`-bit values packed least significant bit first.
fn unpack(bytes: &[u8], width: usize) -> Vec<u32> {
    let n = bytes.len() * 8 / width;
    (0..n)
        .map(|k| {
            (0..width).fold(0, |v, b| {
                let bit = k * width + b;
                v | ((((bytes[bit / 8] >> (bit % 8)) & 1) as u32) << b)
            })
        })
        .collect()
}

/// Chromaprint's `chromaprint_decode_fingerprint` (base64, then
/// `FingerprintDecompressor`): the raw 32-bit sub-fingerprints.
pub fn decode_fingerprint(data: &str) -> Option<Vec<u32>> {
    let input = base64_decode(data)?;
    if input.len() < 4 {
        return None;
    }
    let num_values = ((input[1] as usize) << 16) | ((input[2] as usize) << 8) | input[3] as usize;
    let mut bits = unpack(&input[4..], 3);
    let (mut found, mut exceptional) = (0usize, 0usize);
    let mut end = None;
    for (i, &b) in bits.iter().enumerate() {
        if b == 0 {
            found += 1;
            if found == num_values {
                end = Some(i + 1);
                break;
            }
        } else if b == 7 {
            exceptional += 1;
        }
    }
    if found != num_values {
        return None;
    }
    bits.truncate(end.unwrap_or(0));
    let offset = 4 + (bits.len() * 3).div_ceil(8);
    let ex_size = (exceptional * 5).div_ceil(8);
    if input.len() < offset + ex_size {
        return None;
    }
    if exceptional > 0 {
        let ex = unpack(&input[offset..offset + ex_size], 5);
        let mut j = 0;
        for b in bits.iter_mut() {
            if *b == 7 {
                *b += ex[j];
                j += 1;
            }
        }
    }
    let mut out = Vec::with_capacity(num_values);
    let (mut value, mut last_bit) = (0u32, 0u32);
    for b in bits {
        if b == 0 {
            let prev = out.last().copied().unwrap_or(0);
            out.push(value ^ prev);
            value = 0;
            last_bit = 0;
            continue;
        }
        last_bit += b;
        if last_bit <= 32 {
            value |= 1 << (last_bit - 1);
        }
    }
    Some(out)
}

/// pyacoustid's `_match_fingerprints`.
fn match_fingerprints(a: &[u32], b: &[u32]) -> f64 {
    let (asize, bsize) = (a.len(), b.len());
    if asize == 0 || bsize == 0 {
        return 0.0;
    }
    let mut counts = vec![0u32; asize + bsize + 1];
    for i in 0..asize {
        let jbegin = i.saturating_sub(MAX_ALIGN_OFFSET);
        let jend = bsize.min(i + MAX_ALIGN_OFFSET);
        for j in jbegin..jend {
            if (a[i] ^ b[j]).count_ones() <= MAX_BIT_ERROR {
                counts[i + bsize - j] += 1;
            }
        }
    }
    let top = counts.into_iter().max().unwrap_or(0);
    top as f64 / asize.min(bsize) as f64
}

/// pyacoustid's `compare_fingerprints`: a similarity score from 0 to 1.
pub fn compare_fingerprints(a: &str, b: &str) -> Result<f64> {
    let decode = |s: &str| match decode_fingerprint(s) {
        Some(v) => Ok(v),
        None => bail!("fingerprint decoding failed"),
    };
    Ok(match_fingerprints(&decode(a)?, &decode(b)?))
}

#[cfg(test)]
#[path = "chroma_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "chroma",
        order: 1,
        defaults: r#"chroma:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    auto: yes
"#,
        config_keys: &["chroma"],
        redacted: &["acoustid.apikey"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 290,
                plugin: "chroma",
                build: build_submit,
                run: run_submit,
            },
            crate::plugin_registry::PluginCommand {
                order: 300,
                plugin: "chroma",
                build: build_fingerprint,
                run: run_fingerprint,
            },
            crate::plugin_registry::PluginCommand {
                order: 310,
                plugin: "chroma",
                build: build_chromasearch,
                run: run_chromasearch,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Chroma))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct SubmitArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_submit() -> clap::Command {
    use clap::Args as _;
    SubmitArgs::augment_args(
        clap::Command::new("submit").about("Submit Acoustid fingerprints (plugin: chroma)"),
    )
}

pub fn run_submit(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = SubmitArgs::from_arg_matches(m)?;
    crate::plugins::chroma::submit_command(cfg, lib, a.limit, &a.query)
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct FingerprintArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_fingerprint() -> clap::Command {
    use clap::Args as _;
    FingerprintArgs::augment_args(
        clap::Command::new("fingerprint")
            .about("Generate fingerprints for items without them (plugin: chroma)"),
    )
}

pub fn run_fingerprint(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = FingerprintArgs::from_arg_matches(m)?;
    crate::plugins::chroma::fingerprint_command(cfg, lib, a.limit, &a.query)
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ChromasearchArgs {
    /// Print paths for matched items or albums
    #[arg(short = 'p', long)]
    path: bool,
    /// Print with custom format
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Fingerprint to search for (from the output of fpcalc -plain)
    #[arg(short = 's', long)]
    search: Option<String>,
    /// Number of items in result
    #[arg(short = 'c', long, default_value_t = 5, allow_negative_numbers = true)]
    count: i64,
    /// Don't stop searching once we found an exact match
    #[arg(long)]
    full: bool,
    /// Write computed fingerprints to files
    #[arg(short = 'w', long)]
    write: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_chromasearch() -> clap::Command {
    use clap::Args as _;
    ChromasearchArgs::augment_args(
        clap::Command::new("chromasearch")
            .about("Search local database by chroma fingerprint (plugin: chroma)"),
    )
}

pub fn run_chromasearch(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ChromasearchArgs::from_arg_matches(m)?;
    crate::plugins::chroma::chromasearch_command(
        cfg,
        lib,
        crate::plugins::chroma::SearchOpts {
            search: a.search,
            count: a.count,
            full: a.full,
            write: a.write,
            path: a.path,
            format: a.format,
            limit: a.limit,
            query: a.query,
        },
    )
}
