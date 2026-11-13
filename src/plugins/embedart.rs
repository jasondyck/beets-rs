//! embedart: a port of beets 2.14's `beetsplug/embedart.py`. Embeds album
//! art into files' metadata when art is set (the `art_set` event), and adds
//! the `embedart`, `extractart` and `clearart` commands.

use super::art::{self, EmbedOpts};
use super::{INFO, ImportTask, Plugin, PluginCtx, PluginLog, WARNING};
use crate::config::Config;
use crate::library::Library;
use crate::model::Kind;
use crate::model::{Album, Item, Value};
use crate::query::Query;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub struct EmbedArt {
    pub log: PluginLog,
    auto: bool,
    opts: EmbedOpts,
    remove_art_file: bool,
    clearart_on_import: bool,
}

/// confuse's `.get(int)`, leniently.
pub fn cfg_int(cfg: &Config, key: &str) -> i64 {
    cfg.f64(key) as i64
}

impl EmbedArt {
    pub fn new(cfg: &Config) -> Self {
        EmbedArt {
            log: PluginLog::new("embedart"),
            auto: cfg.bool("embedart.auto"),
            opts: EmbedOpts {
                maxwidth: cfg_int(cfg, "embedart.maxwidth").max(0) as u32,
                compare_threshold: cfg_int(cfg, "embedart.compare_threshold") as f64,
                ifempty: cfg.bool("embedart.ifempty"),
                quality: cfg_int(cfg, "embedart.quality").clamp(0, 100) as u8,
            },
            remove_art_file: cfg.bool("embedart.remove_art_file"),
            clearart_on_import: cfg.bool("embedart.clearart_on_import"),
        }
    }

    /// `process_album`, the `art_set` listener: embed the album's new art
    /// when `auto` is on and tags are being written.
    pub fn process_album(&self, ctx: &PluginCtx, album: &mut Album) -> Result<()> {
        if !(self.auto && ctx.write) {
            return Ok(());
        }
        self.log.at_level(WARNING, || {
            // As beets: the listener doesn't pass `quality`.
            let opts = EmbedOpts {
                quality: 0,
                ..self.opts
            };
            art::embed_album(&self.log, ctx, album, opts);
            self.remove_artfile(ctx, album)
        })
    }

    /// `remove_artfile`: with `remove_art_file`, delete the album's art file.
    pub fn remove_artfile(&self, ctx: &PluginCtx, album: &mut Album) -> Result<()> {
        let art = album.path_field("artpath").to_path_buf();
        if self.remove_art_file && !art.as_os_str().is_empty() && art.is_file() {
            self.log.debug(&format!(
                "Removing album art file for {}",
                art::show_album(ctx, album)
            ));
            std::fs::remove_file(&art)?;
            album.set("artpath", Value::Path(PathBuf::new()));
            ctx.lib.store(album)?;
        }
        Ok(())
    }

    fn clear_imported(&self, ctx: &PluginCtx, items: &mut [Item]) {
        if !self.clearart_on_import {
            return;
        }
        self.log.at_level(WARNING, || {
            for item in items {
                self.log
                    .debug(&format!("clearart-on-import {}", item.path().display()));
                art::clear_item(&self.log, ctx, item);
            }
        });
    }
}

impl Plugin for EmbedArt {
    fn name(&self) -> &'static str {
        "embedart"
    }

    /// beets' `import_task_files` listener (with `clearart_on_import`).
    fn import_task_files(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        self.clear_imported(ctx, task.items);
        Ok(())
    }
}

/// `_confirm`: list the objects and ask whether to modify their artwork.
fn confirm(shown: &[String], album: bool) -> bool {
    let noun = if album { "album" } else { "file" };
    let s = if shown.len() > 1 { "s" } else { "" };
    for line in shown {
        println!("{line}");
    }
    crate::ui::input_yn(
        &format!("Modify artwork for {} {noun}{s} (Y/n)?", shown.len()),
        false,
    )
}

/// beets' `normpath`: `~` expanded, absolute and normalised.
pub fn normpath(p: &Path) -> PathBuf {
    let p = crate::config::expand_tilde(&p.to_string_lossy());
    let abs = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// `mimetypes.guess_extension` for the types an image URL may report.
fn guess_extension(content_type: &str) -> Option<&'static str> {
    Some(match content_type {
        "image/jpeg" | "image/pjpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/bmp" => ".bmp",
        "image/tiff" => ".tiff",
        "image/x-icon" | "image/vnd.microsoft.icon" => ".ico",
        "image/svg+xml" => ".svg",
        "image/avif" => ".avif",
        "image/heic" => ".heic",
        "text/html" => ".html",
        "text/plain" => ".txt",
        "application/json" => ".json",
        "application/octet-stream" => ".bin",
        _ => return None,
    })
}

pub struct EmbedCmd {
    pub file: Option<PathBuf>,
    pub url: Option<String>,
    pub yes: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

fn items(lib: &Library, q: &[String], limit: Option<usize>) -> Result<Vec<Item>> {
    let mut v = lib.items(&Query::parse(Kind::Item, q)?)?;
    v.truncate(limit.unwrap_or(usize::MAX));
    Ok(v)
}

fn albums(lib: &Library, q: &[String], limit: Option<usize>) -> Result<Vec<Album>> {
    let mut v = lib.albums(&Query::parse(Kind::Album, q)?)?;
    v.truncate(limit.unwrap_or(usize::MAX));
    Ok(v)
}

/// The `embedart` command.
pub fn embed_command(ctx: &PluginCtx, opts: &EmbedCmd) -> Result<()> {
    let ea = EmbedArt::new(ctx.cfg);
    ea.log.set_base_level(INFO);
    let log = &ea.log;
    if let Some(file) = &opts.file {
        let imagepath = normpath(file);
        if !imagepath.is_file() {
            bail!("image file {} not found", imagepath.display());
        }
        let mut items = items(ctx.lib, &opts.query, opts.limit)?;
        let shown: Vec<String> = items.iter().map(|i| art::show_item(ctx, i)).collect();
        if !opts.yes && !confirm(&shown, false) {
            return Ok(());
        }
        for item in &mut items {
            art::embed_item(log, ctx, item, &imagepath, ea.opts, false);
        }
    } else if let Some(url) = &opts.url {
        let session = super::requests::Session::plain(super::USER_AGENT);
        let resp = match session.get_any(url, Default::default()) {
            Ok(r) if r.status >= 400 => {
                let kind = if r.status < 500 { "Client" } else { "Server" };
                log.error(&format!(
                    "{} {kind} Error: {} for url: {url}",
                    r.status,
                    super::requests::reason(r.status)
                ));
                return Ok(());
            }
            Ok(r) => r,
            Err(e) => {
                log.error(&e.to_string());
                return Ok(());
            }
        };
        let Some(ext) = resp.content_type().and_then(guess_extension) else {
            log.error("Invalid image file");
            return Ok(());
        };
        let tempimg = std::env::temp_dir().join(format!("image{ext}"));
        if let Err(e) = std::fs::write(&tempimg, &resp.body) {
            log.error(&format!("Unable to save image: {e}"));
            return Ok(());
        }
        let mut items = items(ctx.lib, &opts.query, opts.limit)?;
        let shown: Vec<String> = items.iter().map(|i| art::show_item(ctx, i)).collect();
        if !opts.yes && !confirm(&shown, false) {
            let _ = std::fs::remove_file(&tempimg);
            return Ok(());
        }
        for item in &mut items {
            art::embed_item(log, ctx, item, &tempimg, ea.opts, false);
        }
        let _ = std::fs::remove_file(&tempimg);
    } else {
        let mut albums = albums(ctx.lib, &opts.query, opts.limit)?;
        let shown: Vec<String> = albums.iter().map(|a| art::show_album(ctx, a)).collect();
        if !opts.yes && !confirm(&shown, true) {
            return Ok(());
        }
        for album in &mut albums {
            art::embed_album(log, ctx, album, ea.opts);
            ea.remove_artfile(ctx, album)?;
        }
    }
    Ok(())
}

pub struct ExtractCmd {
    pub outpath: Option<PathBuf>,
    pub filename: Option<String>,
    pub associate: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// The `extractart` command.
pub fn extract_command(ctx: &PluginCtx, opts: &ExtractCmd) -> Result<()> {
    let log = PluginLog::new("embedart");
    if let Some(out) = &opts.outpath {
        let items = items(ctx.lib, &opts.query, opts.limit)?;
        art::extract_first(&log, ctx, &normpath(out), &items)?;
        return Ok(());
    }
    let filename = PathBuf::from(
        opts.filename
            .clone()
            .unwrap_or_else(|| ctx.cfg.str("art_filename")),
    );
    if filename.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        log.error("Only specify a name rather than a path for -n");
        return Ok(());
    }
    for mut album in albums(ctx.lib, &opts.query, opts.limit)? {
        if !opts.associate {
            continue;
        }
        let items = ctx.lib.album_items(album.id.unwrap())?;
        let Some(dir) = super::fetchart::album_dir(&items) else {
            continue;
        };
        if let Some(artpath) = art::extract_first(&log, ctx, &dir.join(&filename), &items)? {
            art::set_art(ctx, &mut album, &artpath, true)?;
            ctx.lib.store(&album)?;
        }
    }
    Ok(())
}

/// The `clearart` command.
pub fn clear_command(
    ctx: &PluginCtx,
    yes: bool,
    limit: Option<usize>,
    query: &[String],
) -> Result<()> {
    let log = PluginLog::new("embedart");
    let mut items = items(ctx.lib, query, limit)?;
    let shown: Vec<String> = items.iter().map(|i| art::show_item(ctx, i)).collect();
    if !yes && !confirm(&shown, false) {
        return Ok(());
    }
    art::clear(&log, ctx, &mut items);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artresizer::tests::{picture, write_image};
    use crate::media;
    use crate::plugins::requests::testserver::{Reply, serve};
    use crate::ui::script;
    use serde_yaml::Value as Y;

    struct Env {
        dir: tempfile::TempDir,
        cfg: Config,
        lib: Library,
    }

    fn set_deep(cfg: &mut Config, prefix: &str, y: Y) {
        match y {
            Y::Mapping(m) => {
                for (k, v) in m {
                    let k = k.as_str().unwrap();
                    let key = if prefix.is_empty() {
                        k.to_string()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    set_deep(cfg, &key, v);
                }
            }
            v => cfg.set(prefix, v),
        }
    }

    impl Env {
        /// A library with one album of `n` generated MP3s (None without ffmpeg).
        fn new(yaml: &str, n: usize) -> Option<(Env, Album)> {
            let dir = tempfile::tempdir().unwrap();
            let music = dir.path().join("music");
            let adir = music.join("A/B");
            std::fs::create_dir_all(&adir).unwrap();
            let mut cfg = Config::defaults();
            cfg.set("directory", Y::String(music.display().to_string()));
            cfg.set("plugins", serde_yaml::from_str("[embedart]").unwrap());
            set_deep(&mut cfg, "", serde_yaml::from_str(yaml).unwrap());
            let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
            let mut items = vec![];
            for i in 0..n {
                let p = adir.join(format!("{i}.mp3"));
                let ok = std::process::Command::new("ffmpeg")
                    .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.2"])
                    .arg(&p)
                    .status()
                    .is_ok_and(|s| s.success());
                if !ok {
                    return None;
                }
                let mut it = media::read(&p).unwrap();
                it.set_str("title", format!("t{i}"));
                it.set_str("album", "B");
                it.set_str("albumartist", "A");
                items.push(it);
            }
            let album = lib.add_album(&mut items).unwrap();
            Some((Env { dir, cfg, lib }, album))
        }

        fn ctx(&self) -> PluginCtx<'_> {
            PluginCtx {
                cfg: &self.cfg,
                lib: &self.lib,
                write: true,
            }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn image(&self, name: &str, variant: u32, size: u32) -> PathBuf {
            let p = self.path(name);
            write_image(&p, &picture(size, size, variant), 90);
            p
        }

        fn items(&self) -> Vec<Item> {
            self.lib
                .items(&Query::parse(Kind::Item, &[] as &[String]).unwrap())
                .unwrap()
        }

        fn art(&self, i: usize) -> Option<Vec<u8>> {
            media::get_art(self.items()[i].path()).unwrap()
        }

        fn embed(&self, file: Option<&Path>, yes: bool) {
            embed_command(
                &self.ctx(),
                &EmbedCmd {
                    file: file.map(Path::to_path_buf),
                    url: None,
                    yes,
                    limit: None,
                    query: vec![],
                },
            )
            .unwrap();
        }
    }

    /// `test_embedart.py` `test_embed_art_from_file_with_yes_input` / `_no_input`.
    #[test]
    fn embed_from_file_confirms() {
        let Some((env, _)) = Env::new("{}", 1) else {
            return;
        };
        let img = env.image("x.jpg", 0, 20);
        script::set(vec![("n", None)]);
        env.embed(Some(&img), false);
        assert_eq!(script::prompts(), ["Modify artwork for 1 file (Y/n)? "]);
        assert_eq!(env.art(0), None);
        script::set(vec![("y", None)]);
        env.embed(Some(&img), false);
        assert_eq!(env.art(0).unwrap(), std::fs::read(&img).unwrap());
    }

    #[test]
    fn embed_from_file_and_missing_file() {
        let Some((env, _)) = Env::new("{}", 2) else {
            return;
        };
        let img = env.image("x.png", 0, 20);
        env.embed(Some(&img), true);
        assert_eq!(env.art(1).unwrap(), std::fs::read(&img).unwrap());
        let err = embed_command(
            &env.ctx(),
            &EmbedCmd {
                file: Some(env.path("nope.jpg")),
                url: None,
                yes: true,
                limit: None,
                query: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("image file {} not found", env.path("nope.jpg").display())
        );
    }

    /// `test_embed_art_from_album`, `test_art_file_missing`,
    /// `test_embed_art_remove_art_file`.
    #[test]
    fn embed_album_art() {
        let Some((env, mut album)) = Env::new("{}", 2) else {
            return;
        };
        // No art: nothing happens.
        env.embed(None, true);
        assert_eq!(env.art(0), None);
        let img = env.image("music/A/B/cover.jpg", 0, 20);
        album.set(
            "artpath",
            Value::Path(PathBuf::from("/nonexistent/cover.jpg")),
        );
        env.lib.store(&album).unwrap();
        env.embed(None, true);
        assert_eq!(env.art(0), None, "art file missing");
        album.set("artpath", Value::Path(img.clone()));
        env.lib.store(&album).unwrap();
        script::set(vec![("", None)]);
        env.embed(None, false);
        assert_eq!(script::prompts(), ["Modify artwork for 1 album (Y/n)? "]);
        assert_eq!(env.art(0).unwrap(), std::fs::read(&img).unwrap());
        assert_eq!(env.art(1).unwrap(), std::fs::read(&img).unwrap());

        let mut cfg = env.cfg.clone();
        cfg.set("embedart.remove_art_file", Y::Bool(true));
        let env2 = Env {
            dir: tempfile::tempdir().unwrap(),
            cfg,
            lib: Library::open_with(&env.path("l.db"), Some(&env.path("music")), false).unwrap(),
        };
        env2.embed(None, true);
        assert!(!img.exists(), "art file removed");
        let stored = env2.lib.get_album(album.id.unwrap()).unwrap().unwrap();
        assert_eq!(stored.path_field("artpath"), Path::new(""));
    }

    /// `test_embed_non_image_file`.
    #[test]
    fn non_image_not_embedded() {
        let Some((env, _)) = Env::new("{}", 1) else {
            return;
        };
        let p = env.path("not-an-image.jpg");
        std::fs::write(&p, b"I am not an image.").unwrap();
        let (_, logs) = crate::logging::capture(|| env.embed(Some(&p), true));
        assert_eq!(env.art(0), None);
        assert_eq!(
            logs,
            ["embedart: not embedding image of unsupported type: None"]
        );
    }

    /// `test_reject_different_art` / `test_accept_similar_art`.
    #[test]
    fn compare_threshold() {
        let Some((env, _)) = Env::new("embedart: {compare_threshold: 5}", 1) else {
            return;
        };
        let orig = env.image("orig.jpg", 0, 225);
        let similar = env.path("similar.jpg");
        write_image(&similar, &picture(500, 490, 0), 60);
        let different = env.image("different.jpg", 1, 348);
        env.embed(Some(&orig), true);
        let (_, logs) = crate::logging::capture(|| env.embed(Some(&different), true));
        assert_eq!(
            env.art(0).unwrap(),
            std::fs::read(&orig).unwrap(),
            "rejected"
        );
        assert!(
            logs.iter()
                .any(|l| l == "embedart: Image not similar; skipping."),
            "{logs:?}"
        );
        env.embed(Some(&similar), true);
        assert_eq!(
            env.art(0).unwrap(),
            std::fs::read(&similar).unwrap(),
            "accepted"
        );
    }

    #[test]
    fn ifempty_and_maxwidth() {
        let Some((env, _)) = Env::new("embedart: {ifempty: yes, maxwidth: 10}", 1) else {
            return;
        };
        let a = env.image("a.jpg", 0, 40);
        let b = env.image("b.jpg", 1, 40);
        env.embed(Some(&a), true);
        let art = env.art(0).unwrap();
        let img = image::load_from_memory(&art).unwrap();
        assert_eq!((img.width(), img.height()), (10, 10), "resized to maxwidth");
        let (_, logs) = crate::logging::capture(|| env.embed(Some(&b), true));
        assert_eq!(env.art(0).unwrap(), art, "kept");
        assert_eq!(logs, ["embedart: media file already contained art"]);
    }

    /// `test_extracted_extension`, `test_filename_opt`, `-a`.
    #[test]
    fn extractart() {
        let Some((env, album)) = Env::new("{}", 1) else {
            return;
        };
        let img = env.image("x.png", 0, 16);
        env.embed(Some(&img), true);
        let out = env.path("out/extracted");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        extract_command(
            &env.ctx(),
            &ExtractCmd {
                outpath: Some(out.clone()),
                filename: None,
                associate: false,
                limit: None,
                query: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read(env.path("out/extracted.png")).unwrap(),
            std::fs::read(&img).unwrap()
        );
        // -n must be a name.
        let (_, logs) = crate::logging::capture(|| {
            extract_command(
                &env.ctx(),
                &ExtractCmd {
                    outpath: None,
                    filename: Some("sub/x".into()),
                    associate: true,
                    limit: None,
                    query: vec![],
                },
            )
            .unwrap();
        });
        assert_eq!(
            logs,
            ["embedart: Only specify a name rather than a path for -n"]
        );
        // Without -o or -a nothing is written.
        extract_command(
            &env.ctx(),
            &ExtractCmd {
                outpath: None,
                filename: Some("front".into()),
                associate: false,
                limit: None,
                query: vec![],
            },
        )
        .unwrap();
        assert!(!env.path("music/A/B/front.png").exists());
        // -a: written next to the album's files and set as its art.
        extract_command(
            &env.ctx(),
            &ExtractCmd {
                outpath: None,
                filename: None,
                associate: true,
                limit: None,
                query: vec![],
            },
        )
        .unwrap();
        let stored = env.lib.get_album(album.id.unwrap()).unwrap().unwrap();
        assert_eq!(
            stored.path_field("artpath"),
            env.path("music/A/B/cover.png")
        );
    }

    /// `test_clear_art_with_yes_input` / `_no_input`.
    #[test]
    fn clearart() {
        let Some((env, _)) = Env::new("{}", 1) else {
            return;
        };
        let img = env.image("x.jpg", 0, 16);
        env.embed(Some(&img), true);
        script::set(vec![("n", None)]);
        clear_command(&env.ctx(), false, None, &[]).unwrap();
        assert!(env.art(0).is_some());
        clear_command(&env.ctx(), true, None, &[]).unwrap();
        assert_eq!(env.art(0), None);
    }

    /// `test_embed_art_from_url_*`.
    #[test]
    fn embed_from_url() {
        let Some((env, _)) = Env::new("{}", 1) else {
            return;
        };
        let png = crate::artresizer::encode(&picture(8, 8, 0), image::ImageFormat::Png, 0).unwrap();
        let png2 = png.clone();
        let s = serve(move |r| match r.target.as_str() {
            "/image.png" => Reply {
                status: 200,
                content_type: "image/png",
                body: png2.clone(),
            },
            "/page" => Reply::text("<html>not an image</html>"),
            _ => Reply::status(404),
        });
        let run = |path: &str| {
            crate::logging::capture(|| {
                embed_command(
                    &env.ctx(),
                    &EmbedCmd {
                        file: None,
                        url: Some(format!("{}{path}", s.base)),
                        yes: true,
                        limit: None,
                        query: vec![],
                    },
                )
                .unwrap();
            })
            .1
        };
        assert!(run("/image.png").is_empty());
        assert_eq!(env.art(0).unwrap(), png);
        // Not an image: the type check refuses it.
        let logs = run("/page");
        assert_eq!(logs, ["embedart: Invalid image file"]);
        let logs = run("/missing");
        assert_eq!(
            logs,
            [format!(
                "embedart: 404 Client Error: Not Found for url: {}/missing",
                s.base
            )]
        );
    }

    /// `test_clearart_on_import_*`.
    #[test]
    fn clearart_on_import() {
        for on in [false, true] {
            let Some((env, mut album)) =
                Env::new(&format!("embedart: {{clearart_on_import: {on}}}"), 1)
            else {
                return;
            };
            let img = env.image("x.jpg", 0, 16);
            env.embed(Some(&img), true);
            let ea = EmbedArt::new(&env.cfg);
            let mut items = env.items();
            let mut task = ImportTask {
                album: Some(&mut album),
                items: &mut items,
                paths: &[],
                asis: false,
                op: crate::fileops::FileOp::Copy,
            };
            ea.import_task_files(&env.ctx(), &mut task).unwrap();
            assert_eq!(env.art(0).is_none(), on);
        }
    }

    /// The `art_set` listener: auto-embedding when art is set.
    #[test]
    fn art_set_embeds_automatically() {
        let Some((env, mut album)) = Env::new("{}", 1) else {
            return;
        };
        let img = env.image("new.jpg", 0, 16);
        art::set_art(&env.ctx(), &mut album, &img, true).unwrap();
        assert_eq!(album.path_field("artpath"), env.path("music/A/B/cover.jpg"));
        assert_eq!(env.art(0).unwrap(), std::fs::read(&img).unwrap());
        // Not when tags aren't written, nor with auto off.
        let Some((env, mut album)) = Env::new("embedart: {auto: no}", 1) else {
            return;
        };
        let img = env.image("new.jpg", 0, 16);
        art::set_art(&env.ctx(), &mut album, &img, true).unwrap();
        assert_eq!(env.art(0), None);
    }

    #[test]
    fn normpath_is_absolute_and_normal() {
        assert_eq!(normpath(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert!(normpath(Path::new("x")).is_absolute());
    }

    #[test]
    fn guesses_extensions() {
        assert_eq!(guess_extension("image/jpeg"), Some(".jpg"));
        assert_eq!(guess_extension("image/png"), Some(".png"));
        assert_eq!(guess_extension("image/jpeg; charset=x"), None);
        assert_eq!(guess_extension("foo/bar"), None);
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "embedart",
        order: 4,
        defaults: r#"embedart:
    maxwidth: 0
    auto: yes
    compare_threshold: 0
    ifempty: no
    remove_art_file: no
    quality: 0
    clearart_on_import: no
"#,
        config_keys: &["embedart"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "embedart",
                build: build_embedart,
                run: run_embedart,
            },
            crate::plugin_registry::PluginCommand {
                plugin: "embedart",
                build: build_extractart,
                run: run_extractart,
            },
            crate::plugin_registry::PluginCommand {
                plugin: "embedart",
                build: build_clearart,
                run: run_clearart,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(EmbedArt::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct EmbedartArgs {
    /// The image file to embed
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    file: Option<PathBuf>,
    /// Skip confirmation
    #[arg(short = 'y', long)]
    yes: bool,
    /// The URL of the image file to embed
    #[arg(short = 'u', long, value_name = "URL")]
    url: Option<String>,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_embedart() -> clap::Command {
    use clap::Args as _;
    EmbedartArgs::augment_args(
        clap::Command::new("embedart")
            .about("embed image files into file metadata"),
    )
}

pub fn run_embedart(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = EmbedartArgs::from_arg_matches(m)?;
    let ctx = crate::plugins::PluginCtx {
        cfg,
        lib,
        write: cfg.bool("import.write"),
    };
    crate::plugins::embedart::embed_command(
        &ctx,
        &crate::plugins::embedart::EmbedCmd {
            file: a.file,
            url: a.url,
            yes: a.yes,
            limit: a.limit,
            query: a.query,
        },
    )
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ExtractartArgs {
    /// Image output file
    #[arg(short = 'o', value_name = "OUTPATH")]
    outpath: Option<PathBuf>,
    /// Image filename to create for all matched albums
    #[arg(short = 'n', value_name = "FILENAME")]
    filename: Option<String>,
    /// Associate the extracted images with the album
    #[arg(short = 'a')]
    associate: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_extractart() -> clap::Command {
    use clap::Args as _;
    ExtractartArgs::augment_args(
        clap::Command::new("extractart")
            .about("extract an image from file metadata"),
    )
}

pub fn run_extractart(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ExtractartArgs::from_arg_matches(m)?;
    let ctx = crate::plugins::PluginCtx {
        cfg,
        lib,
        write: cfg.bool("import.write"),
    };
    crate::plugins::embedart::extract_command(
        &ctx,
        &crate::plugins::embedart::ExtractCmd {
            outpath: a.outpath,
            filename: a.filename,
            associate: a.associate,
            limit: a.limit,
            query: a.query,
        },
    )
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ClearartArgs {
    /// Skip confirmation
    #[arg(short = 'y', long)]
    yes: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_clearart() -> clap::Command {
    use clap::Args as _;
    ClearartArgs::augment_args(
        clap::Command::new("clearart").about("remove images from file metadata"),
    )
}

pub fn run_clearart(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ClearartArgs::from_arg_matches(m)?;
    let ctx = crate::plugins::PluginCtx {
        cfg,
        lib,
        write: cfg.bool("import.write"),
    };
    crate::plugins::embedart::clear_command(&ctx, a.yes, a.limit, &a.query)
}
