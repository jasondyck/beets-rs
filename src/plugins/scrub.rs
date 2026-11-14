//! Port of beets 2.14's `scrub` plugin (`beetsplug/scrub.py`): removes
//! every tag from files, then writes the library's metadata (and the
//! embedded images) back, on import (`auto`, beets' `import_task_files`
//! event, when tags are written) or with `beet scrub [-W] [-l N] [QUERY]`.
//!
//! beets strips a file with each Mutagen class that can open it (ID3,
//! MP3, APEv2, FLAC, MP4, the Ogg formats, WavPack, Monkey's Audio, ...).
//! The same by format: MP3s lose their ID3v2, ID3v1 and APEv2 tags, FLAC
//! files their Vorbis comments (not their PICTURE blocks, as Mutagen's
//! `FLAC.delete`), MP4s their `ilst`, Ogg Vorbis/Opus/Speex their
//! comments, WavPack/Monkey's Audio/Musepack/AAC their APEv2 tag. AIFF
//! and WAV aren't in beets' list, so their tags stay.

use crate::config::Config;
use crate::library::Library;
use crate::mediafile::MediaFile;
use crate::model::{Item, Kind};
use crate::plugins::{INFO, ImportTask, Plugin, PluginCtx, PluginLog, WARNING};
use crate::query::Query;
use anyhow::{Context, Result};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::ogg::OggPictureStorage;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::{TagExt, TagType};
use std::path::Path;

/// `_scrub`: remove every tag (of the kinds beets removes) from the file.
pub fn scrub(path: &Path) -> Result<()> {
    // ASF (Windows Media): every attribute is deleted; Mutagen's
    // `ASFTag.delete` isn't implemented, so beets deletes each tag and
    // saves, which is what `remove_all` and `save` do here.
    if crate::asf::is_asf(path) {
        let mut a = crate::asf::Asf::open(path)?;
        a.remove_all();
        return a.save();
    }
    let ft = lofty::probe::Probe::open(path)?
        .guess_file_type()?
        .file_type()
        .context("unknown file type")?;
    // Only the tags the file has: lofty "removes" a missing ID3v1 tag by
    // appending an empty one.
    let tagged = lofty::read_from_path(path)?;
    let remove = |tags: &[TagType]| -> Result<()> {
        for t in tags.iter().filter(|t| tagged.contains_tag_type(**t)) {
            t.remove_from_path(path, WriteOptions::default())?;
        }
        Ok(())
    };
    match ft {
        FileType::Mpeg => remove(&[TagType::Ape, TagType::Id3v2, TagType::Id3v1])?,
        FileType::Flac => {
            // Mutagen's `FLAC.delete` drops the VORBIS_COMMENT block only.
            let mut f = std::fs::File::open(path)?;
            let flac = lofty::flac::FlacFile::read_from(&mut f, ParseOptions::new())?;
            drop(f);
            let mut vc = VorbisComments::new();
            for (pic, info) in flac.pictures().iter().cloned() {
                vc.insert_picture(pic, Some(info))?;
            }
            vc.save_to_path(path, WriteOptions::default())?;
        }
        FileType::Mp4 => remove(&[TagType::Mp4Ilst])?,
        FileType::Vorbis | FileType::Opus | FileType::Speex => remove(&[TagType::VorbisComments])?,
        FileType::WavPack | FileType::Ape | FileType::Mpc | FileType::Aac => {
            remove(&[TagType::Ape])?;
        }
        _ => {}
    }
    Ok(())
}

/// `_scrub_item`: scrub the item's file and, with `restore`, write the
/// library's tags (through the `write` event) and the file's images back.
pub fn scrub_item(item: &mut Item, restore: bool, log: &PluginLog) {
    let path = item.path().to_path_buf();
    let images = if restore {
        match MediaFile::open(&path) {
            Ok(mf) => mf.pictures(),
            Err(e) => {
                log.error(&format!("could not open file to scrub: {e:#}"));
                return;
            }
        }
    } else {
        vec![]
    };
    if let Err(e) = scrub(&path) {
        log.error(&format!("could not scrub {}: {e:#}", path.display()));
    }
    if restore {
        log.debug("writing new tags after scrub");
        // `item.try_write()`.
        if let Err(e) = crate::media::write_item(item) {
            crate::logging::emit(format!("{e:#}"));
        }
        if !images.is_empty() {
            log.debug("restoring art");
            let r = MediaFile::open(&path).and_then(|mut mf| {
                mf.set_pictures(&images);
                mf.save(crate::media::id3v23())
            });
            if let Err(e) = r {
                log.error(&format!("could not write tags: {e:#}"));
            }
        }
    }
}

pub struct Scrub {
    auto: bool,
}

impl Scrub {
    pub fn new(cfg: &Config) -> Self {
        Scrub {
            auto: cfg.bool("scrub.auto"),
        }
    }
}

impl Plugin for Scrub {
    fn name(&self) -> &'static str {
        "scrub"
    }

    /// beets' `import_task_files` listener (registered with `auto`):
    /// scrub the imported files when tags are written.
    fn import_task_files(&self, ctx: &PluginCtx, task: &mut ImportTask) -> Result<()> {
        if !self.auto || !ctx.write {
            return Ok(());
        }
        let log = PluginLog::new("scrub");
        log.set_base_level(WARNING);
        for item in task.items.iter_mut() {
            log.debug(&format!("auto-scrubbing {}", item.path().display()));
            scrub_item(item, true, &log);
        }
        Ok(())
    }
}

/// `beet scrub [-W/--nowrite] [-l N] [QUERY]`.
pub fn command(lib: &Library, write: bool, limit: Option<usize>, query: &[String]) -> Result<()> {
    let log = PluginLog::new("scrub");
    log.set_base_level(INFO);
    let mut items = lib.items(&Query::parse(Kind::Item, query)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    for item in &mut items {
        log.info(&format!("scrubbing: {}", item.path().display()));
        scrub_item(item, write, &log);
    }
    Ok(())
}

#[cfg(test)]
#[path = "scrub/tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "scrub",
        order: 21,
        defaults: r#"scrub:
    auto: yes
"#,
        config_keys: &["scrub"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "scrub",
                build: build_scrub,
                run: run_scrub,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(Scrub::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ScrubArgs {
    /// Leave tags empty
    #[arg(short = 'W', long = "nowrite")]
    nowrite: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_scrub() -> clap::Command {
    use clap::Args as _;
    ScrubArgs::augment_args(clap::Command::new("scrub").about("clean audio tags"))
}

pub fn run_scrub(
    _cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ScrubArgs::from_arg_matches(m)?;
    crate::plugins::scrub::command(lib, !a.nowrite, a.limit, &a.query)
}
