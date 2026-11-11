//! fetchart: find album art in the source folder or on the Cover Art Archive
//! and store it as `cover.<ext>` next to the album's files.

use super::{Plugin, PluginCtx, http_agent};
use crate::config::Config;
use crate::fileops::FileOp;
use crate::model::{Album, Item, Value};
use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::{Path, PathBuf};

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif"];
const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;

pub struct FetchArt {
    auto: bool,
    sources: Vec<String>,
    cover_names: Vec<String>,
    art_filename: String,
    maxwidth: u32,
    agent: ureq::Agent,
}

enum Found {
    File(PathBuf),
    Download { bytes: Vec<u8>, ext: &'static str },
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The directory an album's files live in (that of its first item).
pub fn album_dir(items: &[Item]) -> Option<PathBuf> {
    items
        .first()
        .and_then(|i| i.path().parent())
        .map(Path::to_path_buf)
}

fn ext_for(content_type: &str, bytes: &[u8]) -> Option<&'static str> {
    match content_type.split(';').next().unwrap_or("").trim() {
        "image/jpeg" | "image/jpg" => return Some("jpg"),
        "image/png" => return Some("png"),
        "image/webp" => return Some("webp"),
        "image/gif" => return Some("gif"),
        _ => {}
    }
    // Sniff when the server is vague (application/octet-stream).
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some("jpg"),
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("webp"),
        [b'G', b'I', b'F', ..] => Some("gif"),
        _ => None,
    }
}

impl FetchArt {
    pub fn new(cfg: &Config) -> Self {
        FetchArt {
            auto: cfg.bool("fetchart.auto"),
            sources: cfg.list("fetchart.sources"),
            cover_names: cfg
                .list("fetchart.cover_names")
                .into_iter()
                .map(|s| s.to_lowercase())
                .collect(),
            art_filename: cfg.str("fetchart.art_filename"),
            maxwidth: cfg.f64("fetchart.maxwidth").max(0.0) as u32,
            agent: http_agent(),
        }
    }

    /// Best image in a directory: one whose name contains a cover name
    /// (earlier names win), else the only image there.
    fn search_filesystem(&self, dir: &Path) -> Option<PathBuf> {
        let mut images: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && is_image(p))
            .collect();
        images.sort();
        for name in &self.cover_names {
            if let Some(p) = images.iter().find(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.to_lowercase().contains(name.as_str()))
            }) {
                return Some(p.clone());
            }
        }
        (images.len() == 1).then(|| images.remove(0))
    }

    /// Cover Art Archive front image for the release, falling back to the
    /// release group.
    fn search_coverart(&self, album: &Album) -> Result<Option<(Vec<u8>, &'static str)>> {
        // CAA serves 250/500/1200px thumbnails; pick the smallest that covers maxwidth.
        let size = match self.maxwidth {
            0 => String::new(),
            w if w <= 250 => "-250".into(),
            w if w <= 500 => "-500".into(),
            w if w <= 1200 => "-1200".into(),
            _ => String::new(),
        };
        let mut urls = vec![];
        if !album.str("mb_albumid").is_empty() {
            urls.push(format!(
                "https://coverartarchive.org/release/{}/front{size}",
                album.str("mb_albumid")
            ));
        }
        if !album.str("mb_releasegroupid").is_empty() {
            urls.push(format!(
                "https://coverartarchive.org/release-group/{}/front{size}",
                album.str("mb_releasegroupid")
            ));
        }
        for url in urls {
            match self.agent.get(&url).call() {
                Ok(resp) => {
                    let ct = resp.content_type().to_string();
                    let mut bytes = vec![];
                    resp.into_reader()
                        .take(MAX_IMAGE_BYTES)
                        .read_to_end(&mut bytes)
                        .context("downloading cover art")?;
                    match ext_for(&ct, &bytes) {
                        Some(ext) => return Ok(Some((bytes, ext))),
                        None => eprintln!("warning: fetchart: {url} did not return an image"),
                    }
                }
                Err(ureq::Error::Status(404, _)) => continue,
                Err(e) => return Err(e).context("Cover Art Archive request"),
            }
        }
        Ok(None)
    }

    fn find(&self, album: &Album, dirs: &[&Path]) -> Result<Option<Found>> {
        for source in &self.sources {
            match source.as_str() {
                "filesystem" => {
                    for d in dirs {
                        if let Some(p) = self.search_filesystem(d) {
                            return Ok(Some(Found::File(p)));
                        }
                    }
                }
                "coverart" => {
                    if let Some((bytes, ext)) = self.search_coverart(album)? {
                        return Ok(Some(Found::Download { bytes, ext }));
                    }
                }
                other => {
                    bail!("unknown fetchart source {other:?} (available: filesystem, coverart)")
                }
            }
        }
        Ok(None)
    }

    /// Fetch art for an album and store it. Returns the art path if set.
    pub fn fetch_for_album(
        &self,
        ctx: &PluginCtx,
        album: &mut Album,
        items: &[Item],
        source_dir: Option<&Path>,
        op: FileOp,
        force: bool,
    ) -> Result<Option<PathBuf>> {
        let current = album.path_field("artpath");
        if !force && current.is_file() {
            return Ok(None);
        }
        let Some(dest_dir) = album_dir(items) else {
            return Ok(None);
        };
        let mut dirs: Vec<&Path> = vec![];
        if let Some(s) = source_dir {
            dirs.push(s);
        }
        if source_dir != Some(dest_dir.as_path()) {
            dirs.push(&dest_dir);
        }
        let Some(found) = self.find(album, &dirs)? else {
            return Ok(None);
        };
        let artpath = match found {
            Found::File(src) => {
                let ext = src
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_else(|| "jpg".into());
                let dest = dest_dir.join(format!("{}.{ext}", self.art_filename));
                if src != dest {
                    match op {
                        FileOp::Move => crate::fileops::move_file(&src, &dest)?,
                        _ => crate::fileops::copy_file(&src, &dest)?,
                    }
                }
                dest
            }
            Found::Download { bytes, ext } => {
                let dest = dest_dir.join(format!("{}.{ext}", self.art_filename));
                std::fs::write(&dest, bytes)
                    .with_context(|| format!("writing {}", dest.display()))?;
                dest
            }
        };
        album.set("artpath", Value::Path(artpath.clone()));
        ctx.lib.store(album)?;
        Ok(Some(artpath))
    }
}

impl Plugin for FetchArt {
    fn name(&self) -> &'static str {
        "fetchart"
    }

    fn album_imported(
        &self,
        ctx: &PluginCtx,
        album: &mut Album,
        items: &mut [Item],
        source_dir: &Path,
        op: FileOp,
    ) -> Result<()> {
        if !self.auto {
            return Ok(());
        }
        // As beets' `_is_source_file_removal_enabled`: when the originals
        // go (move, or copy with `import.delete`), the source art moves too.
        let op = if op == FileOp::Copy && ctx.cfg.bool("import.delete") {
            FileOp::Move
        } else {
            op
        };
        if let Some(p) = self.fetch_for_album(ctx, album, items, Some(source_dir), op, false)? {
            println!("fetchart: {}", p.display());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_named_cover_then_lone_image() {
        let cfg = Config::defaults();
        let f = FetchArt::new(&cfg);
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("back.jpg"), b"x").unwrap();
        assert_eq!(
            f.search_filesystem(d.path()),
            Some(d.path().join("back.jpg"))
        );
        std::fs::write(d.path().join("Folder.png"), b"x").unwrap();
        std::fs::write(d.path().join("scan-front.jpg"), b"x").unwrap();
        // "front" comes before "folder" in cover_names.
        assert_eq!(
            f.search_filesystem(d.path()),
            Some(d.path().join("scan-front.jpg"))
        );
        std::fs::remove_file(d.path().join("scan-front.jpg")).unwrap();
        std::fs::write(d.path().join("other.jpg"), b"x").unwrap();
        assert_eq!(
            f.search_filesystem(d.path()),
            Some(d.path().join("Folder.png"))
        );
    }

    #[test]
    fn sniffs_types() {
        assert_eq!(ext_for("image/jpeg", b""), Some("jpg"));
        assert_eq!(
            ext_for("application/octet-stream", &[0x89, b'P', b'N', b'G']),
            Some("png")
        );
        assert_eq!(ext_for("text/html", b"<html>"), None);
    }
}
