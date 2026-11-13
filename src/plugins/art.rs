//! A port of beets' `beetsplug/_utils/art.py`: embedding, extracting and
//! clearing items' embedded album art, as embedart uses it.

use super::{PluginCtx, PluginLog};
use crate::artresizer;
use crate::commands::format_record;
use crate::media;
use crate::model::{Album, Item};
use crate::template::Template;
use std::path::{Path, PathBuf};

/// mediafile's `image_mime_type` (the `filetype` package): the image type
/// from the data's magic bytes.
pub fn image_mime_type(data: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| data.starts_with(m);
    if starts(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if starts(b"GIF") {
        Some("image/gif")
    } else if data.len() >= 12 && starts(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some("image/tiff")
    } else if starts(b"BM") {
        Some("image/bmp")
    } else if starts(b"8BPS") {
        Some("image/vnd.adobe.photoshop")
    } else if starts(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else if starts(&[0xFF, 0x0A]) || starts(b"\0\0\0\x0cJXL \r\n\x87\n") {
        Some("image/jxl")
    } else if data.len() >= 12 && &data[4..8] == b"ftyp" {
        match &data[8..12] {
            b"avif" | b"avis" => Some("image/avif"),
            b"heic" | b"heix" | b"mif1" | b"msf1" => Some("image/heic"),
            _ => None,
        }
    } else {
        None
    }
}

/// mediafile's `image_extension`: the file extension for the image data
/// ("tiff" rather than "tif", as beets has always used).
pub fn image_extension(data: &[u8]) -> Option<&'static str> {
    Some(match image_mime_type(data)? {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/tiff" => "tiff",
        "image/bmp" => "bmp",
        "image/vnd.adobe.photoshop" => "psd",
        "image/x-icon" => "ico",
        "image/jxl" => "jxl",
        "image/avif" => "avif",
        "image/heic" => "heic",
        _ => return None,
    })
}

/// How beets shows an item or album in log messages (`format(obj)`).
pub fn show_item(ctx: &PluginCtx, item: &Item) -> String {
    format_record(
        &Template::parse(&ctx.cfg.str("format_item")),
        item,
        Some(ctx.lib),
    )
}

pub fn show_album(ctx: &PluginCtx, album: &Album) -> String {
    format_record(
        &Template::parse(&ctx.cfg.str("format_album")),
        album,
        Some(ctx.lib),
    )
}

/// `get_art`: the item's embedded art, if any.
pub fn get_art(log: &PluginLog, item: &Item) -> Option<Vec<u8>> {
    match media::get_art(item.path()) {
        Ok(art) => art,
        Err(e) => {
            log.warning(&format!(
                "Could not extract art from {}: {e:#}",
                item.path().display()
            ));
            None
        }
    }
}

/// `try_write(tags={"images": ...})`: write the item's tags with new
/// pictures; errors are logged (beets' "error writing PATH: REASON").
fn try_write_images(
    log: &PluginLog,
    ctx: &PluginCtx,
    item: &mut Item,
    image: Option<&[u8]>,
) -> bool {
    match media::write_with_images(item, image) {
        Ok(()) => {
            media::refresh_mtime(item);
            if let Err(e) = ctx.lib.store(item) {
                log.error(&format!("{e:#}"));
            }
            true
        }
        Err(e) => {
            crate::logging::emit(format!("error writing {}: {e:#}", item.path().display()));
            false
        }
    }
}

/// Options for embedding.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbedOpts {
    pub maxwidth: u32,
    pub compare_threshold: f64,
    pub ifempty: bool,
    pub quality: u8,
}

/// `embed_item`: embed an image file into the item's media file.
pub fn embed_item(
    log: &PluginLog,
    ctx: &PluginCtx,
    item: &mut Item,
    imagepath: &Path,
    opts: EmbedOpts,
    as_album: bool,
) {
    if opts.compare_threshold != 0.0 {
        match check_art_similarity(log, ctx, item, imagepath, opts.compare_threshold) {
            None => {
                log.warning("Error while checking art similarity; skipping.");
                return;
            }
            Some(false) => {
                log.info("Image not similar; skipping.");
                return;
            }
            Some(true) => {}
        }
    }
    if opts.ifempty && get_art(log, item).is_some() {
        log.info("media file already contained art");
        return;
    }
    let original = imagepath;
    let mut imagepath = imagepath.to_path_buf();
    let mut resized = None;
    if opts.maxwidth != 0 && !as_album {
        imagepath = resize_image(log, &imagepath, opts.maxwidth, opts.quality);
        if imagepath != original {
            resized = Some(imagepath.clone());
        }
    }
    log.debug(&format!("embedding {}", imagepath.display()));
    let data = std::fs::read(&imagepath);
    if let Some(r) = resized {
        let _ = std::fs::remove_file(r);
    }
    let data = match data {
        Ok(d) => d,
        Err(e) => {
            log.warning(&format!("could not read image file: {e}"));
            return;
        }
    };
    // Only JPEG and PNG are safe in every tag format.
    let mime = image_mime_type(&data);
    if !matches!(mime, Some("image/jpeg" | "image/png")) {
        log.info(&format!(
            "not embedding image of unsupported type: {}",
            mime.unwrap_or("None")
        ));
        return;
    }
    try_write_images(log, ctx, item, Some(&data));
}

/// `embed_album`: embed the album's art file into all its items.
pub fn embed_album(log: &PluginLog, ctx: &PluginCtx, album: &Album, opts: EmbedOpts) {
    let art = album.path_field("artpath");
    if art.as_os_str().is_empty() {
        log.info(&format!(
            "No album art present for {}",
            show_album(ctx, album)
        ));
        return;
    }
    if !art.is_file() {
        log.info(&format!(
            "Album art not found at {} for {}",
            art.display(),
            show_album(ctx, album)
        ));
        return;
    }
    let mut imagepath = art.to_path_buf();
    let resized = opts.maxwidth != 0;
    if resized {
        imagepath = resize_image(log, &imagepath, opts.maxwidth, opts.quality);
    }
    log.info(&format!(
        "Embedding album art into {}",
        show_album(ctx, album)
    ));
    let items = match album.id.map(|id| ctx.lib.album_items(id)) {
        Some(Ok(items)) => items,
        Some(Err(e)) => {
            log.error(&format!("{e:#}"));
            vec![]
        }
        None => vec![],
    };
    for mut item in items {
        embed_item(log, ctx, &mut item, &imagepath, opts, true);
    }
    if resized && imagepath != art {
        let _ = std::fs::remove_file(&imagepath);
    }
}

/// `resize_image`.
pub fn resize_image(log: &PluginLog, imagepath: &Path, maxwidth: u32, quality: u8) -> PathBuf {
    log.debug(&format!(
        "Resizing album art to {maxwidth} pixels wide and encoding at quality level {quality}"
    ));
    artresizer::resize(maxwidth, imagepath, quality, 0)
}

/// `check_art_similarity`: whether the image is similar to the item's
/// embedded art (always, if it has none); `None` if comparing failed.
pub fn check_art_similarity(
    log: &PluginLog,
    ctx: &PluginCtx,
    item: &Item,
    imagepath: &Path,
    compare_threshold: f64,
) -> Option<bool> {
    let tmp = artresizer::py_temp_name();
    let art = match extract(log, ctx, &tmp, item) {
        Ok(Some(art)) => art,
        Ok(None) => return Some(true),
        Err(e) => {
            log.debug(&format!("{e:#}"));
            return None;
        }
    };
    let r = artresizer::compare(&art, imagepath, compare_threshold);
    let _ = std::fs::remove_file(&art);
    r
}

/// `extract`: write the item's embedded art to `outpath` plus the image's
/// extension; returns the path written.
/// A failure to write it is an error (beets lets the exception end the
/// command).
pub fn extract(
    log: &PluginLog,
    ctx: &PluginCtx,
    outpath: &Path,
    item: &Item,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(art) = get_art(log, item) else {
        log.info(&format!(
            "No album art present in {}, skipping.",
            show_item(ctx, item)
        ));
        return Ok(None);
    };
    let Some(ext) = image_extension(&art) else {
        log.warning(&format!("Unknown image type in {}.", item.path().display()));
        return Ok(None);
    };
    let mut out = outpath.as_os_str().to_owned();
    out.push(format!(".{ext}"));
    let out = PathBuf::from(out);
    log.info(&format!(
        "Extracting album art from: {} to: {}",
        show_item(ctx, item),
        out.display()
    ));
    std::fs::write(&out, art).map_err(|e| anyhow::anyhow!(py_os_error(&e, &out)))?;
    Ok(Some(out))
}

/// Python's `OSError` message: "[Errno 2] No such file or directory: 'PATH'".
pub fn py_os_error(e: &std::io::Error, path: &Path) -> String {
    let msg = e.to_string();
    let msg = match msg.rfind(" (os error ") {
        Some(i) => msg[..i].to_string(),
        None => msg,
    };
    match e.raw_os_error() {
        Some(n) => format!("[Errno {n}] {msg}: '{}'", path.display()),
        None => format!("{msg}: '{}'", path.display()),
    }
}

/// `extract_first`: extract from the first item that has art.
pub fn extract_first(
    log: &PluginLog,
    ctx: &PluginCtx,
    outpath: &Path,
    items: &[Item],
) -> anyhow::Result<Option<PathBuf>> {
    for i in items {
        if let Some(p) = extract(log, ctx, outpath, i)? {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// `clear_item`: remove the item's embedded pictures, if it has any.
pub fn clear_item(log: &PluginLog, ctx: &PluginCtx, item: &mut Item) {
    let has = match media::has_art(item.path()) {
        Ok(h) => h,
        Err(e) => {
            crate::logging::emit(format!("error reading {}: {e:#}", item.path().display()));
            return;
        }
    };
    if has {
        log.debug(&format!("Clearing art for {}", show_item(ctx, item)));
        try_write_images(log, ctx, item, None);
    }
}

/// `clear`: remove embedded art from every item.
pub fn clear(log: &PluginLog, ctx: &PluginCtx, items: &mut [Item]) {
    log.info(&format!("Clearing album art from {} items", items.len()));
    for item in items {
        clear_item(log, ctx, item);
    }
}

/// `os.path.samefile`: both exist and are the same file.
pub fn samefile(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    }
}

/// beets' `Album.set_art`: copy (or move) the image into place as the
/// album's art file, replacing any existing art, and send `art_set`. The
/// caller stores the album.
pub fn set_art(ctx: &PluginCtx, album: &mut Album, path: &Path, copy: bool) -> anyhow::Result<()> {
    use anyhow::Context;
    let oldart = album.path_field("artpath").to_path_buf();
    let items = ctx
        .lib
        .album_items(album.id.context("album not in the library")?)?;
    let dir = super::fetchart::album_dir(&items).context("empty album")?;
    let artdest = crate::paths::art_destination(ctx.cfg, album, path, &dir, Some(ctx.lib))?;
    if !oldart.as_os_str().is_empty() && samefile(path, &oldart) {
        // Art already set.
        return Ok(());
    }
    if samefile(path, &artdest) {
        // Art already in place.
        album.set("artpath", crate::model::Value::Path(path.to_path_buf()));
        return Ok(());
    }
    // beets' `util.remove(path, soft=True)`: missing files are fine.
    let remove = |p: &Path| -> anyhow::Result<()> {
        match std::fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("removing {}", p.display()))
            }
            _ => Ok(()),
        }
    };
    if !oldart.as_os_str().is_empty() {
        remove(&oldart)?;
    }
    remove(&artdest)?;
    if copy {
        crate::fileops::copy_file(path, &artdest)?;
    } else {
        crate::fileops::move_file(path, &artdest)?;
    }
    album.set("artpath", crate::model::Value::Path(artdest));
    // beets' `art_set` event (embedart embeds it, permissions fixes it).
    if crate::events::listening("art_set") {
        crate::events::send("art_set", &[("album", crate::events::Arg::Album(album))]);
    }
    super::send_art_set(ctx, album);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_like_filetype() {
        assert_eq!(image_extension(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_mime_type(b"\x89PNG\r\n\x1a\nxxxx"), Some("image/png"));
        assert_eq!(image_extension(b"II*\0rest"), Some("tiff"));
        assert_eq!(image_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_extension(b"<html>"), None);
        assert_eq!(image_mime_type(b""), None);
    }
}
