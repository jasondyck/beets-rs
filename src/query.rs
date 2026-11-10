//! beets query language.
//!
//! Supported syntax (per command-line argument):
//! - `term`               case-insensitive substring over the default fields
//! - `field:term`         substring on one field
//! - `field:=term`        exact, case-sensitive; `field:=~term` exact, case-insensitive
//! - `field::regex`       regular expression (`::regex` searches the default fields)
//! - `year:1990..1999`    numeric range (open ends allowed: `1990..`, `..1999`)
//! - `length:3:00..4:00`  durations in M:SS
//! - `added:2024-01..`    dates (YYYY, YYYY-MM, YYYY-MM-DD)
//! - `comp:true`          booleans
//! - `path:/some/dir`     everything under a directory
//! - `-term` / `^term`    negation
//! - `,`                  separates OR-ed groups of terms
//! - `field+` / `field-`  sort ascending / descending

use crate::model::{FieldType, Kind, Record, Value, field_type, parse_bool, parse_duration};
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use regex::Regex;
use std::cmp::Ordering;
use std::path::PathBuf;

const ITEM_SEARCH_FIELDS: &[&str] = &[
    "artist",
    "title",
    "comments",
    "album",
    "albumartist",
    "genre",
];
const ALBUM_SEARCH_FIELDS: &[&str] = &["album", "albumartist", "genre"];

#[derive(Debug, Clone)]
pub enum Pred {
    True,
    Substr {
        field: Option<String>,
        pat: String,
    },
    Exact {
        field: String,
        pat: String,
        ci: bool,
    },
    Regex {
        field: Option<String>,
        re: Regex,
    },
    Numeric {
        field: String,
        lo: Option<f64>,
        hi: Option<f64>,
    },
    Bool {
        field: String,
        val: bool,
    },
    Date {
        field: String,
        lo: Option<f64>,
        hi: Option<f64>,
    },
    Path {
        prefix: PathBuf,
    },
    Not(Box<Pred>),
    And(Vec<Pred>),
    Or(Vec<Pred>),
}

#[derive(Debug, Clone)]
pub struct SortKey {
    pub field: String,
    pub asc: bool,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub pred: Pred,
    pub sort: Vec<SortKey>,
}

impl Query {
    pub fn parse<S: AsRef<str>>(kind: Kind, args: &[S]) -> Result<Self> {
        let mut groups: Vec<Vec<Pred>> = vec![vec![]];
        let mut sort = vec![];
        for arg in args {
            let arg = arg.as_ref();
            if arg == "," {
                groups.push(vec![]);
                continue;
            }
            if let Some(k) = parse_sort(arg) {
                sort.push(k);
                continue;
            }
            groups.last_mut().unwrap().push(parse_term(kind, arg)?);
        }
        let groups: Vec<Pred> = groups
            .into_iter()
            .map(|g| match g.len() {
                0 => Pred::True,
                1 => g.into_iter().next().unwrap(),
                _ => Pred::And(g),
            })
            .collect();
        let pred = if groups.len() == 1 {
            groups.into_iter().next().unwrap()
        } else {
            Pred::Or(groups)
        };
        if sort.is_empty() {
            sort = default_sort(kind);
        }
        Ok(Query { pred, sort })
    }

    pub fn matches(&self, rec: &Record) -> bool {
        self.pred.matches(rec)
    }

    pub fn sort(&self, recs: &mut [Record]) {
        let keys = &self.sort;
        recs.sort_by(|a, b| {
            for k in keys {
                let ord = compare_field(a, b, &k.field);
                let ord = if k.asc { ord } else { ord.reverse() };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.id.cmp(&b.id)
        });
    }
}

fn default_sort(kind: Kind) -> Vec<SortKey> {
    let fields: &[&str] = match kind {
        Kind::Item => &["albumartist", "album", "disc", "track"],
        Kind::Album => &["albumartist", "album"],
    };
    fields
        .iter()
        .map(|f| SortKey {
            field: f.to_string(),
            asc: true,
        })
        .collect()
}

fn compare_field(a: &Record, b: &Record, field: &str) -> Ordering {
    let ty = field_type(a.kind, field);
    let (va, vb) = (a.get(field), b.get(field));
    if ty.is_some_and(|t| t.is_numeric() || matches!(t, FieldType::Date | FieldType::Bool)) {
        let fa = va.and_then(|v| v.as_f64()).unwrap_or(0.0);
        let fb = vb.and_then(|v| v.as_f64()).unwrap_or(0.0);
        return fa.partial_cmp(&fb).unwrap_or(Ordering::Equal);
    }
    let sa = va.map(|v| v.raw().to_lowercase()).unwrap_or_default();
    let sb = vb.map(|v| v.raw().to_lowercase()).unwrap_or_default();
    sa.cmp(&sb)
}

fn parse_sort(term: &str) -> Option<SortKey> {
    let re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)([+-])$").unwrap();
    let c = re.captures(term)?;
    Some(SortKey {
        field: c[1].to_string(),
        asc: &c[2] == "+",
    })
}

pub fn parse_term(kind: Kind, term: &str) -> Result<Pred> {
    if term.len() > 1 && (term.starts_with('-') || term.starts_with('^')) {
        return Ok(Pred::Not(Box::new(parse_term(kind, &term[1..])?)));
    }
    let field_re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):(.*)$").unwrap();
    let (field, rest) = match field_re.captures(term) {
        Some(c) => (
            Some(c[1].to_string()),
            c.get(2).unwrap().as_str().to_string(),
        ),
        None => (None, term.to_string()),
    };

    // Regex: `field::re` or `::re` (the latter lands here as field=None, rest="::re"
    // because an empty field name doesn't match the field regex).
    if let Some(re) = rest.strip_prefix(':').filter(|_| field.is_some()) {
        return Ok(Pred::Regex {
            field,
            re: Regex::new(re).with_context(|| format!("bad regex {re:?}"))?,
        });
    }
    if field.is_none()
        && let Some(re) = rest.strip_prefix("::")
    {
        return Ok(Pred::Regex {
            field: None,
            re: Regex::new(re).with_context(|| format!("bad regex {re:?}"))?,
        });
    }

    let Some(field) = field else {
        // A bare term that names an existing path is a path query, like beets.
        if rest.contains('/') && std::path::Path::new(&rest).exists() {
            return Ok(Pred::Path {
                prefix: absolute(&rest),
            });
        }
        return Ok(Pred::Substr {
            field: None,
            pat: rest.to_lowercase(),
        });
    };

    if let Some(p) = rest.strip_prefix("=~") {
        return Ok(Pred::Exact {
            field,
            pat: p.to_lowercase(),
            ci: true,
        });
    }
    if let Some(p) = rest.strip_prefix('=') {
        return Ok(Pred::Exact {
            field,
            pat: p.to_string(),
            ci: false,
        });
    }

    let ty = if field == "singleton" && kind == Kind::Item {
        Some(FieldType::Bool)
    } else {
        field_type(kind, &field)
    };
    Ok(match ty {
        Some(FieldType::Path) => Pred::Path {
            prefix: absolute(&rest),
        },
        Some(FieldType::Bool) => Pred::Bool {
            val: parse_bool(&rest).with_context(|| format!("{field}: not a boolean: {rest:?}"))?,
            field,
        },
        Some(FieldType::Date) => {
            let (lo, hi) =
                parse_date_range(&rest).with_context(|| format!("{field}: bad date {rest:?}"))?;
            Pred::Date { field, lo, hi }
        }
        Some(t) if t.is_numeric() => {
            let parse = |s: &str| -> Result<Option<f64>> {
                if s.is_empty() {
                    return Ok(None);
                }
                let v = if t == FieldType::Duration {
                    parse_duration(s)
                } else {
                    s.parse().ok()
                };
                v.map(Some)
                    .with_context(|| format!("{field}: not a number: {s:?}"))
            };
            match rest.split_once("..") {
                Some((a, b)) => Pred::Numeric {
                    lo: parse(a)?,
                    hi: parse(b)?,
                    field,
                },
                None if rest.is_empty() => Pred::Substr {
                    field: Some(field),
                    pat: String::new(),
                },
                None => {
                    let v = parse(&rest)?;
                    Pred::Numeric {
                        lo: v,
                        hi: v,
                        field,
                    }
                }
            }
        }
        _ => Pred::Substr {
            field: Some(field),
            pat: rest.to_lowercase(),
        },
    })
}

fn absolute(p: &str) -> PathBuf {
    let p = crate::config::expand_tilde(p);
    let p = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    std::fs::canonicalize(&p).unwrap_or(p)
}

/// Parse "2024", "2024-03", "2024-03-05" or a `..` range of those into
/// [start, end) timestamps.
fn parse_date_range(s: &str) -> Result<(Option<f64>, Option<f64>)> {
    let (a, b) = match s.split_once("..") {
        Some((a, b)) => (a, b),
        None => (s, s),
    };
    let lo = if a.is_empty() {
        None
    } else {
        Some(date_period(a)?.0)
    };
    let hi = if b.is_empty() {
        None
    } else {
        Some(date_period(b)?.1)
    };
    Ok((lo, hi))
}

fn date_period(s: &str) -> Result<(f64, f64)> {
    let parts: Vec<i32> = s.split('-').map(|p| p.parse()).collect::<Result<_, _>>()?;
    let (start, end) = match parts.as_slice() {
        [y] => (
            NaiveDate::from_ymd_opt(*y, 1, 1),
            NaiveDate::from_ymd_opt(y + 1, 1, 1),
        ),
        [y, m] => {
            let start = NaiveDate::from_ymd_opt(*y, *m as u32, 1);
            let end = if *m == 12 {
                NaiveDate::from_ymd_opt(y + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(*y, *m as u32 + 1, 1)
            };
            (start, end)
        }
        [y, m, d] => {
            let start = NaiveDate::from_ymd_opt(*y, *m as u32, *d as u32);
            (start, start.and_then(|s| s.succ_opt()))
        }
        _ => bail!("bad date"),
    };
    let ts = |d: Option<NaiveDate>| -> Result<f64> {
        let d = d.context("invalid date")?;
        let dt = Local
            .with_ymd_and_hms(d.year(), d.month(), d.day(), 0, 0, 0)
            .earliest()
            .context("invalid local time")?;
        Ok(dt.timestamp() as f64)
    };
    Ok((ts(start)?, ts(end)?))
}

fn search_fields(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Item => ITEM_SEARCH_FIELDS,
        Kind::Album => ALBUM_SEARCH_FIELDS,
    }
}

fn str_of(rec: &Record, field: &str) -> Option<String> {
    rec.get(field).map(|v| match v {
        Value::Bool(b) => {
            if b {
                "True".into()
            } else {
                "False".into()
            }
        }
        v => v.raw(),
    })
}

impl Pred {
    pub fn matches(&self, rec: &Record) -> bool {
        match self {
            Pred::True => true,
            Pred::Not(p) => !p.matches(rec),
            Pred::And(ps) => ps.iter().all(|p| p.matches(rec)),
            Pred::Or(ps) => ps.iter().any(|p| p.matches(rec)),
            Pred::Substr {
                field: Some(f),
                pat,
            } => str_of(rec, f).is_some_and(|s| s.to_lowercase().contains(pat.as_str())),
            Pred::Substr { field: None, pat } => search_fields(rec.kind)
                .iter()
                .any(|f| rec.str(f).to_lowercase().contains(pat.as_str())),
            Pred::Exact { field, pat, ci } => str_of(rec, field).is_some_and(|s| {
                if *ci {
                    s.to_lowercase() == *pat
                } else {
                    s == *pat
                }
            }),
            Pred::Regex { field: Some(f), re } => str_of(rec, f).is_some_and(|s| re.is_match(&s)),
            Pred::Regex { field: None, re } => search_fields(rec.kind)
                .iter()
                .any(|f| re.is_match(rec.str(f))),
            Pred::Numeric { field, lo, hi } => {
                let Some(v) = rec.get(field).and_then(|v| v.as_f64()) else {
                    return false;
                };
                lo.is_none_or(|lo| v >= lo) && hi.is_none_or(|hi| v <= hi)
            }
            Pred::Bool { field, val } => {
                let v = match rec.get(field) {
                    Some(Value::Bool(b)) => b,
                    Some(Value::Str(s)) => parse_bool(&s).unwrap_or(false),
                    Some(v) => v.as_f64().unwrap_or(0.0) != 0.0,
                    None => false,
                };
                v == *val
            }
            Pred::Date { field, lo, hi } => {
                let v = rec.float(field);
                v != 0.0 && lo.is_none_or(|lo| v >= lo) && hi.is_none_or(|hi| v < hi)
            }
            Pred::Path { prefix } => {
                let path = match rec.kind {
                    Kind::Item => rec.path().to_path_buf(),
                    Kind::Album => return false,
                };
                path == *prefix || path.starts_with(prefix)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(artist: &str, title: &str, year: i64) -> Record {
        let mut r = Record::new_item();
        r.set_str("artist", artist);
        r.set_str("title", title);
        r.set_int("year", year);
        r.set_path(format!("/music/{artist}/{title}.mp3"));
        r
    }

    fn q(args: &[&str]) -> Query {
        Query::parse(Kind::Item, args).unwrap()
    }

    #[test]
    fn substring_and_fields() {
        let it = item("The Beatles", "Help!", 1965);
        assert!(q(&["beatles"]).matches(&it));
        assert!(q(&["artist:beat", "title:help"]).matches(&it));
        assert!(!q(&["title:beat"]).matches(&it));
        assert!(q(&[]).matches(&it));
    }

    #[test]
    fn exact_regex_negation_or() {
        let it = item("The Beatles", "Help!", 1965);
        assert!(q(&["artist:=The Beatles"]).matches(&it));
        assert!(!q(&["artist:=the beatles"]).matches(&it));
        assert!(q(&["artist:=~the beatles"]).matches(&it));
        assert!(q(&["title::^H.lp"]).matches(&it));
        assert!(q(&["::Beat"]).matches(&it));
        assert!(!q(&["-beatles"]).matches(&it));
        assert!(q(&["^stones"]).matches(&it));
        assert!(q(&["stones", ",", "help"]).matches(&it));
        assert!(!q(&["stones", ",", "zappa"]).matches(&it));
    }

    #[test]
    fn numeric_ranges() {
        let it = item("A", "B", 1965);
        assert!(q(&["year:1965"]).matches(&it));
        assert!(q(&["year:1960..1970"]).matches(&it));
        assert!(q(&["year:1965.."]).matches(&it));
        assert!(!q(&["year:..1964"]).matches(&it));
        assert!(Query::parse(Kind::Item, &["year:abc"]).is_err());
        let mut it = it;
        it.set("length", Value::Float(200.0));
        assert!(q(&["length:3:00..3:30"]).matches(&it));
    }

    #[test]
    fn path_and_bool() {
        let it = item("A", "B", 1);
        assert!(q(&["path:/music/A"]).matches(&it));
        assert!(!q(&["path:/music/AB"]).matches(&it));
        assert!(q(&["comp:false"]).matches(&it));
        assert!(q(&["singleton:true"]).matches(&it));
    }

    #[test]
    fn sorting() {
        let mut v = vec![
            item("b", "x", 2000),
            item("a", "y", 1990),
            item("c", "z", 1995),
        ];
        q(&["year-"]).sort(&mut v);
        let years: Vec<i64> = v.iter().map(|r| r.int("year")).collect();
        assert_eq!(years, vec![2000, 1995, 1990]);
        q(&["artist+"]).sort(&mut v);
        assert_eq!(v[0].str("artist"), "a");
    }

    #[test]
    fn dates() {
        let mut it = item("A", "B", 1);
        let ts = crate::model::parse_datetime("2024-03-15 12:00:00").unwrap();
        it.set("added", Value::Float(ts));
        assert!(q(&["added:2024"]).matches(&it));
        assert!(q(&["added:2024-03"]).matches(&it));
        assert!(!q(&["added:2024-04.."]).matches(&it));
        assert!(q(&["added:..2024-03-15"]).matches(&it));
    }
}
