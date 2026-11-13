//! `beet migrate-beets`: copy a beets library database into beets-rs'.
//!
//! The beets database is opened read-only and immutable (SQLite's
//! `mode=ro&immutable=1`), so nothing is ever written to it or beside it.

use crate::config::{Config, expand_tilde};
use crate::library::Library;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// beets' default `directory`.
const BEETS_DEFAULT_DIRECTORY: &str = "~/Music";

pub fn migrate_beets(
    cfg: &Config,
    lib: &Library,
    database: &Path,
    beets_config: Option<&Path>,
    directory: Option<&Path>,
    force: bool,
) -> Result<()> {
    if !database.is_file() {
        bail!("no beets library at {}", database.display());
    }
    let ours = cfg.library_path();
    if let (Ok(a), Ok(b)) = (database.canonicalize(), ours.canonicalize())
        && a == b
    {
        bail!("{} is the beets-rs library itself", database.display());
    }
    // An immutable open ignores a journal or write-ahead log: with either
    // around, beets may be writing (or crashed while writing).
    for suffix in ["-journal", "-wal"] {
        let mut p = database.as_os_str().to_owned();
        p.push(suffix);
        let p = PathBuf::from(p);
        if std::fs::metadata(&p).is_ok_and(|m| m.len() > 0) {
            bail!(
                "{} exists: beets may be using the library. Close beets (or let a beets \
                 command finish) and try again",
                p.display()
            );
        }
    }
    let src_dir = match (directory, beets_config) {
        (Some(d), _) => Some(crate::query::normpath(d)),
        (None, Some(c)) => Some(beets_directory(c)?),
        (None, None) => None,
    };
    let (items, albums) = lib.counts()?;
    if items + albums > 0 {
        if !force {
            bail!(
                "the beets-rs library already has {items} items and {albums} albums; \
                 use --force to replace them"
            );
        }
        lib.backup(&ours, "-before-migrate-beets.bak")?;
    }
    let src = open_read_only(database)?;
    let n = lib
        .copy_from_beets(&src, src_dir.as_deref())
        .with_context(|| format!("copying {}", database.display()))?;
    drop(src);
    println!(
        "Copied {} items and {} albums ({} item and {} album attributes) from {}.",
        n.items,
        n.albums,
        n.item_attrs,
        n.album_attrs,
        database.display()
    );
    // A wrong directory shows as files that aren't there.
    let all = crate::query::Query::parse::<&str>(crate::model::Kind::Item, &[])?;
    let missing: Vec<PathBuf> = lib
        .items(&all)?
        .iter()
        .map(|i| i.path().to_path_buf())
        .filter(|p| !p.exists())
        .collect();
    if let Some(first) = missing.first() {
        eprintln!(
            "warning: {} of {} items' files don't exist (such as {}); is the beets library \
             directory right?",
            missing.len(),
            n.items,
            first.display()
        );
    }
    Ok(())
}

/// The beets database, read-only and immutable.
fn open_read_only(path: &Path) -> Result<Connection> {
    let path = std::path::absolute(path)?;
    let uri = format!(
        "file:{}?mode=ro&immutable=1",
        uri_escape(path.as_os_str().as_bytes())
    );
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", path.display()))?;
    // Fail here, not halfway, on a file that isn't a database.
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .with_context(|| format!("reading {}", path.display()))?;
    Ok(conn)
}

/// Percent-encode a path for an SQLite `file:` URI.
fn uri_escape(path: &[u8]) -> String {
    let mut out = String::new();
    for &b in path {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            write!(out, "%{b:02X}").unwrap();
        }
    }
    out
}

/// beets' `directory` from its config file (read only): `~` expanded,
/// relative to the config's directory, `~/Music` when unset.
fn beets_directory(config: &Path) -> Result<PathBuf> {
    let text =
        std::fs::read_to_string(config).with_context(|| format!("reading {}", config.display()))?;
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&text).with_context(|| format!("parsing {}", config.display()))?;
    let dir = match doc.get("directory") {
        Some(serde_yaml::Value::String(s)) => s.clone(),
        None | Some(serde_yaml::Value::Null) => BEETS_DEFAULT_DIRECTORY.into(),
        Some(_) => bail!("{}: directory must be a filename", config.display()),
    };
    let dir = expand_tilde(&dir);
    let dir = if dir.is_absolute() {
        dir
    } else {
        std::path::absolute(config)?
            .parent()
            .unwrap_or(Path::new("/"))
            .join(dir)
    };
    Ok(crate::query::normpath(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;
    use crate::query::Query;

    struct Env {
        d: tempfile::TempDir,
        cfg: Config,
        lib: Library,
    }

    /// A beets-rs library in a temp dir, with its music directory there.
    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.config_dir = d.path().join("rs");
        cfg.set(
            "directory",
            d.path().join("rs/music").to_string_lossy().into(),
        );
        cfg.set(
            "library",
            d.path().join("rs/library.db").to_string_lossy().into(),
        );
        let lib = Library::open_with(&cfg.library_path(), Some(&cfg.directory()), true).unwrap();
        Env { d, cfg, lib }
    }

    /// A hash of the file's bytes, its mtime, and what its directory holds.
    fn snapshot(db: &Path) -> (u64, std::time::SystemTime, Vec<std::ffi::OsString>) {
        let mut names: Vec<_> = std::fs::read_dir(db.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::fs::read(db).unwrap().hash(&mut h);
        (
            h.finish(),
            std::fs::metadata(db).unwrap().modified().unwrap(),
            names,
        )
    }

    fn items(lib: &Library) -> Vec<crate::model::Item> {
        lib.items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
            .unwrap()
    }

    /// A beets 2.14 library made by beets: copied with its ids, album
    /// links, flexible attributes and lists; its paths, relative to beets'
    /// directory, point at the same files; the beets database is never
    /// written (same bytes and mtime, no journal or other file beside it).
    #[test]
    fn copies_a_beets_library_read_only() {
        let e = env();
        let beets = e.d.path().join("beets");
        std::fs::create_dir(&beets).unwrap();
        let db = beets.join("library.db");
        crate::library::make_beets_fixture(&db);
        let beets_music = beets.join("music");
        std::fs::write(beets.join("config.yaml"), "directory: music\n").unwrap();
        let before = snapshot(&db);
        // Its paths are relative: the directory is needed.
        let err = migrate_beets(&e.cfg, &e.lib, &db, None, None, false).unwrap_err();
        assert!(format!("{err:#}").contains("--directory"), "{err:#}");
        assert_eq!(e.lib.counts().unwrap(), (0, 0));
        migrate_beets(
            &e.cfg,
            &e.lib,
            &db,
            Some(&beets.join("config.yaml")),
            None,
            false,
        )
        .unwrap();
        assert_eq!(snapshot(&db), before);

        let its = items(&e.lib);
        assert_eq!(its.len(), 3);
        let one = its.iter().find(|i| i.str("title") == "One").unwrap();
        assert_eq!(one.id, Some(1));
        assert_eq!(one.int("album_id"), 1);
        assert_eq!(
            one.path(),
            beets_music.join("Artist/Ünïcode Album/01 One.mp3")
        );
        assert_eq!(one.list("genres"), ["Rock", "Pop"]);
        assert_eq!(one.str("mood"), "happy");
        assert_eq!(one.str("rating"), "5");
        assert_eq!(one.float("added"), 1700000000.5);
        let single = its.iter().find(|i| i.str("artist") == "Solo").unwrap();
        assert_eq!((single.id, single.int("album_id")), (Some(3), 0));
        assert_eq!(single.str("comments"), "hello");
        let albums = e.lib.all_albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].str("rating"), "5");
        assert_eq!(albums[0].float("added"), 1700000000.25);

        // Not again into a library that has music, unless forced (and
        // then backed up first).
        let err = migrate_beets(&e.cfg, &e.lib, &db, None, Some(&beets_music), false).unwrap_err();
        assert!(format!("{err:#}").contains("--force"), "{err:#}");
        migrate_beets(&e.cfg, &e.lib, &db, None, Some(&beets_music), true).unwrap();
        assert_eq!(e.lib.counts().unwrap(), (3, 1));
        assert!(
            e.d.path()
                .join("rs/library.db-before-migrate-beets.bak")
                .exists()
        );
        assert_eq!(snapshot(&db), before);
    }

    /// When beets' directory is beets-rs' too, the rows are beets' own,
    /// byte for byte, paths included.
    #[test]
    fn same_directory_copies_rows_as_they_are() {
        let e = env();
        let db = e.d.path().join("beets.db");
        crate::library::make_beets_fixture(&db);
        migrate_beets(&e.cfg, &e.lib, &db, None, Some(&e.cfg.directory()), false).unwrap();
        let rows = |c: &Connection, t: &str| -> Vec<Vec<rusqlite::types::Value>> {
            let mut stmt = c
                .prepare(&format!("SELECT * FROM {t} ORDER BY rowid"))
                .unwrap();
            let n = stmt.column_count();
            stmt.query_map([], |r| (0..n).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let ours = Connection::open(e.cfg.library_path()).unwrap();
        let theirs = Connection::open(&db).unwrap();
        for t in [
            "items",
            "albums",
            "item_attributes",
            "album_attributes",
            "migrations",
        ] {
            assert_eq!(rows(&ours, t), rows(&theirs, t), "{t}");
        }
    }

    /// A library from an older beets: absolute paths, the single-valued
    /// `genre`, lyrics with a source footer, `artpath` inherited onto
    /// items, no record of migrations. beets' migrations run on the copy.
    #[test]
    fn copies_an_older_beets_library() {
        let e = env();
        let db = e.d.path().join("old.db");
        let music = e.cfg.directory();
        let m = music.display();
        Connection::open(&db)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, path BLOB, album_id INTEGER,
                     title TEXT, genre TEXT, lyrics TEXT, added REAL);
                 CREATE TABLE albums (id INTEGER PRIMARY KEY, album TEXT, artpath BLOB, genre TEXT);
                 CREATE TABLE item_attributes (id INTEGER PRIMARY KEY, entity_id INTEGER,
                     key TEXT, value TEXT, UNIQUE(entity_id, key) ON CONFLICT REPLACE);
                 CREATE TABLE album_attributes (id INTEGER PRIMARY KEY, entity_id INTEGER,
                     key TEXT, value TEXT, UNIQUE(entity_id, key) ON CONFLICT REPLACE);
                 INSERT INTO albums VALUES (4, 'Al', CAST('{m}/Al/cover.jpg' AS BLOB), 'Rock, Pop');
                 INSERT INTO items VALUES
                     (7, '{m}/Al/1.mp3', 4, 'One', 'Rock, Pop', 'la\n\nSource: https://genius.com/x', 1.5),
                     (9, CAST('/elsewhere/2.mp3' AS BLOB), NULL, 'Two', '', '', 2.5);
                 INSERT INTO item_attributes (entity_id, key, value) VALUES
                     (7, 'artpath', '/x.jpg'), (7, 'mood', 'calm'), (8, 'gone', 'x');"
            ))
            .unwrap();
        migrate_beets(&e.cfg, &e.lib, &db, None, None, false).unwrap();
        let its = items(&e.lib);
        assert_eq!(its.len(), 2);
        let one = its.iter().find(|i| i.id == Some(7)).unwrap();
        assert_eq!(one.path(), music.join("Al/1.mp3"));
        assert_eq!(one.int("album_id"), 4);
        assert_eq!(one.list("genres"), ["Rock", "Pop"]);
        assert_eq!(one.str("lyrics"), "la");
        assert_eq!(one.str("lyrics_backend"), "genius");
        assert_eq!(one.str("lyrics_url"), "https://genius.com/x");
        assert_eq!(one.str("mood"), "calm");
        assert!(!one.flex.contains_key("artpath"));
        let two = its.iter().find(|i| i.id == Some(9)).unwrap();
        assert_eq!(two.path(), Path::new("/elsewhere/2.mp3"));
        let al = e.lib.get_album(4).unwrap().unwrap();
        assert_eq!(al.list("genres"), ["Rock", "Pop"]);
        assert_eq!(al.path_field("artpath"), music.join("Al/cover.jpg"));
        let raw: Vec<u8> = Connection::open(e.cfg.library_path())
            .unwrap()
            .query_row("SELECT path FROM items WHERE id = 7", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, b"Al/1.mp3");
    }

    #[test]
    fn refuses_a_library_beets_may_be_writing() {
        let e = env();
        let db = e.d.path().join("beets.db");
        crate::library::make_beets_fixture(&db);
        let journal = e.d.path().join("beets.db-journal");
        std::fs::write(&journal, b"x").unwrap();
        let err =
            migrate_beets(&e.cfg, &e.lib, &db, Some(Path::new("/m")), None, false).unwrap_err();
        assert!(format!("{err:#}").contains("beets may be using"), "{err:#}");
        assert_eq!(std::fs::read(&journal).unwrap(), b"x");
        let err =
            migrate_beets(&e.cfg, &e.lib, &e.cfg.library_path(), None, None, false).unwrap_err();
        assert!(format!("{err:#}").contains("itself"), "{err:#}");
    }

    #[test]
    fn uri_escaping() {
        assert_eq!(uri_escape(b"/a b/x?y#z%.db"), "/a%20b/x%3Fy%23z%25.db");
        assert_eq!(uri_escape("/é".as_bytes()), "/%C3%A9");
    }

    #[test]
    fn beets_directory_from_its_config() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("config.yaml");
        std::fs::write(&c, "directory: music\nlibrary: x.db\n").unwrap();
        assert_eq!(beets_directory(&c).unwrap(), d.path().join("music"));
        std::fs::write(&c, "directory: /m/../music/\n").unwrap();
        assert_eq!(beets_directory(&c).unwrap(), Path::new("/music"));
        std::fs::write(&c, "plugins: []\n").unwrap();
        assert_eq!(
            beets_directory(&c).unwrap(),
            dirs::home_dir().unwrap().join("Music")
        );
    }
}
