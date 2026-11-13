//! A port of beets 2.14's `mbsync` plugin (`beetsplug/mbsync.py`):
//! `beet mbsync` fetches each matched singleton's recording (by
//! `mb_trackid`) and each album's release (by `mb_albumid`) from
//! MusicBrainz again and applies it as the autotagger does (never from
//! scratch), showing the albums' changes, then moving, writing and
//! storing as `import.move`/`copy` and `import.write` (or `-m`/`-M`,
//! `-W`) say; `-p` only shows the changes.

use crate::config::Config;
use crate::fileops::FileOp;
use crate::hooks;
use crate::library::Library;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo};
use crate::model::{Item, Kind, Record};
use crate::paths::PathFormatter;
use crate::plugins::PluginLog;
use crate::query::Query;
use crate::template::Template;
use anyhow::Result;
use serde_yaml::Value as Y;
use std::collections::HashMap;

/// The plugin: only a command.
pub struct MbsyncPlugin;

impl crate::plugins::Plugin for MbsyncPlugin {
    fn name(&self) -> &'static str {
        "mbsync"
    }
}

/// `beet mbsync`'s options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub pretend: bool,
    /// -m / -M; None: `import.move` or `import.copy` (beets' `should_move`).
    pub mv: Option<bool>,
    /// -W; None: `import.write` (beets' `should_write`).
    pub write: Option<bool>,
    /// -f: `format_item` and `format_album` for this run.
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// Where the metadata comes from: the MusicBrainz client (a fake one in
/// tests), or none when the source plugin isn't enabled.
pub trait Source {
    fn album_for_id(&self, id: &str) -> Result<Option<AlbumInfo>>;
    fn track_for_id(&self, id: &str) -> Result<Option<TrackInfo>>;
}

impl Source for MusicBrainz {
    fn album_for_id(&self, id: &str) -> Result<Option<AlbumInfo>> {
        MusicBrainz::album_for_id(self, id)
    }
    fn track_for_id(&self, id: &str) -> Result<Option<TrackInfo>> {
        MusicBrainz::track_for_id(self, id)
    }
}

/// `beet mbsync`.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let mut cfg = cfg.clone();
    if let Some(f) = &opts.format {
        // `add_format_option()` without a target: both formats.
        cfg.set("format_item", Y::String(f.clone()));
        cfg.set("format_album", Y::String(f.clone()));
    }
    let mb = MusicBrainz::new(&cfg);
    Sync::new(&cfg, lib, &opts, &mb).run(&opts)
}

pub struct Sync<'a> {
    cfg: &'a Config,
    lib: &'a Library,
    source: &'a dyn Source,
    log: PluginLog,
    mv: bool,
    write: bool,
    pretend: bool,
    fmt: PathFormatter,
}

impl<'a> Sync<'a> {
    pub fn new(cfg: &'a Config, lib: &'a Library, opts: &CmdOpts, source: &'a dyn Source) -> Self {
        Sync {
            cfg,
            lib,
            source,
            log: PluginLog::new("mbsync"),
            mv: opts
                .mv
                .unwrap_or_else(|| cfg.bool("import.move") || cfg.bool("import.copy")),
            write: opts.write.unwrap_or_else(|| cfg.bool("import.write")),
            pretend: opts.pretend,
            fmt: PathFormatter::new(cfg).expect("path formats were checked at startup"),
        }
    }

    /// `MBSyncPlugin.func`: singletons, then albums.
    pub fn run(&self, opts: &CmdOpts) -> Result<()> {
        self.singletons(&opts.query, opts.limit)?;
        self.albums(&opts.query, opts.limit)
    }

    /// `str(model)`: `format_item` / `format_album`.
    fn show(&self, rec: &Record) -> String {
        let key = match rec.kind {
            Kind::Item => "format_item",
            Kind::Album => "format_album",
        };
        let t = Template::parse(&self.cfg.str(key));
        crate::commands::format_record(&t, rec, Some(self.lib))
    }

    /// `metadata_plugins.get_metadata_source(name)`: only MusicBrainz, and
    /// only while its plugin is enabled.
    fn source_for(&self, data_source: &str) -> Option<&dyn Source> {
        (data_source.eq_ignore_ascii_case("musicbrainz")
            && crate::plugins::enabled(self.cfg, "musicbrainz"))
        .then_some(self.source)
    }

    /// `maybe_handle_plugin_error`: a lookup failure is logged (unless
    /// `raise_on_error`) and counts as not found.
    fn lookup<T>(&self, method: &str, r: Result<Option<T>>) -> Result<Option<T>> {
        match r {
            Ok(v) => Ok(v),
            Err(e) if !self.cfg.bool("raise_on_error") => {
                crate::logging::emit(format!("Error in 'MusicBrainz.{method}': {e:#}"));
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// `MBSyncPlugin.singletons`.
    fn singletons(&self, query: &[String], limit: Option<usize>) -> Result<()> {
        let mut q = query.to_vec();
        q.push("singleton:true".into());
        let mut items = self.lib.items(&Query::parse(Kind::Item, &q)?)?;
        items.truncate(limit.unwrap_or(usize::MAX));
        for mut item in items {
            let track_id = item.str("mb_trackid").to_string();
            if track_id.is_empty() {
                self.log.info(&format!(
                    "Skipping singleton with no mb_trackid: {}",
                    self.show(&item)
                ));
                continue;
            }
            let ds = item
                .flex
                .get("data_source")
                .cloned()
                .unwrap_or_else(|| "MusicBrainz".into());
            let info = match self.source_for(&ds) {
                Some(s) => self.lookup("track_for_id", s.track_for_id(&track_id))?,
                None => None,
            };
            let Some(info) = info else {
                self.log.info(&format!(
                    "Recording ID not found: {track_id} for track {}",
                    self.show(&item)
                ));
                continue;
            };
            // `TrackMatch.apply_metadata(from_scratch=False)`.
            let o = hooks::ApplyOpts::from_config(self.cfg);
            hooks::update(&mut item, &hooks::track_item_data(&info.to_data(), &o));
            self.lib
                .transaction(|_| self.apply_item_changes(&mut item))?;
        }
        Ok(())
    }

    /// beets' `apply_item_changes`: move (if in the library), write, store.
    fn apply_item_changes(&self, item: &mut Item) -> Result<()> {
        if self.pretend {
            return Ok(());
        }
        if self.mv && item.path().starts_with(self.cfg.directory()) {
            // `item.move(with_album=False)`.
            let album = self.lib.get_album(item.int("album_id"))?;
            let dest = self.fmt.destination(item, album.as_ref(), Some(self.lib));
            crate::commands::move_item_file(self.cfg, item, &dest, FileOp::Move)?;
        }
        if self.write
            && let Err(e) = crate::media::write_item(item)
        {
            // `try_write`: logged, not fatal.
            crate::logging::emit(format!("{e:#}"));
        }
        self.lib.store(item)
    }

    /// `MBSyncPlugin.albums`.
    fn albums(&self, query: &[String], limit: Option<usize>) -> Result<()> {
        let mut albums = self.lib.albums(&Query::parse(Kind::Album, query)?)?;
        albums.truncate(limit.unwrap_or(usize::MAX));
        for album in albums {
            let album_id = album.str("mb_albumid").to_string();
            if album_id.is_empty() {
                self.log.info(&format!(
                    "Skipping album with no mb_albumid: {}",
                    self.show(&album)
                ));
                continue;
            }
            // `album.items()`, in the default item order.
            let q = vec![format!("album_id:{}", album.id.unwrap_or(0))];
            let mut items = self.lib.items(&Query::parse(Kind::Item, &q)?)?;
            let ds = match album.flex.get("data_source").filter(|d| !d.is_empty()) {
                Some(d) => d.clone(),
                None => match items.first() {
                    Some(i) => i
                        .flex
                        .get("data_source")
                        .cloned()
                        .unwrap_or_else(|| "MusicBrainz".into()),
                    None => continue,
                },
            };
            let info = match self.source_for(&ds) {
                Some(s) => self.lookup("album_for_id", s.album_for_id(&album_id))?,
                None => None,
            };
            let Some(info) = info else {
                self.log.info(&format!(
                    "Release ID {album_id} not found for album {}",
                    self.show(&album)
                ));
                continue;
            };
            let pairs = map_items(&items, &info);
            self.log
                .debug(&format!("applying changes to {}", self.show(&album)));
            self.lib.transaction(|lib| {
                // `AlbumMatch.apply_metadata(from_scratch=False)`.
                let o = hooks::ApplyOpts::from_config(self.cfg);
                for &(i, t) in &pairs {
                    let data = hooks::merge_with_album(&info.tracks[t], &info, &o);
                    hooks::update(&mut items[i], &data);
                }
                let mut changed = false;
                let mut any_changed = 0;
                for (n, item) in items.iter_mut().enumerate() {
                    let old = lib
                        .get_item(item.id.unwrap_or(0))?
                        .unwrap_or_else(|| item.clone());
                    if crate::commands::show_model_changes(&self.show(&old), &old, item) {
                        changed = true;
                        any_changed = n;
                        self.apply_item_changes(item)?;
                    }
                }
                if !changed || self.pretend {
                    return Ok(());
                }
                // The album takes its fields from a changed item.
                let mut new = album.clone();
                let from = &items[any_changed];
                for key in album_item_keys() {
                    if let Some(v) = from.get(key) {
                        new.set(key, v);
                    }
                }
                crate::commands::store_album_inherit(lib, &album, &new)?;
                // Move the art (and any inconsistent items).
                if self.mv && items[0].path().starts_with(self.cfg.directory()) {
                    self.log.debug(&format!("moving album {}", self.show(&new)));
                    crate::commands::move_model(self.cfg, lib, &new, FileOp::Move, None)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

/// beets' `Album.item_keys`: every album field but `artpath` and `id`.
fn album_item_keys() -> impl Iterator<Item = &'static str> {
    crate::model::schema(Kind::Album)
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| *n != "artpath")
}

/// The items' tracks on the release: by release track id where the item
/// has one on it, else by recording id (a recording on the release more
/// than once by disc and track number). Pairs of (item, track) indices.
pub fn map_items(items: &[Item], info: &AlbumInfo) -> Vec<(usize, usize)> {
    let mut by_release_track: HashMap<&str, usize> = HashMap::new();
    let mut by_recording: HashMap<&str, Vec<usize>> = HashMap::new();
    for (t, track) in info.tracks.iter().enumerate() {
        by_release_track.insert(&track.release_track_id, t);
        by_recording.entry(&track.track_id).or_default().push(t);
    }
    let mut pairs = vec![];
    for (i, item) in items.iter().enumerate() {
        let rt = item.str("mb_releasetrackid");
        if !rt.is_empty()
            && let Some(&t) = by_release_track.get(rt)
        {
            pairs.push((i, t));
            continue;
        }
        let candidates = by_recording
            .get(item.str("mb_trackid"))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let [t] = candidates {
            pairs.push((i, *t));
        } else if let Some(&t) = candidates.iter().find(|&&t| {
            let track = &info.tracks[t];
            track.medium_index as i64 == item.int("track")
                && track.medium as i64 == item.int("disc")
        }) {
            pairs.push((i, t));
        }
    }
    pairs
}

#[cfg(test)]
#[path = "mbsync_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "mbsync",
        order: 15,
        defaults: r#""#,
        config_keys: &["mbsync"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 190,
                plugin: "mbsync",
                build: build_mbsync,
                run: run_mbsync,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(MbsyncPlugin))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct MbsyncArgs {
    /// Show all changes but do nothing
    #[arg(short = 'p', long)]
    pretend: bool,
    /// Move files in the library directory
    #[arg(short = 'm', long = "move", overrides_with = "nomove")]
    mv: bool,
    /// Don't move files in library
    #[arg(short = 'M', long = "nomove", overrides_with = "mv")]
    nomove: bool,
    /// Don't write updated metadata to files
    #[arg(short = 'W', long = "nowrite")]
    nowrite: bool,
    /// Print with custom format
    #[arg(short = 'f', long)]
    format: Option<String>,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_mbsync() -> clap::Command {
    use clap::Args as _;
    MbsyncArgs::augment_args(
        clap::Command::new("mbsync").about("Update metadata from musicbrainz (plugin: mbsync)"),
    )
}

pub fn run_mbsync(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = MbsyncArgs::from_arg_matches(m)?;
    crate::plugins::mbsync::command(
        cfg,
        lib,
        crate::plugins::mbsync::CmdOpts {
            pretend: a.pretend,
            // The last of -m/-M given wins (clap's overrides).
            mv: if a.mv {
                Some(true)
            } else if a.nomove {
                Some(false)
            } else {
                None
            },
            write: a.nowrite.then_some(false),
            format: a.format,
            limit: a.limit,
            query: a.query,
        },
    )
}
