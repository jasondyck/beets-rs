//! Items and albums: a fixed schema of typed fields plus free-form
//! "flexible attributes", mirroring beets' `dbcore` models.

use anyhow::{Result, bail};
use chrono::{Local, NaiveDateTime, TimeZone};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Float: reserved for flexible typed fields
pub enum FieldType {
    Str,
    Int,
    /// Integer formatted with zero padding to the given width (beets' PaddedInt).
    Padded(usize),
    Float,
    Bool,
    /// Seconds, formatted as M:SS.
    Duration,
    /// Bits per second, formatted as "320kbps".
    Bitrate,
    /// Hertz, formatted as "44kHz".
    SampleRate,
    /// Unix timestamp, formatted with `time_format`.
    Date,
    Path,
    /// A list of strings (beets' `DelimitedString`), stored joined with
    /// the given delimiter and formatted joined with "; ".
    List(&'static str),
    /// beets' `NullFloat`: a float that may be null (unset).
    NullFloat,
    /// beets' `MusicalKey`: a nullable string, normalized ("Ebm" is "D#m").
    MusicalKey,
}

impl FieldType {
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            FieldType::Int
                | FieldType::Padded(_)
                | FieldType::Float
                | FieldType::NullFloat
                | FieldType::Duration
                | FieldType::Bitrate
                | FieldType::SampleRate
        )
    }

    pub fn sql_type(self) -> &'static str {
        match self {
            FieldType::Str | FieldType::List(_) | FieldType::MusicalKey => "TEXT",
            FieldType::Path => "BLOB",
            FieldType::Float | FieldType::NullFloat | FieldType::Duration | FieldType::Date => {
                "REAL"
            }
            _ => "INTEGER",
        }
    }

    pub fn default_value(self) -> Value {
        match self {
            FieldType::Str | FieldType::List(_) => Value::Str(String::new()),
            FieldType::NullFloat | FieldType::MusicalKey => Value::Null,
            FieldType::Path => Value::Path(PathBuf::new()),
            FieldType::Bool => Value::Bool(false),
            FieldType::Float | FieldType::Duration | FieldType::Date => Value::Float(0.0),
            _ => Value::Int(0),
        }
    }

    /// Parse user input (from `modify` or `set_fields`) into a value of this
    /// type, as beets' dbcore types do: anything unparsable becomes the
    /// type's null (0, 0.0, false) rather than an error.
    pub fn parse(self, s: &str) -> Value {
        let int = || s.trim().parse::<i64>().unwrap_or(0);
        let float = || s.trim().parse::<f64>().unwrap_or(0.0);
        match self {
            FieldType::Str => Value::Str(s.to_string()),
            FieldType::List(d) => Value::Str(join_list(&split_list(s, d), d)),
            FieldType::NullFloat => s.trim().parse().map_or(Value::Null, Value::Float),
            FieldType::MusicalKey => Value::Str(normalize_key(s)),
            FieldType::Path => Value::Path(PathBuf::from(s)),
            // beets' `str2bool`.
            FieldType::Bool => Value::Bool(matches!(
                s.to_lowercase().as_str(),
                "yes" | "1" | "true" | "t" | "y"
            )),
            FieldType::Float => Value::Float(float()),
            // "M:SS", else a number of seconds.
            FieldType::Duration => Value::Float(match s.split_once(':') {
                Some((m, ss))
                    if !m.is_empty()
                        && m.bytes().all(|c| c.is_ascii_digit())
                        && ss.len() == 2
                        && ss.as_bytes()[0].is_ascii_digit()
                        && ss.as_bytes()[0] <= b'5'
                        && ss.as_bytes()[1].is_ascii_digit() =>
                {
                    (m.parse::<f64>().unwrap_or(0.0)) * 60.0 + ss.parse::<f64>().unwrap_or(0.0)
                }
                _ => float(),
            }),
            // `time_format`, else a timestamp.
            FieldType::Date => Value::Float(
                NaiveDateTime::parse_from_str(s, TIME_FORMAT)
                    .ok()
                    .and_then(|d| Local.from_local_datetime(&d).earliest())
                    .map(|d| d.timestamp() as f64)
                    .unwrap_or_else(float),
            ),
            _ => Value::Int(int()),
        }
    }
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" | "t" => Some(true),
        "0" | "false" | "no" | "n" | "off" | "f" | "" => Some(false),
        _ => None,
    }
}

/// beets' `human_seconds_short`: whole seconds as M:SS (minutes unbounded).
pub fn format_duration(secs: f64) -> String {
    let secs = secs as i64;
    format!("{}:{:02}", secs.div_euclid(60), secs.rem_euclid(60))
}

pub const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

#[cfg(test)]
pub fn parse_datetime(s: &str) -> Option<f64> {
    if s.is_empty() {
        return Some(0.0);
    }
    let naive = NaiveDateTime::parse_from_str(s, TIME_FORMAT)
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        })?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|d| d.timestamp() as f64)
}

pub fn format_datetime(ts: f64) -> String {
    match Local.timestamp_opt(ts as i64, 0).earliest() {
        Some(d) => d.format(TIME_FORMAT).to_string(),
        None => String::new(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// SQL NULL / Python None (nullable types only).
    Null,
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Path(PathBuf),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Bool(b) => Some(*b as i64 as f64),
            Value::Str(s) => s.trim().parse().ok(),
            Value::Path(_) | Value::Null => None,
        }
    }

    /// Plain string form with no type-specific formatting.
    pub fn raw(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Str(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Path(p) => p.to_string_lossy().into_owned(),
        }
    }

    /// Human-readable form, as used in templates and listings.
    pub fn format(&self, ty: Option<FieldType>) -> String {
        let num = self.as_f64().unwrap_or(0.0);
        match ty {
            Some(FieldType::Padded(w)) => format!("{:0w$}", num as i64, w = w),
            Some(FieldType::Float | FieldType::NullFloat) => format!("{num:.1}"),
            Some(FieldType::List(d)) => split_list(&self.raw(), d).join(SEMI),
            Some(FieldType::Bool) => if num != 0.0 { "True" } else { "False" }.into(),
            Some(FieldType::Duration) => format_duration(num),
            // beets' `ScaledInt`: integer division and a suffix.
            Some(FieldType::Bitrate) => format!("{}kbps", (num as i64).div_euclid(1000)),
            Some(FieldType::SampleRate) => format!("{}kHz", (num as i64).div_euclid(1000)),
            Some(FieldType::Date) => format_datetime(num),
            _ => self.raw(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Item,
    Album,
}

use FieldType::*;

/// beets' `MULTI_VALUE_DELIMITER`: how multi-valued fields are joined in
/// the database.
pub const MULTI: &str = "\\\u{2400}";
/// beets' `SEMICOLON_SPACE_DSV` delimiter (`albumtypes`), also the one
/// lists are formatted and parsed with.
pub const SEMI: &str = "; ";
const MV: FieldType = List(MULTI);

pub const ITEM_FIELDS: &[(&str, FieldType)] = &[
    ("path", Path),
    ("album_id", Int),
    ("title", Str),
    ("artist", Str),
    ("artists", MV),
    ("artist_sort", Str),
    ("artists_sort", MV),
    ("artist_credit", Str),
    ("artists_credit", MV),
    ("albumartist", Str),
    ("albumartists", MV),
    ("albumartist_sort", Str),
    ("albumartists_sort", MV),
    ("albumartist_credit", Str),
    ("albumartists_credit", MV),
    ("album", Str),
    ("genres", MV),
    ("style", Str),
    ("composers", MV),
    ("composer_sort", Str),
    ("lyricists", MV),
    ("arrangers", MV),
    ("remixers", MV),
    ("grouping", Str),
    ("subtitle", Str),
    ("year", Padded(4)),
    ("month", Padded(2)),
    ("day", Padded(2)),
    ("original_year", Padded(4)),
    ("original_month", Padded(2)),
    ("original_day", Padded(2)),
    ("track", Padded(2)),
    ("tracktotal", Padded(2)),
    ("disc", Padded(2)),
    ("disctotal", Padded(2)),
    ("disctitle", Str),
    ("comments", Str),
    ("lyrics", Str),
    ("bpm", Int),
    ("comp", Bool),
    ("initial_key", MusicalKey),
    ("isrc", Str),
    ("encoder", Str),
    ("mb_trackid", Str),
    ("mb_releasetrackid", Str),
    ("mb_albumid", Str),
    ("mb_artistid", Str),
    ("mb_artistids", MV),
    ("mb_albumartistid", Str),
    ("mb_albumartistids", MV),
    ("mb_releasegroupid", Str),
    ("mb_workid", Str),
    ("release_group_title", Str),
    ("work", Str),
    ("work_disambig", Str),
    ("trackdisambig", Str),
    ("albumtype", Str),
    ("albumtypes", List(SEMI)),
    ("label", Str),
    ("barcode", Str),
    ("catalognum", Str),
    ("asin", Str),
    ("script", Str),
    ("language", Str),
    ("country", Str),
    ("albumstatus", Str),
    ("media", Str),
    ("albumdisambig", Str),
    ("releasegroupdisambig", Str),
    ("acoustid_fingerprint", Str),
    ("acoustid_id", Str),
    ("rg_track_gain", NullFloat),
    ("rg_track_peak", NullFloat),
    ("rg_album_gain", NullFloat),
    ("rg_album_peak", NullFloat),
    ("r128_track_gain", NullFloat),
    ("r128_album_gain", NullFloat),
    ("artists_ids", MV),
    ("composers_ids", MV),
    ("lyricists_ids", MV),
    ("arrangers_ids", MV),
    ("remixers_ids", MV),
    ("discogs_albumid", Int),
    ("discogs_artistid", Int),
    ("discogs_labelid", Int),
    ("data_source", Str),
    ("length", Duration),
    ("bitrate", Bitrate),
    ("bitrate_mode", Str),
    ("samplerate", SampleRate),
    ("bitdepth", Int),
    ("channels", Int),
    ("format", Str),
    ("encoder_info", Str),
    ("encoder_settings", Str),
    ("mtime", Date),
    ("added", Date),
];

pub const ALBUM_FIELDS: &[(&str, FieldType)] = &[
    ("artpath", Path),
    ("added", Date),
    ("albumartist", Str),
    ("albumartists", MV),
    ("albumartist_sort", Str),
    ("albumartists_sort", MV),
    ("albumartist_credit", Str),
    ("albumartists_credit", MV),
    ("album", Str),
    ("genres", MV),
    ("style", Str),
    ("year", Padded(4)),
    ("month", Padded(2)),
    ("day", Padded(2)),
    ("original_year", Padded(4)),
    ("original_month", Padded(2)),
    ("original_day", Padded(2)),
    ("disctotal", Padded(2)),
    ("comp", Bool),
    ("mb_albumid", Str),
    ("mb_albumartistid", Str),
    ("mb_albumartistids", MV),
    ("mb_releasegroupid", Str),
    ("release_group_title", Str),
    ("albumtype", Str),
    ("albumtypes", List(SEMI)),
    ("label", Str),
    ("barcode", Str),
    ("catalognum", Str),
    ("asin", Str),
    ("script", Str),
    ("language", Str),
    ("country", Str),
    ("albumstatus", Str),
    ("albumdisambig", Str),
    ("releasegroupdisambig", Str),
    ("rg_album_gain", NullFloat),
    ("rg_album_peak", NullFloat),
    ("r128_album_gain", NullFloat),
    ("discogs_albumid", Int),
    ("discogs_artistid", Int),
    ("discogs_labelid", Int),
    ("data_source", Str),
];

/// beets' `ITEM_LEGACY_TO_LIST_FIELD` / `ALBUM_LEGACY_TO_LIST_FIELD`:
/// single-valued field names beets 2.x replaced with lists.
pub fn legacy_list_field(kind: Kind, field: &str) -> Option<&'static str> {
    match (kind, field) {
        (_, "genre") => Some("genres"),
        (Kind::Item, "arranger") => Some("arrangers"),
        (Kind::Item, "composer") => Some("composers"),
        (Kind::Item, "lyricist") => Some("lyricists"),
        (Kind::Item, "remixer") => Some("remixers"),
        _ => None,
    }
}

/// beets' `maybe_replace_legacy_field`: the list field for a legacy name,
/// with beets' deprecation warning.
pub fn replace_legacy_field(kind: Kind, field: &str, modify: bool) -> String {
    match legacy_list_field(kind, field) {
        Some(new) => {
            let sep = if modify {
                " (separate values by '; ')"
            } else {
                ""
            };
            eprintln!(
                "The '{field}' field is deprecated and will be removed in version 3.0.0. \
                 Use '{new}'{sep} instead."
            );
            new.to_string()
        }
        None => field.to_string(),
    }
}

/// beets' `DelimitedString.parse`: split on the database delimiter if
/// present, else on "; ".
pub fn split_list(s: &str, delim: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![];
    }
    let d = if s.contains(delim) { delim } else { SEMI };
    s.split(d).map(str::to_string).collect()
}

/// beets' `DelimitedString.normalize` for a list, then `to_sql`: values
/// holding "; " are split, and all are joined with the database delimiter.
pub fn join_list<S: AsRef<str>>(values: &[S], delim: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for v in values {
        out.extend(v.as_ref().split(SEMI));
    }
    out.join(delim)
}

/// beets' `MusicalKey.parse`: flats as sharps, "minor" as "m", no
/// "major", capitalized.
pub fn normalize_key(key: &str) -> String {
    let mut k = key.to_lowercase();
    for (flat, sharp) in [
        ("db", "c#"),
        ("eb", "d#"),
        ("gb", "f#"),
        ("ab", "g#"),
        ("bb", "a#"),
    ] {
        k = k.replace(flat, sharp);
    }
    // `[\W\s]+minor` / `[\W\s]+major`.
    static MINOR: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[\W\s]+minor").unwrap());
    static MAJOR: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[\W\s]+major").unwrap());
    let k = MINOR.replace_all(&k, "m");
    let k = MAJOR.replace_all(&k, "");
    // Python's `str.capitalize`.
    let mut c = k.chars();
    match c.next() {
        Some(f) => f
            .to_uppercase()
            .chain(c.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// Album fields that are mirrored onto every item of the album.
pub fn album_item_shared_fields() -> impl Iterator<Item = &'static str> {
    ALBUM_FIELDS
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| *n != "added" && *n != "artpath")
}

pub fn schema(kind: Kind) -> &'static [(&'static str, FieldType)] {
    match kind {
        Kind::Item => ITEM_FIELDS,
        Kind::Album => ALBUM_FIELDS,
    }
}

pub fn field_type(kind: Kind, name: &str) -> Option<FieldType> {
    if name == "id" {
        return Some(FieldType::Int);
    }
    schema(kind)
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

/// One row of the `items` or `albums` table.
#[derive(Clone, Debug)]
pub struct Record {
    pub kind: Kind,
    pub id: Option<i64>,
    values: BTreeMap<&'static str, Value>,
    pub flex: BTreeMap<String, String>,
}

pub type Item = Record;
pub type Album = Record;

impl Record {
    pub fn new(kind: Kind) -> Self {
        let values = schema(kind)
            .iter()
            .map(|(n, t)| (*n, t.default_value()))
            .collect();
        Record {
            kind,
            id: None,
            values,
            flex: BTreeMap::new(),
        }
    }

    /// A record from values already of the schema's types, in schema order
    /// (as the library reads them).
    pub fn from_values(kind: Kind, id: i64, values: Vec<Value>) -> Self {
        Record {
            kind,
            id: Some(id),
            values: schema(kind).iter().map(|(n, _)| *n).zip(values).collect(),
            flex: BTreeMap::new(),
        }
    }

    pub fn new_item() -> Self {
        Self::new(Kind::Item)
    }

    pub fn new_album() -> Self {
        Self::new(Kind::Album)
    }

    pub fn field_type(&self, name: &str) -> Option<FieldType> {
        field_type(self.kind, name)
    }

    pub fn has_field(&self, name: &str) -> bool {
        name == "id" || self.values.contains_key(name) || self.flex.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if name == "id" {
            return Some(Value::Int(self.id.unwrap_or(0)));
        }
        if let Some(v) = self.values.get(name) {
            return Some(v.clone());
        }
        // Items expose `singleton` like beets does.
        if self.kind == Kind::Item && name == "singleton" {
            return Some(Value::Bool(self.int("album_id") == 0));
        }
        self.flex.get(name).map(|s| Value::Str(s.clone()))
    }

    /// Formatted value for display/templates.
    pub fn formatted(&self, name: &str) -> Option<String> {
        let ty = if name == "singleton" {
            Some(FieldType::Bool)
        } else {
            self.field_type(name)
        };
        // A singleton's `album_id` is null (beets' `FOREIGN_ID`).
        if self.kind == Kind::Item && name == "album_id" && self.int("album_id") == 0 {
            return Some(String::new());
        }
        self.get(name).map(|v| v.format(ty))
    }

    pub fn set(&mut self, name: &str, value: Value) {
        if let Some((key, ty)) = schema(self.kind).iter().find(|(n, _)| *n == name) {
            let coerced = coerce(value, *ty);
            let old = self.values.insert(key, coerced);
            // beets' `Item.__setitem__`: changing a tag field marks the
            // file as out of date ("Reset mtime on dirty").
            if self.kind == Kind::Item
                && crate::media::TAG_FIELDS.contains(key)
                && old.as_ref() != self.values.get(key)
            {
                self.values.insert("mtime", Value::Float(0.0));
            }
        } else if name != "id" {
            self.flex.insert(name.to_string(), value.raw());
        }
    }

    pub fn set_str(&mut self, name: &str, s: impl Into<String>) {
        self.set(name, Value::Str(s.into()));
    }

    pub fn set_int(&mut self, name: &str, i: i64) {
        self.set(name, Value::Int(i));
    }

    /// Set a field from user-supplied text, parsing according to its type.
    pub fn set_parsed(&mut self, name: &str, s: &str) -> Result<()> {
        if name == "id" || name == "path" && self.kind == Kind::Item && s.is_empty() {
            bail!("field {name} cannot be set this way");
        }
        match self.field_type(name) {
            Some(ty) => self.set(name, ty.parse(s)),
            None => {
                self.flex.insert(name.to_string(), s.to_string());
            }
        }
        Ok(())
    }

    /// Remove a flexible attribute. Fixed fields can't be deleted.
    pub fn remove_flex(&mut self, name: &str) -> bool {
        self.flex.remove(name).is_some()
    }

    pub fn str(&self, name: &str) -> &str {
        match self.values.get(name) {
            Some(Value::Str(s)) => s,
            _ => self.flex.get(name).map(|s| s.as_str()).unwrap_or(""),
        }
    }

    /// A list field's values (beets' `DelimitedString`); for any other
    /// field, its string value, if not empty.
    pub fn list(&self, name: &str) -> Vec<String> {
        match self.field_type(name) {
            Some(FieldType::List(d)) => split_list(self.str(name), d),
            _ => {
                let s = self.str(name);
                if s.is_empty() {
                    vec![]
                } else {
                    vec![s.to_string()]
                }
            }
        }
    }

    /// Set a list field (values containing "; " are split, as beets does).
    pub fn set_list<S: AsRef<str>>(&mut self, name: &str, values: &[S]) {
        let d = match self.field_type(name) {
            Some(FieldType::List(d)) => d,
            _ => SEMI,
        };
        self.set(name, Value::Str(join_list(values, d)));
    }

    pub fn int(&self, name: &str) -> i64 {
        self.values
            .get(name)
            .and_then(|v| v.as_f64())
            .map(|f| f as i64)
            .unwrap_or(0)
    }

    pub fn float(&self, name: &str) -> f64 {
        self.values
            .get(name)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }

    pub fn bool(&self, name: &str) -> bool {
        matches!(self.values.get(name), Some(Value::Bool(true)))
    }

    pub fn path(&self) -> &Path {
        match self.values.get("path") {
            Some(Value::Path(p)) => p,
            _ => Path::new(""),
        }
    }

    /// A path-typed field other than `path` (e.g. an album's `artpath`).
    pub fn path_field(&self, name: &str) -> &Path {
        match self.values.get(name) {
            Some(Value::Path(p)) => p,
            _ => Path::new(""),
        }
    }

    pub fn set_path(&mut self, p: impl Into<PathBuf>) {
        self.values.insert("path", Value::Path(p.into()));
    }

    /// Fixed fields in schema order, for persistence.
    pub fn fixed_values(&self) -> impl Iterator<Item = (&'static str, &Value)> {
        schema(self.kind)
            .iter()
            .map(move |(n, _)| (*n, &self.values[n]))
    }

    /// All field names: fixed, then flexible.
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = std::iter::once("id".to_string())
            .chain(schema(self.kind).iter().map(|(n, _)| n.to_string()))
            .collect();
        keys.extend(self.flex.keys().cloned());
        keys
    }
}

fn coerce(value: Value, ty: FieldType) -> Value {
    match (ty, value) {
        (FieldType::Str, v) => Value::Str(v.raw()),
        (FieldType::List(d), v) => Value::Str(join_list(&split_list(&v.raw(), d), d)),
        (FieldType::NullFloat | FieldType::MusicalKey, Value::Null) => Value::Null,
        (FieldType::NullFloat, Value::Str(s)) => s.trim().parse().map_or(Value::Null, Value::Float),
        (FieldType::NullFloat, v) => v.as_f64().map_or(Value::Null, Value::Float),
        (FieldType::MusicalKey, v) => Value::Str(normalize_key(&v.raw())),
        (FieldType::Path, Value::Path(p)) => Value::Path(p),
        (FieldType::Path, v) => Value::Path(PathBuf::from(v.raw())),
        (FieldType::Bool, Value::Bool(b)) => Value::Bool(b),
        (FieldType::Bool, Value::Str(s)) => Value::Bool(parse_bool(&s).unwrap_or(false)),
        (FieldType::Bool, v) => Value::Bool(v.as_f64().unwrap_or(0.0) != 0.0),
        (FieldType::Float | FieldType::Duration | FieldType::Date, v) => {
            Value::Float(v.as_f64().unwrap_or(0.0))
        }
        (_, v) => Value::Int(v.as_f64().unwrap_or(0.0) as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // beets' `Item.__setitem__`: changing a tag field resets `mtime`.
    #[test]
    fn changing_tag_fields_resets_mtime() {
        let mut it = Record::new_item();
        it.set_str("title", "a");
        it.set("mtime", Value::Float(5.0));
        it.set_str("title", "a");
        assert_eq!(it.float("mtime"), 5.0, "same value: not dirty");
        it.flex.insert("mood".into(), "x".into());
        it.set_str("mood", "y");
        it.set_int("bitrate", 1);
        it.set_path("/x.mp3");
        assert_eq!(it.float("mtime"), 5.0, "flex, audio properties, path");
        it.set_str("title", "b");
        assert_eq!(it.float("mtime"), 0.0);
        let mut al = Record::new_album();
        al.set("artpath", Value::Path("/a.jpg".into()));
        al.set_str("album", "x");
        assert!(al.get("mtime").is_none());
    }

    #[test]
    fn formatting() {
        let mut it = Record::new_item();
        it.set_int("track", 3);
        it.set_int("year", 1999);
        it.set("length", Value::Float(185.4));
        it.set_int("bitrate", 320_000);
        it.set_int("samplerate", 44100);
        assert_eq!(it.formatted("track").unwrap(), "03");
        assert_eq!(it.formatted("year").unwrap(), "1999");
        assert_eq!(it.formatted("length").unwrap(), "3:05");
        assert_eq!(it.formatted("bitrate").unwrap(), "320kbps");
        assert_eq!(it.formatted("samplerate").unwrap(), "44kHz");
        assert_eq!(it.formatted("comp").unwrap(), "False");
        assert_eq!(it.formatted("singleton").unwrap(), "True");
    }

    #[test]
    fn flex_and_parse() {
        let mut it = Record::new_item();
        it.set_parsed("mood", "happy").unwrap();
        it.set_parsed("year", "2001").unwrap();
        it.set_parsed("length", "4:10").unwrap();
        assert_eq!(it.str("mood"), "happy");
        assert_eq!(it.int("year"), 2001);
        assert_eq!(it.float("length"), 250.0);
        // As beets: unparsable values become the type's null, not errors.
        let parsed = |name: &str, s: &str| {
            let mut it = Record::new_item();
            it.set_parsed(name, s).unwrap();
            it.get(name).unwrap()
        };
        assert_eq!(parsed("year", "abc"), Value::Int(0));
        assert_eq!(parsed("year", "2003.7"), Value::Int(0));
        assert_eq!(parsed("year", " 1999 "), Value::Int(1999));
        assert_eq!(parsed("bpm", "-5"), Value::Int(-5));
        assert_eq!(parsed("comp", "yes"), Value::Bool(true));
        assert_eq!(parsed("comp", "on"), Value::Bool(false));
        assert_eq!(parsed("length", "3:7"), Value::Float(0.0));
        assert_eq!(parsed("length", "1:07"), Value::Float(67.0));
        assert_eq!(parsed("length", "1:60"), Value::Float(0.0));
        assert_eq!(parsed("length", "12.5"), Value::Float(12.5));
        assert_eq!(parsed("added", "1700000000"), Value::Float(1.7e9));
        assert_eq!(parsed("added", "2024-03-15"), Value::Float(0.0));
        assert_eq!(
            parsed("added", "2024-03-15 12:00:00"),
            Value::Float(parse_datetime("2024-03-15 12:00:00").unwrap())
        );
        assert_eq!(parsed("title", " x "), Value::Str(" x ".into()));
        assert!(it.set_parsed("id", "3").is_err());
    }
}
