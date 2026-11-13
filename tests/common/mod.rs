//! Shared helpers for the CLI integration tests: a temp `BEETSRSDIR` with a
//! generated-audio corpus, and runs of the built `beet` binary.
#![allow(dead_code)] // each test binary uses a subset of the helpers

use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// The binary cargo built for this test run.
pub const BEET: &str = env!("CARGO_BIN_EXE_beet");

/// The reference beets wrapper (the maintainer-approved); tests that use it skip
/// when it isn't there.
pub const REFERENCE: &str = "~/.cache/beets-reference/beet";

/// The config shared by the tests (and the reference comparisons).
pub fn config_text(root: &Path, plugins: &str) -> String {
    format!(
        "directory: {}/lib\n\
         library: {}/library.db\n\
         plugins: [{plugins}]\n\
         import:\n    \
             copy: yes\n    \
             write: no\n    \
             resume: no\n    \
             incremental: no\n    \
             duplicate_action: keep\n    \
             log:\n\
         ui:\n    \
             color: no\n\
         paths:\n    \
             default: $albumartist/$album/$track $title\n",
        root.display(),
        root.display()
    )
}

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Write a 0.5 s MP3 with these tags.
pub fn make_mp3(path: &Path, artist: &str, album: &str, title: &str, track: u32, year: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let status = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=d=0.5",
        ])
        .args(["-metadata", &format!("artist={artist}")])
        .args(["-metadata", &format!("album_artist={artist}")])
        .args(["-metadata", &format!("album={album}")])
        .args(["-metadata", &format!("title={title}")])
        .args(["-metadata", &format!("track={track}")])
        .args(["-metadata", &format!("date={year}")])
        .arg(path)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed for {}", path.display());
}

pub struct Out {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    pub fn stdout_lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }

    /// stdout without the database-backup lines a first library setup
    /// prints (beets-rs prints them on stdout, beets logs them to stderr).
    pub fn stdout_clean(&self) -> Vec<&str> {
        self.stdout
            .lines()
            .filter(|l| !l.starts_with("Created database backup at: "))
            .collect()
    }

    /// stderr without the database-backup lines.
    pub fn stderr_clean(&self) -> Vec<&str> {
        self.stderr
            .lines()
            .filter(|l| !l.starts_with("Created database backup at: "))
            .collect()
    }

    pub fn assert_ok(&self, what: &str) -> &Self {
        assert!(
            self.success(),
            "{what} failed (exit {}):\n--- stdout\n{}\n--- stderr\n{}",
            self.status,
            self.stdout,
            self.stderr
        );
        self
    }

    pub fn assert_stdout(&self, what: &str, expected: &str) -> &Self {
        assert_eq!(self.stdout, expected, "{what}");
        self
    }
}

/// A temp config dir (`BEETSRSDIR`), library dir and source corpus.
pub struct Env {
    pub tmp: TempDir,
    pub root: PathBuf,
    pub lib: PathBuf,
    pub src: PathBuf,
    pub db: PathBuf,
}

impl Env {
    pub fn new() -> Option<Env> {
        Env::with(Some(""), "")
    }

    pub fn with_plugins(plugins: &str) -> Option<Env> {
        Env::with(Some(plugins), "")
    }

    /// None without ffmpeg (the tests then skip).
    pub fn with(plugins: Option<&str>, extra: &str) -> Option<Env> {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg is not installed");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cfg");
        let lib = tmp.path().join("lib");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        let plugins = plugins.unwrap_or("");
        let mut config = config_text(tmp.path(), plugins);
        config.push_str(extra);
        std::fs::write(root.join("config.yaml"), config).unwrap();
        let db = tmp.path().join("library.db");
        let env = Env {
            tmp,
            db,
            root,
            lib,
            src,
        };
        // Create (and migrate) the library once, so later commands don't log
        // its setup.
        let _ = env.run(&["ls"]);
        Some(env)
    }

    /// An album of MP3s under the source corpus: `artist/album/NN title.mp3`.
    pub fn add_album(&self, artist: &str, album: &str, year: &str, titles: &[&str]) {
        for (i, title) in titles.iter().enumerate() {
            let path = self
                .src
                .join(artist)
                .join(album)
                .join(format!("{:02} {title}.mp3", i + 1));
            make_mp3(&path, artist, album, title, i as u32 + 1, year);
        }
    }

    /// `beet` with this environment, from the config dir.
    pub fn run(&self, args: &[&str]) -> Out {
        self.run_in(&self.root, args)
    }

    pub fn run_in(&self, cwd: &Path, args: &[&str]) -> Out {
        let out = Command::new(BEET)
            .args(args)
            .current_dir(cwd)
            .env("BEETSRSDIR", &self.root)
            .env_remove("BEETSDIR")
            .env("NO_COLOR", "1")
            .env("COLUMNS", "100")
            .env("TZ", "UTC")
            .output()
            .expect("run beet");
        Out {
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// The usual first step: `beet import -A -q` (as-is, quiet, copying).
    pub fn import(&self) -> Out {
        let out = self.run(&["import", "-A", "-q", self.src.to_str().unwrap()]);
        out.assert_ok("import");
        out
    }

    pub fn ls(&self, args: &[&str]) -> Vec<String> {
        let mut full = vec!["ls"];
        full.extend_from_slice(args);
        let out = self.run(&full);
        out.assert_ok("ls");
        out.stdout_lines().iter().map(|s| s.to_string()).collect()
    }

    /// The library paths of the items, as `ls -p` prints them.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.ls(&["-p"]).into_iter().map(PathBuf::from).collect()
    }

    pub fn file_exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// Make a file's mtime newer than now, so `beet update` re-reads it (a
/// same-second edit can compare equal and be skipped).
pub fn touch_newer(path: &Path) {
    let when = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let times = std::fs::FileTimes::new().set_modified(when);
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for touching")
        .set_times(times)
        .expect("set mtime");
}

pub fn reference_available() -> bool {
    Path::new(REFERENCE).exists() && ffmpeg_available()
}

/// Run the reference beets in its own temp config dir.
pub struct RefEnv {
    pub tmp: TempDir,
    pub root: PathBuf,
    pub lib: PathBuf,
    pub src: PathBuf,
}

impl RefEnv {
    pub fn new(plugins: &str) -> Option<RefEnv> {
        if !reference_available() {
            eprintln!("skipping: reference beets is not available");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ref");
        let lib = tmp.path().join("lib");
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(root.join("config.yaml"), config_text(tmp.path(), plugins)).unwrap();
        let env = RefEnv {
            tmp,
            root,
            lib,
            src,
        };
        let _ = env.run(&["ls"]);
        Some(env)
    }

    pub fn add_album(&self, artist: &str, album: &str, year: &str, titles: &[&str]) {
        for (i, title) in titles.iter().enumerate() {
            let path = self
                .src
                .join(artist)
                .join(album)
                .join(format!("{:02} {title}.mp3", i + 1));
            make_mp3(&path, artist, album, title, i as u32 + 1, year);
        }
    }

    pub fn run(&self, args: &[&str]) -> Out {
        let out = Command::new(REFERENCE)
            .args(args)
            .current_dir(&self.root)
            .env("BEETSDIR", &self.root)
            .env_remove("BEETSRSDIR")
            .env("NO_COLOR", "1")
            .env("COLUMNS", "100")
            .env("TZ", "UTC")
            .env("PYTHONWARNINGS", "ignore")
            .output()
            .expect("run reference beet");
        Out {
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    pub fn import(&self) -> Out {
        let out = self.run(&["import", "-A", "-q", self.src.to_str().unwrap()]);
        out.assert_ok("reference import");
        out
    }
}

/// stdout/stderr with each tool's own temp root replaced by `<ROOT>` and
/// beets' database-backup lines dropped.
pub fn normalise(text: &str, root: &Path) -> String {
    text.replace(&root.display().to_string(), "<ROOT>")
        .lines()
        .filter(|l| !l.starts_with("Created database backup at: "))
        .collect::<Vec<_>>()
        .join("\n")
}
