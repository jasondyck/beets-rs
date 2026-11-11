//! `beet import`: group files into albums, match them against MusicBrainz,
//! let the user choose, then copy/move, tag and add them to the library.

use crate::autotag::{self, AlbumMatch, Recommendation, TrackMatch};
use crate::config::Config;
use crate::library::{Library, now};
use crate::mb::MusicBrainz;
use crate::media;
use crate::model::{Album, Item, Value, format_duration};
use crate::paths::PathFormatter;
use crate::plugins::{self, Plugin, PluginCtx};
use crate::state::ImportState;
use crate::ui::{self, Choice};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Copy,
    Move,
    None,
}

pub struct ImportOptions {
    pub op: FileOp,
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
        let op = if cfg.bool("import.move") {
            FileOp::Move
        } else if cfg.bool("import.copy") {
            FileOp::Copy
        } else {
            FileOp::None
        };
        ImportOptions {
            op,
            write: cfg.bool("import.write"),
            quiet: cfg.bool("import.quiet"),
            quiet_fallback_asis: cfg.str("import.quiet_fallback") == "asis",
            timid: cfg.bool("import.timid"),
            autotag: cfg.bool("import.autotag"),
            singletons: cfg.bool("import.singletons"),
            group_albums: cfg.bool("import.group_albums"),
            search_ids: vec![],
            pretend: false,
            incremental: cfg.bool("import.incremental"),
            incremental_skip_later: cfg.bool("import.incremental_skip_later"),
            resume: match cfg.str("import.resume").as_str() {
                "yes" | "true" => Resume::Yes,
                "no" | "false" => Resume::No,
                _ => Resume::Ask,
            },
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
}

static DISC_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(cd|dis[ck]|side|part)[\s._-]*[0-9a-z]{1,3}\b").unwrap());

/// Group audio files into album-sized chunks: one per directory, merging
/// "CD1"/"Disc 2" style subdirectories into their parent. Each group comes
/// with its directory (or the file itself, for a single-file import), which
/// identifies it in the import state.
pub fn group_files(root: &Path) -> Result<Vec<(PathBuf, Vec<PathBuf>)>> {
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

/// Append ".1", ".2", ... before the extension until the path is free.
fn unique_path(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1.. {
        let cand = p.with_file_name(format!("{stem}.{n}{ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!()
}

pub fn move_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if std::fs::rename(src, dest).is_err() {
        // Cross-device: copy then remove.
        std::fs::copy(src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        std::fs::remove_file(src).with_context(|| format!("removing {}", src.display()))?;
    }
    Ok(())
}

pub fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::copy(src, dest)
        .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    Ok(())
}

/// Remove now-empty directories from `dir` up to (not including) `stop`.
pub fn prune_dirs(dir: &Path, stop: &Path) {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d == stop || !d.starts_with(stop) || std::fs::remove_dir(d).is_err() {
            break;
        }
        cur = d.parent();
    }
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
        Ok(Importer {
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
            for (key, group) in group_files(&p)? {
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
                    self.import_album(&group)?
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
            if self.lib.has_path(f)? {
                println!(
                    "{}",
                    ui::dim(&format!("Skipping {} (already in library)", f.display()))
                );
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

    fn import_album(&mut self, files: &[PathBuf]) -> Result<Outcome> {
        let items = self.read_items(files)?;
        if items.is_empty() {
            return Ok(Outcome::Done);
        }
        let dir = files[0].parent().unwrap_or(Path::new("")).to_path_buf();
        if self.opts.group_albums {
            self.import_groups(items, &dir)
        } else {
            self.import_album_items(items, &dir)
        }
    }

    /// "Group albums" and `-g`: run each tag-defined album in the directory
    /// through the album flow. An abort stops at once; otherwise the
    /// directory counts as skipped if any of its albums was.
    fn import_groups(&mut self, items: Vec<Item>, dir: &Path) -> Result<Outcome> {
        let mut outcome = Outcome::Done;
        for group in group_albums(items) {
            match self.import_album_items(group, dir)? {
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

    fn import_album_items(&mut self, mut items: Vec<Item>, dir: &Path) -> Result<Outcome> {
        println!();
        println!("{} ({} items)", dir.display(), items.len());

        let decision = if self.opts.autotag {
            self.choose_album(&items)?
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

        if !self.resolve_duplicates(&items, m.as_deref())? {
            self.stats.skipped += 1;
            return Ok(Outcome::Skipped);
        }

        if let Some(m) = &m {
            // Items with no matching track are dropped, as in beets.
            autotag::apply_album(self.cfg, &mut items, m);
            let keep: Vec<usize> = m.mapping.iter().map(|(i, _)| *i).collect();
            let mut idx = 0;
            items.retain(|_| {
                let k = keep.contains(&idx);
                idx += 1;
                k
            });
        } else if items.iter().all(|i| i.str("albumartist").is_empty()) {
            let cur = autotag::current_metadata(&items);
            for i in &mut items {
                i.set_str(
                    "albumartist",
                    if cur.va {
                        self.cfg.str("va_name")
                    } else {
                        cur.artist.clone()
                    },
                );
            }
        }
        if self.opts.pretend {
            for i in &items {
                println!("  {}", i.path().display());
            }
            return Ok(Outcome::Done);
        }
        self.finalize_album(items, m.is_some(), dir)?;
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
        loop {
            if fresh {
                fresh = false;
                if self.opts.quiet {
                    return Ok(match (cands.into_iter().next(), rec) {
                        (Some(m), Recommendation::Strong) => {
                            self.show_album_match(items, &m);
                            Decision::Apply(Box::new(m))
                        }
                        _ => {
                            let asis = self.opts.quiet_fallback_asis;
                            println!(
                                "{}",
                                if asis {
                                    "Importing as-is (no strong match)."
                                } else {
                                    "Skipping (no strong match)."
                                }
                            );
                            if asis { Decision::AsIs } else { Decision::Skip }
                        }
                    });
                }
                if !cands.is_empty() && rec == Recommendation::Strong && !self.opts.timid {
                    self.show_album_match(items, &cands[0]);
                    return Ok(Decision::Apply(Box::new(cands.swap_remove(0))));
                }
                // Show the best match directly when it's good enough.
                focus = (!cands.is_empty() && rec >= Recommendation::Medium).then_some((0, true));
            }

            let choice = if let Some((k, default_apply)) = focus {
                self.show_album_match(items, &cands[k]);
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
                Some(Choice::Number(n)) => focus = Some((n - 1, n == 1)),
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

    /// Returns false to skip this album.
    fn resolve_duplicates(&mut self, items: &[Item], m: Option<&AlbumMatch>) -> Result<bool> {
        let (artist, album, mbid) = match m {
            Some(m) => (
                if m.info.va {
                    self.cfg.str("va_name")
                } else {
                    m.info.artist.clone()
                },
                m.info.album.clone(),
                m.info.album_id.clone(),
            ),
            None => {
                let cur = autotag::current_metadata(items);
                (cur.artist, cur.album, String::new())
            }
        };
        let dups: Vec<Album> = self
            .lib
            .all_albums()?
            .into_iter()
            .filter(|a| {
                // Like beets' default duplicate_keys (albumartist album), plus same release ID.
                (!mbid.is_empty() && a.str("mb_albumid") == mbid)
                    || (a.str("albumartist").eq_ignore_ascii_case(&artist)
                        && a.str("album").eq_ignore_ascii_case(&album))
            })
            .collect();
        if dups.is_empty() {
            return Ok(true);
        }
        let action = self.cfg.str("import.duplicate_action");
        println!("{}", ui::yellow("This album is already in the library!"));
        let choice = match action.as_str() {
            "skip" => 's',
            "keep" => 'k',
            "remove" => 'r',
            _ if self.opts.quiet => 's',
            _ => match ui::options(&["Skip new", "Keep all", "Remove old"], None, None) {
                Some(Choice::Letter(c)) => c,
                _ => 's',
            },
        };
        match choice {
            'k' => Ok(true),
            'r' => {
                let libdir = self.cfg.directory();
                for a in dups {
                    for it in self.lib.album_items(a.id.unwrap())? {
                        if it.path().starts_with(&libdir) {
                            std::fs::remove_file(it.path()).ok();
                            if let Some(p) = it.path().parent() {
                                prune_dirs(p, &libdir);
                            }
                        }
                        self.lib.remove(&it)?;
                    }
                    self.lib.remove(&a)?;
                }
                Ok(true)
            }
            _ => {
                println!("Skipping.");
                Ok(false)
            }
        }
    }

    /// Place files, write tags and store the album + items in the library.
    fn finalize_album(
        &mut self,
        mut items: Vec<Item>,
        tagged: bool,
        source_dir: &Path,
    ) -> Result<()> {
        let added = now();
        for i in &mut items {
            i.set("added", Value::Float(added));
        }
        let n = items.len();
        let lib = self.lib;
        let mut album = lib.transaction(|lib| {
            let album = lib.add_album(&mut items)?;
            for item in &mut items {
                self.place_and_write(item, Some(&album), tagged)?;
                lib.store(item)?;
            }
            Ok(album)
        })?;
        let ctx = self.plugin_ctx();
        let op = self.opts.op;
        plugins::each(self.plugins, |p| {
            p.album_imported(&ctx, &mut album, &mut items, source_dir, op)
        });
        if op == FileOp::Move
            && let Some(root) = self.roots.last()
        {
            prune_dirs(source_dir, root);
        }
        self.stats.albums += 1;
        self.stats.items += n;
        Ok(())
    }

    fn plugin_ctx(&self) -> PluginCtx<'a> {
        PluginCtx {
            cfg: self.cfg,
            lib: self.lib,
            write: self.opts.write,
        }
    }

    fn place_and_write(&self, item: &mut Item, album: Option<&Album>, tagged: bool) -> Result<()> {
        let src = item.path().to_path_buf();
        let dest = self.fmt.destination(item, album, Some(self.lib));
        match self.opts.op {
            FileOp::None => {}
            _ if dest == src => {}
            FileOp::Copy => {
                let dest = unique_path(&dest);
                copy_file(&src, &dest)?;
                item.set_path(dest);
            }
            FileOp::Move => {
                let dest = unique_path(&dest);
                move_file(&src, &dest)?;
                item.set_path(dest);
                if let (Some(parent), Some(root)) = (src.parent(), self.roots.last()) {
                    prune_dirs(parent, root);
                }
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
        Ok(())
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
        let decision = if self.opts.autotag {
            self.choose_item(&item)?
        } else {
            Decision::AsIs
        };
        let tagged = match decision {
            Decision::Abort => return Ok(Outcome::Aborted),
            Decision::Skip => {
                self.stats.skipped += 1;
                return Ok(Outcome::Skipped);
            }
            Decision::AsIs => false,
            Decision::Apply(m) => {
                autotag::apply_track(&mut item, &m.info);
                true
            }
            Decision::Tracks | Decision::Group => unreachable!("not offered for tracks"),
        };
        if self.opts.pretend {
            return Ok(Outcome::Done);
        }
        item.set("added", Value::Float(now()));
        item.set_int("album_id", 0);
        self.place_and_write(&mut item, None, tagged)?;
        self.lib.add(&mut item)?;
        let ctx = self.plugin_ctx();
        plugins::each(self.plugins, |p| p.item_imported(&ctx, &mut item));
        self.stats.items += 1;
        Ok(Outcome::Done)
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
        loop {
            if self.opts.quiet {
                return Ok(match (cands.into_iter().next(), rec) {
                    (Some(m), Recommendation::Strong) => Decision::Apply(Box::new(m)),
                    _ if self.opts.quiet_fallback_asis => Decision::AsIs,
                    _ => Decision::Skip,
                });
            }
            if !cands.is_empty() && rec == Recommendation::Strong && !self.opts.timid {
                let m = cands.swap_remove(0);
                println!(
                    "  Match ({}): {} - {}",
                    ui::similarity(m.distance.distance(), strong, medium),
                    m.info.artist,
                    m.info.title
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
                    "{}. {} - {} ({}){}{}",
                    n + 1,
                    m.info.artist,
                    m.info.title,
                    ui::similarity(m.distance.distance(), strong, medium),
                    penalty,
                    ui::dim(&len_str(m.info.length))
                );
            }
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
        let groups = group_files(d.path()).unwrap();
        let sizes: Vec<usize> = groups.iter().map(|(_, g)| g.len()).collect();
        assert_eq!(sizes, vec![2, 2, 1, 1]);
        let keys: Vec<PathBuf> = groups.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys[1], d.path().join("B"));
    }

    #[test]
    fn unique_and_prune() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.mp3");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(unique_path(&f), d.path().join("x.1.mp3"));
        let deep = d.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        prune_dirs(&deep, d.path());
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
}
