//! A port of the `mediafile` package's tag mapping (MediaFile 0.17, as
//! beets 2.14 uses it): which tag holds each field in each format, how
//! values are read (the first storage style with a value wins) and how
//! they are written (every writable style, field by field in MediaFile's
//! order), on top of lofty's format-specific tags.
//!
//! Formats: ID3v2 (MP3, AIFF, WAV; ID3v2.3 on request, as `id3v23`), MP4
//! atoms (`----:com.apple.iTunes:` freeform atoms for the rest), Vorbis
//! comments (FLAC, Ogg Vorbis, Opus, Speex) and APEv2 (WavPack, Monkey's
//! Audio, Musepack). ASF (WMA) isn't supported by lofty.
//!
//! Deliberate difference: where beets writes an empty string, 0 or an empty
//! list as an empty tag (`TITLE=`, `TRACKNUMBER=0`, a `----` atom without
//! data), beets-rs removes the tag. MediaFile reads both the same way.

use crate::model::{Item, Value};
use anyhow::{Context, Result, bail};
use lofty::TextEncoding;
use lofty::ape::{ApeItem, ApeTag};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::id3::v1::Id3v1Tag;
use lofty::id3::v2::{
    BinaryFrame, CommentFrame, ExtendedTextFrame, Frame, FrameId, Id3v2Tag, Id3v2Version,
    KeyValueFrame, SyncTextContentType, SynchronizedTextFrame, TextInformationFrame,
    TimestampFormat, TimestampFrame, UniqueFileIdentifierFrame, UnsynchronizedTextFrame,
};
use lofty::mp4::{Atom, AtomData, AtomIdent, DataType, Ilst};
use lofty::ogg::OggPictureStorage;
use lofty::ogg::tag::VorbisComments;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::properties::FileProperties;
use lofty::tag::{Accessor, ItemValue, TagExt};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------ storage styles

/// One of MediaFile's `StorageStyle`s: where a field lives in one format.
#[derive(Clone, Copy, Debug)]
enum K {
    // ID3v2.
    /// `MP3StorageStyle` / `MP3ListStorageStyle`: a text frame.
    Frame(&'static str),
    /// `MP3DescStorageStyle`: the `TXXX` frame with this description.
    Txxx(&'static str),
    /// `MP3ListDescStorageStyle(desc, split_v23)`.
    TxxxList(&'static str, bool),
    /// `MP3DescStorageStyle(key="COMM")`: the comment with this description.
    Comm(&'static str),
    /// `MP3DescStorageStyle(key="USLT", multispec=False)`.
    Uslt,
    /// `MP3DescStorageStyle(key="WXXX", attr="url", multispec=False)`
    /// (read only here: `url` isn't an item field).
    Wxxx,
    /// `MP3UFIDStorageStyle(owner)`.
    Ufid(&'static str),
    /// `MP3ListPeopleStorageStyle(frame, involvement)`.
    People(&'static str, &'static str),
    /// `MP3SlashPackStorageStyle(frame, pack_pos)`.
    SlashPack(&'static str, usize),
    /// `MP3SoundCheckStorageStyle`: half of `COMM:iTunNORM:eng`.
    Id3Sc(usize),
    // MP4.
    /// `MP4StorageStyle` / `MP4ListStorageStyle`.
    Atom(&'static str),
    /// `MP4TupleStorageStyle(key, index)`.
    Tuple(&'static str, usize),
    /// `MP4BoolStorageStyle`.
    Mp4Bool(&'static str),
    /// `MP4StorageStyle(key, as_type=int)`.
    Mp4Int(&'static str),
    /// `MP4SoundCheckStorageStyle`: half of `----:com.apple.iTunes:iTunNORM`.
    Mp4Sc(usize),
    // Vorbis comments and APEv2 (MediaFile's plain `StorageStyle`).
    Key(&'static str),
    /// `ASFStorageStyle`: an ASF attribute name (`WM/AlbumTitle`, ...).
    Asf(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Id3,
    Mp4,
    Plain,
    Asf,
}

impl K {
    fn family(self) -> Family {
        match self {
            K::Frame(_)
            | K::Txxx(_)
            | K::TxxxList(..)
            | K::Comm(_)
            | K::Uslt
            | K::Wxxx
            | K::Ufid(_)
            | K::People(..)
            | K::SlashPack(..)
            | K::Id3Sc(_) => Family::Id3,
            K::Atom(_) | K::Tuple(..) | K::Mp4Bool(_) | K::Mp4Int(_) | K::Mp4Sc(_) => Family::Mp4,
            K::Key(_) => Family::Plain,
            K::Asf(_) => Family::Asf,
        }
    }

    /// The pair-packed styles, where a 0 is still written while the other
    /// half of the pair is set.
    fn packed(self) -> bool {
        matches!(self, K::SlashPack(..) | K::Tuple(..))
    }
}

#[derive(Clone, Copy, Debug)]
struct S {
    k: K,
    read_only: bool,
    /// `float_places`.
    places: usize,
    suffix: &'static str,
}

const fn s(k: K) -> S {
    S {
        k,
        read_only: false,
        places: 2,
        suffix: "",
    }
}

const fn ro(k: K) -> S {
    S {
        k,
        read_only: true,
        places: 2,
        suffix: "",
    }
}

/// ReplayGain gain: two places and " dB".
const fn gain(k: K) -> S {
    S {
        k,
        read_only: false,
        places: 2,
        suffix: " dB",
    }
}

/// ReplayGain peak: six places.
const fn peak(k: K) -> S {
    S {
        k,
        read_only: false,
        places: 6,
        suffix: "",
    }
}

/// A MediaFile field's output type.
#[derive(Clone, Copy, Debug)]
enum Ty {
    Str,
    Int,
    Bool,
    Float,
    /// `ListMediaField`.
    List,
    /// `QNumberField(8)`: an integer holding a fraction in 1/256ths.
    QNum,
    /// `DateField` (year, month and day are written together); the extra
    /// styles are the year fallback (`year=` styles).
    Date(&'static [S]),
}

struct Field {
    name: &'static str,
    ty: Ty,
    styles: &'static [S],
}

const fn f(name: &'static str, ty: Ty, styles: &'static [S]) -> Field {
    Field { name, ty, styles }
}

use K::*;
use Ty::*;

const ITUNES: &str = "----:com.apple.iTunes:";

macro_rules! itunes {
    ($name:literal) => {
        concat!("----:com.apple.iTunes:", $name)
    };
}

/// A `MediaField` whose styles are the usual `TXXX:<desc>`, `----:<desc>`
/// and `<KEY>`.
macro_rules! desc_field {
    ($name:literal, $ty:expr, $desc:literal, $key:literal) => {
        f(
            $name,
            $ty,
            &[s(Txxx($desc)), s(Atom(itunes!($desc))), s(Key($key))],
        )
    };
}

/// The same as a list (`MP3ListDescStorageStyle`).
macro_rules! list_desc_field {
    ($name:literal, $desc:literal, $key:literal, $split:literal) => {
        f(
            $name,
            List,
            &[
                s(TxxxList($desc, $split)),
                s(Atom(itunes!($desc))),
                s(Key($key)),
            ],
        )
    };
}

const DATE_STYLES: &[S] = &[s(Frame("TDRC")), s(Atom("\u{a9}day")), s(Key("DATE"))];
const ORIGINAL_DATE_STYLES: &[S] = &[
    s(Frame("TDOR")),
    s(Atom(itunes!("ORIGINAL YEAR"))),
    s(Atom(itunes!("ORIGINALDATE"))),
    s(Key("ORIGINALDATE")),
];

const ALBUMTYPES: &[S] = &[
    s(TxxxList("MusicBrainz Album Type", true)),
    s(Atom(itunes!("MusicBrainz Album Type"))),
    s(Key("RELEASETYPE")),
    s(Key("MUSICBRAINZ_ALBUMTYPE")),
];
const CATALOGNUMS: &[S] = &[
    s(TxxxList("CATALOGNUMBER", true)),
    ro(TxxxList("CATALOGID", false)),
    ro(TxxxList("DISCOGS_CATALOG", false)),
    s(Atom(itunes!("CATALOGNUMBER"))),
    ro(Atom(itunes!("CATALOGID"))),
    ro(Atom(itunes!("DISCOGS_CATALOG"))),
    s(Key("CATALOGNUMBER")),
    ro(Key("CATALOGID")),
    ro(Key("DISCOGS_CATALOG")),
];
const LANGUAGES: &[S] = &[
    s(Frame("TLAN")),
    s(Atom(itunes!("LANGUAGE"))),
    s(Key("LANGUAGE")),
];
const ARRANGERS: &[S] = &[
    s(People("TIPL", "arranger")),
    s(Atom(itunes!("Arranger"))),
    s(Key("ARRANGER")),
];
const COMPOSERS: &[S] = &[s(Frame("TCOM")), s(Atom("\u{a9}wrt")), s(Key("COMPOSER"))];
const GENRE_STYLES: &[S] = &[s(Frame("TCON")), s(Atom("\u{a9}gen")), s(Key("GENRE"))];
const LYRICISTS: &[S] = &[
    s(Frame("TEXT")),
    s(Atom(itunes!("LYRICIST"))),
    s(Key("LYRICIST")),
];
const MB_ARTISTIDS: &[S] = &[
    s(TxxxList("MusicBrainz Artist Id", true)),
    s(Atom(itunes!("MusicBrainz Artist Id"))),
    s(Key("MUSICBRAINZ_ARTISTID")),
];
const MB_ALBUMARTISTIDS: &[S] = &[
    s(TxxxList("MusicBrainz Album Artist Id", true)),
    s(Atom(itunes!("MusicBrainz Album Artist Id"))),
    s(Key("MUSICBRAINZ_ALBUMARTISTID")),
];

/// The MediaFile fields beets reads and writes (`Item._media_tag_fields`),
/// in MediaFile's write order (`sorted_fields`: by name, the dates where
/// their year sorts).
const FIELDS: &[Field] = &[
    desc_field!(
        "acoustid_fingerprint",
        Str,
        "Acoustid Fingerprint",
        "ACOUSTID_FINGERPRINT"
    ),
    desc_field!("acoustid_id", Str, "Acoustid Id", "ACOUSTID_ID"),
    f(
        "album",
        Str,
        &[s(Frame("TALB")), s(Atom("\u{a9}alb")), s(Key("ALBUM"))],
    ),
    f(
        "albumartist",
        Str,
        &[
            s(Frame("TPE2")),
            s(Atom("aART")),
            s(Key("ALBUM ARTIST")),
            s(Key("ALBUM_ARTIST")),
            s(Key("ALBUMARTIST")),
        ],
    ),
    desc_field!(
        "albumartist_credit",
        Str,
        "Album Artist Credit",
        "ALBUMARTIST_CREDIT"
    ),
    f(
        "albumartist_sort",
        Str,
        &[
            s(Frame("TSO2")),
            s(Txxx("ALBUMARTISTSORT")),
            s(Atom("soaa")),
            s(Key("ALBUMARTISTSORT")),
        ],
    ),
    f(
        "albumartists",
        List,
        &[
            s(TxxxList("ALBUMARTISTS", false)),
            s(TxxxList("ALBUM_ARTISTS", false)),
            ro(TxxxList("ALBUM ARTISTS", false)),
            s(Atom(itunes!("ALBUMARTISTS"))),
            s(Atom(itunes!("ALBUM_ARTISTS"))),
            ro(Atom(itunes!("ALBUM ARTISTS"))),
            s(Key("ALBUMARTISTS")),
            s(Key("ALBUM_ARTISTS")),
            ro(Key("ALBUM ARTISTS")),
        ],
    ),
    list_desc_field!(
        "albumartists_credit",
        "ALBUMARTISTS_CREDIT",
        "ALBUMARTISTS_CREDIT",
        false
    ),
    list_desc_field!(
        "albumartists_sort",
        "ALBUMARTISTS_SORT",
        "ALBUMARTISTS_SORT",
        false
    ),
    desc_field!(
        "albumdisambig",
        Str,
        "MusicBrainz Album Comment",
        "MUSICBRAINZ_ALBUMCOMMENT"
    ),
    f(
        "albumstatus",
        Str,
        &[
            s(Txxx("MusicBrainz Album Status")),
            s(Atom(itunes!("MusicBrainz Album Status"))),
            s(Key("RELEASESTATUS")),
            s(Key("MUSICBRAINZ_ALBUMSTATUS")),
        ],
    ),
    f("albumtype", Str, ALBUMTYPES),
    f("albumtypes", List, ALBUMTYPES),
    f("arrangers", List, ARRANGERS),
    f(
        "artist",
        Str,
        &[s(Frame("TPE1")), s(Atom("\u{a9}ART")), s(Key("ARTIST"))],
    ),
    desc_field!("artist_credit", Str, "Artist Credit", "ARTIST_CREDIT"),
    f(
        "artist_sort",
        Str,
        &[s(Frame("TSOP")), s(Atom("soar")), s(Key("ARTISTSORT"))],
    ),
    list_desc_field!("artists", "ARTISTS", "ARTISTS", false),
    list_desc_field!("artists_credit", "ARTISTS_CREDIT", "ARTISTS_CREDIT", false),
    list_desc_field!("artists_sort", "ARTISTS_SORT", "ARTISTS_SORT", false),
    desc_field!("asin", Str, "ASIN", "ASIN"),
    f(
        "barcode",
        Str,
        &[
            s(Txxx("BARCODE")),
            s(Atom(itunes!("BARCODE"))),
            s(Key("BARCODE")),
            ro(Key("UPC")),
            ro(Key("EAN/UPN")),
            ro(Key("EAN")),
            ro(Key("UPN")),
        ],
    ),
    f(
        "bpm",
        Int,
        &[s(Frame("TBPM")), s(Mp4Int("tmpo")), s(Key("BPM"))],
    ),
    f("catalognum", Str, CATALOGNUMS),
    f(
        "comments",
        Str,
        &[
            s(Comm("")),
            s(Atom("\u{a9}cmt")),
            s(Key("DESCRIPTION")),
            s(Key("COMMENT")),
        ],
    ),
    f(
        "comp",
        Bool,
        &[s(Frame("TCMP")), s(Mp4Bool("cpil")), s(Key("COMPILATION"))],
    ),
    f(
        "composer_sort",
        Str,
        &[s(Frame("TSOC")), s(Atom("soco")), s(Key("COMPOSERSORT"))],
    ),
    f("composers", List, COMPOSERS),
    desc_field!(
        "country",
        Str,
        "MusicBrainz Album Release Country",
        "RELEASECOUNTRY"
    ),
    // `date0`..`date2`.
    f("date", Date(&[s(Key("YEAR"))]), DATE_STYLES),
    f(
        "disc",
        Int,
        &[
            s(SlashPack("TPOS", 0)),
            s(Tuple("disk", 0)),
            s(Key("DISC")),
            s(Key("DISCNUMBER")),
        ],
    ),
    f(
        "disctitle",
        Str,
        &[
            s(Frame("TSST")),
            s(Atom(itunes!("DISCSUBTITLE"))),
            s(Key("DISCSUBTITLE")),
        ],
    ),
    f(
        "disctotal",
        Int,
        &[
            s(SlashPack("TPOS", 1)),
            s(Tuple("disk", 1)),
            s(Key("DISCTOTAL")),
            s(Key("DISCC")),
            s(Key("TOTALDISCS")),
        ],
    ),
    f(
        "encoder",
        Str,
        &[
            s(Frame("TENC")),
            s(Atom("\u{a9}too")),
            s(Key("ENCODEDBY")),
            s(Key("ENCODER")),
        ],
    ),
    f("genres", List, GENRE_STYLES),
    f(
        "grouping",
        Str,
        &[s(Frame("TIT1")), s(Atom("\u{a9}grp")), s(Key("GROUPING"))],
    ),
    f(
        "initial_key",
        Str,
        &[
            s(Frame("TKEY")),
            s(Atom(itunes!("initialkey"))),
            s(Key("INITIALKEY")),
        ],
    ),
    f(
        "isrc",
        Str,
        &[s(Frame("TSRC")), s(Atom(itunes!("ISRC"))), s(Key("ISRC"))],
    ),
    f(
        "label",
        Str,
        &[
            s(Frame("TPUB")),
            s(Txxx("LABEL")),
            s(Atom(itunes!("LABEL"))),
            s(Atom(itunes!("publisher"))),
            ro(Atom(itunes!("Label"))),
            s(Key("LABEL")),
            s(Key("PUBLISHER")),
        ],
    ),
    f("language", Str, LANGUAGES),
    f("lyricists", List, LYRICISTS),
    f(
        "lyrics",
        Str,
        &[s(Uslt), s(Atom("\u{a9}lyr")), s(Key("LYRICS"))],
    ),
    f("mb_albumartistid", Str, MB_ALBUMARTISTIDS),
    f("mb_albumartistids", List, MB_ALBUMARTISTIDS),
    desc_field!(
        "mb_albumid",
        Str,
        "MusicBrainz Album Id",
        "MUSICBRAINZ_ALBUMID"
    ),
    f("mb_artistid", Str, MB_ARTISTIDS),
    f("mb_artistids", List, MB_ARTISTIDS),
    desc_field!(
        "mb_releasegroupid",
        Str,
        "MusicBrainz Release Group Id",
        "MUSICBRAINZ_RELEASEGROUPID"
    ),
    desc_field!(
        "mb_releasetrackid",
        Str,
        "MusicBrainz Release Track Id",
        "MUSICBRAINZ_RELEASETRACKID"
    ),
    f(
        "mb_trackid",
        Str,
        &[
            s(Ufid("http://musicbrainz.org")),
            s(Atom(itunes!("MusicBrainz Track Id"))),
            s(Key("MUSICBRAINZ_TRACKID")),
        ],
    ),
    desc_field!(
        "mb_workid",
        Str,
        "MusicBrainz Work Id",
        "MUSICBRAINZ_WORKID"
    ),
    f(
        "media",
        Str,
        &[
            s(Frame("TMED")),
            s(Txxx("MEDIA")),
            s(Atom(itunes!("MEDIA"))),
            s(Key("MEDIA")),
        ],
    ),
    // `original_date0`..`original_date2`.
    f("original_date", Date(&[]), ORIGINAL_DATE_STYLES),
    desc_field!(
        "r128_album_gain",
        QNum,
        "R128_ALBUM_GAIN",
        "R128_ALBUM_GAIN"
    ),
    desc_field!(
        "r128_track_gain",
        QNum,
        "R128_TRACK_GAIN",
        "R128_TRACK_GAIN"
    ),
    f(
        "remixers",
        List,
        &[
            s(Frame("TPE4")),
            s(Atom(itunes!("REMIXERS"))),
            s(Key("REMIXER")),
        ],
    ),
    f(
        "rg_album_gain",
        Float,
        &[
            gain(Txxx("REPLAYGAIN_ALBUM_GAIN")),
            gain(Txxx("replaygain_album_gain")),
            gain(Atom(itunes!("replaygain_album_gain"))),
            gain(Key("REPLAYGAIN_ALBUM_GAIN")),
        ],
    ),
    f(
        "rg_album_peak",
        Float,
        &[
            peak(Txxx("REPLAYGAIN_ALBUM_PEAK")),
            peak(Txxx("replaygain_album_peak")),
            peak(Atom(itunes!("replaygain_album_peak"))),
            peak(Key("REPLAYGAIN_ALBUM_PEAK")),
        ],
    ),
    f(
        "rg_track_gain",
        Float,
        &[
            gain(Txxx("REPLAYGAIN_TRACK_GAIN")),
            gain(Txxx("replaygain_track_gain")),
            s(Id3Sc(0)),
            gain(Atom(itunes!("replaygain_track_gain"))),
            s(Mp4Sc(0)),
            gain(Key("REPLAYGAIN_TRACK_GAIN")),
        ],
    ),
    f(
        "rg_track_peak",
        Float,
        &[
            peak(Txxx("REPLAYGAIN_TRACK_PEAK")),
            peak(Txxx("replaygain_track_peak")),
            s(Id3Sc(1)),
            peak(Atom(itunes!("replaygain_track_peak"))),
            s(Mp4Sc(1)),
            peak(Key("REPLAYGAIN_TRACK_PEAK")),
        ],
    ),
    f(
        "script",
        Str,
        &[
            s(Txxx("Script")),
            s(Atom(itunes!("SCRIPT"))),
            s(Key("SCRIPT")),
        ],
    ),
    f("subtitle", Str, &[s(Frame("TIT3")), s(Key("SUBTITLE"))]),
    f(
        "title",
        Str,
        &[s(Frame("TIT2")), s(Atom("\u{a9}nam")), s(Key("TITLE"))],
    ),
    f(
        "track",
        Int,
        &[
            s(SlashPack("TRCK", 0)),
            s(Tuple("trkn", 0)),
            s(Key("TRACK")),
            s(Key("TRACKNUMBER")),
        ],
    ),
    f(
        "tracktotal",
        Int,
        &[
            s(SlashPack("TRCK", 1)),
            s(Tuple("trkn", 1)),
            s(Key("TRACKTOTAL")),
            s(Key("TRACKC")),
            s(Key("TOTALTRACKS")),
        ],
    ),
];

/// MediaFile's other readable tag fields, which beets doesn't store on
/// items (read for `beet info`, never written): the single-value views of
/// list fields (`single_field()`), the lists behind `catalognum` and
/// `language`, `copyright` and `url`. (`date`, `original_date`, `art`,
/// `images` and `synced_lyrics` have their own accessors.)
const EXTRA_FIELDS: &[Field] = &[
    f("arranger", Str, ARRANGERS),
    f("catalognums", List, CATALOGNUMS),
    f("composer", Str, COMPOSERS),
    f(
        "copyright",
        Str,
        &[s(Frame("TCOP")), s(Atom("cprt")), s(Key("COPYRIGHT"))],
    ),
    f("genre", Str, GENRE_STYLES),
    f("languages", List, LANGUAGES),
    f("lyricist", Str, LYRICISTS),
    f("url", Str, &[s(Wxxx), s(Atom("\u{a9}url")), s(Key("URL"))]),
];

/// MediaFile's ASF styles, by field name: the attribute names in the
/// Windows Media (ASF) files, the read-only alternative names included.
/// The five names of the Content Description Object (`Title`, `Author`,
/// `Copyright`, `Description`, `Rating`) are stored in that object;
/// every other attribute goes to the Extended Content Description
/// Object, with a list's further values in the Metadata Library Object
/// (see `asf.rs`).
const ASF_STYLES: &[(&str, &[S])] = &[
    ("acoustid_fingerprint", &[s(Asf("Acoustid/Fingerprint"))]),
    ("acoustid_id", &[s(Asf("Acoustid/Id"))]),
    ("album", &[s(Asf("WM/AlbumTitle"))]),
    ("albumartist", &[s(Asf("WM/AlbumArtist"))]),
    ("albumartist_credit", &[s(Asf("beets/Album Artist Credit"))]),
    ("albumartist_sort", &[s(Asf("WM/AlbumArtistSortOrder"))]),
    ("albumartists", &[s(Asf("WM/AlbumArtists"))]),
    ("albumartists_credit", &[s(Asf("beets/AlbumArtistsCredit"))]),
    ("albumartists_sort", &[s(Asf("beets/AlbumArtistsSort"))]),
    ("albumdisambig", &[s(Asf("MusicBrainz/Album Comment"))]),
    ("albumstatus", &[s(Asf("MusicBrainz/Album Status"))]),
    ("albumtype", &[s(Asf("MusicBrainz/Album Type"))]),
    ("albumtypes", &[s(Asf("MusicBrainz/Album Type"))]),
    ("arranger", &[s(Asf("beets/Arranger"))]),
    ("arrangers", &[s(Asf("beets/Arranger"))]),
    ("artist", &[s(Asf("Author"))]),
    ("artist_credit", &[s(Asf("beets/Artist Credit"))]),
    ("artist_sort", &[s(Asf("WM/ArtistSortOrder"))]),
    ("artists", &[s(Asf("WM/ARTISTS"))]),
    ("artists_credit", &[s(Asf("beets/ArtistsCredit"))]),
    ("artists_sort", &[s(Asf("beets/ArtistsSort"))]),
    ("asin", &[s(Asf("MusicBrainz/ASIN"))]),
    ("barcode", &[s(Asf("WM/Barcode"))]),
    ("bpm", &[s(Asf("WM/BeatsPerMinute"))]),
    (
        "catalognum",
        &[
            s(Asf("WM/CatalogNo")),
            ro(Asf("CATALOGID")),
            ro(Asf("DISCOGS_CATALOG")),
        ],
    ),
    (
        "catalognums",
        &[
            s(Asf("WM/CatalogNo")),
            ro(Asf("CATALOGID")),
            ro(Asf("DISCOGS_CATALOG")),
        ],
    ),
    ("comments", &[s(Asf("WM/Comments")), s(Asf("Description"))]),
    ("comp", &[s(Asf("WM/IsCompilation"))]),
    ("composer", &[s(Asf("WM/Composer"))]),
    ("composer_sort", &[s(Asf("WM/Composersortorder"))]),
    ("composers", &[s(Asf("WM/Composer"))]),
    ("copyright", &[s(Asf("Copyright"))]),
    ("country", &[s(Asf("MusicBrainz/Album Release Country"))]),
    ("date", &[s(Asf("WM/Year"))]),
    ("disc", &[s(Asf("WM/PartOfSet"))]),
    ("disctitle", &[s(Asf("WM/SetSubTitle"))]),
    ("disctotal", &[s(Asf("TotalDiscs"))]),
    ("encoder", &[s(Asf("WM/EncodedBy"))]),
    ("genre", &[s(Asf("WM/Genre"))]),
    ("genres", &[s(Asf("WM/Genre"))]),
    ("grouping", &[s(Asf("WM/ContentGroupDescription"))]),
    ("initial_key", &[s(Asf("INITIALKEY"))]),
    ("isrc", &[s(Asf("WM/ISRC"))]),
    ("label", &[s(Asf("WM/Publisher"))]),
    ("language", &[s(Asf("WM/Language"))]),
    ("languages", &[s(Asf("WM/Language"))]),
    ("lyricist", &[s(Asf("WM/Writer"))]),
    ("lyricists", &[s(Asf("WM/Writer"))]),
    ("lyrics", &[s(Asf("WM/Lyrics"))]),
    ("mb_albumartistid", &[s(Asf("MusicBrainz/Album Artist Id"))]),
    (
        "mb_albumartistids",
        &[s(Asf("MusicBrainz/Album Artist Id"))],
    ),
    ("mb_albumid", &[s(Asf("MusicBrainz/Album Id"))]),
    ("mb_artistid", &[s(Asf("MusicBrainz/Artist Id"))]),
    ("mb_artistids", &[s(Asf("MusicBrainz/Artist Id"))]),
    (
        "mb_releasegroupid",
        &[s(Asf("MusicBrainz/Release Group Id"))],
    ),
    (
        "mb_releasetrackid",
        &[s(Asf("MusicBrainz/Release Track Id"))],
    ),
    ("mb_trackid", &[s(Asf("MusicBrainz/Track Id"))]),
    ("mb_workid", &[s(Asf("MusicBrainz/Work Id"))]),
    ("media", &[s(Asf("WM/Media"))]),
    ("original_date", &[s(Asf("WM/OriginalReleaseYear"))]),
    ("r128_album_gain", &[s(Asf("R128_ALBUM_GAIN"))]),
    ("r128_track_gain", &[s(Asf("R128_TRACK_GAIN"))]),
    ("remixers", &[s(Asf("WM/REMIXERS"))]),
    ("rg_album_gain", &[gain(Asf("replaygain_album_gain"))]),
    ("rg_album_peak", &[peak(Asf("replaygain_album_peak"))]),
    ("rg_track_gain", &[gain(Asf("replaygain_track_gain"))]),
    ("rg_track_peak", &[peak(Asf("replaygain_track_peak"))]),
    ("script", &[s(Asf("WM/Script"))]),
    ("subtitle", &[s(Asf("Subtitle"))]),
    ("title", &[s(Asf("Title"))]),
    ("track", &[s(Asf("WM/TrackNumber"))]),
    ("tracktotal", &[s(Asf("TotalTracks"))]),
    ("url", &[s(Asf("WM/URL"))]),
];

/// A field's ASF styles (none for the fields Windows Media can't hold).
fn asf_styles(name: &str) -> &'static [S] {
    ASF_STYLES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(&[], |(_, s)| s)
}

/// The names of [`EXTRA_FIELDS`].
pub fn extra_field_names() -> impl Iterator<Item = &'static str> {
    EXTRA_FIELDS.iter().map(|f| f.name)
}

/// Item fields stored in tags (beets' `Item._media_tag_fields`), sorted.
pub const TAG_FIELDS: &[&str] = &[
    "acoustid_fingerprint",
    "acoustid_id",
    "album",
    "albumartist",
    "albumartist_credit",
    "albumartist_sort",
    "albumartists",
    "albumartists_credit",
    "albumartists_sort",
    "albumdisambig",
    "albumstatus",
    "albumtype",
    "albumtypes",
    "arrangers",
    "artist",
    "artist_credit",
    "artist_sort",
    "artists",
    "artists_credit",
    "artists_sort",
    "asin",
    "barcode",
    "bpm",
    "catalognum",
    "comments",
    "comp",
    "composer_sort",
    "composers",
    "country",
    "day",
    "disc",
    "disctitle",
    "disctotal",
    "encoder",
    "genres",
    "grouping",
    "initial_key",
    "isrc",
    "label",
    "language",
    "lyricists",
    "lyrics",
    "mb_albumartistid",
    "mb_albumartistids",
    "mb_albumid",
    "mb_artistid",
    "mb_artistids",
    "mb_releasegroupid",
    "mb_releasetrackid",
    "mb_trackid",
    "mb_workid",
    "media",
    "month",
    "original_day",
    "original_month",
    "original_year",
    "r128_album_gain",
    "r128_track_gain",
    "remixers",
    "rg_album_gain",
    "rg_album_peak",
    "rg_track_gain",
    "rg_track_peak",
    "script",
    "subtitle",
    "title",
    "track",
    "tracktotal",
    "year",
];

// ------------------------------------------------------------------ values

/// A raw value as a storage style returns it (before `safe_cast`).
#[derive(Clone, Debug, PartialEq)]
enum Raw {
    S(String),
    I(i64),
    B(bool),
    F(f64),
}

impl Raw {
    /// Python truthiness.
    fn truthy(&self) -> bool {
        match self {
            Raw::S(s) => !s.is_empty(),
            Raw::I(i) => *i != 0,
            Raw::B(b) => *b,
            Raw::F(f) => *f != 0.0,
        }
    }

    fn as_str(&self) -> String {
        match self {
            Raw::S(s) => s.clone(),
            Raw::I(i) => i.to_string(),
            Raw::B(b) => if *b { "True" } else { "False" }.into(),
            Raw::F(f) => f.to_string(),
        }
    }

    /// `safe_cast(int, ...)`: a number from the front of a string.
    fn as_int(&self) -> i64 {
        match self {
            Raw::I(i) => *i,
            Raw::B(b) => i64::from(*b),
            Raw::F(f) => *f as i64,
            Raw::S(s) => {
                let t = s.trim();
                let b = t.as_bytes();
                let mut end = usize::from(matches!(b.first(), Some(b'+' | b'-')));
                let digits = end;
                while end < b.len() && b[end].is_ascii_digit() {
                    end += 1;
                }
                if end == digits {
                    0
                } else {
                    // Out of range values (beets' bit_length check) are 0.
                    t[..end].parse().unwrap_or(0)
                }
            }
        }
    }

    /// `safe_cast(bool, ...)`: `bool(int(val))`, False when not a number.
    fn as_bool(&self) -> bool {
        match self {
            Raw::B(b) => *b,
            Raw::I(i) => *i != 0,
            Raw::F(f) => *f as i64 != 0,
            Raw::S(s) => s
                .trim()
                .replace('_', "")
                .parse::<i64>()
                .is_ok_and(|i| i != 0),
        }
    }

    /// `safe_cast(float, ...)`: a decimal number from the front of a string.
    // Matches beets (Python): tag values are truncated/rounded as Python does.
    #[allow(clippy::cast_precision_loss)]
    fn as_float(&self) -> f64 {
        match self {
            Raw::I(i) => *i as f64,
            Raw::F(f) => *f,
            Raw::B(b) => i64::from(*b) as f64,
            Raw::S(s) => {
                static NUM: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                    regex::Regex::new(r"^[\+-]?([0-9]+\.?[0-9]*|[0-9]*\.[0-9]+)").unwrap()
                });
                NUM.find(s.trim())
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0.0)
            }
        }
    }
}

/// A value to write, as beets passes it to MediaFile.
#[derive(Clone, Debug, PartialEq)]
enum In {
    S(String),
    I(i64),
    B(bool),
    F(f64),
}

impl In {
    /// `StorageStyle.serialize` for a string-typed style.
    fn serialize(&self, st: &S) -> String {
        format!("{}{}", self.bare(st), st.suffix)
    }

    /// The serialized value without the style's suffix.
    fn bare(&self, st: &S) -> String {
        match self {
            In::S(s) => s.clone(),
            In::I(i) => i.to_string(),
            In::B(b) => i64::from(*b).to_string(),
            In::F(f) => format!("{:.*}", st.places, f),
        }
    }

    fn int(&self) -> i64 {
        match self {
            In::I(i) => *i,
            In::B(b) => i64::from(*b),
            In::F(f) => *f as i64,
            In::S(s) => Raw::S(s.clone()).as_int(),
        }
    }

    // Matches beets (Python): tag values are truncated/rounded as Python does.
    #[allow(clippy::cast_precision_loss)]
    fn float(&self) -> f64 {
        match self {
            In::I(i) => *i as f64,
            In::B(b) => i64::from(*b) as f64,
            In::F(f) => *f,
            In::S(s) => Raw::S(s.clone()).as_float(),
        }
    }
}

/// `StorageStyle.deserialize`: strip the suffix.
fn deserialize(st: &S, raw: Raw) -> Raw {
    match raw {
        Raw::S(s) if !st.suffix.is_empty() && s.ends_with(st.suffix) => {
            Raw::S(s[..s.len() - st.suffix.len()].to_string())
        }
        r => r,
    }
}

// ------------------------------------------------------------- sound check

/// `sc_decode`: an iTunes Sound Check string as (gain, peak).
fn sc_decode(data: &str) -> (f64, f64) {
    let hex: String = data.chars().filter(|c| *c != ' ').collect();
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    let Some(bytes) = bytes.filter(|b| b.len() == 40 && hex.len() == 80) else {
        return (0.0, 0.0);
    };
    let n: Vec<i32> = bytes
        .chunks(4)
        .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let maxgain = n[0].max(n[1]);
    let gain = if maxgain > 0 {
        (f64::from(maxgain) / 1000.0).log10() * -10.0
    } else {
        0.0
    };
    let peak = f64::from(n[6].max(n[7])) / 32768.0;
    (round_to(gain, 2), round_to(peak, 6))
}

/// Python's `round(x, n)` (to the nearest, ties to even, of the decimal).
fn round_to(x: f64, places: usize) -> f64 {
    format!("{x:.places$}").parse().unwrap_or(x)
}

/// `sc_encode`: ReplayGain gain and peak as a Sound Check string.
// Matches beets (Python): tag values are truncated/rounded as Python does.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn sc_encode(gain: f64, peak: f64) -> String {
    let peak = peak * 32768.0;
    let g = |reference: f64| -> i64 {
        let v = ((10f64.powf(gain / -10.0)) * reference).round_ties_even();
        let v = v.min(65534.0) as i64;
        if v == 0 { 1 } else { v }
    };
    let (g1, g2) = (g(1000.0), g(2500.0));
    let p = peak as i64;
    [g1, g1, g2, g2, 0, 0, p, p, 0, 0]
        .iter()
        .map(|v| format!(" {:08X}", *v as u32))
        .collect()
}

// ------------------------------------------------------------------ genres

// The genre list behind ID3v1's one-byte genre and ID3v2's TCON frame.
// genre_name() indexes it, genre_index() is its inverse for writing an
// ID3v1 tag.

/// The ID3v1 genre list, numbers 0-79 (ID3v1 specification, "Genre").
const ID3V1_GENRES: [&str; 80] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "Alt. Rock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta Rap",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychedelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
];

/// The Winamp genre extensions, numbers 80-191 (Winamp's published genre list).
const WINAMP_GENRES: [&str; 112] = [
    "Folk",
    "Folk-Rock",
    "National Folk",
    "Swing",
    "Fast-Fusion",
    "Bebop",
    "Latin",
    "Revival",
    "Celtic",
    "Bluegrass",
    "Avantgarde",
    "Gothic Rock",
    "Progressive Rock",
    "Psychedelic Rock",
    "Symphonic Rock",
    "Slow Rock",
    "Big Band",
    "Chorus",
    "Easy Listening",
    "Acoustic",
    "Humour",
    "Speech",
    "Chanson",
    "Opera",
    "Chamber Music",
    "Sonata",
    "Symphony",
    "Booty Bass",
    "Primus",
    "Porn Groove",
    "Satire",
    "Slow Jam",
    "Club",
    "Tango",
    "Samba",
    "Folklore",
    "Ballad",
    "Power Ballad",
    "Rhythmic Soul",
    "Freestyle",
    "Duet",
    "Punk Rock",
    "Drum Solo",
    "A Cappella",
    "Euro-House",
    "Dance Hall",
    "Goa",
    "Drum & Bass",
    "Club-House",
    "Hardcore",
    "Terror",
    "Indie",
    "BritPop",
    "Afro-Punk",
    "Polsk Punk",
    "Beat",
    "Christian Gangsta Rap",
    "Heavy Metal",
    "Black Metal",
    "Crossover",
    "Contemporary Christian",
    "Christian Rock",
    "Merengue",
    "Salsa",
    "Thrash Metal",
    "Anime",
    "JPop",
    "Synthpop",
    "Abstract",
    "Art Rock",
    "Baroque",
    "Bhangra",
    "Big Beat",
    "Breakbeat",
    "Chillout",
    "Downtempo",
    "Dub",
    "EBM",
    "Eclectic",
    "Electro",
    "Electroclash",
    "Emo",
    "Experimental",
    "Garage",
    "Global",
    "IDM",
    "Illbient",
    "Industro-Goth",
    "Jam Band",
    "Krautrock",
    "Leftfield",
    "Lounge",
    "Math Rock",
    "New Romantic",
    "Nu-Breakz",
    "Post-Punk",
    "Post-Rock",
    "Psytrance",
    "Shoegaze",
    "Space Rock",
    "Trop Rock",
    "World Music",
    "Neoclassical",
    "Audiobook",
    "Audio Theatre",
    "Neue Deutsche Welle",
    "Podcast",
    "Indie Rock",
    "G-Funk",
    "Dubstep",
    "Garage Rock",
    "Psybient",
];

/// The name of genre number `index` (None beyond the Winamp list).
fn genre_name(index: usize) -> Option<&'static str> {
    match index.checked_sub(ID3V1_GENRES.len()) {
        None => ID3V1_GENRES.get(index).copied(),
        Some(i) => WINAMP_GENRES.get(i).copied(),
    }
}

/// The genre number of `name`, for ID3v1's one-byte genre field (None
/// if it is not in the list).
fn genre_index(name: &str) -> Option<u8> {
    let index = ID3V1_GENRES
        .iter()
        .chain(WINAMP_GENRES.iter())
        .position(|g| *g == name)?;
    u8::try_from(index).ok()
}

/// The names a TCON frame's values stand for.
///
/// ID3v2.3's TCON (`Content type`) was a text frame whose value could
/// be a genre number in parentheses, optionally followed by a
/// refinement: "(17)", "(4)Eurodisco". "(RX)" and "(CR)" stand for
/// "Remix" and "Cover", and a value starting with a literal "(" is
/// escaped by doubling that parenthesis. ID3v2.4 allows several of
/// these in one frame, NUL separated, and a bare genre number is also
/// accepted; that form and NUL splitting are what the tag reader feeds
/// us, so both versions are accepted here and turned into the genre
/// names beets stores (unknown genre numbers become "Unknown").
fn tcon_genres(values: &[String]) -> Vec<String> {
    let mut genres = Vec::new();
    for value in values {
        parse_tcon_value(value, &mut genres);
    }
    genres
}

/// Add the genres of one NUL-separated TCON value to `genres`.
fn parse_tcon_value(value: &str, genres: &mut Vec<String>) {
    if value.is_empty() {
        return;
    }

    // A bare number below 256 names a genre (ID3v2.4's deprecated
    // numeric form; ID3v2.3 already allowed "(17)" without refinement).
    if value.bytes().all(|b| b.is_ascii_digit())
        && let Ok(number) = value.parse::<usize>()
    {
        if number < 256 {
            genres.push(genre_name(number).unwrap_or(UNKNOWN_GENRE).to_string());
            return;
        }
        // Past the one-byte range it is no genre reference, just text.
        genres.push(value.to_string());
        return;
    }

    // The two special references, when they appear on their own.
    match value {
        "RX" => {
            genres.push(REMIX_GENRE.to_string());
            return;
        }
        "CR" => {
            genres.push(COVER_GENRE.to_string());
            return;
        }
        _ => {}
    }

    let (references, rest) = leading_genre_refs(value);
    if references.is_empty() {
        // No "(...)" prefix: the whole value is the genre, with an
        // escaped leading parenthesis.
        genres.push(unescape_paren(value).to_string());
        return;
    }
    // A refinement after the references is the genre text (unless it
    // merely repeats a reference, as in "(17)Rock").
    let refinement = rest
        .map(unescape_paren)
        .filter(|text| !references.iter().any(|g| g == text));
    genres.extend(references);
    if let Some(text) = refinement {
        genres.push(text.to_string());
    }
}

const REMIX_GENRE: &str = "Remix";
const COVER_GENRE: &str = "Cover";
const UNKNOWN_GENRE: &str = "Unknown";

/// Split a TCON value into its leading "(...)" genre references and the
/// refinement text that follows them. A parenthesised token that is not
/// a number, "RX" or "CR" ends the reference list (it is text, as in
/// "(foo) bar").
fn leading_genre_refs(value: &str) -> (Vec<String>, Option<&str>) {
    let mut references = Vec::new();
    let mut rest = value;
    while let Some(after_paren) = rest.strip_prefix('(') {
        let Some(close) = after_paren.find(')') else {
            break;
        };
        let Some(genre) = genre_reference(&after_paren[..close]) else {
            break;
        };
        references.push(genre.to_string());
        rest = &after_paren[close + 1..];
    }
    (references, (!rest.is_empty()).then_some(rest))
}

/// The genre a parenthesised TCON reference stands for: a genre number,
/// "RX" (remix) or "CR" (cover). Out-of-range numbers are "Unknown".
fn genre_reference(token: &str) -> Option<&'static str> {
    match token {
        "RX" => Some(REMIX_GENRE),
        "CR" => Some(COVER_GENRE),
        _ if !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()) => Some(
            token
                .parse::<usize>()
                .ok()
                .and_then(genre_name)
                .unwrap_or(UNKNOWN_GENRE),
        ),
        _ => None,
    }
}

/// Drop one parenthesis from a value whose first character is an
/// escaped "(" (ID3v2.3: "((foo)" is the genre "(foo)").
fn unescape_paren(value: &str) -> &str {
    value.strip_prefix("((").map_or(value, |_| &value[1..])
}

// -------------------------------------------------------------------- tags

/// The file's tag of the kind MediaFile uses for its format.
enum Tags {
    Id3 {
        tag: Id3v2Tag,
        /// The tag on disk is ID3v2.3 (MediaFile's `split_v23`).
        v23: bool,
        /// An existing ID3v1 tag (its fields stand in for missing ID3v2
        /// frames and are updated on save).
        v1: Option<Id3v1Tag>,
    },
    Mp4(Ilst),
    Vorbis(VorbisComments),
    Ape(ApeTag),
    Asf(crate::asf::Asf),
}

fn fid(id: &str) -> FrameId<'static> {
    FrameId::Valid(Cow::Owned(id.to_string()))
}

/// The values of a multi-valued ID3 text: split on NUL (and stripped of
/// stray byte order marks, as UTF-16 values each carry one).
fn id3_split(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s.split('\0').map(|p| p.replace('\u{feff}', "")).collect();
    if v.len() > 1 && v.last().is_some_and(String::is_empty) {
        v.pop();
    }
    v
}

fn mp4_ident(key: &str) -> AtomIdent<'static> {
    match key.strip_prefix(ITUNES) {
        Some(name) => AtomIdent::Freeform {
            mean: Cow::Borrowed("com.apple.iTunes"),
            name: Cow::Owned(name.to_string()),
        },
        None => {
            let mut b = [0u8; 4];
            // "©" is one byte (0xA9) in an atom name.
            for (i, c) in key.chars().take(4).enumerate() {
                b[i] = c as u32 as u8;
            }
            AtomIdent::Fourcc(b)
        }
    }
}

/// An ASF attribute as MediaFile's `ASFStorageStyle` deserializes it
/// (`attribute.value`, with bytes decoded for text fields).
fn asf_raw(v: &crate::asf::Value) -> Raw {
    match v {
        crate::asf::Value::Bool(b) => Raw::B(*b),
        crate::asf::Value::DWord(v) => Raw::I(i64::from(*v)),
        crate::asf::Value::QWord(v) => Raw::I(i64::try_from(*v).unwrap_or(i64::MAX)),
        crate::asf::Value::Word(v) => Raw::I(i64::from(*v)),
        other => Raw::S(other.as_text()),
    }
}

/// MediaFile's `_unpack_asf_image`: a `WM/Picture` attribute as
/// (mime, data, type index, description).
fn unpack_asf_picture(v: &crate::asf::Value) -> Option<Picture> {
    let crate::asf::Value::Bytes(d) = v else {
        return None;
    };
    let type_index = *d.first()?;
    let size = u32::from_le_bytes(d.get(1..5)?.try_into().ok()?) as usize;
    let mut at = 5usize;
    let read_string = |at: &mut usize| -> Option<String> {
        let mut out: Vec<u8> = Vec::new();
        loop {
            let unit = d.get(*at..*at + 2)?;
            *at += 2;
            if unit == [0, 0] {
                return Some(String::from_utf16_lossy(
                    &out.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c))
                        .collect::<Vec<_>>(),
                ));
            }
            out.extend_from_slice(unit);
        }
    };
    let mime = read_string(&mut at)?;
    let desc = read_string(&mut at)?;
    let end = at.checked_add(size)?.min(d.len());
    let data = d.get(at..end)?.to_vec();
    let pic_type = PictureType::from_u8(type_index);
    let mime = if mime.is_empty() {
        sniff_mime(&data)
    } else {
        MimeType::from_str(&mime)
    };
    Some(
        Picture::unchecked(data)
            .pic_type(pic_type)
            .mime_type(mime)
            .description(desc)
            .build(),
    )
}

/// The image MIME type from its first bytes (MediaFile's `Image.mime_type`
/// guesses from the data, ignoring the tag's own type).
fn sniff_mime(data: &[u8]) -> MimeType {
    match data {
        [0xff, 0xd8, ..] => MimeType::Jpeg,
        [0x89, b'P', b'N', b'G', ..] => MimeType::Png,
        [b'G', b'I', b'F', ..] => MimeType::Gif,
        [b'B', b'M', ..] => MimeType::Bmp,
        [b'I', b'I', 0x2a, 0x00, ..] | [b'M', b'M', 0x00, 0x2a, ..] => MimeType::Tiff,
        _ => MimeType::Unknown(String::new()),
    }
}

/// MediaFile's `_pack_asf_image`: type index, image size, MIME type and
/// description as UTF-16LE strings, then the image bytes.
fn pack_asf_picture(pic: &Picture) -> crate::asf::Value {
    let mime = pic.mime_type().map_or("", MimeType::as_str);
    let mut data = Vec::new();
    data.extend_from_slice(&pic.pic_type().as_u8().to_le_bytes());
    data.extend_from_slice(&u32::try_from(pic.data().len()).unwrap_or(0).to_le_bytes());
    for s in [mime, pic.description().unwrap_or("")] {
        data.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        data.extend_from_slice(&[0, 0]);
    }
    data.extend_from_slice(pic.data());
    crate::asf::Value::Bytes(data)
}

fn mp4_raw(d: &AtomData) -> Option<Raw> {
    Some(match d {
        AtomData::UTF8(s) | AtomData::UTF16(s) => Raw::S(s.clone()),
        AtomData::SignedInteger(i) => Raw::I(i64::from(*i)),
        AtomData::UnsignedInteger(u) => Raw::I(i64::from(*u)),
        AtomData::Bool(b) => Raw::B(*b),
        AtomData::Unknown { data, .. } => Raw::S(String::from_utf8_lossy(data).into_owned()),
        AtomData::Picture(_) => return None,
    })
}

/// An MP4 integer atom's value (any width, big-endian, signed).
fn mp4_int(d: &AtomData) -> Option<i64> {
    match d {
        AtomData::SignedInteger(i) => Some(i64::from(*i)),
        AtomData::UnsignedInteger(u) => Some(i64::from(*u)),
        AtomData::Bool(b) => Some(i64::from(*b)),
        AtomData::Unknown { data, .. } if !data.is_empty() && data.len() <= 8 => {
            let mut v: i64 = if data[0] & 0x80 != 0 { -1 } else { 0 };
            for b in data {
                v = (v << 8) | i64::from(*b);
            }
            Some(v)
        }
        _ => None,
    }
}

impl Tags {
    fn family(&self) -> Family {
        match self {
            Tags::Id3 { .. } => Family::Id3,
            Tags::Mp4(_) => Family::Mp4,
            Tags::Vorbis(_) | Tags::Ape(_) => Family::Plain,
            Tags::Asf(_) => Family::Asf,
        }
    }

    // ---- ID3 helpers

    fn id3(&self) -> &Id3v2Tag {
        match self {
            Tags::Id3 { tag, .. } => tag,
            _ => unreachable!(),
        }
    }

    fn id3_mut(&mut self) -> &mut Id3v2Tag {
        match self {
            Tags::Id3 { tag, .. } => tag,
            _ => unreachable!(),
        }
    }

    /// A text frame's values (`frame.text`), the ID3v1 tag standing in
    /// for missing frames as beets' MediaFile does.
    fn id3_text(&self, id: &str) -> Option<Vec<String>> {
        let Tags::Id3 { tag, v1, .. } = self else {
            return None;
        };
        for fr in tag {
            if fr.id_str() != id {
                continue;
            }
            match fr {
                Frame::Text(t) if id == "TCON" => return Some(tcon_genres(&id3_split(&t.value))),
                Frame::Text(t) => return Some(id3_split(&t.value)),
                Frame::Timestamp(t) => return Some(vec![t.timestamp.to_string()]),
                _ => {}
            }
        }
        let v1 = v1.as_ref()?;
        let text = |s: Option<Cow<'_, str>>| {
            s.map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|s| vec![s])
        };
        match id {
            "TIT2" => text(v1.title()),
            "TPE1" => text(v1.artist()),
            "TALB" => text(v1.album()),
            "TDRC" => v1.year.map(|y| vec![y.to_string()]),
            "TRCK" => v1
                .track_number
                .filter(|t| *t != 0)
                .map(|t| vec![t.to_string()]),
            "TCON" => v1
                .genre
                .filter(|g| *g != 255)
                .map(|g| tcon_genres(&[g.to_string()])),
            _ => None,
        }
    }

    fn id3_set_text(&mut self, id: &str, values: &[String]) {
        let tag = self.id3_mut();
        let _ = tag.remove(&fid(id)).count();
        if values.is_empty() || values.iter().all(String::is_empty) && values.len() == 1 {
            // A frame with no text is not stored (beets' MediaFile does
            // not save empty text frames).
            return;
        }
        let value = values.join("\0");
        if matches!(id, "TDRC" | "TDOR" | "TDRL" | "TDEN" | "TDTG") {
            if let Ok(ts) = value.parse() {
                tag.insert(Frame::Timestamp(TimestampFrame::new(
                    fid(id),
                    TextEncoding::UTF8,
                    ts,
                )));
            }
            return;
        }
        tag.insert(Frame::Text(TextInformationFrame::new(
            fid(id),
            TextEncoding::UTF8,
            value,
        )));
    }

    /// Frames with a description (`TXXX`, `COMM`, `USLT`) matching `desc`
    /// (ignoring case), as (description, text).
    fn id3_desc_frames(&self, key: &str, desc: &str) -> Vec<String> {
        let lower = desc.to_lowercase();
        self.id3()
            .iter()
            .filter_map(|fr| match fr {
                Frame::UserText(t) if key == "TXXX" && t.description.to_lowercase() == lower => {
                    Some(t.content.to_string())
                }
                Frame::Comment(t) if key == "COMM" && t.description.to_lowercase() == lower => {
                    Some(t.content.to_string())
                }
                Frame::UnsynchronizedText(t)
                    if key == "USLT" && t.description.to_lowercase() == lower =>
                {
                    Some(t.content.to_string())
                }
                Frame::UserUrl(t) if key == "WXXX" && t.description.to_lowercase() == lower => {
                    Some(t.content.to_string())
                }
                _ => None,
            })
            .collect()
    }

    fn is_desc_frame(fr: &Frame<'_>, key: &str, lower: &str) -> bool {
        match fr {
            Frame::UserText(t) => key == "TXXX" && t.description.to_lowercase() == lower,
            Frame::Comment(t) => key == "COMM" && t.description.to_lowercase() == lower,
            Frame::UnsynchronizedText(t) => key == "USLT" && t.description.to_lowercase() == lower,
            _ => false,
        }
    }

    /// `MP3DescStorageStyle.delete`: the first matching frame.
    fn id3_delete_desc(&mut self, key: &str, desc: &str) {
        let lower = desc.to_lowercase();
        let mut done = false;
        self.id3_mut().retain(|fr| {
            if !done && Self::is_desc_frame(fr, key, &lower) {
                done = true;
                return false;
            }
            true
        });
    }

    fn id3_new_desc_frame(key: &str, desc: &str, lang: [u8; 3], text: String) -> Frame<'static> {
        match key {
            "COMM" => Frame::Comment(CommentFrame::new(
                TextEncoding::UTF8,
                lang,
                desc.to_string(),
                text,
            )),
            "USLT" => Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(
                TextEncoding::UTF8,
                lang,
                desc.to_string(),
                text,
            )),
            _ => Frame::UserText(ExtendedTextFrame::new(
                TextEncoding::UTF8,
                desc.to_string(),
                text,
            )),
        }
    }

    /// `MP3DescStorageStyle.store`: change every matching frame in place,
    /// else add one.
    fn id3_store_desc(&mut self, key: &str, desc: &str, lang: [u8; 3], text: String) {
        let lower = desc.to_lowercase();
        let mut found = false;
        self.id3_mut().retain_mut(|fr| {
            if Self::is_desc_frame(fr, key, &lower) {
                found = true;
                match fr {
                    Frame::UserText(t) => {
                        t.content = Cow::Owned(text.clone());
                        t.encoding = TextEncoding::UTF8;
                    }
                    Frame::Comment(t) => {
                        t.content = Cow::Owned(text.clone());
                        t.encoding = TextEncoding::UTF8;
                    }
                    Frame::UnsynchronizedText(t) => {
                        t.content = Cow::Owned(text.clone());
                        t.encoding = TextEncoding::UTF8;
                    }
                    _ => {}
                }
            }
            true
        });
        if !found {
            let fr = Self::id3_new_desc_frame(key, desc, lang, text);
            self.id3_mut().insert(fr);
        }
    }

    /// `MP3ListDescStorageStyle.store`: replace the frame.
    fn id3_store_desc_list(&mut self, desc: &str, values: &[String]) {
        self.id3_delete_desc("TXXX", desc);
        if values.is_empty() {
            return;
        }
        let fr = Self::id3_new_desc_frame("TXXX", desc, *b"XXX", values.join("\0"));
        // `insert` would replace a frame with this exact description.
        let lower = desc.to_lowercase();
        self.id3_mut()
            .retain(|f| !Self::is_desc_frame(f, "TXXX", &lower));
        self.id3_mut().insert(fr);
    }

    fn id3_people(&self, frame: &str) -> Vec<(String, String)> {
        self.id3()
            .iter()
            .filter(|f| f.id_str() == frame)
            .filter_map(|f| match f {
                Frame::KeyValue(kv) => Some(kv),
                _ => None,
            })
            .flat_map(|kv| {
                kv.key_value_pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }

    /// `MP3PeopleStorageStyle._store_people`.
    fn id3_store_people(&mut self, frame: &str, involvement: &str, values: &[String]) {
        let inv = involvement.to_lowercase();
        let mut people: Vec<(String, String)> = self
            .id3_people(frame)
            .into_iter()
            .filter(|(k, _)| k.to_lowercase() != inv)
            .collect();
        people.extend(values.iter().map(|v| (involvement.to_string(), v.clone())));
        let tag = self.id3_mut();
        let _ = tag.remove(&fid(frame)).count();
        if !people.is_empty() {
            let pairs: Vec<(Cow<'static, str>, Cow<'static, str>)> = people
                .into_iter()
                .map(|(k, v)| (Cow::Owned(k), Cow::Owned(v)))
                .collect();
            tag.insert(Frame::KeyValue(KeyValueFrame::new(
                fid(frame),
                TextEncoding::UTF8,
                pairs,
            )));
        }
    }

    fn id3_ufid(&self, owner: &str) -> Option<String> {
        self.id3().iter().find_map(|f| match f {
            Frame::UniqueFileIdentifier(u) if u.owner == owner => {
                Some(String::from_utf8_lossy(&u.identifier).into_owned())
            }
            _ => None,
        })
    }

    fn id3_set_ufid(&mut self, owner: &str, value: Option<&str>) {
        self.id3_mut()
            .retain(|f| !matches!(f, Frame::UniqueFileIdentifier(u) if u.owner == owner));
        if let Some(v) = value {
            self.id3_mut()
                .insert(Frame::UniqueFileIdentifier(UniqueFileIdentifierFrame::new(
                    owner.to_string(),
                    v.as_bytes().to_vec(),
                )));
        }
    }

    // ---- MP4 helpers

    fn mp4(&self) -> &Ilst {
        match self {
            Tags::Mp4(t) => t,
            _ => unreachable!(),
        }
    }

    fn mp4_mut(&mut self) -> &mut Ilst {
        match self {
            Tags::Mp4(t) => t,
            _ => unreachable!(),
        }
    }

    fn mp4_values(&self, key: &str) -> Option<Vec<Raw>> {
        let atom = self.mp4().get(&mp4_ident(key))?;
        Some(atom.data().filter_map(mp4_raw).collect())
    }

    fn mp4_set_strings(&mut self, key: &str, values: &[String]) {
        let ident = mp4_ident(key);
        let ilst = self.mp4_mut();
        let _ = ilst.remove(&ident).count();
        if values.is_empty() {
            return;
        }
        let data = values.iter().map(|v| AtomData::UTF8(v.clone())).collect();
        if let Some(atom) = Atom::from_collection(ident, data) {
            ilst.insert(atom);
        }
    }

    fn mp4_set_data(&mut self, key: &str, data: AtomData) {
        let ident = mp4_ident(key);
        let ilst = self.mp4_mut();
        let _ = ilst.remove(&ident).count();
        ilst.insert(Atom::new(ident, data));
    }

    fn mp4_delete(&mut self, key: &str) {
        let _ = self.mp4_mut().remove(&mp4_ident(key)).count();
    }

    /// A `trkn`/`disk` pair (0 for missing halves).
    ///
    /// Apple's iTunes metadata stores both in one "data" atom as
    /// big-endian 16-bit integers after two reserved bytes: `trkn` is
    /// the 8-byte form "reserved, number, total, reserved", `disk` the
    /// 6-byte form "reserved, number, total". The atom's data type is
    /// binary, not the integer type used by `tmpo`.
    fn mp4_tuple(&self, key: &str) -> Option<(u16, u16)> {
        /// Offset of the number in the data atom's payload.
        const NUMBER: usize = 2;
        /// Offset of a pair's total, if the atom has one.
        const TOTAL: usize = 4;
        let atom = self.mp4().get(&mp4_ident(key))?;
        atom.data().find_map(|d| match d {
            AtomData::Unknown { data, .. } if data.len() >= TOTAL => {
                let number = u16::from_be_bytes([data[NUMBER], data[NUMBER + 1]]);
                let total = if data.len() >= TOTAL + 2 {
                    u16::from_be_bytes([data[TOTAL], data[TOTAL + 1]])
                } else {
                    0
                };
                Some((number, total))
            }
            _ => None,
        })
    }

    /// Write a `trkn`/`disk` pair in the iTunes metadata layout (see
    /// `mp4_tuple`), as a binary atom.
    fn mp4_set_tuple(&mut self, key: &str, (number, total): (u16, u16)) {
        let mut data = vec![0, 0];
        data.extend(number.to_be_bytes());
        data.extend(total.to_be_bytes());
        if key == "trkn" {
            // The 8-byte track form ends with two more unused bytes.
            data.extend([0, 0]);
        }
        self.mp4_set_data(
            key,
            AtomData::Unknown {
                code: DataType::Reserved,
                data,
            },
        );
    }

    // ---- Vorbis / APE helpers

    fn plain_values(&self, key: &str) -> Option<Vec<String>> {
        match self {
            Tags::Vorbis(vc) => {
                let v: Vec<String> = vc.get_all(key).map(str::to_string).collect();
                (!v.is_empty()).then_some(v)
            }
            Tags::Ape(ape) => {
                let item = ape.get(key)?;
                match item.value() {
                    ItemValue::Text(t) | ItemValue::Locator(t) => {
                        Some(t.split('\0').map(str::to_string).collect())
                    }
                    ItemValue::Binary(b) => Some(vec![String::from_utf8_lossy(b).into_owned()]),
                }
            }
            _ => None,
        }
    }

    fn plain_set(&mut self, key: &str, values: &[String]) {
        match self {
            Tags::Vorbis(vc) => {
                let _ = vc.remove(key).count();
                for v in values {
                    vc.push(key.to_string(), v.clone());
                }
            }
            Tags::Ape(ape) => {
                ape.remove(key);
                if !values.is_empty()
                    && let Ok(item) =
                        ApeItem::new(key.to_string(), ItemValue::Text(values.join("\0")))
                {
                    ape.insert(item);
                }
            }
            _ => {}
        }
    }

    // ---- storage style operations

    /// `fetch` + `deserialize` for list access (`get_list`): None when the
    /// tag is missing.
    fn get_list(&self, st: &S) -> Option<Vec<Raw>> {
        let strs = |v: Vec<String>| v.into_iter().map(Raw::S).collect::<Vec<_>>();
        let raw = match st.k {
            Frame(id) => Some(strs(self.id3_text(id).unwrap_or_default())),
            Txxx(desc) | Comm(desc) => {
                let key = if matches!(st.k, Comm(_)) {
                    "COMM"
                } else {
                    "TXXX"
                };
                self.id3_desc_frames(key, desc)
                    .first()
                    .map(|t| strs(id3_split(t)))
            }
            TxxxList(desc, split_v23) => {
                let v23 = matches!(self, Tags::Id3 { v23: true, .. });
                Some(match self.id3_desc_frames("TXXX", desc).first() {
                    Some(t) => {
                        let vals = id3_split(t);
                        if v23 && split_v23 {
                            strs(
                                vals.iter()
                                    .flat_map(|v| v.split('/').map(str::to_string))
                                    .collect(),
                            )
                        } else {
                            strs(vals)
                        }
                    }
                    None => vec![],
                })
            }
            Uslt => self
                .id3_desc_frames("USLT", "")
                .first()
                .map(|t| vec![Raw::S(t.clone())]),
            Wxxx => self
                .id3_desc_frames("WXXX", "")
                .first()
                .map(|t| vec![Raw::S(t.clone())]),
            Ufid(owner) => self.id3_ufid(owner).map(|s| vec![Raw::S(s)]),
            People(frame, inv) => {
                let inv = inv.to_lowercase();
                Some(strs(
                    self.id3_people(frame)
                        .into_iter()
                        .filter(|(k, _)| k.to_lowercase() == inv)
                        .map(|(_, v)| v)
                        .collect(),
                ))
            }
            Atom(key) => self.mp4_values(key),
            Mp4Int(key) => self
                .mp4()
                .get(&mp4_ident(key))
                .map(|a| a.data().filter_map(mp4_int).map(Raw::I).collect::<Vec<_>>()),
            Key(key) => self.plain_values(key).map(strs),
            Asf(name) => Some(self.asf_values(name).iter().map(asf_raw).collect()),
            SlashPack(..) | Tuple(..) | Mp4Bool(_) | Id3Sc(_) | Mp4Sc(_) => {
                return self.get(st).map(|r| vec![r]);
            }
        }?;
        Some(raw.into_iter().map(|r| deserialize(st, r)).collect())
    }

    /// A style's `get`: the first value.
    fn get(&self, st: &S) -> Option<Raw> {
        match st.k {
            SlashPack(id, pos) => {
                let text = self.id3_text(id)?.into_iter().next()?;
                if text.is_empty() {
                    return None;
                }
                text.split('/').nth(pos).map(|s| Raw::S(s.to_string()))
            }
            Tuple(key, idx) => {
                let (a, b) = self.mp4_tuple(key).unwrap_or((0, 0));
                let v = if idx == 0 { a } else { b };
                (v != 0).then_some(Raw::I(i64::from(v)))
            }
            Mp4Bool(key) => {
                let atom = self.mp4().get(&mp4_ident(key))?;
                atom.data().next().and_then(mp4_int).map(|i| Raw::B(i != 0))
            }
            Id3Sc(idx) => {
                let data = self
                    .id3_desc_frames("COMM", "iTunNORM")
                    .into_iter()
                    .next()?;
                let data = id3_split(&data).into_iter().next().unwrap_or_default();
                let (g, p) = sc_decode(&data);
                Some(Raw::F(if idx == 0 { g } else { p }))
            }
            Mp4Sc(idx) => {
                let data = self
                    .mp4_values(&format!("{ITUNES}iTunNORM"))?
                    .into_iter()
                    .next()?;
                let (g, p) = sc_decode(&data.as_str());
                Some(Raw::F(if idx == 0 { g } else { p }))
            }
            _ => self.get_list(st)?.into_iter().next(),
        }
    }

    // ---- ASF helpers

    fn asf(&self) -> Option<&crate::asf::Asf> {
        match self {
            Tags::Asf(a) => Some(a),
            _ => None,
        }
    }

    fn asf_mut(&mut self) -> Option<&mut crate::asf::Asf> {
        match self {
            Tags::Asf(a) => Some(a),
            _ => None,
        }
    }

    fn asf_values(&self, name: &str) -> Vec<crate::asf::Value> {
        self.asf().map(|a| a.values(name)).unwrap_or_default()
    }

    /// A style's `set` (one value).
    // Matches beets (Python): tag values are truncated/rounded as Python does.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn set(&mut self, st: &S, v: &In) {
        match st.k {
            SlashPack(id, pos) => {
                let cur = self
                    .id3_text(id)
                    .and_then(|t| t.into_iter().next())
                    .unwrap_or_default();
                let mut items: Vec<Option<String>> = if cur.is_empty() {
                    vec![]
                } else {
                    cur.split('/').map(|s| Some(s.to_string())).collect()
                };
                while items.len() < 2 {
                    items.push(None);
                }
                items[pos] = Some(v.int().to_string());
                if items[0].is_none() {
                    items[0] = Some(String::new());
                }
                if items[1].is_none() {
                    items.pop();
                }
                let text = items.into_iter().flatten().collect::<Vec<_>>().join("/");
                self.id3_set_text(id, &[text]);
            }
            Tuple(key, idx) => {
                let (mut a, mut b) = self.mp4_tuple(key).unwrap_or((0, 0));
                let n = v.int().clamp(0, i64::from(u16::MAX)) as u16;
                if idx == 0 {
                    a = n;
                } else {
                    b = n;
                }
                self.mp4_set_tuple(key, (a, b));
            }
            Mp4Bool(key) => {
                self.mp4_set_data(key, AtomData::Bool(v.int() != 0));
            }
            Mp4Int(key) => {
                // `tmpo` is the tempo in BPM as a big-endian signed
                // 16-bit integer (iTunes metadata data type 21), so it
                // is clamped to that range.
                let n = v.int().clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
                self.mp4_set_data(
                    key,
                    AtomData::Unknown {
                        code: DataType::BeSignedInteger,
                        data: n.to_be_bytes().to_vec(),
                    },
                );
            }
            Id3Sc(idx) | Mp4Sc(idx) => {
                let cur = match st.k {
                    Id3Sc(_) => self
                        .id3_desc_frames("COMM", "iTunNORM")
                        .into_iter()
                        .next()
                        .map(|d| id3_split(&d).into_iter().next().unwrap_or_default()),
                    _ => self
                        .mp4_values(&format!("{ITUNES}iTunNORM"))
                        .and_then(|v| v.into_iter().next())
                        .map(|r| r.as_str()),
                };
                let mut gp = match cur {
                    Some(d) => {
                        let (g, p) = sc_decode(&d);
                        [g, p]
                    }
                    None => [0.0, 0.0],
                };
                gp[idx] = v.float();
                let data = sc_encode(gp[0], gp[1]);
                match st.k {
                    Id3Sc(_) => self.id3_store_desc("COMM", "iTunNORM", *b"eng", data),
                    _ => self.mp4_set_strings(&format!("{ITUNES}iTunNORM"), &[data]),
                }
            }
            Txxx(desc) => self.id3_store_desc("TXXX", desc, *b"XXX", v.serialize(st)),
            Comm(desc) => self.id3_store_desc("COMM", desc, *b"XXX", v.serialize(st)),
            Uslt => self.id3_store_desc("USLT", "", *b"XXX", v.serialize(st)),
            // Only in EXTRA_FIELDS, which are never written.
            Wxxx => {}
            Ufid(owner) => self.id3_set_ufid(owner, Some(&v.serialize(st))),
            Asf(name) => {
                let value = match v {
                    In::B(b) => crate::asf::Value::Bool(*b),
                    _ => crate::asf::Value::Unicode(v.serialize(st)),
                };
                if let Some(a) = self.asf_mut() {
                    a.set(name, vec![value]);
                }
            }
            // `set_list` adds the suffix.
            Frame(_) | TxxxList(..) | People(..) | Atom(_) | Key(_) => {
                self.set_list(st, &[v.bare(st)]);
            }
        }
    }

    /// A style's `set_list` / `store` of a list.
    fn set_list(&mut self, st: &S, values: &[String]) {
        let values: Vec<String> = values.iter().map(|v| format!("{v}{}", st.suffix)).collect();
        match st.k {
            Frame(id) => self.id3_set_text(id, &values),
            TxxxList(desc, _) => self.id3_store_desc_list(desc, &values),
            People(frame, inv) => self.id3_store_people(frame, inv, &values),
            Atom(key) => self.mp4_set_strings(key, &values),
            Key(key) => self.plain_set(key, &values),
            Asf(name) => {
                let values = values.into_iter().map(crate::asf::Value::Unicode).collect();
                if let Some(a) = self.asf_mut() {
                    a.set(name, values);
                }
            }
            _ => {
                if let Some(v) = values.into_iter().next() {
                    self.set(st, &In::S(v));
                } else {
                    self.delete(st);
                }
            }
        }
    }

    /// A style's `delete`.
    fn delete(&mut self, st: &S) {
        match st.k {
            Frame(id) => {
                let _ = self.id3_mut().remove(&fid(id)).count();
            }
            SlashPack(id, 0) => {
                let _ = self.id3_mut().remove(&fid(id)).count();
            }
            SlashPack(id, _) => {
                // `set(None)`: keep the number, drop the total.
                if let Some(first) = self
                    .id3_text(id)
                    .and_then(|t| t.into_iter().next())
                    .filter(|t| !t.is_empty())
                {
                    let n = first.split('/').next().unwrap_or("").to_string();
                    self.id3_set_text(id, &[n]);
                }
            }
            Txxx(desc) | TxxxList(desc, _) => self.id3_delete_desc("TXXX", desc),
            Comm(desc) => self.id3_delete_desc("COMM", desc),
            Uslt => self.id3_delete_desc("USLT", ""),
            Wxxx => {}
            Id3Sc(_) => self.id3_delete_desc("COMM", "iTunNORM"),
            Ufid(owner) => self.id3_set_ufid(owner, None),
            People(frame, inv) => self.id3_store_people(frame, inv, &[]),
            Tuple(key, 0) => self.mp4_delete(key),
            Tuple(key, _) => {
                if let Some((a, _)) = self.mp4_tuple(key) {
                    self.mp4_set_tuple(key, (a, 0));
                }
            }
            Atom(key) | Mp4Bool(key) | Mp4Int(key) => self.mp4_delete(key),
            Mp4Sc(_) => self.mp4_delete(&format!("{ITUNES}iTunNORM")),
            Key(key) => self.plain_set(key, &[]),
            Asf(name) => {
                if let Some(a) = self.asf_mut() {
                    a.remove(name);
                }
            }
        }
    }
}

// --------------------------------------------------------------- the file

/// A file's tags, as MediaFile sees them.
pub struct MediaFile {
    path: PathBuf,
    kind: Kind,
    tags: Tags,
    /// A new `synced_lyrics` value (None: unchanged), stored when saving.
    sylt: Option<Vec<(String, u32)>>,
}

/// The container a file uses: lofty's formats, or ASF (Windows Media),
/// which lofty doesn't know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Lofty(FileType),
    Asf,
}

/// A field's value as MediaFile returns it (`None` where it returns None).
#[derive(Clone, Debug, PartialEq)]
pub enum Got {
    Str(Option<String>),
    Int(Option<i64>),
    Bool(Option<bool>),
    Float(Option<f64>),
    List(Option<Vec<String>>),
}

/// A field by its name (`FIELDS`, then `EXTRA_FIELDS`).
fn field_named(name: &str) -> Option<&'static Field> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<&'static str, &'static Field>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        FIELDS
            .iter()
            .chain(EXTRA_FIELDS)
            .map(|f| (f.name, f))
            .collect()
    })
    .get(name)
    .copied()
}

impl MediaFile {
    /// A file's tags.
    pub fn open(path: &Path) -> Result<MediaFile> {
        Ok(Self::open_impl(path, false)?.0)
    }

    /// A file's tags and audio properties, from one read of the file.
    pub fn open_with_properties(path: &Path) -> Result<(MediaFile, FileProperties)> {
        Self::open_impl(path, true)
    }

    fn open_impl(path: &Path, read_properties: bool) -> Result<(MediaFile, FileProperties)> {
        // ASF (Windows Media) is outside lofty: read its own header.
        if crate::asf::is_asf(path) {
            let a = crate::asf::Asf::open(path)?;
            let properties = a.properties();
            return Ok((
                MediaFile {
                    path: path.to_path_buf(),
                    kind: Kind::Asf,
                    tags: Tags::Asf(a),
                    sylt: None,
                },
                properties,
            ));
        }
        let ft = lofty::probe::Probe::open(path)
            .with_context(|| format!("reading {}", path.display()))?
            .guess_file_type()?
            .file_type()
            .with_context(|| format!("unknown file type: {}", path.display()))?;
        let mut file =
            std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
        // Pictures are read too: saving the tag writes back what it holds.
        let opts = ParseOptions::new().read_properties(read_properties);
        // beets leaves "DISCNUMBER=2/3" alone (lofty would split it).
        let vorbis_opts = ParseOptions::new()
            .read_properties(read_properties)
            .implicit_conversions(false);
        let (tags, properties): (Tags, FileProperties) = match ft {
            FileType::Mpeg => {
                let f = lofty::mpeg::MpegFile::read_from(&mut file, opts)?;
                (
                    Self::id3(f.id3v2().cloned(), f.id3v1().cloned()),
                    (*f.properties()).into(),
                )
            }
            FileType::Aiff => {
                let f = lofty::iff::aiff::AiffFile::read_from(&mut file, opts)?;
                (
                    Self::id3(f.id3v2().cloned(), None),
                    f.properties().clone().into(),
                )
            }
            FileType::Wav => {
                let f = lofty::iff::wav::WavFile::read_from(&mut file, opts)?;
                (
                    Self::id3(f.id3v2().cloned(), None),
                    (*f.properties()).into(),
                )
            }
            FileType::Aac => {
                let f = lofty::aac::AacFile::read_from(&mut file, opts)?;
                (
                    Self::id3(f.id3v2().cloned(), None),
                    (*f.properties()).into(),
                )
            }
            FileType::Mp4 => {
                let f = lofty::mp4::Mp4File::read_from(&mut file, opts)?;
                (
                    Tags::Mp4(f.ilst().cloned().unwrap_or_default()),
                    f.properties().clone().into(),
                )
            }
            FileType::Flac => {
                let f = lofty::flac::FlacFile::read_from(&mut file, vorbis_opts)?;
                let mut vc = f.vorbis_comments().cloned().unwrap_or_default();
                // FLAC keeps pictures in their own blocks; written back
                // with the comments.
                for (p, info) in f.pictures() {
                    vc.insert_picture(p.clone(), Some(*info))?;
                }
                (Tags::Vorbis(vc), (*f.properties()).into())
            }
            FileType::Vorbis => {
                let f = lofty::ogg::VorbisFile::read_from(&mut file, vorbis_opts)?;
                (
                    Tags::Vorbis(f.vorbis_comments().clone()),
                    (*f.properties()).into(),
                )
            }
            FileType::Opus => {
                let f = lofty::ogg::OpusFile::read_from(&mut file, vorbis_opts)?;
                (
                    Tags::Vorbis(f.vorbis_comments().clone()),
                    (*f.properties()).into(),
                )
            }
            FileType::Speex => {
                let f = lofty::ogg::SpeexFile::read_from(&mut file, vorbis_opts)?;
                (
                    Tags::Vorbis(f.vorbis_comments().clone()),
                    (*f.properties()).into(),
                )
            }
            FileType::Ape => {
                let f = lofty::ape::ApeFile::read_from(&mut file, opts)?;
                (
                    Tags::Ape(f.ape().cloned().unwrap_or_default()),
                    f.properties().clone().into(),
                )
            }
            FileType::WavPack => {
                let f = lofty::wavpack::WavPackFile::read_from(&mut file, opts)?;
                (
                    Tags::Ape(f.ape().cloned().unwrap_or_default()),
                    (*f.properties()).into(),
                )
            }
            FileType::Mpc => {
                let f = lofty::musepack::MpcFile::read_from(&mut file, opts)?;
                (
                    Tags::Ape(f.ape().cloned().unwrap_or_default()),
                    f.properties().clone().into(),
                )
            }
            other => bail!("unsupported file type {other:?}: {}", path.display()),
        };
        Ok((
            MediaFile {
                path: path.to_path_buf(),
                kind: Kind::Lofty(ft),
                tags,
                sylt: None,
            },
            properties,
        ))
    }

    /// The file's type.
    /// The file's container: lofty's format, or ASF.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    fn id3(tag: Option<Id3v2Tag>, v1: Option<Id3v1Tag>) -> Tags {
        let v23 = tag
            .as_ref()
            .is_some_and(|t| t.original_version() == Id3v2Version::V3);
        Tags::Id3 {
            tag: tag.unwrap_or_default(),
            v23,
            v1,
        }
    }

    fn styles(&self, field: &Field) -> impl Iterator<Item = &'static S> {
        let fam = self.tags.family();
        let all: &'static [S] = match fam {
            Family::Asf => asf_styles(field.name),
            _ => field.styles,
        };
        all.iter().filter(move |s| s.k.family() == fam)
    }

    /// `MediaField.__get__`: the first style with a (truthy) value.
    fn get_raw(&self, field: &Field) -> Option<Raw> {
        let mut out = None;
        for st in self.styles(field) {
            out = self.tags.get(st);
            if out.as_ref().is_some_and(Raw::truthy) {
                break;
            }
        }
        out
    }

    /// `DateField._get_date_tuple`.
    fn date_tuple(&self, field: &Field) -> [Option<i64>; 3] {
        let Date(year_styles) = field.ty else {
            unreachable!()
        };
        let mut items: Vec<Option<String>> = match self.get_raw(field) {
            Some(Raw::S(s)) => {
                let s = match s.find(['T', 't', ' ']) {
                    Some(i) => &s[..i],
                    None => &s[..],
                };
                s.split(['-', '/']).map(|p| Some(p.to_string())).collect()
            }
            _ => vec![],
        };
        items.truncate(3);
        while items.len() < 3 {
            items.push(None);
        }
        let mut year_raw = None;
        if items[0].as_deref().is_none_or(str::is_empty) && !year_styles.is_empty() {
            let fam = self.tags.family();
            for st in year_styles.iter().filter(|s| s.k.family() == fam) {
                year_raw = self.tags.get(st);
                if year_raw.as_ref().is_some_and(Raw::truthy) {
                    break;
                }
            }
        }
        // Python's `int()`: surrounding whitespace allowed, nothing else.
        let int = |s: &str| s.trim().parse::<i64>().ok();
        let mut out = [None; 3];
        for (i, it) in items.iter().enumerate() {
            out[i] = it.as_deref().and_then(int);
        }
        if let Some(y) = year_raw {
            // `safe_cast(str, ...)` of the year field, then `int()`.
            out[0] = int(&y.as_str());
        }
        out
    }

    /// The value MediaFile returns for a beets item field.
    pub fn get(&self, name: &str) -> Option<Got> {
        let (field, part) = match name {
            "year" | "month" | "day" => (
                field_named("date")?,
                ["year", "month", "day"].iter().position(|p| *p == name),
            ),
            "original_year" | "original_month" | "original_day" => (
                field_named("original_date")?,
                ["original_year", "original_month", "original_day"]
                    .iter()
                    .position(|p| *p == name),
            ),
            _ => (field_named(name)?, None),
        };
        if let Some(i) = part {
            return Some(Got::Int(self.date_tuple(field)[i]));
        }
        Some(match field.ty {
            Str => Got::Str(self.get_raw(field).map(|r| r.as_str())),
            Int => Got::Int(self.get_raw(field).map(|r| r.as_int())),
            Bool => Got::Bool(self.get_raw(field).map(|r| r.as_bool())),
            Float => Got::Float(self.get_raw(field).map(|r| r.as_float())),
            QNum => Got::Float(self.get_raw(field).map(|r| r.as_int() as f64 / 256.0)),
            List => {
                let mut out = None;
                for st in self.styles(field) {
                    if let Some(v) = self.tags.get_list(st)
                        && !v.is_empty()
                    {
                        out = Some(v.iter().map(Raw::as_str).collect());
                        break;
                    }
                }
                Got::List(out)
            }
            Date(_) => return None,
        })
    }

    /// `DateField.__get__` for `date` or `original_date`: Python's
    /// `str(datetime.date(year, month or 1, day or 1))`, None without a
    /// year or for an impossible date.
    pub fn date(&self, name: &str) -> Option<String> {
        let field = field_named(name)?;
        if !matches!(field.ty, Date(_)) {
            return None;
        }
        let [y, m, d] = self.date_tuple(field);
        let y = y.filter(|&y| y != 0)?;
        let m = m.filter(|&m| m != 0).unwrap_or(1);
        let d = d.filter(|&d| d != 0).unwrap_or(1);
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let mdays = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > mdays[m as usize - 1]
        {
            return None;
        }
        Some(format!("{y:04}-{m:02}-{d:02}"))
    }

    /// `MediaFile.update` with an item's media fields (beets' `Item.write`).
    /// `MediaFile.update` with the item's media fields, the ones in `nulls`
    /// set to None (deleted), as beets' `Item.write` with a `write` listener
    /// (zero) setting tags to None. `nulls` may name MediaFile fields that
    /// aren't item fields: `catalognums` and `languages` delete their single
    /// fields' tags; `genre`, `composer`, `arranger`, `lyricist`, `date` and
    /// `original_date` are deleted and then written again from the item
    /// fields that sort after them, as in MediaFile. (`url`, `copyright`
    /// and pictures are left to the caller.)
    pub fn update_with(&mut self, item: &Item, nulls: &std::collections::BTreeSet<String>) {
        for field in FIELDS {
            match field.ty {
                Date(year_styles) => self.update_date(field, year_styles, item, nulls),
                _ => self.update_field(field, item, nulls.contains(field.name)),
            }
        }
        for (list, single) in [("catalognums", "catalognum"), ("languages", "language")] {
            if nulls.contains(list)
                && let Some(field) = field_named(single)
            {
                self.update_field(field, item, true);
            }
        }
    }

    // Matches beets (Python): tag values are truncated/rounded as Python does.
    #[allow(clippy::cast_possible_truncation)]
    fn update_field(&mut self, field: &Field, item: &Item, null: bool) {
        let value = if null { None } else { item.get(field.name) };
        let styles: Vec<&'static S> = self.styles(field).collect();
        // None: `delattr`, every style (read-only ones too).
        if matches!(value, Some(Value::Null) | None) {
            for st in styles {
                self.tags.delete(st);
            }
            return;
        }
        let value = value.unwrap();
        if matches!(field.ty, List) {
            let values = item.list(field.name);
            for st in styles.into_iter().filter(|s| !s.read_only) {
                if values.is_empty() {
                    self.tags.delete(st);
                } else {
                    self.tags.set_list(st, &values);
                }
            }
            return;
        }
        let v = match (field.ty, &value) {
            (Float, v) => In::F(v.as_f64().unwrap_or(0.0)),
            (QNum, v) => In::I((v.as_f64().unwrap_or(0.0) * 256.0).round_ties_even() as i64),
            (Bool, v) => In::B(v.as_f64().unwrap_or(0.0) != 0.0),
            (Int, v) => In::I(v.as_f64().unwrap_or(0.0) as i64),
            (_, v) => In::S(v.raw()),
        };
        // beets writes "", 0 and False as such; we remove the tag (read the
        // same way). A packed number stays while the other half is set.
        let empty = match &v {
            In::S(s) => s.is_empty(),
            In::I(i) => *i == 0,
            In::B(b) => !b,
            In::F(_) => false,
        };
        let pair_set = match field.name {
            "track" | "tracktotal" => item.int("track") != 0 || item.int("tracktotal") != 0,
            "disc" | "disctotal" => item.int("disc") != 0 || item.int("disctotal") != 0,
            _ => false,
        };
        for st in styles.into_iter().filter(|s| !s.read_only) {
            if empty && !(st.k.packed() && pair_set) {
                self.tags.delete(st);
            } else {
                self.tags.set(st, &v);
            }
        }
    }

    /// `DateField._set_date_tuple` for the item's year, month and day (the
    /// three `DateItemField` writes end there).
    fn update_date(
        &mut self,
        field: &Field,
        year_styles: &'static [S],
        item: &Item,
        nulls: &std::collections::BTreeSet<String>,
    ) {
        let prefix = if field.name == "original_date" {
            "original_"
        } else {
            ""
        };
        let part = |p: &str| {
            let name = format!("{prefix}{p}");
            if nulls.contains(&name) {
                0
            } else {
                item.int(&name)
            }
        };
        let (y, m, d) = (part("year"), part("month"), part("day"));
        let styles: Vec<&'static S> = self.styles(field).collect();
        let fam = self.tags.family();
        let year: Vec<&'static S> = year_styles.iter().filter(|s| s.k.family() == fam).collect();
        if y == 0 {
            for st in styles.into_iter().chain(year) {
                self.tags.delete(st);
            }
            return;
        }
        let mut date = format!("{y:04}");
        if m != 0 {
            date.push_str(&format!("-{m:02}"));
            if d != 0 {
                date.push_str(&format!("-{d:02}"));
            }
        }
        for st in styles.into_iter().filter(|s| !s.read_only) {
            self.tags.set(st, &In::S(date.clone()));
        }
        for st in year.into_iter().filter(|s| !s.read_only) {
            self.tags.set(st, &In::I(y));
        }
    }

    /// The embedded pictures (MediaFile's `images`).
    pub fn pictures(&self) -> Vec<Picture> {
        match &self.tags {
            Tags::Id3 { tag, .. } => tag
                .iter()
                .filter_map(|f| match f {
                    Frame::Picture(p) => Some(p.picture.clone().into_owned()),
                    _ => None,
                })
                .collect(),
            Tags::Mp4(ilst) => ilst
                .pictures()
                .map(|p| p.cloned().collect())
                .unwrap_or_default(),
            Tags::Vorbis(vc) => vc.pictures().iter().map(|(p, _)| p.clone()).collect(),
            Tags::Ape(ape) => ape
                .into_iter()
                .filter(|i| i.key().to_lowercase().starts_with("cover art ("))
                .filter_map(|i| match i.value() {
                    ItemValue::Binary(b) => Picture::from_ape_bytes(i.key(), b).ok(),
                    _ => None,
                })
                .collect(),
            Tags::Asf(a) => a
                .values("WM/Picture")
                .iter()
                .filter_map(unpack_asf_picture)
                .collect(),
        }
    }

    /// Put `pic` in as the front cover, replacing any front cover (in MP4,
    /// whose cover atoms have no type, every picture).
    pub fn set_front_cover(&mut self, mut pic: Picture) {
        pic.set_pic_type(PictureType::CoverFront);
        match &mut self.tags {
            Tags::Id3 { tag, .. } => {
                tag.remove_picture_type(PictureType::CoverFront);
                tag.insert_picture(pic);
            }
            Tags::Mp4(ilst) => {
                ilst.remove_pictures();
                ilst.insert_picture(pic);
            }
            Tags::Vorbis(vc) => {
                vc.remove_picture_type(PictureType::CoverFront);
                let _ = vc.insert_picture(pic, None);
            }
            Tags::Ape(ape) => {
                if let Some(key) = pic.pic_type().as_ape_key()
                    && let Ok(item) =
                        ApeItem::new(key.to_string(), ItemValue::Binary(pic.as_ape_bytes()))
                {
                    ape.insert(item);
                }
            }
            Tags::Asf(a) => {
                let mut pictures: Vec<Picture> = a
                    .values("WM/Picture")
                    .iter()
                    .filter_map(unpack_asf_picture)
                    .filter(|p| p.pic_type() != PictureType::CoverFront)
                    .collect();
                pictures.push(pic);
                let values = pictures.iter().map(pack_asf_picture).collect();
                a.set("WM/Picture", values);
            }
        }
    }

    /// MediaFile's `images = pictures`: every picture replaced by these,
    /// their types kept.
    pub fn set_pictures(&mut self, pictures: &[Picture]) {
        self.clear_pictures();
        if let Tags::Asf(a) = &mut self.tags {
            let values = pictures.iter().map(pack_asf_picture).collect();
            a.set("WM/Picture", values);
            return;
        }
        for pic in pictures.iter().cloned() {
            match &mut self.tags {
                Tags::Id3 { tag, .. } => {
                    tag.insert(Frame::Picture(lofty::id3::v2::AttachedPictureFrame::new(
                        TextEncoding::UTF8,
                        pic,
                    )));
                }
                Tags::Mp4(ilst) => ilst.insert_picture(pic),
                Tags::Vorbis(vc) => {
                    let _ = vc.insert_picture(pic, None);
                }
                Tags::Ape(ape) => {
                    if let Some(key) = pic.pic_type().as_ape_key()
                        && let Ok(item) =
                            ApeItem::new(key.to_string(), ItemValue::Binary(pic.as_ape_bytes()))
                    {
                        ape.insert(item);
                    }
                }
                // Handled above, before the loop.
                Tags::Asf(_) => {}
            };
        }
    }

    /// Remove every picture (MediaFile's `images = []`). Whether there
    /// were any.
    pub fn clear_pictures(&mut self) -> bool {
        match &mut self.tags {
            Tags::Id3 { tag, .. } => {
                let n = tag.len();
                tag.retain(|f| !matches!(f, Frame::Picture(_)));
                n != tag.len()
            }
            Tags::Mp4(ilst) => {
                let had = ilst.pictures().is_some_and(|mut p| p.next().is_some());
                ilst.remove_pictures();
                had
            }
            Tags::Vorbis(vc) => !vc.remove_pictures().is_empty(),
            Tags::Asf(a) => {
                let had = !a.values("WM/Picture").is_empty();
                a.remove("WM/Picture");
                had
            }
            Tags::Ape(ape) => {
                let keys: Vec<String> = (&*ape)
                    .into_iter()
                    .filter(|i| matches!(i.value(), ItemValue::Binary(_)))
                    .map(|i| i.key().to_string())
                    .filter(|k| k.to_lowercase().starts_with("cover art ("))
                    .collect();
                for k in &keys {
                    ape.remove(k);
                }
                !keys.is_empty()
            }
        }
    }

    /// Set MediaFile's `synced_lyrics` (`MP3SYLTStorageStyle`: ID3 only):
    /// `(text, milliseconds)` pairs, or none to remove the SYLT frames.
    pub fn set_synced_lyrics(&mut self, lines: &[(String, u32)]) {
        if matches!(self.tags, Tags::Id3 { .. }) {
            self.sylt = Some(lines.to_vec());
        }
    }

    /// `synced_lyrics`: the first SYLT frame's `(text, milliseconds)`.
    pub fn synced_lyrics(&self) -> Option<Vec<(String, u32)>> {
        let Tags::Id3 { tag, .. } = &self.tags else {
            return None;
        };
        tag.iter().find_map(|f| match f {
            Frame::Binary(b) if b.id().as_str() == "SYLT" => {
                SynchronizedTextFrame::parse(&b.data, lofty::id3::v2::FrameFlags::default())
                    .ok()
                    .map(|s| s.content.into_iter().map(|(ms, t)| (t, ms)).collect())
            }
            _ => None,
        })
    }

    /// Save the tags (`id3v23`: write MP3s' ID3 tags as ID3v2.3).
    pub fn save(self, id3v23: bool) -> Result<()> {
        let path = &self.path;
        let ctx = || format!("writing tags to {}", path.display());
        match self.tags {
            Tags::Id3 { mut tag, v1, .. } => {
                let v23 = id3v23 && self.kind == Kind::Lofty(FileType::Mpeg);
                let mut opts = WriteOptions::default();
                if v23 {
                    to_v23(&mut tag);
                    opts = opts.use_id3v23(true);
                }
                if let Some(sylt) = self.sylt {
                    store_sylt(&mut tag, &sylt, opts)?;
                }
                if let Some(mut v1) = v1 {
                    update_v1(&mut v1, &tag);
                    v1.save_to_path(path, WriteOptions::default())
                        .with_context(ctx)?;
                }
                let placeholder = if v23 { hide_v23_genre(&mut tag) } else { None };
                tag.save_to_path(path, opts).with_context(ctx)?;
                if let Some(id) = placeholder {
                    rename_v23_frame(path, id.as_bytes(), b"TCON").with_context(ctx)?;
                }
            }
            Tags::Mp4(t) => t
                .save_to_path(path, WriteOptions::default())
                .with_context(ctx)?,
            Tags::Vorbis(t) => t
                .save_to_path(path, WriteOptions::default())
                .with_context(ctx)?,
            Tags::Ape(t) => t
                .save_to_path(path, WriteOptions::default())
                .with_context(ctx)?,
            Tags::Asf(a) => a.save().with_context(ctx)?,
        }
        Ok(())
    }
}

/// `MP3SYLTStorageStyle.store`: one SYLT frame (UTF-8, language "XXX",
/// millisecond timestamps, lyrics) replacing any others; none for no lines.
fn store_sylt(tag: &mut Id3v2Tag, lines: &[(String, u32)], opts: WriteOptions) -> Result<()> {
    tag.retain(|f| f.id().as_str() != "SYLT");
    if lines.is_empty() {
        return Ok(());
    }
    let frame = SynchronizedTextFrame::new(
        TextEncoding::UTF8,
        *b"XXX",
        TimestampFormat::MS,
        SyncTextContentType::Lyrics,
        None,
        lines.iter().map(|(t, ms)| (*ms, t.clone())).collect(),
    );
    let data = frame
        .as_bytes(opts)
        .map_err(|e| anyhow::anyhow!("encoding SYLT frame: {e}"))?;
    tag.insert(Frame::Binary(BinaryFrame::new(
        FrameId::Valid(Cow::Borrowed("SYLT")),
        data,
    )));
    Ok(())
}

/// Downgrade the in-memory tag to what ID3v2.3 can store.
///
/// ID3v2.4 allows a text information frame to hold several values,
/// NUL separated, while ID3v2.3 stores one string per frame and uses
/// "/" between the parts of a value that has several (ID3v2.3 §4.2,
/// "Text information frames"; compare v2.4 §4.2). The same applies to
/// a frame's description/content pair (TXXX, COMM). TCON is handled
/// first, through the genre rules, so that its references become the
/// genre names v2.3 can carry; the writer in turn applies the date
/// frame conversion that v2.4 Appendix A prescribes for saving as
/// v2.3 (TDRC to TYER/TDAT/TIME, TDOR to TORY, IPLS to TIPL) and
/// drops frames that only exist in v2.4.
fn to_v23(tag: &mut Id3v2Tag) {
    tag.retain_mut(|fr| {
        match fr {
            Frame::Text(t) if t.id().as_str() == "TCON" => {
                t.value = Cow::Owned(tcon_genres(&id3_split(&t.value)).join("/"));
            }
            Frame::Text(t) if t.value.contains('\0') => {
                t.value = Cow::Owned(downgrade_values(&t.value));
            }
            Frame::UserText(t) if t.content.contains('\0') => {
                t.content = Cow::Owned(downgrade_values(&t.content));
            }
            Frame::Comment(t) if t.content.contains('\0') => {
                t.content = Cow::Owned(downgrade_values(&t.content));
            }
            _ => {}
        }
        true
    });
}

/// One v2.3 string from a v2.4 NUL-separated list.
fn downgrade_values(value: &str) -> String {
    id3_split(value).join("/")
}

/// lofty writes an ID3v2.3 `TCON` its own way (a number or "Remix" in
/// parentheses, "(80)"), where beets writes the genre text as it is
/// ("80"). So the genre goes out under an unused text frame ID, which
/// `rename_v23_frame` turns back into `TCON` in the file. Returns that ID
/// (None without a genre, or for an unsynchronised tag, where frame
/// headers can't be found by walking them).
fn hide_v23_genre(tag: &mut Id3v2Tag) -> Option<&'static str> {
    if tag.flags().unsynchronisation {
        return None;
    }
    let id = ["TCOX", "TCOY", "TCOZ", "TXGN"]
        .into_iter()
        .find(|id| !tag.iter().any(|f| f.id_str() == *id))?;
    let mut found = None;
    tag.retain(|f| match f {
        Frame::Text(t) if f.id_str() == "TCON" => {
            found = Some((t.encoding, t.value.to_string()));
            false
        }
        _ => true,
    });
    let (encoding, value) = found?;
    tag.insert(Frame::Text(TextInformationFrame::new(
        fid(id),
        encoding,
        value,
    )));
    Some(id)
}

/// Rename the frame `from` of the ID3v2.3 tag at the start of `path` to
/// `to` (same length: nothing else in the file moves).
fn rename_v23_frame(path: &Path, from: &[u8], to: &[u8]) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut h = [0u8; 10];
    f.read_exact(&mut h)?;
    if &h[..3] != b"ID3" || h[3] != 3 {
        bail!("no ID3v2.3 tag after writing");
    }
    let size = h[6..10]
        .iter()
        .fold(0usize, |acc, &b| (acc << 7) | (b & 0x7f) as usize);
    let mut body = vec![0u8; size];
    f.read_exact(&mut body)?;
    let mut pos = 0;
    if h[5] & 0x40 != 0 {
        // Extended header: its size (excluding these 4 bytes), then it.
        let n = u32::from_be_bytes(body.get(..4).context("bad tag")?.try_into()?) as usize;
        pos = 4 + n;
    }
    while pos + 10 <= body.len() && body[pos] != 0 {
        let id = &body[pos..pos + 4];
        let len = u32::from_be_bytes(body[pos + 4..pos + 8].try_into()?) as usize;
        if id == from {
            f.seek(SeekFrom::Start(10 + pos as u64))?;
            f.write_all(to)?;
            return Ok(());
        }
        pos += 10 + len;
    }
    bail!(
        "frame {} not found after writing",
        String::from_utf8_lossy(from)
    )
}

/// Update an existing ID3v1 tag from the ID3v2 frames before saving.
///
/// ID3v1 (§"The ID3v1 tag") stores title, artist, album and a comment
/// in 30-character text fields, then a four-digit year and a one-byte
/// genre number from the ID3v1 genre list. ID3v1.1 takes the last two
/// bytes of the comment field for a track number: a zero byte
/// followed by the number. The ID3v2 tag is the source of truth here:
/// its text frames fill those fields, TRCK's first number (the part
/// before "/") becomes the v1.1 track, TCON's first genre name its
/// number, and TDRC its first four characters the year. An ID3v2
/// comment cannot fill the v1 comment field (that field would have to
/// give up its track number), so the v1 comment is left clear.
fn update_v1(v1: &mut Id3v1Tag, tag: &Id3v2Tag) {
    /// The first text value of an ID3v2 text frame (or its timestamp).
    fn first_text(tag: &Id3v2Tag, id: &str) -> Option<String> {
        tag.iter().find_map(|f| match f {
            Frame::Text(t) if f.id_str() == id => id3_split(&t.value).into_iter().next(),
            Frame::Timestamp(t) if f.id_str() == id => Some(t.timestamp.to_string()),
            _ => None,
        })
    }

    /// An ID3v1 text field: empty values are not written and the text
    /// is cut to the field's 30 characters.
    fn v1_text(tag: &Id3v2Tag, id: &str) -> Option<String> {
        const V1_TEXT_LEN: usize = 30;
        first_text(tag, id)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(V1_TEXT_LEN).collect())
    }

    v1.title = v1_text(tag, "TIT2");
    v1.artist = v1_text(tag, "TPE1");
    v1.album = v1_text(tag, "TALB");
    v1.comment = None;
    v1.track_number =
        first_text(tag, "TRCK").and_then(|t| t.split('/').next()?.trim().parse::<u8>().ok());
    v1.genre = first_text(tag, "TCON").and_then(|g| {
        let first = tcon_genres(&id3_split(&g)).into_iter().next()?;
        genre_index(&first)
    });
    // The ID3v1 year is four digits ("YYYY").
    v1.year =
        first_text(tag, "TDRC").and_then(|y| y.chars().take(4).collect::<String>().parse().ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, normalize_key};
    use lofty::file::TaggedFileExt;

    /// A 0.3 s file made by ffmpeg (None when ffmpeg is missing).
    fn make(dir: &Path, name: &str, args: &[&str]) -> Option<PathBuf> {
        let p = dir.join(name);
        std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.3"])
            .args(args)
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success())
            .then_some(p)
    }

    fn read(p: &Path) -> Item {
        crate::media::read(p).unwrap()
    }

    /// A FLAC whose Vorbis comments are exactly `comments`, in order.
    /// ffmpeg rewrites some key names itself (7.x folds TRACK and
    /// TRACKNUMBER into one `track` tag), so the comment block is written
    /// out by hand instead of through `-metadata`.
    fn flac_comments(dir: &Path, name: &str, comments: &[(&str, &str)]) -> Option<PathBuf> {
        let p = make(dir, name, &[])?;
        let data = std::fs::read(&p).ok()?;
        if data.get(..4) != Some(b"fLaC") {
            return None;
        }
        // The metadata blocks, without any existing comment block.
        let mut blocks: Vec<(u8, &[u8])> = vec![];
        let mut pos = 4;
        loop {
            let header = data.get(pos..pos + 4)?;
            let last = header[0] & 0x80 != 0;
            let kind = header[0] & 0x7f;
            let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
            let body = data.get(pos + 4..pos + 4 + len)?;
            if kind != 4 {
                blocks.push((kind, body));
            }
            pos += 4 + len;
            if last {
                break;
            }
        }
        let mut vc = Vec::new();
        vc.extend_from_slice(&4u32.to_le_bytes());
        vc.extend_from_slice(b"test");
        vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for (k, v) in comments {
            let c = format!("{k}={v}");
            vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
            vc.extend_from_slice(c.as_bytes());
        }
        // After STREAMINFO, where ffmpeg puts it too.
        blocks.insert(1.min(blocks.len()), (4, &vc));
        let mut out = b"fLaC".to_vec();
        for (i, (kind, body)) in blocks.iter().enumerate() {
            let last = if i + 1 == blocks.len() { 0x80 } else { 0 };
            out.push(last | kind);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
            out.extend_from_slice(body);
        }
        out.extend_from_slice(&data[pos..]);
        std::fs::write(&p, out).ok()?;
        Some(p)
    }

    /// Every tag field set to something.
    fn full_item(p: &Path) -> Item {
        let mut it = read(p);
        for f in TAG_FIELDS {
            match it.field_type(f) {
                Some(crate::model::FieldType::List(_)) => {
                    it.set_list(f, &[format!("{f}1"), format!("{f}2")]);
                }
                Some(crate::model::FieldType::Str) => it.set_str(f, format!("{f} v")),
                _ => {}
            }
        }
        for (f, v) in [
            ("track", 3),
            ("tracktotal", 12),
            ("disc", 1),
            ("disctotal", 2),
            ("bpm", 120),
            ("year", 2001),
            ("month", 3),
            ("day", 9),
            ("original_year", 1990),
            ("original_month", 4),
        ] {
            it.set_int(f, v);
        }
        it.set("comp", Value::Bool(true));
        for (f, v) in [
            ("rg_track_gain", -3.5),
            ("rg_track_peak", 0.98),
            ("rg_album_gain", 1.25),
            ("rg_album_peak", 0.5),
            ("r128_track_gain", -2.25),
            ("r128_album_gain", 3.0),
        ] {
            it.set(f, Value::Float(v));
        }
        it.set_str("initial_key", "Ebm");
        it
    }

    /// Tags and audio properties come from one read of the file (the
    /// import path reads thousands of files), with the same values a
    /// properties-only read gives.
    #[test]
    fn open_with_properties_matches_separate_reads() {
        let d = tempfile::tempdir().unwrap();
        for ext in ["mp3", "flac", "m4a", "ogg"] {
            let Some(p) = make(d.path(), &format!("p.{ext}"), &[]) else {
                return;
            };
            let file = lofty::read_from_path(&p).unwrap();
            let want = file.properties().clone();
            let (mf, props) = MediaFile::open_with_properties(&p).unwrap();
            assert_eq!(props, want, "{ext}");
            assert_eq!(mf.get("title"), MediaFile::open(&p).unwrap().get("title"));
            assert_eq!(mf.kind(), Kind::Lofty(file.file_type()));
        }
    }

    /// Fields a format can't hold (MediaFile has no style for them there).
    fn unsupported(ext: &str, f: &str) -> bool {
        ext == "m4a" && f == "subtitle"
    }

    /// Everything written comes back, in every format; MediaFile's
    /// single fields that share a list's tag (`albumtype`, `mb_artistid`,
    /// `catalognum`, `language`) read the list's first value.
    #[test]
    fn every_field_round_trips_in_every_format() {
        let d = tempfile::tempdir().unwrap();
        for ext in ["mp3", "m4a", "flac", "ogg", "opus", "wv", "aiff", "wav"] {
            let Some(p) = make(d.path(), &format!("t.{ext}"), &[]) else {
                return;
            };
            let it = full_item(&p);
            crate::media::write(&it).unwrap();
            let back = read(&p);
            for f in TAG_FIELDS {
                let want = match *f {
                    "albumtype" => "albumtypes1".to_string(),
                    "mb_artistid" => "mb_artistids1".to_string(),
                    "mb_albumartistid" => "mb_albumartistids1".to_string(),
                    _ if unsupported(ext, f) => String::new(),
                    _ => it.formatted(f).unwrap(),
                };
                assert_eq!(back.formatted(f).unwrap(), want, "{ext}: {f}");
            }
            assert_eq!(back.str("initial_key"), "D#m");
            assert_eq!(back.float("r128_track_gain"), -2.25, "{ext}");
            // Clearing removes the tags (beets writes them empty).
            let mut empty = Item::new_item();
            empty.set_path(&p);
            crate::media::write(&empty).unwrap();
            let back = read(&p);
            for f in TAG_FIELDS {
                assert_eq!(back.get(f), empty.get(f), "{ext}: cleared {f}");
            }
        }
        // ASF (Windows Media), whose tags aren't lofty's.
        let Some(p) = make(d.path(), "t.wma", &["-c:a", "wmav2"]) else {
            return;
        };
        let it = full_item(&p);
        crate::media::write(&it).unwrap();
        let back = read(&p);
        for f in TAG_FIELDS {
            let want = match *f {
                "albumtype" => "albumtypes1".to_string(),
                "mb_artistid" => "mb_artistids1".to_string(),
                "mb_albumartistid" => "mb_albumartistids1".to_string(),
                _ => it.formatted(f).unwrap(),
            };
            assert_eq!(back.formatted(f).unwrap(), want, "wma: {f}");
        }
        assert_eq!(back.str("initial_key"), "D#m");
        assert_eq!(back.float("r128_track_gain"), -2.25, "wma");
        let mut empty = Item::new_item();
        empty.set_path(&p);
        crate::media::write(&empty).unwrap();
        let back = read(&p);
        for f in TAG_FIELDS {
            assert_eq!(back.get(f), empty.get(f), "wma: cleared {f}");
        }
    }

    /// `id3v23`: an ID3v2.3 tag, multiple values joined with "/" (which
    /// MediaFile splits again only for its `split_v23` fields), dates as
    /// TYER/TDAT.
    #[test]
    fn id3v23_writes_version_3() {
        let d = tempfile::tempdir().unwrap();
        let Some(p) = make(d.path(), "t.mp3", &[]) else {
            return;
        };
        let mut it = read(&p);
        it.set_list("genres", &["Rock", "Pop"]);
        it.set_list("mb_artistids", &["a", "b"]);
        it.set_list("artists", &["A", "B"]);
        it.set_int("year", 2001);
        it.set_int("month", 3);
        it.set_int("day", 9);
        it.set_int("original_year", 1990);
        crate::media::write_with(&it, &p, true).unwrap();
        let mf = MediaFile::open(&p).unwrap();
        let Tags::Id3 { tag, v23, .. } = &mf.tags else {
            panic!()
        };
        assert!(v23);
        assert_eq!(tag.original_version(), Id3v2Version::V3);
        let back = read(&p);
        assert_eq!(back.list("genres"), ["Rock/Pop"]);
        assert_eq!(back.list("artists"), ["A/B"]);
        assert_eq!(back.list("mb_artistids"), ["a", "b"]);
        assert_eq!(back.str("mb_artistid"), "a");
        assert_eq!(
            (back.int("year"), back.int("month"), back.int("day")),
            (2001, 3, 9)
        );
        assert_eq!(back.int("original_year"), 1990);
        // Without the option the next write is ID3v2.4 again.
        crate::media::write_with(&back, &p, false).unwrap();
        let mf = MediaFile::open(&p).unwrap();
        assert!(matches!(mf.tags, Tags::Id3 { v23: false, .. }));
    }

    /// Embedding and clearing art keeps every tag.
    #[test]
    fn art_changes_keep_the_tags() {
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99,
            0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let d = tempfile::tempdir().unwrap();
        for ext in ["mp3", "m4a", "flac", "ogg", "wv", "wma"] {
            let args: &[&str] = if ext == "wma" {
                &["-c:a", "wmav2"]
            } else {
                &[]
            };
            let Some(p) = make(d.path(), &format!("t.{ext}"), args) else {
                return;
            };
            let it = full_item(&p);
            crate::media::write(&it).unwrap();
            let before = read(&p);
            crate::media::embed_art(&p, png).unwrap();
            assert!(crate::media::has_art(&p).unwrap(), "{ext}");
            let after = read(&p);
            for f in TAG_FIELDS {
                assert_eq!(after.get(f), before.get(f), "{ext}: {f} after embedding");
            }
            crate::media::clear_art(&p).unwrap();
            let after = read(&p);
            for f in TAG_FIELDS {
                assert_eq!(after.get(f), before.get(f), "{ext}: {f} after clearing");
            }
        }
    }

    /// MediaFile's reading rules on tags other software writes.
    #[test]
    fn reads_like_mediafile() {
        let d = tempfile::tempdir().unwrap();
        let comments = [
            ("ALBUM ARTIST", "First"),
            ("ALBUMARTIST", "Second"),
            ("TRACK", "0"),
            ("TRACKNUMBER", "3"),
            ("DISCNUMBER", "2/3"),
            ("genre", "Rock; Pop"),
            ("UPC", "123"),
            ("REPLAYGAIN_TRACK_GAIN", "-4.20 dB"),
            ("R128_TRACK_GAIN", "-512"),
            ("DATE", "2003-05-12T10:00"),
            ("YEAR", "1999"),
            ("initialkey", "Db Minor"),
        ];
        let Some(p) = flac_comments(d.path(), "t.flac", &comments) else {
            return;
        };
        let it = read(&p);
        assert_eq!(it.str("albumartist"), "First");
        // "0" is a value: the first style with one wins.
        assert_eq!(it.int("track"), 0);
        assert_eq!((it.int("disc"), it.int("disctotal")), (2, 0));
        // beets splits a list value holding "; ".
        assert_eq!(it.list("genres"), ["Rock", "Pop"]);
        assert_eq!(it.str("barcode"), "123");
        assert_eq!(it.float("rg_track_gain"), -4.2);
        assert_eq!(it.float("r128_track_gain"), -2.0);
        assert_eq!(it.get("rg_album_gain"), Some(Value::Null));
        assert_eq!(
            (it.int("year"), it.int("month"), it.int("day")),
            (2003, 5, 12)
        );
        assert_eq!(it.str("initial_key"), "Cm");
        // A missing DATE falls back to YEAR.
        let Some(p) = flac_comments(d.path(), "y.flac", &[("YEAR", "1999")]) else {
            return;
        };
        assert_eq!(read(&p).int("year"), 1999);
        // ffmpeg's MP3 "compilation" is a TXXX frame MediaFile doesn't read.
        let Some(p) = make(d.path(), "c.mp3", &["-metadata", "compilation=1"]) else {
            return;
        };
        assert!(!read(&p).bool("comp"));
    }

    /// Writing doesn't touch the read-only alternatives (MediaFile reads
    /// them when the main tag is empty), and keeps unrelated tags.
    #[test]
    fn writes_leave_read_only_and_unknown_tags() {
        let d = tempfile::tempdir().unwrap();
        let Some(p) = make(
            d.path(),
            "t.ogg",
            &["-metadata", "UPC=999", "-metadata", "MOOD=calm"],
        ) else {
            return;
        };
        let mut it = read(&p);
        it.set_str("barcode", "111");
        crate::media::write(&it).unwrap();
        let Tags::Vorbis(vc) = MediaFile::open(&p).unwrap().tags else {
            panic!()
        };
        assert_eq!(vc.get("BARCODE"), Some("111"));
        assert_eq!(vc.get("UPC"), Some("999"));
        assert_eq!(vc.get("MOOD"), Some("calm"));
        it.set_str("barcode", "");
        crate::media::write(&it).unwrap();
        assert_eq!(read(&p).str("barcode"), "999");
    }

    /// An MP3 with only an ID3v1 tag reads through it, as beets merges
    /// it; writing updates it.
    #[test]
    fn id3v1_tags_are_read_and_updated() {
        let d = tempfile::tempdir().unwrap();
        let Some(p) = make(
            d.path(),
            "t.mp3",
            &[
                "-write_id3v1",
                "1",
                "-metadata",
                "title=Old",
                "-metadata",
                "artist=Art",
                "-metadata",
                "date=1999",
                "-metadata",
                "track=4",
            ],
        ) else {
            return;
        };
        lofty::tag::TagType::Id3v2
            .remove_from_path(&p, WriteOptions::default())
            .unwrap();
        let mut it = read(&p);
        assert_eq!(
            (
                it.str("title"),
                it.str("artist"),
                it.int("year"),
                it.int("track")
            ),
            ("Old", "Art", 1999, 4)
        );
        it.set_str("title", "New");
        it.set_str("artist", "NewArt");
        it.set_str("album", "NewAlb");
        it.set_int("year", 2001);
        it.set_int("track", 3);
        it.set_list("genres", &["Pop"]);
        crate::media::write(&it).unwrap();
        let f = lofty::mpeg::MpegFile::read_from(
            &mut std::fs::File::open(&p).unwrap(),
            ParseOptions::new(),
        )
        .unwrap();
        assert_eq!(f.id3v1().unwrap().title.as_deref(), Some("New"));
        assert_eq!(read(&p).str("title"), "New");
        // The ID3v1 field layout (title, artist, album, year, comment
        // with the v1.1 track number, genre number).
        let raw = std::fs::read(&p).unwrap();
        let v1 = &raw[raw.len() - 128..];
        assert_eq!(&v1[..3], b"TAG");
        assert_eq!(
            &v1[3..33],
            b"New\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
        );
        assert_eq!(
            &v1[33..63],
            b"NewArt\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
        );
        assert_eq!(
            &v1[63..93],
            b"NewAlb\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
        );
        assert_eq!(&v1[93..97], b"2001");
        assert_eq!(v1[125], 0, "v1.1 track marker");
        assert_eq!(v1[126], 3, "v1.1 track number");
        assert_eq!(v1[127], 13, "genre 13 = Pop");
    }

    /// The genre tables cover ID3v1's 80 numbers and Winamp's 112
    /// extensions, and `genre_index()` inverts `genre_name()`.
    #[test]
    fn genre_table_covers_both_lists() {
        assert_eq!(ID3V1_GENRES.len(), 80);
        assert_eq!(WINAMP_GENRES.len(), 112);
        for (i, name) in ID3V1_GENRES.iter().chain(WINAMP_GENRES.iter()).enumerate() {
            assert_eq!(genre_name(i), Some(*name), "{i}");
            assert_eq!(genre_index(name), Some(i as u8), "{name}");
        }
        assert_eq!(genre_name(192), None);
        assert_eq!(genre_index("No Such Genre"), None);
    }

    /// TCON values to genre names (expected values checked against
    /// beets 2.14.1, which reads and writes every ID3v1/Winamp genre).
    #[test]
    fn tcon_genre_parsing() {
        let g = |v: &[&str]| tcon_genres(&v.iter().map(ToString::to_string).collect::<Vec<_>>());
        assert_eq!(g(&["80"]), ["Folk"]);
        assert_eq!(g(&["255"]), ["Unknown"]);
        assert_eq!(g(&["300"]), ["300"]);
        assert_eq!(g(&["RX", "CR"]), ["Remix", "Cover"]);
        assert_eq!(g(&["(17)Rock"]), ["Rock"]);
        assert_eq!(g(&["(17)(RX)Foo"]), ["Rock", "Remix", "Foo"]);
        assert_eq!(g(&["((foo)"]), ["(foo)"]);
        assert_eq!(g(&["(999)x"]), ["Unknown", "x"]);
        assert_eq!(g(&["(foo) bar", "", "1"]), ["(foo) bar", "Classic Rock"]);
        assert_eq!(g(&["29"]), ["Jazz+Funk"]);
    }

    /// The raw text of the first `TCON` frame of an ID3v2.3 tag.
    fn raw_v23_tcon(p: &Path) -> String {
        let d = std::fs::read(p).unwrap();
        assert_eq!((&d[..3], d[3]), (&b"ID3"[..], 3));
        let size = d[6..10]
            .iter()
            .fold(0usize, |a, &b| (a << 7) | (b & 0x7f) as usize);
        let body = &d[10..10 + size];
        let mut pos = 0;
        while pos + 10 <= body.len() && body[pos] != 0 {
            let n = u32::from_be_bytes(body[pos + 4..pos + 8].try_into().unwrap()) as usize;
            if &body[pos..pos + 4] == b"TCON" {
                let c = &body[pos + 10..pos + 10 + n];
                assert_eq!(c[0], 1, "UTF-16");
                let units: Vec<u16> = c[3..]
                    .chunks(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                return String::from_utf16(&units).unwrap();
            }
            pos += 10 + n;
        }
        panic!("no TCON")
    }

    /// ID3v2.3 genres as beets writes them: genre names joined with
    /// "/", without lofty's "(80)"/"(RX)" reference forms; read back
    /// (in any version) through the genre rules too.
    #[test]
    fn id3v23_genre_conversion() {
        let d = tempfile::tempdir().unwrap();
        let Some(p) = make(d.path(), "t.mp3", &[]) else {
            return;
        };
        for (genres, raw, back) in [
            (&["80"][..], "Folk", &["Folk"][..]),
            (&["Remix"], "Remix", &["Remix"]),
            (&["Rock", "80"], "Rock/Folk", &["Rock/Folk"]),
            (&["(17)Pop"], "Rock/Pop", &["Rock/Pop"]),
            (&["(foo) bar"], "(foo) bar", &["(foo) bar"]),
        ] {
            let mut it = read(&p);
            it.set_list("genres", genres);
            it.set_str("title", "kept");
            crate::media::write_with(&it, &p, true).unwrap();
            assert_eq!(raw_v23_tcon(&p), raw, "{genres:?}");
            let back_item = read(&p);
            assert_eq!(back_item.list("genres"), back, "{genres:?}");
            assert_eq!(back_item.str("title"), "kept");
        }
        // ID3v2.4 keeps the text; reading it maps the number.
        let mut it = read(&p);
        it.set_list("genres", &["80"]);
        crate::media::write_with(&it, &p, false).unwrap();
        assert_eq!(read(&p).list("genres"), ["Folk"]);
    }

    #[test]
    fn sound_check_as_mediafile() {
        assert_eq!(
            sc_encode(-3.5, 0.98),
            " 000008BF 000008BF 000015DD 000015DD 00000000 00000000 00007D70 00007D70 \
             00000000 00000000"
        );
        assert_eq!(sc_decode(&sc_encode(-3.5, 0.98)), (-3.5, 0.97998));
        assert_eq!(
            sc_encode(6.0, 0.25),
            " 000000FB 000000FB 00000274 00000274 00000000 00000000 00002000 00002000 \
             00000000 00000000"
        );
        assert_eq!(sc_decode("junk"), (0.0, 0.0));
    }

    #[test]
    fn safe_cast_and_keys_as_python() {
        assert_eq!(Raw::S(" 12abc".into()).as_int(), 12);
        assert_eq!(Raw::S("-3/12".into()).as_int(), -3);
        assert_eq!(Raw::S("x1".into()).as_int(), 0);
        assert_eq!(Raw::S("-3.50 dB".into()).as_float(), -3.5);
        assert_eq!(Raw::S(".5".into()).as_float(), 0.5);
        assert!(Raw::S(" 1 ".into()).as_bool());
        assert!(!Raw::S("yes".into()).as_bool());
        for (k, want) in [
            ("Ebm", "D#m"),
            ("C major", "C"),
            ("a minor", "Am"),
            ("bb", "A#"),
            ("Db Minor", "Cm"),
            ("f#m", "F#m"),
        ] {
            assert_eq!(normalize_key(k), want, "{k}");
        }
    }
}
