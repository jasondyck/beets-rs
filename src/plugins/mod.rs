//! Plugin framework. Plugins are compiled in and enabled through the
//! beets-style `plugins:` config list. They hook into import and can add
//! their own subcommands (wired up in `main.rs`/`commands.rs`).

pub mod art;
pub mod chroma;
pub mod convert;
pub mod duplicates;
pub mod edit;
pub mod embedart;
pub mod fetchart;
pub mod ftintitle;
pub mod info;
pub mod lastgenre;
pub mod lyrics;
pub mod mbsync;
pub mod missing;
pub mod replaygain;
pub mod requests;
pub mod scrub;
pub mod the;
pub mod types;
pub mod zero;

use crate::config::Config;
use crate::fileops::FileOp;
use crate::library::Library;
use crate::model::{Album, Item};
use anyhow::{Result, bail};
use std::cell::Cell;
use std::path::PathBuf;

pub const USER_AGENT: &str = concat!(
    "beets-rs/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/beetbox/beets )"
);

/// What a plugin gets to work with.
#[allow(dead_code)] // `cfg` is for plugins that need their own settings at hook time
pub struct PluginCtx<'a> {
    pub cfg: &'a Config,
    pub lib: &'a Library,
    /// Whether tag changes should be written to files (import.write).
    pub write: bool,
}

/// What an import stage or `import_task_files` listener sees of beets'
/// `ImportTask`.
pub struct ImportTask<'t> {
    /// The album being imported (None for a singleton).
    pub album: Option<&'t mut Album>,
    /// The imported items (`task.imported_items()`).
    pub items: &'t mut [Item],
    /// `task.paths`: the source directory (or file, for a singleton).
    pub paths: &'t [PathBuf],
    /// Imported as-is (`Action.ASIS`) rather than with a match applied.
    pub asis: bool,
    /// How the files are (to be) placed.
    pub op: FileOp,
}

/// A plugin's import hooks, as beets runs them: `early_import_stage` and
/// `import_stage` (beets' `early_import_stages`/`import_stages`) between
/// adding the task to the library and placing its files, every early
/// stage before every import stage, each followed by reloading the task
/// from the library; `import_task_files` once the files are placed and
/// written; `album_imported`/`item_imported` when the task is done.
pub trait Plugin {
    fn name(&self) -> &'static str;

    fn early_import_stage(&self, _ctx: &PluginCtx, _task: &mut ImportTask) -> Result<()> {
        Ok(())
    }

    fn import_stage(&self, _ctx: &PluginCtx, _task: &mut ImportTask) -> Result<()> {
        Ok(())
    }

    /// beets' `import_task_files` event.
    fn import_task_files(&self, _ctx: &PluginCtx, _task: &mut ImportTask) -> Result<()> {
        Ok(())
    }

    /// beets' `album_imported` event.
    fn album_imported(&self, _ctx: &PluginCtx, _album: &Album) -> Result<()> {
        Ok(())
    }

    /// beets' `item_imported` event.
    fn item_imported(&self, _ctx: &PluginCtx, _item: &Item) -> Result<()> {
        Ok(())
    }

    /// beets' `import_task_choice`: the user (or `-q`) chose what to do
    /// with a task while autotagging; `asis` for "Use as-is".
    fn import_task_choice(&self, _asis: bool) {}

    /// beets' `before_choose_candidate`: extra choices for the importer's
    /// prompt (after the built-in ones).
    fn prompt_choices(&self, _task: &ChoiceTask) -> Vec<PromptChoice> {
        vec![]
    }

    /// One of this plugin's prompt choices was picked. True for beets'
    /// `Action.RETAG` (import the task's items, as the callback left them,
    /// as tagged); false goes back to the prompt.
    fn run_prompt_choice(&self, _short: char, _task: &mut ChoiceTask) -> Result<bool> {
        Ok(false)
    }

    /// beets' `import_task_apply`: a match was applied to the task's items
    /// (before they're added to the library).
    fn import_task_apply(&self, _ctx: &PluginCtx, _task: &mut ImportTask) -> Result<()> {
        Ok(())
    }
}

pub const AVAILABLE: &[&str] = &[
    "chroma",
    "duplicates",
    "edit",
    "embedart",
    "fetchart",
    "ftintitle",
    "info",
    "lastgenre",
    "lyrics",
    "mbsync",
    "missing",
    "musicbrainz",
    "replaygain",
    "convert",
    "scrub",
    "the",
    "zero",
    "types",
];

/// The plugins to load: `plugins` (or `-p`) minus `disabled_plugins` (`-P`),
/// as beets' `get_plugin_names`.
pub fn names(cfg: &Config) -> Vec<String> {
    let disabled = cfg.list("disabled_plugins");
    cfg.list("plugins")
        .into_iter()
        .filter(|p| !disabled.contains(p))
        .collect()
}

pub fn load(cfg: &Config) -> Result<Vec<Box<dyn Plugin>>> {
    let mut out: Vec<Box<dyn Plugin>> = vec![];
    let names = names(cfg);
    let mut sorted = names.clone();
    sorted.sort();
    crate::logging::debug!("Loading plugins: {}", sorted.join(", "));
    for name in names {
        match name.as_str() {
            // beets' `_get_plugin`: a plugin that fails to set up is logged
            // ("** error loading plugin", with the traceback in beets) and
            // left out.
            "fetchart" => match fetchart::FetchArt::new(cfg) {
                Ok(p) => out.push(Box::new(p)),
                Err(e) => crate::logging::emit(format!("** error loading plugin {name}\n{e:#}")),
            },
            "lastgenre" => match lastgenre::LastGenre::new(cfg) {
                Ok(p) => out.push(Box::new(p)),
                Err(e) => crate::logging::emit(format!("** error loading plugin {name}\n{e:#}")),
            },
            "lyrics" => out.push(Box::new(lyrics::LyricsPlugin::new(cfg))),
            "embedart" => out.push(Box::new(embedart::EmbedArt::new(cfg))),
            "info" => out.push(Box::new(info::InfoPlugin)),
            "ftintitle" => out.push(Box::new(ftintitle::FtInTitle::new(cfg))),
            "duplicates" => out.push(Box::new(duplicates::DuplicatesPlugin)),
            "scrub" => out.push(Box::new(scrub::Scrub::new(cfg))),
            "convert" => out.push(Box::new(convert::ConvertPlugin::new())),
            "edit" => out.push(Box::new(edit::EditPlugin::new(cfg))),
            "missing" => out.push(Box::new(missing::MissingPlugin)),
            "mbsync" => out.push(Box::new(mbsync::MbsyncPlugin)),
            "replaygain" => match replaygain::ReplayGain::new(cfg) {
                Ok(p) => out.push(Box::new(p)),
                Err(e) => crate::logging::emit(format!("** error loading plugin {name}\n{e:#}")),
            },
            // Also a metadata source: its lookups and candidates are wired
            // into `MbSource` and `autotag` (see the module docs).
            "chroma" => out.push(Box::new(chroma::Chroma)),
            // `%the{}` lives in the template environment (`FormatEnv`);
            // loading only logs the config warnings.
            "the" => out.push(Box::new(the::The::load(cfg))),
            "zero" => match zero::Zero::new(cfg) {
                Ok(p) => out.push(Box::new(p)),
                Err(e) => crate::logging::emit(format!("** error loading plugin {name}\n{e:#}")),
            },
            "types" => out.push(Box::new(types::TypesPlugin)),
            // The autotagger's metadata source, as in beets 2.14: it's
            // queried only while this plugin is enabled.
            "musicbrainz" => crate::mb::warn_deprecated_config(cfg),
            other => bail!(
                "unknown plugin {other:?} (available: {})",
                AVAILABLE.join(", ")
            ),
        }
    }
    // Hooks run in the order the plugins are listed, as beets' listeners.
    // (embedart embeds fetchart's art through the `art_set` event, not
    // through its own import hook, so the two can come in either order.)
    Ok(out)
}

pub fn enabled(cfg: &Config, name: &str) -> bool {
    names(cfg).iter().any(|p| p == name)
}

/// Run a hook on every plugin; failures are warnings, never fatal to import.
pub fn each(plugins: &[Box<dyn Plugin>], mut f: impl FnMut(&dyn Plugin) -> Result<()>) {
    for p in plugins {
        if let Err(e) = f(p.as_ref()) {
            eprintln!("warning: {}: {e:#}", p.name());
        }
    }
}

/// beets' `art_set` event, sent when an album's art file is set: embedart
/// (if enabled) embeds the new art into the album's files.
pub fn send_art_set(ctx: &PluginCtx, album: &mut Album) {
    if enabled(ctx.cfg, "embedart") {
        let ea = embedart::EmbedArt::new(ctx.cfg);
        if let Err(e) = ea.process_album(ctx, album) {
            eprintln!("warning: embedart: {e:#}");
        }
    }
}

/// Python's logging levels.
pub const DEBUG: u8 = 10;
pub const INFO: u8 = 20;
pub const WARNING: u8 = 30;
pub const ERROR: u8 = 40;

/// A plugin's logger, as beets' (`"name: message"` on stderr). Its base
/// level is INFO while the plugin's own command runs and WARNING in import
/// hooks and event listeners; each `-v` lowers it a level.
pub struct PluginLog {
    name: &'static str,
    base_level: Cell<u8>,
}

impl PluginLog {
    pub fn new(name: &'static str) -> Self {
        PluginLog {
            name,
            base_level: Cell::new(INFO),
        }
    }

    pub fn set_base_level(&self, level: u8) {
        self.base_level.set(level);
    }

    pub fn base_level(&self) -> u8 {
        self.base_level.get()
    }

    /// Run `f` with the base level at `level`, restoring it afterwards.
    pub fn at_level<R>(&self, level: u8, f: impl FnOnce() -> R) -> R {
        let old = self.base_level.replace(level);
        let r = f();
        self.base_level.set(old);
        r
    }

    pub fn log(&self, level: u8, msg: &str) {
        let v = crate::logging::verbosity();
        let threshold = self.base_level.get().saturating_sub(10 * v).max(DEBUG);
        if level >= threshold {
            crate::logging::emit(format!("{}: {msg}", self.name));
        }
    }

    pub fn debug(&self, msg: &str) {
        self.log(DEBUG, msg);
    }

    pub fn info(&self, msg: &str) {
        self.log(INFO, msg);
    }

    pub fn warning(&self, msg: &str) {
        self.log(WARNING, msg);
    }

    pub fn error(&self, msg: &str) {
        self.log(ERROR, msg);
    }
}

/// beets' `plugins.feat_tokens`: a pattern matching "feat."-like words
/// that separate a main artist (or a title) from secondary artists; with
/// `for_artist`, also "with", "vs", "and", "con" and "&".
pub fn feat_tokens(for_artist: bool, custom_words: &[String]) -> String {
    let mut words: Vec<&str> = vec!["ft", "featuring", "feat", "feat.", "ft."];
    words.extend(custom_words.iter().map(String::as_str));
    if for_artist {
        words.extend(["with", "vs", "and", "con", "&"]);
    }
    let tokens: Vec<String> = words
        .iter()
        .map(|w| fancy_regex::escape(w).into_owned())
        .collect();
    let tokens = tokens.join("|");
    let bracketed = format!(r"(?:(?<=\s)|^)(?:\(|\[)(?:{tokens})(?=\s)");
    let plain = format!(r"(?<=[\s(\[])(?:{tokens})(?=\s)");
    format!("(?:{bracketed}|{plain})")
}

/// A plugin's choice at the importer prompt (beets' `PromptChoice`).
#[derive(Debug, Clone, PartialEq)]
pub struct PromptChoice {
    pub short: char,
    pub long: &'static str,
}

/// What a prompt choice's callback sees of the import task.
pub struct ChoiceTask<'t> {
    pub cfg: &'t Config,
    pub is_album: bool,
    /// The task's items, which the callback may change.
    pub items: &'t mut Vec<Item>,
    /// How many candidates there are.
    pub candidates: usize,
    /// The candidate on screen (beets' `task.selected_match`), 0-based.
    pub selected: Option<usize>,
    /// beets' `task.match = candidates[n]; task.apply_metadata()`.
    pub apply_candidate: &'t dyn Fn(usize, &mut Vec<Item>),
}

/// beets' `_get_choices`: the plugins' prompt choices, each with the index
/// of its plugin; those whose letter is taken (by a built-in choice, given
/// by its long name in `builtin`, or an earlier plugin's) are dropped with
/// beets' warning.
pub fn prompt_choices(
    plugins: &[Box<dyn Plugin>],
    task: &ChoiceTask,
    builtin: &[&'static str],
) -> Vec<(usize, PromptChoice)> {
    let letter = |long: &str| {
        long.chars()
            .find(|c| c.is_uppercase())
            .unwrap_or_else(|| long.chars().next().unwrap_or(' '))
            .to_ascii_lowercase()
    };
    let mut taken: Vec<(char, &str)> = std::iter::once("Apply")
        .chain(builtin.iter().copied())
        .map(|l| (letter(l), l))
        .collect();
    let mut out = vec![];
    for (i, p) in plugins.iter().enumerate() {
        for c in p.prompt_choices(task) {
            if let Some((_, first)) = taken.iter().find(|(l, _)| *l == c.short) {
                crate::logging::emit(format!(
                    "Prompt choice '{}' removed due to conflict with '{first}' (short letter: '{}')",
                    c.long, c.short
                ));
                continue;
            }
            taken.push((c.short, c.long));
            out.push((i, c));
        }
    }
    out
}
