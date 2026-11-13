//! A minimal port of the parts of soco that beets' `sonosupdate` plugin
//! uses: SSDP discovery of a Sonos player (`soco.discovery.any_soco`)
//! and the ContentDirectory `RefreshShareIndex` SOAP call
//! (`soco.music_library.start_library_update`).
//!
//! Only the IPv4 multicast discovery of soco's `discover()` is ported
//! (no interface enumeration — the system's default multicast interface
//! is used), with its M-SEARCH message, three sends, 100 ms receive
//! steps, `timeout` of 5 s and the `household_id` "Sonos" response check.
//! The device's port comes from the response's `LOCATION` header (1400
//! when it is missing), where soco always assumes 1400; real players
//! advertise 1400, and this lets tests point discovery at a fake device.

use anyhow::{Result, anyhow};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// soco's `MCAST_GRP` and `MCAST_PORT`.
pub const MULTICAST: &str = "239.255.255.250:1900";
/// soco's default discovery `timeout` (seconds).
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// soco's `config.REQUEST_TIMEOUT` (seconds).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// soco's `select()` step while waiting for discovery responses.
const POLL: Duration = Duration::from_millis(100);
/// `SoCo.__init__`'s default UPnP port.
const SOCO_PORT: u16 = 1400;
/// soco's `household_id` default, which a discovery response must contain.
const HOUSEHOLD_ID: &[u8] = b"Sonos";

/// soco's `PLAYER_SEARCH` M-SEARCH datagram.
const M_SEARCH: &str = "M-SEARCH * HTTP/1.1\n\
HOST: 239.255.255.250:1900\n\
MAN: \"ssdp:discover\"\n\
MX: 1\n\
ST: urn:schemas-upnp-org:device:ZonePlayer:1\n";

/// The `RefreshShareIndex` envelope soco's `Service.build_command` sends
/// for `start_library_update("")`.
const REFRESH_SHARE_INDEX: &str = "<?xml version=\"1.0\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body>\
<u:RefreshShareIndex xmlns:u=\"urn:schemas-upnp-org:service:ContentDirectory:1\">\
<AlbumArtistDisplayOption></AlbumArtistDisplayOption>\
</u:RefreshShareIndex>\
</s:Body>\
</s:Envelope>";

const SOAP_ACTION: &str =
    "urn:schemas-upnp-org:service:ContentDirectory:1#RefreshShareIndex";

/// A Sonos player (soco's `SoCo` instance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub ip: String,
    pub port: u16,
}

impl Device {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.ip, self.port)
    }

    /// `SoCo.music_library.start_library_update()`: a UPnP
    /// ContentDirectory `RefreshShareIndex` action with an empty
    /// `AlbumArtistDisplayOption`.
    pub fn start_library_update(&self) -> Result<()> {
        let url = format!("{}/MediaServer/ContentDirectory/Control", self.base_url());
        let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
        let response = agent
            .post(&url)
            .set("Content-Type", "text/xml; charset=\"utf-8\"")
            .set("SOAPACTION", SOAP_ACTION)
            .send_bytes(REFRESH_SHARE_INDEX.as_bytes());
        match response {
            Ok(r) => {
                let body = r.into_string()?;
                soap_response(200, &body, &self.ip)
            }
            Err(ureq::Error::Status(status, r)) => {
                let body = r.into_string().unwrap_or_default();
                soap_response(status, &body, &self.ip)
            }
            Err(ureq::Error::Transport(t)) => Err(anyhow!("{t}")),
        }
    }
}

/// How soco's `Service.send_command` treats a response.
fn soap_response(status: u16, body: &str, ip: &str) -> Result<()> {
    match status {
        200 => {
            // soco unwraps the action's out arguments (none here), which
            // parses the envelope.
            let doc = roxmltree::Document::parse(body)?;
            if doc.descendants().any(|n| {
                n.has_tag_name(("http://schemas.xmlsoap.org/soap/envelope/", "Body"))
            }) {
                Ok(())
            } else {
                Err(anyhow!("invalid SOAP response"))
            }
        }
        405 => Err(anyhow!("RefreshShareIndex not supported on {ip}")),
        500 => {
            // A UPnP error: soco raises `SoapFault`, whose message is
            // "faultcode: faultstring".
            let code = fault_text(body, "faultcode").unwrap_or_default();
            let string = fault_text(body, "faultstring").unwrap_or_default();
            Err(anyhow!("{code}: {string}"))
        }
        _ => Err(anyhow!("HTTP status {status}")),
    }
}

fn fault_text(body: &str, tag: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(body).ok()?;
    doc.descendants()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.text())
        .map(str::to_string)
}

/// `soco.discovery.any_soco()`: any visible Sonos player, or `None`.
pub fn any_soco() -> Option<Device> {
    discover(&MULTICAST.parse().unwrap(), DISCOVERY_TIMEOUT)
}

/// `soco.discovery.discover()` without the interface enumeration or the
/// zone topology query: send the M-SEARCH to `target` (multicast in
/// production, a test socket in tests) and return the first player that
/// answers with a "Sonos" household.
pub fn discover(target: &SocketAddr, timeout: Duration) -> Option<Device> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // UPnP v1.0 requires a TTL of 4 (soco's `create_socket`).
    let _ = socket.set_multicast_ttl_v4(4);
    let _ = socket.set_read_timeout(Some(POLL));

    // "Send a few times to each socket. UDP is unreliable."
    let mut sent = 0;
    for _ in 0..3 {
        if socket.send_to(M_SEARCH.as_bytes(), target).is_ok() {
            sent += 1;
        }
    }
    if sent == 0 {
        return None;
    }

    let start = Instant::now();
    let mut buf = [0u8; 1024];
    loop {
        if start.elapsed() > timeout {
            return None;
        }
        match socket.recv_from(&mut buf) {
            Ok((n, addr)) => {
                let data = &buf[..n];
                // "To prevent misbehaved devices on the network disrupting
                // the discovery process, we check that the response contains
                // the 'Sonos' string."
                if !data.windows(HOUSEHOLD_ID.len()).any(|w| w == HOUSEHOLD_ID) {
                    continue;
                }
                let (ip, port) = location(data)
                    .unwrap_or_else(|| (addr.ip().to_string(), SOCO_PORT));
                return Some(Device { ip, port });
            }
            // The read timeout: keep waiting until the discovery timeout.
            Err(_) => continue,
        }
    }
}

/// The device's address from the SSDP response's `LOCATION` header
/// (`http://host:port/xml/device_description.xml`).
fn location(data: &[u8]) -> Option<(String, u16)> {
    let text = String::from_utf8_lossy(data);
    let value = text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case("LOCATION").then_some(value)
    })?;
    let rest = value.trim().strip_prefix("http://")?;
    let host = rest.split('/').next()?;
    let (ip, port) = match host.rsplit_once(':') {
        Some((ip, port)) => (ip, port.parse().ok()?),
        None => (host, SOCO_PORT),
    };
    ip.parse::<Ipv4Addr>().ok()?;
    Some((ip.to_string(), port))
}

#[cfg(test)]
#[path = "sonos_tests.rs"]
mod tests;
