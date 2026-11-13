//! Port of beets 2.14's `autotag/hooks.py`: a candidate's metadata as the
//! plain mapping beets' `AlbumInfo`/`TrackInfo` are (`dict(info)`), and
//! what applying it does: `raw_data` (artist credits, `correct_list_fields`,
//! track numbering), `item_data` (null values dropped unless
//! `overwrite_null`, info names mapped to item fields), `merge_with_album`
//! and `item.update(data)`.

use crate::config::Config;
use crate::mb::{AlbumInfo, TrackInfo};
use crate::model::{FieldType, Record, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A Python value as beets' info dicts hold them.
#[derive(Debug, Clone, PartialEq)]
pub enum V {
    None,
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<String>),
}

impl V {
    pub fn opt_str(s: Option<String>) -> V {
        s.map_or(V::None, V::Str)
    }

    pub fn opt_list(l: Option<Vec<String>>) -> V {
        l.map_or(V::None, V::List)
    }

    /// An int that's None when 0 (how our structs keep a missing number).
    pub fn int_or_none(i: i64) -> V {
        if i == 0 { V::None } else { V::Int(i) }
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            V::None => false,
            V::Str(s) => !s.is_empty(),
            V::Int(i) => *i != 0,
            V::Float(f) => *f != 0.0,
            V::Bool(b) => *b,
            V::List(l) => !l.is_empty(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_list(&self) -> Option<&[String]> {
        match self {
            V::List(l) => Some(l),
            _ => None,
        }
    }

    /// Python's `str()` of the value.
    pub fn py_str(&self) -> String {
        match self {
            V::None => "None".into(),
            V::Str(s) => s.clone(),
            V::Int(i) => i.to_string(),
            V::Float(f) => py_float(*f),
            V::Bool(b) => if *b { "True" } else { "False" }.into(),
            V::List(l) => format!(
                "[{}]",
                l.iter().map(|s| py_repr(s)).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// The value as our model stores it in a field of type `ty`.
    fn to_value(&self, ty: Option<FieldType>) -> Value {
        match self {
            V::None => Value::Null,
            V::Str(s) => Value::Str(s.clone()),
            V::Int(i) => Value::Int(*i),
            V::Float(f) => Value::Float(*f),
            V::Bool(b) => Value::Bool(*b),
            V::List(l) => {
                let d = match ty {
                    Some(FieldType::List(d)) => d,
                    _ => crate::model::SEMI,
                };
                Value::Str(crate::model::join_list(l, d))
            }
        }
    }
}

/// Python's `str()` of a float: always with a decimal point.
pub fn py_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

/// Python's `repr()` of a string.
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                write!(out, "\\x{:02x}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// An info as a mapping (`dict(info)`).
pub type Data = BTreeMap<String, V>;

/// The settings `raw_data`/`item_data` read.
#[derive(Debug, Clone, Default)]
pub struct ApplyOpts {
    pub artist_credit: bool,
    pub per_disc_numbering: bool,
    pub original_date: bool,
    /// `overwrite_null.album` / `.track`: info fields applied even when
    /// None or empty.
    pub null_album: Vec<String>,
    pub null_track: Vec<String>,
}

impl ApplyOpts {
    pub fn from_config(cfg: &Config) -> Self {
        ApplyOpts {
            artist_credit: cfg.bool("artist_credit"),
            per_disc_numbering: cfg.bool("per_disc_numbering"),
            original_date: cfg.bool("original_date"),
            null_album: cfg.list("overwrite_null.album"),
            null_track: cfg.list("overwrite_null.track"),
        }
    }
}

/// beets' `SYNCHRONISED_LIST_FIELDS`.
const SYNCHRONISED_LIST_FIELDS: [(&str, &str); 5] = [
    ("albumtype", "albumtypes"),
    ("artist", "artists"),
    ("artist_id", "artists_ids"),
    ("artist_sort", "artists_sort"),
    ("artist_credit", "artists_credit"),
];

/// beets' `correct_list_fields`: each single value heads its list, unless
/// the list already represents it (a joined credit sharing words with the
/// list's names, or containing them all); an empty single value takes the
/// list's first.
pub fn correct_list_fields(data: &mut Data) {
    for (single_field, list_field) in SYNCHRONISED_LIST_FIELDS {
        let single = data
            .get(single_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let list: Vec<String> = data
            .get(list_field)
            .and_then(|v| v.as_list())
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        let single_lower = single.to_lowercase();
        let list_lower: Vec<String> = list.iter().map(|v| v.to_lowercase()).collect();
        if !list.contains(&single)
            && (single_lower
                .split_whitespace()
                .any(|w| list_lower.iter().any(|v| v == w))
                || (list.len() > 1 && list_lower.iter().all(|v| single_lower.contains(v.as_str()))))
        {
            continue;
        }
        if !single.is_empty() {
            let mut new = vec![single];
            for v in list {
                if !new.contains(&v) {
                    new.push(v);
                }
            }
            data.insert(list_field.into(), V::List(new));
        } else if let Some(first) = list.into_iter().next() {
            data.insert(single_field.into(), V::Str(first));
        }
    }
}

/// beets' `Info.raw_data`: with `artist_credit`, the credited names
/// replace the artist names; then the list fields are synchronised.
fn info_raw_data(info: &Data, o: &ApplyOpts) -> Data {
    let mut data = info.clone();
    if o.artist_credit {
        let pick = |a: &str, b: &str| {
            let v = info.get(a).filter(|v| v.truthy()).or_else(|| info.get(b));
            v.cloned().unwrap_or(V::None)
        };
        let artist = pick("artist_credit", "artist");
        let artists = pick("artists_credit", "artists");
        data.insert("artist".into(), artist);
        data.insert("artists".into(), artists);
    }
    correct_list_fields(&mut data);
    data
}

/// beets' `Info.item_data`: None and empty lists dropped (unless the field
/// is in `overwrite_null`), ignored fields dropped, info names mapped to
/// the item's.
fn info_item_data(raw: Data, ignored: &[&str], map: &[(&str, &str)], nullable: &[String]) -> Data {
    let mut data: Data = raw
        .into_iter()
        .filter(|(k, v)| {
            !ignored.contains(&k.as_str())
                && (!matches!(v, V::None) && !matches!(v, V::List(l) if l.is_empty())
                    || nullable.contains(k))
        })
        .collect();
    for (info_field, media_field) in map {
        if let Some(v) = data.remove(*info_field) {
            data.insert(media_field.to_string(), v);
        }
    }
    data
}

const ALBUM_IGNORED: [&str; 2] = ["data_url", "tracks"];
const ALBUM_FIELD_MAP: [(&str, &str); 12] = [
    ("album_id", "mb_albumid"),
    ("artist", "albumartist"),
    ("artists", "albumartists"),
    ("artist_id", "mb_albumartistid"),
    ("artists_ids", "mb_albumartistids"),
    ("artist_credit", "albumartist_credit"),
    ("artists_credit", "albumartists_credit"),
    ("artist_sort", "albumartist_sort"),
    ("artists_sort", "albumartists_sort"),
    ("mediums", "disctotal"),
    ("releasegroup_id", "mb_releasegroupid"),
    ("va", "comp"),
];
const TRACK_IGNORED: [&str; 3] = ["data_url", "index", "medium_total"];
const TRACK_FIELD_MAP: [(&str, &str); 6] = [
    ("artist_id", "mb_artistid"),
    ("artists_ids", "mb_artistids"),
    ("medium", "disc"),
    ("release_track_id", "mb_releasetrackid"),
    ("track_id", "mb_trackid"),
    ("medium_index", "track"),
];

/// beets' `AlbumInfo.raw_data`: with a year, a missing month and day are 0.
pub fn album_raw_data(a: &Data, o: &ApplyOpts) -> Data {
    let mut data = info_raw_data(a, o);
    if data.get("year").is_some_and(V::truthy) {
        for k in ["month", "day"] {
            let v = a
                .get(k)
                .filter(|v| v.truthy())
                .cloned()
                .unwrap_or(V::Int(0));
            data.insert(k.into(), v);
        }
    }
    data
}

/// beets' `AlbumInfo.item_data`, with `original_date` replacing the date.
pub fn album_item_data(a: &Data, o: &ApplyOpts) -> Data {
    let mut data = info_item_data(
        album_raw_data(a, o),
        &ALBUM_IGNORED,
        &ALBUM_FIELD_MAP,
        &o.null_album,
    );
    if o.original_date
        && let Some(oy) = data.get("original_year").filter(|v| v.truthy()).cloned()
    {
        data.insert("year".into(), oy);
        for (k, ok) in [("month", "original_month"), ("day", "original_day")] {
            let v = data
                .get(ok)
                .filter(|v| v.truthy())
                .cloned()
                .unwrap_or(V::Int(0));
            data.insert(k.into(), v);
        }
    }
    data
}

/// beets' `TrackInfo.raw_data`: the release track id (or the recording's),
/// and the track number per disc or per release.
pub fn track_raw_data(t: &Data, o: &ApplyOpts) -> Data {
    let get = |k: &str| t.get(k).cloned().unwrap_or(V::None);
    let mut data = info_raw_data(t, o);
    let rtid = get("release_track_id");
    data.insert(
        "mb_releasetrackid".into(),
        if rtid.truthy() { rtid } else { get("track_id") },
    );
    data.insert("track".into(), get("index"));
    let medium_index = match (o.per_disc_numbering, get("medium_index")) {
        (true, V::None) | (false, _) => get("index"),
        (true, mi) => mi,
    };
    data.insert("medium_index".into(), medium_index);
    if o.per_disc_numbering && get("medium_total") != V::None {
        data.insert("tracktotal".into(), get("medium_total"));
    }
    data
}

pub fn track_item_data(t: &Data, o: &ApplyOpts) -> Data {
    info_item_data(
        track_raw_data(t, o),
        &TRACK_IGNORED,
        &TRACK_FIELD_MAP,
        &o.null_track,
    )
}

/// beets' `TrackInfo.merge_with_album`: the album's values fill the
/// track's empty ones (but not its artist credit), and the item gets the
/// album's item data, the release's track count and the track's.
pub fn merge_with_album(track: &TrackInfo, album: &AlbumInfo, o: &ApplyOpts) -> Data {
    let album_data = album.to_data();
    let album_raw = album_raw_data(&album_data, o);
    let t = track.to_data();
    let raw_track = track_raw_data(&t, o);
    let mut merged = t.clone();
    for (k, v) in &raw_track {
        if k == "artist_credit" || v.truthy() {
            continue;
        }
        if let Some(av) = album_raw.get(k).filter(|v| v.truthy()) {
            merged.insert(k.clone(), av.clone());
        }
    }
    let mut out = album_item_data(&album_data, o);
    out.insert("tracktotal".into(), V::Int(album.tracks.len() as i64));
    out.extend(track_item_data(&merged, o));
    out
}

/// beets' `Model.update(data)`: each value set on the record, a fixed
/// field typed as such (None as its null), anything else a flexible
/// attribute. Returns the fields whose value changed (beets' dirty
/// fields).
pub fn update(rec: &mut Record, data: &Data) -> Vec<String> {
    let mut changed = vec![];
    for (k, v) in data {
        if k == "id" {
            continue;
        }
        let ty = rec.field_type(k);
        let value = match (v, ty) {
            (V::None, Some(ty)) => ty.default_value(),
            _ => v.to_value(ty),
        };
        let old = rec.get(k);
        rec.set(k, value);
        if rec.get(k) != old {
            changed.push(k.clone());
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(v: &[&str]) -> V {
        V::List(v.iter().map(ToString::to_string).collect())
    }

    fn data(pairs: &[(&str, V)]) -> Data {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// Cases from beets' `test_autotag.py` (`test_correct_list_fields`).
    #[test]
    fn correct_list_fields_as_beets() {
        let check = |single: V, list_v: V, want_single: V, want_list: V| {
            let mut d = data(&[("artist", single), ("artists", list_v)]);
            correct_list_fields(&mut d);
            assert_eq!(
                (d["artist"].clone(), d["artists"].clone()),
                (want_single, want_list)
            );
        };
        let s = |x: &str| V::Str(x.into());
        check(s(""), list(&[]), s(""), list(&[]));
        check(s("1"), list(&[]), s("1"), list(&["1"]));
        check(s(""), list(&["1"]), s("1"), list(&["1"]));
        check(s("1"), list(&["1"]), s("1"), list(&["1"]));
        check(s("1"), list(&["2", "1"]), s("1"), list(&["1", "2"]));
        check(s("1"), list(&["2"]), s("1"), list(&["1", "2"]));
        // Joined credits are left alone.
        check(
            s("A feat. B"),
            list(&["A", "B"]),
            s("A feat. B"),
            list(&["A", "B"]),
        );
        check(
            s("Aaa & Bbb"),
            list(&["aaa", "bbb"]),
            s("Aaa & Bbb"),
            list(&["aaa", "bbb"]),
        );
        check(s("x"), V::None, s("x"), list(&["x"]));
        let mut d = data(&[
            ("albumtype", s("album")),
            ("albumtypes", list(&["compilation"])),
        ]);
        correct_list_fields(&mut d);
        assert_eq!(d["albumtypes"], list(&["album", "compilation"]));
    }

    #[test]
    fn py_str_of_values() {
        assert_eq!(list(&["a", "it's"]).py_str(), "['a', \"it's\"]");
        assert_eq!(V::Float(3.0).py_str(), "3.0");
        assert_eq!(V::Bool(true).py_str(), "True");
        assert_eq!(V::None.py_str(), "None");
    }
}
