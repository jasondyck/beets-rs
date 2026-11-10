//! Subcommand implementations.

use crate::config::{Config, DEFAULT_CONFIG, default_config_dir};
use crate::importer::{self, FileOp, ImportOptions, Importer};
use crate::library::Library;
use crate::media;
use crate::model::{
    ALBUM_FIELDS, Album, ITEM_FIELDS, Item, Kind, Record, album_item_shared_fields,
};
use crate::paths::PathFormatter;
use crate::plugins::{self, PluginCtx};
use crate::query::Query;
use crate::template::{Context, Template};
use crate::ui;
use crate::{Cli, Cmd};
use anyhow::{Context as _, Result, bail};
use regex::Regex;
use serde_yaml::Value as Y;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub fn run(cli: Cli) -> Result<()> {
    let mut cfg = Config::load(cli.config.as_deref())?;
    if let Some(l) = &cli.library {
        cfg.set("library", Y::String(l.to_string_lossy().into()));
    }
    if let Some(d) = &cli.directory {
        cfg.set("directory", Y::String(d.to_string_lossy().into()));
    }

    // Commands that don't need the library.
    match &cli.cmd {
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
    let plugins = plugins::load(&cfg)?;
    match cli.cmd {
        Cmd::Import {
            copy,
            nocopy,
            mv,
            write,
            nowrite,
            quiet,
            timid,
            noautotag,
            singletons,
            search_id,
            pretend,
            paths,
        } => {
            let mut opts = ImportOptions::from_config(&cfg);
            if copy {
                opts.op = FileOp::Copy;
            }
            if nocopy {
                opts.op = FileOp::None;
            }
            if mv {
                opts.op = FileOp::Move;
            }
            if write {
                opts.write = true;
            }
            if nowrite {
                opts.write = false;
            }
            opts.quiet |= quiet;
            opts.timid |= timid;
            opts.autotag &= !noautotag;
            opts.singletons |= singletons;
            opts.search_ids = search_id;
            opts.pretend = pretend;
            let mut imp = Importer::new(&cfg, &lib, opts, &plugins)?;
            imp.run(&paths)?;
            let s = &imp.stats;
            println!();
            println!(
                "Imported {} album(s), {} item(s); skipped {}.",
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
            yes,
            nomove,
            nowrite,
            args,
        } => modify(
            &cfg,
            &lib,
            album,
            yes,
            !nomove,
            !nowrite && cfg.bool("import.write"),
            &args,
        ),
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
        Cmd::Fields | Cmd::Config { .. } => unreachable!(),
    }
}

struct RecCtx<'a>(&'a Record);
impl Context for RecCtx<'_> {
    fn field(&self, name: &str) -> Option<String> {
        self.0.formatted(name)
    }
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
fn parse_modify_args(args: &[String]) -> (Vec<String>, Vec<(String, String)>, Vec<String>) {
    let set_re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$").unwrap();
    let del_re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)!$").unwrap();
    let (mut query, mut sets, mut dels) = (vec![], vec![], vec![]);
    for a in args {
        if let Some(c) = set_re.captures(a) {
            sets.push((c[1].to_string(), c[2].to_string()));
        } else if let Some(c) = del_re.captures(a) {
            dels.push(c[1].to_string());
        } else {
            query.push(a.clone());
        }
    }
    (query, sets, dels)
}

fn show_changes(before: &Record, after: &Record, label: &str) -> bool {
    let mut changed = false;
    let mut keys = before.keys();
    for k in after.keys() {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    for k in keys {
        if k == "mtime" {
            continue;
        }
        let (a, b) = (before.formatted(&k), after.formatted(&k));
        if a != b {
            if !changed {
                println!("{label}");
                changed = true;
            }
            match (a, b) {
                (Some(a), Some(b)) => println!("  {k}: {} -> {}", ui::red(&a), ui::green(&b)),
                (None, Some(b)) => println!("  {k}: {}", ui::green(&b)),
                (Some(a), None) => println!("  {k}: {} -> {}", ui::red(&a), ui::dim("(deleted)")),
                _ => {}
            }
        }
    }
    changed
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
        importer::copy_file(&src, &dest)?;
    } else {
        importer::move_file(&src, &dest)?;
        if let Some(p) = src.parent() {
            importer::prune_dirs(p, &cfg.directory());
        }
    }
    item.set_path(dest);
    Ok(true)
}

fn modify(
    cfg: &Config,
    lib: &Library,
    album: bool,
    yes: bool,
    mv: bool,
    write: bool,
    args: &[String],
) -> Result<()> {
    let (q, sets, dels) = parse_modify_args(args);
    if sets.is_empty() && dels.is_empty() {
        bail!("no modifications specified");
    }
    let kind = if album { Kind::Album } else { Kind::Item };
    let objs = if album {
        lib.albums(&Query::parse(kind, &q)?)?
    } else {
        lib.items(&Query::parse(kind, &q)?)?
    };
    let t = Template::parse(&cfg.str(if album { "format_album" } else { "format_item" }));

    let mut changed: Vec<Record> = vec![];
    for obj in &objs {
        let mut new = obj.clone();
        for (k, v) in &sets {
            // Values are templates, so `title='$title (live)'` works.
            let v = format_record(&Template::parse(v), obj);
            new.set_parsed(k, &v)?;
        }
        for k in &dels {
            new.remove_flex(k);
        }
        if show_changes(obj, &new, &format_record(&t, obj)) {
            changed.push(new);
        }
    }
    if changed.is_empty() {
        println!("No changes to make.");
        return Ok(());
    }
    let what = if album { "albums" } else { "items" };
    if !yes && !ui::yes_no(&format!("Really modify {} {what}", changed.len())) {
        return Ok(());
    }
    let fmt = PathFormatter::new(cfg)?;
    let in_lib = |i: &Item| i.path().starts_with(cfg.directory());
    let mut albums_touched: HashSet<i64> = HashSet::new();
    lib.transaction(|lib| {
        for obj in &mut changed {
            lib.store(obj)?;
            let mut items = if album {
                lib.album_items(obj.id.unwrap())?
            } else {
                vec![obj.clone()]
            };
            for item in &mut items {
                if album {
                    for f in album_item_shared_fields() {
                        if let Some(v) = obj.get(f) {
                            item.set(f, v);
                        }
                    }
                }
                if write && let Err(e) = media::write_item(item) {
                    eprintln!("warning: {e:#}");
                }
                if mv
                    && in_lib(item)
                    && let Err(e) = move_item(&fmt, lib, cfg, item, false)
                {
                    eprintln!("warning: {e:#}");
                }
                lib.store(item)?;
                albums_touched.insert(item.int("album_id"));
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
    lib.transaction(|lib| {
        let mut touched_albums: HashSet<i64> = HashSet::new();
        for i in &items {
            if delete {
                if let Err(e) = std::fs::remove_file(i.path()) {
                    eprintln!("warning: {}: {e}", i.path().display());
                }
                if let Some(p) = i.path().parent() {
                    importer::prune_dirs(p, &libdir);
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
                    importer::prune_dirs(p, &libdir);
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
                            importer::copy_file(art, &target)?;
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
        if show_changes(&item, &new, &format_record(&t, &item)) {
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
fn sync_art(cfg: &Config, lib: &Library, album_id: i64) -> Result<()> {
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
    importer::move_file(&art, &dest)?;
    if let Some(p) = art.parent() {
        importer::prune_dirs(p, &cfg.directory());
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
    fn modify_args() {
        let args: Vec<String> = ["artist:foo", "genre=Rock", "mood!", "-live", "year:2000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (q, s, d) = parse_modify_args(&args);
        assert_eq!(q, vec!["artist:foo", "-live", "year:2000"]);
        assert_eq!(s, vec![("genre".to_string(), "Rock".to_string())]);
        assert_eq!(d, vec!["mood"]);
    }
}
