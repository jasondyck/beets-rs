//! ASF ("Windows Media") audio properties: the length from the File
//! Properties Object's play duration and preroll, and the bitrate from
//! the first audio Stream Properties Object's average byte rate (the
//! file's maximum bitrate when it has none), as Mutagen's `ASFInfo`
//! reports them to beets.
//!
//! The parser implements the object layout of Microsoft's "Advanced
//! Systems Format (ASF) Specification": a Header Object holding objects
//! of a 16-byte GUID and a 64-bit size, and audio stream type data as a
//! `WAVEFORMATEX` structure.

use super::{Info, Source};

/// The Header Object (`75B22630-668E-11CF-A6D9-00AA0062CE6C`).
const HEADER_GUID: [u8; 16] = [
    0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce, 0x6c,
];
/// `8CABDCA1-A947-11CF-8EE4-00C00C205365`.
const FILE_PROPERTIES_GUID: [u8; 16] = [
    0xa1, 0xdc, 0xab, 0x8c, 0x47, 0xa9, 0xcf, 0x11, 0x8e, 0xe4, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
/// `B7DC0791-A9B7-11CF-8EE6-00C00C205365`.
const STREAM_PROPERTIES_GUID: [u8; 16] = [
    0x91, 0x07, 0xdc, 0xb7, 0xb7, 0xa9, 0xcf, 0x11, 0x8e, 0xe6, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
/// `F8699E40-5B4D-11CF-A8FD-00805F5C442B`: an audio media stream.
const AUDIO_MEDIA_GUID: [u8; 16] = [
    0x40, 0x9e, 0x69, 0xf8, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b,
];

/// The GUIDs for the tests (the same constants, published for the tests
/// in `tests.rs` that build a header).
#[cfg(test)]
pub const HEADER_GUID_FOR_TEST: [u8; 16] = HEADER_GUID;
#[cfg(test)]
pub const FILE_PROPERTIES_GUID_FOR_TEST: [u8; 16] = FILE_PROPERTIES_GUID;
#[cfg(test)]
pub const STREAM_PROPERTIES_GUID_FOR_TEST: [u8; 16] = STREAM_PROPERTIES_GUID;
#[cfg(test)]
pub const AUDIO_MEDIA_GUID_FOR_TEST: [u8; 16] = AUDIO_MEDIA_GUID;

/// A real header is kilobytes; anything near this is corrupt.
const MAX_HEADER: u64 = 64 << 20;
const MAX_OBJECTS: usize = 10_000;

pub fn parse(src: &mut Source) -> Option<Info> {
    let head = src.at(0, 24)?;
    if head[..16] != HEADER_GUID {
        return None;
    }
    let header_size = u64::from_le_bytes(head[16..24].try_into().ok()?);
    if header_size < 30 || header_size > src.len || header_size > MAX_HEADER {
        return None;
    }
    let body_len = usize::try_from(header_size - 24).ok()?;
    let body = src.at(24, body_len)?;
    let count = crate::audioprops::le32(&body, 0)?;
    let mut at = 6usize;
    let mut length = None;
    let mut stream_bitrate = None;
    for _ in 0..count.min(MAX_OBJECTS as u32) {
        let guid = body.get(at..at + 16)?;
        let size = u64::from_le_bytes(body.get(at + 16..at + 24)?.try_into().ok()?);
        if size < 24 || size > (body.len() - at) as u64 {
            return None;
        }
        let len = usize::try_from(size - 24).ok()?;
        let obj = body.get(at + 24..at + 24 + len)?;
        if guid == &FILE_PROPERTIES_GUID[..] {
            length = file_length(obj);
        } else if guid == &STREAM_PROPERTIES_GUID[..] && stream_bitrate.is_none() {
            stream_bitrate = audio_stream(obj).map(|a| i64::from(a.byte_rate) * 8);
        }
        at += usize::try_from(size).ok()?;
    }
    // Mutagen reports 0 when the stream has no byte rate; MediaFile then
    // estimates the bitrate from the file size, which the caller does.
    Some(Info {
        length: length.unwrap_or(0.0),
        bitrate: stream_bitrate.unwrap_or(0),
    })
}

/// The File Properties Object's play duration (100 ns units, less the
/// preroll in ms).
fn file_length(body: &[u8]) -> Option<f64> {
    // File ID GUID, file size, creation date, packet count, play
    // duration, send duration and preroll.
    if body.len() < 64 {
        return None;
    }
    let u64_at = |at: usize| {
        body.get(at..at + 8)
            .and_then(|s| s.try_into().ok())
            .map_or(0, u64::from_le_bytes)
    };
    let play = u64_at(40) as f64 / 1e7;
    let preroll = u64_at(56) as f64 / 1000.0;
    Some(play - preroll)
}

struct AudioStream {
    byte_rate: u32,
}

/// A Stream Properties Object's `WAVEFORMATEX` when it is an audio
/// stream.
fn audio_stream(body: &[u8]) -> Option<AudioStream> {
    if body.get(..16)? != AUDIO_MEDIA_GUID {
        return None;
    }
    // Two GUIDs (32), time offset (8), type-specific length (4) and
    // error-correction length (4), flags (2), reserved (4).
    let type_len = u32::from_le_bytes(body.get(40..44)?.try_into().ok()?);
    if type_len < 12 {
        return None;
    }
    let t = body.get(54..)?;
    Some(AudioStream {
        byte_rate: u32::from_le_bytes(t.get(8..12)?.try_into().ok()?),
    })
}
