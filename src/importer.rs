//! `beet import`: group files into albums, match them against MusicBrainz,
//! let the user choose, then copy/move, tag and add them to the library.

use crate::autotag::{self, AlbumMatch, Recommendation, TrackMatch};
use crate::config::Config;
use crate::duplicates::{self, DuplicateAction, Found, KeyValues};
use crate::fileops::{self, FileOp, Globs, WalkFilter, prune_dirs};
use crate::library::{Library, now};
use crate::mb::MusicBrainz;
use crate::media;
use crate::model::{Album, Item, Value, format_duration};
use crate::paths::PathFormatter;
use crate::plugins::{self, Plugin, PluginCtx};
use crate::state::ImportState;
use crate::template::{self, Template};
use crate::ui::{self, Choice};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub struct ImportOptions {
    pub op: FileOp,
    /// `import.delete`: remove the originals after copying them (copy only).
    pub delete: bool,
    pub write: bool,
    pub quiet: bool,
    pub quiet_fallback_asis: bool,
    pub timid: bool,
    pub autotag: bool,
    pub singletons: bool,
    /// Split every directory into albums by tags (`-g`), as "Group albums" does.
    pub group_albums: bool,
    pub search_ids: Vec<String>,
    pub pretend: bool,
    /// Skip directories recorded in the state file's history.
    pub incremental: bool,
    /// With `incremental`, don't record skipped albums, so they come back.
    pub incremental_skip_later: bool,
    pub resume: Resume,
    /// `import.set_fields` / `--set`: field name and value template.
    pub set_fields: Vec<(String, String)>,
    /// `import.log` / `-l`: where to log skipped and as-is albums.
    pub log: Option<PathBuf>,
    /// `--flat`: each import path is one album, however deep its files are.
    pub flat: bool,
    pub none_rec_action: NoneRecAction,
    /// `import.detail`: list every track in a match, not just changed ones.
    pub detail: bool,
    /// `import.from_scratch`: clear existing tags before applying a match.
    pub from_scratch: bool,
}

/// Flexible attributes that a reimport takes from the new match instead of
/// keeping the old value (beets' `REIMPORT_FRESH_FIELDS_ITEM`).
const REIMPORT_FRESH_FIELDS_ITEM: &[&str] = &[
    "data_source",
    "bandcamp_album_id",
    "spotify_album_id",
    "deezer_album_id",
    "beatport_album_id",
    "tidal_album_id",
    "tidal_track_id",
    "tidal_artist_id",
    "tidal_track_popularity",
    "tidal_album_popularity",
    "tidal_updated",
    "data_url",
];

/// beets' `_reduce_and_log`: copy the old record's flexible attributes onto
/// the new one, except fresh fields the new record sets to something else.
fn preserve_flex(new: &mut Item, old: &Item, fresh: &[&str]) {
    for (k, v) in &old.flex {
        let overwritten = fresh.contains(&k.as_str())
            && new
                .get(k)
                .is_some_and(|n| !n.raw().is_empty() && n.raw() != *v);
        if !overwritten {
            new.flex.insert(k.clone(), v.clone());
        }
    }
}

/// beets' `align_album_level_fields` for an as-is album: the most common
/// (albumartist, or artist) becomes every item's albumartist if enough of
/// the items share it (`SINGLE_ARTIST_THRESH`); otherwise it's a VA album.
fn align_asis(items: &mut [Item], va_name: &str) {
    const SINGLE_ARTIST_THRESH: f64 = 0.25;
    let mut counts: Vec<(String, usize)> = vec![];
    for i in items.iter() {
        let a = match i.str("albumartist") {
            "" => i.str("artist"),
            aa => aa,
        };
        match counts.iter_mut().find(|(k, _)| k == a) {
            Some((_, n)) => *n += 1,
            None => counts.push((a.to_string(), 1)),
        }
    }
    // Ties go to the first seen, as Python's `Counter.most_common`.
    let Some((artist, freq)) = counts.into_iter().rev().max_by_key(|(_, n)| *n) else {
        return;
    };
    let single = freq == items.len()
        || (freq > 1 && freq as f64 / items.len() as f64 >= SINGLE_ARTIST_THRESH);
    for i in items.iter_mut() {
        i.set_str(
            "albumartist",
            if single { artist.as_str() } else { va_name },
        );
        i.set("comp", Value::Bool(!single));
    }
}

/// beets' `clear()` for `from_scratch`: reset every tag-backed field.
fn clear_tags(item: &mut Item) {
    for f in media::TAG_FIELDS {
        if let Some(ty) = item.field_type(f) {
            item.set(f, ty.default_value());
        }
    }
}

/// Library rows a reimport replaces (beets' `record_replaced`): for each new
/// item, the existing items with the same path, plus their albums.
#[derive(Default)]
struct Replaced {
    items: Vec<Vec<Item>>,
    albums: Vec<Album>,
}

/// What the duplicate check decided.
enum DupOutcome {
    Proceed,
    Skip,
    /// "Merge all": the duplicates' items, to import together with the new ones.
    Merge(Vec<Item>),
    /// "Upgrade": import only these new items (indices); the superseded old
    /// ones are already gone. `album_id`: the surviving old album the kept
    /// items join.
    Upgrade {
        kept: Vec<usize>,
        album_id: Option<i64>,
    },
}

/// `import.none_rec_action`: what to do, outside quiet mode, when the
/// autotagger has no recommendation at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoneRecAction {
    Ask,
    AsIs,
    Skip,
}

/// Split a `--set` argument into field and value, as beets' `_store_dict`.
pub fn parse_set_field(kv: &str) -> Result<(&str, &str)> {
    match kv.split_once('=') {
        Some((k, v)) if !k.is_empty() && !v.is_empty() => Ok((k, v)),
        _ => bail!("supplied argument `{kv}' is not of the form `key=value'"),
    }
}

/// Template context for `set_fields`: the record's own fields, then (for
/// an item) its album's.
struct SetCtx<'a> {
    rec: &'a Item,
    album: Option<&'a Album>,
}

impl template::Context for SetCtx<'_> {
    fn field(&self, name: &str) -> Option<String> {
        if self.rec.has_field(name) {
            self.rec.formatted(name)
        } else {
            self.album.and_then(|a| a.formatted(name))
        }
    }
}

/// `import.resume`: what to do when an earlier import of a path was interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Yes,
    No,
    Ask,
}

/// How one album (or one directory of singletons) went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done,
    Skipped,
    Aborted,
}

impl ImportOptions {
    pub fn from_config(cfg: &Config) -> Self {
        ImportOptions {
            op: FileOp::for_import(cfg, cfg.bool("import.copy"), cfg.bool("import.move")),
            delete: cfg.bool("import.delete"),
            write: cfg.bool("import.write"),
            quiet: cfg.bool("import.quiet"),
            quiet_fallback_asis: cfg.str("import.quiet_fallback") == "asis",
            timid: cfg.bool("import.timid"),
            autotag: cfg.bool("import.autotag"),
            singletons: cfg.bool("import.singletons"),
            group_albums: cfg.bool("import.group_albums"),
            search_ids: vec![],
            pretend: cfg.bool("import.pretend"),
            incremental: cfg.bool("import.incremental"),
            incremental_skip_later: cfg.bool("import.incremental_skip_later"),
            resume: match cfg.str("import.resume").as_str() {
                "yes" | "true" => Resume::Yes,
                "no" | "false" => Resume::No,
                _ => Resume::Ask,
            },
            set_fields: cfg.mapping("import.set_fields"),
            // A relative path in the config file is relative to the config
            // dir (like confuse's `as_filename`); `-l` is relative to the cwd.
            log: match cfg.str("import.log") {
                s if s.is_empty() => None,
                s => Some(cfg.config_dir.join(crate::config::expand_tilde(&s))),
            },
            flat: cfg.bool("import.flat"),
            none_rec_action: match cfg.str("import.none_rec_action").as_str() {
                "asis" => NoneRecAction::AsIs,
                "skip" => NoneRecAction::Skip,
                _ => NoneRecAction::Ask,
            },
            detail: cfg.bool("import.detail"),
            from_scratch: cfg.bool("import.from_scratch"),
        }
    }
}

#[derive(Default, Debug)]
pub struct Stats {
    pub albums: usize,
    pub items: usize,
    pub skipped: usize,
    /// Directories passed over because an earlier import already handled them.
    pub already: usize,
}

enum Decision<M> {
    Apply(Box<M>),
    AsIs,
    Skip,
    Abort,
    /// "as Tracks": import the album's items as singletons (albums only).
    Tracks,
    /// "Group albums": split the items into albums by tags (albums only).
    Group,
}

/// The album prompt's choices after the candidate-specific ones, in beets'
/// order (`_get_choices` for an album task).
const ALBUM_CHOICES: [&str; 7] = [
    "Skip",
    "Use as-is",
    "as Tracks",
    "Group albums",
    "Enter search",
    "enter Id",
    "aBort",
];

/// Shown when confirming one candidate: "Apply, More candidates" + the rest.
const ALBUM_CONFIRM: [&str; 9] = [
    "Apply",
    "More candidates",
    "Skip",
    "Use as-is",
    "as Tracks",
    "Group albums",
    "Enter search",
    "enter Id",
    "aBort",
];

/// beets' `group_albums` stage: split items into albums keyed by
/// (albumartist, or artist when that's empty; album), in key order. Items
/// keep their relative order within a group.
pub fn group_albums(items: Vec<Item>) -> Vec<Vec<Item>> {
    let mut groups: BTreeMap<(String, String), Vec<Item>> = BTreeMap::new();
    for item in items {
        let artist = match item.str("albumartist") {
            "" => item.str("artist"),
            aa => aa,
        };
        let key = (artist.to_string(), item.str("album").to_string());
        groups.entry(key).or_default().push(item);
    }
    groups.into_values().collect()
}

pub struct Importer<'a> {
    cfg: &'a Config,
    lib: &'a Library,
    fmt: PathFormatter,
    mb: MusicBrainz,
    opts: ImportOptions,
    pub stats: Stats,
    roots: Vec<PathBuf>,
    plugins: &'a [Box<dyn Plugin>],
    state: ImportState,
    state_path: PathBuf,
    log: Option<std::fs::File>,
    /// Paths of library items already merged into another album this run.
    merged: HashSet<PathBuf>,
    clutter: Globs,
    walk: WalkFilter,
}

static DISC_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(cd|dis[ck]|side|part)[\s._-]*[0-9a-z]{1,3}\b").unwrap());

/// Group audio files into album-sized chunks: one per directory, merging
/// "CD1"/"Disc 2" style subdirectories into their parent. Each group comes
/// with its directory (or the file itself, for a single-file import), which
/// identifies it in the import state. Directory walks skip what `filter`
/// says to (`ignore`, `ignore_hidden`); a file given directly is never
/// skipped.
pub fn group_files(root: &Path, filter: &WalkFilter) -> Result<Vec<(PathBuf, Vec<PathBuf>)>> {
    if root.is_file() {
        return Ok(if media::is_audio(root) {
            vec![(root.to_path_buf(), vec![root.to_path_buf()])]
        } else {
            vec![]
        });
    }
    if !root.is_dir() {
        bail!("no such file or directory: {}", root.display());
    }
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !filter.skips(e.file_name()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("warning: {e}");
                continue;
            }
        };
        if entry.file_type().is_file() && media::is_audio(entry.path()) {
            let dir = entry.path().parent().unwrap_or(root).to_path_buf();
            by_dir
                .entry(dir)
                .or_default()
                .push(entry.path().to_path_buf());
        }
    }
    // Merge disc subdirectories into the parent when the parent has no audio
    // of its own and every audio subdirectory looks like a disc.
    let mut groups: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for (dir, files) in &by_dir {
        let key = match dir.parent() {
            Some(parent) if dir != root && !by_dir.contains_key(parent) => {
                let siblings: Vec<&PathBuf> = by_dir
                    .keys()
                    .filter(|d| d.parent() == Some(parent))
                    .collect();
                let all_discs = siblings.iter().all(|d| {
                    d.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| DISC_DIR.is_match(n))
                });
                if all_discs && siblings.len() > 1 {
                    parent.to_path_buf()
                } else {
                    dir.clone()
                }
            }
            _ => dir.clone(),
        };
        groups.entry(key).or_default().extend(files.iter().cloned());
    }
    Ok(groups.into_iter().collect())
}

fn file_label(item: &Item) -> String {
    item.path()
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn len_str(secs: Option<f64>) -> String {
    secs.map(|s| format!(" ({})", format_duration(s)))
        .unwrap_or_default()
}

impl<'a> Importer<'a> {
    pub fn new(
        cfg: &'a Config,
        lib: &'a Library,
        opts: ImportOptions,
        plugins: &'a [Box<dyn Plugin>],
    ) -> Result<Self> {
        let log = match &opts.log {
            Some(path) => Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|_| {
                        anyhow::anyhow!("Could not open log file for writing: {}", path.display())
                    })?,
            ),
            None => None,
        };
        Ok(Importer {
            log,
            merged: HashSet::new(),
            clutter: fileops::clutter(cfg),
            walk: WalkFilter::from_config(cfg),
            cfg,
            lib,
            fmt: PathFormatter::new(cfg)?,
            mb: MusicBrainz::new(cfg),
            opts,
            stats: Stats::default(),
            roots: vec![],
            plugins,
            state: ImportState::load(&cfg.state_path()),
            state_path: cfg.state_path(),
        })
    }

    pub fn run(&mut self, paths: &[PathBuf]) -> Result<()> {
        if self.log.is_some() {
            // Python's time.asctime(), as in beets' "import started" line.
            let now = chrono::Local::now().format("%a %b %e %H:%M:%S %Y");
            self.log_line(&format!("import started {now}"));
        }
        // As in beets: incremental imports use history instead of progress,
        // `resume: no` neither resumes nor records progress, and quiet mode
        // never asks, so `resume: ask` behaves like `no` there.
        let track_progress = !self.opts.incremental
            && !self.opts.pretend
            && match self.opts.resume {
                Resume::Yes => true,
                Resume::Ask => !self.opts.quiet,
                Resume::No => false,
            };
        for p in paths {
            let p = std::fs::canonicalize(p)
                .with_context(|| format!("no such file or directory: {}", p.display()))?;
            self.roots.push(if p.is_dir() {
                p.clone()
            } else {
                p.parent().unwrap_or(&p).to_path_buf()
            });
            let done = if track_progress {
                self.resume_point(&p)?
            } else {
                if self.opts.resume == Resume::No
                    && !self.opts.pretend
                    && self.state.progress.remove(&p).is_some()
                {
                    // Explicitly starting over: drop the stale progress so a
                    // later run doesn't offer to resume it.
                    self.state.save(&self.state_path)?;
                }
                Default::default()
            };
            let groups = if self.opts.flat && p.is_dir() {
                let files = group_files(&p, &self.walk)?
                    .into_iter()
                    .flat_map(|(_, f)| f)
                    .collect();
                vec![(p.clone(), files)]
            } else {
                group_files(&p, &self.walk)?
            };
            for (key, group) in groups {
                if done.contains(&key) {
                    self.stats.already += 1;
                    continue;
                }
                if self.opts.incremental && self.state.history.contains(&key) {
                    println!(
                        "{}",
                        ui::dim(&format!(
                            "Skipping previously-imported path: {}",
                            key.display()
                        ))
                    );
                    self.stats.already += 1;
                    continue;
                }
                if self.opts.pretend {
                    self.log_files(&key, &group)?;
                    continue;
                }
                let outcome = if self.opts.singletons {
                    let mut outcome = Outcome::Done;
                    for f in group {
                        match self.import_singleton(&f)? {
                            Outcome::Done => {}
                            Outcome::Skipped => outcome = Outcome::Skipped,
                            Outcome::Aborted => {
                                outcome = Outcome::Aborted;
                                break;
                            }
                        }
                    }
                    outcome
                } else {
                    self.import_album(&key, &group)?
                };
                if outcome == Outcome::Aborted {
                    // Progress so far stays recorded, so the import can resume.
                    println!("Import aborted.");
                    return Ok(());
                }
                self.record(&p, key, outcome, track_progress)?;
            }
            if track_progress && self.state.progress.remove(&p).is_some() {
                self.state.save(&self.state_path)?;
            }
        }
        Ok(())
    }

    /// The groups an interrupted earlier import of `top` already finished,
    /// if the user wants to resume it; otherwise forget that progress.
    fn resume_point(&mut self, top: &Path) -> Result<std::collections::BTreeSet<PathBuf>> {
        let Some(done) = self.state.progress.get(top) else {
            return Ok(Default::default());
        };
        let resume = match self.opts.resume {
            Resume::Yes => true,
            Resume::No => false,
            Resume::Ask => ui::yes_no(&format!(
                "Import of the directory:\n{}\nwas interrupted. Resume?",
                top.display()
            )),
        };
        if resume {
            println!(
                "Resuming import of {} ({} album(s) already done).",
                top.display(),
                done.len()
            );
            return Ok(done.clone());
        }
        self.state.progress.remove(top);
        self.state.save(&self.state_path)?;
        Ok(Default::default())
    }

    /// Note that a group is finished, in the history (incremental) or the
    /// top path's progress, and persist it right away so an interruption
    /// loses at most the album in flight.
    fn record(&mut self, top: &Path, key: PathBuf, outcome: Outcome, progress: bool) -> Result<()> {
        if self.opts.pretend {
            return Ok(());
        }
        if self.opts.incremental {
            if outcome == Outcome::Skipped && self.opts.incremental_skip_later {
                return Ok(());
            }
            self.state.history.insert(key);
        } else if progress {
            self.state
                .progress
                .entry(top.to_path_buf())
                .or_default()
                .insert(key);
        }
        self.state.save(&self.state_path)
    }

    fn read_items(&mut self, files: &[PathBuf]) -> Result<Vec<Item>> {
        let mut items = vec![];
        for f in files {
            // Files already in the library are reimported, replacing their
            // rows, unless a merge already took them this run.
            if self.merged.contains(f) {
                continue;
            }
            match media::read(f) {
                Ok(i) => items.push(i),
                Err(e) => eprintln!("warning: {e:#}"),
            }
        }
        items.sort_by_key(|i| (i.int("disc"), i.int("track"), i.path().to_path_buf()));
        Ok(items)
    }

    fn thresholds(&self) -> (f64, f64) {
        (
            self.cfg.f64("match.strong_rec_thresh"),
            self.cfg.f64("match.medium_rec_thresh"),
        )
    }

    // ------------------------------------------------------------- albums

    /// `-L`: reimport library items matching `query`, grouped into their
    /// albums (or one by one with `-s`), as beets' `query_tasks`. There is
    /// no resume or incremental state for query imports.
    pub fn run_library(&mut self, query: &[String]) -> Result<()> {
        use crate::model::Kind;
        use crate::query::Query;
        if self.log.is_some() {
            let now = chrono::Local::now().format("%a %b %e %H:%M:%S %Y");
            self.log_line(&format!("import started {now}"));
        }
        self.roots.push(self.cfg.directory());
        if self.opts.singletons {
            for item in self.lib.items(&Query::parse(Kind::Item, query)?)? {
                if self.opts.pretend {
                    println!("Singleton: {}", item.path().display());
                    continue;
                }
                let mut item = item;
                item.id = None;
                if self.import_singleton_item(item)? == Outcome::Aborted {
                    println!("Import aborted.");
                    return Ok(());
                }
            }
            return Ok(());
        }
        for album in self.lib.albums(&Query::parse(Kind::Album, query)?)? {
            // Re-read from the database: an earlier merge may have taken it.
            let items: Vec<Item> = self
                .lib
                .album_items(album.id.unwrap())?
                .into_iter()
                .filter(|i| !self.merged.contains(i.path()))
                .map(|mut i| {
                    // beets' `_freshen_items`.
                    i.id = None;
                    i.set_int("album_id", 0);
                    i
                })
                .collect();
            let Some(dir) = items
                .first()
                .and_then(|i| i.path().parent())
                .map(Path::to_path_buf)
            else {
                continue;
            };
            if self.opts.pretend {
                println!("Album: {}", dir.display());
                for i in &items {
                    println!("  {}", i.path().display());
                }
                continue;
            }
            let outcome = if self.opts.group_albums {
                self.import_groups(items, &dir)?
            } else {
                self.import_album_items(items, &dir, std::slice::from_ref(&dir))?
            };
            if outcome == Outcome::Aborted {
                println!("Import aborted.");
                return Ok(());
            }
        }
        Ok(())
    }

    /// `--pretend`: list what would be imported, as beets' `log_files`.
    fn log_files(&mut self, key: &Path, files: &[PathBuf]) -> Result<()> {
        let items = self.read_items(files)?;
        if self.opts.singletons {
            for i in &items {
                println!("Singleton: {}", i.path().display());
            }
        } else if !items.is_empty() {
            println!("Album: {}", key.display());
            for i in &items {
                println!("  {}", i.path().display());
            }
        }
        Ok(())
    }

    /// Append a line to the import log, if there is one.
    fn log_line(&mut self, line: &str) {
        if let Some(f) = &mut self.log
            && let Err(e) = writeln!(f, "{line}")
        {
            eprintln!("warning: writing import log: {e}");
        }
    }

    /// beets' `tag_log`: a status and the task's paths, `; `-separated.
    fn tag_log(&mut self, status: &str, paths: &[PathBuf]) {
        let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        self.log_line(&format!("{status} {}", paths.join("; ")));
    }

    /// `key` is the album's directory (or the import path with `--flat`).
    fn import_album(&mut self, key: &Path, files: &[PathBuf]) -> Result<Outcome> {
        let items = self.read_items(files)?;
        if items.is_empty() {
            return Ok(Outcome::Done);
        }
        let dir = if self.opts.flat && key.is_dir() {
            key.to_path_buf()
        } else {
            files[0].parent().unwrap_or(Path::new("")).to_path_buf()
        };
        if self.opts.group_albums {
            self.import_groups(items, &dir)
        } else {
            self.import_album_items(items, &dir, &[key.to_path_buf()])
        }
    }

    /// "Group albums" and `-g`: run each tag-defined album in the directory
    /// through the album flow. An abort stops at once; otherwise the
    /// directory counts as skipped if any of its albums was.
    fn import_groups(&mut self, items: Vec<Item>, dir: &Path) -> Result<Outcome> {
        let mut outcome = Outcome::Done;
        for group in group_albums(items) {
            // Grouped by tag, so (as in beets) the log names the files.
            let paths: Vec<PathBuf> = group.iter().map(|i| i.path().to_path_buf()).collect();
            match self.import_album_items(group, dir, &paths)? {
                Outcome::Done => {}
                Outcome::Skipped => outcome = Outcome::Skipped,
                Outcome::Aborted => return Ok(Outcome::Aborted),
            }
        }
        Ok(outcome)
    }

    /// "as Tracks": each item goes through the singleton flow. Skipping some
    /// tracks still leaves the album done, as beets has no album task left.
    fn import_as_tracks(&mut self, items: Vec<Item>) -> Result<Outcome> {
        for item in items {
            if self.import_singleton_item(item)? == Outcome::Aborted {
                return Ok(Outcome::Aborted);
            }
        }
        Ok(Outcome::Done)
    }

    /// `paths` identify the album in the import log.
    fn import_album_items(
        &mut self,
        mut items: Vec<Item>,
        dir: &Path,
        paths: &[PathBuf],
    ) -> Result<Outcome> {
        println!();
        println!("{} ({} items)", dir.display(), items.len());

        let decision = if self.opts.autotag {
            let d = self.choose_album(&items)?;
            self.log_choice(&d, paths);
            d
        } else {
            Decision::AsIs
        };
        let m = match decision {
            Decision::Abort => return Ok(Outcome::Aborted),
            Decision::Skip => {
                self.stats.skipped += 1;
                return Ok(Outcome::Skipped);
            }
            Decision::AsIs => None,
            Decision::Apply(m) => Some(m),
            Decision::Tracks => return self.import_as_tracks(items),
            Decision::Group => return self.import_groups(items, dir),
        };

        // Apply the choice to a copy: a merge starts over from the
        // untouched items, and the duplicate check and upgrade compare
        // against the new metadata.
        let album_keys = self.cfg.list("import.duplicate_keys.album");
        let mut applied = items.clone();
        let want = if let Some(m) = &m {
            if self.opts.from_scratch {
                for &(i, _) in &m.mapping {
                    clear_tags(&mut applied[i]);
                }
            }
            // Items with no matching track are dropped, as in beets.
            autotag::apply_album(self.cfg, &mut applied, m);
            let keep: Vec<usize> = m.mapping.iter().map(|(i, _)| *i).collect();
            let mut idx = 0;
            applied.retain(|_| {
                let k = keep.contains(&idx);
                idx += 1;
                k
            });
            match applied.first() {
                Some(first) => duplicates::key_values(first, &album_keys),
                None => duplicates::likely_key_values(&items, &album_keys),
            }
        } else {
            align_asis(&mut applied, &self.cfg.str("va_name"));
            duplicates::likely_key_values(&items, &album_keys)
        };

        let mut graft = None;
        match self.resolve_duplicates(true, &applied, &items, &want, paths)? {
            DupOutcome::Proceed => {}
            DupOutcome::Skip => {
                self.stats.skipped += 1;
                return Ok(Outcome::Skipped);
            }
            DupOutcome::Merge(dups) => {
                // As beets: tag the new and old items together as one album;
                // the old rows are then replaced like any reimport.
                let mut paths = paths.to_vec();
                for d in &dups {
                    self.merged.insert(d.path().to_path_buf());
                    paths.push(d.path().to_path_buf());
                }
                items.extend(dups);
                return self.import_album_items(items, dir, &paths);
            }
            DupOutcome::Upgrade { kept, album_id } => {
                let mut idx = 0;
                applied.retain(|_| {
                    let k = kept.contains(&idx);
                    idx += 1;
                    k
                });
                graft = album_id;
            }
        }
        self.finalize_album(applied, m.is_some(), dir, graft)?;
        Ok(Outcome::Done)
    }

    fn choose_album(&mut self, items: &[Item]) -> Result<Decision<AlbumMatch>> {
        let cur = autotag::current_metadata(items);
        println!("Tagging:\n    {} - {}", cur.artist, cur.album);
        let ids = self.opts.search_ids.clone();
        let (mut cands, mut rec) =
            autotag::album_candidates(self.cfg, &self.mb, items, None, &ids)?;

        // `fresh`: candidates just (re)fetched, so quiet/strong handling
        // applies. `focus`: the candidate shown for confirmation, with
        // whether Apply is the default (beets requires an explicit choice
        // for anything but the first); None shows the candidate list.
        let mut fresh = true;
        let mut focus: Option<(usize, bool)> = None;
        // As in beets, the summary judgment covers only the first lookup.
        if let Some(d) = self.summary_judgment(rec) {
            return Ok(d);
        }
        loop {
            if fresh {
                fresh = false;
                if !cands.is_empty() && rec == Recommendation::Strong && !self.opts.timid {
                    self.show_album_match(items, &cands[0]);
                    return Ok(Decision::Apply(Box::new(cands.swap_remove(0))));
                }
                // As beets: show the best match directly unless there's no
                // recommendation; at low, Apply isn't the default.
                focus = (!cands.is_empty() && rec != Recommendation::None)
                    .then_some((0, rec > Recommendation::Low));
            }

            let choice = if let Some((k, default_apply)) = focus {
                self.show_album_match(items, &cands[k]);
                self.bell();
                match ui::options(
                    &ALBUM_CONFIRM,
                    None,
                    default_apply.then_some(Choice::Letter('a')),
                ) {
                    Some(Choice::Letter('a')) => {
                        return Ok(Decision::Apply(Box::new(cands.swap_remove(k))));
                    }
                    Some(Choice::Letter('m')) => {
                        focus = None;
                        continue;
                    }
                    other => other,
                }
            } else {
                if cands.is_empty() {
                    println!(
                        "{}",
                        ui::yellow(&format!(
                            "No matching release found for {} tracks.",
                            items.len()
                        ))
                    );
                }
                self.pick_album_candidate(&cands)?
            };

            match choice {
                None | Some(Choice::Letter('b')) => return Ok(Decision::Abort),
                Some(Choice::Number(n)) => {
                    focus = Some((n - 1, n == 1 && rec > Recommendation::Low));
                }
                Some(Choice::Letter('s')) => return Ok(Decision::Skip),
                Some(Choice::Letter('u')) => return Ok(Decision::AsIs),
                Some(Choice::Letter('t')) => return Ok(Decision::Tracks),
                Some(Choice::Letter('g')) => return Ok(Decision::Group),
                Some(Choice::Letter('e')) => {
                    let artist = ui::read_line("Artist: ").unwrap_or_default();
                    let album = ui::read_line("Album: ").unwrap_or_default();
                    (cands, rec) = autotag::album_candidates(
                        self.cfg,
                        &self.mb,
                        items,
                        Some((&artist, &album)),
                        &[],
                    )?;
                    fresh = true;
                }
                Some(Choice::Letter('i')) => {
                    let id = ui::read_line("Enter release ID: ").unwrap_or_default();
                    (cands, rec) =
                        autotag::album_candidates(self.cfg, &self.mb, items, None, &[id])?;
                    // An explicitly chosen release is never auto-applied.
                    rec = rec.min(Recommendation::Medium);
                    fresh = true;
                }
                _ => {}
            }
        }
    }

    fn pick_album_candidate(&self, cands: &[AlbumMatch]) -> Result<Option<Choice>> {
        let (strong, medium) = self.thresholds();
        if !cands.is_empty() {
            println!("Candidates:");
        }
        for (n, m) in cands.iter().enumerate() {
            let d = m.distance.distance();
            let keys = m.distance.keys();
            let penalty = if keys.is_empty() {
                String::new()
            } else {
                format!(" {}", ui::yellow(&format!("({})", keys.join(", "))))
            };
            println!(
                "{}. {} - {} ({}){}{}",
                n + 1,
                m.info.artist,
                m.info.album,
                ui::similarity(d, strong, medium),
                penalty,
                ui::dim(&disambig(&m.info))
            );
        }
        let numbers = if cands.is_empty() {
            None
        } else {
            Some(cands.len())
        };
        Ok(ui::options(
            &ALBUM_CHOICES,
            numbers,
            numbers.map(|_| Choice::Number(1)),
        ))
    }

    fn show_album_match(&self, items: &[Item], m: &AlbumMatch) {
        let (strong, medium) = self.thresholds();
        let cur = autotag::current_metadata(items);
        let info = &m.info;
        let keys = m.distance.keys();
        let penalty = if keys.is_empty() {
            String::new()
        } else {
            format!(" {}", ui::yellow(&format!("({})", keys.join(", "))))
        };
        println!();
        println!(
            "  Match ({}){}:",
            ui::similarity(m.distance.distance(), strong, medium),
            penalty
        );
        println!(
            "  {}",
            ui::bold(&format!("{} - {}", info.artist, info.album))
        );
        println!(
            "  {}",
            ui::dim(&format!(
                "https://musicbrainz.org/release/{}",
                info.album_id
            ))
        );
        let d = disambig(info);
        if !d.is_empty() {
            println!("  {}", ui::dim(d.trim()));
        }
        if !cur.va && !cur.artist.is_empty() && cur.artist != info.artist {
            println!(" * Artist: {} -> {}", cur.artist, ui::yellow(&info.artist));
        }
        if !cur.album.is_empty() && cur.album != info.album {
            println!(" * Album: {} -> {}", cur.album, ui::yellow(&info.album));
        }
        let mut pairs = m.mapping.clone();
        pairs.sort_by_key(|(_, t)| *t);
        for (i, t) in pairs {
            let item = &items[i];
            let track = &info.tracks[t];
            let old_title = if item.str("title").is_empty() {
                file_label(item)
            } else {
                item.str("title").to_string()
            };
            let idx_changed = item.int("track") as usize != track.index
                && item.int("track") as usize != track.medium_index;
            let title_changed = old_title != track.title;
            let len_off = track
                .length
                .is_some_and(|l| (l - item.float("length")).abs() > 10.0);
            if !(title_changed || idx_changed || len_off) {
                if self.opts.detail {
                    println!("   {} (#{})", old_title, item.int("track"));
                }
                continue;
            }
            let td = autotag::track_distance(self.cfg, item, track, info.va).distance();
            let new = format!("{} (#{})", track.title, track.index);
            let new = if td <= strong {
                new
            } else if td <= medium {
                ui::yellow(&new)
            } else {
                ui::red(&new)
            };
            println!(
                " * {} (#{}){} -> {}{}",
                old_title,
                item.int("track"),
                if len_off {
                    len_str(Some(item.float("length")))
                } else {
                    String::new()
                },
                new,
                if len_off {
                    len_str(track.length)
                } else {
                    String::new()
                },
            );
        }
        if !m.extra_tracks.is_empty() {
            let pct = m.extra_tracks.len() as f64 / info.tracks.len() as f64 * 100.0;
            println!(
                "{}",
                ui::yellow(&format!(
                    "Missing tracks ({}/{} - {pct:.1}%):",
                    m.extra_tracks.len(),
                    info.tracks.len()
                ))
            );
            for &t in &m.extra_tracks {
                let tr = &info.tracks[t];
                println!(" ! {} (#{}){}", tr.title, tr.index, len_str(tr.length));
            }
        }
        if !m.extra_items.is_empty() {
            println!(
                "{}",
                ui::yellow(&format!("Unmatched tracks ({}):", m.extra_items.len()))
            );
            for &i in &m.extra_items {
                let it = &items[i];
                let title = if it.str("title").is_empty() {
                    file_label(it)
                } else {
                    it.str("title").to_string()
                };
                println!(
                    " ! {} (#{}){}",
                    title,
                    it.int("track"),
                    len_str(Some(it.float("length")))
                );
            }
        }
    }

    /// Remove a duplicate item from the library, deleting its file when
    /// it's inside the library directory (beets' `remove_duplicates`).
    fn remove_duplicate_item(&self, it: &Item) -> Result<()> {
        let libdir = self.cfg.directory();
        if it.path().starts_with(&libdir) {
            std::fs::remove_file(it.path()).ok();
            if let Some(p) = it.path().parent() {
                prune_dirs(p, &libdir, &self.clutter);
            }
        }
        self.lib.remove(it)
    }

    /// Remove a duplicate album row, and its art file if that's inside the
    /// library directory.
    fn remove_duplicate_album(&self, id: i64) -> Result<()> {
        let Some(a) = self.lib.get_album(id)? else {
            return Ok(());
        };
        let libdir = self.cfg.directory();
        let art = a.path_field("artpath");
        if !art.as_os_str().is_empty() && art.starts_with(&libdir) && art.is_file() {
            std::fs::remove_file(art).ok();
            if let Some(p) = art.parent() {
                prune_dirs(p, &libdir, &self.clutter);
            }
        }
        self.lib.remove(&a)
    }

    /// The albums (or, for a track, singletons) already in the library with
    /// the same `import.duplicate_keys` values (beets' `find_duplicates`).
    /// An album whose files are all part of this import is being reimported,
    /// not duplicated; so is the track's own row.
    fn find_duplicates(&self, album: bool, task: &[Item], want: &KeyValues) -> Result<Vec<Found>> {
        let task_paths: HashSet<&Path> = task.iter().map(|i| i.path()).collect();
        let mut found = vec![];
        if album {
            for a in self.lib.all_albums()? {
                if !duplicates::matches(&a, want) {
                    continue;
                }
                let album_items = self.lib.album_items(a.id.unwrap())?;
                if !album_items.iter().all(|i| task_paths.contains(i.path())) {
                    found.push(Found {
                        album_id: a.id,
                        items: album_items,
                    });
                }
            }
        } else {
            let all = crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[])?;
            for it in self.lib.items(&all)? {
                if it.int("album_id") == 0
                    && !task_paths.contains(it.path())
                    && duplicates::matches(&it, want)
                {
                    found.push(Found {
                        album_id: None,
                        items: vec![it],
                    });
                }
            }
        }
        Ok(found)
    }

    /// beets' `_resolve_duplicates`: look for the album (`album`) or track
    /// already in the library and settle what to do. `new` are the items
    /// as they'll be imported, `task` all of the task's original items,
    /// `want` the duplicate key values; `paths` identify it in the log.
    fn resolve_duplicates(
        &mut self,
        album: bool,
        new: &[Item],
        task: &[Item],
        want: &KeyValues,
        paths: &[PathBuf],
    ) -> Result<DupOutcome> {
        let found = self.find_duplicates(album, task, want)?;
        if found.is_empty() {
            return Ok(DupOutcome::Proceed);
        }
        crate::logging::debug!(
            "found duplicates: {:?}",
            found
                .iter()
                .map(|f| f.album_id.or(f.items[0].id).unwrap_or(0))
                .collect::<Vec<_>>()
        );
        let mut action = DuplicateAction::from_config(&self.cfg.str("import.duplicate_action"));
        if action == DuplicateAction::Ask {
            let what = if album { "album" } else { "track" };
            println!(
                "{}",
                ui::yellow(&format!("This {what} is already in the library!"))
            );
            action = if self.opts.quiet {
                DuplicateAction::Skip
            } else {
                let verbose = self
                    .cfg
                    .bool("import.duplicate_verbose_prompt")
                    .then(|| Template::parse(&self.cfg.str("format_item")));
                for dup in &found {
                    for line in duplicates::report("Old", &dup.items, !album, verbose.as_ref()) {
                        println!("{line}");
                    }
                }
                for line in duplicates::report("New", new, !album, verbose.as_ref()) {
                    println!("{line}");
                }
                match ui::options(&DuplicateAction::PROMPT, None, None) {
                    Some(Choice::Letter(c)) => DuplicateAction::from_letter(c),
                    _ => DuplicateAction::Skip,
                }
            };
        }
        let mut upgrade = None;
        if action == DuplicateAction::Upgrade {
            let keys = self.cfg.list("import.duplicate_keys.item");
            let u = duplicates::resolve_upgrade_target(new, &found, &keys);
            if u.kept.is_empty() {
                // No track improves on what's there: beets declines.
                action = DuplicateAction::Skip;
            } else {
                upgrade = Some(u);
            }
        }
        // As beets' `log_choice(task, duplicate=True)`: a merge or upgrade
        // keeps both.
        self.tag_log(
            match action {
                DuplicateAction::Remove => "duplicate-replace",
                DuplicateAction::Skip => "duplicate-skip",
                _ => "duplicate-keep",
            },
            paths,
        );
        match action {
            DuplicateAction::Keep | DuplicateAction::Ask => Ok(DupOutcome::Proceed),
            DuplicateAction::Merge => {
                let mut merged = vec![];
                for dup in found {
                    for mut it in dup.items {
                        // Make them look fresh, as beets' `_freshen_items`.
                        it.id = None;
                        it.set_int("album_id", 0);
                        merged.push(it);
                    }
                }
                Ok(DupOutcome::Merge(merged))
            }
            DuplicateAction::Remove => {
                for dup in found {
                    for it in &dup.items {
                        self.remove_duplicate_item(it)?;
                    }
                    if let Some(id) = dup.album_id {
                        self.remove_duplicate_album(id)?;
                    }
                }
                Ok(DupOutcome::Proceed)
            }
            DuplicateAction::Upgrade => {
                let u = upgrade.expect("set for upgrades");
                for it in &u.superseded {
                    self.remove_duplicate_item(it)?;
                }
                // The kept tracks join the old album if any of it survives;
                // otherwise it goes (beets' `_remove_upgrade_duplicates`).
                let mut album_id = None;
                if let Some(id) = u.album_id {
                    if self.lib.album_items(id)?.is_empty() {
                        self.remove_duplicate_album(id)?;
                    } else {
                        album_id = Some(id);
                    }
                }
                Ok(DupOutcome::Upgrade {
                    kept: u.kept,
                    album_id,
                })
            }
            DuplicateAction::Skip => {
                println!("Skipping.");
                Ok(DupOutcome::Skip)
            }
        }
    }

    /// beets' `record_replaced`.
    fn record_replaced(&self, items: &[Item]) -> Result<Replaced> {
        let mut r = Replaced::default();
        let mut album_ids = HashSet::new();
        for item in items {
            let dups = self.lib.items_with_path(item.path())?;
            for d in &dups {
                let id = d.int("album_id");
                if id != 0
                    && album_ids.insert(id)
                    && let Some(a) = self.lib.get_album(id)?
                {
                    r.albums.push(a);
                }
            }
            r.items.push(dups);
        }
        Ok(r)
    }

    /// beets' `remove_replaced`: drop the old item rows, and their albums
    /// once empty.
    fn remove_replaced(&self, r: &Replaced) -> Result<()> {
        for d in r.items.iter().flatten() {
            self.lib.remove(d)?;
        }
        for a in &r.albums {
            if self.lib.album_items(a.id.unwrap())?.is_empty() {
                self.lib.remove(a)?;
            }
        }
        Ok(())
    }

    /// beets' `reimport_metadata`: keep `added`, flexible attributes and
    /// (for albums) the art path from the rows being replaced. The old album
    /// is the one whose files were in the new album's directory.
    fn reimport_metadata(&self, album: Option<&mut Album>, items: &mut [Item], r: &Replaced) {
        if let Some(album) = album
            && let Some(dir) = items.first().and_then(|i| i.path().parent())
        {
            let old = r.albums.iter().find(|a| {
                r.items
                    .iter()
                    .flatten()
                    .any(|d| d.int("album_id") == a.id.unwrap() && d.path().parent() == Some(dir))
            });
            if let Some(old) = old {
                let mut fresh = REIMPORT_FRESH_FIELDS_ITEM.to_vec();
                fresh.push("media");
                album.set("added", Value::Float(old.float("added")));
                preserve_flex(album, old, &fresh);
                album.set(
                    "artpath",
                    Value::Path(old.path_field("artpath").to_path_buf()),
                );
            }
        }
        for (item, dups) in items.iter_mut().zip(&r.items) {
            for d in dups {
                if d.float("added") != 0.0 {
                    item.set("added", Value::Float(d.float("added")));
                }
                preserve_flex(item, d, REIMPORT_FRESH_FIELDS_ITEM);
            }
        }
    }

    /// The file operation for one item: as beets' `manipulate_files`, in
    /// copy and link modes a reimported file already inside the library is
    /// moved, so it's never duplicated (or linked to itself).
    fn file_op(&self, item: &Item, replaces: bool) -> FileOp {
        if self.opts.op.keeps_source()
            && self.opts.op != FileOp::None
            && replaces
            && item.path().starts_with(self.cfg.directory())
        {
            FileOp::Move
        } else {
            self.opts.op
        }
    }

    /// Place files, write tags and store the album + items in the library.
    fn finalize_album(
        &mut self,
        mut items: Vec<Item>,
        tagged: bool,
        source_dir: &Path,
        graft: Option<i64>,
    ) -> Result<()> {
        let added = now();
        for i in &mut items {
            i.set("added", Value::Float(added));
        }
        let n = items.len();
        let lib = self.lib;
        let mut moved_in_library = false;
        let mut copied = vec![];
        let mut album = lib.transaction(|lib| {
            let replaced = self.record_replaced(&items)?;
            self.remove_replaced(&replaced)?;
            let mut album = match graft {
                // An upgrade adds its tracks to the surviving old album; as
                // in beets, `set_fields` doesn't touch that album.
                Some(id) => {
                    let album = lib
                        .get_album(id)?
                        .with_context(|| format!("album {id} vanished"))?;
                    for item in &mut items {
                        item.set_int("album_id", id);
                        lib.add(item)?;
                    }
                    album
                }
                None => lib.add_album(&mut items)?,
            };
            self.reimport_metadata(Some(&mut album), &mut items, &replaced);
            if graft.is_some() {
                self.apply_set_fields(None, &mut items);
            } else {
                self.apply_set_fields(Some(&mut album), &mut items);
            }
            lib.store(&album)?;
            for (item, dups) in items.iter_mut().zip(&replaced.items) {
                let op = self.file_op(item, !dups.is_empty());
                moved_in_library |= op == FileOp::Move && !dups.is_empty();
                copied.extend(self.place_and_write(item, Some(&album), tagged, op)?);
                lib.store(item)?;
            }
            Ok(album)
        })?;
        if moved_in_library {
            // Kept art follows its reimported album, as beets' `Item.move`.
            crate::commands::sync_art(self.cfg, lib, album.id.unwrap())?;
            if let Some(a) = lib.get_album(album.id.unwrap())? {
                album = a;
            }
        }
        let ctx = self.plugin_ctx();
        let op = self.opts.op;
        plugins::each(self.plugins, |p| {
            p.album_imported(&ctx, &mut album, &mut items, source_dir, op)
        });
        self.delete_originals(&copied);
        if op == FileOp::Move
            && let Some(root) = self.roots.last()
        {
            prune_dirs(source_dir, root, &self.clutter);
        }
        self.stats.albums += 1;
        self.stats.items += n;
        Ok(())
    }

    /// beets' `set_fields`: evaluate each value template against the album
    /// and against each item, before files are placed and tags written.
    fn apply_set_fields(&self, mut album: Option<&mut Album>, items: &mut [Item]) {
        let set = |rec: &mut Item, field: &str, value: String| {
            if let Err(e) = rec.set_parsed(field, &value) {
                eprintln!("warning: can't set {field}={value}: {e:#}");
            }
        };
        for (field, value) in &self.opts.set_fields {
            let t = Template::parse(value);
            if let Some(a) = album.as_deref_mut() {
                let v = t.render(&SetCtx {
                    rec: a,
                    album: None,
                });
                set(a, field, v);
            }
            for item in items.iter_mut() {
                let v = t.render(&SetCtx {
                    rec: item,
                    album: album.as_deref(),
                });
                set(item, field, v);
            }
        }
    }

    /// beets' `log_choice` for a prompt decision: skips and as-is imports.
    fn log_choice<M>(&mut self, d: &Decision<M>, paths: &[PathBuf]) {
        match d {
            Decision::Skip => self.tag_log("skip", paths),
            Decision::AsIs => self.tag_log("asis", paths),
            _ => {}
        }
    }

    /// beets' `_summary_judgment`: decide without asking, in quiet mode or
    /// per `none_rec_action`. None means ask the user, or (quiet with a
    /// strong recommendation) apply the best match.
    fn summary_judgment<M>(&self, rec: Recommendation) -> Option<Decision<M>> {
        let asis = if self.opts.quiet {
            if rec == Recommendation::Strong {
                return None;
            }
            self.opts.quiet_fallback_asis
        } else if self.opts.timid || rec != Recommendation::None {
            return None;
        } else {
            match self.opts.none_rec_action {
                NoneRecAction::Ask => return None,
                NoneRecAction::AsIs => true,
                NoneRecAction::Skip => false,
            }
        };
        println!(
            "{}",
            if asis {
                "Importing as-is."
            } else {
                "Skipping."
            }
        );
        Some(if asis { Decision::AsIs } else { Decision::Skip })
    }

    fn plugin_ctx(&self) -> PluginCtx<'a> {
        PluginCtx {
            cfg: self.cfg,
            lib: self.lib,
            write: self.opts.write,
        }
    }

    /// beets' `import.delete` (`ImportTask.cleanup`): once the album is
    /// stored, remove the files that were copied into the library and prune
    /// the directories they leave empty, up to the import path.
    fn delete_originals(&self, copied: &[PathBuf]) {
        if !self.opts.delete {
            return;
        }
        for old in copied {
            if let Err(e) = std::fs::remove_file(old) {
                eprintln!("warning: removing {}: {e}", old.display());
                continue;
            }
            if let (Some(parent), Some(root)) = (old.parent(), self.roots.last()) {
                prune_dirs(parent, root, &self.clutter);
            }
        }
    }

    /// Place the item's file with `op`, write its tags and refresh its
    /// mtime. Returns the source path when the file was copied and the
    /// original should go with `import.delete`.
    fn place_and_write(
        &self,
        item: &mut Item,
        album: Option<&Album>,
        tagged: bool,
        op: FileOp,
    ) -> Result<Option<PathBuf>> {
        let src = item.path().to_path_buf();
        let dest = self.fmt.destination(item, album, Some(self.lib));
        let mut copied = None;
        if op != FileOp::None && dest != src {
            let dest = fileops::unique_path(&dest);
            fileops::transfer(op, &src, &dest)?;
            item.set_path(dest);
            if op == FileOp::Move {
                // Files moved within the library prune up to the library root.
                let libdir = self.cfg.directory();
                let root = if src.starts_with(&libdir) {
                    Some(libdir.as_path())
                } else {
                    self.roots.last().map(PathBuf::as_path)
                };
                if let (Some(parent), Some(root)) = (src.parent(), root) {
                    prune_dirs(parent, root, &self.clutter);
                }
            } else if op == FileOp::Copy {
                copied = Some(src);
            }
        }
        if self.opts.write
            && tagged
            && let Err(e) = media::write(item)
        {
            eprintln!("warning: {e:#}");
        }
        if let Ok(meta) = std::fs::metadata(item.path())
            && let Ok(m) = meta.modified()
        {
            let ts = m
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            item.set("mtime", Value::Float(ts));
        }
        Ok(copied)
    }

    // --------------------------------------------------------- singletons

    fn import_singleton(&mut self, file: &Path) -> Result<Outcome> {
        let mut items = self.read_items(&[file.to_path_buf()])?;
        match items.pop() {
            Some(item) => self.import_singleton_item(item),
            None => Ok(Outcome::Done),
        }
    }

    fn import_singleton_item(&mut self, mut item: Item) -> Result<Outcome> {
        println!();
        println!("{}", item.path().display());
        let paths = [item.path().to_path_buf()];
        let decision = if self.opts.autotag {
            let d = self.choose_item(&item)?;
            self.log_choice(&d, &paths);
            d
        } else {
            Decision::AsIs
        };
        let original = item.clone();
        let tagged = match decision {
            Decision::Abort => return Ok(Outcome::Aborted),
            Decision::Skip => {
                self.stats.skipped += 1;
                return Ok(Outcome::Skipped);
            }
            Decision::AsIs => false,
            Decision::Apply(m) => {
                if self.opts.from_scratch {
                    clear_tags(&mut item);
                }
                autotag::apply_track(&mut item, &m.info);
                true
            }
            Decision::Tracks | Decision::Group => unreachable!("not offered for tracks"),
        };
        let want = duplicates::key_values(&item, &self.cfg.list("import.duplicate_keys.item"));
        let new = std::slice::from_ref(&item);
        match self.resolve_duplicates(false, new, std::slice::from_ref(&original), &want, &paths)? {
            DupOutcome::Proceed | DupOutcome::Upgrade { .. } => {}
            DupOutcome::Skip => {
                self.stats.skipped += 1;
                return Ok(Outcome::Skipped);
            }
            DupOutcome::Merge(dups) => {
                // As beets: the track and its duplicates become one album task.
                let mut paths = paths.to_vec();
                for d in &dups {
                    self.merged.insert(d.path().to_path_buf());
                    paths.push(d.path().to_path_buf());
                }
                let dir = original
                    .path()
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf();
                let mut items = vec![original];
                items.extend(dups);
                return self.import_album_items(items, &dir, &paths);
            }
        }
        item.set("added", Value::Float(now()));
        item.set_int("album_id", 0);
        let lib = self.lib;
        let copied = lib.transaction(|lib| {
            let replaced = self.record_replaced(std::slice::from_ref(&item))?;
            self.remove_replaced(&replaced)?;
            self.reimport_metadata(None, std::slice::from_mut(&mut item), &replaced);
            self.apply_set_fields(None, std::slice::from_mut(&mut item));
            let op = self.file_op(&item, !replaced.items[0].is_empty());
            let copied = self.place_and_write(&mut item, None, tagged, op)?;
            lib.add(&mut item)?;
            Ok(copied)
        })?;
        let ctx = self.plugin_ctx();
        plugins::each(self.plugins, |p| p.item_imported(&ctx, &mut item));
        self.delete_originals(copied.as_slice());
        self.stats.items += 1;
        Ok(Outcome::Done)
    }

    /// `import.bell`: ring the terminal bell before asking about a match.
    fn bell(&self) {
        if self.cfg.bool("import.bell") {
            print!("\x07");
            std::io::stdout().flush().ok();
        }
    }

    /// A singleton candidate's disambiguation: its album in brackets, with
    /// `import.singleton_album_disambig` (beets' `TrackMatch` disambig data).
    fn track_disambig(&self, info: &crate::mb::TrackInfo) -> String {
        if self.cfg.bool("import.singleton_album_disambig") && !info.album.is_empty() {
            format!(" ([{}])", info.album)
        } else {
            String::new()
        }
    }

    fn choose_item(&mut self, item: &Item) -> Result<Decision<TrackMatch>> {
        println!(
            "Tagging track: {} - {}",
            item.str("artist"),
            item.str("title")
        );
        let ids = self.opts.search_ids.clone();
        let (mut cands, mut rec) = autotag::item_candidates(self.cfg, &self.mb, item, None, &ids)?;
        let (strong, medium) = self.thresholds();
        if let Some(d) = self.summary_judgment(rec) {
            return Ok(d);
        }
        loop {
            if !cands.is_empty() && rec == Recommendation::Strong && !self.opts.timid {
                let m = cands.swap_remove(0);
                println!(
                    "  Match ({}): {} - {}{}",
                    ui::similarity(m.distance.distance(), strong, medium),
                    m.info.artist,
                    m.info.title,
                    ui::dim(&self.track_disambig(&m.info))
                );
                return Ok(Decision::Apply(Box::new(m)));
            }
            if cands.is_empty() {
                println!("{}", ui::yellow("No matching recording found."));
            } else {
                println!("Candidates:");
            }
            for (n, m) in cands.iter().enumerate() {
                let keys = m.distance.keys();
                let penalty = if keys.is_empty() {
                    String::new()
                } else {
                    format!(" {}", ui::yellow(&format!("({})", keys.join(", "))))
                };
                println!(
                    "{}. {} - {} ({}){}{}{}",
                    n + 1,
                    m.info.artist,
                    m.info.title,
                    ui::similarity(m.distance.distance(), strong, medium),
                    penalty,
                    ui::dim(&len_str(m.info.length)),
                    ui::dim(&self.track_disambig(&m.info))
                );
            }
            self.bell();
            let numbers = if cands.is_empty() {
                None
            } else {
                Some(cands.len())
            };
            match ui::options(
                &["Skip", "Use as-is", "Enter search", "enter Id", "aBort"],
                numbers,
                numbers.map(|_| Choice::Number(1)),
            ) {
                None | Some(Choice::Letter('b')) => return Ok(Decision::Abort),
                Some(Choice::Number(n)) => {
                    return Ok(Decision::Apply(Box::new(cands.swap_remove(n - 1))));
                }
                Some(Choice::Letter('s')) => return Ok(Decision::Skip),
                Some(Choice::Letter('u')) => return Ok(Decision::AsIs),
                Some(Choice::Letter('e')) => {
                    let artist = ui::read_line("Artist: ").unwrap_or_default();
                    let title = ui::read_line("Track: ").unwrap_or_default();
                    (cands, rec) = autotag::item_candidates(
                        self.cfg,
                        &self.mb,
                        item,
                        Some((&artist, &title)),
                        &[],
                    )?;
                }
                Some(Choice::Letter('i')) => {
                    let id = ui::read_line("Enter recording ID: ").unwrap_or_default();
                    (cands, rec) = autotag::item_candidates(self.cfg, &self.mb, item, None, &[id])?;
                    rec = rec.min(Recommendation::Medium);
                }
                _ => {}
            }
        }
    }
}

fn disambig(info: &crate::mb::AlbumInfo) -> String {
    let mut parts = vec![];
    if !info.media.is_empty() {
        parts.push(if info.mediums > 1 {
            format!("{}x{}", info.mediums, info.media)
        } else {
            info.media.clone()
        });
    }
    if info.year > 0 {
        parts.push(info.year.to_string());
    }
    for s in [
        &info.country,
        &info.label,
        &info.catalognum,
        &info.albumdisambig,
    ] {
        if !s.is_empty() {
            parts.push(s.clone());
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_directory_and_merges_discs() {
        let d = tempfile::tempdir().unwrap();
        let mk = |p: &str| {
            let p = d.path().join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        };
        mk("A/01.mp3");
        mk("A/02.mp3");
        mk("A/cover.jpg");
        mk("B/CD1/01.flac");
        mk("B/CD2/01.flac");
        mk("C/Live/01.mp3");
        mk("C/Studio/01.mp3");
        let groups = group_files(d.path(), &WalkFilter::default()).unwrap();
        let sizes: Vec<usize> = groups.iter().map(|(_, g)| g.len()).collect();
        assert_eq!(sizes, vec![2, 2, 1, 1]);
        let keys: Vec<PathBuf> = groups.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys[1], d.path().join("B"));
    }

    #[test]
    fn walk_skips_ignored_and_hidden_entries() {
        let d = tempfile::tempdir().unwrap();
        let mk = |p: &str| {
            let p = d.path().join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        };
        mk("A/01.mp3");
        mk("A/._01.mp3");
        mk(".Trash/01.mp3");
        mk("lost+found/01.mp3");
        mk("Old~/01.mp3");
        mk("System Volume Information/01.mp3");
        let files = |f: &WalkFilter| -> Vec<PathBuf> {
            group_files(d.path(), f)
                .unwrap()
                .into_iter()
                .flat_map(|(_, g)| g)
                .map(|p| p.strip_prefix(d.path()).unwrap().to_path_buf())
                .collect()
        };
        let mut cfg = Config::defaults();
        assert_eq!(
            files(&WalkFilter::from_config(&cfg)),
            [PathBuf::from("A/01.mp3")]
        );
        assert_eq!(files(&WalkFilter::default()).len(), 6);
        // ignore_hidden alone still skips dot-files.
        cfg.set("ignore", serde_yaml::from_str("[]").unwrap());
        assert_eq!(files(&WalkFilter::from_config(&cfg)).len(), 4);
        // A hidden file given directly is imported.
        let direct = d.path().join("A/._01.mp3");
        let g = group_files(&direct, &WalkFilter::from_config(&cfg)).unwrap();
        assert_eq!(g, [(direct.clone(), vec![direct])]);
        // The import root itself is never filtered.
        let g = group_files(&d.path().join(".Trash"), &WalkFilter::from_config(&cfg)).unwrap();
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn unique_and_prune() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.mp3");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(fileops::unique_path(&f), d.path().join("x.1.mp3"));
        let deep = d.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        prune_dirs(&deep, d.path(), &Globs::default());
        assert!(!d.path().join("a").exists());
        assert!(d.path().exists());
    }

    /// Config + library in a temp dir, with an empty album dir per name.
    /// The files aren't real audio, so each album reads as empty and
    /// finishes as `Done` without needing MusicBrainz.
    fn setup(albums: &[&str]) -> (tempfile::TempDir, Config, Library, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.config_dir = d.path().join("cfg");
        // Never let a test touch the real music directory.
        cfg.set("directory", d.path().join("lib").to_string_lossy().into());
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        for a in albums {
            std::fs::create_dir_all(src.join(a)).unwrap();
            std::fs::write(src.join(a).join("01.mp3"), b"").unwrap();
        }
        let src = std::fs::canonicalize(src).unwrap();
        (d, cfg, Library::in_memory().unwrap(), src)
    }

    fn opts(cfg: &Config) -> ImportOptions {
        let mut o = ImportOptions::from_config(cfg);
        o.autotag = false;
        o.op = FileOp::None;
        o
    }

    #[test]
    fn incremental_records_and_skips() {
        let (_d, cfg, lib, src) = setup(&["A", "B"]);
        let mut o = opts(&cfg);
        o.incremental = true;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        let st = ImportState::load(&cfg.state_path());
        assert!(st.history.contains(&src.join("A")) && st.history.contains(&src.join("B")));
        assert!(st.progress.is_empty());

        std::fs::create_dir_all(src.join("C")).unwrap();
        std::fs::write(src.join("C/01.mp3"), b"").unwrap();
        let mut o = opts(&cfg);
        o.incremental = true;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!(imp.stats.already, 2);
        assert!(
            ImportState::load(&cfg.state_path())
                .history
                .contains(&src.join("C"))
        );
    }

    #[test]
    fn resumes_interrupted_import() {
        let (_d, cfg, lib, src) = setup(&["A", "B", "C"]);
        // Simulate an import of `src` that was interrupted after album A.
        let mut st = ImportState::default();
        st.progress
            .entry(src.clone())
            .or_default()
            .insert(src.join("A"));
        st.save(&cfg.state_path()).unwrap();

        let mut o = opts(&cfg);
        o.resume = Resume::Yes;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!(imp.stats.already, 1);
        // Finishing the top path clears its progress; no history without -i.
        let st = ImportState::load(&cfg.state_path());
        assert!(st.progress.is_empty() && st.history.is_empty());
    }

    #[test]
    fn noresume_starts_over_and_pretend_records_nothing() {
        let (_d, cfg, lib, src) = setup(&["A", "B"]);
        let mut st = ImportState::default();
        st.progress
            .entry(src.clone())
            .or_default()
            .insert(src.join("A"));
        st.save(&cfg.state_path()).unwrap();

        let mut o = opts(&cfg);
        o.pretend = true;
        o.incremental = true;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        let st = ImportState::load(&cfg.state_path());
        assert!(st.history.is_empty(), "pretend must not record history");
        assert_eq!(st.progress.len(), 1, "pretend must not touch progress");

        let mut o = opts(&cfg);
        o.resume = Resume::No;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!(imp.stats.already, 0);
        assert!(ImportState::load(&cfg.state_path()).progress.is_empty());
    }

    #[test]
    fn incremental_skip_later_leaves_skipped_albums_out_of_history() {
        let (_d, cfg, lib, src) = setup(&[]);
        for skip_later in [false, true] {
            let mut o = opts(&cfg);
            o.incremental = true;
            o.incremental_skip_later = skip_later;
            let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
            let key = src.join(format!("skipped-{skip_later}"));
            imp.record(&src, key.clone(), Outcome::Skipped, false)
                .unwrap();
            imp.record(&src, src.join("done"), Outcome::Done, false)
                .unwrap();
            let st = ImportState::load(&cfg.state_path());
            assert_eq!(st.history.contains(&key), !skip_later);
            assert!(st.history.contains(&src.join("done")));
            assert!(st.progress.is_empty(), "incremental records no progress");
        }
    }

    #[test]
    fn progress_is_saved_per_album_and_cleared_when_path_completes() {
        let (_d, cfg, lib, src) = setup(&["A", "B"]);
        let mut imp = Importer::new(&cfg, &lib, opts(&cfg), &[]).unwrap();
        // Mid-import, each finished album is persisted immediately.
        imp.record(&src, src.join("A"), Outcome::Done, true)
            .unwrap();
        let st = ImportState::load(&cfg.state_path());
        assert!(st.progress[&src].contains(&src.join("A")));
        assert!(st.history.is_empty(), "no history without incremental");
        // A full run over the path resumes past A and then clears progress.
        let mut o = opts(&cfg);
        o.resume = Resume::Yes;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!(imp.stats.already, 1);
        assert!(ImportState::load(&cfg.state_path()).progress.is_empty());
    }

    #[test]
    fn quiet_ask_neither_resumes_nor_discards_progress() {
        let (_d, cfg, lib, src) = setup(&["A", "B"]);
        let mut st = ImportState::default();
        st.progress
            .entry(src.clone())
            .or_default()
            .insert(src.join("A"));
        st.save(&cfg.state_path()).unwrap();

        let mut o = opts(&cfg);
        o.quiet = true;
        o.resume = Resume::Ask;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!(imp.stats.already, 0, "quiet mode must not resume");
        let st = ImportState::load(&cfg.state_path());
        assert_eq!(st.progress[&src].len(), 1, "earlier progress left as is");
    }

    fn tagged(artist: &str, albumartist: &str, album: &str, track: i64) -> Item {
        let mut i = Item::new_item();
        i.set_str("artist", artist);
        i.set_str("albumartist", albumartist);
        i.set_str("album", album);
        i.set_int("track", track);
        i
    }

    fn summary(groups: &[Vec<Item>]) -> Vec<Vec<(String, i64)>> {
        groups
            .iter()
            .map(|g| {
                g.iter()
                    .map(|i| (i.str("album").to_string(), i.int("track")))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn group_albums_splits_mixed_directory() {
        let items = vec![
            tagged("B", "", "Second", 1),
            tagged("A", "", "First", 1),
            tagged("B", "", "Second", 2),
            tagged("A", "", "First", 2),
            // Same album name, different artist: a separate album.
            tagged("C", "", "First", 1),
        ];
        assert_eq!(
            summary(&group_albums(items)),
            vec![
                vec![("First".into(), 1), ("First".into(), 2)],
                vec![("Second".into(), 1), ("Second".into(), 2)],
                vec![("First".into(), 1)],
            ]
        );
    }

    #[test]
    fn group_albums_prefers_albumartist_and_keeps_untagged_together() {
        let items = vec![
            // A compilation: one albumartist over different track artists.
            tagged("X", "Various Artists", "Comp", 1),
            tagged("Y", "Various Artists", "Comp", 2),
            // No album tag: grouped by artist with an empty album, as beets.
            tagged("Z", "", "", 1),
            tagged("Z", "", "", 2),
            tagged("W", "", "", 1),
        ];
        let groups = group_albums(items);
        let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 1, 2]);
        assert_eq!(groups[0][0].str("albumartist"), "Various Artists");
        assert_eq!(groups[1][0].str("artist"), "W");
        assert_eq!(groups[2][0].str("artist"), "Z");
    }

    /// Real tagged files (needs ffmpeg; skipped without it).
    fn make_track(dir: &Path, name: &str, artist: &str, album: &str, track: u32) -> bool {
        std::fs::create_dir_all(dir).unwrap();
        std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"])
            .args(["-metadata", &format!("artist={artist}")])
            .args(["-metadata", &format!("album={album}")])
            .args(["-metadata", &format!("track={track}")])
            .arg(dir.join(name))
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn group_albums_option_imports_each_album_and_records_the_directory() {
        let (_d, cfg, lib, src) = setup(&[]);
        let mixed = src.join("mixed");
        for (n, artist, album, t) in [
            ("1.mp3", "A", "First", 1),
            ("2.mp3", "B", "Second", 1),
            ("3.mp3", "A", "First", 2),
        ] {
            if !make_track(&mixed, n, artist, album, t) {
                return;
            }
        }
        let mut o = opts(&cfg);
        o.group_albums = true;
        o.incremental = true;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!((imp.stats.albums, imp.stats.items), (2, 3));
        let mut albums: Vec<String> = lib
            .all_albums()
            .unwrap()
            .iter()
            .map(|a| format!("{} - {}", a.str("albumartist"), a.str("album")))
            .collect();
        albums.sort();
        assert_eq!(albums, ["A - First", "B - Second"]);
        // The state key is still the directory, not the tag groups.
        let st = ImportState::load(&cfg.state_path());
        assert_eq!(st.history.iter().collect::<Vec<_>>(), [&mixed]);
    }

    #[test]
    fn set_field_arguments_need_key_and_value() {
        assert_eq!(
            parse_set_field("genre=Rock=Pop").unwrap(),
            ("genre", "Rock=Pop")
        );
        for bad in ["genre", "=Rock", "genre="] {
            let e = parse_set_field(bad).unwrap_err().to_string();
            assert!(e.contains("is not of the form `key=value'"), "{e}");
        }
    }

    #[test]
    fn summary_judgment_follows_quiet_timid_and_none_rec_action() {
        let (_d, cfg, lib, _src) = setup(&[]);
        let judge = |f: &dyn Fn(&mut ImportOptions), rec| {
            let mut o = opts(&cfg);
            f(&mut o);
            let imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
            match imp.summary_judgment::<AlbumMatch>(rec) {
                None => "ask",
                Some(Decision::AsIs) => "asis",
                Some(Decision::Skip) => "skip",
                Some(_) => "other",
            }
        };
        use Recommendation as R;
        assert_eq!(judge(&|_| {}, R::None), "ask");
        assert_eq!(
            judge(&|o| o.none_rec_action = NoneRecAction::Skip, R::None),
            "skip"
        );
        assert_eq!(
            judge(&|o| o.none_rec_action = NoneRecAction::AsIs, R::None),
            "asis"
        );
        // Only "no recommendation" triggers it, and timid always asks.
        assert_eq!(
            judge(&|o| o.none_rec_action = NoneRecAction::Skip, R::Low),
            "ask"
        );
        assert_eq!(
            judge(
                &|o| {
                    o.none_rec_action = NoneRecAction::Skip;
                    o.timid = true;
                },
                R::None
            ),
            "ask"
        );
        // Quiet: strong applies (None here), anything else uses the fallback.
        assert_eq!(judge(&|o| o.quiet = true, R::Strong), "ask");
        assert_eq!(judge(&|o| o.quiet = true, R::Medium), "skip");
        assert_eq!(
            judge(
                &|o| {
                    o.quiet = true;
                    o.quiet_fallback_asis = true;
                },
                R::Medium
            ),
            "asis"
        );
    }

    /// Two real tagged tracks in `src/<dir>` (None without ffmpeg).
    fn album_dir(src: &Path, dir: &str, album: &str) -> Option<PathBuf> {
        let d = src.join(dir);
        for t in 1..=2 {
            if !make_track(&d, &format!("{t}.mp3"), "Artist", album, t) {
                return None;
            }
        }
        Some(d)
    }

    #[test]
    fn set_fields_are_templates_applied_to_album_and_items() {
        let (_d, cfg, lib, src) = setup(&[]);
        let Some(_) = album_dir(&src, "a", "Some Album") else {
            return;
        };
        let mut o = opts(&cfg);
        o.set_fields = vec![
            ("genre".into(), "Genre of $album".into()),
            ("collection".into(), "Unordered".into()),
            ("disctotal".into(), "3".into()),
        ];
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        let albums = lib.all_albums().unwrap();
        assert_eq!(albums.len(), 1);
        let a = &albums[0];
        assert_eq!(a.str("genre"), "Genre of Some Album");
        assert_eq!(a.str("collection"), "Unordered");
        let items = lib.album_items(a.id.unwrap()).unwrap();
        assert_eq!(items.len(), 2);
        for i in &items {
            assert_eq!(i.str("genre"), "Genre of Some Album");
            assert_eq!(i.str("collection"), "Unordered");
            // Typed fields are parsed, not stored as text.
            assert_eq!(i.int("disctotal"), 3);
        }
    }

    #[test]
    fn log_records_start_choices_and_duplicates() {
        let (d, mut cfg, lib, src) = setup(&[]);
        let Some(album) = album_dir(&src, "a", "Dup Album") else {
            return;
        };
        cfg.set("directory", d.path().join("lib").to_string_lossy().into());
        cfg.set("import.duplicate_action", "skip".into());
        let log = d.path().join("import.log");
        let run = || {
            let mut o = opts(&cfg);
            o.op = FileOp::Copy;
            o.log = Some(log.clone());
            let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
            imp.run(std::slice::from_ref(&src)).unwrap();
            imp
        };
        run();
        // The second import finds the copy already in the library.
        let mut imp = run();
        assert_eq!(imp.stats.skipped, 1);
        // Prompt decisions are logged the same way (these need no lookup).
        imp.log_choice(&Decision::<AlbumMatch>::Skip, std::slice::from_ref(&album));
        imp.log_choice(
            &Decision::<AlbumMatch>::AsIs,
            &[album.join("1.mp3"), album.join("2.mp3")],
        );
        imp.log_choice(&Decision::<AlbumMatch>::Abort, std::slice::from_ref(&album));
        drop(imp);

        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(lines[0].starts_with("import started "));
        assert!(lines[1].starts_with("import started "));
        let a = album.display();
        assert_eq!(lines[2], format!("duplicate-skip {a}"));
        assert_eq!(lines[3], format!("skip {a}"));
        assert_eq!(lines[4], format!("asis {a}/1.mp3; {a}/2.mp3"));
    }

    #[test]
    fn unwritable_log_is_an_error() {
        let (d, cfg, lib, _src) = setup(&[]);
        let mut o = opts(&cfg);
        o.log = Some(d.path().join("missing-dir/import.log"));
        let e = Importer::new(&cfg, &lib, o, &[]).err().unwrap().to_string();
        assert!(
            e.starts_with("Could not open log file for writing: "),
            "{e}"
        );
    }

    #[test]
    fn flat_imports_the_whole_tree_as_one_album() {
        let (_d, cfg, lib, src) = setup(&[]);
        let top = src.join("box");
        let (Some(_), Some(_)) = (
            album_dir(&top, "Disc A", "Box Set"),
            album_dir(&top, "extras/Bonus", "Box Set"),
        ) else {
            return;
        };
        let mut o = opts(&cfg);
        o.flat = true;
        o.incremental = true;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&top)).unwrap();
        assert_eq!((imp.stats.albums, imp.stats.items), (1, 4));
        // The import path itself is the album's key.
        let st = ImportState::load(&cfg.state_path());
        assert_eq!(st.history.iter().collect::<Vec<_>>(), [&top]);
    }

    #[test]
    fn pretend_imports_and_records_nothing() {
        let (d, cfg, lib, src) = setup(&[]);
        let Some(_) = album_dir(&src, "a", "Pretend Album") else {
            return;
        };
        let log = d.path().join("import.log");
        let mut o = opts(&cfg);
        o.pretend = true;
        o.incremental = true;
        o.autotag = true; // never reached: pretend doesn't look anything up
        o.set_fields = vec![("genre".into(), "x".into())];
        o.log = Some(log.clone());
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&src)).unwrap();
        assert_eq!(
            (imp.stats.albums, imp.stats.items, imp.stats.skipped),
            (0, 0, 0)
        );
        assert!(lib.all_albums().unwrap().is_empty());
        assert!(!cfg.state_path().exists());
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 1, "only the start line: {text}");
    }

    fn import_dir(
        cfg: &Config,
        lib: &Library,
        dir: &Path,
        f: impl Fn(&mut ImportOptions),
    ) -> Stats {
        let mut o = opts(cfg);
        f(&mut o);
        let mut imp = Importer::new(cfg, lib, o, &[]).unwrap();
        imp.run(&[dir.to_path_buf()]).unwrap();
        imp.stats
    }

    fn all_items(lib: &Library) -> Vec<Item> {
        lib.items(&crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[]).unwrap())
            .unwrap()
    }

    #[test]
    fn reimport_by_path_replaces_rows_and_keeps_added_flex_and_art() {
        let (_d, cfg, lib, src) = setup(&[]);
        let Some(dir) = album_dir(&src, "a", "Album") else {
            return;
        };
        import_dir(&cfg, &lib, &dir, |_| {});
        // The user's own additions since the first import.
        let mut album = lib.all_albums().unwrap().pop().unwrap();
        album.set("added", Value::Float(1000.0));
        album.flex.insert("mood".into(), "sunny".into());
        album.set("artpath", Value::Path("/art/cover.jpg".into()));
        lib.store(&album).unwrap();
        for mut i in all_items(&lib) {
            i.set("added", Value::Float(2000.0));
            i.flex.insert("rating".into(), "5".into());
            lib.store(&i).unwrap();
        }

        // Reimport: the album isn't a duplicate of itself (no prompt, so no
        // skip), and the rows are replaced rather than added.
        let mut o = opts(&cfg);
        o.set_fields = vec![("mood".into(), "$mood and bright".into())];
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(
            (imp.stats.skipped, imp.stats.albums, imp.stats.items),
            (0, 1, 2)
        );
        let albums = lib.all_albums().unwrap();
        assert_eq!(albums.len(), 1);
        let items = all_items(&lib);
        assert_eq!(items.len(), 2);
        let a = &albums[0];
        assert_eq!(a.float("added"), 1000.0);
        assert_eq!(a.path_field("artpath"), Path::new("/art/cover.jpg"));
        // set_fields runs after preservation, so it sees (and wins over) it.
        assert_eq!(
            a.flex.get("mood").map(String::as_str),
            Some("sunny and bright")
        );
        for i in &items {
            assert_eq!(i.int("album_id"), a.id.unwrap());
            assert_eq!(i.float("added"), 2000.0);
            assert_eq!(i.flex.get("rating").map(String::as_str), Some("5"));
        }
    }

    #[test]
    fn preserve_flex_lets_fresh_fields_from_the_new_match_win() {
        // `data_source` is a fixed field here; the plugin IDs are flexible.
        let mut old = Item::new_item();
        old.flex.insert("spotify_album_id".into(), "old".into());
        old.flex.insert("mood".into(), "sunny".into());
        let mut new = Item::new_item();
        new.flex.insert("spotify_album_id".into(), "new".into());
        preserve_flex(&mut new, &old, REIMPORT_FRESH_FIELDS_ITEM);
        assert_eq!(new.flex["spotify_album_id"], "new");
        assert_eq!(new.flex["mood"], "sunny");
        // A fresh field the new record leaves empty keeps the old value.
        let mut empty = Item::new_item();
        preserve_flex(&mut empty, &old, REIMPORT_FRESH_FIELDS_ITEM);
        assert_eq!(empty.flex["spotify_album_id"], "old");
    }

    #[test]
    fn copy_mode_reimport_moves_library_files_instead_of_copying() {
        let (d, cfg, lib, src) = setup(&[]);
        let Some(dir) = album_dir(&src, "a", "Album") else {
            return;
        };
        let copy = |o: &mut ImportOptions| o.op = FileOp::Copy;
        import_dir(&cfg, &lib, &dir, copy);
        let libdir = d.path().join("lib");
        let count = || walkdir_files(&libdir);
        assert_eq!(count(), 2);
        // Retag in place with a new path format: files move, never double.
        let mut cfg2 = cfg.clone();
        cfg2.set(
            "paths",
            serde_yaml::from_str("default: moved/$album/$track").unwrap(),
        );
        let s = import_dir(&cfg2, &lib, &libdir, copy);
        assert_eq!((s.skipped, s.albums, s.items), (0, 1, 2));
        assert_eq!(count(), 2);
        assert_eq!(all_items(&lib).len(), 2);
        for i in all_items(&lib) {
            assert!(
                i.path().starts_with(libdir.join("moved/Album")),
                "{}",
                i.path().display()
            );
            assert!(i.path().is_file());
        }
        // The old album folder was pruned.
        assert!(!libdir.join("Artist").exists());
    }

    fn is_symlink(p: &Path) -> bool {
        p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    }

    #[test]
    fn link_hardlink_and_reflink_modes_leave_the_sources() {
        for op in [FileOp::Link, FileOp::Hardlink, FileOp::ReflinkAuto] {
            let (_d, cfg, lib, src) = setup(&[]);
            let Some(dir) = album_dir(&src, "a", "Album") else {
                return;
            };
            // `delete` only applies to copies.
            let s = import_dir(&cfg, &lib, &dir, |o| {
                o.op = op;
                o.delete = true;
            });
            assert_eq!((s.albums, s.items), (1, 2), "{op:?}");
            assert!(dir.join("1.mp3").is_file() && dir.join("2.mp3").is_file());
            for i in all_items(&lib) {
                let p = i.path();
                assert!(p.starts_with(cfg.directory()), "{}", p.display());
                assert_eq!(is_symlink(p), op == FileOp::Link, "{op:?}");
                if op == FileOp::Link {
                    assert!(std::fs::read_link(p).unwrap().starts_with(&dir));
                }
                #[cfg(unix)]
                if op == FileOp::Hardlink {
                    use std::os::unix::fs::MetadataExt;
                    let name = p.file_name().unwrap().to_string_lossy();
                    let orig = dir.join(if name.starts_with("01") {
                        "1.mp3"
                    } else {
                        "2.mp3"
                    });
                    assert_eq!(p.metadata().unwrap().ino(), orig.metadata().unwrap().ino());
                }
            }
        }
    }

    #[test]
    fn link_mode_reimport_moves_the_library_files() {
        let (d, cfg, lib, src) = setup(&[]);
        let Some(dir) = album_dir(&src, "a", "Album") else {
            return;
        };
        let link = |o: &mut ImportOptions| o.op = FileOp::Link;
        import_dir(&cfg, &lib, &dir, link);
        let mut cfg2 = cfg.clone();
        cfg2.set(
            "paths",
            serde_yaml::from_str("default: moved/$album/$track").unwrap(),
        );
        let libdir = d.path().join("lib");
        import_dir(&cfg2, &lib, &libdir, link);
        // The symlinks moved (not linked to themselves) and still resolve.
        assert_eq!(walkdir_files(&libdir), 2);
        for i in all_items(&lib) {
            assert!(i.path().starts_with(libdir.join("moved/Album")));
            assert!(is_symlink(i.path()) && i.path().is_file());
        }
        assert!(!libdir.join("Artist").exists());
    }

    #[test]
    fn copy_with_delete_removes_originals_and_prunes_clutter() {
        let (_d, cfg, lib, src) = setup(&[]);
        let Some(dir) = album_dir(&src.join("outer"), "disc", "Album") else {
            return;
        };
        std::fs::write(dir.join("Thumbs.DB"), b"").unwrap();
        std::fs::write(src.join("keep.txt"), b"").unwrap();
        let s = import_dir(&cfg, &lib, &src, |o| {
            o.op = FileOp::Copy;
            o.delete = true;
        });
        assert_eq!((s.albums, s.items), (1, 2));
        for i in all_items(&lib) {
            assert!(i.path().starts_with(cfg.directory()) && i.path().is_file());
        }
        // The emptied directories (clutter aside) went, up to the import
        // path, which stays.
        assert!(!src.join("outer").exists());
        assert!(src.join("keep.txt").exists());
    }

    #[test]
    fn move_prunes_source_directories_holding_only_clutter() {
        let (_d, cfg, lib, src) = setup(&[]);
        let Some(dir) = album_dir(&src, "a", "Album") else {
            return;
        };
        std::fs::write(dir.join(".DS_Store"), b"").unwrap();
        import_dir(&cfg, &lib, &src, |o| o.op = FileOp::Move);
        assert!(!dir.exists());
        assert!(src.exists());
        // Without the clutter rule the directory stays.
        let (_d, mut cfg, lib, src) = setup(&[]);
        cfg.set("clutter", serde_yaml::from_str("[]").unwrap());
        let Some(dir) = album_dir(&src, "a", "Album") else {
            return;
        };
        std::fs::write(dir.join(".DS_Store"), b"").unwrap();
        import_dir(&cfg, &lib, &src, |o| o.op = FileOp::Move);
        assert!(dir.join(".DS_Store").exists());
    }

    fn walkdir_files(dir: &Path) -> usize {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                n += if p.is_dir() { walkdir_files(&p) } else { 1 };
            }
        }
        n
    }

    #[test]
    fn library_query_reimports_only_matching_albums() {
        let (_d, cfg, lib, src) = setup(&[]);
        let (Some(a), Some(b)) = (
            album_dir(&src, "a", "First"),
            album_dir(&src, "b", "Second"),
        ) else {
            return;
        };
        import_dir(&cfg, &lib, &a, |_| {});
        import_dir(&cfg, &lib, &b, |_| {});
        let mut o = opts(&cfg);
        o.set_fields = vec![("comments".into(), "retagged".into())];
        // What `beet import -L` sets up.
        o.resume = Resume::No;
        o.incremental = false;
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap();
        imp.run_library(&["album:First".to_string()]).unwrap();
        assert_eq!(
            (imp.stats.albums, imp.stats.items, imp.stats.skipped),
            (1, 2, 0)
        );
        assert_eq!(lib.all_albums().unwrap().len(), 2);
        let items = all_items(&lib);
        assert_eq!(items.len(), 4);
        for i in items {
            let want = if i.str("album") == "First" {
                "retagged"
            } else {
                ""
            };
            assert_eq!(i.str("comments"), want, "{}", i.str("album"));
        }
        // Nothing recorded for a query import.
        assert_eq!(ImportState::load(&cfg.state_path()).history.len(), 0);
        assert!(ImportState::load(&cfg.state_path()).progress.is_empty());
    }

    #[test]
    fn from_scratch_clears_tags_not_set_by_the_match() {
        let mut item = Item::new_item();
        item.set_str("genre", "Polka");
        item.set_str("comments", "old rip");
        item.set_int("year", 1999);
        item.flex.insert("rating".into(), "5".into());
        item.set_path(PathBuf::from("/x/1.mp3"));
        clear_tags(&mut item);
        let t = crate::mb::TrackInfo {
            title: "New Title".into(),
            artist: "New Artist".into(),
            ..Default::default()
        };
        autotag::apply_track(&mut item, &t);
        assert_eq!(item.str("title"), "New Title");
        assert_eq!(item.str("genre"), "");
        assert_eq!(item.str("comments"), "");
        assert_eq!(item.int("year"), 0);
        // Only tag fields go: flexible attributes and the path stay.
        assert_eq!(item.flex["rating"], "5");
        assert_eq!(item.path(), Path::new("/x/1.mp3"));
    }

    #[test]
    fn merge_combines_new_items_with_the_duplicate_album() {
        let (_d, mut cfg, lib, src) = setup(&[]);
        let (Some(a), Some(b)) = (album_dir(&src, "a", "Same"), album_dir(&src, "b", "Same"))
        else {
            return;
        };
        import_dir(&cfg, &lib, &a, |_| {});
        for mut i in all_items(&lib) {
            i.set("added", Value::Float(3000.0));
            lib.store(&i).unwrap();
        }
        cfg.set("import.duplicate_action", "merge".into());
        let log = src.join("import.log");
        let s = import_dir(&cfg, &lib, &b, |o| o.log = Some(log.clone()));
        assert_eq!((s.skipped, s.albums, s.items), (0, 1, 4));
        let albums = lib.all_albums().unwrap();
        assert_eq!(
            albums.len(),
            1,
            "the old album was replaced by the merged one"
        );
        let items = all_items(&lib);
        assert_eq!(items.len(), 4);
        for i in &items {
            assert_eq!(i.int("album_id"), albums[0].id.unwrap());
            // The old album's items keep their `added`.
            assert_eq!(
                i.float("added") == 3000.0,
                i.path().starts_with(&a),
                "{}",
                i.path().display()
            );
        }
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains(&format!("duplicate-keep {}", b.display())),
            "{text}"
        );
    }

    /// An MP3 with the given tags and bitrate (kbps); false without ffmpeg.
    fn make_mp3(dir: &Path, name: &str, tags: &[(&str, &str)], kbps: u32) -> bool {
        std::fs::create_dir_all(dir).unwrap();
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"]);
        for (k, v) in tags {
            cmd.args(["-metadata", &format!("{k}={v}")]);
        }
        cmd.args(["-b:a", &format!("{kbps}k")])
            .arg(dir.join(name))
            .status()
            .is_ok_and(|s| s.success())
    }

    /// An album directory of (title, kbps) tracks by "Artist".
    fn album_with(src: &Path, dir: &str, album: &str, tracks: &[(&str, u32)]) -> Option<PathBuf> {
        let d = src.join(dir);
        for (n, (title, kbps)) in tracks.iter().enumerate() {
            let track = (n + 1).to_string();
            let tags = [
                ("artist", "Artist"),
                ("album", album),
                ("title", *title),
                ("track", track.as_str()),
            ];
            if !make_mp3(&d, &format!("{track}.mp3"), &tags, *kbps) {
                return None;
            }
        }
        Some(d)
    }

    fn copy_op(o: &mut ImportOptions) {
        o.op = FileOp::Copy;
    }

    #[test]
    fn duplicate_keys_decide_what_counts_as_a_duplicate() {
        let (_d, mut cfg, lib, src) = setup(&[]);
        let (Some(a), Some(b), Some(c)) = (
            album_with(&src, "a", "Same", &[("One", 128)]),
            album_with(&src, "b", "same", &[("One", 128)]),
            album_with(&src, "c", "Other", &[("One", 128)]),
        ) else {
            return;
        };
        cfg.set("import.duplicate_action", "skip".into());
        import_dir(&cfg, &lib, &a, copy_op);
        // Keys match exactly, so a different case isn't a duplicate...
        assert_eq!(import_dir(&cfg, &lib, &b, copy_op).skipped, 0);
        // ...and a different album isn't either, unless only the artist counts.
        assert_eq!(import_dir(&cfg, &lib, &c, copy_op).skipped, 0);
        cfg.set("import.duplicate_keys.album", "albumartist".into());
        let c2 = album_with(&src, "c2", "Third", &[("One", 128)]).unwrap();
        assert_eq!(import_dir(&cfg, &lib, &c2, copy_op).skipped, 1);
        assert_eq!(lib.all_albums().unwrap().len(), 3);
    }

    #[test]
    fn singletons_are_checked_against_singletons_with_item_keys() {
        let (_d, mut cfg, lib, src) = setup(&[]);
        let song = |dir: &str, name: &str, title: &str| {
            let d = src.join(dir);
            make_mp3(&d, name, &[("artist", "Artist"), ("title", title)], 128).then_some(d)
        };
        let (Some(a), Some(b), Some(c)) = (
            song("a", "x.mp3", "Song"),
            song("b", "y.mp3", "Song"),
            song("c", "z.mp3", "Else"),
        ) else {
            return;
        };
        let single = |o: &mut ImportOptions| {
            o.op = FileOp::Copy;
            o.singletons = true;
        };
        // An album track with the same artist and title isn't a duplicate.
        let album = album_with(&src, "album", "Alb", &[("Song", 128)]).unwrap();
        cfg.set("import.duplicate_action", "skip".into());
        import_dir(&cfg, &lib, &album, copy_op);
        assert_eq!(import_dir(&cfg, &lib, &a, single).skipped, 0);
        assert_eq!(import_dir(&cfg, &lib, &b, single).skipped, 1);
        assert_eq!(import_dir(&cfg, &lib, &c, single).skipped, 0);
        cfg.set("import.duplicate_action", "keep".into());
        assert_eq!(import_dir(&cfg, &lib, &b, single).items, 1);
        let songs = |lib: &Library| {
            all_items(lib)
                .into_iter()
                .filter(|i| i.int("album_id") == 0 && i.str("title") == "Song")
                .collect::<Vec<_>>()
        };
        assert_eq!(songs(&lib).len(), 2);
        // Remove old: both earlier copies go, files and all (the new copy
        // then takes the freed path).
        cfg.set("import.duplicate_action", "remove".into());
        let libdir = cfg.directory();
        assert_eq!(walkdir_files(&libdir), 4);
        let d = song("d", "w.mp3", "Song").unwrap();
        import_dir(&cfg, &lib, &d, single);
        let now = songs(&lib);
        assert_eq!(now.len(), 1);
        assert!(now[0].path().starts_with(&libdir) && now[0].path().is_file());
        assert_eq!(all_items(&lib).len(), 3);
        assert_eq!(walkdir_files(&libdir), 3);
    }

    #[test]
    fn upgrade_replaces_only_better_tracks_and_keeps_the_album() {
        let (_d, mut cfg, lib, src) = setup(&[]);
        let Some(a) = album_with(&src, "a", "Up", &[("One", 64), ("Two", 192)]) else {
            return;
        };
        import_dir(&cfg, &lib, &a, copy_op);
        let old_album = lib.all_albums().unwrap()[0].id.unwrap();
        let old_one = all_items(&lib)
            .into_iter()
            .find(|i| i.str("title") == "One")
            .unwrap();
        cfg.set("import.duplicate_action", "upgrade".into());
        let log = src.join("import.log");

        // Only "One" is better: it replaces the old one in the old album.
        let b = album_with(&src, "b", "Up", &[("One", 192), ("Two", 64)]).unwrap();
        let s = import_dir(&cfg, &lib, &b, |o| {
            copy_op(o);
            o.log = Some(log.clone());
        });
        assert_eq!((s.skipped, s.items), (0, 1));
        // The old file was removed; the new one took its place.
        assert_eq!(walkdir_files(&cfg.directory()), 2);
        let new_one = all_items(&lib)
            .into_iter()
            .find(|i| i.str("title") == "One")
            .unwrap();
        assert_ne!(new_one.id, old_one.id);
        assert_eq!(new_one.path(), old_one.path());
        let albums = lib.all_albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, Some(old_album));
        let items = all_items(&lib);
        assert_eq!(items.len(), 2);
        for i in &items {
            assert_eq!(i.int("album_id"), old_album);
            assert!(i.int("bitrate") > 150_000, "{}", i.str("title"));
        }
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains(&format!("duplicate-keep {}", b.display())),
            "{text}"
        );

        // Nothing better: declined, as a skip.
        let c = album_with(&src, "c", "Up", &[("One", 64), ("Two", 64)]).unwrap();
        let s = import_dir(&cfg, &lib, &c, |o| {
            copy_op(o);
            o.log = Some(log.clone());
        });
        assert_eq!(s.skipped, 1);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains(&format!("duplicate-skip {}", c.display())),
            "{text}"
        );

        // Everything better: the old album goes and the new one replaces it.
        let d = album_with(&src, "d", "Up", &[("One", 320), ("Two", 320)]).unwrap();
        import_dir(&cfg, &lib, &d, copy_op);
        let albums = lib.all_albums().unwrap();
        assert_eq!(albums.len(), 1);
        let items = all_items(&lib);
        assert_eq!(items.len(), 2);
        for i in &items {
            assert!(i.int("bitrate") > 300_000);
            assert_eq!(i.int("album_id"), albums[0].id.unwrap());
        }
    }

    #[test]
    fn singleton_upgrade_replaces_one_worse_copy() {
        let (_d, mut cfg, lib, src) = setup(&[]);
        let song = |dir: &str, kbps| {
            let d = src.join(dir);
            make_mp3(
                &d,
                "s.mp3",
                &[("artist", "Artist"), ("title", "Song")],
                kbps,
            )
            .then_some(d)
        };
        let Some(a) = song("a", 64) else {
            return;
        };
        let single = |o: &mut ImportOptions| {
            o.op = FileOp::Copy;
            o.singletons = true;
        };
        import_dir(&cfg, &lib, &a, single);
        cfg.set("import.duplicate_action", "upgrade".into());
        let worse = song("b", 32).unwrap();
        assert_eq!(import_dir(&cfg, &lib, &worse, single).skipped, 1);
        let better = song("c", 256).unwrap();
        assert_eq!(import_dir(&cfg, &lib, &better, single).items, 1);
        let items = all_items(&lib);
        assert_eq!(items.len(), 1);
        assert!(items[0].int("bitrate") > 200_000);
    }

    #[test]
    fn singleton_candidates_show_their_album_unless_disabled() {
        let (_d, mut cfg, lib, _src) = setup(&[]);
        let info = crate::mb::TrackInfo {
            album: "Alb".into(),
            ..Default::default()
        };
        let disambig = |cfg: &Config| {
            Importer::new(cfg, &lib, opts(cfg), &[])
                .unwrap()
                .track_disambig(&info)
        };
        assert_eq!(disambig(&cfg), " ([Alb])");
        cfg.set("import.singleton_album_disambig", false.into());
        assert_eq!(disambig(&cfg), "");
    }

    #[test]
    fn as_is_albums_get_a_plurality_albumartist_or_va() {
        let mk = |albumartist: &str, artist: &str| tagged(artist, albumartist, "X", 1);
        // Mixed old/new items (as after a merge) align to one albumartist.
        let mut items = vec![mk("A", "A"), mk("A", "A"), mk("", "A")];
        align_asis(&mut items, "Various Artists");
        assert!(
            items
                .iter()
                .all(|i| i.str("albumartist") == "A" && !i.bool("comp"))
        );
        // Ties go to the first seen.
        let mut items = vec![mk("", "B"), mk("", "C"), mk("", "C"), mk("", "B")];
        align_asis(&mut items, "VA");
        assert!(items.iter().all(|i| i.str("albumartist") == "B"));
        // Five different artists: below the 25% threshold, so VA.
        let mut items: Vec<Item> = ["P", "Q", "R", "S", "T"]
            .iter()
            .map(|a| mk("", a))
            .collect();
        align_asis(&mut items, "VA");
        assert!(
            items
                .iter()
                .all(|i| i.str("albumartist") == "VA" && i.bool("comp"))
        );
    }
}
