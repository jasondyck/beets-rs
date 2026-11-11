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
    /// Hertz, formatted as "44.1kHz".
    SampleRate,
    /// Unix timestamp, formatted with `time_format`.
    Date,
    Path,
}

impl FieldType {
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            FieldType::Int
                | FieldType::Padded(_)
                | FieldType::Float
                | FieldType::Duration
                | FieldType::Bitrate
                | FieldType::SampleRate
        )
    }

    pub fn sql_type(self) -> &'static str {
        match self {
            FieldType::Str => "TEXT",
            FieldType::Path => "BLOB",
            FieldType::Float | FieldType::Duration | FieldType::Date => "REAL",
            _ => "INTEGER",
        }
    }

    pub fn default_value(self) -> Value {
        match self {
            FieldType::Str => Value::Str(String::new()),
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

pub fn format_duration(secs: f64) -> String {
    let secs = secs.max(0.0).round() as i64;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
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
            Value::Path(_) => None,
        }
    }

    /// Plain string form with no type-specific formatting.
    pub fn raw(&self) -> String {
        match self {
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
            Some(FieldType::Float) => format!("{num:.1}"),
            Some(FieldType::Bool) => if num != 0.0 { "True" } else { "False" }.into(),
            Some(FieldType::Duration) => format_duration(num),
            Some(FieldType::Bitrate) => format!("{}kbps", (num / 1000.0).round() as i64),
            Some(FieldType::SampleRate) => {
                let khz = num / 1000.0;
                if khz.fract() == 0.0 {
                    format!("{khz}kHz")
                } else {
                    format!("{khz:.1}kHz")
                }
            }
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

pub const ITEM_FIELDS: &[(&str, FieldType)] = &[
    ("path", Path),
    ("album_id", Int),
    ("title", Str),
    ("artist", Str),
    ("artist_sort", Str),
    ("artist_credit", Str),
    ("albumartist", Str),
    ("albumartist_sort", Str),
    ("albumartist_credit", Str),
    ("album", Str),
    ("genre", Str),
    ("composer", Str),
    ("grouping", Str),
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
    ("isrc", Str),
    ("mb_trackid", Str),
    ("mb_releasetrackid", Str),
    ("mb_albumid", Str),
    ("mb_artistid", Str),
    ("mb_albumartistid", Str),
    ("mb_releasegroupid", Str),
    ("albumtype", Str),
    ("label", Str),
    ("catalognum", Str),
    ("country", Str),
    ("albumstatus", Str),
    ("media", Str),
    ("albumdisambig", Str),
    ("releasegroupdisambig", Str),
    ("data_source", Str),
    ("length", Duration),
    ("bitrate", Bitrate),
    ("samplerate", SampleRate),
    ("bitdepth", Int),
    ("channels", Int),
    ("format", Str),
    ("mtime", Date),
    ("added", Date),
];

pub const ALBUM_FIELDS: &[(&str, FieldType)] = &[
    ("artpath", Path),
    ("added", Date),
    ("albumartist", Str),
    ("albumartist_sort", Str),
    ("albumartist_credit", Str),
    ("album", Str),
    ("genre", Str),
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
    ("mb_releasegroupid", Str),
    ("albumtype", Str),
    ("label", Str),
    ("catalognum", Str),
    ("country", Str),
    ("albumstatus", Str),
    ("albumdisambig", Str),
    ("releasegroupdisambig", Str),
    ("data_source", Str),
];

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
        self.get(name).map(|v| v.format(ty))
    }

    pub fn set(&mut self, name: &str, value: Value) {
        if let Some((key, ty)) = schema(self.kind).iter().find(|(n, _)| *n == name) {
            let coerced = coerce(value, *ty);
            self.values.insert(key, coerced);
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
        assert_eq!(it.formatted("samplerate").unwrap(), "44.1kHz");
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
