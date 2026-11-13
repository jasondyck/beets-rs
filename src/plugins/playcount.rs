//! A port of beets' `beetsplug/_utils/playcount.py`: matching play-count
//! entries (from last.fm or ListenBrainz) to library items -- by recording
//! MBID when there is one, else by exact artist/title (and album), else by
//! substring matches -- and storing them in a `<source>_play_count` flexible
//! attribute.

use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::PluginLog;
use crate::query::Query;
use anyhow::Result;

/// One play-count entry (`Track` in the Python module).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub mbid: Option<String>,
    pub name: String,
    pub artist: String,
    pub playcount: i64,
    pub album: Option<String>,
}

fn all_items(lib: &Library) -> Result<Vec<Item>> {
    lib.items(&Query::parse(Kind::Item, &[] as &[String])?)
}

/// The items a play-count entry matches, in library order: the MBID match
/// when there is one, else exact title with artist or album, else the same
/// as substrings.
pub fn get_items(lib: &Library, track: &Track, log: &PluginLog) -> Result<Vec<Item>> {
    let items = all_items(lib)?;
    if let Some(mbid) = track.mbid.as_deref().filter(|m| !m.is_empty()) {
        // beets' `MatchQuery("mb_trackid", mbid)`: SQL equality.
        let found: Vec<Item> = items
            .iter()
            .filter(|i| i.str("mb_trackid") == mbid)
            .cloned()
            .collect();
        if !found.is_empty() {
            return Ok(found);
        }
    }

    let album = track.album.as_deref().unwrap_or("");
    log.debug(&format!(
        "query: {} - {} ({album})",
        track.artist, track.name
    ));
    let titles = [track.name.clone(), track.name.replace('\'', "\u{2019}")];
    // `StringQuery`: exact, case-insensitively.
    let exact = |i: &Item| {
        titles
            .iter()
            .any(|t| i.str("title").to_lowercase() == t.to_lowercase())
            && (i.str("artist").to_lowercase() == track.artist.to_lowercase()
                || (!album.is_empty() && i.str("album").to_lowercase() == album.to_lowercase()))
    };
    let found: Vec<Item> = items.iter().filter(|i| exact(i)).cloned().collect();
    if !found.is_empty() {
        return Ok(found);
    }

    // The substring fallback.
    let substring = |i: &Item| {
        titles
            .iter()
            .any(|t| i.str("title").to_lowercase().contains(&t.to_lowercase()))
            && (i
                .str("artist")
                .to_lowercase()
                .contains(&track.artist.to_lowercase())
                || (!album.is_empty()
                    && i.str("album")
                        .to_lowercase()
                        .contains(&album.to_lowercase())))
    };
    Ok(items.iter().filter(|i| substring(i)).cloned().collect())
}

/// `process_track`: store the count on every matching item; false when
/// nothing matched.
pub fn process_track(lib: &Library, track: &Track, log: &PluginLog, source: &str) -> Result<bool> {
    let items = get_items(lib, track, log)?;
    if items.is_empty() {
        return Ok(false);
    }
    let field = format!("{source}_play_count");
    for mut item in items {
        let count = item.int(&field);
        log.debug(&format!(
            "match: {} - {} ({}) updating: {field} {count} => {}",
            item.str("artist"),
            item.str("title"),
            item.str("album"),
            track.playcount
        ));
        item.set_int(&field, track.playcount);
        lib.store(&item)?;
    }
    Ok(true)
}

/// `update_play_counts`: every entry in one transaction; (found, unknown).
pub fn update_play_counts(
    lib: &Library,
    tracks: &[Track],
    log: &PluginLog,
    source: &str,
) -> Result<(usize, usize)> {
    let total = tracks.len();
    let mut total_found = 0;
    let mut total_fails = 0;
    log.info(&format!(
        "Received {total} tracks in this page, processing..."
    ));
    lib.transaction(|lib| {
        for (i, track) in tracks.iter().enumerate() {
            if (i + 1) % 250 == 0 {
                log.info(&format!("Processing track {}/{total} ...", i + 1));
            }
            if process_track(lib, track, log, source)? {
                total_found += 1;
            } else {
                total_fails += 1;
            }
        }
        Ok(())
    })?;
    if total_fails > 0 {
        log.info(&format!(
            "Acquired {total_found}/{total} play-counts ({total_fails} unknown)"
        ));
    }
    Ok((total_found, total_fails))
}

#[cfg(test)]
#[path = "playcount_tests.rs"]
mod tests;
