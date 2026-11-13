//! Destination paths (beets' `Item.destination`, `Album.art_destination`
//! and `util.legalize_path`): path-format selection, template evaluation,
//! `asciify_paths`, replacements and truncation.

use crate::config::Config;
use crate::formatted::{FormatEnv, Formatted};
use crate::library::Library;
use crate::logging::debug;
use crate::model::{Album, Item, Kind};
use crate::query::{Obj, Query, normpath};
use crate::template::{Template, asciify_path};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// beets' `MAX_FILENAME_LENGTH`.
const MAX_FILENAME_LENGTH: usize = 200;

pub struct PathFormatter {
    /// Query formats in config order (`comp` and `singleton` as the
    /// queries `comp:true` and `singleton:true`), then `default`.
    formats: Vec<(Query, Template)>,
    default: Option<Template>,
    env: Rc<FormatEnv>,
    max_len: usize,
    directory: PathBuf,
}

/// beets' `PF_KEY_QUERIES`.
fn key_query(key: &str) -> &str {
    match key {
        "comp" => "comp:true",
        "singleton" => "singleton:true",
        k => k,
    }
}

/// beets' `get_max_filename_length`: `max_filename_length`, else what the
/// library directory's filesystem allows (at most 200).
fn max_filename_length(cfg: &Config, directory: &Path) -> usize {
    let configured = cfg.f64("max_filename_length") as usize;
    if configured > 0 {
        return configured;
    }
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(directory.as_os_str().as_bytes()) else {
        return MAX_FILENAME_LENGTH;
    };
    // SAFETY: all-zero bytes are a valid `libc::statvfs` (a plain C struct of
    // integers); `statvfs` fills it in.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `st` a writable statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return MAX_FILENAME_LENGTH;
    }
    (st.f_namemax as usize).min(MAX_FILENAME_LENGTH)
}

impl PathFormatter {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        Self::with_formats(cfg, cfg.mapping("paths"), cfg.directory())
    }

    /// A formatter with its own path formats and base directory (beets'
    /// `Item.destination(basedir=..., path_formats=...)`); no formats
    /// means the library's.
    pub fn with_formats(
        cfg: &Config,
        path_formats: Vec<(String, String)>,
        directory: PathBuf,
    ) -> anyhow::Result<Self> {
        let path_formats = if path_formats.is_empty() {
            cfg.mapping("paths")
        } else {
            path_formats
        };
        let mut formats = vec![];
        let mut default = None;
        for (key, fmt) in path_formats {
            let tmpl = Template::parse(&fmt);
            if key == "default" {
                default = Some(tmpl);
            } else {
                let args = shell_split(key_query(&key));
                formats.push((Query::parse(Kind::Item, &args)?, tmpl));
            }
        }
        Ok(PathFormatter {
            formats,
            default,
            env: Rc::new(FormatEnv::new(cfg)?),
            max_len: max_filename_length(cfg, &directory),
            directory,
        })
    }

    /// The first path format (in config order) whose query matches the
    /// item, else `default`.
    fn choose(&self, item: &Item, album: Option<&Album>) -> Option<&Template> {
        let obj = Obj {
            rec: item,
            album,
            album_dir: None,
            lib: None,
            album_counts: None,
        };
        self.formats
            .iter()
            .find(|(q, _)| q.matches_obj(&obj))
            .map(|(_, t)| t)
            .or(self.default.as_ref())
    }

    /// The library destination for an item. `album` is the item's album
    /// (if any); `lib` makes the item's album and the `%?unique` functions
    /// available.
    pub fn destination(
        &self,
        item: &Item,
        album: Option<&Album>,
        lib: Option<&Library>,
    ) -> PathBuf {
        self.destination_ext(item, album, lib, None)
    }

    /// `destination` with the file's extension replaced by `extension`
    /// (beets' `extension` argument, without the dot).
    pub fn destination_ext(
        &self,
        item: &Item,
        album: Option<&Album>,
        lib: Option<&Library>,
        extension: Option<&str>,
    ) -> PathBuf {
        let view = Formatted::with_env(item, lib, self.env.clone()).for_path();
        // Without the album given, the view loads it from the library.
        let view = match album {
            Some(a) => view.album(Some(a)),
            None => view,
        };
        let Some(tmpl) = self.choose(item, view.the_album()) else {
            return self
                .directory
                .join(item.path().file_name().unwrap_or_default());
        };
        let mut subpath = tmpl.render(&view);
        if self.env.asciify_paths {
            subpath = asciify_path(&subpath, &self.env.path_sep_replace);
        }
        let ext = match extension {
            Some(e) => format!(".{e}"),
            None => item
                .path()
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
        };
        let (legal, fallback) = self.legalize_path(&subpath, &ext);
        if fallback {
            eprintln!(
                "Fell back to default replacements when naming file {subpath}. \
                 Configure replacements to avoid lengthening the filename."
            );
        }
        normpath(&self.directory.join(legal.trim_start_matches('/')))
    }

    /// One round of beets' `_legalize_stage`: sanitize, add the
    /// (lowercased) extension, truncate. Also whether truncation changed it.
    fn legalize_stage(&self, path: &str, user: bool, ext: &str) -> (String, bool) {
        let path = self.env.sanitize_path(path, user) + &ext.to_lowercase();
        let truncated = truncate_path(&path, self.max_len);
        let changed = truncated != path;
        (truncated, changed)
    }

    /// beets' `legalize_path`: sanitize and truncate, re-sanitizing after
    /// truncation; if that has to truncate again, use the default
    /// replacements instead of the configured ones (the flag).
    fn legalize_path(&self, path: &str, ext: &str) -> (String, bool) {
        let first = self.legalize_stage(path, true, ext).0;
        let first = splitext(&first).0.to_string();
        let (second, truncated) = self.legalize_stage(&first, true, ext);
        if !truncated {
            return (second, false);
        }
        debug!("path truncated: {second}");
        (self.legalize_stage(&first, false, ext).0, true)
    }
}

/// beets' `Album.art_destination`: `art_filename` evaluated for the album,
/// sanitized, in `item_dir` with the image's extension.
pub fn art_destination(
    cfg: &Config,
    album: &Album,
    image: &Path,
    item_dir: &Path,
    lib: Option<&Library>,
) -> anyhow::Result<PathBuf> {
    let env = Rc::new(FormatEnv::new(cfg)?);
    let view = Formatted::with_env(album, lib, env.clone()).for_path();
    let mut subpath = Template::parse(&cfg.str("art_filename")).render(&view);
    if env.asciify_paths {
        subpath = asciify_path(&subpath, &env.path_sep_replace);
    }
    let subpath = env.sanitize_path(&subpath, true);
    let ext = image
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    Ok(item_dir.join(format!("{subpath}{ext}")))
}

/// `os.path.splitext`: the extension of the last component, unless that
/// component starts with its only dots.
fn splitext(path: &str) -> (&str, &str) {
    let name_start = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let name = &path[name_start..];
    let lead = name.len() - name.trim_start_matches('.').len();
    match name[lead..].rfind('.') {
        Some(i) => path.split_at(name_start + lead + i),
        None => (path, ""),
    }
}

/// beets' `truncate_str`: at most `len` bytes, dropping a split character.
fn truncate_str(s: &str, len: usize) -> &str {
    if s.len() <= len {
        return s;
    }
    let mut end = len;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// beets' `truncate_path`: each component to `max` bytes, the last one's
/// stem to `max` less the extension's length.
fn truncate_path(path: &str, max: usize) -> String {
    let comps = crate::formatted::components(path);
    let Some((last, parents)) = comps.split_last() else {
        return String::new();
    };
    let (stem, suffix) = pathlib_suffix(last);
    let mut out: Vec<&str> = parents.iter().map(|p| truncate_str(p, max)).collect();
    let stem = truncate_str(stem, max.saturating_sub(suffix.chars().count()));
    out.push(stem);
    let mut joined = String::new();
    for c in out {
        if c.starts_with('/') {
            joined = "/".into();
        } else if !c.is_empty() {
            if !joined.is_empty() && !joined.ends_with('/') {
                joined.push('/');
            }
            joined.push_str(c);
        }
    }
    joined + suffix
}

/// pathlib's `stem`/`suffix` of a file name.
fn pathlib_suffix(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name.split_at(i),
        _ => (name, ""),
    }
}

/// Minimal shell-style split for query strings in config (handles quotes).
pub fn shell_split(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Record, Value};
    use crate::template::Template;

    fn fmt(extra: &str) -> PathFormatter {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(&file, format!("directory: /lib\n{extra}")).unwrap();
        PathFormatter::new(&Config::load_file(&file).unwrap()).unwrap()
    }

    fn item() -> Record {
        let mut it = Record::new_item();
        it.set_str("albumartist", "AC/DC");
        it.set_str("artist", "AC/DC");
        it.set_str("album", "Back in Black");
        it.set_str("title", "Hells Bells?");
        it.set_int("track", 1);
        it.set_int("album_id", 1);
        it.set_path("/in/01.MP3");
        it
    }

    fn dest(f: &PathFormatter, it: &Item) -> String {
        f.destination(it, None, None).to_string_lossy().into_owned()
    }

    #[test]
    fn default_path() {
        let p = fmt("").destination(&item(), None, None);
        assert_eq!(
            p,
            PathBuf::from("/lib/AC_DC/Back in Black/01 Hells Bells_.mp3")
        );
    }

    #[test]
    fn singleton_comp_and_query_formats() {
        let f = fmt("paths:\n  'title:bells': Bells/$title\n");
        assert_eq!(dest(&f, &item()), "/lib/Bells/Hells Bells_.mp3");
        let f = fmt("");
        let mut it = item();
        it.set_int("album_id", 0);
        assert_eq!(dest(&f, &it), "/lib/Non-Album/AC_DC/Hells Bells_.mp3");
        it.set_int("album_id", 1);
        it.set("comp", Value::Bool(true));
        assert_eq!(
            dest(&f, &it),
            "/lib/Compilations/Back in Black/01 Hells Bells_.mp3"
        );
    }

    // From beets' test_library.py DestinationTest.
    #[test]
    fn query_order_first_match_wins() {
        let f = fmt(
            "paths:\n  'title:hells': A/$title\n  'title:bells': B/$title\n  default: D/$title\n",
        );
        assert_eq!(dest(&f, &item()), "/lib/A/Hells Bells_.mp3");
        // `comp` is just a query, in config order.
        let f = fmt("paths:\n  'title:hells': A/$title\n  comp: C/$title\n");
        let mut it = item();
        it.set("comp", Value::Bool(true));
        assert_eq!(dest(&f, &it), "/lib/A/Hells Bells_.mp3");
        let f = fmt("paths:\n  comp: C/$title\n  'title:hells': A/$title\n");
        assert_eq!(dest(&f, &it), "/lib/C/Hells Bells_.mp3");
    }

    #[test]
    fn separators_in_values_and_templates() {
        let f = fmt(
            "paths:\n  default: $artist/%if{$comp,x,$album/sub}/$title\npath_sep_replace: '+'\n",
        );
        assert_eq!(
            dest(&f, &item()),
            "/lib/AC+DC/Back in Black/sub/Hells Bells_.mp3"
        );
        // A separator from `%asciify` becomes path_sep_replace too.
        let f = fmt("paths:\n  default: '%asciify{½}'\n");
        assert_eq!(dest(&f, &item()), "/lib/1_2.mp3");
    }

    #[test]
    fn asciify_and_replacements() {
        let mut it = item();
        it.set_str("title", "Café ½");
        let f = fmt("asciify_paths: yes\npaths:\n  default: $title\n");
        assert_eq!(dest(&f, &it), "/lib/Cafe  1_2.mp3");
        // `replace:` replaces the defaults wholly, applied in order.
        let f = fmt("paths:\n  default: $title\nreplace:\n  'é': e\n  'e': E\n");
        assert_eq!(dest(&f, &it), "/lib/CafE ½.mp3");
        let f = fmt("paths:\n  default: $title\nreplace:\n  '(\\w+) (\\S+)': '\\2 \\1'\n");
        assert_eq!(dest(&f, &it), "/lib/½ Café.mp3");
    }

    #[test]
    fn empty_components() {
        let mut it = item();
        it.set_str("album", "");
        // A leading empty component becomes `_` (the `[\\/]` rule)...
        let f = fmt("paths:\n  default: $album/$title\n");
        assert_eq!(dest(&f, &it), "/lib/_/Hells Bells_.mp3");
        // ... other empty components are dropped.
        let f = fmt("paths:\n  default: x/$album/$title\n");
        assert_eq!(dest(&f, &it), "/lib/x/Hells Bells_.mp3");
        let f = fmt("paths:\n  default: $title/$album/x\nempty_component_replace: _\n");
        assert_eq!(dest(&f, &it), "/lib/Hells Bells_/_/x.mp3");
        // Empty artist and albumartist leave an empty component.
        let mut it = item();
        it.set_str("artist", "");
        it.set_str("albumartist", "");
        let f = fmt("");
        assert_eq!(dest(&f, &it), "/lib/_/Back in Black/01 Hells Bells_.mp3");
    }

    #[test]
    fn truncation_keeps_extension() {
        let mut it = item();
        it.set_str("title", "x".repeat(30));
        it.set_str("album", "é".repeat(20));
        let f = fmt("max_filename_length: 10\npaths:\n  default: $album/$title\n");
        assert_eq!(
            dest(&f, &it),
            format!("/lib/{}/{}.mp3", "é".repeat(5), "x".repeat(6))
        );
        // Truncation leaving trailing space is re-sanitized.
        it.set_str("title", "abcde fghij");
        let f = fmt("max_filename_length: 10\npaths:\n  default: $title\n");
        assert_eq!(dest(&f, &it), "/lib/abcde.mp3");
    }

    // beets' test_util.py TestPathLegalization (max length 5).
    #[test]
    fn legalize_path_table() {
        let cases: &[(&str, &str, bool)] = &[
            ("{}", "_abcd", false),
            ("{'abcdX$': '1ST'}", ":1ST", false),
            ("{'abcdX$': 'TOO_LONG'}", ":TOO_", false),
            ("{'abcdX$': '1ST', '1ST$': '2ND'}", ":2ND", false),
            ("{'abcdX$': 'TOO_LONG', 'TOO_$': '2ND'}", ":2ND", false),
            ("{'abcdX$': '1ST', '1ST$': 'TOO_LONG'}", ":TOO_", false),
            ("{'abcdX$': 'TOO_LONG', 'TOO_$': 'TOO_LONG'}", "_TOO_", true),
        ];
        for (replace, want, truncated) in cases {
            let f = fmt(&format!("max_filename_length: 5\nreplace: {replace}\n"));
            assert_eq!(
                f.legalize_path(":abcdX", ""),
                (want.to_string(), *truncated),
                "{replace}"
            );
        }
        for (path, want) in [
            ("abcdeX/fgh", "abcde/fgh"),
            ("abcde/fXX.ext", "abcde/f.ext"),
            ("a🎹/a.ext", "a🎹/a.ext"),
            ("ab🎹/a.ext", "ab/a.ext"),
            ("f.a.e", "f.a.e"),
        ] {
            assert_eq!(truncate_path(path, 5), want);
        }
    }

    // beets' test_library.py TestDestination cases not covered above.
    #[test]
    fn upstream_destination_cases() {
        // test_unicode_extension_in_fragment
        let mut it = item();
        it.set_path("/in/bar.caf\u{e9}");
        let f = fmt("paths:\n  default: foo\n");
        assert_eq!(dest(&f, &it), "/lib/foo.caf\u{e9}");
        // test_albumtype_query_path / test_albumtype_path_fallback_to_comp
        let mut it = item();
        it.set("comp", Value::Bool(true));
        it.set_str("albumtype", "sometype");
        let f = fmt("paths:\n  default: two\n  albumtype:sometype: four\n  comp: three\n");
        assert_eq!(dest(&f, &it), "/lib/four.mp3");
        let f = fmt("paths:\n  default: two\n  albumtype:anothertype: four\n  comp: three\n");
        assert_eq!(dest(&f, &it), "/lib/three.mp3");
        // test_destination_with_empty_component / _final_component
        let mut it = item();
        it.set_str("artist", "");
        it.set_str("albumartist", "");
        it.set_str("album", "one");
        it.set_str("title", "three");
        let f = fmt("empty_component_replace: _\npaths:\n  default: $album/$artist/$title\n");
        assert_eq!(dest(&f, &it), "/lib/one/_/three.mp3");
        it.set_str("title", "");
        let f = fmt("empty_component_replace: _\npaths:\n  default: $album/$title\n");
        assert_eq!(dest(&f, &it), "/lib/one/_.mp3");
        // test_destination_escapes_leading_dot
        let mut it = item();
        it.set_str("album", ".something");
        assert_eq!(
            dest(&fmt(""), &it),
            "/lib/AC_DC/_something/01 Hells Bells_.mp3"
        );
    }

    #[test]
    fn custom_replacements_and_empty_leading_field() {
        // As beets' test_destination_stays_in_basedir_with_empty_leading_field.
        let mut it = item();
        it.set_str("album", "");
        let f = fmt("paths:\n  default: $album/$title\nreplace:\n  a: e\n");
        assert_eq!(dest(&f, &it), "/lib/Hells Bells?.mp3");
        it.set_str("album", "bar");
        assert_eq!(dest(&f, &it), "/lib/ber/Hells Bells?.mp3");
    }

    #[test]
    fn art_filename_template() {
        let mut al = Record::new_album();
        al.set_str("album", "Post/Live");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(&file, "art_filename: 'folder $album'\n").unwrap();
        let cfg = Config::load_file(&file).unwrap();
        let img = Path::new("/x/img.JPG");
        assert_eq!(
            art_destination(&cfg, &al, img, Path::new("/lib/a"), None).unwrap(),
            PathBuf::from("/lib/a/folder Post_Live.JPG")
        );
        assert_eq!(
            art_destination(&Config::defaults(), &al, img, Path::new("/a"), None).unwrap(),
            PathBuf::from("/a/cover.JPG")
        );
    }

    /// beets' `_common.item()` (the fields we have).
    fn upstream_item() -> Record {
        let mut it = Record::new_item();
        for (k, v) in [
            ("title", "the title"),
            ("artist", "the artist"),
            ("albumartist", "the album artist"),
            ("album", "the album"),
            ("format", "FLAC"),
        ] {
            it.set_str(k, v);
        }
        for (k, v) in [
            ("year", 1),
            ("month", 2),
            ("day", 3),
            ("track", 4),
            ("tracktotal", 5),
            ("disc", 6),
            ("disctotal", 7),
            ("bitrate", 128000),
        ] {
            it.set_int(k, v);
        }
        it.set("length", Value::Float(60.0));
        it.set_path("/in/x.mp3");
        it
    }

    fn base(t: &str) -> PathFormatter {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(
            &file,
            format!(
                "directory: /base\npaths:\n  default: {t}\n  singleton: {t}\n",
                t = serde_json::to_string(t).unwrap()
            ),
        )
        .unwrap();
        PathFormatter::new(&Config::load_file(&file).unwrap()).unwrap()
    }

    fn at(t: &str, it: &Item, lib: Option<&Library>) -> String {
        let p = base(t)
            .destination(it, None, lib)
            .to_string_lossy()
            .into_owned();
        p.strip_suffix(".mp3").unwrap_or(&p).to_string()
    }

    // beets' test_library.py TestDestinationFunction.
    #[test]
    fn upstream_destination_functions() {
        let mut it = upstream_item();
        it.flex.insert("bar".into(), "3".into());
        it.flex.insert("series".into(), "Now".into());
        it.set_str("albumtype", "album; ep; compilation");
        for (t, want) in [
            ("%upper{foo}", "/base/FOO"),
            ("%upper{$title}", "/base/THE TITLE"),
            ("%capitalize{$title}", "/base/The title"),
            ("%title{$title}", "/base/The Title"),
            ("%title{I can't}", "/base/I Can't"),
            ("%asciify{ab\u{a2}\u{bd}d}", "/base/abC_ 1_2d"),
            ("%left{$title, 3}", "/base/the"),
            ("%right{$title,3}", "/base/tle"),
            ("x%if{,foo}", "/base/x"),
            ("x%if{false,foo}", "/base/x"),
            ("%if{bar,foo}", "/base/foo"),
            ("%if{,foo,baz}", "/base/baz"),
            ("%if{false,foo,baz}", "/base/baz"),
            ("%if{0,foo,baz}", "/base/baz"),
            ("%foo{bar}", "/base/%foo{bar}"),
            ("%ifdef{bar}", "/base/3"),
            (" %ifdef{baz}/$artist", "/base/the artist"),
            ("$artist/%ifdef{baz}", "/base/the artist"),
            ("%ifdef{artist,cool}", "/base/cool"),
            (
                "%ifdef{series,$series Series,Albums}/$album",
                "/base/Now Series/the album",
            ),
            ("%ifdef{plays,$plays,not_played}", "/base/not_played"),
            ("%first{$albumtype,1,2}", "/base/compilation"),
            ("%first{Alice / Bob / Eve,2,0, / , & }", "/base/Alice & Bob"),
            // test_library.py DestinationTest.
            ("$year-$month-$day", "/base/0001-02-03"),
            (
                "$track $tracktotal $disc $disctotal $bpm",
                "/base/04 05 06 07 0",
            ),
            ("$album ($format)", "/base/the album (FLAC)"),
            ("$bitrate $length", "/base/128kbps 1_00"),
        ] {
            assert_eq!(at(t, &it, None), want, "template {t:?}");
        }
    }

    // beets' test_library.py TestDisambiguation.
    #[test]
    fn upstream_aunique() {
        let setup = || {
            let lib = Library::in_memory().unwrap();
            let mut i1 = upstream_item();
            i1.set_int("year", 2001);
            let mut v1 = vec![i1];
            let a1 = lib.add_album(&mut v1).unwrap();
            let mut i2 = upstream_item();
            i2.set_int("year", 2002);
            let mut v2 = vec![i2];
            let a2 = lib.add_album(&mut v2).unwrap();
            let (i1, i2) = (v1.remove(0), v2.remove(0));
            (lib, i1, i2, a1, a2)
        };
        let t = "foo%aunique{albumartist album,year}/$title";
        let (lib, i1, _i2, mut a1, mut a2) = setup();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo [2001]/the title");
        assert_eq!(
            at(
                "foo%aunique{albumartist album,year,()}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo (2001)/the title"
        );
        assert_eq!(
            at(
                "foo%aunique{albumartist album,year,}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo 2001/the title"
        );
        assert_eq!(
            at(
                "foo%aunique{albumartist album,year,()}%aunique{albumartist album,year,[]}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo (2001) [2001]/the title"
        );
        assert_eq!(
            at(
                "foo%aunique{albumartist album,month year}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo [2001]/the title"
        );
        a1.set_str("albumtype", "bar");
        lib.store(&a1).unwrap();
        assert_eq!(
            at("foo%aunique{}/$title", &i1, Some(&lib)),
            "/base/foo [bar]/the title"
        );
        // Without a library, nothing.
        assert_eq!(at(t, &i1, None), "/base/foo/the title");

        // Distinct albums need nothing; a third identical one does.
        a2.set_str("album", "different album");
        lib.store(&a2).unwrap();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo/the title");
        let mut i3 = upstream_item();
        i3.set_int("year", 2003);
        lib.add_album(&mut [i3]).unwrap();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo [2001]/the title");

        // Fallback numbers when the disambiguators are identical.
        let (lib, i1, i2, _, mut a2) = setup();
        a2.set_int("year", 2001);
        lib.store(&a2).unwrap();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo [1]/the title");
        assert_eq!(at(t, &i2, Some(&lib)), "/base/foo [2]/the title");

        // Disambiguation values are sanitized for the comparison.
        let (lib, i1, _, mut a1, mut a2) = setup();
        a2.set_int("year", 2001);
        a1.set_str("albumtype", "foo/bar");
        lib.store(&a1).unwrap();
        lib.store(&a2).unwrap();
        assert_eq!(
            at(
                "foo%aunique{albumartist album,albumtype}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo [foo_bar]/the title"
        );

        // An empty disambiguator is dropped.
        let (lib, i1, _, mut a1, mut a2) = setup();
        a1.set_str("albumdisambig", "");
        a2.set_str("albumdisambig", "foo");
        lib.store(&a1).unwrap();
        lib.store(&a2).unwrap();
        assert_eq!(
            at(
                "foo%aunique{albumartist album,albumdisambig}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo/the title"
        );

        // Flexible attributes as keys.
        let (lib, i1, _, mut a1, mut a2) = setup();
        a1.flex.insert("flex".into(), "flex1".into());
        a2.flex.insert("flex".into(), "flex2".into());
        lib.store(&a1).unwrap();
        lib.store(&a2).unwrap();
        assert_eq!(
            at(
                "foo%aunique{albumartist album flex,year}/$title",
                &i1,
                Some(&lib)
            ),
            "/base/foo/the title"
        );
    }

    // beets' test_library.py TestSingletonDisambiguation.
    #[test]
    fn upstream_sunique() {
        let setup = || {
            let lib = Library::in_memory().unwrap();
            let mut i1 = upstream_item();
            i1.set_int("year", 2001);
            lib.add(&mut i1).unwrap();
            let mut i2 = upstream_item();
            i2.set_int("year", 2002);
            lib.add(&mut i2).unwrap();
            (lib, i1, i2)
        };
        let t = "foo/$title%sunique{artist title,year}";
        let (lib, i1, mut i2) = setup();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo/the title [2001]");
        assert_eq!(
            at("foo/$title%sunique{artist title,year,()}", &i1, Some(&lib)),
            "/base/foo/the title (2001)"
        );
        assert_eq!(
            at("foo/$title%sunique{artist title,year,}", &i1, Some(&lib)),
            "/base/foo/the title 2001"
        );
        assert_eq!(
            at(
                "foo/$title%sunique{albumartist album,month year}",
                &i1,
                Some(&lib)
            ),
            "/base/foo/the title [2001]"
        );
        i2.set_str("trackdisambig", "live version");
        i2.set_int("year", 2001);
        lib.store(&i2).unwrap();
        assert_eq!(
            at("foo/$title%sunique{}", &i2, Some(&lib)),
            "/base/foo/the title [live version]"
        );

        let (lib, i1, mut i2) = setup();
        i2.set_int("year", 2001);
        lib.store(&i2).unwrap();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo/the title [1]");
        assert_eq!(at(t, &i2, Some(&lib)), "/base/foo/the title [2]");

        // Items in albums don't count, and get nothing.
        let (lib, i1, mut i2) = setup();
        lib.add_album(std::slice::from_mut(&mut i2)).unwrap();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo/the title");
        assert_eq!(at(t, &i2, Some(&lib)), "/base/foo/the title");

        let (lib, i1, mut i2) = setup();
        i2.set_str("title", "different track");
        lib.store(&i2).unwrap();
        assert_eq!(at(t, &i1, Some(&lib)), "/base/foo/the title");
    }

    // beets' test_library.py TestTrackDisambiguation.
    #[test]
    fn upstream_tunique() {
        let lib = Library::in_memory().unwrap();
        let mk = |track| {
            let mut i = upstream_item();
            i.set_str("title", "Common Title");
            i.set_int("track", track);
            i.set_int("disc", 1);
            i.set_str("artist", "Some Artist");
            i
        };
        let mut items = vec![mk(7), mk(11)];
        lib.add_album(&mut items).unwrap();
        let t = "$title%tunique{title,track}";
        assert_eq!(at(t, &items[0], Some(&lib)), "/base/Common Title [07]");
        assert_eq!(at(t, &items[1], Some(&lib)), "/base/Common Title [11]");
        items[1].set_int("disc", 2);
        lib.store(&items[1]).unwrap();
        assert_eq!(
            at("$title%tunique{}", &items[0], Some(&lib)),
            "/base/Common Title [01]"
        );
        assert_eq!(
            at("$title%tunique{}", &items[1], Some(&lib)),
            "/base/Common Title [02]"
        );
        // A singleton with the same title is not in the album's scope.
        let mut s = mk(3);
        lib.add(&mut s).unwrap();
        assert_eq!(at(t, &s, Some(&lib)), "/base/Common Title");
        assert_eq!(at(t, &items[0], Some(&lib)), "/base/Common Title [07]");
        // Albums can't use it.
        let al = lib.get_album(items[0].int("album_id")).unwrap().unwrap();
        let f = crate::formatted::Formatted::new(&al, Some(&lib));
        assert_eq!(
            Template::parse("%tunique{}").render(&f),
            "<tunique is only implemented for items>"
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(splitext("a/b.c/d"), ("a/b.c/d", ""));
        assert_eq!(splitext("a/.mp3"), ("a/.mp3", ""));
        assert_eq!(splitext("a/x.y.mp3"), ("a/x.y", ".mp3"));
        assert_eq!(truncate_str("🎹🎶", 6), "🎹");
        assert_eq!(
            shell_split("genre:jazz 'artist:miles davis'"),
            vec!["genre:jazz", "artist:miles davis"]
        );
    }
}
