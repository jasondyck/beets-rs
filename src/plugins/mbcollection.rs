//! A port of beets 2.14's `mbcollection` plugin
//! (`beetsplug/mbcollection.py`): `beet mbupdate` adds the library's albums
//! (those with an `mb_albumid`) to the user's MusicBrainz collection, with
//! `-r`/`--remove` also removing releases that are no longer in the
//! library; with `mbcollection.auto` each imported album is added.
//!
//! The collection API is authenticated with `musicbrainz.user` and
//! `musicbrainz.pass` (HTTP digest), and pages/submits in chunks of 100 and
//! 200 respectively.

use crate::config::Config;
use crate::library::Library;
use crate::model::Album;
use crate::plugins::mbapi::MusicBrainzApi;
use crate::plugins::requests::HttpError;
use crate::plugins::{ImportTask, PluginCtx, PluginLog};
use anyhow::Result;
use serde_json::Value as J;
use std::cell::RefCell;
use std::rc::Rc;

/// `UUID_PAT`.
fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

pub struct MusicBrainzCollectionPlugin {
    cfg: Config,
    log: PluginLog,
    /// The cached `collection` property (its data and API).
    collection: RefCell<Option<(J, Rc<MusicBrainzApi>)>>,
    /// `MBCollection.FETCH_CHUNK_SIZE`.
    pub fetch_chunk_size: usize,
    /// `MBCollection.SUBMISSION_CHUNK_SIZE`.
    pub submission_chunk_size: usize,
}

impl crate::plugins::Plugin for MusicBrainzCollectionPlugin {
    fn name(&self) -> &'static str {
        "mbcollection"
    }

    /// beets' `import_stages = [self.imported]`: add the imported album.
    fn import_stage(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if let Some(album) = task.album.as_ref() {
            let albums = [(*album).clone()];
            match self.update_album_list(ctx.lib, &albums, false) {
                Ok(()) => {}
                Err(Failure::Http(e)) => self
                    .log
                    .error(&format!("Failed to update MusicBrainz collection: {e}")),
                Err(Failure::User(m)) => return Err(anyhow::anyhow!(m)),
            }
        }
        Ok(())
    }
}

/// A fetched MusicBrainz collection (`MBCollection`).
pub struct Collection {
    pub data: J,
    pub api: Rc<MusicBrainzApi>,
    /// `FETCH_CHUNK_SIZE`: releases per fetch request.
    pub fetch_chunk_size: usize,
    /// `SUBMISSION_CHUNK_SIZE`: MBIDs per add/remove request.
    pub submission_chunk_size: usize,
}

impl Collection {
    pub fn new(data: J, api: Rc<MusicBrainzApi>) -> Self {
        Collection {
            data,
            api,
            fetch_chunk_size: 100,
            submission_chunk_size: 200,
        }
    }

    pub fn id(&self) -> String {
        self.data["id"].as_str().unwrap_or_default().to_string()
    }

    pub fn release_count(&self) -> usize {
        self.data["release_count"].as_u64().unwrap_or(0) as usize
    }

    pub fn releases_url(&self) -> String {
        format!("{}/collection/{}/releases", self.api.api_root, self.id())
    }

    /// A page of the collection's releases.
    pub fn get_releases(&self, offset: usize) -> Result<Vec<J>, HttpError> {
        let data = self.api.get_json(
            &self.releases_url(),
            &[
                ("limit", self.fetch_chunk_size.to_string()),
                ("offset", offset.to_string()),
            ],
        )?;
        Ok(match data.get("releases") {
            Some(J::Array(a)) => a.clone(),
            _ => vec![],
        })
    }

    /// All releases, fetched in successive pages.
    pub fn releases(&self) -> Result<Vec<J>, HttpError> {
        let mut out = vec![];
        let mut offset = 0;
        while offset < self.release_count() {
            out.extend(self.get_releases(offset)?);
            offset += self.fetch_chunk_size;
        }
        Ok(out)
    }

    pub fn add_releases(&self, releases: &[String]) -> Result<(), HttpError> {
        for chunk in releases.chunks(self.submission_chunk_size) {
            self.api
                .put(&format!("{}/{}", self.releases_url(), chunk.join("%3B")))?;
        }
        Ok(())
    }

    pub fn remove_releases(&self, releases: &[String]) -> Result<(), HttpError> {
        for chunk in releases.chunks(self.submission_chunk_size) {
            self.api
                .delete(&format!("{}/{}", self.releases_url(), chunk.join("%3B")))?;
        }
        Ok(())
    }
}

/// Why updating failed: the command logs HTTP errors, and reports
/// everything else (`UserError` in beets).
pub enum Failure {
    Http(HttpError),
    User(String),
}

impl From<HttpError> for Failure {
    fn from(e: HttpError) -> Self {
        Failure::Http(e)
    }
}

impl MusicBrainzCollectionPlugin {
    pub fn new(cfg: &Config) -> Self {
        MusicBrainzCollectionPlugin {
            cfg: cfg.clone(),
            log: PluginLog::new("mbcollection"),
            collection: RefCell::new(None),
            fetch_chunk_size: 100,
            submission_chunk_size: 200,
        }
    }

    fn api(&self) -> Rc<MusicBrainzApi> {
        Rc::new(MusicBrainzApi::new(&self.cfg))
    }

    /// beets' `collection` property: the configured release collection, or
    /// the first one.
    fn wrap(&self, data: J, api: Rc<MusicBrainzApi>) -> Collection {
        let mut c = Collection::new(data, api);
        c.fetch_chunk_size = self.fetch_chunk_size;
        c.submission_chunk_size = self.submission_chunk_size;
        c
    }

    pub fn collection(&self) -> Result<Collection, Failure> {
        if let Some((data, api)) = self.collection.borrow().as_ref() {
            return Ok(self.wrap(data.clone(), api.clone()));
        }
        let api = self.api();
        let data = api
            .get_json(&format!("{}/collection", api.api_root), &[])
            .map_err(Failure::Http)?;
        let collections: Vec<J> = match data.get("collections") {
            Some(J::Array(a)) => a.clone(),
            _ => vec![],
        };
        if collections.is_empty() {
            return Err(Failure::User("no collections exist for user".into()));
        }
        let by_id: std::collections::BTreeMap<String, J> = collections
            .into_iter()
            .filter(|c| c["entity_type"] == "release")
            .map(|c| (c["id"].as_str().unwrap_or_default().to_string(), c))
            .collect();
        if by_id.is_empty() {
            return Err(Failure::User("No release collection found.".into()));
        }
        let wanted = self.cfg.str("mbcollection.collection");
        let collection = if !wanted.is_empty() {
            by_id
                .get(&wanted)
                .cloned()
                .ok_or_else(|| Failure::User(format!("invalid collection ID: {wanted}")))?
        } else {
            by_id.into_values().next().unwrap()
        };
        let collection = self.wrap(collection, api);
        *self.collection.borrow_mut() = Some((collection.data.clone(), collection.api.clone()));
        Ok(collection)
    }

    /// `update_album_list`: add the given albums' MBIDs to the collection,
    /// optionally removing releases that are no longer in the library.
    pub fn update_album_list(
        &self,
        lib: &Library,
        albums: &[Album],
        remove_missing: bool,
    ) -> Result<(), Failure> {
        let collection = self.collection()?;
        let album_ids: Vec<String> = albums
            .iter()
            .map(|a| a.str("mb_albumid").to_string())
            .filter(|id| is_uuid(id))
            .collect();
        self.log.info(&format!(
            "Updating MusicBrainz collection {}...",
            collection.id()
        ));
        collection.add_releases(&album_ids).map_err(Failure::Http)?;
        if remove_missing {
            let query = crate::query::Query::parse(crate::model::Kind::Album, &[] as &[String])
                .map_err(|e| Failure::User(format!("{e}")))?;
            let lib_ids: std::collections::BTreeSet<String> = lib
                .albums(&query)
                .map_err(|e| Failure::User(format!("{e}")))?
                .iter()
                .map(|a| a.str("mb_albumid").to_string())
                .collect();
            let in_collection: Vec<String> = collection
                .releases()
                .map_err(Failure::Http)?
                .iter()
                .map(|r| r["id"].as_str().unwrap_or_default().to_string())
                .collect();
            let remove: Vec<String> = in_collection
                .into_iter()
                .filter(|id| !lib_ids.contains(id))
                .collect();
            collection.remove_releases(&remove).map_err(Failure::Http)?;
        }
        self.log.info("...MusicBrainz collection updated.");
        Ok(())
    }
}

/// `beet mbupdate [-r]`: with `remove` from the command line, else
/// `mbcollection.remove`.
pub fn command(cfg: &Config, lib: &Library, remove: Option<bool>) -> Result<()> {
    command_chunked(cfg, lib, remove, 100, 200)
}

/// `command` with the collection's chunk sizes (beets' tests patch the
/// class constants; this is the same knob).
pub fn command_chunked(
    cfg: &Config,
    lib: &Library,
    remove: Option<bool>,
    fetch_chunk_size: usize,
    submission_chunk_size: usize,
) -> Result<()> {
    let mut cfg = cfg.clone();
    if let Some(r) = remove {
        cfg.set("mbcollection.remove", serde_yaml::Value::Bool(r));
    }
    let mut plugin = MusicBrainzCollectionPlugin::new(&cfg);
    plugin.fetch_chunk_size = fetch_chunk_size;
    plugin.submission_chunk_size = submission_chunk_size;
    let albums = lib.albums(&crate::query::Query::parse(
        crate::model::Kind::Album,
        &[] as &[String],
    )?)?;
    match plugin.update_album_list(lib, &albums, cfg.bool("mbcollection.remove")) {
        Ok(()) => Ok(()),
        Err(Failure::Http(e)) => {
            plugin
                .log
                .error(&format!("Failed to update MusicBrainz collection: {e}"));
            Ok(())
        }
        Err(Failure::User(m)) => Err(anyhow::anyhow!(m)),
    }
}

/// The plugin's `__init__`: its defaults are `auto: no`, `collection: ''`,
/// `remove: no`; with `auto`, each imported album is added.
pub fn load(cfg: &Config) -> MusicBrainzCollectionPlugin {
    MusicBrainzCollectionPlugin::new(cfg)
}

#[cfg(test)]
#[path = "mbcollection_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "mbcollection",
        order: 40,
        defaults: r#"mbcollection:
    auto: no
    collection: ''
    remove: no
"#,
        config_keys: &["mbcollection"],
        // `musicbrainz.pass` is not redacted here: beets sets that flag only
        // when the collection API is first used, so `beet config` shows it.
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 430,
                plugin: "mbcollection",
                build: build_mbupdate,
                run: run_mbupdate,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct MbupdateArgs {
    /// Remove albums not in beets library
    #[arg(short = 'r', long)]
    remove: bool,
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_mbupdate() -> clap::Command {
    use clap::Args as _;
    MbupdateArgs::augment_args(
        clap::Command::new("mbupdate").about("Update MusicBrainz collection (plugin: mbcollection)"),
    )
}

pub fn run_mbupdate(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = MbupdateArgs::from_arg_matches(m)?;
    command(cfg, lib, a.remove.then_some(true))
}
