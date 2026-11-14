//! Audio stream properties exactly as beets stores them: the length in
//! seconds and the bitrate in bits per second that beets reports for each
//! format, plus the MP3 encoder details.
//!
//! These are read from the container headers instead of lofty, which
//! rounds lengths to milliseconds and bitrates to kbps. Formats whose
//! stream info can't be read fall back to lofty's values in the caller.
//!
//! The parsers implement the published file-format specifications:
//!
//! - MPEG audio frame headers: ISO/IEC 11172-3 (MPEG-1) and ISO/IEC
//!   13818-3 (MPEG-2); the Xing/Info VBR header (as written by LAME and
//!   others); the LAME tag, "Mp3 Info Tag revision 1" (Gabriel Bouvigne);
//!   ID3v2.3/2.4 tag headers for skipping tags.
//! - FLAC: the FLAC format specification, STREAMINFO metadata block.
//! - MP4/M4A: ISO/IEC 14496-12 (file/box structure, `mdhd`, `stsd`) and
//!   ISO/IEC 14496-14 (MP4 audio sample entries, `esds`
//!   `DecoderConfigDescriptor`); the Apple Lossless "alac" magic cookie.
//! - Ogg: RFC 3533 (page framing, granule position), the Vorbis I
//!   specification (identification header), RFC 7845 (Ogg Opus,
//!   `OpusHead` pre-skip, 48 kHz granule clock).
//! - WavPack: the WavPack 4 and 5 block header (`wvpk`).
//! - Monkey's Audio: the "MAC " file header (version 3.x layouts and the
//!   3.98+ header).
//! - WAVE: the RIFF WAVE `fmt ` and `data` chunks.
//! - AIFF/AIFF-C: the `COMM` chunk, including the 80-bit extended
//!   sample rate.

mod ape;
mod flac;
mod mp4;
mod mpeg;
mod ogg;
mod riff;
mod wavpack;

use lofty::file::FileType;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A stream's length in seconds and bitrate in bits per second (0 when
/// the format gives none; the caller then estimates it from the size).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    pub length: f64,
    pub bitrate: i64,
}

/// The MP3 encoder details beets reports: `bitrate_mode` ("CBR", "VBR",
/// "ABR" or ""), `encoder_info` (e.g. "LAME 3.100.0+") and
/// `encoder_settings` (e.g. "-V 2"). Empty for other formats.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Encoder {
    pub bitrate_mode: &'static str,
    pub info: String,
    pub settings: String,
}

/// beets' `length` and `bitrate` for `path`, and for MP3s the encoder
/// details; `None` when the format isn't covered or its headers can't be
/// parsed.
pub fn read(path: &Path, ft: FileType) -> Option<(f64, i64, Encoder)> {
    let mut src = Source::open(path)?;
    let mut enc = Encoder::default();
    let info = match ft {
        FileType::Mpeg => {
            let (info, e) = mpeg::parse(&mut src)?;
            enc = e;
            info
        }
        FileType::Flac => flac::parse(&mut src)?,
        FileType::Mp4 => mp4::parse(&mut src)?,
        FileType::Vorbis => ogg::parse(&mut src, ogg::Codec::Vorbis)?,
        FileType::Opus => ogg::parse(&mut src, ogg::Codec::Opus)?,
        FileType::WavPack => wavpack::parse(&mut src)?,
        FileType::Ape => ape::parse(&mut src)?,
        FileType::Wav => riff::parse_wave(&mut src)?,
        FileType::Aiff => riff::parse_aiff(&mut src)?,
        _ => return None,
    };
    Some((info.length, mediafile_bitrate(info, src.len), enc))
}

/// MediaFile's `bitrate`: the format's own, else estimated from the file
/// size (`int(filesize * 8 / length)`), the case for lossless formats
/// that don't state a bitrate.
pub fn mediafile_bitrate(info: Info, filesize: u64) -> i64 {
    if info.bitrate != 0 {
        info.bitrate
    } else if info.length == 0.0 {
        0
    } else {
        (filesize as f64 * 8.0 / info.length) as i64
    }
}

/// The file-being-parsed: its handle and size, with byte-range helpers.
pub(crate) struct Source {
    file: File,
    pub len: u64,
}

impl Source {
    fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        Some(Self { file, len })
    }

    /// `n` bytes at `offset`, or `None` when the file is shorter.
    pub fn at(&mut self, offset: u64, n: usize) -> Option<Vec<u8>> {
        if offset.checked_add(n as u64)? > self.len {
            return None;
        }
        let mut buf = vec![0; n];
        self.file.seek(SeekFrom::Start(offset)).ok()?;
        self.file.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    /// Up to `n` bytes from `offset` (fewer at the end of the file).
    pub fn up_to(&mut self, offset: u64, n: usize) -> Option<Vec<u8>> {
        if offset > self.len {
            return None;
        }
        let want = (self.len - offset).min(n as u64) as usize;
        let mut buf = vec![0; want];
        self.file.seek(SeekFrom::Start(offset)).ok()?;
        self.file.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    /// The first `n` bytes, or the whole file if shorter.
    pub fn head(&mut self, n: usize) -> Option<Vec<u8>> {
        self.up_to(0, n)
    }
}

/// `N` bytes at `at`, with an overflow-checked end offset.
fn fixed<const N: usize>(d: &[u8], at: usize) -> Option<[u8; N]> {
    let end = at.checked_add(N)?;
    d.get(at..end)?.try_into().ok()
}

pub(crate) fn be16(d: &[u8], at: usize) -> Option<u16> {
    fixed(d, at).map(u16::from_be_bytes)
}

pub(crate) fn be32(d: &[u8], at: usize) -> Option<u32> {
    fixed(d, at).map(u32::from_be_bytes)
}

pub(crate) fn be64(d: &[u8], at: usize) -> Option<u64> {
    fixed(d, at).map(u64::from_be_bytes)
}

pub(crate) fn le16(d: &[u8], at: usize) -> Option<u16> {
    fixed(d, at).map(u16::from_le_bytes)
}

pub(crate) fn le32(d: &[u8], at: usize) -> Option<u32> {
    fixed(d, at).map(u32::from_le_bytes)
}

/// `length` is only meaningful with a positive sample rate.
pub(crate) fn seconds(samples: f64, sample_rate: u32) -> Option<f64> {
    (sample_rate != 0).then(|| samples / f64::from(sample_rate))
}

#[cfg(test)]
mod tests;
