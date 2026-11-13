//! Port of beets 2.14's `albumtypes` plugin: the `$atypes` album field,
//! the album's types (`albumtypes`) as configured (`types`: pairs of type
//! and label, in order), each in `bracket`, leaving out `ignore_va` types
//! for Various Artists albums.

use crate::config::Config;
use crate::model::Record;
use serde_yaml::Value as Y;

/// MusicBrainz's Various Artists id.
pub const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

#[derive(Debug)]
pub struct AlbumTypes {
    types: Vec<(String, Option<String>)>,
    ignore_va: Vec<String>,
    bracket: String,
}

fn scalar(v: &Y) -> Option<String> {
    match v {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

impl AlbumTypes {
    pub fn new(cfg: &Config) -> Self {
        // confuse's `as_pairs`: [key, value] lists, one-entry mappings or
        // bare keys.
        let mut types = vec![];
        if let Some(Y::Sequence(seq)) = cfg.get("albumtypes.types") {
            for t in seq {
                match t {
                    Y::Sequence(p) if !p.is_empty() => {
                        types.push((scalar(&p[0]).unwrap_or_default(), p.get(1).and_then(scalar)));
                    }
                    Y::Mapping(m) => {
                        for (k, v) in m {
                            types.push((scalar(k).unwrap_or_default(), scalar(v)));
                        }
                    }
                    other => {
                        if let Some(s) = scalar(other) {
                            types.push((s, None));
                        }
                    }
                }
            }
        }
        AlbumTypes {
            types,
            ignore_va: cfg.list("albumtypes.ignore_va"),
            bracket: cfg.str("albumtypes.bracket"),
        }
    }

    /// `_atypes`.
    pub fn atypes(&self, album: &Record) -> String {
        let b: Vec<char> = self.bracket.chars().collect();
        let (l, r) = if b.len() == 2 {
            (b[0].to_string(), b[1].to_string())
        } else {
            (String::new(), String::new())
        };
        let albumtypes = album.list("albumtypes");
        let is_va = album.str("mb_albumartistid") == VARIOUS_ARTISTS_ID;
        let mut out = String::new();
        for (t, label) in &self.types {
            let Some(label) = label.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            if albumtypes.contains(t) && (!is_va || !self.ignore_va.contains(t)) {
                out.push_str(&format!("{l}{label}{r}"));
            }
        }
        out
    }
}

pub struct AlbumTypesPlugin;

impl super::Plugin for AlbumTypesPlugin {
    fn name(&self) -> &'static str {
        "albumtypes"
    }
}

#[cfg(test)]
mod tests {
    //! Ported from beets' `test/plugins/test_albumtypes.py`.
    use super::*;

    fn plugin(cfg_yaml: Option<&str>) -> AlbumTypes {
        let mut cfg = Config::defaults();
        if let Some(y) = cfg_yaml {
            cfg.set("albumtypes", serde_yaml::from_str(y).unwrap());
        }
        AlbumTypes::new(&cfg)
    }

    fn album(types: &[&str], artist_id: &str) -> Record {
        let mut a = Record::new_album();
        a.set_list("albumtypes", types);
        a.set_str("mb_albumartistid", artist_id);
        a
    }

    #[test]
    fn renames_types() {
        let p = plugin(Some(
            "{types: [[ep, EP], [remix, Remix]], ignore_va: [], bracket: '()'}",
        ));
        assert_eq!(p.atypes(&album(&["ep", "remix"], "0")), "(EP)(Remix)");
    }

    #[test]
    fn returns_only_specified_types() {
        let p = plugin(Some(
            "{types: [[ep, EP], [soundtrack, '']], ignore_va: [], bracket: '()'}",
        ));
        assert_eq!(
            p.atypes(&album(&["ep", "remix", "soundtrack"], "0")),
            "(EP)"
        );
    }

    #[test]
    fn respects_type_order() {
        let p = plugin(Some(
            "{types: [[remix, Remix], [ep, EP]], ignore_va: [], bracket: '()'}",
        ));
        assert_eq!(p.atypes(&album(&["ep", "remix"], "0")), "(Remix)(EP)");
    }

    #[test]
    fn ignores_va() {
        let p = plugin(Some(
            "{types: [[ep, EP], [soundtrack, OST]], ignore_va: [ep], bracket: '()'}",
        ));
        assert_eq!(
            p.atypes(&album(&["ep", "soundtrack"], VARIOUS_ARTISTS_ID)),
            "(OST)"
        );
    }

    #[test]
    fn respects_defaults() {
        let p = plugin(None);
        assert_eq!(
            p.atypes(&album(
                &["ep", "single", "soundtrack", "live", "compilation", "remix"],
                VARIOUS_ARTISTS_ID
            )),
            "[EP][Single][OST][Live][Remix]"
        );
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "albumtypes",
        order: 31,
        defaults: r#"albumtypes:
    types:
    - [ep, EP]
    - [single, Single]
    - [soundtrack, OST]
    - [live, Live]
    - [compilation, Anthology]
    - [remix, Remix]
    ignore_va: [compilation]
    bracket: '[]'
"#,
        config_keys: &["albumtypes"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(AlbumTypesPlugin))
}
