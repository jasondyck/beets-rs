//! MPEG audio (MP3): the first frame header, an optional Xing/Info VBR
//! header and the LAME extension tag.
//!
//! Frame headers follow ISO/IEC 11172-3 (MPEG-1 audio) and ISO/IEC
//! 13818-3 (MPEG-2 audio); the Xing/Info layout and the LAME tag fields
//! are described by the Xing header and "Mp3 Info Tag revision 1"
//! documents. ID3v2 tags (ID3v2.3/2.4 specification) in front of the
//! stream are skipped.

use super::{Encoder, Info, Source, be32};

/// MPEG audio version from the frame header.
#[derive(Clone, Copy, PartialEq)]
enum Version {
    Mpeg1,
    Mpeg2,
    Mpeg25,
}

impl Version {
    fn side_info_len(self, mono: bool) -> u64 {
        match (self, mono) {
            (Version::Mpeg1, false) => 32,
            (Version::Mpeg1, true) => 17,
            (_, false) => 17,
            (_, true) => 9,
        }
    }

    /// Samples per frame for layer III (the only layer MP3 files use).
    fn layer3_samples(self) -> u32 {
        match self {
            Version::Mpeg1 => 1152,
            _ => 576,
        }
    }

    /// The layer III frame size in bytes (ISO/IEC 11172-3 §2.4.2.3).
    fn layer3_frame_size(self, bitrate: u32, sample_rate: u32, padding: u32) -> u64 {
        let coefficient = if self == Version::Mpeg1 { 144 } else { 72 };
        (coefficient * u64::from(bitrate) / u64::from(sample_rate)) + u64::from(padding)
    }
}

struct Header {
    version: Version,
    layer: u8,
    bitrate: u32,
    sample_rate: u32,
    mono: bool,
    /// Bytes in this frame; the Xing/Info frame is included.
    frame_size: u64,
    samples: u32,
}

const MPEG1_LAYER3_BITRATES: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const MPEG2_LAYER3_BITRATES: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];
const MPEG1_SAMPLE_RATES: [u32; 3] = [44100, 48000, 32000];
const MPEG2_SAMPLE_RATES: [u32; 3] = [22050, 24000, 16000];
const MPEG25_SAMPLE_RATES: [u32; 3] = [11025, 12000, 8000];

impl Header {
    /// Decode the four-byte frame header at `d[at..]`.
    fn parse(d: &[u8], at: usize) -> Option<Self> {
        let b = d.get(at..at + 4)?;
        if b[0] != 0xff || b[1] & 0xe0 != 0xe0 {
            return None;
        }
        let version = match (b[1] >> 3) & 3 {
            3 => Version::Mpeg1,
            2 => Version::Mpeg2,
            0 => Version::Mpeg25,
            _ => return None,
        };
        let layer = (b[1] >> 1) & 3; // 1 = layer III, 2 = layer II, 3 = layer I
        if layer == 0 {
            return None;
        }
        let bitrate_index = (b[2] >> 4) as usize;
        let sample_rate_index = ((b[2] >> 2) & 3) as usize;
        if bitrate_index == 0 || bitrate_index == 15 || sample_rate_index == 3 {
            return None;
        }
        let bitrate = match version {
            Version::Mpeg1 => MPEG1_LAYER3_BITRATES[bitrate_index],
            _ => MPEG2_LAYER3_BITRATES[bitrate_index],
        } * 1000;
        let sample_rate = match version {
            Version::Mpeg1 => MPEG1_SAMPLE_RATES,
            Version::Mpeg2 => MPEG2_SAMPLE_RATES,
            Version::Mpeg25 => MPEG25_SAMPLE_RATES,
        }[sample_rate_index];
        let padding = u32::from((b[2] >> 1) & 1);
        let mono = b[3] >> 6 == 3;
        let (frame_size, samples) = match (version, layer) {
            // Layer I: 384 samples, four-byte slots.
            (v, 3) => {
                let c = if v == Version::Mpeg1 { 12 } else { 6 };
                (
                    ((c * u64::from(bitrate) / u64::from(sample_rate)) + u64::from(padding)) * 4,
                    384,
                )
            }
            // Layer II: 1152 samples per frame in every version.
            (_, 2) => (
                144 * u64::from(bitrate) / u64::from(sample_rate) + u64::from(padding),
                1152,
            ),
            (v, _) => (
                v.layer3_frame_size(bitrate, sample_rate, padding),
                v.layer3_samples(),
            ),
        };
        Some(Self {
            version,
            layer,
            bitrate,
            sample_rate,
            mono,
            frame_size,
            samples,
        })
    }
}

/// The offset just past any leading ID3v2 tags (ID3v2.3/2.4 §3.1: a
/// ten-byte header with a 28-bit synchsafe size, plus an optional footer).
fn skip_id3v2(d: &[u8]) -> Option<usize> {
    let mut at = 0;
    while d.get(at..at + 3) == Some(b"ID3") {
        let flags = *d.get(at + 5)?;
        let size = d.get(at + 6..at + 10)?;
        let size = ((size[0] as usize) << 21)
            | ((size[1] as usize) << 14)
            | ((size[2] as usize) << 7)
            | size[3] as usize;
        at += 10 + size + if flags & 0x10 != 0 { 10 } else { 0 };
    }
    (at <= d.len()).then_some(at)
}

/// Find the first frame header at or after `from`. A header is accepted
/// when a Xing/Info header follows it or the next frame starts where this
/// one ends, so stray 0xff bytes in tags don't win.
fn find_frame(d: &[u8], from: usize) -> Option<usize> {
    for at in from..d.len().saturating_sub(4) {
        let Some(header) = Header::parse(d, at) else {
            continue;
        };
        let side = header.version.side_info_len(header.mono) as usize;
        let xing = d.get(at + 4 + side..at + 8 + side);
        let next = at + header.frame_size as usize;
        let next_ok = d
            .get(next..next + 2)
            .is_none_or(|b| b[0] == 0xff && b[1] & 0xe0 == 0xe0);
        if xing == Some(b"Xing") || xing == Some(b"Info") || next_ok {
            return Some(at);
        }
    }
    None
}

/// A Xing/Info VBR header: the frame and byte counts and the encoder
/// quality field.
struct Vbr {
    /// Whether the magic was "Info" (CBR) rather than "Xing" (VBR).
    info: bool,
    frames: u32,
    bytes: u32,
    quality: Option<u32>,
}

fn vbr_header(d: &[u8], at: usize) -> Option<Vbr> {
    let id = d.get(at..at + 4)?;
    let info = match id {
        b"Xing" => false,
        b"Info" => true,
        _ => return None,
    };
    let flags = be32(d, at + 4)?;
    let mut p = at + 8;
    let mut frames = 0;
    let mut bytes = 0;
    if flags & 1 != 0 {
        frames = be32(d, p)?;
        p += 4;
    }
    if flags & 2 != 0 {
        bytes = be32(d, p)?;
        p += 4;
    }
    if flags & 4 != 0 {
        p += 100;
    }
    let quality = if flags & 8 != 0 {
        Some(be32(d, p)?)
    } else {
        None
    };
    Some(Vbr {
        info,
        frames,
        bytes,
        quality,
    })
}

/// A parsed LAME extension tag ("Mp3 Info Tag revision 1").
struct Lame {
    version: LameVersion,
    vbr_method: u8,
    lowpass: u8,
    ath_type: u8,
    bitrate: u8,
    delay: u32,
    padding: u32,
    preset: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LameVersion {
    pub major: u32,
    pub minor: u32,
    /// The version description after "LAME ", e.g. "3.100.0+".
    pub desc: String,
    /// Whether this version's tag uses the LAME field layout (older
    /// 3.8x encoders wrote a string that only identifies them).
    pub header: bool,
}

fn lame_tag(d: &[u8], at: usize) -> Option<Lame> {
    let tag = d.get(at..at + 36)?;
    let version = lame_version(&tag[..9])?;
    let preset = u16::from_be_bytes([tag[26], tag[27]]);
    let dp = (u32::from(tag[21]) << 16) | (u32::from(tag[22]) << 8) | u32::from(tag[23]);
    Some(Lame {
        version,
        vbr_method: tag[9] & 0x0f,
        lowpass: tag[10],
        ath_type: tag[19] & 0x0f,
        bitrate: tag[20],
        delay: dp >> 12,
        padding: dp & 0xfff,
        preset: preset & 0x7ff,
    })
}

/// Recognise the encoder version string at the start of a LAME tag.
///
/// The 9-byte field starts with "LAME" (older encoders wrote "LAME" or
/// "L3."); a space-padded version means the stable release, while a
/// letter or punctuation suffix marks a prerelease. The parenthesised
/// forms are the pre-3.90 names and don't have the tag fields.
pub fn lame_version(s: &[u8]) -> Option<LameVersion> {
    let s = s.split(|&b| b == 0).next().unwrap_or(s);
    let rest = match s.strip_prefix(b"LAME") {
        Some(r) => r,
        None => s.strip_prefix(b"L")?,
    };
    if !rest.first()?.is_ascii_digit() {
        return None;
    }
    let text = std::str::from_utf8(rest).ok()?;
    let (major, after_major) = leading_number(text, usize::MAX);
    let after_dot = after_major.strip_prefix('.')?;
    let (minor, suffix) = leading_number(after_dot, 3);
    if major == 0 || after_dot.is_empty() {
        return None;
    }
    let base = format!("{major}.{minor}");
    // Releases whose minor version is known to write the tag fields.
    let known = (major >= 4 && minor <= 100) || (major == 3 && matches!(minor, 90..=999));
    if suffix.starts_with(" (") {
        // Pre-3.90 encoders spell out "(alpha)"/"(beta)" after a space.
        return Some(LameVersion {
            major,
            minor,
            desc: format!("{base}{suffix}"),
            header: false,
        });
    }
    let (desc, header) = if !known {
        (format!("{base} (?)"), false)
    } else {
        let desc = match suffix.chars().next() {
            Some('r') => format!("{base}.1+"),
            Some('a') => format!("{base} (alpha)"),
            Some('b') => format!("{base} (beta)"),
            Some('.') => format!("{base}.0+"),
            Some(c) if c.is_ascii_digit() => format!("{base}.0+"),
            Some(' ') | None => {
                // 3.90..3.96 and 3.100+ mark the released encoder with a
                // trailing "+"; 3.97..3.99 and 4.x do not.
                let plus = major == 3 && !matches!(minor, 97..=99);
                format!("{base}.0{}", if plus { "+" } else { "" })
            }
            _ => format!("{base} (?)"),
        };
        (desc, true)
    };
    Some(LameVersion {
        major,
        minor,
        desc,
        header,
    })
}

/// Split a leading run of ASCII digits (at most `max_digits`) from the
/// rest.
fn leading_number(s: &str, max_digits: usize) -> (u32, &str) {
    let end = s
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len())
        .min(max_digits);
    (s[..end].parse().unwrap_or(0), &s[end..])
}

/// The `encoder_settings` guess: the LAME preset names, the bitrate for
/// CBR/ABR, or the `-V` level from the Xing quality field.
fn guess_settings(lame: &Lame, quality: u32) -> String {
    let (major, minor) = (lame.version.major, lame.version.minor);
    let bitrate = if lame.preset != 0 {
        u32::from(lame.preset)
    } else {
        u32::from(lame.bitrate)
    };
    let suffix_plus = if bitrate == 255 { "+" } else { "" };
    match lame.vbr_method {
        1 => {
            if lame.preset == 1003 {
                "--preset insane".into()
            } else {
                format!("-b {bitrate}{suffix_plus}")
            }
        }
        2 => {
            let new_names = (major, minor) >= (3, 96);
            if (major, minor) >= (3, 93) && lame.preset >= 1000
                || (major, minor) >= (3, 94) && (8..=320).contains(&lame.preset)
            {
                format!("--preset {}", lame.preset)
            } else if new_names {
                format!("--abr {bitrate}{suffix_plus}")
            } else {
                format!("--alt-preset {bitrate}{suffix_plus}")
            }
        }
        3..=5 => {
            if let Some(name) = preset_name(lame.preset, lame.vbr_method, major, minor) {
                return name.into();
            }
            let mut level = 10 - (i64::from(quality) + 9) / 10;
            // LAME 3.99+ writes an automatic lowpass (0) and the minimum
            // bitrate in the tag's bitrate field for the low VBR levels;
            // those combinations identify V 9, V 8 and V 7.
            if lame.lowpass == 0 && (major, minor) >= (3, 99) && matches!(lame.vbr_method, 4 | 5) {
                match lame.bitrate {
                    8 if (31..=50).contains(&quality) => level += 3,
                    32 if (41..=50).contains(&quality) => level += 2,
                    _ => {}
                }
            }
            let level = level.clamp(0, 10);
            let mode = if (major, minor) >= (3, 98) {
                if lame.vbr_method == 3 {
                    " --vbr-old"
                } else {
                    ""
                }
            } else if lame.vbr_method == 3 {
                ""
            } else {
                " --vbr-new"
            };
            format!("-V {level}{mode}")
        }
        _ => String::new(),
    }
}

/// The named LAME presets written by 3.93..3.97.
fn preset_name(preset: u16, method: u8, major: u32, minor: u32) -> Option<&'static str> {
    if (major, minor) < (3, 93) || (major, minor) >= (3, 98) {
        return None;
    }
    let name = match (preset, method) {
        (1001, 2..=5) => "--preset standard",
        (1002, 2..=5) => "--preset extreme",
        (1004, 2..=5) => "--preset fast standard",
        (1005, 2..=5) => "--preset fast extreme",
        (1006, 2..=5) => "--preset medium",
        (1007, 2..=5) => "--preset fast medium",
        _ => return None,
    };
    Some(name)
}

/// The bitrate mode from the LAME VBR method, falling back to the
/// Xing/Info header's magic when the tag has no method.
fn bitrate_mode(method: u8, info: bool) -> &'static str {
    match method {
        0 => {
            if info {
                "CBR"
            } else {
                "VBR"
            }
        }
        1 | 8 => "CBR",
        2 | 9 => "ABR",
        _ => "VBR",
    }
}

/// `-V` settings for the "r3mix" preset, which LAME 3.90..3.92 wrote with
/// quality 88, a 19.5 kHz lowpass and ATH type 3.
fn is_r3mix(lame: &Lame, quality: u32) -> bool {
    let (major, minor) = (lame.version.major, lame.version.minor);
    lame.version.header
        && (major, minor) >= (3, 90)
        && (major, minor) < (3, 93)
        && lame.vbr_method == 4
        && quality == 88
        && lame.lowpass == 195
        && lame.ath_type == 3
}

pub fn parse(src: &mut Source) -> Option<(Info, Encoder)> {
    let head = src.head(1 << 20)?;
    let after_tags = skip_id3v2(&head)?;
    let frame_at = find_frame(&head, after_tags)?;
    let header = Header::parse(&head, frame_at)?;
    let mut enc = Encoder::default();
    let xing_at = frame_at + 4 + header.version.side_info_len(header.mono) as usize;
    let info = match vbr_header(&head, xing_at) {
        Some(vbr) => {
            let lame = lame_tag(&head, xing_at + 120);
            let recognized = lame.as_ref().is_some_and(|l| l.version.header);
            let quality = vbr.quality.unwrap_or(0);
            if let Some(lame) = &lame {
                enc.info = format!("LAME {}", lame.version.desc);
                enc.bitrate_mode = bitrate_mode(lame.vbr_method, vbr.info);
                if recognized {
                    enc.settings = if is_r3mix(lame, quality) {
                        "--preset r3mix".into()
                    } else {
                        guess_settings(lame, quality)
                    };
                }
            } else {
                enc.bitrate_mode = if vbr.info { "CBR" } else { "VBR" };
            }
            let (delay, padding) = match (&lame, recognized) {
                (Some(l), true) => (l.delay, l.padding),
                _ => (0, 0),
            };
            let length = if vbr.frames > 0 {
                let samples =
                    f64::from(vbr.frames) * f64::from(header.samples) - f64::from(delay + padding);
                seconds_positive(samples, header.sample_rate)
            } else {
                0.0
            };
            let bitrate = if vbr.frames > 0 && vbr.bytes > 0 {
                xing_bitrate(&vbr, &header)
            } else if vbr.frames > 0 {
                0
            } else {
                i64::from(header.bitrate)
            };
            Info { length, bitrate }
        }
        None => {
            enc.bitrate_mode = "";
            // No VBR header: the nominal first-frame bitrate and the rest
            // of the file (after the ID3v2 tag) at that rate.
            let bitrate = i64::from(header.bitrate);
            let length = if bitrate > 0 {
                (src.len - frame_at as u64) as f64 * 8.0 / bitrate as f64
            } else {
                0.0
            };
            Info { length, bitrate }
        }
    };
    // Layer I/II streams aren't MP3s for beets' encoder fields.
    if header.layer != 1 {
        enc = Encoder::default();
    }
    Some((info, enc))
}

fn seconds_positive(samples: f64, sample_rate: u32) -> f64 {
    super::seconds(samples.max(0.0), sample_rate).unwrap_or(0.0)
}

/// The Xing bitrate: the audio bytes (the header frame is not audio) at
/// the stream's frame rate, rounded half to even.
fn xing_bitrate(vbr: &Vbr, header: &Header) -> i64 {
    let audio_bytes = f64::from(vbr.bytes) - header.frame_size as f64;
    let seconds = f64::from(vbr.frames) * f64::from(header.samples) / f64::from(header.sample_rate);
    (audio_bytes * 8.0 / seconds).round_ties_even() as i64
}
