//! lyrics: fetch lyrics into the `lyrics` field, a port of beets 2.14's
//! `beetsplug/lyrics.py`.
//!
//! Sources (`sources`, in order): lrclib, google (Custom Search, only with
//! `google_API_key`), genius, tekstowo, musixmatch and lrcmux; tekstowo and
//! musixmatch are off by default. Found lyrics' source goes to
//! `lyrics_backend`/`lyrics_url`; instrumental tracks get no text and
//! `lyrics_instrumental` set. With `synced`, LRC timestamps are kept (and
//! written to MP3s' SYLT frame by the `lyrics` command); with
//! `translate.api_key` and `translate.to_language`, lines are translated
//! with Microsoft's Translator.

pub mod backends;
pub mod html;
pub mod rest;
pub mod text;
pub mod translate;

use super::requests::{HttpError, Request, Session, format_url};
use super::{Plugin, PluginCtx};
use crate::config::Config;
use crate::fileops::FileOp;
use crate::library::Library;
use crate::model::{Album, Item, Kind};
use crate::query::{Obj, Query};
use crate::template::Template;
use anyhow::Result;
use backends::{Backend, Urls};
use fancy_regex::Regex as FRegex;
use serde_json::Value as J;
use serde_yaml::Value as Y;
use std::cell::{Cell, OnceCell};
use std::path::{Path, PathBuf};
use text::Lyrics;
use translate::Translator;

// ------------------------------------------------------------ utilities

/// beets' `plugins.feat_tokens`: a pattern matching "feat."-like words
/// that separate a main artist (or title) from secondary artists.
pub fn feat_tokens(for_artist: bool) -> String {
    let mut words = vec!["ft", "featuring", "feat", "feat.", "ft."];
    if for_artist {
        words.extend(["with", "vs", "and", "con", "&"]);
    }
    let tokens: Vec<String> = words
        .iter()
        .map(|w| fancy_regex::escape(w).into_owned())
        .collect();
    let tokens = tokens.join("|");
    let bracketed = format!(r"(?:(?<=\s)|^)(?:\(|\[)(?:{tokens})(?=\s)");
    let plain = format!(r"(?<=[\s(\[])(?:{tokens})(?=\s)");
    format!("(?:{bracketed}|{plain})")
}

/// `generate_alternatives`: the string, then the first group of each
/// pattern that matches it.
fn alternatives(s: &str, patterns: &[String]) -> Vec<String> {
    let mut out = vec![s.to_string()];
    for p in patterns {
        let re = FRegex::new(&format!("(?i){p}")).unwrap();
        if let Ok(Some(c)) = re.captures(s)
            && let Some(m) = c.get(1)
        {
            out.push(m.as_str().to_string());
        }
    }
    out
}

/// beets' `search_pairs`: the (artist, titles) to search for: the item's,
/// plus the artist without featured artists, the artist sort name, and
/// the title without a parenthesized suffix, featured artists or a
/// subtitle; "A / B" titles are also tried as both songs.
pub fn search_pairs(item: &Item) -> Vec<(String, Vec<String>)> {
    let title = item.str("title").trim();
    let artist = item.str("artist").trim();
    let artist_sort = item.str("artist_sort").trim();
    if title.is_empty() || artist.is_empty() {
        return vec![];
    }
    let mut artists = vec![];
    let lower = artist.to_lowercase();
    if !lower.contains("various") {
        artists.extend(alternatives(
            artist,
            &[format!("(.*?) {}", feat_tokens(true))],
        ));
    }
    let sort_lower = artist_sort.to_lowercase();
    if !artist_sort.is_empty() && lower != sort_lower && !sort_lower.contains("various") {
        artists.push(artist_sort.to_string());
    }
    let titles = alternatives(
        title,
        &[
            r"(.+?)\s+[(].*[)]$".to_string(),
            format!("(.*?) {}", feat_tokens(false)),
            r"(.+?)\s*:.*".to_string(),
        ],
    );
    let mut multi = vec![];
    for t in titles {
        let split: Option<Vec<String>> = t
            .contains(" / ")
            .then(|| t.split(" / ").map(|x| x.trim().to_string()).collect());
        multi.push(vec![t]);
        multi.extend(split);
    }
    artists
        .into_iter()
        .flat_map(|a| multi.iter().map(move |t| (a.clone(), t.clone())))
        .collect()
}

/// beets' `slug`: an ASCII, lowercase, dash-separated version of `text`.
pub fn slug(text: &str) -> String {
    static NON_WORD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\W+").unwrap());
    let ascii = deunicode::deunicode(text).to_lowercase();
    NON_WORD
        .replace_all(ascii.trim(), "-")
        .trim_matches('-')
        .to_string()
}

/// `sanitize_choices`: known choices, without repeats, "*" standing for
/// the ones not listed.
fn sanitize_choices(choices: &[String], all: &[&str]) -> Vec<String> {
    let mut seen: Vec<&str> = vec![];
    let mut out = vec![];
    for s in choices {
        if !seen.contains(&s.as_str()) {
            if all.contains(&s.as_str()) {
                out.push(s.clone());
            } else if s == "*" {
                out.extend(
                    all.iter()
                        .filter(|a| !choices.iter().any(|c| c == *a))
                        .map(|a| a.to_string()),
                );
            }
        }
        seen.push(s);
    }
    out
}

// ---------------------------------------------------------------- config

/// The plugin's settings (beets' `lyrics:` section).
#[derive(Clone, Debug)]
pub struct Conf {
    pub auto: bool,
    pub auto_ignore: Option<String>,
    pub dist_thresh: f64,
    pub google_api_key: Option<String>,
    pub google_engine_id: String,
    pub genius_api_key: String,
    pub lrcmux_url: String,
    pub lrcmux_sources: Vec<String>,
    pub fallback: Option<String>,
    pub force: bool,
    pub keep_synced: bool,
    pub local: bool,
    pub print: bool,
    pub rest_directory: Option<String>,
    pub synced: bool,
    pub sources: Vec<String>,
    pub translator: Option<Translator>,
    pub format_item: String,
}

/// A setting that may be null (`None`).
fn opt_str(cfg: &Config, key: &str) -> Option<String> {
    match cfg.get(key)? {
        Y::Null => None,
        Y::String(s) => Some(s.clone()),
        Y::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
        Y::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Conf {
    pub fn new(cfg: &Config) -> Self {
        let translator = match (
            opt_str(cfg, "lyrics.translate.api_key").filter(|s| !s.is_empty()),
            opt_str(cfg, "lyrics.translate.to_language").filter(|s| !s.is_empty()),
        ) {
            (Some(key), Some(to)) => Some(Translator::new(
                &key,
                &to,
                &cfg.list("lyrics.translate.from_languages"),
            )),
            _ => None,
        };
        Conf {
            auto: cfg.bool("lyrics.auto"),
            auto_ignore: opt_str(cfg, "lyrics.auto_ignore").filter(|s| !s.is_empty()),
            dist_thresh: cfg.f64("lyrics.dist_thresh"),
            google_api_key: opt_str(cfg, "lyrics.google_API_key"),
            google_engine_id: cfg.str("lyrics.google_engine_ID"),
            genius_api_key: cfg.str("lyrics.genius_api_key"),
            lrcmux_url: cfg.str("lyrics.lrcmux.url"),
            lrcmux_sources: cfg.list("lyrics.lrcmux.sources"),
            fallback: opt_str(cfg, "lyrics.fallback"),
            force: cfg.bool("lyrics.force"),
            keep_synced: cfg.bool("lyrics.keep_synced"),
            local: cfg.bool("lyrics.local"),
            print: cfg.bool("lyrics.print"),
            rest_directory: opt_str(cfg, "lyrics.rest_directory"),
            synced: cfg.bool("lyrics.synced"),
            sources: cfg.list("lyrics.sources"),
            translator,
            format_item: cfg.str("format_item"),
        }
    }
}

// ---------------------------------------------------------------- plugin

/// Python's logging levels.
const DEBUG: u8 = 10;
const INFO: u8 = 20;
const WARNING: u8 = 30;
const ERROR: u8 = 40;

pub struct LyricsPlugin {
    pub conf: Conf,
    pub urls: Urls,
    session: Session,
    backends: OnceCell<Vec<Backend>>,
    /// The logger's base level: INFO for the command, WARNING in import
    /// stages (each `-v` lowers it a level).
    base_level: Cell<u8>,
}

/// The `lyrics` command's options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub print: bool,
    pub rest_directory: Option<String>,
    pub force: bool,
    /// `--keep-synced` / `--no-keep-synced`, if given.
    pub keep_synced: Option<bool>,
    pub local: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

impl LyricsPlugin {
    pub fn new(cfg: &Config) -> Self {
        Self::with_conf(Conf::new(cfg), Urls::default())
    }

    pub fn with_conf(conf: Conf, urls: Urls) -> Self {
        LyricsPlugin {
            conf,
            urls,
            session: Session::new(),
            backends: OnceCell::new(),
            base_level: Cell::new(INFO),
        }
    }

    // ---- logging (beets' plugin logger: "lyrics: " and the class name)

    fn log(&self, level: u8, msg: &str) {
        let v = crate::logging::verbosity();
        let threshold = self.base_level.get().saturating_sub(10 * v).max(DEBUG);
        if level >= threshold {
            crate::logging::emit(format!("lyrics: {msg}"));
        }
    }

    pub fn debug(&self, msg: &str) {
        self.log(DEBUG, msg);
    }

    pub fn info(&self, msg: &str) {
        self.log(INFO, msg);
    }

    pub fn error(&self, msg: &str) {
        self.log(ERROR, msg);
    }

    /// `handle_request`: a failed request is logged, not fatal.
    pub fn warn_request_error(&self, class: &str, e: &HttpError) {
        match e {
            HttpError::Json => self.error(&format!("{class}: Could not decode response JSON data")),
            e => self.error(&format!("{class}: Request error: {e}")),
        }
    }

    fn fmt_item(&self, lib: &Library, item: &Item) -> String {
        crate::commands::format_record(&Template::parse(&self.conf.format_item), item, Some(lib))
    }

    // ---- requests

    pub fn get_text(
        &self,
        b: Backend,
        url: &str,
        params: &[(&str, String)],
        force_utf8: bool,
        allow_redirects: bool,
    ) -> Result<String, HttpError> {
        let url = format_url(url, params);
        self.debug(&format!("{}: Fetching HTML from {url}", b.class()));
        let r = self.session.get(
            &url,
            Request {
                allow_redirects,
                ..Default::default()
            },
        )?;
        Ok(r.text(force_utf8))
    }

    pub fn get_json(
        &self,
        b: Backend,
        url: &str,
        params: &[(&str, String)],
        headers: &[(&str, &str)],
    ) -> Result<J, HttpError> {
        let url = format_url(url, params);
        self.debug(&format!("{}: Fetching JSON from {url}", b.class()));
        self.session
            .get(
                &url,
                Request {
                    headers,
                    allow_redirects: true,
                    ..Default::default()
                },
            )?
            .json()
    }

    // ---- lookups

    /// The configured sources, in order (google only with an API key).
    pub fn backends(&self) -> &[Backend] {
        self.backends.get_or_init(|| {
            let mut chosen = sanitize_choices(&self.conf.sources, backends::NAMES);
            let has_key = self
                .conf
                .google_api_key
                .as_deref()
                .is_some_and(|k| !k.is_empty());
            if chosen.iter().any(|c| c == "google") && !has_key {
                self.error("LyricsPlugin: Disabling Google source: no API key configured.");
                chosen.retain(|c| c != "google");
            }
            chosen
                .iter()
                .filter_map(|c| Backend::from_name(c))
                .collect()
        })
    }

    /// The first lyrics found, trying each source in turn.
    pub fn get_lyrics(
        &self,
        artist: &str,
        title: &str,
        album: &str,
        length: i64,
    ) -> Option<Lyrics> {
        self.info(&format!(
            "LyricsPlugin: Fetching lyrics for {artist} - {title}"
        ));
        for b in self.backends() {
            match b.fetch(self, artist, title, album, length) {
                Ok(Some(l)) => return Some(l),
                Ok(None) => {}
                Err(e) => self.warn_request_error(b.class(), &e),
            }
        }
        None
    }

    /// The first match for any of the item's search pairs.
    pub fn find_lyrics(&self, item: &Item) -> Option<Lyrics> {
        let album = item.str("album");
        let length = py_round(item.float("length"));
        for (artist, titles) in search_pairs(item) {
            for title in titles {
                if let Some(l) = self.get_lyrics(&artist, &title, album, length) {
                    return Some(l);
                }
            }
        }
        None
    }

    /// beets' `add_item_lyrics`: fetch and store lyrics for one item, and
    /// with `write`, write them (and synced lyrics' SYLT frame) to its
    /// file. Returns whether the item's lyrics changed.
    pub fn add_item_lyrics(&self, lib: &Library, item: &mut Item, write: bool) -> Result<bool> {
        self.add_item_lyrics_with(lib, item, write, |i| self.find_lyrics(i))
    }

    fn add_item_lyrics_with(
        &self,
        lib: &Library,
        item: &mut Item,
        write: bool,
        find: impl FnOnce(&Item) -> Option<Lyrics>,
    ) -> Result<bool> {
        if self.conf.local {
            return Ok(false);
        }
        if !self.conf.force && !item.str("lyrics").is_empty() {
            self.info(&format!(
                "LyricsPlugin: 🔵 Lyrics already present: {}",
                self.fmt_item(lib, item)
            ));
            return Ok(false);
        }
        let existing = Lyrics::from_item(item);
        if self.conf.keep_synced && existing.synced() {
            self.info(&format!(
                "LyricsPlugin: 🔵 Keeping synced lyrics: {}",
                self.fmt_item(lib, item)
            ));
            return Ok(false);
        }
        let found = find(item);
        let was_found = found.is_some();
        let (text, sylt) = match found {
            Some(mut new) => {
                self.info(&format!(
                    "LyricsPlugin: 🟢 Found lyrics: {}",
                    self.fmt_item(lib, item)
                ));
                if let Some(t) = &self.conf.translator {
                    new = t.translate(self, new, &existing);
                }
                if self.conf.synced && existing.synced() && !new.synced() {
                    self.info(&format!(
                        "LyricsPlugin: 🔴 Not updating synced lyrics with non-synced ones: {}",
                        self.fmt_item(lib, item)
                    ));
                    return Ok(false);
                }
                let instrumental = if new.instrumental { "1" } else { "0" };
                for (key, value) in [
                    ("backend", new.backend.as_deref()),
                    ("url", new.url.as_deref()),
                    ("instrumental", Some(instrumental)),
                    ("language", new.language.as_deref()),
                    ("translation_language", new.translation_language.as_deref()),
                ] {
                    let key = format!("lyrics_{key}");
                    match value {
                        Some(v) => item.set_str(&key, v),
                        None => {
                            item.remove_flex(&key);
                        }
                    }
                }
                // The LRC text (with timestamps) is kept in the database
                // and USLT, so synced lyrics are still recognized; SYLT
                // gets them too, for players that support it.
                let sylt = new.synced().then(|| new.sylt());
                (Some(new.full_text()), sylt)
            }
            None => {
                self.info(&format!(
                    "LyricsPlugin: 🔴 Lyrics not found: {}",
                    self.fmt_item(lib, item)
                ));
                (self.conf.fallback.clone(), None)
            }
        };
        match text {
            Some(text) if text != item.str("lyrics") => {
                item.set_str("lyrics", text);
                lib.store(item)?;
                if write {
                    // As beets' `try_write`: errors are logged.
                    if let Err(e) = crate::media::write_with_sylt(item, sylt.as_deref()) {
                        crate::logging::emit(format!("{e:#}"));
                    }
                }
                Ok(true)
            }
            _ => {
                // Instrumental matches (and unchanged text) update only the
                // `lyrics_*` fields.
                if was_found {
                    lib.store(item)?;
                }
                Ok(false)
            }
        }
    }

    /// beets' import stage: lyrics for the imported items, except those
    /// matching `auto_ignore`.
    fn imported(&self, ctx: &PluginCtx, items: &mut [Item]) -> Result<()> {
        let ignore = match &self.conf.auto_ignore {
            Some(q) => Some(Query::parse(Kind::Item, &crate::paths::shell_split(q))?),
            None => None,
        };
        self.base_level.set(WARNING);
        let result = (|| {
            for item in items.iter_mut() {
                if ignore
                    .as_ref()
                    .is_some_and(|q| q.matches_obj(&Obj::new(item)))
                {
                    continue;
                }
                if self.add_item_lyrics(ctx.lib, item, false)? && ctx.write {
                    // beets writes imported files after its plugin stages.
                    match crate::media::write_item(item) {
                        Ok(()) => ctx.lib.store(item)?,
                        Err(e) => crate::logging::emit(format!("{e:#}")),
                    }
                }
            }
            Ok(())
        })();
        self.base_level.set(INFO);
        result
    }
}

/// Python's `round()` to an integer (halves to even).
fn py_round(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        (r - x.signum()) as i64
    } else {
        r as i64
    }
}

impl Plugin for LyricsPlugin {
    fn name(&self) -> &'static str {
        "lyrics"
    }

    fn album_imported(
        &self,
        ctx: &PluginCtx,
        _album: &mut Album,
        items: &mut [Item],
        _source_dir: &Path,
        _op: FileOp,
    ) -> Result<()> {
        if self.conf.auto {
            self.imported(ctx, items)?;
        }
        Ok(())
    }

    fn item_imported(&self, ctx: &PluginCtx, item: &mut Item) -> Result<()> {
        if self.conf.auto {
            self.imported(ctx, std::slice::from_mut(item))?;
        }
        Ok(())
    }
}

/// The `lyrics` command.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let mut conf = Conf::new(cfg);
    // The options default to the settings.
    conf.force |= opts.force;
    conf.local |= opts.local;
    conf.print |= opts.print;
    if let Some(k) = opts.keep_synced {
        conf.keep_synced = k;
    }
    if opts.rest_directory.is_some() {
        conf.rest_directory = opts.rest_directory.clone();
    }
    let plugin = LyricsPlugin::with_conf(conf, Urls::default());
    run_command(&plugin, lib, cfg.bool("import.write"), &opts)
}

fn run_command(p: &LyricsPlugin, lib: &Library, write: bool, opts: &CmdOpts) -> Result<()> {
    let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
    items.truncate(opts.limit.unwrap_or(usize::MAX));
    for item in items.iter_mut() {
        p.add_item_lyrics(lib, item, write)?;
        if p.conf.print && !item.str("lyrics").is_empty() {
            println!("{}", item.str("lyrics"));
        }
    }
    if let Some(dir) = p.conf.rest_directory.as_deref().filter(|d| !d.is_empty()) {
        let with_lyrics: Vec<Item> = items
            .into_iter()
            .filter(|i| !i.str("lyrics").is_empty())
            .collect();
        if !with_lyrics.is_empty() {
            rest::RestFiles {
                directory: expanduser(dir),
            }
            .write(&with_lyrics)?;
        }
    }
    Ok(())
}

/// `Path(p).expanduser()`.
fn expanduser(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => dirs::home_dir()
            .unwrap_or_default()
            .join(rest.trim_start_matches('/')),
        _ => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests;
