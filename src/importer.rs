//! `beet import`: group files into albums, match them against MusicBrainz,
//! let the user choose, then copy/move, tag and add them to the library.

use crate::autotag::{self, AlbumMatch, Recommendation, TrackMatch};
use crate::config::Config;
use crate::library::{Library, now};
use crate::mb::MusicBrainz;
use crate::media;
use crate::model::{Album, Item, Value, format_duration};
use crate::paths::PathFormatter;
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
    pub search_ids: Vec<String>,
    pub pretend: bool,
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
            search_ids: vec![],
            pretend: false,
        }
    }
}

#[derive(Default, Debug)]
pub struct Stats {
    pub albums: usize,
    pub items: usize,
    pub skipped: usize,
}

enum Decision<M> {
    Apply(Box<M>),
    AsIs,
    Skip,
    Abort,
}

pub struct Importer<'a> {
    cfg: &'a Config,
    lib: &'a Library,
    fmt: PathFormatter,
    mb: MusicBrainz,
    opts: ImportOptions,
    pub stats: Stats,
    roots: Vec<PathBuf>,
}

static DISC_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(cd|dis[ck]|side|part)[\s._-]*[0-9a-z]{1,3}\b").unwrap());

/// Group audio files into album-sized chunks: one per directory, merging
/// "CD1"/"Disc 2" style subdirectories into their parent.
pub fn group_files(root: &Path) -> Result<Vec<Vec<PathBuf>>> {
    if root.is_file() {
        return Ok(if media::is_audio(root) {
            vec![vec![root.to_path_buf()]]
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
    Ok(groups.into_values().collect())
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
    pub fn new(cfg: &'a Config, lib: &'a Library, opts: ImportOptions) -> Result<Self> {
        Ok(Importer {
            cfg,
            lib,
            fmt: PathFormatter::new(cfg)?,
            mb: MusicBrainz::new(cfg),
            opts,
            stats: Stats::default(),
            roots: vec![],
        })
    }

    pub fn run(&mut self, paths: &[PathBuf]) -> Result<()> {
        for p in paths {
            let p = std::fs::canonicalize(p)
                .with_context(|| format!("no such file or directory: {}", p.display()))?;
            self.roots.push(if p.is_dir() {
                p.clone()
            } else {
                p.parent().unwrap_or(&p).to_path_buf()
            });
            for group in group_files(&p)? {
                let keep_going = if self.opts.singletons {
                    let mut go = true;
                    for f in group {
                        if !self.import_singleton(&f)? {
                            go = false;
                            break;
                        }
                    }
                    go
                } else {
                    self.import_album(&group)?
                };
                if !keep_going {
                    println!("Import aborted.");
                    return Ok(());
                }
            }
        }
        Ok(())
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

    /// Returns false if the user aborted.
    fn import_album(&mut self, files: &[PathBuf]) -> Result<bool> {
        let mut items = self.read_items(files)?;
        if items.is_empty() {
            return Ok(true);
        }
        let dir = files[0].parent().unwrap_or(Path::new("")).to_path_buf();
        println!();
        println!("{}", dir.display());

        let decision = if self.opts.autotag {
            self.choose_album(&items)?
        } else {
            Decision::AsIs
        };
        let m = match decision {
            Decision::Abort => return Ok(false),
            Decision::Skip => {
                self.stats.skipped += 1;
                return Ok(true);
            }
            Decision::AsIs => None,
            Decision::Apply(m) => Some(m),
        };

        if !self.resolve_duplicates(&items, m.as_deref())? {
            self.stats.skipped += 1;
            return Ok(true);
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
            return Ok(true);
        }
        self.finalize_album(items, m.is_some())?;
        Ok(true)
    }

    fn choose_album(&mut self, items: &[Item]) -> Result<Decision<AlbumMatch>> {
        let cur = autotag::current_metadata(items);
        println!("Tagging:\n    {} - {}", cur.artist, cur.album);
        let ids = self.opts.search_ids.clone();
        let (mut cands, mut rec) =
            autotag::album_candidates(self.cfg, &self.mb, items, None, &ids)?;

        loop {
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

            // Show the best match directly when it's good enough, else list candidates.
            let choice = if !cands.is_empty() && rec >= Recommendation::Medium {
                self.show_album_match(items, &cands[0]);
                match ui::options(
                    &[
                        "Apply",
                        "More candidates",
                        "Skip",
                        "Use as-is",
                        "Enter search",
                        "enter Id",
                        "aBort",
                    ],
                    None,
                    Some(Choice::Letter('a')),
                ) {
                    Some(Choice::Letter('a')) => {
                        return Ok(Decision::Apply(Box::new(cands.swap_remove(0))));
                    }
                    Some(Choice::Letter('m')) => self.pick_album_candidate(&cands)?,
                    other => other,
                }
            } else {
                if cands.is_empty() {
                    println!(
                        "{}",
                        ui::yellow("No matching release found for this album.")
                    );
                }
                self.pick_album_candidate(&cands)?
            };

            match choice {
                None | Some(Choice::Letter('b')) => return Ok(Decision::Abort),
                Some(Choice::Number(n)) => {
                    let m = cands.swap_remove(n - 1);
                    self.show_album_match(items, &m);
                    match ui::options(
                        &["Apply", "Skip", "Use as-is", "aBort"],
                        None,
                        Some(Choice::Letter('a')),
                    ) {
                        Some(Choice::Letter('a')) => return Ok(Decision::Apply(Box::new(m))),
                        Some(Choice::Letter('s')) => return Ok(Decision::Skip),
                        Some(Choice::Letter('u')) => return Ok(Decision::AsIs),
                        _ => return Ok(Decision::Abort),
                    }
                }
                Some(Choice::Letter('s')) => return Ok(Decision::Skip),
                Some(Choice::Letter('u')) => return Ok(Decision::AsIs),
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
                }
                Some(Choice::Letter('i')) => {
                    let id = ui::read_line("Enter release ID: ").unwrap_or_default();
                    (cands, rec) =
                        autotag::album_candidates(self.cfg, &self.mb, items, None, &[id])?;
                    // An explicitly chosen release is never auto-applied.
                    rec = rec.min(Recommendation::Medium);
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
            &["Skip", "Use as-is", "Enter search", "enter Id", "aBort"],
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
    fn finalize_album(&mut self, mut items: Vec<Item>, tagged: bool) -> Result<()> {
        let added = now();
        for i in &mut items {
            i.set("added", Value::Float(added));
        }
        let n = items.len();
        let lib = self.lib;
        lib.transaction(|lib| {
            let album = lib.add_album(&mut items)?;
            for item in &mut items {
                self.place_and_write(item, Some(&album), tagged)?;
                lib.store(item)?;
            }
            Ok(())
        })?;
        self.stats.albums += 1;
        self.stats.items += n;
        Ok(())
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

    fn import_singleton(&mut self, file: &Path) -> Result<bool> {
        let mut items = self.read_items(&[file.to_path_buf()])?;
        let Some(mut item) = items.pop() else {
            return Ok(true);
        };
        println!();
        println!("{}", file.display());
        let decision = if self.opts.autotag {
            self.choose_item(&item)?
        } else {
            Decision::AsIs
        };
        let tagged = match decision {
            Decision::Abort => return Ok(false),
            Decision::Skip => {
                self.stats.skipped += 1;
                return Ok(true);
            }
            Decision::AsIs => false,
            Decision::Apply(m) => {
                autotag::apply_track(&mut item, &m.info);
                true
            }
        };
        if self.opts.pretend {
            return Ok(true);
        }
        item.set("added", Value::Float(now()));
        item.set_int("album_id", 0);
        self.place_and_write(&mut item, None, tagged)?;
        self.lib.add(&mut item)?;
        self.stats.items += 1;
        Ok(true)
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
        let sizes: Vec<usize> = groups.iter().map(|g| g.len()).collect();
        assert_eq!(sizes, vec![2, 2, 1, 1]);
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
}
