//! Configuration: beets-compatible YAML keys layered over built-in defaults.

use anyhow::Result;
use serde_yaml::{Mapping, Value as Y};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = r#"# --------------- Main ---------------

library: library.db
directory: ~/Music
statefile: state.pickle
create_backup_before_migrations: yes
tempfile_prefix: '.'

# --------------- Plugins ---------------

plugins: [musicbrainz]

pluginpath: []

raise_on_error: no

# --------------- Import ---------------

clutter: ["Thumbs.DB", ".DS_Store"]
ignore: [".*", "*~", "System Volume Information", "lost+found"]
ignore_hidden: yes

import:
    # common options
    write: yes
    copy: yes
    move: no
    timid: no
    quiet: no
    log:
    # other options
    default_action: apply
    languages: []
    quiet_fallback: skip
    none_rec_action: ask
    # rare options
    link: no
    hardlink: no
    reflink: no
    delete: no
    resume: ask
    incremental: no
    incremental_skip_later: no
    from_scratch: no
    autotag: yes
    singletons: no
    detail: no
    flat: no
    group_albums: no
    pretend: false
    search_ids: []
    duplicate_keys:
        album: albumartist album
        item: artist title
    duplicate_action: ask
    duplicate_verbose_prompt: no
    bell: no
    set_fields: {}
    ignored_alias_types: []
    singleton_album_disambig: yes
    fix_ext_inplace: no
    remux_mp3_in_wav: yes

# --------------- Paths ---------------

path_sep_replace: _
empty_component_replace:
drive_sep_replace: _
asciify_paths: false
art_filename: cover
max_filename_length: 0
replace:
    # Replace bad characters with _
    # prohibited in many filesystem paths
    '[<>:\?\*\|]': _
    # double quotation mark "
    '\"': _
    # path separators: \ or /
    '[\\/]': _
    # starting and closing periods
    '^\.': _
    '\.$': _
    # control characters
    '[\x00-\x1f]': _
    # dash at the start of a filename (causes command line ambiguity)
    '^-': _
    # Replace bad characters with nothing
    # starting and closing whitespace
    '\s+$': ''
    '^\s+': ''

aunique:
    keys: albumartist album
    disambiguators: albumtype year label catalognum albumdisambig releasegroupdisambig
    bracket: '[]'

sunique:
    keys: artist title
    disambiguators: year trackdisambig
    bracket: '[]'

tunique:
    keys: title
    disambiguators: disc track artist
    bracket: '[]'

# --------------- Tagging ---------------

per_disc_numbering: no
original_date: no
artist_credit: no
id3v23: no
va_name: "Various Artists"
paths:
    default: $albumartist/$album%aunique{}/$track $title
    singleton: Non-Album/$artist/$title
    comp: Compilations/$album%aunique{}/$track $title

# --------------- Performance ---------------

threaded: yes
timeout: 5.0

# --------------- UI ---------------

verbose: 0
terminal_encoding:

ui:
    terminal_width: 80
    length_diff_thresh: 10.0
    color: yes
    colors:
        text_success: ['bold', 'green']
        text_warning: ['bold', 'yellow']
        text_error: ['bold', 'red']
        text_highlight: ['bold', 'red']
        text_highlight_minor: ['white']
        action_default: ['bold', 'cyan']
        action: ['bold', 'cyan']
        # New Colors
        text_faint: ['faint']
        import_path: ['bold', 'blue']
        import_path_items: ['bold', 'blue']
        changed: ['yellow']
        text_diff_added: ['bold', 'green']
        text_diff_removed: ['bold', 'red']
        action_description: ['white']
    import:
        indentation:
            match_header: 2
            match_details: 2
            match_tracklist: 5
        layout: column

# --------------- Search ---------------

format_item: $artist - $album - $title
format_album: $albumartist - $album
time_format: '%Y-%m-%d %H:%M:%S'
format_raw_length: no

sort_album: albumartist+ album+
sort_item: artist+ album+ disc+ track+
sort_case_insensitive: yes

# --------------- Autotagger ---------------

overwrite_null:
  album: []
  track: []

match:
    strong_rec_thresh: 0.04
    medium_rec_thresh: 0.25
    rec_gap_thresh: 0.25
    max_rec:
        missing_tracks: medium
        unmatched_tracks: medium
    distance_weights:
        data_source: 2.0
        artist: 3.0
        album: 3.0
        media: 1.0
        mediums: 1.0
        year: 1.0
        country: 0.5
        label: 0.5
        catalognum: 0.5
        albumdisambig: 0.5
        album_id: 5.0
        tracks: 2.0
        missing_tracks: 0.9
        unmatched_tracks: 0.6
        track_title: 3.0
        track_artist: 2.0
        track_index: 1.0
        track_length: 2.0
        track_id: 5.0
        medium: 1.0
    preferred:
        countries: []
        media: []
        original_year: no
    ignored: []
    required: []
    ignored_media: []
    ignore_data_tracks: yes
    ignore_video_tracks: yes
    track_length_grace: 10
    track_length_max: 30
    album_disambig_fields: data_source media year country label catalognum albumdisambig
    singleton_disambig_fields: data_source index track_alt album
"#;

/// confuse's `ConfigError`: a bad setting, reported as
/// "configuration error: MESSAGE".
#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone)]
pub struct Config {
    /// The effective configuration: defaults, then every source over them.
    root: Y,
    /// Command-line overrides (beets' `config.set_args` source), highest
    /// priority.
    cli: Y,
    /// The files read, highest priority first (each `-c` file, the
    /// includes, the user's config.yaml).
    layers: Vec<Y>,
    /// Directory holding config.yaml; relative `library` paths resolve against it.
    pub config_dir: PathBuf,
    /// Files configuration was read from, highest priority first (beets'
    /// `config.sources`).
    pub sources: Vec<PathBuf>,
}

/// The defaults with every bundled plugin's settings: beets' "not yet
/// loaded" view, used by [`Config::defaults`] (tests and plugin setup).
/// [`Config::load`] instead adds only the enabled plugins' defaults, as
/// beets' `config.add` at plugin load does.
fn default_tree() -> Y {
    let ctx = crate::plugin_registry::DefaultsCtx::from_tree(&core_tree());
    merge(core_tree(), crate::plugin_registry::default_tree(&ctx))
}

/// The defaults of an effective configuration: the core's, then the
/// enabled plugins' in `plugins:` order (a shared section such as `mpd`
/// gets each plugin's keys, as beets' `__init__` adds them).
fn default_tree_for(names: &[String], tree: &Y) -> Y {
    let mut out = core_tree();
    let ctx = crate::plugin_registry::DefaultsCtx::from_tree(tree);
    let plugins = crate::plugin_registry::defaults_for(names, &ctx);
    crate::plugin_registry::merge_first(&mut out, &plugins);
    out
}

/// A list setting from a raw YAML tree (`Config::list` without a `Config`).
fn list_in(root: &Y, key: &str) -> Vec<String> {
    let mut cur = root;
    for part in key.split('.') {
        let Some(v) = cur
            .as_mapping()
            .and_then(|m| m.get(Y::String(part.to_string())))
        else {
            return vec![];
        };
        cur = v;
    }
    match cur {
        Y::Sequence(seq) => seq
            .iter()
            .filter_map(|v| match v {
                Y::String(s) => Some(s.clone()),
                Y::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Y::String(s) => s.split_whitespace().map(String::from).collect(),
        _ => vec![],
    }
}

/// confuse's `ConfigReadError`: "PATH could not be read: REASON".
fn read_yaml(file: &Path) -> Result<Y> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("{} could not be read: {e}", file.display()))?;
    let y: Y = serde_yaml::from_str(&yaml11_bools(&text))
        .map_err(|e| anyhow::anyhow!("{} could not be read: {e}", file.display()))?;
    Ok(if y.is_null() {
        Y::Mapping(Mapping::new())
    } else {
        y
    })
}

/// Set a dotted key in a YAML tree, making mappings on the way.
fn set_in(root: &mut Y, key: &str, value: Y) {
    let mut cur = root;
    let parts: Vec<&str> = key.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        if !cur.is_mapping() {
            *cur = Y::Mapping(Mapping::new());
        }
        let map = cur.as_mapping_mut().unwrap();
        if i == parts.len() - 1 {
            map.insert(Y::String(part.to_string()), value);
            return;
        }
        cur = map
            .entry(Y::String(part.to_string()))
            .or_insert_with(|| Y::Mapping(Mapping::new()));
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if p == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(p)
}

/// Default config directory: $BEETSRSDIR, else ~/.config/beets-rs.
/// (Not ~/.config/beets, so we never touch a real beets library.db.)
pub fn default_config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BEETSRSDIR") {
        return expand_tilde(&d);
    }
    dirs::config_dir()
        .unwrap_or_else(|| expand_tilde("~/.config"))
        .join("beets-rs")
}

/// Deep-merge `over` onto `base`. Mapping keys from `over` come first so a
/// user's `paths:` entries take priority over the defaults' order.
pub(crate) fn merge(base: Y, over: Y) -> Y {
    match (base, over) {
        (Y::Mapping(base), Y::Mapping(over)) => {
            let mut out = Mapping::new();
            let mut base = base;
            for (k, v) in over {
                let merged = match base.shift_remove(&k) {
                    Some(b) => merge(b, v),
                    None => v,
                };
                out.insert(k, merged);
            }
            for (k, v) in base {
                out.insert(k, v);
            }
            Y::Mapping(out)
        }
        (_, over) => over,
    }
}

/// The core defaults (beets' `config_default.yaml`), without the plugins'
/// sections.
pub(crate) fn core_tree() -> Y {
    serde_yaml::from_str(&yaml11_bools(DEFAULT_CONFIG)).expect("default config parses")
}

/// confuse's `RootView.flatten` over `sources`, highest priority first:
/// keys in first-seen order, values from the first source that has them,
/// mappings merged key by key.
fn flatten_sources(sources: &[&Y]) -> Y {
    let maps: Vec<&Mapping> = sources.iter().filter_map(|s| s.as_mapping()).collect();
    flatten_maps(&maps)
}

/// YAML 1.1 booleans: serde_yaml reads unquoted `yes`/`no`/`on`/`off`/
/// `true`/`false` as strings, but beets' loader (and PyYAML) read them as
/// booleans (`peak: 'true'` stays a string).
pub(crate) fn yaml11_bools(text: &str) -> String {
    fn bool_of(v: &str) -> Option<&'static str> {
        match v {
            "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" => Some("true"),
            "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" => {
                Some("false")
            }
            _ => None,
        }
    }
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        let trimmed = body.trim_start();
        let indent = body.len() - trimmed.len();
        // Where the value starts: after a list marker or a key's colon.
        let value_start = if trimmed.starts_with("# ") || trimmed == "#" {
            None
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            Some(indent + 2 + rest.len() - rest.trim_start().len())
        } else if let Some(i) = trimmed.find(':') {
            let rest = &trimmed[i + 1..];
            Some(indent + i + 1 + rest.len() - rest.trim_start().len())
        } else {
            None
        };
        let Some(vs) = value_start else {
            out.push_str(body);
            out.push_str(nl);
            continue;
        };
        let rest = &body[vs..];
        let mut val = rest;
        if let Some(i) = val.find(" #") {
            val = &val[..i];
        }
        let val = val.trim_end();
        match bool_of(val) {
            Some(b) => {
                out.push_str(&body[..vs]);
                out.push_str(b);
                out.push_str(&body[vs + val.len()..]);
                out.push_str(nl);
            }
            None => {
                out.push_str(body);
                out.push_str(nl);
            }
        }
    }
    out
}

fn flatten_maps(maps: &[&Mapping]) -> Y {
    let mut keys: Vec<Y> = vec![];
    for m in maps {
        for k in m.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    let mut out = Mapping::new();
    for k in keys {
        let vals: Vec<&Y> = maps.iter().filter_map(|m| m.get(&k)).collect();
        let value = if vals.iter().all(|v| v.is_mapping()) {
            let sub: Vec<&Mapping> = vals.iter().filter_map(|v| v.as_mapping()).collect();
            flatten_maps(&sub)
        } else {
            // confuse falls back to the highest priority value when the
            // sources disagree on the value's type.
            vals[0].clone()
        };
        out.insert(k, value);
    }
    Y::Mapping(out)
}

/// Replace the dotted key `key` with "REDACTED" (beets' `set_redaction`).
fn set_redacted(out: &mut Y, key: &str) {
    let mut cur = out;
    let parts: Vec<&str> = key.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        let Some(v) = cur
            .as_mapping_mut()
            .and_then(|m| m.get_mut(Y::String(part.to_string())))
        else {
            return;
        };
        if i == parts.len() - 1 {
            *v = Y::String("REDACTED".into());
            return;
        }
        cur = v;
    }
}

/// confuse's `restore_yaml_comments`: put the top-level comments (and blank
/// lines) of beets' default config before the same keys in the dump.
fn restore_comments(data: &str) -> String {
    use std::collections::HashMap;
    let mut comment_map: HashMap<String, String> = HashMap::new();
    let lines: Vec<&str> = DEFAULT_CONFIG.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if !(line.is_empty() || line.starts_with('#')) {
            i += 1;
            continue;
        }
        let mut comment = String::new();
        let mut j = i;
        while j < lines.len() {
            let l = lines[j];
            if l.is_empty() {
                comment.push('\n');
            } else if l.starts_with('#') {
                comment.push_str(l);
                comment.push('\n');
            } else {
                break;
            }
            j += 1;
        }
        if j >= lines.len() {
            break;
        }
        let key = lines[j].split(':').next().unwrap_or("").trim().to_string();
        comment_map.insert(key, comment);
        i = j;
    }
    let mut out = String::new();
    for line in data.lines() {
        let key = line.split(':').next().unwrap_or("").trim();
        if let Some(c) = comment_map.get(key) {
            out.push_str(c);
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

impl Config {
    /// beets' config loading: the defaults, the user's `config.yaml` (in
    /// $BEETSRSDIR or ~/.config/beets-rs), the files its `include:` names
    /// (relative to that directory), then each `-c` file on top.
    pub fn load(overlays: &[PathBuf]) -> Result<Config> {
        let dir = default_config_dir();
        // As confuse, make sure the configuration directory exists.
        std::fs::create_dir_all(&dir).ok();
        Config::load_in(&dir, overlays)
    }

    /// `load` with the configuration directory given.
    pub fn load_in(dir: &Path, overlays: &[PathBuf]) -> Result<Config> {
        let defaults: Y = default_tree();
        let mut cfg = Config {
            root: defaults,
            cli: Y::Mapping(Mapping::new()),
            layers: vec![],
            config_dir: dir.to_path_buf(),
            sources: vec![],
        };
        let file = dir.join("config.yaml");
        if file.exists() {
            cfg.layer(read_yaml(&file)?, file);
        }
        // beets' `IncludeLazyConfig`: a file that can't be read is reported
        // and ends the includes.
        for inc in cfg.list("include") {
            let p = expand_tilde(&inc);
            let p = if p.is_absolute() { p } else { dir.join(p) };
            match read_yaml(&p) {
                Ok(y) => cfg.layer(y, p),
                Err(e) => {
                    eprintln!("configuration `import` failed: {e}");
                    break;
                }
            }
        }
        for o in overlays {
            cfg.layer(read_yaml(o)?, o.clone());
        }
        // beets adds each enabled plugin's defaults when it loads; only
        // those reach the configuration.
        cfg.rebuild();
        Ok(cfg)
    }

    /// Rebuild the effective configuration from the core defaults, the
    /// enabled plugins' defaults (in `plugins:` order) and the sources read
    /// so far. Called after loading, when `plugins`/`disabled_plugins`
    /// change, and once the command line has overridden `directory` (some
    /// computed defaults, such as mpdstats' `music_directory`, use it).
    pub fn reload_plugin_defaults(&mut self) {
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let mut view = core_tree();
        for layer in self.layers.iter().rev() {
            view = merge(view, layer.clone());
        }
        view = merge(view, self.cli.clone());
        let plugins = list_in(&view, "plugins");
        let disabled = list_in(&view, "disabled_plugins");
        let names: Vec<String> = plugins
            .into_iter()
            .filter(|p| !disabled.contains(p))
            .collect();
        let mut root = default_tree_for(&names, &view);
        // As `Config::layer`: a user's `replace:` mapping overrides the
        // default rules whole rather than merging into them.
        let has_replace = self
            .layers
            .iter()
            .chain(std::iter::once(&self.cli))
            .any(|y| y.get("replace").is_some_and(Y::is_mapping));
        if has_replace && let Y::Mapping(m) = &mut root {
            m.remove("replace");
        }
        for layer in self.layers.iter().rev() {
            root = merge(root, layer.clone());
        }
        root = merge(root, self.cli.clone());
        self.root = root;
    }

    /// Put a source over everything read so far.
    fn layer(&mut self, y: Y, from: PathBuf) {
        // beets reads `replace` with `get(dict)`, which takes the
        // highest-priority mapping whole: a user's `replace:` replaces the
        // default rules rather than adding to them.
        if let (Y::Mapping(root), Some(r)) = (&mut self.root, y.get("replace"))
            && r.is_mapping()
        {
            root.remove("replace");
        }
        self.root = merge(std::mem::take(&mut self.root), y.clone());
        self.layers.insert(0, y);
        self.sources.insert(0, from);
    }

    /// The defaults plus one file (in its directory), for tests.
    #[cfg(test)]
    pub fn load_file(file: &Path) -> Result<Config> {
        let mut cfg = Config::defaults();
        cfg.config_dir = file.parent().unwrap().to_path_buf();
        cfg.layer(read_yaml(file)?, file.to_path_buf());
        Ok(cfg)
    }

    pub fn defaults() -> Config {
        Config {
            root: default_tree(),
            cli: Y::Mapping(Mapping::new()),
            layers: vec![],
            config_dir: default_config_dir(),
            sources: vec![],
        }
    }

    /// beets' `config -p`: the files read, highest priority first, with
    /// the user's `config.yaml` first when it doesn't exist (yet).
    pub fn source_paths(&self) -> Vec<PathBuf> {
        let mut out = self.sources.clone();
        let user = self.config_dir.join("config.yaml");
        if !out.contains(&user) {
            out.insert(0, user);
        }
        out
    }

    /// beets' `config.dump(full, redact)`: the configuration as confuse
    /// dumps it: the sources' keys in priority order (command line, then
    /// the files, then the defaults if `full`), each value the highest
    /// priority source's, the enabled plugins' sensitive values as
    /// "REDACTED", and the default config's comments before matching keys.
    pub fn dump(&self, full: bool, redact: bool) -> String {
        let sets = self.plugin_set_sources();
        let mut sources: Vec<&Y> = sets.iter().collect();
        sources.push(&self.cli);
        sources.extend(self.layers.iter());
        let core = core_tree();
        if full {
            sources.push(&core);
        }
        let plugins = self.plugin_defaults();
        sources.push(&plugins);
        let mut out = flatten_sources(&sources);
        if redact {
            let plugins = self.list("plugins");
            let disabled = self.list("disabled_plugins");
            let enabled =
                |p: &str| plugins.iter().any(|x| x == p) && !disabled.iter().any(|x| x == p);
            for def in crate::plugin_registry::defs() {
                if !enabled(def.name) {
                    continue;
                }
                for key in def.redacted {
                    set_redacted(&mut out, key);
                }
            }
        }
        if out.as_mapping().is_none_or(Mapping::is_empty) {
            return String::new();
        }
        let body = crate::pyyaml::dump_confuse(&out);
        restore_comments(&body)
    }

    /// The sources beets' plugins `set` while they load, highest
    /// priority first (beets loads the plugins in `plugins` order and
    /// each `set` goes above the previous one).
    fn plugin_set_sources(&self) -> Vec<Y> {
        let names = crate::plugins::names(self);
        let mut out = vec![];
        for name in names.iter().rev() {
            for def in crate::plugin_registry::load_sets() {
                if def.plugin != name {
                    continue;
                }
                let mut source = Y::Mapping(Mapping::new());
                for (key, value) in (def.set)(self) {
                    set_in(&mut source, key, value);
                }
                out.push(source);
            }
        }
        out
    }

    /// The sources beets' plugins add when they load (`config.add`): the
    /// `disabled_plugins` list, then each enabled plugin's sections in
    /// `plugins` order.
    fn plugin_defaults(&self) -> Y {
        let mut out = Mapping::new();
        out.insert(Y::String("disabled_plugins".into()), Y::Sequence(vec![]));
        // Each plugin's own `config.add`, in load (`plugins`) order, with
        // beets' computed values (mpdstats' `music_directory`): an earlier
        // plugin's keys stay first and a later plugin's keys join a shared
        // section at the end (mpdupdate and mpdstats both add `mpd`).
        let names = crate::plugins::names(self);
        let mut view = core_tree();
        for layer in self.layers.iter().rev() {
            view = merge(view, layer.clone());
        }
        view = merge(view, self.cli.clone());
        let ctx = crate::plugin_registry::DefaultsCtx::from_tree(&view);
        let defaults = crate::plugin_registry::defaults_for(&names, &ctx);
        for name in names {
            let Some(def) = crate::plugin_registry::find(&name) else {
                continue;
            };
            for k in def.config_keys {
                if let Some(v) = defaults.get(k) {
                    out.insert(Y::String(k.to_string()), v.clone());
                }
            }
        }
        Y::Mapping(out)
    }

    /// Override a dotted key from the command line.
    pub fn set(&mut self, key: &str, value: Y) {
        set_in(&mut self.cli, key, value.clone());
        set_in(&mut self.root, key, value);
        // The enabled plugins decide which defaults apply.
        if matches!(key, "plugins" | "disabled_plugins") {
            self.rebuild();
        }
    }

    pub fn get(&self, key: &str) -> Option<&Y> {
        let mut cur = &self.root;
        for part in key.split('.') {
            cur = cur.as_mapping()?.get(Y::String(part.to_string()))?;
        }
        Some(cur)
    }

    pub fn str(&self, key: &str) -> String {
        match self.get(key) {
            Some(Y::String(s)) => s.clone(),
            Some(Y::Number(n)) => n.to_string(),
            Some(Y::Bool(b)) => b.to_string(),
            _ => String::new(),
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Y::Bool(b)) => *b,
            Some(Y::String(s)) => crate::model::parse_bool(s).unwrap_or(false),
            Some(Y::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => false,
        }
    }

    pub fn f64(&self, key: &str) -> f64 {
        match self.get(key) {
            Some(Y::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Y::String(s)) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn mapping(&self, key: &str) -> Vec<(String, String)> {
        match self.get(key) {
            Some(Y::Mapping(m)) => m
                .iter()
                .filter_map(|(k, v)| {
                    let k = match k {
                        Y::String(s) => s.clone(),
                        other => serde_yaml::to_string(other).ok()?.trim().to_string(),
                    };
                    let v = match v {
                        Y::String(s) => s.clone(),
                        Y::Null => String::new(),
                        other => serde_yaml::to_string(other).ok()?.trim().to_string(),
                    };
                    Some((k, v))
                })
                .collect(),
            _ => vec![],
        }
    }

    /// A list given either as a YAML sequence or a space-separated string
    /// (beets accepts both, e.g. `plugins: fetchart lyrics`).
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Y::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| match v {
                    Y::String(s) => Some(s.clone()),
                    Y::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect(),
            Some(Y::String(s)) => s.split_whitespace().map(String::from).collect(),
            _ => vec![],
        }
    }

    pub fn directory(&self) -> PathBuf {
        let d = expand_tilde(&self.str("directory"));
        if d.is_absolute() {
            d
        } else {
            std::env::current_dir().unwrap_or_default().join(d)
        }
    }

    pub fn library_path(&self) -> PathBuf {
        let l = expand_tilde(&self.str("library"));
        if l.is_absolute() {
            l
        } else {
            self.config_dir.join(l)
        }
    }

    /// The import state file (`statefile`), relative to the config dir.
    /// beets' default name is `state.pickle`; an older beets-rs wrote its
    /// JSON state as `state.json`, so that file is moved into place when the
    /// default is used and `state.pickle` doesn't exist yet.
    pub fn state_path(&self) -> PathBuf {
        let raw = self.str("statefile");
        let s = expand_tilde(&raw);
        let path = if s.is_absolute() {
            s
        } else {
            self.config_dir.join(s)
        };
        if raw == "state.pickle"
            && !path.exists()
            && let old = self.config_dir.join("state.json")
            && old.exists()
        {
            match std::fs::rename(&old, &path) {
                Ok(()) => crate::logging::debug!(
                    "renamed {} to {} (beets' state file name)",
                    old.display(),
                    path.display()
                ),
                Err(e) => crate::logging::debug!(
                    "could not rename {} to {}: {e}",
                    old.display(),
                    path.display()
                ),
            }
        }
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // beets' plugins that `set` config while loading (`importfeeds`'
    // `relative_to`): the source goes above everything, so the section
    // and its key come first.
    #[test]
    fn plugin_load_time_sets_come_first() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(&file, "directory: /srv/music\nplugins: [importfeeds]\n").unwrap();
        let c = Config::load_file(&file).unwrap();
        for full in [false, true] {
            let out = c.dump(full, true);
            assert_eq!(out.lines().next(), Some("importfeeds:"), "{out}");
            let lines: Vec<&str> = out.lines().collect();
            assert!(lines.contains(&"    relative_to: /srv/music"), "{out}");
            let keys: Vec<&str> = lines
                .iter()
                .skip(1)
                .take_while(|l| l.is_empty() || l.starts_with(' '))
                .filter(|l| l.starts_with("    "))
                .map(|l| l.trim().split(':').next().unwrap())
                .collect();
            assert_eq!(
                keys,
                ["relative_to", "formats", "m3u_name", "dir", "absolute_path"],
                "{out}"
            );
        }
    }

    // As confuse: a section's keys from the higher source first, then the
    // rest in the defaults' order.
    #[test]
    fn merge_keeps_key_order() {
        let y = |s: &str| serde_yaml::from_str::<Y>(s).unwrap();
        let merged = merge(y("{a: 1, b: 2, c: 3, d: 4}"), y("{b: 9}"));
        let keys: Vec<&str> = merged
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap())
            .collect();
        assert_eq!(keys, ["b", "a", "c", "d"]);
    }

    #[test]
    fn merge_user_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(
            &file,
            "directory: /srv/music\nimport:\n  move: yes\npaths:\n  'genre:jazz': Jazz/$album/$title\n",
        )
        .unwrap();
        let c = Config::load_file(&file).unwrap();
        assert_eq!(c.directory(), PathBuf::from("/srv/music"));
        assert!(c.bool("import.move"));
        assert!(c.bool("import.copy"));
        let paths = c.mapping("paths");
        assert_eq!(paths[0].0, "genre:jazz");
        assert!(paths.iter().any(|(k, _)| k == "default"));
        assert_eq!(c.library_path(), dir.path().join("library.db"));
        assert_eq!(c.f64("match.strong_rec_thresh"), 0.04);
        assert_eq!(c.list("plugins"), ["musicbrainz"]);
        assert_eq!(
            c.list("fetchart.sources"),
            vec![
                "filesystem",
                "coverart",
                "itunes",
                "amazon",
                "albumart",
                "cover_art_url"
            ]
        );
    }

    fn write(p: &Path, text: &str) {
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn overlays_and_includes_layer_like_beets() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        write(
            &dir.join("config.yaml"),
            "directory: /a\nlibrary: lib.db\ninclude: [inc1.yaml, inc2.yaml]\nimport:\n  move: yes\n",
        );
        write(
            &dir.join("inc1.yaml"),
            "directory: /b\nlyrics:\n  synced: yes\n",
        );
        write(&dir.join("inc2.yaml"), "directory: /c\n");
        let o1 = dir.join("o1.yaml");
        let o2 = dir.join("sub/o2.yaml");
        std::fs::create_dir(dir.join("sub")).unwrap();
        write(&o1, "directory: /d\nimport:\n  write: no\n");
        write(&o2, "library: other.db\n");

        let c = Config::load_in(dir, &[]).unwrap();
        assert_eq!(c.str("directory"), "/c", "later includes win");
        assert!(c.bool("lyrics.synced"));
        assert!(c.bool("import.move"));

        let c = Config::load_in(dir, &[o1.clone(), o2.clone()]).unwrap();
        assert_eq!(c.str("directory"), "/d", "-c goes over the user config");
        assert!(c.bool("import.move"), "and keeps the rest of it");
        assert!(!c.bool("import.write"));
        assert!(c.bool("import.copy"), "defaults stay underneath");
        // Relative paths still resolve against the config directory.
        assert_eq!(c.library_path(), dir.join("other.db"));
        assert_eq!(
            c.source_paths(),
            [
                o2,
                o1,
                dir.join("inc2.yaml"),
                dir.join("inc1.yaml"),
                dir.join("config.yaml")
            ]
        );

        // A missing -c file is an error, as in beets.
        let e = Config::load_in(dir, &[dir.join("nope.yaml")]).unwrap_err();
        assert!(
            e.to_string().starts_with(&format!(
                "{} could not be read: ",
                dir.join("nope.yaml").display()
            )),
            "{e}"
        );
    }

    #[test]
    fn missing_include_stops_the_includes() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        write(
            &dir.join("config.yaml"),
            "include:\n  - missing.yaml\n  - ok.yaml\n",
        );
        write(&dir.join("ok.yaml"), "directory: /ok\n");
        let c = Config::load_in(dir, &[]).unwrap();
        assert_ne!(c.str("directory"), "/ok");
        assert_eq!(c.source_paths(), [dir.join("config.yaml")]);
    }

    #[test]
    fn source_paths_list_a_missing_user_config_first() {
        let d = tempfile::tempdir().unwrap();
        let o = d.path().join("o.yaml");
        write(&o, "directory: /x\n");
        let c = Config::load_in(d.path(), std::slice::from_ref(&o)).unwrap();
        assert_eq!(c.source_paths(), [d.path().join("config.yaml"), o]);
    }

    #[test]
    fn old_state_json_moves_to_beets_state_pickle() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let mut old = crate::state::ImportState::default();
        old.history.insert(PathBuf::from("/music/a"));
        old.save(&dir.join("state.json")).unwrap();
        let cfg = Config::load_in(dir, &[]).unwrap();
        let p = cfg.state_path();
        assert_eq!(p, dir.join("state.pickle"));
        assert!(!dir.join("state.json").exists(), "the old file is moved");
        assert_eq!(
            crate::state::ImportState::load(&p).history,
            [PathBuf::from("/music/a")].into_iter().collect(),
            "its history is kept"
        );
        // An explicit statefile setting is left alone.
        let d2 = tempfile::tempdir().unwrap();
        let dir2 = d2.path();
        std::fs::write(dir2.join("state.json"), b"{}").unwrap();
        let mut cfg = Config::load_in(dir2, &[]).unwrap();
        cfg.set("statefile", Y::String("state.json".into()));
        assert_eq!(cfg.state_path(), dir2.join("state.json"));
        assert!(dir2.join("state.json").exists());
    }

    #[test]
    fn dump_shows_sources_or_everything_and_redacts() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let c = Config::load_in(dir, &[]).unwrap();
        let empty = c.dump(false, true);
        assert!(
            empty.contains("disabled_plugins: []") && empty.contains("musicbrainz:"),
            "nothing set: the default plugins' sections only"
        );
        assert!(!empty.contains("match:"));
        let full = c.dump(true, true);
        assert!(full.starts_with("# --------------- Main ---------------\n\nlibrary:"));
        assert!(
            full.contains(
                "\n\n# --------------- Plugins ---------------\n\nplugins: [musicbrainz]"
            )
        );
        assert!(
            full.contains("clutter: [Thumbs.DB, .DS_Store]"),
            "short lists are inline"
        );
        assert!(full.contains("\n- lost+found\n"), "longer lists are blocks");
        assert!(full.contains("statefile: state.pickle"));
        assert!(full.contains("strong_rec_thresh: 0.04"));

        write(
            &dir.join("config.yaml"),
            "directory: /m\nplugins: [fetchart]\nfetchart:\n  fanarttv_key: secret\nlyrics:\n  genius_api_key: other\n",
        );
        let mut c = Config::load_in(dir, &[]).unwrap();
        c.set("library", Y::String("cli.db".into()));
        let out = c.dump(false, true);
        let y: Y = serde_yaml::from_str(&out).unwrap();
        assert_eq!(y["directory"], Y::from("/m"));
        assert_eq!(y["library"], Y::from("cli.db"), "command-line values count");
        assert!(y.get("match").is_none(), "no defaults without -d");
        assert_eq!(y["fetchart"]["fanarttv_key"], Y::from("REDACTED"));
        assert_eq!(
            y["fetchart"]["cover_names"],
            serde_yaml::from_str::<Y>("[cover, front, art, album, folder]").unwrap(),
            "an enabled plugin's defaults show"
        );
        // lyrics isn't enabled, so its keys aren't marked for redaction.
        assert_eq!(y["lyrics"]["genius_api_key"], Y::from("other"));
        assert!(y.get("embedart").is_none());

        let clear: Y = serde_yaml::from_str(&c.dump(false, false)).unwrap();
        assert_eq!(clear["fetchart"]["fanarttv_key"], Y::from("secret"));
        let full: Y = serde_yaml::from_str(&c.dump(true, true)).unwrap();
        assert_eq!(full["fetchart"]["fanarttv_key"], Y::from("REDACTED"));
        assert_eq!(full["match"]["strong_rec_thresh"], Y::from(0.04));
    }

    /// beets adds a plugin's defaults when it loads, so a shared section
    /// (`mpd`) holds only the enabled plugins' keys, in `plugins:` order.
    #[test]
    fn plugin_defaults_are_only_for_enabled_plugins() {
        let load = |plugins: &str| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("config.yaml"),
                format!("plugins: [{plugins}]\ndirectory: /srv/music\n"),
            )
            .unwrap();
            Config::load_in(dir.path(), &[]).unwrap()
        };
        let keys = |cfg: &Config| {
            cfg.get("mpd")
                .and_then(|v| v.as_mapping())
                .map(|m| {
                    m.keys()
                        .map(|k| k.as_str().unwrap().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        let mpdupdate = load("mpdupdate");
        assert_eq!(keys(&mpdupdate), ["host", "port", "password"]);
        assert_eq!(mpdupdate.str("mpd.host"), "localhost");
        assert_eq!(mpdupdate.get("mpd.music_directory"), None);

        let mpdstats = load("mpdstats");
        assert_eq!(
            keys(&mpdstats),
            [
                "music_directory",
                "strip_path",
                "rating",
                "rating_mix",
                "host",
                "port",
                "password",
                "played_ratio_threshold"
            ]
        );
        // beets' computed default: the effective music directory.
        assert_eq!(mpdstats.str("mpd.music_directory"), "/srv/music");
        assert_eq!(mpdstats.str("mpd.host"), "localhost");

        let both = load("mpdupdate, mpdstats");
        assert_eq!(
            keys(&both),
            [
                "host",
                "port",
                "password",
                "music_directory",
                "strip_path",
                "rating",
                "rating_mix",
                "played_ratio_threshold"
            ]
        );
        let rev = load("mpdstats, mpdupdate");
        assert_eq!(
            keys(&rev),
            [
                "music_directory",
                "strip_path",
                "rating",
                "rating_mix",
                "host",
                "port",
                "password",
                "played_ratio_threshold"
            ]
        );
        // The user's values still win over every plugin default.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "plugins: [mpdupdate, mpdstats]\ndirectory: /srv/music\nmpd:\n  host: elsewhere\n",
        )
        .unwrap();
        let cfg = Config::load_in(dir.path(), &[]).unwrap();
        assert_eq!(cfg.str("mpd.host"), "elsewhere");
        assert_eq!(cfg.str("mpd.music_directory"), "/srv/music");
    }
}
