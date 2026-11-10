//! SQLite-backed library of items and albums.

use crate::model::{Album, FieldType, Item, Kind, Record, Value, schema};
use crate::query::Query;
use anyhow::{Context, Result};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

pub struct Library {
    conn: Connection,
}

fn table(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "items",
        Kind::Album => "albums",
    }
}

fn attr_table(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "item_attributes",
        Kind::Album => "album_attributes",
    }
}

fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Str(s) => SqlValue::Text(s.clone()),
        Value::Int(i) => SqlValue::Integer(*i),
        Value::Float(f) => SqlValue::Real(*f),
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Path(p) => SqlValue::Blob(p.as_os_str().as_bytes().to_vec()),
    }
}

fn from_sql(v: SqlValue, ty: FieldType) -> Value {
    match (ty, v) {
        (_, SqlValue::Null) => ty.default_value(),
        (FieldType::Path, SqlValue::Blob(b)) => {
            Value::Path(PathBuf::from(std::ffi::OsString::from_vec(b)))
        }
        (FieldType::Path, SqlValue::Text(s)) => Value::Path(PathBuf::from(s)),
        (FieldType::Str, SqlValue::Text(s)) => Value::Str(s),
        (FieldType::Bool, SqlValue::Integer(i)) => Value::Bool(i != 0),
        (FieldType::Float | FieldType::Duration | FieldType::Date, SqlValue::Real(f)) => {
            Value::Float(f)
        }
        (FieldType::Float | FieldType::Duration | FieldType::Date, SqlValue::Integer(i)) => {
            Value::Float(i as f64)
        }
        (_, SqlValue::Integer(i)) => Value::Int(i),
        (_, SqlValue::Real(f)) => Value::Int(f as i64),
        (_, SqlValue::Text(s)) => Value::Str(s),
        (t, _) => t.default_value(),
    }
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening library {}", path.display()))?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        for kind in [Kind::Item, Kind::Album] {
            let t = table(kind);
            conn.execute(
                &format!("CREATE TABLE IF NOT EXISTS {t} (id INTEGER PRIMARY KEY)"),
                [],
            )?;
            // Add any columns missing from an older schema.
            let existing: Vec<String> = conn
                .prepare(&format!("PRAGMA table_info({t})"))?
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<_, _>>()?;
            for (name, ty) in schema(kind) {
                if !existing.iter().any(|e| e == name) {
                    conn.execute(
                        &format!("ALTER TABLE {t} ADD COLUMN {name} {}", ty.sql_type()),
                        [],
                    )?;
                }
            }
            let a = attr_table(kind);
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {a} (
                    entity_id INTEGER NOT NULL REFERENCES {t}(id) ON DELETE CASCADE,
                    key TEXT NOT NULL, value TEXT, PRIMARY KEY (entity_id, key));"
            ))?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS items_album_id ON items(album_id);
             CREATE INDEX IF NOT EXISTS items_path ON items(path);",
        )?;
        Ok(Library { conn })
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
                Err(e)
            }
        }
    }

    /// Insert a new record, assigning its id.
    pub fn add(&self, rec: &mut Record) -> Result<i64> {
        let t = table(rec.kind);
        let cols: Vec<&str> = rec.fixed_values().map(|(n, _)| n).collect();
        let vals: Vec<SqlValue> = rec.fixed_values().map(|(_, v)| to_sql(v)).collect();
        let placeholders = vec!["?"; cols.len()].join(",");
        self.conn.execute(
            &format!(
                "INSERT INTO {t} ({}) VALUES ({placeholders})",
                cols.join(",")
            ),
            rusqlite::params_from_iter(vals),
        )?;
        let id = self.conn.last_insert_rowid();
        rec.id = Some(id);
        self.store_flex(rec)?;
        Ok(id)
    }

    /// Write all fields of an existing record.
    pub fn store(&self, rec: &Record) -> Result<()> {
        let id = rec.id.context("record has no id; add it first")?;
        let t = table(rec.kind);
        let sets: Vec<String> = rec.fixed_values().map(|(n, _)| format!("{n}=?")).collect();
        let mut vals: Vec<SqlValue> = rec.fixed_values().map(|(_, v)| to_sql(v)).collect();
        vals.push(SqlValue::Integer(id));
        self.conn.execute(
            &format!("UPDATE {t} SET {} WHERE id=?", sets.join(",")),
            rusqlite::params_from_iter(vals),
        )?;
        self.store_flex(rec)
    }

    fn store_flex(&self, rec: &Record) -> Result<()> {
        let a = attr_table(rec.kind);
        let id = rec.id.unwrap();
        self.conn
            .execute(&format!("DELETE FROM {a} WHERE entity_id=?"), [id])?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "INSERT INTO {a} (entity_id, key, value) VALUES (?,?,?)"
        ))?;
        for (k, v) in &rec.flex {
            stmt.execute(params![id, k, v])?;
        }
        Ok(())
    }

    pub fn remove(&self, rec: &Record) -> Result<()> {
        if let Some(id) = rec.id {
            if rec.kind == Kind::Album {
                self.conn
                    .execute("UPDATE items SET album_id=0 WHERE album_id=?", [id])?;
            }
            self.conn
                .execute(&format!("DELETE FROM {} WHERE id=?", table(rec.kind)), [id])?;
        }
        Ok(())
    }

    fn row_to_record(kind: Kind, row: &Row) -> rusqlite::Result<Record> {
        let mut rec = Record::new(kind);
        rec.id = Some(row.get("id")?);
        for (name, ty) in schema(kind) {
            let v: SqlValue = row.get(*name)?;
            rec.set(name, from_sql(v, *ty));
        }
        Ok(rec)
    }

    fn load(&self, kind: Kind, where_sql: &str, args: &[SqlValue]) -> Result<Vec<Record>> {
        let t = table(kind);
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM {t} {where_sql}"))?;
        let mut recs: Vec<Record> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Self::row_to_record(kind, r)
            })?
            .collect::<Result<_, _>>()?;
        if recs.is_empty() {
            return Ok(recs);
        }
        // Attach flexible attributes.
        let mut flex: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
        let a = attr_table(kind);
        let mut stmt = if recs.len() == 1 {
            let mut s = self.conn.prepare(&format!(
                "SELECT entity_id, key, value FROM {a} WHERE entity_id=?"
            ))?;
            let rows = s.query_map([recs[0].id.unwrap()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?))
            })?;
            for row in rows {
                let (id, k, v): (i64, String, Option<String>) = row?;
                flex.entry(id).or_default().insert(k, v.unwrap_or_default());
            }
            None
        } else {
            Some(
                self.conn
                    .prepare(&format!("SELECT entity_id, key, value FROM {a}"))?,
            )
        };
        if let Some(s) = stmt.as_mut() {
            let rows = s.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?))
            })?;
            for row in rows {
                let (id, k, v): (i64, String, Option<String>) = row?;
                flex.entry(id).or_default().insert(k, v.unwrap_or_default());
            }
        }
        for r in &mut recs {
            if let Some(f) = flex.remove(&r.id.unwrap()) {
                r.flex = f;
            }
        }
        Ok(recs)
    }

    pub fn items(&self, q: &Query) -> Result<Vec<Item>> {
        let mut v: Vec<Item> = self
            .load(Kind::Item, "", &[])?
            .into_iter()
            .filter(|r| q.matches(r))
            .collect();
        q.sort(&mut v);
        Ok(v)
    }

    pub fn albums(&self, q: &Query) -> Result<Vec<Album>> {
        let mut v: Vec<Album> = self
            .load(Kind::Album, "", &[])?
            .into_iter()
            .filter(|r| q.matches(r))
            .collect();
        q.sort(&mut v);
        Ok(v)
    }

    pub fn all_albums(&self) -> Result<Vec<Album>> {
        self.load(Kind::Album, "", &[])
    }

    #[cfg(test)]
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

    pub fn has_path(&self, path: &Path) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM items WHERE path=?",
            [SqlValue::Blob(path.as_os_str().as_bytes().to_vec())],
            |r| r.get(0),
        )?;
        Ok(n > 0)
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
        album.set("added", Value::Float(now()));
        self.add(&mut album)?;
        let id = album.id.unwrap();
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

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(lib.has_path(Path::new("/music/a/b.flac")).unwrap());

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
