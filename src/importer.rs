//! `beet import`: group files into albums, match them against MusicBrainz,
//! let the user choose, then copy/move, tag and add them to the library.

use crate::autotag::{self, AlbumMatch, Recommendation, TrackMatch};
use crate::config::Config;
use crate::display::Display;
use crate::duplicates::{self, DuplicateAction, Found, KeyValues};
use crate::fileops::{self, FileOp, Globs, WalkFilter, prune_dirs};
use crate::formatted::Formatted;
use crate::library::Library;
use crate::mb::MusicBrainz;
use crate::media;
use crate::model::{Album, Item, Value};
use crate::paths::PathFormatter;
use crate::pipeline::{Cancel, Lookahead};
use crate::plugins::{self, Plugin, PluginCtx};
use crate::state::ImportState;
use crate::template::Template;
use crate::ui::{self, Choice};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

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
    /// `import.default_action`: the confirm prompt's default letter, if any.
    pub default_action: Option<char>,
    /// `import.detail`: list every track in a match, not just changed ones.
    pub detail: bool,
    /// `import.from_scratch`: clear existing tags before applying a match.
    pub from_scratch: bool,
    /// `threaded`: read and look up the next few tasks while the current
    /// one is being decided on and imported.
    pub threaded: bool,
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

/// `import.default_action` (apply, skip, asis or none) as the letter the
/// confirm prompt defaults to; an unknown value is an error, as in beets.
pub fn default_action(cfg: &Config) -> Result<Option<char>> {
    match cfg.str("import.default_action").as_str() {
        "apply" => Ok(Some('a')),
        "skip" => Ok(Some('s')),
        "asis" => Ok(Some('u')),
        "none" => Ok(None),
        v => bail!(
            "import.default_action: must be one of ['apply', 'skip', 'asis', 'none'], not '{v}'"
        ),
    }
}

/// Split a `--set` argument into field and value, as beets' `_store_dict`.
pub fn parse_set_field(kv: &str) -> Result<(&str, &str)> {
    match kv.split_once('=') {
        Some((k, v)) if !k.is_empty() && !v.is_empty() => Ok((k, v)),
        _ => bail!("supplied argument `{kv}' is not of the form `key=value'"),
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
            default_action: default_action(cfg).unwrap_or(Some('a')),
            detail: cfg.bool("import.detail"),
            from_scratch: cfg.bool("import.from_scratch"),
            threaded: cfg.bool("threaded"),
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

/// The track prompt's choices (`_get_choices` for a singleton task).
const ITEM_CHOICES: [&str; 5] = ["Skip", "Use as-is", "Enter search", "enter Id", "aBort"];

/// Confirming one track candidate.
const ITEM_CONFIRM: [&str; 7] = [
    "Apply",
    "More candidates",
    "Skip",
    "Use as-is",
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

/// Where each task's first lookup gets its candidates: MusicBrainz, or a
/// stand-in in tests. Shared with the threaded pipeline's lookup thread.
/// (Searches the user asks for at the prompt always go to MusicBrainz.)
pub trait CandidateSource: Send + Sync {
    fn album(&self, items: &[Item]) -> Result<(Vec<AlbumMatch>, Recommendation)>;
    fn item(&self, item: &Item) -> Result<(Vec<TrackMatch>, Recommendation)>;
}

/// The autotagger's lookup, with `--search-id`s if any.
struct MbSource {
    cfg: Config,
    mb: Arc<MusicBrainz>,
    ids: Vec<String>,
}

impl CandidateSource for MbSource {
    fn album(&self, items: &[Item]) -> Result<(Vec<AlbumMatch>, Recommendation)> {
        autotag::album_candidates(&self.cfg, &self.mb, items, None, &self.ids)
    }

    fn item(&self, item: &Item) -> Result<(Vec<TrackMatch>, Recommendation)> {
        autotag::item_candidates(&self.cfg, &self.mb, item, None, &self.ids)
    }
}

/// A task's directories and audio files (see [`group_files`]).
type Group = (Vec<PathBuf>, Vec<PathBuf>);

/// A unit of lookahead: (import path, group, file) indices; the file index
/// is always 0 unless importing singletons, where each file is a unit.
type Pos = (usize, usize, usize);

/// At most this many units are read or looked up ahead of time, the one
/// being imported included, so an abort wastes few requests.
const LOOKAHEAD: usize = 4;

/// Which lookups the pipeline does for a unit's items, as the sequential
/// flow would on reaching it.
#[derive(Clone, Copy)]
enum LookupMode {
    None,
    Album,
    Groups,
    Tracks,
}

enum Candidates {
    Album(Vec<AlbumMatch>, Recommendation),
    Track(Vec<TrackMatch>, Recommendation),
}

/// A lookup done ahead of time, with what it logged.
struct Looked {
    log: Vec<String>,
    result: Result<Candidates>,
}

/// A unit read (and looked up) by the pipeline.
struct Prepared {
    pos: Pos,
    files: Vec<PathBuf>,
    /// Each file's modification time just before it was read: if one has
    /// changed by the time the unit is imported, it's read again.
    stamps: Vec<Option<SystemTime>>,
    items: Vec<Item>,
    /// What reading logged (unreadable files).
    log: Vec<String>,
    /// Keyed by the paths of the items each lookup was for.
    lookups: Vec<(Vec<PathBuf>, Looked)>,
}

fn stamps(files: &[PathBuf]) -> Vec<Option<SystemTime>> {
    files
        .iter()
        .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .collect()
}

/// Read each file's tags, leaving out those in `skip`; unreadable files
/// are logged (with [`crate::logging::emit`]) and left out.
fn read_files(files: &[PathBuf], skip: &HashSet<PathBuf>) -> Vec<Item> {
    let mut items = vec![];
    for f in files {
        if skip.contains(f) {
            continue;
        }
        match media::read(f) {
            Ok(i) => items.push(i),
            Err(e) => crate::logging::emit(format!("warning: {e:#}")),
        }
    }
    items
}

fn item_paths(items: &[Item]) -> Vec<PathBuf> {
    items.iter().map(|i| i.path().to_path_buf()).collect()
}

/// The pipeline's lookup stage: the lookups the sequential flow would do
/// first for these items, stopping at an error as it would.
fn look_up(
    src: &dyn CandidateSource,
    mode: LookupMode,
    items: &[Item],
    cancel: &Cancel,
) -> Vec<(Vec<PathBuf>, Looked)> {
    let units: Vec<Vec<Item>> = match mode {
        LookupMode::None => vec![],
        _ if items.is_empty() => vec![],
        LookupMode::Album => vec![items.to_vec()],
        LookupMode::Groups => group_albums(items.to_vec()),
        LookupMode::Tracks => items.iter().map(|i| vec![i.clone()]).collect(),
    };
    let mut out = vec![];
    for unit in units {
        if cancel.is_set() {
            break;
        }
        let (result, log) = crate::logging::capture(|| match mode {
            LookupMode::Tracks => src.item(&unit[0]).map(|(c, r)| Candidates::Track(c, r)),
            _ => src.album(&unit).map(|(c, r)| Candidates::Album(c, r)),
        });
        let failed = result.is_err();
        out.push((item_paths(&unit), Looked { log, result }));
        if failed {
            break;
        }
    }
    out
}

/// The threaded pipeline (`threaded: yes`) and where its planning is.
struct Ahead {
    pipe: Lookahead<(Pos, Vec<PathBuf>), Prepared>,
    /// The units in flight, oldest first.
    queued: VecDeque<Pos>,
    /// The next unit to consider submitting.
    cursor: Pos,
}

/// The import paths' tasks, walked as they're needed: by the main loop on
/// reaching a path, or earlier by the lookahead.
struct Plan {
    paths: Vec<PathBuf>,
    roots: Vec<Option<RootPlan>>,
}

enum RootPlan {
    Ready {
        groups: Vec<Group>,
        /// What the walk logged, replayed when the import reaches the path.
        log: Vec<String>,
        /// The tasks a resumed import skips, if known without asking.
        done: Option<BTreeSet<PathBuf>>,
    },
    /// Not importable (the main loop reports why on reaching it).
    Failed,
}

pub struct Importer<'a> {
    cfg: &'a Config,
    lib: &'a Library,
    fmt: PathFormatter,
    mb: Arc<MusicBrainz>,
    source: Arc<dyn CandidateSource>,
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
    /// The unit about to be imported, as the pipeline read it.
    prepared: Option<Prepared>,
    /// Its lookups, taken by the prompts they were done for.
    lookups: HashMap<Vec<PathBuf>, Looked>,
}

/// beets' `_MULTIDISC_MARKERS`, as `MULTIDISC_PATTERNS`: a disc marker,
/// optional punctuation, then a digit; group 1 is everything before it.
static MULTIDISC_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"dis[ck]", r"cd", r"cassette", r"digital\s+media", r"vinyl"]
        .iter()
        .map(|m| Regex::new(&format!(r"(?i)^(.*{m}[\W_]*)\d")).unwrap())
        .collect()
});

/// beets' `get_numbered_variant_pattern`: the same prefix, then a digit.
fn numbered_variant(prefix: &str) -> Regex {
    Regex::new(&format!("(?i)^{}\\d", regex::escape(prefix))).unwrap()
}

/// beets' `sorted_walk`: depth-first, each directory before its
/// subdirectories, names sorted case-insensitively, entries `filter` skips
/// left out (the root itself never is). Yields (directory, subdirectory
/// names, files). Symlinked directories are followed, once each.
fn sorted_walk(root: &Path, filter: &WalkFilter) -> Vec<(PathBuf, Vec<String>, Vec<PathBuf>)> {
    fn walk(
        dir: &Path,
        filter: &WalkFilter,
        seen: &mut HashSet<PathBuf>,
        out: &mut Vec<(PathBuf, Vec<String>, Vec<PathBuf>)>,
    ) {
        if let Ok(real) = std::fs::canonicalize(dir)
            && !seen.insert(real)
        {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                crate::logging::emit(format!(
                    "warning: could not list directory {}: {e}",
                    dir.display()
                ));
                return;
            }
        };
        let mut dirs = vec![];
        let mut files = vec![];
        for e in entries.flatten() {
            let name = e.file_name();
            if filter.skips(&name) {
                continue;
            }
            let name = name.to_string_lossy().into_owned();
            if e.path().is_dir() {
                dirs.push(name);
            } else {
                files.push(name);
            }
        }
        let key = |n: &String| (n.to_lowercase(), n.clone());
        dirs.sort_by_key(key);
        files.sort_by_key(key);
        let files = files.iter().map(|f| dir.join(f)).collect();
        out.push((dir.to_path_buf(), dirs.clone(), files));
        for d in dirs {
            walk(&dir.join(d), filter, seen, out);
        }
    }
    let mut out = vec![];
    walk(root, filter, &mut HashSet::new(), &mut out);
    out
}

/// beets' `albums_in_dir`: (directories, files) for each directory holding
/// files, collapsing multi-disc albums: a directory with no files of its
/// own whose subdirectories all look like discs of one sequence ("CD 1",
/// "cd 3 - bonus"), or a run of directories named like numbered discs
/// ("Album Disc 1", "Album Disc 2", "Album (CD 1)"); a collapsed album's
/// directories include every subdirectory walked inside it.
fn albums_in_dir(root: &Path, filter: &WalkFilter) -> Vec<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut out = vec![];
    let mut collapse_paths: Vec<PathBuf> = vec![];
    let mut collapse_items: Vec<PathBuf> = vec![];
    let mut collapse_pat: Option<Regex> = None;
    let base = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    for (dir, dirs, items) in sorted_walk(root, filter) {
        if !collapse_paths.is_empty() {
            let inside = collapse_paths
                .iter()
                .any(|p| dir != *p && dir.starts_with(p));
            if inside
                || collapse_pat
                    .as_ref()
                    .is_some_and(|r| r.is_match(&base(&dir)))
            {
                collapse_paths.push(dir);
                collapse_items.extend(items);
                continue;
            }
            if !collapse_items.is_empty() {
                out.push((
                    std::mem::take(&mut collapse_paths),
                    std::mem::take(&mut collapse_items),
                ));
            }
            collapse_pat = None;
            collapse_paths.clear();
            collapse_items.clear();
        }

        let mut start = false;
        for marker in MULTIDISC_PATTERNS.iter() {
            if !dirs.is_empty() && items.is_empty() {
                // A nested multi-disc album: the first subdirectory sets the
                // pattern the others must follow.
                start = true;
                let mut sub_pat: Option<Regex> = None;
                for sub in &dirs {
                    match &sub_pat {
                        None => match marker.captures(sub) {
                            Some(c) => sub_pat = Some(numbered_variant(&c[1])),
                            None => {
                                start = false;
                                break;
                            }
                        },
                        Some(r) if !r.is_match(sub) => {
                            start = false;
                            break;
                        }
                        Some(_) => {}
                    }
                }
                if start {
                    break;
                }
            } else if let Some(c) = marker.captures(&base(&dir)) {
                // The first directory of a flattened multi-disc album.
                start = true;
                collapse_pat = Some(numbered_variant(&c[1]));
                break;
            }
        }
        if start {
            collapse_paths = vec![dir];
            collapse_items = items;
            continue;
        }
        if !items.is_empty() {
            out.push((vec![dir], items));
        }
    }
    if !collapse_paths.is_empty() && !collapse_items.is_empty() {
        out.push((collapse_paths, collapse_items));
    }
    out
}

/// The import tasks under `root`, as beets' `ImportTaskFactory.paths`: each
/// with its directories (or the file itself, for a single-file import) and
/// its audio files; groups with no audio are dropped, as beets drops tasks
/// with no readable items. Directory walks skip what `filter` says to
/// (`ignore`, `ignore_hidden`); a file given directly is never skipped.
pub fn group_files(root: &Path, filter: &WalkFilter) -> Result<Vec<(Vec<PathBuf>, Vec<PathBuf>)>> {
    if root.is_file() {
        return Ok(if media::is_audio(root) {
            vec![(vec![root.to_path_buf()], vec![root.to_path_buf()])]
        } else {
            vec![]
        });
    }
    if !root.is_dir() {
        bail!("no such file or directory: {}", root.display());
    }
    Ok(albums_in_dir(root, filter)
        .into_iter()
        .filter_map(|(dirs, files)| {
            let audio: Vec<PathBuf> = files.into_iter().filter(|f| media::is_audio(f)).collect();
            (!audio.is_empty()).then_some((dirs, audio))
        })
        .collect())
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
        let mb = Arc::new(MusicBrainz::new(cfg));
        let source = Arc::new(MbSource {
            cfg: cfg.clone(),
            mb: mb.clone(),
            ids: opts.search_ids.clone(),
        });
        Ok(Importer {
            log,
            mb,
            source,
            prepared: None,
            lookups: HashMap::new(),
            merged: HashSet::new(),
            clutter: fileops::clutter(cfg),
            walk: WalkFilter::from_config(cfg),
            cfg,
            lib,
            fmt: PathFormatter::new(cfg)?,
            opts,
            stats: Stats::default(),
            roots: vec![],
            plugins,
            state: ImportState::load(&cfg.state_path()),
            state_path: cfg.state_path(),
        })
    }

    /// Take each task's first lookup from `src` instead of MusicBrainz.
    #[cfg(test)]
    fn with_source(mut self, src: Arc<dyn CandidateSource>) -> Self {
        self.source = src;
        self
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
        let mut plan = Plan {
            paths: paths.to_vec(),
            roots: paths.iter().map(|_| None).collect(),
        };
        let mut ahead = (self.opts.threaded && !self.opts.pretend).then(|| self.start_pipeline());
        for (ri, p) in paths.iter().enumerate() {
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
            let groups = match plan.roots[ri].take() {
                Some(RootPlan::Ready { groups, log, .. }) => {
                    crate::logging::replay(&log);
                    groups
                }
                _ => self.root_groups(&p)?,
            };
            plan.roots[ri] = Some(RootPlan::Ready {
                groups: groups.clone(),
                log: vec![],
                done: Some(done.clone()),
            });
            for (gi, (paths, group)) in groups.into_iter().enumerate() {
                // The state (progress, history) keys a task by its first
                // path: the album's directory, the parent of nested discs,
                // or the first of a run of sibling disc directories.
                let key = paths[0].clone();
                if done.contains(&key) {
                    self.stats.already += 1;
                    continue;
                }
                if self.opts.incremental && self.state.history.contains(&key) {
                    crate::logging::debug!("Skipping previously-imported path: {}", key.display());
                    self.stats.already += 1;
                    continue;
                }
                if self.opts.pretend {
                    self.log_files(&key, &group)?;
                    continue;
                }
                let outcome = if self.opts.singletons {
                    let mut outcome = Outcome::Done;
                    for (fi, f) in group.iter().enumerate() {
                        self.prepare(ahead.as_mut(), &mut plan, (ri, gi, fi), track_progress);
                        let o = self.import_singleton(f);
                        self.unprepare();
                        match o? {
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
                    self.prepare(ahead.as_mut(), &mut plan, (ri, gi, 0), track_progress);
                    let o = self.import_album(&paths, &group);
                    self.unprepare();
                    o?
                };
                if outcome == Outcome::Aborted {
                    // Stop the lookahead first: it only reads, so nothing
                    // it was doing needs finishing.
                    drop(ahead);
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

    /// An import path's tasks: its album groups, or one with every file
    /// (`--flat`).
    fn root_groups(&self, top: &Path) -> Result<Vec<Group>> {
        Ok(if self.opts.flat && top.is_dir() {
            let files = group_files(top, &self.walk)?
                .into_iter()
                .flat_map(|(_, f)| f)
                .collect();
            vec![(vec![top.to_path_buf()], files)]
        } else {
            group_files(top, &self.walk)?
        })
    }

    /// Start the threaded pipeline: one thread reads the files of upcoming
    /// units, another looks up their candidates.
    fn start_pipeline(&self) -> Ahead {
        let mode = match (
            self.opts.autotag,
            self.opts.singletons,
            self.opts.group_albums,
        ) {
            (false, _, _) => LookupMode::None,
            (true, true, _) => LookupMode::Tracks,
            (true, false, true) => LookupMode::Groups,
            (true, false, false) => LookupMode::Album,
        };
        let src = self.source.clone();
        let pipe = Lookahead::new(
            |(pos, files): (Pos, Vec<PathBuf>), _: &Cancel| {
                let stamps = stamps(&files);
                let (items, log) = crate::logging::capture(|| read_files(&files, &HashSet::new()));
                Prepared {
                    pos,
                    files,
                    stamps,
                    items,
                    log,
                    lookups: vec![],
                }
            },
            move |mut p: Prepared, cancel: &Cancel| {
                p.lookups = look_up(src.as_ref(), mode, &p.items, cancel);
                p
            },
        );
        Ahead {
            pipe,
            queued: VecDeque::new(),
            cursor: (0, 0, 0),
        }
    }

    /// Before importing the unit at `cur`: queue the units after it, up to
    /// [`LOOKAHEAD`], and take its own from the pipeline if it was queued.
    fn prepare(&mut self, ahead: Option<&mut Ahead>, plan: &mut Plan, cur: Pos, progress: bool) {
        let Some(a) = ahead else {
            return;
        };
        // Units the import passed over (an abort of singletons, a skipped
        // path) are dropped first, so they don't count against the bound.
        while a.queued.front().is_some_and(|&q| q < cur) {
            a.queued.pop_front();
            if a.pipe.next().is_none() {
                a.queued.clear();
            }
        }
        self.plan_ahead(a, plan, cur, progress);
        if a.queued.front() == Some(&cur) {
            a.queued.pop_front();
            match a.pipe.next() {
                Some(p) => {
                    debug_assert_eq!(p.pos, cur);
                    self.prepared = Some(p);
                }
                // The pipeline died; carry on without it.
                None => a.queued.clear(),
            }
        }
    }

    fn unprepare(&mut self) {
        self.prepared = None;
        self.lookups.clear();
    }

    /// Queue units from the cursor on until [`LOOKAHEAD`] are in flight,
    /// skipping the tasks the main loop will skip. Planning stops at an
    /// import path that needs asking whether to resume, until it's asked.
    fn plan_ahead(&self, a: &mut Ahead, plan: &mut Plan, cur: Pos, progress: bool) {
        a.cursor = a.cursor.max(cur);
        while a.pipe.in_flight() < LOOKAHEAD {
            let (r, g, f) = a.cursor;
            if r >= plan.roots.len() {
                return;
            }
            if plan.roots[r].is_none() {
                plan.roots[r] = Some(self.plan_root(&plan.paths[r], progress));
            }
            let Some(RootPlan::Ready { groups, done, .. }) = &plan.roots[r] else {
                return;
            };
            let Some(done) = done else {
                return;
            };
            let Some((paths, files)) = groups.get(g) else {
                a.cursor = (r + 1, 0, 0);
                continue;
            };
            let key = &paths[0];
            if done.contains(key)
                || (self.opts.incremental && self.state.history.contains(key))
                || f >= files.len()
            {
                a.cursor = (r, g + 1, 0);
                continue;
            }
            let unit = if self.opts.singletons {
                a.cursor = (r, g, f + 1);
                vec![files[f].clone()]
            } else {
                a.cursor = (r, g + 1, 0);
                files.clone()
            };
            if !a.pipe.submit(((r, g, f), unit)) {
                return; // the pipeline died
            }
            a.queued.push_back((r, g, f));
        }
    }

    /// Walk import path `r` ahead of the main loop, capturing what the walk
    /// logs, and work out which of its tasks a resumed import would skip.
    fn plan_root(&self, top: &Path, progress: bool) -> RootPlan {
        let Ok(top) = std::fs::canonicalize(top) else {
            return RootPlan::Failed;
        };
        let (groups, log) = crate::logging::capture(|| self.root_groups(&top));
        let Ok(groups) = groups else {
            return RootPlan::Failed;
        };
        let done = match self.state.progress.get(&top) {
            Some(d) if progress => (self.opts.resume == Resume::Yes).then(|| d.clone()),
            _ => Some(BTreeSet::new()),
        };
        RootPlan::Ready { groups, log, done }
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
        if let Some(p) = self.prepared.take()
            && p.files == files
            && !files.iter().any(|f| self.merged.contains(f))
            && p.stamps == stamps(files)
        {
            crate::logging::replay(&p.log);
            self.lookups = p.lookups.into_iter().collect();
            return Ok(p.items);
        }
        // Files already in the library are reimported, replacing their
        // rows, unless a merge already took them this run. Path order, as
        // beets: the library ids follow it.
        Ok(read_files(files, &self.merged))
    }

    /// The first lookup for these items, if the pipeline did it.
    fn prefetched(&mut self, items: &[Item]) -> Option<Result<Candidates>> {
        let l = self.lookups.remove(&item_paths(items))?;
        crate::logging::replay(&l.log);
        Some(l.result)
    }

    /// The match display, with `--detail` (or `import.detail`).
    fn display(&self) -> Display {
        let mut d = Display::new(self.cfg);
        d.show_all = self.opts.detail;
        d
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

    /// `paths`: the task's directories (the import path with `--flat`).
    fn import_album(&mut self, paths: &[PathBuf], files: &[PathBuf]) -> Result<Outcome> {
        let items = self.read_items(files)?;
        if items.is_empty() {
            return Ok(Outcome::Done);
        }
        let dir = if self.opts.flat && paths[0].is_dir() {
            paths[0].clone()
        } else {
            files[0].parent().unwrap_or(Path::new("")).to_path_buf()
        };
        if self.opts.group_albums {
            self.import_groups(items, &dir)
        } else {
            self.import_album_items(items, &dir, paths)
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
        // As beets: the task's paths, one per line. With autotagging this is
        // `choose_match`'s header on stdout; without it beets never calls
        // `choose_match` and `import_asis` only logs the paths at info level,
        // which goes to stderr, plain, even without -v.
        let shown: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        if self.opts.autotag {
            println!();
            println!(
                "{} {}",
                ui::colorize("import_path", &shown.join("\n")),
                ui::colorize("import_path_items", &format!("({} items)", items.len()))
            );
        } else {
            eprintln!("{}", shown.join("\n"));
        }

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
        let (mut cands, mut rec) = match self.prefetched(items) {
            Some(Ok(Candidates::Album(c, r))) => (c, r),
            Some(Err(e)) => return Err(e),
            _ => self.source.album(items)?,
        };

        // `fresh`: candidates just (re)fetched, so quiet/strong handling
        // applies. `focus`: the candidate shown for confirmation, with
        // whether the prompt may have a default (`default_action`): beets
        // requires an explicit choice at low recommendations and for any
        // candidate but the first. None shows the candidate list.
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

            let choice = if let Some((k, has_default)) = focus {
                self.show_album_match(items, &cands[k]);
                self.bell();
                match ui::options(
                    &ALBUM_CONFIRM,
                    None,
                    has_default
                        .then_some(self.opts.default_action)
                        .flatten()
                        .map(Choice::Letter),
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
                    println!("No matching release found for {} tracks.", items.len());
                    println!(
                        "For help, see: https://beets.readthedocs.org/en/latest/faq.html#nomatch"
                    );
                }
                self.pick_album_candidate(items, &cands)?
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

    /// beets' candidate list ("Finding tags for album ...") and prompt.
    fn pick_album_candidate(&self, items: &[Item], cands: &[AlbumMatch]) -> Result<Option<Choice>> {
        if cands.is_empty() {
            return Ok(ui::options(&ALBUM_CHOICES, None, Some(Choice::Letter('s'))));
        }
        let cur = autotag::current_metadata(items);
        let disp = self.display();
        println!();
        println!("Finding tags for album \"{} - {}\".", cur.artist, cur.album);
        println!("  Candidates:");
        for (n, m) in cands.iter().enumerate() {
            for line in disp.candidate_lines(
                n + 1,
                &m.distance,
                &m.info.artist,
                &m.info.album,
                &Display::album_disambig(&m.info),
            ) {
                println!("{line}");
            }
        }
        Ok(ui::options(
            &ALBUM_CHOICES,
            Some(cands.len()),
            Some(Choice::Number(1)),
        ))
    }

    fn show_album_match(&self, items: &[Item], m: &AlbumMatch) {
        let cur = autotag::current_metadata(items);
        let d = self.display();
        d.show_album_change(items, &cur, m);
        d.flush();
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
        // The library narrows the candidates in SQL (beets' duplicate query);
        // `matches` still has the final say, so keys SQL can't check exactly
        // behave as before.
        if album {
            for a in self
                .lib
                .duplicate_candidates(crate::model::Kind::Album, want, false)?
            {
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
            for it in self
                .lib
                .duplicate_candidates(crate::model::Kind::Item, want, true)?
            {
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
            eprintln!("This {what} is already in the library!");
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
                match ui::options(&DuplicateAction::PROMPT, None, Some(Choice::Letter('s'))) {
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
        let paths: Vec<&Path> = items.iter().map(|i| i.path()).collect();
        for dups in self.lib.items_with_paths(&paths)? {
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
                let v = t.render(&Formatted::new(a, Some(self.lib)));
                set(a, field, v);
            }
            for item in items.iter_mut() {
                let v = t.render(&Formatted::new(item, Some(self.lib)).album(album.as_deref()));
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
        // As beets, the stored mtime stays the one read from the source
        // (0 when the tags were changed and not written); a write records
        // the file's new one.
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
        // beets: `choose_item`'s header when autotagging, else an info log
        // line on stderr (see `import_album_items`).
        if self.opts.autotag {
            println!();
            println!("{}", item.path().display());
        } else {
            eprintln!("{}", item.path().display());
        }
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
                autotag::apply_track(self.cfg, &mut item, &m.info);
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
        item.set_int("album_id", 0);
        let lib = self.lib;
        let copied = lib.transaction(|lib| {
            let replaced = self.record_replaced(std::slice::from_ref(&item))?;
            self.remove_replaced(&replaced)?;
            self.reimport_metadata(None, std::slice::from_mut(&mut item), &replaced);
            self.apply_set_fields(None, std::slice::from_mut(&mut item));
            let op = self.file_op(&item, !replaced.items[0].is_empty());
            // As beets: added before its files are placed, so the path
            // format sees the item's id (`%sunique`).
            lib.add(&mut item)?;
            let copied = self.place_and_write(&mut item, None, tagged, op)?;
            lib.store(&item)?;
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

    fn choose_item(&mut self, item: &Item) -> Result<Decision<TrackMatch>> {
        // An album's lookup for a lone track ("as Tracks") isn't this one.
        let (mut cands, mut rec) = match self.prefetched(std::slice::from_ref(item)) {
            Some(Ok(Candidates::Track(c, r))) => (c, r),
            Some(Err(e)) => return Err(e),
            _ => self.source.item(item)?,
        };
        let disp = self.display();
        if let Some(d) = self.summary_judgment(rec) {
            return Ok(d);
        }
        // The same dance as `choose_album` (beets' `choose_candidate`
        // serves both): `fresh` after a lookup, `focus` while confirming.
        let mut fresh = true;
        let mut focus: Option<(usize, bool)> = None;
        loop {
            if fresh {
                fresh = false;
                if !cands.is_empty() && rec == Recommendation::Strong && !self.opts.timid {
                    let m = cands.swap_remove(0);
                    disp.show_track_change(item, &m);
                    disp.flush();
                    return Ok(Decision::Apply(Box::new(m)));
                }
                focus = (!cands.is_empty() && rec != Recommendation::None)
                    .then_some((0, rec > Recommendation::Low));
            }

            let choice = if let Some((k, has_default)) = focus {
                disp.show_track_change(item, &cands[k]);
                disp.flush();
                self.bell();
                match ui::options(
                    &ITEM_CONFIRM,
                    None,
                    has_default
                        .then_some(self.opts.default_action)
                        .flatten()
                        .map(Choice::Letter),
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
                self.pick_item_candidate(&disp, item, &cands)
            };

            match choice {
                None | Some(Choice::Letter('b')) => return Ok(Decision::Abort),
                Some(Choice::Number(n)) => {
                    focus = Some((n - 1, n == 1 && rec > Recommendation::Low));
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
                    fresh = true;
                }
                Some(Choice::Letter('i')) => {
                    let id = ui::read_line("Enter recording ID: ").unwrap_or_default();
                    (cands, rec) = autotag::item_candidates(self.cfg, &self.mb, item, None, &[id])?;
                    rec = rec.min(Recommendation::Medium);
                    fresh = true;
                }
                _ => {}
            }
        }
    }

    /// beets' candidate list for a track ("Finding tags for track ...") and
    /// prompt; with no candidates, just the prompt.
    fn pick_item_candidate(
        &self,
        disp: &Display,
        item: &Item,
        cands: &[TrackMatch],
    ) -> Option<Choice> {
        if cands.is_empty() {
            println!("No matching recordings found.");
            return ui::options(&ITEM_CHOICES, None, Some(Choice::Letter('s')));
        }
        println!();
        println!(
            "Finding tags for track \"{} - {}\".",
            item.str("artist"),
            item.str("title")
        );
        println!("  Candidates:");
        for (n, m) in cands.iter().enumerate() {
            for line in disp.candidate_lines(
                n + 1,
                &m.distance,
                &m.info.artist,
                &m.info.title,
                &disp.track_disambig(&m.info),
            ) {
                println!("{line}");
            }
        }
        ui::options(&ITEM_CHOICES, Some(cands.len()), Some(Choice::Number(1)))
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
        // The task's paths, shown in the import header: the parent then
        // each disc directory for a merged album, else the directory.
        let b = d.path().join("B");
        assert_eq!(groups[1].0, vec![b.clone(), b.join("CD1"), b.join("CD2")]);
        assert_eq!(groups[0].0, vec![d.path().join("A")]);
        let one = d.path().join("A/01.mp3");
        assert_eq!(
            group_files(&one, &WalkFilter::default()).unwrap(),
            [(vec![one.clone()], vec![one])]
        );
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    /// Upstream's `AlbumsInDirTest`.
    #[test]
    fn albums_in_dir_finds_each_album() {
        let d = tempfile::tempdir().unwrap();
        for f in [
            "album1/album1song1.mp3",
            "album1/album1song2.mp3",
            "album2/album2song.mp3",
            "more/album3/album3song.mp3",
            "more/album4/album4song.mp3",
        ] {
            touch(&d.path().join(f));
        }
        let albums = group_files(d.path(), &WalkFilter::default()).unwrap();
        let found: Vec<(String, usize)> = albums
            .iter()
            .map(|(_, f)| {
                let name = f[0].file_name().unwrap().to_string_lossy();
                (name[..6].to_string(), f.len())
            })
            .collect();
        assert_eq!(
            found,
            [
                ("album1".to_string(), 2),
                ("album2".into(), 1),
                ("album3".into(), 1),
                ("album4".into(), 1)
            ]
        );
    }

    /// Upstream's `MultiDiscAlbumsInDirTest.create_music`: the directories
    /// and, with `files`, a song in each but the two nested-album roots.
    fn multidisc_tree(base: &Path, files: bool, name: &str, alt: &str) -> Vec<PathBuf> {
        let dirs: Vec<PathBuf> = [
            // Nested, several subdirectories; a false-positive marker in
            // the root and a subtitle for disc 3.
            "ABCD1234".to_string(),
            "ABCD1234/cd 1".into(),
            "ABCD1234/cd 3 - bonus".into(),
            // Nested, one subdirectory; punctuation before the number.
            "album".into(),
            "album/cd _ 1".into(),
            // Flattened, with a case typo; false positive in the parent.
            "artist [CD5]".into(),
            format!("artist [CD5]/{name} disc 1"),
            format!("artist [CD5]/{alt} disc 2"),
            // A single-disc album sorted between the discs.
            format!("artist [CD5]/{name} S"),
        ]
        .iter()
        .map(|d| base.join(d))
        .collect();
        for d in &dirs {
            std::fs::create_dir_all(d).unwrap();
        }
        if files {
            for (i, d) in [0, 1, 2, 4, 6, 7, 8].iter().enumerate() {
                touch(&dirs[*d].join(format!("song{i}.mp3")));
            }
        }
        dirs
    }

    #[test]
    fn albums_in_dir_coalesces_multidisc_albums_as_beets() {
        for (name, alt) in [("CAT", "CAt"), ("C\u{c1}T", "C\u{c1}t")] {
            let d = tempfile::tempdir().unwrap();
            let dirs = multidisc_tree(d.path(), true, name, alt);
            let albums = group_files(d.path(), &WalkFilter::default()).unwrap();
            let got: Vec<(Vec<PathBuf>, usize)> =
                albums.into_iter().map(|(p, f)| (p, f.len())).collect();
            assert_eq!(
                got,
                [
                    (dirs[0..3].to_vec(), 3),
                    (dirs[3..5].to_vec(), 1),
                    (dirs[6..8].to_vec(), 2),
                    (dirs[8..].to_vec(), 1),
                ],
                "{name}"
            );
        }
        // No empty albums.
        let d = tempfile::tempdir().unwrap();
        multidisc_tree(d.path(), false, "CAT", "CAt");
        assert!(
            group_files(d.path(), &WalkFilter::default())
                .unwrap()
                .is_empty()
        );
    }

    /// Upstream's `test_coalesce_markers`, plus "Album (CD 1)" siblings and
    /// a subfolder without audio, which still counts as one of the paths.
    #[test]
    fn albums_in_dir_coalesces_every_marker() {
        for (marker, s1, s2) in [
            ("Disc", " 1", " 02"),
            ("disk 757", " 1", " 02"),
            ("CD", "01", "02"),
            ("disc", "_1", "_2"),
            ("cAsSeTtE", " 1", " 02"),
            ("Digital   Media", " 1", " 02"),
            ("vinyl", " 1", " 02"),
            ("12 vinyl", " 1", " 02"),
        ] {
            let d = tempfile::tempdir().unwrap();
            let album = d.path().join("Album Name");
            let discs: Vec<PathBuf> = [s1, s2]
                .iter()
                .map(|s| album.join(format!("{marker}{s}")))
                .collect();
            for disc in &discs {
                touch(&disc.join("song.mp3"));
            }
            let albums = group_files(d.path(), &WalkFilter::default()).unwrap();
            assert_eq!(albums.len(), 1, "{marker}");
            for disc in &discs {
                assert!(albums[0].0.contains(disc), "{marker}");
            }
            assert_eq!(albums[0].1.len(), 2);
        }

        let d = tempfile::tempdir().unwrap();
        let p = |s: &str| d.path().join(s);
        touch(&p("Album (CD 1)/01.mp3"));
        touch(&p("Album (CD 1)/Scans/front.jpg"));
        touch(&p("Album (CD 2)/01.mp3"));
        touch(&p("Other/01.mp3"));
        let albums = group_files(d.path(), &WalkFilter::default()).unwrap();
        assert_eq!(
            albums,
            [
                (
                    vec![
                        p("Album (CD 1)"),
                        p("Album (CD 1)/Scans"),
                        p("Album (CD 2)")
                    ],
                    vec![p("Album (CD 1)/01.mp3"), p("Album (CD 2)/01.mp3")]
                ),
                (vec![p("Other")], vec![p("Other/01.mp3")]),
            ]
        );
        // As in beets, a parent holding a file of its own isn't a nested
        // multi-disc root, but its disc folders still collapse as siblings.
        let d = tempfile::tempdir().unwrap();
        let p = |s: &str| d.path().join(s);
        touch(&p("A/cover.jpg"));
        touch(&p("A/Disc 1/01.mp3"));
        touch(&p("A/Disc 2/01.mp3"));
        let albums = group_files(d.path(), &WalkFilter::default()).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].0, [p("A/Disc 1"), p("A/Disc 2")]);
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
        assert_eq!(g, [(vec![direct.clone()], vec![direct])]);
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
    fn default_action_is_the_confirm_prompts_default_letter() {
        let mut cfg = Config::defaults();
        assert_eq!(default_action(&cfg).unwrap(), Some('a'));
        for (v, want) in [("skip", Some('s')), ("asis", Some('u')), ("none", None)] {
            cfg.set("import.default_action", v.into());
            assert_eq!(default_action(&cfg).unwrap(), want);
            assert_eq!(ImportOptions::from_config(&cfg).default_action, want);
        }
        cfg.set("import.default_action", "later".into());
        let e = default_action(&cfg).unwrap_err().to_string();
        assert!(
            e.starts_with("import.default_action: must be one of"),
            "{e}"
        );
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
        item.set_list("genres", &["Polka"]);
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
        autotag::apply_track(&Config::defaults(), &mut item, &t);
        assert_eq!(item.str("title"), "New Title");
        assert_eq!(item.str("genres"), "");
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

    /// Import must not scale with library size: the duplicate check once
    /// loaded every album (and every attribute row) per imported album, so
    /// importing N albums into a big library was quadratic. With 50,000
    /// albums already in the library, importing 20 more stays quick.
    #[test]
    #[ignore = "timing; run with --ignored"]
    fn import_cost_does_not_grow_with_library_size() {
        let (_d, cfg, lib, src) = setup(&[]);
        lib.transaction(|lib| {
            for n in 0..50_000 {
                let mut it = Item::new_item();
                it.set_str("albumartist", format!("Artist {}", n % 500));
                it.set_str("album", format!("Album {n}"));
                it.set_path(format!("/music/{n}.mp3"));
                let mut a = lib.add_album(&mut [it])?;
                a.flex.insert("source".into(), "cd".into());
                lib.store(&a)?;
            }
            Ok(())
        })
        .unwrap();
        let mut dirs = vec![];
        for n in 0..20 {
            let Some(d) = album_with(
                &src,
                &format!("new{n}"),
                &format!("New {n}"),
                &[("One", 128)],
            ) else {
                return; // no ffmpeg
            };
            dirs.push(d);
        }
        let start = std::time::Instant::now();
        for d in &dirs {
            import_dir(&cfg, &lib, d, |o| o.op = FileOp::Copy);
        }
        let took = start.elapsed();
        assert_eq!(lib.all_albums().unwrap().len(), 50_020);
        assert!(
            took.as_secs_f64() < 2.0,
            "20 imports into 50k albums took {took:?}"
        );
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

    // As beets: an album's items get library ids in file (path) order, not
    // track order (found by tools/diffbeets).
    #[test]
    fn album_items_added_in_path_order() {
        let (_d, cfg, lib, src) = setup(&[]);
        let d = src.join("a");
        for (name, title, track) in [
            ("1.mp3", "First", "2"),
            ("2.mp3", "Second", ""),
            ("3.mp3", "Third", "1"),
        ] {
            let mut tags = vec![("artist", "Artist"), ("album", "Al"), ("title", title)];
            if !track.is_empty() {
                tags.push(("track", track));
            }
            if !make_mp3(&d, name, &tags, 128) {
                return;
            }
        }
        import_dir(&cfg, &lib, &d, |_| {});
        let a = &lib.all_albums().unwrap()[0];
        let mut items = lib.album_items(a.id.unwrap()).unwrap();
        items.sort_by_key(|i| i.id);
        let titles: Vec<&str> = items.iter().map(|i| i.str("title")).collect();
        assert_eq!(titles, ["First", "Second", "Third"]);
        // As-is took the artist as albumartist, a tag change not written to
        // the file: beets resets mtime then ("dirty"), so `update` re-reads.
        for i in &items {
            assert_eq!(i.str("albumartist"), "Artist");
            assert_eq!(i.float("mtime"), 0.0);
        }
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
                .display()
                .track_disambig(&info)
        };
        assert_eq!(disambig(&cfg), "MusicBrainz, [Alb]");
        cfg.set("import.singleton_album_disambig", false.into());
        assert_eq!(disambig(&cfg), "MusicBrainz");
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

    /// A candidate source that never finds anything, slowly, and records
    /// what it was asked for.
    struct FakeSource {
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        delay: std::time::Duration,
    }

    impl FakeSource {
        fn new(delay_ms: u64) -> (Arc<Self>, Arc<std::sync::Mutex<Vec<String>>>) {
            let calls = Arc::new(std::sync::Mutex::new(vec![]));
            let src = FakeSource {
                calls: calls.clone(),
                delay: std::time::Duration::from_millis(delay_ms),
            };
            (Arc::new(src), calls)
        }
    }

    impl CandidateSource for FakeSource {
        fn album(&self, items: &[Item]) -> Result<(Vec<AlbumMatch>, Recommendation)> {
            let names: Vec<&str> = items.iter().map(|i| i.str("title")).collect();
            let call = format!("album {} {}", items[0].str("album"), names.join(","));
            self.calls.lock().unwrap().push(call);
            std::thread::sleep(self.delay);
            Ok((vec![], Recommendation::None))
        }

        fn item(&self, item: &Item) -> Result<(Vec<TrackMatch>, Recommendation)> {
            let call = format!("item {} {}", item.str("album"), item.str("title"));
            self.calls.lock().unwrap().push(call);
            std::thread::sleep(self.delay);
            Ok((vec![], Recommendation::None))
        }
    }

    /// Albums A..F of two tracks each (the last a mix of two albums, for
    /// -g); None without ffmpeg.
    fn lookahead_corpus(src: &Path, n: usize) -> Option<()> {
        for (k, name) in ["A", "B", "C", "D", "E", "F"].iter().take(n).enumerate() {
            let album = if k == n - 1 { "Mixed" } else { name };
            album_with(src, name, album, &[("one", 128), ("two", 128)])?;
        }
        // Make the last directory hold two albums by tag.
        let last = src.join(["A", "B", "C", "D", "E", "F"][n - 1]);
        make_mp3(
            &last,
            "3.mp3",
            &[("artist", "Other"), ("album", "Else"), ("title", "three")],
            128,
        )
        .then_some(())
    }

    /// A library row: (id, album_id, album, title, path).
    type Row = (i64, i64, String, String, PathBuf);
    type Mode = (&'static str, fn(&mut ImportOptions));

    /// Import with the fake source in quiet mode (no match: as-is) and
    /// return the library's (id, album_id, album, title, path) rows and the
    /// lookups made, in order.
    fn quiet_import(
        threaded: bool,
        f: impl Fn(&mut ImportOptions),
    ) -> Option<(Vec<Row>, Vec<String>)> {
        let (d, cfg, lib, _) = setup(&[]);
        let src = d.path().join("music");
        lookahead_corpus(&src, 5)?;
        let mut o = opts(&cfg);
        o.autotag = true;
        o.quiet = true;
        o.quiet_fallback_asis = true;
        o.threaded = threaded;
        f(&mut o);
        let (fake, calls) = FakeSource::new(5);
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap().with_source(fake);
        imp.run(std::slice::from_ref(&src)).unwrap();
        let rows = all_items(&lib)
            .iter()
            .map(|i| {
                (
                    i.id.unwrap(),
                    i.int("album_id"),
                    i.str("album").to_string(),
                    i.str("title").to_string(),
                    i.path().strip_prefix(&src).unwrap().to_path_buf(),
                )
            })
            .collect();
        let calls = calls.lock().unwrap().clone();
        Some((rows, calls))
    }

    #[test]
    fn threaded_import_does_what_the_sequential_one_does() {
        let modes: [Mode; 4] = [
            ("albums", |_| {}),
            ("singletons", |o| o.singletons = true),
            ("group albums", |o| o.group_albums = true),
            ("flat", |o| o.flat = true),
        ];
        for (name, f) in modes {
            let Some(seq) = quiet_import(false, f) else {
                return; // no ffmpeg
            };
            let thr = quiet_import(true, f).unwrap();
            assert_eq!(seq, thr, "{name}");
            assert!(!seq.0.is_empty() && !seq.1.is_empty(), "{name}");
            // Each lookup happens once, in import order.
            let unique: HashSet<&String> = thr.1.iter().collect();
            assert_eq!(unique.len(), thr.1.len(), "{name}: {:?}", thr.1);
        }
    }

    #[test]
    fn lookahead_is_bounded_and_stops_when_dropped() {
        let (d, cfg, lib, _) = setup(&[]);
        let src = d.path().join("music");
        if lookahead_corpus(&src, 6).is_none() {
            return;
        }
        let mut o = opts(&cfg);
        o.autotag = true;
        let (fake, calls) = FakeSource::new(0);
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap().with_source(fake);
        let mut plan = Plan {
            paths: vec![src.clone()],
            roots: vec![None],
        };
        let mut ahead = imp.start_pipeline();
        imp.prepare(Some(&mut ahead), &mut plan, (0, 0, 0), false);
        let p = imp.prepared.as_ref().expect("first unit prepared");
        assert_eq!(p.pos, (0, 0, 0));
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.lookups.len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(300));
        // The unit being imported and three more; then nothing until the
        // import moves on.
        assert_eq!(calls.lock().unwrap().len(), LOOKAHEAD);
        assert_eq!(ahead.queued.len(), LOOKAHEAD - 1);

        // Moving on to the third unit queues two more (the last there is).
        imp.unprepare();
        imp.prepare(Some(&mut ahead), &mut plan, (0, 2, 0), false);
        assert_eq!(imp.prepared.as_ref().unwrap().pos, (0, 2, 0));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(calls.lock().unwrap().len(), 6);

        // An abort drops the pipeline; it starts nothing more.
        let (fake, calls) = FakeSource::new(50);
        let mut imp = Importer::new(&cfg, &lib, opts(&cfg), &[])
            .unwrap()
            .with_source(fake);
        imp.opts.autotag = true;
        let mut plan = Plan {
            paths: vec![src.clone()],
            roots: vec![None],
        };
        let mut ahead = imp.start_pipeline();
        imp.prepare(Some(&mut ahead), &mut plan, (0, 0, 0), false);
        drop(ahead);
        std::thread::sleep(std::time::Duration::from_millis(300));
        // The first unit, and the one that was being looked up.
        let calls = calls.lock().unwrap().clone();
        assert!(calls.len() <= 2, "{calls:?}");
    }

    #[test]
    fn a_file_changed_after_the_lookahead_read_it_is_read_again() {
        let (d, cfg, lib, _) = setup(&[]);
        let src = d.path().join("music");
        if lookahead_corpus(&src, 2).is_none() {
            return;
        }
        let mut o = opts(&cfg);
        o.autotag = true;
        o.quiet = true; // no prompt
        let (fake, calls) = FakeSource::new(0);
        let mut imp = Importer::new(&cfg, &lib, o, &[]).unwrap().with_source(fake);
        let mut plan = Plan {
            paths: vec![src.clone()],
            roots: vec![None],
        };
        let mut ahead = imp.start_pipeline();
        imp.prepare(Some(&mut ahead), &mut plan, (0, 0, 0), false);
        let files = imp.prepared.as_ref().unwrap().files.clone();
        let f = std::fs::File::options()
            .write(true)
            .open(&files[0])
            .unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86400))
            .unwrap();
        let items = imp.read_items(&files).unwrap();
        assert_eq!(items.len(), 2);
        // Read afresh, so its lookup is done again too.
        assert!(imp.lookups.is_empty());
        assert!(imp.prepared.is_none());
        imp.choose_album(&items).ok();
        let calls = calls.lock().unwrap().clone();
        let first = calls.iter().filter(|c| *c == "album A one,two").count();
        assert_eq!(first, 2, "{calls:?}");
    }
}
