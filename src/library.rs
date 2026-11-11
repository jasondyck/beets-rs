//! SQLite-backed library of items and albums.

use crate::model::{Album, FieldType, Item, Kind, Record, Value, schema};
use crate::query::Query;
use anyhow::{Context, Result};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params};
use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Library {
    conn: Connection,
    /// Values computed from the library's contents (beets' `_memotable`),
    /// dropped whenever this process writes to the library.
    memo: RefCell<HashMap<String, Rc<dyn Any>>>,
}

fn table(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "items",
        Kind::Album => "albums",
    }
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
) -> crate::query::Obj<'a> {
    crate::query::Obj {
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

fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Str(s) => SqlValue::Text(s.clone()),
        Value::Int(i) => SqlValue::Integer(*i),
        Value::Float(f) => SqlValue::Real(*f),
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        // No path (an album without art) is NULL, as in beets.
        Value::Path(p) if p.as_os_str().is_empty() => SqlValue::Null,
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
        Ok(Library {
            conn,
            memo: RefCell::new(HashMap::new()),
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
        self.invalidate();
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
        self.invalidate();
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

    /// The column of each schema field (and `id`) in a statement's result.
    fn columns(kind: Kind, stmt: &rusqlite::Statement) -> rusqlite::Result<(usize, Vec<usize>)> {
        let fields = schema(kind)
            .iter()
            .map(|(n, _)| stmt.column_index(n))
            .collect::<rusqlite::Result<_>>()?;
        Ok((stmt.column_index("id")?, fields))
    }

    fn row_to_record(
        kind: Kind,
        row: &Row,
        cols: &(usize, Vec<usize>),
    ) -> rusqlite::Result<Record> {
        let mut values = Vec::with_capacity(cols.1.len());
        for ((_, ty), &i) in schema(kind).iter().zip(&cols.1) {
            values.push(from_sql(row.get::<_, SqlValue>(i)?, *ty));
        }
        Ok(Record::from_values(kind, row.get(cols.0)?, values))
    }

    fn load(&self, kind: Kind, where_sql: &str, args: &[SqlValue]) -> Result<Vec<Record>> {
        let t = table(kind);
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT * FROM {t} {where_sql}"))?;
        let cols = Self::columns(kind, &stmt)?;
        let mut recs: Vec<Record> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Self::row_to_record(kind, r, &cols)
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
            let rows = s.query_map([recs[0].id.unwrap()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?))
            })?;
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
            if let Some(f) = flex.remove(&r.id.unwrap()) {
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
        let cols = Self::columns(kind, &stmt)?;
        let mut recs: Vec<Record> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Self::row_to_record(kind, r, &cols)
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
            if let Some(f) = flex.remove(&r.id.unwrap()) {
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
                .map(|a| (a.id.unwrap(), a))
                .collect(),
            Kind::Album => HashMap::new(),
        };
        let dirs: HashMap<i64, PathBuf> = match kind {
            Kind::Album => self.album_dirs()?,
            Kind::Item => HashMap::new(),
        };
        if clause.is_none() {
            recs.retain(|r| q.matches_obj(&obj_of(kind, r, &albums, &dirs)));
        }
        if sort.is_slow() {
            let objs: Vec<_> = recs
                .iter()
                .map(|r| obj_of(kind, r, &albums, &dirs))
                .collect();
            let order = sort.sort_slow(&objs);
            drop(objs);
            let mut slots: Vec<Option<Record>> = recs.into_iter().map(Some).collect();
            recs = order
                .into_iter()
                .map(|i| slots[i].take().unwrap())
                .collect();
        }
        Ok(recs)
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
             ON items.album_id = albums.id WHERE items.album_id != 0 \
             ORDER BY {order}items.id"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, SqlValue>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, path) = row?;
            let path = match path {
                SqlValue::Blob(b) => PathBuf::from(std::ffi::OsString::from_vec(b)),
                SqlValue::Text(s) => PathBuf::from(s),
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

    /// Items whose file is `path` (beets' `PathQuery` on reimport).
    pub fn items_with_path(&self, path: &Path) -> Result<Vec<Item>> {
        self.load(
            Kind::Item,
            "WHERE path=?",
            &[SqlValue::Blob(path.as_os_str().as_bytes().to_vec())],
        )
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
