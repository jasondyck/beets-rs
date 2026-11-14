//! beets' `thumbnails` plugin: freedesktop.org album-folder thumbnails.

use crate::config::Config;
use crate::events::{Arg, Event, Listener};
use crate::library::Library;
use crate::model::{Album, Kind};
use crate::plugins::{Plugin, PluginLog};
use crate::query::Query;
use anyhow::{Context as _, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn normal_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thumbnails/normal")
}
pub fn large_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thumbnails/large")
}

pub struct Thumbnails {
    pub auto: bool,
    pub force: bool,
    pub dolphin: bool,
    pub log: PluginLog,
}

impl Thumbnails {
    pub fn new(cfg: &Config) -> Self {
        Thumbnails {
            auto: cfg.bool("thumbnails.auto"),
            force: cfg.bool("thumbnails.force"),
            dolphin: cfg.bool("thumbnails.dolphin"),
            log: PluginLog::new("thumbnails"),
        }
    }

    /// `_check_local_ok`: image resizing and metadata writing are
    /// available, and the thumbnail directories exist.
    pub fn check(&self) -> bool {
        if !magick_available() {
            self.log.warning(
                "No local image resizing capabilities, cannot generate thumbnails",
            );
            return false;
        }
        for d in [normal_dir(), large_dir()] {
            if let Err(e) = std::fs::create_dir_all(&d) {
                self.log.warning(&format!("cannot create {}: {e}", d.display()));
                return false;
            }
        }
        true
    }

    pub fn process_album(&self, album: &Album) {
        self.log.debug(&format!("generating thumbnail for {}", album.str("album")));
        let artpath = album.path_field("artpath");
        if artpath.as_os_str().is_empty() {
            self.log.warning(&format!("album {} has no art", album.str("album")));
            return;
        }
        let dir = album_dir(album).unwrap_or_else(|| artpath.parent().unwrap_or(Path::new(".")).to_path_buf());
        if self.dolphin {
            self.make_dolphin_cover_thumbnail(&dir, artpath);
        }
        let Some((w, h)) = crate::artresizer::get_size(artpath) else {
            self.log.warning(&format!(
                "problem getting the picture size for {}",
                artpath.display()
            ));
            return;
        };
        let mut wrote = true;
        if w.max(h) >= 256 {
            wrote &= self.make_cover_thumbnail(&dir, artpath, 256, &large_dir());
        }
        wrote &= self.make_cover_thumbnail(&dir, artpath, 128, &normal_dir());
        if wrote {
            self.log.info(&format!("wrote thumbnail for {}", album.str("album")));
        } else {
            self.log.info(&format!("nothing to do for {}", album.str("album")));
        }
    }

    pub fn make_cover_thumbnail(
        &self,
        album_dir: &Path,
        artpath: &Path,
        size: u32,
        target_dir: &Path,
    ) -> bool {
        let target = target_dir.join(Self::thumbnail_file_name(album_dir));
        if target.exists()
            && let (Ok(tm), Ok(am)) = (
                std::fs::metadata(&target).and_then(|m| m.modified()),
                std::fs::metadata(artpath).and_then(|m| m.modified()),
            )
            && tm > am
        {
            if self.force {
                self.log.debug(&format!(
                    "found a suitable {size}x{size} thumbnail for {}, forcing regeneration",
                    album_dir.display()
                ));
            } else {
                self.log.debug(&format!(
                    "{size}x{size} thumbnail for {} exists and is recent enough",
                    album_dir.display()
                ));
                return false;
            }
        }
        let resized = target.with_extension("resize.png");
        if let Err(e) = crate::artresizer::resize_to(size, artpath, &resized, 0, 0) {
            self.log.warning(&format!("could not resize {}: {e}", artpath.display()));
            return false;
        }
        self.add_tags(album_dir, artpath, &resized);
        if let Err(e) = std::fs::rename(&resized, &target) {
            self.log.warning(&format!("could not move {}: {e}", resized.display()));
            return false;
        }
        true
    }

    pub fn thumbnail_file_name(path: &Path) -> String {
        let uri = uri(path);
        format!("{}.png", md5(uri.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    fn add_tags(&self, _album_dir: &Path, artpath: &Path, image: &Path) {
        let mtime = std::fs::metadata(artpath)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0.0, |d| d.as_secs_f64());
        if let Err(e) = write_metadata(
            image,
            &[
                ("Thumb::URI", uri(artpath)),
                ("Thumb::MTime", crate::hooks::py_float(mtime)),
            ],
        ) {
            self.log.warning(&format!(
                "could not write metadata to {}: {e}",
                image.display()
            ));
        }
    }

    fn make_dolphin_cover_thumbnail(&self, album_path: &Path, artpath: &Path) {
        let out = album_path.join(".directory");
        if out.exists() {
            return;
        }
        let name = artpath.file_name().unwrap_or_default().to_string_lossy();
        if std::fs::write(&out, format!("[Desktop Entry]\nIcon=./{name}")).is_ok() {
            self.log.debug(&format!("Wrote file {}", out.display()));
        }
    }
}

impl Plugin for Thumbnails {
    fn name(&self) -> &'static str {
        "thumbnails"
    }
}

impl Listener for Thumbnails {
    fn wants(&self, event: &str) -> bool {
        event == "art_set" && self.auto && self.check()
    }

    fn on(&self, event: &Event) {
        if let Some(Arg::Album(album)) = event.arg("album") {
            self.process_album(album);
        }
    }
}

pub struct CmdOpts {
    pub force: bool,
    pub dolphin: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

pub fn command(lib: &Library, opts: &CmdOpts) -> Result<()> {
    let plugin = Thumbnails {
        auto: true,
        force: opts.force,
        dolphin: opts.dolphin,
        log: PluginLog::new("thumbnails"),
    };
    if !plugin.check() {
        return Ok(());
    }
    let mut albums = lib.albums(&Query::parse(Kind::Album, &opts.query)?)?;
    if let Some(n) = opts.limit {
        albums.truncate(n);
    }
    for album in albums {
        plugin.process_album(&album);
    }
    Ok(())
}

fn album_dir(album: &Album) -> Option<PathBuf> {
    let art = album.path_field("artpath");
    art.parent().map(Path::to_path_buf)
}

fn magick_available() -> bool {
    std::process::Command::new("magick")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn write_metadata(path: &Path, values: &[(&str, String)]) -> Result<()> {
    let tmp = path.with_extension("meta.png");
    let mut cmd = std::process::Command::new("magick");
    cmd.arg(path);
    for (k, v) in values {
        cmd.arg("-set").arg(k).arg(v);
    }
    cmd.arg(&tmp);
    let status = cmd.status().context("running magick")?;
    if !status.success() {
        anyhow::bail!("magick failed");
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A `file://` URI, with percent-encoded non-ASCII and special bytes.
fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.as_os_str().as_encoded_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || "/-._~!$&'()*+,;=:@".contains(c) {
            out.push(c);
        } else {
            write!(out, "%{b:02X}").unwrap();
        }
    }
    out
}

// ---------------------------------------------------------------- MD5

fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | (!b & d), i)
            } else if i < 32 {
                ((d & b) | (!d & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
#[path = "thumbnails_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "thumbnails",
        order: 75,
        defaults: r#"thumbnails:
    auto: yes
    force: no
    dolphin: no
"#,
        config_keys: &["thumbnails"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            plugin: "thumbnails",
            build: build_thumbnails,
            run: run_thumbnails,
        }],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    let p = Thumbnails::new(cfg);
    // beets' `__init__`: registers an `art_set` listener when `auto` is set
    // and the environment can make thumbnails.
    if p.auto && p.check() {
        crate::events::register(std::sync::Arc::new(Thumbnails::new(cfg)));
    }
    Ok(Box::new(p))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct ThumbnailsArgs {
    /// Force regeneration of thumbnails deemed fine
    #[arg(short = 'f', long)]
    force: bool,
    /// Create Dolphin-compatible thumbnail information (for KDE)
    #[arg(long)]
    dolphin: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_thumbnails() -> clap::Command {
    use clap::Args as _;
    ThumbnailsArgs::augment_args(
        clap::Command::new("thumbnails").about("Create album thumbnails"),
    )
}

pub fn run_thumbnails(
    _cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = ThumbnailsArgs::from_arg_matches(m)?;
    crate::plugins::thumbnails::command(
        lib,
        &crate::plugins::thumbnails::CmdOpts {
            force: a.force,
            dolphin: a.dolphin,
            limit: a.limit,
            query: a.query,
        },
    )
}
