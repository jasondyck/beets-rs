//! FLAC: the STREAMINFO metadata block from the FLAC format
//! specification (sample rate, channel count, sample size and total
//! sample count). FLAC doesn't record a bitrate.

use super::{Info, Source, seconds};

const STREAMINFO_LEN: usize = 34;

pub fn parse(src: &mut Source) -> Option<Info> {
    if src.at(0, 4)?.as_slice() != b"fLaC" {
        return None;
    }
    // The marker, then the metadata blocks; STREAMINFO is the first one
    // and the audio frames follow the last block of the chain.
    let mut at = 4u64;
    let mut streaminfo = None;
    loop {
        let block = src.at(at, 4)?;
        let last = block[0] & 0x80 != 0;
        let kind = block[0] & 0x7f;
        let len = (u64::from(block[1]) << 16) | (u64::from(block[2]) << 8) | u64::from(block[3]);
        if kind == 0 && len as usize >= STREAMINFO_LEN {
            let d = src.at(at + 4, STREAMINFO_LEN)?;
            let packed = u64::from_be_bytes(d[10..18].try_into().ok()?);
            streaminfo = Some(((packed >> 44) as u32, packed & 0x0000_000f_ffff_ffff));
        }
        at += 4 + len;
        if last {
            break;
        }
    }
    let (sample_rate, total_samples) = streaminfo?;
    let length = seconds(total_samples as f64, sample_rate)?;
    // The stream bitrate covers the audio frames, not the metadata.
    let bitrate = if length > 0.0 {
        ((src.len - at) as f64 * 8.0 / length) as i64
    } else {
        0
    };
    Some(Info { length, bitrate })
}
