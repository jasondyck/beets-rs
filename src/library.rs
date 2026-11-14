//! SQLite-backed library of items and albums.

use crate::model::{Album, FieldType, Item, Kind, Record, Value, schema};
use crate::query::Query;
use anyhow::{Context, Result, bail};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params};
use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A table's `SELECT *` column mapping: the `id` column, then each schema
/// field's column.
type Columns = Rc<(usize, Vec<usize>)>;

pub struct Library {
    conn: Connection,
    /// Values computed from the library's contents (beets' `_memotable`),
    /// dropped whenever this process writes to the library.
    memo: RefCell<HashMap<String, Rc<dyn Any>>>,
    /// The library directory (normalized): paths inside it are stored
    /// relative to it, as beets does. None: stored as they are.
    music_dir: Option<PathBuf>,
    /// Each table's `SELECT *` column mapping, by kind (items, albums):
    /// every load of a kind has the same columns.
    cols: RefCell<[Option<Columns>; 2]>,
}

fn table(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "items",
        Kind::Album => "albums",
    }
}

/// The INSERT and UPDATE statements for a kind: the same for every record
/// of that kind, so they're built once (their statements stay in the
/// connection's statement cache).
fn add_sql(kind: Kind) -> &'static str {
    static ITEM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static ALBUM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let cell = match kind {
        Kind::Item => &ITEM,
        Kind::Album => &ALBUM,
    };
    cell.get_or_init(|| {
        let cols: Vec<&str> = schema(kind).iter().map(|(n, _)| *n).collect();
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table(kind),
            cols.join(","),
            vec!["?"; cols.len()].join(",")
        )
    })
}

fn store_sql(kind: Kind) -> &'static str {
    static ITEM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static ALBUM: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let cell = match kind {
        Kind::Item => &ITEM,
        Kind::Album => &ALBUM,
    };
    cell.get_or_init(|| {
        let sets: Vec<String> = schema(kind).iter().map(|(n, _)| format!("{n}=?")).collect();
        format!("UPDATE {} SET {} WHERE id=?", table(kind), sets.join(","))
    })
}

fn other(kind: Kind) -> Kind {
    match kind {
        Kind::Item => Kind::Album,
        Kind::Album => Kind::Item,
    }
}

/// A record with what beets' models reach through it (see `query::Obj`).
fn obj_of<'a>(
    kind: Kind,
    r: &'a Record,
    albums: &'a HashMap<i64, Album>,
    dirs: &'a HashMap<i64, PathBuf>,
    counts: &HashMap<i64, crate::query::AlbumCounts>,
    lib: &'a Library,
) -> crate::query::Obj<'a> {
    let album_id = match kind {
        Kind::Item => r.int("album_id"),
        Kind::Album => r.id.unwrap_or(0),
    };
    crate::query::Obj {
        lib: Some(lib),
        album_counts: counts.get(&album_id).copied(),
        rec: r,
        album: match kind {
            Kind::Item => albums.get(&r.int("album_id")),
            Kind::Album => None,
        },
        album_dir: match kind {
            Kind::Album => r.id.and_then(|id| dirs.get(&id)).map(PathBuf::as_path),
            Kind::Item => None,
        },
    }
}

fn attr_table(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "item_attributes",
        Kind::Album => "album_attributes",
    }
}

/// Python's `os.path.normpath` for an absolute path: `.` and `..`
/// resolved lexically (bytes kept as they are).
pub fn lexical_normpath(p: &Path) -> PathBuf {
    let mut out: Vec<&std::ffi::OsStr> = vec![];
    for c in p.components() {
        use std::path::Component as C;
        match c {
            C::RootDir | C::Prefix(_) | C::CurDir => {}
            C::ParentDir => {
                out.pop();
            }
            C::Normal(s) => out.push(s),
        }
    }
    let mut r = PathBuf::from("/");
    r.extend(out);
    r
}

/// beets' `normalize_path_for_db`: an absolute path inside the library
/// directory as `os.path.relpath` gives it; any other path unchanged.
pub fn to_db_path(path: &[u8], music_dir: Option<&Path>) -> Vec<u8> {
    let Some(music) = music_dir else {
        return path.to_vec();
    };
    if !path.starts_with(b"/") {
        return path.to_vec();
    }
    let m = music.as_os_str().as_bytes();
    let mut prefix = m.to_vec();
    if !prefix.ends_with(b"/") {
        prefix.push(b'/');
    }
    if path != m && !path.starts_with(&prefix) {
        return path.to_vec();
    }
    // `os.path.relpath`: both sides normalized, `..` for what's left of
    // the start after the common part.
    let p = lexical_normpath(Path::new(std::ffi::OsStr::from_bytes(path)));
    let s = lexical_normpath(music);
    let p: Vec<_> = p.components().skip(1).collect();
    let s: Vec<_> = s.components().skip(1).collect();
    let common = p.iter().zip(&s).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&[u8]> = vec![b".."; s.len() - common];
    parts.extend(p[common..].iter().map(|c| c.as_os_str().as_bytes()));
    if parts.is_empty() {
        b".".to_vec()
    } else {
        parts.join(&b'/')
    }
}

/// beets' `expand_path_from_db`: a stored relative path is under the
/// library directory.
pub fn from_db_path(path: Vec<u8>, music_dir: Option<&Path>) -> PathBuf {
    match music_dir {
        Some(m) if !path.is_empty() && !path.starts_with(b"/") => {
            lexical_normpath(&m.join(std::ffi::OsStr::from_bytes(&path)))
        }
        _ => PathBuf::from(std::ffi::OsString::from_vec(path)),
    }
}

impl Library {
    /// A field's value as beets stores it: paths relative to the library
    /// directory (no path, an album without art, is NULL), a singleton's
    /// `album_id` NULL.
    fn to_sql(&self, name: &str, v: &Value) -> SqlValue {
        match v {
            Value::Null => SqlValue::Null,
            Value::Str(s) => SqlValue::Text(s.clone()),
            Value::Int(0) if name == "album_id" => SqlValue::Null,
            Value::Int(i) => SqlValue::Integer(*i),
            Value::Float(f) => SqlValue::Real(*f),
            Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
            Value::Path(p) if p.as_os_str().is_empty() => SqlValue::Null,
            Value::Path(p) => SqlValue::Blob(to_db_path(
                p.as_os_str().as_bytes(),
                self.music_dir.as_deref(),
            )),
        }
    }
}

// beets stores/reads these as Python numbers; the casts mirror that.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn from_sql(v: SqlValue, ty: FieldType, music_dir: Option<&Path>) -> Value {
    match (ty, v) {
        (_, SqlValue::Null) => ty.default_value(),
        (FieldType::Path, SqlValue::Blob(b)) => Value::Path(from_db_path(b, music_dir)),
        (FieldType::Path, SqlValue::Text(s)) => {
            Value::Path(from_db_path(s.into_bytes(), music_dir))
        }
        (FieldType::Str, SqlValue::Text(s)) => Value::Str(s),
        (FieldType::Bool, SqlValue::Integer(i)) => Value::Bool(i != 0),
        (
            FieldType::Float(_) | FieldType::NullFloat | FieldType::Duration | FieldType::Date,
            SqlValue::Real(f),
        ) => Value::Float(f),
        (
            FieldType::Float(_) | FieldType::NullFloat | FieldType::Duration | FieldType::Date,
            SqlValue::Integer(i),
        ) => Value::Float(i as f64),
        (FieldType::NullFloat, SqlValue::Text(s)) => {
            s.trim().parse().map_or(Value::Null, Value::Float)
        }
        (_, SqlValue::Integer(i)) => Value::Int(i),
        (_, SqlValue::Real(f)) => Value::Int(f as i64),
        (_, SqlValue::Text(s)) => Value::Str(s),
        (t, _) => t.default_value(),
    }
}

/// beets' schema statements, as SQLite keeps them in `sqlite_master`
/// (beets' `Database._ensure_migration_state_table`, `_make_table`,
/// `_make_attribute_table` and `Item._indices`).
const MIGRATIONS_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS migrations (
                    name TEXT NOT NULL,
                    table_name TEXT NOT NULL,
                    PRIMARY KEY(name, table_name)
                );";

fn attribute_table_sql(a: &str) -> String {
    format!(
        "
                CREATE TABLE IF NOT EXISTS {a} (
                    id INTEGER PRIMARY KEY,
                    entity_id INTEGER,
                    key TEXT,
                    value TEXT,
                    UNIQUE(entity_id, key) ON CONFLICT REPLACE);
                CREATE INDEX IF NOT EXISTS {a}_by_entity
                    ON {a} (entity_id);
                "
    )
}

/// `CREATE TABLE` for a model: its fields (and `id`) sorted by name.
fn model_table_sql(kind: Kind) -> String {
    let mut cols: Vec<(&str, &str)> = schema(kind)
        .iter()
        .map(|(n, t)| (*n, t.sql_type()))
        .chain(std::iter::once(("id", "INTEGER PRIMARY KEY")))
        .collect();
    cols.sort_unstable();
    let cols: Vec<String> = cols.iter().map(|(n, t)| format!("{n} {t}")).collect();
    format!("CREATE TABLE {} ({});\n", table(kind), cols.join(", "))
}

fn columns_of(conn: &Connection, t: &str) -> Result<Vec<(String, String)>> {
    Ok(conn
        .prepare(&format!("PRAGMA table_info({t})"))?
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        .collect::<Result<_, _>>()?)
}

/// beets' `_make_table`, `_make_attribute_table` and `_create_indices`:
/// a missing table is created with beets' statement, an existing one
/// gets the columns it lacks.
fn make_tables(conn: &Connection) -> Result<()> {
    for kind in [Kind::Item, Kind::Album] {
        let t = table(kind);
        let existing = columns_of(conn, t)?;
        if existing.is_empty() {
            conn.execute_batch(&model_table_sql(kind))?;
        } else {
            for (name, ty) in schema(kind) {
                if !existing.iter().any(|(e, _)| e == name) {
                    conn.execute_batch(&format!(
                        "ALTER TABLE {t} ADD COLUMN {name} {};\n",
                        ty.sql_type()
                    ))?;
                }
            }
        }
        conn.execute_batch(&attribute_table_sql(attr_table(kind)))?;
        if kind == Kind::Item {
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_item_album_id ON items (album_id);",
            )?;
        }
    }
    Ok(())
}

/// Whether the database has beets-rs' layout from before it matched
/// beets' (flexible attribute tables without an `id`, a `data_source`
/// column, absolute paths, singletons with `album_id` 0).
fn has_old_layout(conn: &Connection) -> Result<bool> {
    let cols = columns_of(conn, "item_attributes")?;
    Ok(!cols.is_empty() && !cols.iter().any(|(c, _)| c == "id"))
}

/// Rebuild an old beets-rs database in beets' layout: tables recreated
/// with beets' statements (columns beets doesn't have, such as a legacy
/// `genre`, kept as beets keeps them), singletons' `album_id` NULL,
/// `data_source` moved to the flexible attributes. Paths are made
/// relative afterwards by beets' `relative_path` migration.
fn rebuild_old_layout(conn: &Connection, backup: Option<&Path>) -> Result<()> {
    if let Some(db) = backup {
        backup_db(conn, db, "-before-beets_layout.bak")?;
    }
    println!("Migrating the library to beets' database layout...");
    let tx = conn.unchecked_transaction()?;
    let migrations: Vec<(String, String)> = if columns_of(&tx, "migrations")?.is_empty() {
        vec![]
    } else {
        let rows = tx
            .prepare("SELECT name, table_name FROM migrations")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        tx.execute_batch("DROP TABLE migrations")?;
        rows
    };
    for kind in [Kind::Item, Kind::Album] {
        let (t, a) = (table(kind), attr_table(kind));
        tx.execute_batch(&format!(
            "ALTER TABLE {t} RENAME TO old_{t}; ALTER TABLE {a} RENAME TO old_{a};"
        ))?;
    }
    tx.execute_batch(MIGRATIONS_TABLE_SQL)?;
    for (name, t) in &migrations {
        tx.execute(
            "INSERT INTO migrations(name, table_name) VALUES (?, ?)",
            [name, t],
        )?;
    }
    make_tables(&tx)?;
    for kind in [Kind::Item, Kind::Album] {
        let (t, a) = (table(kind), attr_table(kind));
        let new: Vec<String> = columns_of(&tx, t)?.into_iter().map(|(c, _)| c).collect();
        let mut cols = vec![];
        for (c, ty) in columns_of(&tx, &format!("old_{t}"))? {
            if c == "data_source" {
                continue;
            }
            if !new.contains(&c) {
                tx.execute_batch(&format!("ALTER TABLE {t} ADD COLUMN {c} {ty};\n"))?;
            }
            cols.push(c);
        }
        let cols = cols.join(", ");
        tx.execute_batch(&format!(
            "INSERT INTO {t} ({cols}) SELECT {cols} FROM old_{t} ORDER BY id;
             INSERT INTO {a} (entity_id, key, value)
                 SELECT entity_id, key, value FROM old_{a} ORDER BY entity_id, key;"
        ))?;
        if columns_of(&tx, &format!("old_{t}"))?
            .iter()
            .any(|(c, _)| c == "data_source")
        {
            tx.execute_batch(&format!(
                "INSERT INTO {a} (entity_id, key, value)
                     SELECT id, 'data_source', data_source FROM old_{t}
                     WHERE data_source IS NOT NULL AND data_source != '' ORDER BY id;"
            ))?;
        }
        tx.execute_batch(&format!("DROP TABLE old_{a}; DROP TABLE old_{t};"))?;
    }
    tx.execute_batch("UPDATE items SET album_id = NULL WHERE album_id = 0;")?;
    tx.commit()?;
    println!("Migration complete");
    Ok(())
}

/// beets' `Library._migrations`, in order, with the tables each runs on.
const MIGRATIONS: &[(&str, &[Kind])] = &[
    ("multi_genre_field", &[Kind::Item, Kind::Album]),
    ("lyrics_metadata_in_flex_fields", &[Kind::Item]),
    ("multi_remixer_field", &[Kind::Item]),
    ("multi_lyricist_field", &[Kind::Item]),
    ("multi_composer_field", &[Kind::Item]),
    ("multi_arranger_field", &[Kind::Item]),
    ("relative_path", &[Kind::Item, Kind::Album]),
    ("remove_inherited_artpath", &[Kind::Item]),
    ("instrumental_lyrics_in_flex_field", &[Kind::Item]),
];

/// beets' `MultiValueFieldMigration`s: the old field each list replaces.
fn legacy_field_of(migration: &str) -> Option<(&'static str, &'static str)> {
    Some(match migration {
        "multi_genre_field" => ("genre", "genres"),
        "multi_remixer_field" => ("remixer", "remixers"),
        "multi_lyricist_field" => ("lyricist", "lyricists"),
        "multi_composer_field" => ("composer", "composers"),
        "multi_arranger_field" => ("arranger", "arrangers"),
        _ => return None,
    })
}

/// Runs beets' data migrations not yet recorded in the `migrations` table,
/// each once per table. As beets', each backs the database up first.
pub(crate) struct Migrator<'a> {
    conn: &'a Connection,
    /// The database file to back up (None: no backups).
    backup: Option<&'a Path>,
    music_dir: Option<&'a Path>,
}

impl Migrator<'_> {
    pub(crate) fn run(&self) -> Result<()> {
        for (name, kinds) in MIGRATIONS {
            for kind in *kinds {
                let t = table(*kind);
                let done: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM migrations WHERE name = ? AND table_name = ?)",
                    [name, &t],
                    |r| r.get(0),
                )?;
                if done {
                    continue;
                }
                let columns: Vec<String> = columns_of(self.conn, t)?
                    .into_iter()
                    .map(|(c, _)| c)
                    .collect();
                // As beets' `Migration.migrate_model`: a backup before any
                // migration not yet recorded, whether or not it has data
                // to change (so a new database gets one per migration).
                self.backup(t, name)?;
                match *name {
                    "lyrics_metadata_in_flex_fields" => self.lyrics_metadata(*kind)?,
                    "relative_path" => {
                        for field in ["path", "artpath"] {
                            if columns.iter().any(|c| c == field) {
                                self.relative_path(*kind, field)?;
                            }
                        }
                    }
                    "remove_inherited_artpath" => self.remove_inherited_artpath(*kind)?,
                    "instrumental_lyrics_in_flex_field" => self.instrumental_lyrics(*kind)?,
                    _ => {
                        let (old, new) = legacy_field_of(name).expect(
                            "BUG: every MIGRATIONS entry is either handled above or has a legacy field",
                        );
                        if columns.iter().any(|c| c == old) {
                            self.multi_value(*kind, old, new)?;
                        }
                    }
                }
                self.conn.execute(
                    "INSERT INTO migrations(name, table_name) VALUES (?, ?)",
                    [name, &t],
                )?;
            }
        }
        Ok(())
    }

    fn backup(&self, t: &str, name: &str) -> Result<()> {
        match self.backup {
            Some(db) => backup_db(self.conn, db, &format!("-before-{t}-{name}.bak")),
            None => Ok(()),
        }
    }

    /// `MultiValueFieldMigration`: backfill a list field from the old
    /// single-valued column. Values with a "; ", ", " or " / " are split.
    fn multi_value(&self, kind: Kind, old: &str, new: &str) -> Result<()> {
        let t = table(kind);
        let rows: Vec<(i64, String, Option<String>)> = self
            .conn
            .prepare(&format!(
                "SELECT id, {old}, {new} FROM {t} WHERE {old} IS NOT NULL AND {old} != ''"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        let total = rows.len();
        let todo: Vec<&(i64, String, Option<String>)> = rows
            .iter()
            .filter(|(_, _, l)| l.as_deref().is_none_or(str::is_empty))
            .collect();
        if todo.is_empty() {
            return Ok(());
        }
        let mut migrated = total - todo.len();
        println!("Migrating {new} for {total} {t}...");
        for chunk in todo.chunks(1000) {
            let tx = self.conn.unchecked_transaction()?;
            for (id, value, _) in chunk {
                tx.execute(
                    &format!("UPDATE {t} SET {new} = ? WHERE id = ?"),
                    rusqlite::params![legacy_to_list_value(value), id],
                )?;
            }
            tx.commit()?;
            migrated += chunk.len();
            println!("  Migrated {migrated} {t} ({migrated}/{total} processed)...");
        }
        println!("Migration complete: {migrated} of {total} {t} updated");
        Ok(())
    }

    /// `LyricsMetadataInFlexFieldsMigration`: lyrics stored with an inline
    /// "Source: URL" footer keep the text; the URL and its backend move to
    /// `lyrics_url` and `lyrics_backend`, and the language guess (langdetect
    /// with seed 0, as beets) to `lyrics_language`/`lyrics_translation_language`.
    fn lyrics_metadata(&self, kind: Kind) -> Result<()> {
        let (t, a) = (table(kind), attr_table(kind));
        let done: std::collections::HashSet<i64> = self
            .conn
            .prepare(&format!(
                "SELECT entity_id FROM {a} WHERE key == 'lyrics_backend'"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let rows: Vec<(i64, String)> = self
            .conn
            .prepare(&format!(
                "SELECT id, lyrics FROM {t} WHERE lyrics IS NOT NULL AND lyrics != ''"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let total = rows.len();
        let todo: Vec<&(i64, String)> = rows.iter().filter(|(id, _)| !done.contains(id)).collect();
        if todo.is_empty() {
            return Ok(());
        }
        let mut migrated = total - todo.len();
        println!("Migrating lyrics for {total} {t}...");
        for chunk in todo.chunks(100) {
            let tx = self.conn.unchecked_transaction()?;
            for (id, lyrics) in chunk {
                let (text, source) = legacy_lyrics(lyrics);
                if text != *lyrics {
                    tx.execute(
                        &format!("UPDATE {t} SET lyrics = ? WHERE id = ?"),
                        rusqlite::params![text, id],
                    )?;
                }
                // beets' `lyr_fields`: backend, URL and the languages
                // langdetect finds (each only when it has a value).
                let (backend, url) = match source {
                    Some((backend, url)) => (Some(backend), Some(url)),
                    None => (None, None),
                };
                let (language, translation_language) = legacy_language(&text);
                for (key, value) in [
                    ("lyrics_backend", backend),
                    ("lyrics_url", url),
                    ("lyrics_language", language),
                    ("lyrics_translation_language", translation_language),
                ] {
                    let Some(value) = value else { continue };
                    tx.execute(
                        &format!("INSERT INTO {a} (entity_id, key, value) VALUES (?, ?, ?)"),
                        rusqlite::params![id, key, value],
                    )?;
                }
            }
            tx.commit()?;
            migrated += chunk.len();
            println!("  Migrated {migrated} {t} ({migrated}/{total} processed)...");
        }
        println!("Migration complete: {migrated} of {total} {t} updated");
        Ok(())
    }

    /// `RelativePathMigration`: absolute paths inside the library
    /// directory become relative to it (every absolute path is rewritten
    /// as a blob).
    fn relative_path(&self, kind: Kind, field: &str) -> Result<()> {
        let t = table(kind);
        let rows: Vec<(i64, SqlValue)> = self
            .conn
            .prepare(&format!(
                "SELECT id, {field} FROM {t} WHERE {field} IS NOT NULL"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let total = rows.len();
        let todo: Vec<(i64, Vec<u8>)> = rows
            .into_iter()
            .filter_map(|(id, v)| match v {
                SqlValue::Blob(b) => Some((id, b)),
                SqlValue::Text(s) => Some((id, s.into_bytes())),
                _ => None,
            })
            .filter(|(_, p)| p.starts_with(b"/"))
            .collect();
        if todo.is_empty() {
            return Ok(());
        }
        println!("Migrating {field} for {total} {t}...");
        let tx = self.conn.unchecked_transaction()?;
        for (id, p) in &todo {
            tx.execute(
                &format!("UPDATE {t} SET {field} = ? WHERE id = ?"),
                rusqlite::params![to_db_path(p, self.music_dir), id],
            )?;
        }
        tx.commit()?;
        println!("Migration complete: {} of {total} {t} updated", todo.len());
        Ok(())
    }

    /// `RemoveInheritedArtpathMigration`.
    fn remove_inherited_artpath(&self, kind: Kind) -> Result<()> {
        let (t, a) = (table(kind), attr_table(kind));
        let total: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {a} WHERE key == 'artpath'"),
            [],
            |r| r.get(0),
        )?;
        if total == 0 {
            return Ok(());
        }
        println!("Removing inherited artpath from {total} {t}...");
        self.conn
            .execute(&format!("DELETE FROM {a} WHERE key == 'artpath'"), [])?;
        println!("Migration complete: {total} {t} updated");
        Ok(())
    }

    /// `InstrumentalLyricsInFlexFieldMigration`: the "[Instrumental]"
    /// marker becomes `lyrics_instrumental`.
    fn instrumental_lyrics(&self, kind: Kind) -> Result<()> {
        let (t, a) = (table(kind), attr_table(kind));
        let ids: Vec<i64> = self
            .conn
            .prepare(&format!("SELECT id FROM {t} WHERE lyrics == ?"))?
            .query_map([INSTRUMENTAL_LYRICS], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        if ids.is_empty() {
            return Ok(());
        }
        let total = ids.len();
        println!("Migrating instrumental lyrics for {total} {t}...");
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            &format!("UPDATE {t} SET lyrics = '' WHERE lyrics == ?"),
            [INSTRUMENTAL_LYRICS],
        )?;
        for id in ids {
            tx.execute(
                &format!(
                    "INSERT INTO {a} (entity_id, key, value) VALUES (?, 'lyrics_instrumental', '1')"
                ),
                [id],
            )?;
        }
        tx.commit()?;
        println!("Migration complete: {total} of {total} {t} updated");
        Ok(())
    }
}

/// beets' `INSTRUMENTAL_LYRICS`.
const INSTRUMENTAL_LYRICS: &str = "[Instrumental]";

/// beets' `Lyrics.from_legacy_text(...).full_text` and its source: the
/// text before a "\n\nSource: URL" footer, and (backend, URL) when there
/// is a footer.
fn legacy_lyrics(text: &str) -> (String, Option<(String, String)>) {
    let mut parts = text.split("\n\nSource: ");
    let body = parts.next().unwrap_or_default();
    let source = parts.next().map(|url| {
        let url = url.trim().to_string();
        let root = url_netloc(&url);
        let root = root.strip_prefix("www.").unwrap_or(root);
        let root = root.split('.').next().unwrap_or_default();
        let backend = if ["genius", "musixmatch", "lrclib", "tekstowo"].contains(&root) {
            root
        } else {
            "google"
        };
        (backend.to_string(), url)
    });
    // beets also blanks the instrumental marker here, before its own
    // `instrumental_lyrics_in_flex_field` migration could record it; it's
    // kept for that migration instead.
    (body.to_string(), source)
}

/// The language guesses for a legacy lyrics body, as beets'
/// `Lyrics.from_legacy_text(...).language`/`translation_language` (the
/// instrumental marker is left for the instrumental migration and gets
/// none).
fn legacy_language(text: &str) -> (Option<String>, Option<String>) {
    if text == INSTRUMENTAL_LYRICS {
        return (None, None);
    }
    let lyrics = crate::plugins::lyrics::text::Lyrics::new(text, None, None);
    (lyrics.language, lyrics.translation_language)
}

/// `urllib.parse.urlparse(url).netloc`.
fn url_netloc(url: &str) -> &str {
    let rest = match url.split_once(':') {
        Some((scheme, rest))
            if scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            rest
        }
        _ => url,
    };
    match rest.strip_prefix("//") {
        Some(r) => r.split(['/', '?', '#']).next().unwrap_or_default(),
        None => "",
    }
}

/// beets' `Migration._before_migration_backup`: a copy of the database at
/// `<library><suffix>` (replacing an older one), made with SQLite so it's
/// consistent, and beets' message.
pub(crate) fn backup_db(conn: &Connection, db: &Path, suffix: &str) -> Result<()> {
    let mut dest = db.as_os_str().to_owned();
    dest.push(suffix);
    let dest = PathBuf::from(dest);
    // `VACUUM INTO` won't overwrite: write beside it, then rename over.
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    conn.execute("VACUUM INTO ?", [tmp.to_string_lossy()])
        .with_context(|| format!("backing up the library to {}", dest.display()))?;
    std::fs::rename(&tmp, &dest)?;
    println!(
        "Created database backup at: {}.",
        crate::template::py_repr(&dest.to_string_lossy())
    );
    Ok(())
}

/// `MultiValueFieldMigration.convert_to_list_value`: the first of the
/// known separators the value holds becomes the list delimiter.
fn legacy_to_list_value(value: &str) -> String {
    for sep in ["; ", ", ", " / "] {
        if value.contains(sep) {
            return value.replace(sep, crate::model::MULTI);
        }
    }
    value.to_string()
}

impl Library {
    /// beets' `Database.load_extension`: load the SQLite extension at
    /// `path` into the connection. SQLite adds the platform's library
    /// suffix when the path has none or does not exist as given.
    ///
    /// Extension loading is enabled only for the load (rusqlite's
    /// [`rusqlite::LoadExtensionGuard`]), so SQL's `load_extension()`
    /// cannot run later queries as code.
    pub fn load_extension(&self, path: &Path) -> Result<()> {
        // SAFETY: extension loading is on only while loading the
        // configured extension; no SQL runs in between (beets keeps it
        // enabled for the connection's life, which also lets any later
        // query call `load_extension()`).
        unsafe {
            let _guard = rusqlite::LoadExtensionGuard::new(&self.conn)?;
            self.conn.load_extension(path, None::<&str>)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, None, true)
    }

    /// Open (creating or migrating) the library database at `path`, with
    /// paths stored relative to `music_dir`; `backup` is
    /// `create_backup_before_migrations`.
    pub fn open_with(path: &Path, music_dir: Option<&Path>, backup: bool) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening library {}", path.display()))?;
        Self::init(conn, music_dir, backup.then_some(path))
    }

    /// The library directory (beets' `lib.directory`), if set.
    pub fn music_dir(&self) -> Option<&Path> {
        self.music_dir.as_deref()
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None, None)
    }

    /// An in-memory library storing paths relative to `music_dir`.
    #[cfg(test)]
    pub fn in_memory_at(music_dir: &Path) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, Some(music_dir), None)
    }

    /// `backup`: the database file, to back up before each migration not
    /// yet recorded (None: no backups).
    fn init(conn: Connection, music_dir: Option<&Path>, backup: Option<&Path>) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        let music_dir = music_dir.map(crate::query::normpath);
        if has_old_layout(&conn)? {
            rebuild_old_layout(&conn, backup)?;
        }
        conn.execute_batch(MIGRATIONS_TABLE_SQL)?;
        make_tables(&conn)?;
        Migrator {
            conn: &conn,
            backup,
            music_dir: music_dir.as_deref(),
        }
        .run()?;
        Ok(Library {
            conn,
            memo: RefCell::new(HashMap::new()),
            music_dir,
            cols: RefCell::new([None, None]),
        })
    }

    /// A memoized value computed from the library's contents: `f`'s
    /// result for `key`, until the library is next written to.
    pub fn memo<T: 'static>(&self, key: String, f: impl FnOnce() -> T) -> Rc<T> {
        if let Some(v) = self.memo.borrow().get(&key)
            && let Ok(v) = v.clone().downcast::<T>()
        {
            return v;
        }
        let v = Rc::new(f());
        self.memo.borrow_mut().insert(key, v.clone());
        v
    }

    /// Drop memoized values (after any write).
    fn invalidate(&self) {
        self.memo.borrow_mut().clear();
    }

    /// beets' `SELECT DISTINCT key` over a model's flexible attributes
    /// (`fields`).
    pub fn flex_keys(&self, kind: Kind) -> Result<Vec<String>> {
        Ok(self
            .conn
            .prepare(&format!("SELECT DISTINCT key FROM ({})", attr_table(kind)))?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    pub fn transaction<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN")?;
        match f(self) {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                self.conn.execute_batch("ROLLBACK").ok();
                self.invalidate();
                Err(e)
            }
        }
    }

    /// Insert a new record, assigning its id.
    /// Insert a new record. As beets' `Model.add`, this stamps `added` with
    /// the current time (a reimport restores the old value afterwards).
    pub fn add(&self, rec: &mut Record) -> Result<i64> {
        self.invalidate();
        rec.set("added", Value::Float(now()));
        let vals: Vec<SqlValue> = rec.fixed_values().map(|(n, v)| self.to_sql(n, v)).collect();
        self.conn
            .prepare_cached(add_sql(rec.kind))?
            .execute(rusqlite::params_from_iter(vals))?;
        let id = self.conn.last_insert_rowid();
        rec.id = Some(id);
        self.store_flex(rec)?;
        self.changed(rec);
        Ok(id)
    }

    /// Write all fields of an existing record.
    pub fn store(&self, rec: &Record) -> Result<()> {
        self.invalidate();
        let id = rec.id.context("record has no id; add it first")?;
        let mut vals: Vec<SqlValue> = rec.fixed_values().map(|(n, v)| self.to_sql(n, v)).collect();
        vals.push(SqlValue::Integer(id));
        self.conn
            .prepare_cached(store_sql(rec.kind))?
            .execute(rusqlite::params_from_iter(vals))?;
        self.store_flex(rec)?;
        self.changed(rec);
        Ok(())
    }

    /// beets' `database_change` event, after a record is written.
    fn changed(&self, rec: &Record) {
        if crate::events::listening("database_change") {
            crate::events::send(
                "database_change",
                &[
                    ("lib", crate::events::Arg::Lib(self)),
                    ("model", crate::events::Arg::Model(rec)),
                ],
            );
        }
    }

    fn store_flex(&self, rec: &Record) -> Result<()> {
        let a = attr_table(rec.kind);
        let id = rec
            .id
            .expect("BUG: store_flex runs only for records added or loaded from the database");
        self.conn
            .prepare_cached(&format!("DELETE FROM {a} WHERE entity_id=?"))?
            .execute([id])?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "INSERT INTO {a} (entity_id, key, value) VALUES (?,?,?)"
        ))?;
        for (k, v) in &rec.flex {
            stmt.execute(params![id, k, v])?;
        }
        Ok(())
    }

    pub fn remove(&self, rec: &Record) -> Result<()> {
        self.invalidate();
        if let Some(id) = rec.id {
            if rec.kind == Kind::Album {
                self.conn
                    .execute("UPDATE items SET album_id=NULL WHERE album_id=?", [id])?;
            }
            // beets' `Model.remove`: the row and its flexible attributes.
            self.conn
                .execute(&format!("DELETE FROM {} WHERE id=?", table(rec.kind)), [id])?;
            self.conn.execute(
                &format!("DELETE FROM {} WHERE entity_id=?", attr_table(rec.kind)),
                [id],
            )?;
        }
        self.changed(rec);
        Ok(())
    }

    /// The column of each schema field (and `id`) in a statement's result.
    /// Every `SELECT *` on a table has the same columns, so the mapping is
    /// made once (finding 97 columns by name on every load was noticeable).
    fn columns(&self, kind: Kind, stmt: &rusqlite::Statement) -> rusqlite::Result<Columns> {
        let index = match kind {
            Kind::Item => 0,
            Kind::Album => 1,
        };
        if let Some(cols) = &self.cols.borrow()[index] {
            return Ok(cols.clone());
        }
        let fields = schema(kind)
            .iter()
            .map(|(n, _)| stmt.column_index(n))
            .collect::<rusqlite::Result<_>>()?;
        let cols = Rc::new((stmt.column_index("id")?, fields));
        self.cols.borrow_mut()[index] = Some(cols.clone());
        Ok(cols)
    }

    fn row_to_record(
        &self,
        kind: Kind,
        row: &Row,
        cols: &(usize, Vec<usize>),
    ) -> rusqlite::Result<Record> {
        let mut values = Vec::with_capacity(cols.1.len());
        for ((_, ty), &i) in schema(kind).iter().zip(&cols.1) {
            values.push(from_sql(
                row.get::<_, SqlValue>(i)?,
                *ty,
                self.music_dir.as_deref(),
            ));
        }
        Ok(Record::from_values(kind, row.get(cols.0)?, values))
    }

    fn load(&self, kind: Kind, where_sql: &str, args: &[SqlValue]) -> Result<Vec<Record>> {
        let t = table(kind);
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT * FROM {t} {where_sql}"))?;
        let cols = self.columns(kind, &stmt)?;
        let mut recs: Vec<Record> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                self.row_to_record(kind, r, &cols)
            })?
            .collect::<Result<_, _>>()?;
        if recs.is_empty() {
            return Ok(recs);
        }
        // Attach flexible attributes.
        let mut flex: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
        let a = attr_table(kind);
        let mut stmt = if recs.len() == 1 {
            let mut s = self.conn.prepare_cached(&format!(
                "SELECT entity_id, key, value FROM {a} WHERE entity_id=?"
            ))?;
            let rows = s.query_map(
                [recs[0]
                    .id
                    .expect("BUG: records loaded from the database have an id")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?)),
            )?;
            for row in rows {
                let (id, k, v): (i64, String, Option<String>) = row?;
                flex.entry(id).or_default().insert(k, v.unwrap_or_default());
            }
            None
        } else if where_sql.is_empty() {
            Some(
                self.conn
                    .prepare_cached(&format!("SELECT entity_id, key, value FROM {a}"))?,
            )
        } else {
            // Only the loaded rows' attributes: reading the whole attribute
            // table here made every small load (an album's items, say) cost
            // as much as the library is big.
            Some(self.conn.prepare_cached(&format!(
                "SELECT entity_id, key, value FROM {a} \
                 WHERE entity_id IN (SELECT id FROM {t} {where_sql})"
            ))?)
        };
        let flex_args: &[SqlValue] = if where_sql.is_empty() { &[] } else { args };
        if let Some(s) = stmt.as_mut() {
            let rows = s.query_map(rusqlite::params_from_iter(flex_args.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?))
            })?;
            for row in rows {
                let (id, k, v): (i64, String, Option<String>) = row?;
                flex.entry(id).or_default().insert(k, v.unwrap_or_default());
            }
        }
        for r in &mut recs {
            if let Some(f) = flex.remove(
                &r.id
                    .expect("BUG: records loaded from the database have an id"),
            ) {
                r.flex = f;
            }
        }
        Ok(recs)
    }

    pub fn items(&self, q: &Query) -> Result<Vec<Item>> {
        self.fetch(Kind::Item, q)
    }

    pub fn albums(&self, q: &Query) -> Result<Vec<Album>> {
        self.fetch(Kind::Album, q)
    }

    /// beets' `Database._get_results`: the query's `WHERE` clause (joining
    /// the related table when it names its fields) and the SQL part of the
    /// sort run in SQLite; slow queries and sorts run here, with each item's
    /// album (and each album's directory) at hand as beets' models have.
    fn fetch(&self, kind: Kind, q: &Query) -> Result<Vec<Record>> {
        let t = table(kind);
        let o = table(other(kind));
        let join = match kind {
            Kind::Item => format!("LEFT JOIN {o} ON {t}.album_id = {o}.id"),
            Kind::Album => format!("LEFT JOIN {o} ON {t}.id = {o}.album_id"),
        };
        let sort = q.effective_sort();
        let clause = q.pred.clause();
        let (where_sql, args) = clause.clone().unwrap_or_else(|| ("1".into(), vec![]));
        let where_join = q.pred.needs_join(kind);
        let sort_join = sort.needs_join(kind);
        let order = match sort.order_clause(kind) {
            Some(o) => format!("{o}, {t}.id"),
            None => format!("{t}.id"),
        };
        // `inner` selects the matching rows; grouping by id undoes the
        // duplicates a join brings (as beets does).
        let inner = if where_join {
            format!("SELECT {t}.* FROM {t} {join} WHERE {where_sql} GROUP BY {t}.id")
        } else {
            format!("SELECT {t}.* FROM {t} WHERE {where_sql}")
        };
        // Sorting albums by an item field joins in every item: keep one row
        // per album (beets would list the album once per item).
        let regroup = if sort_join && kind == Kind::Album {
            format!(" GROUP BY {t}.id")
        } else {
            String::new()
        };
        let sql = match (where_join, sort_join) {
            (false, false) => format!("{inner} ORDER BY {order}"),
            (false, true) => {
                format!("SELECT {t}.* FROM {t} {join} WHERE {where_sql}{regroup} ORDER BY {order}")
            }
            (true, false) => format!("SELECT {t}.* FROM ({inner}) {t} ORDER BY {order}"),
            (true, true) => {
                format!("SELECT {t}.* FROM ({inner}) {t} {join}{regroup} ORDER BY {order}")
            }
        };
        crate::query::register_sql_functions(&self.conn)?;
        let mut stmt = self.conn.prepare(&sql)?;
        let cols = self.columns(kind, &stmt)?;
        let mut recs: Vec<Record> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                self.row_to_record(kind, r, &cols)
            })?
            .collect::<Result<_, _>>()?;
        let a = attr_table(kind);
        let mut flex: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT entity_id, key, value FROM {a} WHERE entity_id IN (SELECT id FROM ({inner}))"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?))
        })?;
        for row in rows {
            let (id, k, v): (i64, String, Option<String>) = row?;
            flex.entry(id).or_default().insert(k, v.unwrap_or_default());
        }
        for r in &mut recs {
            if let Some(f) = flex.remove(
                &r.id
                    .expect("BUG: records loaded from the database have an id"),
            ) {
                r.flex = f;
            }
        }
        if clause.is_some() && !sort.is_slow() {
            return Ok(recs);
        }

        // In-memory matching and sorting need what beets' models reach
        // through: an item's album, an album's directory.
        let albums: HashMap<i64, Album> = match kind {
            Kind::Item => self
                .load(Kind::Album, "", &[])?
                .into_iter()
                .map(|a| {
                    (
                        a.id.expect("BUG: albums loaded from the database have an id"),
                        a,
                    )
                })
                .collect(),
            Kind::Album => HashMap::new(),
        };
        let dirs: HashMap<i64, PathBuf> = match kind {
            Kind::Album => self.album_dirs()?,
            Kind::Item => HashMap::new(),
        };
        // Album getters (`albumtotal`, `$missing`): for album queries, and
        // item queries that reach them through the album.
        let counts = if kind == Kind::Album || q.uses_any(&["albumtotal", "missing"]) {
            self.album_counts()?
        } else {
            HashMap::new()
        };
        if clause.is_none() {
            recs.retain(|r| q.matches_obj(&obj_of(kind, r, &albums, &dirs, &counts, self)));
        }
        if sort.is_slow() {
            let objs: Vec<_> = recs
                .iter()
                .map(|r| obj_of(kind, r, &albums, &dirs, &counts, self))
                .collect();
            let order = sort.sort_slow(&objs);
            drop(objs);
            let mut slots: Vec<Option<Record>> = recs.into_iter().map(Some).collect();
            recs = order
                .into_iter()
                .map(|i| {
                    slots[i]
                        .take()
                        .expect("BUG: the sort order visits each loaded record once")
                })
                .collect();
        }
        Ok(recs)
    }

    /// Each album's `albumtotal` (as `Formatted` computes it: the first
    /// item's `tracktotal`, or with `per_disc_numbering` on a multi-disc
    /// album the sum over its discs) and item count.
    fn album_counts(&self) -> Result<HashMap<i64, crate::query::AlbumCounts>> {
        let per_disc = crate::formatted::env().per_disc_numbering;
        let mut stmt = self.conn.prepare(
            "SELECT items.album_id, items.disc, items.tracktotal, albums.disctotal \
             FROM items JOIN albums ON items.album_id = albums.id \
             ORDER BY items.album_id, items.disc, items.track, items.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            ))
        })?;
        // Per album: (counts, discs counted so far).
        let mut out: HashMap<i64, (crate::query::AlbumCounts, Vec<i64>)> = HashMap::new();
        for row in rows {
            let (id, disc, tracktotal, disctotal) = row?;
            let (c, discs) = out.entry(id).or_default();
            c.items += 1;
            if disctotal == 1 || !per_disc {
                if c.items == 1 {
                    c.albumtotal = tracktotal;
                }
            } else if !discs.contains(&disc) && (discs.len() as i64) < disctotal {
                c.albumtotal += tracktotal;
                discs.push(disc);
            }
        }
        Ok(out.into_iter().map(|(k, (c, _))| (k, c)).collect())
    }

    /// Each album's directory (beets' `Album.item_dir`): that of its first
    /// item in the default item order.
    fn album_dirs(&self) -> Result<HashMap<i64, PathBuf>> {
        let order = Query::parse::<&str>(Kind::Item, &[])?
            .effective_sort()
            .order_clause(Kind::Item)
            .map(|o| format!("{o}, "))
            .unwrap_or_default();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT items.album_id, items.path FROM items LEFT JOIN albums \
             ON items.album_id = albums.id WHERE items.album_id IS NOT NULL \
             ORDER BY {order}items.id"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, SqlValue>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, path) = row?;
            let path = match path {
                SqlValue::Blob(b) => from_db_path(b, self.music_dir.as_deref()),
                SqlValue::Text(s) => from_db_path(s.into_bytes(), self.music_dir.as_deref()),
                _ => continue,
            };
            out.entry(id)
                .or_insert_with(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
        }
        Ok(out)
    }

    #[allow(dead_code)] // tests, and handy for commands
    pub fn all_albums(&self) -> Result<Vec<Album>> {
        self.load(Kind::Album, "", &[])
    }

    /// Candidates for beets' duplicate query: the albums or items whose
    /// text fields (fixed or flexible) among `want` hold exactly the wanted
    /// values, narrowed in SQL. This is a superset filter only — keys that
    /// aren't plain text (numbers, dates, …) or are wanted as missing are
    /// left for the caller to check with `duplicates::matches`. With
    /// `singletons_only`, items that belong to an album are skipped.
    pub fn duplicate_candidates(
        &self,
        kind: Kind,
        want: &[(String, Option<String>)],
        singletons_only: bool,
    ) -> Result<Vec<Record>> {
        let a = attr_table(kind);
        let mut conds: Vec<String> = vec![];
        let mut args: Vec<SqlValue> = vec![];
        if singletons_only {
            conds.push("(album_id IS NULL OR album_id = 0)".into());
        }
        for (key, value) in want {
            let Some(value) = value else { continue };
            // A NULL column or attribute value reads back as "".
            let empty = if value.is_empty() {
                " OR {x} IS NULL"
            } else {
                ""
            };
            match crate::model::field_type(kind, key) {
                Some(crate::model::FieldType::Str) => {
                    let col = format!("\"{key}\"");
                    conds.push(format!("({col} = ?{})", empty.replace("{x}", &col)));
                    args.push(SqlValue::Text(value.clone()));
                }
                Some(_) => {}
                None if key == "singleton" => {}
                None => {
                    // Flexible attribute: it must exist with this value.
                    conds.push(format!(
                        "id IN (SELECT entity_id FROM {a} WHERE key = ? AND (value = ?{}))",
                        empty.replace("{x}", "value")
                    ));
                    args.push(SqlValue::Text(key.clone()));
                    args.push(SqlValue::Text(value.clone()));
                }
            }
        }
        let where_sql = if conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conds.join(" AND "))
        };
        self.load(kind, &where_sql, &args)
    }

    pub fn get_item(&self, id: i64) -> Result<Option<Item>> {
        Ok(self
            .load(Kind::Item, "WHERE id=?", &[SqlValue::Integer(id)])?
            .pop())
    }

    pub fn get_album(&self, id: i64) -> Result<Option<Album>> {
        if id == 0 {
            return Ok(None);
        }
        Ok(self
            .load(Kind::Album, "WHERE id=?", &[SqlValue::Integer(id)])?
            .pop())
    }

    pub fn album_items(&self, album_id: i64) -> Result<Vec<Item>> {
        let mut items = self.load(
            Kind::Item,
            "WHERE album_id=?",
            &[SqlValue::Integer(album_id)],
        )?;
        items.sort_by_key(|i| (i.int("disc"), i.int("track")));
        Ok(items)
    }

    /// For each path, the items whose file it is (beets' `PathQuery` per
    /// item on reimport), in one query per few hundred paths: beets' schema
    /// has no index on `path`, so each query reads the whole table.
    pub fn items_with_paths(&self, paths: &[&Path]) -> Result<Vec<Vec<Item>>> {
        let key = |p: &Path| to_db_path(p.as_os_str().as_bytes(), self.music_dir.as_deref());
        let mut found: HashMap<Vec<u8>, Vec<Item>> = HashMap::new();
        for chunk in paths.chunks(500) {
            let args: Vec<SqlValue> = chunk.iter().map(|p| SqlValue::Blob(key(p))).collect();
            let where_sql = format!(
                "WHERE path IN ({}) ORDER BY id",
                vec!["?"; args.len()].join(",")
            );
            for it in self.load(Kind::Item, &where_sql, &args)? {
                let k = key(it.path());
                let v = found.entry(k).or_default();
                if !v.iter().any(|o: &Item| o.id == it.id) {
                    v.push(it);
                }
            }
        }
        Ok(paths
            .iter()
            .map(|p| found.get(&key(p)).cloned().unwrap_or_default())
            .collect())
    }

    /// Items whose file is `path` (beets' `PathQuery` on reimport).
    #[cfg(test)]
    pub fn items_with_path(&self, path: &Path) -> Result<Vec<Item>> {
        self.load(
            Kind::Item,
            "WHERE path=?",
            &[SqlValue::Blob(to_db_path(
                path.as_os_str().as_bytes(),
                self.music_dir.as_deref(),
            ))],
        )
    }

    /// The first column of an SQL query's rows, as SQLite has them (the
    /// web plugin's raw `SELECT DISTINCT`s).
    pub fn query_column(&self, sql: &str) -> Result<Vec<SqlValue>> {
        Ok(self
            .conn
            .prepare(sql)?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }

    /// Every row of a raw statement with parameters, as its column values
    /// (the bpd plugin's `list` and `stats`).
    pub fn raw_query(&self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>> {
        let mut stmt = self.conn.prepare(sql)?;
        let n = stmt.column_count();
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
            (0..n)
                .map(|i| r.get::<_, SqlValue>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// How many items and albums the library holds.
    pub fn counts(&self) -> Result<(i64, i64)> {
        Ok((
            self.conn
                .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?,
            self.conn
                .query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?,
        ))
    }

    /// Back the database up beside itself, at `<library><suffix>`.
    pub fn backup(&self, db: &Path, suffix: &str) -> Result<()> {
        backup_db(&self.conn, db, suffix)
    }

    /// Copy a beets library database's items, albums and flexible
    /// attributes into this (empty) library, ids kept. `src_dir` is the
    /// beets library directory its relative paths are under. Columns this
    /// schema lacks are added, as beets keeps them; the beets data
    /// migrations `src` hasn't had then run here. Replaces what the
    /// library holds.
    pub fn copy_from_beets(&self, src: &Connection, src_dir: Option<&Path>) -> Result<CopyCounts> {
        self.invalidate();
        let mut counts = CopyCounts::default();
        let tx = self.conn.unchecked_transaction()?;
        for kind in [Kind::Item, Kind::Album] {
            let (t, a) = (table(kind), attr_table(kind));
            tx.execute_batch(&format!("DELETE FROM {t}; DELETE FROM {a};"))?;
            let ours: Vec<String> = columns_of(&tx, t)?.into_iter().map(|(c, _)| c).collect();
            let theirs = columns_of(src, t)?;
            if theirs.is_empty() {
                bail!("not a beets library: it has no {t} table");
            }
            for (c, ty) in &theirs {
                if !ours.contains(c) {
                    tx.execute_batch(&format!("ALTER TABLE {t} ADD COLUMN {c} {ty};\n"))?;
                }
            }
            let names: Vec<&str> = theirs.iter().map(|(c, _)| c.as_str()).collect();
            let mut insert = tx.prepare(&format!(
                "INSERT INTO {t} ({}) VALUES ({})",
                names.join(", "),
                vec!["?"; names.len()].join(", ")
            ))?;
            let mut select =
                src.prepare(&format!("SELECT {} FROM {t} ORDER BY id", names.join(", ")))?;
            let mut rows = select.query([])?;
            let mut ids = std::collections::HashSet::new();
            while let Some(row) = rows.next()? {
                let mut vals = Vec::with_capacity(names.len());
                for (i, name) in names.iter().enumerate() {
                    let v: SqlValue = row.get(i)?;
                    vals.push(match (*name, v) {
                        ("path" | "artpath", SqlValue::Blob(b)) => self.beets_path(b, src_dir)?,
                        ("path" | "artpath", SqlValue::Text(s)) => {
                            self.beets_path(s.into_bytes(), src_dir)?
                        }
                        ("id", v) => {
                            if let SqlValue::Integer(id) = v {
                                ids.insert(id);
                            }
                            v
                        }
                        (_, v) => v,
                    });
                }
                insert.execute(rusqlite::params_from_iter(vals))?;
            }
            let n = ids.len();
            if columns_of(src, a)?.is_empty() {
                continue;
            }
            let mut insert = tx.prepare(&format!(
                "INSERT INTO {a} (entity_id, key, value) VALUES (?, ?, ?)"
            ))?;
            let mut select = src.prepare(&format!(
                "SELECT entity_id, key, value FROM {a} ORDER BY rowid"
            ))?;
            let mut rows = select.query([])?;
            let mut attrs = 0;
            while let Some(row) = rows.next()? {
                let id: SqlValue = row.get(0)?;
                // Attributes of rows that are gone stay behind.
                if !matches!(id, SqlValue::Integer(i) if ids.contains(&i)) {
                    continue;
                }
                insert.execute(rusqlite::params![
                    id,
                    row.get::<_, SqlValue>(1)?,
                    row.get::<_, SqlValue>(2)?
                ])?;
                attrs += 1;
            }
            match kind {
                Kind::Item => (counts.items, counts.item_attrs) = (n, attrs),
                Kind::Album => (counts.albums, counts.album_attrs) = (n, attrs),
            }
        }
        // The migrations beets ran on `src` are done; paths were converted
        // here. The others run below, on the copied data.
        tx.execute("DELETE FROM migrations", [])?;
        if !columns_of(src, "migrations")?.is_empty() {
            let mut stmt = src.prepare("SELECT name, table_name FROM migrations")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                tx.execute(
                    "INSERT OR IGNORE INTO migrations(name, table_name) VALUES (?, ?)",
                    rusqlite::params![row.get::<_, SqlValue>(0)?, row.get::<_, SqlValue>(1)?],
                )?;
            }
        }
        for t in ["items", "albums"] {
            tx.execute(
                "INSERT OR IGNORE INTO migrations(name, table_name) VALUES ('relative_path', ?)",
                [t],
            )?;
        }
        tx.commit()?;
        Migrator {
            conn: &self.conn,
            backup: None,
            music_dir: self.music_dir.as_deref(),
        }
        .run()?;
        Ok(counts)
    }

    /// A beets database path as this library stores it: relative ones are
    /// under `src_dir`.
    fn beets_path(&self, path: Vec<u8>, src_dir: Option<&Path>) -> Result<SqlValue> {
        if path.is_empty() {
            return Ok(SqlValue::Blob(path));
        }
        if !path.starts_with(b"/") && src_dir.is_none() {
            bail!(
                "the beets library stores paths relative to its directory (such as {:?}): \
                 pass --directory or --beets-config",
                String::from_utf8_lossy(&path)
            );
        }
        let abs = from_db_path(path, src_dir);
        Ok(SqlValue::Blob(to_db_path(
            abs.as_os_str().as_bytes(),
            self.music_dir.as_deref(),
        )))
    }

    /// Create an album from items (album-level fields taken from the first item)
    /// and link the items to it. Items must already have their fields set.
    pub fn add_album(&self, items: &mut [Item]) -> Result<Album> {
        let mut album = Record::new_album();
        if let Some(first) = items.first() {
            for f in crate::model::album_item_shared_fields() {
                if let Some(v) = first.get(f) {
                    album.set(f, v);
                }
            }
        }
        self.add(&mut album)?;
        let id = album.id.expect("BUG: add() assigns the album an id");
        for item in items.iter_mut() {
            item.set_int("album_id", id);
            if item.id.is_some() {
                self.store(item)?;
            } else {
                self.add(item)?;
            }
        }
        Ok(album)
    }
}

/// What `copy_from_beets` copied.
#[derive(Debug, Default, PartialEq)]
pub struct CopyCounts {
    pub items: usize,
    pub albums: usize,
    pub item_attrs: usize,
    pub album_attrs: usize,
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// A beets 2.14 library database made by reference beets
/// (`tools/fixtures/beets-library.sh`), written to `path`. Its paths are
/// relative to a library directory.
#[cfg(test)]
pub(crate) fn make_beets_fixture(path: &Path) {
    Connection::open(path)
        .unwrap()
        .execute_batch(include_str!("../tests/fixtures/beets-2.14-library.sql"))
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        let mut v: Vec<_> = conn
            .prepare("SELECT type, name, sql FROM sqlite_master")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        v.sort();
        v
    }

    fn migrations(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT name, table_name FROM migrations ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// A new library has beets 2.14's schema, statement for statement, and
    /// every beets migration recorded as done, as beets records them.
    #[test]
    fn new_library_has_beets_schema() {
        let d = tempfile::tempdir().unwrap();
        let beets = d.path().join("beets.db");
        make_beets_fixture(&beets);
        let ours = Library::open(&d.path().join("library.db")).unwrap();
        let beets = Connection::open(&beets).unwrap();
        assert_eq!(master(&ours.conn), master(&beets));
        assert_eq!(migrations(&ours.conn), migrations(&beets));
    }

    /// As beets: paths inside the library directory are stored relative to
    /// it, other paths as they are, a singleton's `album_id` is NULL.
    #[test]
    fn stores_paths_relative_and_singletons_null() {
        let d = tempfile::tempdir().unwrap();
        let music = d.path().join("music");
        let lib = Library::open_with(&d.path().join("l.db"), Some(&music), false).unwrap();
        let mut inside = Record::new_item();
        inside.set_path(music.join("A/b.mp3"));
        lib.add(&mut inside).unwrap();
        let mut outside = Record::new_item();
        outside.set_path("/elsewhere/c.mp3");
        let mut album = lib.add_album(&mut [outside]).unwrap();
        album.set("artpath", Value::Path(music.join("A/cover.jpg")));
        lib.store(&album).unwrap();
        let raw = |sql: &str| -> Vec<SqlValue> {
            lib.conn
                .prepare(sql)
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            raw("SELECT path FROM items ORDER BY id"),
            [
                SqlValue::Blob(b"A/b.mp3".to_vec()),
                SqlValue::Blob(b"/elsewhere/c.mp3".to_vec())
            ]
        );
        assert_eq!(
            raw("SELECT album_id FROM items ORDER BY id"),
            [SqlValue::Null, SqlValue::Integer(1)]
        );
        assert_eq!(
            raw("SELECT artpath FROM albums"),
            [SqlValue::Blob(b"A/cover.jpg".to_vec())]
        );
        // Read back absolute; path queries see the stored form.
        let got = lib.get_item(inside.id.unwrap()).unwrap().unwrap();
        assert_eq!(got.path(), music.join("A/b.mp3"));
        assert_eq!(got.int("album_id"), 0);
        assert_eq!(
            lib.items_with_path(&music.join("A/b.mp3")).unwrap().len(),
            1
        );
        let a = lib.get_album(album.id.unwrap()).unwrap().unwrap();
        assert_eq!(a.path_field("artpath"), music.join("A/cover.jpg"));
        // Removing a record removes its flexible attributes (no cascade).
        let mut it = got;
        it.flex.insert("mood".into(), "x".into());
        lib.store(&it).unwrap();
        lib.remove(&it).unwrap();
        assert!(raw("SELECT key FROM item_attributes").is_empty());
    }

    #[test]
    fn items_with_paths_groups_by_path() {
        let d = tempfile::tempdir().unwrap();
        let music = d.path().join("music");
        let lib = Library::in_memory_at(&music).unwrap();
        let add = |p: PathBuf| {
            let mut it = Record::new_item();
            it.set_path(p);
            lib.add(&mut it).unwrap()
        };
        let a = add(music.join("a.mp3"));
        let b1 = add(PathBuf::from("/out/b.mp3"));
        let b2 = add(PathBuf::from("/out/b.mp3"));
        let (pa, pb, pc) = (
            music.join("a.mp3"),
            PathBuf::from("/out/b.mp3"),
            music.join("c.mp3"),
        );
        let got = lib
            .items_with_paths(&[pb.as_path(), pc.as_path(), pa.as_path()])
            .unwrap();
        let ids: Vec<Vec<i64>> = got
            .iter()
            .map(|v| v.iter().map(|i| i.id.unwrap()).collect())
            .collect();
        assert_eq!(ids, [vec![b1, b2], vec![], vec![a]]);
        let many: Vec<PathBuf> = (0..1200).map(|n| music.join(format!("{n}.mp3"))).collect();
        let mut refs: Vec<&Path> = many.iter().map(PathBuf::as_path).collect();
        refs.push(&pa);
        let got = lib.items_with_paths(&refs).unwrap();
        assert_eq!(got.len(), 1201);
        assert_eq!(got[1200].len(), 1);
    }

    #[test]
    fn db_paths_as_beets() {
        let m = Some(Path::new("/music"));
        let to = |p: &str| String::from_utf8(to_db_path(p.as_bytes(), m)).unwrap();
        assert_eq!(to("/music/a/b.mp3"), "a/b.mp3");
        assert_eq!(to("/music"), ".");
        assert_eq!(to("/music/a/../b.mp3"), "b.mp3");
        assert_eq!(to("/music/../x.mp3"), "../x.mp3");
        assert_eq!(to("/musicx/a.mp3"), "/musicx/a.mp3");
        assert_eq!(to("rel/a.mp3"), "rel/a.mp3");
        assert_eq!(to(""), "");
        assert_eq!(to_db_path(b"/music/a.mp3", None), b"/music/a.mp3");
        // Bytes that aren't UTF-8 survive both ways.
        assert_eq!(to_db_path(b"/music/\xff.mp3", m), b"\xff.mp3");
        assert_eq!(
            from_db_path(b"\xff.mp3".to_vec(), m).as_os_str().as_bytes(),
            b"/music/\xff.mp3"
        );
        assert_eq!(from_db_path(b"a/./b".to_vec(), m), Path::new("/music/a/b"));
        assert_eq!(from_db_path(b"/x/y".to_vec(), m), Path::new("/x/y"));
        assert_eq!(from_db_path(b"a".to_vec(), None), Path::new("a"));
    }

    #[test]
    fn legacy_lyrics_as_beets() {
        assert_eq!(legacy_lyrics("la la"), ("la la".into(), None));
        let (text, src) = legacy_lyrics("la\n\nSource: https://www.genius.com/x \n");
        assert_eq!(text, "la");
        assert_eq!(
            src,
            Some(("genius".into(), "https://www.genius.com/x".into()))
        );
        let (_, src) = legacy_lyrics("la\n\nSource: https://example.com/x");
        assert_eq!(src.unwrap().0, "google");
        let (_, src) = legacy_lyrics("la\n\nSource: lrclib.net/x");
        assert_eq!(src.unwrap().0, "google");
    }

    /// A library made by beets-rs before it used beets' layout: rebuilt in
    /// beets' layout (backed up first), paths made relative, singletons'
    /// `album_id` NULL, `data_source` a flexible attribute, beets' data
    /// migrations run on what it held.
    #[test]
    fn old_beets_rs_layout_is_rebuilt() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("library.db");
        let music = d.path().join("music");
        let m = music.display().to_string();
        {
            // As beets-rs' `init` made it: tables grown column by column,
            // cascading attribute tables, its own indexes.
            let c = Connection::open(&db).unwrap();
            for kind in [Kind::Item, Kind::Album] {
                let t = table(kind);
                c.execute_batch(&format!("CREATE TABLE {t} (id INTEGER PRIMARY KEY)"))
                    .unwrap();
                for (name, ty) in schema(kind)
                    .iter()
                    .chain([&("data_source", FieldType::Str)])
                {
                    c.execute_batch(&format!(
                        "ALTER TABLE {t} ADD COLUMN {name} {}",
                        ty.sql_type()
                    ))
                    .unwrap();
                }
                c.execute_batch(&format!(
                    "CREATE TABLE IF NOT EXISTS {a} (
                    entity_id INTEGER NOT NULL REFERENCES {t}(id) ON DELETE CASCADE,
                    key TEXT NOT NULL, value TEXT, PRIMARY KEY (entity_id, key));",
                    a = attr_table(kind)
                ))
                .unwrap();
            }
            c.execute_batch(&format!(
                "CREATE INDEX items_album_id ON items(album_id);
                 CREATE INDEX items_path ON items(path);
                 ALTER TABLE items ADD COLUMN genre TEXT;
                 CREATE TABLE migrations (
            name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            PRIMARY KEY(name, table_name));
                 INSERT INTO migrations VALUES ('multi_genre_field', 'items');
                 INSERT INTO albums (id, album, artpath, data_source)
                     VALUES (1, 'Al', CAST('{m}/Al/cover.jpg' AS BLOB), 'MusicBrainz');
                 INSERT INTO items (id, path, album_id, title, data_source, genre, genres, lyrics)
                     VALUES (1, CAST('{m}/Al/1.mp3' AS BLOB), 1, 'One', 'MusicBrainz', 'Old', '', ''),
                            (2, CAST('/other/2.mp3' AS BLOB), 0, 'Two', '', '', 'Jazz', '[Instrumental]');
                 INSERT INTO item_attributes VALUES (1, 'mood', 'happy');
                 INSERT INTO album_attributes VALUES (1, 'rating', '5');"
            ))
            .unwrap();
        }
        let lib = Library::open_with(&db, Some(&music), true).unwrap();
        assert!(d.path().join("library.db-before-beets_layout.bak").exists());
        // The schema is beets', but for the legacy column beets would keep.
        let beets = d.path().join("beets.db");
        make_beets_fixture(&beets);
        let beets = Connection::open(&beets).unwrap();
        let got = master(&lib.conn);
        let items_sql = |m: &[(String, String, Option<String>)]| {
            m.iter()
                .find(|r| r.1 == "items")
                .unwrap()
                .2
                .clone()
                .unwrap()
        };
        assert_eq!(
            items_sql(&got),
            format!(
                "{}, genre TEXT)",
                items_sql(&master(&beets)).strip_suffix(')').unwrap()
            )
        );
        let others = |m: Vec<(String, String, Option<String>)>| -> Vec<_> {
            m.into_iter().filter(|r| r.1 != "items").collect()
        };
        assert_eq!(others(got), others(master(&beets)));
        // The migrations recorded before stay done (`genre` isn't
        // migrated again), the others ran.
        assert_eq!(migrations(&lib.conn).len(), migrations(&beets).len());
        let raw = |sql: &str| -> SqlValue { lib.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            raw("SELECT path FROM items WHERE id = 1"),
            SqlValue::Blob(b"Al/1.mp3".to_vec())
        );
        assert_eq!(
            raw("SELECT path FROM items WHERE id = 2"),
            SqlValue::Blob(b"/other/2.mp3".to_vec())
        );
        assert_eq!(
            raw("SELECT artpath FROM albums"),
            SqlValue::Blob(b"Al/cover.jpg".to_vec())
        );
        assert_eq!(
            raw("SELECT album_id FROM items WHERE id = 2"),
            SqlValue::Null
        );
        let one = lib.get_item(1).unwrap().unwrap();
        assert_eq!(one.path(), music.join("Al/1.mp3"));
        assert_eq!(one.str("data_source"), "MusicBrainz");
        assert_eq!(one.str("mood"), "happy");
        assert!(one.list("genres").is_empty());
        let two = lib.get_item(2).unwrap().unwrap();
        assert_eq!(two.int("album_id"), 0);
        assert!(!two.flex.contains_key("data_source"));
        assert_eq!(two.str("lyrics"), "");
        assert_eq!(two.str("lyrics_instrumental"), "1");
        let al = lib.get_album(1).unwrap().unwrap();
        assert_eq!(al.str("data_source"), "MusicBrainz");
        assert_eq!(al.str("rating"), "5");
        assert_eq!(al.path_field("artpath"), music.join("Al/cover.jpg"));
        // Opening it again changes nothing.
        drop(lib);
        let before = std::fs::read(&db).unwrap();
        Library::open_with(&db, Some(&music), true).unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), before);
    }

    /// beets' multi-value migrations: a library with the old `genre` and
    /// `composer` columns gets `genres` and `composers`, split on the
    /// known separators, once; lists already set are left alone.
    #[test]
    fn migrates_single_valued_fields_to_lists() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("library.db");
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, genre TEXT, composer TEXT, genres TEXT);
                 CREATE TABLE albums (id INTEGER PRIMARY KEY, genre TEXT);
                 INSERT INTO items VALUES (1, 'Rock, Pop', 'A / B', NULL);
                 INSERT INTO items VALUES (2, 'Jazz', '', NULL);
                 INSERT INTO items VALUES (3, 'Old', 'C', 'Kept');
                 INSERT INTO albums VALUES (1, 'Rock; Pop');",
            )
            .unwrap();
        }
        let lib = Library::open(&db).unwrap();
        let items = lib
            .items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
            .unwrap();
        let get = |id: i64, f: &str| items.iter().find(|i| i.id == Some(id)).unwrap().list(f);
        assert_eq!(get(1, "genres"), ["Rock", "Pop"]);
        assert_eq!(get(1, "composers"), ["A", "B"]);
        assert_eq!(get(2, "genres"), ["Jazz"]);
        assert!(get(2, "composers").is_empty());
        assert_eq!(get(3, "genres"), ["Kept"]);
        assert_eq!(lib.all_albums().unwrap()[0].list("genres"), ["Rock", "Pop"]);
        // Each migration backed the database up first, as beets names it;
        // the copy has the data from before.
        let bak = |table: &str, name: &str| {
            d.path()
                .join(format!("library.db-before-{table}-{name}.bak"))
        };
        let c = Connection::open(bak("items", "multi_genre_field")).unwrap();
        let (genre, genres): (String, Option<String>) = c
            .query_row("SELECT genre, genres FROM items WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((genre.as_str(), genres), ("Rock, Pop", None));
        assert!(bak("albums", "multi_genre_field").exists());
        assert!(bak("items", "multi_composer_field").exists());
        // As beets, even a migration with nothing to change.
        assert!(bak("items", "multi_lyricist_field").exists());
        assert!(bak("items", "instrumental_lyrics_in_flex_field").exists());
        // Once only: a later change to the old column isn't migrated again.
        lib.conn
            .execute(
                "UPDATE items SET genres = '', genre = 'New' WHERE id = 2",
                [],
            )
            .unwrap();
        drop(lib);
        let lib = Library::open(&db).unwrap();
        let items = lib
            .items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
            .unwrap();
        assert!(
            items
                .iter()
                .find(|i| i.id == Some(2))
                .unwrap()
                .list("genres")
                .is_empty()
        );
        // `create_backup_before_migrations: no`.
        let db2 = d.path().join("other.db");
        Connection::open(&db2)
            .unwrap()
            .execute_batch(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, genre TEXT);
                 INSERT INTO items VALUES (1, 'Rock');",
            )
            .unwrap();
        Library::open_with(&db2, None, false).unwrap();
        assert!(
            !d.path()
                .join("other.db-before-items-multi_genre_field.bak")
                .exists()
        );
    }

    // As beets' `Model.add`: every new record gets its own `added`; an
    // album without art stores NULL, as beets does.
    #[test]
    fn add_stamps_added_and_no_art_is_null() {
        let lib = Library::in_memory().unwrap();
        let mut it = Record::new_item();
        it.set("added", Value::Float(1.0));
        lib.add(&mut it).unwrap();
        assert!(it.float("added") > 1.0);
        let mut items = [Record::new_item(), Record::new_item()];
        let album = lib.add_album(&mut items).unwrap();
        assert!(album.float("added") > 1.0);
        assert!(items[1].float("added") >= items[0].float("added"));
        let art: Option<Vec<u8>> = lib
            .conn
            .query_row("SELECT artpath FROM albums", [], |r| r.get(0))
            .unwrap();
        assert_eq!(art, None);
        let back = lib.get_album(album.id.unwrap()).unwrap().unwrap();
        assert_eq!(back.path_field("artpath"), Path::new(""));
    }

    #[test]
    fn memo_is_dropped_on_writes() {
        let lib = Library::in_memory().unwrap();
        let count = |lib: &Library| *lib.memo("n".into(), || lib.all_albums().unwrap().len());
        assert_eq!(count(&lib), 0);
        let mut album = lib.add_album(&mut [Record::new_item()]).unwrap();
        assert_eq!(count(&lib), 1);
        // A cached value stays until the next write.
        assert_eq!(*lib.memo("n".into(), || 99usize), 1);
        album.set_str("album", "x");
        lib.store(&album).unwrap();
        assert_eq!(*lib.memo("n".into(), || 99usize), 99);
        lib.remove(&album).unwrap();
        assert_eq!(count(&lib), 0);
    }

    fn album(lib: &Library, artist: &str, title: &str, flex: &[(&str, &str)]) -> Album {
        let mut it = Record::new_item();
        it.set_str("albumartist", artist);
        it.set_str("album", title);
        let mut a = lib.add_album(&mut [it]).unwrap();
        for (k, v) in flex {
            a.flex.insert(k.to_string(), v.to_string());
        }
        lib.store(&a).unwrap();
        a
    }

    fn want(pairs: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect()
    }

    fn ids(recs: &[Record]) -> Vec<i64> {
        let mut v: Vec<i64> = recs.iter().map(|r| r.id.unwrap()).collect();
        v.sort();
        v
    }

    #[test]
    fn duplicate_candidates_narrow_by_text_and_flex_fields() {
        let lib = Library::in_memory().unwrap();
        let a = album(&lib, "AC/DC", "Back in Black", &[("source", "cd")]);
        let b = album(&lib, "AC/DC", "Back in Black", &[("source", "vinyl")]);
        let _c = album(&lib, "AC/DC", "Highway to Hell", &[]);
        let _d = album(&lib, "ac/dc", "Back in Black", &[]);
        let fixed = want(&[
            ("albumartist", Some("AC/DC")),
            ("album", Some("Back in Black")),
        ]);
        let got = lib
            .duplicate_candidates(Kind::Album, &fixed, false)
            .unwrap();
        assert_eq!(ids(&got), vec![a.id.unwrap(), b.id.unwrap()]);
        // Flexible keys narrow too, and the loaded rows carry their attributes.
        let flex = want(&[("album", Some("Back in Black")), ("source", Some("vinyl"))]);
        let got = lib.duplicate_candidates(Kind::Album, &flex, false).unwrap();
        assert_eq!(ids(&got), vec![b.id.unwrap()]);
        assert_eq!(got[0].flex.get("source").map(String::as_str), Some("vinyl"));
    }

    #[test]
    fn duplicate_candidates_never_drop_what_matches_would_keep() {
        let lib = Library::in_memory().unwrap();
        let blank = album(&lib, "", "Untitled", &[]);
        let named = album(&lib, "Someone", "Untitled", &[]);
        // An empty wanted value also finds empty columns.
        let w = want(&[("albumartist", Some("")), ("album", Some("Untitled"))]);
        let got = lib.duplicate_candidates(Kind::Album, &w, false).unwrap();
        assert_eq!(ids(&got), vec![blank.id.unwrap()]);
        // Keys SQL doesn't check exactly (non-text, or wanted missing) don't
        // narrow at all: the caller's `matches` decides those.
        let w = want(&[
            ("year", Some("0")),
            ("label", None),
            ("album", Some("Untitled")),
        ]);
        let got = lib.duplicate_candidates(Kind::Album, &w, false).unwrap();
        assert_eq!(ids(&got), vec![blank.id.unwrap(), named.id.unwrap()]);
        // No keys at all: every row.
        assert_eq!(
            lib.duplicate_candidates(Kind::Album, &[], false)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn duplicate_candidates_for_singletons_skip_album_tracks() {
        let lib = Library::in_memory().unwrap();
        let mut single = Record::new_item();
        single.set_str("artist", "A");
        single.set_str("title", "T");
        single.set_path("/m/single.mp3");
        lib.add(&mut single).unwrap();
        let mut in_album = single.clone();
        in_album.id = None;
        in_album.set_path("/m/album/1.mp3");
        lib.add_album(&mut [in_album]).unwrap();
        let w = want(&[("artist", Some("A")), ("title", Some("T"))]);
        assert_eq!(
            lib.duplicate_candidates(Kind::Item, &w, false)
                .unwrap()
                .len(),
            2
        );
        let got = lib.duplicate_candidates(Kind::Item, &w, true).unwrap();
        assert_eq!(ids(&got), vec![single.id.unwrap()]);
    }

    #[test]
    fn filtered_loads_attach_only_their_own_flex_attributes() {
        let lib = Library::in_memory().unwrap();
        let a = album(&lib, "X", "One", &[]);
        let b = album(&lib, "X", "Two", &[]);
        for (album, value) in [(&a, "a"), (&b, "b")] {
            for mut it in lib.album_items(album.id.unwrap()).unwrap() {
                it.flex.insert("tag".into(), value.into());
                lib.store(&it).unwrap();
            }
        }
        let items = lib.album_items(b.id.unwrap()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].flex.get("tag").map(String::as_str), Some("b"));
    }

    #[test]
    fn roundtrip() {
        let lib = Library::in_memory().unwrap();
        let mut it = Record::new_item();
        it.set_str("title", "Song");
        it.set_int("track", 4);
        it.set_path("/music/a/b.flac");
        it.flex.insert("mood".into(), "calm".into());
        lib.add(&mut it).unwrap();
        let got = lib.get_item(it.id.unwrap()).unwrap().unwrap();
        assert_eq!(got.str("title"), "Song");
        assert_eq!(got.int("track"), 4);
        assert_eq!(got.path(), Path::new("/music/a/b.flac"));
        assert_eq!(got.str("mood"), "calm");
        assert_eq!(
            lib.items_with_path(Path::new("/music/a/b.flac"))
                .unwrap()
                .len(),
            1
        );

        let mut items = vec![got];
        let album = lib.add_album(&mut items).unwrap();
        assert_eq!(lib.album_items(album.id.unwrap()).unwrap().len(), 1);
        lib.remove(&album).unwrap();
        assert_eq!(
            lib.get_item(it.id.unwrap())
                .unwrap()
                .unwrap()
                .int("album_id"),
            0
        );
    }
}
