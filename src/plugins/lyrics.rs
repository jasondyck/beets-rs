//! lyrics: fetch lyrics from LRCLIB (https://lrclib.net) into the `lyrics` field.

use super::{Plugin, PluginCtx, http_agent};
use crate::config::Config;
use crate::importer::FileOp;
use crate::model::{Album, Item};
use anyhow::{Context, Result};
use serde_json::Value as J;
use std::path::Path;

pub const INSTRUMENTAL: &str = "[Instrumental]";
const BASE: &str = "https://lrclib.net/api";
/// Search results must be within this many seconds of the track's length.
const DURATION_SLACK: f64 = 10.0;

pub struct Lyrics {
    auto: bool,
    force: bool,
    synced: bool,
    agent: ureq::Agent,
}

/// Choose the text to store from an LRCLIB record.
fn pick_text(rec: &J, synced: bool) -> Option<String> {
    if rec.get("instrumental").and_then(|v| v.as_bool()) == Some(true) {
        return Some(INSTRUMENTAL.into());
    }
    let get = |k: &str| {
        rec.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    if synced {
        get("syncedLyrics").or_else(|| get("plainLyrics"))
    } else {
        get("plainLyrics")
    }
}

/// From search results, the closest-duration record with lyrics.
fn best_search_result(results: &[J], length: f64, synced: bool) -> Option<String> {
    let mut candidates: Vec<(&J, f64)> = results
        .iter()
        .map(|r| {
            let d = r
                .get("duration")
                .and_then(|d| d.as_f64())
                .unwrap_or(f64::MAX);
            (r, (d - length).abs())
        })
        .filter(|(_, diff)| length <= 0.0 || *diff <= DURATION_SLACK)
        .collect();
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    candidates
        .into_iter()
        .find_map(|(r, _)| pick_text(r, synced))
}

impl Lyrics {
    pub fn new(cfg: &Config) -> Self {
        Lyrics {
            auto: cfg.bool("lyrics.auto"),
            force: cfg.bool("lyrics.force"),
            synced: cfg.bool("lyrics.synced"),
            agent: http_agent(),
        }
    }

    pub fn fetch(
        &self,
        artist: &str,
        title: &str,
        album: &str,
        length: f64,
    ) -> Result<Option<String>> {
        if artist.is_empty() || title.is_empty() {
            return Ok(None);
        }
        // Exact lookup first; LRCLIB matches duration within ~2s.
        let mut req = self
            .agent
            .get(&format!("{BASE}/get"))
            .query("artist_name", artist)
            .query("track_name", title);
        if !album.is_empty() {
            req = req.query("album_name", album);
        }
        if length > 0.0 {
            req = req.query("duration", &(length.round() as i64).to_string());
        }
        match req.call() {
            Ok(resp) => {
                let rec: J = resp.into_json().context("parsing LRCLIB response")?;
                if let Some(t) = pick_text(&rec, self.synced) {
                    return Ok(Some(t));
                }
            }
            Err(ureq::Error::Status(404, _)) => {}
            Err(e) => return Err(e).context("LRCLIB request"),
        }
        // Then a looser search, matched on duration.
        let resp = self
            .agent
            .get(&format!("{BASE}/search"))
            .query("artist_name", artist)
            .query("track_name", title)
            .call()
            .context("LRCLIB search")?;
        let results: Vec<J> = resp.into_json().context("parsing LRCLIB search")?;
        Ok(best_search_result(&results, length, self.synced))
    }

    /// Fetch and store lyrics for one item. Returns true if lyrics were set.
    pub fn fetch_item(&self, ctx: &PluginCtx, item: &mut Item, force: bool) -> Result<bool> {
        if !item.str("lyrics").is_empty() && !(force || self.force) {
            return Ok(false);
        }
        let artist = item.str("artist").to_string();
        let title = item.str("title").to_string();
        let Some(text) = self.fetch(&artist, &title, item.str("album"), item.float("length"))?
        else {
            println!("lyrics: not found: {artist} - {title}");
            return Ok(false);
        };
        item.set_str("lyrics", text);
        if ctx.write
            && let Err(e) = crate::media::write_item(item)
        {
            eprintln!("warning: {e:#}");
        }
        ctx.lib.store(item)?;
        println!("lyrics: fetched {artist} - {title}");
        Ok(true)
    }
}

impl Plugin for Lyrics {
    fn name(&self) -> &'static str {
        "lyrics"
    }

    fn album_imported(
        &self,
        ctx: &PluginCtx,
        _album: &mut Album,
        items: &mut [Item],
        _source_dir: &Path,
        _op: FileOp,
    ) -> Result<()> {
        for item in items {
            self.item_imported(ctx, item)?;
        }
        Ok(())
    }

    fn item_imported(&self, ctx: &PluginCtx, item: &mut Item) -> Result<()> {
        if self.auto {
            self.fetch_item(ctx, item, false)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chooses_text() {
        let r = json!({"plainLyrics": "la la", "syncedLyrics": "[00:01.00] la la", "instrumental": false});
        assert_eq!(pick_text(&r, false).unwrap(), "la la");
        assert_eq!(pick_text(&r, true).unwrap(), "[00:01.00] la la");
        assert_eq!(
            pick_text(&json!({"instrumental": true}), false).unwrap(),
            INSTRUMENTAL
        );
        assert_eq!(pick_text(&json!({"plainLyrics": "  "}), false), None);
        let only_plain = json!({"plainLyrics": "words", "syncedLyrics": null});
        assert_eq!(pick_text(&only_plain, true).unwrap(), "words");
    }

    #[test]
    fn search_prefers_closest_duration() {
        let results = vec![
            json!({"duration": 300.0, "plainLyrics": "far"}),
            json!({"duration": 241.0, "plainLyrics": "close"}),
            json!({"duration": 238.0, "plainLyrics": null}),
        ];
        assert_eq!(best_search_result(&results, 240.0, false).unwrap(), "close");
        assert_eq!(best_search_result(&results, 100.0, false), None);
    }
}
