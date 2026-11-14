//! ASF (Advanced Systems Format, "Windows Media") tags: the header
//! object's Content Description, Extended Content Description, Metadata
//! and Metadata Library objects, written from Microsoft's "Advanced
//! Systems Format (ASF) Specification".
//!
//! Values are stored under MediaFile's attribute names (`Title`,
//! `WM/AlbumTitle`, `MusicBrainz/Album Id`, ...; see `asf_styles` in
//! `mediafile`). As Mutagen does, on save the five Content Description
//! attributes stay in that object, the first value of every other
//! attribute goes in the Extended Content Description Object and any
//! further values in the Metadata Library Object (which has a 32-bit
//! value length). Unknown header objects are kept byte for byte, and
//! audio data is never touched: a new header whose size still fits the
//! old one is written in place, padded to the old size; otherwise the
//! header is rewritten and the data object copied after it, with the
//! File Properties object's file size updated.
//!
//! A file with a corrupt header is an error, never a panic: every
//! length and offset is checked against the object that contains it,
//! and object and attribute counts are capped.

use anyhow::{Context, Result, bail};
use lofty::properties::FileProperties;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// A header object's GUID and body, as parsed (and kept for unknown
/// objects).
type RawObject = ([u8; 16], Vec<u8>);

// ------------------------------------------------------------------ GUIDs

/// The Header Object (`75B22630-668E-11CF-A6D9-00AA0062CE6C`).
pub const HEADER_GUID: [u8; 16] = [
    0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce, 0x6c,
];
/// `8CABDCA1-A947-11CF-8EE4-00C00C205365`.
const FILE_PROPERTIES_GUID: [u8; 16] = [
    0xa1, 0xdc, 0xab, 0x8c, 0x47, 0xa9, 0xcf, 0x11, 0x8e, 0xe4, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
/// `B7DC0791-A9B7-11CF-8EE6-00C00C205365`.
const STREAM_PROPERTIES_GUID: [u8; 16] = [
    0x91, 0x07, 0xdc, 0xb7, 0xb7, 0xa9, 0xcf, 0x11, 0x8e, 0xe6, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
/// `5FBF03B5-A92E-11CF-8EE3-00C00C205365`.
const HEADER_EXTENSION_GUID: [u8; 16] = [
    0xb5, 0x03, 0xbf, 0x5f, 0x2e, 0xa9, 0xcf, 0x11, 0x8e, 0xe3, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
/// `75B22633-668E-11CF-A6D9-00AA0062CE6C`.
const CONTENT_DESCRIPTION_GUID: [u8; 16] = [
    0x33, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce, 0x6c,
];
/// `D2D0A440-E307-11D2-97F0-00A0C95EA850`.
const EXTENDED_CONTENT_DESCRIPTION_GUID: [u8; 16] = [
    0x40, 0xa4, 0xd0, 0xd2, 0x07, 0xe3, 0xd2, 0x11, 0x97, 0xf0, 0x00, 0xa0, 0xc9, 0x5e, 0xa8, 0x50,
];
/// `C5F8CBEA-5BAF-4877-8467-AA8C44FA4CCA`.
const METADATA_GUID: [u8; 16] = [
    0xea, 0xcb, 0xf8, 0xc5, 0xaf, 0x5b, 0x77, 0x48, 0x84, 0x67, 0xaa, 0x8c, 0x44, 0xfa, 0x4c, 0xca,
];
/// `44231C94-9498-49D1-A141-1D134E457054`.
const METADATA_LIBRARY_GUID: [u8; 16] = [
    0x94, 0x1c, 0x23, 0x44, 0x98, 0x94, 0xd1, 0x49, 0xa1, 0x41, 0x1d, 0x13, 0x4e, 0x45, 0x70, 0x54,
];
/// `1806D474-CDF4-420A-9F41-50B05132B05A` (the Padding Object).
const PADDING_GUID: [u8; 16] = [
    0x74, 0xd4, 0x06, 0x18, 0xdf, 0xca, 0x09, 0x45, 0xa4, 0xba, 0x9a, 0xab, 0xcb, 0x96, 0xaa, 0xe8,
];
/// The Header Extension Object's reserved GUID (`ASF_Reserved_1`).
const HEADER_EXTENSION_RESERVED_GUID: [u8; 16] = [
    0x11, 0xd2, 0xd3, 0xab, 0xba, 0xa9, 0xcf, 0x11, 0x8e, 0xe6, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
/// `F8699E40-5B4D-11CF-A8FD-00805F5C442B`: an audio media stream.
const AUDIO_MEDIA_GUID: [u8; 16] = [
    0x40, 0x9e, 0x69, 0xf8, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b,
];

/// The five Content Description Object attributes, in the object's field
/// order.
pub const CONTENT_DESCRIPTION_NAMES: [&str; 5] =
    ["Title", "Author", "Copyright", "Description", "Rating"];

const GUID_LEN: usize = 16;
const OBJECT_HEADER_LEN: u64 = 24;
/// No real file has thousands of header objects.
const MAX_OBJECTS: usize = 10_000;
/// A header bigger than this is corrupt (real headers are kilobytes).
const MAX_HEADER: u64 = 64 << 20;
/// Sanity cap on the attributes of one object.
const MAX_ATTRIBUTES: usize = 100_000;

// ----------------------------------------------------------------- values

/// An ASF attribute value, by its data type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Data type 0: a UTF-16LE string with a terminating NUL.
    Unicode(String),
    /// 1: a byte array.
    Bytes(Vec<u8>),
    /// 2: a 32-bit boolean (0 or 1).
    Bool(bool),
    /// 3.
    DWord(u32),
    /// 4.
    QWord(u64),
    /// 5.
    Word(u16),
    /// 6: a 16-byte GUID.
    Guid([u8; 16]),
    /// Any other data type, kept verbatim.
    Unknown(u16, Vec<u8>),
}

impl Value {
    fn type_id(&self) -> u16 {
        match self {
            Value::Unicode(_) => 0,
            Value::Bytes(_) => 1,
            Value::Bool(_) => 2,
            Value::DWord(_) => 3,
            Value::QWord(_) => 4,
            Value::Word(_) => 5,
            Value::Guid(_) => 6,
            Value::Unknown(t, _) => *t,
        }
    }

    /// The attribute's data bytes (without the length prefix).
    fn encode(&self) -> Vec<u8> {
        match self {
            Value::Unicode(s) => {
                let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
                out.extend_from_slice(&[0, 0]);
                out
            }
            Value::Bytes(b) | Value::Unknown(_, b) => b.clone(),
            Value::Bool(b) => u32::from(*b).to_le_bytes().to_vec(),
            Value::DWord(v) => v.to_le_bytes().to_vec(),
            Value::QWord(v) => v.to_le_bytes().to_vec(),
            Value::Word(v) => v.to_le_bytes().to_vec(),
            Value::Guid(g) => g.to_vec(),
        }
    }

    /// The value as MediaFile's `ASFStorageStyle` sees it: an int-typed
    /// attribute becomes its number, a bool its flag.
    pub fn as_text(&self) -> String {
        match self {
            Value::Unicode(s) => s.clone(),
            Value::Bytes(b) | Value::Unknown(_, b) => String::from_utf8_lossy(b).into_owned(),
            Value::Bool(b) => i64::from(*b).to_string(),
            Value::DWord(v) => v.to_string(),
            Value::QWord(v) => v.to_string(),
            Value::Word(v) => v.to_string(),
            Value::Guid(g) => g.iter().map(|b| format!("{b:02x}")).collect(),
        }
    }

    fn decode(type_id: u16, data: &[u8]) -> Value {
        let le = |n: usize| -> u64 {
            data.iter()
                .take(n)
                .rev()
                .fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
        };
        match type_id {
            0 => {
                let units: Vec<u16> = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                let s = String::from_utf16_lossy(&units);
                Value::Unicode(s.trim_end_matches('\0').to_string())
            }
            1 => Value::Bytes(data.to_vec()),
            2 => Value::Bool(le(4) != 0),
            3 => Value::DWord(le(4) as u32),
            4 => Value::QWord(le(8)),
            5 => Value::Word(le(2) as u16),
            6 => {
                let mut g = [0u8; 16];
                let n = data.len().min(16);
                g[..n].copy_from_slice(&data[..n]);
                Value::Guid(g)
            }
            other => Value::Unknown(other, data.to_vec()),
        }
    }
}

/// An Extended Content Description attribute.
#[derive(Clone, Debug)]
struct Attr {
    name: String,
    value: Value,
}

/// A Metadata Object / Metadata Library Object attribute.
#[derive(Clone, Debug)]
struct MetaAttr {
    language: u16,
    stream: u16,
    name: String,
    value: Value,
}

// ---------------------------------------------------------------- parsing

/// A cursor over an object body: every read checks the remaining length.
struct Cursor<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.d.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A UTF-16LE string of `bytes` bytes (a terminating NUL is dropped).
    fn utf16(&mut self, bytes: usize) -> Option<String> {
        let raw = self.take(bytes)?;
        Some(utf16(raw))
    }
}

fn utf16(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_string()
}

/// The children of an object body that starts with an object count and
/// two reserved bytes (`Header Object`): (guid, body), until `end`.
fn children(d: &[u8]) -> Option<Vec<RawObject>> {
    let mut c = Cursor::new(d);
    let count = c.u32()?;
    c.take(2)?;
    let mut out = Vec::new();
    let mut seen = 0u32;
    while seen < count && out.len() < MAX_OBJECTS {
        let at = c.pos;
        let guid: [u8; 16] = c.take(GUID_LEN)?.try_into().ok()?;
        let size = c.u64()?;
        if size < OBJECT_HEADER_LEN {
            // A zero-filled tail after the last object, as some writers
            // leave: stop rather than call the file corrupt.
            if d[at..].iter().all(|b| *b == 0) {
                break;
            }
            return None;
        }
        let len = usize::try_from(size - OBJECT_HEADER_LEN).ok()?;
        let body = c.take(len)?.to_vec();
        out.push((guid, body));
        seen += 1;
    }
    Some(out)
}

/// Header-extension children: a plain sequence of objects (no count).
fn inner_children(d: &[u8]) -> Option<Vec<RawObject>> {
    let mut c = Cursor::new(d);
    let mut out = Vec::new();
    while c.remaining() >= OBJECT_HEADER_LEN as usize && out.len() < MAX_OBJECTS {
        let guid: [u8; 16] = c.take(GUID_LEN)?.try_into().ok()?;
        let size = c.u64()?;
        if size < OBJECT_HEADER_LEN {
            if d[c.pos - OBJECT_HEADER_LEN as usize..]
                .iter()
                .all(|b| *b == 0)
            {
                break;
            }
            return None;
        }
        let len = usize::try_from(size - OBJECT_HEADER_LEN).ok()?;
        let body = c.take(len)?.to_vec();
        out.push((guid, body));
    }
    Some(out)
}

/// The Content Description Object: five strings.
fn content_description(body: &[u8]) -> Option<[Option<String>; 5]> {
    let mut c = Cursor::new(body);
    let mut lens = [0usize; 5];
    for l in &mut lens {
        *l = usize::from(c.u16()?);
    }
    let mut out: [Option<String>; 5] = Default::default();
    for (i, l) in lens.iter().enumerate() {
        if *l > 0 {
            out[i] = Some(c.utf16(*l)?);
        }
    }
    Some(out)
}

/// The Extended Content Description Object: a count, then named values
/// (16-bit value lengths).
fn extended_content_description(body: &[u8]) -> Option<Vec<Attr>> {
    let mut c = Cursor::new(body);
    let count = c.u16()?;
    let mut out = Vec::new();
    for _ in 0..count.min(MAX_ATTRIBUTES as u16) {
        let name_len = usize::from(c.u16()?);
        if name_len % 2 != 0 || name_len > c.remaining() {
            return None;
        }
        let name = c.utf16(name_len)?;
        let type_id = c.u16()?;
        let len = usize::from(c.u16()?);
        let data = c.take(len)?;
        out.push(Attr {
            name,
            value: Value::decode(type_id, data),
        });
    }
    Some(out)
}

/// A Metadata Object / Metadata Library Object: a count, then records
/// with a language index, stream number, name and a 32-bit value length.
fn metadata(body: &[u8]) -> Option<Vec<MetaAttr>> {
    let mut c = Cursor::new(body);
    let count = c.u16()?;
    let mut out = Vec::new();
    for _ in 0..count.min(MAX_ATTRIBUTES as u16) {
        let language = c.u16()?;
        let stream = c.u16()?;
        let name_len = usize::from(c.u16()?);
        let type_id = c.u16()?;
        let len = usize::try_from(c.u32()?).ok()?;
        if name_len % 2 != 0 || name_len > c.remaining() {
            return None;
        }
        let name = c.utf16(name_len)?;
        let data = c.take(len)?;
        out.push(MetaAttr {
            language,
            stream,
            name,
            value: Value::decode(type_id, data),
        });
    }
    Some(out)
}

/// A parsed header: the tag stores, plus everything kept verbatim.
struct Header {
    /// The File Properties Object body (file size patched on save).
    file_properties: Option<Vec<u8>>,
    /// Stream Properties Object bodies, for the audio properties.
    stream_properties: Vec<Vec<u8>>,
    content: [Option<String>; 5],
    extended: Vec<Attr>,
    metadata: Vec<MetaAttr>,
    metadata_library: Vec<MetaAttr>,
    /// Header Extension Object prologue GUID and reserved word.
    extension: Option<([u8; 16], u16)>,
    /// Other top-level header objects, kept as they are.
    objects: Vec<RawObject>,
    /// Other header-extension objects, kept as they are.
    extension_objects: Vec<RawObject>,
    /// The old header's total size (where the data object starts).
    size: u64,
}

/// An ASF file's tags, with its header parsed and its audio data left on
/// disk.
pub struct Asf {
    path: PathBuf,
    header: Header,
}

/// Whether `path` starts with the ASF Header Object's GUID.
pub fn is_asf(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 16];
    f.read_exact(&mut magic).is_ok() && magic == HEADER_GUID
}

impl Asf {
    /// Parse `path`'s header object.
    pub fn open(path: &Path) -> Result<Asf> {
        let ctx = || format!("reading {}", path.display());
        let mut f = std::fs::File::open(path).with_context(ctx)?;
        let file_len = f.metadata().with_context(ctx)?.len();
        let mut head = [0u8; 16];
        f.read_exact(&mut head).with_context(ctx)?;
        if head != HEADER_GUID {
            bail!("not an ASF file: {}", path.display());
        }
        let mut size = [0u8; 8];
        f.read_exact(&mut size).with_context(ctx)?;
        let size = u64::from_le_bytes(size);
        if size < OBJECT_HEADER_LEN + 6 || size > file_len || size > MAX_HEADER {
            bail!("invalid ASF header size {size}: {}", path.display());
        }
        let body_len = usize::try_from(size - OBJECT_HEADER_LEN)?;
        let mut body = vec![0u8; body_len];
        f.read_exact(&mut body).with_context(ctx)?;
        let kids = children(&body).with_context(|| format!("parsing {}", path.display()))?;
        let mut header = Header {
            file_properties: None,
            stream_properties: vec![],
            content: Default::default(),
            extended: vec![],
            metadata: vec![],
            metadata_library: vec![],
            extension: None,
            objects: vec![],
            extension_objects: vec![],
            size,
        };
        for (guid, obj) in kids {
            match guid {
                FILE_PROPERTIES_GUID => header.file_properties = Some(obj),
                STREAM_PROPERTIES_GUID => header.stream_properties.push(obj),
                CONTENT_DESCRIPTION_GUID => {
                    let values = content_description(&obj)
                        .with_context(|| format!("parsing {}", path.display()))?;
                    for (slot, value) in header.content.iter_mut().zip(values) {
                        if value.is_some() {
                            *slot = value;
                        }
                    }
                }
                EXTENDED_CONTENT_DESCRIPTION_GUID => {
                    let attrs = extended_content_description(&obj)
                        .with_context(|| format!("parsing {}", path.display()))?;
                    header.extended.extend(attrs);
                }
                HEADER_EXTENSION_GUID => {
                    let Some((reserved, word, inner)) = extension_prologue(&obj) else {
                        continue;
                    };
                    header.extension = Some((reserved, word));
                    for (g, o) in inner {
                        match g {
                            METADATA_GUID => {
                                let records = metadata(&o)
                                    .with_context(|| format!("parsing {}", path.display()))?;
                                header.metadata.extend(records);
                            }
                            METADATA_LIBRARY_GUID => {
                                let records = metadata(&o)
                                    .with_context(|| format!("parsing {}", path.display()))?;
                                header.metadata_library.extend(records);
                            }
                            PADDING_GUID => {}
                            _ => header.extension_objects.push((g, o)),
                        }
                    }
                }
                PADDING_GUID => {}
                _ => header.objects.push((guid, obj)),
            }
        }
        Ok(Asf {
            path: path.to_path_buf(),
            header,
        })
    }

    /// The values stored under `name`, in the order beets reads them: the
    /// Content Description field first, then the Extended Content
    /// Description, the Metadata Object and the Metadata Library.
    pub fn values(&self, name: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if let Some(i) = content_index(name)
            && let Some(s) = &self.header.content[i]
        {
            out.push(Value::Unicode(s.clone()));
        }
        out.extend(
            self.header
                .extended
                .iter()
                .filter(|a| a.name == name)
                .map(|a| a.value.clone()),
        );
        for record in self
            .header
            .metadata
            .iter()
            .chain(&self.header.metadata_library)
        {
            if record.name == name {
                out.push(record.value.clone());
            }
        }
        out
    }

    /// Replace every value stored under `name`. Content Description names
    /// go to that object; other names' first value goes to the Extended
    /// Content Description Object and the rest to the Metadata Library.
    pub fn set(&mut self, name: &str, values: Vec<Value>) {
        self.remove(name);
        if values.is_empty() || name.encode_utf16().count() * 2 > usize::from(u16::MAX) {
            return;
        }
        if let Some(i) = content_index(name) {
            self.header.content[i] = Some(values[0].as_text());
            return;
        }
        for (i, value) in values.into_iter().enumerate() {
            let attr = Attr {
                name: name.to_string(),
                value,
            };
            let fits = attr.name.len() * 2 <= usize::from(u16::MAX)
                && attr.value.encode().len() <= usize::from(u16::MAX);
            if i == 0 && fits {
                self.header.extended.push(attr);
            } else {
                self.header.metadata_library.push(MetaAttr {
                    language: 0,
                    stream: 0,
                    name: attr.name,
                    value: attr.value,
                });
            }
        }
    }

    /// Remove every value stored under `name`.
    pub fn remove(&mut self, name: &str) {
        if let Some(i) = content_index(name) {
            self.header.content[i] = None;
        }
        self.header.extended.retain(|a| a.name != name);
        self.header.metadata.retain(|a| a.name != name);
        self.header.metadata_library.retain(|a| a.name != name);
    }

    /// Remove every tag (the `scrub` plugin's `del f[tag]` loop).
    pub fn remove_all(&mut self) {
        self.header.content = Default::default();
        self.header.extended.clear();
        self.header.metadata.clear();
        self.header.metadata_library.clear();
    }

    /// Mutagen's `ASFInfo` values as lofty's properties: the length from
    /// the File Properties Object (play duration in 100 ns minus the
    /// preroll in ms), the bitrate from the first audio stream's average
    /// byte rate (0 when it states none: MediaFile then estimates it from
    /// the file size), and the sample rate and channel count from that
    /// stream's `WAVEFORMATEX`. There is no bit depth: beets reads 0.
    pub fn properties(&self) -> FileProperties {
        let length = self.header.file_properties.as_deref().map_or(0.0, |b| {
            let u64_at = |at: usize| -> u64 {
                b.get(at..at + 8)
                    .and_then(|s| s.try_into().ok())
                    .map_or(0, u64::from_le_bytes)
            };
            u64_at(40) as f64 / 1e7 - u64_at(56) as f64 / 1000.0
        });
        let mut sample_rate = None;
        let mut channels = None;
        let mut bitrate = 0i64;
        for stream in &self.header.stream_properties {
            let Some(audio) = audio_stream(stream) else {
                continue;
            };
            sample_rate = Some(audio.sample_rate);
            channels = Some(audio.channels);
            bitrate = i64::from(audio.byte_rate) * 8;
            break;
        }
        let kbps: Option<u32> = (bitrate / 1000).try_into().ok().filter(|b| *b != 0);
        let duration = std::time::Duration::from_secs_f64(length.max(0.0));
        FileProperties::new(
            duration,
            kbps,
            kbps,
            sample_rate.filter(|r| *r != 0),
            None,
            channels.and_then(|c| c.try_into().ok()).filter(|c| *c != 0),
            None,
        )
    }

    /// Rewrite the header, leaving the audio data alone.
    pub fn save(self) -> Result<()> {
        let ctx = || format!("writing tags to {}", self.path.display());
        let mut header = self.header;
        // The data object and packets, exactly as they are on disk.
        let old_len = std::fs::metadata(&self.path).with_context(ctx)?.len();
        let trailer_len = old_len
            .checked_sub(header.size)
            .with_context(|| format!("invalid ASF header size {}", self.path.display()))?;
        let body = rebuild(&mut header, old_len, 0);
        // A header that still fits the old one is written in place (data
        // object offset and file size unchanged), padded to the old size.
        if let Some(diff) = header.size.checked_sub(body.len() as u64)
            && (diff == 0 || diff >= OBJECT_HEADER_LEN)
        {
            let body = if diff == 0 {
                body
            } else {
                rebuild(&mut header, old_len, usize::try_from(diff).unwrap_or(0))
            };
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .open(&self.path)
                .with_context(ctx)?;
            f.write_all(&body).with_context(ctx)?;
            return Ok(());
        }
        // Otherwise grow or shrink the header and move the data object.
        let new_len = old_len - header.size + body.len() as u64;
        let body = rebuild(&mut header, new_len, 0);
        let mut out = write_temp(&self.path, &body)?;
        let mut src = std::fs::File::open(&self.path).with_context(ctx)?;
        src.seek(SeekFrom::Start(header.size)).with_context(ctx)?;
        std::io::copy(&mut src.take(trailer_len), &mut out).with_context(ctx)?;
        out.flush().with_context(ctx)?;
        drop(out);
        let model = std::fs::metadata(&self.path).with_context(ctx)?;
        let _ = std::fs::set_permissions(body_path(&self.path), model.permissions());
        std::fs::rename(body_path(&self.path), &self.path).with_context(ctx)?;
        Ok(())
    }
}

/// The Header Extension Object prologue: (reserved GUID, word, children).
fn extension_prologue(body: &[u8]) -> Option<([u8; 16], u16, Vec<RawObject>)> {
    let mut c = Cursor::new(body);
    let reserved: [u8; 16] = c.take(16)?.try_into().ok()?;
    let word = c.u16()?;
    let _size = c.u32()?;
    let inner = inner_children(&body[c.pos..])?;
    Some((reserved, word, inner))
}

fn content_index(name: &str) -> Option<usize> {
    CONTENT_DESCRIPTION_NAMES.iter().position(|n| *n == name)
}

/// An audio stream's `WAVEFORMATEX` fields.
pub(crate) struct AudioStream {
    pub sample_rate: u32,
    pub channels: u16,
    pub byte_rate: u32,
}

/// The first audio stream's `WAVEFORMATEX`, if the Stream Properties
/// Object is one.
pub(crate) fn audio_stream(body: &[u8]) -> Option<AudioStream> {
    if body.get(..16)? != AUDIO_MEDIA_GUID {
        return None;
    }
    // Two GUIDs, time offset, type-specific and error-correction data
    // lengths, flags and a reserved DWORD: the type data starts at 54.
    let type_len = body.get(40..44)?.try_into().ok().map(u32::from_le_bytes)?;
    if type_len < 14 {
        return None;
    }
    let t = body.get(54..)?;
    Some(AudioStream {
        channels: u16::from_le_bytes(t.get(2..4)?.try_into().ok()?),
        sample_rate: u32::from_le_bytes(t.get(4..8)?.try_into().ok()?),
        byte_rate: u32::from_le_bytes(t.get(8..12)?.try_into().ok()?),
    })
}

// --------------------------------------------------------------- writing

fn object(guid: &[u8; 16], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + body.len());
    out.extend_from_slice(guid);
    out.extend_from_slice(&(24 + body.len() as u64).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn utf16_bytes(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.extend_from_slice(&[0, 0]);
    out
}

fn content_body(content: &[Option<String>; 5]) -> Vec<u8> {
    let strings: Vec<Vec<u8>> = content
        .iter()
        .map(|s| s.as_deref().map_or_else(Vec::new, utf16_bytes))
        .collect();
    let mut out = Vec::new();
    for s in &strings {
        out.extend_from_slice(&u16::try_from(s.len()).unwrap_or(0).to_le_bytes());
    }
    for s in strings {
        out.extend_from_slice(&s);
    }
    out
}

fn extended_body(attrs: &[Attr]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&u16::try_from(attrs.len()).unwrap_or(0).to_le_bytes());
    for a in attrs {
        let name = utf16_bytes(&a.name);
        let data = a.value.encode();
        out.extend_from_slice(&u16::try_from(name.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&name);
        out.extend_from_slice(&a.value.type_id().to_le_bytes());
        out.extend_from_slice(&u16::try_from(data.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&data);
    }
    out
}

fn metadata_body(attrs: &[MetaAttr]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&u16::try_from(attrs.len()).unwrap_or(0).to_le_bytes());
    for a in attrs {
        let name = utf16_bytes(&a.name);
        let data = a.value.encode();
        out.extend_from_slice(&a.language.to_le_bytes());
        out.extend_from_slice(&a.stream.to_le_bytes());
        out.extend_from_slice(&u16::try_from(name.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&a.value.type_id().to_le_bytes());
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&name);
        out.extend_from_slice(&data);
    }
    out
}

/// Build the header object, patching the File Properties Object's file
/// size to `file_size`; `pad` adds a Padding Object of that many bytes to
/// the header extension (which keeps the data object's offset).
fn rebuild(header: &mut Header, file_size: u64, pad: usize) -> Vec<u8> {
    let mut top: Vec<Vec<u8>> = Vec::new();
    if let Some(props) = &mut header.file_properties {
        if props.len() >= 24 {
            props[16..24].copy_from_slice(&file_size.to_le_bytes());
        }
        top.push(object(&FILE_PROPERTIES_GUID, props));
    }
    for stream in &header.stream_properties {
        top.push(object(&STREAM_PROPERTIES_GUID, stream));
    }
    top.push(object(
        &CONTENT_DESCRIPTION_GUID,
        &content_body(&header.content),
    ));
    top.push(object(
        &EXTENDED_CONTENT_DESCRIPTION_GUID,
        &extended_body(&header.extended),
    ));
    for (guid, body) in &header.objects {
        top.push(object(guid, body));
    }
    let mut ext_children: Vec<Vec<u8>> = Vec::new();
    if !header.metadata.is_empty() {
        ext_children.push(object(&METADATA_GUID, &metadata_body(&header.metadata)));
    }
    if !header.metadata_library.is_empty() {
        ext_children.push(object(
            &METADATA_LIBRARY_GUID,
            &metadata_body(&header.metadata_library),
        ));
    }
    for (guid, body) in &header.extension_objects {
        ext_children.push(object(guid, body));
    }
    if pad >= OBJECT_HEADER_LEN as usize {
        ext_children.push(object(
            &PADDING_GUID,
            &vec![0u8; pad - OBJECT_HEADER_LEN as usize],
        ));
    }
    let has_ext = header.extension.is_some() || !ext_children.is_empty();
    if has_ext {
        let (guid, word) = header
            .extension
            .unwrap_or((HEADER_EXTENSION_RESERVED_GUID, 6));
        let mut body = Vec::new();
        body.extend_from_slice(&guid);
        body.extend_from_slice(&word.to_le_bytes());
        let data_len: usize = ext_children.iter().map(Vec::len).sum();
        body.extend_from_slice(&u32::try_from(data_len).unwrap_or(0).to_le_bytes());
        for child in ext_children {
            body.extend_from_slice(&child);
        }
        top.push(object(&HEADER_EXTENSION_GUID, &body));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&HEADER_GUID);
    let total: usize = OBJECT_HEADER_LEN as usize + 6 + top.iter().map(Vec::len).sum::<usize>();
    out.extend_from_slice(&(total as u64).to_le_bytes());
    out.extend_from_slice(&(top.len() as u32).to_le_bytes());
    out.extend_from_slice(&[1, 2]);
    for child in top {
        out.extend_from_slice(&child);
    }
    out
}

/// A temporary file beside `path` (same filesystem, so the rename is
/// atomic).
fn body_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".asf-tmp");
    path.with_file_name(name)
}

fn write_temp(path: &Path, header: &[u8]) -> Result<std::fs::File> {
    let tmp = body_path(path);
    let mut f =
        std::fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(header)
        .with_context(|| format!("writing {}", tmp.display()))?;
    Ok(f)
}

#[cfg(test)]
#[path = "asf_tests.rs"]
mod tests;
