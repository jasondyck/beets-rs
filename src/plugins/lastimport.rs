//! A port of beets 2.14's `lastimport` plugin (`beetsplug/lastimport.py`):
//! `beet lastimport` reads the user's top tracks from last.fm (page by
//! page) and stores their play counts in the items' `lastfm_play_count`
//! field, matching tracks by MBID first, then artist/title/album.
//!
//! Settings: `lastfm.user` and `lastfm.api_key` (beets' public key by
//! default), plus `lastimport.per_page` and `lastimport.retry_limit`.

use crate::config::Config;
use crate::library::Library;
use crate::plugins::PluginLog;
use crate::plugins::playcount::{self, Track};
use crate::plugins::requests::{Request, Session};
use anyhow::Result;

/// The last.fm Web API endpoint.
pub const API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

/// The plugin's `item_types`.
pub const ITEM_TYPES: &[(&str, crate::model::FieldType)] =
    &[("lastfm_play_count", crate::model::FieldType::Int)];

pub struct LastImportPlugin;

impl crate::plugins::Plugin for LastImportPlugin {
    fn name(&self) -> &'static str {
        "lastimport"
    }
}

/// One page of top tracks, and the total number of pages.
#[allow(dead_code)] // beets' plugin API; the command uses `import_lastfm`.
pub fn fetch_tracks(cfg: &Config, page: i64, limit: usize) -> Result<(Vec<Track>, i64)> {
    fetch_tracks_from(API_URL, cfg, page, limit)
}

/// `fetch_tracks` against another endpoint (a fake server in tests).
pub fn fetch_tracks_from(
    api_url: &str,
    cfg: &Config,
    page: i64,
    limit: usize,
) -> Result<(Vec<Track>, i64)> {
    let user = cfg.str("lastfm.user");
    let api_key = cfg.str("lastfm.api_key");
    // The parameters go in the query string, as pylast sends them.
    let url = crate::plugins::requests::format_url(
        api_url,
        &[
            ("method", "user.getTopTracks".into()),
            ("api_key", api_key),
            ("user", user),
            ("period", "overall".into()),
            ("page", page.to_string()),
            ("limit", limit.to_string()),
        ],
    );
    let session = Session::new();
    let response = session
        .get(&url, Request::default())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let text = response.text(true);
    parse_top_tracks(&text)
}

/// pylast parses last.fm's XML; `_extract` takes the n-th element with a
/// name, `_number` its text.
fn parse_top_tracks(xml: &str) -> Result<(Vec<Track>, i64)> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| anyhow::anyhow!("error: {e}"))?;
    let root = doc.root_element();
    if root.attribute("status") == Some("failed") {
        let message = root
            .descendants()
            .find(|n| n.has_tag_name("error"))
            .and_then(|n| n.text())
            .unwrap_or("operation failed");
        anyhow::bail!("{message}");
    }
    let toptracks = root
        .descendants()
        .find(|n| n.has_tag_name("toptracks"))
        .ok_or_else(|| anyhow::anyhow!("Last.fm reported no data."))?;
    let total_pages: i64 = toptracks
        .attribute("totalPages")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let mut tracks = vec![];
    for node in doc.descendants().filter(|n| n.has_tag_name("track")) {
        let extract = |name: &str, i: usize| -> String {
            node.descendants()
                .filter(|n| n.has_tag_name(name))
                .nth(i)
                .and_then(|n| n.text())
                .unwrap_or_default()
                .to_string()
        };
        tracks.push(Track {
            mbid: Some(extract("mbid", 0)),
            artist: extract("name", 1).trim().to_string(),
            name: extract("name", 0).trim().to_string(),
            playcount: extract("playcount", 0).trim().parse().unwrap_or(0),
            album: None,
        });
    }
    Ok((tracks, total_pages))
}

/// `import_lastfm`.
pub fn import_lastfm(cfg: &Config, lib: &Library, log: &PluginLog) -> Result<()> {
    import_lastfm_from(API_URL, cfg, lib, log)
}

/// `import_lastfm` against another endpoint (a fake server in tests).
pub fn import_lastfm_from(
    api_url: &str,
    cfg: &Config,
    lib: &Library,
    log: &PluginLog,
) -> Result<()> {
    let user = cfg.str("lastfm.user");
    let per_page = cfg.str("lastimport.per_page").parse().unwrap_or(500);
    if user.is_empty() {
        anyhow::bail!("You must specify a user name for lastimport");
    }
    log.info(&format!("Fetching last.fm library for @{user}"));

    let mut page_total: i64 = 1;
    let mut page_current: i64 = 0;
    let mut found_total = 0usize;
    let mut unknown_total = 0usize;
    let retry_limit: i64 = cfg.str("lastimport.retry_limit").parse().unwrap_or(3);
    while page_current < page_total {
        log.info(&format!(
            "Querying page #{}{}...",
            page_current + 1,
            if page_total > 1 {
                format!("/{page_total}")
            } else {
                String::new()
            }
        ));
        for retry in 0..retry_limit {
            let (tracks, total) = fetch_tracks_from(api_url, cfg, page_current + 1, per_page)?;
            page_total = total;
            if page_total < 1 {
                anyhow::bail!("Last.fm reported no data.");
            }
            if !tracks.is_empty() {
                let (found, unknown) = playcount::update_play_counts(lib, &tracks, log, "lastfm")?;
                found_total += found;
                unknown_total += unknown;
                break;
            }
            log.error(&format!("ERROR: unable to read page #{}", page_current + 1));
            if retry < retry_limit {
                log.info(&format!(
                    "Retrying page #{}... ({}/{} retry)",
                    page_current + 1,
                    retry + 1,
                    retry_limit
                ));
            } else {
                // beets passes extra arguments its format string has no
                // placeholders for; this is what it prints.
                log.error(&format!("FAIL: unable to fetch page #{page_current}, "));
            }
        }
        page_current += 1;
    }
    log.info("... done!");
    log.info(&format!("finished processing {page_total} song pages"));
    log.info(&format!("{unknown_total} unknown play-counts"));
    log.info(&format!("{found_total} play-counts imported"));
    Ok(())
}

/// `beet lastimport`.
pub fn command(cfg: &Config, lib: &Library) -> Result<()> {
    import_lastfm(cfg, lib, &PluginLog::new("lastimport"))
}

/// The plugin's `__init__`: last.fm's settings and the play-count type.
pub fn load(_cfg: &Config) -> LastImportPlugin {
    LastImportPlugin
}

#[cfg(test)]
#[path = "lastimport_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "lastimport",
        order: 41,
        defaults: r#"lastfm:
    user: ''
    api_key: 2dc3914abf35f0d9c92d97d8f8e42b43

lastimport:
    per_page: 500
    retry_limit: 3
"#,
        config_keys: &["lastfm", "lastimport"],
        item_types: crate::plugins::lastimport::ITEM_TYPES,
        album_types: &[],
        redacted: &["lastfm.user", "lastfm.api_key"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "lastimport",
                build: build_lastimport,
                run: run_lastimport,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct LastimportArgs {
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_lastimport() -> clap::Command {
    use clap::Args as _;
    LastimportArgs::augment_args(
        clap::Command::new("lastimport").about("import last.fm play-count"),
    )
}

pub fn run_lastimport(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let _a = LastimportArgs::from_arg_matches(m)?;
    command(cfg, lib)
}
