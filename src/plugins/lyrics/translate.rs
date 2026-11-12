//! beets' `Translator`: lyrics translated line by line with Microsoft's
//! (Bing) Translator API, when `translate.api_key` and
//! `translate.to_language` are set.

use super::LyricsPlugin;
use super::text::Lyrics;
use crate::plugins::requests::{HttpError, Request, format_url};
use serde_json::{Value as J, json};

const SEPARATOR: &str = " | ";

#[derive(Clone, Debug)]
pub struct Translator {
    pub api_key: String,
    pub to_language: String,
    pub from_languages: Vec<String>,
}

impl Translator {
    pub fn new(api_key: &str, to_language: &str, from_languages: &[String]) -> Self {
        Translator {
            api_key: api_key.to_string(),
            to_language: to_language.to_uppercase(),
            from_languages: from_languages.iter().map(|l| l.to_uppercase()).collect(),
        }
    }

    /// Translations of `texts`: the unique ones are sent joined in one
    /// request, and mapped back.
    fn get_translations(&self, p: &LyricsPlugin, texts: &[&str]) -> Result<Vec<String>, HttpError> {
        let mut unique: Vec<&str> = vec![];
        for t in texts {
            if !unique.contains(t) {
                unique.push(t);
            }
        }
        let text = unique.join(SEPARATOR);
        let params = [
            ("api-version", "3.0".to_string()),
            ("to", self.to_language.clone()),
        ];
        let url = format_url(&p.urls.translate, &params);
        p.debug(&format!("Translator: Posting JSON to {url}"));
        let body = json!([{ "text": text }]);
        let resp = p.session.request(
            "POST",
            &url,
            Request {
                headers: &[("Ocp-Apim-Subscription-Key", &self.api_key)],
                json: Some(&body),
                allow_redirects: true,
            },
        )?;
        let data: J = resp.json()?;
        let translated = data[0]["translations"][0]["text"]
            .as_str()
            .ok_or(HttpError::Json)?;
        let translations: Vec<&str> = translated.split(SEPARATOR).collect();
        Ok(texts
            .iter()
            .map(|t| {
                unique
                    .iter()
                    .position(|u| u == t)
                    .and_then(|i| translations.get(i))
                    .unwrap_or(&"")
                    .to_string()
            })
            .collect())
    }

    /// Translate `lyrics` to the target language, unless `old` already
    /// has the translations of the same text, the lyrics are in the target
    /// language already, or `from_languages` doesn't include theirs.
    pub fn translate(&self, p: &LyricsPlugin, mut lyrics: Lyrics, old: &Lyrics) -> Lyrics {
        if lyrics.original_text() == old.original_text() && old.translated() {
            p.info("Translator: 🔵 Translations already exist");
            return old.clone();
        }
        let language = lyrics.language.clone();
        if language.as_deref() == Some(self.to_language.as_str()) {
            p.info(&format!(
                "Translator: 🔵 Lyrics are already in the target language {}",
                self.to_language
            ));
        } else if !self.from_languages.is_empty()
            && !language
                .as_ref()
                .is_some_and(|l| self.from_languages.contains(l))
        {
            let langs: Vec<String> = self
                .from_languages
                .iter()
                .map(|l| crate::template::py_repr(l))
                .collect();
            p.info(&format!(
                "Translator: 🔵 Configuration [{}] does not permit translating from {}",
                langs.join(", "),
                language.as_deref().unwrap_or("None")
            ));
        } else {
            let lines = lyrics.text_lines();
            match self.get_translations(p, &lines) {
                Ok(t) => {
                    lyrics.translations = t;
                    lyrics.translation_language = Some(self.to_language.clone());
                    p.info(&format!(
                        "Translator: 🟢 Translated lyrics to {}",
                        self.to_language
                    ));
                }
                Err(e) => p.warn_request_error("Translator", &e),
            }
        }
        lyrics
    }
}
