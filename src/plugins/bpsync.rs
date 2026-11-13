//! A port of beets 2.14's deprecated `bpsync` plugin
//! (`beetsplug/bpsync.py`): update library metadata from Beatport. Items
//! whose `data_source` is `Beatport` and whose `mb_trackid` is a Beatport
//! track id, and albums likewise (by `mb_albumid`), are re-fetched through
//! the `beatport` metadata source, updated as the autotagger would (never
//! from scratch), shown item by item, then moved, written and stored as
//! `-m`/`-M`, `-W` and `import.move`/`import.copy`/`import.write` say; `-p`
//! only shows the changes.
//!
//! The plugin is dead in beets (it wraps the deprecated `beatport` source,
//! which needs OAuth credentials); beets-rs logs the same deprecation
//! warnings at load. Its command builds its own Beatport client, as beets'
//! `BPSyncPlugin.__init__` does with its `BeatportPlugin()` instance, so it
//! works whether or not `beatport` is in `plugins:`.

use crate::config::Config;
use crate::fileops::FileOp;
use crate::hooks;
use crate::library::Library;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo};
use crate::metadata_plugins::{MetadataSource, SourceCtx};
use crate::model::{Item, Kind, Record};
use crate::paths::PathFormatter;
use crate::plugins::PluginLog;
use crate::plugins::beatport::Beatport;
use crate::query::Query;
use crate::template::Template;
use anyhow::Result;
use serde_yaml::Value as Y;
use std::collections::HashMap;

/// Beatport's `data_source` name (beets' `BeatportPlugin.data_source`).
const DATA_SOURCE: &str = "Beatport";

pub struct BPSyncPlugin;

impl crate::plugins::Plugin for BPSyncPlugin {
    fn name(&self) -> &'static str {
        "bpsync"
    }
}

pub fn load(_cfg: &Config) -> BPSyncPlugin {
    let log = PluginLog::new("bpsync");
    log.warning("The 'bpsync' plugin is deprecated and will be removed in version 3.0.0.");
    let beatport = PluginLog::new("beatport");
    beatport.warning("The 'beatport' plugin is deprecated and will be removed in version 3.0.0.");
    BPSyncPlugin
}

/// The command's options (beets' `BPSyncCLIOpts`).
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub pretend: bool,
    /// -m / -M; None: `import.move` or `import.copy` (beets' `should_move`).
    pub mv: Option<bool>,
    /// -W: don't write (`import.write` otherwise; beets' `should_write`).
    pub nowrite: bool,
    /// -f: `format_item` and `format_album` for this run.
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// Where the metadata comes from: the `beatport` source (a fake one in
/// tests).
pub trait Source {
    fn album_for_id(&self, id: &str) -> Result<Option<AlbumInfo>>;
    fn track_for_id(&self, id: &str) -> Result<Option<TrackInfo>>;
}

/// The real source: beets' `BeatportPlugin` instance with its client.
pub struct BeatportSource<'a> {
    pub beatport: &'a Beatport,
    pub cfg: &'a Config,
    mb: MusicBrainz,
}

impl<'a> BeatportSource<'a> {
    pub fn new(cfg: &'a Config, beatport: &'a Beatport) -> Self {
        BeatportSource {
            beatport,
            cfg,
            mb: MusicBrainz::new(cfg),
        }
    }
}

impl Source for BeatportSource<'_> {
    fn album_for_id(&self, id: &str) -> Result<Option<AlbumInfo>> {
        let ctx = SourceCtx {
            cfg: self.cfg,
            mb: &self.mb,
        };
        self.beatport.album_for_id(&ctx, id)
    }

    fn track_for_id(&self, id: &str) -> Result<Option<TrackInfo>> {
        let ctx = SourceCtx {
            cfg: self.cfg,
            mb: &self.mb,
        };
        self.beatport.track_for_id(&ctx, id)
    }
}

/// `beet bpsync`.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let mut cfg = cfg.clone();
    if let Some(f) = &opts.format {
        // `add_format_option()` without a target: both formats.
        cfg.set("format_item", Y::String(f.clone()));
        cfg.set("format_album", Y::String(f.clone()));
    }
    // beets' `BPSyncPlugin.__init__` builds its own `BeatportPlugin()`.
    let beatport = Beatport::from_config(&cfg)?;
    let source = BeatportSource::new(&cfg, &beatport);
    Sync::new(&cfg, lib, opts, &source).run(opts)
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
            log: PluginLog::new("bpsync"),
            mv: opts
                .mv
                .unwrap_or_else(|| cfg.bool("import.move") || cfg.bool("import.copy")),
            write: if opts.nowrite {
                false
            } else {
                cfg.bool("import.write")
            },
            pretend: opts.pretend,
            fmt: PathFormatter::new(cfg).expect("path formats were checked at startup"),
        }
    }

    /// `BPSyncPlugin.func`: singletons, then albums.
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

    /// A record's `data_source` (a flexible attribute).
    fn data_source(rec: &Record) -> String {
        rec.flex.get("data_source").cloned().unwrap_or_default()
    }

    /// `str.isnumeric()` on a Beatport id.
    fn is_numeric(s: &str) -> bool {
        !s.is_empty() && s.chars().all(char::is_numeric)
    }

    /// `BPSyncPlugin.is_beatport_track`.
    fn is_beatport_track(item: &Item) -> bool {
        Self::data_source(item) == DATA_SOURCE
            && Self::is_numeric(item.str("mb_trackid"))
    }

    /// `BPSyncPlugin.singletons`.
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
            if !Self::is_beatport_track(&item) {
                self.log.info(&format!(
                    "Skipping non-{DATA_SOURCE} singleton: {}",
                    self.show(&item)
                ));
                continue;
            }
            let Some(info) = self.source.track_for_id(&track_id)? else {
                continue;
            };
            // `TrackMatch(Distance(), trackinfo, item).apply_metadata(
            // from_scratch=False)`.
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

    /// `BPSyncPlugin.get_album_tracks`: the album's items, or None with
    /// beets' log message when it isn't a Beatport release.
    fn get_album_tracks(&self, album: &crate::model::Album) -> Result<Option<Vec<Item>>> {
        let album_id = album.str("mb_albumid").to_string();
        if album_id.is_empty() {
            self.log.info(&format!(
                "Skipping album with no mb_albumid: {}",
                self.show(album)
            ));
            return Ok(None);
        }
        if !Self::is_numeric(&album_id) {
            self.log.info(&format!(
                "Skipping album with invalid {DATA_SOURCE} ID: {}",
                self.show(album)
            ));
            return Ok(None);
        }
        // `album.items()`, in the default item order.
        let q = vec![format!("album_id:{}", album.id.unwrap_or(0))];
        let items = self.lib.items(&Query::parse(Kind::Item, &q)?)?;
        if Self::data_source(album) == DATA_SOURCE {
            return Ok(Some(items));
        }
        if !items.iter().all(Self::is_beatport_track) {
            self.log.info(&format!(
                "Skipping non-{DATA_SOURCE} release: {}",
                self.show(album)
            ));
            return Ok(None);
        }
        Ok(Some(items))
    }

    /// `BPSyncPlugin.albums`.
    fn albums(&self, query: &[String], limit: Option<usize>) -> Result<()> {
        let mut albums = self.lib.albums(&Query::parse(Kind::Album, query)?)?;
        albums.truncate(limit.unwrap_or(usize::MAX));
        for album in albums {
            let Some(items) = self.get_album_tracks(&album)? else {
                continue;
            };
            if items.is_empty() {
                continue;
            }
            let album_id = album.str("mb_albumid").to_string();
            let Some(info) = self.source.album_for_id(&album_id)? else {
                self.log.info(&format!(
                    "Release ID {album_id} not found for album {}",
                    self.show(&album)
                ));
                continue;
            };
            // beets' `{track.track_id: track}` and `{item.mb_trackid: item}`
            // dicts: the last item with a track id wins, at its first
            // position.
            let mut track_index: HashMap<&str, usize> = HashMap::new();
            for (t, track) in info.tracks.iter().enumerate() {
                track_index.insert(&track.track_id, t);
            }
            let mut order: Vec<&str> = vec![];
            let mut item_index: HashMap<&str, usize> = HashMap::new();
            for (i, item) in items.iter().enumerate() {
                let id = item.str("mb_trackid");
                if !item_index.contains_key(id) {
                    order.push(id);
                }
                item_index.insert(id, i);
            }
            let pairs: Vec<(usize, usize)> = order
                .iter()
                .filter_map(|id| {
                    Some((*item_index.get(id)?, *track_index.get(id)?))
                })
                .collect();

            self.log
                .info(&format!("applying changes to {}", self.show(&album)));
            let mut items = items;
            self.lib.transaction(|lib| {
                // `AlbumMatch(Distance(), albuminfo, dict(item_info_pairs))
                // .apply_metadata(from_scratch=False)`.
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
                if self.pretend || !changed {
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
        .filter(|n| *n != "artpath" && *n != "id")
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct BpsyncArgs {
    /// show all changes but do nothing
    #[arg(short = 'p', long)]
    pretend: bool,
    /// move files in the library directory
    #[arg(short = 'm', long = "move", overrides_with = "nomove")]
    mv: bool,
    /// don't move files in library
    #[arg(short = 'M', long = "nomove", overrides_with = "mv")]
    nomove: bool,
    /// don't write updated metadata to files
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

pub fn build_bpsync() -> clap::Command {
    use clap::Args as _;
    BpsyncArgs::augment_args(clap::Command::new("bpsync").about("update metadata from Beatport"))
}

pub fn run_bpsync(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = BpsyncArgs::from_arg_matches(m)?;
    command(
        cfg,
        lib,
        &CmdOpts {
            pretend: a.pretend,
            mv: if a.mv {
                Some(true)
            } else if a.nomove {
                Some(false)
            } else {
                None
            },
            nowrite: a.nowrite,
            format: a.format,
            limit: a.limit,
            query: a.query,
        },
    )
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "bpsync",
        order: 67,
        // `BPSyncPlugin.__init__` builds a `BeatportPlugin()`, whose
        // settings it adds (beets shows the `beatport:` section with
        // bpsync enabled too).
        defaults: crate::plugins::beatport::DEFAULT_CONFIG,
        config_keys: &["beatport"],
        item_types: &[],
        album_types: &[],
        redacted: &["beatport.apikey", "beatport.apisecret"],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "bpsync",
            build: build_bpsync,
            run: run_bpsync,
        }],
    }
}

fn registry_make(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

#[cfg(test)]
#[path = "bpsync_tests.rs"]
mod tests;
