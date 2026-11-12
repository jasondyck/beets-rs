//! fetchart: a port of beets 2.14's `beetsplug/fetchart.py`. Finds album art
//! in the album's folder or on the web (Cover Art Archive, iTunes, Amazon,
//! AlbumArt.org, Wikipedia, Google, fanart.tv, Last.fm, Spotify, a
//! `cover_art_url` field), checks it against `minwidth`/`enforce_ratio`,
//! resizes or converts it as configured (`maxwidth`, `max_filesize`,
//! `cover_format`, `deinterlace`), and sets it as the album's art: on
//! import and with `beet fetchart`.

use super::art::{self, image_mime_type};
use super::requests::{HttpError, Request, Response, Session, format_url};
use super::{INFO, Plugin, PluginCtx, PluginLog, WARNING};
use crate::artresizer;
use crate::config::Config;
use crate::fileops::{FileOp, WalkFilter};
use crate::library::Library;
use crate::model::Kind;
use crate::model::{Album, Item, Value};
use crate::query::Query;
use anyhow::{Result, bail};
use serde_json::Value as J;
use std::path::{Path, PathBuf};

/// `CONTENT_TYPES`: the image types fetchart accepts, with extensions.
const CONTENT_TYPES: &[(&str, &[&str])] = &[
    ("image/jpeg", &["jpg", "jpeg"]),
    ("image/png", &["png"]),
    ("image/webp", &["webp"]),
];

fn image_extensions() -> impl Iterator<Item = &'static str> {
    CONTENT_TYPES.iter().flat_map(|(_, e)| e.iter().copied())
}

/// The directory an album's files live in (that of its first item):
/// beets' `Album.item_dir`.
pub fn album_dir(items: &[Item]) -> Option<PathBuf> {
    items
        .first()
        .and_then(|i| i.path().parent())
        .map(Path::to_path_buf)
}

// ---------------------------------------------------------------- python-isms

/// Python's `repr(float)`.
pub fn py_float(f: f64) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// Python's `repr(str)`.
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
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// Python's `str.title()`.
pub fn py_title(s: &str) -> String {
    let mut out = String::new();
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_lowercase() || c.is_uppercase();
    }
    out
}

fn py_keys(v: &J) -> String {
    match v.as_object() {
        Some(m) => format!(
            "[{}]",
            m.keys().map(|k| py_repr(k)).collect::<Vec<_>>().join(", ")
        ),
        None => "[]".into(),
    }
}

// ---------------------------------------------------------------- config

/// Service endpoints (overridable for tests).
#[derive(Clone, Debug)]
pub struct Urls {
    pub caa: String,
    pub amazon: String,
    pub albumart: String,
    pub google: String,
    pub fanarttv: String,
    pub itunes: String,
    pub dbpedia: String,
    pub wikipedia: String,
    pub lastfm: String,
    pub spotify: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            caa: "https://coverartarchive.org".into(),
            amazon: "https://images.amazon.com/images/P/".into(),
            albumart: "https://www.albumart.org/index_detail.php".into(),
            google: "https://www.googleapis.com/customsearch/v1".into(),
            fanarttv: "https://webservice.fanart.tv/v3/music/albums/".into(),
            itunes: "https://itunes.apple.com/search".into(),
            dbpedia: "https://dbpedia.org/sparql".into(),
            wikipedia: "https://en.wikipedia.org/w/api.php".into(),
            lastfm: "https://ws.audioscrobbler.com/2.0".into(),
            spotify: "https://open.spotify.com/album/".into(),
        }
    }
}

/// fanart.tv's key for beets.
const FANARTTV_PROJECT_KEY: &str = "61a7d0ab4e67162b7a0c7c35915cd48e";

#[derive(Clone, Debug, Default)]
pub struct Conf {
    pub auto: bool,
    pub fetch_for_asis: bool,
    pub minwidth: u32,
    pub maxwidth: u32,
    pub quality: u8,
    pub max_filesize: u64,
    pub enforce_ratio: bool,
    pub margin_px: Option<u32>,
    pub margin_percent: Option<f64>,
    pub cautious: bool,
    pub cover_names: Vec<String>,
    pub fallback: Option<PathBuf>,
    pub store_source: bool,
    pub high_resolution: bool,
    pub deinterlace: bool,
    pub cover_format: Option<String>,
    pub google_key: Option<String>,
    pub google_engine: Option<String>,
    pub lastfm_key: Option<String>,
    pub fanarttv_key: Option<String>,
}

fn opt_str(cfg: &Config, key: &str) -> Option<String> {
    let s = cfg.str(key);
    (!s.is_empty()).then_some(s)
}

fn int(cfg: &Config, key: &str) -> i64 {
    cfg.f64(key) as i64
}

impl Conf {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut c = Conf {
            auto: cfg.bool("fetchart.auto"),
            fetch_for_asis: cfg.bool("fetchart.fetch_for_asis"),
            minwidth: int(cfg, "fetchart.minwidth").max(0) as u32,
            maxwidth: int(cfg, "fetchart.maxwidth").max(0) as u32,
            quality: int(cfg, "fetchart.quality").clamp(0, 100) as u8,
            max_filesize: int(cfg, "fetchart.max_filesize").max(0) as u64,
            cautious: cfg.bool("fetchart.cautious"),
            cover_names: cfg.list("fetchart.cover_names"),
            fallback: opt_str(cfg, "fetchart.fallback")
                .map(|p| crate::plugins::embedart::normpath(Path::new(&p))),
            store_source: cfg.bool("fetchart.store_source"),
            high_resolution: cfg.bool("fetchart.high_resolution"),
            deinterlace: cfg.bool("fetchart.deinterlace"),
            cover_format: opt_str(cfg, "fetchart.cover_format"),
            google_key: opt_str(cfg, "fetchart.google_key"),
            google_engine: opt_str(cfg, "fetchart.google_engine"),
            lastfm_key: opt_str(cfg, "fetchart.lastfm_key"),
            fanarttv_key: opt_str(cfg, "fetchart.fanarttv_key"),
            ..Default::default()
        };
        // `enforce_ratio`: a bool, or a margin in pixels ("10px") or
        // percent of the long edge ("0.5%").
        match cfg.get("fetchart.enforce_ratio") {
            Some(serde_yaml::Value::Bool(b)) => c.enforce_ratio = *b,
            None | Some(serde_yaml::Value::Null) => {}
            Some(serde_yaml::Value::String(s)) if crate::model::parse_bool(s).is_some() => {
                c.enforce_ratio = crate::model::parse_bool(s).unwrap_or(false);
            }
            Some(serde_yaml::Value::String(s)) => {
                let px = regex::Regex::new(r"^(0|[1-9][0-9]*)px").unwrap();
                let pct = regex::Regex::new(r"^(100(\.00?)?|[1-9]?[0-9](\.[0-9]{1,2})?)%").unwrap();
                if (px.is_match(s) || pct.is_match(s)) && s.ends_with('%') {
                    c.margin_percent = Some(s[..s.len() - 1].parse::<f64>()? / 100.0);
                } else if (px.is_match(s) || pct.is_match(s)) && s.ends_with("px") {
                    c.margin_px = Some(s[..s.len() - 2].parse()?);
                } else {
                    bail!(
                        "fetchart.enforce_ratio: must be one of a boolean, a string matching \
                         '(0|[1-9][0-9]*)px' or a string matching \
                         '(100(\\.00?)?|[1-9]?[0-9](\\.[0-9]{{1,2}})?)%', not {s:?}"
                    );
                }
                c.enforce_ratio = true;
            }
            Some(other) => bail!(
                "fetchart.enforce_ratio: must be a boolean or a margin, not {}",
                serde_yaml::to_string(other).unwrap_or_default().trim()
            ),
        }
        Ok(c)
    }
}

// ---------------------------------------------------------------- candidates

/// `ImageAction`: whether an image is usable or needs post-processing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageAction {
    Bad,
    Exact,
    Downscale,
    Downsize,
    Deinterlace,
    Reformat,
}

/// `MetadataMatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataMatch {
    Exact,
    Fallback,
}

/// A matching artwork: `Candidate`.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub source_name: &'static str,
    #[allow(dead_code)] // as beets: recorded, though nothing reads it yet
    pub match_: Option<MetadataMatch>,
    pub size: Option<(u32, u32)>,
    /// Debug lines logged when the candidate is reached (beets' sources are
    /// generators that log as they yield).
    pre_log: Vec<String>,
}

impl Candidate {
    fn new(source_name: &'static str) -> Self {
        Candidate {
            path: None,
            url: None,
            source_name,
            match_: None,
            size: None,
            pre_log: vec![],
        }
    }

    fn url(source: &'static str, url: impl Into<String>, m: Option<MetadataMatch>) -> Self {
        Candidate {
            url: Some(url.into()),
            match_: m,
            ..Candidate::new(source)
        }
    }

    fn file(source: &'static str, path: PathBuf, m: MetadataMatch) -> Self {
        Candidate {
            path: Some(path),
            match_: Some(m),
            ..Candidate::new(source)
        }
    }

    /// `_validate`: whether the image is usable as it is, needs work, or is
    /// unusable, given the options. `skip` lists actions already done.
    fn check(&mut self, log: &PluginLog, c: &Conf, skip: &[ImageAction]) -> ImageAction {
        let Some(path) = self.path.clone() else {
            return ImageAction::Bad;
        };
        if !(c.enforce_ratio
            || c.minwidth != 0
            || c.maxwidth != 0
            || c.max_filesize != 0
            || c.deinterlace
            || c.cover_format.is_some())
        {
            return ImageAction::Exact;
        }
        if self.size.is_none() {
            self.size = artresizer::get_size(&path);
        }
        log.debug(&format!(
            "image size: {}",
            match self.size {
                Some((w, h)) => format!("({w}, {h})"),
                None => "None".into(),
            }
        ));
        let Some((w, h)) = self.size else {
            log.warning(
                "Could not get size of image (please see documentation for dependencies). \
                 The configuration options `minwidth`, `enforce_ratio` and `max_filesize` \
                 may be violated.",
            );
            return ImageAction::Exact;
        };
        let short_edge = w.min(h);
        let long_edge = w.max(h);

        if c.minwidth != 0 && w < c.minwidth {
            log.debug(&format!("image too small ({w} < {})", c.minwidth));
            return ImageAction::Bad;
        }

        let edge_diff = long_edge - short_edge;
        if c.enforce_ratio {
            if let Some(m) = c.margin_px.filter(|&m| m != 0) {
                if edge_diff > m {
                    log.debug(&format!(
                        "image is not close enough to being square, \
                         ({long_edge} - {short_edge} > {m})"
                    ));
                    return ImageAction::Bad;
                }
            } else if let Some(p) = c.margin_percent.filter(|&p| p != 0.0) {
                let margin_px = p * long_edge as f64;
                if edge_diff as f64 > margin_px {
                    log.debug(&format!(
                        "image is not close enough to being square, \
                         ({long_edge} - {short_edge} > {})",
                        py_float(margin_px)
                    ));
                    return ImageAction::Bad;
                }
            } else if edge_diff != 0 {
                log.debug(&format!("image is not square ({w} != {h})"));
                return ImageAction::Bad;
            }
        }

        let mut downscale = false;
        if c.maxwidth != 0 && w > c.maxwidth {
            log.debug(&format!("image needs rescaling ({w} > {})", c.maxwidth));
            downscale = true;
        }

        let mut downsize = false;
        if c.max_filesize != 0 {
            let filesize = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if filesize > c.max_filesize {
                log.debug(&format!(
                    "image needs resizing ({filesize}B > {}B)",
                    c.max_filesize
                ));
                downsize = true;
            }
        }

        let mut reformat = false;
        if let Some(cf) = &c.cover_format {
            let fmt = artresizer::get_format(&path);
            reformat = fmt.as_deref() != Some(cf.as_str());
            if reformat {
                log.debug(&format!(
                    "image needs reformatting: {} -> {cf}",
                    fmt.as_deref().unwrap_or("None")
                ));
            }
        }

        if downscale && !skip.contains(&ImageAction::Downscale) {
            return ImageAction::Downscale;
        }
        if reformat && !skip.contains(&ImageAction::Reformat) {
            return ImageAction::Reformat;
        }
        if c.deinterlace && !skip.contains(&ImageAction::Deinterlace) {
            return ImageAction::Deinterlace;
        }
        if downsize && !skip.contains(&ImageAction::Downsize) {
            return ImageAction::Downsize;
        }
        ImageAction::Exact
    }

    /// `resize`: process the image until it's valid (or can't be helped).
    fn resize(&mut self, log: &PluginLog, c: &Conf) {
        let mut current = self.check(log, c, &[]);
        let mut done = vec![];
        while !matches!(current, ImageAction::Bad | ImageAction::Exact) {
            self.process(c, current);
            done.push(current);
            current = self.check(log, c, &done);
        }
    }

    /// `_resize`: one processing step. (As in beets, `size` keeps the
    /// original dimensions.)
    fn process(&mut self, c: &Conf, action: ImageAction) {
        let (Some(path), Some(size)) = (self.path.clone(), self.size) else {
            return;
        };
        self.path = Some(match action {
            ImageAction::Downscale => {
                artresizer::resize(c.maxwidth, &path, c.quality, c.max_filesize)
            }
            ImageAction::Downsize => {
                artresizer::resize(size.0.max(size.1), &path, c.quality, c.max_filesize)
            }
            ImageAction::Deinterlace => artresizer::deinterlace(&path),
            ImageAction::Reformat => artresizer::reformat(
                &path,
                c.cover_format.as_deref().unwrap_or_default(),
                c.deinterlace,
            ),
            ImageAction::Bad | ImageAction::Exact => path.clone(),
        });
        // An intermediate temporary file is no longer needed.
        if self.path.as_deref() != Some(path.as_path()) && is_temp(&path) && path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `path` is one of our temporary files (a download or a
/// processed image), to be removed once used.
fn is_temp(path: &Path) -> bool {
    path.starts_with(std::env::temp_dir())
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("beets-"))
}

// ---------------------------------------------------------------- sources

/// The art sources, in beets' `ART_SOURCES` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    FileSystem,
    CoverArt,
    ITunes,
    AlbumArt,
    Amazon,
    Wikipedia,
    Google,
    FanartTV,
    LastFM,
    Spotify,
    CoverArtUrl,
}

const ALL_SOURCES: [SourceKind; 11] = [
    SourceKind::FileSystem,
    SourceKind::CoverArt,
    SourceKind::ITunes,
    SourceKind::AlbumArt,
    SourceKind::Amazon,
    SourceKind::Wikipedia,
    SourceKind::Google,
    SourceKind::FanartTV,
    SourceKind::LastFM,
    SourceKind::Spotify,
    SourceKind::CoverArtUrl,
];

impl SourceKind {
    pub fn id(self) -> &'static str {
        match self {
            SourceKind::FileSystem => "filesystem",
            SourceKind::CoverArt => "coverart",
            SourceKind::ITunes => "itunes",
            SourceKind::AlbumArt => "albumart",
            SourceKind::Amazon => "amazon",
            SourceKind::Wikipedia => "wikipedia",
            SourceKind::Google => "google",
            SourceKind::FanartTV => "fanarttv",
            SourceKind::LastFM => "lastfm",
            SourceKind::Spotify => "spotify",
            SourceKind::CoverArtUrl => "cover_art_url",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SourceKind::FileSystem => "Filesystem",
            SourceKind::CoverArt => "Cover Art Archive",
            SourceKind::ITunes => "iTunes Store",
            SourceKind::AlbumArt => "AlbumArt.org scraper",
            SourceKind::Amazon => "Amazon",
            SourceKind::Wikipedia => "Wikipedia (queried through DBpedia)",
            SourceKind::Google => "Google Images",
            SourceKind::FanartTV => "fanart.tv",
            SourceKind::LastFM => "Last.fm",
            SourceKind::Spotify => "Spotify",
            SourceKind::CoverArtUrl => "Cover Art URL",
        }
    }

    /// `VALID_MATCHING_CRITERIA`.
    pub fn criteria(self) -> &'static [&'static str] {
        match self {
            SourceKind::CoverArt => &["release", "releasegroup"],
            _ => &["default"],
        }
    }

    /// `LOC`: whether it finds local files.
    pub fn local(self) -> bool {
        self == SourceKind::FileSystem
    }

    /// `available`: whether the source can be used (API keys present).
    fn available(self, log: &PluginLog, c: &Conf) -> bool {
        match self {
            SourceKind::Google if c.google_key.is_none() => {
                log.debug("google: Disabling art source due to missing key");
                false
            }
            SourceKind::LastFM if c.lastfm_key.is_none() => {
                log.debug("lastfm: Disabling art source due to missing key");
                false
            }
            _ => true,
        }
    }
}

/// A configured source with its matching criterion.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub kind: SourceKind,
    pub match_by: Vec<String>,
}

impl Source {
    pub fn description(&self) -> String {
        format!("{}[{}]", self.kind.id(), self.match_by.join(", "))
    }
}

/// beets' `sanitize_pairs(..., raise_on_unknown=True)`.
pub fn sanitize_pairs(
    pairs: &[(String, String)],
    all: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let others: Vec<&(String, String)> = all.iter().filter(|x| !pairs.contains(x)).collect();
    let mut seen: Vec<(String, String)> = vec![];
    for (k, values) in pairs {
        for v in values.split_whitespace() {
            let x = (k.clone(), v.to_string());
            if all.contains(&x) {
                if !seen.contains(&x) {
                    seen.push(x);
                }
            } else if k == "*" {
                for o in &others {
                    if !seen.contains(o) {
                        seen.push((*o).clone());
                    }
                }
            } else if v == "*" {
                let new: Vec<(String, String)> = others
                    .iter()
                    .filter(|o| !seen.contains(o) && o.0 == *k)
                    .map(|o| (*o).clone())
                    .collect();
                if new.is_empty() {
                    bail!("setting {k}={v} is not recognized");
                }
                seen.extend(new);
            } else {
                bail!("setting {k}={v} is not recognized");
            }
        }
    }
    Ok(seen)
}

/// confuse's `as_pairs(default_value="*")` for `sources` (a single
/// string is one source, as beets reads it).
fn source_pairs(cfg: &Config) -> Vec<(String, String)> {
    use serde_yaml::Value as Y;
    let scalar = |v: &Y| match v {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    match cfg.get("fetchart.sources") {
        Some(Y::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| match v {
                Y::Mapping(m) if m.len() == 1 => {
                    let (k, v) = m.iter().next()?;
                    let v = match v {
                        Y::Sequence(s) => s.iter().filter_map(scalar).collect::<Vec<_>>().join(" "),
                        v => scalar(v).unwrap_or_else(|| "*".into()),
                    };
                    Some((scalar(k)?, v))
                }
                v => Some((scalar(v)?, "*".into())),
            })
            .collect(),
        Some(Y::Mapping(m)) => m
            .iter()
            .filter_map(|(k, v)| {
                let v = match v {
                    Y::Sequence(s) => s.iter().filter_map(scalar).collect::<Vec<_>>().join(" "),
                    v => scalar(v).unwrap_or_else(|| "*".into()),
                };
                Some((scalar(k)?, v))
            })
            .collect(),
        Some(v) => scalar(v).map(|s| vec![(s, "*".into())]).unwrap_or_default(),
        None => vec![],
    }
}

// ---------------------------------------------------------------- plugin

pub struct FetchArt {
    pub conf: Conf,
    pub urls: Urls,
    pub sources: Vec<Source>,
    pub log: PluginLog,
    session: Session,
}

impl FetchArt {
    pub fn new(cfg: &Config) -> Result<Self> {
        Self::with(Conf::new(cfg)?, Urls::default(), cfg)
    }

    pub fn with(conf: Conf, urls: Urls, cfg: &Config) -> Result<Self> {
        let log = PluginLog::new("fetchart");
        let sources = Self::get_sources(&log, &conf, cfg)?;
        if cfg.get("fetchart.remote_priority").is_some() {
            log.warning(
                "The `fetch_art.remote_priority` configuration option has been deprecated. \
                 Instead, place `filesystem` at the end of your `sources` list.",
            );
        }
        let mut sources = sources;
        if cfg.bool("fetchart.remote_priority") {
            let (fs, others): (Vec<Source>, Vec<Source>) = sources
                .into_iter()
                .partition(|s| s.kind == SourceKind::FileSystem);
            sources = others.into_iter().chain(fs).collect();
        }
        Ok(FetchArt {
            conf,
            urls,
            sources,
            log,
            session: Session::plain("beets"),
        })
    }

    fn get_sources(log: &PluginLog, conf: &Conf, cfg: &Config) -> Result<Vec<Source>> {
        // The plugin is set up once per run in beets: log about missing
        // keys only the first time.
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let quiet = PluginLog::new("fetchart");
        quiet.set_base_level(u8::MAX);
        let log = if LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            &quiet
        } else {
            log
        };
        let available: Vec<(String, String)> = ALL_SOURCES
            .iter()
            .filter(|k| k.available(log, conf))
            .flat_map(|k| {
                k.criteria()
                    .iter()
                    .map(move |c| (k.id().to_string(), c.to_string()))
            })
            .collect();
        let pairs =
            sanitize_pairs(&source_pairs(cfg), &available).map_err(|e| anyhow::anyhow!("{e}"))?;
        if pairs.is_empty() {
            bail!("fetchart: no sources defined in config");
        }
        Ok(pairs
            .into_iter()
            .map(|(id, c)| Source {
                kind: *ALL_SOURCES.iter().find(|k| k.id() == id).unwrap(),
                match_by: vec![c],
            })
            .collect())
    }

    /// `_logged_get`: a GET logged at debug level ("getting URL: ...").
    fn request(
        &self,
        url: &str,
        params: &[(&str, String)],
        headers: &[(&str, &str)],
        message: &str,
    ) -> Result<Response, HttpError> {
        let full = format_url(url, params);
        self.log.debug(&format!("{message}: {full}"));
        self.session.get_any(
            &full,
            Request {
                headers,
                allow_redirects: true,
                ..Default::default()
            },
        )
    }

    fn get(&self, url: &str, params: &[(&str, String)]) -> Result<Response, HttpError> {
        self.request(url, params, &[], "getting URL")
    }

    // ---- candidates per source

    fn candidates(
        &self,
        ctx: &PluginCtx,
        source: &Source,
        album: &Album,
        paths: Option<&[PathBuf]>,
    ) -> Vec<Candidate> {
        match source.kind {
            SourceKind::FileSystem => self.filesystem(ctx, paths),
            SourceKind::CoverArt => self.coverart(source, album),
            SourceKind::ITunes => self.itunes(album),
            SourceKind::AlbumArt => self.albumart(album),
            SourceKind::Amazon => self.amazon(album),
            SourceKind::Wikipedia => self.wikipedia(album),
            SourceKind::Google => self.google(album),
            SourceKind::FanartTV => self.fanarttv(album),
            SourceKind::LastFM => self.lastfm(album),
            SourceKind::Spotify => self.spotify(ctx, album),
            SourceKind::CoverArtUrl => self.cover_art_url(ctx, album),
        }
    }

    /// `FileSystem.filename_priority`.
    fn filename_priority(filename: &str, cover_names: &[String]) -> Vec<usize> {
        cover_names
            .iter()
            .enumerate()
            .filter(|(_, x)| filename.contains(x.as_str()))
            .map(|(i, _)| i)
            .collect()
    }

    fn filesystem(&self, ctx: &PluginCtx, paths: Option<&[PathBuf]>) -> Vec<Candidate> {
        let mut out = vec![];
        let Some(paths) = paths.filter(|p| !p.is_empty()) else {
            return out;
        };
        let names: Vec<String> = self
            .conf
            .cover_names
            .iter()
            .map(|n| regex::escape(n))
            .collect();
        let cover_pat = regex::Regex::new(&format!(r"(?i)(\b|_)({})(\b|_)", names.join("|")))
            .expect("escaped names");
        let filter = WalkFilter::from_config(ctx.cfg);
        let exts: Vec<String> = image_extensions().map(|e| format!(".{e}")).collect();
        for path in paths {
            if !path.is_dir() {
                continue;
            }
            // Only the album directory itself (not subdirectories), with
            // `ignore`/`ignore_hidden` applied and names sorted as beets'
            // `sorted_walk`.
            let mut files: Vec<PathBuf> = std::fs::read_dir(path)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| !filter.skips(&e.file_name()))
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default();
            files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
            let mut images: Vec<PathBuf> = files
                .into_iter()
                .filter(|p| {
                    let suffix = p
                        .extension()
                        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
                        .unwrap_or_default();
                    exts.contains(&suffix) && p.is_file()
                })
                .collect();
            images.sort_by_key(|p| {
                Self::filename_priority(
                    &p.file_name().unwrap_or_default().to_string_lossy(),
                    &self.conf.cover_names,
                )
            });
            let mut remaining = vec![];
            for p in images {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                if !names.is_empty() && cover_pat.is_match(&stem) {
                    let mut c = Candidate::file("filesystem", p.clone(), MetadataMatch::Exact);
                    c.pre_log.push(format!(
                        "using well-named art file {}",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    out.push(c);
                } else {
                    remaining.push(p);
                }
            }
            if let Some(fallback) = &self.conf.fallback {
                let mut c =
                    Candidate::file("filesystem", fallback.clone(), MetadataMatch::Fallback);
                c.pre_log
                    .push(format!("using fallback art file {}", fallback.display()));
                out.push(c);
            }
            if !self.conf.cautious
                && let Some(first) = remaining.first()
            {
                let mut c = Candidate::file("filesystem", first.clone(), MetadataMatch::Fallback);
                c.pre_log
                    .push(format!("using fallback art file {}", first.display()));
                out.push(c);
            }
        }
        out
    }

    fn coverart(&self, source: &Source, album: &Album) -> Vec<Candidate> {
        const LEGACY: &[(&str, &str)] = &[("250", "small"), ("500", "large")];
        let preferred = [250, 500, 1200]
            .contains(&self.conf.maxwidth)
            .then(|| self.conf.maxwidth.to_string());
        let image_urls = |url: &str| -> Vec<String> {
            let resp = match self.get(url, &[]) {
                Ok(r) => r,
                Err(_) => {
                    self.log.debug(&format!(
                        "{}: error receiving response",
                        SourceKind::CoverArt.name()
                    ));
                    return vec![];
                }
            };
            let data = match resp.json() {
                Ok(d) => d,
                Err(_) => {
                    self.log.debug(&format!(
                        "{}: error loading response: {}",
                        SourceKind::CoverArt.name(),
                        resp.text(false)
                    ));
                    return vec![];
                }
            };
            let mut out = vec![];
            for item in data
                .get("images")
                .and_then(J::as_array)
                .into_iter()
                .flatten()
            {
                let Some(types) = item.get("types") else {
                    continue;
                };
                let is_front = match types {
                    J::Array(a) => a.iter().any(|t| t == "Front"),
                    J::String(s) => s.contains("Front"),
                    _ => false,
                };
                if !is_front {
                    continue;
                }
                let Some(mut image_url) = item.get("image").and_then(J::as_str).map(String::from)
                else {
                    continue;
                };
                if let (Some(pw), Some(J::Object(thumbs))) = (&preferred, item.get("thumbnails")) {
                    let legacy = LEGACY.iter().find(|(k, _)| k == pw).map(|(_, v)| *v);
                    if let Some(u) = thumbs
                        .get(pw.as_str())
                        .or_else(|| legacy.and_then(|l| thumbs.get(l)))
                        .and_then(J::as_str)
                    {
                        image_url = u.to_string();
                    }
                }
                out.push(image_url);
            }
            out
        };
        let mut out = vec![];
        let mbid = album.str("mb_albumid");
        if source.match_by.iter().any(|m| m == "release") && !mbid.is_empty() {
            let url = format!("{}/release/{mbid}", self.urls.caa);
            out.extend(
                image_urls(&url)
                    .into_iter()
                    .map(|u| Candidate::url("coverart", u, Some(MetadataMatch::Exact))),
            );
        }
        let rgid = album.str("mb_releasegroupid");
        if source.match_by.iter().any(|m| m == "releasegroup") && !rgid.is_empty() {
            let url = format!("{}/release-group/{rgid}", self.urls.caa);
            out.extend(
                image_urls(&url)
                    .into_iter()
                    .map(|u| Candidate::url("coverart", u, Some(MetadataMatch::Fallback))),
            );
        }
        out
    }

    fn amazon(&self, album: &Album) -> Vec<Candidate> {
        let asin = album.str("asin");
        if asin.is_empty() {
            return vec![];
        }
        [1, 2]
            .iter()
            .map(|i| {
                Candidate::url(
                    "amazon",
                    format!("{}{asin}.{i:02}.LZZZZZZZ.jpg", self.urls.amazon),
                    Some(MetadataMatch::Exact),
                )
            })
            .collect()
    }

    fn albumart(&self, album: &Album) -> Vec<Candidate> {
        let asin = album.str("asin");
        if asin.is_empty() {
            return vec![];
        }
        let params = [("asin", asin.to_string())];
        let resp = match self.get(&self.urls.albumart, &params) {
            Ok(r) => {
                self.log.debug(&format!(
                    "scraped art URL: {}",
                    format_url(&self.urls.albumart, &params)
                ));
                r
            }
            Err(_) => {
                self.log.debug("error scraping art page");
                return vec![];
            }
        };
        let pat = regex::Regex::new(r#"href\s*=\s*"([^>"]*)"[^>]*title\s*=\s*"View larger image""#)
            .unwrap();
        match pat.captures(&resp.text(false)) {
            Some(m) => vec![Candidate::url(
                "albumart",
                &m[1],
                Some(MetadataMatch::Exact),
            )],
            None => {
                self.log.debug("no image found on page");
                vec![]
            }
        }
    }

    fn google(&self, album: &Album) -> Vec<Candidate> {
        let (aa, al) = (album.str("albumartist"), album.str("album"));
        if aa.is_empty() || al.is_empty() {
            return vec![];
        }
        let params = [
            ("key", self.conf.google_key.clone().unwrap_or_default()),
            ("cx", self.conf.google_engine.clone().unwrap_or_default()),
            ("q", format!("{aa},{al}")),
            ("searchType", "image".to_string()),
        ];
        let Ok(resp) = self.get(&self.urls.google, &params) else {
            self.log.debug("google: error receiving response");
            return vec![];
        };
        let Ok(data) = resp.json() else {
            self.log.debug(&format!(
                "google: error loading response: {}",
                resp.text(false)
            ));
            return vec![];
        };
        if let Some(err) = data.get("error") {
            let reason = err
                .pointer("/errors/0/reason")
                .and_then(J::as_str)
                .unwrap_or_default();
            self.log.debug(&format!("google fetchart error: {reason}"));
            return vec![];
        }
        data.get("items")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("link").and_then(J::as_str))
            .map(|l| Candidate::url("google", l, Some(MetadataMatch::Exact)))
            .collect()
    }

    fn fanarttv(&self, album: &Album) -> Vec<Candidate> {
        let rgid = album.str("mb_releasegroupid");
        if rgid.is_empty() {
            return vec![];
        }
        let mut headers = vec![("api-key", FANARTTV_PROJECT_KEY)];
        if let Some(k) = &self.conf.fanarttv_key {
            headers.push(("client-key", k.as_str()));
        }
        let url = format!("{}{rgid}", self.urls.fanarttv);
        let Ok(resp) = self.request(&url, &[], &headers, "getting URL") else {
            self.log.debug("fanart.tv: error receiving response");
            return vec![];
        };
        let Ok(data) = resp.json() else {
            self.log.debug(&format!(
                "fanart.tv: error loading response: {}",
                resp.text(false)
            ));
            return vec![];
        };
        if data.get("status").and_then(J::as_str) == Some("error") {
            let msg = data
                .get("error message")
                .and_then(J::as_str)
                .unwrap_or_default();
            let lower = msg.to_lowercase();
            if lower.contains("not found") {
                self.log.debug("fanart.tv: no image found");
            } else if lower.contains("api key") {
                self.log.warning(
                    "fanart.tv: Invalid API key given, please enter a valid one in your config file.",
                );
            } else {
                self.log
                    .debug(&format!("fanart.tv: error on request: {msg}"));
            }
            return vec![];
        }
        let mut matches: Vec<&J> = vec![];
        if let Some(albums) = data.get("albums").and_then(J::as_object) {
            for (mbid, art) in albums {
                match art.get("albumcover").and_then(J::as_array) {
                    Some(covers) if mbid == rgid => matches.extend(covers),
                    _ => self
                        .log
                        .debug("fanart.tv: unexpected mb_releasegroupid in response!"),
                }
            }
        }
        let likes = |x: &J| match x.get("likes") {
            Some(J::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
            Some(J::Number(n)) => n.as_i64().unwrap_or(0),
            _ => 0,
        };
        matches.sort_by_key(|x| std::cmp::Reverse(likes(x)));
        matches
            .into_iter()
            .filter_map(|m| m.get("url").and_then(J::as_str))
            .map(|u| Candidate {
                size: Some((1000, 1000)),
                ..Candidate::url("fanarttv", u, Some(MetadataMatch::Exact))
            })
            .collect()
    }

    fn itunes(&self, album: &Album) -> Vec<Candidate> {
        let (aa, al) = (album.str("albumartist"), album.str("album"));
        if aa.is_empty() || al.is_empty() {
            return vec![];
        }
        let term = format!("{aa} {al}");
        let params = [
            ("term", term.clone()),
            ("entity", "album".into()),
            ("media", "music".into()),
            ("limit", "200".into()),
        ];
        let r = match self.get(&self.urls.itunes, &params) {
            Ok(r) if r.status >= 400 => {
                let kind = if r.status < 500 { "Client" } else { "Server" };
                self.log.debug(&format!(
                    "iTunes search failed: {} {kind} Error: {} for url: {}",
                    r.status,
                    super::requests::reason(r.status),
                    format_url(&self.urls.itunes, &params)
                ));
                return vec![];
            }
            Ok(r) => r,
            Err(e) => {
                self.log.debug(&format!("iTunes search failed: {e}"));
                return vec![];
            }
        };
        let data: J = match serde_json::from_slice(&r.body) {
            Ok(d) => d,
            Err(e) => {
                self.log
                    .debug(&format!("Could not decode json response: {e}"));
                return vec![];
            }
        };
        let Some(candidates) = data.get("results").and_then(J::as_array) else {
            self.log.debug(&format!(
                "'results' not found in json. Fields are {} ",
                py_keys(&data)
            ));
            return vec![];
        };
        if candidates.is_empty() {
            self.log.debug(&format!(
                "iTunes search for {} got no results",
                py_repr(&term)
            ));
            return vec![];
        }
        let suffix = if self.conf.high_resolution {
            "10000x10000-999"
        } else {
            "1200x1200bb"
        };
        let mut out = vec![];
        let malformed = |key: &str, c: &J| {
            self.log.debug(&format!(
                "Malformed itunes candidate: {} not found in {}",
                py_repr(key),
                py_keys(c)
            ));
        };
        for c in candidates {
            let (Some(artist), Some(collection)) = (c.get("artistName"), c.get("collectionName"))
            else {
                malformed(
                    if c.get("artistName").is_none() {
                        "artistName"
                    } else {
                        "collectionName"
                    },
                    c,
                );
                continue;
            };
            if artist.as_str() == Some(aa) && collection.as_str() == Some(al) {
                match c.get("artworkUrl100").and_then(J::as_str) {
                    Some(u) => out.push(Candidate::url(
                        "itunes",
                        u.replace("100x100bb", suffix),
                        Some(MetadataMatch::Exact),
                    )),
                    None => malformed("artworkUrl100", c),
                }
            }
        }
        match candidates[0].get("artworkUrl100").and_then(J::as_str) {
            Some(u) => out.push(Candidate::url(
                "itunes",
                u.replace("100x100bb", suffix),
                Some(MetadataMatch::Fallback),
            )),
            None => malformed("artworkUrl100", candidates.last().unwrap()),
        }
        out
    }

    fn wikipedia(&self, album: &Album) -> Vec<Candidate> {
        let (aa, al) = (album.str("albumartist"), album.str("album"));
        if aa.is_empty() || al.is_empty() {
            return vec![];
        }
        let query = format!(
            r#"PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
                 PREFIX dbpprop: <http://dbpedia.org/property/>
                 PREFIX owl: <http://dbpedia.org/ontology/>
                 PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
                 PREFIX foaf: <http://xmlns.com/foaf/0.1/>

                 SELECT DISTINCT ?pageId ?coverFilename WHERE {{
                   ?subject owl:wikiPageID ?pageId .
                   ?subject dbpprop:name ?name .
                   ?subject rdfs:label ?label .
                   {{ ?subject dbpprop:artist ?artist }}
                     UNION
                   {{ ?subject owl:artist ?artist }}
                   {{ ?artist foaf:name "{artist}"@en }}
                     UNION
                   {{ ?artist dbpprop:name "{artist}"@en }}
                   ?subject rdf:type <http://dbpedia.org/ontology/Album> .
                   ?subject dbpprop:cover ?coverFilename .
                   FILTER ( regex(?name, "{album}", "i") )
                  }}
                 Limit 1"#,
            artist = py_title(aa),
            album = al
        );
        let json_headers = [("content-type", "application/json")];
        let Ok(resp) = self.request(
            &self.urls.dbpedia,
            &[
                ("format", "application/sparql-results+json".into()),
                ("timeout", "2500".into()),
                ("query", query),
            ],
            &json_headers,
            "getting URL",
        ) else {
            self.log.debug("dbpedia: error receiving response");
            return vec![];
        };
        let mut cover_filename = None;
        let mut page_id = None;
        let parsed = resp
            .json()
            .ok()
            .and_then(|d| d.pointer("/results/bindings").cloned());
        match parsed.as_ref().and_then(J::as_array) {
            Some(results) if !results.is_empty() => {
                let f = results[0]
                    .pointer("/coverFilename/value")
                    .and_then(J::as_str);
                let p = results[0].pointer("/pageId/value").and_then(J::as_str);
                match (f, p) {
                    (Some(f), Some(p)) => {
                        cover_filename = Some(format!("File:{f}"));
                        page_id = Some(p.to_string());
                    }
                    _ => self.log.debug(&format!(
                        "wikipedia: error scraping dbpedia response: {}",
                        resp.text(false)
                    )),
                }
            }
            Some(_) => self.log.debug("wikipedia: album not found on dbpedia"),
            None => self.log.debug(&format!(
                "wikipedia: error scraping dbpedia response: {}",
                resp.text(false)
            )),
        }
        let (Some(mut cover_filename), Some(page_id)) = (cover_filename, page_id) else {
            return vec![];
        };

        // DBpedia sometimes gives an incomplete name ("foo .bar"): find the
        // real one among the page's images.
        if let Some((lpart, rpart)) = cover_filename.rsplit_once(" .")
            && !rpart.contains('.')
        {
            self.log
                .debug("wikipedia: dbpedia provided incomplete cover_filename");
            let (lpart, rpart) = (lpart.to_string(), rpart.to_string());
            let Ok(resp) = self.request(
                &self.urls.wikipedia,
                &[
                    ("format", "json".into()),
                    ("action", "query".into()),
                    ("continue", String::new()),
                    ("prop", "images".into()),
                    ("pageids", page_id.clone()),
                ],
                &json_headers,
                "getting URL",
            ) else {
                self.log.debug("wikipedia: error receiving response");
                return vec![];
            };
            let images = resp.json().ok().and_then(|d| {
                d.get("query")?
                    .get("pages")?
                    .get(&page_id)?
                    .get("images")?
                    .as_array()
                    .cloned()
            });
            let Some(images) = images else {
                self.log
                    .debug("wikipedia: failed to retrieve a cover_filename");
                return vec![];
            };
            let pat = regex::Regex::new(&format!(
                r"^{}.*?\.{}",
                regex::escape(&lpart),
                regex::escape(&rpart)
            ))
            .unwrap();
            for img in &images {
                let Some(title) = img.get("title").and_then(J::as_str) else {
                    self.log
                        .debug("wikipedia: failed to retrieve a cover_filename");
                    return vec![];
                };
                if pat.is_match(title) {
                    cover_filename = title.to_string();
                    break;
                }
            }
        }

        let Ok(resp) = self.request(
            &self.urls.wikipedia,
            &[
                ("format", "json".into()),
                ("action", "query".into()),
                ("continue", String::new()),
                ("prop", "imageinfo".into()),
                ("iiprop", "url".into()),
                ("titles", cover_filename),
            ],
            &json_headers,
            "getting URL",
        ) else {
            self.log.debug("wikipedia: error receiving response");
            return vec![];
        };
        let mut out = vec![];
        let pages = resp
            .json()
            .ok()
            .and_then(|d| d.pointer("/query/pages").cloned());
        match pages.as_ref().and_then(J::as_object) {
            Some(pages) => {
                for page in pages.values() {
                    match page.pointer("/imageinfo/0/url").and_then(J::as_str) {
                        Some(u) => {
                            out.push(Candidate::url("wikipedia", u, Some(MetadataMatch::Exact)))
                        }
                        None => {
                            self.log.debug("wikipedia: error scraping imageinfo");
                            break;
                        }
                    }
                }
            }
            None => self.log.debug("wikipedia: error scraping imageinfo"),
        }
        out
    }

    fn lastfm(&self, album: &Album) -> Vec<Candidate> {
        const SIZES: &[(&str, (u32, u32))] = &[
            ("mega", (300, 300)),
            ("extralarge", (300, 300)),
            ("large", (174, 174)),
            ("medium", (64, 64)),
            ("small", (34, 34)),
        ];
        let mbid = album.str("mb_albumid");
        if mbid.is_empty() {
            return vec![];
        }
        let params = [
            ("method", "album.getinfo".to_string()),
            ("api_key", self.conf.lastfm_key.clone().unwrap_or_default()),
            ("mbid", mbid.to_string()),
            ("format", "json".to_string()),
        ];
        let Ok(resp) = self.get(&self.urls.lastfm, &params) else {
            self.log.debug("lastfm: error receiving response");
            return vec![];
        };
        let Ok(data) = resp.json() else {
            self.log.debug(&format!(
                "lastfm: error loading response: {}",
                resp.text(false)
            ));
            return vec![];
        };
        if let Some(err) = data.get("error") {
            if err.as_i64() == Some(6) {
                self.log.debug(&format!("lastfm: no results for {mbid}"));
            } else {
                let msg = match data.get("message") {
                    Some(J::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                self.log
                    .error(&format!("lastfm: failed to get album info: {msg} ({err})"));
            }
            return vec![];
        }
        let mut images: Vec<(String, String)> = vec![];
        for img in data
            .pointer("/album/image")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(size), Some(text)) = (
                img.get("size").and_then(J::as_str),
                img.get("#text").and_then(J::as_str),
            ) {
                images.retain(|(s, _)| s != size);
                images.push((size.to_string(), text.to_string()));
            }
        }
        SIZES
            .iter()
            .filter_map(|(name, dims)| {
                let (_, url) = images.iter().find(|(s, _)| s == name)?;
                Some(Candidate {
                    size: Some(*dims),
                    ..Candidate::url("lastfm", url.clone(), None)
                })
            })
            .collect()
    }

    fn spotify(&self, ctx: &PluginCtx, album: &Album) -> Vec<Candidate> {
        let first = album
            .id
            .and_then(|id| ctx.lib.album_items(id).ok())
            .and_then(|items| items.into_iter().next());
        let id = first.as_ref().and_then(|i| i.get("spotify_album_id"));
        let Some(id) = id else {
            self.log.debug("Fetchart: no Spotify album ID found");
            return vec![];
        };
        let url = format!("{}{}", self.urls.spotify, id.raw());
        let resp = match self.session.get_any(&url, Request::default()) {
            Ok(r) if r.status >= 400 => {
                let kind = if r.status < 500 { "Client" } else { "Server" };
                self.log.debug(&format!(
                    "Error: {} {kind} Error: {} for url: {url}",
                    r.status,
                    super::requests::reason(r.status)
                ));
                return vec![];
            }
            Ok(r) => r,
            Err(e) => {
                self.log.debug(&format!("Error: {e}"));
                return vec![];
            }
        };
        let html = scraper::Html::parse_document(&resp.text(false));
        let sel = scraper::Selector::parse(r#"meta[property="og:image"]"#).unwrap();
        match html
            .select(&sel)
            .next()
            .and_then(|m| m.value().attr("content"))
        {
            Some(u) => vec![Candidate::url("spotify", u, Some(MetadataMatch::Exact))],
            None => {
                self.log
                    .debug("Spotify: Unexpected response, og:image tag missing");
                vec![]
            }
        }
    }

    fn cover_art_url(&self, ctx: &PluginCtx, album: &Album) -> Vec<Candidate> {
        let shown = art::show_album(ctx, album);
        let image_url = match album.get("cover_art_url").filter(|v| !v.raw().is_empty()) {
            Some(v) => v.raw(),
            None => {
                let first = album
                    .id
                    .and_then(|id| ctx.lib.album_items(id).ok())
                    .and_then(|items| items.into_iter().next());
                match first.and_then(|i| i.get("cover_art_url")) {
                    Some(v) => v.raw(),
                    None => {
                        self.log
                            .debug(&format!("Cover art URL not found for {shown}"));
                        return vec![];
                    }
                }
            }
        };
        self.log
            .debug(&format!("Cover art URL {image_url} found for {shown}"));
        if image_url.is_empty() {
            self.log
                .debug(&format!("Cover art URL not found for {shown}"));
            return vec![];
        }
        vec![Candidate::url(
            "cover_art_url",
            image_url,
            Some(MetadataMatch::Exact),
        )]
    }

    // ---- downloading

    /// `RemoteArtSource.fetch_image`: download to a temporary file, trusting
    /// the data's magic bytes over the server's Content-Type.
    fn fetch_image(&self, c: &mut Candidate) {
        let Some(url) = c.url.clone() else {
            return;
        };
        let resp = match self.request(&url, &[], &[], "downloading image") {
            Ok(r) => r,
            Err(e) => {
                self.log.debug(&format!("error fetching art: {e}"));
                return;
            }
        };
        if resp.body.len() < 32 {
            // Not enough data: a corrupt image.
            return;
        }
        let ct = resp.content_type().map(str::to_string);
        let real_ct = image_mime_type(&resp.body[..32])
            .map(str::to_string)
            .or(ct.clone());
        let Some((real, exts)) = real_ct
            .as_deref()
            .and_then(|r| CONTENT_TYPES.iter().find(|(t, _)| *t == r))
        else {
            self.log.debug(&format!(
                "not a supported image: {}",
                real_ct.as_deref().unwrap_or("unknown content type")
            ));
            return;
        };
        let ext = format!(".{}", exts[0]);
        if ct.as_deref() != Some(*real) {
            self.log.warning(&format!(
                "Server specified {}, but returned a {real} image. Correcting the extension to {ext}",
                ct.as_deref().unwrap_or("None")
            ));
        }
        let filename = artresizer::temp_filename("fetchart_", Some(Path::new(&format!("x{ext}"))));
        if let Err(e) = std::fs::write(&filename, &resp.body) {
            self.log.debug(&format!("error fetching art: {e}"));
            return;
        }
        self.log
            .debug(&format!("downloaded art to: {}", filename.display()));
        c.path = Some(filename);
    }

    /// `cleanup`: remove a rejected download.
    fn cleanup(&self, c: &Candidate) {
        if let Some(p) = &c.path
            && is_temp(p)
            && let Err(e) = std::fs::remove_file(p)
        {
            self.log.debug(&format!("error cleaning up tmp art: {e}"));
        }
    }

    /// `art_for_album`: the first valid candidate from the sources in
    /// order, processed as configured. With `local_only`, only local
    /// sources are tried.
    pub fn art_for_album(
        &self,
        ctx: &PluginCtx,
        album: &Album,
        paths: Option<&[PathBuf]>,
        local_only: bool,
    ) -> Option<Candidate> {
        let mut out = None;
        for source in &self.sources {
            if !(source.kind.local() || !local_only) {
                continue;
            }
            self.log.debug(&format!(
                "trying source {} for album {} - {}",
                source.description(),
                album.str("albumartist"),
                album.str("album")
            ));
            for mut c in self.candidates(ctx, source, album, paths) {
                for line in std::mem::take(&mut c.pre_log) {
                    self.log.debug(&line);
                }
                if !source.kind.local() {
                    self.fetch_image(&mut c);
                }
                if c.check(&self.log, &self.conf, &[]) != ImageAction::Bad {
                    self.log.debug(&format!(
                        "using {} image {}",
                        if source.kind.local() {
                            "local"
                        } else {
                            "remote"
                        },
                        c.path.as_deref().unwrap_or(Path::new("")).display()
                    ));
                    out = Some(c);
                    break;
                }
                if !source.kind.local() {
                    self.cleanup(&c);
                }
            }
            if out.is_some() {
                break;
            }
        }
        if let Some(c) = out.as_mut() {
            c.resize(&self.log, &self.conf);
        }
        out
    }

    /// `_set_art`: set the candidate as the album's art and store it.
    /// False if the art file couldn't be written.
    fn set_art(&self, ctx: &PluginCtx, album: &mut Album, c: &Candidate, copy: bool) -> bool {
        let Some(path) = &c.path else {
            return false;
        };
        let r = art::set_art(ctx, album, path, copy);
        // Downloads and processed images are temporary.
        if is_temp(path) && path.exists() {
            let _ = std::fs::remove_file(path);
        }
        if let Err(e) = r {
            self.log.warning(&format!(
                "fetchart: could not write art for {} - {}: {e:#}",
                album.str("albumartist"),
                album.str("album")
            ));
            return false;
        }
        if self.conf.store_source {
            self.log.debug(&format!(
                "Storing art_source for {} - {}",
                album.str("albumartist"),
                album.str("album")
            ));
            album.set("art_source", Value::Str(c.source_name.to_string()));
        }
        if let Err(e) = ctx.lib.store(album) {
            self.log.error(&format!("{e:#}"));
        }
        true
    }

    fn is_candidate_fallback(&self, c: &Candidate) -> bool {
        match (&c.path, &self.conf.fallback) {
            (Some(p), Some(f)) => art::samefile(p, f),
            _ => false,
        }
    }

    /// `fetch_art`, the import stage: find art for an imported album.
    pub fn fetch_art(
        &self,
        ctx: &PluginCtx,
        album: &Album,
        paths: &[PathBuf],
        asis: bool,
    ) -> Option<Candidate> {
        let art = album.path_field("artpath");
        if !art.as_os_str().is_empty() && art.is_file() {
            // Probably a re-import: the album already has art.
            return None;
        }
        // As-is imports only search the web with `fetch_for_asis`.
        let local = asis && !self.conf.fetch_for_asis;
        self.art_for_album(ctx, album, Some(paths), local)
    }

    /// `assign_art`, on `import_task_files`: place the art found for the
    /// album. When the source files go (move, or `import.delete`), the
    /// art is moved too.
    pub fn assign_art(&self, ctx: &PluginCtx, album: &mut Album, c: &Candidate, op: FileOp) {
        let removal = ctx.cfg.bool("import.delete") || op == FileOp::Move;
        // The configured fallback image is never moved away.
        let copy = !removal || self.is_candidate_fallback(c);
        self.set_art(ctx, album, c, copy);
        // (The source directory is pruned by the importer.)
    }

    /// `batch_fetch_art`: the `fetchart` command.
    pub fn batch_fetch_art(
        &self,
        ctx: &PluginCtx,
        albums: Vec<Album>,
        force: bool,
        quiet: bool,
    ) -> Result<()> {
        use crate::ui::colorize;
        for mut album in albums {
            let shown = art::show_album(ctx, &album);
            let art_path = album.path_field("artpath").to_path_buf();
            if !art_path.as_os_str().is_empty() && !force && art_path.is_file() {
                if !quiet {
                    println!(
                        "{shown}: {}",
                        colorize("text_highlight_minor", "has album art")
                    );
                }
                continue;
            }
            // Normally look in the album's folder first; when forcing,
            // only the web sources.
            let local_paths: Option<Vec<PathBuf>> = if force {
                None
            } else {
                let items = ctx.lib.album_items(album.id.unwrap())?;
                Some(album_dir(&items).into_iter().collect())
            };
            let message = match self.art_for_album(ctx, &album, local_paths.as_deref(), false) {
                Some(c) => {
                    if self.set_art(ctx, &mut album, &c, false) {
                        colorize("text_success", "found album art")
                    } else {
                        colorize("text_error", "error writing album art")
                    }
                }
                None => colorize("text_error", "no art found"),
            };
            println!("{shown}: {message}");
        }
        Ok(())
    }
}

impl Plugin for FetchArt {
    fn name(&self) -> &'static str {
        "fetchart"
    }

    fn album_imported(
        &self,
        ctx: &PluginCtx,
        album: &mut Album,
        _items: &mut [Item],
        source_dir: &Path,
        op: FileOp,
        asis: bool,
    ) -> Result<()> {
        if !self.conf.auto {
            return Ok(());
        }
        self.log.at_level(WARNING, || {
            let paths = [source_dir.to_path_buf()];
            if let Some(c) = self.fetch_art(ctx, album, &paths, asis) {
                self.assign_art(ctx, album, &c, op);
            }
        });
        Ok(())
    }
}

/// The `fetchart` command's options.
pub struct CmdOpts {
    pub force: bool,
    pub quiet: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// The `fetchart` command.
pub fn command(ctx: &PluginCtx, lib: &Library, opts: CmdOpts) -> Result<()> {
    let fa = FetchArt::new(ctx.cfg)?;
    fa.log.set_base_level(INFO);
    let mut albums = lib.albums(&Query::parse(Kind::Album, &opts.query)?)?;
    albums.truncate(opts.limit.unwrap_or(usize::MAX));
    fa.batch_fetch_art(ctx, albums, opts.force, opts.quiet)
}

#[cfg(test)]
#[path = "fetchart_tests.rs"]
mod tests;
