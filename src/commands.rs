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
use std::path::PathBuf;

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
            query,
        } => move_cmd(&cfg, &lib, album, dest, copy, pretend, &query),
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

/// Recompute paths and move an item's file to match its metadata.
fn move_item(
    fmt: &PathFormatter,
    lib: &Library,
    cfg: &Config,
    item: &mut Item,
    copy: bool,
) -> Result<bool> {
    let album = lib.get_album(item.int("album_id"))?;
    let dest = fmt.destination(item, album.as_ref(), Some(lib));
    let src = item.path().to_path_buf();
    if dest == src {
        return Ok(false);
    }
    let dest = if dest.exists() {
        bail!("destination exists: {}", dest.display())
    } else {
        dest
    };
    if copy {
        fileops::copy_file(&src, &dest)?;
    } else {
        fileops::move_file(&src, &dest)?;
        if let Some(p) = src.parent() {
            fileops::prune_dirs(p, &cfg.directory(), &fileops::clutter(cfg));
        }
    }
    item.set_path(dest);
    Ok(true)
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
            && let Err(e) = move_item(&fmt, lib, cfg, item, false)
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

fn move_cmd(
    cfg: &Config,
    lib: &Library,
    album: bool,
    dest: Option<PathBuf>,
    copy: bool,
    pretend: bool,
    q: &[String],
) -> Result<()> {
    let mut cfg = cfg.clone();
    if let Some(d) = &dest {
        cfg.set("directory", Y::String(d.to_string_lossy().into()));
    }
    let fmt = PathFormatter::new(&cfg)?;
    let items: Vec<Item> = if album {
        let mut v = vec![];
        for a in lib.albums(&Query::parse(Kind::Album, q)?)? {
            v.extend(lib.album_items(a.id.unwrap())?);
        }
        v
    } else {
        lib.items(&Query::parse(Kind::Item, q)?)?
    };
    let mut n = 0;
    // Album id -> directory its items ended up in, to bring the art along.
    let mut album_dirs: HashMap<i64, PathBuf> = HashMap::new();
    for mut item in items {
        let album = lib.get_album(item.int("album_id"))?;
        if let Some(dir) = fmt.destination(&item, album.as_ref(), Some(lib)).parent()
            && item.int("album_id") != 0
        {
            album_dirs.insert(item.int("album_id"), dir.to_path_buf());
        }
        let target = fmt.destination(&item, album.as_ref(), Some(lib));
        if target == item.path() {
            continue;
        }
        if pretend {
            println!("{} -> {}", item.path().display(), target.display());
            continue;
        }
        match move_item(&fmt, lib, &cfg, &mut item, copy) {
            Ok(true) => {
                // With --dest, copies stay where they are in the library.
                if !(copy && dest.is_some()) {
                    lib.store(&item)?;
                }
                n += 1;
            }
            Ok(false) => {}
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    if !pretend {
        for (id, dir) in album_dirs {
            if copy {
                // Exported copies get a copy of the art; the library keeps its own.
                if let Some(a) = lib.get_album(id)? {
                    let art = a.path_field("artpath");
                    if art.is_file() {
                        let target = dir.join(art.file_name().unwrap_or_default());
                        if target != art && !target.exists() {
                            fileops::copy_file(art, &target)?;
                        }
                    }
                }
            } else {
                sync_art(&cfg, lib, id)?;
            }
        }
        println!("{} {n} item(s).", if copy { "Copied" } else { "Moved" });
    }
    Ok(())
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
                && let Err(e) = move_item(&fmt, lib, cfg, &mut new, false)
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

/// Keep an album's art next to its items after they move.
/// Move an album's art next to its items, if it lives elsewhere.
pub(crate) fn sync_art(cfg: &Config, lib: &Library, album_id: i64) -> Result<()> {
    let Some(mut album) = lib.get_album(album_id)? else {
        return Ok(());
    };
    let art = album.path_field("artpath").to_path_buf();
    if !art.is_file() {
        return Ok(());
    }
    let items = lib.album_items(album_id)?;
    let Some(dir) = plugins::fetchart::album_dir(&items) else {
        return Ok(());
    };
    let dest = dir.join(art.file_name().unwrap_or_default());
    if dest == art {
        return Ok(());
    }
    if dest.exists() {
        eprintln!("warning: not moving art, {} exists", dest.display());
        return Ok(());
    }
    fileops::move_file(&art, &dest)?;
    if let Some(p) = art.parent() {
        fileops::prune_dirs(p, &cfg.directory(), &fileops::clutter(cfg));
    }
    album.set("artpath", crate::model::Value::Path(dest));
    lib.store(&album)
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
