//! Port of beets 2.14's `bucket` plugin: `%bucket{text[,field]}` puts a
//! year into a year range (`bucket_year`, `extrapolate`) and anything else
//! into an alphabetical range (`bucket_alpha`, `bucket_alpha_regex`).

use crate::config::Config;
use anyhow::{Result, bail};
use fancy_regex::Regex;
use serde_yaml::Value as Y;

const ASCII_DIGITS: &str = "0123456789abcdefghijklmnopqrstuvwxyz";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct YearSpan {
    pub from: i64,
    pub to: Option<i64>,
    pub str: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpanFormat {
    fromnchars: usize,
    tonchars: usize,
    fmt: String,
}

fn digits(s: &str) -> Vec<i64> {
    let re = regex::Regex::new(r"\d+").expect("BUG: the digits regex is valid");
    re.find_iter(s)
        .filter_map(|m| m.as_str().parse().ok())
        .collect()
}

/// `span_from_str`.
pub fn span_from_str(span_str: &str) -> Result<YearSpan> {
    let years = digits(span_str);
    let repr = crate::hooks::py_repr(span_str);
    if years.is_empty() {
        bail!("invalid range defined for year bucket {repr}: no year found");
    }
    let yearfrom = years[0];
    let mut out = vec![];
    for d in years {
        if yearfrom < 100 {
            bail!(
                "invalid range defined for year bucket {repr}: {yearfrom} must be expressed on 4 digits"
            );
        }
        let d = if d < 100 {
            if d % 100 < yearfrom % 100 {
                yearfrom - yearfrom % 100 + 100 + d
            } else {
                yearfrom - yearfrom % 100 + d
            }
        } else {
            d
        };
        out.push(d);
    }
    Ok(YearSpan {
        from: out[0],
        to: (out.len() > 1).then(|| {
            *out.last()
                .expect("BUG: out has more than one year")
        }),
        str: Some(span_str.to_string()),
    })
}

fn this_year() -> i64 {
    use chrono::Datelike;
    chrono::Local::now().year() as i64
}

/// `complete_year_spans`: sort by start; open ends reach the next span (the
/// last one the current year).
fn complete_year_spans(spans: &mut [YearSpan]) {
    spans.sort_by_key(|s| s.from);
    for k in 0..spans.len().saturating_sub(1) {
        if spans[k].to.is_none() {
            spans[k].to = Some(spans[k + 1].from - 1);
        }
    }
    if let Some(last) = spans.last_mut()
        && last.to.is_none()
    {
        last.to = Some(this_year());
    }
}

/// Python's `range(start, stop, step)` as a Vec.
fn py_range(start: i64, stop: i64, step: i64) -> Vec<i64> {
    let mut out = vec![];
    let mut k = start;
    while (step > 0 && k < stop) || (step < 0 && k > stop) {
        out.push(k);
        k += step;
    }
    out
}

/// `extend_year_spans` (with beets' fixed 1900-2014 range).
fn extend_year_spans(spans: &[YearSpan], spanlen: i64) -> Vec<YearSpan> {
    let (start, end) = (1900, 2014);
    let mut ext = spans.to_vec();
    for w in spans.windows(2) {
        for f in py_range(
            w[0].to
                .expect("BUG: complete_year_spans fills every span's `to`")
                + 1,
            w[1].from,
            spanlen,
        ) {
            ext.push(YearSpan {
                from: f,
                ..Default::default()
            });
        }
    }
    for f in py_range(spans[0].from - spanlen, start, -spanlen) {
        ext.push(YearSpan {
            from: f,
            ..Default::default()
        });
    }
    for f in py_range(
        spans
            .last()
            .expect("BUG: extend_year_spans is called with a non-empty list")
            .to
            .expect("BUG: complete_year_spans fills every span's `to`")
            + 1,
        end,
        spanlen,
    ) {
        ext.push(YearSpan {
            from: f,
            ..Default::default()
        });
    }
    complete_year_spans(&mut ext);
    ext
}

/// `str2fmt`.
fn str2fmt(s: &str) -> SpanFormat {
    let re = regex::Regex::new(
        r"^(?P<bef>\D*)(?P<fromyear>\d+)(?P<sep>\D*)(?P<toyear>\d*)(?P<after>\D*)",
    )
    .expect("BUG: the str2fmt pattern is valid");
    match re.captures(s) {
        Some(c) => {
            let g = |n: &str| c.name(n).map_or("", |m| m.as_str());
            let tonchars = g("toyear").len();
            SpanFormat {
                fromnchars: g("fromyear").len(),
                tonchars,
                fmt: format!(
                    "{}{{}}{}{}{}",
                    g("bef"),
                    g("sep"),
                    if tonchars > 0 { "{}" } else { "" },
                    g("after")
                ),
            }
        }
        None => SpanFormat {
            fromnchars: 0,
            tonchars: 0,
            fmt: "{}".into(),
        },
    }
}

/// Python's `str(n)[-k:]` (the whole string for k = 0).
fn tail(n: i64, k: usize) -> String {
    let s = n.to_string();
    let chars: Vec<char> = s.chars().collect();
    if k == 0 || k >= chars.len() {
        s
    } else {
        chars[chars.len() - k..].iter().collect()
    }
}

fn format_span(f: &SpanFormat, from: i64, to: i64) -> String {
    let mut args = vec![tail(from, f.fromnchars)];
    if f.tonchars > 0 {
        args.push(tail(to, f.tonchars));
    }
    crate::plugins::the::py_format(&f.fmt, &args.iter().map(String::as_str).collect::<Vec<_>>())
}

/// `sorted(xs, key=xs.count)[-1]`: the most common, the last of a tie in
/// the (stably) sorted order.
fn mode<T: Clone + PartialEq>(xs: &[T]) -> T {
    let mut v: Vec<T> = xs.to_vec();
    v.sort_by_key(|x| xs.iter().filter(|y| *y == x).count());
    v.last()
        .expect("BUG: mode is called with at least one value")
        .clone()
}

fn extract_modes(spans: &[YearSpan]) -> (i64, SpanFormat) {
    let mut lens: Vec<i64> = spans
        .iter()
        .map(|s| {
            s.to.expect("BUG: complete_year_spans fills every span's `to`") - s.from + 1
        })
        .collect();
    lens.sort();
    let reprs: Vec<SpanFormat> = spans
        .iter()
        .map(|s| str2fmt(s.str.as_deref().unwrap_or("")))
        .collect();
    (mode(&lens), mode(&reprs))
}

#[derive(Debug)]
pub struct Bucket {
    year_spans: Vec<YearSpan>,
    repr_mode: Option<SpanFormat>,
    alpha_names: Vec<String>,
    alpha_spans: Vec<Regex>,
}

fn str_list(v: Option<&Y>) -> Vec<String> {
    match v {
        Some(Y::Sequence(s)) => s
            .iter()
            .map(|x| match x {
                Y::String(s) => s.clone(),
                Y::Number(n) => n.to_string(),
                other => serde_yaml::to_string(other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            })
            .collect(),
        _ => vec![],
    }
}

impl Bucket {
    /// `setup` from the `bucket:` config.
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut year_spans = vec![];
        for s in str_list(cfg.get("bucket.bucket_year")) {
            year_spans.push(span_from_str(&s)?);
        }
        complete_year_spans(&mut year_spans);
        let mut repr_mode = None;
        if !year_spans.is_empty() && cfg.bool("bucket.extrapolate") {
            let (len, fmt) = extract_modes(&year_spans);
            year_spans = extend_year_spans(&year_spans, len);
            repr_mode = Some(fmt);
        }
        let alpha_names = str_list(cfg.get("bucket.bucket_alpha"));
        let regexes = cfg.mapping("bucket.bucket_alpha_regex");
        let mut alpha_spans = vec![];
        for elem in &alpha_names {
            if let Some((_, re)) = regexes.iter().find(|(k, _)| k == elem) {
                alpha_spans.push(
                    Regex::new(re)
                        .map_err(|e| anyhow::anyhow!("invalid regular expression {re:?}: {e}"))?,
                );
                continue;
            }
            let mut chars: Vec<char> = elem
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            chars.sort();
            if chars.is_empty() {
                bail!(
                    "invalid range defined for alpha bucket '{elem}': no alphanumeric character found"
                );
            }
            let index = |c: char| {
                ASCII_DIGITS
                    .find(c)
                    .ok_or_else(|| anyhow::anyhow!("substring not found"))
            };
            let (b, e) = (
                index(chars[0])?,
                index(*chars.last().expect("BUG: chars is non-empty (checked above)"))?,
            );
            alpha_spans.push(Regex::new(&format!(
                "(?i)^[{}]",
                &ASCII_DIGITS[b..=e.max(b)]
            ))?);
        }
        Ok(Bucket {
            year_spans,
            repr_mode,
            alpha_names,
            alpha_spans,
        })
    }

    fn find_bucket_year(&self, year: &str) -> Result<String, String> {
        let y: i64 = year.trim().parse().map_err(|_| {
            format!(
                "invalid literal for int() with base 10: {}",
                crate::hooks::py_repr(year)
            )
        })?;
        for ys in &self.year_spans {
            if ys.from <= y && y <= ys.to.unwrap_or(i64::MAX) {
                if let Some(s) = &ys.str {
                    return Ok(s.clone());
                }
                let f = self
                    .repr_mode
                    .as_ref()
                    .expect("BUG: generated spans come from extend_year_spans, which sets repr_mode");
                return Ok(format_span(
                    f,
                    ys.from,
                    ys.to.expect("BUG: complete_year_spans fills every span's `to`"),
                ));
            }
        }
        Ok(year.to_string())
    }

    fn find_bucket_alpha(&self, s: &str) -> Result<String, String> {
        for (i, span) in self.alpha_spans.iter().enumerate() {
            if span.find(s).ok().flatten().is_some_and(|m| m.start() == 0) {
                return Ok(self.alpha_names[i].clone());
            }
        }
        match s.chars().next() {
            Some(c) => Ok(c.to_uppercase().collect()),
            None => Err("string index out of range".into()),
        }
    }

    /// `_tmpl_bucket(text, field=None)`.
    pub fn bucket(&self, text: &str, field: Option<&str>) -> Result<String, String> {
        let field = match field {
            Some(f) if !f.is_empty() => Some(f),
            _ if text.chars().count() == 4 && text.chars().all(|c| c.is_numeric()) => Some("year"),
            _ => None,
        };
        if field == Some("year") {
            self.find_bucket_year(text)
        } else {
            self.find_bucket_alpha(text)
        }
    }

    /// `%bucket{...}` with Python's errors for the wrong arity.
    pub fn call(&self, args: &[String]) -> Result<String, String> {
        let q = "BucketPlugin._tmpl_bucket()";
        match args.len() {
            0 => Err(format!(
                "{q} missing 1 required positional argument: 'text'"
            )),
            1 => self.bucket(&args[0], None),
            2 => self.bucket(&args[0], Some(&args[1])),
            n => Err(format!(
                "{q} takes from 2 to 3 positional arguments but {} were given",
                n + 1
            )),
        }
    }
}

pub struct BucketPlugin;

impl super::Plugin for BucketPlugin {
    fn name(&self) -> &'static str {
        "bucket"
    }
}

#[cfg(test)]
mod tests {
    //! Ported from beets' test/plugins/test_bucket.py.
    use super::*;

    fn setup(year: &[&str], alpha: &[&str], regex: &str, extrapolate: bool) -> Result<Bucket> {
        let mut cfg = Config::defaults();
        let list = |v: &[&str]| Y::Sequence(v.iter().map(|s| Y::String(s.to_string())).collect());
        cfg.set("bucket.bucket_year", list(year));
        cfg.set("bucket.bucket_alpha", list(alpha));
        cfg.set(
            "bucket.bucket_alpha_regex",
            serde_yaml::from_str(regex).unwrap(),
        );
        cfg.set("bucket.extrapolate", Y::Bool(extrapolate));
        Bucket::new(&cfg)
    }

    fn b(p: &Bucket, s: &str) -> String {
        p.bucket(s, None).unwrap()
    }

    #[test]
    fn year_single_year() {
        let p = setup(&["1950s", "1970s"], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "1959"), "1950s");
        assert_eq!(b(&p, "1969"), "1950s");
    }

    #[test]
    fn year_single_year_last_folder() {
        let p = setup(&["1950", "1970"], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "2014"), "1970");
        let next = (this_year() + 1).to_string();
        assert_eq!(b(&p, &next), next);
    }

    #[test]
    fn year_two_years() {
        let p = setup(&["1950-59", "1960-1969"], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "1959"), "1950-59");
        assert_eq!(b(&p, "1969"), "1960-1969");
    }

    #[test]
    fn year_multiple_years() {
        let p = setup(&["1950,51,52,53"], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "1953"), "1950,51,52,53");
        assert_eq!(b(&p, "1974"), "1974");
    }

    #[test]
    fn year_out_of_range() {
        let p = setup(&["1950-59", "1960-69"], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "1974"), "1974");
        let p = setup(&[], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "1974"), "1974");
    }

    #[test]
    fn year_out_of_range_extrapolate() {
        let p = setup(&["1950-59", "1960-69"], &[], "{}", true).unwrap();
        assert_eq!(b(&p, "1914"), "1910-19");
        let p = setup(&["1962-81", "2002", "2012"], &[], "{}", true).unwrap();
        assert_eq!(b(&p, "1983"), "1982");
        let p = setup(&["1962-81", "2002", "2012-14"], &[], "{}", true).unwrap();
        assert_eq!(b(&p, "1983"), "1982-01");
        let p = setup(
            &["1932", "1942", "1952", "1962-81", "2002"],
            &[],
            "{}",
            true,
        )
        .unwrap();
        assert_eq!(b(&p, "1975"), "1962-81");
    }

    #[test]
    fn alpha_all_chars() {
        let p = setup(&[], &["ABCD", "FGH", "IJKL"], "{}", false).unwrap();
        assert_eq!(b(&p, "garry"), "FGH");
    }

    #[test]
    fn alpha_first_last_chars() {
        let p = setup(&[], &["0->9", "A->D", "F-H", "I->Z"], "{}", false).unwrap();
        assert_eq!(b(&p, "garry"), "F-H");
        assert_eq!(b(&p, "2pac"), "0->9");
    }

    #[test]
    fn alpha_out_of_range() {
        let p = setup(&[], &["ABCD", "FGH", "IJKL"], "{}", false).unwrap();
        assert_eq!(b(&p, "errol"), "E");
        let p = setup(&[], &[], "{}", false).unwrap();
        assert_eq!(b(&p, "errol"), "E");
    }

    #[test]
    fn alpha_regex() {
        let p = setup(
            &[],
            &["foo", "bar"],
            "{foo: '^[a-d]', bar: '^[e-z]'}",
            false,
        )
        .unwrap();
        assert_eq!(b(&p, "alpha"), "foo");
        assert_eq!(b(&p, "delta"), "foo");
        assert_eq!(b(&p, "zeta"), "bar");
        assert_eq!(b(&p, "Alpha"), "A");
    }

    #[test]
    fn alpha_regex_mix() {
        let p = setup(
            &[],
            &["A - D", "E - L"],
            "{'A - D': '^[0-9a-dA-D\u{2026}\u{e4}\u{c4}]'}",
            false,
        )
        .unwrap();
        assert_eq!(b(&p, "alpha"), "A - D");
        assert_eq!(b(&p, "\u{c4}rzte"), "A - D");
        assert_eq!(b(&p, "112"), "A - D");
        assert_eq!(b(&p, "\u{2026}and Oceans"), "A - D");
        assert_eq!(b(&p, "Eagles"), "E - L");
    }

    #[test]
    fn bad_alpha_range_def() {
        assert!(setup(&[], &["$%"], "{}", false).is_err());
    }

    #[test]
    fn bad_year_range_def_no4digits() {
        assert!(setup(&["62-64"], &[], "{}", false).is_err());
    }

    #[test]
    fn bad_year_range_def_nodigits() {
        assert!(setup(&["nodigits"], &[], "{}", false).is_err());
    }

    #[test]
    fn test_span_from_str() {
        for (s, from, to) in [
            ("1980 2000", 1980, 2000),
            ("1980 00", 1980, 2000),
            ("1930 00", 1930, 2000),
            ("1930 50", 1930, 1950),
        ] {
            let d = span_from_str(s).unwrap();
            assert_eq!((d.from, d.to), (from, Some(to)), "{s}");
        }
    }

    #[test]
    fn template_errors() {
        let p = setup(&[], &[], "{}", false).unwrap();
        assert_eq!(
            p.bucket("abc", Some("year")).unwrap_err(),
            "invalid literal for int() with base 10: 'abc'"
        );
        assert_eq!(p.bucket("", None).unwrap_err(), "string index out of range");
        assert_eq!(
            p.call(&[]).unwrap_err(),
            "BucketPlugin._tmpl_bucket() missing 1 required positional argument: 'text'"
        );
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "bucket",
        order: 30,
        defaults: r#"bucket:
    bucket_year: []
    bucket_alpha: []
    bucket_alpha_regex: {}
    extrapolate: no
"#,
        config_keys: &["bucket"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Bucket::new(cfg)?;
    Ok(Box::new(BucketPlugin))
}
