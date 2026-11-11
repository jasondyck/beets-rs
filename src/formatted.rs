//! beets' formatted views of items and albums for templates
//! (`dbcore.db.FormattedMapping`, `library.FormattedItemMapping`), the
//! library-dependent template functions (`%aunique`, `%sunique`,
//! `%tunique`) and path sanitizing (`util.sanitize_path`).

use crate::config::Config;
use crate::library::Library;
use crate::model::{ALBUM_FIELDS, Album, ITEM_FIELDS, Item, Kind, Record};
use crate::query::Query;
use crate::template::{Context, asciify_path};
use anyhow::{Result, anyhow};
use fancy_regex::Regex;
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A `[aunique]`/`[sunique]`/`[tunique]` config section.
#[derive(Debug, Clone)]
pub struct UniqueCfg {
    pub keys: String,
    pub disambiguators: String,
    pub bracket: String,
}

/// The settings formatting and path generation use.
#[derive(Debug, Clone)]
pub struct FormatEnv {
    /// Distinguishes settings in the library's memo of `%?unique` groups.
    id: u64,
    pub path_sep_replace: String,
    pub drive_sep_replace: String,
    pub asciify_paths: bool,
    /// `replace:`, in config order.
    pub replacements: Vec<(Regex, String)>,
    pub empty_component_replace: Option<String>,
    pub per_disc_numbering: bool,
    pub aunique: UniqueCfg,
    pub sunique: UniqueCfg,
    pub tunique: UniqueCfg,
}

/// beets' `CHAR_REPLACE`: the replacements used when the configured ones
/// would make a path too long.
fn default_replacements() -> Vec<(Regex, String)> {
    [
        (r"[\\/]", "_"),
        (r"^\.", "_"),
        (r"[\x00-\x1f]", ""),
        (r#"[<>:"\?\*\|]"#, "_"),
        (r"\.$", "_"),
        (r"\s+$", ""),
    ]
    .into_iter()
    .map(|(p, r)| (Regex::new(p).unwrap(), r.to_string()))
    .collect()
}

/// Turn a Python `re.sub` replacement (`\1`, `\g<name>`, backslash escapes)
/// into the `regex` crate's syntax (where `$` is special).
fn python_repl(r: &str) -> String {
    let mut out = String::new();
    let mut chars = r.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => out.push_str("$$"),
            '\\' => match chars.next() {
                Some(d) if d.is_ascii_digit() => {
                    let mut n = d.to_string();
                    if let Some(&d2) = chars.peek()
                        && d2.is_ascii_digit()
                    {
                        n.push(d2);
                        chars.next();
                    }
                    out.push_str(&format!("${{{n}}}"));
                }
                Some('g') if chars.peek() == Some(&'<') => {
                    chars.next();
                    let name: String = chars.by_ref().take_while(|&c| c != '>').collect();
                    out.push_str(&format!("${{{name}}}"));
                }
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out
}

impl FormatEnv {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut replacements = vec![];
        for (pat, rep) in cfg.mapping("replace") {
            let re = Regex::new(&pat)
                .map_err(|_| anyhow!("Malformed regular expression in replace: {pat}"))?;
            replacements.push((re, python_repl(&rep)));
        }
        let unique = |name: &str| UniqueCfg {
            keys: cfg.str(&format!("{name}.keys")),
            disambiguators: cfg.str(&format!("{name}.disambiguators")),
            bracket: cfg.str(&format!("{name}.bracket")),
        };
        let ecr = cfg.str("empty_component_replace");
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Ok(FormatEnv {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            path_sep_replace: cfg.str("path_sep_replace"),
            drive_sep_replace: cfg.str("drive_sep_replace"),
            asciify_paths: cfg.bool("asciify_paths"),
            replacements,
            empty_component_replace: match cfg.get("empty_component_replace") {
                Some(serde_yaml::Value::Bool(false) | serde_yaml::Value::Null) | None => None,
                _ if ecr.is_empty() => None,
                _ => Some(ecr),
            },
            per_disc_numbering: cfg.bool("per_disc_numbering"),
            aunique: unique("aunique"),
            sunique: unique("sunique"),
            tunique: unique("tunique"),
        })
    }

    pub fn defaults() -> Self {
        FormatEnv::new(&Config::defaults()).expect("default replacements compile")
    }

    fn unique_cfg(&self, name: &str) -> &UniqueCfg {
        match name {
            "aunique" => &self.aunique,
            "sunique" => &self.sunique,
            _ => &self.tunique,
        }
    }

    /// beets' `sanitize_path`: fill empty components
    /// (`empty_component_replace`), then apply the replacements (the
    /// configured ones, or with `None` beets' built-in `CHAR_REPLACE`) to
    /// each component.
    pub fn sanitize_path(&self, path: &str, user: bool) -> String {
        let filled;
        let path = match &self.empty_component_replace {
            Some(r) => {
                filled = fill_empty_components(path, r);
                filled.as_str()
            }
            None => path,
        };
        let defaults;
        let replacements = if user && !self.replacements.is_empty() {
            &self.replacements
        } else {
            defaults = default_replacements();
            &defaults
        };
        let comps: Vec<String> = components(path)
            .into_iter()
            .map(|mut comp| {
                for (re, rep) in replacements {
                    if let Ok(c) = re.try_replacen(&comp, 0, rep.as_str()) {
                        comp = c.into_owned();
                    }
                }
                comp
            })
            .collect();
        join_components(&comps)
    }
}

/// Fill every empty `/`-separated component (leading, trailing, or the
/// whole path) with `r`.
fn fill_empty_components(path: &str, r: &str) -> String {
    path.split('/')
        .map(|c| if c.is_empty() { r } else { c })
        .collect::<Vec<_>>()
        .join("/")
}

/// beets' `util.components`: the components of a path, with a leading run
/// of separators kept as the first component (`/a//b` → `/`, `a`, `b`);
/// empty components are dropped.
pub fn components(path: &str) -> Vec<String> {
    let mut out = vec![];
    let trimmed = path.trim_start_matches('/');
    let lead = path.len() - trimmed.len();
    if lead > 0 {
        out.push(path[..lead].to_string());
    }
    out.extend(
        trimmed
            .split('/')
            .filter(|c| !c.is_empty())
            .map(String::from),
    );
    out
}

/// `os.path.join(*comps)`.
fn join_components(comps: &[String]) -> String {
    let mut out = String::new();
    for c in comps {
        if c.starts_with('/') {
            out = c.clone();
        } else {
            if !out.is_empty() && !out.ends_with('/') {
                out.push('/');
            }
            out.push_str(c);
        }
    }
    out
}

thread_local! {
    static ENV: RefCell<Rc<FormatEnv>> = RefCell::new(Rc::new(FormatEnv::defaults()));
}

/// Take the formatting settings from the configuration (for this thread).
pub fn configure(cfg: &Config) -> Result<()> {
    let env = FormatEnv::new(cfg)?;
    ENV.with(|e| *e.borrow_mut() = Rc::new(env));
    Ok(())
}

/// The formatting settings `configure` set.
pub fn env() -> Rc<FormatEnv> {
    ENV.with(|e| e.borrow().clone())
}

// ------------------------------------------------------------ the mapping

/// beets' `album.item_keys`: album fields that are set on its items.
fn album_item_key(key: &str) -> bool {
    key != "artpath" && ALBUM_FIELDS.iter().any(|(n, _)| *n == key)
}

fn item_fixed_field(key: &str) -> bool {
    key == "id" || ITEM_FIELDS.iter().any(|(n, _)| *n == key)
}

/// Computed fields (beets' `_getters`).
fn computed(kind: Kind, key: &str) -> bool {
    match kind {
        Kind::Item => matches!(key, "singleton" | "filesize" | "has_cover_art"),
        Kind::Album => matches!(key, "path" | "albumtotal"),
    }
}

/// A formatted view of an item or album: each field's value as beets'
/// templates see it. An item's album is reachable (album fields and
/// flexible attributes; album values first in paths), `artist` and
/// `albumartist` fall back to each other, and in paths (`for_path`) values
/// can't add path separators.
pub struct Formatted<'a> {
    rec: &'a Record,
    album: OnceCell<Option<Album>>,
    given_album: Option<Option<&'a Album>>,
    for_path: bool,
    lib: Option<&'a Library>,
    env: Rc<FormatEnv>,
    items: OnceCell<Vec<Item>>,
}

impl<'a> Formatted<'a> {
    /// A view of `rec`; with a library, an item's album is loaded when
    /// needed and the `%?unique` functions work.
    pub fn new(rec: &'a Record, lib: Option<&'a Library>) -> Self {
        Self::with_env(rec, lib, env())
    }

    pub fn with_env(rec: &'a Record, lib: Option<&'a Library>, env: Rc<FormatEnv>) -> Self {
        Formatted {
            rec,
            album: OnceCell::new(),
            given_album: None,
            for_path: false,
            lib,
            env,
            items: OnceCell::new(),
        }
    }

    /// Use this album as the item's (instead of loading it).
    pub fn album(mut self, album: Option<&'a Album>) -> Self {
        self.given_album = Some(album);
        self
    }

    /// Format for a path (beets' `for_path`).
    pub fn for_path(mut self) -> Self {
        self.for_path = true;
        self
    }

    /// The item's album (given, or loaded from the library).
    pub fn the_album(&self) -> Option<&Album> {
        if self.rec.kind != Kind::Item {
            return None;
        }
        if let Some(given) = self.given_album {
            return given;
        }
        self.album
            .get_or_init(|| {
                let id = self.rec.int("album_id");
                if id == 0 {
                    return None;
                }
                self.lib?.get_album(id).ok().flatten()
            })
            .as_ref()
    }

    /// An album's items (for its computed fields).
    fn album_items(&self) -> &[Item] {
        self.items.get_or_init(|| match (self.lib, self.rec.id) {
            (Some(lib), Some(id)) if self.rec.kind == Kind::Album => {
                lib.album_items(id).unwrap_or_default()
            }
            _ => vec![],
        })
    }

    /// Whether `rec` is the record this mapping formats (not its album:
    /// an item and its album can share an id).
    fn is_self(&self, rec: &Record) -> bool {
        rec.kind == self.rec.kind && rec.id == self.rec.id
    }

    fn has_key(rec: &Record, key: &str) -> bool {
        rec.has_field(key) || computed(rec.kind, key)
    }

    /// beets' `FormattedItemMapping.album_keys` membership.
    fn album_key(&self, album: &Album, key: &str) -> bool {
        Self::has_key(album, key) && (album_item_key(key) || !item_fixed_field(key))
    }

    /// The value of one field of `rec` (the record itself or the album).
    fn value(&self, rec: &Record, key: &str) -> String {
        let v = match (rec.kind, key) {
            (Kind::Item, "filesize") => std::fs::metadata(rec.path())
                .map(|m| m.len().to_string())
                .unwrap_or_else(|_| "0".into()),
            (Kind::Item, "has_cover_art") => {
                let art = crate::media::has_art(rec.path()).unwrap_or(false);
                if art { "True" } else { "False" }.into()
            }
            (Kind::Album, "path") if self.is_self(rec) => self
                .album_items()
                .first()
                .and_then(|i| i.path().parent())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            (Kind::Album, "albumtotal") if self.is_self(rec) => {
                let items = self.album_items();
                let total = if rec.int("disctotal") == 1 || !self.env.per_disc_numbering {
                    items.first().map(|i| i.int("tracktotal")).unwrap_or(0)
                } else {
                    let mut counted = vec![];
                    let mut total = 0;
                    for i in items {
                        let disc = i.int("disc");
                        if counted.contains(&disc) {
                            continue;
                        }
                        total += i.int("tracktotal");
                        counted.push(disc);
                        if counted.len() as i64 == rec.int("disctotal") {
                            break;
                        }
                    }
                    total
                };
                total.to_string()
            }
            // An item's album's computed fields, through a view of it.
            (Kind::Album, "path" | "albumtotal") => {
                return Formatted::with_env(rec, self.lib, self.env.clone()).value(rec, key);
            }
            _ => rec.formatted(key).unwrap_or_default(),
        };
        if self.for_path { self.path_value(v) } else { v }
    }

    /// beets' `_get_formatted` for paths: a drive-like `X:` prefix gets
    /// `drive_sep_replace` for its colons, and `/` becomes
    /// `path_sep_replace`.
    fn path_value(&self, v: String) -> String {
        let b = v.as_bytes();
        let v = if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            let mut out = String::with_capacity(v.len());
            let mut prev_letter = false;
            for c in v.chars() {
                if c == ':' && prev_letter {
                    out.push_str(&self.env.drive_sep_replace);
                } else {
                    out.push(c);
                }
                prev_letter = c.is_ascii_alphabetic();
            }
            out
        } else {
            v
        };
        v.replace('/', &self.env.path_sep_replace)
    }

    fn get_raw(&self, key: &str) -> Option<String> {
        if self.rec.kind == Kind::Album {
            return Self::has_key(self.rec, key).then(|| self.value(self.rec, key));
        }
        // The album is only loaded when it can matter: in paths (album
        // values first), or for a key the item doesn't have.
        let own = Self::has_key(self.rec, key);
        if !own || self.for_path {
            let album = self.the_album().filter(|a| self.album_key(a, key));
            if let Some(a) = album {
                return Some(self.value(a, key));
            }
        }
        own.then(|| self.value(self.rec, key))
    }

    /// The formatted value of `key`, or None if there's no such field.
    pub fn get(&self, key: &str) -> Option<String> {
        let v = self.get_raw(key)?;
        if self.rec.kind == Kind::Item && v.is_empty() {
            // `artist` and `albumartist` fall back to one another.
            let other = match key {
                "artist" => Some("albumartist"),
                "albumartist" => Some("artist"),
                _ => None,
            };
            if let Some(o) = other.and_then(|o| self.get_raw(o)) {
                return Some(o);
            }
        }
        Some(v)
    }

    /// `formatted(for_path=True).get(field)`: missing fields are empty.
    fn path_field(rec: &Record, lib: Option<&Library>, env: &Rc<FormatEnv>, f: &str) -> String {
        Formatted::with_env(rec, lib, env.clone())
            .for_path()
            .get(f)
            .unwrap_or_default()
    }

    /// beets' `_tmpl_unique`. The collision groups of the whole scope
    /// (every album, every singleton, or one album's tracks) are computed
    /// once and memoized in the library until it changes, as beets'
    /// `_group_memotable`: a group is the records with the same raw key
    /// values (beets' `duplicates_query`) and the same path-normalized ones.
    #[allow(clippy::too_many_arguments)]
    fn unique(
        &self,
        lib: &Library,
        name: &str,
        args: &[String],
        id: i64,
        db_item: &Record,
        scope: i64,
        candidates: impl FnOnce() -> Vec<Record>,
        keys_default: Option<&str>,
        album: Option<&Album>,
    ) -> String {
        let cfg = self.env.unique_cfg(name);
        let arg = |i: usize| args.get(i).map(String::as_str);
        let bracket = arg(2).unwrap_or(&cfg.bracket);
        let keys = arg(0)
            .filter(|s| !s.is_empty())
            .or(keys_default)
            .unwrap_or(&cfg.keys);
        let disam = arg(1)
            .filter(|s| !s.is_empty())
            .unwrap_or(&cfg.disambiguators);
        let b: Vec<char> = bracket.chars().collect();
        let (l, r) = if b.len() == 2 {
            (b[0].to_string(), b[1].to_string())
        } else {
            (String::new(), String::new())
        };
        let env = &self.env;
        let memo_key = format!("{name}\0{keys}\0{disam}\0{scope}\0{}", env.id);
        let keys: Vec<&str> = keys.split_whitespace().collect();
        let disam: Vec<&str> = disam.split_whitespace().collect();
        let normalized = |rec: &Record, field: &str| {
            let view = Formatted::with_env(rec, self.lib, env.clone()).for_path();
            let view = if rec.kind == Kind::Item {
                view.album(album)
            } else {
                view
            };
            let mut v = view.get(field).unwrap_or_default();
            if env.asciify_paths {
                v = asciify_path(&v, &env.path_sep_replace);
            }
            env.sanitize_path(&v, true)
        };
        let group_key = |rec: &Record| -> GroupKey {
            (
                keys.iter().map(|k| format!("{:?}", rec.get(k))).collect(),
                keys.iter().map(|k| normalized(rec, k)).collect(),
            )
        };
        let groups = lib.memo(memo_key, || {
            let candidates = candidates();
            let mut members: HashMap<GroupKey, Vec<&Record>> = HashMap::new();
            for c in &candidates {
                members.entry(group_key(c)).or_default().push(c);
            }
            members
                .into_iter()
                .map(|(k, group)| {
                    let disambiguator = (group.len() > 1)
                        .then(|| {
                            disam.iter().find(|d| {
                                let values: HashSet<String> =
                                    group.iter().map(|c| normalized(c, d)).collect();
                                values.len() == group.len()
                            })
                        })
                        .flatten()
                        .map(|d| d.to_string());
                    (k, (group.len(), disambiguator))
                })
                .collect::<Groups>()
        });
        // A record whose key changed without being stored is in no group.
        let (count, disambiguator) = groups
            .get(&group_key(db_item))
            .cloned()
            .unwrap_or((0, None));
        if count <= 1 {
            return String::new();
        }
        match disambiguator {
            None => format!(" {l}{id}{r}"),
            Some(d) => {
                let v = Self::path_field(db_item, self.lib, env, &d);
                if v.is_empty() {
                    String::new()
                } else {
                    format!(" {l}{v}{r}")
                }
            }
        }
    }

    fn aunique(&self, args: &[String]) -> String {
        let Some(lib) = self.lib else {
            return String::new();
        };
        let album_id = match self.rec.kind {
            Kind::Item => self.rec.int("album_id"),
            Kind::Album => self.rec.id.unwrap_or(0),
        };
        if album_id == 0 {
            return String::new();
        }
        let loaded;
        let album = match self.rec.kind {
            Kind::Album => self.rec,
            Kind::Item => match self.the_album().filter(|a| a.id == Some(album_id)) {
                Some(a) => a,
                None => match lib.get_album(album_id).ok().flatten() {
                    Some(a) => {
                        loaded = a;
                        &loaded
                    }
                    None => return String::new(),
                },
            },
        };
        self.unique(
            lib,
            "aunique",
            args,
            album_id,
            album,
            0,
            || lib.all_albums().unwrap_or_default(),
            None,
            None,
        )
    }

    fn sunique(&self, args: &[String]) -> Result<String, String> {
        let Some(lib) = self.lib else {
            return Ok(String::new());
        };
        if self.rec.kind != Kind::Item {
            return Err("sunique is only implemented for items".into());
        }
        let Some(id) = self.rec.id else {
            return Ok(String::new());
        };
        if self.rec.int("album_id") != 0 {
            return Ok(String::new());
        }
        let singletons = || {
            Query::parse(Kind::Item, &["singleton:true"])
                .and_then(|q| lib.items(&q))
                .unwrap_or_default()
        };
        Ok(self.unique(
            lib, "sunique", args, id, self.rec, 0, singletons, None, None,
        ))
    }

    fn tunique(&self, args: &[String]) -> Result<String, String> {
        let Some(lib) = self.lib else {
            return Ok(String::new());
        };
        if self.rec.kind != Kind::Item {
            return Err("tunique is only implemented for items".into());
        }
        let Some(id) = self.rec.id else {
            return Ok(String::new());
        };
        let album_id = self.rec.int("album_id");
        if album_id == 0 {
            return Ok(String::new());
        }
        let keys = args
            .first()
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| self.env.tunique.keys.clone());
        let keys = if keys.split_whitespace().next().is_none() {
            "title".to_string()
        } else {
            keys
        };
        let album = self.the_album();
        Ok(self.unique(
            lib,
            "tunique",
            args,
            id,
            self.rec,
            album_id,
            || lib.album_items(album_id).unwrap_or_default(),
            Some(&keys),
            album,
        ))
    }
}

/// A collision group: the raw and the path-normalized key values.
type GroupKey = (Vec<String>, Vec<String>);
/// Each group's size and disambiguating field.
type Groups = HashMap<GroupKey, (usize, Option<String>)>;

impl Context for Formatted<'_> {
    fn field(&self, name: &str) -> Option<String> {
        self.get(name)
    }

    /// `field in item` (with the album's fields), then the field's
    /// non-path formatted value.
    fn ifdef(&self, name: &str) -> Option<String> {
        let exists = Self::has_key(self.rec, name)
            || self.the_album().is_some_and(|a| Self::has_key(a, name));
        if !exists {
            return None;
        }
        let view = Formatted {
            rec: self.rec,
            album: OnceCell::new(),
            given_album: Some(self.the_album()),
            for_path: false,
            lib: self.lib,
            env: self.env.clone(),
            items: OnceCell::new(),
        };
        Some(view.get(name).unwrap_or_default())
    }

    fn function(&self, name: &str, args: &[String]) -> Option<Result<String, String>> {
        match name {
            "aunique" => Some(Ok(self.aunique(args))),
            "sunique" => Some(self.sunique(args)),
            "tunique" => Some(self.tunique(args)),
            _ => None,
        }
    }

    fn sep_replace(&self) -> String {
        self.env.path_sep_replace.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Value;
    use crate::template::Template;

    fn render(t: &str, rec: &Record, album: Option<&Album>, for_path: bool) -> String {
        let f = Formatted::with_env(rec, None, Rc::new(FormatEnv::defaults())).album(album);
        let f = if for_path { f.for_path() } else { f };
        Template::parse(t).render(&f)
    }

    #[test]
    fn computed_and_typed_fields() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.mp3");
        std::fs::write(&p, [0u8; 1234]).unwrap();
        let mut it = Record::new_item();
        it.set_path(&p);
        it.set_int("bitrate", 320_999);
        it.set_int("samplerate", 44100);
        it.set("length", Value::Float(3725.9));
        it.set_int("track", 3);
        it.set_int("disc", 1);
        it.set_int("year", 999);
        it.set_str("format", "MP3");
        it.id = Some(7);
        assert_eq!(
            render(
                "$bitrate $samplerate $length $track/$disc $year $format $filesize $id",
                &it,
                None,
                false
            ),
            "320kbps 44kHz 62:05 03/01 0999 MP3 1234 7"
        );
        assert_eq!(render("$singleton $album_id", &it, None, false), "True ");
        assert_eq!(
            render("$path", &it, None, true),
            p.to_string_lossy().replace('/', "_")
        );
    }

    #[test]
    fn album_fields_and_fallbacks() {
        let mut it = Record::new_item();
        it.set_str("artist", "Solo");
        it.set_str("year", "2001");
        it.set_int("album_id", 1);
        let mut al = Record::new_album();
        al.id = Some(1);
        al.set_int("year", 1999);
        al.flex.insert("mood".into(), "calm/sad".into());
        // Item values first, album values first in paths.
        assert_eq!(render("$year", &it, Some(&al), false), "2001");
        assert_eq!(render("$year", &it, Some(&al), true), "1999");
        assert_eq!(render("$mood", &it, Some(&al), false), "calm/sad");
        assert_eq!(render("$mood", &it, Some(&al), true), "calm_sad");
        assert_eq!(render("$albumartist", &it, None, false), "Solo");
        assert_eq!(render("%ifdef{mood,y,n}", &it, Some(&al), true), "y");
        assert_eq!(render("%ifdef{mood}", &it, Some(&al), true), "calm/sad");
        assert_eq!(render("%ifdef{mood,y,n}", &it, None, true), "n");
        it.set_str("title", "C:/x: y");
        assert_eq!(render("$title", &it, None, true), "C__x_ y");
        assert_eq!(render("$title", &it, None, false), "C:/x: y");
        assert_eq!(render("%sunique{}", &al, None, false), "");
    }

    // An item can share its id with its album; the album's computed fields
    // must still come from the album (found by the diffbeets corpus).
    #[test]
    fn album_computed_fields_when_ids_coincide() {
        let lib = crate::library::Library::in_memory().unwrap();
        let mut items: Vec<Record> = (1..=2)
            .map(|t| {
                let mut it = Record::new_item();
                it.set_int("track", t);
                it.set_int("tracktotal", 2);
                it.set_path(format!("/lib/a/{t}.mp3"));
                it
            })
            .collect();
        let album = lib.add_album(&mut items).unwrap();
        assert_eq!(album.id, items[0].id);
        let env = Rc::new(FormatEnv::defaults());
        for it in &items {
            let f = Formatted::with_env(it, Some(&lib), env.clone());
            assert_eq!(Template::parse("$albumtotal").render(&f), "2");
        }
    }

    #[test]
    fn sanitize() {
        let env = FormatEnv::defaults();
        assert_eq!(env.sanitize_path("a//b/.c./d ", true), "a/b/_c_/d");
        // A leading empty component becomes `_` with the default rules.
        assert_eq!(env.sanitize_path("/a/b", true), "_/a/b");
        assert_eq!(env.sanitize_path("a/-b?", true), "a/_b_");
        let mut env = env;
        env.empty_component_replace = Some("_".into());
        assert_eq!(env.sanitize_path("/a//b/", true), "_/a/_/b/_");
        assert_eq!(python_repl(r"\1$x\g<n>"), "${1}$$x${n}");
    }
}
