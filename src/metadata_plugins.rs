//! A port of beets 2.14's `beets/metadata_plugins.py`: the interface a
//! plugin implements to be an extra metadata source (discogs, spotify,
//! deezer, beatport, chroma), and the lookups the autotagger runs over
//! every enabled source.
//!
//! Each source registers itself from its plugin file with an
//! `inventory::submit!` of [`MetadataSourceDef`], like the plugin registry
//! (`crate::plugin_registry`), so adding a source touches only its files.
//! A source's `data_source` is the string stored in items' `data_source`
//! field and compared by the autotagger (`Discogs`, `Spotify`, ...); its
//! `plugin` is its name in the `plugins:` list.
//!
//! MusicBrainz is a metadata source in beets too, but its candidates are
//! built in [`crate::autotag`] from release searches with the
//! [`MusicBrainz`] client the autotagger owns, so it stays out of the
//! trait: the callers add its results themselves, and `count`/`penalty`
//! know its name.

use crate::config::Config;
use crate::mb::{AlbumInfo, MusicBrainz, TrackInfo};
use crate::model::Item;
use anyhow::Result;
use std::collections::BTreeSet;

/// beets' `MetadataSourcePlugin.DEFAULT_DATA_SOURCE_MISMATCH_PENALTY`.
pub const DEFAULT_DATA_SOURCE_MISMATCH_PENALTY: f64 = 0.5;

/// What a metadata source plugin can do (`MetadataSourcePlugin`).
pub trait MetadataSource {
    /// The `data_source` name (`Discogs`, `Spotify`, ...).
    fn data_source(&self) -> &'static str;

    fn album_for_id(&self, ctx: &SourceCtx, id: &str) -> Result<Option<AlbumInfo>>;

    fn track_for_id(&self, ctx: &SourceCtx, id: &str) -> Result<Option<TrackInfo>>;

    /// Album candidates for a search by the given artist and album name
    /// (`candidates`).
    fn candidates(
        &self,
        ctx: &SourceCtx,
        items: &[Item],
        artist: &str,
        album: &str,
        va_likely: bool,
    ) -> Result<Vec<AlbumInfo>>;

    /// Track candidates for a search by the given artist and title
    /// (`item_candidates`).
    fn item_candidates(
        &self,
        ctx: &SourceCtx,
        item: &Item,
        artist: &str,
        title: &str,
    ) -> Result<Vec<TrackInfo>>;
}

/// What a source lookup runs with: the config, and the MusicBrainz source
/// (chroma's candidates resolve through it).
pub struct SourceCtx<'a> {
    pub cfg: &'a Config,
    pub mb: &'a MusicBrainz,
}

/// A plugin's registration as a metadata source.
pub struct MetadataSourceDef {
    /// Order among the sources (the plugin's order in beets' bundled list).
    pub order: u16,
    /// The `plugins:` name.
    pub plugin: &'static str,
    /// The `data_source` the infos it makes carry.
    pub data_source: &'static str,
    /// Whether the source can be built in this run (discogs, spotify and
    /// beatport need credentials their setup would ask for).
    pub configured: fn(&Config) -> bool,
    /// The source's constructor.
    pub make: fn(&Config) -> Result<Box<dyn MetadataSource>>,
}

inventory::collect!(MetadataSourceDef);

/// Every registered source, in beets' bundled-plugin order.
pub fn defs() -> Vec<&'static MetadataSourceDef> {
    let mut defs: Vec<&'static MetadataSourceDef> =
        inventory::iter::<MetadataSourceDef>.into_iter().collect();
    defs.sort_by_key(|d| d.order);
    defs
}

fn def_for(plugin: &str) -> Option<&'static MetadataSourceDef> {
    inventory::iter::<MetadataSourceDef>
        .into_iter()
        .find(|d| d.plugin == plugin)
}

fn def_by_source(data_source: &str) -> Option<&'static MetadataSourceDef> {
    inventory::iter::<MetadataSourceDef>
        .into_iter()
        .find(|d| d.data_source == data_source)
}

/// beets' `find_metadata_source_plugins()`: the enabled metadata source
/// plugins that set up, in the order of the `plugins:` list. A plugin that
/// fails to set up is left out, as beets' `find_plugins` only has loaded
/// ones.
pub fn sources(cfg: &Config) -> Vec<Box<dyn MetadataSource>> {
    let mut out: Vec<Box<dyn MetadataSource>> = vec![];
    for name in crate::plugins::names(cfg) {
        let Some(def) = def_for(&name) else {
            continue;
        };
        if !(def.configured)(cfg) {
            continue;
        }
        match (def.make)(cfg) {
            Ok(source) => out.push(source),
            Err(e) => crate::logging::debug!("** error loading plugin {name}\n{e:#}"),
        }
    }
    out
}

/// beets' `get_metadata_source(name)`: the enabled source with this
/// `data_source` that would set up, if any.
pub fn source(cfg: &Config, data_source: &str) -> Option<Box<dyn MetadataSource>> {
    let def = def_by_source(data_source)?;
    if !crate::plugins::enabled(cfg, def.plugin) || !(def.configured)(cfg) {
        return None;
    }
    (def.make)(cfg).ok()
}

/// How many metadata source plugins are loaded (`find_metadata_source_plugins()`
/// length): the enabled ones that would set up, MusicBrainz included.
pub fn count(cfg: &Config) -> usize {
    let plugins = crate::plugins::names(cfg)
        .iter()
        .filter(|n| def_for(n).is_some_and(|d| (d.configured)(cfg)))
        .count();
    plugins + usize::from(crate::plugins::enabled(cfg, "musicbrainz"))
}

/// The config section holding a source's `source_weight` /
/// `data_source_mismatch_penalty`.
fn source_section(data_source: &str) -> Option<&'static str> {
    if data_source == "MusicBrainz" {
        return Some("musicbrainz");
    }
    def_by_source(data_source).map(|d| d.plugin)
}

/// beets' `get_penalty`: a source's `source_weight` (deprecated) or
/// `data_source_mismatch_penalty`, else the default.
pub fn penalty(cfg: &Config, data_source: &str) -> f64 {
    let Some(plugin) = source_section(data_source) else {
        return DEFAULT_DATA_SOURCE_MISMATCH_PENALTY;
    };
    if cfg.get(&format!("{plugin}.source_weight")).is_some() {
        cfg.f64(&format!("{plugin}.source_weight"))
    } else if cfg
        .get(&format!("{plugin}.data_source_mismatch_penalty"))
        .is_some()
    {
        cfg.f64(&format!("{plugin}.data_source_mismatch_penalty"))
    } else {
        // Not loaded (its settings aren't in the effective configuration).
        DEFAULT_DATA_SOURCE_MISMATCH_PENALTY
    }
}

/// `get_penalty` for every known source, as a lookup table.
pub fn penalties(cfg: &Config) -> std::collections::HashMap<String, f64> {
    let mut out: std::collections::HashMap<String, f64> = defs()
        .iter()
        .map(|d| (d.data_source.to_string(), penalty(cfg, d.data_source)))
        .collect();
    out.insert("MusicBrainz".to_string(), penalty(cfg, "MusicBrainz"));
    out
}

/// beets' `maybe_handle_plugin_error`: a source's failure is logged
/// (`Error in 'Discogs.candidates': ...`) and ignored, unless
/// `raise_on_error` is set.
pub fn handle<T>(
    cfg: &Config,
    source: &dyn MetadataSource,
    method: &str,
    result: Result<Option<T>>,
) -> Result<Option<T>> {
    match result {
        Ok(v) => Ok(v),
        Err(e) if cfg.bool("raise_on_error") => Err(e),
        Err(e) => {
            crate::logging::emit(format!(
                "Error in '{}.{method}': {e:#}",
                source.data_source()
            ));
            Ok(None)
        }
    }
}

/// `handle` for a candidates list: a failure is an empty list.
pub fn handle_vec<T>(
    cfg: &Config,
    source: &dyn MetadataSource,
    method: &str,
    result: Result<Vec<T>>,
) -> Result<Vec<T>> {
    match result {
        Ok(v) => Ok(v),
        Err(e) if cfg.bool("raise_on_error") => Err(e),
        Err(e) => {
            crate::logging::emit(format!(
                "Error in '{}.{method}': {e:#}",
                source.data_source()
            ));
            Ok(vec![])
        }
    }
}

/// beets' `metadata_plugins.albums_for_ids` for one id: every source's
/// `album_for_id` (the ones whose id format doesn't match return None).
pub fn albums_for_ids(cfg: &Config, mb: &MusicBrainz, id: &str) -> Vec<AlbumInfo> {
    let ctx = SourceCtx { cfg, mb };
    let mut out = vec![];
    for source in sources(cfg) {
        let handled = handle(
            cfg,
            source.as_ref(),
            "album_for_id",
            source.album_for_id(&ctx, id),
        );
        match handled {
            Ok(Some(info)) => out.push(info),
            Ok(None) => {}
            Err(_) => return out,
        }
    }
    out
}

/// beets' `metadata_plugins.tracks_for_ids` for one id.
pub fn tracks_for_ids(cfg: &Config, mb: &MusicBrainz, id: &str) -> Vec<TrackInfo> {
    let ctx = SourceCtx { cfg, mb };
    let mut out = vec![];
    for source in sources(cfg) {
        let handled = handle(
            cfg,
            source.as_ref(),
            "track_for_id",
            source.track_for_id(&ctx, id),
        );
        match handled {
            Ok(Some(info)) => out.push(info),
            Ok(None) => {}
            Err(_) => return out,
        }
    }
    out
}

/// beets' `metadata_plugins.candidates`: every source's album candidates,
/// failures logged and skipped.
pub fn candidates(
    cfg: &Config,
    mb: &MusicBrainz,
    items: &[Item],
    artist: &str,
    album: &str,
    va_likely: bool,
) -> Result<Vec<AlbumInfo>> {
    let ctx = SourceCtx { cfg, mb };
    let mut out = vec![];
    for source in sources(cfg) {
        let mut v = handle_vec(
            cfg,
            source.as_ref(),
            "candidates",
            source.candidates(&ctx, items, artist, album, va_likely),
        )?;
        out.append(&mut v);
    }
    Ok(out)
}

/// beets' `metadata_plugins.item_candidates`.
pub fn item_candidates(
    cfg: &Config,
    mb: &MusicBrainz,
    item: &Item,
    artist: &str,
    title: &str,
) -> Result<Vec<TrackInfo>> {
    let ctx = SourceCtx { cfg, mb };
    let mut out = vec![];
    for source in sources(cfg) {
        let mut v = handle_vec(
            cfg,
            source.as_ref(),
            "item_candidates",
            source.item_candidates(&ctx, item, artist, title),
        )?;
        out.append(&mut v);
    }
    Ok(out)
}

/// A string field that is None (empty, and listed in `nulls`) in beets.
pub fn opt_str(
    nulls: &mut BTreeSet<&'static str>,
    field: &'static str,
    v: Option<String>,
) -> String {
    match v {
        Some(s) => s,
        None => {
            nulls.insert(field);
            String::new()
        }
    }
}

/// `get_artist`: the artists' names joined (articles moved to the front),
/// and the first artist's id. `artists` is a list of `(id, name, join)`
/// triples; `join_key` uses a per-artist join word ("Feat.") instead of
/// ", ".
pub fn get_artist(
    artists: &[(&str, &str, Option<&str>)],
    join_key: bool,
) -> (String, Option<String>) {
    let mut artist_id = None;
    let mut artist = String::new();
    let total = artists.len();
    for (idx, (id, name, join)) in artists.iter().enumerate() {
        if artist_id.is_none() {
            artist_id = Some(id.to_string());
        }
        let name = articles_to_front(name);
        if idx < total - 1 {
            match (join_key, join) {
                (true, Some(j)) if !j.is_empty() => {
                    artist.push_str(&name);
                    artist.push(' ');
                    artist.push_str(j);
                    artist.push(' ');
                }
                _ => {
                    artist.push_str(&name);
                    artist.push_str(", ");
                }
            }
        } else {
            artist.push_str(&name);
        }
    }
    (artist, artist_id)
}

/// `re.sub(r"^(.*?), (a|an|the)$", r"\2 \1", name, flags=re.I)`.
pub fn articles_to_front(name: &str) -> String {
    let Some((main, article)) = name.rsplit_once(", ") else {
        return name.to_string();
    };
    if ["a", "an", "the"].contains(&article.to_lowercase().as_str()) {
        format!("{article} {main}")
    } else {
        name.to_string()
    }
}

/// beets' `extract_release_id` for a source: an id out of a bare id or a
/// URL (see [`crate::mb::extract_id`] for MusicBrainz).
pub fn extract_id(source: &str, url: &str) -> Option<String> {
    let pat = match source {
        "spotify" => r"(?:^|open\.spotify\.com/[^/]+/|spotify:[a-z]+:)([0-9A-Za-z]{22})",
        "deezer" => r"(?:^|deezer\.com/)(?:[a-z]*/)?(?:[^/]+/)?(\d+)",
        "beatport" => r"(?:^|beatport\.com/release/.+/)(\d+)$",
        "discogs" => r"(?:^|\[?r|discogs\.com/(?:[^/]+/)?release/)(\d+)\b",
        "musicbrainz" => r"(\w{8}(?:-\w{4}){3}-\w{12})",
        "bandcamp" => r"(.+)",
        "tidal" => r"(?:^|tidal\.com/(?:browse/)?(?:album|track)/)(\d+)",
        // Unknown sources: the id/url as-is (beets logs it at debug).
        _ => return Some(url.to_string()),
    };
    let re = regex::Regex::new(pat).ok()?;
    re.captures(url).map(|c| c[1].to_string())
}

/// A source context for tests whose plugin logic doesn't use it.
#[cfg(test)]
pub mod test_ctx {
    use super::SourceCtx;
    use crate::config::Config;
    use crate::mb::MusicBrainz;
    use std::sync::LazyLock;

    static CFG: LazyLock<Config> = LazyLock::new(Config::defaults);
    static MB: LazyLock<MusicBrainz> = LazyLock::new(|| MusicBrainz::new(&CFG));

    pub fn ctx() -> SourceCtx<'static> {
        SourceCtx { cfg: &CFG, mb: &MB }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(plugins: &str) -> Config {
        let mut cfg = Config::defaults();
        cfg.set("plugins", serde_yaml::from_str(plugins).unwrap());
        cfg
    }

    #[test]
    fn penalties_come_from_the_enabled_sources() {
        let cfg = cfg_with("[musicbrainz]");
        assert_eq!(penalty(&cfg, "MusicBrainz"), 0.5);
        assert_eq!(penalty(&cfg, "Deezer"), 0.5);
        let mut cfg = cfg_with("[musicbrainz, deezer]");
        cfg.set(
            "deezer.data_source_mismatch_penalty",
            serde_yaml::from_str("0.9").unwrap(),
        );
        assert_eq!(penalty(&cfg, "Deezer"), 0.9);
        // `source_weight` wins (deprecated).
        cfg.set("deezer.source_weight", serde_yaml::from_str("1.5").unwrap());
        assert_eq!(penalty(&cfg, "Deezer"), 1.5);
        assert_eq!(penalties(&cfg).get("Deezer"), Some(&1.5));
        assert_eq!(penalties(&cfg).get("Discogs"), Some(&0.5));
        assert_eq!(penalties(&cfg).get("Acoustid"), Some(&0.5));
    }

    #[test]
    fn count_counts_the_sources_that_load() {
        assert_eq!(count(&cfg_with("[musicbrainz]")), 1);
        assert_eq!(count(&cfg_with("[musicbrainz, deezer]")), 2);
        // No credentials: beets' loader leaves these out.
        assert_eq!(
            count(&cfg_with("[musicbrainz, discogs, spotify, beatport]")),
            1
        );
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with("[musicbrainz, spotify, discogs, beatport]");
        cfg.config_dir = dir.path().to_path_buf();
        std::fs::write(
            dir.path().join("spotify_token.json"),
            "{\"access_token\": \"x\"}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("discogs_token.json"),
            "{\"token\": \"t\", \"secret\": \"s\"}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("beatport_token.json"),
            "{\"token\": \"t\", \"secret\": \"s\"}",
        )
        .unwrap();
        assert_eq!(count(&cfg), 4);
    }

    #[test]
    fn every_plugin_source_is_registered() {
        for (plugin, data_source) in [
            ("chroma", "Acoustid"),
            ("discogs", "Discogs"),
            ("spotify", "Spotify"),
            ("deezer", "Deezer"),
            ("beatport", "Beatport"),
            ("tidal", "Tidal"),
        ] {
            let def = defs()
                .into_iter()
                .find(|d| d.plugin == plugin)
                .unwrap_or_else(|| panic!("{plugin} is not registered"));
            assert_eq!(def.data_source, data_source, "{plugin}");
        }
        // A source is built only for an enabled (and configured) plugin;
        // tidal needs no token until a lookup.
        let cfg = cfg_with("[tidal]");
        assert_eq!(count(&cfg), 1);
        assert!(source(&cfg, "Tidal").is_some());
        assert!(source(&cfg, "Discogs").is_none());
        assert!(source(&cfg, "MusicBrainz").is_none());
        let cfg = cfg_with("[musicbrainz]");
        assert!(source(&cfg, "Tidal").is_none());
    }

    #[test]
    fn artists_and_articles() {
        assert_eq!(articles_to_front("Beatles, The"), "The Beatles");
        assert_eq!(articles_to_front("Smith, A"), "A Smith");
        assert_eq!(articles_to_front("Album, Live"), "Album, Live");
        // beets' regex moves the article to the front, even when the name
        // already has one.
        assert_eq!(articles_to_front("The Beatles, The"), "The The Beatles");
        assert_eq!(
            get_artist(&[("1", "A", Some("Feat.")), ("2", "B", None)], true),
            ("A Feat. B".to_string(), Some("1".to_string()))
        );
    }

    #[test]
    fn ids_from_urls() {
        assert_eq!(
            extract_id(
                "spotify",
                "https://open.spotify.com/album/0yhKyyjyKXWUieJ4w1IAEa"
            )
            .as_deref(),
            Some("0yhKyyjyKXWUieJ4w1IAEa")
        );
        assert_eq!(
            extract_id("deezer", "https://www.deezer.com/album/1").as_deref(),
            Some("1")
        );
        assert_eq!(
            extract_id(
                "discogs",
                "https://www.discogs.com/release/123456-something"
            )
            .as_deref(),
            Some("123456")
        );
        assert_eq!(
            extract_id("beatport", "https://beatport.com/release/charade/1742984").as_deref(),
            Some("1742984")
        );
    }
}
