//! Tests for the soco port: SSDP discovery against a fake device on
//! 127.0.0.1 and the ContentDirectory SOAP call against a fake server.

use super::*;
use crate::plugins::requests::testserver::{Reply, serve};
use std::thread::JoinHandle;
use std::time::Instant;

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
fn fake_soap(status: u16, content_type: &'static str, body: &'static str) -> u16 {
    port_of(
        &serve(move |_| Reply {
            status,
            content_type,
            body: body.as_bytes().to_vec(),
        })
        .base,
    )
}

/// A UDP socket standing in for a Sonos player's SSDP responder: it
/// records the M-SEARCH datagrams it receives and answers them with
/// `responses` (in order).
fn fake_ssdp(responses: Vec<String>) -> (SocketAddr, JoinHandle<Vec<String>>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let addr = socket.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut seen = vec![];
        let mut buf = [0u8; 2048];
        let mut responses = responses.into_iter();
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            seen.push(String::from_utf8_lossy(&buf[..n]).into_owned());
            if let Some(response) = responses.next() {
                socket.send_to(response.as_bytes(), from).unwrap();
            }
        }
        seen
    });
    (addr, handle)
}

fn ssdp_response(port: u16) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
         CACHE-CONTROL: max-age = 1800\r\n\
         EXT:\r\n\
         LOCATION: http://127.0.0.1:{port}/xml/device_description.xml\r\n\
         SERVER: Linux UPnP/1.0 Sonos/26.1-76230 (ZPS3)\r\n\
         ST: urn:schemas-upnp-org:device:ZonePlayer:1\r\n\
         USN: uuid:RINCON_000000000000::urn:schemas-upnp-org:device:ZonePlayer:1\r\n\
         X-RINCON-HOUSEHOLD: Sonos_abc\r\n\r\n"
    )
}

#[test]
fn discovery_finds_a_player_and_refreshes_its_library() {
    let server = serve(|_| Reply {
        status: 200,
        content_type: "text/xml",
        body: SOAP_OK.as_bytes().to_vec(),
    });
    let port = port_of(&server.base);
    let (addr, handle) = fake_ssdp(vec![ssdp_response(port)]);

    let device = discover(&addr, Duration::from_secs(2)).unwrap();
    assert_eq!(
        device,
        Device {
            ip: "127.0.0.1".into(),
            port
        }
    );
    device.start_library_update().unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/MediaServer/ContentDirectory/Control");
    assert_eq!(
        request.header("content-type"),
        Some("text/xml; charset=\"utf-8\"")
    );
    assert_eq!(
        request.header("soapaction"),
        Some("urn:schemas-upnp-org:service:ContentDirectory:1#RefreshShareIndex")
    );
    assert_eq!(
        std::str::from_utf8(&request.body).unwrap(),
        REFRESH_SHARE_INDEX
    );

    let searches = handle.join().unwrap();
    assert_eq!(searches, vec![M_SEARCH.to_string(); 3]);
}

#[test]
fn discovery_ignores_responses_without_sonos() {
    let other = "HTTP/1.1 200 OK\r\n\
         LOCATION: http://127.0.0.1:9/xml/device_description.xml\r\n\
         SERVER: Linux UPnP/1.0 Other/1.0\r\n\r\n"
        .to_string();
    let (addr, handle) = fake_ssdp(vec![other, ssdp_response(1400)]);
    let device = discover(&addr, Duration::from_secs(2)).unwrap();
    assert_eq!(
        device,
        Device {
            ip: "127.0.0.1".into(),
            port: 1400
        }
    );
    handle.join().unwrap();
}

#[test]
fn a_response_without_a_location_is_on_port_1400() {
    let response = "HTTP/1.1 200 OK\r\n\
         SERVER: Linux UPnP/1.0 Sonos/26.1-76230 (ZPS3)\r\n\
         X-RINCON-HOUSEHOLD: Sonos_abc\r\n\r\n"
        .to_string();
    let (addr, handle) = fake_ssdp(vec![response]);
    let device = discover(&addr, Duration::from_secs(2)).unwrap();
    assert_eq!(
        device,
        Device {
            ip: "127.0.0.1".into(),
            port: 1400
        }
    );
    handle.join().unwrap();
}

#[test]
fn discovery_times_out_without_a_response() {
    let (addr, handle) = fake_ssdp(vec![]);
    let start = Instant::now();
    assert_eq!(discover(&addr, Duration::from_millis(300)), None);
    assert!(start.elapsed() >= Duration::from_millis(300));
    handle.join().unwrap();
}

#[test]
fn a_soap_fault_is_an_error() {
    let port = fake_soap(500, "text/xml", SOAP_FAULT);
    let err = Device {
        ip: "127.0.0.1".into(),
        port,
    }
    .start_library_update()
    .unwrap_err();
    assert_eq!(err.to_string(), "s:Client: UPnPError");
}

#[test]
fn a_405_means_the_action_is_not_supported() {
    let port = fake_soap(405, "text/plain", "");
    let err = Device {
        ip: "127.0.0.1".into(),
        port,
    }
    .start_library_update()
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "RefreshShareIndex not supported on 127.0.0.1"
    );
}

#[test]
fn a_non_soap_error_status_is_an_error() {
    let port = fake_soap(404, "text/plain", "");
    let err = Device {
        ip: "127.0.0.1".into(),
        port,
    }
    .start_library_update()
    .unwrap_err();
    assert_eq!(err.to_string(), "HTTP status 404");
}

#[test]
fn an_unparseable_200_is_an_error() {
    let port = fake_soap(200, "text/xml", "not xml");
    let err = Device {
        ip: "127.0.0.1".into(),
        port,
    }
    .start_library_update()
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}

/// The device description URL soco's `get_speaker_info` reads (not used
/// by the plugin, but part of what a discovery response points at).
#[test]
fn the_location_parser_handles_a_plain_host_and_a_port() {
    assert_eq!(
        location(b"HTTP/1.1 200 OK\r\nLOCATION: http://10.0.0.2/xml/device_description.xml\r\n"),
        Some(("10.0.0.2".to_string(), 1400))
    );
    assert_eq!(
        location(b"HTTP/1.1 200 OK\r\nlocation: http://10.0.0.2:1401/x\r\n"),
        Some(("10.0.0.2".to_string(), 1401))
    );
    assert_eq!(location(b"HTTP/1.1 200 OK\r\n\r\n"), None);
}
