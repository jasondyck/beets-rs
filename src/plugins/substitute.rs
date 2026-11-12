//! Port of beets 2.14's `substitute` plugin: the `%substitute{text}`
//! template function, which applies the `substitute:` rules (regex ->
//! replacement, without case, in config order) in turn.

use crate::config::Config;
use anyhow::Result;
use fancy_regex::Regex;

#[derive(Debug)]
pub struct Substitute {
    rules: Vec<(Regex, String)>,
}

impl Substitute {
    pub fn new(cfg: &Config) -> Result<Self> {
        let mut rules = vec![];
        for (k, v) in cfg.mapping("substitute") {
            let re = Regex::new(&format!("(?i){k}"))
                .map_err(|e| anyhow::anyhow!("invalid regular expression {k:?}: {e}"))?;
            rules.push((re, crate::formatted::python_repl(&v)));
        }
        Ok(Substitute { rules })
    }

    /// `tmpl_substitute`.
    pub fn apply(&self, text: &str) -> String {
        let mut text = text.to_string();
        for (re, repl) in &self.rules {
            text = re.replace_all(&text, repl.as_str()).into_owned();
        }
        text
    }

    /// `%substitute{...}` with Python's errors for the wrong arity.
    pub fn call(&self, args: &[String]) -> Result<String, String> {
        let q = "Substitute.tmpl_substitute()";
        match args.len() {
            0 => Err(format!(
                "{q} missing 1 required positional argument: 'text'"
            )),
            1 => Ok(self.apply(&args[0])),
            n => Err(format!(
                "{q} takes 2 positional arguments but {} were given",
                n + 1
            )),
        }
    }
}

pub struct SubstitutePlugin;

impl super::Plugin for SubstitutePlugin {
    fn name(&self) -> &'static str {
        "substitute"
    }
}

#[cfg(test)]
mod tests {
    //! Ported from beets' test/plugins/test_substitute.py.
    use super::*;

    fn run(rules: &str, cases: &[(&str, &str)]) {
        let mut cfg = Config::defaults();
        cfg.set("substitute", serde_yaml::from_str(rules).unwrap());
        let s = Substitute::new(&cfg).unwrap();
        for (input, want) in cases {
            assert_eq!(s.apply(input), *want, "{input}");
        }
    }

    #[test]
    fn simple_substitute() {
        run("{a: x, b: y, c: z}", &[("a", "x"), ("b", "y"), ("c", "z")]);
    }

    #[test]
    fn case_insensitivity() {
        run("{a: x}", &[("A", "x")]);
    }

    #[test]
    fn unmatched_input_preserved() {
        run("{a: x}", &[("c", "c")]);
    }

    #[test]
    fn regex_to_static() {
        run(
            "{'.*jimi hendrix.*': Jimi Hendrix}",
            &[("The Jimi Hendrix Experience", "Jimi Hendrix")],
        );
    }

    #[test]
    fn regex_capture_group() {
        run(
            r"{'^(.*?)(,| &| and).*': '\1'}",
            &[
                ("King Creosote & Jon Hopkins", "King Creosote"),
                (
                    "Michael Hurley, The Holy Modal Rounders, Jeffrey Frederick & The Clamtones",
                    "Michael Hurley",
                ),
                ("James Yorkston and the Athletes", "James Yorkston"),
            ],
        );
    }

    #[test]
    fn partial_substitution() {
        run(r"{'\.': ''}", &[("U.N.P.O.C.", "UNPOC")]);
    }

    #[test]
    fn rules_applied_in_definition_order() {
        run("{a: x, '[ab]': y, b: z}", &[("a", "x"), ("b", "y")]);
    }

    #[test]
    fn rules_applied_in_sequence() {
        run("{a: b, b: c, d: a}", &[("a", "c"), ("b", "c"), ("d", "a")]);
    }
}
