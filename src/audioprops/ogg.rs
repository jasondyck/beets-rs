//! Ogg Vorbis and Ogg Opus: the sample rate and nominal bitrate from the
//! codec identification header, and the stream length from the last
//! page's granule position.
//!
//! Page framing and the granule position are RFC 3533; the Vorbis
//! identification header is the Vorbis I specification; Opus files use
//! the `OpusHead` header and 48 kHz granule clock of RFC 7845 (whose
//! pre-skip adjusts the length).

use super::{Info, Source, le16, le32};

#[derive(Clone, Copy)]
pub enum Codec {
    Vorbis,
    Opus,
}

const PAGE_HEADER: usize = 27;
/// Enough of the file's tail to hold the last page (Ogg pages may be up
/// to 65,307 bytes of body plus the segment table).
const TAIL: usize = 1 << 17;

struct Page {
    offset: u64,
    serial: u32,
    granule: u64,
    /// Bytes from the page start through the body.
    len: u64,
}

/// Decode the page header at `d[at..]`; `file_len` bounds the body.
fn page(d: &[u8], at: usize, file_len: u64) -> Option<Page> {
    let h = d.get(at..at + PAGE_HEADER)?;
    if &h[0..4] != b"OggS" || h[4] != 0 {
        return None;
    }
    let segments = h[26] as usize;
    let table = d.get(at + PAGE_HEADER..at + PAGE_HEADER + segments)?;
    let body: usize = table.iter().map(|&n| n as usize).sum();
    let len = (PAGE_HEADER + segments + body) as u64;
    if (at as u64).checked_add(len)? > file_len {
        return None;
    }
    Some(Page {
        offset: at as u64,
        serial: le32(h, 14)?,
        granule: u64::from_le_bytes(h[6..14].try_into().ok()?),
        len,
    })
}

/// The Ogg CRC-32 (RFC 3533 §6): polynomial 0x04c11db7, no reflection,
/// computed with the CRC field zeroed.
pub(crate) fn crc(page: &[u8]) -> u32 {
    let mut crc = 0u32;
    for (i, &b) in page.iter().enumerate() {
        let b = if (22..26).contains(&i) { 0 } else { b };
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn page_crc_ok(src: &mut Source, p: &Page) -> bool {
    let Some(bytes) = src.at(p.offset, p.len as usize) else {
        return false;
    };
    let wanted = le32(&bytes, 22).unwrap_or(0);
    crc(&bytes) == wanted
}

/// The last page of `serial`, preferring one whose checksum matches.
fn find_last(src: &mut Source, serial: u32) -> Option<Page> {
    let start = src.len.saturating_sub(TAIL as u64);
    let tail = src.at(start, (src.len - start) as usize)?;
    let mut fallback = None;
    for at in (0..tail.len().saturating_sub(4)).rev() {
        if &tail[at..at + 4] != b"OggS" {
            continue;
        }
        let Some(p) = page(&tail, at, src.len - start) else {
            continue;
        };
        if p.serial != serial || p.granule == u64::MAX {
            continue;
        }
        let mut p = p;
        p.offset += start;
        if page_crc_ok(src, &p) {
            return Some(p);
        }
        fallback.get_or_insert(p);
    }
    fallback
}

/// Where the audio pages start: just past the page that finishes the
/// second packet (the `OpusTags` comment header).
fn audio_data_offset(src: &mut Source) -> Option<u64> {
    let mut at = 0u64;
    let mut packets = 0usize;
    loop {
        let head = src.at(at, PAGE_HEADER)?;
        let nseg = head[PAGE_HEADER - 1] as usize;
        let table = src.at(at + PAGE_HEADER as u64, nseg)?;
        packets += table.iter().filter(|&&n| n < 255).count();
        let body: u64 = table.iter().map(|&n| u64::from(n)).sum();
        at += PAGE_HEADER as u64 + nseg as u64 + body;
        if at > src.len {
            return None;
        }
        if packets >= 2 {
            return Some(at);
        }
    }
}

pub fn parse(src: &mut Source, codec: Codec) -> Option<Info> {
    // The identification header is the first packet of the first page.
    let header = src.at(0, PAGE_HEADER)?;
    let nseg = header[PAGE_HEADER - 1] as usize;
    let head = src.up_to(0, PAGE_HEADER + nseg + 255)?;
    let first = page(&head, 0, src.len)?;
    let start = PAGE_HEADER + head[PAGE_HEADER - 1] as usize;
    let packet = head.get(start..)?;
    match codec {
        Codec::Vorbis => {
            if packet.get(0..7)? != b"\x01vorbis" {
                return None;
            }
            let sample_rate = le32(packet, 12)?;
            if sample_rate == 0 {
                return None;
            }
            let nominal = le32(packet, 20)? as i32;
            let last = find_last(src, first.serial)?;
            Some(Info {
                length: last.granule as f64 / f64::from(sample_rate),
                bitrate: i64::from(nominal.max(0)),
            })
        }
        Codec::Opus => {
            if packet.get(0..8)? != b"OpusHead" {
                return None;
            }
            let pre_skip = u64::from(le16(packet, 10)?);
            let last = find_last(src, first.serial)?;
            let samples = last.granule.saturating_sub(pre_skip);
            let length = samples as f64 / 48000.0;
            // Opus pools no nominal bitrate; the average is the audio
            // bytes after the two header packets (OpusHead and OpusTags)
            // at the stream's length.
            let audio = audio_data_offset(src)?;
            let bitrate = if length > 0.0 {
                ((src.len - audio) as f64 * 8.0 / length).round_ties_even() as i64
            } else {
                0
            };
            Some(Info { length, bitrate })
        }
    }
}
