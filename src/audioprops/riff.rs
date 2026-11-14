//! WAVE and AIFF: the format header's sample rate, channels and sample
//! size, and the audio chunk's frame count.
//!
//! RIFF WAVE is the Microsoft RIFF/WAVE format (`fmt ` and `data`
//! chunks); AIFF and AIFF-C are the Apple Audio Interchange File Format
//! (`COMM` chunk, including its 80-bit extended sample rate).

use super::{Info, Source, be16, be32, le16, le32, seconds};

/// Real files have few chunks; a malformed file full of empty ones must
/// not build an unbounded vector.
const MAX_CHUNKS: usize = 100_000;
/// Header chunks hold a few dozen bytes at most; a declared size in the
/// gigabytes must not be allocated.
const MAX_HEADER_CHUNK: u64 = 4096;
const MAX_COMM_CHUNK: u64 = 1024;

/// The chunks of a RIFF/IFF file payload, in file order.
fn chunks(src: &mut Source, start: u64, end: u64, little_endian: bool) -> Vec<([u8; 4], u64, u64)> {
    let mut out = Vec::new();
    let mut at = start;
    loop {
        if out.len() >= MAX_CHUNKS {
            break;
        }
        let Some(remaining) = end.checked_sub(at) else {
            break;
        };
        if remaining < 8 {
            break;
        }
        let Some(h) = src.at(at, 8) else { break };
        let kind: [u8; 4] = h[0..4].try_into().unwrap();
        let size = u64::from(
            if little_endian {
                le32(&h, 4)
            } else {
                be32(&h, 4)
            }
            .unwrap_or(0),
        );
        let Some(payload) = at.checked_add(8) else {
            break;
        };
        let Some(chunk_end) = payload.checked_add(size) else {
            break;
        };
        if chunk_end > end {
            break;
        }
        out.push((kind, payload, chunk_end));
        // Chunks are word aligned.
        let Some(next) = chunk_end.checked_add(size & 1) else {
            break;
        };
        if next <= at {
            break;
        }
        at = next;
    }
    out
}

/// A bitrate field that doesn't fit an `i64` (malformed headers only).
fn saturated_bitrate(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

/// An 80-bit IEEE 754 extended float, big-endian, as AIFF stores the
/// sample rate.
fn extended(b: &[u8]) -> Option<f64> {
    let b: [u8; 10] = b.try_into().ok()?;
    let sign = if b[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exponent = ((u32::from(b[0] & 0x7f) << 8) | u32::from(b[1])) as i32;
    let mantissa = u64::from_be_bytes(b[2..10].try_into().ok()?);
    if exponent == 0 && mantissa == 0 {
        return Some(0.0);
    }
    if exponent == 0x7fff {
        return Some(f64::INFINITY * sign);
    }
    Some(sign * mantissa as f64 * 2f64.powi(exponent - 16383 - 63))
}

/// RIFF WAVE: `fmt ` gives the byte rate and block alignment, `data` the
/// audio bytes.
pub fn parse_wave(src: &mut Source) -> Option<Info> {
    let head = src.at(0, 12)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let all = chunks(src, 12, src.len, true);
    let fmt = all.iter().find(|(k, _, _)| k == b"fmt ")?;
    let fmt_len = fmt.2.checked_sub(fmt.1)?.min(MAX_HEADER_CHUNK) as usize;
    let d = src.at(fmt.1, fmt_len)?;
    let channels = u32::from(le16(&d, 2)?);
    let sample_rate = le32(&d, 4)?;
    let block_align = u32::from(le16(&d, 12)?);
    let bits = u32::from(le16(&d, 14)?);
    let data = all.iter().find(|(k, _, _)| k == b"data")?;
    if block_align == 0 || sample_rate == 0 {
        return None;
    }
    let frames = (data.2 - data.1) as f64 / f64::from(block_align);
    let bitrate = i128::from(sample_rate) * i128::from(channels) * i128::from(bits);
    Some(Info {
        length: seconds(frames, sample_rate)?,
        bitrate: saturated_bitrate(bitrate),
    })
}

/// AIFF/AIFF-C: `COMM` holds channel count, frame count, sample size and
/// sample rate.
pub fn parse_aiff(src: &mut Source) -> Option<Info> {
    let head = src.at(0, 12)?;
    if &head[0..4] != b"FORM" || (&head[8..12] != b"AIFF" && &head[8..12] != b"AIFC") {
        return None;
    }
    let comm = chunks(src, 12, src.len, false)
        .into_iter()
        .find(|(k, _, _)| k == b"COMM")
        .map(|(_, start, end)| (start, end))?;
    let comm_len = comm.1.checked_sub(comm.0)?.min(MAX_COMM_CHUNK) as usize;
    let d = src.at(comm.0, comm_len)?;
    let channels = i64::from(be16(&d, 0)?);
    let frames = f64::from(be32(&d, 2)?);
    let bits = i64::from(be16(&d, 6)?);
    let sample_rate = extended(d.get(8..18)?)?;
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return None;
    }
    let bitrate = i128::from(sample_rate as i64) * i128::from(channels) * i128::from(bits);
    Some(Info {
        length: frames / sample_rate,
        bitrate: saturated_bitrate(bitrate),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_float_44100() {
        // 44100 as an 80-bit extended float.
        let b = [0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0];
        assert_eq!(extended(&b), Some(44100.0));
    }

    #[test]
    fn extended_float_zero_and_negative() {
        assert_eq!(extended(&[0; 10]), Some(0.0));
        // 22050 Hz, negative (not meaningful for a rate, but the field is
        // signed).
        let b = [0xc0, 0x0d, 0xac, 0x44, 0, 0, 0, 0, 0, 0];
        assert_eq!(extended(&b), Some(-22050.0));
    }
}
