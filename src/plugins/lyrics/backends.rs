//! The lyrics sources: beets' `Backend` subclasses.

use super::LyricsPlugin;
use super::html::{self, Soup};
use super::text::{INSTRUMENTAL_LYRICS, Lyrics, line_parts, splitlines};
use crate::plugins::requests::{HttpError, format_url, netloc, quote, quote_plus};
use fancy_regex::Regex as FRegex;
use regex::Regex;
use serde_json::Value as J;
use std::sync::LazyLock;

type Fetched = Result<Option<Lyrics>, HttpError>;

/// The tags stripped from Musixmatch's HTML lyrics fragments.
static CLEAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^>]+>|</p>.*").unwrap());

/// `BACKEND_BY_NAME`, in beets' order.
pub const NAMES: &[&str] = &[
    "lrclib",
    "google",
    "genius",
    "tekstowo",
    "musixmatch",
    "lrcmux",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    LrcLib,
    Google,
    Genius,
    Tekstowo,
    MusiXmatch,
    LrcMux,
}

/// The service URLs (overridden in tests).
#[derive(Clone, Debug)]
pub struct Urls {
    pub lrclib: String,
    pub google: String,
    pub genius: String,
    pub tekstowo: String,
    pub musixmatch: String,
    pub translate: String,
}

impl Default for Urls {
    fn default() -> Self {
        Urls {
            lrclib: "https://lrclib.net/api".into(),
            google: "https://www.googleapis.com/customsearch/v1".into(),
            genius: "https://api.genius.com/search".into(),
            tekstowo: "https://www.tekstowo.pl".into(),
            musixmatch: "https://www.musixmatch.com/lyrics".into(),
            translate: "https://api.cognitive.microsofttranslator.com/translate".into(),
        }
    }
}

impl Backend {
    pub fn from_name(name: &str) -> Option<Backend> {
        Some(match name {
            "lrclib" => Backend::LrcLib,
            "google" => Backend::Google,
            "genius" => Backend::Genius,
            "tekstowo" => Backend::Tekstowo,
            "musixmatch" => Backend::MusiXmatch,
            "lrcmux" => Backend::LrcMux,
            _ => return None,
        })
    }

    /// The backend's name: its class name, lowercased.
    pub fn name(self) -> &'static str {
        match self {
            Backend::LrcLib => "lrclib",
            Backend::Google => "google",
            Backend::Genius => "genius",
            Backend::Tekstowo => "tekstowo",
            Backend::MusiXmatch => "musixmatch",
            Backend::LrcMux => "lrcmux",
        }
    }

    /// The class name its log messages start with.
    pub fn class(self) -> &'static str {
        match self {
            Backend::LrcLib => "LRCLib",
            Backend::Google => "Google",
            Backend::Genius => "Genius",
            Backend::Tekstowo => "Tekstowo",
            Backend::MusiXmatch => "MusiXmatch",
            Backend::LrcMux => "LRCMux",
        }
    }

    pub fn fetch(
        self,
        p: &LyricsPlugin,
        artist: &str,
        title: &str,
        album: &str,
        length: i64,
    ) -> Fetched {
        match self {
            Backend::LrcLib => lrclib(p, artist, title, album, length),
            Backend::MusiXmatch => musixmatch(p, artist, title),
            Backend::LrcMux => lrcmux(p, artist, title, album, length),
            Backend::Google | Backend::Genius | Backend::Tekstowo => {
                self.search_fetch(p, artist, title)
            }
        }
    }

    // ---- SearchBackend

    /// `SearchBackend.check_match`: both the artist and the title are
    /// within `dist_thresh` of the result's.
    pub fn check_match(
        self,
        p: &LyricsPlugin,
        artist: &str,
        title: &str,
        r: &SearchResult,
    ) -> bool {
        let dist = crate::autotag::string_dist(Some(artist), Some(&r.artist))
            .max(crate::autotag::string_dist(Some(title), Some(&r.title)));
        let dist = py_round2(dist);
        let thresh = p.conf.dist_thresh;
        if dist <= thresh {
            return true;
        }
        if (dist - thresh).abs() <= 0.4 {
            p.debug(&format!(
                "{}: ({}, {}) does not match ({artist}, {title}) but dist was close: {dist:.2}",
                self.class(),
                r.artist,
                r.title
            ));
        }
        false
    }

    fn search(
        self,
        p: &LyricsPlugin,
        artist: &str,
        title: &str,
    ) -> Result<Vec<SearchResult>, HttpError> {
        match self {
            Backend::Genius => genius_search(p, artist, title),
            Backend::Tekstowo => tekstowo_search(p, artist, title),
            Backend::Google => google_search(p, artist, title),
            _ => Ok(vec![]),
        }
    }

    fn get_results(
        self,
        p: &LyricsPlugin,
        artist: &str,
        title: &str,
    ) -> Result<Vec<SearchResult>, HttpError> {
        let mut results: Vec<SearchResult> = self
            .search(p, artist, title)?
            .into_iter()
            .filter(|r| self.check_match(p, artist, title, r))
            .collect();
        if self == Backend::Google {
            // Try results from preferred sources first.
            results.sort_by(|a, b| {
                source_dist_factor(a.source()).total_cmp(&source_dist_factor(b.source()))
            });
        }
        Ok(results)
    }

    fn search_fetch(self, p: &LyricsPlugin, artist: &str, title: &str) -> Fetched {
        for result in self.get_results(p, artist, title)? {
            let html = if self == Backend::Google {
                // Google's `get_text` logs an error and moves on to the
                // next result.
                match p.get_text(self, &result.url, &[], false, false) {
                    Ok(h) => h,
                    Err(e) => {
                        p.warn_request_error(self.class(), &e);
                        continue;
                    }
                }
            } else {
                p.get_text(self, &result.url, &[], self == Backend::Genius, true)?
            };
            if html.is_empty() {
                continue;
            }
            if let Some(lyrics) = self.scrape(&html) {
                return Ok(Some(Lyrics::new(
                    lyrics,
                    Some(self.name()),
                    Some(&result.url),
                )));
            }
        }
        Ok(None)
    }

    pub fn scrape(self, html: &str) -> Option<String> {
        match self {
            Backend::Genius => genius_scrape(html),
            Backend::Tekstowo => tekstowo_scrape(html),
            Backend::Google => google_scrape(html),
            _ => None,
        }
    }
}

/// `round(x, 2)`.
fn py_round2(x: f64) -> f64 {
    let scaled = x * 100.0;
    let r = scaled.round();
    // Python rounds halves to even.
    let r = if (scaled - scaled.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        r - scaled.signum()
    } else {
        r
    };
    r / 100.0
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub artist: String,
    pub title: String,
    pub url: String,
}

impl SearchResult {
    pub fn source(&self) -> &str {
        netloc(&self.url)
    }
}

// ---------------------------------------------------------------- LRCLib

/// beets' `LRCLyrics`: an LRCLib candidate, ranked against the item.
#[derive(Debug)]
struct LrcLyrics {
    target_duration: f64,
    id: String,
    duration: f64,
    instrumental: bool,
    plain: Option<String>,
    synced: Option<String>,
}

/// Percentage tolerance for max duration difference between lyrics and item.
const DURATION_DIFF_TOLERANCE: f64 = 0.05;

impl LrcLyrics {
    /// Synced lyrics count only when their last timestamp fits the item.
    fn verify_synced(duration: f64, lyrics: Option<&str>) -> Option<String> {
        let lyrics = lyrics.filter(|l| !l.is_empty())?;
        let last = *splitlines(lyrics).last()?;
        let (ts, _) = line_parts(last);
        if ts.is_empty() {
            return None;
        }
        let (mm, ss) = ts.trim_matches(['[', ']']).split_once(':')?;
        let secs = 60.0 * mm.parse::<f64>().ok()? + ss.parse::<f64>().ok()?;
        (secs <= duration).then(|| lyrics.to_string())
    }

    fn make(c: &J, target: f64) -> LrcLyrics {
        let s = |k: &str| c.get(k).and_then(J::as_str).map(String::from);
        LrcLyrics {
            target_duration: target,
            id: match c.get("id") {
                Some(J::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            },
            duration: c.get("duration").and_then(J::as_f64).unwrap_or(0.0),
            instrumental: c.get("instrumental").and_then(J::as_bool).unwrap_or(false),
            plain: s("plainLyrics"),
            synced: Self::verify_synced(target, c.get("syncedLyrics").and_then(J::as_str)),
        }
    }

    fn duration_dist(&self) -> f64 {
        (self.duration - self.target_duration).abs()
    }

    fn has_text(&self) -> bool {
        let some = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.is_empty());
        self.instrumental || some(&self.plain) || some(&self.synced)
    }

    fn is_valid(&self) -> bool {
        self.has_text() && self.duration_dist() <= self.target_duration * DURATION_DIFF_TOLERANCE
    }

    /// Closest duration, synced lyrics first.
    fn dist(&self) -> (bool, f64) {
        (self.synced.is_none(), self.duration_dist())
    }

    fn get_text(&self, want_synced: bool) -> String {
        if self.instrumental {
            return INSTRUMENTAL_LYRICS.into();
        }
        if want_synced && let Some(s) = &self.synced {
            return splitlines(s)
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n");
        }
        if let Some(p) = self.plain.as_deref().filter(|p| !p.is_empty()) {
            return p.to_string();
        }
        // 'plainLyrics' may be null while synced lyrics are available: use
        // those without their timestamps.
        if let Some(s) = &self.synced {
            return splitlines(s)
                .into_iter()
                .map(|l| line_parts(l).1)
                .collect::<Vec<_>>()
                .join("\n");
        }
        String::new()
    }
}

fn pick_best(cands: Vec<LrcLyrics>) -> Option<LrcLyrics> {
    let mut best: Option<LrcLyrics> = None;
    for c in cands.into_iter().filter(LrcLyrics::is_valid) {
        let better = match &best {
            None => true,
            Some(b) => c.dist().partial_cmp(&b.dist()) == Some(std::cmp::Ordering::Less),
        };
        if better {
            best = Some(c);
        }
    }
    best
}

/// LRCLib: the search results first (ranked by our own rules), then the
/// `/get` endpoint's single match.
fn lrclib(p: &LyricsPlugin, artist: &str, title: &str, album: &str, length: i64) -> Fetched {
    let b = Backend::LrcLib;
    let base = &p.urls.lrclib;
    let get_url = format!("{base}/get");
    let mut params = vec![
        ("artist_name", artist.to_string()),
        ("track_name", title.to_string()),
    ];
    let search = p.get_json(b, &format!("{base}/search"), &params, &[])?;
    params.push(("duration", length.to_string()));
    if !album.is_empty() {
        params.push(("album_name", album.to_string()));
    }
    let target = length as f64;
    let found = |group: Vec<J>| {
        let cands = group.iter().map(|c| LrcLyrics::make(c, target)).collect();
        pick_best(cands).map(|best| {
            Lyrics::new(
                best.get_text(p.conf.synced),
                Some(b.name()),
                Some(&format!("{get_url}/{}", best.id)),
            )
        })
    };
    let search = match search {
        J::Array(a) => a,
        _ => vec![],
    };
    if let Some(l) = found(search) {
        return Ok(Some(l));
    }
    match p.get_json(b, &get_url, &params, &[]) {
        Ok(rec) => Ok(found(vec![rec])),
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

// ------------------------------------------------------------ MusiXmatch

/// `MusiXmatch.encode`: the site's URL slug for a name.
pub fn musixmatch_encode(text: &str) -> String {
    static REPLACEMENTS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
        [
            ("['\u{2018}\u{2019}]", "-"),
            (r"\s+", "-"),
            ("<", "Less_Than"),
            (">", "Greater_Than"),
            ("#", "Number_"),
            (r"[\[\{]", "("),
            (r"[\]\}]", ")"),
        ]
        .into_iter()
        .map(|(p, r)| (Regex::new(p).unwrap(), r))
        .collect()
    });
    let mut text = text.to_string();
    for (re, rep) in REPLACEMENTS.iter() {
        text = re.replace_all(&text, regex::NoExpand(rep)).into_owned();
    }
    quote(&deunicode::deunicode(&text), "/")
}

fn musixmatch(p: &LyricsPlugin, artist: &str, title: &str) -> Fetched {
    let b = Backend::MusiXmatch;
    let url = format!(
        "{}/{}/{}",
        p.urls.musixmatch,
        musixmatch_encode(artist),
        musixmatch_encode(title)
    );
    let html = p.get_text(b, &url, &[], false, true)?;
    if html.contains("We detected that your IP is blocked") {
        p.error(&format!("{}: Failed: Blocked IP address", b.class()));
        return Ok(None);
    }
    let parts: Vec<String> = html
        .split("<p class=\"mxm-lyrics__content")
        .map(|part| CLEAN.replace_all(part, "").into_owned())
        .collect();
    let lyrics = parts
        .join("\n")
        .trim_matches([',', '"'])
        .replace("\\n", "\n");
    // Pages for missing songs, and what's left after a CAPTCHA.
    if lyrics.contains("Instant lyrics for all your music.")
        || lyrics.contains("Lyrics | Musixmatch")
    {
        return Ok(None);
    }
    Ok(Some(Lyrics::new(lyrics, Some(b.name()), Some(&url))))
}

// ---------------------------------------------------------------- Genius

fn genius_search(
    p: &LyricsPlugin,
    artist: &str,
    title: &str,
) -> Result<Vec<SearchResult>, HttpError> {
    let b = Backend::Genius;
    let auth = format!("Bearer {}", p.conf.genius_api_key);
    let params = [("q", format!("{artist} {title}"))];
    let data = match p.get_json(b, &p.urls.genius, &params, &[("Authorization", &auth)]) {
        Ok(d) => d,
        // The API's error responses are JSON too.
        Err(HttpError::Status { body, .. }) => {
            serde_json::from_slice(&body).map_err(|_| HttpError::Json)?
        }
        Err(e) => return Err(e),
    };
    if data.get("response").is_none() {
        let meta = data.get("meta").ok_or(HttpError::Json)?;
        let message = meta.get("message").and_then(J::as_str).unwrap_or_default();
        let status = meta.get("status").cloned().unwrap_or(J::Null);
        if status.as_u64() == Some(429) {
            return Err(HttpError::Other(format!(
                "HTTP Error: 429 Too Many Requests. {message}"
            )));
        }
        return Err(HttpError::Other(format!("{message} Status: {status}")));
    }
    let hits = data["response"]["hits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(hits
        .iter()
        .map(|hit| {
            let r = &hit["result"];
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_string();
            SearchResult {
                artist: s("artist_names"),
                title: s("title"),
                url: s("url"),
            }
        })
        .collect())
}

/// Genius embeds the lyrics' HTML in the page's JSON data.
pub fn genius_scrape(html: &str) -> Option<String> {
    static IN_JSON: LazyLock<FRegex> =
        LazyLock::new(|| FRegex::new(r#"(?<=.\\"html\\":\\").*?(?=(?<!\\)\\")"#).unwrap());
    static BACKSLASH: LazyLock<FRegex> = LazyLock::new(|| FRegex::new(r"\\(?=[^\\])").unwrap());
    static ESCAPED_QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\\+""#).unwrap());
    let m = IN_JSON.find(html).ok()??;
    let text = BACKSLASH.replace_all(m.as_str(), "").replace("\\n", "\n");
    let lyrics = Soup::parse(&html::pre_process(&text))
        .get_text()
        .trim()
        .to_string();
    Some(ESCAPED_QUOTE.replace_all(&lyrics, "\"").into_owned())
}

// -------------------------------------------------------------- Tekstowo

/// Python's `str.title()`.
fn py_title(s: &str) -> String {
    let mut out = String::new();
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

pub fn tekstowo_search_url(base: &str, artist: &str, title: &str) -> String {
    let artistitle = format!("{} {}", py_title(artist), py_title(title));
    format!(
        "{base}/szukaj,{}.html",
        quote_plus(&deunicode::deunicode(&artistitle))
    )
}

fn tekstowo_search(
    p: &LyricsPlugin,
    artist: &str,
    title: &str,
) -> Result<Vec<SearchResult>, HttpError> {
    let b = Backend::Tekstowo;
    // beets passes the title as the artist and vice versa here.
    let url = tekstowo_search_url(&p.urls.tekstowo, title, artist);
    let html = p.get_text(b, &url, &[], false, true)?;
    Ok(tekstowo_results(&p.urls.tekstowo, &html))
}

pub fn tekstowo_results(base: &str, html: &str) -> Vec<SearchResult> {
    if html.is_empty() {
        return vec![];
    }
    let soup = Soup::parse(&html::pre_process(html));
    soup.select("div[class=flex-group] > a[title*=' - ']")
        .into_iter()
        .filter_map(|tag| {
            let (artist, title) = tag.value().attr("title")?.split_once(" - ")?;
            let href = tag.value().attr("href").unwrap_or_default();
            Some(SearchResult {
                artist: artist.to_string(),
                title: title.to_string(),
                url: format!("{base}{href}"),
            })
        })
        .collect()
}

pub fn tekstowo_scrape(html: &str) -> Option<String> {
    let soup = Soup::parse(&html::pre_process(html));
    let div = soup
        .select("div.song-text > div.inner-text")
        .into_iter()
        .next()?;
    Some(html::element_text(div))
}

// ---------------------------------------------------------------- Google

/// Some letras.mus.br pages which do not contain lyrics.
const EXCLUDE_PAGES: &[&str] = &[
    "significado.html",
    "traduccion.html",
    "traducao.html",
    "significados.html",
];

fn source_dist_factor(source: &str) -> f64 {
    match source {
        "www.azlyrics.com" => 0.5,
        "www.songlyrics.com" => 0.6,
        _ => 1.0,
    }
}

fn google_search(
    p: &LyricsPlugin,
    artist: &str,
    title: &str,
) -> Result<Vec<SearchResult>, HttpError> {
    let params = [
        ("key", p.conf.google_api_key.clone().unwrap_or_default()),
        ("cx", p.conf.google_engine_id.clone()),
        ("q", format!("{artist} {title}")),
        ("siteSearch", "www.musixmatch.com".to_string()),
        ("siteSearchFilter", "e".to_string()),
        ("excludeTerms", EXCLUDE_PAGES.join(", ")),
    ];
    let data = p.get_json(Backend::Google, &p.urls.google, &params, &[])?;
    let items = data
        .get("items")
        .and_then(J::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .map(|item| google_search_result(artist, title, item))
        .collect())
}

/// `Google.make_search_result`: the artist and title, from the result's
/// page title.
pub fn google_search_result(artist: &str, title: &str, item: &J) -> SearchResult {
    static NOISE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(paroles( et traduction| de chanson)?|letras?( de)?|liedtexte|dainų žodžiai|original song full text\.|official|20[12]\d version|(absolute |az)?lyrics( complete)?|www\S+|\S+\.(com|net|mus\.br))([^\w.]|$)",
        )
        .unwrap()
    });
    static PARTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r" +(?:[ :|-]+|par|by) +|, ").unwrap());
    let og_title = item
        .get("pagemap")
        .and_then(|m| m.get("metatags"))
        .and_then(|t| t.get(0))
        .and_then(|t| t.get("og:title"))
        .and_then(J::as_str)
        .filter(|t| !t.is_empty());
    let url_title = og_title.unwrap_or_else(|| item["title"].as_str().unwrap_or_default());
    let clean = NOISE.replace_all(url_title, "");
    let clean = clean.trim_matches([' ', '.', '-', '|']);
    let mut parts: Vec<&str> = vec![];
    for part in PARTS.split(clean) {
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    let (result_artist, result_title) = if parts.len() == 1 {
        let part = parts[0];
        let re = Regex::new(&format!(r"(?i)\W*({})\W*", regex::escape(title))).unwrap();
        match re.captures(part) {
            Some(m) => (part.replace(&m[0], ""), m[1].to_string()),
            None => (String::new(), part.to_string()),
        }
    } else {
        let closest = |target: &str| {
            let mut best = parts[0];
            let mut best_d = f64::INFINITY;
            for p in &parts {
                let d = crate::autotag::string_dist(Some(target), Some(p));
                if d < best_d {
                    best = p;
                    best_d = d;
                }
            }
            best.to_string()
        };
        (closest(artist), closest(title))
    };
    SearchResult {
        artist: result_artist,
        title: result_title,
        url: item["link"].as_str().unwrap_or_default().to_string(),
    }
}

/// The longest text in the page.
pub fn google_scrape(html: &str) -> Option<String> {
    let strings = Soup::parse(&html::google_pre_process(html)).stripped_strings();
    let mut best: Option<String> = None;
    for s in strings {
        if best
            .as_ref()
            .is_none_or(|b| s.chars().count() >= b.chars().count())
        {
            best = Some(s);
        }
    }
    best
}

// ---------------------------------------------------------------- LRCMux

fn lrcmux(p: &LyricsPlugin, artist: &str, title: &str, album: &str, length: i64) -> Fetched {
    let b = Backend::LrcMux;
    let url = format!("{}/get", p.conf.lrcmux_url.trim_end_matches('/'));
    let synced = p.conf.synced;
    let mut params = vec![
        ("artist", artist.to_string()),
        ("title", title.to_string()),
        ("duration", length.to_string()),
        ("format", if synced { "lrc" } else { "txt" }.to_string()),
        ("level", if synced { "line" } else { "none" }.to_string()),
    ];
    if !album.is_empty() {
        params.push(("album", album.to_string()));
    }
    if !p.conf.lrcmux_sources.is_empty() {
        params.push(("sources", p.conf.lrcmux_sources.join(",")));
    }
    let full_url = format_url(&url, &params);
    match p.get_text(b, &url, &params, false, true) {
        Ok(text) => {
            let text = text.trim();
            Ok((!text.is_empty()).then(|| Lyrics::new(text, Some(b.name()), Some(&full_url))))
        }
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn musixmatch_encoding() {
        assert_eq!(
            musixmatch_encode("If They\u{2019}re Shooting at You"),
            "If-They-re-Shooting-at-You"
        );
        assert_eq!(
            musixmatch_encode("\u{2018}Round Midnight"),
            "-Round-Midnight"
        );
        assert_eq!(musixmatch_encode("Don't Stop"), "Don-t-Stop");
    }

    #[test]
    fn title_case_as_python() {
        assert_eq!(py_title("hello wORLD it's 2x"), "Hello World It'S 2X");
    }

    #[test]
    fn rounding() {
        assert_eq!(py_round2(0.105), 0.1);
        assert_eq!(py_round2(0.1149), 0.11);
    }
}
