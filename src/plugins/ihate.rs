//! A port of beets 2.14's `ihate` plugin (`beetsplug/ihate.py`): when a
//! match is applied (`import_task_choice`), a task whose items match one
//! of the `skip` queries is skipped instead, and one matching a `warn`
//! query is logged.

use crate::config::Config;
use crate::model::{Item, Kind};
use crate::plugins::{ChoiceInfo, PluginCtx, PluginLog};
use crate::query::Query;

pub struct IHatePlugin {
    log: PluginLog,
}

pub fn load(_cfg: &Config) -> IHatePlugin {
    let log = PluginLog::new("ihate");
    log.set_base_level(crate::plugins::WARNING);
    IHatePlugin { log }
}

/// `do_i_hate_this`: any of the queries (parsed for albums or items, as
/// the task is) matches any of the items.
pub fn do_i_hate_this(items: &[Item], album: bool, patterns: &[String]) -> bool {
    let kind = if album { Kind::Album } else { Kind::Item };
    patterns.iter().any(|p| {
        let parts = crate::plugins::duplicates::shlex_split(p).unwrap_or_default();
        match Query::parse(kind, &parts) {
            Ok(q) => items
                .iter()
                .any(|i| q.matches_obj(&crate::query::Obj::new(i))),
            Err(_) => false,
        }
    })
}

impl crate::plugins::Plugin for IHatePlugin {
    fn name(&self) -> &'static str {
        "ihate"
    }

    fn import_task_choice_skip(&self, ctx: &PluginCtx, task: &ChoiceInfo) -> bool {
        let skip = ctx.cfg.list("ihate.skip");
        let warn = ctx.cfg.list("ihate.warn");
        if !task.apply {
            self.log.debug("user made a decision, nothing to do");
            return false;
        }
        if skip.is_empty() && warn.is_empty() {
            self.log.debug("nothing to do");
            return false;
        }
        self.log.debug("processing your hate");
        if do_i_hate_this(task.imported_items, task.album, &skip) {
            self.log.info(&format!("skipped: {}", task.desc));
            return true;
        }
        if do_i_hate_this(task.imported_items, task.album, &warn) {
            self.log.info(&format!("you may hate this: {}", task.desc));
        }
        false
    }
}

#[cfg(test)]
#[path = "ihate_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "ihate",
        order: 9,
        defaults: r#"ihate:
    warn: []
    skip: []
"#,
        config_keys: &["ihate"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}
