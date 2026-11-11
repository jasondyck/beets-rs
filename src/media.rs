//! Reading and writing tags in audio files (via lofty).

use crate::model::{Item, Value};
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::Tag;
use std::path::Path;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "mp4", "aac", "alac", "ogg", "oga", "opus", "wv", "ape", "mpc", "aif",
    "aiff", "wav", "spx",
];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn format_name(ft: FileType) -> &'static str {
    match ft {
        FileType::Aac => "AAC",
        FileType::Aiff => "AIFF",
        FileType::Ape => "APE",
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Mp4 => "AAC",
        FileType::Mpc => "Musepack",
        FileType::Opus => "Opus",
        FileType::Vorbis => "OGG",
        FileType::Speex => "Speex",
        FileType::Wav => "WAV",
        FileType::WavPack => "WavPack",
        _ => "Unknown",
    }
}

/// String-valued tag keys and the item fields they map to.
const TEXT_KEYS: &[(&str, ItemKey)] = &[
    ("artist_sort", ItemKey::TrackArtistSortOrder),
    ("albumartist", ItemKey::AlbumArtist),
    ("albumartist_sort", ItemKey::AlbumArtistSortOrder),
    ("composer", ItemKey::Composer),
    ("grouping", ItemKey::ContentGroup),
    ("disctitle", ItemKey::SetSubtitle),
    ("isrc", ItemKey::Isrc),
    ("mb_trackid", ItemKey::MusicBrainzRecordingId),
    ("mb_releasetrackid", ItemKey::MusicBrainzTrackId),
    ("mb_albumid", ItemKey::MusicBrainzReleaseId),
    ("mb_artistid", ItemKey::MusicBrainzArtistId),
    ("mb_albumartistid", ItemKey::MusicBrainzReleaseArtistId),
    ("mb_releasegroupid", ItemKey::MusicBrainzReleaseGroupId),
    ("albumtype", ItemKey::MusicBrainzReleaseType),
    ("label", ItemKey::Label),
    ("catalognum", ItemKey::CatalogNumber),
    ("country", ItemKey::ReleaseCountry),
    ("media", ItemKey::OriginalMediaType),
];

/// Item fields stored in file tags (used by `write` to detect differences).
pub const TAG_FIELDS: &[&str] = &[
    "title",
    "artist",
    "album",
    "genre",
    "comments",
    "track",
    "tracktotal",
    "disc",
    "disctotal",
    "bpm",
    "year",
    "month",
    "day",
    "original_year",
    "original_month",
    "original_day",
    "comp",
    "artist_sort",
    "albumartist",
    "albumartist_sort",
    "composer",
    "grouping",
    "disctitle",
    "lyrics",
    "isrc",
    "mb_trackid",
    "mb_releasetrackid",
    "mb_albumid",
    "mb_artistid",
    "mb_albumartistid",
    "mb_releasegroupid",
    "albumtype",
    "label",
    "catalognum",
    "country",
    "media",
];

/// Audio properties `read` takes from the file besides its tags.
pub const PROPERTY_FIELDS: &[&str] = &[
    "length",
    "bitrate",
    "samplerate",
    "bitdepth",
    "channels",
    "format",
];

/// beets' `Item._media_fields`: everything `read` sets from a file.
pub fn media_fields() -> impl Iterator<Item = &'static str> {
    TAG_FIELDS.iter().chain(PROPERTY_FIELDS).copied()
}

/// Parse "YYYY", "YYYY-MM" or "YYYY-MM-DD..." into (year, month, day).
fn parse_date(s: &str) -> (i64, i64, i64) {
    let mut parts = s.trim().splitn(3, ['-', '/']);
    let num = |p: Option<&str>| -> i64 {
        p.map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
    };
    let y = num(parts.next());
    let m = num(parts.next());
    let d = num(parts.next());
    (y, m, d)
}

fn date_string(y: i64, m: i64, d: i64) -> Option<String> {
    match (y, m, d) {
        (0, _, _) => None,
        (y, 0, _) => Some(format!("{y:04}")),
        (y, m, 0) => Some(format!("{y:04}-{m:02}")),
        (y, m, d) => Some(format!("{y:04}-{m:02}-{d:02}")),
    }
}

fn pick_tag(file: &TaggedFile) -> Option<&Tag> {
    file.primary_tag().or_else(|| file.first_tag())
}

/// An MPEG layer III file's bitrate as mutagen (and so beets) computes it:
/// from the Xing/Info header's frame and byte counts when there is one,
/// else the first frame's nominal bitrate. `None` for anything else (other
/// layers, no frame found), where lofty's figure is used.
fn mpeg_bitrate(path: &Path) -> Option<i64> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_end(&mut data)
        .ok()?;
    mpeg_bitrate_of(&data)
}

fn mpeg_bitrate_of(data: &[u8]) -> Option<i64> {
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
        let h = u32::from_be_bytes(data[i..i + 4].try_into().ok()?);
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
    let off = start
        + match (mpeg1, mono) {
            (true, false) => 36,
            (true, true) | (false, false) => 21,
            (false, true) => 13,
        };
    let xing = data.get(off..off + 8);
    let Some(x) = xing.filter(|x| &x[..4] == b"Xing" || &x[..4] == b"Info") else {
        return Some(bitrate);
    };
    let flags = u32::from_be_bytes(x[4..8].try_into().ok()?);
    let mut p = off + 8;
    let mut field = |present: bool| -> Option<Option<i64>> {
        if !present {
            return Some(None);
        }
        let v = u32::from_be_bytes(data.get(p..p + 4)?.try_into().ok()?);
        p += 4;
        Some(Some(v as i64))
    };
    let frames = field(flags & 1 != 0)?;
    let bytes = field(flags & 2 != 0)?;
    match (frames, bytes) {
        (Some(frames), Some(bytes)) if frames * frame_size > 0 => {
            let audio_bytes = (bytes - frame_length).max(0);
            let samples = (frames * frame_size) as f64;
            Some(((audio_bytes * 8 * sample_rate) as f64 / samples).round_ties_even() as i64)
        }
        _ => Some(bitrate),
    }
}

/// An MP4 file's audio sample entry `samplesize` (mutagen's
/// `bits_per_sample`; 16 in AAC files, where lofty has no bit depth): the
/// first entry of the first sound track's `stsd`.
fn mp4_sample_size(path: &Path) -> Option<i64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    type Boxes = Vec<([u8; 4], u64, u64)>;
    // The child boxes of the box spanning `start..end`: (type, body start, end).
    fn boxes(f: &mut std::fs::File, start: u64, end: u64) -> Option<Boxes> {
        let mut out = vec![];
        let mut pos = start;
        while pos + 8 <= end {
            f.seek(SeekFrom::Start(pos)).ok()?;
            let mut h = [0u8; 16];
            f.read_exact(&mut h[..8]).ok()?;
            let mut size = u32::from_be_bytes(h[..4].try_into().ok()?) as u64;
            let mut header = 8;
            if size == 1 {
                f.read_exact(&mut h[8..16]).ok()?;
                size = u64::from_be_bytes(h[8..16].try_into().ok()?);
                header = 16;
            } else if size == 0 {
                size = end - pos;
            }
            if size < header || pos + size > end {
                break;
            }
            out.push((h[4..8].try_into().ok()?, pos + header, pos + size));
            pos += size;
        }
        Some(out)
    }
    let find = |boxes: &[([u8; 4], u64, u64)], name: &[u8; 4]| {
        boxes
            .iter()
            .find(|(t, _, _)| t == name)
            .map(|&(_, s, e)| (s, e))
    };
    let top = boxes(&mut f, 0, file_len)?;
    let (ms, me) = find(&top, b"moov")?;
    let moov = boxes(&mut f, ms, me)?;
    for (_, ts, te) in moov.iter().filter(|(t, _, _)| t == b"trak") {
        let trak = boxes(&mut f, *ts, *te)?;
        let Some((ds, de)) = find(&trak, b"mdia") else {
            continue;
        };
        let mdia = boxes(&mut f, ds, de)?;
        let Some((hs, _)) = find(&mdia, b"hdlr") else {
            continue;
        };
        let mut handler = [0u8; 4];
        // hdlr: version/flags (4), pre_defined (4), handler_type (4).
        f.seek(SeekFrom::Start(hs + 8)).ok()?;
        f.read_exact(&mut handler).ok()?;
        if &handler != b"soun" {
            continue;
        }
        let (ns, ne) = find(&mdia, b"minf")?;
        let minf = boxes(&mut f, ns, ne)?;
        let (ss, se) = find(&minf, b"stbl")?;
        let stbl = boxes(&mut f, ss, se)?;
        let (ds, _) = find(&stbl, b"stsd")?;
        // stsd: version/flags (4), entry count (4), then the first entry's
        // box header (8), reserved (6), data ref index (2), reserved (8),
        // channel count (2), sample size (2).
        let mut buf = [0u8; 36];
        f.seek(SeekFrom::Start(ds)).ok()?;
        f.read_exact(&mut buf).ok()?;
        if u32::from_be_bytes(buf[4..8].try_into().ok()?) == 0 {
            return None;
        }
        return Some(u16::from_be_bytes(buf[34..36].try_into().ok()?) as i64);
    }
    None
}

/// Read an audio file into a new (unsaved) item.
pub fn read(path: &Path) -> Result<Item> {
    let file = lofty::read_from_path(path)
        .with_context(|| format!("reading tags from {}", path.display()))?;
    let mut item = Item::new_item();
    item.set_path(path);
    let props = file.properties();
    item.set("length", Value::Float(props.duration().as_secs_f64()));
    let bitrate = match file.file_type() {
        FileType::Mpeg => mpeg_bitrate(path),
        _ => None,
    };
    item.set_int(
        "bitrate",
        bitrate.unwrap_or(
            props
                .audio_bitrate()
                .or(props.overall_bitrate())
                .unwrap_or(0) as i64
                * 1000,
        ),
    );
    item.set_int("samplerate", props.sample_rate().unwrap_or(0) as i64);
    let bitdepth = props
        .bit_depth()
        .map(|b| b as i64)
        .or_else(|| match file.file_type() {
            FileType::Mp4 => mp4_sample_size(path),
            _ => None,
        });
    item.set_int("bitdepth", bitdepth.unwrap_or(0));
    item.set_int("channels", props.channels().unwrap_or(0) as i64);
    let format = match (file.file_type(), path.extension().and_then(|e| e.to_str())) {
        (FileType::Mp4, Some(e))
            if e.eq_ignore_ascii_case("m4a") && props.bit_depth().is_some() =>
        {
            "ALAC"
        }
        (ft, _) => format_name(ft),
    };
    item.set_str("format", format);
    if let Some(tag) = pick_tag(&file) {
        item.set_str("title", tag.title().unwrap_or_default());
        item.set_str("artist", tag.artist().unwrap_or_default());
        item.set_str("album", tag.album().unwrap_or_default());
        item.set_str("genre", tag.genre().unwrap_or_default());
        item.set_str("comments", tag.comment().unwrap_or_default());
        item.set_int("track", tag.track().unwrap_or(0) as i64);
        item.set_int("tracktotal", tag.track_total().unwrap_or(0) as i64);
        item.set_int("disc", tag.disk().unwrap_or(0) as i64);
        item.set_int("disctotal", tag.disk_total().unwrap_or(0) as i64);
        for (field, key) in TEXT_KEYS {
            if let Some(v) = tag.get_string(*key) {
                item.set_str(field, v);
            }
        }
        // ID3v2 only knows USLT, which lofty calls UnsyncLyrics.
        if let Some(l) = tag
            .get_string(ItemKey::Lyrics)
            .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
        {
            item.set_str("lyrics", l);
        }
        // ID3 keeps the label in TPUB, which lofty reads back as Publisher.
        if item.str("label").is_empty()
            && let Some(v) = tag.get_string(ItemKey::Publisher)
        {
            item.set_str("label", v);
        }
        let date = tag
            .get_string(ItemKey::RecordingDate)
            .or_else(|| tag.get_string(ItemKey::Year))
            .or_else(|| tag.get_string(ItemKey::ReleaseDate));
        if let Some(d) = date {
            let (y, m, dd) = parse_date(d);
            item.set_int("year", y);
            item.set_int("month", m);
            item.set_int("day", dd);
        }
        if let Some(d) = tag.get_string(ItemKey::OriginalReleaseDate) {
            let (y, m, dd) = parse_date(d);
            item.set_int("original_year", y);
            item.set_int("original_month", m);
            item.set_int("original_day", dd);
        }
        if let Some(c) = tag.get_string(ItemKey::FlagCompilation) {
            item.set(
                "comp",
                Value::Bool(crate::model::parse_bool(c.trim()).unwrap_or(false)),
            );
        }
        if let Some(b) = tag
            .get_string(ItemKey::IntegerBpm)
            .or_else(|| tag.get_string(ItemKey::Bpm))
        {
            item.set_int("bpm", b.trim().parse::<f64>().unwrap_or(0.0).round() as i64);
        }
    }
    // Last: setting the tag fields reset it (beets' `from_path`).
    refresh_mtime(&mut item);
    Ok(item)
}

/// Write tags and refresh the item's stored mtime so `update` doesn't
/// mistake our own write for an external change.
pub fn write_item(item: &mut Item) -> Result<()> {
    write(item)?;
    refresh_mtime(item);
    Ok(())
}

/// Record the file's current mtime on the item.
pub fn refresh_mtime(item: &mut Item) {
    if let Ok(m) = std::fs::metadata(item.path()).and_then(|m| m.modified()) {
        let ts = m
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        item.set("mtime", Value::Float(ts));
    }
}

/// Open a file and make sure it has a primary tag to edit.
fn open_for_edit(path: &Path) -> Result<TaggedFile> {
    let mut file =
        lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let tag_type = file.primary_tag_type();
    if file.tag(tag_type).is_none() {
        file.insert_tag(Tag::new(tag_type));
    }
    Ok(file)
}

fn save(file: &TaggedFile, path: &Path) -> Result<()> {
    file.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("writing tags to {}", path.display()))
}

/// Embed an image as the front cover, replacing any existing front cover.
pub fn embed_art(path: &Path, image: &[u8]) -> Result<()> {
    let mut pic = Picture::from_reader(&mut &image[..])
        .map_err(|e| anyhow::anyhow!("not a usable image: {e}"))?;
    pic.set_pic_type(PictureType::CoverFront);
    let mut file = open_for_edit(path)?;
    let tag = file.primary_tag_mut().unwrap();
    if tag.tag_type() == lofty::tag::TagType::Mp4Ilst {
        // MP4 cover atoms carry no picture type, so every one counts as "the" cover.
        while tag.picture_count() > 0 {
            tag.remove_picture(0);
        }
    } else {
        tag.remove_picture_type(PictureType::CoverFront);
    }
    tag.push_picture(pic);
    save(&file, path)
}

/// Whether any tag in the file carries a picture.
pub fn has_art(path: &Path) -> Result<bool> {
    let file =
        lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(file.tags().iter().any(|t| t.picture_count() > 0))
}

/// The embedded front cover (or else the first picture) and a file extension for it.
pub fn extract_art(path: &Path) -> Result<Option<(Vec<u8>, &'static str)>> {
    let file =
        lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let pics: Vec<&Picture> = file.tags().iter().flat_map(|t| t.pictures()).collect();
    let Some(pic) = pics
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or(pics.first())
    else {
        return Ok(None);
    };
    let ext = match pic.mime_type() {
        Some(MimeType::Png) => "png",
        Some(MimeType::Gif) => "gif",
        Some(MimeType::Bmp) => "bmp",
        Some(MimeType::Tiff) => "tiff",
        _ => "jpg",
    };
    Ok(Some((pic.data().to_vec(), ext)))
}

/// Remove every embedded picture. Returns whether anything was removed.
pub fn clear_art(path: &Path) -> Result<bool> {
    let mut file =
        lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let mut removed = false;
    let types: Vec<_> = file.tags().iter().map(|t| t.tag_type()).collect();
    for tt in types {
        let tag = file.tag_mut(tt).unwrap();
        while tag.picture_count() > 0 {
            tag.remove_picture(0);
            removed = true;
        }
    }
    if removed {
        save(&file, path)?;
    }
    Ok(removed)
}

/// Write an item's metadata into its file.
pub fn write(item: &Item) -> Result<()> {
    write_to(item, item.path())
}

pub fn write_to(item: &Item, path: &Path) -> Result<()> {
    let mut file =
        lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let tag_type = file.primary_tag_type();
    if file.tag(tag_type).is_none() {
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file.tag_mut(tag_type).unwrap();

    let set_text = |tag: &mut Tag, key: ItemKey, v: &str| {
        if v.is_empty() {
            tag.remove_key(key);
        } else {
            tag.insert_text(key, v.to_string());
        }
    };
    set_text(tag, ItemKey::TrackTitle, item.str("title"));
    set_text(tag, ItemKey::TrackArtist, item.str("artist"));
    set_text(tag, ItemKey::AlbumTitle, item.str("album"));
    set_text(tag, ItemKey::Genre, item.str("genre"));
    set_text(tag, ItemKey::Comment, item.str("comments"));
    for (field, key) in TEXT_KEYS {
        set_text(tag, *key, item.str(field));
    }
    let lyrics = item.str("lyrics");
    tag.remove_key(ItemKey::Lyrics);
    tag.remove_key(ItemKey::UnsyncLyrics);
    if !lyrics.is_empty() && !tag.insert_text(ItemKey::Lyrics, lyrics.to_string()) {
        tag.insert_text(ItemKey::UnsyncLyrics, lyrics.to_string());
    }
    let num = |tag: &mut Tag, key: ItemKey, v: i64| {
        if v > 0 {
            tag.insert_text(key, v.to_string());
        } else {
            tag.remove_key(key);
        }
    };
    num(tag, ItemKey::TrackNumber, item.int("track"));
    num(tag, ItemKey::TrackTotal, item.int("tracktotal"));
    num(tag, ItemKey::DiscNumber, item.int("disc"));
    num(tag, ItemKey::DiscTotal, item.int("disctotal"));
    num(tag, ItemKey::IntegerBpm, item.int("bpm"));

    tag.remove_key(ItemKey::Year);
    match date_string(item.int("year"), item.int("month"), item.int("day")) {
        Some(d) => {
            tag.insert_text(ItemKey::RecordingDate, d);
        }
        None => tag.remove_key(ItemKey::RecordingDate),
    }
    match date_string(
        item.int("original_year"),
        item.int("original_month"),
        item.int("original_day"),
    ) {
        Some(d) => {
            tag.insert_text(ItemKey::OriginalReleaseDate, d);
        }
        None => tag.remove_key(ItemKey::OriginalReleaseDate),
    }
    if item.bool("comp") {
        tag.insert_text(ItemKey::FlagCompilation, "1".into());
    } else {
        tag.remove_key(ItemKey::FlagCompilation);
    }

    file.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("writing tags to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal MPEG 1 layer III stream: an ID3v2 tag, then one 128 kbps
    /// 44.1 kHz stereo frame, with an Info header when `xing` gives its
    /// frame and byte counts.
    fn mpeg(xing: Option<(u32, u32)>) -> Vec<u8> {
        let mut d = b"ID3\x04\x00\x00\x00\x00\x00\x0a".to_vec();
        d.extend([0u8; 10]);
        let mut frame = vec![0xff, 0xfb, 0x90, 0x00];
        frame.resize(417, 0);
        if let Some((frames, bytes)) = xing {
            frame[36..40].copy_from_slice(b"Info");
            frame[40..44].copy_from_slice(&3u32.to_be_bytes());
            frame[44..48].copy_from_slice(&frames.to_be_bytes());
            frame[48..52].copy_from_slice(&bytes.to_be_bytes());
        }
        d.extend(frame);
        d
    }

    // mutagen's figures: (bytes - first frame) * 8 * rate / (frames * 1152),
    // rounded half to even; without a Xing/Info header, the frame's own.
    #[test]
    fn mpeg_bitrate_as_mutagen() {
        assert_eq!(mpeg_bitrate_of(&mpeg(None)), Some(128_000));
        // 40 frames of 417 bytes after the Info frame.
        let bytes = 41 * 417;
        // 40 * 417 * 8 * 44100 / (40 * 1152) = 127706.25
        assert_eq!(mpeg_bitrate_of(&mpeg(Some((40, bytes)))), Some(127_706));
        assert_eq!(mpeg_bitrate_of(b"not audio"), None);
    }

    // mutagen's `bits_per_sample` for AAC: the sample entry's sample size.
    #[test]
    fn mp4_aac_bit_depth() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.m4a");
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=d=0.2",
                "-c:a",
                "aac",
            ])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        assert_eq!(read(&p).unwrap().int("bitdepth"), 16);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("2003-05-12T00:00"), (2003, 5, 12));
        assert_eq!(parse_date("1999"), (1999, 0, 0));
        assert_eq!(date_string(1999, 3, 0).unwrap(), "1999-03");
    }
}

#[cfg(test)]
mod roundtrip {
    use super::*;

    /// Needs ffmpeg to make a real file; skipped when it's missing.
    fn make(ext: &str) -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(format!("t.{ext}"));
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some((d, p))
    }

    #[test]
    fn embeds_extracts_and_clears_art() {
        // 1x1 PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99,
            0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        for ext in ["mp3", "flac", "m4a", "ogg"] {
            let Some((_d, p)) = make(ext) else { return };
            assert!(!has_art(&p).unwrap());
            embed_art(&p, png).unwrap();
            // Embedding twice replaces rather than stacking covers.
            embed_art(&p, png).unwrap();
            let file = lofty::read_from_path(&p).unwrap();
            let n: u32 = file.tags().iter().map(|t| t.picture_count()).sum();
            assert_eq!(n, 1, "{ext}");
            let (data, kind) = extract_art(&p).unwrap().unwrap();
            assert_eq!((data.as_slice(), kind), (png, "png"), "{ext}");
            // Ordinary tag writes keep the picture.
            let mut item = read(&p).unwrap();
            item.set_str("title", "x");
            write(&item).unwrap();
            assert!(has_art(&p).unwrap(), "{ext}");
            assert!(clear_art(&p).unwrap());
            assert!(!has_art(&p).unwrap());
        }
    }

    #[test]
    fn writes_and_reads_back_all_tag_fields() {
        for ext in ["mp3", "flac", "m4a", "ogg"] {
            let Some((_d, p)) = make(ext) else { return };
            let mut item = read(&p).unwrap();
            item.set_str("title", "T");
            item.set_str("lyrics", "line one\nline two");
            item.set_str("label", "Lbl");
            item.set_int("track", 3);
            item.set_int("year", 2001);
            write(&item).unwrap();
            let back = read(&p).unwrap();
            for f in ["title", "lyrics", "label", "track", "year"] {
                assert_eq!(back.formatted(f), item.formatted(f), "{ext}: {f}");
            }
        }
    }
}
