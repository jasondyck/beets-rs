//! A port of beets 2.14's `acousticbrainz` plugin
//! (`beetsplug/acousticbrainz.py`): fetch AcousticBrainz metadata for an
//! item's MusicBrainz recording id (`musicbrainz`'s `/low-level` and
//! `/high-level` documents, merged) and store it in the flexible attributes
//! beets knows.

use crate::config::Config;
use crate::library::Library;
use crate::model::{FieldType, Item, Value};
use crate::plugins::requests::{Request as HttpRequest, Session};
use crate::plugins::{ImportTask, Plugin, PluginCtx, PluginLog};
use crate::query::Query;
use crate::template::Template;
use anyhow::{Result, bail};
use serde_json::Value as J;
use std::collections::BTreeMap;

/// beets' `LEVELS`.
const LEVELS: [&str; 2] = ["/low-level", "/high-level"];

/// One leaf of beets' `ABSCHEME`.
#[derive(Clone, Copy)]
pub enum Node {
    /// A leaf mapped to this item attribute.
    Field(&'static str),
    /// A leaf that is part of a composite attribute (part number).
    Composite(&'static str, usize),
    /// A nested mapping (`(key, Node)` children).
    Group(&'static [(&'static str, Node)]),
}

use Node::{Composite, Field, Group};

/// beets' `ABSCHEME`, as code (Python's nested dicts and composite tuples).
pub fn scheme() -> &'static [(&'static str, Node)] {
    static HIGHLEVEL: &[(&str, Node)] = &[
        (
            "danceability",
            Group(&[("all", Group(&[("danceable", Field("danceable"))]))]),
        ),
        ("gender", Group(&[("value", Field("gender"))])),
        (
            "genre_rosamerica",
            Group(&[("value", Field("genre_rosamerica"))]),
        ),
        (
            "mood_acoustic",
            Group(&[("all", Group(&[("acoustic", Field("mood_acoustic"))]))]),
        ),
        (
            "mood_aggressive",
            Group(&[(
                "all",
                Group(&[("aggressive", Field("mood_aggressive"))]),
            )]),
        ),
        (
            "mood_electronic",
            Group(&[(
                "all",
                Group(&[("electronic", Field("mood_electronic"))]),
            )]),
        ),
        (
            "mood_happy",
            Group(&[("all", Group(&[("happy", Field("mood_happy"))]))]),
        ),
        (
            "mood_party",
            Group(&[("all", Group(&[("party", Field("mood_party"))]))]),
        ),
        (
            "mood_relaxed",
            Group(&[("all", Group(&[("relaxed", Field("mood_relaxed"))]))]),
        ),
        (
            "mood_sad",
            Group(&[("all", Group(&[("sad", Field("mood_sad"))]))]),
        ),
        ("moods_mirex", Group(&[("value", Field("moods_mirex"))])),
        (
            "ismir04_rhythm",
            Group(&[("value", Field("rhythm"))]),
        ),
        (
            "tonal_atonal",
            Group(&[("all", Group(&[("tonal", Field("tonal"))]))]),
        ),
        ("timbre", Group(&[("value", Field("timbre"))])),
        (
            "voice_instrumental",
            Group(&[("value", Field("voice_instrumental"))]),
        ),
    ];
    static LOWLEVEL: &[(&str, Node)] = &[("average_loudness", Field("average_loudness"))];
    static RHYTHM: &[(&str, Node)] = &[("bpm", Field("bpm"))];
    static TONAL: &[(&str, Node)] = &[
        ("chords_changes_rate", Field("chords_changes_rate")),
        ("chords_key", Field("chords_key")),
        ("chords_number_rate", Field("chords_number_rate")),
        ("chords_scale", Field("chords_scale")),
        ("key_key", Composite("initial_key", 0)),
        ("key_scale", Composite("initial_key", 1)),
        ("key_strength", Field("key_strength")),
    ];
    static TOP: &[(&str, Node)] = &[
        ("highlevel", Group(HIGHLEVEL)),
        ("lowlevel", Group(LOWLEVEL)),
        ("rhythm", Group(RHYTHM)),
        ("tonal", Group(TONAL)),
    ];
    TOP
}

/// beets' `_map_data_to_scheme`: the `(attribute, value)` leaves of `data`
/// that `scheme` maps, composite attributes joined with a space.
pub fn map_data_to_scheme(
    data: &J,
    scheme: &[(&'static str, Node)],
    warn: &mut dyn FnMut(&str),
) -> Vec<(String, J)> {
    let mut out = vec![];
    let mut composites: BTreeMap<String, Vec<String>> = BTreeMap::new();
    data_to_scheme_child(data, scheme, &mut composites, &mut out, warn);
    for (attr, parts) in composites {
        out.push((attr, J::String(parts.join(" "))));
    }
    out
}

fn data_to_scheme_child(
    subdata: &J,
    subscheme: &[(&'static str, Node)],
    composites: &mut BTreeMap<String, Vec<String>>,
    out: &mut Vec<(String, J)>,
    warn: &mut dyn FnMut(&str),
) {
    for (k, v) in subscheme {
        let Some(child) = subdata.get(k) else {
            warn(k);
            continue;
        };
        match v {
            Group(children) => data_to_scheme_child(child, children, composites, out, warn),
            Composite(attr, part) => {
                let text = match child {
                    J::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let parts = composites.entry((*attr).to_string()).or_default();
                while parts.len() <= *part {
                    parts.push(String::new());
                }
                parts[*part] = text;
            }
            Field(attr) => out.push(((*attr).to_string(), child.clone())),
        }
    }
}

/// beets' `_generate_urls`.
pub fn generate_urls(base_url: &str, mbid: &str) -> Vec<String> {
    LEVELS
        .iter()
        .map(|level| format!("{base_url}{mbid}{level}"))
        .collect()
}

pub struct AcousticPlugin {
    base_url: String,
    auto: bool,
    force: bool,
    tags: Vec<String>,
    log: PluginLog,
    session: Session,
}

pub fn load(cfg: &Config) -> Result<AcousticPlugin> {
    let base_url = cfg.str("acousticbrainz.base_url");
    if !base_url.is_empty() && !base_url.starts_with("http") {
        bail!("AcousticBrainz server base URL must start with an HTTP scheme");
    }
    let base_url = if !base_url.is_empty() && !base_url.ends_with('/') {
        format!("{base_url}/")
    } else {
        base_url
    };
    let log = PluginLog::new("acousticbrainz");
    log.warning("This plugin is deprecated.");
    Ok(AcousticPlugin {
        base_url,
        auto: cfg.bool("acousticbrainz.auto"),
        force: cfg.bool("acousticbrainz.force"),
        tags: cfg.list("acousticbrainz.tags"),
        log,
        session: Session::plain("beets/2.14.1"),
    })
}

impl AcousticPlugin {
    /// `_get_data`: the merged JSON of the two levels, or `{}`.
    fn get_data(&self, mbid: &str) -> J {
        if self.base_url.is_empty() {
            // beets raises a UserError here; the listener can't, so log it.
            self.log.error(
                "This plugin is deprecated since AcousticBrainz has shut down. \
                 See the base_url configuration option.",
            );
            return J::Object(Default::default());
        }
        let mut data = serde_json::Map::new();
        for url in generate_urls(&self.base_url, mbid) {
            self.log.debug(&format!("fetching URL: {url}"));
            let resp = match self.session.get(&url, HttpRequest::default()) {
                Ok(r) => r,
                Err(e) => {
                    self.log.info(&format!("request error: {e}"));
                    return J::Object(Default::default());
                }
            };
            if resp.status == 404 {
                self.log.info(&format!("recording ID {mbid} not found"));
                return J::Object(Default::default());
            }
            match resp.json() {
                Ok(J::Object(m)) => data.extend(m),
                Ok(_) => {}
                Err(_) => {
                    self.log
                        .debug(&format!("Invalid Response: {}", resp.text(true)));
                    return J::Object(Default::default());
                }
            }
        }
        J::Object(data)
    }

    /// `_fetch_info`.
    fn fetch_info(&self, cfg: &Config, lib: &Library, items: &[Item], write: bool, force: bool) {
        let force = force || self.force;
        for item in items {
            if !force && !item.str("mood_acoustic").is_empty() {
                self.log.info(&format!("data already present for: {}", Self::show(cfg, lib, item)));
                continue;
            }
            if item.str("mb_trackid").is_empty() {
                continue;
            }
            self.log
                .info(&format!("getting data for: {}", Self::show(cfg, lib, item)));
            let data = self.get_data(item.str("mb_trackid"));
            if data.as_object().is_none_or(serde_json::Map::is_empty) {
                continue;
            }
            let mut warn = |k: &str| {
                self.log
                    .warning(&format!("Acousticbrainz did not provide info about {k}"));
            };
            let mut item = item.clone();
            for (attr, value) in map_data_to_scheme(&data, scheme(), &mut warn) {
                if !self.tags.is_empty() && !self.tags.contains(&attr) {
                    self.log.debug(&format!(
                        "skipping attribute {attr} of {} (value {}) due to config",
                        Self::show(cfg, lib, &item),
                        value
                    ));
                    continue;
                }
                self.log.debug(&format!(
                    "attribute {attr} of {} set to {}",
                    Self::show(cfg, lib, &item),
                    value
                ));
                let v = match &value {
                    J::Number(n) => Value::Float(n.as_f64().unwrap_or(0.0)),
                    J::String(s) => Value::Str(s.clone()),
                    J::Bool(b) => Value::Bool(*b),
                    other => Value::Str(other.to_string()),
                };
                item.set(&attr, v);
            }
            let _ = lib.store(&item);
            if write {
                let _ = crate::media::write_item(&mut item);
            }
        }
    }

    /// beets' `{}` of an Item: the `format_item` template.
    fn show(cfg: &Config, lib: &Library, item: &Item) -> String {
        crate::commands::format_record(&Template::parse(&cfg.str("format_item")), item, Some(lib))
    }
}

impl Plugin for AcousticPlugin {
    fn name(&self) -> &'static str {
        "acousticbrainz"
    }

    /// `import_task_files` (with `auto`).
    fn import_task_files(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if !self.auto {
            return Ok(());
        }
        self.fetch_info(ctx.cfg, ctx.lib, task.items, false, true);
        Ok(())
    }
}

/// Item attribute types (beets' `AcousticPlugin.item_types`).
pub const ITEM_TYPES: &[(&str, FieldType)] = &[
    ("average_loudness", FieldType::Float(6)),
    ("chords_changes_rate", FieldType::Float(6)),
    ("chords_key", FieldType::Str),
    ("chords_number_rate", FieldType::Float(6)),
    ("chords_scale", FieldType::Str),
    ("danceable", FieldType::Float(6)),
    ("gender", FieldType::Str),
    ("genre_rosamerica", FieldType::Str),
    ("key_strength", FieldType::Float(6)),
    ("mood_acoustic", FieldType::Float(6)),
    ("mood_aggressive", FieldType::Float(6)),
    ("mood_electronic", FieldType::Float(6)),
    ("mood_happy", FieldType::Float(6)),
    ("mood_party", FieldType::Float(6)),
    ("mood_relaxed", FieldType::Float(6)),
    ("mood_sad", FieldType::Float(6)),
    ("moods_mirex", FieldType::Str),
    ("rhythm", FieldType::Float(6)),
    ("timbre", FieldType::Str),
    ("tonal", FieldType::Float(6)),
    ("voice_instrumental", FieldType::Str),
];

/// The plugin instance is the fetcher; `load` returns it, and the command
/// creates a fresh one (as beets' `commands` does).
pub fn command(
    cfg: &Config,
    lib: &Library,
    force: bool,
    limit: Option<usize>,
    query: &[String],
) -> Result<()> {
    let plugin = load(cfg)?;
    let mut items = lib.items(&Query::parse(crate::model::Kind::Item, query)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    plugin.fetch_info(cfg, lib, &items, cfg.bool("import.write"), force);
    Ok(())
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct AcousticbrainzArgs {
    /// re-download data when already present
    #[arg(short = 'f', long)]
    force: bool,
    /// Limit query results
    #[arg(long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_acousticbrainz() -> clap::Command {
    use clap::Args as _;
    AcousticbrainzArgs::augment_args(
        clap::Command::new("acousticbrainz").about("fetch metadata from AcousticBrainz"),
    )
}

pub fn run_acousticbrainz(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = AcousticbrainzArgs::from_arg_matches(m)?;
    command(cfg, lib, a.force, a.limit, &a.query)
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "acousticbrainz",
        order: 64,
        defaults: r#"acousticbrainz:
    auto: yes
    force: false
    tags: []
    base_url: ''
"#,
        config_keys: &["acousticbrainz"],
        item_types: crate::plugins::acousticbrainz::ITEM_TYPES,
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "acousticbrainz",
            build: build_acousticbrainz,
            run: run_acousticbrainz,
        }],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)?))
}

#[cfg(test)]
#[path = "acousticbrainz_tests.rs"]
mod tests;
