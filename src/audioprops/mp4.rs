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
/// Real files have a handful of boxes; stop at a number no sane file
/// reaches so a file full of tiny boxes can't build a huge vector.
const MAX_BOXES: usize = 100_000;
/// The codec boxes we read (`esds`, `alac`) are tiny; their declared size
/// must not make us allocate more than this.
const MAX_CODEC_BOX: u64 = 64 * 1024;
/// A real `esds` nests an `ES_Descriptor` and a
/// `DecoderConfigDescriptor`; a crafted one must not recurse deeply.
const MAX_DESCRIPTOR_DEPTH: u32 = 8;

/// The child boxes of the box payload `start..end`.
fn children(src: &mut Source, start: u64, end: u64) -> Vec<([u8; 4], Range)> {
    let mut out = Vec::new();
    let mut at = start;
    loop {
        if out.len() >= MAX_BOXES {
            break;
        }
        let Some(remaining) = end.checked_sub(at) else {
            break;
        };
        if remaining < HEADER {
            break;
        }
        let Some(h) = src.at(at, 8) else { break };
        let declared = u64::from(be32(&h, 0).unwrap_or(0));
        let (size, header) = match declared {
            // A zero size means "to the end of the parent box".
            0 => (remaining, HEADER),
            // The 64-bit size follows the normal header.
            1 => {
                let Some(ext_at) = at.checked_add(HEADER) else {
                    break;
                };
                let Some(ext) = src.at(ext_at, 8) else { break };
                (be64(&ext, 0).unwrap_or(0), HEADER + 8)
            }
            size => (size, HEADER),
        };
        if size < header {
            break;
        }
        let Some(payload) = at.checked_add(header) else {
            break;
        };
        let Some(next) = at.checked_add(size) else {
            break;
        };
        if next > end || next <= at {
            break;
        }
        out.push((h[4..8].try_into().unwrap(), (payload, next)));
        at = next;
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
    walk_descriptors(data, &mut p, data.len(), 0).unwrap_or(0)
}

fn walk_descriptors(data: &[u8], p: &mut usize, end: usize, depth: u32) -> Option<u32> {
    if depth > MAX_DESCRIPTOR_DEPTH {
        return None;
    }
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
            0x04 => return be32(data, body.checked_add(9)?),
            // ES_Descriptor: ES_ID, flags and optional fields, then its
            // child descriptors.
            0x03 => {
                let flags = *data.get(body.checked_add(2)?)?;
                let mut child = body.checked_add(3)?;
                if flags & 0x80 != 0 {
                    child = child.checked_add(2)?;
                }
                if flags & 0x40 != 0 {
                    let url_len = *data.get(child)? as usize;
                    child = child.checked_add(1)?.checked_add(url_len)?;
                }
                if flags & 0x20 != 0 {
                    child = child.checked_add(2)?;
                }
                *p = child;
                if let Some(bitrate) = walk_descriptors(data, p, next, depth + 1) {
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
    let Some(children_at) = stsd.0.checked_add(8) else {
        return 0;
    };
    let Some((kind, (start, end))) = children(src, children_at, stsd.1).into_iter().next() else {
        return 0;
    };
    let Some(codec_at) = start.checked_add(28) else {
        return 0;
    };
    if kind == *b"mp4a" {
        // Audio sample entry fields end 28 bytes after the box payload
        // begins; the codec's boxes follow.
        if let Some(esds) = child(src, (codec_at, end), b"esds")
            && let Some(data) = read_codec_box(src, esds)
        {
            return esds_bitrate(&data);
        }
        if let Some((_, wave)) = children(src, codec_at, end)
            .into_iter()
            .find(|(k, _)| k == b"wave")
            && let Some(esds) = child(src, wave, b"esds")
            && let Some(data) = read_codec_box(src, esds)
        {
            return esds_bitrate(&data);
        }
        0
    } else if kind == *b"alac" {
        if let Some(alac) = child(src, (codec_at, end), b"alac")
            && let Some(data) = read_codec_box(src, alac)
        {
            return alac_bitrate(&data).unwrap_or(0);
        }
        0
    } else {
        0
    }
}

/// A codec configuration box's payload, capped: these hold a few dozen
/// bytes, so a declared size in the gigabytes must not be allocated.
fn read_codec_box(src: &mut Source, range: Range) -> Option<Vec<u8>> {
    let size = range.1.checked_sub(range.0)?.min(MAX_CODEC_BOX) as usize;
    src.at(range.0, size)
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
