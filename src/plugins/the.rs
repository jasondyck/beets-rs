//! Port of beets 2.14's `the` plugin (`beetsplug/the.py`): the `%the{}`
//! template function, which moves a leading article to the end ("The
//! Beatles" -> "Beatles, The") or strips it.
//!
//! Config (`the:`): `the` and `a` (the built-in patterns for "the" and
//! "a"/"an", on by default), `patterns` (more regexes, matched without
//! case), `strip` (drop the article instead of moving it) and `format`
//! (a Python format string: `{0}` the rest, `{1}` the article).

use crate::config::Config;
use crate::plugins::PluginLog;
use fancy_regex::Regex;

pub const PATTERN_THE: &str = r"^the\s";
pub const PATTERN_A: &str = r"^[a][n]?\s";
#[cfg(test)]
pub const FORMAT: &str = "{}, {}";

#[derive(Debug)]
pub struct The {
    /// The patterns, as configured (invalid ones too: they never match).
    pub patterns: Vec<String>,
    strip: bool,
    format: String,
}

impl The {
    /// The plugin as it loads (`%the{}` itself lives in the template
    /// environment, `FormatEnv`): warns about invalid patterns and ones that
    /// aren't anchored, as beets does.
    pub fn load(cfg: &Config) -> Self {
        Self::build(cfg, true)
    }

    /// The plugin's settings, without the load-time warnings (for each
    /// template environment).
    pub fn new(cfg: &Config) -> Self {
        Self::build(cfg, false)
    }

    fn build(cfg: &Config, warn: bool) -> Self {
        let log = PluginLog::new("the");
        let mut patterns = cfg.list("the.patterns");
        for p in patterns.iter().filter(|p| !p.is_empty()) {
            if !warn {
                break;
            }
            if Regex::new(p).is_err() {
                log.error(&format!("invalid pattern: {p}"));
            } else if !(p.starts_with('^') || p.ends_with('$')) {
                log.warning(&format!("warning: \"{p}\" will not match string start/end"));
            }
        }
        if cfg.bool("the.a") {
            patterns.insert(0, PATTERN_A.to_string());
        }
        if cfg.bool("the.the") {
            patterns.insert(0, PATTERN_THE.to_string());
        }
        if patterns.is_empty() && warn {
            log.warning("no patterns defined!");
        }
        The {
            patterns,
            strip: cfg.bool("the.strip"),
            format: match cfg.get("the.format") {
                Some(serde_yaml::Value::String(s)) => s.clone(),
                _ => cfg.str("the.format"),
            },
        }
    }

    /// `unthe`: move `pattern`'s match (case-insensitive) in `text` to the
    /// end, as `format` says, or strip it.
    pub fn unthe(&self, text: &str, pattern: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let Ok(re) = Regex::new(&format!("(?i){pattern}")) else {
            // beets raises `re.error` here; the template shows it.
            return text.to_string();
        };
        // `m.findall(text)[0]`: the whole match, or the group if the
        // pattern has one.
        let Ok(Some(caps)) = re.captures(text) else {
            return text.to_string();
        };
        let t = if re.captures_len() == 2 {
            caps.get(1).map_or("", |m| m.as_str())
        } else {
            caps.get(0).map_or("", |m| m.as_str())
        };
        let r = re.replace_all(text, "");
        let r = r.trim();
        if self.strip {
            return r.to_string();
        }
        py_format(&self.format, &[r, t.trim()]).trim().to_string()
    }

    /// The `%the{text}` template function.
    pub fn template_func(&self, text: &str) -> String {
        if self.patterns.is_empty() {
            return text.to_string();
        }
        if text.is_empty() {
            return String::new();
        }
        let mut r = String::new();
        for p in &self.patterns {
            r = self.unthe(text, p);
            if r != text {
                PluginLog::new("the").debug(&format!("\"{text}\" -> \"{r}\""));
                break;
            }
        }
        r
    }

    /// `%the{...}` called with its arguments, Python's errors for the
    /// wrong number of them.
    pub fn call(&self, args: &[String]) -> Result<String, String> {
        let qual = "ThePlugin.the_template_func()";
        match args.len() {
            0 => Err(format!(
                "{qual} missing 1 required positional argument: 'text'"
            )),
            1 => Ok(self.template_func(&args[0])),
            n => Err(format!(
                "{qual} takes 2 positional arguments but {} were given",
                n + 1
            )),
        }
    }
}

impl crate::plugins::Plugin for The {
    fn name(&self) -> &'static str {
        "the"
    }
}

/// Python's `str.format` with positional arguments: `{}` (automatic
/// numbering), `{0}`, `{1}`, `{{` and `}}`. Format specs and conversions
/// are ignored; an unknown index or a malformed field gives the format
/// string as it is.
pub fn py_format(fmt: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut auto = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut field = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    field.push(c);
                }
                if !closed {
                    return fmt.to_string();
                }
                let name = field.split([':', '!']).next().unwrap_or("");
                let idx = if name.is_empty() {
                    auto += 1;
                    auto - 1
                } else {
                    match name.parse::<usize>() {
                        Ok(i) => i,
                        Err(_) => return fmt.to_string(),
                    }
                };
                match args.get(idx) {
                    Some(a) => out.push_str(a),
                    None => return fmt.to_string(),
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    //! Ported from beets' `test/plugins/test_the.py`.
    use super::*;
    use serde_yaml::Value as Y;

    fn cfg(extra: &str) -> Config {
        let mut c = Config::defaults();
        let y: Y = serde_yaml::from_str(extra).unwrap();
        if let Y::Mapping(m) = y {
            for (k, v) in m {
                c.set(&format!("the.{}", k.as_str().unwrap()), v);
            }
        }
        c
    }

    fn plugin() -> The {
        The::new(&Config::defaults())
    }

    #[test]
    fn unthe_with_default_patterns() {
        let p = plugin();
        assert_eq!(p.unthe("", PATTERN_THE), "");
        assert_eq!(p.unthe("The Something", PATTERN_THE), "Something, The");
        assert_eq!(p.unthe("The The", PATTERN_THE), "The, The");
        assert_eq!(p.unthe("The    The", PATTERN_THE), "The, The");
        assert_eq!(p.unthe("The   The   X", PATTERN_THE), "The   X, The");
        assert_eq!(p.unthe("the The", PATTERN_THE), "The, the");
        assert_eq!(p.unthe("Protected The", PATTERN_THE), "Protected The");
        assert_eq!(p.unthe("A Boy", PATTERN_A), "Boy, A");
        assert_eq!(p.unthe("a girl", PATTERN_A), "girl, a");
        assert_eq!(p.unthe("An Apple", PATTERN_A), "Apple, An");
        assert_eq!(p.unthe("An A Thing", PATTERN_A), "A Thing, An");
        assert_eq!(p.unthe("the An Arse", PATTERN_A), "the An Arse");
        assert_eq!(
            p.unthe("TET - Travailleur", PATTERN_THE),
            "TET - Travailleur"
        );
    }

    #[test]
    fn unthe_with_strip() {
        let p = The::new(&cfg("strip: true"));
        assert_eq!(p.unthe("The Something", PATTERN_THE), "Something");
        assert_eq!(p.unthe("An A", PATTERN_A), "A");
    }

    #[test]
    fn template_function_with_defaults() {
        let p = plugin();
        assert_eq!(p.patterns, [PATTERN_THE, PATTERN_A]);
        assert_eq!(p.template_func("The The"), "The, The");
        assert_eq!(p.template_func("An A"), "A, An");
    }

    #[test]
    fn custom_pattern() {
        let mut c = cfg("{patterns: ['^test\\s']}");
        c.set("the.format", Y::String(FORMAT.into()));
        let p = The::new(&c);
        assert_eq!(p.template_func("test passed"), "passed, test");
    }

    #[test]
    fn custom_format() {
        let p = The::new(&cfg(
            "{patterns: ['^the\\s', '^[a][n]?\\s'], format: '{1} ({0})'}",
        ));
        assert_eq!(p.template_func("The A"), "The (A)");
    }

    #[test]
    fn no_patterns_and_empty_text() {
        let p = The::new(&cfg("{the: no, a: no}"));
        assert!(p.patterns.is_empty());
        assert_eq!(p.template_func("The X"), "The X");
        assert_eq!(plugin().template_func(""), "");
        // No pattern matches: the last attempt's result, the text.
        assert_eq!(plugin().template_func("Beatles"), "Beatles");
    }

    #[test]
    fn group_in_pattern_and_formats() {
        let p = The::new(&cfg("{the: no, a: no, patterns: ['^(los)\\s+']}"));
        assert_eq!(p.template_func("Los Lobos"), "Lobos, Los");
        assert_eq!(py_format("{{{}}}-{}", &["a", "b"]), "{a}-b");
        assert_eq!(py_format("{0:>3}|{1!r}", &["a", "b"]), "a|b");
    }

    #[test]
    fn call_arity() {
        let p = plugin();
        assert_eq!(
            p.call(&[]).unwrap_err(),
            "ThePlugin.the_template_func() missing 1 required positional argument: 'text'"
        );
        assert_eq!(
            p.call(&["a".into(), "b".into()]).unwrap_err(),
            "ThePlugin.the_template_func() takes 2 positional arguments but 3 were given"
        );
    }
}

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "the",
        order: 23,
        defaults: r#"the:
    the: yes
    a: yes
    format: '{}, {}'
    strip: no
    patterns: []
"#,
        config_keys: &["the"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(The::load(cfg)))
}
