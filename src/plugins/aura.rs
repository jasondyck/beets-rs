//! A port of beets 2.14's `aura` plugin (`beetsplug/aura.py`): an
//! [AURA](https://github.com/aura-home/aura) JSON:API server over the
//! library (`/aura/tracks`, `/aura/albums`, `/aura/artists`, images and
//! audio files), run with `beet aura [-d] [HOST] [PORT]`.
//!
//! Flask's blueprint is `App::handle`, a pure request → response function
//! (what beets' tests drive through Flask's test client); `serve` puts it
//! on the same small HTTP server as the web plugin. Differences from
//! beets: no flask-cors (the `cors` list is honoured for `Origin`), no
//! Pillow (image documents have no `width`/`height`), and a bad `page`/
//! `limit` parameter falls back to the default instead of a 500.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Kind, Record};
use crate::plugins::web::{self, Body, Json, Request, Response};
use crate::query::{Query, Sort};
use anyhow::Result;
use std::path::PathBuf;

pub struct AuraPlugin;

impl crate::plugins::Plugin for AuraPlugin {
    fn name(&self) -> &'static str {
        "aura"
    }
}

/// What Flask 3's `jsonify` sends (beets' `JSONIFY_MIMETYPE` setting no
/// longer applies), which is what the reference does.
pub const JSONAPI_MIMETYPE: &str = "application/json";

/// The server information document's attributes.
pub const SERVER_INFO: &[(&str, &str)] = &[
    ("aura-version", "0"),
    ("server", "beets-aura"),
    ("server-version", "0.1"),
];

/// Maps AURA Track attribute to beets Item attribute.
pub const TRACK_ATTR_MAP: &[(&str, &str)] = &[
    // Required
    ("title", "title"),
    ("artist", "artist"),
    // Optional
    ("album", "album"),
    ("track", "track"),
    ("tracktotal", "tracktotal"),
    ("disc", "disc"),
    ("disctotal", "disctotal"),
    ("year", "year"),
    ("month", "month"),
    ("day", "day"),
    ("bpm", "bpm"),
    ("genre", "genres"),
    ("genres", "genres"),
    ("recording-mbid", "mb_trackid"),
    ("track-mbid", "mb_releasetrackid"),
    ("composer", "composers"),
    ("albumartist", "albumartist"),
    ("comments", "comments"),
    // Audio metadata
    ("duration", "length"),
    ("framerate", "samplerate"),
    ("channels", "channels"),
    ("bitrate", "bitrate"),
    ("bitdepth", "bitdepth"),
    ("size", "filesize"),
];

/// Maps AURA Album attribute to beets Album attribute.
pub const ALBUM_ATTR_MAP: &[(&str, &str)] = &[
    // Required
    ("title", "album"),
    ("artist", "albumartist"),
    // Optional
    ("tracktotal", "albumtotal"),
    ("disctotal", "disctotal"),
    ("year", "year"),
    ("month", "month"),
    ("day", "day"),
    ("genre", "genres"),
    ("genres", "genres"),
    ("release-mbid", "mb_albumid"),
    ("release-group-mbid", "mb_releasegroupid"),
];

/// Maps AURA Artist attribute to beets Item field.
pub const ARTIST_ATTR_MAP: &[(&str, &str)] = &[("name", "artist"), ("artist-mbid", "mb_artistid")];

/// The app over a library, as `create_app()` builds it.
pub struct App<'a> {
    pub cfg: &'a Config,
    pub lib: &'a Library,
}

impl App<'_> {
    pub fn page_limit(&self) -> usize {
        match self.cfg.get("aura.page_limit") {
            Some(v) => v.as_f64().unwrap_or(500.0).max(0.0) as usize,
            None => 500,
        }
    }

    /// A request's response: the route's document, or Werkzeug's errors.
    pub fn handle(&self, req: &Request) -> Response {
        let (raw_path, qs) = req.url.split_once('?').unwrap_or((&req.url, ""));
        let path = web::percent_decode(raw_path);
        if req.method != "GET" && req.method != "HEAD" {
            return web::abort(405, None);
        }
        match self.route(&path, web::parse_args(qs), req) {
            Ok(r) => r,
            Err(e) => {
                crate::logging::emit(format!("Exception on {path} [{}]\n{e:#}", req.method));
                web::abort(500, None)
            }
        }
    }

    fn route(
        &self,
        path: &str,
        args: Vec<(String, String)>,
        req: &Request,
    ) -> Result<Response> {
        let doc = Doc {
            app: self,
            args,
            req,
        };
        // The blueprint is registered under "/aura".
        let Some(path) = path.strip_prefix("/aura") else {
            return Ok(web::abort(404, None));
        };
        let (path, tail) = match path.strip_prefix('/') {
            Some(t) => ("/", t),
            None => return Ok(web::abort(404, None)),
        };
        let _ = path;
        let parts: Vec<&str> = tail.split('/').collect();
        Ok(match parts.as_slice() {
            ["server"] => doc.server_info(),
            ["tracks"] => doc.all_tracks()?,
            ["tracks", id] => doc.single_track(parse_id(id)),
            ["tracks", id, "audio"] => doc.audio_file(parse_id(id)),
            ["albums"] => doc.all_albums()?,
            ["albums", id] => doc.single_album(parse_id(id)),
            ["artists"] => doc.all_artists()?,
            // Flask's `<path:artist_id>` allows slashes.
            ["artists", rest @ ..] if !rest.is_empty() => doc.single_artist(&rest.join("/")),
            ["images", id] => doc.single_image(id),
            ["images", id, "file"] => doc.image_file(id),
            _ => return Ok(web::abort(404, None)),
        })
    }
}

/// Flask's `<int:...>` converter: only digits (a non-number is a 404).
fn parse_id(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// One request's view of the arguments and URL.
struct Doc<'a, 'r> {
    app: &'a App<'a>,
    args: Vec<(String, String)>,
    req: &'r Request,
}

impl Doc<'_, '_> {
    fn arg(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn lib(&self) -> &Library {
        self.app.lib
    }

    /// `make_response(document, status)` for a JSON:API document.
    fn json_response(&self, body: Json, status: u16) -> Response {
        Response::new(status, JSONAPI_MIMETYPE, body.to_flask_json().into_bytes())
    }

    /// The JSON:API error document beets' `AURADocument.error` makes.
    fn error(&self, status: &str, title: &str, detail: &str) -> Response {
        let code = status
            .split_whitespace()
            .next()
            .and_then(|c| c.parse().ok())
            .unwrap_or(500);
        self.json_response(
            Json::Obj(vec![(
                "errors".into(),
                Json::List(vec![Json::Obj(vec![
                    ("status".into(), Json::Str(status.into())),
                    ("title".into(), Json::Str(title.into())),
                    ("detail".into(), Json::Str(detail.into())),
                ])]),
            )]),
            code,
        )
    }

    fn server_info(&self) -> Response {
        let mut attributes = vec![];
        for (k, v) in SERVER_INFO {
            attributes.push(((*k).to_string(), Json::Str((*v).into())));
        }
        attributes.push((
            "auth-required".into(),
            Json::Bool(false),
        ));
        attributes.push((
            "features".into(),
            Json::List(
                ["albums", "artists", "images"]
                    .iter()
                    .map(|f| Json::Str((*f).into()))
                    .collect(),
            ),
        ));
        self.json_response(
            Json::Obj(vec![(
                "data".into(),
                Json::Obj(vec![
                    ("type".into(), Json::Str("server".into())),
                    ("id".into(), Json::Str("0".into())),
                    ("attributes".into(), Json::Obj(attributes)),
                ]),
            )]),
            200,
        )
    }

    // -------------------------------------------------------- filters, sorts

    /// beets' `translate_filters`: `filter[attribute]=value` as exact
    /// matches (the attribute names are translated).
    fn filters(&self, kind: Kind, map: &[(&str, &str)]) -> Result<Query> {
        let mut ands = vec![];
        for (key, v) in &self.args {
            let Some(attr) = filter_attribute(key) else {
                continue;
            };
            let field = map
                .iter()
                .find(|(a, _)| *a == attr)
                .map(|(_, b)| *b)
                .unwrap_or(attr.as_str());
            ands.push(Query::match_field(kind, field, v)?.pred);
        }
        let mut q = Query::match_field(kind, "id", "")?;
        q.pred = match ands.len() {
            0 => crate::query::Pred::True,
            1 => ands.pop().unwrap(),
            _ => crate::query::Pred::And(ands),
        };
        Ok(q)
    }

    /// beets' `translate_sorts`: the `sort` parameter, a comma separated
    /// list of fields (`-` for descending), sorted slowly (any field).
    fn sorts(&self, kind: Kind, map: &[(&str, &str)]) -> Sort {
        let Some(sort_arg) = self.arg("sort") else {
            return Sort::default();
        };
        let trimmed = sort_arg.trim_matches(',');
        let mut sorts = vec![];
        for part in trimmed.split(',') {
            if part.is_empty() {
                continue;
            }
            let (ascending, attr) = match part.strip_prefix('-') {
                Some(rest) => (false, rest),
                None => (true, part),
            };
            let field = map
                .iter()
                .find(|(a, _)| *a == attr)
                .map(|(_, b)| *b)
                .unwrap_or(attr);
            sorts.push(crate::query::slow_sort(field, ascending));
        }
        let _ = kind;
        Sort(sorts)
    }

    // ----------------------------------------------------------- pagination

    /// beets' `paginate`: one page of the collection, and the next page's
    /// URL.
    fn paginate(&self, count: usize) -> (usize, usize, Option<String>) {
        let get = |name: &str, default: usize| -> usize {
            self.arg(name)
                .and_then(|v| v.parse::<i64>().ok())
                .map(|v| v.max(0) as usize)
                .unwrap_or(default)
        };
        let page = get("page", 0);
        let limit = get("limit", self.app.page_limit());
        let start = page.saturating_mul(limit);
        let end = (start + limit).min(count);
        // beets asks for the next page even when this one ended exactly at
        // the collection's end.
        let next = if start + limit > count {
            None
        } else if self.args.is_empty() {
            Some(format!("{}?page=1", self.request_url()))
        } else if self.arg("page").is_none() {
            Some(format!("{}&page=1", self.request_url()))
        } else {
            Some(
                self.request_url()
                    .replace(&format!("page={page}"), &format!("page={}", page + 1)),
            )
        };
        (start, end, next)
    }

    /// `request.url`: the request's full URL as the client sent it.
    fn request_url(&self) -> String {
        let host = self.req.header("Host").unwrap_or("localhost");
        format!("http://{host}{}", self.req.url)
    }

    // ------------------------------------------------------------ documents

    /// beets' `all_resources`: the collection document with links and
    /// included resources.
    fn all_resources(&self, kind: Kind, map: &[(&str, &str)]) -> Result<Response> {
        let mut query = self.filters(kind, map)?;
        query.sort = self.sorts(kind, map);
        let collection: Vec<Record> = match kind {
            Kind::Album => self.lib().albums(&query)?,
            Kind::Item => self.lib().items(&query)?,
        };
        let (start, end, next) = self.paginate(collection.len());
        let data: Vec<Json> = collection
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .map(|r| self.resource_object(kind, r))
            .collect();
        let mut doc = vec![("data".to_string(), Json::List(data.clone()))];
        if let Some(next) = next {
            doc.push((
                "links".into(),
                Json::Obj(vec![("next".into(), Json::Str(next))]),
            ));
        }
        if let Some(include) = self.arg("include") {
            doc.push(("included".into(), Json::List(self.get_included(&data, include))));
        }
        Ok(self.json_response(Json::Obj(doc), 200))
    }

    fn resource_object(&self, kind: Kind, rec: &Record) -> Json {
        match kind {
            Kind::Item => track_resource(self.lib(), rec),
            Kind::Album => album_resource(self.lib(), rec),
        }
    }

    /// beets' `get_included`: the resource objects for the requested
    /// relationship types.
    fn get_included(&self, data: &[Json], include_str: &str) -> Vec<Json> {
        let to_include: Vec<&str> = include_str.trim_matches(',').split(',').collect();
        let mut identifiers: Vec<Json> = vec![];
        for res in data {
            let Json::Obj(fields) = res else { continue };
            let Some((_, Json::Obj(rels))) = fields.iter().find(|(k, _)| k == "relationships") else {
                continue;
            };
            for (rel_name, rel) in rels {
                if !to_include.contains(&rel_name.as_str()) {
                    continue;
                }
                let Json::Obj(rel_obj) = rel else { continue };
                let Some((_, Json::List(ids))) = rel_obj.iter().find(|(k, _)| k == "data") else {
                    continue;
                };
                for id in ids {
                    if !identifiers.contains(id) {
                        // Deduplicate by value, as beets' `not in` does.
                        if !identifiers.iter().any(|i| same_identifier(i, id)) {
                            identifiers.push(id.clone());
                        }
                    }
                }
            }
        }
        let mut included = vec![];
        for id in &identifiers {
            let Json::Obj(fields) = id else { continue };
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(k, _)| k == name)
                    .and_then(|(_, v)| match v {
                        Json::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            };
            let res_type = get("type");
            let id = get("id");
            let object = match res_type.as_str() {
                "track" => id
                    .parse::<i64>()
                    .ok()
                    .and_then(|id| self.lib().get_item(id).ok().flatten())
                    .map(|item| track_resource(self.lib(), &item)),
                "album" => id
                    .parse::<i64>()
                    .ok()
                    .and_then(|id| self.lib().get_album(id).ok().flatten())
                    .map(|album| album_resource(self.lib(), &album)),
                "artist" => artist_resource(self.lib(), &id),
                "image" => image_resource(self.lib(), &id),
                _ => {
                    crate::logging::emit(format!("Invalid resource type: {res_type}"));
                    None
                }
            };
            if let Some(o) = object {
                included.push(o);
            }
        }
        included
    }

    /// beets' `single_resource_document`.
    fn single_resource_document(&self, object: Json) -> Response {
        let mut doc = vec![("data".to_string(), object.clone())];
        if let Some(include) = self.arg("include") {
            doc.push((
                "included".into(),
                Json::List(self.get_included(std::slice::from_ref(&object), include)),
            ));
        }
        self.json_response(Json::Obj(doc), 200)
    }

    // ---------------------------------------------------------- endpoints

    fn all_tracks(&self) -> Result<Response> {
        self.all_resources(Kind::Item, TRACK_ATTR_MAP)
    }

    fn all_albums(&self) -> Result<Response> {
        self.all_resources(Kind::Album, ALBUM_ATTR_MAP)
    }

    fn all_artists(&self) -> Result<Response> {
        let mut query = self.filters(Kind::Item, ARTIST_ATTR_MAP)?;
        query.sort = self.sorts(Kind::Item, ARTIST_ATTR_MAP);
        let tracks = self.lib().items(&query)?;
        let mut collection: Vec<String> = vec![];
        for track in tracks {
            let artist = track.str("artist");
            if !collection.contains(&artist.to_string()) {
                collection.push(artist.to_string());
            }
        }
        let (start, end, next) = self.paginate(collection.len());
        let data: Vec<Json> = collection
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .filter_map(|a| artist_resource(self.lib(), a))
            .collect();
        let mut doc = vec![("data".to_string(), Json::List(data.clone()))];
        if let Some(next) = next {
            doc.push((
                "links".into(),
                Json::Obj(vec![("next".into(), Json::Str(next))]),
            ));
        }
        if let Some(include) = self.arg("include") {
            doc.push((
                "included".into(),
                Json::List(self.get_included(&data, include)),
            ));
        }
        Ok(self.json_response(Json::Obj(doc), 200))
    }

    fn single_track(&self, id: Option<i64>) -> Response {
        let Some(id) = id else {
            return web::abort(404, None);
        };
        match self.lib().get_item(id) {
            Ok(Some(item)) => self.single_resource_document(track_resource(self.lib(), &item)),
            _ => self.error(
                "404 Not Found",
                "No track with the requested id.",
                &format!("There is no track with an id of {id} in the library."),
            ),
        }
    }

    fn single_album(&self, id: Option<i64>) -> Response {
        let Some(id) = id else {
            return web::abort(404, None);
        };
        match self.lib().get_album(id) {
            Ok(Some(album)) => self.single_resource_document(album_resource(self.lib(), &album)),
            _ => self.error(
                "404 Not Found",
                "No album with the requested id.",
                &format!("There is no album with an id of {id} in the library."),
            ),
        }
    }

    fn single_artist(&self, artist_id: &str) -> Response {
        match artist_resource(self.lib(), artist_id) {
            Some(resource) => self.single_resource_document(resource),
            None => self.error(
                "404 Not Found",
                "No artist with the requested id.",
                &format!("There is no artist with an id of {artist_id} in the library."),
            ),
        }
    }

    fn single_image(&self, image_id: &str) -> Response {
        match image_resource(self.lib(), image_id) {
            Some(resource) => self.single_resource_document(resource),
            None => self.error(
                "404 Not Found",
                "No image with the requested id.",
                &format!("There is no image with an id of {image_id} in the library."),
            ),
        }
    }

    /// `/aura/tracks/<id>/audio`: the track's file, as a download.
    fn audio_file(&self, id: Option<i64>) -> Response {
        let Some(id) = id else {
            return web::abort(404, None);
        };
        let track = match self.lib().get_item(id) {
            Ok(Some(t)) => t,
            _ => {
                return self.error(
                    "404 Not Found",
                    "No track with the requested id.",
                    &format!("There is no track with an id of {id} in the library."),
                );
            }
        };
        let path = track.path();
        if !path.is_file() {
            return self.error(
                "404 Not Found",
                "No audio file for the requested track.",
                &format!(
                    "There is no audio file for track {id} at the expected location"
                ),
            );
        }
        let mime = web::mimetype(&path.to_string_lossy());
        if mime == "application/octet-stream" {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            return self.error(
                "500 Internal Server Error",
                "Requested audio file has an unknown mimetype.",
                &format!(
                    "The audio file for track {id} has an unknown mimetype. Its file extension \
                     is {ext}."
                ),
            );
        }
        if !accepts(self.req.header("Accept"), mime) {
            return self.error(
                "406 Not Acceptable",
                "Unsupported MIME type or bitrate parameter in Accept header.",
                &format!(
                    "The audio file for track {id} is only available as {mime} and bitrate \
                     parameters are not supported."
                ),
            );
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match web::send_file(path, "attachment", &name, self.req) {
            Ok(r) => r,
            Err(e) => {
                crate::logging::emit(format!("{e:#}"));
                web::abort(500, None)
            }
        }
    }

    /// `/aura/images/<id>/file`: the image's file.
    fn image_file(&self, image_id: &str) -> Response {
        let Some(path) = image_path(self.lib(), image_id) else {
            return self.error(
                "404 Not Found",
                "No image with the requested id.",
                &format!("There is no image with an id of {image_id} in the library"),
            );
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match web::send_file(&path, "inline", &name, self.req) {
            Ok(r) => r,
            Err(e) => {
                crate::logging::emit(format!("{e:#}"));
                web::abort(500, None)
            }
        }
    }
}

/// beets' `translate_filters` key regex: `filter[<attribute>]`, matched
/// from the start of the key.
fn filter_attribute(key: &str) -> Option<String> {
    let rest = key.strip_prefix("filter[")?;
    let end = rest.find(']')?;
    let attr = &rest[..end];
    if attr.is_empty()
        || !attr
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(attr.to_string())
}

/// Two resource identifiers are the same when their type and id are.
fn same_identifier(a: &Json, b: &Json) -> bool {
    fn pair(v: &Json) -> Option<(String, String)> {
        let Json::Obj(fields) = v else { return None };
        let get = |name: &str| {
            fields.iter().find(|(k, _)| k == name).and_then(|(_, v)| match v {
                Json::Str(s) => Some(s.clone()),
                _ => None,
            })
        };
        Some((get("type")?, get("id")?))
    }
    pair(a) == pair(b)
}

/// Whether an `Accept` header takes `mime`, as werkzeug's
/// `request.accept_mimetypes.best_match([mime])` has it: no header accepts
/// nothing, `*/*` and a type wildcard match, and a parameterized item only
/// matches a value with the same parameters.
fn accepts(header: Option<&str>, mime: &str) -> bool {
    // request.accept_mimetypes.best_match([mime])
    let Some(header) = header else { return false };
    let (value_type, value_subtype) = mime.split_once('/').unwrap_or((mime, ""));
    header.split(',').any(|part| {
        let mut pieces = part.split(';');
        let item = pieces.next().unwrap_or("").trim().to_ascii_lowercase();
        let params: Vec<String> = pieces
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty() && !p.starts_with("q="))
            .collect();
        if item.is_empty() || !item.contains('/') {
            return false;
        }
        let (item_type, item_subtype) = item.split_once('/').unwrap_or((&item, ""));
        if item_type == "*" && item_subtype != "*" {
            return false;
        }
        (item_type == "*" && item_subtype == "*")
            || (item_type == value_type
                && (item_subtype == "*" || value_subtype == "*" || item_subtype == value_subtype)
                && params.is_empty())
    })
}

// ----------------------------------------------------------------- documents

/// `getattr(rec, field)`, truthy values only, as JSON.
fn attribute_value(kind: Kind, rec: &Record, field: &str) -> Option<Json> {
    match field {
        "filesize" => {
            let size = std::fs::metadata(rec.path()).map(|m| m.len()).unwrap_or(0);
            (size != 0).then_some(Json::Int(size as i64))
        }
        _ => {
            let value = rec.get(field)?;
            let json = web::value_json(kind, field, Some(value.clone()));
            let truthy = match &json {
                Json::Null => false,
                Json::Bool(b) => *b,
                Json::Int(i) => *i != 0,
                Json::Float(f) => *f != 0.0,
                Json::Str(s) => !s.is_empty(),
                Json::List(l) => !l.is_empty(),
                Json::Obj(o) => !o.is_empty(),
            };
            truthy.then_some(json)
        }
    }
}

/// The attributes of a resource object from an AURA → beets field map.
fn attributes(lib: &Library, kind: Kind, rec: &Record, map: &[(&str, &str)]) -> Vec<(String, Json)> {
    let mut out = vec![];
    for (aura_attr, beets_attr) in map {
        let value = if *beets_attr == "albumtotal" && kind == Kind::Album {
            // beets' `Album._albumtotal`: the first item's `tracktotal`, or
            // with `per_disc_numbering` a sum over the discs.
            let total = albumtotal(lib, rec);
            (total != 0).then_some(Json::Int(total))
        } else {
            attribute_value(kind, rec, beets_attr)
        };
        if let Some(v) = value {
            out.push(((*aura_attr).to_string(), v));
        }
    }
    out
}

fn relationship(kind: &str, ids: Vec<String>) -> Json {
    Json::Obj(vec![(
        "data".into(),
        Json::List(
            ids.into_iter()
                .map(|id| {
                    Json::Obj(vec![
                        ("type".into(), Json::Str(kind.to_string())),
                        ("id".into(), Json::Str(id)),
                    ])
                })
                .collect(),
        ),
    )])
}

/// A path-valued field (`artpath`).
fn path_field(rec: &Record, name: &str) -> Option<PathBuf> {
    match rec.get(name) {
        Some(crate::model::Value::Path(p)) if !p.as_os_str().is_empty() => Some(p),
        _ => None,
    }
}

/// beets' `Album._albumtotal`.
fn albumtotal(lib: &Library, album: &Record) -> i64 {
    let items = lib.album_items(album.id.unwrap_or(0)).unwrap_or_default();
    if album.int("disctotal") == 1 || !crate::formatted::env().per_disc_numbering {
        return items.first().map(|i| i.int("tracktotal")).unwrap_or(0);
    }
    let mut counted = vec![];
    let mut total = 0;
    for i in items {
        let disc = i.int("disc");
        if counted.contains(&disc) {
            continue;
        }
        total += i.int("tracktotal");
        counted.push(disc);
    }
    total
}

/// beets' `TrackDocument.get_resource_object`.
pub fn track_resource(lib: &Library, track: &Record) -> Json {
    let mut relationships = vec![(
        "artists".to_string(),
        relationship("artist", vec![track.str("artist").to_string()]),
    )];
    // Only add the album relationship if not a singleton.
    if !track.bool("singleton") {
        relationships.push((
            "albums".to_string(),
            relationship("album", vec![track.int("album_id").to_string()]),
        ));
    }
    Json::Obj(vec![
        ("type".into(), Json::Str("track".into())),
        ("id".into(), Json::Str(track.id.unwrap_or(0).to_string())),
        (
            "attributes".into(),
            Json::Obj(attributes(lib, Kind::Item, track, TRACK_ATTR_MAP)),
        ),
        ("relationships".into(), Json::Obj(relationships)),
    ])
}

/// beets' `AlbumDocument.get_resource_object`.
pub fn album_resource(lib: &Library, album: &Record) -> Json {
    let id = album.id.unwrap_or(0);
    let tracks = lib.album_items(id).unwrap_or_default();
    let mut relationships = vec![(
        "tracks".to_string(),
        relationship(
            "track",
            tracks
                .iter()
                .map(|t| t.id.unwrap_or(0).to_string())
                .collect(),
        ),
    )];
    let artpath = path_field(album, "artpath").unwrap_or_default();
    if !artpath.as_os_str().is_empty() {
        let filename = artpath
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        relationships.push((
            "images".to_string(),
            relationship("image", vec![format!("album-{id}-{filename}")]),
        ));
    }
    let artists = tracks.iter().map(|t| t.str("artist")).collect::<Vec<_>>();
    if artists.contains(&album.str("albumartist")) {
        relationships.push((
            "artists".to_string(),
            relationship("artist", vec![album.str("albumartist").to_string()]),
        ));
    }
    Json::Obj(vec![
        ("type".into(), Json::Str("album".into())),
        ("id".into(), Json::Str(id.to_string())),
        (
            "attributes".into(),
            Json::Obj(attributes(lib, Kind::Album, album, ALBUM_ATTR_MAP)),
        ),
        ("relationships".into(), Json::Obj(relationships)),
    ])
}

/// beets' `ArtistDocument.get_resource_object`: None when no track has the
/// artist.
pub fn artist_resource(lib: &Library, artist_id: &str) -> Option<Json> {
    let tracks = lib
        .items(&Query::match_field(Kind::Item, "artist", artist_id).ok()?)
        .ok()?;
    let first = tracks.first()?;
    let relationships = vec![
        (
            "tracks".to_string(),
            relationship(
                "track",
                tracks
                    .iter()
                    .map(|t| t.id.unwrap_or(0).to_string())
                    .collect(),
            ),
        ),
        ("albums".to_string(), {
            let albums = lib
                .albums(&Query::match_field(Kind::Album, "albumartist", artist_id).ok()?)
                .unwrap_or_default();
            relationship(
                "album",
                albums
                    .iter()
                    .map(|a| a.id.unwrap_or(0).to_string())
                    .collect(),
            )
        }),
    ];
    let mut relationships = relationships;
    // beets only adds the albums relationship when there are albums.
    if let Some(Json::Obj(fields)) = relationships.last_mut().map(|(_, v)| v) {
        let empty = match fields.iter().find(|(k, _)| k == "data") {
            Some((_, Json::List(l))) => l.is_empty(),
            _ => true,
        };
        if empty {
            relationships.pop();
        }
    }
    Json::Obj(vec![
        ("type".into(), Json::Str("artist".into())),
        ("id".into(), Json::Str(artist_id.into())),
        (
            "attributes".into(),
            Json::Obj(attributes(lib, Kind::Item, first, ARTIST_ATTR_MAP)),
        ),
        ("relationships".into(), Json::Obj(relationships)),
    ])
    .into()
}

/// beets' `safe_filename`: a plain name, no directory traversal.
pub fn safe_filename(fn_: &str) -> bool {
    if std::path::Path::new(fn_)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        != Some(fn_.to_string())
    {
        return false;
    }
    fn_ != "." && fn_ != ".."
}

/// beets' `ImageDocument.get_image_path`: `<parent_type>-<parent_id>-
/// <filename>` in the album's art directory.
pub fn image_path(lib: &Library, image_id: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = image_id.split('-').collect();
    if parts.len() < 3 {
        return None;
    }
    let parent_type = parts[0];
    let parent_id = parts[1];
    let img_filename = parts[2..].join("-");
    if !safe_filename(&img_filename) {
        return None;
    }
    let dir = if parent_type == "album" {
        let album = lib.get_album(parent_id.parse().ok()?).ok()??;
        let artpath = path_field(&album, "artpath")?;
        if artpath.as_os_str().is_empty() {
            return None;
        }
        artpath.parent()?.to_path_buf()
    } else {
        return None;
    };
    let img_path = dir.join(img_filename);
    img_path.is_file().then_some(img_path)
}

/// beets' `ImageDocument.get_resource_object`.
pub fn image_resource(lib: &Library, image_id: &str) -> Option<Json> {
    let image_path = image_path(lib, image_id)?;
    let size = std::fs::metadata(&image_path).ok()?.len();
    let mimetype = web::mimetype(&image_path.to_string_lossy());
    let mut attributes = vec![("role".to_string(), Json::Str("cover".into()))];
    if mimetype != "application/octet-stream" {
        attributes.push(("mimetype".into(), Json::Str(mimetype.into())));
    }
    if size != 0 {
        attributes.push(("size".into(), Json::Int(size as i64)));
    }
    let parts: Vec<&str> = image_id.split('-').collect();
    let relationships = vec![(
        format!("{}s", parts[0]),
        relationship(parts[0], vec![parts[1].to_string()]),
    )];
    Some(Json::Obj(vec![
        ("id".into(), Json::Str(image_id.into())),
        ("type".into(), Json::Str("image".into())),
        ("attributes".into(), Json::Obj(attributes)),
        ("relationships".into(), Json::Obj(relationships)),
    ]))
}

/// `beet aura`'s options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    /// `-d/--debug`: Flask's debug mode.
    pub debug: bool,
    /// HOST and PORT.
    pub args: Vec<String>,
}

/// `beet aura`: serve the library until interrupted.
pub fn command(cfg: &Config, lib: &Library, opts: CmdOpts) -> Result<()> {
    let host = match opts.args.first() {
        Some(h) => h.clone(),
        None => cfg.str("aura.host"),
    };
    let port = match opts.args.get(1) {
        Some(p) => p
            .trim()
            .parse::<u16>()
            .map_err(|_| anyhow::anyhow!("invalid literal for int() with base 10: '{p}'"))?,
        None => cfg
            .str("aura.port")
            .trim()
            .parse::<u16>()
            .map_err(|_| anyhow::anyhow!("configuration error: aura.port: must be a number"))?,
    };
    let addr = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map_or(addr.clone(), |a| a.to_string());
    println!(" * Debug mode: {}", if opts.debug { "on" } else { "off" });
    eprintln!(" * Running on http://{bound}\nPress CTRL+C to quit");
    let app = App { cfg, lib };
    web::serve_with(|req| app.handle(req), &server);
    Ok(())
}

/// A `Body` reference for the file responses (keeps the type used).
#[allow(dead_code)]
fn _body_marker() -> Body {
    Body::Bytes(vec![])
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "aura",
        order: 81,
        defaults: r#"aura:
    host: 127.0.0.1
    port: 8337
    cors: []
    cors_supports_credentials: no
    page_limit: 500
"#,
        config_keys: &["aura"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "aura",
                build: build_aura,
                run: run_aura,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(AuraPlugin))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct AuraArgs {
    /// Use Flask debug mode
    #[arg(short = 'd', long)]
    debug: bool,
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_aura() -> clap::Command {
    use clap::Args as _;
    AuraArgs::augment_args(
        clap::Command::new("aura").about("run an AURA server"),
    )
}

pub fn run_aura(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = AuraArgs::from_arg_matches(m)?;
    crate::plugins::aura::command(
        cfg,
        lib,
        crate::plugins::aura::CmdOpts {
            debug: a.debug,
            args: a.args,
        },
    )
}

#[cfg(test)]
#[path = "aura_tests.rs"]
mod tests;
