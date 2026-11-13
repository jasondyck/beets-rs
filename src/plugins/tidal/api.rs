//! A port of beets' `beetsplug/tidal/api.py`: the JSON:API client for
//! `openapi.tidal.com/v2` (`searchResults`, `tracks`, `albums`), resolving
//! pagination and batching the filters Tidal accepts (20 per request).

use super::Urls;
use super::session::TidalSession;
use anyhow::Result;
use serde_json::{Map, Value as J};
use std::path::{Path, PathBuf};

/// Tidal allows at most this many filters per request.
pub const MAX_FILTER_SIZE: usize = 20;

pub struct TidalApi {
    pub client_id: String,
    pub token_path: PathBuf,
    urls: Urls,
    session: Option<TidalSession>,
}

impl TidalApi {
    pub fn new(client_id: &str, token_path: &Path, urls: Urls) -> Self {
        TidalApi {
            client_id: client_id.to_string(),
            token_path: token_path.to_path_buf(),
            urls,
            session: None,
        }
    }

    /// The lazily created session (`TidalAPI.session`).
    pub fn session(&mut self) -> &mut TidalSession {
        if self.session.is_none() {
            self.session = Some(TidalSession::new(
                &self.client_id,
                &self.token_path,
                self.urls.clone(),
            ));
        }
        self.session
            .as_mut()
            .expect("BUG: the session was created just above")
    }

    /// `TidalAPI.search_results`.
    pub fn search_results(
        &mut self,
        query: &str,
        explicit_filter: &str,
        include: &[&str],
        country_code: &str,
    ) -> Result<J> {
        let mut params: Vec<(&str, String)> = vec![
            ("filter[query]", query.to_string()),
            ("explicitFilter", explicit_filter.to_string()),
            ("countryCode", country_code.to_string()),
        ];
        // A list value becomes one pair per element (requests repeats the
        // key; it is not comma-joined).
        params.extend(include.iter().map(|i| ("include", i.to_string())));
        self.get_json("/searchResults", &params)
    }

    fn get_json(&mut self, path: &str, params: &[(&str, String)]) -> Result<J> {
        self.session().get_json(path, params)
    }

    /// `TidalAPI.get_tracks`.
    pub fn get_tracks(
        &mut self,
        ids: &[String],
        isrcs: &[String],
        include: &[&str],
        country_code: &str,
    ) -> Result<J> {
        let mut doc = empty_doc();
        for (id_batch, isrc_batch) in zip_batches(ids, isrcs) {
            let mut params: Vec<(&str, String)> = vec![("countryCode", country_code.to_string())];
            if !id_batch.is_empty() {
                params.extend(repeated("filter[id]", &id_batch));
            }
            if !isrc_batch.is_empty() {
                params.extend(repeated("filter[isrc]", &isrc_batch));
            }
            let page = self.get_paginated("/tracks", include, &params)?;
            merge(&mut doc, page);
        }
        Ok(doc)
    }

    /// `TidalAPI.get_albums`.
    pub fn get_albums(
        &mut self,
        ids: &[String],
        barcode_ids: &[String],
        include: &[&str],
        country_code: &str,
    ) -> Result<J> {
        let mut doc = empty_doc();
        for (id_batch, barcode_batch) in zip_batches(ids, barcode_ids) {
            let mut params: Vec<(&str, String)> = vec![("countryCode", country_code.to_string())];
            if !id_batch.is_empty() {
                params.extend(repeated("filter[id]", &id_batch));
            }
            if !barcode_batch.is_empty() {
                params.extend(repeated("filter[barcodeId]", &barcode_batch));
            }
            let page = self.get_paginated("/albums", include, &params)?;
            merge(&mut doc, page);
        }
        Ok(doc)
    }

    /// `TidalAPI.get_paginated`: follow `links.next` until there is none,
    /// merging each page into the document.
    pub fn get_paginated(
        &mut self,
        url: &str,
        include: &[&str],
        params: &[(&str, String)],
    ) -> Result<J> {
        let mut doc = empty_doc();
        doc.as_object_mut()
            .expect("BUG: empty_doc is a JSON object")
            .insert(
                "links".into(),
            J::Object(Map::from_iter([("next".into(), J::String(url.into()))])),
        );
        let mut guard = 0;
        while let Some(next) = doc
            .get("links")
            .and_then(|l| l.get("next"))
            .and_then(|n| n.as_str())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
        {
            let mut page_params: Vec<(&str, String)> = params.to_vec();
            page_params.extend(include.iter().map(|i| ("include", i.to_string())));
            let page = self.get_json(&next, &page_params)?;
            merge(&mut doc, page);
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        dedupe_included(&mut doc);
        Ok(doc)
    }
}

/// `Document` with empty `data` and `included`.
fn empty_doc() -> J {
    let mut m = Map::new();
    m.insert("data".into(), J::Array(vec![]));
    m.insert("included".into(), J::Array(vec![]));
    J::Object(m)
}

fn repeated<'a>(key: &'a str, values: &[String]) -> Vec<(&'a str, String)> {
    values.iter().map(|v| (key, v.clone())).collect()
}

/// `zip_longest(_batched(ids, n), _batched(isrcs, n), fillvalue=())`.
fn zip_batches(ids: &[String], others: &[String]) -> Vec<(Vec<String>, Vec<String>)> {
    let a: Vec<Vec<String>> = ids.chunks(MAX_FILTER_SIZE).map(<[String]>::to_vec).collect();
    let b: Vec<Vec<String>> = others
        .chunks(MAX_FILTER_SIZE)
        .map(<[String]>::to_vec)
        .collect();
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| {
            (
                a.get(i).cloned().unwrap_or_default(),
                b.get(i).cloned().unwrap_or_default(),
            )
        })
        .collect()
}

/// `TidalAPI.merge_multiresource_pagination`: append `data`, dedupe
/// `included` by `(type, id)`, and take the last page's `links`.
pub fn merge(a: &mut J, b: J) {
    let b_data = b
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    if let Some(J::Array(data)) = a.get_mut("data") {
        data.extend(b_data);
    }
    let mut seen: Vec<(String, String)> = a
        .get("included")
        .and_then(|i| i.as_array())
        .map(|items| items.iter().map(resource_key).collect())
        .unwrap_or_default();
    let b_included = b
        .get("included")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    if let Some(J::Array(included)) = a.get_mut("included") {
        for item in b_included {
            let key = resource_key(&item);
            if !seen.contains(&key) {
                seen.push(key);
                included.push(item);
            }
        }
    }
    let links = b
        .get("links")
        .cloned()
        .unwrap_or_else(|| J::Object(Map::new()));
    a.as_object_mut()
        .expect("BUG: merged documents start as JSON objects (empty_doc)")
        .insert("links".into(), links);
}

fn dedupe_included(doc: &mut J) {
    let items = doc
        .get("included")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    let mut seen: Vec<(String, String)> = vec![];
    let mut out: Vec<J> = vec![];
    for item in items.into_iter().rev() {
        let key = resource_key(&item);
        if !seen.contains(&key) {
            seen.push(key);
            out.push(item);
        }
    }
    out.reverse();
    doc.as_object_mut()
        .expect("BUG: the paginated document is a JSON object")
        .insert("included".into(), J::Array(out));
}

fn resource_key(item: &J) -> (String, String) {
    (
        item.get("type").and_then(|t| t.as_str()).unwrap_or("").into(),
        item.get("id").and_then(|i| i.as_str()).unwrap_or("").into(),
    )
}
