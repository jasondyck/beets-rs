//! File operations shared by the importer, `move` and the plugins: beets'
//! `util.move`/`copy`/`link`/`hardlink`/`reflink`, `prune_dirs` (with
//! `clutter`) and the directory walk filter for `ignore`/`ignore_hidden`.

use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// beets' `MoveOperation`, plus `None` for "leave files where they are".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Copy,
    Move,
    /// Symbolic link (`import.link`).
    Link,
    /// Hard link (`import.hardlink`).
    Hardlink,
    /// Copy-on-write clone; an error where unsupported (`import.reflink: yes`).
    Reflink,
    /// Clone where possible, else a plain copy (`import.reflink: auto`).
    ReflinkAuto,
    None,
}

/// The `import.reflink` setting: yes, no or auto.
fn reflink_setting(cfg: &Config) -> Option<FileOp> {
    use serde_yaml::Value as Y;
    match cfg.get("import.reflink") {
        Some(Y::String(s)) if s == "auto" => Some(FileOp::ReflinkAuto),
        Some(Y::String(s)) => match crate::model::parse_bool(s) {
            Some(true) => Some(FileOp::Reflink),
            Some(false) => None,
            None => {
                eprintln!("warning: import.reflink must be yes, no or auto, not {s:?}");
                None
            }
        },
        _ if cfg.bool("import.reflink") => Some(FileOp::Reflink),
        _ => None,
    }
}

impl FileOp {
    /// The import file operation, with `copy` and `move` as given (they can
    /// be overridden on the command line) and the link settings from the
    /// config. As beets' `ImportSession.set_config`: move, link, hardlink
    /// and reflink, in that order, each win over everything after them,
    /// and copy only applies when none of them is set.
    pub fn for_import(cfg: &Config, copy: bool, mv: bool) -> FileOp {
        if mv {
            FileOp::Move
        } else if cfg.bool("import.link") {
            FileOp::Link
        } else if cfg.bool("import.hardlink") {
            FileOp::Hardlink
        } else if let Some(op) = reflink_setting(cfg) {
            op
        } else if copy {
            FileOp::Copy
        } else {
            FileOp::None
        }
    }

    /// Whether the source file stays where it was (everything but move).
    pub fn keeps_source(self) -> bool {
        self != FileOp::Move
    }
}

fn create_parent(dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
}

/// Carry out `op` from `src` to `dest`, creating `dest`'s directory. The
/// destination must not exist yet (callers pick a unique path first).
pub fn transfer(op: FileOp, src: &Path, dest: &Path) -> Result<()> {
    match op {
        FileOp::Copy => copy_file(src, dest),
        FileOp::Move => move_file(src, dest),
        FileOp::Link => link_file(src, dest),
        FileOp::Hardlink => hardlink_file(src, dest),
        FileOp::Reflink => reflink_file(src, dest, false),
        FileOp::ReflinkAuto => reflink_file(src, dest, true),
        FileOp::None => Ok(()),
    }
}

/// beets' `util.unique_path`: `path` if it doesn't exist, else the first
/// free `base.N.ext`, counting on from a `.N` the base already ends with.
pub fn unique_path(path: &Path) -> PathBuf {
    unique_path_for(path, None)
}

/// `unique_path` where `own` (the file being moved) counts as free, so a
/// file already at `foo.1.mp3` with `foo.mp3` taken stays put rather than
/// going on to `foo.2.mp3` (unlike beets, which flips between the two on
/// every move).
pub fn unique_path_for(path: &Path, own: Option<&Path>) -> PathBuf {
    let free = |p: &Path| own == Some(p) || !p.exists();
    if free(path) {
        return path.to_path_buf();
    }
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    // `os.path.splitext`: a leading dot doesn't start an extension.
    let (base, ext) = match name.rfind('.') {
        Some(i) if name[..i].chars().any(|c| c != '.') => name.split_at(i),
        _ => (name.as_str(), ""),
    };
    let (base, mut num) = match base.rsplit_once('.') {
        Some((b, n)) if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => {
            (b, n.parse::<u64>().unwrap_or(0))
        }
        _ => (base, 0),
    };
    loop {
        num += 1;
        let cand = path.with_file_name(format!("{base}.{num}{ext}"));
        if free(&cand) {
            return cand;
        }
    }
}

pub fn move_file(src: &Path, dest: &Path) -> Result<()> {
    // beets' `util.move`: nothing to do if it is already the same file;
    // the OS calls get `syspath` (the long-path prefix on Windows; Rust's
    // std adds it only for its own calls).
    if crate::osutil::same_file(src, dest) {
        return Ok(());
    }
    create_parent(dest)?;
    let (from, to) = (crate::osutil::syspath(src), crate::osutil::syspath(dest));
    if std::fs::rename(&from, &to).is_err() {
        // Cross-device: copy then remove.
        std::fs::copy(&from, &to)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        std::fs::remove_file(&from).with_context(|| format!("removing {}", src.display()))?;
    }
    Ok(())
}

pub fn copy_file(src: &Path, dest: &Path) -> Result<()> {
    if crate::osutil::same_file(src, dest) {
        return Ok(());
    }
    create_parent(dest)?;
    let (from, to) = (crate::osutil::syspath(src), crate::osutil::syspath(dest));
    std::fs::copy(&from, &to)
        .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    Ok(())
}

/// A symbolic link at `dest` pointing to `src`, as given (beets' `util.link`).
pub fn link_file(src: &Path, dest: &Path) -> Result<()> {
    create_parent(dest)?;
    let ctx = || format!("linking {} to {}", src.display(), dest.display());
    crate::osutil::symlink_file(&crate::osutil::syspath(src), &crate::osutil::syspath(dest))
        .with_context(ctx)
}

/// A hard link at `dest` to `src`, with symlinks in `src` resolved first
/// (beets' `util.hardlink`).
pub fn hardlink_file(src: &Path, dest: &Path) -> Result<()> {
    create_parent(dest)?;
    let ctx = || format!("linking {} to {}", src.display(), dest.display());
    let origin = std::fs::canonicalize(src).with_context(ctx)?;
    std::fs::hard_link(&origin, dest).map_err(|e| match e.kind() {
        ErrorKind::CrossesDevices => anyhow!("Cannot hard link across devices.").context(ctx()),
        _ => anyhow!(e).context(ctx()),
    })
}

/// A copy-on-write clone of `src` at `dest`. With `fallback` (reflink:
/// auto), any failure falls back to a plain copy; otherwise it's an error
/// explaining why (beets' `util.reflink`).
pub fn reflink_file(src: &Path, dest: &Path, fallback: bool) -> Result<()> {
    create_parent(dest)?;
    let ctx = || format!("reflinking {} to {}", src.display(), dest.display());
    let clone = reflink_copy::reflink(src, dest);
    if fallback {
        // beets' `reflink(..., fallback=True)` suppresses *any* error from
        // the clone and copies instead (an unsupported filesystem says
        // EOPNOTSUPP, but a sandboxed FICLONE ioctl can say EPERM, and so
        // on). Only an existing destination stays an error, since beets'
        // copy refuses to replace it.
        match clone {
            Ok(()) => Ok(()),
            Err(e) if dest.exists() => Err(anyhow!(e)).with_context(ctx),
            Err(_) => copy_file(src, dest),
        }
    } else {
        clone.map_err(|e| {
            let why = match e.kind() {
                ErrorKind::CrossesDevices => "Cannot reflink across devices",
                ErrorKind::Unsupported => "Device does not support reflinks",
                _ => return anyhow!(e).context(ctx()),
            };
            anyhow!(why).context(ctx())
        })
    }
}

/// Shell-style glob patterns (beets uses Python's `fnmatch`), matched
/// against a file's base name.
#[derive(Debug, Clone, Default)]
pub struct Globs(Vec<glob::Pattern>);

impl Globs {
    pub fn new(patterns: &[String]) -> Globs {
        Globs(
            patterns
                .iter()
                .map(|p| {
                    // fnmatch treats a malformed pattern (e.g. a lone "[")
                    // as literal text.
                    glob::Pattern::new(p)
                        .unwrap_or_else(|_| glob::Pattern::new(&glob::Pattern::escape(p)).unwrap())
                })
                .collect(),
        )
    }

    pub fn matches(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        self.0.iter().any(|p| p.matches(&name))
    }
}

/// The `clutter` setting: files that don't stop a directory being pruned.
pub fn clutter(cfg: &Config) -> Globs {
    Globs::new(&cfg.list("clutter"))
}

/// Remove `dir` and its ancestors up to (not including) `stop` while they
/// are empty or contain only `clutter` (beets' `util.prune_dirs`). Nothing
/// is removed if `dir` isn't inside `stop`.
pub fn prune_dirs(dir: &Path, stop: &Path, clutter: &Globs) {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d == stop || !d.starts_with(stop) {
            break;
        }
        if d.is_dir() {
            let Ok(entries) = std::fs::read_dir(d) else {
                break;
            };
            let only_clutter = entries
                .map(|e| e.map(|e| clutter.matches(&e.file_name())))
                .all(|m| m.unwrap_or(false));
            if !only_clutter || std::fs::remove_dir_all(d).is_err() {
                break;
            }
        }
        cur = d.parent();
    }
}

/// Which entries a directory walk for import skips: names matching
/// `ignore`, and hidden ones with `ignore_hidden` (beets' `sorted_walk`).
#[derive(Debug, Clone, Default)]
pub struct WalkFilter {
    pub ignore: Globs,
    pub ignore_hidden: bool,
}

impl WalkFilter {
    pub fn from_config(cfg: &Config) -> WalkFilter {
        WalkFilter {
            ignore: Globs::new(&cfg.list("ignore")),
            ignore_hidden: cfg.bool("ignore_hidden"),
        }
    }

    /// Whether to skip a file or directory with this base name. Outside
    /// Windows and macOS beets' "hidden" is a leading dot.
    pub fn skips(&self, name: &OsStr) -> bool {
        self.ignore.matches(name)
            || (self.ignore_hidden && name.as_encoded_bytes().first() == Some(&b'.'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_counts_on_like_beets() {
        let d = tempfile::tempdir().unwrap();
        let p = |n: &str| d.path().join(n);
        assert_eq!(unique_path(&p("a.mp3")), p("a.mp3"));
        for n in ["a.mp3", "a.1.mp3", "b.3.mp3", ".hidden"] {
            std::fs::write(p(n), b"").unwrap();
        }
        assert_eq!(unique_path(&p("a.mp3")), p("a.2.mp3"));
        assert_eq!(unique_path(&p("b.3.mp3")), p("b.4.mp3"));
        assert_eq!(unique_path(&p(".hidden")), p(".hidden.1"));
        // The file being moved doesn't block its own name.
        assert_eq!(
            unique_path_for(&p("a.mp3"), Some(&p("a.1.mp3"))),
            p("a.1.mp3")
        );
        assert_eq!(unique_path_for(&p("a.mp3"), Some(&p("a.mp3"))), p("a.mp3"));
    }

    /// Defaults with dotted keys set to YAML values.
    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let mut c = Config::defaults();
        for (k, v) in pairs {
            c.set(k, serde_yaml::from_str(v).unwrap());
        }
        c
    }

    #[test]
    fn import_operation_precedence_follows_beets() {
        let op = |pairs: &[(&str, &str)], copy, mv| FileOp::for_import(&cfg(pairs), copy, mv);
        assert_eq!(op(&[], true, false), FileOp::Copy);
        assert_eq!(op(&[], false, false), FileOp::None);
        assert_eq!(op(&[], true, true), FileOp::Move);
        // Move beats the links; the links beat copy, in beets' order.
        let all = [
            ("import.link", "yes"),
            ("import.hardlink", "yes"),
            ("import.reflink", "yes"),
        ];
        assert_eq!(op(&all, true, true), FileOp::Move);
        assert_eq!(op(&all, true, false), FileOp::Link);
        assert_eq!(op(&all, false, false), FileOp::Link);
        let hr = [("import.hardlink", "yes"), ("import.reflink", "auto")];
        assert_eq!(op(&hr, true, false), FileOp::Hardlink);
        let reflink = |v| op(&[("import.reflink", v)], true, false);
        assert_eq!(reflink("auto"), FileOp::ReflinkAuto);
        assert_eq!(reflink("yes"), FileOp::Reflink);
        assert_eq!(reflink("no"), FileOp::Copy);
    }

    #[test]
    fn link_hardlink_and_reflink_auto() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src.mp3");
        std::fs::write(&src, b"audio").unwrap();

        let sym = d.path().join("a/sym.mp3");
        transfer(FileOp::Link, &src, &sym).unwrap();
        assert!(sym.symlink_metadata().unwrap().file_type().is_symlink());
        // Rust's `symlink_file` stores the target as given (a plain path,
        // even when the link itself is created verbatim on Windows).
        assert_eq!(std::fs::read_link(&sym).unwrap(), src);

        // A hard link through the symlink links the real file.
        let hard = d.path().join("b/hard.mp3");
        transfer(FileOp::Hardlink, &sym, &hard).unwrap();
        assert!(!hard.symlink_metadata().unwrap().file_type().is_symlink());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                hard.metadata().unwrap().ino(),
                src.metadata().unwrap().ino()
            );
        }

        // reflink: auto always gets the file there, cloned or copied.
        let auto = d.path().join("c/auto.mp3");
        transfer(FileOp::ReflinkAuto, &src, &auto).unwrap();
        assert_eq!(std::fs::read(&auto).unwrap(), b"audio");
        assert!(src.exists());

        // reflink: yes either clones or explains why it couldn't.
        let strict = d.path().join("d/strict.mp3");
        match transfer(FileOp::Reflink, &src, &strict) {
            Ok(()) => assert_eq!(std::fs::read(&strict).unwrap(), b"audio"),
            Err(e) => assert!(format!("{e:#}").starts_with("reflinking "), "{e:#}"),
        }

        // An existing destination is never overwritten.
        assert!(transfer(FileOp::Link, &src, &sym).is_err());
        assert!(transfer(FileOp::Hardlink, &src, &hard).is_err());
    }

    #[test]
    fn prune_removes_directories_holding_only_clutter() {
        let d = tempfile::tempdir().unwrap();
        let clutter = clutter(&Config::defaults());
        let deep = d.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("Thumbs.DB"), b"").unwrap();
        std::fs::write(d.path().join("a/b/.DS_Store"), b"").unwrap();
        std::fs::write(d.path().join("a/keep.txt"), b"").unwrap();
        prune_dirs(&deep, d.path(), &clutter);
        assert!(!d.path().join("a/b").exists());
        assert!(d.path().join("a/keep.txt").exists());

        // Matching is case-sensitive, like fnmatch on POSIX; and without
        // clutter only empty directories go.
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("thumbs.db"), b"").unwrap();
        prune_dirs(&deep, d.path(), &clutter);
        assert!(deep.exists());
        std::fs::remove_file(deep.join("thumbs.db")).unwrap();
        std::fs::write(deep.join("Thumbs.DB"), b"").unwrap();
        prune_dirs(&deep, d.path(), &Globs::default());
        assert!(deep.exists());

        // Never outside or at the stop directory.
        prune_dirs(d.path(), d.path(), &clutter);
        assert!(d.path().exists());
        let other = tempfile::tempdir().unwrap();
        prune_dirs(other.path(), d.path(), &clutter);
        assert!(other.path().exists());
    }

    #[test]
    fn walk_filter_uses_beets_defaults() {
        let f = WalkFilter::from_config(&Config::defaults());
        for name in [
            ".hidden",
            "x.mp3~",
            "System Volume Information",
            "lost+found",
        ] {
            assert!(f.skips(OsStr::new(name)), "{name}");
        }
        assert!(!f.skips(OsStr::new("song.mp3")));
        let f = WalkFilter::from_config(&cfg(&[
            ("ignore", "['*.wav', '[ab]*']"),
            ("ignore_hidden", "no"),
        ]));
        assert!(f.skips(OsStr::new("x.wav")));
        assert!(f.skips(OsStr::new("b.mp3")));
        assert!(!f.skips(OsStr::new(".hidden")));
        assert!(!f.skips(OsStr::new("c.mp3")));
        // A malformed pattern is literal text.
        assert!(Globs::new(&["[x".into()]).matches(OsStr::new("[x")));
    }
}
