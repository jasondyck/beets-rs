//! Ports of beets 2.14's `rewrite` and `advancedrewrite` plugins: template
//! fields that replace fixed fields' values (`$artist` shows the rewritten
//! artist), for items and, where the album has the field, albums.
//!
//! `rewrite:` maps `FIELD REGEX` to a replacement: the value (lowercased)
//! matching the regex from its start is replaced; every rule of a field
//! applies in turn; list fields per value; an `artist` rule also rewrites
//! `albumartist`. `advancedrewrite:` is a list of such single-rule
//! mappings and of `{match: QUERY, replacements: {FIELD: VALUE or list}}`
//! rules (the first matching query wins, after the simple rules).

use crate::config::Config;
use crate::model::{FieldType, Kind, Record};
use crate::query::{Obj, Query};
use anyhow::{Result, bail};
use fancy_regex::Regex;
use serde_yaml::Value as Y;
use std::cell::Cell;
use std::collections::HashMap;

thread_local! {
    static IN_REWRITE: Cell<bool> = const { Cell::new(false) };
}

/// A replacement: a string, or a list for a list field.
#[derive(Debug, Clone, PartialEq)]
pub enum Repl {
    Str(String),
    List(Vec<String>),
}

#[derive(Debug, Default)]
pub struct FieldRules {
    simple: Vec<(Regex, String)>,
    advanced: Vec<(Query, Repl)>,
}

/// One plugin's rules, by field.
#[derive(Debug, Default)]
pub struct Rewriter {
    rules: HashMap<String, FieldRules>,
}

fn is_list(field: &str) -> bool {
    matches!(
        crate::model::field_type(Kind::Item, field),
        Some(FieldType::List(_))
    )
}

/// One of `Item._fields`.
fn item_field(field: &str) -> bool {
    crate::model::field_type(Kind::Item, field).is_some()
}

/// `pattern.lower()` compiled as Python's `re.compile`.
fn compile(pattern: &str) -> Result<Regex> {
    Regex::new(&pattern.to_lowercase())
        .map_err(|e| anyhow::anyhow!("invalid regular expression {pattern:?}: {e}"))
}

impl Rewriter {
    fn rules_for(&mut self, field: &str) -> &mut FieldRules {
        self.rules.entry(field.to_string()).or_default()
    }

    /// The `rewrite` plugin's rules (`rewrite:`).
    pub fn rewrite(cfg: &Config) -> Result<Self> {
        let mut r = Rewriter::default();
        let log = super::PluginLog::new("rewrite");
        for (key, value) in cfg.mapping("rewrite") {
            let Some((field, pattern)) = key.trim().split_once(char::is_whitespace) else {
                bail!("invalid rewrite specification");
            };
            let pattern = pattern.trim_start();
            if !item_field(field) {
                bail!("invalid field name ({field}) in rewriter");
            }
            log.debug(&format!("adding template field {key}"));
            let re = compile(pattern)?;
            r.rules_for(field).simple.push((re.clone(), value.clone()));
            if field == "artist" {
                r.rules_for("albumartist").simple.push((re, value));
            }
        }
        Ok(r)
    }

    /// The `advancedrewrite` plugin's rules (`advancedrewrite:`).
    pub fn advanced(cfg: &Config) -> Result<Self> {
        let corresponding = |f: &str| match f {
            "artist" => Some("albumartist"),
            "artists" => Some("albumartists"),
            "artist_sort" => Some("albumartist_sort"),
            "artists_sort" => Some("albumartists_sort"),
            _ => None,
        };
        let log = super::PluginLog::new("advancedrewrite");
        let mut r = Rewriter::default();
        let rules = match cfg.get("advancedrewrite") {
            None | Some(Y::Null) => return Ok(r),
            Some(Y::Sequence(s)) => s.clone(),
            Some(_) => bail!("advancedrewrite: must be a list"),
        };
        let as_str = |v: &Y| -> Option<String> {
            match v {
                Y::String(s) => Some(s.clone()),
                Y::Number(n) => Some(n.to_string()),
                Y::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
                _ => None,
            }
        };
        for rule in rules {
            let Y::Mapping(m) = rule else {
                bail!("advancedrewrite: each rule must be a mapping");
            };
            let has_match = m.contains_key(Y::String("match".into()));
            if !has_match {
                // A simple rule.
                if m.len() != 1 {
                    bail!(
                        "Simple rewrites must have only one rule, but found multiple entries. Did you forget to prepend a dash (-)?"
                    );
                }
                let (k, v) = m.iter().next().unwrap();
                let key = as_str(k).unwrap_or_default();
                let Some(value) = as_str(v) else {
                    bail!("advancedrewrite: a simple rule's value must be a string");
                };
                let Some((field, pattern)) = key.trim().split_once(char::is_whitespace) else {
                    bail!("Invalid simple rewrite specification {key}");
                };
                let pattern = pattern.trim_start();
                if !item_field(field) {
                    bail!("invalid field name {field} in rewriter");
                }
                log.debug(&format!(
                    "adding simple rewrite '{pattern}' \u{2192} '{value}' for field {field}"
                ));
                let re = compile(pattern)?;
                r.rules_for(field).simple.push((re.clone(), value.clone()));
                if let Some(a) = corresponding(field) {
                    r.rules_for(a).simple.push((re, value));
                }
                continue;
            }
            let get = |k: &str| m.get(Y::String(k.into()));
            let Some(match_str) = get("match").and_then(as_str) else {
                bail!("advancedrewrite: `match` must be a string");
            };
            let Some(Y::Mapping(replacements)) = get("replacements") else {
                bail!("advancedrewrite: `replacements` must be a mapping");
            };
            if replacements.is_empty() {
                bail!("Advanced rewrites must have at least one replacement");
            }
            let parts = crate::paths::shell_split(&match_str);
            let query = Query::parse(Kind::Item, &parts)?;
            for (k, v) in replacements {
                let field = as_str(k).unwrap_or_default();
                if !item_field(&field) {
                    bail!("Invalid field name {field} in rewriter");
                }
                let repl = match v {
                    Y::Sequence(items) => {
                        let items: Vec<String> = items.iter().filter_map(as_str).collect();
                        log.debug(&format!(
                            "adding advanced rewrite to '{}' for field {field}",
                            crate::pyyaml::PyVal::List(
                                items
                                    .iter()
                                    .cloned()
                                    .map(crate::pyyaml::PyVal::Str)
                                    .collect()
                            )
                            .py_repr()
                        ));
                        if !is_list(&field) {
                            bail!(
                                "Field {field} is not a multi-valued field but a list was given: {}",
                                items.join(", ")
                            );
                        }
                        Repl::List(items)
                    }
                    other => {
                        let Some(s) = as_str(other) else {
                            bail!("Invalid type of replacement {other:?} for field {field}");
                        };
                        log.debug(&format!(
                            "adding advanced rewrite to '{s}' for field {field}"
                        ));
                        if is_list(&field) {
                            Repl::List(vec![s])
                        } else {
                            Repl::Str(s)
                        }
                    }
                };
                r.rules_for(&field)
                    .advanced
                    .push((query.clone(), repl.clone()));
                if let Some(a) = corresponding(&field) {
                    r.rules_for(a).advanced.push((query.clone(), repl));
                }
            }
        }
        Ok(r)
    }

    /// Whether this rewriter defines `field` for records of `kind` (album
    /// template fields only for the album's own fields).
    pub fn defines(&self, kind: Kind, field: &str) -> bool {
        self.rules.contains_key(field)
            && (kind == Kind::Item || crate::model::field_type(Kind::Album, field).is_some())
    }

    /// The rewritten value of `field`, if a rule applies.
    pub fn value(&self, rec: &Record, album: Option<&Record>, field: &str) -> Option<Repl> {
        let rules = self.rules.get(field)?;
        // A rule's query that looks at a rewritten field sees the stored
        // value (beets would recurse without end).
        if IN_REWRITE.with(|c| c.get()) {
            return None;
        }
        let orig = if is_list(field) {
            Repl::List(rec.list(field))
        } else if matches!(
            rec.field_type(field),
            Some(FieldType::Str | FieldType::MusicalKey)
        ) {
            Repl::Str(rec.str(field).to_string())
        } else {
            // Not a string: simple rules leave it; the value as stored.
            Repl::Str(rec.get(field).map(|v| v.raw()).unwrap_or_default())
        };
        let textual = is_list(field)
            || matches!(
                rec.field_type(field),
                Some(FieldType::Str | FieldType::MusicalKey)
            );
        let rewrite_one = |s: &str, re: &Regex, repl: &str| -> String {
            if re
                .find(&s.to_lowercase())
                .ok()
                .flatten()
                .is_some_and(|m| m.start() == 0)
            {
                repl.to_string()
            } else {
                s.to_string()
            }
        };
        let mut v = orig.clone();
        if textual {
            for (re, repl) in &rules.simple {
                v = match v {
                    Repl::Str(s) => Repl::Str(rewrite_one(&s, re, repl)),
                    Repl::List(l) => {
                        Repl::List(l.iter().map(|s| rewrite_one(s, re, repl)).collect())
                    }
                };
            }
        }
        if v != orig {
            return Some(v);
        }
        let mut obj = Obj::new(rec);
        obj.album = album;
        IN_REWRITE.with(|c| c.set(true));
        let hit = rules
            .advanced
            .iter()
            .find(|(q, _)| q.matches_obj(&obj))
            .map(|(_, r)| r.clone());
        IN_REWRITE.with(|c| c.set(false));
        hit.filter(|r| *r != orig)
    }

    /// The record with `field` rewritten, for formatting.
    pub fn rewritten(&self, rec: &Record, album: Option<&Record>, field: &str) -> Option<Record> {
        let v = self.value(rec, album, field)?;
        let mut r = rec.clone();
        match v {
            Repl::List(l) => r.set_list(field, &l),
            Repl::Str(s) => {
                let _ = r.set_parsed(field, &s);
            }
        }
        Some(r)
    }
}

pub struct RewritePlugin(pub &'static str);

impl super::Plugin for RewritePlugin {
    fn name(&self) -> &'static str {
        self.0
    }
}

#[cfg(test)]
#[path = "rewrite/tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "rewrite",
        order: 27,
        defaults: r#""#,
        config_keys: &["rewrite"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Rewriter::rewrite(cfg)?;
    Ok(Box::new(RewritePlugin("rewrite")))
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "advancedrewrite",
        order: 28,
        defaults: r#""#,
        config_keys: &["advancedrewrite"],
        redacted: &[],
        make: Some(registry_make_advanced),
        fatal: true,
        commands: &[],
    }
}

fn registry_make_advanced(
    cfg: &crate::config::Config,
) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Rewriter::advanced(cfg)?;
    Ok(Box::new(RewritePlugin("advancedrewrite")))
}
