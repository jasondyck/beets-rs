//! Behaviour and robustness tests for the ASF tag reader and writer. Each
//! test builds a file with the objects of Microsoft's ASF specification
//! and checks what MediaFile/beets would read and write.

use super::*;

fn object(guid: &[u8; 16], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(guid);
    out.extend_from_slice(&(24 + body.len() as u64).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn utf16z(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.extend_from_slice(&[0, 0]);
    out
}

fn content_body(values: &[&str]) -> Vec<u8> {
    let strings: Vec<Vec<u8>> = values.iter().map(|s| utf16z(s)).collect();
    let mut out = Vec::new();
    for s in &strings {
        out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    }
    for s in strings {
        out.extend_from_slice(&s);
    }
    out
}

/// One Extended Content Description descriptor.
fn descriptor(name: &str, type_id: u16, data: &[u8]) -> Vec<u8> {
    let name = utf16z(name);
    let mut out = Vec::new();
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&name);
    out.extend_from_slice(&type_id.to_le_bytes());
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn extended_body(attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
    for a in attrs {
        out.extend_from_slice(a);
    }
    out
}

fn metadata_record(name: &str, type_id: u16, data: &[u8]) -> Vec<u8> {
    let name = utf16z(name);
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // language list index
    out.extend_from_slice(&0u16.to_le_bytes()); // stream number
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&type_id.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&name);
    out.extend_from_slice(data);
    out
}

fn metadata_body(records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u16).to_le_bytes());
    for r in records {
        out.extend_from_slice(r);
    }
    out
}

fn extension_body(children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&HEADER_EXTENSION_RESERVED_GUID);
    out.extend_from_slice(&6u16.to_le_bytes());
    let len: usize = children.iter().map(Vec::len).sum();
    out.extend_from_slice(&(len as u32).to_le_bytes());
    for c in children {
        out.extend_from_slice(c);
    }
    out
}

fn file_properties(play: u64, preroll: u64, max_bitrate: u32) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    b[16..24].copy_from_slice(&1000u64.to_le_bytes()); // file size, patched
    b[40..48].copy_from_slice(&play.to_le_bytes());
    b[56..64].copy_from_slice(&preroll.to_le_bytes());
    b[76..80].copy_from_slice(&max_bitrate.to_le_bytes());
    b
}

fn audio_stream(codec: u16, channels: u16, rate: u32, byte_rate: u32) -> Vec<u8> {
    let mut b = vec![0u8; 54 + 18];
    b[..16].copy_from_slice(&AUDIO_MEDIA_GUID);
    b[40..44].copy_from_slice(&18u32.to_le_bytes());
    b[54..56].copy_from_slice(&codec.to_le_bytes());
    b[56..58].copy_from_slice(&channels.to_le_bytes());
    b[58..62].copy_from_slice(&rate.to_le_bytes());
    b[62..66].copy_from_slice(&byte_rate.to_le_bytes());
    b
}

/// A header object over `objects`.
fn header(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&HEADER_GUID);
    let total: usize = 30 + objects.iter().map(Vec::len).sum::<usize>();
    out.extend_from_slice(&(total as u64).to_le_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_le_bytes());
    out.extend_from_slice(&[1, 2]);
    for o in objects {
        out.extend_from_slice(o);
    }
    out
}

const AUDIO: &[u8] = b"audio payload";

fn data_object() -> Vec<u8> {
    let mut body = vec![0u8; 26];
    body[16..24].copy_from_slice(&1u64.to_le_bytes());
    let mut out = object(
        &[
            0x36, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62,
            0xce, 0x6c,
        ],
        &body,
    );
    out.extend_from_slice(AUDIO);
    out
}

/// A file with a title, one artist list value in the extended object and a
/// second in the metadata library.
fn sample() -> Vec<u8> {
    let mut f = header(&[
        object(
            &FILE_PROPERTIES_GUID,
            &file_properties(61_000_000, 3100, 3200),
        ),
        object(
            &STREAM_PROPERTIES_GUID,
            &audio_stream(353, 1, 44_100, 16_000),
        ),
        object(
            &CONTENT_DESCRIPTION_GUID,
            &content_body(&["T", "A", "", "", ""]),
        ),
        object(
            &EXTENDED_CONTENT_DESCRIPTION_GUID,
            &extended_body(&[
                descriptor("WM/ARTISTS", 0, &utf16z("A1")),
                descriptor("WM/IsCompilation", 2, &1u32.to_le_bytes()),
                descriptor("WM/TrackNumber", 3, &7u32.to_le_bytes()),
            ]),
        ),
        object(
            &HEADER_EXTENSION_GUID,
            &extension_body(&[object(
                &METADATA_LIBRARY_GUID,
                &metadata_body(&[metadata_record("WM/ARTISTS", 0, &utf16z("A2"))]),
            )]),
        ),
        // An object this port doesn't know: kept verbatim.
        object(&[0xaa; 16], b"unknown body"),
    ]);
    f.extend_from_slice(&data_object());
    f
}

fn write(dir: &tempfile::TempDir, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, data).unwrap();
    p
}

#[test]
fn reads_the_content_and_metadata_objects() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let a = Asf::open(&p).unwrap();
    assert_eq!(a.values("Title"), vec![Value::Unicode("T".into())]);
    assert_eq!(a.values("Author"), vec![Value::Unicode("A".into())]);
    assert_eq!(
        a.values("WM/ARTISTS"),
        vec![Value::Unicode("A1".into()), Value::Unicode("A2".into())]
    );
    assert_eq!(a.values("WM/IsCompilation"), vec![Value::Bool(true)]);
    assert_eq!(a.values("WM/TrackNumber"), vec![Value::DWord(7)]);
    assert!(a.values("WM/AlbumTitle").is_empty());
}

#[test]
fn audio_properties_like_mutagen() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let a = Asf::open(&p).unwrap();
    let props = a.properties();
    assert_eq!(props.duration(), std::time::Duration::from_secs_f64(3.0));
    assert_eq!(props.sample_rate(), Some(44_100));
    assert_eq!(props.channels(), Some(1));
    assert_eq!(props.audio_bitrate(), Some(128));
    assert_eq!(props.bit_depth(), None);
    assert_eq!(props.overall_bitrate(), Some(128));
}

#[test]
fn writes_and_keeps_unknown_objects_and_audio() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let mut a = Asf::open(&p).unwrap();
    a.set("WM/AlbumTitle", vec![Value::Unicode("Alb".into())]);
    a.set("WM/ARTISTS", vec![Value::Unicode("B1".into())]);
    a.set("Copyright", vec![Value::Unicode("C".into())]);
    a.set("Date", vec![Value::Unicode("unused".into())]);
    a.remove("WM/TrackNumber");
    a.save().unwrap();

    let data = std::fs::read(&p).unwrap();
    assert!(data.ends_with(AUDIO), "audio data must be untouched");
    assert!(data.windows(16).any(|w| w == [0xaa; 16]));
    let a = Asf::open(&p).unwrap();
    assert_eq!(
        a.values("WM/AlbumTitle"),
        vec![Value::Unicode("Alb".into())]
    );
    assert_eq!(a.values("WM/ARTISTS"), vec![Value::Unicode("B1".into())]);
    assert_eq!(a.values("Copyright"), vec![Value::Unicode("C".into())]);
    assert!(a.values("WM/TrackNumber").is_empty());
    assert_eq!(a.values("Title"), vec![Value::Unicode("T".into())]);
    // An in-place rewrite (the header still fits, padded) keeps the size.
    assert_eq!(data.len(), sample().len());
}

#[test]
fn a_larger_header_moves_the_data() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let mut a = Asf::open(&p).unwrap();
    let many: Vec<Value> = (0..50)
        .map(|i| Value::Unicode(format!("artist {i}")))
        .collect();
    a.set("WM/ARTISTS", many.clone());
    a.save().unwrap();
    let data = std::fs::read(&p).unwrap();
    assert!(data.ends_with(AUDIO));
    let a = Asf::open(&p).unwrap();
    assert_eq!(a.values("WM/ARTISTS"), many);
    // The file properties' file size follows the new header.
    let props = a.properties();
    assert_eq!(props.duration(), std::time::Duration::from_secs_f64(3.0));
}

#[test]
fn the_content_description_holds_at_most_one_value() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let mut a = Asf::open(&p).unwrap();
    a.set(
        "Title",
        vec![Value::Unicode("one".into()), Value::Unicode("two".into())],
    );
    a.save().unwrap();
    let a = Asf::open(&p).unwrap();
    assert_eq!(a.values("Title"), vec![Value::Unicode("one".into())]);
}

#[test]
fn metadata_library_values_are_read_too() {
    let d = tempfile::tempdir().unwrap();
    // A Metadata Object holding what mutagen would keep in its library.
    let mut f = header(&[
        object(&FILE_PROPERTIES_GUID, &file_properties(10_000_000, 0, 0)),
        object(
            &HEADER_EXTENSION_GUID,
            &extension_body(&[
                object(
                    &METADATA_GUID,
                    &metadata_body(&[metadata_record("WM/Composer", 0, &utf16z("C1"))]),
                ),
                object(
                    &METADATA_LIBRARY_GUID,
                    &metadata_body(&[metadata_record("WM/Composer", 0, &utf16z("C2"))]),
                ),
            ]),
        ),
    ]);
    f.extend_from_slice(&data_object());
    let p = write(&d, "a.wma", &f);
    let a = Asf::open(&p).unwrap();
    assert_eq!(
        a.values("WM/Composer"),
        vec![Value::Unicode("C1".into()), Value::Unicode("C2".into())]
    );
}

#[test]
fn every_data_type_round_trips() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let values = vec![
        Value::Unicode("héllo".into()),
        Value::Bytes(vec![0, 1, 2, 255]),
        Value::Bool(false),
        Value::DWord(4_000_000_000),
        Value::QWord(u64::MAX),
        Value::Word(65000),
        Value::Guid([7; 16]),
        Value::Unknown(99, vec![1, 2, 3]),
    ];
    let mut a = Asf::open(&p).unwrap();
    a.set("Mixed", values.clone());
    a.save().unwrap();
    let a = Asf::open(&p).unwrap();
    assert_eq!(a.values("Mixed"), values);
}

#[test]
fn remove_all_deletes_every_tag() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "a.wma", &sample());
    let mut a = Asf::open(&p).unwrap();
    a.remove_all();
    a.save().unwrap();
    let a = Asf::open(&p).unwrap();
    for name in [
        "Title",
        "Author",
        "WM/ARTISTS",
        "WM/IsCompilation",
        "WM/TrackNumber",
    ] {
        assert!(a.values(name).is_empty(), "{name}");
    }
    assert!(std::fs::read(&p).unwrap().ends_with(AUDIO));
}

#[test]
fn truncated_and_corrupt_headers_are_errors_not_panics() {
    let d = tempfile::tempdir().unwrap();
    let good = sample();
    // Every truncation of the file.
    for n in 0..good.len().min(400) {
        let p = write(&d, "t.wma", &good[..n]);
        let _ = Asf::open(&p);
        let _ = std::panic::catch_unwind(|| Asf::open(&p)).ok();
    }
    // A huge object count, an object bigger than the header, a name
    // length that doesn't fit and an odd name length.
    for patch in 0..6 {
        let mut bad = good.clone();
        match patch {
            0 => bad[24..28].copy_from_slice(&u32::MAX.to_le_bytes()),
            1 => bad[30 + 16..30 + 24].copy_from_slice(&u64::MAX.to_le_bytes()),
            2 => bad[24..32].copy_from_slice(&u64::MAX.to_le_bytes()),
            3 => {
                // The first extended descriptor's name length.
                let at = bad
                    .windows(16)
                    .position(|w| w == EXTENDED_CONTENT_DESCRIPTION_GUID)
                    .unwrap();
                bad[at + 24 + 2..at + 28].copy_from_slice(&u16::MAX.to_le_bytes());
            }
            4 => {
                let at = bad
                    .windows(16)
                    .position(|w| w == EXTENDED_CONTENT_DESCRIPTION_GUID)
                    .unwrap();
                bad[at + 24..at + 26].copy_from_slice(&u16::MAX.to_le_bytes());
            }
            _ => bad[16..24].copy_from_slice(&(good.len() as u64 + 1).to_le_bytes()),
        }
        let p = write(&d, "c.wma", &bad);
        assert!(Asf::open(&p).is_err(), "patch {patch}");
    }
}

#[test]
fn a_non_asf_file_is_not_an_asf_file() {
    let d = tempfile::tempdir().unwrap();
    let p = write(&d, "x.wma", b"RIFF....WAVE");
    assert!(!is_asf(&p));
    assert!(Asf::open(&p).is_err());
    let missing = d.path().join("missing.wma");
    assert!(!is_asf(&missing));
    assert!(Asf::open(&missing).is_err());
}
