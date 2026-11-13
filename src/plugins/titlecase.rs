//! beets' `titlecase` plugin: NYT-style title casing for imported
//! metadata, a `%titlecase{}` template function and `beet titlecase`.

use crate::config::Config;
use crate::library::Library;
use crate::mb::{AlbumInfo, InfoListener, TrackInfo};
use crate::model::Record;
use crate::plugins::{Plugin, PluginLog};
use crate::query::Query;
use anyhow::Result;
use regex::Regex;
use std::sync::Arc;


#[derive(Debug)]
pub struct Titlecase {
    auto: bool,
    after_choice: bool,
    preserve_words: Vec<(String, String)>,
    preserve_phrases: Vec<(String, Regex)>,
    fields: Vec<String>,
    replace: Vec<(String, String)>,
    separators: String,
    force_lowercase: bool,
    small_first_last: bool,
    the_artist: bool,
    all_caps: bool,
    all_lowercase: bool,
}

impl Titlecase {
    pub fn new(cfg: &Config) -> Self {
        let mut preserve_words = vec![];
        let mut preserve_phrases = vec![];
        for s in cfg.list("titlecase.preserve") {
            if s.contains(' ') {
                if let Ok(re) = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(&s))) {
                    preserve_phrases.push((s, re));
                }
            } else {
                preserve_words.push((s.to_uppercase(), s));
            }
        }
        // `replace` as confuse's `Pairs`: a list of two-element lists,
        // single-element mappings, or plain strings (replacement "").
        let replace = match cfg.get("titlecase.replace") {
            Some(serde_yaml::Value::Sequence(items)) => items
                .iter()
                .filter_map(|item| match item {
                    serde_yaml::Value::Sequence(pair) if pair.len() == 2 => {
                        Some((scalar(&pair[0]), scalar(&pair[1])))
                    }
                    serde_yaml::Value::Mapping(m) if m.len() == 1 => {
                        m.iter().next().map(|(k, v)| (scalar(k), scalar(v)))
                    }
                    serde_yaml::Value::String(s) => Some((s.clone(), String::new())),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        let separators = cfg.list("titlecase.separators").concat();
        Titlecase {
            auto: cfg.bool("titlecase.auto"),
            after_choice: cfg.bool("titlecase.after_choice"),
            preserve_words,
            preserve_phrases,
            fields: cfg.list("titlecase.fields"),
            replace,
            separators,
            force_lowercase: cfg.bool("titlecase.force_lowercase"),
            small_first_last: cfg.bool("titlecase.small_first_last"),
            the_artist: cfg.bool("titlecase.the_artist"),
            all_caps: cfg.bool("titlecase.all_caps"),
            all_lowercase: cfg.bool("titlecase.all_lowercase"),
        }
    }

    /// `TitlecasePlugin.titlecase`: the configured casing of `text`.
    pub fn titlecase(&self, text: &str, field: &str) -> String {
        // Split at configured separators (each chunk is cased on its own).
        if !self.separators.is_empty()
            && let Ok(re) = separator_regex(&self.separators)
        {
            let mut out = String::new();
            let mut consumed = 0;
            let mut any = false;
            for caps in re.captures_iter(text) {
                let whole = caps.get(0).unwrap();
                // beets' regex has a `(?=.)` lookahead: a chunk that ends
                // the string is left to the normal path below.
                if whole.end() >= text.len() {
                    continue;
                }
                out.push_str(&self.titlecase(&caps[1], field));
                out.push_str(&caps[2]);
                consumed = whole.end();
                any = true;
            }
            if any {
                return out + &self.titlecase(&text[consumed..], field);
            }
        }
        // Check if A-Z is all uppercase or all lowercase.
        if self.all_lowercase && is_lower(text) {
            return text.to_string();
        }
        if self.all_caps && is_upper(text) {
            return text.to_string();
        }
        // Any necessary replacements go first, mainly punctuation.
        let mut titlecased = if self.force_lowercase {
            text.to_lowercase()
        } else {
            text.to_string()
        };
        for (target, replacement) in &self.replace {
            titlecased = titlecased.replace(target, replacement);
        }
        let mut out = self.titlecase_text(&titlecased, self.small_first_last);
        // Apply the "The Artist" feature.
        if self.the_artist && field.contains("artist") {
            out = Regex::new(r"\bthe\b")
                .unwrap()
                .replace_all(&out, "The")
                .into_owned();
        }
        // More complicated phrase replacements.
        for (phrase, re) in &self.preserve_phrases {
            out = re.replace_all(&out, phrase.as_str()).into_owned();
        }
        out
    }

    /// The preserved-case callback of python-titlecase: a word in
    /// `titlecase.preserve` keeps its configured case.
    fn preserve_word(&self, word: &str) -> Option<String> {
        let upper = word.to_uppercase();
        self.preserve_words
            .iter()
            .find(|(u, _)| *u == upper)
            .map(|(_, p)| p.clone())
    }

    /// A faithful port of python-titlecase 2.4.1's `titlecase()` (the
    /// algorithm beets' plugin uses), including its punctuation and
    /// initialism rules.
    fn titlecase_text(&self, text: &str, small_first_last: bool) -> String {
        let p = patterns();
        let mut processed: Vec<String> = vec![];
        for line in p.line_break.split(text) {
            let all_caps = line.to_uppercase() == line;
            // (the word, whether a callback/preserved word made it immutable)
            let mut tokens: Vec<(String, bool)> = vec![];
            for word in line.split(['\t', ' ']) {
                if let Some(preserved) = self.preserve_word(word) {
                    tokens.push((preserved, true));
                    continue;
                }
                if all_caps && p.uc_initials.is_match(word) {
                    tokens.push((word.to_string(), false));
                    continue;
                }
                if p.apos_second.is_match(word) && word.chars().count() >= 3 {
                    tokens.push((apos_second(word), false));
                    continue;
                }
                if let Some(caps) = p.mac_mc.captures(word) {
                    let first = capitalize_first(&caps[1]);
                    let rest = self.titlecase_text(&caps[2], true);
                    tokens.push((format!("{first}{rest}"), false));
                    continue;
                }
                if p.mr_mrs_ms_dr.is_match(word) {
                    tokens.push((capitalize_first_keep_rest(word), false));
                    continue;
                }
                if p.inline_period.is_match(word) || (!all_caps && p.uc_elsewhere.is_match(word)) {
                    tokens.push((word.to_string(), false));
                    continue;
                }
                if p.small_words.is_match(word) {
                    tokens.push((word.to_lowercase(), false));
                    continue;
                }
                if word.contains('/') && !word.contains("//") {
                    let joined = word
                        .split('/')
                        .map(|t| self.titlecase_text(t, false))
                        .collect::<Vec<_>>()
                        .join("/");
                    tokens.push((joined, false));
                    continue;
                }
                if word.contains('-') {
                    let joined = word
                        .split('-')
                        .map(|t| self.titlecase_text(t, false))
                        .collect::<Vec<_>>()
                        .join("-");
                    tokens.push((joined, false));
                    continue;
                }
                let w = if all_caps {
                    word.to_lowercase()
                } else {
                    word.to_string()
                };
                if p.consonants.is_match(&w) && w.chars().count() > 2 {
                    tokens.push((w.to_uppercase(), false));
                    continue;
                }
                // Just a normal word that needs to be capitalized.
                tokens.push((
                    p.capfirst
                        .replace(&w, |c: &regex::Captures| c[0].to_uppercase())
                        .into_owned(),
                    false,
                ));
            }
            if small_first_last && !tokens.is_empty() {
                if !tokens[0].1 {
                    let word = tokens[0].0.clone();
                    tokens[0].0 = p
                        .small_first
                        .replace(&word, |c: &regex::Captures| {
                            format!("{}{}", &c[1], capitalize_first(&c[2]))
                        })
                        .into_owned();
                }
                let last = tokens.len() - 1;
                if !tokens[last].1 {
                    let word = tokens[last].0.clone();
                    tokens[last].0 = p
                        .small_last
                        .replace(&word, |c: &regex::Captures| {
                            capitalize_first_keep_rest(&c[0])
                        })
                        .into_owned();
                }
            }
            let mut result = tokens
                .into_iter()
                .map(|(w, _)| w)
                .collect::<Vec<_>>()
                .join(" ");
            result = p
                .subphrase
                .replace_all(&result, |c: &regex::Captures| {
                    format!("{}{}", &c[1], capitalize_first(&c[2]))
                })
                .into_owned();
            processed.push(result);
        }
        processed.join("\n")
    }

    /// `titlecase_fields` for a library item (or any record).
    pub fn titlecase_record(&self, rec: &mut Record) -> bool {
        let mut changed = false;
        for field in &self.fields {
            if matches!(rec.field_type(field), Some(crate::model::FieldType::List(_))) {
                let vals = rec.list(field);
                if !vals.is_empty() {
                    let cased: Vec<String> =
                        vals.iter().map(|s| self.titlecase(s, field)).collect();
                    if cased != vals {
                        rec.set_list(field, &cased);
                        changed = true;
                    }
                }
                continue;
            }
            let Some(value) = rec.get(field) else {
                continue;
            };
            if let crate::model::Value::Str(s) = value
                && !s.is_empty()
            {
                let cased = self.titlecase(&s, field);
                if cased != s {
                    rec.set(field, crate::model::Value::Str(cased));
                    changed = true;
                }
            }
        }
        changed
    }

    fn titlecase_info(&self, info: &mut TrackInfo) {
        for field in &self.fields {
            match field.as_str() {
                "title" => info.title = self.titlecase(&info.title, field),
                "artist" => info.artist = self.titlecase(&info.artist, field),
                "artist_credit" => info.artist_credit = self.titlecase(&info.artist_credit, field),
                "artists" => {
                    info.artists = info.artists.iter().map(|s| self.titlecase(s, field)).collect()
                }
                "artists_sort" => {
                    info.artists_sort =
                        info.artists_sort.iter().map(|s| self.titlecase(s, field)).collect()
                }
                _ => {}
            }
        }
    }
}

/// The regexes of python-titlecase 2.4.1.
struct Patterns {
    small_words: Regex,
    small_first: Regex,
    small_last: Regex,
    subphrase: Regex,
    mac_mc: Regex,
    mr_mrs_ms_dr: Regex,
    inline_period: Regex,
    uc_elsewhere: Regex,
    capfirst: Regex,
    apos_second: Regex,
    uc_initials: Regex,
    consonants: Regex,
    line_break: Regex,
}

const SMALL_RE: &str = r"a|an|and|as|at|but|by|en|for|if|in|of|on|or|the|to|v\.?|via|vs\.?";

fn punct_class() -> String {
    // python-titlecase's PUNCT set, escaped for a Rust regex character class.
    let chars = "!\"“#$%&'‘()*+,-–‒—―./:;?@[\\]_`{|}~";
    let mut out = String::new();
    for c in chars.chars() {
        if "\\^-][".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn patterns() -> &'static Patterns {
    static PATTERNS: std::sync::OnceLock<Patterns> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        let punct = punct_class();
        let re = |s: String| Regex::new(&s).expect("titlecase regex");
        Patterns {
            small_words: re(format!(r"(?i)^({SMALL_RE})$")),
            small_first: re(format!(r"(?i)^([{punct}]*)({SMALL_RE})\b")),
            small_last: re(format!(r"(?i)\b({SMALL_RE})[{punct}]?$")),
            subphrase: re(format!(r"([:.;?!\-–‒—―][ ])({SMALL_RE})")),
            mac_mc: re(r"^([Mm]c|MC)(\w.+)".to_string()),
            mr_mrs_ms_dr: re(r"(?i)^((m((rs?)|s))|Dr)$".to_string()),
            inline_period: re(r"(?i)[\p{L}][.][\p{L}]".to_string()),
            uc_elsewhere: re(format!(r"[{punct}]*?[\p{{L}}]+[\p{{Lu}}]+?")),
            capfirst: re(format!(r"^[{punct}]*?([\p{{L}}])")),
            apos_second: re(r"(?i)^[dol]{1}['‘]{1}[\p{L}]+(?:['s]{2})?$".to_string()),
            uc_initials: re(
                r"^(?:[\p{Lu}]{1}\.{1}|[\p{Lu}]{1}\.{1}[\p{Lu}]{1})+$".to_string(),
            ),
            consonants: re(r"(?i)^[b-df-hj-np-tv-xz]+$".to_string()),
            line_break: re(r"[\r\n]+".to_string()),
        }
    })
}

/// The separator regex of beets' plugin: chunks end at the separators plus
/// the whitespace after them.
fn separator_regex(separators: &str) -> Result<Regex> {
    let mut class = String::new();
    for c in separators.chars() {
        if "\\^-][".contains(c) {
            class.push('\\');
        }
        class.push(c);
    }
    Ok(Regex::new(&format!(r"(.*?[{class}]+)(\s*)"))?)
}

fn is_lower(s: &str) -> bool {
    s.chars().any(char::is_lowercase) && !s.chars().any(char::is_uppercase)
}
fn is_upper(s: &str) -> bool {
    s.chars().any(char::is_uppercase) && !s.chars().any(char::is_lowercase)
}

/// Python's `str.capitalize()`.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `word[0].upper() + word[1:]` (used for Mr/Mrs/Ms/Dr).
fn capitalize_first_keep_rest(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `APOS_SECOND`: `d'Angelo`, `l'Arc`, `o'Reily`.
fn apos_second(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let first = if !matches!(chars[0].to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u') {
        chars[0].to_lowercase().to_string()
    } else {
        chars[0].to_uppercase().to_string()
    };
    let mut out = first;
    out.push(chars[1]);
    out.extend(chars[2].to_uppercase());
    out.extend(&chars[3..]);
    out
}

fn scalar(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

pub struct TitlecasePlugin {
    pub core: Arc<Titlecase>,
}

impl TitlecasePlugin {
    pub fn new(cfg: &Config) -> Self {
        let core = Arc::new(Titlecase::new(cfg));
        if core.auto && !core.after_choice {
            crate::mb::add_info_listener(core.clone());
        }
        TitlecasePlugin { core }
    }
}

impl Plugin for TitlecasePlugin {
    fn name(&self) -> &'static str {
        "titlecase"
    }

    fn import_stage(
        &self,
        ctx: &crate::plugins::PluginCtx,
        task: &mut crate::plugins::ImportTask,
    ) -> Result<()> {
        if !self.core.auto || !self.core.after_choice {
            return Ok(());
        }
        for item in task.items.iter_mut() {
            if self.core.titlecase_record(item) {
                ctx.lib.store(item)?;
            }
        }
        Ok(())
    }
}

impl InfoListener for Titlecase {
    fn album_info(&self, info: &mut AlbumInfo) {
        for field in &self.fields {
            match field.as_str() {
                "album" => info.album = self.titlecase(&info.album, field),
                "artist" => info.artist = self.titlecase(&info.artist, field),
                "artist_credit" => info.artist_credit = self.titlecase(&info.artist_credit, field),
                "artists" => {
                    info.artists = info.artists.iter().map(|s| self.titlecase(s, field)).collect()
                }
                "artists_sort" => {
                    info.artists_sort =
                        info.artists_sort.iter().map(|s| self.titlecase(s, field)).collect()
                }
                _ => {}
            }
        }
        for t in &mut info.tracks {
            self.titlecase_info(t);
        }
    }

    fn track_info(&self, info: &mut TrackInfo) {
        self.titlecase_info(info);
    }
}

pub struct CmdOpts {
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let core = Titlecase::new(cfg);
    let mut items = lib.items(&Query::parse(crate::model::Kind::Item, &opts.query)?)?;
    if let Some(n) = opts.limit {
        items.truncate(n);
    }
    let write = cfg.bool("import.write");
    let log = PluginLog::new("titlecase");
    for mut item in items {
        log.info(&format!("titlecasing {}:", item.str("title")));
        if core.titlecase_record(&mut item) {
            lib.store(&item)?;
            if write {
                crate::media::write_item(&mut item)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "titlecase_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "titlecase",
        order: 46,
        defaults: r#"titlecase:
    auto: yes
    preserve: []
    fields: []
    replace: []
    separators: []
    force_lowercase: no
    small_first_last: yes
    the_artist: yes
    all_caps: no
    all_lowercase: no
    after_choice: no
"#,
        config_keys: &["titlecase"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[crate::plugin_registry::PluginCommand {
            order: 510,
            plugin: "titlecase",
            build: build_titlecase,
            run: run_titlecase,
        }],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(TitlecasePlugin::new(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct TitlecaseArgs {
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_titlecase() -> clap::Command {
    use clap::Args as _;
    TitlecaseArgs::augment_args(
        clap::Command::new("titlecase")
            .about("Apply titlecasing to metadata (plugin: titlecase)"),
    )
}

pub fn run_titlecase(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = TitlecaseArgs::from_arg_matches(m)?;
    crate::plugins::titlecase::command(
        cfg,
        lib,
        crate::plugins::titlecase::CmdOpts {
            limit: a.limit,
            query: a.query,
        },
    )
}
