//! MusicBrainz web service client (release + recording search and lookup).

use crate::config::Config;
use anyhow::{Context, Result, bail};
use serde_json::Value as J;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const VA_ARTIST_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";
const USER_AGENT: &str = concat!(
    "beets-rs/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/beetbox/beets )"
);

#[derive(Debug, Clone, Default)]
pub struct TrackInfo {
    pub title: String,
    pub track_id: String,
    pub release_track_id: String,
    pub artist: String,
    pub artist_id: String,
    pub artist_sort: String,
    pub artist_credit: String,
    pub length: Option<f64>,
    /// 1-based position across the whole release.
    pub index: usize,
    pub medium: usize,
    pub medium_index: usize,
    pub medium_total: usize,
    pub disctitle: String,
    pub media: String,
    pub isrc: String,
    /// The first release a searched-for recording appears on (display only,
    /// for `import.singleton_album_disambig`).
    pub album: String,
}

#[derive(Debug, Clone, Default)]
pub struct AlbumInfo {
    pub album_id: String,
    pub album: String,
    pub artist: String,
    pub artist_id: String,
    pub artist_sort: String,
    pub artist_credit: String,
    pub va: bool,
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub original_year: i64,
    pub original_month: i64,
    pub original_day: i64,
    pub country: String,
    pub label: String,
    pub catalognum: String,
    pub albumtype: String,
    pub albumstatus: String,
    pub albumdisambig: String,
    pub releasegroup_id: String,
    pub releasegroupdisambig: String,
    pub media: String,
    pub mediums: usize,
    pub tracks: Vec<TrackInfo>,
}

pub struct MusicBrainz {
    agent: ureq::Agent,
    base: String,
    interval: Duration,
    /// The last request's time; locked while waiting for the next slot, so
    /// threads sharing the client share the rate limit.
    last: Mutex<Option<Instant>>,
    pub search_limit: usize,
    aliases: AliasPrefs,
}

/// Which MusicBrainz alias to use for names and titles: `import.languages`,
/// `import.ignored_alias_types` and `musicbrainz.aliases_as_credits`.
#[derive(Debug, Clone, Default)]
pub struct AliasPrefs {
    pub languages: Vec<String>,
    /// Lower-cased alias types never used.
    pub ignored_types: Vec<String>,
    /// Use the alias for the artist credit too, not just name and sort name.
    pub as_credits: bool,
}

impl AliasPrefs {
    pub fn from_config(cfg: &Config) -> Self {
        AliasPrefs {
            languages: cfg.list("import.languages"),
            ignored_types: cfg
                .list("import.ignored_alias_types")
                .iter()
                .map(|t| t.to_lowercase())
                .collect(),
            as_credits: cfg.bool("musicbrainz.aliases_as_credits"),
        }
    }

    /// beets' `_preferred_alias`: the first primary alias for the first
    /// configured locale that has one, skipping ignored alias types.
    fn preferred<'j>(&self, obj: &'j J) -> Option<&'j J> {
        let aliases = obj.get("aliases")?.as_array()?;
        self.languages.iter().find_map(|locale| {
            aliases.iter().find(|al| {
                al.get("locale").and_then(|l| l.as_str()) == Some(locale.as_str())
                    && al.get("primary").and_then(|p| p.as_bool()) == Some(true)
                    && !self.ignored_types.contains(&s(al, "type").to_lowercase())
            })
        })
    }

    /// An object's title, or its preferred alias's name.
    fn title(&self, obj: &J) -> String {
        match self.preferred(obj) {
            Some(al) => s(al, "name"),
            None => s(obj, "title"),
        }
    }
}

fn s(v: &J, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Lucene-escape a search term.
fn escape(term: &str) -> String {
    let mut out = String::new();
    for c in term.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn parse_date(d: &str) -> (i64, i64, i64) {
    let mut it = d.split('-').map(|p| p.parse::<i64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Flatten an artist-credit list into (display name, first artist id, sort
/// name, credit name), using each artist's preferred alias for the name and
/// sort name (and the credit, with `aliases_as_credits`).
fn artist_credit(ac: Option<&J>, prefs: &AliasPrefs) -> (String, String, String, String) {
    let Some(list) = ac.and_then(|a| a.as_array()) else {
        return Default::default();
    };
    let (mut name, mut sort, mut credit, mut id) =
        (String::new(), String::new(), String::new(), String::new());
    for c in list {
        let artist = c.get("artist").cloned().unwrap_or(J::Null);
        let join = s(c, "joinphrase");
        let alias = prefs.preferred(&artist);
        let named = alias.unwrap_or(&artist);
        let aname = s(named, "name");
        let cname = match alias {
            Some(al) if prefs.as_credits => s(al, "name"),
            _ => c
                .get("name")
                .and_then(|x| x.as_str())
                .map(String::from)
                .unwrap_or(aname.clone()),
        };
        name.push_str(&aname);
        name.push_str(&join);
        credit.push_str(&cname);
        credit.push_str(&join);
        let sn = s(named, "sort-name");
        sort.push_str(if sn.is_empty() { &aname } else { &sn });
        sort.push_str(&join);
        if id.is_empty() {
            id = s(&artist, "id");
        }
    }
    (name, id, sort, credit)
}

impl MusicBrainz {
    pub fn new(cfg: &Config) -> Self {
        let scheme = if cfg.bool("musicbrainz.https") {
            "https"
        } else {
            "http"
        };
        let host = cfg.str("musicbrainz.host");
        let rate = cfg.f64("musicbrainz.ratelimit").max(0.0);
        let per = cfg.f64("musicbrainz.ratelimit_interval").max(0.0);
        let interval = if rate > 0.0 {
            Duration::from_secs_f64(per / rate)
        } else {
            Duration::ZERO
        };
        MusicBrainz {
            agent: ureq::AgentBuilder::new()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build(),
            base: format!("{scheme}://{host}/ws/2"),
            interval,
            last: Mutex::new(None),
            search_limit: cfg.f64("musicbrainz.searchlimit").max(1.0) as usize,
            aliases: AliasPrefs::from_config(cfg),
        }
    }

    fn throttle(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < self.interval {
                std::thread::sleep(self.interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<J> {
        let url = format!("{}/{}", self.base, path);
        crate::logging::trace!("musicbrainz: GET {url} {query:?}");
        let mut attempt = 0;
        loop {
            self.throttle();
            let mut req = self.agent.get(&url).query("fmt", "json");
            for (k, v) in query {
                req = req.query(k, v);
            }
            match req.call() {
                Ok(resp) => return resp.into_json().context("parsing MusicBrainz response"),
                Err(ureq::Error::Status(503 | 429, _)) if attempt < 4 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(attempt));
                }
                Err(ureq::Error::Status(404, _)) => bail!("not found on MusicBrainz: {path}"),
                Err(e) => return Err(e).with_context(|| format!("MusicBrainz request {path}")),
            }
        }
    }

    /// Search releases; returns MBIDs in score order.
    pub fn search_releases(&self, artist: &str, album: &str, tracks: usize) -> Result<Vec<String>> {
        let mut parts = vec![];
        if !album.trim().is_empty() {
            parts.push(format!("release:({})", escape(album.trim())));
        }
        if !artist.trim().is_empty() {
            parts.push(format!("artist:({})", escape(artist.trim())));
        }
        if parts.is_empty() {
            return Ok(vec![]);
        }
        if tracks > 0 {
            parts.push(format!("tracks:{tracks}"));
        }
        let q = parts.join(" AND ");
        let limit = self.search_limit.to_string();
        crate::logging::debug!("Searching for MusicBrainz releases with: {q}");
        let res = self.get("release/", &[("query", &q), ("limit", &limit)])?;
        Ok(res
            .get("releases")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .map(|r| s(r, "id"))
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn release(&self, id: &str) -> Result<AlbumInfo> {
        crate::logging::debug!("Requesting MusicBrainz release {id}");
        let j = self.get(
            &format!("release/{id}"),
            &[(
                "inc",
                "recordings artist-credits labels release-groups isrcs aliases",
            )],
        )?;
        Ok(parse_release(&j, &self.aliases))
    }

    pub fn search_recordings(&self, artist: &str, title: &str) -> Result<Vec<TrackInfo>> {
        let mut parts = vec![];
        if !title.trim().is_empty() {
            parts.push(format!("recording:({})", escape(title.trim())));
        }
        if !artist.trim().is_empty() {
            parts.push(format!("artist:({})", escape(artist.trim())));
        }
        if parts.is_empty() {
            return Ok(vec![]);
        }
        let limit = self.search_limit.to_string();
        crate::logging::debug!(
            "Searching for MusicBrainz recordings with: {}",
            parts.join(" AND ")
        );
        let res = self.get(
            "recording/",
            &[("query", &parts.join(" AND ")), ("limit", &limit)],
        )?;
        Ok(res
            .get("recordings")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .map(|r| parse_recording(r, &self.aliases))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn recording(&self, id: &str) -> Result<TrackInfo> {
        crate::logging::debug!("Requesting MusicBrainz recording {id}");
        let j = self.get(
            &format!("recording/{id}"),
            &[("inc", "artist-credits isrcs aliases")],
        )?;
        Ok(parse_recording(&j, &self.aliases))
    }
}

pub fn parse_recording(r: &J, prefs: &AliasPrefs) -> TrackInfo {
    let (artist, artist_id, artist_sort, credit) = artist_credit(r.get("artist-credit"), prefs);
    TrackInfo {
        title: prefs.title(r),
        album: r
            .get("releases")
            .and_then(|rs| rs.as_array())
            .and_then(|rs| rs.first())
            .map(|rel| s(rel, "title"))
            .unwrap_or_default(),
        track_id: s(r, "id"),
        artist,
        artist_id,
        artist_sort,
        artist_credit: credit,
        length: r
            .get("length")
            .and_then(|l| l.as_f64())
            .map(|ms| ms / 1000.0),
        isrc: r
            .get("isrcs")
            .and_then(|i| i.as_array())
            .and_then(|a| a.first())
            .and_then(|i| i.as_str())
            .unwrap_or("")
            .to_string(),
        ..Default::default()
    }
}

pub fn parse_release(j: &J, prefs: &AliasPrefs) -> AlbumInfo {
    let (artist, artist_id, artist_sort, artist_credit_s) =
        artist_credit(j.get("artist-credit"), prefs);
    let (year, month, day) = parse_date(&s(j, "date"));
    let rg = j.get("release-group").cloned().unwrap_or(J::Null);
    let (oy, om, od) = parse_date(&s(&rg, "first-release-date"));
    let label_info = j
        .get("label-info")
        .and_then(|l| l.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(J::Null);
    let label = label_info
        .get("label")
        .map(|l| s(l, "name"))
        .unwrap_or_default();

    let mut tracks = vec![];
    let mut formats = vec![];
    let media = j
        .get("media")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    let mediums = media.len();
    for (mi, medium) in media.iter().enumerate() {
        let pos = medium
            .get("position")
            .and_then(|p| p.as_u64())
            .map(|p| p as usize)
            .unwrap_or(mi + 1);
        let fmt = s(medium, "format");
        if !fmt.is_empty() {
            formats.push(fmt.clone());
        }
        let mtracks = medium
            .get("tracks")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mtotal = mtracks.len();
        for (ti, t) in mtracks.iter().enumerate() {
            let rec = t.get("recording").cloned().unwrap_or(J::Null);
            let mut info = parse_recording(&rec, prefs);
            // The release's track title/credit wins over the recording's,
            // unless the recording has a preferred alias.
            let title = s(t, "title");
            if !title.is_empty() && prefs.preferred(&rec).is_none() {
                info.title = title;
            }
            if t.get("artist-credit").is_some() {
                let (a, aid, asort, acred) = artist_credit(t.get("artist-credit"), prefs);
                info.artist = a;
                info.artist_id = aid;
                info.artist_sort = asort;
                info.artist_credit = acred;
            }
            if let Some(l) = t.get("length").and_then(|l| l.as_f64()) {
                info.length = Some(l / 1000.0);
            }
            info.release_track_id = s(t, "id");
            info.medium = pos;
            info.medium_index = t
                .get("position")
                .and_then(|p| p.as_u64())
                .map(|p| p as usize)
                .unwrap_or(ti + 1);
            info.medium_total = mtotal;
            info.disctitle = s(medium, "title");
            info.media = fmt.clone();
            info.index = tracks.len() + 1;
            tracks.push(info);
        }
    }
    formats.dedup();
    AlbumInfo {
        album_id: s(j, "id"),
        album: prefs.title(j),
        va: artist_id == VA_ARTIST_ID,
        artist,
        artist_id,
        artist_sort,
        artist_credit: artist_credit_s,
        year,
        month,
        day,
        original_year: oy,
        original_month: om,
        original_day: od,
        country: s(j, "country"),
        label,
        catalognum: s(&label_info, "catalog-number"),
        albumtype: s(&rg, "primary-type").to_lowercase(),
        albumstatus: s(j, "status").to_lowercase(),
        albumdisambig: s(j, "disambiguation"),
        releasegroup_id: s(&rg, "id"),
        releasegroupdisambig: s(&rg, "disambiguation"),
        media: formats.join("/"),
        mediums,
        tracks,
    }
}

/// Pull an MBID out of a bare id or a musicbrainz.org URL.
pub fn extract_id(s: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
    )
    .unwrap();
    re.find(s).map(|m| m.as_str().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_json() {
        let j: J = serde_json::from_str(
            r#"{
            "id": "rel-1", "title": "Album", "date": "2003-05-12", "country": "GB", "status": "Official",
            "artist-credit": [{"name": "A", "joinphrase": " & ", "artist": {"id": "a1", "name": "A", "sort-name": "A, The"}},
                              {"name": "B", "joinphrase": "", "artist": {"id": "b1", "name": "B", "sort-name": "B"}}],
            "release-group": {"id": "rg", "primary-type": "Album", "first-release-date": "2001"},
            "label-info": [{"catalog-number": "CAT1", "label": {"name": "Lbl"}}],
            "media": [
              {"position": 1, "format": "CD", "tracks": [
                {"id": "t1", "position": 1, "title": "One", "length": 180000, "recording": {"id": "r1", "title": "One"}},
                {"id": "t2", "position": 2, "title": "Two", "length": null, "recording": {"id": "r2", "title": "Two", "length": 200000}}]},
              {"position": 2, "format": "CD", "title": "Bonus", "tracks": [
                {"id": "t3", "position": 1, "title": "Three", "recording": {"id": "r3", "title": "Three"},
                 "artist-credit": [{"name": "C", "artist": {"id": "c1", "name": "C"}}]}]}
            ]}"#,
        )
        .unwrap();
        let a = parse_release(&j, &AliasPrefs::default());
        assert_eq!(a.artist, "A & B");
        assert_eq!(a.artist_sort, "A, The & B");
        assert_eq!(a.artist_id, "a1");
        assert_eq!(
            (a.year, a.month, a.day, a.original_year),
            (2003, 5, 12, 2001)
        );
        assert_eq!(a.label, "Lbl");
        assert_eq!(a.catalognum, "CAT1");
        assert_eq!(a.albumtype, "album");
        assert_eq!(a.media, "CD");
        assert_eq!(a.mediums, 2);
        assert_eq!(a.tracks.len(), 3);
        assert_eq!(a.tracks[1].length, Some(200.0));
        assert_eq!(a.tracks[2].index, 3);
        assert_eq!(a.tracks[2].medium, 2);
        assert_eq!(a.tracks[2].medium_index, 1);
        assert_eq!(a.tracks[2].artist, "C");
        assert_eq!(a.tracks[2].disctitle, "Bonus");
    }

    #[test]
    fn preferred_aliases_follow_languages_and_ignored_types() {
        let j: J = serde_json::from_str(
            r#"{
            "id": "rel", "title": "Arubamu",
            "aliases": [{"name": "Album (en)", "locale": "en", "primary": true, "type": "Release name"}],
            "artist-credit": [{"name": "Kurejitto", "joinphrase": "", "artist": {
                "id": "a1", "name": "Aatisuto", "sort-name": "Aatisuto",
                "aliases": [
                  {"name": "Not primary", "sort-name": "x", "locale": "en", "primary": false, "type": "Artist name"},
                  {"name": "Search", "sort-name": "Search", "locale": "de", "primary": true, "type": "Search hint"},
                  {"name": "Artist", "sort-name": "Artist, The", "locale": "en", "primary": true, "type": "Artist name"},
                  {"name": "Kuenstler", "sort-name": "Kuenstler", "locale": "de", "primary": true, "type": "Artist name"}]}}],
            "media": [{"position": 1, "tracks": [
                {"id": "t1", "position": 1, "title": "Torakku", "recording": {"id": "r1", "title": "Torakku",
                  "aliases": [{"name": "Track", "locale": "en", "primary": true, "type": null}]}},
                {"id": "t2", "position": 2, "title": "Track title", "recording": {"id": "r2", "title": "Rec title"}}]}]
            }"#,
        )
        .unwrap();
        let prefs = |langs: &[&str], ignored: &[&str], as_credits| AliasPrefs {
            languages: langs.iter().map(|s| s.to_string()).collect(),
            ignored_types: ignored.iter().map(|s| s.to_string()).collect(),
            as_credits,
        };
        // No languages: the plain names, and track titles win.
        let a = parse_release(&j, &AliasPrefs::default());
        assert_eq!(
            (a.artist.as_str(), a.album.as_str()),
            ("Aatisuto", "Arubamu")
        );
        assert_eq!(a.artist_credit, "Kurejitto");
        assert_eq!(a.tracks[0].title, "Torakku");
        // English: the primary English aliases for artist, sort, album and
        // the recording (which then beats the track title).
        let a = parse_release(&j, &prefs(&["fr", "en", "de"], &[], false));
        assert_eq!(a.artist, "Artist");
        assert_eq!(a.artist_sort, "Artist, The");
        assert_eq!(a.artist_credit, "Kurejitto");
        assert_eq!(a.album, "Album (en)");
        assert_eq!(a.tracks[0].title, "Track");
        assert_eq!(a.tracks[1].title, "Track title");
        // German, with search hints ignored (case-insensitively), and the
        // alias used for the credit too.
        let a = parse_release(&j, &prefs(&["de"], &["search hint"], true));
        assert_eq!(a.artist, "Kuenstler");
        assert_eq!(a.artist_credit, "Kuenstler");
        assert_eq!(a.album, "Arubamu");
    }

    #[test]
    fn searched_recordings_carry_their_first_release() {
        let r: J = serde_json::from_str(
            r#"{"id": "r1", "title": "Song", "releases": [{"title": "First"}, {"title": "Second"}]}"#,
        )
        .unwrap();
        assert_eq!(parse_recording(&r, &AliasPrefs::default()).album, "First");
    }

    #[test]
    fn ids() {
        assert_eq!(
            extract_id("https://musicbrainz.org/release/0FA1E0C7-1C4A-4F2B-9A5E-ABCDEF012345")
                .as_deref(),
            Some("0fa1e0c7-1c4a-4f2b-9a5e-abcdef012345")
        );
        assert_eq!(extract_id("nope"), None);
        assert_eq!(escape("AC/DC: (live)"), "AC\\/DC\\: \\(live\\)");
    }
}
