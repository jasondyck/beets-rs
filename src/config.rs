//! Configuration: beets-compatible YAML keys layered over built-in defaults.

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value as Y};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = r#"
directory: ~/Music
library: library.db

import:
    write: yes
    copy: yes
    move: no
    quiet: no
    quiet_fallback: skip
    timid: no
    autotag: yes
    singletons: no
    duplicate_action: ask

paths:
    default: $albumartist/$album%aunique{}/$track $title
    singleton: Non-Album/$artist/$title
    comp: Compilations/$album%aunique{}/$track $title

replace:
    '[\\/]': _
    '^\.': _
    '[\x00-\x1f]': ''
    '[<>:"\?\*\|]': _
    '\.$': _
    '\s+$': ''
    '^\s+': ''
    '^-': _

asciify_paths: no
max_filename_length: 0
per_disc_numbering: no
original_date: no
va_name: Various Artists
format_item: $artist - $album - $title
format_album: $albumartist - $album
sort_item: albumartist+ album+ disc+ track+
sort_album: albumartist+ album+

aunique:
    keys: albumartist album
    disambiguators: albumtype year label catalognum albumdisambig releasegroupdisambig
    bracket: '[]'

match:
    strong_rec_thresh: 0.04
    medium_rec_thresh: 0.25
    rec_gap_thresh: 0.25
    max_rec:
        missing_tracks: medium
        unmatched_tracks: medium
    distance_weights:
        artist: 3.0
        album: 3.0
        year: 1.0
        media: 1.0
        mediums: 1.0
        country: 0.5
        label: 0.5
        catalognum: 0.5
        albumdisambig: 0.5
        album_id: 5.0
        tracks: 2.0
        missing_tracks: 0.9
        unmatched_tracks: 0.6
        track_title: 3.0
        track_artist: 2.0
        track_index: 1.0
        track_length: 2.0
        track_id: 5.0
    track_length_grace: 10
    track_length_max: 30

musicbrainz:
    host: musicbrainz.org
    https: yes
    ratelimit: 1
    ratelimit_interval: 1.0
    searchlimit: 5
"#;

#[derive(Debug, Clone)]
pub struct Config {
    root: Y,
    /// Directory holding config.yaml; relative `library` paths resolve against it.
    pub config_dir: PathBuf,
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if p == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(p)
}

/// Default config directory: $BEETSRSDIR, else ~/.config/beets-rs.
/// (Not ~/.config/beets, so we never touch a real beets library.db.)
pub fn default_config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BEETSRSDIR") {
        return expand_tilde(&d);
    }
    dirs::config_dir()
        .unwrap_or_else(|| expand_tilde("~/.config"))
        .join("beets-rs")
}

/// Deep-merge `over` onto `base`. Mapping keys from `over` come first so a
/// user's `paths:` entries take priority over the defaults' order.
fn merge(base: Y, over: Y) -> Y {
    match (base, over) {
        (Y::Mapping(base), Y::Mapping(over)) => {
            let mut out = Mapping::new();
            let mut base = base;
            for (k, v) in over {
                let merged = match base.remove(&k) {
                    Some(b) => merge(b, v),
                    None => v,
                };
                out.insert(k, merged);
            }
            for (k, v) in base {
                out.insert(k, v);
            }
            Y::Mapping(out)
        }
        (_, over) => over,
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let (file, dir) = match path {
            Some(p) => (
                p.to_path_buf(),
                p.parent().map(Path::to_path_buf).unwrap_or_default(),
            ),
            None => {
                let d = default_config_dir();
                (d.join("config.yaml"), d)
            }
        };
        let mut root: Y = serde_yaml::from_str(DEFAULT_CONFIG).expect("default config parses");
        if file.exists() {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let user: Y = serde_yaml::from_str(&text)
                .with_context(|| format!("parsing {}", file.display()))?;
            if !user.is_null() {
                root = merge(root, user);
            }
        } else if path.is_some() {
            anyhow::bail!("config file {} does not exist", file.display());
        }
        Ok(Config {
            root,
            config_dir: dir,
        })
    }

    #[cfg(test)]
    pub fn defaults() -> Config {
        Config {
            root: serde_yaml::from_str(DEFAULT_CONFIG).unwrap(),
            config_dir: default_config_dir(),
        }
    }

    /// Override a dotted key from the command line.
    pub fn set(&mut self, key: &str, value: Y) {
        let mut cur = &mut self.root;
        let parts: Vec<&str> = key.split('.').collect();
        for (i, part) in parts.iter().enumerate() {
            if !cur.is_mapping() {
                *cur = Y::Mapping(Mapping::new());
            }
            let map = cur.as_mapping_mut().unwrap();
            let k = Y::String(part.to_string());
            if i == parts.len() - 1 {
                map.insert(k, value);
                return;
            }
            if !map.contains_key(&k) {
                map.insert(k.clone(), Y::Mapping(Mapping::new()));
            }
            cur = map.get_mut(&k).unwrap();
        }
    }

    pub fn get(&self, key: &str) -> Option<&Y> {
        let mut cur = &self.root;
        for part in key.split('.') {
            cur = cur.as_mapping()?.get(Y::String(part.to_string()))?;
        }
        Some(cur)
    }

    pub fn str(&self, key: &str) -> String {
        match self.get(key) {
            Some(Y::String(s)) => s.clone(),
            Some(Y::Number(n)) => n.to_string(),
            Some(Y::Bool(b)) => b.to_string(),
            _ => String::new(),
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Y::Bool(b)) => *b,
            Some(Y::String(s)) => crate::model::parse_bool(s).unwrap_or(false),
            Some(Y::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => false,
        }
    }

    pub fn f64(&self, key: &str) -> f64 {
        match self.get(key) {
            Some(Y::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Y::String(s)) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn mapping(&self, key: &str) -> Vec<(String, String)> {
        match self.get(key) {
            Some(Y::Mapping(m)) => m
                .iter()
                .filter_map(|(k, v)| {
                    let k = match k {
                        Y::String(s) => s.clone(),
                        other => serde_yaml::to_string(other).ok()?.trim().to_string(),
                    };
                    let v = match v {
                        Y::String(s) => s.clone(),
                        Y::Null => String::new(),
                        other => serde_yaml::to_string(other).ok()?.trim().to_string(),
                    };
                    Some((k, v))
                })
                .collect(),
            _ => vec![],
        }
    }

    pub fn directory(&self) -> PathBuf {
        let d = expand_tilde(&self.str("directory"));
        if d.is_absolute() {
            d
        } else {
            std::env::current_dir().unwrap_or_default().join(d)
        }
    }

    pub fn library_path(&self) -> PathBuf {
        let l = expand_tilde(&self.str("library"));
        if l.is_absolute() {
            l
        } else {
            self.config_dir.join(l)
        }
    }

    pub fn dump(&self) -> String {
        serde_yaml::to_string(&self.root).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_user_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(
            &file,
            "directory: /srv/music\nimport:\n  move: yes\npaths:\n  'genre:jazz': Jazz/$album/$title\n",
        )
        .unwrap();
        let c = Config::load(Some(&file)).unwrap();
        assert_eq!(c.directory(), PathBuf::from("/srv/music"));
        assert!(c.bool("import.move"));
        assert!(c.bool("import.copy"));
        let paths = c.mapping("paths");
        assert_eq!(paths[0].0, "genre:jazz");
        assert!(paths.iter().any(|(k, _)| k == "default"));
        assert_eq!(c.library_path(), dir.path().join("library.db"));
        assert_eq!(c.f64("match.strong_rec_thresh"), 0.04);
    }
}
