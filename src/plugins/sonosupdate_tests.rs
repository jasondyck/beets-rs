//! Tests for the Sonos update listener and the soco port it uses.

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SOAP_OK: &str = "<?xml version=\"1.0\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:RefreshShareIndexResponse \
xmlns:u=\"urn:schemas-upnp-org:service:ContentDirectory:1\">\
</u:RefreshShareIndexResponse></s:Body></s:Envelope>";

const SOAP_FAULT: &str = "<?xml version=\"1.0\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><s:Fault><faultcode>s:Client</faultcode>\
<faultstring>UPnPError</faultstring></s:Fault></s:Body></s:Envelope>";

fn port_of(base: &str) -> u16 {
    base.rsplit(':').next().unwrap().parse().unwrap()
}

/// A fake UPnP endpoint answering every request with `body`.
fn fake_soap(status: u16, body: &'static str) -> u16 {
    port_of(
        &serve(move |_| Reply {
            status,
            content_type: "text/xml",
            body: body.as_bytes().to_vec(),
        })
        .base,
    )
}

// ------------------------------------------------------------ the plugin

fn state(log: PluginLog, discover: Box<dyn Fn() -> Option<Device>>) -> State {
    State {
        pending: Cell::new(false),
        log,
        discover,
    }
}

fn warning_log() -> PluginLog {
    let log = PluginLog::new("sonosupdate");
    log.set_base_level(crate::plugins::WARNING);
    log
}

#[test]
fn a_library_change_asks_for_an_update_on_exit() {
    let port = fake_soap(200, SOAP_OK);
    let device = Device { ip: "127.0.0.1".into(), port };
    let ((), lines) = crate::logging::capture(|| {
        // With -v, beets shows the listener's INFO lines.
        let log = PluginLog::new("sonosupdate");
        crate::events::register(std::rc::Rc::new(state(
            log,
            Box::new(move || Some(device.clone())),
        )));
        crate::events::send("database_change", &[]);
        crate::events::send("cli_exit", &[]);
    });
    assert_eq!(
        lines,
        [
            "sonosupdate: Requesting a Sonos library update...",
            "sonosupdate: Sonos update triggered",
        ]
    );
}

#[test]
fn no_device_means_beets_warning() {
    let ((), lines) = crate::logging::capture(|| {
        crate::events::register(std::rc::Rc::new(state(
            warning_log(),
            Box::new(|| None),
        )));
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
fn a_failed_refresh_is_reported() {
    let port = fake_soap(500, SOAP_FAULT);
    let device = Device { ip: "127.0.0.1".into(), port };
    let ((), lines) = crate::logging::capture(|| {
        crate::events::register(std::rc::Rc::new(state(
            warning_log(),
            Box::new(move || Some(device.clone())),
        )));
        crate::events::send("database_change", &[]);
        crate::events::send("cli_exit", &[]);
    });
    assert!(
        lines
            .iter()
            .any(|l| l == "sonosupdate: Sonos update failed: s:Client: UPnPError"),
        "{lines:?}"
    );
}

#[test]
fn no_change_means_no_update() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let ((), lines) = crate::logging::capture(|| {
        crate::events::register(std::rc::Rc::new(state(
            warning_log(),
            Box::new(move || {
                calls2.fetch_add(1, Ordering::SeqCst);
                None
            }),
        )));
        crate::events::send("cli_exit", &[]);
    });
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn one_update_per_command() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let ((), _lines) = crate::logging::capture(|| {
        crate::events::register(std::rc::Rc::new(state(
            warning_log(),
            Box::new(move || {
                calls2.fetch_add(1, Ordering::SeqCst);
                None
            }),
        )));
        crate::events::send("database_change", &[]);
        crate::events::send("database_change", &[]);
        crate::events::send("cli_exit", &[]);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn load_registers_the_listener() {
    let cfg = Config::defaults();
    let _plugin = load(&cfg);
    assert!(crate::events::listening("database_change"));
    assert!(crate::events::listening("cli_exit"));
}
