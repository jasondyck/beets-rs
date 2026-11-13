//! A port of beets 2.14's `subsonicplaylist` plugin
//! (`beetsplug/subsonicplaylist.py`): `beet subsonicplaylist` fetches
//! Subsonic playlists and tags the library items in them with the
//! `subsonic_playlist` flexible attribute.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind};
use crate::plugins::requests::{Request as HttpRequest, Session};
use crate::plugins::{Plugin, PluginLog};
use crate::query::Query;
use anyhow::{Result, anyhow};
use md5::{Digest as _, Md5};
use roxmltree::Document;
use std::collections::BTreeMap;
use std::collections::HashSet;

pub struct SubsonicPlaylistPlugin;

impl Plugin for SubsonicPlaylistPlugin {
    fn name(&self) -> &'static str {
        "subsonicplaylist"
    }
}

/// A playlist entry: (artist, album, title).
pub type TrackKey = (String, String, String);

/// beets' `filter_to_be_removed`.
pub fn filter_to_be_removed(items: &[Item], keys: &HashSet<TrackKey>) -> Vec<Item> {
    let matches = |item: &Item, key: &TrackKey| {
        key.0 == item.str("artist") && key.1 == item.str("album") && key.2 == item.str("title")
    };
    if items.len() > keys.len() {
        let mut dont_remove: Vec<usize> = vec![];
        for key in keys {
            for (i, item) in items.iter().enumerate() {
                if matches(item, key) && !dont_remove.contains(&i) {
                    dont_remove.push(i);
                }
            }
        }
        items
            .iter()
            .enumerate()
            .filter(|(i, _)| !dont_remove.contains(i))
            .map(|(_, i)| i.clone())
            .collect()
    } else {
        items
            .iter()
            .filter(|item| !keys.iter().any(|key| matches(item, key)))
            .cloned()
            .collect()
    }
}

/// Python's `random.choices(string.ascii_lowercase + string.digits)` salt.
fn salt() -> String {
    // A plain deterministic-enough source: the process id and time, mapped
    // to the same alphabet (beets' only requirement is a random string).
    let mut n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64) << 32;
    let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    let mut out = String::new();
    for _ in 0..10 {
        out.push(alphabet[(n % alphabet.len() as u64) as usize]);
        n /= alphabet.len() as u64 + 7;
        n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    out
}

struct State {
    cfg: Config,
    log: PluginLog,
    session: Session,
}

impl State {
    fn str(&self, key: &str) -> String {
        self.cfg.str(&format!("subsonicplaylist.{key}"))
    }

    fn list(&self, key: &str) -> Vec<String> {
        self.cfg.list(&format!("subsonicplaylist.{key}"))
    }

    /// beets' `generate_token`: the password's MD5 with a random salt.
    fn generate_token(&self) -> (String, String) {
        let salt = salt();
        let token = hex::encode(Md5::digest(format!("{}{salt}", self.str("password"))));
        (token, salt)
    }

    /// beets' `send`: a Subsonic REST GET.
    fn send(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<String> {
        let (token, salt) = self.generate_token();
        let mut all: Vec<(&str, String)> = vec![
            ("u", self.str("username")),
            ("t", token),
            ("s", salt),
            ("v", "1.12.0".into()),
            ("c", "beets".into()),
        ];
        for (k, v) in params {
            all.push((k, (*v).to_string()));
        }
        let base = self.str("base_url");
        let query = crate::plugins::requests::urlencode(&all);
        let url = format!("{base}/rest/{endpoint}?{query}");
        let resp = self
            .session
            .get(&url, HttpRequest::default())
            .map_err(|e| anyhow!("{e}"))?;
        Ok(resp.text(true))
    }

    /// beets' `get_playlist`: the playlist's name and its tracks.
    fn get_playlist(&self, id: &str) -> Result<Option<(String, Vec<TrackKey>)>> {
        let xml = self.send("getPlaylist", &[("id", id)])?;
        let doc = Document::parse(&xml)?;
        let root = doc.root_element();
        let Some(playlist) = root.children().find(|n| n.is_element()) else {
            return Ok(None);
        };
        // Beets checks the first child's `code` attribute for an API error.
        if let Some(code) = playlist.attribute("code")
            && code != "200"
        {
            self.log.warning(
                playlist
                    .attribute("message")
                    .unwrap_or("error getting playlist, but no error message found"),
            );
            return Ok(None);
        }
        let name = playlist.attribute("name").unwrap_or("undefined").to_string();
        let tracks = playlist
            .children()
            .filter(|n| n.is_element())
            .map(|t| {
                (
                    t.attribute("artist").unwrap_or_default().to_string(),
                    t.attribute("album").unwrap_or_default().to_string(),
                    t.attribute("title").unwrap_or_default().to_string(),
                )
            })
            .collect();
        Ok(Some((name, tracks)))
    }

    /// beets' `get_playlists`.
    fn get_playlists(&self, ids: &[String]) -> BTreeMap<TrackKey, String> {
        let mut out: BTreeMap<TrackKey, String> = BTreeMap::new();
        for id in ids {
            if let Ok(Some((name, tracks))) = self.get_playlist(id) {
                for track in tracks {
                    let entry = out.entry(track).or_insert_with(|| ";".to_string());
                    entry.push_str(&format!("{name};"));
                }
            }
        }
        out
    }

    /// The playlist ids named in `playlist_names` (from `getPlaylists`).
    fn named_playlist_ids(&self) -> Result<Vec<String>> {
        let xml = self.send("getPlaylists", &[])?;
        let doc = Document::parse(&xml)?;
        let root = doc.root_element();
        let Some(playlists) = root.children().find(|n| n.is_element()) else {
            return Ok(vec![]);
        };
        if let Some(code) = playlists.attribute("code")
            && code != "200"
        {
            self.log.warning(
                playlists
                    .attribute("message")
                    .unwrap_or("error getting playlists, but no error message found"),
            );
            return Ok(vec![]);
        }
        let names = self.list("playlist_names");
        let mut ids = vec![];
        for name in names {
            for playlist in playlists.children().filter(|n| n.is_element()) {
                if playlist.attribute("name") == Some(name.as_str())
                    && let Some(id) = playlist.attribute("id")
                {
                    ids.push(id.to_string());
                }
            }
        }
        Ok(ids)
    }

    /// beets' `update_tags`: set (and write) `subsonic_playlist`.
    fn update_tags(&self, playlists: &BTreeMap<TrackKey, String>, lib: &Library) -> Result<()> {
        let items = lib.items(&Query::parse::<&str>(Kind::Item, &[])?)?;
        for (key, tag) in playlists {
            let matches: Vec<Item> = items
                .iter()
                .filter(|i| {
                    i.str("artist") == key.0 && i.str("album") == key.1 && i.str("title") == key.2
                })
                .cloned()
                .collect();
            if matches.is_empty() {
                self.log.warning(&format!("{tag} | track not found ({key:?})"));
                continue;
            }
            for mut item in matches {
                item.set_str("subsonic_playlist", tag.clone());
                if let Err(e) = crate::media::write_item(&mut item) {
                    self.log.warning(&format!("{e:#}"));
                }
                // beets' `try_sync`: a store failure aborts the command.
                lib.store(&item)?;
            }
        }
        Ok(())
    }

    /// beets' `commands`' `build_playlist`.
    fn build_playlist(&self, lib: &Library, delete: bool) -> Result<()> {
        let mut ids = self.list("playlist_ids");
        if !self.list("playlist_names").is_empty() {
            ids.extend(self.named_playlist_ids()?);
        }
        let playlist_dict = self.get_playlists(&ids);
        if delete {
            let existing: Vec<Item> = lib
                .items(&Query::parse::<&str>(Kind::Item, &[])?)?
                .into_iter()
                .filter(|i| !i.str("subsonic_playlist").is_empty())
                .collect();
            let keys: HashSet<TrackKey> = playlist_dict.keys().cloned().collect();
            for mut item in filter_to_be_removed(&existing, &keys) {
                item.set_str("subsonic_playlist", "");
                if let Err(e) = crate::media::write_item(&mut item) {
                    self.log.warning(&format!("{e:#}"));
                }
                // beets' `try_sync`: a store failure aborts the command.
                lib.store(&item)?;
            }
        }
        self.update_tags(&playlist_dict, lib)
    }
}

pub fn load(cfg: &Config) -> SubsonicPlaylistPlugin {
    let log = PluginLog::new("subsonicplaylist");
    log.set_base_level(crate::plugins::INFO);
    // The `subsonic_playlist` attribute is written by the command; the
    // state is only needed when it runs.
    let _ = State {
        cfg: cfg.clone(),
        log,
        session: Session::plain("beets/2.14.1"),
    };
    SubsonicPlaylistPlugin
}

/// The command (`beet subsonicplaylist [-d]`).
pub fn command(cfg: &Config, lib: &Library, delete: bool) -> Result<()> {
    let log = PluginLog::new("subsonicplaylist");
    log.set_base_level(crate::plugins::INFO);
    let state = State {
        cfg: cfg.clone(),
        log,
        session: Session::plain("beets/2.14.1"),
    };
    state.build_playlist(lib, delete || cfg.bool("subsonicplaylist.delete"))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct SubsonicplaylistArgs {
    /// delete tag from items not in any playlist anymore
    #[arg(short = 'd', long)]
    delete: bool,
}

pub fn build_subsonicplaylist() -> clap::Command {
    use clap::Args as _;
    SubsonicplaylistArgs::augment_args(
        clap::Command::new("subsonicplaylist").about("import a subsonic playlist"),
    )
}

pub fn run_subsonicplaylist(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = SubsonicplaylistArgs::from_arg_matches(m)?;
    command(cfg, lib, a.delete)
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "subsonicplaylist",
        order: 65,
        defaults: r#"subsonicplaylist:
    delete: false
    playlist_ids: []
    playlist_names: []
    username: ''
    password: ''
"#,
        config_keys: &["subsonicplaylist"],
        item_types: &[],
        album_types: &[],
        redacted: &["subsonicplaylist.password"],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "subsonicplaylist",
            build: build_subsonicplaylist,
            run: run_subsonicplaylist,
        }],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

#[cfg(test)]
#[path = "subsonicplaylist_tests.rs"]
mod tests;
