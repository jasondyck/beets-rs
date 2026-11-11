//! Audio length and bitrate exactly as beets stores them: mutagen's
//! `info.length` and `info.bitrate` per format, and MediaFile's fallback
//! for a bitrate the format doesn't give (`int(filesize * 8 / length)`,
//! the case for WavPack and Monkey's Audio).
//!
//! lofty rounds lengths to milliseconds and bitrates to kbps, so these are
//! read from the headers here; formats not covered keep lofty's values.

use lofty::file::FileType;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// mutagen's stream info: length in seconds and bitrate in bits per
/// second (0 when the format has none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    pub length: f64,
    pub bitrate: i64,
}

/// mutagen's MP3 encoder details, as MediaFile reports them: the
/// `bitrate_mode` ("CBR", "VBR", "ABR" or "" when unknown), `encoder_info`
/// (e.g. "LAME 3.100.0+", "FhG") and `encoder_settings` (a guess such as
/// "-V 4" or "-b 128"). Empty for other formats.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Encoder {
    pub bitrate_mode: &'static str,
    pub info: String,
    pub settings: String,
}

/// beets' `length` and `bitrate` for `path` and, for MP3s, the encoder
/// details; `None` when the format isn't covered or its headers can't be
/// parsed (use lofty's then).
pub fn read(path: &Path, ft: FileType) -> Option<(f64, i64, Encoder)> {
    let mut f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut enc = Encoder::default();
    let info = match ft {
        FileType::Mpeg => mpeg_full(&head(&mut f, 1 << 20)?, size).map(|(i, e)| {
            enc = e;
            i
        }),
        FileType::Flac => flac(&mut f, size),
        FileType::Mp4 => mp4(&mut f, size),
        FileType::Vorbis => ogg(&mut f, size, Ogg::Vorbis),
        FileType::Opus => ogg(&mut f, size, Ogg::Opus),
        FileType::WavPack => wavpack(&mut f),
        FileType::Ape => ape(&head(&mut f, 76)?),
        FileType::Wav => wav(&mut f, size),
        FileType::Aiff => aiff(&mut f, size),
        _ => None,
    }?;
    Some((info.length, mediafile_bitrate(info, size), enc))
}

/// MediaFile's `bitrate`: the format's own, else estimated from the size.
pub fn mediafile_bitrate(info: Info, filesize: u64) -> i64 {
    if info.bitrate != 0 {
        info.bitrate
    } else if info.length == 0.0 {
        0
    } else {
        (filesize as f64 * 8.0 / info.length) as i64
    }
}

fn head(f: &mut File, n: u64) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    f.seek(SeekFrom::Start(0)).ok()?;
    f.take(n).read_to_end(&mut data).ok()?;
    Some(data)
}

fn read_at(f: &mut File, off: u64, n: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0; n];
    f.seek(SeekFrom::Start(off)).ok()?;
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn le32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn le16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

// ------------------------------------------------------------------- MP3

/// mutagen's `MPEGInfo` for a layer III stream (`data`: the file's start;
/// `size`: the whole file's). The bitrate comes from the Xing/Info
/// header's byte and frame counts when there is one, else the first
/// frame's nominal bitrate; the length from the Xing frame count less
/// LAME's encoder delay and padding, a VBRI header's frame count, or else
/// the stream size at the nominal bitrate.
#[cfg(test)]
pub fn mpeg(data: &[u8], size: u64) -> Option<Info> {
    mpeg_full(data, size).map(|(i, _)| i)
}

/// `mpeg` with mutagen's encoder details (`_parse_vbr_header`).
pub fn mpeg_full(data: &[u8], size: u64) -> Option<(Info, Encoder)> {
    // Skip ID3v2 tags (there can be several).
    let mut pos = 0;
    while data.len() >= pos + 10 && &data[pos..pos + 3] == b"ID3" {
        let b = &data[pos + 6..pos + 10];
        let size = b
            .iter()
            .fold(0usize, |acc, &x| (acc << 7) | (x & 0x7f) as usize);
        let footer = if data[pos + 5] & 0x10 != 0 { 10 } else { 0 };
        pos += 10 + size + footer;
    }
    // The first plausible layer III frame header.
    let (start, h) = (pos..data.len().saturating_sub(4)).find_map(|i| {
        let h = be32(data, i)?;
        let valid = h >> 21 == 0x7ff
            && (h >> 19) & 3 != 1
            && (h >> 17) & 3 == 1
            && !matches!((h >> 12) & 0xf, 0 | 0xf)
            && (h >> 10) & 3 != 3;
        valid.then_some((i, h))
    })?;
    let version = (h >> 19) & 3; // 3: MPEG 1, 2: MPEG 2, 0: MPEG 2.5
    let mpeg1 = version == 3;
    const BR1: [i64; 15] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    const BR2: [i64; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    let idx = ((h >> 12) & 0xf) as usize;
    let bitrate = if mpeg1 { BR1[idx] } else { BR2[idx] } * 1000;
    let sample_rate = [44100, 48000, 32000][((h >> 10) & 3) as usize]
        / match version {
            3 => 1,
            2 => 2,
            _ => 4,
        };
    let padding = ((h >> 9) & 1) as i64;
    let mono = (h >> 6) & 3 == 3;
    let frame_size: i64 = if mpeg1 { 1152 } else { 576 };
    let frame_length = (frame_size / 8 * bitrate) / sample_rate + padding;
    // No VBR header: the length from the stream's size.
    let estimate = Info {
        length: 8.0 * size.saturating_sub(start as u64) as f64 / bitrate as f64,
        bitrate,
    };
    let off = start
        + match (mpeg1, mono) {
            (true, false) => 36,
            (true, true) | (false, false) => 21,
            (false, true) => 13,
        };
    // No (complete) Xing header: a VBRI header, else the estimate.
    let fallback = || match vbri(data, start + 36, frame_size, sample_rate) {
        Some(i) => (
            i,
            Encoder {
                bitrate_mode: "VBR",
                info: "FhG".into(),
                settings: String::new(),
            },
        ),
        None => (estimate, Encoder::default()),
    };
    let is_info = match data.get(off..off + 4) {
        Some(b"Xing") => false,
        Some(b"Info") => true,
        _ => return Some(fallback()),
    };
    let Some(flags) = be32(data, off + 4) else {
        return Some(fallback());
    };
    let mut p = off + 8;
    let mut field = |flag: u32, len: usize| -> Option<Option<i64>> {
        if flags & flag == 0 {
            return Some(None);
        }
        let v = if len == 4 { be32(data, p)? as i64 } else { 0 };
        data.get(p..p + len)?;
        p += len;
        Some(Some(v))
    };
    // A truncated Xing header counts as none.
    let Some((frames, bytes, vbr_scale)) = (|| {
        let frames = field(1, 4)?;
        let bytes = field(2, 4)?;
        field(4, 100)?; // TOC
        let scale = field(8, 4)?;
        Some((frames, bytes, scale))
    })() else {
        return Some(fallback());
    };
    let vbr_scale = vbr_scale.unwrap_or(-1);
    let lame = lame_version(data, p);
    let header = lame.as_ref().and_then(|l| {
        l.has_header
            .then(|| lame_header(data, p + 9, vbr_scale))
            .flatten()
    });
    let mut info = estimate;
    if let Some(frames) = frames {
        let mut samples = frames * frame_size;
        if let Some(bytes) = bytes
            && samples > 0
        {
            let audio_bytes = (bytes - frame_length).max(0);
            info.bitrate =
                ((audio_bytes * 8 * sample_rate) as f64 / samples as f64).round_ties_even() as i64;
        }
        if let Some(h) = &header {
            samples -= h.delay + h.padding;
        }
        info.length = samples.max(0) as f64 / sample_rate as f64;
    }
    // `_guess_xing_bitrate_mode`.
    let bitrate_mode = match header.as_ref().map(|h| h.vbr_method) {
        Some(1 | 8) => "CBR",
        Some(2 | 9) => "ABR",
        Some(3..=6) => "VBR",
        _ if is_info => "CBR",
        _ if vbr_scale != -1 || lame.as_ref().is_some_and(|l| !l.desc.is_empty()) => "VBR",
        _ => "",
    };
    let enc = Encoder {
        bitrate_mode,
        info: lame
            .as_ref()
            .map(|l| format!("LAME {}", l.desc))
            .unwrap_or_default(),
        settings: match (&header, &lame) {
            (Some(h), Some(l)) => h.guess_settings(l.major, l.minor),
            _ => String::new(),
        },
    };
    Some((info, enc))
}

/// mutagen's `LAMEHeader.parse_version`: the LAME version string after
/// the Xing header.
struct LameVersion {
    major: u32,
    minor: u32,
    desc: String,
    /// An extended LAME header follows (9 bytes into the version string).
    has_header: bool,
}

fn lame_version(data: &[u8], p: usize) -> Option<LameVersion> {
    let v = data.get(p..p + 20)?;
    if !(v.starts_with(b"LAME") || v.starts_with(b"L3.99")) {
        return None;
    }
    // `lstrip(b"EMAL")`.
    let rest = &v[v.iter().take_while(|c| b"EMAL".contains(c)).count()..];
    let major = (*rest.first()? as char).to_digit(10)?;
    let rest = &rest[1..];
    let rest = &rest[rest.iter().take_while(|&&c| c == b'.').count()..];
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    let minor: u32 = std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()?;
    let rest = &rest[digits..];
    let alpha_390 = rest.len() >= 11 && rest[rest.len() - 11] == b'(';
    if (major, minor) < (3, 90) || ((major, minor) == (3, 90) && alpha_390) {
        // `data.strip(b"\x00").rstrip()`, as ASCII.
        let flag = trim_bytes(rest, |c| c == 0);
        let flag = trim_end_bytes(flag, |c| c.is_ascii_whitespace());
        let flag = if flag.is_ascii() {
            String::from_utf8_lossy(flag).into_owned()
        } else {
            " (?)".into()
        };
        return Some(LameVersion {
            major,
            minor,
            desc: format!("{major}.{minor}{flag}"),
            has_header: false,
        });
    }
    if rest.len() < 11 {
        return None;
    }
    let flag = trim_end_bytes(&rest[..rest.len() - 11], |c| c == 0);
    let (patch, flag_string) = match flag {
        b"a" => ("", " (alpha)"),
        b"b" => ("", " (beta)"),
        b"r" => (".1+", ""),
        b" " if (major, minor) > (3, 96) => (".0", ""),
        b" " => (".0+", ""),
        b"" | b"." => (".0+", ""),
        _ => ("", " (?)"),
    };
    Some(LameVersion {
        major,
        minor,
        desc: format!("{major}.{minor}{patch}{flag_string}"),
        has_header: true,
    })
}

fn trim_bytes(b: &[u8], f: impl Fn(u8) -> bool + Copy) -> &[u8] {
    let start = b.iter().take_while(|&&c| f(c)).count();
    trim_end_bytes(&b[start..], f)
}

fn trim_end_bytes(b: &[u8], f: impl Fn(u8) -> bool) -> &[u8] {
    let end = b.len() - b.iter().rev().take_while(|&&c| f(c)).count();
    &b[..end]
}

/// The fields of mutagen's `LAMEHeader` beets uses.
struct LameHeader {
    vbr_method: u8,
    lowpass_filter: u32,
    quality: i64,
    vbr_quality: i64,
    encoding_flags: u8,
    ath_type: u8,
    bitrate: u32,
    delay: i64,
    padding: i64,
    preset_used: u32,
}

/// The extended LAME header at `h` (27 bytes); None for an unsupported
/// revision.
fn lame_header(data: &[u8], h: usize, vbr_scale: i64) -> Option<LameHeader> {
    let h = data.get(h..h + 27)?;
    if h[0] >> 4 != 0 {
        return None;
    }
    let d = u32::from_be_bytes([0, h[12], h[13], h[14]]);
    Some(LameHeader {
        vbr_method: h[0] & 0xf,
        lowpass_filter: h[1] as u32 * 100,
        quality: (100 - vbr_scale).rem_euclid(10),
        vbr_quality: (100 - vbr_scale).div_euclid(10),
        encoding_flags: h[10] >> 4,
        ath_type: h[10] & 0xf,
        bitrate: h[11] as u32,
        delay: (d >> 12) as i64,
        padding: (d & 0xfff) as i64,
        preset_used: ((h[17] as u32 & 0x07) << 8) | h[18] as u32,
    })
}

impl LameHeader {
    /// mutagen's `LAMEHeader.guess_settings`.
    fn guess_settings(&self, major: u32, minor: u32) -> String {
        let v = (major, minor);
        let old = matches!(v, (3, 90) | (3, 91) | (3, 92));
        let vq = self.vbr_quality;
        match self.vbr_method {
            2 => {
                if old && self.encoding_flags != 0 {
                    return if self.bitrate < 255 {
                        format!("--alt-preset {}", self.bitrate)
                    } else {
                        format!("--alt-preset {}+", self.bitrate)
                    };
                }
                return if self.preset_used != 0 {
                    format!("--preset {}", self.preset_used)
                } else if self.bitrate < 255 {
                    format!("--abr {}", self.bitrate)
                } else {
                    format!("--abr {}+", self.bitrate)
                };
            }
            1 => {
                return match self.preset_used {
                    0 if self.bitrate < 255 => format!("-b {}", self.bitrate),
                    0 => "-b 255+".into(),
                    1003 => "--preset insane".into(),
                    p => format!("-b {p}"),
                };
            }
            _ => {}
        }
        let m = self.vbr_method;
        if old {
            let key = (vq, self.quality, m, self.lowpass_filter, self.ath_type);
            match key {
                (1, 2, 4, 19500, 3) => return "--preset r3mix".into(),
                (2, 2, 3, 19000, 4) => return "--alt-preset standard".into(),
                (2, 2, 3, 19500, 2) => return "--alt-preset extreme".into(),
                _ => {}
            }
            match m {
                3 => return format!("-V {vq}"),
                4 | 5 => return format!("-V {vq} --vbr-new"),
                _ => {}
            }
        } else if matches!(v, (3, 93..=97)) {
            match self.preset_used {
                1001 => return "--preset standard".into(),
                1002 => return "--preset extreme".into(),
                1004 => return "--preset fast standard".into(),
                1005 => return "--preset fast extreme".into(),
                1006 => return "--preset medium".into(),
                1007 => return "--preset fast medium".into(),
                _ => {}
            }
            match m {
                3 => return format!("-V {vq}"),
                4 | 5 => return format!("-V {vq} --vbr-new"),
                _ => {}
            }
        } else if v == (3, 98) {
            match m {
                3 => return format!("-V {vq} --vbr-old"),
                4 | 5 => return format!("-V {vq}"),
                _ => {}
            }
        } else if v >= (3, 99) {
            match m {
                3 => return format!("-V {vq} --vbr-old"),
                4 | 5 => {
                    // https://sourceforge.net/p/lame/bugs/455/
                    let p = match (vq, self.bitrate, self.lowpass_filter) {
                        (5, 32, 0) => 7,
                        (5, 8, 0) => 8,
                        (6, 8, 0) => 9,
                        _ => vq,
                    };
                    return format!("-V {p}");
                }
                _ => {}
            }
        }
        String::new()
    }
}

/// mutagen's VBRI header (Fraunhofer) at `off`.
fn vbri(data: &[u8], off: usize, frame_size: i64, sample_rate: i64) -> Option<Info> {
    let d = data.get(off..off + 26)?;
    if &d[..4] != b"VBRI" || u16::from_be_bytes([d[4], d[5]]) != 1 {
        return None;
    }
    let bytes = be32(d, 10)? as i64;
    let frames = be32(d, 14)? as i64;
    let entries = u16::from_be_bytes([d[18], d[19]]) as usize;
    let entry_size = u16::from_be_bytes([d[22], d[23]]) as usize;
    if !matches!(entry_size, 2 | 4) {
        return None;
    }
    data.get(off + 26..off + 26 + entries * entry_size)?;
    let length = (frame_size * frames) as f64 / sample_rate as f64;
    let bitrate = if length != 0.0 {
        ((bytes * 8) as f64 / length) as i64
    } else {
        0
    };
    Some(Info { length, bitrate })
}

// ------------------------------------------------------------------ FLAC

/// mutagen's `FLAC`: total samples over the sample rate; the bitrate from
/// the size of what follows the metadata blocks.
fn flac(f: &mut File, size: u64) -> Option<Info> {
    let mut off = 0u64;
    // mutagen skips ID3v2 tags in front of the stream.
    loop {
        let h = read_at(f, off, 10)?;
        if &h[..3] != b"ID3" {
            break;
        }
        let size = h[6..10]
            .iter()
            .fold(0u64, |acc, &x| (acc << 7) | (x & 0x7f) as u64);
        off += 10 + size;
    }
    let d = read_at(f, off, 4 + 4 + 34)?;
    if &d[..4] != b"fLaC" || d[4] & 0x7f != 0 {
        return None;
    }
    let s = &d[8..];
    let rate = ((s[10] as u64) << 12) | ((s[11] as u64) << 4) | (s[12] as u64 >> 4);
    if rate == 0 {
        return None;
    }
    let total = s[13..18].iter().fold(0u64, |acc, &x| (acc << 8) | x as u64) & 0xF_FFFF_FFFF;
    let length = total as f64 / rate as f64;
    // Walk the metadata blocks to where the audio starts.
    let mut pos = off + 4;
    loop {
        let h = read_at(f, pos, 4)?;
        pos += 4 + u32::from_be_bytes([0, h[1], h[2], h[3]]) as u64;
        if h[0] & 0x80 != 0 {
            break;
        }
    }
    let bitrate = if length != 0.0 {
        (size.saturating_sub(pos) as f64 * 8.0 / length) as i64
    } else {
        0
    };
    Some(Info { length, bitrate })
}

// ------------------------------------------------------------------- MP4

/// The boxes in `[start, end)`: (type, data start, data end).
fn boxes(f: &mut File, start: u64, end: u64) -> Vec<([u8; 4], u64, u64)> {
    let mut out = vec![];
    let mut pos = start;
    while pos + 8 <= end {
        let Some(h) = read_at(f, pos, 8) else { break };
        let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        let mut header = 8;
        if size == 1 {
            let Some(l) = read_at(f, pos + 8, 8) else {
                break;
            };
            size = u64::from_be_bytes(l.try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < header || pos + size > end {
            break;
        }
        out.push((h[4..8].try_into().unwrap(), pos + header, pos + size));
        pos += size;
    }
    out
}

fn child(f: &mut File, (s, e): (u64, u64), name: &[u8; 4]) -> Option<(u64, u64)> {
    boxes(f, s, e)
        .into_iter()
        .find(|(n, ..)| n == name)
        .map(|(_, s, e)| (s, e))
}

/// The box at the end of a path of nested boxes.
fn descend(f: &mut File, mut range: (u64, u64), names: &[&[u8; 4]]) -> Option<(u64, u64)> {
    for name in names {
        range = child(f, range, name)?;
    }
    Some(range)
}

/// mutagen's `MP4Info`: the first sound track's `mdhd` duration over its
/// time scale, and the bitrate its sample entry gives (`esds` average
/// bitrate, ALAC cookie, AC-3 rate code).
fn mp4(f: &mut File, size: u64) -> Option<Info> {
    let moov = child(f, (0, size), b"moov")?;
    let trak = boxes(f, moov.0, moov.1).into_iter().find_map(|(n, s, e)| {
        if &n != b"trak" {
            return None;
        }
        let hdlr = descend(f, (s, e), &[b"mdia", b"hdlr"])?;
        let d = read_at(f, hdlr.0, 12)?;
        (&d[8..12] == b"soun").then_some((s, e))
    });
    let Some(trak) = trak else {
        // No sound track: mutagen falls back to the movie's length.
        let mvhd = child(f, moov, b"mvhd")?;
        let length = mdhd_length(&read_at(f, mvhd.0, (mvhd.1 - mvhd.0) as usize)?)?;
        return Some(Info { length, bitrate: 0 });
    };
    let mdia = child(f, trak, b"mdia")?;
    let mdhd = child(f, mdia, b"mdhd")?;
    let length = mdhd_length(&read_at(f, mdhd.0, (mdhd.1 - mdhd.0) as usize)?)?;
    let bitrate = (|| {
        let stsd = descend(f, mdia, &[b"minf", b"stbl", b"stsd"])?;
        let d = read_at(f, stsd.0, (stsd.1 - stsd.0) as usize)?;
        sample_entry_bitrate(&d)
    })()
    .unwrap_or(0);
    Some(Info { length, bitrate })
}

/// `mdhd`/`mvhd`: duration / time scale (version 0 or 1).
fn mdhd_length(d: &[u8]) -> Option<f64> {
    let (unit, length) = match d.first()? {
        0 => (be32(d, 12)? as u64, be32(d, 16)? as u64),
        1 => (
            be32(d, 20)? as u64,
            u64::from_be_bytes(d.get(24..32)?.try_into().ok()?),
        ),
        _ => return None,
    };
    Some(if unit == 0 {
        0.0
    } else {
        length as f64 / unit as f64
    })
}

/// The bitrate of the first entry of an `stsd` box (its data).
fn sample_entry_bitrate(d: &[u8]) -> Option<i64> {
    if d[0] != 0 || be32(d, 4)? == 0 {
        return None;
    }
    let entry = d.get(8..)?;
    let size = be32(entry, 0)? as usize;
    let kind = entry.get(4..8)?;
    let entry = entry.get(8..size.min(entry.len()))?;
    // 28 bytes of AudioSampleEntry, then the codec's box.
    let extra = entry.get(28..)?;
    let esize = (be32(extra, 0)? as usize).min(extra.len());
    let ekind = extra.get(4..8)?;
    let body = extra.get(8..esize)?;
    match (kind, ekind) {
        (b"mp4a", b"esds") => esds_bitrate(body),
        (b"alac", b"alac") => {
            // Full box header, frameLength (4), compatibleVersion (1)...
            if body[0] != 0 || *body.get(8)? != 0 {
                return Some(0);
            }
            Some(be32(body, 20)? as i64)
        }
        (b"ac-3", b"dac3") => {
            let code = (u32::from_be_bytes([0, body[0], body[1], body[2]]) >> 5) & 0x1f;
            const RATES: [i64; 19] = [
                32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576,
                640,
            ];
            Some(RATES.get(code as usize).map_or(0, |r| r * 1000))
        }
        _ => Some(0),
    }
}

/// An `esds` box's DecoderConfigDescriptor `avgBitrate`.
fn esds_bitrate(d: &[u8]) -> Option<i64> {
    if d[0] != 0 || *d.get(4)? != 3 {
        return None;
    }
    let mut p = 5;
    let mut desc_len = || -> Option<()> {
        for _ in 0..4 {
            let b = *d.get(p)?;
            p += 1;
            if b >> 7 == 0 {
                return Some(());
            }
        }
        None
    };
    desc_len()?;
    // ES_ID (2), flags (1) and what they announce.
    let flags = *d.get(p + 2)?;
    p += 3;
    if flags & 0x80 != 0 {
        p += 2;
    }
    if flags & 0x40 != 0 {
        p += 1 + *d.get(p)? as usize;
    }
    if flags & 0x20 != 0 {
        p += 2;
    }
    if *d.get(p)? != 4 {
        return None;
    }
    p += 1;
    let mut desc_len = || -> Option<()> {
        for _ in 0..4 {
            let b = *d.get(p)?;
            p += 1;
            if b >> 7 == 0 {
                return Some(());
            }
        }
        None
    };
    desc_len()?;
    // objectType (1), streamType (1), bufferSize (3), maxBitrate (4).
    Some(be32(d, p + 9)? as i64)
}

// ------------------------------------------------------------------- Ogg

#[derive(Clone, Copy, PartialEq)]
enum Ogg {
    Vorbis,
    Opus,
}

/// An Ogg page header at `off`: its granule position, serial, flags,
/// lacing values and total size.
struct Page {
    flags: u8,
    position: i64,
    serial: u32,
    lacing: Vec<u8>,
    end: u64,
}

impl Page {
    fn read(f: &mut File, off: u64) -> Option<Page> {
        let h = read_at(f, off, 27)?;
        if &h[..4] != b"OggS" || h[4] != 0 {
            return None;
        }
        let lacing = read_at(f, off + 27, h[26] as usize)?;
        let body: u64 = lacing.iter().map(|&l| l as u64).sum();
        Some(Page {
            flags: h[5],
            position: i64::from_le_bytes(h[6..14].try_into().ok()?),
            serial: le32(&h, 14)?,
            end: off + 27 + lacing.len() as u64 + body,
            lacing,
        })
    }

    fn body_start(&self) -> u64 {
        self.end - self.lacing.iter().map(|&l| l as u64).sum::<u64>()
    }

    /// mutagen's `complete`: the last packet ends on this page.
    fn complete(&self) -> bool {
        self.lacing.last().is_none_or(|&l| l < 255)
    }

    /// mutagen's `len(packets)`.
    fn packets(&self) -> usize {
        self.lacing.iter().filter(|&&l| l < 255).count() + usize::from(!self.complete())
    }

    fn last(&self) -> bool {
        self.flags & 4 != 0
    }
}

/// mutagen's `OggVorbisInfo` and `OggOpusInfo`.
fn ogg(f: &mut File, size: u64, kind: Ogg) -> Option<Info> {
    let magic: &[u8] = match kind {
        Ogg::Vorbis => b"\x01vorbis",
        Ogg::Opus => b"OpusHead",
    };
    // The identification header's page.
    let mut off = 0;
    let first = loop {
        let page = Page::read(f, off)?;
        let head = read_at(
            f,
            page.body_start(),
            28.min((page.end - page.body_start()) as usize),
        )?;
        if head.starts_with(magic) {
            break (page, head);
        }
        off = page.end;
    };
    let (page, head) = first;
    let serial = page.serial;
    let mut info = match kind {
        Ogg::Vorbis => {
            let rate = le32(&head, 12)? as i64;
            if rate == 0 || head.len() < 28 {
                return None;
            }
            let br = |at| le32(&head, at).map(|v| (v as i32).max(0) as i64);
            let (max, nominal, min) = (br(16)?, br(20)?, br(24)?);
            let bitrate = if nominal == 0 {
                (max + min) / 2
            } else if max != 0 && max < nominal {
                max
            } else if min > nominal {
                min
            } else {
                nominal
            };
            (rate, bitrate)
        }
        Ogg::Opus => (48000, 0),
    };
    let mut audio_size = 0;
    let pre_skip = if kind == Ogg::Opus {
        let pre_skip = le16(&head, 10)? as i64;
        // The comment header's pages: the audio is what follows them.
        let mut off = page.end;
        let mut tags = loop {
            let p = Page::read(f, off)?;
            off = p.end;
            if p.serial == serial && read_at(f, p.body_start(), 8)? == b"OpusTags" {
                break p;
            }
        };
        while !(tags.complete() || tags.packets() > 1) {
            let p = Page::read(f, off)?;
            off = p.end;
            if p.serial == serial {
                tags = p;
            }
        }
        audio_size = size - off;
        pre_skip
    } else {
        0
    };
    let last = find_last(f, size, serial)?;
    let length = (last.position - pre_skip) as f64 / info.0 as f64;
    if kind == Ogg::Opus {
        info.1 = if length != 0.0 {
            (audio_size as f64 * 8.0 / length).round_ties_even() as i64
        } else {
            0
        };
    }
    Some(Info {
        length,
        bitrate: info.1,
    })
}

/// mutagen's `OggPage.find_last(serial, finishing=True)`: the last page of
/// the stream with a packet ending on it.
fn find_last(f: &mut File, size: u64, serial: u32) -> Option<Page> {
    let valid = |p: &Page| p.position != -1;
    let tail_start = size.saturating_sub(256 * 256);
    let tail = read_at(f, tail_start, (size - tail_start) as usize)?;
    if let Some(i) = tail.windows(4).rposition(|w| w == b"OggS")
        && let Some(p) = Page::read(f, tail_start + i as u64)
        && p.serial == serial
        && valid(&p)
        && p.last()
    {
        return Some(p);
    }
    // Multiplexed (or no end-of-stream flag): walk the whole stream.
    let mut best = None;
    let mut off = 0;
    while let Some(p) = Page::read(f, off) {
        off = p.end;
        if p.serial == serial {
            let last = p.last();
            if valid(&p) {
                best = Some(p);
            }
            if last {
                break;
            }
        }
    }
    best
}

// ---------------------------------------------------------- WavPack, APE

/// mutagen's `WavPackInfo`: the first block's total samples (or the sum
/// of all blocks' when unknown) over the sample rate; no bitrate.
fn wavpack(f: &mut File) -> Option<Info> {
    const RATES: [i64; 15] = [
        6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200,
        96000, 192000,
    ];
    let header = |f: &mut File, off: u64| {
        let h = read_at(f, off, 32)?;
        (&h[..4] == b"wvpk").then_some(h)
    };
    let h = header(f, 0)?;
    let flags = le32(&h, 24)?;
    let mut rate = *RATES.get(((flags >> 23) & 0xf) as usize)?;
    if (flags >> 31) & 1 != 0 {
        rate *= 4;
    }
    let total = le32(&h, 12)?;
    let samples = if total == u32::MAX || le32(&h, 16)? != 0 {
        let mut samples = le32(&h, 20)? as i64;
        let mut off = 0u64;
        let mut h = h;
        loop {
            off += le32(&h, 4)? as u64 + 8;
            match header(f, off) {
                Some(next) => {
                    samples += le32(&next, 20)? as i64;
                    h = next;
                }
                None => break,
            }
        }
        samples
    } else {
        total as i64
    };
    Some(Info {
        length: samples as f64 / rate as f64,
        bitrate: 0,
    })
}

/// mutagen's `MonkeysAudioInfo`: total blocks over the sample rate.
pub fn ape(h: &[u8]) -> Option<Info> {
    if h.len() < 76 || &h[..4] != b"MAC " {
        return None;
    }
    let version = le16(h, 4)?;
    let (blocks_per_frame, final_blocks, total_frames, rate) = if version >= 3980 {
        (le32(h, 56)?, le32(h, 60)?, le32(h, 64)?, le32(h, 72)?)
    } else {
        let level = le16(h, 6)?;
        let bpf = if version >= 3950 {
            73728 * 4
        } else if version >= 3900 || (version >= 3800 && level == 4) {
            73728
        } else {
            9216
        };
        (bpf, le32(h, 28)?, le32(h, 24)?, le32(h, 12)?)
    };
    let length = if rate != 0 && total_frames > 0 {
        ((total_frames as u64 - 1) * blocks_per_frame as u64 + final_blocks as u64) as f64
            / rate as f64
    } else {
        0.0
    };
    Some(Info { length, bitrate: 0 })
}

// ------------------------------------------------------------ WAV, AIFF

/// The chunks of a RIFF (little-endian sizes) or IFF (big-endian) file
/// after its 12-byte header: (id, data offset, data size).
fn iff_chunks(f: &mut File, size: u64, le: bool) -> Vec<([u8; 4], u64, u64)> {
    let mut out = vec![];
    let mut pos = 12;
    while pos + 8 <= size {
        let Some(h) = read_at(f, pos, 8) else { break };
        let n: [u8; 4] = h[4..8].try_into().unwrap();
        let len = if le {
            u32::from_le_bytes(n)
        } else {
            u32::from_be_bytes(n)
        } as u64;
        out.push((h[..4].try_into().unwrap(), pos + 8, len));
        pos += 8 + len + (len & 1);
    }
    out
}

/// mutagen's `WaveStreamInfo`: channels x bits x rate; data size over the
/// block alignment over the rate.
fn wav(f: &mut File, size: u64) -> Option<Info> {
    let h = read_at(f, 0, 12)?;
    if &h[..4] != b"RIFF" || &h[8..12] != b"WAVE" {
        return None;
    }
    let chunks = iff_chunks(f, size, true);
    let id = |c: &([u8; 4], u64, u64)| c.0.to_ascii_lowercase();
    let fmt = chunks.iter().find(|c| &id(c) == b"fmt ")?;
    let d = read_at(f, fmt.1, 16)?;
    let channels = le16(&d, 2)? as i64;
    let rate = le32(&d, 4)? as i64;
    let block_align = le16(&d, 12)? as f64;
    let bits = le16(&d, 14)? as i64;
    let samples = match chunks.iter().find(|c| &id(c) == b"data") {
        Some(data) if block_align > 0.0 => data.2 as f64 / block_align,
        _ => 0.0,
    };
    Some(Info {
        length: if rate > 0 { samples / rate as f64 } else { 0.0 },
        bitrate: channels * bits * rate,
    })
}

/// mutagen's `AIFFInfo`: frames over the rate; channels x bits x rate.
fn aiff(f: &mut File, size: u64) -> Option<Info> {
    let h = read_at(f, 0, 12)?;
    if &h[..4] != b"FORM" {
        return None;
    }
    let comm = iff_chunks(f, size, false)
        .into_iter()
        .find(|c| &c.0 == b"COMM")?;
    let d = read_at(f, comm.1, 18)?;
    let channels = i16::from_be_bytes([d[0], d[1]]) as i64;
    let frames = be32(&d, 2)? as f64;
    let bits = i16::from_be_bytes([d[6], d[7]]) as i64;
    let rate = ieee_extended(d[8..18].try_into().ok()?)? as i64;
    Some(Info {
        length: if rate != 0 { frames / rate as f64 } else { 0.0 },
        bitrate: channels * bits * rate,
    })
}

/// An 80-bit IEEE 754 extended float (mutagen's `read_float`).
fn ieee_extended(b: [u8; 10]) -> Option<f64> {
    let exp = (((b[0] & 0x7f) as i32) << 8) | b[1] as i32;
    let mant = u64::from_be_bytes(b[2..10].try_into().ok()?);
    if exp == 0 && mant == 0 {
        return Some(0.0);
    }
    if exp == 0x7fff {
        return None;
    }
    let v = mant as f64 * 2f64.powi(exp - 16383 - 63);
    Some(if b[0] & 0x80 != 0 { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal MPEG 1 layer III stream: an ID3v2 tag, then one 128 kbps
    /// 44.1 kHz stereo frame, with an Info header when `xing` gives its
    /// frame and byte counts, and a LAME header when `lame` gives the
    /// encoder delay and padding.
    fn mpeg_data(xing: Option<(u32, u32)>, lame: Option<(u32, u32)>) -> Vec<u8> {
        let mut d = b"ID3\x04\x00\x00\x00\x00\x00\x0a".to_vec();
        d.extend([0u8; 10]);
        let mut frame = vec![0xff, 0xfb, 0x90, 0x00];
        frame.resize(417, 0);
        if let Some((frames, bytes)) = xing {
            frame[36..40].copy_from_slice(b"Info");
            frame[40..44].copy_from_slice(&3u32.to_be_bytes());
            frame[44..48].copy_from_slice(&frames.to_be_bytes());
            frame[48..52].copy_from_slice(&bytes.to_be_bytes());
            if let Some((delay, pad)) = lame {
                frame[52..61].copy_from_slice(b"LAME3.100");
                let v = (delay << 12) | pad;
                frame[52 + 9 + 12..52 + 9 + 15].copy_from_slice(&v.to_be_bytes()[1..]);
            }
        }
        d.extend(frame);
        d
    }

    // mutagen's figures: (bytes - first frame) * 8 * rate / (frames * 1152),
    // rounded half to even; frames * 1152 less LAME's delay and padding
    // over the rate; without a Xing/Info header, the frame's own bitrate
    // and the stream size at that rate.
    /// mutagen's `LAMEHeader.parse_version` (expected values from mutagen).
    #[test]
    fn lame_versions_as_mutagen() {
        type Want = Option<(u32, u32, &'static str, bool)>;
        let cases: &[(&[u8], Want)] = &[
            (b"LAME3.99r", Some((3, 99, "3.99.1+", true))),
            (b"LAME3.100", Some((3, 100, "3.100.0+", true))),
            (b"LAME3.97 ", Some((3, 97, "3.97.0", true))),
            (b"LAME3.96 ", Some((3, 96, "3.96.0+", true))),
            (b"LAME3.90a", Some((3, 90, "3.90 (alpha)", true))),
            (b"LAME3.92b", Some((3, 92, "3.92 (beta)", true))),
            (b"L3.99r", Some((3, 99, "3.99.1+", true))),
            (b"LAME3.88 (beta)", Some((3, 88, "3.88 (beta)", false))),
            (b"LAME3.90 (alpha)", Some((3, 90, "3.90 (alpha)", false))),
            (b"LAME3.98.", Some((3, 98, "3.98.0+", true))),
            (b"LAME3.93x", Some((3, 93, "3.93 (?)", true))),
            (b"LAME4.0 ", Some((4, 0, "4.0.0", true))),
            (b"Lavc61.3", None),
        ];
        for (s, want) in cases {
            let mut d = s.to_vec();
            d.resize(20, 0);
            let got = lame_version(&d, 0).map(|v| (v.major, v.minor, v.desc, v.has_header));
            let want = want.map(|(a, b, c, h)| (a, b, c.to_string(), h));
            assert_eq!(got, want, "{}", String::from_utf8_lossy(s));
        }
    }

    /// mutagen's `LAMEHeader.guess_settings` (expected values from mutagen).
    #[test]
    fn lame_settings_as_mutagen() {
        let h = |vbr_method, preset_used, vbr_quality, bitrate, lowpass_filter, ath_type, flags| {
            LameHeader {
                vbr_method,
                lowpass_filter,
                quality: 2,
                vbr_quality,
                encoding_flags: flags,
                ath_type,
                bitrate,
                delay: 0,
                padding: 0,
                preset_used,
            }
        };
        for (hdr, (major, minor), want) in [
            (h(4, 1001, 2, 0, 0, 0, 0), (3, 97), "--preset standard"),
            (h(4, 0, 1, 0, 19500, 3, 0), (3, 90), "--preset r3mix"),
            (h(2, 0, 1, 255, 19500, 3, 1), (3, 91), "--alt-preset 255+"),
            (h(4, 0, 5, 32, 0, 3, 1), (3, 99), "-V 7"),
            (h(5, 0, 3, 32, 0, 3, 1), (3, 92), "-V 3 --vbr-new"),
            (h(3, 0, 3, 32, 0, 3, 1), (3, 98), "-V 3 --vbr-old"),
            (h(1, 0, 3, 255, 0, 3, 1), (3, 99), "-b 255+"),
            (h(3, 0, 3, 255, 0, 3, 1), (3, 89), ""),
        ] {
            assert_eq!(hdr.guess_settings(major, minor), want);
        }
    }

    #[test]
    fn mpeg_as_mutagen() {
        let d = mpeg_data(None, None);
        let info = mpeg(&d, d.len() as u64 + 1000).unwrap();
        assert_eq!(info.bitrate, 128_000);
        assert_eq!(info.length, 8.0 * (417.0 + 1000.0) / 128_000.0);
        // 40 frames of 417 bytes after the Info frame.
        let bytes = 41 * 417;
        // 40 * 417 * 8 * 44100 / (40 * 1152) = 127706.25
        let info = mpeg(&mpeg_data(Some((40, bytes)), None), 0).unwrap();
        assert_eq!(info.bitrate, 127_706);
        assert_eq!(info.length, 46080.0 / 44100.0);
        let info = mpeg(&mpeg_data(Some((40, bytes)), Some((576, 1000))), 0).unwrap();
        assert_eq!(info.length, (46080.0 - 1576.0) / 44100.0);
        assert_eq!(mpeg(b"not audio", 9), None);
    }

    #[test]
    fn ape_header() {
        let mut h = vec![0u8; 76];
        h[..4].copy_from_slice(b"MAC ");
        h[4..6].copy_from_slice(&3990u16.to_le_bytes());
        h[56..60].copy_from_slice(&73728u32.to_le_bytes());
        h[60..64].copy_from_slice(&1000u32.to_le_bytes());
        h[64..68].copy_from_slice(&3u32.to_le_bytes());
        h[72..76].copy_from_slice(&44100u32.to_le_bytes());
        let info = ape(&h).unwrap();
        assert_eq!(info.length, (2.0 * 73728.0 + 1000.0) / 44100.0);
        assert_eq!(mediafile_bitrate(info, 1000), (8000.0 / info.length) as i64);
    }

    #[test]
    fn extended_float() {
        // 44100 as an 80-bit float.
        let b = [0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0];
        assert_eq!(ieee_extended(b), Some(44100.0));
    }
}
