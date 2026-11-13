//! beets' `fuzzy` plugin: the `~` query prefix (difflib's
//! `SequenceMatcher` string match).

use crate::config::Config;
use crate::plugins::Plugin;

pub struct FuzzyPlugin;

impl FuzzyPlugin {
    pub fn new(_cfg: &Config) -> Self {
        FuzzyPlugin
    }
}

impl Plugin for FuzzyPlugin {
    fn name(&self) -> &'static str {
        "fuzzy"
    }
}

/// `SequenceMatcher.ratio()` for two strings, as beets' `FuzzyQuery`.
fn ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matched: usize = crate::diff::opcodes(&a, &b)
        .iter()
        .filter(|(op, _, _, _, _)| *op == crate::diff::Op::Equal)
        .map(|(_, i1, i2, _, _)| i2 - i1)
        .sum();
    2.0 * matched as f64 / total as f64
}

/// `SequenceMatcher.quick_ratio()` (an upper bound on [`ratio`]).
fn quick_ratio(a: &str, b: &str) -> f64 {
    use std::collections::HashMap;
    let mut counts: HashMap<char, i64> = HashMap::new();
    for c in b.chars() {
        *counts.entry(c).or_default() += 1;
    }
    let mut matched = 0i64;
    for c in a.chars() {
        if let Some(n) = counts.get_mut(&c)
            && *n > 0
        {
            *n -= 1;
            matched += 1;
        }
    }
    let total = a.chars().count() + b.chars().count();
    if total == 0 {
        1.0
    } else {
        2.0 * matched as f64 / total as f64
    }
}

fn is_lower(s: &str) -> bool {
    let mut cased = false;
    for c in s.chars() {
        if c.is_uppercase() {
            return false;
        }
        if c.is_lowercase() {
            cased = true;
        }
    }
    cased
}

/// `FuzzyQuery.string_match`: smartcase, then a SequenceMatcher ratio at
/// the configured threshold (lowered when the pattern is shorter than the
/// value, so it may match a substring).
pub fn matches(pattern: &str, val: &str) -> bool {
    let val = if is_lower(pattern) {
        val.to_lowercase()
    } else {
        val.to_string()
    };
    let threshold = crate::config::Config::defaults()
        .str("fuzzy.threshold")
        .parse::<f64>()
        .unwrap_or(0.7);
    // The threshold in effect, as beets reads the global config.
    let threshold = crate::query::context()
        .fuzzy
        .map(|(_, t)| t)
        .unwrap_or(threshold);
    let threshold = if pattern.chars().count() < val.chars().count() {
        let p = pattern.chars().count() as f64;
        let v = val.chars().count() as f64;
        threshold * (2.0 * p / (p + v))
    } else {
        threshold
    };
    quick_ratio(pattern, &val) >= threshold && ratio(pattern, &val) >= threshold
}

#[cfg(test)]
#[path = "fuzzy_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "fuzzy",
        order: 43,
        defaults: r#"fuzzy:
    prefix: '~'
    threshold: 0.7
"#,
        config_keys: &["fuzzy"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(FuzzyPlugin::new(cfg)))
}
