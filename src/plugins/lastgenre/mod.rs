//! lastgenre: genres from Last.fm tags, a port of beets 2.14's
//! `beetsplug/lastgenre` package.
//!
//! Tags are fetched for the track, album or artist (`source`), normalized
//! with an alias table (`aliases`), filtered by a whitelist (`whitelist`)
//! and an ignorelist (`ignorelist`), optionally walked up a genre tree
//! (`canonical`, `prefer_specific`), and the first `count` of them stored
//! in the multi-valued `genres` field.
//!
//! The bundled `genres.txt` (the default whitelist), `genres-tree.yaml`
//! (the default canonicalization tree) and `aliases.yaml` are beets' own
//! files, unchanged:
//!
//! > The MIT License. Copyright (c) 2010-2016 Adrian Sampson.
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining
//! > a copy of this software and associated documentation files (the
//! > "Software"), to deal in the Software without restriction, including
//! > without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject to
//! > the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be
//! > included in all copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
//! > EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! > MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
//! > NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
//! > LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! > OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
//! > WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
//!
//! The genre list was originally produced by scraping Wikipedia
//! (<https://gist.github.com/1241307>) and edited by beets' authors.

pub mod client;

use super::{DEBUG, ImportTask, Plugin, PluginCtx, PluginLog, WARNING};
use crate::config::{Config, ConfigError};
use crate::library::Library;
use crate::model::{Album, Item, Kind as ModelKind, Record};
use crate::query::Query;
use crate::template::Template;
use anyhow::{Result, anyhow, bail};
use client::{Kind, LastFmClient, py_list};
use fancy_regex::Regex;
use serde_yaml::Value as Y;
use std::collections::{HashMap, HashSet};

pub const GENRES_TXT: &str = include_str!("genres.txt");
pub const GENRES_TREE_YAML: &str = include_str!("genres-tree.yaml");
pub const ALIASES_YAML: &str = include_str!("aliases.yaml");

const NAME: &str = "lastgenre";

// ------------------------------------------------------------ utilities

/// A compiled pattern that remembers its source (Python's `re.Pattern`
/// with `.pattern`), matched case-insensitively against whole strings.
#[derive(Clone, Debug)]
pub struct Pat {
    pub pattern: String,
    re: Regex,
}

impl Pat {
    /// `re.compile(pattern, re.IGNORECASE)`.
    pub fn new(pattern: &str) -> Result<Pat, String> {
        let re = Regex::new(&format!(r"(?i)\A(?:{pattern})\z")).map_err(|e| e.to_string())?;
        Ok(Pat {
            pattern: pattern.to_string(),
            re,
        })
    }

    pub fn fullmatch(&self, s: &str) -> bool {
        self.re.is_match(s).unwrap_or(false)
    }
}

/// Mapping of lowercase artist name (or `*`) to ignored-genre patterns.
pub type IgnorePatterns = HashMap<String, Vec<Pat>>;
/// Alias patterns with their replacement templates.
pub type AliasPatterns = Vec<(Pat, String)>;

/// Python's `re.escape`.
pub fn py_re_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Log at DEBUG only with `-vvv` (beets' `extra_debug`).
pub fn extra_debug(log: &PluginLog, msg: &str) {
    if crate::logging::verbosity() >= 3 {
        log.log(DEBUG, msg);
    }
}

/// beets' `is_ignored`: whether the genre matches a global (`*`) or the
/// artist's ignorelist pattern.
pub fn is_ignored(log: &PluginLog, patterns: &IgnorePatterns, genre: &str, artist: &str) -> bool {
    let lower = genre.to_lowercase();
    for p in patterns.get("*").into_iter().flatten() {
        if p.fullmatch(&lower) {
            extra_debug(log, &format!("ignored (global): {genre}"));
            return true;
        }
    }
    for p in patterns.get(&artist.to_lowercase()).into_iter().flatten() {
        if p.fullmatch(&lower) {
            extra_debug(log, &format!("ignored (artist: {artist}): {genre}"));
            return true;
        }
    }
    false
}

/// Python's `Match.expand(template)`.
fn expand(
    m: &fancy_regex::Captures<'_, str>,
    groups: usize,
    template: &str,
) -> Result<String, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let group = |n: usize, pos: usize| -> Result<String, String> {
        if n > groups {
            return Err(format!("invalid group reference {n} at position {pos}"));
        }
        Ok(m.get(n).map(|g| g.as_str().to_string()).unwrap_or_default())
    };
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        let Some(&next) = chars.get(i + 1) else {
            return Err(format!("bad escape (end of pattern) at position {start}"));
        };
        match next {
            'g' => {
                if chars.get(i + 2) != Some(&'<') {
                    return Err(format!("missing < at position {}", i + 2));
                }
                let Some(close) = chars[i + 3..].iter().position(|&c| c == '>') else {
                    return Err(format!(
                        "missing >, unterminated name at position {}",
                        i + 3
                    ));
                };
                let name: String = chars[i + 3..i + 3 + close].iter().collect();
                if name.is_empty() {
                    return Err(format!("missing group name at position {}", i + 3));
                }
                if name.chars().all(|c| c.is_ascii_digit()) {
                    out.push_str(&group(name.parse().unwrap_or(usize::MAX), i + 3)?);
                } else {
                    match m.name(&name) {
                        Some(g) => out.push_str(g.as_str()),
                        None => {
                            return Err(format!(
                                "unknown group name {}",
                                crate::template::py_repr(&name)
                            ));
                        }
                    }
                }
                i += 4 + close;
            }
            '0' => {
                // An octal escape of up to three digits.
                let mut j = i + 2;
                let mut v = 0u32;
                while j < chars.len() && j < i + 4 && ('0'..='7').contains(&chars[j]) {
                    v = v * 8 + chars[j].to_digit(8).unwrap();
                    j += 1;
                }
                out.extend(char::from_u32(v));
                i = j;
            }
            '1'..='9' => {
                let mut j = i + 2;
                if chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                }
                let n: usize = chars[i + 1..j].iter().collect::<String>().parse().unwrap();
                out.push_str(&group(n, i + 1)?);
                i = j;
            }
            'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' => {
                out.push(match next {
                    'a' => '\x07',
                    'b' => '\x08',
                    'f' => '\x0c',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'v' => '\x0b',
                    _ => '\\',
                });
                i += 2;
            }
            c if c.is_ascii_alphabetic() => {
                return Err(format!("bad escape \\{c} at position {start}"));
            }
            c => {
                out.push('\\');
                out.push(c);
                i += 2;
            }
        }
    }
    Ok(out)
}

/// beets' `normalize_genre`: the genre with the first alias that matches
/// it applied (lowercase if none does).
pub fn normalize_genre(log: &PluginLog, aliases: &AliasPatterns, genre: &str) -> String {
    let lower = genre.to_lowercase();
    for (pat, template) in aliases {
        let Ok(Some(m)) = pat.re.captures(lower.as_str()) else {
            continue;
        };
        let groups = pat.re.captures_len() - 1;
        match expand(&m, groups, template) {
            Ok(expanded) => {
                if expanded != genre {
                    extra_debug(
                        log,
                        &format!("replaced using 'aliases': {genre} -> {expanded}"),
                    );
                }
                return expanded;
            }
            Err(e) => log.warning(&format!(
                "invalid alias template {template}; skipping for genre {genre}: {e}"
            )),
        }
    }
    lower
}

/// Python's `str.title()`.
pub fn py_title(s: &str) -> String {
    let mut out = String::new();
    let mut prev_cased = false;
    for c in s.chars() {
        let titlecase_digraph = match c {
            'Ǆ' | 'ǅ' | 'ǆ' => Some('ǅ'),
            'Ǉ' | 'ǈ' | 'ǉ' => Some('ǈ'),
            'Ǌ' | 'ǋ' | 'ǌ' => Some('ǋ'),
            'Ǳ' | 'ǲ' | 'ǳ' => Some('ǲ'),
            _ => None,
        };
        if prev_cased {
            out.extend(c.to_lowercase());
        } else if let Some(t) = titlecase_digraph {
            out.push(t);
        } else if c == 'ß' {
            out.push_str("Ss");
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_lowercase() || c.is_uppercase() || titlecase_digraph.is_some();
    }
    out
}

/// beets' `unique_list`.
fn unique_list(v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

// --------------------------------------------- canonicalization tree

/// A YAML scalar as Python's `str()` shows it.
fn py_str(y: &Y) -> String {
    match y {
        Y::String(s) => s.clone(),
        Y::Bool(true) => "True".into(),
        Y::Bool(false) => "False".into(),
        Y::Null => "None".into(),
        Y::Number(n) => n.to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// beets' `flatten_tree`: the tree's branches, root to leaf.
pub fn flatten_tree(elem: &Y, path: &[String], branches: &mut Vec<Vec<String>>) {
    match elem {
        Y::Mapping(m) => {
            for (k, v) in m {
                let mut p = path.to_vec();
                p.push(py_str(k));
                flatten_tree(v, &p, branches);
            }
        }
        Y::Sequence(s) => {
            for sub in s {
                flatten_tree(sub, path, branches);
            }
        }
        Y::Null if path.is_empty() => {}
        other => {
            let mut p = path.to_vec();
            p.push(py_str(other));
            branches.push(p);
        }
    }
}

/// beets' `find_parents`: the genre and its parents, closest first.
pub fn find_parents(candidate: &str, branches: &[Vec<String>]) -> Vec<String> {
    let lower = candidate.to_lowercase();
    for branch in branches {
        if let Some(idx) = branch.iter().position(|g| *g == lower) {
            return branch[..=idx].iter().rev().cloned().collect();
        }
    }
    vec![candidate.to_string()]
}

/// beets' `get_depth`.
pub fn get_depth(tag: &str, branches: &[Vec<String>]) -> Option<usize> {
    branches
        .iter()
        .find_map(|b| b.iter().position(|g| g == tag))
}

/// beets' `sort_by_depth`: the tags in the tree, deepest first (ties in
/// reverse order); tags not in the tree are dropped.
pub fn sort_by_depth<S: AsRef<str>>(tags: &[S], branches: &[Vec<String>]) -> Vec<String> {
    let mut pairs: Vec<(usize, String)> = tags
        .iter()
        .filter_map(|t| get_depth(t.as_ref(), branches).map(|d| (d, t.as_ref().to_string())))
        .collect();
    pairs.sort_by(|a, b| b.cmp(a));
    pairs.into_iter().map(|p| p.1).collect()
}

// ---------------------------------------------------------------- config

/// Python's truthiness of a config value.
fn truthy(y: Option<&Y>) -> bool {
    match y {
        None | Some(Y::Null) => false,
        Some(Y::Bool(b)) => *b,
        Some(Y::Number(n)) => n.as_f64() != Some(0.0),
        Some(Y::String(s)) => match yaml11_bool(s) {
            Some(b) => b,
            None => !s.is_empty(),
        },
        Some(Y::Sequence(s)) => !s.is_empty(),
        Some(Y::Mapping(m)) => !m.is_empty(),
        Some(Y::Tagged(_)) => true,
    }
}

/// YAML 1.1's booleans (confuse reads YAML 1.1, serde_yaml 1.2).
fn yaml11_bool(s: &str) -> Option<bool> {
    match s {
        "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" => Some(true),
        "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" => Some(false),
        _ => None,
    }
}

/// A tri-state "yes, no or file path" setting.
#[derive(Clone, Debug, PartialEq)]
pub enum FileSetting {
    Yes,
    No,
    Path(String),
}

fn file_setting(cfg: &Config, key: &str, what: &str) -> Result<FileSetting> {
    match cfg.get(&format!("{NAME}.{key}")) {
        Some(Y::Bool(true)) => Ok(FileSetting::Yes),
        Some(Y::Bool(false)) => Ok(FileSetting::No),
        Some(Y::String(s)) if yaml11_bool(s) == Some(true) => Ok(FileSetting::Yes),
        Some(Y::String(s)) if yaml11_bool(s) == Some(false) => Ok(FileSetting::No),
        None | Some(Y::Null) => bail!("Invalid {what} setting: expected yes, no, or a file path"),
        Some(Y::String(s)) if s.is_empty() => {
            bail!("Invalid {what} setting: expected yes, no, or a file path")
        }
        Some(other) => Ok(FileSetting::Path(py_str(other))),
    }
}

/// confuse's `get(int)`.
fn config_int(cfg: &Config, key: &str) -> Result<i64> {
    let full = format!("{NAME}.{key}");
    match cfg.get(&full) {
        Some(Y::Number(n)) => match n.as_i64() {
            Some(i) => Ok(i),
            None => Ok(n.as_f64().unwrap_or(0.0) as i64),
        },
        Some(Y::Bool(b)) => Ok(*b as i64),
        _ => Err(ConfigError(format!("{full}: must be a number")).into()),
    }
}

/// Python's type name of a YAML value.
fn py_type(y: &Y) -> &'static str {
    match y {
        Y::Null => "NoneType",
        Y::Bool(_) => "bool",
        Y::Number(n) if n.is_f64() => "float",
        Y::Number(_) => "int",
        Y::String(_) => "str",
        Y::Sequence(_) => "list",
        Y::Mapping(_) => "dict",
        Y::Tagged(_) => "object",
    }
}

/// confuse's `MappingValues(Sequence(str))`: `name` names the view in
/// error messages ("" for a detached view).
fn mapping_of_lists(y: &Y, name: &str) -> Result<Vec<(String, Vec<String>)>> {
    let Y::Mapping(m) = y else {
        return Err(ConfigError(format!("{name} must be a dict, not {}", py_type(y))).into());
    };
    let mut out = vec![];
    for (k, v) in m {
        let key = py_str(k);
        let kname = if name.is_empty() {
            key.clone()
        } else {
            format!("{name}.{key}")
        };
        let Y::Sequence(seq) = v else {
            return Err(ConfigError(format!("{kname} must be a list, not {}", py_type(v))).into());
        };
        let mut vals = vec![];
        for (i, s) in seq.iter().enumerate() {
            match s {
                Y::String(s) => vals.push(s.clone()),
                _ => return Err(ConfigError(format!("{kname}#{i}: must be a string")).into()),
            }
        }
        out.push((key, vals));
    }
    Ok(out)
}

/// The plugin's settings (beets' `lastgenre:` section).
#[derive(Clone, Debug)]
pub struct Conf {
    pub min_weight: i64,
    pub count: i64,
    pub fallback: Option<String>,
    pub cleanup_existing: bool,
    pub source: Y,
    pub force: bool,
    pub keep_existing: bool,
    pub auto: bool,
    pub prefer_specific: bool,
    pub title_case: bool,
    pub pretend: bool,
    pub va_name: String,
    pub format_item: String,
    pub format_album: String,
}

impl Conf {
    pub fn new(cfg: &Config) -> Result<Conf> {
        let b = |k: &str| truthy(cfg.get(&format!("{NAME}.{k}")));
        Ok(Conf {
            min_weight: config_int(cfg, "min_weight")?,
            count: config_int(cfg, "count")?,
            fallback: cfg
                .get("lastgenre.fallback")
                .filter(|y| truthy(Some(y)))
                .map(py_str),
            cleanup_existing: b("cleanup_existing"),
            source: cfg.get("lastgenre.source").cloned().unwrap_or(Y::Null),
            force: b("force"),
            keep_existing: b("keep_existing"),
            auto: b("auto"),
            prefer_specific: b("prefer_specific"),
            title_case: b("title_case"),
            pretend: b("pretend"),
            va_name: cfg.str("va_name"),
            format_item: cfg.str("format_item"),
            format_album: cfg.str("format_album"),
        })
    }

    /// beets' `sources`: the allowed genre sources.
    pub fn sources(&self) -> Result<&'static [&'static str]> {
        match &self.source {
            Y::String(s) if s == "track" => Ok(&["track", "album", "artist"]),
            Y::String(s) if s == "album" => Ok(&["album", "artist"]),
            Y::String(s) if s == "artist" => Ok(&["artist"]),
            other => {
                let shown = match other {
                    Y::String(s) => crate::template::py_repr(s),
                    other => py_str(other),
                };
                Err(ConfigError(format!(
                    "lastgenre.source: must be one of ['track', 'album', 'artist'], not {shown}"
                ))
                .into())
            }
        }
    }
}

// ---------------------------------------------------------------- plugin

/// A `(genres, label)` pair; the label tells where the genres came from.
pub type GenresWithLabel = (Vec<String>, String);

type FetchFn = dyn Fn(Kind, &Record, &[String]) -> Vec<String>;

pub struct LastGenre {
    pub conf: Conf,
    /// Valid genres (lowercase); empty means all are.
    pub whitelist: HashSet<String>,
    pub c14n_branches: Vec<Vec<String>>,
    pub canonicalize: bool,
    pub ignore_patterns: IgnorePatterns,
    pub alias_patterns: AliasPatterns,
    pub client: LastFmClient,
    pub log: PluginLog,
    /// Replaces the client's `fetch` (in tests).
    pub fetch_override: Option<Box<FetchFn>>,
}

impl LastGenre {
    /// beets' `__init__` + `setup`.
    pub fn new(cfg: &Config) -> Result<LastGenre> {
        let log = PluginLog::new(NAME);
        let conf = Conf::new(cfg)?;
        let whitelist = load_whitelist(cfg, &log)?;
        let (c14n_branches, canonicalize) = load_c14n_tree(cfg, &log, conf.prefer_specific)?;
        let ignore_patterns = load_ignorelist(cfg, &log)?;
        let alias_patterns = load_aliases(cfg, &log)?;
        let client = LastFmClient::new(
            conf.min_weight,
            ignore_patterns.clone(),
            alias_patterns.clone(),
        );
        Ok(LastGenre {
            conf,
            whitelist,
            c14n_branches,
            canonicalize,
            ignore_patterns,
            alias_patterns,
            client,
            log,
            fetch_override: None,
        })
    }

    fn sources(&self) -> &'static [&'static str] {
        // Checked when the command or import stage starts.
        self.conf.sources().unwrap_or(&[])
    }

    fn has_source(&self, s: &str) -> bool {
        self.sources().contains(&s)
    }

    fn fetch(&self, kind: Kind, obj: &Record, args: &[String]) -> Vec<String> {
        match &self.fetch_override {
            Some(f) => f(kind, obj, args),
            None => self.client.fetch(&self.log, kind, obj, args),
        }
    }

    // Genre list processing.

    /// beets' `_resolve_genres`: alias, canonicalize, deduplicate, sort
    /// and filter the tags, keeping at most `count`.
    pub fn resolve_genres<S: AsRef<str>>(&self, tags: &[S], artist: &str) -> Vec<String> {
        if tags.is_empty() {
            return vec![];
        }
        let mut tags: Vec<String> = tags.iter().map(|t| t.as_ref().to_string()).collect();
        if !self.alias_patterns.is_empty() {
            tags = tags
                .iter()
                .map(|t| normalize_genre(&self.log, &self.alias_patterns, t))
                .collect();
        }
        let count = self.conf.count;
        if self.canonicalize {
            let mut all = vec![];
            for tag in &tags {
                if is_ignored(&self.log, &self.ignore_patterns, tag, artist) {
                    continue;
                }
                let parents = if !self.whitelist.is_empty() {
                    self.filter_valid(&find_parents(tag, &self.c14n_branches), artist)
                } else {
                    let oldest = find_parents(tag, &self.c14n_branches).pop().unwrap();
                    if is_ignored(&self.log, &self.ignore_patterns, &oldest, artist) {
                        vec![]
                    } else {
                        vec![oldest]
                    }
                };
                all.extend(parents);
                if !self.conf.prefer_specific && all.len() as i64 >= count {
                    break;
                }
            }
            tags = all;
        }
        tags = unique_list(tags);
        if self.conf.prefer_specific {
            tags = sort_by_depth(&tags, &self.c14n_branches);
        }
        let valid = self.filter_valid(&tags, artist);
        py_slice(valid, count)
    }

    /// beets' `_filter_valid`: non-blank (stripped) genres that pass the
    /// whitelist and the ignorelist.
    pub fn filter_valid<S: AsRef<str>>(&self, genres: &[S], artist: &str) -> Vec<String> {
        genres
            .iter()
            .map(|g| g.as_ref().trim())
            .filter(|g| !g.is_empty())
            .filter(|g| {
                (self.whitelist.is_empty() || self.whitelist.contains(&g.to_lowercase()))
                    && !is_ignored(&self.log, &self.ignore_patterns, g, artist)
            })
            .map(str::to_string)
            .collect()
    }

    /// beets' `_format_genres`: title case, if configured.
    pub fn format_genres(&self, tags: Vec<String>) -> Vec<String> {
        if self.conf.title_case {
            tags.iter().map(|t| py_title(t)).collect()
        } else {
            tags
        }
    }

    /// beets' `_artist_for_filter`.
    fn artist_for_filter(obj: &Record) -> String {
        match obj.kind {
            ModelKind::Item => obj.str("artist").to_string(),
            ModelKind::Album => {
                let aa = obj.str("albumartist");
                if aa.is_empty() {
                    obj.str("artist").to_string()
                } else {
                    aa.to_string()
                }
            }
        }
    }

    fn combine_resolve_and_log(&self, old: &[String], new: &[String], artist: &str) -> Vec<String> {
        self.log
            .debug(&format!("raw last.fm tags: {}", py_list(new)));
        self.log.debug(&format!(
            "existing genres taken into account: {}",
            py_list(old)
        ));
        let combined: Vec<String> = old.iter().chain(new).cloned().collect();
        self.resolve_genres(&combined, artist)
    }

    /// beets' `fallback`.
    pub fn fallback(&self) -> GenresWithLabel {
        match &self.conf.fallback {
            Some(f) => (vec![f.clone()], "fallback".into()),
            None => (vec![], "fallback unconfigured".into()),
        }
    }

    /// beets' `_try_resolve_stage`.
    fn try_resolve_stage(
        &self,
        stage: &str,
        keep: &[String],
        new: &[String],
        artist: &str,
    ) -> Option<GenresWithLabel> {
        let resolved = self.combine_resolve_and_log(keep, new, artist);
        if resolved.is_empty() {
            return None;
        }
        let suffix = if self.whitelist.is_empty() {
            "any"
        } else {
            "whitelist"
        };
        let mut label = format!("{stage}, {suffix}");
        if !keep.is_empty() {
            label = format!("keep + {label}");
        }
        Some((self.format_genres(resolved), label))
    }

    /// beets' `_try_resolve_existing_genres`.
    fn try_resolve_existing(&self, obj: &Record, genres: &[String]) -> Option<GenresWithLabel> {
        if self.conf.cleanup_existing {
            let keep: Vec<String> = genres.iter().map(|g| g.to_lowercase()).collect();
            return self.try_resolve_stage("cleanup", &keep, &[], &Self::artist_for_filter(obj));
        }
        Some((genres.to_vec(), "keep any, no-force".into()))
    }

    /// beets' `_try_resolve_original_fallback`.
    fn try_resolve_original_fallback(
        &self,
        obj: &Record,
        genres: &[String],
        keep: &[String],
    ) -> Option<GenresWithLabel> {
        if genres.is_empty() || !self.conf.keep_existing {
            return None;
        }
        let artist = Self::artist_for_filter(obj);
        let normalized = unique_list(
            genres
                .iter()
                .filter_map(|g| {
                    let norm = normalize_genre(&self.log, &self.alias_patterns, g);
                    if norm.is_empty() {
                        None
                    } else if norm != g.to_lowercase() {
                        Some(norm)
                    } else {
                        Some(g.clone())
                    }
                })
                .collect(),
        );
        let valid = self.filter_valid(&normalized, &artist);
        if !valid.is_empty() {
            return Some((valid, "original fallback".into()));
        }
        self.try_resolve_stage("original fallback", keep, &[], &artist)
    }

    /// beets' `_try_resolve_album_stage`.
    fn try_resolve_album_stage(&self, obj: &Record, keep: &[String]) -> Option<GenresWithLabel> {
        let new = self.fetch(Kind::Album, obj, &[]);
        let albumartist = obj.str("albumartist");
        if !new.is_empty()
            && let Some(r) = self.try_resolve_stage("album", keep, &new, albumartist)
        {
            return Some(r);
        }
        if new.is_empty() && albumartist != self.conf.va_name {
            extra_debug(
                &self.log,
                &format!(
                    "No album genre found for \"{albumartist}\", trying multi-valued albumartists field..."
                ),
            );
            let mut multi = vec![];
            for aa in obj.list("albumartists") {
                extra_debug(&self.log, &format!("Fetching album genre for \"{aa}\""));
                multi.extend(self.fetch(
                    Kind::Album,
                    obj,
                    &[aa.clone(), obj.str("album").to_string()],
                ));
            }
            if !multi.is_empty()
                && let Some(r) =
                    self.try_resolve_stage("multi-valued albumartist album", keep, &multi, "")
            {
                return Some(r);
            }
        }
        None
    }

    /// beets' `_fetch_va_genres`: the most popular track (or artist) genre
    /// of a Various Artists album.
    pub fn fetch_va_genres(&self, items: &[Item]) -> Vec<String> {
        let mut genres: Vec<String> = vec![];
        for item in items {
            let mut g = vec![];
            if self.has_source("track") {
                g = self.fetch(Kind::Track, item, &[]);
            }
            if g.is_empty() {
                g = self.fetch(Kind::Artist, item, &[]);
            }
            genres.extend(g);
        }
        match plurality(&genres) {
            Some((most, rank)) => {
                self.log.debug(&format!(
                    "Most popular track genre \"{most}\" ({rank}) for VA album."
                ));
                vec![most]
            }
            None => vec![],
        }
    }

    /// beets' `_fetch_artist_stage`: (stage label, genres, stage artist).
    fn fetch_artist_stage(&self, obj: &Record, items: &[Item]) -> (String, Vec<String>, String) {
        if obj.kind == ModelKind::Item {
            return (
                "artist".into(),
                self.fetch(Kind::Artist, obj, &[]),
                obj.str("artist").to_string(),
            );
        }
        let albumartist = obj.str("albumartist");
        if albumartist != self.conf.va_name {
            let mut new = self.fetch(Kind::AlbumArtist, obj, &[]);
            if !new.is_empty() {
                return ("albumartist".into(), new, albumartist.to_string());
            }
            extra_debug(
                &self.log,
                &format!(
                    "No album artist genre found for \"{albumartist}\", trying multi-valued field..."
                ),
            );
            for aa in obj.list("albumartists") {
                extra_debug(&self.log, &format!("Fetching artist genre for \"{aa}\""));
                new.extend(self.fetch(Kind::AlbumArtist, obj, std::slice::from_ref(&aa)));
            }
            if !new.is_empty() {
                return ("multi-valued albumartist".into(), new, String::new());
            }
            return ("albumartist".into(), vec![], String::new());
        }
        (
            "most popular track".into(),
            self.fetch_va_genres(items),
            String::new(),
        )
    }

    /// beets' `_get_genre`: the genres for an item or album (`items` are
    /// an album's items) and where they came from.
    pub fn get_genre(&self, obj: &Record, items: &[Item]) -> GenresWithLabel {
        let existing = obj.list("genres");
        if !existing.is_empty() && !self.conf.force {
            if let Some(r) = self.try_resolve_existing(obj, &existing) {
                return r;
            }
            return self.fallback();
        }
        let keep: Vec<String> = if self.conf.keep_existing && self.conf.force {
            existing.iter().map(|g| g.to_lowercase()).collect()
        } else {
            vec![]
        };
        if obj.kind == ModelKind::Item && self.has_source("track") {
            let new = self.fetch(Kind::Track, obj, &[]);
            if !new.is_empty()
                && let Some(r) = self.try_resolve_stage("track", &keep, &new, obj.str("artist"))
            {
                return r;
            }
        }
        if self.has_source("album")
            && let Some(r) = self.try_resolve_album_stage(obj, &keep)
        {
            return r;
        }
        if self.has_source("artist") {
            let (stage, new, artist) = self.fetch_artist_stage(obj, items);
            if !new.is_empty()
                && let Some(r) = self.try_resolve_stage(&stage, &keep, &new, &artist)
            {
                return r;
            }
        }
        if let Some(r) = self.try_resolve_original_fallback(obj, &existing, &keep) {
            return r;
        }
        self.fallback()
    }

    // Library updates.

    fn format_obj(&self, lib: &Library, obj: &Record) -> String {
        let fmt = match obj.kind {
            ModelKind::Item => &self.conf.format_item,
            ModelKind::Album => &self.conf.format_album,
        };
        crate::commands::format_record(&Template::parse(fmt), obj, Some(lib))
    }

    /// beets' `_fetch_and_log_genre`: set the object's genres and show
    /// how they changed.
    fn fetch_and_log_genre(&self, lib: &Library, obj: &mut Record, items: &[Item]) -> Result<()> {
        self.log.info(&self.format_obj(lib, obj));
        let (genres, label) = self.get_genre(obj, items);
        obj.set_list("genres", &genres);
        self.log.debug(&format!(
            "Resolved ({label}): {}",
            py_list(&obj.list("genres"))
        ));
        let id = obj.id.unwrap_or_default();
        let fresh = match obj.kind {
            ModelKind::Item => lib.get_item(id)?,
            ModelKind::Album => lib.get_album(id)?,
        };
        if let Some(old) = fresh {
            crate::commands::print_model_changes(&old, obj, &["genres".to_string()]);
        }
        Ok(())
    }

    /// beets' `_process_track`.
    pub fn process_item(&self, lib: &Library, item: &mut Item, write: bool) -> Result<()> {
        self.fetch_and_log_genre(lib, item, &[])?;
        if !self.conf.pretend {
            try_sync_item(lib, item, write)?;
        }
        Ok(())
    }

    /// beets' `_process_album`: the album's genres (and its tracks', with
    /// `source: track`; otherwise the tracks inherit the album's).
    pub fn process_album(&self, lib: &Library, album: &mut Album, write: bool) -> Result<()> {
        let id = album.id.unwrap_or_default();
        let mut items = lib.album_items(id)?;
        self.fetch_and_log_genre(lib, album, &items)?;
        let track = self.has_source("track");
        if track {
            for item in &mut items {
                self.process_item(lib, item, write)?;
            }
        }
        if !self.conf.pretend {
            // `Album.try_sync(write, move=False, inherit=not track)`.
            let old = lib.get_album(id)?;
            lib.store(album)?;
            let mut items = lib.album_items(id)?;
            let changed = old.is_none_or(|o| o.get("genres") != album.get("genres"));
            for item in &mut items {
                if !track && changed {
                    item.set_list("genres", &album.list("genres"));
                }
                try_sync_item(lib, item, write)?;
            }
        }
        Ok(())
    }

    /// The import stage (beets' `imported`): genres for the new album or
    /// singleton, stored in the library; the importer writes them when it
    /// places the files.
    fn imported(
        &self,
        ctx: &PluginCtx,
        album: Option<&mut Album>,
        items: &mut [Item],
    ) -> Result<()> {
        if !self.conf.auto {
            return Ok(());
        }
        self.conf.sources()?;
        self.log.at_level(WARNING, || -> Result<()> {
            match album {
                Some(a) => self.process_album(ctx.lib, a, false),
                None => {
                    for item in items.iter_mut() {
                        self.process_item(ctx.lib, item, false)?;
                    }
                    Ok(())
                }
            }
        })
    }
}

/// Python's `seq[:n]` for a possibly negative `n`.
fn py_slice(v: Vec<String>, n: i64) -> Vec<String> {
    let len = v.len() as i64;
    let end = if n < 0 { (len + n).max(0) } else { n.min(len) };
    v.into_iter().take(end as usize).collect()
}

/// beets' `plurality`: the most common value (the first seen of equals)
/// and its count.
pub fn plurality(v: &[String]) -> Option<(String, usize)> {
    let mut counts: Vec<(&String, usize)> = vec![];
    for s in v {
        match counts.iter_mut().find(|(k, _)| *k == s) {
            Some(e) => e.1 += 1,
            None => counts.push((s, 1)),
        }
    }
    let max = counts.iter().map(|c| c.1).max()?;
    counts
        .into_iter()
        .find(|c| c.1 == max)
        .map(|(s, n)| (s.clone(), n))
}

/// beets' `Item.try_sync(write, move=False)`.
fn try_sync_item(lib: &Library, item: &mut Item, write: bool) -> Result<()> {
    if write && let Err(e) = crate::media::write_item(item) {
        crate::logging::emit(format!("{e:#}"));
    }
    lib.store(item)
}

/// beets' `_load_whitelist`.
fn load_whitelist(cfg: &Config, log: &PluginLog) -> Result<HashSet<String>> {
    let text = match file_setting(cfg, "whitelist", "whitelist")? {
        FileSetting::No => return Ok(HashSet::new()),
        FileSetting::Yes => {
            log.debug("Loading whitelist <bundled genres.txt>");
            GENRES_TXT.to_string()
        }
        FileSetting::Path(p) => {
            log.debug(&format!("Loading whitelist {p}"));
            read_file(&p)?
        }
    };
    Ok(text
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect())
}

fn read_file(p: &str) -> Result<String> {
    let path = crate::config::expand_tilde(p);
    std::fs::read_to_string(&path).map_err(|e| anyhow!("{}: {e}", path.display()))
}

/// beets' `_load_c14n_tree`: the tree's branches and whether to
/// canonicalize (`prefer_specific` loads the default tree without it).
fn load_c14n_tree(
    cfg: &Config,
    log: &PluginLog,
    prefer_specific: bool,
) -> Result<(Vec<Vec<String>>, bool)> {
    let setting = file_setting(cfg, "canonical", "canonical")?;
    let canonicalize = setting != FileSetting::No;
    let text = match setting {
        FileSetting::Yes => Some((
            "<bundled genres-tree.yaml>".to_string(),
            GENRES_TREE_YAML.to_string(),
        )),
        FileSetting::No if prefer_specific => Some((
            "<bundled genres-tree.yaml>".to_string(),
            GENRES_TREE_YAML.to_string(),
        )),
        FileSetting::No => None,
        FileSetting::Path(p) => {
            let t = read_file(&p)?;
            Some((p, t))
        }
    };
    let mut branches = vec![];
    if let Some((name, text)) = text {
        log.debug(&format!("Loading canonicalization tree {name}"));
        let tree: Y = serde_yaml::from_str(&text)?;
        flatten_tree(&tree, &[], &mut branches);
    }
    Ok((branches, canonicalize))
}

/// beets' `_load_ignorelist`: artist (lowercase, or `*`) to patterns;
/// invalid regular expressions match literally.
fn load_ignorelist(cfg: &Config, log: &PluginLog) -> Result<IgnorePatterns> {
    let y = cfg.get("lastgenre.ignorelist");
    if !truthy(y) {
        return Ok(HashMap::new());
    }
    let raw = mapping_of_lists(y.unwrap(), "lastgenre.ignorelist")?;
    let mut out: IgnorePatterns = HashMap::new();
    for (artist, patterns) in raw {
        let compiled: Vec<Pat> = patterns
            .iter()
            .map(|p| Pat::new(p).unwrap_or_else(|_| Pat::new(&py_re_escape(p)).unwrap()))
            .collect();
        let shown: Vec<&str> = compiled.iter().map(|p| p.pattern.as_str()).collect();
        extra_debug(log, &format!("ignore for {artist}: {}", py_list(&shown)));
        out.insert(artist.to_lowercase(), compiled);
    }
    Ok(out)
}

/// beets' `_load_aliases`: the bundled aliases (`yes`), none (`no`), or
/// the configured mapping of genre to patterns.
fn load_aliases(cfg: &Config, log: &PluginLog) -> Result<AliasPatterns> {
    let y = cfg.get("lastgenre.aliases").cloned().unwrap_or(Y::Null);
    let view = match &y {
        Y::Bool(false) => return Ok(vec![]),
        Y::String(s) if yaml11_bool(s) == Some(false) => return Ok(vec![]),
        Y::Bool(true) | Y::Null => None,
        Y::String(s) if s.is_empty() || yaml11_bool(s) == Some(true) => None,
        Y::Mapping(_) => Some(y.clone()),
        _ => {
            return Err(ConfigError("lastgenre.aliases must be a dict or bool.".into()).into());
        }
    };
    let view = match view {
        Some(v) => v,
        None => {
            log.debug("Loading built-in aliases");
            serde_yaml::from_str(ALIASES_YAML)?
        }
    };
    let mut out = vec![];
    for (canonical, patterns) in mapping_of_lists(&view, "")? {
        let lower = canonical.to_lowercase();
        for p in patterns {
            let pat = Pat::new(&p).map_err(|e| anyhow!("invalid alias pattern {p:?}: {e}"))?;
            out.push((pat, lower.clone()));
        }
    }
    log.debug(&format!("Loaded {} alias entries", out.len()));
    Ok(out)
}

impl Plugin for LastGenre {
    fn name(&self) -> &'static str {
        NAME
    }

    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        self.imported(ctx, task.album.as_deref_mut(), task.items)
    }
}

/// The `lastgenre` command's options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub pretend: bool,
    /// `-f`/`-F`, if given.
    pub force: Option<bool>,
    /// `-k`/`-K`, if given.
    pub keep_existing: Option<bool>,
    pub source: Option<String>,
    /// `-a` (the default) or `-A`.
    pub album: bool,
    pub query: Vec<String>,
}

/// The `lastgenre` command: the options override the settings.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let mut cfg = cfg.clone();
    cfg.set("lastgenre.pretend", Y::Bool(opts.pretend));
    if let Some(f) = opts.force {
        cfg.set("lastgenre.force", Y::Bool(f));
    }
    if let Some(k) = opts.keep_existing {
        cfg.set("lastgenre.keep_existing", Y::Bool(k));
    }
    if let Some(s) = &opts.source {
        cfg.set("lastgenre.source", Y::String(s.clone()));
    }
    cfg.set("lastgenre.album", Y::Bool(opts.album));
    // The plugin was set up (and said so) when it loaded; the options only
    // change how it runs.
    let plugin = crate::logging::capture(|| LastGenre::new(&cfg)).0?;
    run_command(&plugin, lib, cfg.bool("import.write"), &opts)
}

fn run_command(p: &LastGenre, lib: &Library, write: bool, opts: &CmdOpts) -> Result<()> {
    p.conf.sources()?;
    if opts.album {
        for mut album in lib.albums(&Query::parse(ModelKind::Album, &opts.query)?)? {
            p.process_album(lib, &mut album, write)?;
        }
    } else {
        for mut item in lib.items(&Query::parse(ModelKind::Item, &opts.query)?)? {
            p.process_item(lib, &mut item, write)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "lastgenre",
        order: 13,
        defaults: r#"lastgenre:
    whitelist: yes
    min_weight: 10
    count: 1
    fallback: null
    canonical: no
    cleanup_existing: no
    source: album
    force: no
    keep_existing: no
    auto: yes
    prefer_specific: no
    title_case: yes
    pretend: no
    ignorelist: {}
    aliases: yes
"#,
        config_keys: &["lastgenre"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "lastgenre",
                build: build_lastgenre,
                run: run_lastgenre,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(LastGenre::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct LastgenreArgs {
    /// Show actions but do nothing
    #[arg(short = 'p', long)]
    pretend: bool,
    /// Modify existing genres
    #[arg(short = 'f', long, overrides_with = "no_force")]
    force: bool,
    /// Don't modify existing genres
    #[arg(short = 'F', long, overrides_with = "force")]
    no_force: bool,
    /// Combine with existing genres when modifying
    #[arg(short = 'k', long, overrides_with = "no_keep_existing")]
    keep_existing: bool,
    /// Don't combine with existing genres when modifying
    #[arg(short = 'K', long, overrides_with = "keep_existing")]
    no_keep_existing: bool,
    /// Genre source: artist, album, or track
    #[arg(short = 's', long, value_name = "SOURCE")]
    source: Option<String>,
    /// Match items instead of albums
    #[arg(short = 'A', long, overrides_with = "albums")]
    items: bool,
    /// Match albums instead of items (default)
    #[arg(short = 'a', long, overrides_with = "items")]
    albums: bool,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_lastgenre() -> clap::Command {
    use clap::Args as _;
    LastgenreArgs::augment_args(
        clap::Command::new("lastgenre").about("fetch genres"),
    )
}

pub fn run_lastgenre(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = LastgenreArgs::from_arg_matches(m)?;
    if !plugins.iter().any(|p| p.name() == "lastgenre") {
        // It failed to load (and said why).
        anyhow::bail!("unknown command 'lastgenre'");
    }
    let flag = |on: bool, off: bool| match (on, off) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    };
    crate::plugins::lastgenre::command(
        cfg,
        lib,
        crate::plugins::lastgenre::CmdOpts {
            pretend: a.pretend,
            force: flag(a.force, a.no_force),
            keep_existing: flag(a.keep_existing, a.no_keep_existing),
            source: a.source,
            album: !a.items,
            query: a.query,
        },
    )
}
