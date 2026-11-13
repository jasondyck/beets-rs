//! Port of beets 2.14's `edit` plugin (`beetsplug/edit.py`): edit items' or
//! albums' metadata as YAML in a text editor (`beet edit`), and the
//! importer's "eDit" and "edit Candidates" choices.
//!
//! The YAML is PyYAML's (`crate::pyyaml`): one document per object, sorted
//! keys; numbers, booleans and lists are native YAML values, the rest is
//! edited as text and parsed back for the field's type.

use crate::commands::{format_record, show_model_changes};
use crate::config::Config;
use crate::library::Library;
use crate::model::{FieldType, Item, Kind, Record, Value, schema};
use crate::plugins::{ChoiceTask, Plugin, PluginLog, PromptChoice, WARNING};
use crate::pyyaml::{self, PyVal};
use crate::query::Query;
use crate::template::Template;
use crate::ui::{self, Choice};
use anyhow::{Result, bail};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One YAML document: field name -> value, in the object's key order.
pub type Doc = Vec<(String, PyVal)>;

/// The edit plugin's settings.
#[derive(Debug, Clone)]
pub struct EditCfg {
    pub albumfields: Vec<String>,
    pub itemfields: Vec<String>,
    pub ignore_fields: Vec<String>,
    /// The `editor` option.
    pub editor: String,
}

impl EditCfg {
    pub fn new(cfg: &Config) -> Self {
        EditCfg {
            albumfields: cfg.list("edit.albumfields"),
            itemfields: cfg.list("edit.itemfields"),
            ignore_fields: cfg.list("edit.ignore_fields"),
            editor: cfg.str("editor"),
        }
    }
}

// ------------------------------------------------------- values and YAML

/// The value beets holds for a field (`obj[key]`), as Python sees it.
fn raw_value(obj: &Record, key: &str) -> PyVal {
    if key == "id" {
        return obj.id.map_or(PyVal::None, |i| PyVal::Int(i as i128));
    }
    let Some(ty) = obj.field_type(key) else {
        return match obj.flex.get(key) {
            Some(s) => PyVal::Str(s.clone()),
            None => PyVal::None,
        };
    };
    let v = obj.get(key).unwrap_or(Value::Null);
    match (ty, v) {
        (_, Value::Null) => PyVal::None,
        // An album without art has a NULL `artpath`.
        (FieldType::Path, v) if v.raw().is_empty() => PyVal::None,
        // A singleton's album id is NULL.
        (_, Value::Int(0)) if key == "album_id" && obj.kind == Kind::Item => PyVal::None,
        (FieldType::List(_), _) => PyVal::List(obj.list(key).into_iter().map(PyVal::Str).collect()),
        // Read from the database, a boolean is the integer SQLite stores
        // (so beets edits it as text: 'True').
        (FieldType::Bool, v) => PyVal::Int((v.as_f64().unwrap_or(0.0) != 0.0) as i128),
        (FieldType::Int | FieldType::Padded(_) | FieldType::Bitrate | FieldType::SampleRate, v) => {
            PyVal::Int(v.as_f64().unwrap_or(0.0) as i128)
        }
        (FieldType::Float(_) | FieldType::Duration | FieldType::Date | FieldType::NullFloat, v) => {
            PyVal::Float(v.as_f64().unwrap_or(0.0))
        }
        (_, v) => PyVal::Str(v.raw()),
    }
}

/// `_safe_value`: whether `value` is representable in YAML as the field's
/// own type (numbers, booleans, lists), so it's used as it is.
fn safe_value(obj: &Record, key: &str, value: &PyVal) -> bool {
    if matches!(value, PyVal::None) {
        return true;
    }
    let ty = if key == "id" {
        Some(FieldType::Int)
    } else {
        obj.field_type(key)
    };
    match ty {
        // Python's bool is an int.
        Some(
            FieldType::Int | FieldType::Padded(_) | FieldType::Bitrate | FieldType::SampleRate,
        ) => matches!(value, PyVal::Int(_) | PyVal::Bool(_)),
        Some(FieldType::Float(_) | FieldType::Duration | FieldType::Date | FieldType::NullFloat) => {
            matches!(value, PyVal::Float(_))
        }
        Some(FieldType::Bool) => matches!(value, PyVal::Bool(_)),
        Some(FieldType::List(_)) => matches!(value, PyVal::List(_)),
        _ => false,
    }
}

/// `flatten`: the object as a YAML document, only `fields` if given.
pub fn flatten(obj: &Record, fields: Option<&BTreeSet<String>>, lib: Option<&Library>) -> Doc {
    let formatted = crate::formatted::Formatted::new(obj, lib);
    // An item's keys include its album's (beets' `Item.keys(with_album=True)`).
    let album = match (obj.kind, lib) {
        (Kind::Item, Some(lib)) if obj.int("album_id") != 0 => {
            lib.get_album(obj.int("album_id")).ok().flatten()
        }
        _ => None,
    };
    let mut keys = obj.keys();
    if let Some(a) = &album {
        for k in a.keys() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    let mut d = vec![];
    for key in keys {
        if fields.is_some_and(|f| !f.contains(&key)) {
            continue;
        }
        let own = obj.has_field(&key) || obj.field_type(&key).is_some();
        let v = match &album {
            Some(a) if !own => match raw_value(a, &key) {
                PyVal::None => PyVal::None,
                // Not the item's own type: edited as text.
                _ => PyVal::Str(formatted.get(&key).unwrap_or_default()),
            },
            _ => raw_value(obj, &key),
        };
        let v = if safe_value(obj, &key, &v) {
            v
        } else {
            PyVal::Str(formatted.get(&key).unwrap_or_default())
        };
        d.push((key, v));
    }
    d
}

/// `apply_`: set the object's fields from a document.
pub fn apply_doc(obj: &mut Record, data: &Doc) {
    for (key, value) in data {
        if key == "id" {
            continue;
        }
        if safe_value(obj, key, value) {
            match value {
                PyVal::None => obj.set(key, Value::Null),
                PyVal::Int(i) => obj.set(key, Value::Int(*i as i64)),
                PyVal::Bool(b) if obj.field_type(key) == Some(FieldType::Bool) => {
                    obj.set(key, Value::Bool(*b));
                }
                PyVal::Bool(b) => obj.set(key, Value::Int(*b as i64)),
                PyVal::Float(f) => obj.set(key, Value::Float(*f)),
                PyVal::List(l) => {
                    let l: Vec<String> = l.iter().map(PyVal::py_str).collect();
                    obj.set_list(key, &l);
                }
                _ => {}
            }
        } else {
            // `set_parse(key, str(value))`.
            let _ = obj.set_parsed(key, &value.py_str());
        }
    }
}

fn doc_get<'a>(d: &'a Doc, key: &str) -> Option<&'a PyVal> {
    d.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn to_pyval(d: &Doc) -> PyVal {
    PyVal::Map(
        d.iter()
            .map(|(k, v)| (PyVal::Str(k.clone()), v.clone()))
            .collect(),
    )
}

/// `dump`.
pub fn dump(docs: &[Doc]) -> String {
    let docs: Vec<PyVal> = docs.iter().map(to_pyval).collect();
    pyyaml::dump_all(&docs)
}

/// `load`: the documents, each a mapping (keys made strings), or the
/// message for beets' `ParseError`.
pub fn load(s: &str) -> Result<Vec<Doc>, String> {
    let docs = pyyaml::load_all(s).map_err(|e| format!("invalid YAML: {e}"))?;
    let mut out = vec![];
    for d in docs {
        match d {
            PyVal::Map(pairs) => {
                out.push(pairs.into_iter().map(|(k, v)| (k.py_str(), v)).collect());
            }
            other => {
                return Err(format!(
                    "each entry must be a dictionary; found {}",
                    other.type_name()
                ));
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------- editing

/// `edit`: open `path` in the editor and wait for it.
fn run_editor(editor: &str, path: &Path, log: &PluginLog) -> Result<()> {
    let editor =
        crate::commands::editor_command(Some(editor.to_string()).filter(|e| !e.is_empty()));
    let mut cmd = crate::paths::shell_split(&editor);
    if cmd.is_empty() {
        cmd.push(editor.clone());
    }
    cmd.push(path.display().to_string());
    let repr: Vec<String> = cmd.iter().map(|a| crate::hooks::py_repr(a)).collect();
    log.debug(&format!("invoking editor command: [{}]", repr.join(", ")));
    if let Err(e) = std::process::Command::new(&cmd[0]).args(&cmd[1..]).status() {
        // Python's OSError text.
        let desc = e.to_string();
        let desc = desc.split(" (os error").next().unwrap_or(&desc).to_string();
        let errno = e.raw_os_error().unwrap_or(0);
        bail!(
            "could not run editor command {}: [Errno {errno}] {desc}: {}",
            crate::hooks::py_repr(&cmd[0]),
            crate::hooks::py_repr(&cmd[0])
        );
    }
    Ok(())
}

/// A fresh temporary file like Python's `NamedTemporaryFile(suffix=".yaml")`.
fn temp_yaml(content: &str) -> Result<PathBuf> {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64
        ^ (std::process::id() as u64) << 32;
    for _ in 0..100 {
        let mut name = String::from("tmp");
        for _ in 0..8 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let c = b"abcdefghijklmnopqrstuvwxyz0123456789_"[(seed >> 33) as usize % 37];
            name.push(c as char);
        }
        let p = dir.join(format!("{name}.yaml"));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            f.write_all(content.as_bytes())?;
            return Ok(p);
        }
    }
    bail!("could not create a temporary file")
}

pub struct Editor {
    pub cfg: EditCfg,
    pub log: PluginLog,
}

impl Editor {
    pub fn new(cfg: &Config) -> Self {
        Editor {
            cfg: EditCfg::new(cfg),
            log: PluginLog::new("edit"),
        }
    }

    /// `_edit_yaml`: let the user edit `old_str`; the documents and the
    /// new text, or None if they gave up (no changes, or unreadable and
    /// not fixed).
    pub fn edit_yaml(&self, old_str: &str) -> Result<Option<(Vec<Doc>, String)>> {
        let path = temp_yaml(old_str)?;
        let r = (|| -> Result<Option<(Vec<Doc>, String)>> {
            loop {
                run_editor(&self.cfg.editor, &path, &self.log)?;
                let new_str = std::fs::read_to_string(&path)?;
                if new_str == old_str {
                    println!("No changes; aborting.");
                    return Ok(None);
                }
                match load(&new_str) {
                    Ok(docs) => return Ok(Some((docs, new_str))),
                    Err(e) => {
                        println!("Could not read data: {e}");
                        if !ui::input_yn("Edit again to fix? (Y/n)", true) {
                            return Ok(None);
                        }
                    }
                }
            }
        })();
        let _ = std::fs::remove_file(&path);
        r
    }

    /// `apply_data`: apply the edited documents to the objects, matched by
    /// `id`; documents with a changed or duplicate id, or a changed
    /// ignored field, are left out.
    pub fn apply_data(&self, objs: &mut [Record], old_data: &[Doc], new_data: &[Doc]) {
        if old_data.len() != new_data.len() {
            self.log.warning(&format!(
                "number of objects changed from {} to {}",
                old_data.len(),
                new_data.len()
            ));
        }
        let id_of = |d: &Doc| doc_get(d, "id").cloned().unwrap_or(PyVal::None);
        let mut counts: HashMap<String, usize> = HashMap::new();
        for d in new_data {
            *counts.entry(id_of(d).py_repr()).or_default() += 1;
        }
        for new in new_data {
            let new_id = id_of(new);
            if counts[&new_id.py_repr()] > 1 {
                self.log.warning(&format!(
                    "ignoring objects with duplicate id {}",
                    new_id.py_str()
                ));
                continue;
            }
            let old = old_data
                .iter()
                .find(|d| id_of(d).py_eq(&new_id) && doc_get(d, "id").is_some());
            let obj_idx = objs
                .iter()
                .position(|o| o.id.is_some_and(|i| PyVal::Int(i as i128).py_eq(&new_id)));
            let (Some(old), Some(idx)) = (old, obj_idx) else {
                self.log.warning("ignoring object whose id changed");
                continue;
            };
            let forbidden = self.cfg.ignore_fields.iter().find(|key| {
                let (a, b) = (doc_get(old, key), doc_get(new, key));
                match (a, b) {
                    (Some(a), Some(b)) => !a.py_eq(b),
                    (None, None) => false,
                    _ => true,
                }
            });
            if let Some(key) = forbidden {
                self.log
                    .warning(&format!("ignoring object whose {key} changed"));
                continue;
            }
            apply_doc(&mut objs[idx], new);
        }
    }

    /// `edit_objects`: edit the objects until the user applies (true) or
    /// gives up (false). `olds` are the objects as stored, for the
    /// changes shown.
    fn edit_objects(
        &self,
        objs: &mut [Record],
        olds: &[Record],
        fields: Option<&BTreeSet<String>>,
        lib: &Library,
        fmt: &Template,
    ) -> Result<bool> {
        let old_data: Vec<Doc> = objs.iter().map(|o| flatten(o, fields, Some(lib))).collect();
        let mut cur_str = dump(&old_data);
        loop {
            let Some((new_data, new_str)) = self.edit_yaml(&cur_str)? else {
                return Ok(false);
            };
            self.apply_data(objs, &old_data, &new_data);
            let mut changed = false;
            for (obj, old) in objs.iter().zip(olds) {
                changed |= show_model_changes(&format_record(fmt, old, Some(lib)), old, obj);
            }
            if !changed {
                println!("No changes to apply.");
                return Ok(false);
            }
            match ui::prompt_options(
                &["continue Editing", "apply", "cancel"],
                false,
                None,
                None,
                None,
                None,
            ) {
                Some(Choice::Letter('a')) => return Ok(true),
                Some(Choice::Letter('e')) => {
                    self.apply_data(objs, &new_data, &old_data);
                    cur_str = new_str;
                }
                _ => {
                    self.apply_data(objs, &new_data, &old_data);
                    return Ok(false);
                }
            }
        }
    }

    /// `_get_fields`.
    fn get_fields(&self, album: bool, extra: &[String]) -> BTreeSet<String> {
        let mut fields: BTreeSet<String> = if album {
            self.cfg.albumfields.iter().cloned().collect()
        } else {
            self.cfg.itemfields.iter().cloned().collect()
        };
        fields.extend(extra.iter().cloned());
        fields.insert("id".into());
        fields
    }
}

/// `beet edit [-a] [-f FIELD]... [--all] [QUERY]`.
pub fn command(
    cfg: &Config,
    lib: &Library,
    album: bool,
    extra: &[String],
    all: bool,
    query: &[String],
) -> Result<()> {
    let ed = Editor::new(cfg);
    ed.log.set_base_level(crate::plugins::INFO);
    let kind = if album { Kind::Album } else { Kind::Item };
    let q = Query::parse(kind, query)?;
    let olds = if album {
        lib.albums(&q)?
    } else {
        lib.items(&q)?
    };
    if olds.is_empty() {
        println!("Nothing to edit.");
        return Ok(());
    }
    let fields = (!all).then(|| ed.get_fields(album, extra));
    let fmt = Template::parse(&cfg.str(if album { "format_album" } else { "format_item" }));
    let mut objs = olds.clone();
    if !ed.edit_objects(&mut objs, &olds, fields.as_ref(), lib, &fmt)? {
        return Ok(());
    }
    // `save_changes`: the changed ones, synced (beets' `try_sync`).
    let changed: Vec<(Record, Record)> = olds
        .into_iter()
        .zip(objs)
        .filter(|(o, n)| dirty(o, n))
        .collect();
    for (_, new) in &changed {
        ed.log.debug(&format!(
            "saving changes to {}",
            format_record(&fmt, new, Some(lib))
        ));
    }
    let write = cfg.bool("import.write");
    let mv = cfg.bool("import.move") || cfg.bool("import.copy");
    crate::commands::apply_changes(cfg, lib, album, changed, write, mv, true)
}

/// Whether any field (fixed or flexible) differs: beets' `_dirty`.
fn dirty(old: &Record, new: &Record) -> bool {
    let mut keys = old.keys();
    keys.extend(new.keys());
    keys.iter().any(|k| old.get(k) != new.get(k))
}

// ----------------------------------------------------------- importing

/// Fixed fields only items have (`Item._field_names - Album._field_names`).
fn item_only_fields() -> BTreeSet<&'static str> {
    let album: BTreeSet<&str> = schema(Kind::Album).iter().map(|(n, _)| *n).collect();
    schema(Kind::Item)
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !album.contains(n))
        .collect()
}

impl Editor {
    /// `_importer_edit_album_header`: the album-level document, from the
    /// first item.
    fn album_header(&self, task: &ChoiceTask) -> Option<Doc> {
        if !task.is_album || task.items.is_empty() {
            return None;
        }
        let mut album_fields: BTreeSet<String> = self.cfg.albumfields.iter().cloned().collect();
        if album_fields.is_empty() {
            return None;
        }
        let item_only = item_only_fields();
        let dropped: Vec<&String> = album_fields
            .iter()
            .filter(|f| item_only.contains(f.as_str()))
            .collect();
        if !dropped.is_empty() {
            let names: Vec<&str> = dropped.iter().map(|s| s.as_str()).collect();
            self.log.warning(&format!(
                "ignoring item-only fields configured in albumfields: {}",
                names.join(", ")
            ));
            album_fields.retain(|f| !item_only.contains(f.as_str()));
        }
        if album_fields.is_empty() {
            return None;
        }
        let header = flatten(&task.items[0], Some(&album_fields), None);
        (!header.is_empty()).then_some(header)
    }

    /// `importer_edit`: edit the task's items (their own tags, or a
    /// candidate's applied). True for beets' `Action.RETAG`.
    pub fn importer_edit(&self, task: &mut ChoiceTask) -> Result<bool> {
        // Negative ids for items not in the library yet.
        let mut temp_ids = vec![];
        for (i, obj) in task.items.iter_mut().enumerate() {
            if obj.id.is_none() {
                obj.id = Some(-(i as i64 + 1));
                temp_ids.push(i);
            }
        }
        let r = self.importer_edit_loop(task);
        for i in temp_ids {
            task.items[i].id = None;
        }
        r
    }

    fn importer_edit_loop(&self, task: &mut ChoiceTask) -> Result<bool> {
        let header = self.album_header(task);
        let header_fields: BTreeSet<String> = header
            .as_ref()
            .map(|h| h.iter().map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        let mut track_fields: BTreeSet<String> = self
            .cfg
            .itemfields
            .iter()
            .filter(|f| !header_fields.contains(*f))
            .cloned()
            .collect();
        track_fields.insert("id".into());
        let old_tracks: Vec<Doc> = task
            .items
            .iter()
            .map(|o| flatten(o, Some(&track_fields), None))
            .collect();
        let mut all_old = vec![];
        if let Some(h) = &header {
            all_old.push(h.clone());
        }
        all_old.extend(old_tracks.iter().cloned());
        let fmt = Template::parse(&task.cfg.str("format_item"));
        let mut cur_str = dump(&all_old);
        loop {
            let Some((new_all, new_str)) = self.edit_yaml(&cur_str)? else {
                return Ok(false);
            };
            let expected = header.is_some() as usize + task.items.len();
            if new_all.len() != expected {
                println!(
                    "Number of documents changed from {expected} to {}.",
                    new_all.len()
                );
                if ui::input_yn("Edit again to fix? (Y/n)", true) {
                    continue;
                }
                return Ok(false);
            }
            let (new_header, new_tracks): (Vec<Doc>, Vec<Doc>) = if header.is_some() {
                let (h, t): (Vec<Doc>, Vec<Doc>) = new_all
                    .into_iter()
                    .partition(|d| doc_get(d, "id").is_none());
                if h.len() != 1 {
                    println!(
                        "Could not identify the album header: exactly one document must have no `id` field."
                    );
                    if ui::input_yn("Edit again to fix? (Y/n)", true) {
                        continue;
                    }
                    return Ok(false);
                }
                (h, t)
            } else {
                (vec![], new_all)
            };
            let olds: Vec<Item> = task.items.clone();
            if let Some(h) = new_header.first() {
                for item in task.items.iter_mut() {
                    apply_doc(item, h);
                }
            }
            self.apply_data(task.items, &old_tracks, &new_tracks);
            let mut changed = false;
            for (item, old) in task.items.iter().zip(&olds) {
                changed |= show_model_changes(&format_record(&fmt, old, None), old, item);
            }
            if !changed {
                println!("No changes to apply.");
                return Ok(false);
            }
            match ui::prompt_options(
                &["continue Editing", "apply", "cancel"],
                false,
                None,
                None,
                None,
                None,
            ) {
                Some(Choice::Letter('a')) => return Ok(true),
                Some(Choice::Letter('e')) => {
                    restore_fixed(task.items, &olds);
                    cur_str = new_str;
                }
                _ => {
                    restore_fixed(task.items, &olds);
                    return Ok(false);
                }
            }
        }
    }

    /// `importer_edit_candidate`: apply the candidate on screen (or one
    /// picked from the list) to the items, then edit them.
    pub fn importer_edit_candidate(&self, task: &mut ChoiceTask) -> Result<bool> {
        if task.candidates == 0 {
            return Ok(false);
        }
        let n = match task.selected {
            Some(k) => k,
            None => match ui::prompt_options(
                &["Back"],
                false,
                None,
                None,
                Some((1, task.candidates)),
                None,
            ) {
                Some(Choice::Number(n)) => n - 1,
                _ => return Ok(false),
            },
        };
        (task.apply_candidate)(n, task.items);
        self.importer_edit(task)
    }
}

/// `_importer_edit_restore_from_copies`: every fixed field back (flexible
/// attributes the edit added stay, as in beets).
fn restore_fixed(items: &mut [Item], copies: &[Item]) {
    for (item, copy) in items.iter_mut().zip(copies) {
        for (name, _) in schema(item.kind) {
            if let Some(v) = copy.get(name) {
                item.set(name, v);
            }
        }
    }
}

pub struct EditPlugin {
    editor: Editor,
}

impl EditPlugin {
    pub fn new(cfg: &Config) -> Self {
        let editor = Editor::new(cfg);
        editor.log.set_base_level(WARNING);
        EditPlugin { editor }
    }
}

impl Plugin for EditPlugin {
    fn name(&self) -> &'static str {
        "edit"
    }

    /// `before_choose_candidate`: "eDit", and "edit Candidates" when
    /// there are candidates.
    fn prompt_choices(&self, task: &ChoiceTask) -> Vec<PromptChoice> {
        let mut c = vec![PromptChoice {
            short: 'd',
            long: "eDit",
        }];
        if task.candidates > 0 {
            c.push(PromptChoice {
                short: 'c',
                long: "edit Candidates",
            });
        }
        c
    }

    fn run_prompt_choice(&self, short: char, task: &mut ChoiceTask) -> Result<bool> {
        match short {
            'd' => self.editor.importer_edit(task),
            'c' => self.editor.importer_edit_candidate(task),
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "edit",
        order: 3,
        defaults: r#"edit:
    albumfields: album albumartist
    itemfields: track title artist album
    ignore_fields: id path
"#,
        config_keys: &["edit"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "edit",
                build: build_edit,
                run: run_edit,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(EditPlugin::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct EditArgs {
    /// Edit this field also
    #[arg(short = 'f', long = "field", value_name = "FIELD")]
    field: Vec<String>,
    /// Edit all fields
    #[arg(long)]
    all: bool,
    /// Match albums instead of tracks
    #[arg(short = 'a', long)]
    album: bool,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_edit() -> clap::Command {
    use clap::Args as _;
    EditArgs::augment_args(
        clap::Command::new("edit").about("interactively edit metadata"),
    )
}

pub fn run_edit(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = EditArgs::from_arg_matches(m)?;
    crate::plugins::edit::command(cfg, lib, a.album, &a.field, a.all, &a.query)
}
