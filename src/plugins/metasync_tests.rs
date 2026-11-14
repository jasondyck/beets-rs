use super::*;
use crate::config::Config;

#[test]
fn itunes_library_syncs_fields() {
    let mut c = Config::defaults();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/metasync/itunes_library_unix.xml");
    c.set(
        "metasync.itunes.library",
        fixture.to_string_lossy().into_owned().into(),
    );
    // The plugin declares these fields as typed flexible attributes.
    c.set(
        "plugins",
        serde_yaml::from_str("[metasync]").unwrap(),
    );
    crate::plugins::types::configure(&c).unwrap();
    let it = Itunes::new(&c).unwrap();
    let mut item = Item::new_item();
    item.set("path", Value::Path("/Music/Alt-J/An Awesome Wave/03 Tessellate.mp3".into()));
    it.sync(&mut item);
    assert_eq!(item.get("itunes_rating"), Some(Value::Int(80)));
    assert_eq!(item.get("itunes_skipcount"), Some(Value::Int(3)));
    assert_eq!(item.get("itunes_playcount"), Some(Value::Int(0)));
    let date = |f: &str| match item.get(f) {
        Some(Value::Float(d)) => d,
        other => panic!("{f}: {other:?}"),
    };
    assert!(date("itunes_lastskipped") > 0.0);
    assert!(date("itunes_dateadded") > 0.0);
    // Track 634 has no "Play Date UTC": beets leaves the field unset.
    assert!(item.get("itunes_lastplayed").is_none());
}

/// beets lower-cases the key on every platform (`bytestring_path(...).lower()`),
/// so a Location differing only in case still matches.
#[test]
fn itunes_paths_match_case_insensitively() {
    let mut c = Config::defaults();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/metasync/itunes_library_unix.xml");
    c.set(
        "metasync.itunes.library",
        fixture.to_string_lossy().into_owned().into(),
    );
    c.set("plugins", serde_yaml::from_str("[metasync]").unwrap());
    crate::plugins::types::configure(&c).unwrap();
    let it = Itunes::new(&c).unwrap();
    let mut item = Item::new_item();
    // The fixture's Location is `/Music/Alt-J/An Awesome Wave/03 Tessellate.mp3`.
    item.set(
        "path",
        Value::Path("/music/alt-j/an awesome wave/03 tessellate.mp3".into()),
    );
    it.sync(&mut item);
    assert_eq!(item.get("itunes_rating"), Some(Value::Int(80)));
    assert_eq!(item.get("itunes_skipcount"), Some(Value::Int(3)));
}

#[test]
fn itunes_windows_paths_match() {
    let mut c = Config::defaults();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/metasync/itunes_library_windows.xml");
    c.set(
        "metasync.itunes.library",
        fixture.to_string_lossy().into_owned().into(),
    );
    c.set("plugins", serde_yaml::from_str("[metasync]").unwrap());
    crate::plugins::types::configure(&c).unwrap();
    let it = Itunes::new(&c).unwrap();
    let mut item = Item::new_item();
    // `file://localhost/G:/Music/...` is stored posix-style, lowercased.
    item.set(
        "path",
        Value::Path("/G:/Music/Alt-J/An Awesome Wave/03 Tessellate.mp3".into()),
    );
    it.sync(&mut item);
    assert_eq!(item.get("itunes_rating"), Some(Value::Int(80)));
    assert_eq!(item.get("itunes_skipcount"), Some(Value::Int(3)));
}

// ---------------------------------------------------------------- amarok

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use zbus::zvariant::{OwnedValue, Value as DbusValue};

/// A private `dbus-daemon` for one test; killed on drop.
struct PrivateBus {
    child: Child,
    address: String,
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a private session bus in a temp directory. `None` when no bus can
/// be made here (then the test skips, like beets' dbus tests).
fn private_bus() -> Option<PrivateBus> {
    let dir = tempfile::tempdir().ok()?;
    let socket = dir.path().join("bus");
    let mut child = Command::new("dbus-daemon")
        .arg("--session")
        .arg(format!("--address=unix:path={}", socket.display()))
        .arg("--print-address=1")
        .arg("--nofork")
        .arg("--nopidfile")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut line = String::new();
    let mut out = BufReader::new(child.stdout.take()?);
    out.read_line(&mut line).ok()?;
    let address = line.trim().to_string();
    if address.is_empty() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    // Keep the directory (and socket) alive with the bus.
    std::mem::forget(dir);
    Some(PrivateBus { child, address })
}

/// A fake `org.kde.amarok` `/Collection` service.
struct FakeCollection {
    path: String,
}

#[zbus::interface(name = "org.kde.amarok.Collection")]
impl FakeCollection {
    #[zbus(name = "Query")]
    fn query(&self, _xml: String) -> Vec<HashMap<String, OwnedValue>> {
        let track = |path: &str, rating: i32| {
            let mut t: HashMap<String, OwnedValue> = HashMap::new();
            let mut put = |key: &str, v: DbusValue<'static>| {
                t.insert(key.to_string(), OwnedValue::try_from(v).unwrap());
            };
            put("xesam:url", DbusValue::from(path.to_string()));
            put("xesam:userRating", DbusValue::from(rating));
            put("xesam:autoRating", DbusValue::from(0.6f64));
            put("xesam:useCount", DbusValue::from(31i32));
            put(
                "xesam:id",
                DbusValue::from("amarok-sqltrackuid://abc123".to_string()),
            );
            put(
                "xesam:firstUsed",
                DbusValue::from(vec![vec![2015i32, 2, 5], vec![15, 41, 4]]),
            );
            put(
                "xesam:lastUsed",
                DbusValue::from(vec![vec![2015i32, 5, 4], vec![12, 20, 51]]),
            );
            t
        };
        vec![
            track("/Music/Other/not-this-one.mp3", 10),
            track(&self.path, 80),
        ]
    }
}

#[test]
fn amarok_syncs_from_a_private_dbus_service() {
    let Some(bus) = private_bus() else {
        eprintln!("skipping: no D-Bus daemon available");
        return;
    };
    let path = "/Music/Alt-J/An Awesome Wave/03 Tessellate.mp3";
    let _service = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.kde.amarok")
        .unwrap()
        .serve_at("/Collection", FakeCollection { path: path.into() })
        .unwrap()
        .build()
        .unwrap();
    let client = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap();

    let mut c = Config::defaults();
    c.set("plugins", serde_yaml::from_str("[metasync]").unwrap());
    crate::plugins::types::configure(&c).unwrap();
    let amarok = Amarok::from_connection(client);
    let mut item = Item::new_item();
    item.set("path", Value::Path(path.into()));
    amarok.sync(&mut item).unwrap();

    assert_eq!(item.get("amarok_rating"), Some(Value::Int(80)));
    assert_eq!(item.get("amarok_score"), Some(Value::Float(0.6)));
    assert_eq!(item.get("amarok_playcount"), Some(Value::Int(31)));
    assert_eq!(item.get("amarok_uid"), Some(Value::Str("abc123".into())));
    let date = |y, m, d, hh, mm, ss| {
        mktime_local(
            chrono::NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(hh, mm, ss)
                .unwrap(),
        )
    };
    assert_eq!(
        item.get("amarok_firstplayed"),
        Some(Value::Float(date(2015, 2, 5, 15, 41, 4)))
    );
    assert_eq!(
        item.get("amarok_lastplayed"),
        Some(Value::Float(date(2015, 5, 4, 12, 20, 51)))
    );
}

#[test]
fn amarok_quoteattr_as_python() {
    assert_eq!(quoteattr("plain"), "\"plain\"");
    assert_eq!(quoteattr("a\"b"), "'a\"b'");
    assert_eq!(quoteattr("a\"b'c"), "\"a&quot;b'c\"");
    assert_eq!(quoteattr("a&b<c>"), "\"a&amp;b&lt;c&gt;\"");
}

#[test]
fn the_format_option_labels_the_no_match_warning() {
    let mut c = Config::defaults();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/metasync/itunes_library_unix.xml");
    c.set(
        "metasync.itunes.library",
        fixture.to_string_lossy().into_owned().into(),
    );
    c.set("plugins", serde_yaml::from_str("[metasync]").unwrap());
    crate::plugins::types::configure(&c).unwrap();
    let lib = crate::library::Library::in_memory().unwrap();
    let mut item = Item::new_item();
    item.set("title", Value::Str("Two".into()));
    item.set("path", Value::Path("/nowhere/Two.mp3".into()));
    lib.add(&mut item).unwrap();
    // beets' `-f` sets `format_item`, which `str(item)` (the warning's
    // item) and the change display use.
    let ((), lines) = crate::logging::capture(|| {
        command(
            &c,
            &lib,
            &CmdOpts {
                pretend: false,
                sources: vec!["itunes".into()],
                limit: None,
                query: vec![],
                format: Some("$title: $itunes_rating".into()),
            },
        )
        .unwrap();
    });
    assert!(
        lines
            .iter()
            .any(|l| l == "metasync: no iTunes match found for Two: $itunes_rating"),
        "{lines:?}"
    );
}
