//! WavPack: the block header (`wvpk`) holds the sample rate in its flags
//! and the stream length either as a total sample count or, when the
//! encoder didn't know it, as the sum of the blocks' sample counts.
//! WavPack has no bitrate field.

use super::{Info, Source, le32, seconds};

/// The WavPack sample-rate table, indexed by the four-bit rate field in
/// the block flags; entry 15 is invalid.
const SAMPLE_RATES: [u32; 15] = [
    6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000,
    192000,
];

const HEADER_LEN: u64 = 32;

pub fn parse(src: &mut Source) -> Option<Info> {
    let h = src.at(0, HEADER_LEN as usize)?;
    if &h[0..4] != b"wvpk" {
        return None;
    }
    let version = u16::from_le_bytes([h[8], h[9]]);
    let flags = le32(&h, 24)?;
    let rate_index = ((flags >> 23) & 0xf) as usize;
    let sample_rate = *SAMPLE_RATES.get(rate_index)?;
    // WavPack 5 stores the high eight bits of the total sample count in
    // the byte before the 32-bit field.
    let mut total = u64::from(le32(&h, 12)?);
    if version >= 0x0500 {
        total |= u64::from(h[11]) << 32;
    }
    if total == 0xffff_ffff || (version >= 0x0500 && total == 0x00ff_ffff_ffff) {
        total = sum_block_samples(src)?;
    }
    Some(Info {
        length: seconds(total as f64, sample_rate)?,
        bitrate: 0,
    })
}

/// A stream with an unknown length records it as the sum of the blocks'
/// sample counts. Real files have one block per fraction of a second; a
/// crafted file full of tiny blocks must not make us walk it forever.
const MAX_BLOCKS: u64 = 1_000_000;

/// Walk the block chain and add up the per-block sample counts.
fn sum_block_samples(src: &mut Source) -> Option<u64> {
    let mut total = 0u64;
    let mut at = 0u64;
    for _ in 0..MAX_BLOCKS {
        let Some(remaining) = src.len.checked_sub(at) else {
            break;
        };
        if remaining < HEADER_LEN {
            break;
        }
        let h = src.at(at, HEADER_LEN as usize)?;
        if &h[0..4] != b"wvpk" {
            break;
        }
        total = total.saturating_add(u64::from(le32(&h, 20)?));
        let size = u64::from(le32(&h, 4)?);
        let Some(next) = at.checked_add(8).and_then(|v| v.checked_add(size)) else {
            break;
        };
        if next <= at {
            break;
        }
        at = next;
    }
    Some(total)
}
