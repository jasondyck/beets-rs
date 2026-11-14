//! MP4/M4A: the audio track's `mdhd` duration and the sample entry's
//! codec bitrate.
//!
//! File structure, `mdhd`, `stsd` and the sample entries are ISO/IEC
//! 14496-12; `mp4a` and the `esds` `DecoderConfigDescriptor` are ISO/IEC
//! 14496-14 (average bitrate). Apple Lossless keeps its average bitrate
//! in the `alac` magic cookie.

use super::{Info, Source, be32, be64, seconds};

type Range = (u64, u64);

const HEADER: u64 = 8;

/// The child boxes of the box payload `start..end`.
fn children(src: &mut Source, start: u64, end: u64) -> Vec<([u8; 4], Range)> {
    let mut out = Vec::new();
    let mut at = start;
    while at + HEADER <= end {
        let Some(h) = src.at(at, 8) else { break };
        let mut size = u64::from(be32(&h, 0).unwrap_or(0));
        let mut payload = at + HEADER;
        if size == 1 {
            let Some(ext) = src.at(at + 8, 8) else { break };
            size = be64(&ext, 0).unwrap_or(0);
            payload += 8;
        } else if size == 0 {
            size = end - at;
        }
        if size < payload - at || at + size > end {
            break;
        }
        out.push((h[4..8].try_into().unwrap(), (payload, at + size)));
        at += size;
    }
    out
}

/// The first direct child box named `name`.
fn child(src: &mut Source, range: Range, name: &[u8; 4]) -> Option<Range> {
    children(src, range.0, range.1)
        .into_iter()
        .find(|(kind, _)| kind == name)
        .map(|(_, r)| r)
}

/// Follow a path of nested boxes from `range`.
fn descend(src: &mut Source, mut range: Range, path: &[&[u8; 4]]) -> Option<Range> {
    for name in path {
        range = child(src, range, name)?;
    }
    Some(range)
}

/// `mdhd` duration in seconds: version 0 uses 32-bit timescale and
/// duration fields, version 1 widens the duration to 64 bits.
fn media_duration(src: &mut Source, mdhd: Range) -> Option<f64> {
    let d = src.at(mdhd.0, 32.min((mdhd.1 - mdhd.0) as usize))?;
    match d.first()? {
        0 => {
            let timescale = be32(&d, 12)?;
            let duration = be32(&d, 16)?;
            seconds(f64::from(duration), timescale)
        }
        1 => {
            let timescale = be32(&d, 20)?;
            let duration = be64(&d, 24)?;
            seconds(duration as f64, timescale)
        }
        _ => None,
    }
}

/// The average bitrate in an `esds` `DecoderConfigDescriptor`, or 0.
fn esds_bitrate(data: &[u8]) -> u32 {
    // Full box header, then a nest of MPEG-4 descriptors (each a tag byte
    // and a length in 7-bit groups).
    let mut p = 4;
    walk_descriptors(data, &mut p, data.len()).unwrap_or(0)
}

fn walk_descriptors(data: &[u8], p: &mut usize, end: usize) -> Option<u32> {
    while *p < end {
        let tag = *data.get(*p)?;
        *p += 1;
        let mut len = 0usize;
        for _ in 0..4 {
            let b = *data.get(*p)?;
            *p += 1;
            len = (len << 7) | (b & 0x7f) as usize;
            if b & 0x80 == 0 {
                break;
            }
        }
        let body = *p;
        let next = body.checked_add(len)?;
        if next > end {
            return None;
        }
        match tag {
            // DecoderConfigDescriptor: object type, stream type, buffer
            // size, max bitrate, average bitrate.
            0x04 => return be32(data, body + 9),
            // ES_Descriptor: ES_ID, flags and optional fields, then its
            // child descriptors.
            0x03 => {
                let flags = *data.get(body + 2)?;
                let mut child = body + 3;
                if flags & 0x80 != 0 {
                    child += 2;
                }
                if flags & 0x40 != 0 {
                    child += 1 + *data.get(child)? as usize;
                }
                if flags & 0x20 != 0 {
                    child += 2;
                }
                *p = child;
                if let Some(bitrate) = walk_descriptors(data, p, next) {
                    return Some(bitrate);
                }
                *p = next;
            }
            _ => *p = next,
        }
    }
    None
}

/// The ALAC magic cookie's average bitrate.
fn alac_bitrate(data: &[u8]) -> Option<u32> {
    // Full box header, then the 16-byte fixed part before avgBitRate.
    be32(data, 4 + 16)
}

/// The bitrate of the first audio sample entry.
fn sample_entry_bitrate(src: &mut Source, stsd: Range) -> u32 {
    if stsd.1 - stsd.0 < 16 {
        return 0;
    }
    let Some((kind, (start, end))) = children(src, stsd.0 + 8, stsd.1).into_iter().next() else {
        return 0;
    };
    if kind == *b"mp4a" {
        // Audio sample entry fields end 28 bytes after the box payload
        // begins; the codec's boxes follow.
        if let Some(esds) = child(src, (start + 28, end), b"esds")
            && let Some(data) = src.at(esds.0, (esds.1 - esds.0) as usize)
        {
            return esds_bitrate(&data);
        }
        if let Some((_, wave)) = children(src, start + 28, end)
            .into_iter()
            .find(|(k, _)| k == b"wave")
            && let Some(esds) = child(src, wave, b"esds")
            && let Some(data) = src.at(esds.0, (esds.1 - esds.0) as usize)
        {
            return esds_bitrate(&data);
        }
        0
    } else if kind == *b"alac" {
        if let Some(alac) = child(src, (start + 28, end), b"alac")
            && let Some(data) = src.at(alac.0, (alac.1 - alac.0) as usize)
        {
            return alac_bitrate(&data).unwrap_or(0);
        }
        0
    } else {
        0
    }
}

pub fn parse(src: &mut Source) -> Option<Info> {
    let top = children(src, 0, src.len);
    let moov = top
        .into_iter()
        .find(|(kind, _)| kind == b"moov")
        .map(|(_, r)| r)?;
    for (kind, trak) in children(src, moov.0, moov.1) {
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(src, trak, b"mdia") else {
            continue;
        };
        let Some(hdlr) = child(src, mdia, b"hdlr") else {
            continue;
        };
        // hdlr: version/flags (4), pre_defined (4), handler_type (4).
        let Some(h) = src.at(hdlr.0, 12) else {
            continue;
        };
        if &h[8..12] != b"soun" {
            continue;
        }
        let length = child(src, mdia, b"mdhd").and_then(|m| media_duration(src, m))?;
        let bitrate = i64::from(
            descend(src, mdia, &[b"minf", b"stbl", b"stsd"])
                .map_or(0, |stsd| sample_entry_bitrate(src, stsd)),
        );
        return Some(Info { length, bitrate });
    }
    None
}
