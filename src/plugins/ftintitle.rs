//! Port of beets 2.14's `ftintitle` plugin (`beetsplug/ftintitle.py`):
//! moves "featured" artists from the artist field to the title ("Alice
//! feat. Bob" / "Song" -> "Alice" / "Song feat. Bob").
//!
//! It runs as an import stage (`auto`), on the releases and recordings
//! fetched from MusicBrainz before they're matched (beets'
//! `albuminfo_received`/`trackinfo_received`, also with `auto`), and as
//! `beet ftintitle [-d] [-l N] [QUERY]`. Config: `auto`, `drop`,
//! `format`, `keep_in_artist`, `preserve_album_artist`, `custom_words`,
//! `bracket_keywords`. It adds the album template field
//! `album_artist_no_feat`.

use crate::config::Config;
use crate::library::Library;
use crate::mb::{AlbumInfo, InfoListener, TrackInfo};
use crate::model::{Item, Kind};
use crate::plugins::{INFO, ImportTask, Plugin, PluginCtx, PluginLog, WARNING};
use crate::query::Query;
use anyhow::Result;
use fancy_regex::Regex;
use std::sync::Arc;

pub const DEFAULT_BRACKET_KEYWORDS: &[&str] = &[
    "abridged",
    "acapella",
    "club",
    "demo",
    "edit",
    "edition",
    "extended",
    "instrumental",
    "live",
    "mix",
    "radio",
    "release",
    "remaster",
    "remastered",
    "remix",
    "rmx",
    "unabridged",
    "unreleased",
    "version",
    "vip",
];

fn feat_regex(for_artist: bool, custom_words: &[String]) -> Regex {
    Regex::new(&format!(
        "(?i){}",
        crate::plugins::feat_tokens(for_artist, custom_words)
    ))
    .expect("feat tokens compile")
}

/// `_split_on_feat_token`: the text around the first match, a bracketed
/// token's closing bracket dropped (or its opener kept on the left when
/// it isn't closed at the end).
fn split_on_feat_token(re: &Regex, artist: &str) -> Option<(String, String)> {
    let m = re.find(artist).ok()??;
    let mut left = artist[..m.start()].trim().to_string();
    let mut right = artist[m.end()..].trim().to_string();
    let opener = m.as_str().chars().next()?;
    let closing = match opener {
        '(' => Some(')'),
        '[' => Some(']'),
        _ => None,
    };
    if let Some(c) = closing {
        if let Some(r) = right.strip_suffix(c) {
            right = r.trim().to_string();
        } else {
            left = format!("{left} {opener}").trim().to_string();
        }
    }
    Some((left, right))
}

/// `split_on_feat`: the main artist and the featured one, if any.
/// Explicit tokens ("feat.") first; then, for an artist field, a comma,
/// then the generic separators ("&", "and", "with", ...).
pub fn split_on_feat(
    artist: &str,
    for_artist: bool,
    custom_words: &[String],
) -> (String, Option<String>) {
    if let Some((l, r)) = split_on_feat_token(&feat_regex(false, custom_words), artist) {
        return (l, Some(r));
    }
    if for_artist && let Some((l, r)) = artist.split_once(',') {
        return (l.trim().to_string(), Some(r.trim().to_string()));
    }
    if for_artist && let Some((l, r)) = split_on_feat_token(&feat_regex(true, custom_words), artist)
    {
        return (l, Some(r));
    }
    (artist.trim().to_string(), None)
}

/// `contains_feat`: whether the title has a "featured" marker.
pub fn contains_feat(title: &str, custom_words: &[String]) -> bool {
    feat_regex(false, custom_words)
        .is_match(title)
        .unwrap_or(false)
}

/// `find_feat_part`: the featured artists in the artist field, given the
/// album artist.
pub fn find_feat_part(
    artist: &str,
    albumartist: Option<&str>,
    custom_words: &[String],
) -> Option<String> {
    let albumartist = albumartist.filter(|a| !a.is_empty());
    // The album artist is the featured one: the rest goes to the title.
    let (artist_part, feat_part) = split_on_feat(artist, true, custom_words);
    if let Some(aa) = albumartist
        && feat_part.as_deref() == Some(aa)
        && !artist_part.is_empty()
    {
        return Some(artist_part);
    }
    // The album artist is in the track artist: look around it.
    if let Some(aa) = albumartist
        && let Some((lhs, rhs)) = artist.split_once(aa)
    {
        if !rhs.is_empty() {
            return split_on_feat(rhs, true, custom_words).1;
        }
        let (lhs, _) = split_on_feat(lhs, true, custom_words);
        if !lhs.is_empty() {
            return Some(lhs);
        }
    }
    // Conservative: only explicit tokens.
    split_on_feat(artist, false, custom_words).1
}

/// The pattern finding where the featured part goes in a title: before
/// the first bracket holding one of `keywords` (any bracket for none),
/// else at the end.
fn bracket_position_pattern(keywords: &[String]) -> Regex {
    let inner: Vec<String> = keywords
        .iter()
        .map(|k| fancy_regex::escape(k).into_owned())
        .collect();
    let inner = inner.join("|");
    let kw = if inner.is_empty() {
        String::new()
    } else {
        format!(r"\b(?={inner})\b")
    };
    Regex::new(&format!(
        r"(?i)(?:\s*?(?=\([^)]*{kw}.*?\)|\[[^\]]*{kw}.*?\]|<[^>]*{kw}.*?>|\{{[^}}]*{kw}.*?\}}|$))"
    ))
    .expect("bracket pattern compiles")
}

/// `insert_ft_into_title`.
pub fn insert_ft_into_title(title: &str, feat_part: &str, keywords: Option<&[String]>) -> String {
    let defaults: Vec<String>;
    let keywords = match keywords {
        Some(k) => k,
        None => {
            defaults = DEFAULT_BRACKET_KEYWORDS
                .iter()
                .map(ToString::to_string)
                .collect();
            &defaults
        }
    };
    let re = bracket_position_pattern(keywords);
    // `pattern.split(title, maxsplit=1)`, joined around the featured part.
    let joined = match re.find(title).ok().flatten() {
        Some(m) => format!("{} {feat_part} {}", &title[..m.start()], &title[m.end()..]),
        None => title.to_string(),
    };
    joined.trim().to_string()
}

/// What `ft_in_title` reads and changes: an item or a fetched track.
pub trait Meta {
    fn get(&self, key: &str) -> String;
    fn set(&mut self, key: &str, value: &str);
    /// An item's path (beets logs it); None for fetched metadata.
    fn filepath(&self) -> Option<String> {
        None
    }
}

impl Meta for Item {
    fn get(&self, key: &str) -> String {
        self.str(key).to_string()
    }
    fn set(&mut self, key: &str, value: &str) {
        self.set_str(key, value);
    }
    fn filepath(&self) -> Option<String> {
        Some(self.path().display().to_string())
    }
}

impl Meta for TrackInfo {
    fn get(&self, key: &str) -> String {
        match key {
            "artist" => self.artist.clone(),
            "artist_sort" => self.artist_sort.clone(),
            "title" => self.title.clone(),
            _ => String::new(),
        }
    }
    fn set(&mut self, key: &str, value: &str) {
        match key {
            "artist" => self.artist = value.to_string(),
            "artist_sort" => self.artist_sort = value.to_string(),
            "title" => self.title = value.to_string(),
            _ => {}
        }
    }
}

/// The plugin's settings.
#[derive(Debug, Clone)]
pub struct FtCore {
    pub auto: bool,
    pub drop: bool,
    pub format: String,
    pub keep_in_artist: bool,
    pub preserve_album_artist: bool,
    pub custom_words: Vec<String>,
    pub bracket_keywords: Vec<String>,
}

impl FtCore {
    pub fn new(cfg: &Config) -> Self {
        FtCore {
            auto: cfg.bool("ftintitle.auto"),
            drop: cfg.bool("ftintitle.drop"),
            format: cfg.str("ftintitle.format"),
            keep_in_artist: cfg.bool("ftintitle.keep_in_artist"),
            preserve_album_artist: cfg.bool("ftintitle.preserve_album_artist"),
            custom_words: cfg.list("ftintitle.custom_words"),
            bracket_keywords: cfg.list("ftintitle.bracket_keywords"),
        }
    }

    /// `update_metadata`: the artist without the featured part (unless
    /// `keep_in_artist`), the sort name too, and the featured part in the
    /// title (unless `drop` or the title has one). Whether anything
    /// changed.
    pub fn update_metadata(&self, m: &mut dyn Meta, feat_part: &str, log: &PluginLog) -> bool {
        let mut changed = false;
        let artist = m.get("artist");
        if self.keep_in_artist {
            log.info(&format!(
                "artist: {artist} (Not changing due to keep_in_artist)"
            ));
        } else {
            let (track_artist, _) = split_on_feat(&artist, true, &self.custom_words);
            log.info(&format!("artist: {artist} -> {track_artist}"));
            m.set("artist", &track_artist);
            changed |= artist != track_artist;
        }
        let artist_sort = m.get("artist_sort");
        if !artist_sort.is_empty() {
            let (no_feat, _) = split_on_feat(&artist_sort, true, &self.custom_words);
            m.set("artist_sort", &no_feat);
            changed |= artist_sort != no_feat;
        }
        let title = m.get("title");
        if !self.drop && !contains_feat(&title, &self.custom_words) {
            let formatted = crate::plugins::the::py_format(&self.format, &[feat_part]);
            let new_title = insert_ft_into_title(&title, &formatted, Some(&self.bracket_keywords));
            log.info(&format!("title: {title} -> {new_title}"));
            m.set("title", &new_title);
            changed |= title != new_title;
        }
        changed
    }

    /// `ft_in_title`: move the featured artists of `m` to its title.
    /// Whether it changed.
    pub fn ft_in_title(&self, m: &mut dyn Meta, albumartist: &str, log: &PluginLog) -> bool {
        let artist = m.get("artist").trim().to_string();
        let albumartist = albumartist.trim();
        if !self.has_feat_candidate(&artist, albumartist) {
            return false;
        }
        if let Some(p) = m.filepath() {
            log.info(&p);
        }
        let aa = (!albumartist.is_empty()).then_some(albumartist);
        let Some(feat_part) = find_feat_part(&artist, aa, &self.custom_words) else {
            log.info("no featuring artists found");
            return false;
        };
        self.update_metadata(m, &feat_part, log)
    }

    fn has_feat_candidate(&self, artist: &str, albumartist: &str) -> bool {
        if self.preserve_album_artist && !albumartist.is_empty() && artist == albumartist {
            return false;
        }
        let for_artist = albumartist.is_empty() || artist.contains(albumartist);
        split_on_feat(artist, for_artist, &self.custom_words)
            .1
            .is_some_and(|f| !f.is_empty())
    }
}

/// `trackinfo_received` / `albuminfo_received`.
impl InfoListener for FtCore {
    fn album_info(&self, info: &mut AlbumInfo) {
        let log = listener_log();
        let albumartist = info.artist.clone();
        for t in &mut info.tracks {
            self.ft_in_title(t, &albumartist, &log);
        }
    }

    fn track_info(&self, info: &mut TrackInfo) {
        self.ft_in_title(info, "", &listener_log());
    }
}

fn listener_log() -> PluginLog {
    let log = PluginLog::new("ftintitle");
    log.set_base_level(WARNING);
    log
}

/// `_album_artist_no_feat`: the `album_artist_no_feat` album field.
pub fn album_artist_no_feat(albumartist: &str, custom_words: &[String]) -> String {
    split_on_feat(albumartist, false, custom_words).0
}

pub struct FtInTitle {
    pub core: Arc<FtCore>,
}

impl FtInTitle {
    /// Load the plugin; with `auto`, listen to fetched metadata.
    pub fn new(cfg: &Config) -> Self {
        let core = Arc::new(FtCore::new(cfg));
        if core.auto {
            crate::mb::add_info_listener(core.clone());
        }
        FtInTitle { core }
    }
}

impl Plugin for FtInTitle {
    fn name(&self) -> &'static str {
        "ftintitle"
    }

    /// `imported`: the import stage.
    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if !self.core.auto {
            return Ok(());
        }
        let log = listener_log();
        for item in task.items.iter_mut() {
            let albumartist = item.str("albumartist").to_string();
            if self.core.ft_in_title(item, &albumartist, &log) {
                ctx.lib.store(item)?;
            }
        }
        Ok(())
    }
}

/// `beet ftintitle [-d] [-l N] [QUERY]`.
pub fn command(
    cfg: &Config,
    lib: &Library,
    drop: bool,
    limit: Option<usize>,
    query: &[String],
) -> Result<()> {
    let mut core = FtCore::new(cfg);
    core.drop |= drop;
    let log = PluginLog::new("ftintitle");
    log.set_base_level(INFO);
    let write = cfg.bool("import.write");
    let mut items = lib.items(&Query::parse(Kind::Item, query)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    for item in &mut items {
        let albumartist = item.str("albumartist").to_string();
        if core.ft_in_title(item, &albumartist, &log) {
            lib.store(item)?;
            // `item.try_write()`, after the store (as beets: the new mtime
            // isn't stored).
            if write && let Err(e) = crate::media::write_item(item) {
                crate::logging::emit(format!("{e:#}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "ftintitle/tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "ftintitle",
        order: 7,
        defaults: r#"ftintitle:
    auto: yes
    drop: no
    format: feat. {}
    keep_in_artist: no
    preserve_album_artist: yes
    custom_words: []
    bracket_keywords:
    - abridged
    - acapella
    - club
    - demo
    - edit
    - edition
    - extended
    - instrumental
    - live
    - mix
    - radio
    - release
    - remaster
    - remastered
    - remix
    - rmx
    - unabridged
    - unreleased
    - version
    - vip
"#,
        config_keys: &["ftintitle"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "ftintitle",
                build: build_ftintitle,
                run: run_ftintitle,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(FtInTitle::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct FtintitleArgs {
    /// Drop featuring from artists and ignore title update
    #[arg(short = 'd', long)]
    drop: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_ftintitle() -> clap::Command {
    use clap::Args as _;
    FtintitleArgs::augment_args(
        clap::Command::new("ftintitle")
            .about("move featured artists to the title field"),
    )
}

pub fn run_ftintitle(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = FtintitleArgs::from_arg_matches(m)?;
    crate::plugins::ftintitle::command(cfg, lib, a.drop, a.limit, &a.query)
}
