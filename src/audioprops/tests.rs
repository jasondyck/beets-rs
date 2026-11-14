//! Behaviour tests for the audio-property parsers. Each test builds a
//! minimal file with the header fields described by the format
//! specifications and checks the values beets reports for it.

use super::*;

/// Write `d` to a temporary file and run one parser over it.
fn read_bytes<T>(d: &[u8], f: impl FnOnce(&mut Source) -> Option<T>) -> Option<T> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a");
    std::fs::write(&path, d).unwrap();
    let mut src = Source::open(&path)?;
    f(&mut src)
}

// ------------------------------------------------------------------- MP3

/// LAME extension tag fields (the 36-byte tag after the Xing header).
#[derive(Clone)]
struct Lame {
    version: &'static str,
    vbr_method: u8,
    lowpass: u8,
    ath_type: u8,
    bitrate: u8,
    delay: u32,
    padding: u32,
    preset: u16,
}

impl Default for Lame {
    fn default() -> Self {
        Self {
            version: "LAME3.100",
            vbr_method: 1,
            lowpass: 170,
            ath_type: 3,
            bitrate: 128,
            delay: 576,
            padding: 1404,
            preset: 0,
        }
    }
}

impl Lame {
    fn bytes(&self) -> Vec<u8> {
        let mut t = vec![0u8; 36];
        let v = self.version.as_bytes();
        t[..v.len()].copy_from_slice(v);
        t[9] = self.vbr_method;
        t[10] = self.lowpass;
        t[19] = self.ath_type;
        t[20] = self.bitrate;
        let dp = (self.delay << 12) | self.padding;
        t[21..24].copy_from_slice(&dp.to_be_bytes()[1..]);
        t[26..28].copy_from_slice(&(self.preset + 0x1000).to_be_bytes());
        t
    }
}

/// A minimal MPEG-1 layer III stream: a 10-byte ID3v2 tag, one 128 kbps
/// 44.1 kHz mono frame with room for a bare Xing/Info header (no TOC or
/// quality field) and, at the fixed 120-byte offset, a LAME tag.
fn mp3(
    magic: &[u8; 4],
    frames: u32,
    bytes: u32,
    quality: Option<u32>,
    lame: Option<Lame>,
) -> Vec<u8> {
    let mut d = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
    let mut frame = vec![0xff, 0xfb, 0x90, 0xc0];
    frame.resize(417, 0);
    frame[21..25].copy_from_slice(magic);
    let flags: u32 = 0x03 | if quality.is_some() { 0x0c } else { 0 };
    frame[25..29].copy_from_slice(&flags.to_be_bytes());
    frame[29..33].copy_from_slice(&frames.to_be_bytes());
    frame[33..37].copy_from_slice(&bytes.to_be_bytes());
    if let Some(q) = quality {
        frame[137..141].copy_from_slice(&q.to_be_bytes());
    }
    if let Some(lame) = lame {
        frame[141..177].copy_from_slice(&lame.bytes());
    }
    d.extend(frame);
    d
}

fn read_mp3(d: &[u8]) -> Option<(f64, i64, Encoder)> {
    let (info, enc) = read_bytes(d, mpeg::parse)?;
    Some((info.length, info.bitrate, enc))
}

#[test]
fn mp3_without_vbr_header_uses_nominal_bitrate() {
    let d = mp3(b"Info", 0, 0, None, None);
    // Remove the Xing header again: no VBR header is found at all.
    let mut d = d;
    d[31..35].copy_from_slice(b"\0\0\0\0");
    d.truncate(10 + 417 + 1000);
    let (length, bitrate, enc) = read_mp3(&d).unwrap();
    assert_eq!(bitrate, 128_000);
    // Everything after the ID3v2 tag at the first frame's nominal rate.
    assert_eq!(length, (d.len() - 10) as f64 * 8.0 / 128_000.0);
    assert_eq!(enc, Encoder::default());
}

#[test]
fn mp3_info_header_bitrate_and_length() {
    let d = mp3(b"Info", 40, 41 * 417, None, None);
    let (length, bitrate, enc) = read_mp3(&d).unwrap();
    // (41 * 417 - 417) * 8 * 44100 / (40 * 1152), rounded half to even.
    assert_eq!(bitrate, 127_706);
    assert_eq!(length, 46080.0 / 44100.0);
    assert_eq!(enc.bitrate_mode, "CBR");
}

#[test]
fn mp3_xing_magic_means_vbr_without_lame_fields() {
    let d = mp3(b"Xing", 40, 41 * 417, None, None);
    let (_, _, enc) = read_mp3(&d).unwrap();
    assert_eq!(enc.bitrate_mode, "VBR");
    assert_eq!(enc.info, "");
}

#[test]
fn mp3_lame_delay_and_padding_shorten_the_length() {
    let lame = Lame {
        delay: 576,
        padding: 1000,
        ..Lame::default()
    };
    let d = mp3(b"Xing", 40, 41 * 417, None, Some(lame));
    let (length, bitrate, enc) = read_mp3(&d).unwrap();
    assert_eq!(length, (46080.0 - 1576.0) / 44100.0);
    assert_eq!(bitrate, 127_706);
    assert_eq!(enc.info, "LAME 3.100.0+");
}

#[test]
fn mp3_legacy_lame_version_has_no_fields() {
    // 3.8x wrote a version string but not the tag fields: no delay
    // adjustment, no settings, and the mode falls back to the magic.
    let lame = Lame {
        version: "LAME3.88 ",
        vbr_method: 4,
        delay: 576,
        padding: 1404,
        ..Lame::default()
    };
    let d = mp3(b"Xing", 40, 41 * 417, None, Some(lame));
    let (length, _, enc) = read_mp3(&d).unwrap();
    assert_eq!(length, 46080.0 / 44100.0);
    assert_eq!(enc.info, "LAME 3.88 (?)");
    assert_eq!(enc.settings, "");
    assert_eq!(enc.bitrate_mode, "VBR");
}

#[test]
fn mp3_lame_version_descriptions() {
    for (version, want) in [
        ("LAME3.99r", "LAME 3.99.1+"),
        ("LAME3.100", "LAME 3.100.0+"),
        ("LAME3.97 ", "LAME 3.97.0"),
        ("LAME3.96 ", "LAME 3.96.0+"),
        ("LAME3.90a", "LAME 3.90 (alpha)"),
        ("LAME3.92b", "LAME 3.92 (beta)"),
        ("L3.99r  ", "LAME 3.99.1+"),
        ("LAME3.98.", "LAME 3.98.0+"),
        ("LAME3.93x", "LAME 3.93 (?)"),
        ("LAME4.0 ", "LAME 4.0.0"),
        ("LAME3.85 ", "LAME 3.85 (?)"),
    ] {
        let got = mpeg::lame_version(version.as_bytes()).map(|v| format!("LAME {}", v.desc));
        assert_eq!(got.as_deref(), Some(want), "{version}");
    }
    assert!(mpeg::lame_version(b"Lavc61.3").is_none());
    assert!(mpeg::lame_version(b"LAME 3.9").is_none());
}

#[test]
fn mp3_bitrate_modes_from_the_lame_method() {
    for (method, want) in [
        (0, "CBR"),
        (1, "CBR"),
        (2, "ABR"),
        (3, "VBR"),
        (4, "VBR"),
        (5, "VBR"),
        (8, "CBR"),
        (9, "ABR"),
    ] {
        let lame = Lame {
            vbr_method: method,
            ..Lame::default()
        };
        let d = mp3(b"Info", 40, 41 * 417, None, Some(lame));
        let (_, _, enc) = read_mp3(&d).unwrap();
        assert_eq!(enc.bitrate_mode, want, "method {method}");
    }
}

#[test]
fn mp3_lame_encoder_settings() {
    struct Case {
        lame: Lame,
        quality: u32,
        want: &'static str,
    }
    let lowpass = |lowpass, vbr_method, version, quality, preset, bitrate, want| Case {
        lame: Lame {
            version,
            vbr_method,
            preset,
            bitrate,
            lowpass,
            ..Lame::default()
        },
        quality,
        want,
    };
    let case = |vbr_method, version, quality, preset, bitrate, want| Case {
        lame: Lame {
            version,
            vbr_method,
            preset,
            bitrate,
            ..Lame::default()
        },
        quality,
        want,
    };
    for c in [
        case(4, "LAME3.97 ", 30, 0, 0, "-V 7 --vbr-new"),
        case(4, "LAME3.99 ", 30, 0, 0, "-V 7"),
        case(3, "LAME3.99 ", 30, 0, 0, "-V 7 --vbr-old"),
        case(3, "LAME3.97 ", 30, 0, 0, "-V 7"),
        case(5, "LAME3.92 ", 30, 0, 0, "-V 7 --vbr-new"),
        case(1, "LAME3.99 ", 0, 0, 64, "-b 64"),
        case(1, "LAME3.99 ", 0, 0, 255, "-b 255+"),
        case(1, "LAME3.92 ", 0, 1003, 0, "--preset insane"),
        case(2, "LAME3.92 ", 0, 0, 255, "--alt-preset 255+"),
        case(2, "LAME3.96 ", 0, 0, 128, "--abr 128"),
        case(2, "LAME3.96 ", 0, 64, 0, "--preset 64"),
        case(4, "LAME3.96 ", 0, 1001, 0, "--preset standard"),
        case(4, "LAME3.96 ", 0, 1004, 0, "--preset fast standard"),
        case(5, "LAME3.96 ", 0, 1006, 0, "--preset medium"),
        case(4, "LAME3.99 ", 0, 1001, 0, "-V 10"),
        case(4, "LAME3.99 ", 55, 0, 0, "-V 4"),
        // LAME 3.99+ automatic lowpass at the lowest VBR levels.
        lowpass(0, 4, "LAME3.100", 40, 0, 8, "-V 9"),
        lowpass(0, 4, "LAME3.100", 50, 0, 8, "-V 8"),
        lowpass(0, 4, "LAME3.100", 50, 0, 32, "-V 7"),
        lowpass(0, 4, "LAME3.98 ", 40, 0, 8, "-V 6"),
    ] {
        let d = mp3(b"Xing", 40, 41 * 417, Some(c.quality), Some(c.lame));
        let (_, _, enc) = read_mp3(&d).unwrap();
        assert_eq!(enc.settings, c.want, "quality {}, {}", c.quality, c.want);
    }
}

#[test]
fn mp3_r3mix_preset_is_recognized_for_3_90_to_3_92() {
    for version in ["LAME3.90 ", "LAME3.92 "] {
        let lame = Lame {
            version,
            vbr_method: 4,
            lowpass: 195,
            ath_type: 3,
            ..Lame::default()
        };
        let d = mp3(b"Xing", 40, 41 * 417, Some(88), Some(lame));
        let (_, _, enc) = read_mp3(&d).unwrap();
        assert_eq!(enc.settings, "--preset r3mix", "{version}");
    }
    let lame = Lame {
        version: "LAME3.93 ",
        vbr_method: 4,
        lowpass: 195,
        ath_type: 3,
        ..Lame::default()
    };
    let d = mp3(b"Xing", 40, 41 * 417, Some(88), Some(lame));
    let (_, _, enc) = read_mp3(&d).unwrap();
    assert_eq!(enc.settings, "-V 1 --vbr-new");
}

#[test]
fn mp3_empty_xing_frame_uses_the_nominal_bitrate() {
    let (length, bitrate, _) = read_mp3(&mp3(b"Xing", 0, 0, None, None)).unwrap();
    assert_eq!((length, bitrate), (0.0, 128_000));
}

#[test]
fn mp3_xing_without_a_byte_count_has_no_bitrate() {
    let (_, bitrate, _) = read_mp3(&mp3(b"Xing", 40, 0, None, None)).unwrap();
    assert_eq!(bitrate, 0);
}

// ------------------------------------------------------------------ FLAC

/// A FLAC file: STREAMINFO with the given rate/channels/bits/samples, the
/// last metadata block, then `audio` bytes of frames.
fn flac(rate: u32, channels: u32, bits: u32, samples: u64, audio: usize) -> Vec<u8> {
    let mut d = b"fLaC".to_vec();
    d.extend([0x80, 0, 0, 34]); // last block, STREAMINFO
    d.extend([0x10, 0x00, 0x10, 0x00]); // min/max block size
    d.extend([0, 0, 0, 0, 0, 0]); // min/max frame size
    let packed = (u64::from(rate) << 44)
        | ((u64::from(channels) - 1) << 41)
        | ((u64::from(bits) - 1) << 36)
        | samples;
    d.extend(packed.to_be_bytes());
    d.extend([0u8; 16]); // MD5
    d.extend(vec![0u8; audio]);
    d
}

#[test]
fn flac_length_and_stream_bitrate() {
    // 2 s at 44.1 kHz, 2 channels, 16 bits, 100000 bytes of frames.
    let d = flac(44100, 2, 16, 88200, 100_000);
    let info = read_bytes(&d, flac::parse).unwrap();
    assert_eq!(info.length, 2.0);
    // The metadata (42 bytes) is not part of the stream bitrate.
    assert_eq!(info.bitrate, 400_000);
}

#[test]
fn flac_without_streaminfo_is_unreadable() {
    let d = b"fLaC\x81\0\0\x04abcd".to_vec(); // a last PADDING block only
    assert_eq!(read_bytes(&d, flac::parse), None);
}

// ------------------------------------------------------------------- MP4

fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend(((payload.len() + 8) as u32).to_be_bytes());
    d.extend(kind);
    d.extend(payload);
    d
}

fn fullbox(version: u8, payload: &[u8]) -> Vec<u8> {
    let mut d = vec![version, 0, 0, 0];
    d.extend(payload);
    d
}

/// An MP4 with one sound track; the sample entry is `kind` with `codec`
/// as its codec box. `mdhd` uses the given version's field widths.
fn mp4(kind: &[u8; 4], codec: &[u8], mdhd_version: u8, timescale: u32, duration: u64) -> Vec<u8> {
    let mdhd = mp4_box(
        b"mdhd",
        &if mdhd_version == 0 {
            fullbox(
                0,
                &[
                    &0u32.to_be_bytes()[..],
                    &0u32.to_be_bytes()[..],
                    &timescale.to_be_bytes()[..],
                    &(duration as u32).to_be_bytes()[..],
                    &[0x55, 0xc4, 0, 0][..],
                ]
                .concat(),
            )
        } else {
            fullbox(
                1,
                &[
                    &0u64.to_be_bytes()[..],
                    &0u64.to_be_bytes()[..],
                    &timescale.to_be_bytes()[..],
                    &duration.to_be_bytes()[..],
                    &[0x55, 0xc4, 0, 0][..],
                ]
                .concat(),
            )
        },
    );
    let hdlr = mp4_box(
        b"hdlr",
        &fullbox(0, &[&[0u8; 4][..], b"soun", b"Sound\0"].concat()),
    );
    // The audio sample entry is 28 bytes before its codec boxes.
    let mut entry = vec![0u8; 6];
    entry.extend(1u16.to_be_bytes()); // data reference index
    entry.extend([0u8; 8]);
    entry.extend(2u16.to_be_bytes()); // channel count
    entry.extend(16u16.to_be_bytes()); // sample size
    entry.extend(0u16.to_be_bytes()); // pre-defined
    entry.extend(0u16.to_be_bytes()); // reserved
    entry.extend(((timescale) << 16).to_be_bytes());
    entry.extend(codec);
    let stsd = mp4_box(
        b"stsd",
        &fullbox(
            0,
            &[&1u32.to_be_bytes()[..], &mp4_box(kind, &entry)[..]].concat(),
        ),
    );
    let stbl = mp4_box(b"stbl", &stsd);
    let minf = mp4_box(b"minf", &stbl);
    let mdia = mp4_box(b"mdia", &[mdhd, hdlr, minf].concat());
    let trak = mp4_box(b"trak", &mdia);
    let moov = mp4_box(b"moov", &trak);
    let mut d = mp4_box(b"ftyp", b"M4A \0\0\0\0M4A mp42isom");
    d.extend(moov);
    d
}

/// An `esds` box with an average bitrate.
fn esds(avg: u32) -> Vec<u8> {
    let mut dec = vec![0x04, 13, 0x40, 0x15, 0, 0, 0];
    dec.extend(0u32.to_be_bytes()); // max bitrate
    dec.extend(avg.to_be_bytes());
    let mut es = vec![0x03, (dec.len() + 3) as u8, 0, 1, 0];
    es.extend(dec);
    mp4_box(b"esds", &fullbox(0, &es))
}

#[test]
fn mp4_aac_duration_and_esds_bitrate() {
    let d = mp4(b"mp4a", &esds(128_000), 0, 44100, 88_200);
    let info = read_bytes(&d, mp4::parse).unwrap();
    assert_eq!(info.length, 2.0);
    assert_eq!(info.bitrate, 128_000);
}

#[test]
fn mp4_version_one_mdhd_and_alac_bitrate() {
    let mut alac = fullbox(0, &[0u8; 16]);
    alac.extend(191_250u32.to_be_bytes()); // avgBitRate
    alac.extend(44100u32.to_be_bytes()); // sample rate
    let d = mp4(b"alac", &mp4_box(b"alac", &alac), 1, 44100, 132_300);
    let info = read_bytes(&d, mp4::parse).unwrap();
    assert_eq!(info.length, 3.0);
    assert_eq!(info.bitrate, 191_250);
}

#[test]
fn mp4_without_a_sound_track_is_unreadable() {
    let hdlr = fullbox(0, &[&[0u8; 4][..], b"vide"].concat());
    let mdia = mp4_box(b"mdia", &hdlr);
    let trak = mp4_box(b"trak", &mdia);
    let moov = mp4_box(b"moov", &trak);
    assert_eq!(read_bytes(&moov, mp4::parse), None);
}

// ------------------------------------------------------------------- Ogg

/// One Ogg page with a checksum.
fn ogg_page(serial: u32, granule: u64, header_type: u8, body: &[u8]) -> Vec<u8> {
    assert!(body.len() < 255);
    let mut d = Vec::new();
    d.extend(b"OggS");
    d.push(0); // version
    d.push(header_type);
    d.extend(granule.to_le_bytes());
    d.extend(serial.to_le_bytes());
    d.extend(0u32.to_le_bytes()); // page sequence
    d.extend(0u32.to_le_bytes()); // checksum, filled below
    d.push(1); // one segment
    d.push(body.len() as u8);
    d.extend(body);
    let sum = ogg::crc(&d);
    d[22..26].copy_from_slice(&sum.to_le_bytes());
    d
}

fn vorbis_id(rate: u32, nominal: i32) -> Vec<u8> {
    let mut d = b"\x01vorbis".to_vec();
    d.extend(0u32.to_le_bytes()); // version
    d.push(1); // channels
    d.extend(rate.to_le_bytes());
    d.extend(0i32.to_le_bytes()); // max
    d.extend(nominal.to_le_bytes());
    d.extend(0i32.to_le_bytes()); // min
    d.push(1); // framing
    d
}

fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut d = b"OpusHead".to_vec();
    d.push(1); // version
    d.push(1); // channels
    d.extend(pre_skip.to_le_bytes());
    d.extend(44100u32.to_le_bytes());
    d.extend(0i16.to_le_bytes());
    d.push(0); // mapping family
    d
}

#[test]
fn vorbis_length_from_last_granule_and_nominal_bitrate() {
    let mut d = ogg_page(7, 0, 2, &vorbis_id(44100, 128_000));
    d.extend(ogg_page(7, 44100, 0, &[0u8; 100]));
    d.extend(ogg_page(7, 88200, 4, &[0u8; 50]));
    let info = read_bytes(&d, |src| ogg::parse(src, ogg::Codec::Vorbis)).unwrap();
    assert_eq!(info.length, 2.0);
    assert_eq!(info.bitrate, 128_000);
}

#[test]
fn vorbis_without_nominal_bitrate_has_none() {
    let mut d = ogg_page(7, 0, 2, &vorbis_id(44100, 0));
    d.extend(ogg_page(7, 44100, 4, &[0u8; 100]));
    let info = read_bytes(&d, |src| ogg::parse(src, ogg::Codec::Vorbis)).unwrap();
    assert_eq!(info.bitrate, 0);
}

#[test]
fn opus_pre_skip_and_audio_data_bitrate() {
    let mut d = ogg_page(9, 0, 2, &opus_head(312));
    d.extend(ogg_page(9, 0, 0, b"OpusTags"));
    let audio_start = d.len();
    d.extend(ogg_page(9, 96_312, 4, &[0u8; 200]));
    let info = read_bytes(&d, |src| ogg::parse(src, ogg::Codec::Opus)).unwrap();
    assert_eq!(info.length, f64::from(96_312 - 312) / 48_000.0);
    let audio = (d.len() - audio_start) as f64;
    assert_eq!(
        info.bitrate,
        (audio * 8.0 / info.length).round_ties_even() as i64
    );
}

// --------------------------------------------------------------- WavPack

fn wavpack_block(total: u32, block_samples: u32, flags: u32) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend(b"wvpk");
    d.extend(32u32.to_le_bytes()); // block size after the first eight bytes
    d.extend(0x0410u16.to_le_bytes()); // version 4.16
    d.extend([0, 0]);
    d.extend(total.to_le_bytes());
    d.extend(0u32.to_le_bytes()); // block index
    d.extend(block_samples.to_le_bytes());
    d.extend(flags.to_le_bytes());
    d.extend(0u32.to_le_bytes()); // CRC
    d.extend([0u8; 8]); // block payload
    d
}

/// Flags: sample-rate index 9 (44100), mono, 16-bit.
fn wavpack_flags() -> u32 {
    (9 << 23) | 4 | 1
}

#[test]
fn wavpack_total_samples() {
    let d = wavpack_block(88_200, 88_200, wavpack_flags());
    let info = read_bytes(&d, wavpack::parse).unwrap();
    assert_eq!(info.length, 2.0);
    assert_eq!(info.bitrate, 0);
}

#[test]
fn wavpack_unknown_length_sums_the_blocks() {
    let mut d = wavpack_block(0xffff_ffff, 44_100, wavpack_flags());
    d.extend(wavpack_block(0, 44_100, wavpack_flags()));
    let info = read_bytes(&d, wavpack::parse).unwrap();
    assert_eq!(info.length, 2.0);
}

// ------------------------------------------------------------------- APE

#[test]
fn ape_modern_header_length() {
    let mut d = vec![0u8; 80];
    d[..4].copy_from_slice(b"MAC ");
    d[4..6].copy_from_slice(&3990u16.to_le_bytes());
    // One frame of 73728 blocks plus 1000 samples in the last frame.
    d[56..60].copy_from_slice(&73_728u32.to_le_bytes());
    d[60..64].copy_from_slice(&1000u32.to_le_bytes());
    d[64..68].copy_from_slice(&2u32.to_le_bytes());
    d[72..76].copy_from_slice(&44_100u32.to_le_bytes());
    let info = read_bytes(&d, ape::parse).unwrap();
    assert_eq!(info.length, (73_728.0 + 1000.0) / 44_100.0);
}

#[test]
fn ape_legacy_header_uses_the_old_layout_and_block_size() {
    let mut d = vec![0u8; 80];
    d[..4].copy_from_slice(b"MAC ");
    d[4..6].copy_from_slice(&3920u16.to_le_bytes());
    d[12..16].copy_from_slice(&44_100u32.to_le_bytes());
    d[24..28].copy_from_slice(&3u32.to_le_bytes());
    d[28..32].copy_from_slice(&1000u32.to_le_bytes());
    let info = read_bytes(&d, ape::parse).unwrap();
    // 3.90..3.94 use 73728 blocks per frame.
    assert_eq!(info.length, (2.0 * 73_728.0 + 1000.0) / 44_100.0);
}

#[test]
fn ape_without_frames_has_no_length() {
    let mut d = vec![0u8; 80];
    d[..4].copy_from_slice(b"MAC ");
    d[4..6].copy_from_slice(&3990u16.to_le_bytes());
    d[56..60].copy_from_slice(&73_728u32.to_le_bytes());
    d[72..76].copy_from_slice(&44_100u32.to_le_bytes());
    let info = read_bytes(&d, ape::parse).unwrap();
    assert_eq!(info.length, 0.0);
}

// ------------------------------------------------------------- WAVE/AIFF

fn wav(
    fmt_tag: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    bits: u16,
    data: usize,
) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend(fmt_tag.to_le_bytes());
    fmt.extend(channels.to_le_bytes());
    fmt.extend(rate.to_le_bytes());
    fmt.extend((rate * 4).to_le_bytes()); // byte rate is not used
    fmt.extend(block_align.to_le_bytes());
    fmt.extend(bits.to_le_bytes());
    let mut body = b"WAVE".to_vec();
    body.extend(b"fmt ");
    body.extend((fmt.len() as u32).to_le_bytes());
    body.extend(&fmt);
    body.extend(b"data");
    body.extend((data as u32).to_le_bytes());
    body.resize(body.len() + data, 0);
    let mut d = b"RIFF".to_vec();
    d.extend((body.len() as u32).to_le_bytes());
    d.extend(body);
    d
}

#[test]
fn wave_length_from_frames_and_pcm_bitrate() {
    let d = wav(1, 2, 44100, 4, 16, 176_400);
    let info = read_bytes(&d, riff::parse_wave).unwrap();
    assert_eq!(info.length, 1.0);
    assert_eq!(info.bitrate, 44100 * 2 * 16);
}

#[test]
fn wave_compressed_stream_has_no_bitrate() {
    // An MPEG layer III WAV: the block alignment gives the frame count.
    let d = wav(0x55, 2, 44100, 1152, 0, 23_040);
    let info = read_bytes(&d, riff::parse_wave).unwrap();
    assert_eq!(info.length, 20.0 / 44_100.0);
    assert_eq!(info.bitrate, 0);
}

fn aiff(channels: u16, frames: u32, bits: u16, rate: f64) -> Vec<u8> {
    let mut comm = Vec::new();
    comm.extend(channels.to_be_bytes());
    comm.extend(frames.to_be_bytes());
    comm.extend(bits.to_be_bytes());
    comm.extend(extended_bytes(rate));
    let mut body = b"AIFF".to_vec();
    body.extend(b"COMM");
    body.extend((comm.len() as u32).to_be_bytes());
    body.extend(&comm);
    let mut d = b"FORM".to_vec();
    d.extend((body.len() as u32).to_be_bytes());
    d.extend(body);
    d
}

/// 80-bit extended float bytes, for building AIFF headers.
fn extended_bytes(x: f64) -> [u8; 10] {
    let bits = x.to_bits();
    let sign = (bits >> 63) as u8;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let mant = bits & 0xf_ffff_ffff_ffff;
    let e = exp - 1023 + 16383;
    let m = (1u64 << 63) | (mant << 11);
    let mut out = [0u8; 10];
    out[0] = (sign << 7) | (e >> 8) as u8;
    out[1] = e as u8;
    out[2..10].copy_from_slice(&m.to_be_bytes());
    out
}

#[test]
fn aiff_length_and_pcm_bitrate() {
    let d = aiff(1, 66150, 24, 44100.0);
    let info = read_bytes(&d, riff::parse_aiff).unwrap();
    assert_eq!(info.length, 1.5);
    assert_eq!(info.bitrate, 44100 * 24);
}

#[test]
fn aiff_with_a_bad_form_type_is_unreadable() {
    let mut d = aiff(1, 44100, 16, 44100.0);
    d[8..12].copy_from_slice(b"WAVE");
    assert_eq!(read_bytes(&d, riff::parse_aiff), None);
}
