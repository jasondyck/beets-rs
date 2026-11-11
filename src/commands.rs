//! Subcommand implementations.

use crate::config::{Config, DEFAULT_CONFIG, default_config_dir};
use crate::fileops::{self, FileOp};
use crate::importer::{self, ImportOptions, Importer, Resume};
use crate::library::Library;
use crate::logging::debug;
use crate::media;
use crate::model::{
    ALBUM_FIELDS, Album, ITEM_FIELDS, Item, Kind, Record, Value, album_item_shared_fields,
};
use crate::paths::PathFormatter;
use crate::plugins::{self, PluginCtx};
use crate::query::Query;
use crate::template::{Context, Template};
use crate::ui;
use crate::{Cli, Cmd};
use anyhow::{Context as _, Result, bail};

use serde_yaml::Value as Y;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn run(cli: Cli) -> Result<()> {
    crate::logging::set_verbosity(cli.verbose);
    let config_file = cli
        .config
        .clone()
        .unwrap_or_else(|| default_config_dir().join("config.yaml"));
    debug!("user configuration: {}", config_file.display());
    let mut cfg = Config::load(cli.config.as_deref())?;
    debug!("data directory: {}", cfg.config_dir.display());
    if let Some(l) = &cli.library {
        cfg.set("library", Y::String(l.to_string_lossy().into()));
    }
    if let Some(d) = &cli.directory {
        cfg.set("directory", Y::String(d.to_string_lossy().into()));
    }
    let csl = |s: &str| Y::Sequence(crate::comma_list(s).into_iter().map(Y::String).collect());
    if let Some(p) = &cli.plugins {
        cfg.set("plugins", csl(p));
    }
    if let Some(p) = &cli.disable_plugins {
        cfg.set("disabled_plugins", csl(p));
    }
    // As beets: `--version` runs the version command, and no command at
    // all shows the help.
    let cmd = match cli.cmd {
        _ if cli.version => Cmd::Version,
        Some(cmd) => cmd,
        None => Cmd::Help { command: None },
    };

    // Commands that don't need the library.
    match &cmd {
        Cmd::Help { command } => return help(command.as_deref()),
        Cmd::Version => return version(&cfg),
        Cmd::Fields => return fields(),
        Cmd::Config { paths, defaults } => {
            if *paths {
                println!(
                    "{}",
                    cli.config
                        .clone()
                        .unwrap_or_else(|| default_config_dir().join("config.yaml"))
                        .display()
                );
            } else if *defaults {
                print!("{}", DEFAULT_CONFIG.trim_start());
            } else {
                print!("{}", cfg.dump());
            }
            return Ok(());
        }
        _ => {}
    }

    let lib = Library::open(&cfg.library_path())?;
    debug!(
        "library database: {}\nlibrary directory: {}",
        cfg.library_path().display(),
        cfg.directory().display()
    );
    let plugins = plugins::load(&cfg)?;
    match cmd {
        Cmd::Import {
            copy,
            nocopy,
            mv,
            nomove,
            write,
            nowrite,
            quiet,
            quiet_fallback,
            log,
            timid,
            autotag,
            noautotag,
            singletons,
            group_albums,
            search_id,
            flat,
            pretend,
            set,
            incremental,
            noincremental,
            incremental_skip_later,
            noincremental_skip_later,
            resume,
            noresume,
            library,
            from_scratch,
            paths,
        } => {
            let mut opts = ImportOptions::from_config(&cfg);
            // As beets: -c/-C and -m/-M override `copy` and `move`
            // separately, -c also turns move off, and move wins over copy.
            let mut do_copy = cfg.bool("import.copy");
            let mut do_move = cfg.bool("import.move");
            if copy {
                do_copy = true;
                do_move = false;
            }
            if nocopy {
                do_copy = false;
            }
            if mv {
                do_move = true;
            }
            if nomove {
                do_move = false;
            }
            opts.op = FileOp::for_import(&cfg, do_copy, do_move);
            if write {
                opts.write = true;
            }
            if nowrite {
                opts.write = false;
            }
            opts.quiet |= quiet;
            if let Some(f) = quiet_fallback {
                opts.quiet_fallback_asis = f == "asis";
            }
            if log.is_some() {
                opts.log = log;
            }
            opts.timid |= timid;
            if autotag {
                opts.autotag = true;
            }
            if noautotag {
                opts.autotag = false;
            }
            opts.singletons |= singletons;
            opts.group_albums |= group_albums;
            opts.flat |= flat;
            opts.search_ids = search_id;
            opts.pretend |= pretend;
            for kv in &set {
                // `--set` pairs override `import.set_fields` entries of the same name.
                let (k, v) = importer::parse_set_field(kv)?;
                opts.set_fields.retain(|(f, _)| f != k);
                opts.set_fields.push((k.to_string(), v.to_string()));
            }
            opts.from_scratch |= from_scratch;
            if library {
                // As beets: a query import never resumes or records history.
                opts.resume = Resume::No;
                opts.incremental = false;
            } else if paths.is_empty() {
                bail!("no path specified");
            }
            if incremental_skip_later {
                opts.incremental_skip_later = true;
            }
            if noincremental_skip_later {
                opts.incremental_skip_later = false;
            }
            if incremental {
                opts.incremental = true;
            }
            if noincremental {
                opts.incremental = false;
            }
            if resume {
                opts.resume = Resume::Yes;
            }
            if noresume {
                opts.resume = Resume::No;
            }
            if opts.quiet && opts.timid {
                bail!("can't be both quiet and timid");
            }
            let pretend = opts.pretend;
            let mut imp = Importer::new(&cfg, &lib, opts, &plugins)?;
            if library {
                let query: Vec<String> = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                imp.run_library(&query)?;
            } else {
                imp.run(&paths)?;
            }
            if pretend {
                return Ok(());
            }
            let s = &imp.stats;
            println!();
            let already = if s.already > 0 {
                format!("; {} already imported earlier", s.already)
            } else {
                String::new()
            };
            println!(
                "Imported {} album(s), {} item(s); skipped {}{already}.",
                s.albums, s.items, s.skipped
            );
            Ok(())
        }
        Cmd::List {
            album,
            path,
            format,
            query,
        } => list(&cfg, &lib, album, path, format, &query),
        Cmd::Modify {
            album,
            mv,
            nomove,
            write,
            nowrite,
            format,
            yes,
            noinherit,
            limit,
            args,
        } => {
            let flag = |on: bool, off: bool| (on || off).then_some(on);
            let opts = ModifyOpts {
                album,
                yes,
                mv: flag(mv, nomove),
                write: flag(write, nowrite),
                inherit: !noinherit,
                format,
                limit,
            };
            modify(&cfg, &lib, opts, &args)
        }
        Cmd::Remove {
            album,
            delete,
            force,
            query,
        } => remove(&cfg, &lib, album, delete, force, &query),
        Cmd::Move {
            album,
            dest,
            copy,
            pretend,
            timid,
            export,
            limit,
            query,
        } => {
            let opts = MoveOpts {
                album,
                dest,
                copy,
                export,
                pretend,
                timid,
                limit,
            };
            move_cmd(&cfg, &lib, &opts, &query)
        }
        Cmd::Update {
            pretend,
            nomove,
            query,
        } => update(&cfg, &lib, pretend, !nomove, &query),
        Cmd::Write { pretend, query } => write(&lib, pretend, &query),
        Cmd::Stats { exact, query } => stats(&lib, exact, &query),
        Cmd::Fetchart { force, query } => fetchart(&cfg, &lib, force, &query),
        Cmd::Embedart { file, yes, query } => embedart(&cfg, &lib, file, yes, &query),
        Cmd::Extractart {
            output,
            name,
            associate,
            query,
        } => extractart(&cfg, &lib, output, name, associate, &query),
        Cmd::Clearart { yes, query } => clearart(&cfg, &lib, yes, &query),
        Cmd::Lyrics {
            print,
            force,
            query,
        } => lyrics(&cfg, &lib, print, force, &query),
        Cmd::Fields | Cmd::Config { .. } | Cmd::Version | Cmd::Help { .. } => unreachable!(),
    }
}

struct RecCtx<'a>(&'a Record);
impl Context for RecCtx<'_> {
    fn field(&self, name: &str) -> Option<String> {
        self.0.formatted(name)
    }
}

/// `beet help [COMMAND]`: the main help, or a command's (by name or alias),
/// exactly as `beet COMMAND --help` shows it.
fn help(command: Option<&str>) -> Result<()> {
    use clap::CommandFactory;
    let Some(name) = command else {
        crate::Cli::command().print_help()?;
        return Ok(());
    };
    match crate::parse_cli(["beet", name, "--help"]) {
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            e.print()?;
            Ok(())
        }
        _ => bail!("unknown command '{name}'"),
    }
}

/// `beet version`: our version, the compiler's (where beets shows Python's)
/// and the loaded plugins.
fn version(cfg: &Config) -> Result<()> {
    for line in version_lines(cfg)? {
        println!("{line}");
    }
    Ok(())
}

fn version_lines(cfg: &Config) -> Result<Vec<String>> {
    let mut names: Vec<&str> = plugins::load(cfg)?.iter().map(|p| p.name()).collect();
    names.sort_unstable();
    Ok(vec![
        format!("beets-rs version {}", env!("CARGO_PKG_VERSION")),
        format!("Rust version {}", env!("BEETS_RS_RUSTC_VERSION")),
        if names.is_empty() {
            "no plugins loaded".into()
        } else {
            format!("plugins: {}", names.join(", "))
        },
    ])
}

pub fn format_record(t: &Template, r: &Record) -> String {
    t.render(&RecCtx(r))
}

fn list(
    cfg: &Config,
    lib: &Library,
    album: bool,
    path: bool,
    format: Option<String>,
    q: &[String],
) -> Result<()> {
    let kind = if album { Kind::Album } else { Kind::Item };
    let query = Query::parse(kind, q)?;
    let fmt = match (format, path, album) {
        (Some(f), _, _) => f,
        (None, true, false) => "$path".into(),
        (None, true, true) => "$path".into(),
        (None, false, false) => cfg.str("format_item"),
        (None, false, true) => cfg.str("format_album"),
    };
    let t = Template::parse(&fmt);
    if album {
        for a in lib.albums(&query)? {
            if path && fmt == "$path" {
                // An album's path is the directory of its items.
                let items = lib.album_items(a.id.unwrap())?;
                if let Some(dir) = items.first().and_then(|i| i.path().parent()) {
                    println!("{}", dir.display());
                }
            } else {
                println!("{}", format_record(&t, &a));
            }
        }
    } else {
        for i in lib.items(&query)? {
            println!("{}", format_record(&t, &i));
        }
    }
    Ok(())
}

/// Split modify arguments into a query, assignments and deletions.
/// One `field=value` assignment of `modify`; `op` is beets' `+`/`-` (from
/// `field+=value` / `field-=value`), only valid for multi-valued fields.
#[derive(Debug, Clone, PartialEq)]
struct Modification {
    field: String,
    op: Option<char>,
    value: String,
}

/// beets' `modify_parse_args`: split arguments into query terms,
/// assignments (`field=value`, no `:` before the `=`) and deletions
/// (`field!`, with no `=` or `:`). A later assignment to the same field
/// replaces an earlier one.
fn parse_modify_args(args: &[String]) -> (Vec<String>, Vec<Modification>, Vec<String>) {
    let (mut query, mut mods, mut dels) = (vec![], Vec::<Modification>::new(), vec![]);
    for arg in args {
        if arg.ends_with('!') && !arg.contains('=') && !arg.contains(':') {
            dels.push(arg[..arg.len() - 1].to_string());
        } else if let Some((key, value)) = arg.split_once('=')
            && !key.contains(':')
        {
            let (field, op) = match key.chars().last() {
                Some(c @ ('+' | '-')) => (&key[..key.len() - 1], Some(c)),
                _ => (key, None),
            };
            let m = Modification {
                field: field.to_string(),
                op,
                value: value.to_string(),
            };
            match mods.iter_mut().find(|x| x.field == m.field) {
                Some(old) => *old = m,
                None => mods.push(m),
            }
        } else {
            query.push(arg.clone());
        }
    }
    (query, mods, dels)
}

/// beets' `_field_diff`: a line showing how `field` changed, or None.
fn field_diff(field: &str, old: &Record, new: &Record) -> Option<String> {
    let (oldval, newval) = (old.get(field), new.get(field));
    if oldval == newval {
        return None;
    }
    if let (Some(Value::Float(a)), Some(Value::Float(b))) = (&oldval, &newval)
        && (a - b).abs() < 0.01
    {
        return None;
    }
    let oldstr = old.formatted(field).unwrap_or_default();
    let newstr = new.formatted(field).unwrap_or_default();
    if newval.is_none() {
        return Some(crate::diff::removed(&format!("{field}: {oldstr}")));
    }
    let Some(oldval) = oldval else {
        return Some(crate::diff::added(&format!("{field}: {newstr}")));
    };
    let (a, b) = if matches!(oldval, Value::Str(_)) {
        crate::diff::colordiff(&oldstr, &newstr)
    } else {
        (crate::diff::removed(&oldstr), crate::diff::added(&newstr))
    };
    Some(format!("{field}: {a} -> {b}"))
}

/// beets' `get_model_changes`: diff lines for every changed field (fixed
/// and flexible, except `mtime`), sorted by field name.
fn model_changes(old: &Record, new: &Record) -> Vec<String> {
    let mut fields: Vec<String> = old.keys();
    for k in new.keys() {
        if !fields.contains(&k) {
            fields.push(k);
        }
    }
    fields.retain(|f| f != "mtime");
    fields.sort();
    fields
        .iter()
        .filter_map(|f| field_diff(f, old, new))
        .collect()
}

/// beets' `show_model_changes`: print `label` (the object, formatted as
/// it was) and its changes, if there are any. Returns whether there were.
fn show_model_changes(label: &str, old: &Record, new: &Record) -> bool {
    let changes = model_changes(old, new);
    if !changes.is_empty() {
        println!("{label}");
        for c in &changes {
            println!("  {}", c.replace('\n', "\n  "));
        }
    }
    !changes.is_empty()
}

/// Move an item's file to its library destination (`modify`/`update`,
/// beets' `Item.move`).
fn move_item(fmt: &PathFormatter, lib: &Library, cfg: &Config, item: &mut Item) -> Result<bool> {
    let album = lib.get_album(item.int("album_id"))?;
    let dest = fmt.destination(item, album.as_ref(), Some(lib));
    move_item_file(cfg, item, &dest, FileOp::Move)
}

/// `beet modify` options besides the arguments.
pub struct ModifyOpts {
    pub album: bool,
    pub yes: bool,
    /// -m/-M; default `import.move` or `import.copy` (beets' `should_move`).
    pub mv: Option<bool>,
    /// -w/-W; default `import.write`.
    pub write: Option<bool>,
    /// Album changes also go to the album's items (off with -I).
    pub inherit: bool,
    /// -f: `format_item` for this run.
    pub format: Option<String>,
    pub limit: Option<usize>,
}

/// Apply `mods` and `dels` to a copy of `obj`: values are templates
/// evaluated against the object, then parsed for the field's type.
fn modified(obj: &Record, mods: &[Modification], dels: &[String]) -> Result<Record> {
    let mut new = obj.clone();
    for m in mods {
        let v = format_record(&Template::parse(&m.value), obj);
        new.set_parsed(&m.field, &v)?;
    }
    for f in dels {
        // As beets: a flexible attribute goes, a fixed field is reset to
        // its type's null; computed or unknown fields are left alone.
        if !new.remove_flex(f)
            && f != "id"
            && let Some(ty) = new.field_type(f)
        {
            new.set(f, ty.default_value());
        }
    }
    Ok(new)
}

/// beets' `modify_func` / `modify_objects`.
fn modify(cfg: &Config, lib: &Library, opts: ModifyOpts, args: &[String]) -> Result<()> {
    let (q, mods, dels) = parse_modify_args(args);
    if mods.is_empty() && dels.is_empty() {
        bail!("no modifications specified");
    }
    let album = opts.album;
    let kind = if album { Kind::Album } else { Kind::Item };
    let mut objs = if album {
        lib.albums(&Query::parse(kind, &q)?)?
    } else {
        lib.items(&Query::parse(kind, &q)?)?
    };
    if let Some(n) = opts.limit {
        objs.truncate(n);
    }
    // No multi-valued fields yet, so no field takes `+=`/`-=`.
    if let Some(m) = mods.iter().find(|m| m.op.is_some()) {
        bail!(
            "field '{}' does not support the {}= operator",
            m.field,
            m.op.unwrap()
        );
    }
    if objs.is_empty() {
        bail!("No matching objects to modify.");
    }
    let t = Template::parse(&match (&opts.format, album) {
        (_, true) => cfg.str("format_album"),
        (Some(f), false) => f.clone(),
        (None, false) => cfg.str("format_item"),
    });
    println!(
        "Modifying {} {}s.",
        objs.len(),
        if album { "album" } else { "item" }
    );
    let mut changed: Vec<(Record, Record)> = vec![];
    for obj in objs {
        let new = modified(&obj, &mods, &dels)?;
        if show_model_changes(&format_record(&t, &obj), &obj, &new) {
            changed.push((obj, new));
        }
    }
    if changed.is_empty() {
        println!("No changes to make.");
        return Ok(());
    }

    let write = opts.write.unwrap_or_else(|| cfg.bool("import.write"));
    let mv = opts
        .mv
        .unwrap_or_else(|| cfg.bool("import.move") || cfg.bool("import.copy"));
    let selected = if opts.yes {
        changed
    } else {
        let extra = match (write, mv) {
            (true, true) => ", move and write tags",
            (true, false) => " and write tags",
            (false, true) => " and move",
            (false, false) => "",
        };
        ui::select_objects(&format!("Really modify{extra}"), &changed, |(old, new)| {
            show_model_changes(&format_record(&t, old), old, new);
        })
    };

    let fmt = PathFormatter::new(cfg)?;
    let mut albums_touched: HashSet<i64> = HashSet::new();
    // beets' `Item.try_sync`: write tags, move files inside the library,
    // store.
    let sync = |lib: &Library, item: &mut Item, touched: &mut HashSet<i64>| -> Result<()> {
        if write && let Err(e) = media::write_item(item) {
            eprintln!("warning: {e:#}");
        }
        if mv
            && item.path().starts_with(cfg.directory())
            && let Err(e) = move_item(&fmt, lib, cfg, item)
        {
            eprintln!("warning: {e:#}");
        }
        touched.insert(item.int("album_id"));
        lib.store(item)
    };
    lib.transaction(|lib| {
        for (old, mut new) in selected {
            if !album {
                sync(lib, &mut new, &mut albums_touched)?;
                continue;
            }
            lib.store(&new)?;
            let mut items = lib.album_items(new.id.unwrap())?;
            if opts.inherit {
                // beets' `Album.store(inherit=True)`: changed fixed fields
                // (but `artpath`/`id`) and flexible attributes, set or
                // deleted, go to the items too.
                for f in model_changes_fields(&old, &new) {
                    for item in &mut items {
                        match new.get(&f) {
                            Some(v) => item.set(&f, v),
                            None => {
                                item.remove_flex(&f);
                            }
                        }
                    }
                }
            }
            for item in &mut items {
                sync(lib, item, &mut albums_touched)?;
            }
        }
        Ok(())
    })?;
    if mv {
        for id in albums_touched {
            sync_art(cfg, lib, id)?;
        }
    }
    Ok(())
}

/// The album fields an inheriting store passes to the items: every changed
/// field except `artpath` and `id`.
fn model_changes_fields(old: &Album, new: &Album) -> Vec<String> {
    let mut fields = old.keys();
    for k in new.keys() {
        if !fields.contains(&k) {
            fields.push(k);
        }
    }
    fields
        .into_iter()
        .filter(|f| f != "artpath" && f != "id" && old.get(f) != new.get(f))
        .collect()
}

fn remove(
    cfg: &Config,
    lib: &Library,
    album: bool,
    delete: bool,
    force: bool,
    q: &[String],
) -> Result<()> {
    let (albums, items): (Vec<Album>, Vec<Item>) = if album {
        let albums = lib.albums(&Query::parse(Kind::Album, q)?)?;
        let mut items = vec![];
        for a in &albums {
            items.extend(lib.album_items(a.id.unwrap())?);
        }
        (albums, items)
    } else {
        (vec![], lib.items(&Query::parse(Kind::Item, q)?)?)
    };
    if items.is_empty() && albums.is_empty() {
        println!("No matching items.");
        return Ok(());
    }
    let t = Template::parse(&cfg.str("format_item"));
    for i in &items {
        println!("{}", format_record(&t, i));
    }
    let verb = if delete {
        "Really DELETE"
    } else {
        "Really remove"
    };
    if !force && !ui::yes_no(&format!("{verb} {} item(s) from the library", items.len())) {
        return Ok(());
    }
    let libdir = cfg.directory();
    let clutter = fileops::clutter(cfg);
    lib.transaction(|lib| {
        let mut touched_albums: HashSet<i64> = HashSet::new();
        for i in &items {
            if delete {
                if let Err(e) = std::fs::remove_file(i.path()) {
                    eprintln!("warning: {}: {e}", i.path().display());
                }
                if let Some(p) = i.path().parent() {
                    fileops::prune_dirs(p, &libdir, &clutter);
                }
            }
            touched_albums.insert(i.int("album_id"));
            lib.remove(i)?;
        }
        let delete_art = |a: &Album| {
            let art = a.path_field("artpath");
            if delete && art.is_file() {
                std::fs::remove_file(art).ok();
                if let Some(p) = art.parent() {
                    fileops::prune_dirs(p, &libdir, &clutter);
                }
            }
        };
        for a in &albums {
            delete_art(a);
            lib.remove(a)?;
        }
        // Drop albums left with no items.
        for id in touched_albums {
            if let Some(a) = lib.get_album(id)?
                && lib.album_items(id)?.is_empty()
            {
                delete_art(&a);
                lib.remove(&a)?;
            }
        }
        Ok(())
    })
}

/// `beet move` options besides the query.
pub struct MoveOpts {
    pub album: bool,
    /// -d: base directory instead of the library directory.
    pub dest: Option<PathBuf>,
    /// -c: copy, and point the library at the copies.
    pub copy: bool,
    /// -e: copy, leaving the library pointing at the originals.
    pub export: bool,
    pub pretend: bool,
    /// -t: ask before acting.
    pub timid: bool,
    pub limit: Option<usize>,
}

/// One object `beet move` acts on: an item (with its album, for the art)
/// or, with -a, an album and all its items; `dests` are the items'
/// destinations.
#[derive(Clone)]
struct MoveObj {
    album: Option<Album>,
    items: Vec<Item>,
    dests: Vec<PathBuf>,
}

impl MoveObj {
    /// beets' `isitemmoved` / `isalbummoved`.
    fn needs_moving(&self) -> bool {
        self.items
            .iter()
            .zip(&self.dests)
            .any(|(i, d)| i.path() != d)
    }

    fn path_changes(&self) -> Vec<(PathBuf, PathBuf)> {
        self.items
            .iter()
            .zip(&self.dests)
            .map(|(i, d)| (i.path().to_path_buf(), d.clone()))
            .collect()
    }
}

/// beets' `show_path_changes`: "old -> new" per line under a
/// "Source -> Destination" header, or over two lines each when the paths
/// don't fit in half the terminal.
fn show_path_changes(cfg: &Config, changes: &[(PathBuf, PathBuf)]) {
    let width = cfg
        .get("ui.terminal_width")
        .and_then(Y::as_u64)
        .map_or(80, |n| n as usize);
    for line in path_changes_lines(ui::term_width(width), changes) {
        println!("{line}");
    }
}

fn path_changes_lines(term_width: usize, changes: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let strs: Vec<(String, String)> = changes
        .iter()
        .map(|(a, b)| (a.display().to_string(), b.display().to_string()))
        .collect();
    let len = |s: &String| s.chars().count();
    let col_width = term_width.saturating_sub(" -> ".len()) / 2;
    let max_width = strs
        .iter()
        .flat_map(|(a, b)| [len(a), len(b)])
        .max()
        .unwrap_or(0);
    let mut out = vec![];
    if max_width > col_width {
        for (a, b) in &strs {
            let (ca, cb) = crate::diff::colordiff(a, b);
            out.push(format!("{ca} \n  -> {cb}"));
        }
    } else {
        // Python's `" " * n` is empty for negative n.
        let title_pad = (max_width + " -> ".len()).saturating_sub("Source ".len());
        out.push(format!("Source {} Destination", " ".repeat(title_pad)));
        for (a, b) in &strs {
            let (ca, cb) = crate::diff::colordiff(a, b);
            out.push(format!("{ca} {} -> {cb}", " ".repeat(max_width - len(a))));
        }
    }
    out
}

/// Where `beet move` puts an item: its destination, made unique if another
/// file has it (the item's own file doesn't count, so a repeated move is
/// a no-op).
fn item_destination(
    fmt: &PathFormatter,
    lib: &Library,
    item: &Item,
    album: Option<&Album>,
) -> PathBuf {
    let d = fmt.destination(item, album, Some(lib));
    fileops::unique_path_for(&d, Some(item.path()))
}

/// beets' `move_func` / `move_objects`.
fn move_cmd(cfg: &Config, lib: &Library, opts: &MoveOpts, q: &[String]) -> Result<()> {
    let dest = match &opts.dest {
        Some(d) => {
            let d = std::path::absolute(crate::config::expand_tilde(&d.to_string_lossy()))?;
            if !d.is_dir() {
                bail!("no such directory: {}", d.display());
            }
            Some(d)
        }
        None => None,
    };
    let mut dcfg = cfg.clone();
    if let Some(d) = &dest {
        dcfg.set("directory", Y::String(d.to_string_lossy().into()));
    }
    let fmt = PathFormatter::new(&dcfg)?;
    let entity = if opts.album { "album" } else { "item" };
    let mut all: Vec<MoveObj> = vec![];
    if opts.album {
        let mut albums = lib.albums(&Query::parse(Kind::Album, q)?)?;
        albums.truncate(opts.limit.unwrap_or(usize::MAX));
        for a in albums {
            let items = lib.album_items(a.id.unwrap())?;
            let dests = items
                .iter()
                .map(|i| item_destination(&fmt, lib, i, Some(&a)))
                .collect();
            all.push(MoveObj {
                album: Some(a),
                items,
                dests,
            });
        }
    } else {
        let mut items = lib.items(&Query::parse(Kind::Item, q)?)?;
        items.truncate(opts.limit.unwrap_or(usize::MAX));
        for i in items {
            let album = lib.get_album(i.int("album_id"))?;
            let d = item_destination(&fmt, lib, &i, album.as_ref());
            all.push(MoveObj {
                album,
                items: vec![i],
                dests: vec![d],
            });
        }
    }
    if all.is_empty() {
        bail!("No matching {entity}s found.");
    }
    let total = all.len();
    let objs: Vec<MoveObj> = all.into_iter().filter(MoveObj::needs_moving).collect();
    let unmoved = total - objs.len();
    let unmoved_msg = if unmoved > 0 {
        format!(" ({unmoved} already in place)")
    } else {
        String::new()
    };
    // Exporting always copies.
    let copy = opts.copy || opts.export;
    let (action, act) = if copy {
        ("Copying", "copy")
    } else {
        ("Moving", "move")
    };
    let n = objs.len();
    // beets logs this at INFO, which goes to stderr.
    eprintln!(
        "{action} {n} {entity}{}{unmoved_msg}.",
        if n == 1 { "" } else { "s" }
    );
    if objs.is_empty() {
        return Ok(());
    }
    if opts.pretend {
        let changes: Vec<_> = objs.iter().flat_map(MoveObj::path_changes).collect();
        show_path_changes(cfg, &changes);
        return Ok(());
    }
    let selected = if opts.timid {
        ui::select_objects(&format!("Really {act}"), &objs, |o| {
            show_path_changes(cfg, &o.path_changes())
        })
    } else {
        objs
    };
    let op = if copy { FileOp::Copy } else { FileOp::Move };
    let store = !opts.export;
    for obj in selected {
        if let Err(e) = move_obj(cfg, lib, obj, op, store, opts.album) {
            eprintln!("error: {e:#}");
        }
    }
    Ok(())
}

/// beets' `Item.move` (one item, then its album's art) or `Album.move`
/// (all items, then the art), with `store` false for -e.
fn move_obj(
    cfg: &Config,
    lib: &Library,
    obj: MoveObj,
    op: FileOp,
    store: bool,
    album_mode: bool,
) -> Result<()> {
    let MoveObj {
        album,
        items,
        dests,
    } = obj;
    let mut moved_item_dir: Option<PathBuf> = None;
    for (mut item, dest) in items.into_iter().zip(dests) {
        debug!("moving: {}", item.path().display());
        let old = item.path().to_path_buf();
        if !move_item_file(cfg, &mut item, &dest, op)? {
            continue;
        }
        if store {
            lib.store(&item)?;
        }
        if moved_item_dir.is_none() && item.path() != old {
            moved_item_dir = item.path().parent().map(PathBuf::from);
        }
    }
    // Items re-read their album: an earlier item may have moved the art.
    let album = match album {
        Some(a) if !album_mode => lib.get_album(a.id.unwrap())?,
        a => a,
    };
    let Some(mut album) = album else {
        return Ok(());
    };
    // `Album.move` puts the art next to the first moved item; `Item.move`
    // next to the album's first item as stored, so an item export (not
    // stored) leaves the art alone.
    let item_dir = if album_mode { moved_item_dir } else { None };
    if move_art(cfg, lib, &mut album, op, item_dir)? && store {
        lib.store(&album)?;
    }
    Ok(())
}

/// beets' `Item.move` for the file: copy or move it to `dest` (made unique
/// if taken by another file), set the path and prune the vacated directory after a move.
/// False when the file is missing.
fn move_item_file(cfg: &Config, item: &mut Item, dest: &Path, op: FileOp) -> Result<bool> {
    let src = item.path().to_path_buf();
    if !src.exists() {
        let what = if op == FileOp::Move {
            "Moving"
        } else {
            "Copying"
        };
        eprintln!("{what}: file not found at {}, skipping", src.display());
        return Ok(false);
    }
    let dest = fileops::unique_path_for(dest, Some(&src));
    if src == dest {
        return Ok(true);
    }
    fileops::transfer(op, &src, &dest)?;
    item.set_path(dest);
    if op == FileOp::Move
        && let Some(p) = src.parent()
    {
        fileops::prune_dirs(p, &cfg.directory(), &fileops::clutter(cfg));
    }
    Ok(true)
}

fn update(cfg: &Config, lib: &Library, pretend: bool, mv: bool, q: &[String]) -> Result<()> {
    let items = lib.items(&Query::parse(Kind::Item, q)?)?;
    let t = Template::parse(&cfg.str("format_item"));
    let fmt = PathFormatter::new(cfg)?;
    let mut touched_albums: HashSet<i64> = HashSet::new();
    let (mut removed, mut updated) = (0, 0);
    for item in items {
        let path = item.path().to_path_buf();
        if !path.exists() {
            println!("{} {}", ui::red("deleting"), format_record(&t, &item));
            if !pretend {
                lib.remove(&item)?;
                touched_albums.insert(item.int("album_id"));
            }
            removed += 1;
            continue;
        }
        let mtime = std::fs::metadata(&path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        if (mtime - item.float("mtime")).abs() < 0.001 {
            continue;
        }
        let fresh = match media::read(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("warning: {e:#}");
                continue;
            }
        };
        let mut new = item.clone();
        let props = [
            "length",
            "bitrate",
            "samplerate",
            "bitdepth",
            "channels",
            "format",
            "mtime",
        ];
        for name in media::TAG_FIELDS.iter().chain(props.iter()) {
            if let Some(v) = fresh.get(name) {
                new.set(name, v);
            }
        }
        if show_model_changes(&format_record(&t, &item), &item, &new) {
            updated += 1;
        }
        if !pretend {
            if mv
                && new.path().starts_with(cfg.directory())
                && let Err(e) = move_item(&fmt, lib, cfg, &mut new)
            {
                eprintln!("warning: {e:#}");
            }
            lib.store(&new)?;
            touched_albums.insert(new.int("album_id"));
            if mv {
                sync_art(cfg, lib, new.int("album_id"))?;
            }
        }
    }
    // Refresh album-level fields from their items, and drop empty albums.
    for id in touched_albums {
        if let Some(mut a) = lib.get_album(id)? {
            let items = lib.album_items(id)?;
            match items.first() {
                None => lib.remove(&a)?,
                Some(first) => {
                    for f in album_item_shared_fields() {
                        if let Some(v) = first.get(f) {
                            a.set(f, v);
                        }
                    }
                    lib.store(&a)?;
                }
            }
        }
    }
    println!("Updated {updated} item(s), removed {removed}.");
    Ok(())
}

/// Move an album's art next to its (stored) items, if it lives elsewhere.
pub(crate) fn sync_art(cfg: &Config, lib: &Library, album_id: i64) -> Result<()> {
    let Some(mut album) = lib.get_album(album_id)? else {
        return Ok(());
    };
    if move_art(cfg, lib, &mut album, FileOp::Move, None)? {
        lib.store(&album)?;
    }
    Ok(())
}

/// beets' `Album.move_art`: move or copy the art into `item_dir` (default:
/// the directory of the album's first stored item), under a unique name.
/// A reference to a missing art file is dropped. True if `artpath`
/// changed (the caller stores the album).
fn move_art(
    cfg: &Config,
    lib: &Library,
    album: &mut Album,
    op: FileOp,
    item_dir: Option<PathBuf>,
) -> Result<bool> {
    let art = album.path_field("artpath").to_path_buf();
    if art.as_os_str().is_empty() {
        return Ok(false);
    }
    if !art.exists() {
        eprintln!(
            "removing reference to missing album art file {}",
            art.display()
        );
        album.set("artpath", crate::model::Value::Path(PathBuf::new()));
        return Ok(true);
    }
    let dir = match item_dir {
        Some(d) => d,
        None => match plugins::fetchart::album_dir(&lib.album_items(album.id.unwrap())?) {
            Some(d) => d,
            None => return Ok(false),
        },
    };
    let dest = dir.join(art.file_name().unwrap_or_default());
    if dest == art {
        return Ok(false);
    }
    let dest = fileops::unique_path(&dest);
    debug!("moving album art {} to {}", art.display(), dest.display());
    fileops::transfer(op, &art, &dest)?;
    if op == FileOp::Move
        && let Some(p) = art.parent()
    {
        fileops::prune_dirs(p, &cfg.directory(), &fileops::clutter(cfg));
    }
    album.set("artpath", crate::model::Value::Path(dest));
    Ok(true)
}

fn require_plugin(cfg: &Config, name: &str) -> Result<()> {
    if !plugins::enabled(cfg, name) {
        bail!("the {name} plugin is not enabled; add it to `plugins:` in your config");
    }
    Ok(())
}

fn fetchart(cfg: &Config, lib: &Library, force: bool, q: &[String]) -> Result<()> {
    require_plugin(cfg, "fetchart")?;
    let fa = plugins::fetchart::FetchArt::new(cfg);
    let ctx = PluginCtx {
        cfg,
        lib,
        write: cfg.bool("import.write"),
    };
    let t = Template::parse(&cfg.str("format_album"));
    for mut album in lib.albums(&Query::parse(Kind::Album, q)?)? {
        let items = lib.album_items(album.id.unwrap())?;
        let label = format_record(&t, &album);
        match fa.fetch_for_album(&ctx, &mut album, &items, None, FileOp::Copy, force) {
            Ok(Some(p)) => println!("{label}: found art: {}", p.display()),
            Ok(None) if !force && album.path_field("artpath").is_file() => {
                println!("{label}: has album art")
            }
            Ok(None) => println!("{label}: no art found"),
            Err(e) => eprintln!("warning: {label}: {e:#}"),
        }
    }
    Ok(())
}

fn embedart(
    cfg: &Config,
    lib: &Library,
    file: Option<PathBuf>,
    yes: bool,
    q: &[String],
) -> Result<()> {
    require_plugin(cfg, "embedart")?;
    let ea = plugins::embedart::EmbedArt::new(cfg);
    let ctx = PluginCtx {
        cfg,
        lib,
        write: true,
    };
    let items = lib.items(&Query::parse(Kind::Item, q)?)?;
    if items.is_empty() {
        println!("No matching items.");
        return Ok(());
    }
    let what = match &file {
        Some(f) => format!("Embed {} into {} file(s)", f.display(), items.len()),
        None => format!("Embed album art into {} file(s)", items.len()),
    };
    if !yes && !ui::yes_no(&what) {
        return Ok(());
    }
    let mut n = 0;
    match file {
        Some(f) => {
            let image = std::fs::read(&f).with_context(|| format!("reading {}", f.display()))?;
            for mut item in items {
                match ea.embed_item(&ctx, &mut item, &image) {
                    Ok(true) => n += 1,
                    Ok(false) => {}
                    Err(e) => eprintln!("warning: {}: {e:#}", item.path().display()),
                }
            }
        }
        None => {
            // Group by album so each album's art file is read once.
            let mut by_album: HashMap<i64, Vec<Item>> = HashMap::new();
            for item in items {
                by_album.entry(item.int("album_id")).or_default().push(item);
            }
            for (id, mut items) in by_album {
                let Some(mut album) = lib.get_album(id)? else {
                    continue;
                };
                if !album.path_field("artpath").is_file() {
                    let t = Template::parse(&cfg.str("format_album"));
                    println!("{}: no album art file", format_record(&t, &album));
                    continue;
                }
                n += ea.embed_album(&ctx, &mut album, &mut items)?;
            }
        }
    }
    println!("Embedded art in {n} file(s).");
    Ok(())
}

fn extractart(
    cfg: &Config,
    lib: &Library,
    output: Option<PathBuf>,
    name: Option<String>,
    associate: bool,
    q: &[String],
) -> Result<()> {
    require_plugin(cfg, "embedart")?;
    if associate {
        let name = name.unwrap_or_else(|| cfg.str("fetchart.art_filename"));
        let t = Template::parse(&cfg.str("format_album"));
        for mut album in lib.albums(&Query::parse(Kind::Album, q)?)? {
            let items = lib.album_items(album.id.unwrap())?;
            let label = format_record(&t, &album);
            let found = items
                .iter()
                .find_map(|i| media::extract_art(i.path()).ok().flatten());
            let (Some((data, ext)), Some(dir)) = (found, plugins::fetchart::album_dir(&items))
            else {
                println!("{label}: no embedded art");
                continue;
            };
            let dest = dir.join(format!("{name}.{ext}"));
            std::fs::write(&dest, data).with_context(|| format!("writing {}", dest.display()))?;
            album.set("artpath", crate::model::Value::Path(dest.clone()));
            lib.store(&album)?;
            println!("{label}: {}", dest.display());
        }
        return Ok(());
    }
    // Without -a: first matching item with art, written to -o (beets behaviour).
    let items = lib.items(&Query::parse(Kind::Item, q)?)?;
    for item in items {
        if let Some((data, ext)) = media::extract_art(item.path())? {
            let base = output.unwrap_or_else(|| PathBuf::from("cover"));
            let mut dest = base.into_os_string();
            dest.push(format!(".{ext}"));
            let dest = PathBuf::from(dest);
            std::fs::write(&dest, data).with_context(|| format!("writing {}", dest.display()))?;
            println!(
                "Extracted art from {} to {}",
                item.path().display(),
                dest.display()
            );
            return Ok(());
        }
    }
    println!("No embedded art found.");
    Ok(())
}

fn clearart(cfg: &Config, lib: &Library, yes: bool, q: &[String]) -> Result<()> {
    require_plugin(cfg, "embedart")?;
    let items = lib.items(&Query::parse(Kind::Item, q)?)?;
    if items.is_empty() {
        println!("No matching items.");
        return Ok(());
    }
    if !yes && !ui::yes_no(&format!("Remove embedded art from {} file(s)", items.len())) {
        return Ok(());
    }
    let mut n = 0;
    for mut item in items {
        match media::clear_art(item.path()) {
            Ok(true) => {
                media::refresh_mtime(&mut item);
                lib.store(&item)?;
                n += 1;
            }
            Ok(false) => {}
            Err(e) => eprintln!("warning: {}: {e:#}", item.path().display()),
        }
    }
    println!("Cleared art from {n} file(s).");
    Ok(())
}

fn lyrics(cfg: &Config, lib: &Library, print: bool, force: bool, q: &[String]) -> Result<()> {
    require_plugin(cfg, "lyrics")?;
    let ly = plugins::lyrics::Lyrics::new(cfg);
    let ctx = PluginCtx {
        cfg,
        lib,
        write: cfg.bool("import.write"),
    };
    let t = Template::parse(&cfg.str("format_item"));
    for mut item in lib.items(&Query::parse(Kind::Item, q)?)? {
        if let Err(e) = ly.fetch_item(&ctx, &mut item, force) {
            eprintln!("warning: {}: {e:#}", format_record(&t, &item));
        }
        if print && !item.str("lyrics").is_empty() {
            println!(
                "\n{}\n\n{}\n",
                ui::bold(&format_record(&t, &item)),
                item.str("lyrics")
            );
        }
    }
    Ok(())
}

fn write(lib: &Library, pretend: bool, q: &[String]) -> Result<()> {
    let items = lib.items(&Query::parse(Kind::Item, q)?)?;
    let mut n = 0;
    for mut item in items {
        let on_disk = match media::read(item.path()) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("warning: {e:#}");
                continue;
            }
        };
        let differs = crate::media::TAG_FIELDS
            .iter()
            .any(|f| on_disk.formatted(f) != item.formatted(f));
        if !differs {
            continue;
        }
        println!("{}", item.path().display());
        if pretend {
            continue;
        }
        match media::write_item(&mut item) {
            Ok(()) => {
                lib.store(&item)?;
                n += 1;
            }
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    if !pretend {
        println!("Wrote tags to {n} file(s).");
    }
    Ok(())
}

fn human_bytes(b: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

fn human_seconds(s: f64) -> String {
    let units = [
        ("year", 31_536_000.0),
        ("week", 604_800.0),
        ("day", 86_400.0),
        ("hour", 3600.0),
        ("minute", 60.0),
    ];
    for (name, size) in units {
        if s >= size {
            return format!("{:.1} {name}s", s / size);
        }
    }
    format!("{s:.1} seconds")
}

fn stats(lib: &Library, exact: bool, q: &[String]) -> Result<()> {
    let items = lib.items(&Query::parse(Kind::Item, q)?)?;
    let mut total_time = 0.0;
    let mut total_size: u64 = 0;
    let (mut artists, mut albums, mut album_artists) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    for i in &items {
        total_time += i.float("length");
        total_size += std::fs::metadata(i.path()).map(|m| m.len()).unwrap_or(0);
        artists.insert(i.str("artist").to_string());
        album_artists.insert(i.str("albumartist").to_string());
        if i.int("album_id") != 0 {
            albums.insert(i.int("album_id"));
        }
    }
    println!("Tracks: {}", items.len());
    if exact {
        println!("Total time: {total_time:.0} seconds");
        println!("Total size: {total_size} bytes");
    } else {
        println!("Total time: {}", human_seconds(total_time));
        println!("Approximate total size: {}", human_bytes(total_size));
    }
    println!("Artists: {}", artists.len());
    println!("Albums: {}", albums.len());
    println!("Album artists: {}", album_artists.len());
    Ok(())
}

fn fields() -> Result<()> {
    println!("Item fields:");
    println!("  id");
    for (n, _) in ITEM_FIELDS {
        println!("  {n}");
    }
    println!("  singleton");
    println!("\nAlbum fields:");
    println!("  id");
    for (n, _) in ALBUM_FIELDS {
        println!("  {n}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lists_loaded_plugins() {
        let mut cfg = Config::defaults();
        let lines = version_lines(&cfg).unwrap();
        assert_eq!(
            lines[0],
            format!("beets-rs version {}", env!("CARGO_PKG_VERSION"))
        );
        assert!(lines[1].starts_with("Rust version "));
        assert_eq!(lines[2], "no plugins loaded");
        cfg.set(
            "plugins",
            serde_yaml::from_str("[lyrics, fetchart, embedart]").unwrap(),
        );
        cfg.set(
            "disabled_plugins",
            serde_yaml::from_str("[embedart]").unwrap(),
        );
        assert_eq!(version_lines(&cfg).unwrap()[2], "plugins: fetchart, lyrics");
        assert!(plugins::enabled(&cfg, "lyrics"));
        assert!(!plugins::enabled(&cfg, "embedart"));
    }

    #[test]
    fn help_for_unknown_commands_is_an_error() {
        let e = help(Some("frob")).unwrap_err();
        assert_eq!(e.to_string(), "unknown command 'frob'");
    }

    fn strs(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn modify_args_follow_beets_rules() {
        let (q, mods, dels) = parse_modify_args(&strs(&[
            "a!",
            "artist:x!",
            "title:a=b",
            "x=y!",
            "comment=a:b",
            "genre+=Rock",
            "mood-=sad",
            "year=1",
            "year=2",
            "^artist=z",
        ]));
        assert_eq!(dels, ["a"]);
        assert_eq!(q, ["artist:x!", "title:a=b"]);
        let m: Vec<(&str, Option<char>, &str)> = mods
            .iter()
            .map(|m| (m.field.as_str(), m.op, m.value.as_str()))
            .collect();
        assert_eq!(
            m,
            [
                ("x", None, "y!"),
                ("comment", None, "a:b"),
                ("genre", Some('+'), "Rock"),
                ("mood", Some('-'), "sad"),
                ("year", None, "2"),
                ("^artist", None, "z"),
            ]
        );
    }

    #[test]
    fn model_changes_list_changed_fields_like_beets() {
        let mut old = Record::new_item();
        old.set_str("title", "A");
        old.set("length", Value::Float(100.0));
        old.flex.insert("gone".into(), "x".into());
        let mut new = old.clone();
        new.set_str("title", "B");
        new.set_int("year", 1999);
        new.set("length", Value::Float(100.005));
        new.set("mtime", Value::Float(5.0));
        new.remove_flex("gone");
        new.flex.insert("mood".into(), "happy".into());
        assert_eq!(
            model_changes(&old, &new),
            [
                "gone: x",
                "mood: happy",
                "title: A -> B",
                "year: 0000 -> 1999"
            ]
        );
        assert!(model_changes(&old, &old).is_empty());
    }

    /// An in-memory library with `n` items in one album (no files).
    fn lib_with_album(n: usize) -> (tempfile::TempDir, Config, Library) {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.set("directory", d.path().join("lib").to_string_lossy().into());
        let lib = Library::in_memory().unwrap();
        let mut items: Vec<Item> = (0..n)
            .map(|i| {
                let mut it = Record::new_item();
                it.set_str("title", format!("T{i}"));
                it.set_str("album", "Alb");
                it.set_int("track", i as i64 + 1);
                it.set_path(d.path().join(format!("src/{i}.mp3")));
                it
            })
            .collect();
        lib.add_album(&mut items).unwrap();
        (d, cfg, lib)
    }

    fn quiet_opts(album: bool) -> ModifyOpts {
        ModifyOpts {
            album,
            yes: true,
            mv: Some(false),
            write: Some(false),
            inherit: true,
            format: None,
            limit: None,
        }
    }

    fn items(lib: &Library) -> Vec<Item> {
        lib.items(&Query::parse::<&str>(Kind::Item, &[]).unwrap())
            .unwrap()
    }

    /// `lib_with_album` with real (dummy) files and `src/cover.jpg` as art.
    fn lib_with_files(n: usize) -> (tempfile::TempDir, Config, Library) {
        let (d, cfg, lib) = lib_with_album(n);
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        for i in items(&lib) {
            std::fs::write(i.path(), b"audio").unwrap();
        }
        let art = d.path().join("src/cover.jpg");
        std::fs::write(&art, b"img").unwrap();
        let mut a = lib.all_albums().unwrap().remove(0);
        a.set("artpath", Value::Path(art));
        lib.store(&a).unwrap();
        (d, cfg, lib)
    }

    fn move_opts() -> MoveOpts {
        MoveOpts {
            album: false,
            dest: None,
            copy: false,
            export: false,
            pretend: false,
            timid: false,
            limit: None,
        }
    }

    fn artpath(lib: &Library) -> PathBuf {
        lib.all_albums().unwrap()[0]
            .path_field("artpath")
            .to_path_buf()
    }

    #[test]
    fn move_consolidates_into_the_library_with_the_art() {
        let (d, cfg, lib) = lib_with_files(2);
        let libdir = d.path().join("lib");
        move_cmd(&cfg, &lib, &move_opts(), &[]).unwrap();
        for i in items(&lib) {
            assert!(i.path().starts_with(&libdir), "{}", i.path().display());
            assert!(i.path().is_file());
        }
        assert!(!d.path().join("src/0.mp3").exists());
        let art = artpath(&lib);
        assert_eq!(art.parent(), items(&lib)[0].path().parent());
        assert!(art.is_file());
        assert!(!d.path().join("src/cover.jpg").exists());
        // A second run has nothing to do.
        move_cmd(&cfg, &lib, &move_opts(), &[]).unwrap();
        assert!(items(&lib)[0].path().is_file());
    }

    #[test]
    fn move_prunes_emptied_library_dirs_but_not_clutter() {
        let (d, cfg, lib) = lib_with_files(1);
        move_cmd(&cfg, &lib, &move_opts(), &[]).unwrap();
        let old_dir = items(&lib)[0].path().parent().unwrap().to_path_buf();
        std::fs::write(old_dir.join(".DS_Store"), b"").unwrap();
        // Retitle the album so the files move to another directory.
        let mut a = lib.all_albums().unwrap().remove(0);
        a.set_str("album", "Other");
        lib.store(&a).unwrap();
        for mut i in items(&lib) {
            i.set_str("album", "Other");
            lib.store(&i).unwrap();
        }
        let mut o = move_opts();
        o.album = true;
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        assert!(!old_dir.exists(), "clutter-only dir pruned");
        assert!(d.path().join("lib").exists());
        assert!(artpath(&lib).is_file());
    }

    #[test]
    fn move_copy_points_the_library_at_the_copies() {
        let (d, cfg, lib) = lib_with_files(2);
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let mut o = move_opts();
        o.copy = true;
        o.dest = Some(out.clone());
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        for i in items(&lib) {
            assert!(i.path().starts_with(&out));
            assert!(i.path().is_file());
        }
        assert!(d.path().join("src/0.mp3").is_file(), "original kept");
        assert!(artpath(&lib).starts_with(&out));
        assert!(d.path().join("src/cover.jpg").is_file());
    }

    #[test]
    fn move_export_copies_without_touching_the_library() {
        let (d, cfg, lib) = lib_with_files(2);
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let before = items(&lib);
        let mut o = move_opts();
        o.export = true;
        o.album = true;
        o.dest = Some(out.clone());
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        let after = items(&lib);
        assert_eq!(
            before.iter().map(|i| i.path()).collect::<Vec<_>>(),
            after.iter().map(|i| i.path()).collect::<Vec<_>>()
        );
        assert!(after.iter().all(|i| i.path().is_file()));
        assert_eq!(artpath(&lib), d.path().join("src/cover.jpg"));
        let copies: Vec<PathBuf> = walk(&out);
        assert_eq!(
            copies
                .iter()
                .filter(|p| p.extension().unwrap() == "mp3")
                .count(),
            2
        );
        // Album exports bring the art along.
        assert!(copies.iter().any(|p| p.file_name().unwrap() == "cover.jpg"));
    }

    #[test]
    fn move_export_of_items_leaves_the_art() {
        let (d, cfg, lib) = lib_with_files(1);
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let mut o = move_opts();
        o.export = true;
        o.dest = Some(out.clone());
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        let copies = walk(&out);
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert_eq!(items(&lib)[0].path(), d.path().join("src/0.mp3"));
    }

    #[test]
    fn move_pretend_limit_and_errors() {
        let (d, cfg, lib) = lib_with_files(2);
        let mut o = move_opts();
        o.pretend = true;
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        assert!(d.path().join("src/0.mp3").is_file());
        assert!(!d.path().join("lib").exists());

        let mut o = move_opts();
        o.limit = Some(1);
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        let moved = items(&lib)
            .iter()
            .filter(|i| i.path().starts_with(d.path().join("lib")))
            .count();
        assert_eq!(moved, 1);

        let e = move_cmd(&cfg, &lib, &move_opts(), &strs(&["title:nope"])).unwrap_err();
        assert_eq!(e.to_string(), "No matching items found.");
        let mut o = move_opts();
        o.album = true;
        let e = move_cmd(&cfg, &lib, &o, &strs(&["album:nope"])).unwrap_err();
        assert_eq!(e.to_string(), "No matching albums found.");
        let mut o = move_opts();
        o.dest = Some(d.path().join("missing"));
        let e = move_cmd(&cfg, &lib, &o, &[]).unwrap_err();
        assert!(e.to_string().starts_with("no such directory: "), "{e}");
    }

    #[test]
    fn move_skips_missing_files_and_drops_missing_art() {
        let (d, cfg, lib) = lib_with_files(2);
        std::fs::remove_file(d.path().join("src/0.mp3")).unwrap();
        std::fs::remove_file(d.path().join("src/cover.jpg")).unwrap();
        let mut o = move_opts();
        o.album = true;
        move_cmd(&cfg, &lib, &o, &[]).unwrap();
        let its = items(&lib);
        assert_eq!(its[0].path(), d.path().join("src/0.mp3"));
        assert!(its[1].path().starts_with(d.path().join("lib")));
        assert_eq!(artpath(&lib), PathBuf::new());
    }

    #[test]
    fn move_takes_a_unique_name_when_the_destination_exists() {
        let (_d, cfg, lib) = lib_with_files(1);
        let dest = {
            let i = &items(&lib)[0];
            let a = lib.get_album(i.int("album_id")).unwrap();
            PathFormatter::new(&cfg)
                .unwrap()
                .destination(i, a.as_ref(), Some(&lib))
        };
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"other").unwrap();
        move_cmd(&cfg, &lib, &move_opts(), &[]).unwrap();
        let p = items(&lib)[0].path().to_path_buf();
        assert_ne!(p, dest);
        assert_eq!(
            p.file_name().unwrap().to_string_lossy(),
            format!("{}.1.mp3", dest.file_stem().unwrap().to_string_lossy())
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"other");
        // Repeated moves leave it at `.1` (no flip to `.2` as in beets),
        // and it counts as already in place.
        for _ in 0..3 {
            move_cmd(&cfg, &lib, &move_opts(), &[]).unwrap();
            assert_eq!(items(&lib)[0].path(), p);
        }
        let i = &items(&lib)[0];
        let a = lib.get_album(i.int("album_id")).unwrap();
        let fmt = PathFormatter::new(&cfg).unwrap();
        assert_eq!(item_destination(&fmt, &lib, i, a.as_ref()), p);
        let mut names: Vec<String> = walk(dest.parent().unwrap())
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let stem = dest.file_stem().unwrap().to_string_lossy();
        assert_eq!(
            names,
            [
                format!("{stem}.1.mp3"),
                format!("{stem}.mp3"),
                "cover.jpg".into()
            ]
        );
    }

    #[test]
    fn modify_moves_to_a_unique_name_when_the_destination_exists() {
        let (_d, cfg, lib) = lib_with_files(1);
        move_cmd(&cfg, &lib, &move_opts(), &[]).unwrap();
        let old = items(&lib)[0].path().to_path_buf();
        let taken = old.with_file_name("01 New.mp3");
        std::fs::write(&taken, b"other").unwrap();
        let mut o = quiet_opts(false);
        o.mv = Some(true);
        modify(&cfg, &lib, o, &strs(&["title=New"])).unwrap();
        let p = items(&lib)[0].path().to_path_buf();
        assert_eq!(p, old.with_file_name("01 New.1.mp3"));
        assert!(p.is_file());
        assert!(!old.exists());
        assert_eq!(std::fs::read(&taken).unwrap(), b"other");
        // Modifying again (same destination) keeps the `.1` name.
        let mut o = quiet_opts(false);
        o.mv = Some(true);
        modify(&cfg, &lib, o, &strs(&["comments=x"])).unwrap();
        assert_eq!(items(&lib)[0].path(), p);
    }

    #[test]
    fn path_changes_layout_as_beets() {
        let ch = vec![
            (PathBuf::from("/a/x.mp3"), PathBuf::from("/b/x.mp3")),
            (PathBuf::from("/a/yy.mp3"), PathBuf::from("/b/yy.mp3")),
        ];
        assert_eq!(
            path_changes_lines(80, &ch),
            [
                "Source        Destination",
                "/a/x.mp3   -> /b/x.mp3",
                "/a/yy.mp3  -> /b/yy.mp3",
            ]
        );
        assert_eq!(
            path_changes_lines(19, &ch[..1]),
            ["/a/x.mp3 \n  -> /b/x.mp3"]
        );
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn modify_items_with_templates_types_and_deletions() {
        let (_d, cfg, lib) = lib_with_album(2);
        let run = |args: &[&str]| modify(&cfg, &lib, quiet_opts(false), &strs(args));
        run(&[
            "title:T1",
            "title=$title (live)",
            "year=1999x",
            "bpm=120",
            "mood=calm",
        ])
        .unwrap();
        let it = &items(&lib)[1];
        assert_eq!(it.str("title"), "T1 (live)");
        assert_eq!(
            it.int("year"),
            0,
            "unparsable values become null, as in beets"
        );
        assert_eq!(it.int("bpm"), 120);
        assert_eq!(it.str("mood"), "calm");
        assert_eq!(items(&lib)[0].str("title"), "T0");
        // Deleting a flex attribute drops it; a fixed field resets.
        run(&["title:T1", "mood!", "bpm!", "nosuch!"]).unwrap();
        let it = &items(&lib)[1];
        assert_eq!(it.get("mood"), None);
        assert_eq!(it.int("bpm"), 0);
        // Nothing to change is fine; errors use beets' wording.
        run(&["title:T1", "bpm=0"]).unwrap();
        let err = |args: &[&str]| run(args).unwrap_err().to_string();
        assert_eq!(err(&["title:T1"]), "no modifications specified");
        assert_eq!(
            err(&["title:zzz", "bpm=1"]),
            "No matching objects to modify."
        );
        assert_eq!(
            err(&["genre+=Rock"]),
            "field 'genre' does not support the += operator"
        );
        // -l caps the matches.
        let mut o = quiet_opts(false);
        o.limit = Some(1);
        modify(&cfg, &lib, o, &strs(&["comment=x"])).unwrap();
        let n = items(&lib)
            .iter()
            .filter(|i| i.str("comment") == "x")
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn album_changes_are_inherited_by_items_unless_noinherit() {
        let (_d, cfg, lib) = lib_with_album(2);
        modify(
            &cfg,
            &lib,
            quiet_opts(true),
            &strs(&["genre=Jazz", "mood=x"]),
        )
        .unwrap();
        assert_eq!(lib.all_albums().unwrap()[0].str("genre"), "Jazz");
        for i in items(&lib) {
            assert_eq!((i.str("genre"), i.str("mood")), ("Jazz", "x"));
        }
        // Deleting the album's flex attribute deletes it from the items.
        modify(&cfg, &lib, quiet_opts(true), &strs(&["mood!"])).unwrap();
        assert!(items(&lib).iter().all(|i| i.get("mood").is_none()));
        // Only changed fields propagate.
        let mut it = items(&lib).remove(0);
        it.set_str("genre", "Own");
        lib.store(&it).unwrap();
        modify(&cfg, &lib, quiet_opts(true), &strs(&["year=2001"])).unwrap();
        assert_eq!(items(&lib)[0].str("genre"), "Own");
        assert!(items(&lib).iter().all(|i| i.int("year") == 2001));
        // -I keeps the album's change to itself.
        let mut o = quiet_opts(true);
        o.inherit = false;
        modify(&cfg, &lib, o, &strs(&["genre=Blues"])).unwrap();
        assert_eq!(lib.all_albums().unwrap()[0].str("genre"), "Blues");
        assert!(items(&lib).iter().all(|i| i.str("genre") != "Blues"));
    }

    #[test]
    fn modify_args() {
        let args: Vec<String> = ["artist:foo", "genre=Rock", "mood!", "-live", "year:2000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (q, s, d) = parse_modify_args(&args);
        assert_eq!(q, vec!["artist:foo", "-live", "year:2000"]);
        assert_eq!(
            s,
            [Modification {
                field: "genre".into(),
                op: None,
                value: "Rock".into()
            }]
        );
        assert_eq!(d, vec!["mood"]);
    }
}
