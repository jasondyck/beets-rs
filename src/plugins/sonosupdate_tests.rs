//! Tests for the Sonos update listener (beets' plugin has no tests; soco's
//! discovery needs a Sonos device).

use super::*;
use crate::config::Config;

#[test]
fn a_library_change_asks_for_an_update_on_exit() {
    let (_out, lines) = crate::logging::capture(|| {
        crate::events::register(std::rc::Rc::new(State {
            pending: std::cell::Cell::new(false),
            log: {
                let log = PluginLog::new("sonosupdate");
                log.set_base_level(crate::plugins::WARNING);
                log
            },
        }));
        crate::events::send("database_change", &[]);
        crate::events::send("cli_exit", &[]);
    });
    assert!(
        lines
            .iter()
            .any(|l| l == "sonosupdate: Could not find a Sonos device."),
        "{lines:?}"
    );
}

#[test]
fn no_change_means_no_update() {
    let (_out, lines) = crate::logging::capture(|| {
        crate::events::register(std::rc::Rc::new(State {
            pending: std::cell::Cell::new(false),
            log: {
                let log = PluginLog::new("sonosupdate");
                log.set_base_level(crate::plugins::WARNING);
                log
            },
        }));
        crate::events::send("cli_exit", &[]);
    });
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn load_registers_the_listener() {
    let cfg = Config::defaults();
    let _plugin = load(&cfg);
    assert!(crate::events::listening("database_change"));
    assert!(crate::events::listening("cli_exit"));
}
