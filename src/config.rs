//! Configuration: beets-compatible YAML keys layered over built-in defaults.

use anyhow::Result;
use serde_yaml::{Mapping, Value as Y};
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = r#"
directory: ~/Music
library: library.db
statefile: state.json
create_backup_before_migrations: yes

clutter: ["Thumbs.DB", ".DS_Store"]
ignore: [".*", "*~", "System Volume Information", "lost+found"]
ignore_hidden: yes

import:
    write: yes
    copy: yes
    move: no
    link: no
    hardlink: no
    reflink: no
    delete: no
    quiet: no
    quiet_fallback: skip
    none_rec_action: ask
    default_action: apply
    timid: no
    log:
    detail: no
    from_scratch: no
    flat: no
    pretend: no
    set_fields: {}
    autotag: yes
    singletons: no
    group_albums: no
    duplicate_keys:
        album: albumartist album
        item: artist title
    duplicate_action: ask
    duplicate_verbose_prompt: no
    bell: no
    languages: []
    ignored_alias_types: []
    singleton_album_disambig: yes
    incremental: no
    incremental_skip_later: no
    resume: ask

paths:
    default: $albumartist/$album%aunique{}/$track $title
    singleton: Non-Album/$artist/$title
    comp: Compilations/$album%aunique{}/$track $title

threaded: yes

path_sep_replace: _
empty_component_replace:
drive_sep_replace: _
art_filename: cover
replace:
    '[<>:\?\*\|]': _
    '\"': _
    '[\\/]': _
    '^\.': _
    '\.$': _
    '[\x00-\x1f]': _
    '^-': _
    '\s+$': ''
    '^\s+': ''

asciify_paths: no
max_filename_length: 0
per_disc_numbering: no
original_date: no
va_name: Various Artists
format_item: $artist - $album - $title
format_album: $albumartist - $album
time_format: '%Y-%m-%d %H:%M:%S'
sort_item: artist+ album+ disc+ track+
sort_album: albumartist+ album+
sort_case_insensitive: yes

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

plugins: [musicbrainz]

fetchart:
    auto: yes
    fetch_for_asis: no
    minwidth: 0
    maxwidth: 0
    quality: 0
    max_filesize: 0
    enforce_ratio: no
    cautious: no
    cover_names: [cover, front, art, album, folder]
    fallback: null
    sources: [filesystem, coverart, itunes, amazon, albumart, cover_art_url]
    store_source: no
    high_resolution: no
    deinterlace: no
    cover_format: null
    google_key: null
    google_engine: '001442825323518660753:hrh5ch1gjzm'
    lastfm_key: null
    fanarttv_key: null

embedart:
    maxwidth: 0
    auto: yes
    compare_threshold: 0
    ifempty: no
    remove_art_file: no
    quality: 0
    clearart_on_import: no

lyrics:
    auto: yes
    auto_ignore: null
    translate:
        api_key: null
        from_languages: []
        to_language: null
    dist_thresh: 0.11
    google_API_key: null
    google_engine_ID: '009217259823014548361:lndtuqkycfu'
    genius_api_key: Ryq93pUGm8bM6eUWwD_M3NOFFDAtp2yEE7W76V-uFL5jks5dNvcGCdarqFjDhP9c
    lrcmux:
        url: https://api.lrcmux.dev
        sources: []
    fallback: null
    force: no
    keep_synced: no
    local: no
    print: no
    rest_directory: null
    synced: no
    # Musixmatch and Tekstowo are disabled by default as they currently
    # block requests with the beets user agent.
    sources: [lrclib, google, genius, lrcmux]

lastgenre:
    whitelist: yes
    min_weight: 10
    count: 1
    fallback: null
    canonical: no
    cleanup_existing: no
    source: album
    force: no
    keep_existing: no
    auto: yes
    prefer_specific: no
    title_case: yes
    pretend: no
    ignorelist: {}
    aliases: yes

duplicates:
    album: no
    checksum: ''
    copy: ''
    count: no
    delete: no
    format: ''
    full: no
    keys: []
    merge: no
    move: ''
    path: no
    tiebreak: {}
    strict: no
    tag: ''
    remove: no

missing:
    count: no
    total: no
    album: no
    release_types: [album]

the:
    the: yes
    a: yes
    format: '{}, {}'
    strip: no
    patterns: []

zero:
    auto: yes
    fields: []
    keep_fields: []
    update_database: no
    omit_single_disc: no

replaygain:
    overwrite: no
    auto: yes
    backend: command
    threads: CPU_COUNT
    parallel_on_import: no
    per_disc: no
    peak: 'true'
    targetlevel: 89
    r128: [Opus]
    r128_targetlevel: 84

chroma:
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    auto: yes

ftintitle:
    auto: yes
    drop: no
    format: feat. {}
    keep_in_artist: no
    preserve_album_artist: yes
    custom_words: []
    bracket_keywords:
    - abridged
    - acapella
    - club
    - demo
    - edit
    - edition
    - extended
    - instrumental
    - live
    - mix
    - radio
    - release
    - remaster
    - remastered
    - remix
    - rmx
    - unabridged
    - unreleased
    - version
    - vip

scrub:
    auto: yes

convert:
    dest:
    pretend: no
    link: no
    hardlink: no
    threads: CPU_COUNT
    format: mp3
    id3v23: inherit
    write_metadata: yes
    formats:
        aac:
            command: ffmpeg -i $source -y -vn -acodec aac -aq 1 $dest
            extension: m4a
        alac:
            command: ffmpeg -i $source -y -vn -acodec alac $dest
            extension: m4a
        flac: ffmpeg -i $source -y -vn -acodec flac $dest
        mp3: ffmpeg -i $source -y -vn -aq 2 $dest
        opus: ffmpeg -i $source -y -vn -acodec libopus -ab 96k $dest
        ogg: ffmpeg -i $source -y -vn -acodec libvorbis -aq 3 $dest
        wma: ffmpeg -i $source -y -vn -acodec wmav2 -vn $dest
    max_bitrate:
    auto: no
    auto_keep: no
    tmpdir:
    quiet: no
    embed: yes
    paths: {}
    no_convert: ''
    never_convert_lossy_files: no
    copy_album_art: no
    album_art_maxwidth: 0
    delete_originals: no
    playlist:
    force: no
    refresh: no
    keep_new: no

musicbrainz:
    host: musicbrainz.org
    https: no
    ratelimit: 1
    ratelimit_interval: 1
    search_limit: 5
    data_source_mismatch_penalty: 0.5
    search_query_ascii: no
    genres: no
    genres_tag: genre
    external_ids:
        discogs: no
        bandcamp: no
        spotify: no
        deezer: no
        tidal: no
    extra_tags: []
    aliases_as_credits: no
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
    /// Every source but the built-in defaults (what `beet config` shows
    /// without `-d`).
    user: Y,
    /// Directory holding config.yaml; relative `library` paths resolve against it.
    pub config_dir: PathBuf,
    /// Files configuration was read from, highest priority first (beets'
    /// `config.sources`).
    pub sources: Vec<PathBuf>,
}

/// Keys whose values `beet config` hides unless `--clear`, per plugin
/// (beets' `redact` flags, set when the plugin loads).
pub const REDACTED_KEYS: &[(&str, &str)] = &[
    ("fetchart", "fetchart.google_key"),
    ("fetchart", "fetchart.google_engine"),
    ("fetchart", "fetchart.fanarttv_key"),
    ("fetchart", "fetchart.lastfm_key"),
    ("lyrics", "lyrics.translate.api_key"),
    ("lyrics", "lyrics.google_API_key"),
    ("lyrics", "lyrics.google_engine_ID"),
    ("lyrics", "lyrics.genius_api_key"),
    ("chroma", "acoustid.apikey"),
];

/// Plugins whose default settings are in `DEFAULT_CONFIG`: beets adds a
/// plugin's defaults when it loads, so `beet config` shows them for the
/// enabled ones.
const PLUGIN_SECTIONS: &[&str] = &[
    "fetchart",
    "embedart",
    "lyrics",
    "lastgenre",
    "missing",
    "the",
    "zero",
    "chroma",
    "ftintitle",
    "replaygain",
    "duplicates",
    "scrub",
    "convert",
];

/// `DEFAULT_CONFIG` parsed, with the defaults computed at run time
/// (replaygain's `threads`: `os.cpu_count()`).
fn default_tree() -> Y {
    let text = DEFAULT_CONFIG.replace(
        "threads: CPU_COUNT",
        &format!("threads: {}", crate::plugins::replaygain::cpu_count()),
    );
    serde_yaml::from_str(&text).expect("default config parses")
}

/// confuse's `ConfigReadError`: "PATH could not be read: REASON".
fn read_yaml(file: &Path) -> Result<Y> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("{} could not be read: {e}", file.display()))?;
    let y: Y = serde_yaml::from_str(&text)
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
        let k = Y::String(part.to_string());
        if i == parts.len() - 1 {
            map.insert(k, value);
            return;
        }
        if !map.contains_key(&k) {
            map.insert(k.clone(), Y::Mapping(Mapping::new()));
        }
        cur = map.get_mut(&k).unwrap();
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
fn merge(base: Y, over: Y) -> Y {
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

/// Deep-merge `over` onto `base`, keeping `base`'s key order and adding
/// new keys after it (for showing the configuration in file order).
fn merge_in_order(base: Y, over: Y) -> Y {
    match (base, over) {
        (Y::Mapping(mut base), Y::Mapping(over)) => {
            for (k, v) in over {
                match base.get_mut(&k) {
                    Some(b) => *b = merge_in_order(std::mem::take(b), v),
                    None => {
                        base.insert(k, v);
                    }
                }
            }
            Y::Mapping(base)
        }
        (_, over) => over,
    }
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
            user: Y::Mapping(Mapping::new()),
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
        Ok(cfg)
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
        self.user = merge_in_order(std::mem::take(&mut self.user), y);
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
            user: Y::Mapping(Mapping::new()),
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

    /// beets' `config.dump(full, redact)`: the configuration as YAML, with
    /// the defaults only if `full` (else just what the sources and the
    /// command line set, plus the enabled plugins' defaults), and the
    /// enabled plugins' sensitive values as "REDACTED" if `redact`.
    pub fn dump(&self, full: bool, redact: bool) -> String {
        let plugins = self.list("plugins");
        let disabled = self.list("disabled_plugins");
        let enabled = |p: &str| plugins.iter().any(|x| x == p) && !disabled.iter().any(|x| x == p);
        let mut out = if full {
            self.root.clone()
        } else {
            let defaults: Y = default_tree();
            let mut base = Mapping::new();
            for p in PLUGIN_SECTIONS.iter().filter(|p| enabled(p)) {
                if let Some(v) = defaults.get(*p) {
                    base.insert(Y::String(p.to_string()), v.clone());
                }
            }
            // The user's keys first, then the plugin defaults they leave
            // out; the user's values win.
            let order = merge_in_order(self.user.clone(), Y::Mapping(base));
            merge_in_order(order, self.user.clone())
        };
        if redact {
            for (plugin, key) in REDACTED_KEYS {
                if !enabled(plugin) {
                    continue;
                }
                let mut cur = &mut out;
                let parts: Vec<&str> = key.split('.').collect();
                for (i, part) in parts.iter().enumerate() {
                    let Some(v) = cur
                        .as_mapping_mut()
                        .and_then(|m| m.get_mut(Y::String(part.to_string())))
                    else {
                        break;
                    };
                    if i == parts.len() - 1 {
                        *v = Y::String("REDACTED".into());
                        break;
                    }
                    cur = v;
                }
            }
        }
        if out.as_mapping().is_none_or(|m| m.is_empty()) {
            return String::new();
        }
        serde_yaml::to_string(&out).unwrap_or_default()
    }

    /// Override a dotted key from the command line.
    pub fn set(&mut self, key: &str, value: Y) {
        set_in(&mut self.user, key, value.clone());
        set_in(&mut self.root, key, value);
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
    pub fn state_path(&self) -> PathBuf {
        let s = expand_tilde(&self.str("statefile"));
        if s.is_absolute() {
            s
        } else {
            self.config_dir.join(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn dump_shows_sources_or_everything_and_redacts() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let c = Config::load_in(dir, &[]).unwrap();
        assert_eq!(c.dump(false, true), "", "nothing set: empty");
        assert!(c.dump(true, true).contains("strong_rec_thresh"));

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
}
