//! The importer's match display (beets' `ui/commands/import_/display.py`
//! and the candidate list of `choose_candidate`).

use crate::autotag::{AlbumMatch, Current, Distance, TrackMatch};
use crate::config::Config;
use crate::diff::colordiff;
use crate::layout::{Side, indent, layout_lines};
use crate::mb::{AlbumInfo, TrackInfo};
use crate::model::Item;
use crate::ui::colorize;

const VARIOUS_ARTISTS: &str = "Various Artists";

/// beets' `human_seconds_short`: M:SS, seconds truncated.
pub fn human_seconds_short(secs: f64) -> String {
    let s = secs.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The settings the display reads, looked up once.
pub struct Display {
    pub width: usize,
    newline: bool,
    header: String,
    detail: String,
    tracklist: String,
    strong: f64,
    medium: f64,
    length_thresh: f64,
    per_disc: bool,
    /// `import.detail`: list unchanged tracks too.
    pub show_all: bool,
    /// Lines shown so far (`take` hands them over).
    out: std::cell::RefCell<Vec<String>>,
    singleton_album_disambig: bool,
    /// `match.album_disambig_fields` / `singleton_disambig_fields`.
    album_fields: Vec<String>,
    track_fields: Vec<String>,
}

impl Display {
    // beets turns float config values into integer widths with Python int().
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(cfg: &Config) -> Display {
        let n = |k: &str, d: f64| {
            cfg.get(k)
                .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(d)
        };
        let width = crate::ui::term_width(n("ui.terminal_width", 80.0) as usize);
        Display {
            width,
            newline: cfg.str("ui.import.layout") == "newline",
            header: indent(n("ui.import.indentation.match_header", 2.0) as usize),
            detail: indent(n("ui.import.indentation.match_details", 2.0) as usize),
            tracklist: indent(n("ui.import.indentation.match_tracklist", 5.0) as usize),
            strong: n("match.strong_rec_thresh", 0.04),
            medium: n("match.medium_rec_thresh", 0.25),
            length_thresh: n("ui.length_diff_thresh", 10.0),
            per_disc: cfg.bool("per_disc_numbering"),
            show_all: cfg.bool("import.detail"),
            singleton_album_disambig: cfg.bool("import.singleton_album_disambig"),
            album_fields: cfg.list("match.album_disambig_fields"),
            track_fields: cfg.list("match.singleton_disambig_fields"),
            out: Default::default(),
        }
    }

    fn emit(&self, line: String) {
        self.out.borrow_mut().push(line);
    }

    /// The lines shown so far, to print.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.out.borrow_mut())
    }

    /// Print the lines shown so far.
    pub fn flush(&self) {
        for line in self.take() {
            println!("{line}");
        }
    }

    /// beets' `Distance.color`.
    pub fn dist_color(&self, d: f64) -> &'static str {
        if d <= self.strong {
            "text_success"
        } else if d <= self.medium {
            "text_warning"
        } else {
            "text_error"
        }
    }

    /// beets' `Distance.string`: the similarity, coloured.
    pub fn dist_string(&self, d: f64) -> String {
        colorize(self.dist_color(d), &format!("{:.1}%", (1.0 - d) * 100.0))
    }

    fn changed_prefix() -> String {
        colorize("changed", "\u{2260}")
    }

    fn print_layout(&self, indent: &str, left: &Side, right: &Side) {
        for line in layout_lines(indent, left, right, self.width, self.newline) {
            self.emit(line);
        }
    }

    /// beets' `Match.disambig_string`: the configured fields the candidate
    /// has, `str()`-formatted (so a missing value shows as "None"), with
    /// `base` overriding some; ", "-joined. Unknown fields are left out
    /// with a warning (beets warns twice, once per look at the fields).
    fn disambig_string(
        fields: &[String],
        value: impl Fn(&str) -> Option<Option<String>>,
        base: impl Fn(&str) -> Option<String>,
    ) -> String {
        let missing: Vec<String> = fields
            .iter()
            .filter(|f| value(f).is_none())
            .map(|f| format!("'{f}'"))
            .collect();
        if !missing.is_empty() {
            for _ in 0..2 {
                crate::logging::emit(format!(
                    "Disambiguation string keys {{{}}} do not exist.",
                    missing.join(", ")
                ));
            }
        }
        fields
            .iter()
            .filter_map(|f| {
                let v = value(f)?;
                Some(base(f).unwrap_or_else(|| v.unwrap_or_else(|| "None".into())))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// An album candidate's disambiguation (`match.album_disambig_fields`;
    /// the media with its count, "2xCD").
    pub fn album_disambig(&self, info: &AlbumInfo) -> String {
        Self::disambig_string(
            &self.album_fields,
            |f| info.field(f),
            |f| {
                (f == "media").then(|| {
                    let media = info.field("media").flatten();
                    let media = media.unwrap_or_else(|| "None".into());
                    if info.mediums > 1 {
                        format!("{}x{media}", info.mediums)
                    } else {
                        media
                    }
                })
            },
        )
    }

    /// A track candidate's (`match.singleton_disambig_fields`): "Index N",
    /// "Track A1", and the album in brackets with
    /// `import.singleton_album_disambig` when the candidate has one.
    pub fn track_disambig(&self, info: &TrackInfo) -> String {
        let shown = |f: &str| info.field(f).flatten().unwrap_or_else(|| "None".into());
        Self::disambig_string(
            &self.track_fields,
            |f| info.field(f),
            |f| match f {
                "index" => Some(format!("Index {}", shown("index"))),
                "track_alt" => Some(format!("Track {}", shown("track_alt"))),
                "album" => Some(match info.field("album").flatten() {
                    Some(a) if self.singleton_album_disambig && !a.is_empty() => format!("[{a}]"),
                    _ => String::new(),
                }),
                _ => None,
            },
        )
    }

    /// beets' `show_match_header`.
    fn show_match_header(
        &self,
        dist: &Distance,
        artist: &str,
        name: &str,
        disambig: &str,
        url: &str,
    ) {
        let d = dist.distance();
        let mut parts = vec![
            String::new(),
            format!("Match ({}):", self.dist_string(d)),
            colorize(self.dist_color(d), &format!("{artist} - {name}")),
        ];
        let keys = dist.generic_penalty_keys();
        if !keys.is_empty() {
            parts.push(colorize(
                "changed",
                &format!("\u{2260} {}", keys.join(", ")),
            ));
        }
        if !disambig.is_empty() {
            parts.push(disambig.to_string());
        }
        if !url.is_empty() {
            parts.push(colorize("text_faint", url));
        }
        // `textwrap.indent` leaves empty lines alone.
        for p in parts {
            if p.is_empty() {
                self.emit(String::new());
            } else {
                for line in p.lines() {
                    self.emit(format!("{}{line}", self.header));
                }
            }
        }
    }

    /// beets' `show_match_details`: the artist and album (or title) lines.
    fn show_match_details(
        &self,
        src_artist: &str,
        src_name: &str,
        artist: &str,
        name: &str,
        type_: &str,
    ) {
        let (mut l, mut r) = (src_artist.to_string(), artist.to_string());
        if r == VARIOUS_ARTISTS {
            (l, r) = (String::new(), String::new());
        }
        if l != r {
            let (l, r) = colordiff(&l, &r);
            let left = Side::new(format!("{} Artist: ", Self::changed_prefix()), l, "");
            self.print_layout(&self.detail, &left, &Side::new("", r, ""));
        } else {
            self.emit(format!("{}* Artist: {r}", self.detail));
        }
        if !src_name.is_empty() {
            if src_name != name && name != VARIOUS_ARTISTS {
                let (l, r) = colordiff(src_name, name);
                let left = Side::new(format!("{} {type_}: ", Self::changed_prefix()), l, "");
                self.print_layout(&self.detail, &left, &Side::new("", r, ""));
            } else {
                self.emit(format!("{}* {type_}: {name}", self.detail));
            }
        }
    }

    /// beets' `TrackChange.show`.
    pub fn show_track_change(&self, item: &Item, m: &TrackMatch) {
        self.show_match_header(
            &m.distance,
            &m.info.artist,
            &m.info.title,
            &self.track_disambig(&m.info),
            &if m.info.track_id.is_empty() {
                String::new()
            } else {
                format!("https://musicbrainz.org/recording/{}", m.info.track_id)
            },
        );
        self.show_match_details(
            item.str("artist"),
            item.str("title"),
            &m.info.artist,
            &m.info.title,
            "Track",
        );
    }

    /// beets' `AlbumChange.show`: header, artist/album lines, the changed
    /// tracks (all with `import.detail`) and the missing and unmatched ones.
    pub fn show_album_change(&self, items: &[Item], src: &Current, m: &AlbumMatch) {
        let info = &m.info;
        self.show_match_header(
            &m.distance,
            &info.artist,
            &info.album,
            &self.album_disambig(info),
            &format!("https://musicbrainz.org/release/{}", info.album_id),
        );
        self.show_match_details(&src.artist, &src.album, &info.artist, &info.album, "Album");

        let mut pairs = m.mapping.clone();
        pairs.sort_by_key(|(_, t)| info.tracks[*t].index);
        let mut lines: Vec<(Side, Side)> = vec![];
        let mut medium: Option<(usize, String)> = None;
        for (i, t) in pairs {
            let (item, track) = (&items[i], &info.tracks[t]);
            let key = (track.medium, track.disctitle.clone());
            if medium.as_ref() != Some(&key) {
                let header = self.medium_info_line(info, track);
                if !header.is_empty() {
                    self.print_tracklist(&lines);
                    lines.clear();
                    self.emit(format!("{}{header}", self.detail));
                }
                medium = Some(key);
            }
            let (left, right) = self.track_line(info, item, track);
            if !right.contents.is_empty() || self.show_all {
                lines.push((left, right));
            }
        }
        self.print_tracklist(&lines);

        if !m.extra_tracks.is_empty() {
            let (n, total) = (m.extra_tracks.len(), info.tracks.len());
            self.emit(format!(
                "Missing tracks ({n}/{total} - {:.1}%):",
                n as f64 / total as f64 * 100.0
            ));
        }
        for &t in &m.extra_tracks {
            let tr = &info.tracks[t];
            let mut line = format!(" ! {} (#{})", tr.title, self.track_index(info, tr));
            if let Some(l) = tr.length.filter(|l| *l > 0.0) {
                line += &format!(" ({})", human_seconds_short(l));
            }
            self.emit(colorize("text_warning", &line));
        }
        if !m.extra_items.is_empty() {
            self.emit(format!("Unmatched tracks ({}):", m.extra_items.len()));
        }
        for &i in &m.extra_items {
            let it = &items[i];
            let mut line = format!(" ! {} (#{})", it.str("title"), self.item_index(it));
            if it.float("length") > 0.0 {
                line += &format!(" ({})", human_seconds_short(it.float("length")));
            }
            self.emit(colorize("text_warning", &line));
        }
    }

    /// beets' `make_medium_info_line`.
    fn medium_info_line(&self, info: &AlbumInfo, track: &TrackInfo) -> String {
        let media = if track.media.is_empty() {
            "Media"
        } else {
            &track.media
        };
        if info.mediums > 1 {
            if !track.disctitle.is_empty() {
                return format!("* {media} {}: {}", track.medium, track.disctitle);
            }
            return format!("* {media} {}", track.medium);
        }
        if !track.disctitle.is_empty() {
            return format!("* {media}: {}", track.disctitle);
        }
        String::new()
    }

    /// beets' `format_index` for a candidate track.
    fn track_index(&self, info: &AlbumInfo, t: &TrackInfo) -> String {
        if self.per_disc {
            if info.mediums > 1 {
                return format!("{}-{}", t.medium, t.medium_index);
            }
            return t.medium_index.to_string();
        }
        t.index.to_string()
    }

    /// beets' `format_index` for an item.
    fn item_index(&self, it: &Item) -> String {
        if self.per_disc && it.int("disctotal") > 1 {
            return format!("{}-{}", it.int("disc"), it.int("track"));
        }
        it.int("track").to_string()
    }

    /// beets' `make_line`: the item's (left) and the track's (right) index,
    /// title and length, the right side empty when nothing changes.
    fn track_line(&self, info: &AlbumInfo, item: &Item, track: &TrackInfo) -> (Side, Side) {
        // Title.
        let (lt, rt, title_changed) = if item.str("title").trim().is_empty() {
            let file = item
                .path()
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            (file, track.title.clone(), true)
        } else {
            let cur = item.str("title").trim();
            let (a, b) = colordiff(cur, &track.title);
            (a, b, cur != track.title)
        };
        // Index.
        let (cur, new) = (self.item_index(item), self.track_index(info, track));
        let n = item.int("track") as usize;
        let index_color = if cur == new {
            "text_faint"
        } else if n == track.index || n == track.medium_index {
            "text_highlight_minor"
        } else {
            "text_highlight"
        };
        let index_changed = cur != new;
        let (li, ri) = (
            colorize(index_color, &format!("(#{cur})")),
            colorize(index_color, &format!("(#{new})")),
        );
        // Length.
        let (il, tl) = (item.float("length"), track.length.unwrap_or(0.0));
        let length_changed = il > 0.0 && tl > 0.0 && (il - tl).abs() >= self.length_thresh;
        let length_color = if length_changed {
            "text_highlight"
        } else {
            "text_highlight_minor"
        };
        let (ll, rl) = (
            colorize(length_color, &format!("({})", human_seconds_short(il))),
            colorize(length_color, &format!("({})", human_seconds_short(tl))),
        );
        let changed = title_changed || index_changed || length_changed;
        let mark = if changed {
            Self::changed_prefix()
        } else {
            "*".into()
        };
        let left = Side::new(format!("{mark} {li} "), lt, format!(" {ll}"));
        if !changed {
            return (left, Side::default());
        }
        (left, Side::new(format!("{ri} "), rt, format!(" {rl}")))
    }

    /// beets' `print_tracklist`: shrink the columns to their contents when
    /// everything fits, else split the width evenly.
    fn print_tracklist(&self, lines: &[(Side, Side)]) {
        if lines.is_empty() {
            return;
        }
        let col = self
            .width
            .saturating_sub(self.tracklist.len() + "*  -> ".len())
            / 2;
        let max_l = lines
            .iter()
            .map(|(l, _)| l.rendered_width())
            .max()
            .unwrap_or(0);
        let max_r = lines
            .iter()
            .map(|(_, r)| r.rendered_width())
            .max()
            .unwrap_or(0);
        let (wl, wr) = if (max_l <= col && max_r <= col) || max_l + max_r <= col * 2 {
            (max_l, max_r)
        } else {
            (col, col)
        };
        for (l, r) in lines {
            let mut l = l.clone();
            let mut r = r.clone();
            l.width = Some(wl);
            r.width = Some(wr);
            self.print_layout(&self.tracklist, &l, &r);
        }
    }

    /// One candidate in beets' candidate list: "  N. 95.3% Artist - Album",
    /// then its penalties (at most three) and disambiguation, indented.
    pub fn candidate_lines(
        &self,
        n: usize,
        dist: &Distance,
        artist: &str,
        name: &str,
        disambig: &str,
    ) -> Vec<String> {
        let d = dist.distance();
        let color = self.dist_color(d);
        let mut out = vec![format!(
            "  {} {} {}",
            colorize(color, &format!("{n}.")),
            self.dist_string(d),
            colorize(
                if n == 1 {
                    color
                } else {
                    "text_highlight_minor"
                },
                &format!("{artist} - {name}")
            ),
        )];
        let mut keys = dist.generic_penalty_keys();
        if !keys.is_empty() {
            if keys.len() > 3 {
                keys.truncate(3);
                keys.push("...".into());
            }
            out.push(format!(
                "{}{}",
                " ".repeat(13),
                colorize("changed", &format!("\u{2260} {}", keys.join(", ")))
            ));
        }
        if !disambig.is_empty() {
            out.push(format!("{}{disambig}", " ".repeat(13)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, track: i64, length: f64) -> Item {
        let mut i = Item::new_item();
        i.set_str("title", title);
        i.set_int("track", track);
        i.set("length", crate::model::Value::Float(length));
        i.set_path(format!("/m/{track}.mp3"));
        i
    }

    fn track(title: &str, index: usize, length: f64) -> TrackInfo {
        TrackInfo {
            title: title.into(),
            index,
            medium: 1,
            medium_index: index,
            length: Some(length),
            ..Default::default()
        }
    }

    #[test]
    fn album_change_as_beets() {
        let cfg = Config::defaults();
        let mut d = Display::new(&cfg);
        d.width = 80;
        let items = vec![
            item("Song One", 1, 180.0),
            item("Song 2", 2, 200.0),
            item("Extra", 9, 60.0),
        ];
        let info = AlbumInfo {
            album_id: "abc".into(),
            artist: "The Band".into(),
            album: "The Album".into(),
            media: "CD".into(),
            mediums: 1,
            year: 2001,
            country: "GB".into(),
            label: "Label".into(),
            catalognum: "CAT 1".into(),
            data_source: "MusicBrainz".into(),
            nulls: ["albumdisambig"].into(),
            tracks: vec![
                track("Song One", 1, 181.0),
                track("Song Two", 2, 230.0),
                track("Gone", 3, 61.0),
            ],
            ..Default::default()
        };
        let mut dist = Distance::new(&cfg);
        dist.add("album", 0.5);
        dist.add("missing_tracks", 1.0);
        let m = AlbumMatch {
            distance: dist,
            info,
            mapping: vec![(0, 0), (1, 1)],
            extra_items: vec![2],
            extra_tracks: vec![2],
        };
        let src = Current {
            artist: "The Band".into(),
            album: "Album".into(),
            ..Default::default()
        };
        d.show_album_change(&items, &src, &m);
        let out = d.take();
        let pct = format!("{:.1}%", (1.0 - m.distance.distance()) * 100.0);
        assert_eq!(
            out,
            [
                String::new(),
                format!("  Match ({pct}):"),
                "  The Band - The Album".into(),
                "  \u{2260} album, missing tracks".into(),
                "  MusicBrainz, CD, 2001, GB, Label, CAT 1, None".into(),
                "  https://musicbrainz.org/release/abc".into(),
                "  * Artist: The Band".into(),
                "  \u{2260} Album: Album -> The Album".into(),
                // Track 1 is unchanged (1s off), so only track 2 shows.
                "     \u{2260} (#2) Song 2 (3:20) -> (#2) Song Two (3:50)".into(),
                "Missing tracks (1/3 - 33.3%):".into(),
                " ! Gone (#3) (1:01)".into(),
                "Unmatched tracks (1):".into(),
                " ! Extra (#9) (1:00)".into(),
            ]
        );
        // With import.detail every track shows.
        d.show_all = true;
        d.show_album_change(&items, &src, &m);
        assert!(
            d.take()
                .contains(&"     * (#1) Song One (3:00)".to_string())
        );
    }

    #[test]
    fn track_change_as_beets() {
        let cfg = Config::defaults();
        let mut d = Display::new(&cfg);
        d.width = 80;
        let mut it = item("Song 1", 1, 180.0);
        it.set_str("artist", "The Band");
        // A searched-for recording: no position, no number, no album.
        let info = TrackInfo {
            track_id: "rec1".into(),
            artist: "The Band".into(),
            data_source: "MusicBrainz".into(),
            ..track("Song One", 0, 181.0)
        };
        let mut dist = Distance::new(&cfg);
        dist.add("track_title", 0.3);
        let m = TrackMatch {
            distance: dist,
            info,
        };
        d.show_track_change(&it, &m);
        let pct = format!("{:.1}%", (1.0 - m.distance.distance()) * 100.0);
        assert_eq!(
            d.take(),
            [
                String::new(),
                format!("  Match ({pct}):"),
                "  The Band - Song One".into(),
                "  \u{2260} title".into(),
                "  MusicBrainz, Index None, Track None, ".into(),
                "  https://musicbrainz.org/recording/rec1".into(),
                "  * Artist: The Band".into(),
                "  \u{2260} Track: Song 1 -> Song One".into(),
            ]
        );

        // A changed artist and an unchanged title; no URL without an ID.
        it.set_str("artist", "Band");
        it.set_str("title", "Song One");
        let m = TrackMatch {
            info: TrackInfo {
                track_id: String::new(),
                ..m.info
            },
            ..m
        };
        d.show_track_change(&it, &m);
        let out = d.take();
        assert!(
            !out.iter().any(|l| l.contains("musicbrainz.org")),
            "{out:?}"
        );
        assert!(
            out.contains(&"  \u{2260} Artist: Band -> The Band".to_string()),
            "{out:?}"
        );
        assert!(out.contains(&"  * Track: Song One".to_string()), "{out:?}");
    }

    #[test]
    fn candidate_lines_as_beets() {
        let cfg = Config::defaults();
        let d = Display::new(&cfg);
        let mut dist = Distance::new(&cfg);
        for k in ["album", "artist", "year", "media"] {
            dist.add(k, 0.1);
        }
        let lines = d.candidate_lines(2, &dist, "A", "B", "MusicBrainz, 2001");
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].starts_with("  2. ") && lines[0].ends_with("% A - B"),
            "{}",
            lines[0]
        );
        assert_eq!(
            lines[1],
            format!("{}\u{2260} album, artist, year, ...", " ".repeat(13))
        );
        assert_eq!(lines[2], format!("{}MusicBrainz, 2001", " ".repeat(13)));
    }

    #[test]
    fn seconds() {
        assert_eq!(human_seconds_short(59.9), "0:59");
        assert_eq!(human_seconds_short(3725.0), "62:05");
    }

    /// The candidates of tests/fixtures/mb as reference beets 2.14 shows
    /// them (with its None for the fields MusicBrainz leaves empty).
    #[test]
    fn disambiguation_as_beets() {
        use crate::mb::{ParseOpts, parse_release};
        let fixture = |id: &str| {
            let path = format!(
                "{}/tests/fixtures/mb/release-{id}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let j = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            parse_release(&j, &ParseOpts::default())
        };
        let full = fixture("11111111-1111-1111-1111-111111111111");
        let deluxe = fixture("22222222-2222-2222-2222-222222222222");
        let mut cfg = Config::defaults();
        let d = Display::new(&cfg);
        assert_eq!(
            d.album_disambig(&full),
            "MusicBrainz, 2xMedia, 2004, GB, Fixture Records, FX-001, None"
        );
        assert_eq!(
            d.album_disambig(&deluxe),
            "MusicBrainz, Digital Media, 2003, , None, None, deluxe edition"
        );
        // Other fields, in the order given.
        cfg.set(
            "match.album_disambig_fields",
            "albumdisambig year albumtype releasegroupdisambig mediums va".into(),
        );
        assert_eq!(
            Display::new(&cfg).album_disambig(&deluxe),
            "deluxe edition, 2003, album, None, 1, False"
        );
        // Unknown fields are left out (with a warning).
        cfg.set("match.album_disambig_fields", "year bogus country".into());
        assert_eq!(Display::new(&cfg).album_disambig(&full), "2004, GB");

        // Tracks of a release have an index and a number; the album shows
        // only for a candidate that has one (never from MusicBrainz).
        let d = Display::new(&cfg);
        assert_eq!(
            d.track_disambig(&full.tracks[1]),
            "MusicBrainz, Index 2, Track 1, "
        );
        cfg.set(
            "match.singleton_disambig_fields",
            "index media disctitle length medium_total".into(),
        );
        assert_eq!(
            Display::new(&cfg).track_disambig(&full.tracks[4]),
            "Index 5, DVD-Video, , 600.0, 1"
        );
    }
}
