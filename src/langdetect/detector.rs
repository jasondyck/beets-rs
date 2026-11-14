//! langdetect's `Detector`: the randomized language detection (n-gram
//! features, alpha smoothing, seven trials and convergence), seeded with 0
//! as beets does so that results are deterministic.
//!
//! A port of langdetect 1.0.9's `detector.py` (Apache-2.0).

use super::ngram::{NGram, normalize_vi};
use super::profile::{Profiles, profiles};
use super::python_random::PyRandom;
use regex::Regex;
use std::sync::LazyLock;

/// `Detector.ITERATION_LIMIT`.
const ITERATION_LIMIT: u32 = 1000;
/// `Detector.PROB_THRESHOLD`.
const PROB_THRESHOLD: f64 = 0.1;
/// `Detector.CONV_THRESHOLD`.
const CONV_THRESHOLD: f64 = 0.99999;
/// `Detector.BASE_FREQ`.
const BASE_FREQ: f64 = 10000.0;
/// `Detector.ALPHA_DEFAULT` / `ALPHA_WIDTH`.
const ALPHA_DEFAULT: f64 = 0.5;
const ALPHA_WIDTH: f64 = 0.05;
/// `Detector.UNKNOWN_LANG`.
pub const UNKNOWN_LANG: &str = "unknown";
/// `DetectorFactory.seed`, which beets sets to 0.
pub const SEED: i64 = 0;

static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[-_.?&~;+=/#0-9A-Za-z]{1,2076}").unwrap());
static MAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[-_.0-9A-Za-z]{1,64}@[-_0-9A-Za-z]{1,255}[-_.0-9A-Za-z]{1,255}").unwrap()
});

/// `langdetect.language.Language`: one language and its probability.
#[derive(Clone, Debug, PartialEq)]
pub struct Language {
    pub lang: String,
    pub prob: f64,
}

pub struct Detector {
    profiles: &'static Profiles,
    text: String,
    /// `Detector.n_trial`.
    n_trial: u32,
    /// `Detector.max_text_length`.
    max_text_length: usize,
    /// `Detector.alpha`.
    alpha: f64,
    random: PyRandom,
}

impl Detector {
    pub fn new() -> Self {
        Detector {
            profiles: profiles(),
            text: String::new(),
            n_trial: 7,
            max_text_length: 10000,
            alpha: ALPHA_DEFAULT,
            random: PyRandom::new(),
        }
    }

    /// `Detector.append`: clean URLs, e-mail addresses and Vietnamese
    /// decomposition, then keep at most `max_text_length` characters with
    /// runs of spaces collapsed.
    pub fn append(&mut self, text: &str) {
        let text = URL_RE.replace_all(text, " ");
        let text = MAIL_RE.replace_all(&text, " ");
        let text = normalize_vi(&text);
        let mut pre = ' ';
        for ch in text.chars().take(self.max_text_length) {
            if ch != ' ' || pre != ' ' {
                self.text.push(ch);
            }
            pre = ch;
        }
    }

    /// `cleaning_text`: drop Latin text when the rest is mostly non-Latin.
    fn cleaning_text(&mut self) {
        let mut latin_count = 0u32;
        let mut non_latin_count = 0u32;
        for ch in self.text.chars() {
            if ('A'..='z').contains(&ch) {
                latin_count += 1;
            } else if ch >= '\u{0300}' && !('\u{1e00}'..='\u{1eff}').contains(&ch) {
                non_latin_count += 1;
            }
        }
        if latin_count * 2 < non_latin_count {
            self.text.retain(|ch| !('A'..='z').contains(&ch));
        }
    }

    /// `_extract_ngrams`: the n-grams of the text that have a profile,
    /// with repetitions, as indices into the profile's rows.
    fn extract_ngrams(&self) -> Vec<u32> {
        let mut result = vec![];
        let mut ngram = NGram::new();
        for ch in self.text.chars() {
            ngram.add_char(ch);
            if ngram.capitalword() {
                continue;
            }
            for n in 1..=NGram::MAX {
                if let Some(w) = ngram.get(n)
                    && let Some(&index) = self.profiles.words.get(w)
                {
                    result.push(index);
                }
            }
        }
        result
    }

    /// `_update_lang_prob`: multiply every language's probability by its
    /// smoothed n-gram probability.
    fn update_lang_prob(&self, prob: &mut [f64], word: u32, alpha: f64) {
        let weight = alpha / BASE_FREQ;
        let mut map = [0.0f64; MAX_LANGS];
        for &(lang, p) in &self.profiles.probs[word as usize] {
            map[lang as usize] = p;
        }
        for (p, m) in prob.iter_mut().zip(map.iter()) {
            *p *= weight + m;
        }
    }

    /// `_normalize_prob`: normalize in place and return the maximum.
    fn normalize_prob(prob: &mut [f64]) -> f64 {
        let sump: f64 = prob.iter().sum();
        let mut maxp = 0.0;
        for p in prob.iter_mut() {
            let v = *p / sump;
            if maxp < v {
                maxp = v;
            }
            *p = v;
        }
        maxp
    }

    /// `_sort_probability`: languages above the probability threshold, most
    /// probable first.
    fn sort_probability(&self, prob: &[f64]) -> Vec<Language> {
        let mut result: Vec<Language> = self
            .profiles
            .langs
            .iter()
            .zip(prob)
            .filter(|&(_, &p)| p > PROB_THRESHOLD)
            .map(|(lang, &p)| Language {
                lang: lang.clone(),
                prob: p,
            })
            .collect();
        // Python's `sort(reverse=True)` is stable, so ties keep the language
        // list's order.
        result.sort_by(|a, b| {
            b.prob
                .partial_cmp(&a.prob)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }

    /// `get_probabilities`: `None` for langdetect's `LangDetectException`
    /// (no features in the text).
    pub fn get_probabilities(&mut self) -> Option<Vec<Language>> {
        self.cleaning_text();
        let ngrams = self.extract_ngrams();
        if ngrams.is_empty() {
            return None;
        }
        let n_langs = self.profiles.langs.len();
        let mut langprob = vec![0.0f64; n_langs];

        self.random.seed(SEED);
        for _ in 0..self.n_trial {
            let mut prob = vec![1.0 / n_langs as f64; n_langs];
            let alpha = self.alpha + self.random.gauss() * ALPHA_WIDTH;
            let mut i = 0u32;
            loop {
                let word = *self.random.choice(&ngrams);
                self.update_lang_prob(&mut prob, word, alpha);
                if i.is_multiple_of(5)
                    && (Self::normalize_prob(&mut prob) > CONV_THRESHOLD || i >= ITERATION_LIMIT)
                {
                    break;
                }
                i += 1;
            }
            for (j, p) in prob.iter().enumerate() {
                langprob[j] += p / f64::from(self.n_trial);
            }
        }
        Some(self.sort_probability(&langprob))
    }

    /// `detect`: the most probable language (`"unknown"` when none is above
    /// the threshold), or `None` when langdetect raises.
    pub fn detect(&mut self) -> Option<String> {
        self.get_probabilities().map(|probabilities| {
            probabilities.first().map_or_else(
                || UNKNOWN_LANG.to_string(),
                |language| language.lang.clone(),
            )
        })
    }
}

impl Default for Detector {
    fn default() -> Self {
        Self::new()
    }
}

/// The profile array size: langdetect ships 55 languages.
const MAX_LANGS: usize = 128;

/// `langdetect.detect(text)` with `DetectorFactory.seed = 0`.
pub fn detect(text: &str) -> Option<String> {
    let mut detector = Detector::new();
    detector.append(text);
    detector.detect()
}
