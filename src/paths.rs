//! Destination paths for items: path-format selection, template evaluation,
//! sanitization and `%aunique{}`.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Album, Item, Kind};
use crate::query::Query;
use crate::template::{Context, Template};
use deunicode::deunicode;
use regex::Regex;
use std::cell::RefCell;
use std::path::PathBuf;

pub struct PathFormatter {
    formats: Vec<(Option<Query>, &'static str, Template)>,
    replace: Vec<(Regex, String)>,
    asciify: bool,
    max_len: usize,
    directory: PathBuf,
    aunique_keys: Vec<String>,
    aunique_disamb: Vec<String>,
    aunique_bracket: String,
}

/// Which built-in key a path format uses.
fn special(key: &str) -> Option<&'static str> {
    match key {
        "default" => Some("default"),
        "singleton" => Some("singleton"),
        "comp" => Some("comp"),
        _ => None,
    }
}

impl PathFormatter {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let mut formats = vec![];
        for (key, fmt) in cfg.mapping("paths") {
            let tmpl = Template::parse(&fmt);
            match special(&key) {
                Some(s) => formats.push((None, s, tmpl)),
                None => {
                    let args = shell_split(&key);
                    formats.push((Some(Query::parse(Kind::Item, &args)?), "", tmpl));
                }
            }
        }
        let mut replace = vec![];
        for (pat, rep) in cfg.mapping("replace") {
            replace.push((
                Regex::new(&pat).map_err(|e| anyhow::anyhow!("bad replace regex {pat:?}: {e}"))?,
                rep,
            ));
        }
        let max = cfg.f64("max_filename_length") as usize;
        let words = |k: &str| {
            cfg.str(k)
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        Ok(PathFormatter {
            formats,
            replace,
            asciify: cfg.bool("asciify_paths"),
            max_len: if max == 0 { 255 } else { max },
            directory: cfg.directory(),
            aunique_keys: words("aunique.keys"),
            aunique_disamb: words("aunique.disambiguators"),
            aunique_bracket: cfg.str("aunique.bracket"),
        })
    }

    fn choose(&self, item: &Item, album: Option<&Album>) -> Option<&Template> {
        for (q, special, t) in &self.formats {
            let hit = match (q, *special) {
                // As beets: the item's fields, then its album's.
                (Some(q), _) => q.matches_obj(&crate::query::Obj {
                    rec: item,
                    album,
                    album_dir: None,
                }),
                (None, "singleton") => item.int("album_id") == 0,
                (None, "comp") => item.bool("comp"),
                _ => false,
            };
            if hit {
                return Some(t);
            }
        }
        self.formats
            .iter()
            .find(|(_, s, _)| *s == "default")
            .map(|(_, _, t)| t)
    }

    /// Compute the library destination for an item. `album` is the item's
    /// album (if any); `lib` enables `%aunique{}` disambiguation.
    pub fn destination(
        &self,
        item: &Item,
        album: Option<&Album>,
        lib: Option<&Library>,
    ) -> PathBuf {
        let tmpl = match self.choose(item, album) {
            Some(t) => t,
            None => {
                return self
                    .directory
                    .join(item.path().file_name().unwrap_or_default());
            }
        };
        let ctx = ItemCtx {
            fmt: self,
            item,
            album,
            lib,
            albums_cache: RefCell::new(None),
        };
        let rendered = tmpl.render(&ctx);
        let rendered = if self.asciify {
            deunicode(&rendered)
        } else {
            rendered
        };

        let ext = item
            .path()
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        let comps: Vec<String> = rendered
            .split('/')
            .map(|c| self.sanitize(c))
            .filter(|c| !c.is_empty())
            .collect();
        let mut out = self.directory.clone();
        let n = comps.len();
        for (i, c) in comps.into_iter().enumerate() {
            let limit = if i + 1 == n {
                self.max_len.saturating_sub(ext.len())
            } else {
                self.max_len
            };
            out.push(truncate_bytes(&c, limit));
        }
        if n == 0 {
            out.push("_");
        }
        let mut s = out.into_os_string();
        s.push(&ext);
        PathBuf::from(s)
    }

    fn sanitize(&self, comp: &str) -> String {
        let mut s = comp.to_string();
        for (re, rep) in &self.replace {
            s = re.replace_all(&s, rep.as_str()).into_owned();
        }
        s
    }
}

fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end().to_string()
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

struct ItemCtx<'a> {
    fmt: &'a PathFormatter,
    item: &'a Item,
    album: Option<&'a Album>,
    lib: Option<&'a Library>,
    albums_cache: RefCell<Option<Vec<Album>>>,
}

impl Context for ItemCtx<'_> {
    fn field(&self, name: &str) -> Option<String> {
        // Item values first, then the album's (for album flexible attributes).
        let v = if self.item.has_field(name) || name == "singleton" {
            self.item.formatted(name)
        } else {
            self.album.and_then(|a| a.formatted(name))
        }?;
        // Field values must not introduce path separators.
        Some(v.replace('/', "_"))
    }

    fn aunique(&self, args: &[String]) -> String {
        let (Some(album), Some(lib)) = (self.album, self.lib) else {
            return String::new();
        };
        let split = |s: &str, d: &[String]| -> Vec<String> {
            if s.trim().is_empty() {
                d.to_vec()
            } else {
                s.split_whitespace().map(String::from).collect()
            }
        };
        let keys = split(
            args.first().map(|s| s.as_str()).unwrap_or(""),
            &self.fmt.aunique_keys,
        );
        let disamb = split(
            args.get(1).map(|s| s.as_str()).unwrap_or(""),
            &self.fmt.aunique_disamb,
        );
        let bracket = args
            .get(2)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or(self.fmt.aunique_bracket.clone());
        let mut bc = bracket.chars();
        let (l, r) = (
            bc.next().map(String::from).unwrap_or_default(),
            bc.next().map(String::from).unwrap_or_default(),
        );

        if self.albums_cache.borrow().is_none() {
            *self.albums_cache.borrow_mut() = Some(lib.all_albums().unwrap_or_default());
        }
        let cache = self.albums_cache.borrow();
        let same: Vec<&Album> = cache
            .as_ref()
            .unwrap()
            .iter()
            .filter(|a| keys.iter().all(|k| a.formatted(k) == album.formatted(k)))
            .collect();
        if same.len() <= 1 {
            return String::new();
        }
        for d in &disamb {
            let vals: Vec<String> = same
                .iter()
                .map(|a| a.formatted(d).unwrap_or_default())
                .collect();
            let mut uniq = vals.clone();
            uniq.sort();
            uniq.dedup();
            if uniq.len() == vals.len() {
                let mine = album.formatted(d).unwrap_or_default();
                return format!(" {l}{mine}{r}");
            }
        }
        format!(" {l}{}{r}", album.id.unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Record;

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
        assert_eq!(
            f.destination(&item(), None, None),
            PathBuf::from("/lib/Bells/Hells Bells_.mp3")
        );
        let f = fmt("");
        let mut it = item();
        it.set_int("album_id", 0);
        assert_eq!(
            f.destination(&it, None, None),
            PathBuf::from("/lib/Non-Album/AC_DC/Hells Bells_.mp3")
        );
        it.set_int("album_id", 1);
        it.set("comp", crate::model::Value::Bool(true));
        assert_eq!(
            f.destination(&it, None, None),
            PathBuf::from("/lib/Compilations/Back in Black/01 Hells Bells_.mp3")
        );
    }

    #[test]
    fn aunique_disambiguates() {
        let lib = Library::in_memory().unwrap();
        let f = fmt("");
        let mut a1 = vec![item()];
        a1[0].set_int("year", 1980);
        let al1 = lib.add_album(&mut a1).unwrap();
        let p = f.destination(&a1[0], Some(&al1), Some(&lib));
        assert_eq!(
            p,
            PathBuf::from("/lib/AC_DC/Back in Black/01 Hells Bells_.mp3")
        );
        let mut a2 = vec![item()];
        a2[0].set_int("year", 2003);
        let al2 = lib.add_album(&mut a2).unwrap();
        let p = f.destination(&a2[0], Some(&al2), Some(&lib));
        assert_eq!(
            p,
            PathBuf::from("/lib/AC_DC/Back in Black [2003]/01 Hells Bells_.mp3")
        );
    }

    #[test]
    fn split() {
        assert_eq!(
            shell_split("genre:jazz 'artist:miles davis'"),
            vec!["genre:jazz", "artist:miles davis"]
        );
    }
}
