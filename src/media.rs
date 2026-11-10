//! Reading and writing tags in audio files (via lofty).

use crate::model::{Item, Value};
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
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
    ("lyrics", ItemKey::Lyrics),
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

/// Read an audio file into a new (unsaved) item.
pub fn read(path: &Path) -> Result<Item> {
    let file = lofty::read_from_path(path)
        .with_context(|| format!("reading tags from {}", path.display()))?;
    let mut item = Item::new_item();
    item.set_path(path);
    let props = file.properties();
    item.set("length", Value::Float(props.duration().as_secs_f64()));
    item.set_int(
        "bitrate",
        props
            .audio_bitrate()
            .or(props.overall_bitrate())
            .unwrap_or(0) as i64
            * 1000,
    );
    item.set_int("samplerate", props.sample_rate().unwrap_or(0) as i64);
    item.set_int("bitdepth", props.bit_depth().unwrap_or(0) as i64);
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
    if let Ok(meta) = std::fs::metadata(path)
        && let Ok(m) = meta.modified()
    {
        let ts = m
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        item.set("mtime", Value::Float(ts));
    }

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
    Ok(item)
}

/// Write tags and refresh the item's stored mtime so `update` doesn't
/// mistake our own write for an external change.
pub fn write_item(item: &mut Item) -> Result<()> {
    write(item)?;
    if let Ok(m) = std::fs::metadata(item.path()).and_then(|m| m.modified()) {
        let ts = m
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        item.set("mtime", Value::Float(ts));
    }
    Ok(())
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

    #[test]
    fn dates() {
        assert_eq!(parse_date("2003-05-12T00:00"), (2003, 5, 12));
        assert_eq!(parse_date("1999"), (1999, 0, 0));
        assert_eq!(date_string(1999, 3, 0).unwrap(), "1999-03");
    }
}
