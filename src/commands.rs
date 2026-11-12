//! Subcommand implementations.

use crate::config::{Config, default_config_dir};
use crate::fileops::{self, FileOp};
use crate::importer::{self, ImportOptions, Importer, Resume};
use crate::library::Library;
use crate::logging::debug;
use crate::media;
use crate::model::{ALBUM_FIELDS, Album, FieldType, ITEM_FIELDS, Item, Kind, Record, Value};
use crate::paths::PathFormatter;
use crate::plugins::{self, PluginCtx};
use crate::query::Query;
use crate::template::Template;
use crate::ui;
use crate::{Cli, Cmd};
use anyhow::{Result, bail};

use serde_yaml::Value as Y;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn run(cli: Cli) -> Result<()> {
    crate::logging::set_verbosity(cli.verbose);
    // As beets, `config -e` works even when the configuration is broken.
    if let Some(Cmd::Config { edit: true, .. }) = &cli.cmd {
        return config_edit(&cli.config);
    }
    let mut cfg = match Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e:#}");
            std::process::exit(1);
        }
    };
    if let Err(e) = ui::configure(&cfg) {
        eprintln!("configuration error: {e:#}");
        std::process::exit(1);
    }
    crate::query::configure(&cfg);
    crate::model::set_time_format(&cfg.str("time_format"));
    crate::media::set_id3v23(cfg.bool("id3v23"));
    debug!("configuration sources (highest → lowest priority):");
    for s in &cfg.sources {
        debug!("YamlSource {}", s.display());
    }
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
    if let Err(e) = crate::formatted::configure(&cfg) {
        eprintln!("configuration error: {e:#}");
        std::process::exit(1);
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
        Cmd::Config {
            paths,
            defaults,
            clear,
            ..
        } => {
            if *paths {
                for p in cfg.source_paths() {
                    println!("{}", p.display());
                }
            } else {
                let out = cfg.dump(*defaults, !*clear);
                if out.trim().is_empty() {
                    println!("Empty configuration");
                } else {
                    println!("{out}");
                }
            }
            return Ok(());
        }
        _ => {}
    }

    let lib = Library::open_with(
        &cfg.library_path(),
        Some(&cfg.directory()),
        cfg.bool("create_backup_before_migrations"),
    )?;
    debug!(
        "library database: {}\nlibrary directory: {}",
        cfg.library_path().display(),
        cfg.directory().display()
    );
    let plugins = plugins::load(&cfg)?;
    // The plugins' flexible attribute types (the types plugin's settings).
    if let Err(e) = plugins::types::configure(&cfg) {
        if e.to_string().starts_with("configuration error") {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Err(e);
    }
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
            // Checked up front: beets fails on a bad value at the first prompt.
            crate::importer::default_action(&cfg)?;
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
            // beets prints no summary after an import; keep ours for -v.
            let s = &imp.stats;
            let already = if s.already > 0 {
                format!("; {} already imported earlier", s.already)
            } else {
                String::new()
            };
            crate::logging::debug!(
                "Imported {} album(s), {} item(s); skipped {}{already}.",
                s.albums,
                s.items,
                s.skipped
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
            limit,
            query,
        } => {
            let opts = RemoveOpts {
                album,
                delete,
                force,
                limit,
            };
            remove(&cfg, &lib, &opts, &query)
        }
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
            album,
            format,
            mv,
            nomove,
            pretend,
            fields,
            exclude_fields,
            limit,
            query,
        } => {
            let opts = UpdateOpts {
                album,
                format,
                mv: (mv || nomove).then_some(mv),
                pretend,
                fields: (!fields.is_empty()).then_some(fields),
                exclude_fields,
                limit,
            };
            update(&cfg, &lib, &opts, &query)
        }
        Cmd::Write {
            pretend,
            force,
            limit,
            query,
        } => write(&cfg, &lib, pretend, force, limit, &query),
        Cmd::Stats {
            exact,
            limit,
            query,
        } => stats(&lib, exact, limit, &query),
        Cmd::Fetchart {
            force,
            quiet,
            limit,
            query,
        } => {
            require_plugin(&cfg, "fetchart")?;
            let ctx = art_ctx(&cfg, &lib);
            plugins::fetchart::command(
                &ctx,
                &lib,
                plugins::fetchart::CmdOpts {
                    force,
                    quiet,
                    limit,
                    query,
                },
            )
        }
        Cmd::Embedart {
            file,
            yes,
            url,
            limit,
            query,
        } => {
            require_plugin(&cfg, "embedart")?;
            let ctx = art_ctx(&cfg, &lib);
            plugins::embedart::embed_command(
                &ctx,
                plugins::embedart::EmbedCmd {
                    file,
                    url,
                    yes,
                    limit,
                    query,
                },
            )
        }
        Cmd::Extractart {
            outpath,
            filename,
            associate,
            limit,
            query,
        } => {
            require_plugin(&cfg, "embedart")?;
            let ctx = art_ctx(&cfg, &lib);
            plugins::embedart::extract_command(
                &ctx,
                plugins::embedart::ExtractCmd {
                    outpath,
                    filename,
                    associate,
                    limit,
                    query,
                },
            )
        }
        Cmd::Clearart { yes, limit, query } => {
            require_plugin(&cfg, "embedart")?;
            let ctx = art_ctx(&cfg, &lib);
            plugins::embedart::clear_command(&ctx, yes, limit, &query)
        }
        Cmd::Lyrics {
            print,
            rest_directory,
            force,
            keep_synced,
            no_keep_synced,
            local,
            limit,
            query,
        } => {
            require_plugin(&cfg, "lyrics")?;
            let keep_synced = if keep_synced {
                Some(true)
            } else if no_keep_synced {
                Some(false)
            } else {
                None
            };
            plugins::lyrics::command(
                &cfg,
                &lib,
                plugins::lyrics::CmdOpts {
                    print,
                    rest_directory,
                    force,
                    keep_synced,
                    local,
                    limit,
                    query,
                },
            )
        }
        Cmd::Ftintitle { drop, limit, query } => {
            require_plugin(&cfg, "ftintitle")?;
            plugins::ftintitle::command(&cfg, &lib, drop, limit, &query)
        }
        Cmd::Convert {
            pretend,
            threads,
            refresh,
            keep_new,
            dest,
            format,
            yes,
            link,
            hardlink,
            playlist,
            force,
            album,
            limit,
            query,
        } => {
            require_plugin(&cfg, "convert")?;
            plugins::convert::command(
                &cfg,
                &lib,
                plugins::convert::CmdOpts {
                    pretend,
                    threads,
                    refresh,
                    keep_new,
                    dest,
                    format,
                    yes,
                    link,
                    hardlink,
                    playlist,
                    force,
                    album,
                    limit,
                    query,
                },
            )
        }
        Cmd::Edit {
            field,
            all,
            album,
            query,
        } => {
            require_plugin(&cfg, "edit")?;
            plugins::edit::command(&cfg, &lib, album, &field, all, &query)
        }
        Cmd::Scrub {
            nowrite,
            limit,
            query,
        } => {
            require_plugin(&cfg, "scrub")?;
            plugins::scrub::command(&lib, !nowrite, limit, &query)
        }
        Cmd::Zero { limit, query } => {
            require_plugin(&cfg, "zero")?;
            plugins::zero::command(&cfg, &lib, &plugins, limit, &query)
        }
        Cmd::Lastgenre {
            pretend,
            force,
            no_force,
            keep_existing,
            no_keep_existing,
            source,
            items,
            albums: _,
            query,
        } => {
            require_plugin(&cfg, "lastgenre")?;
            if !plugins.iter().any(|p| p.name() == "lastgenre") {
                // It failed to load (and said why).
                bail!("unknown command 'lastgenre'");
            }
            let flag = |on: bool, off: bool| match (on, off) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            plugins::lastgenre::command(
                &cfg,
                &lib,
                plugins::lastgenre::CmdOpts {
                    pretend,
                    force: flag(force, no_force),
                    keep_existing: flag(keep_existing, no_keep_existing),
                    source,
                    album: !items,
                    query,
                },
            )
        }
        Cmd::Info {
            album,
            library,
            summarize,
            included_keys,
            keys_only,
            format,
            limit,
            args,
        } => {
            require_plugin(&cfg, "info")?;
            plugins::info::command(
                &lib,
                plugins::info::CmdOpts {
                    album,
                    library,
                    summarize,
                    included_keys,
                    keys_only,
                    format,
                    limit,
                    args,
                },
            )
        }
        Cmd::Replaygain {
            album,
            threads,
            force,
            write,
            nowrite,
            limit,
            query,
        } => {
            require_plugin(&cfg, "replaygain")?;
            if !plugins.iter().any(|p| p.name() == "replaygain") {
                // It failed to load (and said why).
                bail!("unknown command 'replaygain'");
            }
            // Set up again (it said what it had to when it loaded).
            let rg = crate::logging::capture(|| plugins::replaygain::ReplayGain::new(&cfg)).0?;
            let write = match (write, nowrite) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            rg.command(
                &cfg,
                &lib,
                plugins::replaygain::CmdOpts {
                    album,
                    force,
                    threads,
                    write,
                    limit,
                    query,
                },
            )
        }
        Cmd::Duplicates {
            count,
            checksum,
            delete,
            full,
            strict,
            keys,
            merge,
            move_to,
            copy_to,
            tag,
            remove,
            album,
            path,
            format,
            limit,
            query,
        } => {
            require_plugin(&cfg, "duplicates")?;
            plugins::duplicates::command(
                &cfg,
                &lib,
                plugins::duplicates::CmdOpts {
                    count,
                    checksum,
                    delete,
                    full,
                    strict,
                    keys,
                    merge,
                    move_to,
                    copy_to,
                    tag,
                    remove,
                    album,
                    path,
                    format,
                    limit,
                    query,
                },
            )
        }
        Cmd::Mbsync {
            pretend,
            mv,
            nomove,
            nowrite,
            format,
            limit,
            query,
        } => {
            require_plugin(&cfg, "mbsync")?;
            plugins::mbsync::command(
                &cfg,
                &lib,
                plugins::mbsync::CmdOpts {
                    pretend,
                    // The last of -m/-M given wins (clap's overrides).
                    mv: if mv {
                        Some(true)
                    } else if nomove {
                        Some(false)
                    } else {
                        None
                    },
                    write: nowrite.then_some(false),
                    format,
                    limit,
                    query,
                },
            )
        }
        Cmd::Missing {
            count,
            total,
            album,
            release_types,
            format,
            limit,
            query,
        } => {
            require_plugin(&cfg, "missing")?;
            plugins::missing::command(
                &cfg,
                &lib,
                plugins::missing::CmdOpts {
                    count,
                    total,
                    album,
                    release_types,
                    format,
                    limit,
                    query,
                },
            )
        }
        Cmd::Submit { limit, query } => {
            require_plugin(&cfg, "chroma")?;
            plugins::chroma::submit_command(&cfg, &lib, limit, &query)
        }
        Cmd::Fingerprint { limit, query } => {
            require_plugin(&cfg, "chroma")?;
            plugins::chroma::fingerprint_command(&cfg, &lib, limit, &query)
        }
        Cmd::Chromasearch {
            path,
            format,
            search,
            count,
            full,
            write,
            limit,
            query,
        } => {
            require_plugin(&cfg, "chroma")?;
            plugins::chroma::chromasearch_command(
                &cfg,
                &lib,
                plugins::chroma::SearchOpts {
                    search,
                    count,
                    full,
                    write,
                    path,
                    format,
                    limit,
                    query,
                },
            )
        }
        Cmd::Fields => fields(&lib),
        Cmd::MigrateBeets {
            beets_config,
            directory,
            force,
            database,
        } => crate::migrate::migrate_beets(
            &cfg,
            &lib,
            &database,
            beets_config.as_deref(),
            directory.as_deref(),
            force,
        ),
        Cmd::Config { .. } | Cmd::Version | Cmd::Help { .. } => unreachable!(),
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
    if plugins::enabled(cfg, "musicbrainz") {
        names.push("musicbrainz");
    }
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

/// Evaluate a template for an item or album (beets' `evaluate_template`):
/// with the library, an item's album fields and `%aunique` etc. work.
pub fn format_record(t: &Template, r: &Record, lib: Option<&Library>) -> String {
    t.render(&crate::formatted::Formatted::new(r, lib))
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
                println!("{}", format_record(&t, &a, Some(lib)));
            }
        }
    } else {
        for i in lib.items(&query)? {
            println!("{}", format_record(&t, &i, Some(lib)));
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
fn parse_modify_args(kind: Kind, args: &[String]) -> (Vec<String>, Vec<Modification>, Vec<String>) {
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
                field: crate::model::replace_legacy_field(kind, field, true),
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
    // `_multi_value_diff`: the values removed and added.
    if oldval.is_some() && matches!(old.field_type(field), Some(FieldType::List(_))) {
        let (o, n) = (old.list(field), new.list(field));
        let mut removed: Vec<&String> = o.iter().filter(|v| !n.contains(v)).collect();
        let mut added: Vec<&String> = n.iter().filter(|v| !o.contains(v)).collect();
        if removed.is_empty() && added.is_empty() {
            return None;
        }
        removed.sort();
        removed.dedup();
        added.sort();
        added.dedup();
        let mut parts = vec![format!("{field}:")];
        parts.extend(
            removed
                .iter()
                .map(|v| crate::diff::removed(&format!("  - {v}"))),
        );
        parts.extend(
            added
                .iter()
                .map(|v| crate::diff::added(&format!("  + {v}"))),
        );
        return Some(parts.join("\n"));
    }
    // `FormattedItemMapping`: an item's empty artist or album artist shows
    // the other.
    let formatted = |r: &Record| {
        let v = r.formatted(field).unwrap_or_default();
        match (r.kind, field) {
            (Kind::Item, "artist") if v.is_empty() => r.formatted("albumartist").unwrap_or(v),
            (Kind::Item, "albumartist") if v.is_empty() => r.formatted("artist").unwrap_or(v),
            _ => v,
        }
    };
    let oldstr = formatted(old);
    let newstr = formatted(new);
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
#[cfg(test)]
fn model_changes(old: &Record, new: &Record) -> Vec<String> {
    model_changes_in(old, new, None)
}

/// `model_changes` limited to `only` (beets' `fields`; empty means all).
fn model_changes_in(old: &Record, new: &Record, only: Option<&[String]>) -> Vec<String> {
    let mut fields: Vec<String> = old.keys();
    for k in new.keys() {
        if !fields.contains(&k) {
            fields.push(k);
        }
    }
    fields.retain(|f| f != "mtime");
    if let Some(only) = only.filter(|o| !o.is_empty()) {
        fields.retain(|f| only.contains(f));
    }
    fields.sort();
    fields
        .iter()
        .filter_map(|f| field_diff(f, old, new))
        .collect()
}

/// beets' `show_model_changes`: print `label` (the object, formatted as
/// it was) and its changes, if there are any. Returns whether there were.
pub(crate) fn show_model_changes(label: &str, old: &Record, new: &Record) -> bool {
    show_changes(label, old, new, None, false)
}

/// `show_model_changes(new, old, fields, print_obj=False)`: print just the
/// changes to `fields`. Returns whether there were any.
pub fn print_model_changes(old: &Record, new: &Record, fields: &[String]) -> bool {
    let changes = model_changes_in(old, new, Some(fields));
    for c in &changes {
        println!("  {}", c.replace('\n', "\n  "));
    }
    !changes.is_empty()
}

/// `show_model_changes` with beets' `fields` (only these are compared) and
/// `always` (print the label even without changes).
fn show_changes(
    label: &str,
    old: &Record,
    new: &Record,
    fields: Option<&[String]>,
    always: bool,
) -> bool {
    let changes = model_changes_in(old, new, fields);
    if !changes.is_empty() || always {
        println!("{label}");
    }
    if !changes.is_empty() {
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
fn modified(lib: &Library, obj: &Record, mods: &[Modification], dels: &[String]) -> Result<Record> {
    let mut new = obj.clone();
    for m in mods {
        let v = format_record(&Template::parse(&m.value), obj, Some(lib));
        // beets' `ModifyOperation.apply`: `+=` adds the values not there
        // yet, `-=` removes the given ones.
        if let (Some(op), Some(FieldType::List(d))) = (m.op, new.field_type(&m.field)) {
            let given = crate::model::split_list(&v, d);
            let current = obj.list(&m.field);
            let values: Vec<String> = if op == '+' {
                let mut out = current.clone();
                out.extend(given.into_iter().filter(|g| !current.contains(g)));
                out
            } else {
                current.into_iter().filter(|c| !given.contains(c)).collect()
            };
            new.set_list(&m.field, &values);
            continue;
        }
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
    let album = opts.album;
    let kind = if album { Kind::Album } else { Kind::Item };
    let (q, mods, dels) = parse_modify_args(kind, args);
    if mods.is_empty() && dels.is_empty() {
        bail!("no modifications specified");
    }
    let mut objs = if album {
        lib.albums(&Query::parse(kind, &q)?)?
    } else {
        lib.items(&Query::parse(kind, &q)?)?
    };
    if let Some(n) = opts.limit {
        objs.truncate(n);
    }
    // beets' `_check_modify_operations`: only list fields take `+=`/`-=`.
    if let Some(m) = mods.iter().find(|m| {
        m.op.is_some()
            && !matches!(
                crate::model::field_type(kind, &m.field),
                Some(FieldType::List(_))
            )
    }) {
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
        let new = modified(lib, &obj, &mods, &dels)?;
        if show_model_changes(&format_record(&t, &obj, Some(lib)), &obj, &new) {
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
            show_model_changes(&format_record(&t, old, Some(lib)), old, new);
        })
    };

    apply_changes(cfg, lib, album, selected, write, mv, opts.inherit)
}

/// beets' `try_sync` for each changed object (`(old, new)`): write tags,
/// move files inside the library, store; an album's changes go to its
/// items first with `inherit` (`Album.store(inherit=True)`).
pub(crate) fn apply_changes(
    cfg: &Config,
    lib: &Library,
    album: bool,
    selected: Vec<(Record, Record)>,
    write: bool,
    mv: bool,
    inherit: bool,
) -> Result<()> {
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
            if inherit {
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

/// beets' `Album.store(inherit=True)` after changing `old` into `new`: the
/// album, then its items with the changed fields (fixed and flexible).
pub(crate) fn store_album_inherit(lib: &Library, old: &Album, new: &Album) -> Result<()> {
    lib.store(new)?;
    let fields = model_changes_fields(old, new);
    if fields.is_empty() {
        return Ok(());
    }
    for mut item in lib.album_items(new.id.unwrap())? {
        for f in &fields {
            match new.get(f) {
                Some(v) => item.set(f, v),
                None => {
                    item.remove_flex(f);
                }
            }
        }
        lib.store(&item)?;
    }
    Ok(())
}

/// beets' `Item.move` / `Album.move` of one stored model with `op`, under
/// `basedir` (None: the library directory), stored afterwards; album art
/// follows as in `beet move`.
pub(crate) fn move_model(
    cfg: &Config,
    lib: &Library,
    rec: &Record,
    op: FileOp,
    basedir: Option<&Path>,
) -> Result<()> {
    let mut dcfg = cfg.clone();
    if let Some(d) = basedir {
        let d = crate::query::normpath(d);
        dcfg.set("directory", Y::String(d.to_string_lossy().into()));
    }
    let fmt = PathFormatter::new(&dcfg)?;
    let obj = match rec.kind {
        Kind::Album => {
            let items = lib.album_items(rec.id.unwrap())?;
            // Made unique when moved (`move_item_file`), as beets.
            let dests = items
                .iter()
                .map(|i| fmt.destination(i, Some(rec), Some(lib)))
                .collect();
            MoveObj {
                album: Some(rec.clone()),
                items,
                dests,
            }
        }
        Kind::Item => {
            let album = lib.get_album(rec.int("album_id"))?;
            let dest = fmt.destination(rec, album.as_ref(), Some(lib));
            MoveObj {
                album,
                items: vec![rec.clone()],
                dests: vec![dest],
            }
        }
    };
    move_obj(cfg, lib, obj, op, true, rec.kind == Kind::Album)
}

/// beets' `Item.remove(delete, with_album=True)` /
/// `Album.remove(delete, with_items=True)`.
pub(crate) fn remove_model(cfg: &Config, lib: &Library, rec: &Record, delete: bool) -> Result<()> {
    match rec.kind {
        Kind::Item => remove_item(cfg, lib, rec, delete, true),
        Kind::Album => remove_album(cfg, lib, rec, delete, true),
    }
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

/// `beet remove` options besides the query.
pub struct RemoveOpts {
    pub album: bool,
    /// -d: also delete the files (and album art).
    pub delete: bool,
    /// -f: don't ask.
    pub force: bool,
    pub limit: Option<usize>,
}

/// beets' `remove_items` / `remove_albums` / `remove_objects`.
fn remove(cfg: &Config, lib: &Library, opts: &RemoveOpts, q: &[String]) -> Result<()> {
    let kind = if opts.album { Kind::Album } else { Kind::Item };
    let mut objs = if opts.album {
        lib.albums(&Query::parse(kind, q)?)?
    } else {
        lib.items(&Query::parse(kind, q)?)?
    };
    objs.truncate(opts.limit.unwrap_or(usize::MAX));
    if objs.is_empty() {
        bail!("No matching objects to remove.");
    }
    let selected = if opts.force {
        objs
    } else {
        let file_count = if opts.album {
            let mut n = 0;
            for a in &objs {
                n += lib.album_items(a.id.unwrap())?.len();
            }
            n
        } else {
            objs.len()
        };
        let albums = opts.album.then_some(objs.len());
        let (fmt, prompt, prompt_all) = remove_prompts(cfg, opts.delete, file_count, albums);
        let t = Template::parse(&fmt);
        let show = |o: &Record| {
            if opts.album {
                println!();
                for i in lib.album_items(o.id.unwrap()).unwrap_or_default() {
                    println!("{}", format_record(&t, &i, Some(lib)));
                }
            } else {
                println!("{}", format_record(&t, o, Some(lib)));
            }
        };
        for o in &objs {
            show(o);
        }
        ui::select_objects_all(prompt, Some(&prompt_all), &objs, show)
    };
    if selected.is_empty() {
        return Ok(());
    }
    lib.transaction(|lib| {
        for obj in &selected {
            if opts.album {
                remove_album(cfg, lib, obj, opts.delete, true)?;
            } else {
                remove_item(cfg, lib, obj, opts.delete, true)?;
            }
        }
        Ok(())
    })
}

/// `beet remove`'s listing format, per-object prompt and first prompt.
/// beets' `prompt_all` for a plain remove ends in "?", to which
/// `input_select_objects` adds another; we print one.
fn remove_prompts(
    cfg: &Config,
    delete: bool,
    file_count: usize,
    albums: Option<usize>,
) -> (String, &'static str, String) {
    let album_str = match albums {
        Some(n) => format!(" and {n} album{}", if n > 1 { "s" } else { "" }),
        None => String::new(),
    };
    let s = if file_count > 1 { "s" } else { "" };
    if delete {
        (
            "$path - $title".into(),
            "Really DELETE",
            format!("Really DELETE {file_count} file{s}{album_str}"),
        )
    } else {
        (
            cfg.str("format_item"),
            "Really remove from the library",
            format!("Really remove {file_count} item{s}{album_str} from the library"),
        )
    }
}

/// beets' `Item.remove`: drop the item (and its album, if that leaves it
/// empty); with `delete`, delete the file and prune emptied directories.
fn remove_item(
    cfg: &Config,
    lib: &Library,
    item: &Item,
    delete: bool,
    with_album: bool,
) -> Result<()> {
    lib.remove(item)?;
    let id = item.int("album_id");
    if with_album
        && id != 0
        && let Some(a) = lib.get_album(id)?
        && lib.album_items(id)?.is_empty()
    {
        remove_album(cfg, lib, &a, delete, false)?;
    }
    if delete {
        remove_file(item.path());
        if let Some(p) = item.path().parent() {
            fileops::prune_dirs(p, &cfg.directory(), &fileops::clutter(cfg));
        }
    }
    Ok(())
}

/// beets' `Album.remove`: drop the album, with `delete` its art file, and
/// with `with_items` its items too.
fn remove_album(
    cfg: &Config,
    lib: &Library,
    album: &Album,
    delete: bool,
    with_items: bool,
) -> Result<()> {
    let items = if with_items {
        lib.album_items(album.id.unwrap())?
    } else {
        vec![]
    };
    lib.remove(album)?;
    if delete {
        remove_file(album.path_field("artpath"));
    }
    for i in &items {
        remove_item(cfg, lib, i, delete, false)?;
    }
    Ok(())
}

/// beets' `util.remove` (soft): delete a file if there is one.
fn remove_file(path: &Path) {
    if path.as_os_str().is_empty() || !path.exists() {
        return;
    }
    if let Err(e) = std::fs::remove_file(path) {
        eprintln!("error: deleting {}: {e}", path.display());
    }
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
        // Each album is read once, not once per item.
        let mut albums: HashMap<i64, Option<Album>> = HashMap::new();
        for i in items {
            let album_id = i.int("album_id");
            let album = match albums.get(&album_id) {
                Some(a) => a.clone(),
                None => {
                    let a = lib.get_album(album_id)?;
                    albums.insert(album_id, a.clone());
                    a
                }
            };
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
pub(crate) fn move_item_file(
    cfg: &Config,
    item: &mut Item,
    dest: &Path,
    op: FileOp,
) -> Result<bool> {
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
    // beets' `move_file`: a unique path unless `dest` is the file itself.
    // A move may keep the file's own `.N` name (see `unique_path_for`); a
    // copy leaves the source in place, so it's never free.
    let dest = if dest == src {
        src.clone()
    } else if op == FileOp::Move {
        fileops::unique_path_for(dest, Some(&src))
    } else {
        fileops::unique_path(dest)
    };
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

/// `beet update` options besides the query.
pub struct UpdateOpts {
    pub album: bool,
    /// -f: `format_item` for this run.
    pub format: Option<String>,
    /// -m/-M; default `import.move` or `import.copy`.
    pub mv: Option<bool>,
    pub pretend: bool,
    /// -F: only these fields (None: all media fields).
    pub fields: Option<Vec<String>>,
    /// -e: never these fields.
    pub exclude_fields: Vec<String>,
    pub limit: Option<usize>,
}

fn file_mtime(path: &Path) -> f64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        // beets' `current_mtime`: whole seconds.
        .map_or(0.0, |d| d.as_secs() as f64)
}

/// Store `fields` of `new` over the item's stored row (beets'
/// `store(fields=...)`), plus `mtime` when `with_mtime`.
fn store_fields(lib: &Library, new: &Item, fields: &[String], with_mtime: bool) -> Result<()> {
    let Some(mut row) = lib.get_item(new.id.unwrap())? else {
        return Ok(());
    };
    for f in fields
        .iter()
        .map(String::as_str)
        .chain(with_mtime.then_some("mtime"))
    {
        if let Some(v) = new.get(f) {
            row.set(f, v);
        }
    }
    lib.store(&row)
}

/// beets' `update_func` / `update_items`.
fn update(cfg: &Config, lib: &Library, opts: &UpdateOpts, q: &[String]) -> Result<()> {
    let libdir = cfg.directory();
    if !libdir.is_dir() {
        println!("Library path is unavailable or does not exist.");
        println!("{}", libdir.display());
        if ui::input_options(
            &['y', 'n'],
            true,
            "Are you sure you want to continue (y/n)?",
            "Enter Y or N:",
        ) != Some('y')
        {
            return Ok(());
        }
    }
    let items: Vec<Item> = if opts.album {
        let mut albums = lib.albums(&Query::parse(Kind::Album, q)?)?;
        albums.truncate(opts.limit.unwrap_or(usize::MAX));
        let mut v = vec![];
        for a in albums {
            v.extend(lib.album_items(a.id.unwrap())?);
        }
        v
    } else {
        let mut v = lib.items(&Query::parse(Kind::Item, q)?)?;
        v.truncate(opts.limit.unwrap_or(usize::MAX));
        v
    };
    if items.is_empty() {
        bail!("No matching items to update.");
    }
    let mv = opts
        .mv
        .unwrap_or_else(|| cfg.bool("import.move") || cfg.bool("import.copy"));
    let pretend = opts.pretend;
    // As beets: -F limits both the item and the album fields; a move
    // always stores `path`.
    let mut item_fields: Vec<String> = match &opts.fields {
        Some(f) => f.clone(),
        None => media::media_fields().map(String::from).collect(),
    };
    if mv && !item_fields.iter().any(|f| f == "path") {
        item_fields.push("path".into());
    }
    let mut album_fields: Vec<String> = match &opts.fields {
        Some(_) => item_fields.clone(),
        None => ALBUM_FIELDS.iter().map(|(n, _)| n.to_string()).collect(),
    };
    item_fields.retain(|f| !opts.exclude_fields.contains(f));
    album_fields.retain(|f| !opts.exclude_fields.contains(f));
    // A full update records the new mtime, so the file isn't read again;
    // a partial one (-F) leaves it, so the other changes still show later.
    let store_mtime = opts.fields.is_none();

    let t = Template::parse(
        &opts
            .format
            .clone()
            .unwrap_or_else(|| cfg.str("format_item")),
    );
    let fmt = PathFormatter::new(cfg)?;
    lib.transaction(|lib| {
        let mut affected: Vec<i64> = vec![];
        let mut affect = |id: i64| {
            if !affected.contains(&id) {
                affected.push(id);
            }
        };
        for item in items {
            let path = item.path().to_path_buf();
            if path.as_os_str().is_empty() || !path.exists() {
                println!("{}", format_record(&t, &item, Some(lib)));
                println!("{}", ui::colorize("text_error", "  deleted"));
                if !pretend {
                    remove_item(cfg, lib, &item, true, true)?;
                }
                affect(item.int("album_id"));
                continue;
            }
            if file_mtime(&path) <= item.float("mtime") {
                debug!(
                    "skipping {} because mtime is up to date ({})",
                    path.display(),
                    item.float("mtime")
                );
                continue;
            }
            let fresh = match media::read(&path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("error reading {}: {e:#}", path.display());
                    continue;
                }
            };
            let mut new = item.clone();
            for f in media::media_fields().chain(["mtime"]) {
                if let Some(v) = fresh.get(f) {
                    new.set(f, v);
                }
            }
            // Keep an album artist that only mirrored the track artist
            // (as beets does for as-is imports).
            if new.str("albumartist").is_empty()
                && item.str("albumartist") == item.str("artist")
                && item.str("artist") == new.str("artist")
            {
                new.set_str("albumartist", item.str("albumartist"));
            }
            let changed = show_changes(
                &format_record(&t, &item, Some(lib)),
                &item,
                &new,
                Some(&item_fields),
                false,
            );
            if pretend {
                continue;
            }
            if changed {
                if mv
                    && new.path().starts_with(&libdir)
                    && let Err(e) = move_item(&fmt, lib, cfg, &mut new)
                {
                    eprintln!("error moving {}: {e:#}", path.display());
                    continue;
                }
                affect(new.int("album_id"));
            }
            store_fields(lib, &new, &item_fields, store_mtime)?;
        }
        if pretend {
            return Ok(());
        }
        // Albums take their fields from their (first) item again.
        let fa = Template::parse(&cfg.str("format_album"));
        for id in affected {
            if id == 0 {
                continue;
            }
            let Some(mut album) = lib.get_album(id)? else {
                debug!("emptied album {id}");
                continue;
            };
            let items = lib.album_items(id)?;
            let Some(first) = items.first() else {
                continue;
            };
            // beets' `Album.item_keys`, stored (and inherited) where they
            // changed.
            let mut changed: Vec<&str> = vec![];
            for (k, _) in ALBUM_FIELDS {
                if *k == "artpath" || !album_fields.iter().any(|f| f == k) {
                    continue;
                }
                if let Some(v) = first.get(k)
                    && album.get(k).as_ref() != Some(&v)
                {
                    album.set(k, v);
                    changed.push(k);
                }
            }
            lib.store(&album)?;
            for mut i in items.iter().cloned() {
                if changed.iter().any(|k| i.get(k) != album.get(k)) {
                    for k in &changed {
                        if let Some(v) = album.get(k) {
                            i.set(k, v);
                        }
                    }
                    lib.store(&i)?;
                }
            }
            if mv && first.path().starts_with(&libdir) {
                debug!("moving album {id}");
                for mut i in lib.album_items(id)? {
                    if let Err(e) = move_item(&fmt, lib, cfg, &mut i) {
                        eprintln!(
                            "error moving album {}: {e:#}",
                            format_record(&fa, &album, Some(lib))
                        );
                        break;
                    }
                    store_fields(lib, &i, &item_fields, false)?;
                }
                sync_art(cfg, lib, id)?;
            }
        }
        Ok(())
    })
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
    let dest = crate::paths::art_destination(cfg, album, &art, &dir, Some(lib))?;
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

/// beets' `config_edit`: open the first `-c` file, else the user's
/// `config.yaml` (created empty if missing), in the editor, replacing this
/// process.
fn config_edit(configs: &[PathBuf]) -> Result<()> {
    let path = configs
        .first()
        .cloned()
        .unwrap_or_else(|| default_config_dir().join("config.yaml"));
    // The `editor` option, if the configuration can be read at all.
    let configured = Config::load(configs)
        .ok()
        .map(|c| c.str("editor"))
        .filter(|e| !e.is_empty());
    let editor = editor_command(configured);
    if editor.is_empty() {
        bail!("Please set the VISUAL or EDITOR environment variable to edit configuration.");
    }
    let open = || -> std::io::Result<std::io::Error> {
        if !path.is_file() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::File::create(&path)?;
        }
        Ok(interactive_open(&editor, &path))
    };
    let err = match open() {
        Ok(e) | Err(e) => e,
    };
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!("Editor '{editor}' not found.");
    }
    bail!("Could not edit configuration: {err}")
}

/// beets' `util.editor_command`: the `editor` option, `$VISUAL`, `$EDITOR`,
/// then the platform's opener.
pub(crate) fn editor_command(configured: Option<String>) -> String {
    editor_command_with(configured, |k| std::env::var(k).ok())
}

fn editor_command_with(configured: Option<String>, var: impl Fn(&str) -> Option<String>) -> String {
    let env = |k: &str| var(k).filter(|v| !v.is_empty());
    configured
        .or_else(|| env("VISUAL"))
        .or_else(|| env("EDITOR"))
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "open".into()
            } else {
                "xdg-open".into()
            }
        })
}

/// beets' `util.interactive_open`: run `command` (shell-split) on `target`
/// in place of this process. Returns only on failure.
fn interactive_open(command: &str, target: &Path) -> std::io::Error {
    let mut args = crate::paths::shell_split(command);
    if args.is_empty() {
        args.push(command.to_string());
    }
    let mut cmd = std::process::Command::new(&args[0]);
    cmd.args(&args[1..]).arg(target);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.exec()
    }
    #[cfg(not(unix))]
    {
        match cmd.status() {
            Ok(s) => std::process::exit(s.code().unwrap_or(0)),
            Err(e) => e,
        }
    }
}

fn require_plugin(cfg: &Config, name: &str) -> Result<()> {
    if !plugins::enabled(cfg, name) {
        bail!("the {name} plugin is not enabled; add it to `plugins:` in your config");
    }
    Ok(())
}

/// The plugin context for the art commands: tags are written as
/// `import.write` says (beets' `ui.should_write()`).
fn art_ctx<'a>(cfg: &'a Config, lib: &'a Library) -> PluginCtx<'a> {
    PluginCtx {
        cfg,
        lib,
        write: cfg.bool("import.write"),
    }
}

/// beets' `write_items`: write the database's tags to files whose tags
/// differ (or all, with `force`), showing the differences.
fn write(
    cfg: &Config,
    lib: &Library,
    pretend: bool,
    force: bool,
    limit: Option<usize>,
    q: &[String],
) -> Result<()> {
    let mut items = lib.items(&Query::parse(Kind::Item, q)?)?;
    items.truncate(limit.unwrap_or(usize::MAX));
    if items.is_empty() {
        bail!("No matching items to write.");
    }
    let t = Template::parse(&cfg.str("format_item"));
    let tag_fields: Vec<String> = media::TAG_FIELDS.iter().map(|f| f.to_string()).collect();
    for mut item in items {
        if !item.path().exists() {
            // beets logs this at INFO, which goes to stderr.
            eprintln!("missing file: {}", item.path().display());
            continue;
        }
        let clean = match media::read(item.path()) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("error reading {}: {e:#}", item.path().display());
                continue;
            }
        };
        let changed = show_changes(
            &format_record(&t, &clean, Some(lib)),
            &clean,
            &item,
            Some(&tag_fields),
            force,
        );
        if (changed || force) && !pretend {
            // beets' `try_sync(write=True)`: write, then store (the mtime).
            if let Err(e) = media::write_item(&mut item) {
                eprintln!("error: {e:#}");
            }
            lib.store(&item)?;
        }
    }
    Ok(())
}

/// beets' `human_seconds`: an interval in the largest unit it reaches
/// (a "year" is 52 weeks, as in beets).
fn human_seconds(interval: f64) -> String {
    let units: [(f64, &str); 7] = [
        (1.0, "second"),
        (60.0, "minute"),
        (60.0, "hour"),
        (24.0, "day"),
        (7.0, "week"),
        (52.0, "year"),
        (10.0, "decade"),
    ];
    let mut interval = interval;
    for i in 0..units.len() - 1 {
        let (increment, suffix) = units[i];
        interval /= increment;
        if interval < units[i + 1].0 {
            return format!("{interval:.1} {suffix}s");
        }
    }
    let (increment, suffix) = units[units.len() - 1];
    format!("{:.1} {suffix}s", interval / increment)
}

/// beets' `show_stats`. Without `-e` the size is beets' estimate from
/// bitrate × length, not the files' real size. beets prints the exact
/// seconds on a line of their own, which is blank without `-e`; that blank
/// line is kept so the output matches.
fn stats(lib: &Library, exact: bool, limit: Option<usize>, q: &[String]) -> Result<()> {
    let mut items = lib.items(&Query::parse(Kind::Item, q)?)?;
    // beets' `lib.items(query, limit=limit)`: the first N in the sort order.
    items.truncate(limit.unwrap_or(usize::MAX));
    let mut total_time = 0.0;
    let mut total_size: u64 = 0;
    let (mut artists, mut albums, mut album_artists) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    for i in &items {
        let length = i.float("length");
        total_time += length;
        total_size += if exact {
            std::fs::metadata(i.path()).map(|m| m.len()).unwrap_or(0)
        } else {
            (length * i.int("bitrate") as f64 / 8.0).max(0.0) as u64
        };
        artists.insert(i.str("artist").to_string());
        album_artists.insert(i.str("albumartist").to_string());
        if i.int("album_id") != 0 {
            albums.insert(i.int("album_id"));
        }
    }
    let mut size_str = crate::duplicates::human_bytes(total_size);
    if exact {
        size_str += &format!(" ({total_size} bytes)");
    }
    println!("Tracks: {}", items.len());
    println!("Total time: {}", human_seconds(total_time));
    println!(
        "{}",
        if exact {
            format!(" ({total_time:.2} seconds)")
        } else {
            String::new()
        }
    );
    println!(
        "{}: {size_str}",
        if exact {
            "Total size"
        } else {
            "Approximate total size"
        }
    );
    println!("Artists: {}", artists.len());
    println!("Albums: {}", albums.len());
    println!("Album artists: {}", album_artists.len());
    Ok(())
}

#[cfg(test)]
mod stats_tests {
    use super::human_seconds;

    #[test]
    fn human_seconds_matches_beets() {
        // Values from beets' units: a year is 52 weeks, then decades.
        assert_eq!(human_seconds(0.0), "0.0 seconds");
        assert_eq!(human_seconds(59.0), "59.0 seconds");
        assert_eq!(human_seconds(90.0), "1.5 minutes");
        assert_eq!(human_seconds(3.0 * 3600.0), "3.0 hours");
        assert_eq!(human_seconds(10_368.0), "2.9 hours");
        assert_eq!(human_seconds(86_400.0 * 364.0), "1.0 years");
        assert_eq!(human_seconds(86_400.0 * 7.0 * 52.0 * 25.0), "2.5 decades");
    }
}

fn fields(lib: &Library) -> Result<()> {
    for line in fields_lines(lib)? {
        println!("{line}");
    }
    Ok(())
}

/// beets' `fields_func`: each model's fixed and computed fields (sorted),
/// then the flexible attributes the library holds (in the order SQLite's
/// `SELECT DISTINCT` gives them). Plugins' album template fields count as
/// computed album fields.
fn fields_lines(lib: &Library) -> Result<Vec<String>> {
    let env = crate::formatted::env();
    // The inline plugin's fields are template fields too.
    let inline = |kind: Kind| -> Vec<String> {
        env.inline.as_ref().map_or_else(Vec::new, |i| {
            let f = if kind == Kind::Item {
                &i.item_fields
            } else {
                &i.album_fields
            };
            f.iter().map(|f| f.name.clone()).collect()
        })
    };
    let names = |fields: &[(&str, FieldType)], computed: &[String]| {
        let mut v: Vec<String> = fields
            .iter()
            .map(|(n, _)| n.to_string())
            .chain(std::iter::once("id".to_string()))
            .chain(computed.iter().cloned())
            .collect();
        v.sort_unstable();
        v.dedup();
        v.into_iter().map(|n| format!("  {n}")).collect::<Vec<_>>()
    };
    let mut out = vec!["Item fields:".to_string()];
    let mut item_computed: Vec<String> = ["singleton", "filesize", "has_cover_art"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    item_computed.extend(inline(Kind::Item));
    out.extend(names(ITEM_FIELDS, &item_computed));
    out.push("Album fields:".into());
    let mut album_computed: Vec<String> = vec!["path".into(), "albumtotal".into()];
    album_computed.extend(env.album_template_fields.iter().map(|s| s.to_string()));
    album_computed.extend(inline(Kind::Album));
    out.extend(names(ALBUM_FIELDS, &album_computed));
    for (title, kind) in [("Item", Kind::Item), ("Album", Kind::Album)] {
        out.push(format!("{title} flexible attributes:"));
        out.extend(lib.flex_keys(kind)?.into_iter().map(|k| format!("  {k}")));
    }
    Ok(out)
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
        assert_eq!(lines[2], "plugins: musicbrainz");
        cfg.set("plugins", serde_yaml::from_str("[]").unwrap());
        assert_eq!(version_lines(&cfg).unwrap()[2], "no plugins loaded");
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
        let (q, mods, dels) = parse_modify_args(
            Kind::Item,
            &strs(&[
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
            ]),
        );
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
                // beets' legacy `genre` is `genres`.
                ("genres", Some('+'), "Rock"),
                ("mood", Some('-'), "sad"),
                ("year", None, "2"),
                ("^artist", None, "z"),
            ]
        );
    }

    /// beets' `fields`: sorted fixed and computed fields per model, then the
    /// library's flexible attributes (`data_source` is one in beets 2.14).
    #[test]
    fn fields_as_beets() {
        let lib = Library::in_memory().unwrap();
        let mut it = Record::new_item();
        it.flex.insert("mood".into(), "x".into());
        it.set_str("data_source", "MusicBrainz");
        lib.add(&mut it).unwrap();
        let lines = fields_lines(&lib).unwrap();
        let at = |s: &str| lines.iter().position(|l| l == s).unwrap();
        let items = &lines[1..at("Album fields:")];
        let mut sorted = items.to_vec();
        sorted.sort();
        assert_eq!(items, sorted.as_slice());
        for f in [
            "  id",
            "  singleton",
            "  filesize",
            "  has_cover_art",
            "  genres",
        ] {
            assert!(items.iter().any(|l| l == f), "{f}");
        }
        assert!(
            !lines[..at("Item flexible attributes:")]
                .iter()
                .any(|l| l == "  data_source")
        );
        assert_eq!(
            &lines[at("Item flexible attributes:")..],
            [
                "Item flexible attributes:",
                "  data_source",
                "  mood",
                "Album flexible attributes:"
            ]
        );
        let albums = &lines[at("Album fields:") + 1..at("Item flexible attributes:")];
        assert!(albums.iter().any(|l| l == "  albumtotal") && albums.iter().any(|l| l == "  path"));
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
        // Lists: beets' `_multi_value_diff`, values removed then added,
        // each sorted; a reordering is no change.
        let mut old = Record::new_item();
        old.set_list("genres", &["Rock", "Pop", "Ska"]);
        let mut new = old.clone();
        new.set_list("genres", &["Pop", "Jazz", "Blues"]);
        assert_eq!(
            model_changes(&old, &new),
            ["genres:\n  - Rock\n  - Ska\n  + Blues\n  + Jazz"]
        );
        new.set_list("genres", &["Ska", "Pop", "Rock"]);
        assert!(model_changes(&old, &new).is_empty());
        // An item's empty album artist shows the artist (`FormattedItemMapping`).
        let mut old = Record::new_item();
        old.set_str("artist", "Solo");
        let mut new = old.clone();
        new.set_str("albumartist", "VA");
        assert_eq!(model_changes(&old, &new), ["albumartist: Solo -> VA"]);
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

    /// A library (dir `lib`) holding one album of `n` real MP3s by
    /// "Artist", with `cover.jpg` art; None without ffmpeg.
    fn lib_with_mp3s(n: usize) -> Option<(tempfile::TempDir, Config, Library)> {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        cfg.set("directory", d.path().join("lib").to_string_lossy().into());
        let dir = d.path().join("lib/Artist/Alb");
        std::fs::create_dir_all(&dir).unwrap();
        let lib = Library::in_memory().unwrap();
        let mut items = vec![];
        for t in 1..=n {
            let path = dir.join(format!("{t}.mp3"));
            let ok = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.5"])
                .args(["-metadata", "artist=Artist", "-metadata", "album=Alb"])
                .args(["-metadata", &format!("title=T{t}")])
                .args(["-metadata", &format!("track={t}")])
                .arg(&path)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                return None;
            }
            items.push(media::read(&path).unwrap());
        }
        let mut album = lib.add_album(&mut items).unwrap();
        let art = dir.join("cover.jpg");
        std::fs::write(&art, b"img").unwrap();
        album.set("artpath", Value::Path(art));
        lib.store(&album).unwrap();
        Some((d, cfg, lib))
    }

    /// Change a file's tags behind the library's back (and make its mtime
    /// newer than the stored one).
    fn retag(path: &Path, f: impl FnOnce(&mut Item)) {
        let mut i = media::read(path).unwrap();
        f(&mut i);
        media::write(&i).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    fn update_opts() -> UpdateOpts {
        UpdateOpts {
            album: false,
            format: None,
            mv: Some(false),
            pretend: false,
            fields: None,
            exclude_fields: vec![],
            limit: None,
        }
    }

    #[test]
    fn update_reads_changed_files_and_recomputes_albums() {
        let Some((_d, cfg, lib)) = lib_with_mp3s(2) else {
            return;
        };
        let p = items(&lib)[0].path().to_path_buf();
        retag(&p, |i| {
            i.set_str("title", "New");
            i.set_str("album", "Renamed");
        });
        // Pretend changes nothing.
        let mut o = update_opts();
        o.pretend = true;
        update(&cfg, &lib, &o, &[]).unwrap();
        assert_eq!(items(&lib)[0].str("title"), "T1");

        update(&cfg, &lib, &update_opts(), &[]).unwrap();
        let its = items(&lib);
        assert_eq!(its[0].str("title"), "New");
        // The album takes its first item's fields, and passes them on.
        assert_eq!(lib.all_albums().unwrap()[0].str("album"), "Renamed");
        assert!(its.iter().all(|i| i.str("album") == "Renamed"));
        // Up to date now: a db value changed without touching the stored
        // mtime isn't overwritten again.
        let mut i = its[0].clone();
        let id = i.id.unwrap();
        let get = || lib.get_item(id).unwrap().unwrap();
        let mtime = i.float("mtime");
        i.set_str("title", "Edited");
        // As beets, changing a tag field resets the mtime ("dirty").
        assert_eq!(i.float("mtime"), 0.0);
        i.set("mtime", Value::Float(mtime));
        lib.store(&i).unwrap();
        update(&cfg, &lib, &update_opts(), &[]).unwrap();
        assert_eq!(get().str("title"), "Edited");
        // With the reset mtime the file is read again.
        let mut i = get();
        i.set_str("title", "Edited again");
        lib.store(&i).unwrap();
        update(&cfg, &lib, &update_opts(), &[]).unwrap();
        assert_eq!(get().str("title"), "New");
    }

    #[test]
    fn update_fields_and_exclude_fields() {
        let Some((_d, cfg, lib)) = lib_with_mp3s(1) else {
            return;
        };
        let p = items(&lib)[0].path().to_path_buf();
        retag(&p, |i| {
            i.set_str("title", "New");
            i.set_str("artist", "Other");
        });
        let mut o = update_opts();
        o.fields = Some(vec!["title".into()]);
        update(&cfg, &lib, &o, &[]).unwrap();
        let i = &items(&lib)[0];
        assert_eq!((i.str("title"), i.str("artist")), ("New", "Artist"));
        // -F didn't record the mtime, so the artist change is still seen.
        let mut o = update_opts();
        o.exclude_fields = vec!["title".into()];
        let mut i = items(&lib)[0].clone();
        i.set_str("title", "Mine");
        lib.store(&i).unwrap();
        update(&cfg, &lib, &o, &[]).unwrap();
        let i = &items(&lib)[0];
        assert_eq!((i.str("title"), i.str("artist")), ("Mine", "Other"));
    }

    #[test]
    fn update_moves_changed_files_with_the_art() {
        let Some((_d, cfg, lib)) = lib_with_mp3s(1) else {
            return;
        };
        let p = items(&lib)[0].path().to_path_buf();
        retag(&p, |i| i.set_str("title", "New"));
        let mut o = update_opts();
        o.mv = Some(true);
        update(&cfg, &lib, &o, &[]).unwrap();
        let i = &items(&lib)[0];
        assert!(i.path().ends_with("01 New.mp3"), "{}", i.path().display());
        assert!(i.path().is_file());
        assert!(!p.exists());
        let art = artpath(&lib);
        assert!(art.is_file());
        assert_eq!(art.parent(), i.path().parent());
    }

    #[test]
    fn update_removes_missing_items_and_emptied_albums() {
        let Some((d, cfg, lib)) = lib_with_mp3s(2) else {
            return;
        };
        let its = items(&lib);
        std::fs::remove_file(its[0].path()).unwrap();
        update(&cfg, &lib, &update_opts(), &[]).unwrap();
        assert_eq!(items(&lib).len(), 1);
        assert_eq!(lib.all_albums().unwrap().len(), 1);
        std::fs::remove_file(its[1].path()).unwrap();
        update(&cfg, &lib, &update_opts(), &[]).unwrap();
        assert!(items(&lib).is_empty());
        assert!(lib.all_albums().unwrap().is_empty());
        // As beets (`Item.remove(delete=True)`): the album's art goes too,
        // and the emptied directories are pruned.
        assert!(!d.path().join("lib/Artist").exists());
        let e = update(&cfg, &lib, &update_opts(), &[]).unwrap_err();
        assert_eq!(e.to_string(), "No matching items to update.");
    }

    #[test]
    fn write_only_touches_files_whose_tags_differ() {
        let Some((_d, cfg, lib)) = lib_with_mp3s(2) else {
            return;
        };
        let its = items(&lib);
        // Precise mtimes (the stored ones are whole seconds, as beets').
        let mtimes = || -> Vec<std::time::SystemTime> {
            its.iter()
                .map(|i| std::fs::metadata(i.path()).unwrap().modified().unwrap())
                .collect()
        };
        let before = mtimes();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&cfg, &lib, false, false, None, &[]).unwrap();
        assert_eq!(mtimes(), before, "nothing differs, nothing written");

        let mut i = its[0].clone();
        i.set_str("title", "Changed");
        lib.store(&i).unwrap();
        write(&cfg, &lib, true, false, None, &[]).unwrap();
        assert_eq!(media::read(i.path()).unwrap().str("title"), "T1");
        write(&cfg, &lib, false, false, None, &[]).unwrap();
        assert_eq!(media::read(i.path()).unwrap().str("title"), "Changed");
        let after = mtimes();
        assert_ne!(after[0], before[0]);
        assert_eq!(after[1], before[1]);
        // The stored mtime follows, so `update` doesn't re-read it.
        assert_eq!(items(&lib)[0].float("mtime"), file_mtime(i.path()));

        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&cfg, &lib, false, true, None, &[]).unwrap();
        assert!(
            mtimes().iter().zip(&after).all(|(a, b)| a > b),
            "-f writes all"
        );

        std::fs::remove_file(its[1].path()).unwrap();
        write(&cfg, &lib, false, false, None, &[]).unwrap();
        let e = write(&cfg, &lib, false, false, None, &strs(&["title:nope"])).unwrap_err();
        assert_eq!(e.to_string(), "No matching items to write.");
    }

    fn remove_opts(album: bool, delete: bool) -> RemoveOpts {
        RemoveOpts {
            album,
            delete,
            force: true,
            limit: None,
        }
    }

    #[test]
    fn remove_items_drops_emptied_albums_and_keeps_files() {
        let (d, cfg, lib) = lib_with_files(2);
        remove(&cfg, &lib, &remove_opts(false, false), &strs(&["title:T0"])).unwrap();
        assert_eq!(items(&lib).len(), 1);
        assert_eq!(lib.all_albums().unwrap().len(), 1);
        remove(&cfg, &lib, &remove_opts(false, false), &[]).unwrap();
        assert!(items(&lib).is_empty());
        assert!(lib.all_albums().unwrap().is_empty());
        assert!(d.path().join("src/0.mp3").is_file());
        assert!(d.path().join("src/cover.jpg").is_file());
        let e = remove(&cfg, &lib, &remove_opts(false, false), &[]).unwrap_err();
        assert_eq!(e.to_string(), "No matching objects to remove.");
    }

    #[test]
    fn remove_delete_deletes_files_and_art_and_prunes() {
        let (d, mut cfg, lib) = lib_with_files(2);
        // The files live in the library directory here.
        cfg.set("directory", d.path().to_string_lossy().into());
        let mut o = remove_opts(true, true);
        o.limit = Some(1);
        remove(&cfg, &lib, &o, &[]).unwrap();
        assert!(items(&lib).is_empty());
        assert!(lib.all_albums().unwrap().is_empty());
        assert!(!d.path().join("src").exists(), "files, art and dir gone");
        assert!(d.path().exists());
    }

    #[test]
    fn remove_last_item_with_delete_takes_the_art() {
        let (d, cfg, lib) = lib_with_files(1);
        remove(&cfg, &lib, &remove_opts(false, true), &[]).unwrap();
        assert!(!d.path().join("src/0.mp3").exists());
        assert!(!d.path().join("src/cover.jpg").exists());
    }

    #[test]
    fn remove_prompts_as_beets() {
        let cfg = Config::defaults();
        let p = |delete, n, albums| {
            let (f, one, all) = remove_prompts(&cfg, delete, n, albums);
            (f, one.to_string(), all)
        };
        assert_eq!(
            p(true, 3, Some(1)),
            (
                "$path - $title".into(),
                "Really DELETE".into(),
                "Really DELETE 3 files and 1 album".into()
            )
        );
        assert_eq!(p(true, 1, None).2, "Really DELETE 1 file");
        let (f, one, all) = p(false, 2, Some(2));
        assert_eq!(f, cfg.str("format_item"));
        assert_eq!(one, "Really remove from the library");
        assert_eq!(all, "Really remove 2 items and 2 albums from the library");
    }

    #[test]
    fn editor_command_precedence_as_beets() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let both = env(&[("VISUAL", "vis"), ("EDITOR", "ed")]);
        assert_eq!(editor_command_with(Some("code -w".into()), both), "code -w");
        assert_eq!(editor_command_with(None, both), "vis");
        assert_eq!(editor_command_with(None, env(&[("EDITOR", "ed")])), "ed");
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(editor_command_with(None, env(&[])), opener);
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
        // List fields: "; " separates values; `+=` adds the missing ones,
        // `-=` removes; the legacy single-valued name means the list.
        let genres = || items(&lib)[1].list("genres");
        run(&["title:T1", "genres=Rock; Pop"]).unwrap();
        assert_eq!(genres(), ["Rock", "Pop"]);
        run(&["title:T1", "genres+=Jazz; Rock"]).unwrap();
        assert_eq!(genres(), ["Rock", "Pop", "Jazz"]);
        run(&["title:T1", "genres-=Rock; Ska"]).unwrap();
        assert_eq!(genres(), ["Pop", "Jazz"]);
        run(&["title:T1", "genre=Ska"]).unwrap();
        assert_eq!(genres(), ["Ska"]);
        run(&["title:T1", "albumtypes+=live"]).unwrap();
        assert_eq!(items(&lib)[1].str("albumtypes"), "live");
        // Nothing to change is fine; errors use beets' wording.
        run(&["title:T1", "bpm=0"]).unwrap();
        let err = |args: &[&str]| run(args).unwrap_err().to_string();
        assert_eq!(err(&["title:T1"]), "no modifications specified");
        assert_eq!(
            err(&["title:zzz", "bpm=1"]),
            "No matching objects to modify."
        );
        assert_eq!(
            err(&["mood+=calm"]),
            "field 'mood' does not support the += operator"
        );
        assert_eq!(
            err(&["title-=x"]),
            "field 'title' does not support the -= operator"
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
            &strs(&["genres=Jazz", "mood=x"]),
        )
        .unwrap();
        assert_eq!(lib.all_albums().unwrap()[0].str("genres"), "Jazz");
        for i in items(&lib) {
            assert_eq!((i.str("genres"), i.str("mood")), ("Jazz", "x"));
        }
        // Deleting the album's flex attribute deletes it from the items.
        modify(&cfg, &lib, quiet_opts(true), &strs(&["mood!"])).unwrap();
        assert!(items(&lib).iter().all(|i| i.get("mood").is_none()));
        // Only changed fields propagate.
        let mut it = items(&lib).remove(0);
        it.set_str("genres", "Own");
        lib.store(&it).unwrap();
        modify(&cfg, &lib, quiet_opts(true), &strs(&["year=2001"])).unwrap();
        assert_eq!(items(&lib)[0].str("genres"), "Own");
        assert!(items(&lib).iter().all(|i| i.int("year") == 2001));
        // -I keeps the album's change to itself.
        let mut o = quiet_opts(true);
        o.inherit = false;
        modify(&cfg, &lib, o, &strs(&["genres=Blues"])).unwrap();
        assert_eq!(lib.all_albums().unwrap()[0].str("genres"), "Blues");
        assert!(items(&lib).iter().all(|i| i.str("genres") != "Blues"));
    }

    #[test]
    fn modify_args() {
        let args: Vec<String> = ["artist:foo", "genre=Rock", "mood!", "-live", "year:2000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (q, s, d) = parse_modify_args(Kind::Item, &args);
        assert_eq!(q, vec!["artist:foo", "-live", "year:2000"]);
        assert_eq!(
            s,
            [Modification {
                field: "genres".into(),
                op: None,
                value: "Rock".into()
            }]
        );
        assert_eq!(d, vec!["mood"]);
    }
}
