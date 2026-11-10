//! embedart: embed album art into files' tags (and extract or clear it).

use super::{Plugin, PluginCtx};
use crate::config::Config;
use crate::importer::FileOp;
use crate::media;
use crate::model::{Album, Item, Value};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct EmbedArt {
    auto: bool,
    ifempty: bool,
    remove_art_file: bool,
}

impl EmbedArt {
    pub fn new(cfg: &Config) -> Self {
        EmbedArt {
            auto: cfg.bool("embedart.auto"),
            ifempty: cfg.bool("embedart.ifempty"),
            remove_art_file: cfg.bool("embedart.remove_art_file"),
        }
    }

    /// Embed `image` into one item's file. Returns false if skipped (ifempty).
    pub fn embed_item(&self, ctx: &PluginCtx, item: &mut Item, image: &[u8]) -> Result<bool> {
        if self.ifempty && media::has_art(item.path())? {
            return Ok(false);
        }
        media::embed_art(item.path(), image)?;
        media::refresh_mtime(item);
        ctx.lib.store(item)?;
        Ok(true)
    }

    /// Embed an album's art file into all its items. Returns how many files changed.
    pub fn embed_album(
        &self,
        ctx: &PluginCtx,
        album: &mut Album,
        items: &mut [Item],
    ) -> Result<usize> {
        let art = album.path_field("artpath").to_path_buf();
        if !art.is_file() {
            return Ok(0);
        }
        let image = std::fs::read(&art).with_context(|| format!("reading {}", art.display()))?;
        let mut n = 0;
        for item in items.iter_mut() {
            match self.embed_item(ctx, item, &image) {
                Ok(true) => n += 1,
                Ok(false) => {}
                Err(e) => eprintln!("warning: embedart: {}: {e:#}", item.path().display()),
            }
        }
        if self.remove_art_file && n == items.len() {
            std::fs::remove_file(&art).with_context(|| format!("removing {}", art.display()))?;
            album.set("artpath", Value::Path(PathBuf::new()));
            ctx.lib.store(album)?;
        }
        Ok(n)
    }
}

impl Plugin for EmbedArt {
    fn name(&self) -> &'static str {
        "embedart"
    }

    fn album_imported(
        &self,
        ctx: &PluginCtx,
        album: &mut Album,
        items: &mut [Item],
        _source_dir: &Path,
        _op: FileOp,
    ) -> Result<()> {
        if self.auto {
            let n = self.embed_album(ctx, album, items)?;
            if n > 0 {
                println!("embedart: embedded art in {n} file(s)");
            }
        }
        Ok(())
    }
}
