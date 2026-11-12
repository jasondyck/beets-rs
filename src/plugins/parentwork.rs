//! A port of beets 2.14's `parentwork` plugin (`beetsplug/parentwork.py`):
//! `beet parentwork` walks a recording's work up its "parts" relations to
//! the topmost (parent) work and tags the item with it, its composers and
//! the composition date.

use crate::config::Config;
use crate::library::Library;
use crate::mb::MusicBrainz;
use crate::model::{Item, Kind, Value};
use crate::plugins::{ImportTask, PluginCtx, PluginLog};
use crate::query::Query;
use anyhow::Result;
use serde_json::Value as J;
use std::collections::BTreeMap;

/// The fields `find_work` may change (beets' `show_model_changes` list).
const FIELDS: &[&str] = &[
    "parentwork",
    "parentwork_disambig",
    "mb_parentworkid",
    "parent_composer",
    "parent_composer_sort",
    "work_date",
    "parentwork_workid_current",
    "parentwork_date",
];

pub struct ParentWorkPlugin {
    auto: bool,
    force: bool,
    api: MusicBrainz,
    log: PluginLog,
}

impl ParentWorkPlugin {
    pub fn new(cfg: &Config) -> Self {
        ParentWorkPlugin {
            auto: cfg.bool("parentwork.auto"),
            force: cfg.bool("parentwork.force"),
            api: MusicBrainz::new(cfg),
            log: PluginLog::new("parentwork"),
        }
    }

    /// beets' `imported`: fetch parent works for the task's items.
    fn imported(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        for item in task.items.iter_mut() {
            self.find_work(ctx.cfg, ctx.lib, item, self.force, false)?;
            ctx.lib.store(item)?;
        }
        Ok(())
    }

    /// beets' `get_info`: the work's parent tags.
    fn get_info(&self, cfg: &Config, item: &Item, work: &J) -> (BTreeMap<String, String>, bool) {
        let mut info = BTreeMap::new();
        let mut composers: Vec<String> = vec![];
        let mut composer_sorts: Vec<String> = vec![];
        let mut composer_exists = false;
        for artist in array(work, "artist-relations") {
            if string(artist, "type") == "composer" {
                composer_exists = true;
                let a = artist.get("artist").unwrap_or(&J::Null);
                composers.push(string(a, "name"));
                composer_sorts.push(string(a, "sort-name"));
                if let Some(end) = artist.get("end").and_then(|v| v.as_str()) {
                    info.insert("parentwork_date".into(), end.to_string());
                }
            }
            info.insert("parent_composer".into(), composers.join(", "));
            info.insert("parent_composer_sort".into(), composer_sorts.join(", "));
        }
        if !composer_exists {
            self.log.debug(&format!(
                "no composer for {}; add one at https://musicbrainz.org/work/{}",
                self.show(cfg, item),
                string(work, "id")
            ));
        }
        info.insert("parentwork".into(), string(work, "title"));
        info.insert("mb_parentworkid".into(), string(work, "id"));
        if let Some(d) = work.get("disambiguation").and_then(|v| v.as_str()) {
            info.insert("parentwork_disambig".into(), d.to_string());
        }
        (info, composer_exists)
    }

    /// `find_parentwork_info`: the topmost work and the earliest composer
    /// end date along the chain.
    fn find_parentwork_info(&self, mb_workid: &str) -> Result<(J, Option<String>)> {
        let mut work_date: Option<String> = None;
        let mut parent_id = Some(mb_workid.to_string());
        let mut work_info = J::Null;
        while let Some(current) = parent_id {
            work_info = self.api.get_work(&current, &["work-rels", "artist-rels"])?;
            if work_date.is_none() {
                work_date = array(&work_info, "artist-relations")
                    .iter()
                    .find(|a| string(a, "type") == "composer")
                    .and_then(|a| a.get("end"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            parent_id = array(&work_info, "work-relations")
                .iter()
                .find(|w| string(w, "type") == "parts" && string(w, "direction") == "backward")
                .and_then(|w| w.get("work"))
                .and_then(|w| w.get("id"))
                .and_then(|v| v.as_str())
                .map(String::from);
        }
        Ok((work_info, work_date))
    }

    /// beets' `find_work`: find the parent work and set the tags.
    pub fn find_work(
        &self,
        cfg: &Config,
        lib: &Library,
        item: &mut Item,
        force: bool,
        verbose: bool,
    ) -> Result<bool> {
        let mb_workid = item.str("mb_workid").to_string();
        if mb_workid.is_empty() {
            self.log.info(&format!(
                "No work for {}, add one at https://musicbrainz.org/recording/{}",
                self.show(cfg, item),
                item.str("mb_trackid")
            ));
            return Ok(false);
        }
        let old = item.clone();
        let hasparent = item.has_field("parentwork");
        let work_changed = match item.get("parentwork_workid_current") {
            Some(v) => v.raw() != mb_workid,
            None => true,
        };
        let mut work_date: Option<String> = None;
        if force || !hasparent || work_changed {
            let (work_info, date) = match self.find_parentwork_info(&mb_workid) {
                Ok(r) => r,
                Err(e) => {
                    self.log.debug(&format!("error fetching work: {e:#}"));
                    return Ok(false);
                }
            };
            work_date = date;
            let (info, composer_exists) = self.get_info(cfg, item, &work_info);
            let parentwork = info.get("parentwork").cloned().unwrap_or_default();
            if composer_exists {
                self.log.debug(&format!(
                    "Work fetched: {} - {}",
                    parentwork,
                    info.get("parent_composer").cloned().unwrap_or_default()
                ));
            } else {
                self.log.debug(&format!(
                    "Work fetched: {} - no parent composer",
                    parentwork
                ));
            }
            for (k, v) in info {
                if !v.is_empty() {
                    item.set(&k, Value::Str(v));
                }
            }
            item.set(
                "parentwork_workid_current",
                Value::Str(mb_workid.to_string()),
            );
        } else if hasparent {
            self.log
                .debug(&format!("{}: Work present, skipping", self.show(cfg, item)));
            return Ok(false);
        }
        if let Some(d) = work_date.filter(|d| !d.is_empty()) {
            item.set("work_date", Value::Str(d));
        }
        if verbose {
            let fields: Vec<String> = FIELDS.iter().map(|f| f.to_string()).collect();
            let changed = FIELDS.iter().any(|f| old.get(f) != item.get(f));
            if changed {
                let fmt = crate::template::Template::parse(&cfg.str("format_item"));
                println!("{}", crate::commands::format_record(&fmt, &old, Some(lib)));
                crate::commands::print_model_changes(&old, item, &fields);
            }
            return Ok(changed);
        }
        Ok(false)
    }

    /// `format(item)` with `format_item`.
    fn show(&self, cfg: &Config, item: &Item) -> String {
        let fmt = crate::template::Template::parse(&cfg.str("format_item"));
        crate::commands::format_record(&fmt, item, None)
    }
}

impl crate::plugins::Plugin for ParentWorkPlugin {
    fn name(&self) -> &'static str {
        "parentwork"
    }

    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if self.auto {
            self.imported(ctx, task)?;
        }
        Ok(())
    }
}

/// `beet parentwork`'s options.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    pub force: Option<bool>,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// `beet parentwork`.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let plugin = ParentWorkPlugin::new(cfg);
    let force = opts.force.unwrap_or(plugin.force);
    let write = cfg.bool("import.write");
    let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
    items.truncate(opts.limit.unwrap_or(usize::MAX));
    for mut item in items {
        let changed = plugin.find_work(cfg, lib, &mut item, force, true)?;
        if changed {
            lib.store(&item)?;
            let mut item = item;
            if write && let Err(e) = crate::media::write_item(&mut item) {
                crate::logging::emit(format!("{e:#}"));
            }
        }
    }
    Ok(())
}

fn string(v: &J, key: &str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn array<'a>(v: &'a J, key: &str) -> &'a [J] {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "parentwork_tests.rs"]
mod tests;
