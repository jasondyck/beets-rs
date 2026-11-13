//! A port of beets 2.14's `sonosupdate` plugin (`beetsplug/sonosupdate.py`):
//! after a command that changed the library, ask a Sonos controller to
//! update its library. The Sonos discovery (soco's SSDP search) isn't
//! ported: with no device found, beets logs "Could not find a Sonos device."
//! and beets-rs always does (there is no Sonos client in this tree).

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::library::Library;
use crate::plugins::{Plugin, PluginLog};
use std::cell::Cell;
use std::rc::Rc;

pub struct SonosUpdate;

impl Plugin for SonosUpdate {
    fn name(&self) -> &'static str {
        "sonosupdate"
    }
}

struct State {
    /// The library changed during this command.
    pending: Cell<bool>,
    log: PluginLog,
}

pub fn load(_cfg: &Config) -> SonosUpdate {
    let log = PluginLog::new("sonosupdate");
    // Event listeners log at WARNING, as beets' `register_listener` does.
    log.set_base_level(crate::plugins::WARNING);
    crate::events::register(Rc::new(State {
        pending: Cell::new(false),
        log,
    }));
    SonosUpdate
}

impl State {
    /// beets' `update`: `soco.discovery.any_soco()` then
    /// `music_library.start_library_update()`.
    fn update(&self) {
        self.log.info("Requesting a Sonos library update...");
        // No Sonos client: as beets when discovery finds no device.
        self.log.warning("Could not find a Sonos device.");
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "database_change" | "cli_exit")
    }

    fn on(&self, ev: &Event) {
        match ev.name {
            "database_change" => self.pending.set(true),
            "cli_exit" if self.pending.replace(false) => self.update(),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "sonosupdate",
        order: 61,
        defaults: r#""#,
        config_keys: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

#[cfg(test)]
#[path = "sonosupdate_tests.rs"]
mod tests;

/// The `Library` argument of the listener (kept for the signature beets
/// has; the update itself doesn't use it).
#[allow(dead_code)]
fn _library(_: &Library) {}
