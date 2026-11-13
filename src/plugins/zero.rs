//! Port of beets 2.14's `zero` plugin (`beetsplug/zero.py`): clears tag
//! fields in media files when they're written (the `write` event), and the
//! `beet zero` command.
//!
//! Config (`zero:`): `fields` (the fields to clear) or `keep_fields` (clear
//! every other MediaFile field), per-field regex lists (`zero: {comments:
//! [encoded by]}`: clear only matching values, without case), `auto`
//! (clear on every write), `update_database` (clear the library's values
//! too) and `omit_single_disc` (clear `disc`/`disctotal` of single-disc
//! albums).

use crate::config::Config;
use crate::library::Library;
use crate::media::{self, WriteListener, WriteTags};
use crate::model::Kind;
use crate::plugins::{INFO, Plugin, PluginLog, WARNING};
use crate::query::Query;
use anyhow::{Result, anyhow};
use fancy_regex::Regex;
use std::cell::Cell;
use std::sync::Arc;

/// `MediaFile.fields()`, in MediaFile's order.
pub const MEDIAFILE_FIELDS: &[&str] = &[
    "title",
    "artist",
    "artists",
    "album",
    "genres",
    "genre",
    "lyricists",
    "lyricist",
    "composers",
    "composer",
    "composer_sort",
    "arrangers",
    "arranger",
    "grouping",
    "subtitle",
    "track",
    "tracktotal",
    "disc",
    "disctotal",
    "url",
    "lyrics",
    "synced_lyrics",
    "comments",
    "copyright",
    "bpm",
    "comp",
    "albumartist",
    "albumartists",
    "albumtypes",
    "albumtype",
    "label",
    "artist_sort",
    "albumartist_sort",
    "asin",
    "catalognums",
    "catalognum",
    "barcode",
    "isrc",
    "disctitle",
    "encoder",
    "script",
    "languages",
    "language",
    "country",
    "albumstatus",
    "media",
    "albumdisambig",
    "date",
    "year",
    "month",
    "day",
    "original_date",
    "original_year",
    "original_month",
    "original_day",
    "artist_credit",
    "artists_credit",
    "artists_sort",
    "albumartist_credit",
    "albumartists_credit",
    "albumartists_sort",
    "remixers",
    "art",
    "images",
    "mb_trackid",
    "mb_releasetrackid",
    "mb_workid",
    "mb_albumid",
    "mb_artistids",
    "mb_artistid",
    "mb_albumartistids",
    "mb_albumartistid",
    "mb_releasegroupid",
    "acoustid_fingerprint",
    "acoustid_id",
    "rg_track_gain",
    "rg_album_gain",
    "rg_track_peak",
    "rg_album_peak",
    "r128_track_gain",
    "r128_album_gain",
    "initial_key",
];

const ARTWORK_FIELDS: &[&str] = &["images", "art"];

/// What the plugin clears (shared with the `write` listener).
#[derive(Debug)]
pub struct ZeroCore {
    auto: bool,
    update_database: bool,
    omit_single_disc: bool,
    /// The fields to clear, each with its patterns (none: always), in
    /// the order they were added.
    pub fields_to_progs: Vec<(String, Vec<Regex>)>,
}

pub struct Zero {
    pub core: Arc<ZeroCore>,
    log: PluginLog,
    warned: Cell<bool>,
}

/// A config value's truthiness, as confuse's `bool(view)`.
fn truthy(cfg: &Config, key: &str) -> bool {
    use serde_yaml::Value as Y;
    match cfg.get(key) {
        None | Some(Y::Null) => false,
        Some(Y::Bool(b)) => *b,
        Some(Y::String(s)) => !s.is_empty(),
        Some(Y::Sequence(v)) => !v.is_empty(),
        Some(Y::Mapping(m)) => !m.is_empty(),
        Some(Y::Number(n)) => n.as_f64() != Some(0.0),
        Some(_) => true,
    }
}

impl ZeroCore {
    /// Read the config (`ZeroPlugin.__init__`), logging what beets logs.
    pub fn new(cfg: &Config, log: &PluginLog) -> Result<Self> {
        let mut core = ZeroCore {
            auto: cfg.bool("zero.auto"),
            update_database: cfg.bool("zero.update_database"),
            omit_single_disc: cfg.bool("zero.omit_single_disc"),
            fields_to_progs: vec![],
        };
        if truthy(cfg, "zero.fields") && truthy(cfg, "zero.keep_fields") {
            log.warning("cannot blacklist and whitelist at the same time");
        } else if truthy(cfg, "zero.fields") {
            // Blacklist mode.
            for field in cfg.list("zero.fields") {
                core.set_pattern(cfg, log, &field)?;
            }
        } else if truthy(cfg, "zero.keep_fields") {
            // Whitelist mode: every other field. Keeping one artwork field
            // keeps both.
            let mut keep = cfg.list("zero.keep_fields");
            if keep.iter().any(|k| ARTWORK_FIELDS.contains(&k.as_str())) {
                keep.extend(ARTWORK_FIELDS.iter().map(std::string::ToString::to_string));
            }
            for field in MEDIAFILE_FIELDS {
                if !keep.iter().any(|k| k == field) && !["id", "path", "album_id"].contains(field) {
                    core.set_pattern(cfg, log, field)?;
                }
            }
        }
        Ok(core)
    }

    /// `_set_pattern`: a field to clear and its patterns (`zero.<field>`).
    fn set_pattern(&mut self, cfg: &Config, log: &PluginLog, field: &str) -> Result<()> {
        if !MEDIAFILE_FIELDS.contains(&field) {
            log.error(&format!("invalid field: {field}"));
            return Ok(());
        }
        if ["id", "path", "album_id"].contains(&field) {
            log.warning(&format!(
                "field '{field}' ignored, zeroing it would be dangerous"
            ));
            return Ok(());
        }
        let key = format!("zero.{field}");
        if cfg.get(&key).is_none() {
            // No patterns: matches everything.
            self.entry(field);
            return Ok(());
        }
        for pattern in cfg.list(&key) {
            let prog = Regex::new(&format!("(?i){pattern}"))
                .map_err(|e| anyhow!("invalid regular expression {pattern:?}: {e}"))?;
            self.entry(field).push(prog);
        }
        Ok(())
    }

    fn entry(&mut self, field: &str) -> &mut Vec<Regex> {
        let i = match self.fields_to_progs.iter().position(|(f, _)| f == field) {
            Some(i) => i,
            None => {
                self.fields_to_progs.push((field.to_string(), vec![]));
                self.fields_to_progs.len() - 1
            }
        };
        &mut self.fields_to_progs[i].1
    }

    /// `set_fields`: set the fields to clear to None in `tags` (and on the
    /// item with `update_database`). Whether any were.
    pub fn set_fields(&self, tags: &mut WriteTags, log: &PluginLog) -> bool {
        let mut fields_set = false;
        if self.omit_single_disc && tags.item().int("disctotal") == 1 {
            for tag in ["disc", "disctotal"] {
                if tags.contains(tag) {
                    tags.set_null(tag);
                    fields_set = true;
                }
            }
        }
        if self.fields_to_progs.is_empty() {
            log.warning("no fields list to remove");
        }
        for (field, progs) in &self.fields_to_progs {
            let (value, matched) = if tags.contains(field) {
                let value = tags.value_str(field);
                let m = match_progs(&value, progs);
                (value, m)
            } else {
                (String::new(), progs.is_empty())
            };
            if matched {
                fields_set = true;
                log.debug(&format!("{field}: {value} -> None"));
                tags.set_null(field);
                if self.update_database {
                    tags.set_item_null(field);
                }
            }
        }
        fields_set
    }
}

/// Whether `value` matches any of `progs` (all values do if there are
/// none).
fn match_progs(value: &str, progs: &[Regex]) -> bool {
    progs.is_empty() || progs.iter().any(|p| p.is_match(value).unwrap_or(false))
}

impl WriteListener for ZeroCore {
    fn write(&self, tags: &mut WriteTags) {
        if self.auto {
            let log = PluginLog::new("zero");
            log.set_base_level(WARNING);
            self.set_fields(tags, &log);
        }
    }
}

impl Zero {
    /// Load the plugin: read the config and listen to writes.
    pub fn new(cfg: &Config) -> Result<Self> {
        let log = PluginLog::new("zero");
        let core = Arc::new(ZeroCore::new(cfg, &log)?);
        media::add_write_listener(core.clone());
        Ok(Zero {
            core,
            log,
            warned: Cell::new(false),
        })
    }

    /// `process_item`: clear the item's fields in its file (and the
    /// library, with `update_database`).
    pub fn process_item(&self, lib: &Library, item: &mut crate::model::Item) -> Result<()> {
        let mut tags = WriteTags::for_item(item);
        if !self.core.set_fields(&mut tags, &self.log) {
            return Ok(());
        }
        let (nulls, item_nulls) = tags.into_parts();
        media::apply_item_nulls(item, &item_nulls);
        media::write_nulls(item, &nulls)?;
        if self.core.update_database {
            lib.store(item)?;
        }
        Ok(())
    }
}

impl Plugin for Zero {
    fn name(&self) -> &'static str {
        "zero"
    }

    fn import_task_choice(&self, asis: bool) {
        if asis && !self.warned.get() {
            self.log.at_level(WARNING, || {
                self.log.warning("cannot zero in \"as-is\" mode");
            });
            self.warned.set(true);
        }
    }
}

/// `beet zero [-l LIMIT] [QUERY]`.
pub fn command(
    cfg: &Config,
    lib: &Library,
    plugins: &[Box<dyn Plugin>],
    limit: Option<usize>,
    query: &[String],
) -> Result<()> {
    if !plugins.iter().any(|p| p.name() == "zero") {
        return Err(anyhow!("the zero plugin failed to load"));
    }
    // The loaded plugin's settings, again (its warnings were logged at load).
    let quiet = PluginLog::new("zero");
    quiet.set_base_level(u8::MAX);
    let zero = Zero {
        core: Arc::new(ZeroCore::new(cfg, &quiet)?),
        log: PluginLog::new("zero"),
        warned: Cell::new(false),
    };
    zero.log.set_base_level(INFO);
    if query.is_empty() && !crate::ui::input_yn("Remove fields for all items? (Y/n)", true) {
        return Ok(());
    }
    let mut items = lib.items(&Query::parse(Kind::Item, query)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    lib.transaction(|lib| {
        for item in &mut items {
            zero.process_item(lib, item)?;
        }
        Ok(())
    })
}

#[cfg(test)]
#[path = "zero/tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "zero",
        order: 24,
        defaults: r#"zero:
    auto: yes
    fields: []
    keep_fields: []
    update_database: no
    omit_single_disc: no
"#,
        config_keys: &["zero"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "zero",
                build: build_zero,
                run: run_zero,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Zero::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ZeroArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_zero() -> clap::Command {
    use clap::Args as _;
    ZeroArgs::augment_args(clap::Command::new("zero").about("set fields to null"))
}

pub fn run_zero(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ZeroArgs::from_arg_matches(m)?;
    crate::plugins::zero::command(cfg, lib, plugins, a.limit, &a.query)
}
