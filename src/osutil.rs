//! Platform differences in paths and file metadata: beets'
//! `bytestring_path`/`syspath`, and the parts of `os.stat`/`os.chmod`/
//! `os.access` the port relies on.
//!
//! beets keeps paths as the bytes `util.bytestring_path` gives:
//! `os.fsencode(path)`, which is the OS's bytes on Unix (`0xff` in a name
//! survives) and UTF-8 on Windows (where paths are Unicode). The functions
//! here are that conversion, so the rest of the port can compare paths as
//! bytes on every platform.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// A platform's path syntax: `os.path` joins components with `sep` and
/// accepts `alt_sep` too (Windows takes both `\` and `/`). The code that
/// legalizes path fragments takes one as a parameter so the Windows rules
/// can be tested anywhere; [`Flavor::NATIVE`] is the running platform's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flavor {
    pub sep: char,
    pub alt_sep: Option<char>,
}

impl Flavor {
    pub const UNIX: Flavor = Flavor {
        sep: '/',
        alt_sep: None,
    };

    pub const WINDOWS: Flavor = Flavor {
        sep: '\\',
        alt_sep: Some('/'),
    };

    pub const NATIVE: Flavor = if cfg!(windows) {
        Flavor::WINDOWS
    } else {
        Flavor::UNIX
    };

    /// Whether the character separates components (`os.sep`/`os.altsep`).
    pub fn is_sep(self, c: char) -> bool {
        c == self.sep || self.alt_sep == Some(c)
    }
}

/// The magic prefix `syspath` adds to Windows paths.
pub const MAGIC_PREFIX: &str = r"\\?\";

/// beets' `util.bytestring_path`: the bytes a path is stored and compared as.
#[cfg(unix)]
pub fn os_bytes(s: &OsStr) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(s.as_bytes())
}

/// beets' `util.bytestring_path`: UTF-8 on Windows, with the `\\?\` prefix
/// that `syspath` adds stripped again (so `bytestring_path(syspath(X)) == X`).
#[cfg(not(unix))]
pub fn os_bytes(s: &OsStr) -> Cow<'_, [u8]> {
    match s.to_str() {
        Some(t) => Cow::Borrowed(strip_magic_prefix(t).as_bytes()),
        None => Cow::Owned(s.to_string_lossy().into_owned().into_bytes()),
    }
}

/// beets' `util.bytestring_path` for a `Path`.
pub fn path_bytes(p: &Path) -> Cow<'_, [u8]> {
    os_bytes(p.as_os_str())
}

/// The inverse of [`os_bytes`]: a path from its stored bytes.
#[cfg(unix)]
pub fn os_string(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(b).to_os_string()
}

#[cfg(not(unix))]
pub fn os_string(b: &[u8]) -> OsString {
    String::from_utf8_lossy(b).into_owned().into()
}

/// The inverse of [`os_bytes`] for an owned byte vector.
#[cfg(unix)]
pub fn os_string_owned(b: Vec<u8>) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(b)
}

#[cfg(not(unix))]
pub fn os_string_owned(b: Vec<u8>) -> OsString {
    match String::from_utf8(b) {
        Ok(s) => s.into(),
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned().into(),
    }
}

/// beets' `util.syspath`: a path as the OS wants it. On Windows that means
/// the `\\?\` long-path prefix (and `\\?\UNC\` for UNC paths). The
/// platform is a parameter so the Windows rules can be tested on Unix.
pub fn syspath_of(p: &Path, windows: bool) -> PathBuf {
    if !windows {
        return p.to_path_buf();
    }
    // Separators are `/` in library paths (beets' internal bytestrings) and
    // in templates; a verbatim (`\\?\`) path must not contain any.
    let s = p.to_string_lossy().replace('/', "\\");
    if s.starts_with(MAGIC_PREFIX) {
        return s.into();
    }
    let s = if s.starts_with(r"\\") {
        format!("{MAGIC_PREFIX}UNC{}", &s[1..])
    } else {
        format!("{MAGIC_PREFIX}{s}")
    };
    s.into()
}

/// `syspath` for the running platform.
pub fn syspath(p: &Path) -> PathBuf {
    syspath_of(p, cfg!(windows))
}

/// `_from_db_path`: a stored relative path uses `/`; make it the platform's
/// separator (a no-op outside Windows).
pub fn db_path_to_native(path: Vec<u8>, windows: bool) -> Vec<u8> {
    if !windows {
        return path;
    }
    path.into_iter()
        .map(|c| if c == b'/' { b'\\' } else { c })
        .collect()
}

/// Strip the `\\?\` prefix `syspath` added (beets' `bytestring_path`).
#[cfg(not(unix))]
fn strip_magic_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix(MAGIC_PREFIX) {
        // `\\?\UNC\server\share` is `\\server\share` without the prefix.
        if let Some(unc) = rest.strip_prefix("UNC\\") {
            return unc;
        }
        return rest;
    }
    s
}

/// `os.path.normcase` for one path component's bytes: lowercase on
/// Windows, unchanged elsewhere (so undecodable bytes still compare).
#[cfg(unix)]
pub fn fold_case(b: Vec<u8>) -> Vec<u8> {
    b
}

#[cfg(not(unix))]
pub fn fold_case(b: Vec<u8>) -> Vec<u8> {
    match String::from_utf8(b) {
        Ok(s) => s.to_lowercase().into_bytes(),
        Err(e) => String::from_utf8_lossy(e.as_bytes())
            .to_lowercase()
            .into_bytes(),
    }
}

/// The components of a path as keys for `os.path.normcase`d comparison.
pub fn path_components_key(p: &Path) -> Vec<Vec<u8>> {
    p.components()
        .map(|c| fold_case(os_bytes(c.as_os_str()).into_owned()))
        .collect()
}

/// beets' `util.samefile` (without the `syspath` step, which Rust's std
/// does itself): whether two paths are the same file.
#[cfg(unix)]
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if a == b {
        return true;
    }
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(am), Ok(bm)) => (am.dev(), am.ino()) == (bm.dev(), bm.ino()),
        _ => false,
    }
}

#[cfg(not(unix))]
pub fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(am), Ok(bm)) => am == bm,
        _ => false,
    }
}

/// Python's `stat.S_IMODE`-style mode bits (`os.stat(p).st_mode`).
#[cfg(unix)]
pub fn mode(m: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode()
}

/// Windows has no Unix mode: Python reports the file type plus `0o666`
/// (`0o444` when read-only), and directories correspondingly.
#[cfg(not(unix))]
pub fn mode(m: &std::fs::Metadata) -> u32 {
    let kind = if m.is_dir() { 0o040_000 } else { 0o100_000 };
    let rw = if m.permissions().readonly() {
        0o444
    } else if m.is_dir() {
        0o777
    } else {
        0o666
    };
    kind | rw
}

/// `os.chmod`: on Windows only the read-only flag is honoured.
pub fn set_mode(p: &Path, perm: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(perm));
    }
    #[cfg(not(unix))]
    {
        if let Ok(m) = std::fs::metadata(p) {
            let mut perms = m.permissions();
            perms.set_readonly(perm & 0o200 == 0);
            let _ = std::fs::set_permissions(p, perms);
        }
    }
}

/// `os.access(path, bits)`: whether the mode bits ask for are set. Windows
/// has no execute bits and only a read-only flag, so `X_OK` always passes
/// and `W_OK` depends on that flag.
pub fn access(p: &Path, bits: u32) -> bool {
    #[cfg(unix)]
    {
        std::fs::metadata(p).is_ok_and(|m| mode(&m) & bits != 0)
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(p).is_ok_and(|m| bits & 0o222 == 0 || !m.permissions().readonly())
    }
}

/// `os.stat(path).st_mtime`: the file's modification time in seconds.
#[cfg(unix)]
pub fn mtime(m: &std::fs::Metadata) -> Option<f64> {
    use std::os::unix::fs::MetadataExt;
    Some(m.mtime() as f64 + m.mtime_nsec() as f64 * 1e-9)
}

#[cfg(not(unix))]
pub fn mtime(m: &std::fs::Metadata) -> Option<f64> {
    let t = m.modified().ok()?;
    Some(
        t.duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64()),
    )
}

/// `os.stat(path).st_mtime` of a path.
pub fn mtime_of(p: &Path) -> Option<f64> {
    mtime(&std::fs::metadata(p).ok()?)
}

/// Python's `Popen.returncode` when the process has no exit code: minus the
/// signal that killed it (`default` if there is none either). Windows has no
/// signals, so it falls back to `default`.
pub fn returncode_or(st: &std::process::ExitStatus, default: i32) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        st.code().unwrap_or_else(|| -st.signal().unwrap_or(default))
    }
    #[cfg(not(unix))]
    {
        st.code().unwrap_or(default)
    }
}

/// `shutil.which`: the program on `$PATH` that would run (Windows tries
/// the `PATHEXT` extensions, as Python does).
pub fn which(cmd: &str) -> Option<PathBuf> {
    let direct = cmd.contains('/') || (cfg!(windows) && cmd.contains('\\'));
    if direct {
        let p = PathBuf::from(cmd);
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        pathext
            .split(';')
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{cmd}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether `p` is a program to run: executable on Unix, any existing file
/// on Windows (`_access_check`).
fn is_executable(p: &Path) -> bool {
    if cfg!(windows) {
        p.is_file()
    } else {
        p.is_file() && access(p, 0o111)
    }
}

/// Create a symbolic link to a file (`os.symlink` for the importfeeds
/// plugin). On Windows, symlinks need Developer Mode or a privilege, as in
/// beets.
pub fn symlink_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        if src.is_dir() {
            std::os::windows::fs::symlink_dir(src, dst)
        } else {
            std::os::windows::fs::symlink_file(src, dst)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (src, dst);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symbolic links are not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bytes_round_trip() {
        for p in ["/a/b.mp3", "rel/x y.flac", "", "/ünïcode/日本.mp3"] {
            let os = os_string(p.as_bytes());
            assert_eq!(os_bytes(&os), p.as_bytes());
            let os = os_string_owned(p.as_bytes().to_vec());
            assert_eq!(os_bytes(&os), p.as_bytes());
        }
    }

    #[test]
    fn component_keys() {
        let k = path_components_key(Path::new("/a/b"));
        #[cfg(unix)]
        assert_eq!(k, vec![b"/".to_vec(), b"a".to_vec(), b"b".to_vec()]);
        #[cfg(not(unix))]
        assert_eq!(
            k,
            vec!["\\".to_string().into_bytes(), b"a".to_vec(), b"b".to_vec()]
        );
    }

    #[test]
    fn same_file_works() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        std::fs::write(&p, b"x").unwrap();
        assert!(same_file(&p, &p));
        assert!(same_file(&p, &dir.path().join(".").join("a.mp3")));
        assert!(!same_file(&p, &dir.path().join("b.mp3")));
    }

    #[test]
    fn modes_and_access() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        std::fs::write(&p, b"x").unwrap();
        assert!(access(&p, 0o222));
        assert!(access(&p, 0o444));
        let m = std::fs::metadata(&p).unwrap();
        let perm = mode(&m) & 0o777;
        set_mode(&p, perm & 0o777);
        #[cfg(unix)]
        assert_eq!(mode(&std::fs::metadata(&p).unwrap()) & 0o777, perm & 0o777);
        set_mode(&p, 0o600);
        #[cfg(unix)]
        {
            assert!(!access(&p, 0o044));
            assert!(access(&p, 0o200));
        }
        set_mode(&p, 0o644);
        #[cfg(unix)]
        assert!(access(&p, 0o044));
        #[cfg(not(unix))]
        assert!(!access(&p, 0o222) == std::fs::metadata(&p).unwrap().permissions().readonly());
    }

    #[test]
    fn mtimes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        std::fs::write(&p, b"x").unwrap();
        let m = std::fs::metadata(&p).unwrap();
        let t = mtime(&m).unwrap();
        assert!(t > 1_500_000_000.0);
        assert!((mtime_of(&p).unwrap() - t).abs() < 1e-6);
        assert!(mtime_of(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn syspath_is_identity_off_windows() {
        assert_eq!(syspath_of(Path::new("/a/b"), false), PathBuf::from("/a/b"));
    }

    #[test]
    fn syspath_prefixes_like_beets() {
        let win = |p: &str| syspath_of(Path::new(p), true);
        assert_eq!(win(r"C:\a\b"), PathBuf::from(r"\\?\C:\a\b"));
        // UNC paths become `\\?\UNC\...` (with one backslash lost).
        assert_eq!(
            win(r"\\server\share\x"),
            PathBuf::from(r"\\?\UNC\server\share\x")
        );
        // An already-prefixed path keeps its prefix.
        assert_eq!(win(r"\\?\C:\a"), PathBuf::from(r"\\?\C:\a"));
        // Library paths use `/`; a verbatim path must use `\` only.
        assert_eq!(win("C:/a/b"), PathBuf::from(r"\\?\C:\a\b"));
        assert_eq!(
            win("//server/share/x"),
            PathBuf::from(r"\\?\UNC\server\share\x")
        );
    }

    #[test]
    fn stored_paths_become_native() {
        assert_eq!(db_path_to_native(b"a/b.mp3".to_vec(), false), b"a/b.mp3");
        assert_eq!(db_path_to_native(b"a/b.mp3".to_vec(), true), br"a\b.mp3");
        assert_eq!(db_path_to_native(b"a".to_vec(), true), b"a");
    }
}
