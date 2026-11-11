//! Persistent import state (beets' `state.pickle`, here JSON): which
//! directories have been imported (for `import.incremental`) and how far an
//! interrupted import got (for `import.resume`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportState {
    /// Album directories (or single files) already imported; incremental
    /// imports skip these without reading them.
    #[serde(default)]
    pub history: BTreeSet<PathBuf>,
    /// For each top-level import path still in progress, the album
    /// directories finished so far. Removed when the path completes.
    #[serde(default)]
    pub progress: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
}

impl ImportState {
    /// Load the state file; a missing file is empty state, and an unreadable
    /// one is reported and treated as empty rather than blocking imports.
    pub fn load(path: &Path) -> ImportState {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ImportState::default(),
            Err(e) => {
                eprintln!("warning: reading {}: {e}", path.display());
                return ImportState::default();
            }
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!(
                "warning: ignoring corrupt state file {}: {e}",
                path.display()
            );
            ImportState::default()
        })
    }

    /// Write atomically (temp file + rename) so a crash mid-save can't
    /// leave a truncated state file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_corrupt_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/state.json");
        assert!(ImportState::load(&p).history.is_empty());
        let mut s = ImportState::default();
        s.history.insert("/music/a".into());
        s.progress
            .entry("/music".into())
            .or_default()
            .insert("/music/a".into());
        s.save(&p).unwrap();
        let back = ImportState::load(&p);
        assert_eq!(back.history, s.history);
        assert_eq!(back.progress, s.progress);
        std::fs::write(&p, b"{not json").unwrap();
        assert!(ImportState::load(&p).progress.is_empty());
    }
}
