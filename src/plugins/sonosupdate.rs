//! A port of beets 2.14's `sonosupdate` plugin (`beetsplug/sonosupdate.py`):
//! after a command that changed the library, ask a Sonos controller to
//! update its music library. The Sonos client is the minimal soco port in
//! [`super::sonos`] (SSDP discovery and the ContentDirectory refresh).

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::sonos::{self, Device};
use crate::plugins::{Plugin, PluginLog};

pub struct SonosUpdate;

impl Plugin for SonosUpdate {
    fn name(&self) -> &'static str {
        "sonosupdate"
    }
}

struct State {
    /// The library changed during this command.
    pending: std::sync::atomic::AtomicBool,
    log: PluginLog,
    /// beets' `soco.discovery.any_soco()`, injectable for tests.
    discover: Box<dyn Fn() -> Option<Device> + Send + Sync>,
}

pub fn load(_cfg: &Config) -> SonosUpdate {
    let log = PluginLog::new("sonosupdate");
    // Event listeners log at WARNING, as beets' `register_listener` does.
    log.set_base_level(crate::plugins::WARNING);
    crate::events::register(std::sync::Arc::new(State {
        pending: std::sync::atomic::AtomicBool::new(false),
        log,
        discover: Box::new(sonos::any_soco),
    }));
    SonosUpdate
}

impl State {
    /// beets' `update`: `soco.discovery.any_soco()` then
    /// `music_library.start_library_update()`.
    fn update(&self) {
        self.log.info("Requesting a Sonos library update...");

        let Some(device) = (self.discover)() else {
            self.log.warning("Could not find a Sonos device.");
            return;
        };

        // beets would end the command with a traceback here; report it
        // and carry on instead.
        if let Err(e) = device.start_library_update() {
            self.log.warning(&format!("Sonos update failed: {e:#}"));
            return;
        }

        self.log.info("Sonos update triggered");
    }
}

impl Listener for State {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "database_change" | "cli_exit")
    }

    fn on(&self, ev: &Event) {
        match ev.name {
            "database_change" => self.pending.store(true, std::sync::atomic::Ordering::Relaxed),
            "cli_exit" if self.pending.swap(false, std::sync::atomic::Ordering::Relaxed) => self.update(),
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
        item_types: &[],
        album_types: &[],
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
