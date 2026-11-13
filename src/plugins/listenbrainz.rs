//! A port of beets 2.14's `listenbrainz` plugin (`beetsplug/listenbrainz.py`):
//! `beet lbimport` fetches the user's ListenBrainz listening history (or
//! reads a ListenBrainz data export `.zip`), aggregates the individual
//! listens into per-track play counts and stores them in the items'
//! `listenbrainz_play_count` field.
//!
//! Settings: `listenbrainz.token` (required) and `listenbrainz.username`
//! (required).

use crate::config::Config;
use crate::library::Library;
use crate::plugins::PluginLog;
use crate::plugins::mbapi::MusicBrainzApi;
use crate::plugins::playcount::{self, Track};
use crate::plugins::requests::{HttpError, Request, Session};
use anyhow::Result;
use serde_json::Value as J;
use std::collections::BTreeMap;

/// beets' `ROOT`: the trailing slash means requests go out with `//`.
pub const ROOT: &str = "http://api.listenbrainz.org/1/";

/// The plugin's `item_types`.
pub const ITEM_TYPES: &[(&str, crate::model::FieldType)] =
    &[("listenbrainz_play_count", crate::model::FieldType::Int)];

pub struct ListenBrainzPlugin {
    pub token: String,
    pub username: String,
    pub log: PluginLog,
    session: Session,
    /// For `self.mb_api`.
    cfg: Config,
    /// `ROOT`; tests point this at a fake server.
    root: String,
}

impl crate::plugins::Plugin for ListenBrainzPlugin {
    fn name(&self) -> &'static str {
        "listenbrainz"
    }
}

/// The `Authorization` header (present even when the token is empty, as in
/// beets: `Token None`).
fn auth_header(token: &str) -> String {
    format!("Token {token}")
}

impl ListenBrainzPlugin {
    /// `__init__`: reads the settings (a missing one fails the load).
    pub fn new(cfg: &Config) -> Result<Self> {
        let token = cfg
            .get("listenbrainz.token")
            .ok_or_else(|| anyhow::anyhow!("listenbrainz.token not found"))?;
        let token = match token {
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other)?.trim().to_string(),
        };
        let username = cfg
            .get("listenbrainz.username")
            .ok_or_else(|| anyhow::anyhow!("listenbrainz.username not found"))?;
        let username = match username {
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other)?.trim().to_string(),
        };
        Ok(ListenBrainzPlugin {
            token,
            username,
            log: PluginLog::new("listenbrainz"),
            session: Session::new(),
            cfg: cfg.clone(),
            root: ROOT.to_string(),
        })
    }

    /// The `beet lbimport` command's function.
    pub fn lbupdate(
        &self,
        lib: &Library,
        export_file: Option<&str>,
        max_listens: Option<usize>,
    ) -> Result<()> {
        let listens: Vec<J>;
        if let Some(file) = export_file {
            self.log
                .info(&format!("Importing ListenBrainz data from {file}..."));
            if max_listens.is_some() {
                self.log
                    .warning("Ignoring superfluous --max flag when importing from file.");
            }
            listens = self.import_listenbrainz_data_export(file)?;
        } else {
            self.log.info("Fetching ListenBrainz history...");
            match self.get_listens(None, None, None, max_listens)? {
                Some(l) => listens = l,
                None => {
                    self.log.error("Failed to fetch listens from ListenBrainz.");
                    return Ok(());
                }
            }
        }
        if listens.is_empty() {
            self.log.info("No listens found.");
            return Ok(());
        }
        self.log.info(&format!("Found {} listens", listens.len()));
        let tracks = Self::aggregate_listens(&self.get_tracks_from_listens(&listens));
        self.log
            .info(&format!("Aggregated into {} unique tracks", tracks.len()));
        let (found, unknown) =
            playcount::update_play_counts(lib, &tracks, &self.log, "listenbrainz")?;
        self.log.info("... done!");
        self.log.info(&format!("{unknown} unknown play-counts"));
        self.log.info(&format!("{found} play-counts imported"));
        Ok(())
    }

    /// `_aggregate_listens`: one entry per MBID (else artist/title/album),
    /// with the summed play count.
    pub fn aggregate_listens(tracks: &[Track]) -> Vec<Track> {
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        let mut info: BTreeMap<String, Track> = BTreeMap::new();
        let mut order: Vec<String> = vec![];
        for t in tracks {
            let mbid = t.mbid.clone().unwrap_or_default();
            let key = if mbid.is_empty() {
                format!(
                    "{}\u{0}{}\u{0}{}",
                    t.artist,
                    t.name,
                    t.album.clone().unwrap_or_default()
                )
            } else {
                format!("0{mbid}")
            };
            *counts.entry(key.clone()).or_insert(0) += 1;
            if !info.contains_key(&key) {
                order.push(key.clone());
                info.insert(key.clone(), t.clone());
            }
        }
        order
            .into_iter()
            .map(|key| {
                let mut t = info.remove(&key).unwrap();
                t.playcount = counts[&key];
                t
            })
            .collect()
    }

    /// `_make_request`: a GET with the token; `None` on any request error.
    fn make_request(&self, url: &str, params: Option<&[(&str, String)]>) -> Option<J> {
        let full = match params {
            Some(p) if !p.is_empty() => crate::plugins::requests::format_url(url, p),
            _ => url.to_string(),
        };
        let header = auth_header(&self.token);
        let request = Request {
            headers: &[("Authorization", &header)],
            ..Default::default()
        };
        match self.session.get(&full, request) {
            Ok(resp) => {
                if let Some(remaining) = resp.header("X-RateLimit-Remaining")
                    && remaining.trim() == "0"
                    && let Some(reset) = resp
                        .header("X-RateLimit-Reset-In")
                        .and_then(|s| s.trim().parse::<i64>().ok())
                {
                    self.log.debug(&format!(
                        "ListenBrainz rate limit reached; sleeping {}s",
                        reset
                    ));
                    std::thread::sleep(std::time::Duration::from_secs((reset + 1).max(0) as u64));
                }
                resp.json().ok()
            }
            Err(e) => {
                self.log.debug(&format!("Invalid Search Error: {e}"));
                None
            }
        }
    }

    /// `import_listenbrainz_data_export`: every `listens/*.jsonl` line in
    /// the zip.
    pub fn import_listenbrainz_data_export(&self, export_file: &str) -> Result<Vec<J>> {
        use std::io::{BufRead, BufReader};
        let file = std::fs::File::open(export_file)
            .map_err(|e| anyhow::anyhow!("unreadable export file {export_file}: {e}"))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| anyhow::anyhow!("unreadable export file {export_file}: {e}"))?;
        let mut all: Vec<J> = vec![];
        for i in 0..zip.len() {
            let Ok(entry) = zip.by_index(i) else {
                continue;
            };
            let name = entry.name().to_string();
            if !(name.starts_with("listens/") && name.ends_with(".jsonl")) {
                continue;
            }
            self.log.info(&format!("Reading listens from {name}"));
            let reader = BufReader::new(entry);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<J>(&line) {
                    Ok(v) => all.push(v),
                    Err(e) => self.log.error(&format!("Invalid JSON in {name}: {e}")),
                }
            }
        }
        Ok(all)
    }

    /// `get_listens`: pages of the user's history, newest first.
    pub fn get_listens(
        &self,
        min_ts: Option<i64>,
        max_ts: Option<i64>,
        count: Option<usize>,
        max_total: Option<usize>,
    ) -> Result<Option<Vec<J>>> {
        if min_ts.is_some() && max_ts.is_some() {
            anyhow::bail!("min_ts and max_ts are mutually exclusive.");
        }
        let per_page = count.unwrap_or(1000).min(1000);
        let url = format!("{}/user/{}/listens", self.root, self.username);
        let mut all: Vec<J> = vec![];
        let mut max_ts = max_ts;
        loop {
            let page_size = match max_total {
                Some(total) => {
                    let remaining = total.saturating_sub(all.len());
                    if remaining == 0 {
                        break;
                    }
                    remaining.min(per_page)
                }
                None => per_page,
            };
            let mut params: Vec<(&str, String)> = vec![("count", page_size.to_string())];
            if let Some(ts) = max_ts {
                params.push(("max_ts", ts.to_string()));
            }
            if let Some(ts) = min_ts {
                params.push(("min_ts", ts.to_string()));
            }
            let Some(response) = self.make_request(&url, Some(&params)) else {
                if all.is_empty() {
                    return Ok(None);
                }
                break;
            };
            let listens = match response.get("payload").and_then(|p| p.get("listens")) {
                Some(J::Array(a)) => a.clone(),
                _ => break,
            };
            if listens.is_empty() {
                break;
            }
            all.extend(listens.iter().cloned());
            self.log
                .info(&format!("Fetched {} listens so far...", all.len()));
            if listens.len() < page_size {
                break;
            }
            let last_ts = listens
                .last()
                .and_then(|l| l.get("listened_at"))
                .and_then(J::as_i64)
                .unwrap_or(0);
            let new_max_ts = last_ts - 1;
            if max_ts.is_some_and(|m| new_max_ts >= m) {
                break;
            }
            max_ts = Some(new_max_ts);
        }
        Ok(Some(all))
    }

    /// `get_tracks_from_listens`: the entries with a release name, by
    /// recording MBID when the listen knows one.
    pub fn get_tracks_from_listens(&self, listens: &[J]) -> Vec<Track> {
        let mut tracks = vec![];
        for listen in listens {
            let Some(meta) = listen.get("track_metadata") else {
                continue;
            };
            if meta.get("release_name").is_none() || meta["release_name"].is_null() {
                continue;
            }
            let additional = meta.get("additional_info").filter(|v| v.is_object());
            let mut mbid = additional.and_then(|a| a.get("recording_mbid")).cloned();
            if mbid.is_none() || mbid.as_ref().is_some_and(J::is_null) {
                mbid = meta
                    .get("mbid_mapping")
                    .filter(|v| v.is_object())
                    .and_then(|m| m.get("recording_mbid"))
                    .cloned();
            }
            let s = |k: &str| {
                meta.get(k)
                    .and_then(J::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            };
            tracks.push(Track {
                album: Some(s("release_name")),
                name: s("track_name"),
                artist: s("artist_name"),
                mbid: mbid.and_then(|v| v.as_str().map(String::from)),
                playcount: 1,
            });
        }
        tracks
    }

    /// `get_mb_recording_id`: the first recording search result.
    #[allow(dead_code)] // beets' plugin API; `lbimport` does not use it.
    pub fn get_mb_recording_id(&self, track: &J) -> Result<Option<String>, HttpError> {
        let meta = &track["track_metadata"];
        let filters = [
            (
                "",
                meta.get("track_name")
                    .and_then(J::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            (
                "release",
                meta.get("release_name")
                    .and_then(J::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
        ];
        let api = MusicBrainzApi::new(&self.cfg);
        let results = api.search("recording", &filters)?;
        Ok(results
            .first()
            .and_then(|r| r.get("id"))
            .and_then(J::as_str)
            .map(String::from))
    }

    /// `get_track_info`: recording title, first artist and first release's
    /// title/year.
    #[allow(dead_code)] // beets' plugin API; `lbimport` does not use it.
    pub fn get_track_info(&self, tracks: &[J]) -> Result<Vec<J>, HttpError> {
        let api = MusicBrainzApi::new(&self.cfg);
        let mut info = vec![];
        for track in tracks {
            let identifier = track.get("identifier").and_then(J::as_str);
            let Some(identifier) = identifier else {
                continue;
            };
            let recording = api.recording_with_releases(identifier)?;
            let title = recording.get("title").cloned().unwrap_or(J::Null);
            let artist = recording
                .get("artist_credit")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .and_then(|c| c.get("artist"))
                .and_then(|a| a.get("name"))
                .cloned()
                .unwrap_or(J::Null);
            let release = recording
                .get("releases")
                .and_then(|r| r.as_array())
                .and_then(|r| r.first());
            let album = release
                .and_then(|r| r.get("title"))
                .cloned()
                .unwrap_or(J::Null);
            let year = release
                .and_then(|r| r.get("date"))
                .and_then(J::as_str)
                .map(|d| d.split('-').next().unwrap_or_default().to_string())
                .map(J::String)
                .unwrap_or(J::Null);
            info.push(serde_json::json!({
                "identifier": identifier,
                "title": title,
                "artist": artist,
                "album": album,
                "year": year,
            }));
        }
        Ok(info)
    }
}

/// The plugin's `__init__`.
pub fn load(cfg: &Config) -> Result<ListenBrainzPlugin> {
    ListenBrainzPlugin::new(cfg)
}

/// `beet lbimport [-f PATH] [--max N]`.
pub fn command(
    cfg: &Config,
    lib: &Library,
    export_file: Option<&str>,
    max_listens: Option<usize>,
) -> Result<()> {
    let plugin = ListenBrainzPlugin::new(cfg)?;
    plugin.lbupdate(lib, export_file, max_listens)
}

#[cfg(test)]
#[path = "listenbrainz_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "listenbrainz",
        order: 39,
        // beets adds no defaults: `token` and `username` must be set.
        defaults: r#""#,
        config_keys: &[],
        redacted: &["listenbrainz.token"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "listenbrainz",
                build: build_lbimport,
                run: run_lbimport,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct LbimportArgs {
    /// path to a ListenBrainz data export .zip file (instead of fetching from the API)
    #[arg(short = 'f', long = "export-file", value_name = "PATH")]
    export_file: Option<String>,
    /// maximum number of listens to fetch via the API (default: all). This option does not apply when importing a file via -f/--export-file.
    #[arg(long = "max", value_name = "MAX_LISTENS")]
    max_listens: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_lbimport() -> clap::Command {
    use clap::Args as _;
    LbimportArgs::augment_args(
        clap::Command::new("lbimport").about("Import ListenBrainz history"),
    )
}

pub fn run_lbimport(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = LbimportArgs::from_arg_matches(m)?;
    command(cfg, lib, a.export_file.as_deref(), a.max_listens)
}
