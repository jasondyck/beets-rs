//! Reading and writing tags in audio files (via lofty).

use crate::mediafile::{Got, MediaFile};
use crate::model::{Item, Value};
use anyhow::Result;
use lofty::file::FileType;
#[cfg(test)]
use lofty::file::TaggedFileExt;
#[cfg(test)]
use lofty::picture::MimeType;
use lofty::picture::{Picture, PictureType};
use std::collections::BTreeSet;
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
        // mediafile's `TYPES`.
        FileType::Wav => "WAVE",
        FileType::WavPack => "WavPack",
        _ => "Unknown",
    }
}

pub use crate::mediafile::TAG_FIELDS;

/// Audio properties `read` takes from the file besides its tags.
pub const PROPERTY_FIELDS: &[&str] = &[
    "length",
    "bitrate",
    "samplerate",
    "bitdepth",
    "channels",
    "format",
    "bitrate_mode",
    "encoder_info",
    "encoder_settings",
];

/// beets' `Item._media_fields`: everything `read` sets from a file.
pub fn media_fields() -> impl Iterator<Item = &'static str> {
    TAG_FIELDS.iter().chain(PROPERTY_FIELDS).copied()
}

/// An MP4 file's audio sample entry `samplesize` (mutagen's
/// `bits_per_sample`; 16 in AAC files, where lofty has no bit depth): the
/// first entry of the first sound track's `stsd`.
fn mp4_sample_size(path: &Path) -> Option<i64> {
    use std::io::{Read, Seek, SeekFrom};
    type Boxes = Vec<([u8; 4], u64, u64)>;
    // The child boxes of the box spanning `start..end`: (type, body start, end).
    fn boxes(f: &mut std::fs::File, start: u64, end: u64) -> Option<Boxes> {
        let mut out = vec![];
        let mut pos = start;
        while pos + 8 <= end {
            f.seek(SeekFrom::Start(pos)).ok()?;
            let mut h = [0u8; 16];
            f.read_exact(&mut h[..8]).ok()?;
            let mut size = u64::from(u32::from_be_bytes(h[..4].try_into().ok()?));
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
    let mut f = std::fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
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
        return Some(i64::from(u16::from_be_bytes(buf[34..36].try_into().ok()?)));
    }
    None
}

/// Read an audio file into a new (unsaved) item.
pub fn read(path: &Path) -> Result<Item> {
    // One read of the file gives both its tags and its properties.
    let (mf, props) = MediaFile::open_with_properties(path)?;
    let mut item = Item::new_item();
    item.set_path(path);
    // beets' exact values (mutagen's, per format); lofty's (rounded to ms
    // and kbps) for formats `props` doesn't cover.
    let (length, bitrate, enc) = crate::props::read(path, mf.file_type()).unwrap_or_else(|| {
        (
            props.duration().as_secs_f64(),
            i64::from(
                props
                    .audio_bitrate()
                    .or(props.overall_bitrate())
                    .unwrap_or(0),
            ) * 1000,
            Default::default(),
        )
    });
    item.set_str("bitrate_mode", enc.bitrate_mode);
    item.set_str("encoder_info", enc.info);
    item.set_str("encoder_settings", enc.settings);
    item.set("length", Value::Float(length));
    item.set_int("bitrate", bitrate);
    item.set_int("samplerate", i64::from(props.sample_rate().unwrap_or(0)));
    let bitdepth = props
        .bit_depth()
        .map(i64::from)
        .or_else(|| match mf.file_type() {
            FileType::Mp4 => mp4_sample_size(path),
            _ => None,
        });
    item.set_int("bitdepth", bitdepth.unwrap_or(0));
    item.set_int("channels", i64::from(props.channels().unwrap_or(0)));
    let format = match (mf.file_type(), path.extension().and_then(|e| e.to_str())) {
        (FileType::Mp4, Some(e))
            if e.eq_ignore_ascii_case("m4a") && props.bit_depth().is_some() =>
        {
            "ALAC"
        }
        (ft, _) => format_name(ft),
    };
    item.set_str("format", format);
    for f in TAG_FIELDS {
        let v = match mf.get(f) {
            Some(Got::Str(Some(s))) => Value::Str(s),
            Some(Got::Int(Some(i))) => Value::Int(i),
            Some(Got::Bool(Some(b))) => Value::Bool(b),
            Some(Got::Float(Some(x))) => Value::Float(x),
            Some(Got::List(Some(v))) => {
                item.set_list(f, &v);
                continue;
            }
            // The item starts with the type's null value; setting None
            // again would only cost a schema lookup.
            _ => continue,
        };
        item.set(f, v);
    }
    // Last: setting the tag fields reset it (beets' `from_path`).
    refresh_mtime(&mut item);
    Ok(item)
}

/// Write tags and refresh the item's stored mtime so `update` doesn't
/// mistake our own write for an external change.
pub fn write_item(item: &mut Item) -> Result<()> {
    write_mut(item)?;
    refresh_mtime(item);
    Ok(())
}

/// `write`, then the item changes `write` listeners made (zero's
/// `update_database`).
pub fn write_mut(item: &mut Item) -> Result<()> {
    let changed = write_event(item, item.path(), id3v23(), Extra::None, &BTreeSet::new())?;
    apply_item_nulls(item, &changed);
    Ok(())
}

/// beets' `item.write(tags=tags)` where `tags` sets `nulls` to None
/// (removed from the file); then the item changes `write` listeners made.
pub fn write_nulls(item: &mut Item, nulls: &BTreeSet<String>) -> Result<()> {
    let changed = write_event(item, item.path(), id3v23(), Extra::None, nulls)?;
    apply_item_nulls(item, &changed);
    refresh_mtime(item);
    Ok(())
}

/// `item[field] = None` for each field (fixed fields take their null
/// value). Fields the item doesn't have (`images`, `date`, ...) are left
/// out.
pub fn apply_item_nulls(item: &mut Item, fields: &[String]) {
    for f in fields {
        if item.field_type(f).is_some() {
            item.set(f, Value::Null);
        }
    }
}

// ------------------------------------------------------------ write event

/// beets' `write` event: the tags `Item.write` is about to write (the
/// item's media fields plus any extra tags of the write), which listeners
/// may set to None.
pub struct WriteTags<'a> {
    item: &'a Item,
    /// Extra tags of this write (`images`, `synced_lyrics`) with Python's
    /// `str()` of their values.
    extra: Vec<(&'static str, String)>,
    /// Tags set to None: removed from the file.
    nulls: BTreeSet<String>,
    /// Fields set to None on the item itself.
    item_nulls: Vec<String>,
}

impl<'a> WriteTags<'a> {
    /// The tags of a plain `item.write()`.
    pub fn for_item(item: &'a Item) -> Self {
        WriteTags {
            item,
            extra: vec![],
            nulls: BTreeSet::new(),
            item_nulls: vec![],
        }
    }

    /// The tags set to None, and the fields set to None on the item.
    pub fn into_parts(self) -> (BTreeSet<String>, Vec<String>) {
        (self.nulls, self.item_nulls)
    }

    pub fn item(&self) -> &Item {
        self.item
    }

    /// `field in tags`.
    pub fn contains(&self, field: &str) -> bool {
        TAG_FIELDS.contains(&field)
            || self.extra.iter().any(|(k, _)| *k == field)
            || self.nulls.contains(field)
    }

    /// Python's `str()` of `tags[field]`.
    pub fn value_str(&self, field: &str) -> String {
        if self.nulls.contains(field) {
            return "None".into();
        }
        if let Some((_, v)) = self.extra.iter().find(|(k, _)| *k == field) {
            return v.clone();
        }
        py_value_str(self.item, field)
    }

    /// `tags[field] = None`.
    pub fn set_null(&mut self, field: &str) {
        self.nulls.insert(field.to_string());
    }

    /// `item[field] = None`.
    pub fn set_item_null(&mut self, field: &str) {
        if !self.item_nulls.iter().any(|f| f == field) {
            self.item_nulls.push(field.to_string());
        }
    }
}

/// Python's `str()` of an item field's value, as beets holds it.
pub fn py_value_str(item: &Item, field: &str) -> String {
    use crate::hooks::{py_float, py_repr};
    use crate::model::FieldType as T;
    let Some(v) = item.get(field) else {
        return "None".into();
    };
    match (item.field_type(field), v) {
        (_, Value::Null) => "None".into(),
        (Some(T::List(_)), _) => format!(
            "[{}]",
            item.list(field)
                .iter()
                .map(|s| py_repr(s))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (_, Value::Bool(b)) => if b { "True" } else { "False" }.into(),
        (_, Value::Float(f)) => py_float(f),
        (_, v) => v.raw(),
    }
}

/// A listener for the `write` event.
pub trait WriteListener: Send + Sync {
    fn write(&self, tags: &mut WriteTags);
}

type Listeners = Vec<std::sync::Arc<dyn WriteListener>>;

#[cfg(not(test))]
static LISTENERS: std::sync::RwLock<Listeners> = std::sync::RwLock::new(Vec::new());

// Tests run in parallel: each test thread has its own listeners.
#[cfg(test)]
thread_local! {
    static LISTENERS: std::cell::RefCell<Listeners> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Register a `write` listener (a plugin, when it loads).
pub fn add_write_listener(l: std::sync::Arc<dyn WriteListener>) {
    #[cfg(not(test))]
    LISTENERS.write().unwrap().push(l);
    #[cfg(test)]
    LISTENERS.with(|v| v.borrow_mut().push(l));
}

fn listeners() -> Listeners {
    #[cfg(not(test))]
    return LISTENERS.read().unwrap().clone();
    #[cfg(test)]
    LISTENERS.with(|v| v.borrow().clone())
}

#[cfg(test)]
pub fn clear_write_listeners() {
    LISTENERS.with(|v| v.borrow_mut().clear());
}

/// Extra tags of a write besides the item's fields.
enum Extra<'a> {
    None,
    /// `images`: the one front cover, or none.
    Images(Option<Picture>),
    /// `synced_lyrics` (MP3s' SYLT frame).
    Sylt(Option<&'a [(String, u32)]>),
}

/// beets' `Item.write`: send the `write` event, then write the item's
/// tags (and the extra ones) with the tags listeners set to None removed.
/// Returns the fields listeners set to None on the item.
fn write_event(
    item: &Item,
    path: &Path,
    id3v23: bool,
    extra: Extra,
    nulls: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let extra_tags = match &extra {
        Extra::None => vec![],
        Extra::Images(p) => vec![(
            "images",
            if p.is_some() { "[<Image>]" } else { "None" }.to_string(),
        )],
        Extra::Sylt(s) => vec![(
            "synced_lyrics",
            if s.is_some() {
                "<synced lyrics>"
            } else {
                "None"
            }
            .to_string(),
        )],
    };
    let mut tags = WriteTags {
        item,
        extra: extra_tags,
        nulls: nulls.clone(),
        item_nulls: vec![],
    };
    for l in listeners() {
        l.write(&mut tags);
    }
    let nulls = tags.nulls;
    // beets' `write` event: the tags about to be written.
    if crate::events::listening("write") {
        let mut kv: Vec<(String, String)> = vec![];
        for f in TAG_FIELDS {
            if item.get(f).is_some() && !nulls.contains(*f) {
                kv.push(((*f).to_string(), py_value_str(item, f)));
            }
        }
        for (k, v) in &tags.extra {
            if !nulls.contains(*k) {
                kv.push(((*k).to_string(), v.clone()));
            }
        }
        crate::events::send(
            "write",
            &[
                ("item", crate::events::Arg::Item(item)),
                ("path", crate::events::Arg::Path(path)),
                ("tags", crate::events::Arg::Tags(&kv)),
            ],
        );
    }
    let mut mf = MediaFile::open(path)?;
    mf.update_with(item, &nulls);
    match extra {
        Extra::None => {}
        Extra::Images(pic) => {
            mf.clear_pictures();
            if let Some(pic) = pic {
                mf.set_front_cover(pic);
            }
        }
        Extra::Sylt(s) => mf.set_synced_lyrics(s.unwrap_or_default()),
    }
    // `art` sorts before `images`: an `images` tag of the write wins.
    if nulls.contains("images")
        || nulls.contains("art")
            && !matches!(tags.extra.first(), Some(("images", v)) if v != "None")
    {
        mf.clear_pictures();
    }
    if nulls.contains("synced_lyrics") {
        mf.set_synced_lyrics(&[]);
    }
    mf.save(id3v23)?;
    // beets' `after_write`: importadded may put the file's mtime back.
    if crate::events::listening("after_write") {
        crate::events::send(
            "after_write",
            &[
                ("item", crate::events::Arg::Item(item)),
                ("path", crate::events::Arg::Path(path)),
            ],
        );
    }
    Ok(tags.item_nulls)
}

/// Record the file's current mtime on the item.
pub fn refresh_mtime(item: &mut Item) {
    if let Ok(m) = std::fs::metadata(item.path()).and_then(|m| m.modified()) {
        let ts = m
            .duration_since(std::time::UNIX_EPOCH)
            // beets' `current_mtime`: whole seconds.
            .map(|d| d.as_secs() as f64)
            .unwrap_or(0.0);
        item.set("mtime", Value::Float(ts));
    }
}

/// Embed an image as the front cover, replacing any existing front cover
/// (in the tag MediaFile uses, keeping every other tag as it is).
#[cfg(test)]
pub fn embed_art(path: &Path, image: &[u8]) -> Result<()> {
    let pic = Picture::from_reader(&mut &image[..])
        .map_err(|e| anyhow::anyhow!("not a usable image: {e}"))?;
    let mut mf = MediaFile::open(path)?;
    mf.set_front_cover(pic);
    mf.save(id3v23())
}

/// Whether the file has embedded pictures (beets' `has_cover_art`:
/// MediaFile's `images`).
pub fn has_art(path: &Path) -> Result<bool> {
    Ok(!MediaFile::open(path)?.pictures().is_empty())
}

/// The embedded front cover (or else the first picture) and a file extension for it.
#[cfg(test)]
pub fn extract_art(path: &Path) -> Result<Option<(Vec<u8>, &'static str)>> {
    let pics = MediaFile::open(path)?.pictures();
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

/// beets' `item.try_write(tags={"images": ...})`: the item's tags, with the
/// file's pictures replaced by `image` as the only (front cover) picture,
/// or removed for `None`.
pub fn write_with_images(item: &Item, image: Option<&[u8]>) -> Result<()> {
    let pic = match image {
        Some(data) => Some(
            Picture::from_reader(&mut &data[..])
                .map_err(|e| anyhow::anyhow!("not a usable image: {e}"))?,
        ),
        None => None,
    };
    write_event(
        item,
        item.path(),
        id3v23(),
        Extra::Images(pic),
        &BTreeSet::new(),
    )?;
    Ok(())
}

/// `write_with_images` into another file (beets' `item.try_write(path=...,
/// tags={"images": ...}, id3v23=...)`).
pub fn write_with_images_at(
    item: &Item,
    path: &Path,
    id3v23: bool,
    image: Option<&[u8]>,
) -> Result<()> {
    let pic = match image {
        Some(data) => Some(
            Picture::from_reader(&mut &data[..])
                .map_err(|e| anyhow::anyhow!("not a usable image: {e}"))?,
        ),
        None => None,
    };
    write_event(item, path, id3v23, Extra::Images(pic), &BTreeSet::new())?;
    Ok(())
}

/// The embedded pictures' data (MediaFile's `art`: the front cover, else
/// the first picture), or `None` if there are none.
pub fn get_art(path: &Path) -> Result<Option<Vec<u8>>> {
    let pics = MediaFile::open(path)?.pictures();
    Ok(pics
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or(pics.first())
        .map(|p| p.data().to_vec()))
}

/// Remove every embedded picture (from the tag MediaFile uses). Returns
/// whether anything was removed.
#[cfg(test)]
pub fn clear_art(path: &Path) -> Result<bool> {
    let mut mf = MediaFile::open(path)?;
    let removed = mf.clear_pictures();
    if removed {
        mf.save(id3v23())?;
    }
    Ok(removed)
}

/// Write an item's metadata into its file.
#[cfg(test)]
pub fn write(item: &Item) -> Result<()> {
    write_to(item, item.path())
}

#[cfg(test)]
pub fn write_to(item: &Item, path: &Path) -> Result<()> {
    write_with(item, path, id3v23())
}

/// beets' `Item.write`: every media field into the file's tags, MP3s'
/// ID3 tags as ID3v2.3 with `id3v23`.
pub fn write_with(item: &Item, path: &Path, id3v23: bool) -> Result<()> {
    write_event(item, path, id3v23, Extra::None, &BTreeSet::new())?;
    Ok(())
}

/// beets' `item.try_write(tags={"synced_lyrics": ...})`: the item's tags
/// and MP3s' SYLT frame (removed for `None`).
pub fn write_with_sylt(item: &Item, sylt: Option<&[(String, u32)]>) -> Result<()> {
    write_event(
        item,
        item.path(),
        id3v23(),
        Extra::Sylt(sylt),
        &BTreeSet::new(),
    )?;
    Ok(())
}

static ID3V23: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The `id3v23` option, set from the configuration at startup.
pub fn set_id3v23(on: bool) {
    ID3V23.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn id3v23() -> bool {
    ID3V23.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Length and bitrate are mutagen's exact values, not lofty's ms and
    /// kbps: lossless lengths are samples over the rate (1.37 s exactly),
    /// PCM bitrates channels x bits x rate, MP3 lengths whole frames over
    /// the rate, FLAC's bitrate from the audio after the metadata.
    #[test]
    fn exact_length_and_bitrate() {
        let d = tempfile::tempdir().unwrap();
        let make = |name: &str, args: &[&str]| {
            let p = d.path().join(name);
            std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=1.37"])
                .args(args)
                .arg(&p)
                .status()
                .is_ok_and(|s| s.success())
                .then(|| read(&p).unwrap())
        };
        let Some(wav) = make("t.wav", &[]) else {
            return;
        };
        assert_eq!((wav.float("length"), wav.int("bitrate")), (1.37, 705_600));
        let aiff = make("t.aiff", &["-ac", "2"]).unwrap();
        assert_eq!(
            (aiff.float("length"), aiff.int("bitrate")),
            (1.37, 1_411_200)
        );
        for name in ["t.flac", "t.wv", "t.ogg", "t.opus"] {
            assert_eq!(make(name, &[]).unwrap().float("length"), 1.37, "{name}");
        }
        let flac = make(
            "big.flac",
            &["-metadata", &format!("comment={}", "x".repeat(5000))],
        );
        let flac = flac.unwrap();
        let size = std::fs::metadata(flac.path()).unwrap().len();
        // The tags aren't counted.
        assert!(flac.int("bitrate") < (size as f64 * 8.0 / 1.37) as i64 - 5000 * 8);
        let mp3 = make("t.mp3", &["-b:a", "64k"]).unwrap();
        let samples = mp3.float("length") * 44100.0;
        assert_eq!(samples, (samples / 1152.0).round() * 1152.0);
    }

    /// MediaFile reads APEv2 `date` and `album_artist` (as ffmpeg writes
    /// them) case-insensitively; a WAV file's format is "WAVE".
    #[test]
    fn wavpack_date_and_album_artist_and_wave_format() {
        let d = tempfile::tempdir().unwrap();
        let wv = d.path().join("t.wv");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.5"])
            .args(["-metadata", "date=2024-05", "-metadata", "album_artist=AA"])
            .arg(&wv)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        let it = read(&wv).unwrap();
        assert_eq!((it.int("year"), it.int("month")), (2024, 5));
        assert_eq!(it.str("albumartist"), "AA");
        let Some((_d, wav)) = make("wav") else { return };
        assert_eq!(read(&wav).unwrap().str("format"), "WAVE");
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
            let n: u32 = file.tags().iter().map(lofty::tag::Tag::picture_count).sum();
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
