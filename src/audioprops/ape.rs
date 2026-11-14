//! Monkey's Audio: the "MAC " file header. Version 3.98 and later store
//! the block sizes, frame count and sample rate in the header; older
//! headers use a fixed block size per encoder generation and keep the
//! sample rate and frame count at other offsets. Monkey's Audio has no
//! bitrate field.
//!
//! Layout as documented by the Monkey's Audio format specification and
//! the encoder's header structure.

use super::{Info, Source, le16, le32, seconds};

/// Fixed blocks per frame for the pre-3.98 header revisions.
fn legacy_blocks_per_frame(version: u16) -> Option<u32> {
    Some(match version {
        ..3900 => 9216,
        3900..3950 => 73728,
        3950..3980 => 294912,
        _ => return None,
    })
}

pub fn parse(src: &mut Source) -> Option<Info> {
    let h = src.at(0, 80)?;
    if &h[0..4] != b"MAC " {
        return None;
    }
    let version = le16(&h, 4)?;
    let (frames, blocks_per_frame, final_frame_blocks, sample_rate) = if version >= 3980 {
        let blocks_per_frame = le32(&h, 56)?;
        let final_frame_blocks = le32(&h, 60)?;
        let frames = le32(&h, 64)?;
        let sample_rate = le32(&h, 72)?;
        (frames, blocks_per_frame, final_frame_blocks, sample_rate)
    } else {
        let blocks_per_frame = legacy_blocks_per_frame(version)?;
        let sample_rate = le32(&h, 12)?;
        let frames = le32(&h, 24)?;
        let final_frame_blocks = le32(&h, 28)?;
        (frames, blocks_per_frame, final_frame_blocks, sample_rate)
    };
    // A stream of no blocks has no length; the frame count includes the
    // final, partially filled block. The multiplication is done wide so a
    // crafted header's 32-bit fields can't overflow.
    let samples =
        (i128::from(frames) - 1) * i128::from(blocks_per_frame) + i128::from(final_frame_blocks);
    Some(Info {
        length: seconds(samples.max(0) as f64, sample_rate)?,
        bitrate: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_block_sizes() {
        assert_eq!(legacy_blocks_per_frame(3899), Some(9216));
        assert_eq!(legacy_blocks_per_frame(3900), Some(73728));
        assert_eq!(legacy_blocks_per_frame(3949), Some(73728));
        assert_eq!(legacy_blocks_per_frame(3950), Some(294912));
        assert_eq!(legacy_blocks_per_frame(3980), None);
    }
}
