//! A port of beets 2.14's `web` plugin (`beetsplug/web/__init__.py`): a
//! JSON API over the library plus beets' browser UI (its static files and
//! `index.html`, copied from beets under the MIT license; see
//! `web/LICENSE`), served by `beet web [-d] [HOST] [PORT]`.
//!
//! The Flask app is `App::handle`, a pure request → response function
//! (what beets' tests drive through Flask's test client); `serve` puts it
//! behind a small synchronous HTTP server (`tiny_http`). Requests are
//! handled one at a time on the thread that owns the library (beets'
//! models aren't shared between threads either); only writing a response
//! out (an audio file streamed to a slow client, say) happens on a thread
//! of its own, so it doesn't hold up other requests.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Album, FieldType, Item, Kind, Record, Value};
use crate::plugins::PluginLog;
use crate::query::Query;
use anyhow::{Result, bail};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub struct WebPlugin;

impl crate::plugins::Plugin for WebPlugin {
    fn name(&self) -> &'static str {
        "web"
    }
}

/// `index.html` (a Jinja template in beets; its only tags are
/// `url_for('static', ...)` links).
const INDEX_HTML: &str = include_str!("web/templates/index.html");

/// Flask's static folder.
const STATIC_FILES: &[(&str, &[u8])] = &[
    ("backbone.js", include_bytes!("web/static/backbone.js")),
    ("beets.css", include_bytes!("web/static/beets.css")),
    ("beets.js", include_bytes!("web/static/beets.js")),
    ("jquery.js", include_bytes!("web/static/jquery.js")),
    ("underscore.js", include_bytes!("web/static/underscore.js")),
];

// ------------------------------------------------------------ settings

/// flask-cors, as the plugin sets it up: every route, the `cors` origin,
/// `Content-Type` as the allowed header.
#[derive(Clone, Debug)]
pub struct Cors {
    pub origin: String,
    pub supports_credentials: bool,
}

/// What `beet web` puts in the Flask app's config.
#[derive(Clone, Debug)]
pub struct Settings {
    pub include_paths: bool,
    pub readonly: bool,
    pub cors: Option<Cors>,
    pub reverse_proxy: bool,
    /// Flask's debug mode: indented JSON from `jsonify`.
    pub debug: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            include_paths: false,
            readonly: true,
            cors: None,
            reverse_proxy: false,
            debug: false,
        }
    }
}

impl Settings {
    pub fn from_config(cfg: &Config, debug: bool) -> Settings {
        let origin = cfg.str("web.cors");
        Settings {
            include_paths: cfg.bool("web.include_paths"),
            readonly: cfg.bool("web.readonly"),
            cors: (!origin.is_empty()).then(|| Cors {
                origin,
                supports_credentials: cfg.bool("web.cors_supports_credentials"),
            }),
            reverse_proxy: cfg.bool("web.reverse_proxy"),
            debug,
        }
    }
}

// ------------------------------------------------------ requests, responses

/// An HTTP request, as the app sees it.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    /// The request target: the (percent-encoded) path and query string.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    #[cfg(test)]
    pub fn new(method: &str, url: &str) -> Request {
        Request {
            method: method.to_string(),
            url: url.to_string(),
            ..Default::default()
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub enum Body {
    Bytes(Vec<u8>),
    /// A file (already at the start of what's sent) and how many bytes of
    /// it to send.
    File(File, u64),
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub(crate) fn new(status: u16, content_type: &str, body: Vec<u8>) -> Response {
        Response {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: Body::Bytes(body),
        }
    }

    fn json(body: String) -> Response {
        Response::new(200, "application/json", body.into_bytes())
    }

    #[cfg(test)]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn len(&self) -> u64 {
        match &self.body {
            Body::Bytes(b) => b.len() as u64,
            Body::File(_, n) => *n,
        }
    }

    /// The whole body (reading a file body).
    #[cfg(test)]
    pub fn into_bytes(self) -> Vec<u8> {
        match self.body {
            Body::Bytes(b) => b,
            Body::File(f, n) => {
                let mut out = vec![];
                f.take(n).read_to_end(&mut out).ok();
                out
            }
        }
    }
}

/// Werkzeug's HTTP error pages (`flask.abort`).
pub(crate) fn abort(code: u16, description: Option<&str>) -> Response {
    let (name, default) = match code {
        400 => (
            "Bad Request",
            "The browser (or proxy) sent a request that this server could not understand.",
        ),
        404 => (
            "Not Found",
            "The requested URL was not found on the server. If you entered the URL manually \
             please check your spelling and try again.",
        ),
        405 => (
            "Method Not Allowed",
            "The method is not allowed for the requested URL.",
        ),
        415 => (
            "Unsupported Media Type",
            "The server does not support the media type transmitted in the request.",
        ),
        416 => (
            "Requested Range Not Satisfiable",
            "The server cannot provide the requested range.",
        ),
        _ => (
            "Internal Server Error",
            "The server encountered an internal error and was unable to complete your \
             request. Either the server is overloaded or there is an error in the \
             application.",
        ),
    };
    let desc = html_escape::encode_text(description.unwrap_or(default)).replace('\n', "<br>");
    let body = format!(
        "<!doctype html>\n<html lang=en>\n<title>{code} {name}</title>\n<h1>{name}</h1>\n\
         <p>{desc}</p>\n"
    );
    Response::new(code, "text/html; charset=utf-8", body.into_bytes())
}

// ---------------------------------------------------------------- JSON

/// A JSON value as Python's `json` module writes it (key order kept).
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// How `json.dumps` lays its output out.
#[derive(Clone, Copy)]
enum Layout {
    /// `json.dumps` defaults: `", "` and `": "`.
    Default,
    /// Flask's compact `jsonify`: `","` and `":"`.
    Compact,
    /// Flask's `jsonify` in debug mode: `indent=2`.
    Indent,
}

impl Json {
    /// Flask's `jsonify` output (as Flask 3 writes it): keys sorted, compact
    /// separators (the aura plugin's documents).
    pub(crate) fn to_flask_json(&self) -> String {
        self.sorted().dumps(Layout::Compact)
    }

    /// The value with every object's keys sorted (Flask's `sort_keys`).
    fn sorted(&self) -> Json {
        match self {
            Json::List(l) => Json::List(l.iter().map(Json::sorted).collect()),
            Json::Obj(o) => {
                let mut entries: Vec<(String, Json)> =
                    o.iter().map(|(k, v)| (k.clone(), v.sorted())).collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Json::Obj(entries)
            }
            other => other.clone(),
        }
    }

    fn obj(map: BTreeMap<String, Json>) -> Json {
        Json::Obj(map.into_iter().collect())
    }

    fn dumps(&self, layout: Layout) -> String {
        let mut out = String::new();
        self.write(&mut out, layout, 0);
        out
    }

    fn write(&self, out: &mut String, layout: Layout, depth: usize) {
        let (item_sep, key_sep) = match layout {
            Layout::Default => (", ", ": "),
            Layout::Compact => (",", ":"),
            Layout::Indent => (",", ": "),
        };
        let newline = |out: &mut String, depth: usize| {
            if let Layout::Indent = layout {
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
            }
        };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => out.push_str(&i.to_string()),
            Json::Float(f) => out.push_str(&py_json_float(*f)),
            Json::Str(s) => py_json_str(out, s),
            Json::List(l) if l.is_empty() => out.push_str("[]"),
            Json::Obj(o) if o.is_empty() => out.push_str("{}"),
            Json::List(l) => {
                out.push('[');
                for (i, v) in l.iter().enumerate() {
                    if i > 0 {
                        out.push_str(item_sep);
                    }
                    newline(out, depth + 1);
                    v.write(out, layout, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Json::Obj(o) => {
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push_str(item_sep);
                    }
                    newline(out, depth + 1);
                    py_json_str(out, k);
                    out.push_str(key_sep);
                    v.write(out, layout, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

/// `json.dumps` of a float (`float.__repr__`; `NaN`, `Infinity`).
fn py_json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        crate::pyyaml::py_float_repr(f)
    }
}

/// `json.dumps` of a string with `ensure_ascii` (the default, and Flask's).
fn py_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    write!(out, "\\u{u:04x}").unwrap();
                }
            }
        }
    }
    out.push('"');
}

/// Python's `base64.b64encode`.
fn b64encode(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A model field's value as Python has it in `dict(obj)`, for `json.dumps`
/// (bytes, i.e. paths, base64-encoded as `_rep` does).
pub(crate) fn value_json(kind: Kind, key: &str, value: Option<Value>) -> Json {
    let Some(value) = value else {
        return Json::Null;
    };
    match (crate::model::field_type(kind, key), value) {
        (_, Value::Null) => Json::Null,
        (Some(FieldType::List(d)), v) => Json::List(
            crate::model::split_list(&v.raw(), d)
                .into_iter()
                .map(Json::Str)
                .collect(),
        ),
        // A nullable path (`artpath`) that isn't set is None.
        (_, Value::Path(p)) if p.as_os_str().is_empty() => Json::Null,
        (_, Value::Path(p)) => Json::Str(b64encode(&crate::osutil::path_bytes(&p))),
        // A singleton's `album_id` is None.
        (_, Value::Int(0)) if kind == Kind::Item && key == "album_id" => Json::Null,
        (_, Value::Int(i)) => Json::Int(i),
        (_, Value::Float(f)) => Json::Float(f),
        (_, Value::Bool(b)) => Json::Bool(b),
        (_, Value::Str(s)) => Json::Str(s),
    }
}

/// `dict(obj)`: the model's fields (fixed and flexible) by name, sorted
/// (as Flask's `jsonify` sorts them; beets' own order is a set's).
fn model_dict(rec: &Record) -> BTreeMap<String, Json> {
    rec.keys()
        .into_iter()
        .map(|k| {
            let v = value_json(rec.kind, &k, rec.get(&k));
            (k, v)
        })
        .collect()
}

/// beets' `util.displayable_path`.
fn displayable_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- routes

#[derive(Debug, PartialEq)]
enum Route {
    Home,
    Static(String),
    Stats,
    Artists,
    List(Kind),
    Query(Kind, String),
    Ids(Kind, String),
    ItemPath(String),
    ItemFile(i64),
    AlbumArt(i64),
    Values(Kind, String),
}

#[derive(Debug, PartialEq)]
enum Matched {
    Route(Route),
    /// Werkzeug's `strict_slashes`: the URL without its trailing slash.
    Redirect(String),
}

const GET: &[&str] = &["GET"];
const GET_DELETE: &[&str] = &["GET", "DELETE"];
const GET_DELETE_PATCH: &[&str] = &["GET", "DELETE", "PATCH"];

impl Route {
    /// The route's methods (Flask adds HEAD and OPTIONS).
    fn methods(&self) -> &'static [&'static str] {
        match self {
            Route::Query(Kind::Item, _) | Route::Ids(Kind::Item, _) => GET_DELETE_PATCH,
            Route::Query(Kind::Album, _) | Route::Ids(Kind::Album, _) => GET_DELETE,
            _ => GET,
        }
    }
}

/// Werkzeug's `\d+` integer converter.
fn int_part(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The app's URL map.
fn route(path: &str) -> Option<Matched> {
    let r = |r| Some(Matched::Route(r));
    match path {
        "/" => return r(Route::Home),
        "/stats" => return r(Route::Stats),
        "/artist/" => return r(Route::Artists),
        "/artist" => return Some(Matched::Redirect("/artist/".into())),
        _ => {}
    }
    if let Some(f) = path.strip_prefix("/static/")
        && !f.is_empty()
        && !f.starts_with('/')
    {
        return r(Route::Static(f.to_string()));
    }
    for (prefix, kind) in [("/item", Kind::Item), ("/album", Kind::Album)] {
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            return Some(Matched::Redirect(format!("{prefix}/")));
        }
        let Some(rest) = rest.strip_prefix('/') else {
            continue;
        };
        match rest {
            "" | "query/" => return r(Route::List(kind)),
            "query" => return Some(Matched::Redirect(format!("{prefix}/query/"))),
            _ => {}
        }
        if let Some(q) = rest.strip_prefix("query/")
            && !q.starts_with('/')
        {
            return r(Route::Query(kind, q.to_string()));
        }
        if kind == Kind::Item
            && let Some(p) = rest.strip_prefix("path/")
        {
            return r(Route::ItemPath(p.to_string()));
        }
        if let Some(k) = rest.strip_prefix("values/")
            && !k.is_empty()
            && !k.contains('/')
        {
            return r(Route::Values(kind, k.to_string()));
        }
        if let Some((id, tail)) = rest.split_once('/') {
            return match (kind, int_part(id), tail) {
                (Kind::Item, Some(id), "file") => r(Route::ItemFile(id)),
                (Kind::Album, Some(id), "art") => r(Route::AlbumArt(id)),
                _ => None,
            };
        }
        return r(Route::Ids(kind, rest.to_string()));
    }
    None
}

/// `%XX` escapes decoded (as the WSGI server does to `PATH_INFO`).
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Werkzeug's `request.args`: `&`-separated, `+` for a space.
pub(crate) fn parse_args(qs: &str) -> Vec<(String, String)> {
    qs.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let d = |s: &str| percent_decode(&s.replace('+', " "));
            (d(k), d(v))
        })
        .collect()
}

/// One request's view of things.
struct Ctx<'r> {
    req: &'r Request,
    method: String,
    args: Vec<(String, String)>,
    /// The path prefix the app is mounted at (`X-Script-Name` behind a
    /// reverse proxy).
    script_name: String,
}

impl Ctx<'_> {
    fn arg(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `?expand`
    fn is_expand(&self) -> bool {
        self.arg("expand").is_some()
    }

    /// `?delete`
    fn is_delete(&self) -> bool {
        self.arg("delete").is_some()
    }
}

// ------------------------------------------------------------------ app

/// The web app over a library.
pub struct App<'a> {
    pub cfg: &'a Config,
    pub lib: &'a Library,
    pub settings: Settings,
}

impl App<'_> {
    pub fn handle(&self, req: &Request) -> Response {
        let mut resp = self.dispatch(req);
        if let Some(cors) = &self.settings.cors {
            cors.apply(req, &mut resp);
        }
        resp
    }

    fn dispatch(&self, req: &Request) -> Response {
        let (raw_path, qs) = req.url.split_once('?').unwrap_or((&req.url, ""));
        let mut path = percent_decode(raw_path);
        let mut script_name = String::new();
        // `ReverseProxied`
        if self.settings.reverse_proxy
            && let Some(name) = req.header("X-Script-Name")
            && !name.is_empty()
        {
            script_name = name.to_string();
            if let Some(rest) = path.strip_prefix(name) {
                path = rest.to_string();
            }
        }
        let ctx = Ctx {
            req,
            method: req.method.to_ascii_uppercase(),
            args: parse_args(qs),
            script_name,
        };
        let route = match route(&path) {
            None => return abort(404, None),
            Some(Matched::Redirect(to)) => return self.redirect(&ctx, &to, qs),
            Some(Matched::Route(r)) => r,
        };
        let mut allowed: Vec<&str> = route.methods().to_vec();
        allowed.extend(["HEAD", "OPTIONS"]);
        allowed.sort();
        let allow = allowed.join(", ");
        if ctx.method == "OPTIONS" {
            let mut resp = Response::new(200, "text/html; charset=utf-8", vec![]);
            resp.headers.push(("Allow".into(), allow));
            return resp;
        }
        if !allowed.contains(&ctx.method.as_str()) {
            let mut resp = abort(405, None);
            resp.headers.push(("Allow".into(), allow));
            return resp;
        }
        match self.view(route, &ctx) {
            Ok(r) => r,
            Err(e) => {
                crate::logging::emit(format!("Exception on {path} [{}]\n{e:#}", ctx.method));
                abort(500, None)
            }
        }
    }

    fn redirect(&self, ctx: &Ctx, to: &str, qs: &str) -> Response {
        let scheme = match ctx.req.header("X-Scheme") {
            Some(s) if self.settings.reverse_proxy && !s.is_empty() => s,
            _ => "http",
        };
        let host = ctx.req.header("Host").unwrap_or("localhost");
        let mut url = format!("{scheme}://{host}{}{to}", ctx.script_name);
        if !qs.is_empty() {
            url = format!("{url}?{qs}");
        }
        let e = html_escape::encode_double_quoted_attribute(&url);
        let body = format!(
            "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n\
             <h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target \
             URL: <a href=\"{e}\">{e}</a>. If not, click the link.\n"
        );
        let mut resp = Response::new(308, "text/html; charset=utf-8", body.into_bytes());
        resp.headers.push(("Location".into(), url));
        resp
    }

    fn view(&self, route: Route, ctx: &Ctx) -> Result<Response> {
        let m = if ctx.method == "HEAD" {
            "GET"
        } else {
            ctx.method.as_str()
        };
        Ok(match route {
            Route::Home => Self::home(ctx),
            Route::Static(name) => match STATIC_FILES.iter().find(|(n, _)| *n == name) {
                Some((n, data)) => {
                    let mut resp = Response::new(200, mimetype(n), data.to_vec());
                    resp.headers.push((
                        "Content-Disposition".into(),
                        content_disposition("inline", n),
                    ));
                    resp.headers
                        .push(("Cache-Control".into(), "no-cache".into()));
                    resp
                }
                None => abort(404, None),
            },
            Route::Stats => {
                let (items, albums) = self.lib.counts()?;
                self.jsonify(&Json::Obj(vec![
                    ("albums".into(), Json::Int(albums)),
                    ("items".into(), Json::Int(items)),
                ]))
            }
            Route::Artists => {
                let names = self
                    .lib
                    .query_column("SELECT DISTINCT albumartist FROM albums")?;
                let names = names.into_iter().map(sql_json).collect::<Result<_>>()?;
                self.jsonify(&Json::Obj(vec![("artist_names".into(), Json::List(names))]))
            }
            Route::List(kind) => {
                let all = self.fetch(kind, &[] as &[&str])?;
                self.json_generator(&all, root(kind), ctx.is_expand())?
            }
            Route::Ids(kind, ids) => self.resource(kind, &ids, m, ctx)?,
            Route::Query(kind, q) => self.resource_query(kind, &q, m, ctx)?,
            Route::ItemPath(p) => {
                let q = Query::path(Kind::Item, "path", &p);
                match self.lib.items(&q)?.into_iter().next() {
                    Some(item) => {
                        let rep = self.item_rep(&item, &mut HashMap::new())?;
                        self.jsonify(&rep)
                    }
                    None => abort(404, None),
                }
            }
            Route::ItemFile(id) => match self.lib.get_item(id)? {
                Some(item) => item_file(&item, ctx)?,
                None => abort(404, Some(&format!("Item with id {id} not found"))),
            },
            Route::AlbumArt(id) => match self.lib.get_album(id)? {
                Some(a) if !a.path_field("artpath").as_os_str().is_empty() => {
                    let p = a.path_field("artpath");
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    send_file(p, "inline", &name, ctx.req)?
                }
                _ => abort(404, None),
            },
            Route::Values(kind, key) => {
                let sort_key = ctx.arg("sort_key").unwrap_or(&key).to_string();
                self.unique_values(kind, &key, &sort_key)?
            }
        })
    }

    /// Flask's `jsonify`: sorted keys, compact (indented in debug mode),
    /// and a newline.
    fn jsonify(&self, j: &Json) -> Response {
        let layout = if self.settings.debug {
            Layout::Indent
        } else {
            Layout::Compact
        };
        Response::json(format!("{}\n", j.dumps(layout)))
    }

    /// `json_generator`: `{"root":[...]}` around each model's `json.dumps`.
    fn json_generator(&self, objs: &[Record], root: &str, expand: bool) -> Result<Response> {
        let mut out = format!("{{\"{root}\":[");
        let mut albums = HashMap::new();
        for (i, o) in objs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&self.rep(o, expand, &mut albums)?.dumps(Layout::Default));
        }
        out.push_str("]}");
        Ok(Response::json(out))
    }

    /// `_rep`
    fn rep(
        &self,
        obj: &Record,
        expand: bool,
        albums: &mut HashMap<i64, Option<Album>>,
    ) -> Result<Json> {
        match obj.kind {
            Kind::Item => self.item_rep(obj, albums),
            Kind::Album => self.album_rep(obj, expand),
        }
    }

    /// `_rep` of an item: its fields, then (beets' `Item.keys()`) those of
    /// its album it lacks, paths base64-encoded; `path` only with
    /// `include_paths`; `size`, the file's.
    fn item_rep(&self, item: &Item, albums: &mut HashMap<i64, Option<Album>>) -> Result<Json> {
        let mut out = model_dict(item);
        let album_id = item.int("album_id");
        if album_id != 0 {
            if let std::collections::hash_map::Entry::Vacant(e) = albums.entry(album_id) {
                e.insert(self.lib.get_album(album_id)?);
            }
            if let Some(Some(album)) = albums.get(&album_id) {
                for (k, v) in model_dict(album) {
                    out.entry(k).or_insert(v);
                }
            }
        }
        if self.settings.include_paths {
            out.insert("path".into(), Json::Str(displayable_path(item.path())));
        } else {
            out.remove("path");
        }
        let size = std::fs::metadata(item.path()).map_or(0, |m| m.len());
        out.insert("size".into(), Json::Int(size as i64));
        Ok(Json::obj(out))
    }

    /// `_rep` of an album: `artpath` only with `include_paths`; with
    /// `expand`, its items.
    fn album_rep(&self, album: &Album, expand: bool) -> Result<Json> {
        let mut out = model_dict(album);
        if self.settings.include_paths {
            let p = album.path_field("artpath");
            // `displayable_path(None)` is "None".
            let shown = if p.as_os_str().is_empty() {
                "None".to_string()
            } else {
                displayable_path(p)
            };
            out.insert("artpath".into(), Json::Str(shown));
        } else {
            out.remove("artpath");
        }
        if expand {
            let id = album.id.unwrap_or(0);
            let items = self.fetch(Kind::Item, &[format!("album_id:{id}")])?;
            let mut albums = HashMap::from([(id, Some(album.clone()))]);
            let reps = items
                .iter()
                .map(|i| self.item_rep(i, &mut albums))
                .collect::<Result<_>>()?;
            out.insert("items".into(), Json::List(reps));
        }
        Ok(Json::obj(out))
    }

    fn fetch<S: AsRef<str>>(&self, kind: Kind, q: &[S]) -> Result<Vec<Record>> {
        let q = Query::parse(kind, q)?;
        match kind {
            Kind::Item => self.lib.items(&q),
            Kind::Album => self.lib.albums(&q),
        }
    }

    fn get(&self, kind: Kind, id: i64) -> Result<Option<Record>> {
        match kind {
            Kind::Item => self.lib.get_item(id),
            Kind::Album => self.lib.get_album(id),
        }
    }

    /// `entity.remove(delete=...)`
    fn remove(&self, obj: &Record, delete: bool) -> Result<()> {
        match obj.kind {
            Kind::Item => crate::commands::remove_item(self.cfg, self.lib, obj, delete, true),
            Kind::Album => crate::commands::remove_album(self.cfg, self.lib, obj, delete, true),
        }
    }

    /// `entity.update(request.get_json())` and `entity.try_sync(True,
    /// False)` (write the tags, don't move) for each item. None: the
    /// request's error response.
    fn patch(&self, items: &mut [Record], ctx: &Ctx) -> Result<Option<Response>> {
        if items.is_empty() {
            return Ok(None);
        }
        let values = match request_json(ctx.req) {
            Ok(serde_json::Value::Object(m)) => m,
            Ok(_) => bail!("the request's JSON is not an object"),
            Err(resp) => return Ok(Some(resp)),
        };
        for item in items.iter_mut() {
            for (k, v) in &values {
                set_json(item, k, v);
            }
            if let Err(e) = crate::media::write_item(item) {
                crate::logging::emit(format!("{e:#}"));
            }
            self.lib.store(item)?;
        }
        Ok(None)
    }

    /// `resource`: `/item/<ids>`, `/album/<ids>`.
    fn resource(&self, kind: Kind, ids: &str, method: &str, ctx: &Ctx) -> Result<Response> {
        let mut entities = vec![];
        for id in ids.split(',').filter_map(py_int) {
            entities.extend(self.get(kind, id)?);
        }
        let name = root(kind);
        if method == "DELETE" {
            if self.settings.readonly {
                return Ok(abort(405, None));
            }
            for e in &entities {
                self.remove(e, ctx.is_delete())?;
            }
            return Ok(self.deleted());
        }
        if method == "PATCH" && kind == Kind::Item {
            if self.settings.readonly {
                return Ok(abort(405, None));
            }
            if let Some(resp) = self.patch(&mut entities, ctx)? {
                return Ok(resp);
            }
        }
        if method == "GET" || method == "PATCH" && kind == Kind::Item {
            if entities.len() == 1 {
                let rep = self.rep(&entities[0], ctx.is_expand(), &mut HashMap::new())?;
                return Ok(self.jsonify(&rep));
            }
            if !entities.is_empty() {
                return self.json_generator(&entities, name, false);
            }
            if method == "GET" {
                return Ok(abort(404, None));
            }
        }
        Ok(abort(405, None))
    }

    /// `resource_query`: `/item/query/<queries>`, `/album/query/<queries>`.
    fn resource_query(&self, kind: Kind, q: &str, method: &str, ctx: &Ctx) -> Result<Response> {
        // `QueryConverter`: `/` separates the parts; `\` is the path
        // separator, except in regular expressions.
        let parts: Vec<String> = q
            .split('/')
            .map(|p| {
                if p.contains("::") {
                    p.to_string()
                } else {
                    p.replace('\\', "/")
                }
            })
            .collect();
        let mut entities = self.fetch(kind, &parts)?;
        match method {
            "DELETE" => {
                if self.settings.readonly {
                    return Ok(abort(405, None));
                }
                for e in &entities {
                    self.remove(e, ctx.is_delete())?;
                }
                Ok(self.deleted())
            }
            "PATCH" if kind == Kind::Item => {
                if self.settings.readonly {
                    return Ok(abort(405, None));
                }
                if let Some(resp) = self.patch(&mut entities, ctx)? {
                    return Ok(resp);
                }
                self.json_generator(&entities, root(kind), false)
            }
            "GET" => self.json_generator(&entities, "results", ctx.is_expand()),
            _ => Ok(abort(405, None)),
        }
    }

    fn deleted(&self) -> Response {
        self.jsonify(&Json::Obj(vec![("deleted".into(), Json::Bool(true))]))
    }

    /// `/item/values/<key>`, `/album/values/<key>`: `SELECT DISTINCT` of a
    /// field, ordered by `sort_key` (default: the field).
    fn unique_values(&self, kind: Kind, key: &str, sort_key: &str) -> Result<Response> {
        let fixed = |k: &str| k == "id" || crate::model::field_type(kind, k).is_some();
        // beets' `all_keys()`: fixed fields and computed ones (which have
        // no column: an SQL error, so a 500).
        let computed = |k: &str| match kind {
            Kind::Item => matches!(k, "singleton" | "filesize" | "has_cover_art"),
            Kind::Album => {
                matches!(k, "path" | "albumtotal")
                    || k == "missing" && crate::plugins::enabled(self.cfg, "missing")
            }
        };
        if !(fixed(key) || computed(key)) || !(fixed(sort_key) || computed(sort_key)) {
            return Ok(abort(404, None));
        }
        if !fixed(key) || !fixed(sort_key) {
            bail!("no such column: {key} / {sort_key}");
        }
        let table = match kind {
            Kind::Item => "items",
            Kind::Album => "albums",
        };
        let rows = self.lib.query_column(&format!(
            "SELECT DISTINCT {key} FROM {table} ORDER BY {sort_key}"
        ))?;
        let values = rows.into_iter().map(sql_json).collect::<Result<_>>()?;
        Ok(self.jsonify(&Json::Obj(vec![("values".into(), Json::List(values))])))
    }

    /// `index.html`, with `url_for('static', ...)` filled in (Jinja drops
    /// the template's final newline).
    fn home(ctx: &Ctx) -> Response {
        let mut html = INDEX_HTML.to_string();
        for (name, _) in STATIC_FILES {
            html = html.replace(
                &format!("{{{{ url_for('static', filename='{name}') }}}}"),
                &format!("{}/static/{name}", ctx.script_name),
            );
        }
        if html.ends_with('\n') {
            html.pop();
        }
        Response::new(200, "text/html; charset=utf-8", html.into_bytes())
    }
}

fn root(kind: Kind) -> &'static str {
    match kind {
        Kind::Item => "items",
        Kind::Album => "albums",
    }
}

/// Python's `int()` of an id in a list (others are skipped).
fn py_int(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}

/// A raw SQL value, as `jsonify` takes it (bytes it can't serialize).
fn sql_json(v: rusqlite::types::Value) -> Result<Json> {
    use rusqlite::types::Value as V;
    Ok(match v {
        V::Null => Json::Null,
        V::Integer(i) => Json::Int(i),
        V::Real(f) => Json::Float(f),
        V::Text(s) => Json::Str(s),
        V::Blob(_) => bail!("Object of type bytes is not JSON serializable"),
    })
}

/// Flask's `request.get_json()`: 415 unless the request is JSON, 400 if
/// it doesn't parse.
fn request_json(req: &Request) -> std::result::Result<serde_json::Value, Response> {
    let ct = req
        .header("Content-Type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !(ct == "application/json" || ct.starts_with("application/") && ct.ends_with("+json")) {
        return Err(abort(
            415,
            Some(
                "Did not attempt to load JSON data because the request Content-Type was not \
                 'application/json'.",
            ),
        ));
    }
    serde_json::from_slice(&req.body)
        .map_err(|e| abort(400, Some(&format!("Failed to decode JSON object: {e}"))))
}

/// `item[key] = value` from a JSON value (beets' types normalize it; a
/// flexible attribute is stored as SQLite stores the Python value).
fn set_json(item: &mut Item, key: &str, v: &serde_json::Value) {
    use serde_json::Value as J;
    let ty = item.field_type(key);
    let value = match (v, ty) {
        (J::Null, _) => Value::Null,
        (J::Array(a), Some(FieldType::List(d))) => {
            let parts: Vec<String> = a
                .iter()
                .map(|x| match x {
                    J::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            Value::Str(crate::model::join_list(&parts, d))
        }
        (J::Bool(b), Some(_)) => Value::Bool(*b),
        (J::Bool(b), None) => Value::Str(if *b { "1" } else { "0" }.into()),
        (J::Number(n), Some(FieldType::Path)) | (J::Number(n), Some(FieldType::Str)) => {
            Value::Str(n.to_string())
        }
        (J::Number(n), _) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => {
                let f = n.as_f64().unwrap_or(0.0);
                if ty.is_some() {
                    Value::Float(f)
                } else {
                    Value::Str(crate::pyyaml::py_float_repr(f))
                }
            }
        },
        (J::String(s), Some(FieldType::Path)) => Value::Path(s.into()),
        (J::String(s), _) => Value::Str(s.clone()),
        (other, _) => Value::Str(other.to_string()),
    };
    if key == "path" && matches!(value, Value::Null) {
        return;
    }
    item.set(key, value);
}

/// Python's `mimetypes.guess_type` (Flask adds the charset to text).
pub(crate) fn mimetype(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "m4a" | "mp4" | "alac" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/x-wav",
        "aif" | "aiff" | "aifc" => "audio/x-aiff",
        "wma" => "audio/x-ms-wma",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Werkzeug's header-value quoting (a token as it is).
fn quote_header_value(v: &str) -> String {
    const TOKEN: &str = "!#$%&'*+-.^_`|~";
    if !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || TOKEN.contains(c))
    {
        return v.to_string();
    }
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Werkzeug's `Content-Disposition` for `send_file`: a non-ASCII name as
/// an ASCII `filename` plus a UTF-8 `filename*`.
pub(crate) fn content_disposition(kind: &str, name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    if name.is_ascii() {
        return format!("{kind}; filename={}", quote_header_value(name));
    }
    let simple: String = name.nfkd().filter(char::is_ascii).collect();
    let mut quoted = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            quoted.push(b as char);
        } else {
            write!(quoted, "%{b:02X}").unwrap();
        }
    }
    format!(
        "{kind}; filename={}; filename*=UTF-8''{quoted}",
        quote_header_value(&simple)
    )
}

/// `/item/<id>/file`: the file as an attachment, named as `http.server`
/// would (a name that isn't Latin-1 transliterated).
fn item_file(item: &Item, ctx: &Ctx) -> Result<Response> {
    let path = item.path();
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let safe = if base.chars().all(|c| (c as u32) < 256) {
        base
    } else {
        crate::template::unidecode(&base)
    };
    send_file(path, "attachment", &safe, ctx.req)
}

/// Werkzeug's `send_file`: the file with its type, `Content-Disposition`,
/// `Last-Modified`, and byte ranges (`Range: bytes=...`, for seeking in
/// the browser's player). A missing file is an error (a 500).
pub(crate) fn send_file(
    path: &Path,
    disposition: &str,
    name: &str,
    req: &Request,
) -> Result<Response> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let size = meta.len();
    let mut headers = vec![
        ("Content-Type".to_string(), mimetype(name).to_string()),
        (
            "Content-Disposition".into(),
            content_disposition(disposition, name),
        ),
        ("Accept-Ranges".into(), "bytes".into()),
    ];
    if let Ok(m) = meta.modified() {
        let t: chrono::DateTime<chrono::Utc> = m.into();
        headers.push((
            "Last-Modified".into(),
            t.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        ));
    }
    headers.push(("Cache-Control".into(), "no-cache".into()));
    let mut status = 200;
    let mut len = size;
    if let Some(range) = req.header("Range") {
        match parse_range(range, size) {
            Some(Some((start, end))) => {
                file.seek(SeekFrom::Start(start))?;
                len = end - start + 1;
                status = 206;
                headers.push((
                    "Content-Range".into(),
                    format!("bytes {start}-{end}/{size}"),
                ));
            }
            Some(None) => {
                let mut resp = abort(416, None);
                resp.headers
                    .push(("Content-Range".into(), format!("bytes */{size}")));
                return Ok(resp);
            }
            // Not a byte range we understand: the whole file.
            None => {}
        }
    }
    Ok(Response {
        status,
        headers,
        body: Body::File(file, len),
    })
}

/// A single `bytes=` range: Some(Some((first, last))), Some(None) if it
/// can't be satisfied, None if it isn't one.
fn parse_range(h: &str, size: u64) -> Option<Option<(u64, u64)>> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.trim().split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(None);
        }
        (size.saturating_sub(n), size.checked_sub(1)?)
    } else {
        let start: u64 = a.parse().ok()?;
        let end: u64 = if b.is_empty() {
            size.saturating_sub(1)
        } else {
            let e: u64 = b.parse().ok()?;
            if e < start {
                return None;
            }
            e.min(size.saturating_sub(1))
        };
        (start, end)
    };
    if size == 0 || range.0 >= size {
        return Some(None);
    }
    Some(Some(range))
}

// ----------------------------------------------------------------- CORS

impl Cors {
    /// flask-cors' `probably_regex`.
    fn is_regex(s: &str) -> bool {
        s.chars().any(|c| "*\\]?$^[]()".contains(c))
    }

    /// flask-cors' `try_match`.
    fn matches(value: &str, pattern: &str) -> bool {
        if Cors::is_regex(pattern) {
            // `re.match` with IGNORECASE: anchored at the start only.
            let p = format!("(?i)^(?:{pattern})");
            fancy_regex::Regex::new(&p).is_ok_and(|re| re.is_match(value).unwrap_or(false))
        } else {
            value.to_lowercase() == pattern.to_lowercase()
        }
    }

    /// flask-cors' `get_cors_headers` with the plugin's options (`origins`
    /// the `cors` setting, `*` meaning any; `allow_headers` Content-Type;
    /// the default methods; `always_send`, `vary_header`).
    fn apply(&self, req: &Request, resp: &mut Response) {
        const METHODS: &str = "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT";
        let origin = if self.origin == "*" {
            ".*".to_string()
        } else {
            self.origin.clone()
        };
        let wildcard = origin == ".*";
        let set = match req.header("Origin") {
            Some(o) if Cors::matches(o, &origin) => Some(o.to_string()),
            Some(_) => None,
            None if wildcard => (!self.supports_credentials).then(|| "*".to_string()),
            None if Cors::is_regex(&origin) => None,
            None => Some(origin.clone()),
        };
        let Some(set) = set else {
            return;
        };
        let h = &mut resp.headers;
        h.push(("Access-Control-Allow-Origin".into(), set.clone()));
        if self.supports_credentials {
            h.push(("Access-Control-Allow-Credentials".into(), "true".into()));
        }
        if req.method.eq_ignore_ascii_case("OPTIONS") {
            let m = req
                .header("Access-Control-Request-Method")
                .unwrap_or("")
                .to_ascii_uppercase();
            if !m.is_empty() && METHODS.split(", ").any(|x| x == m) {
                if let Some(rh) = req.header("Access-Control-Request-Headers") {
                    let mut ok: Vec<&str> = rh
                        .split(',')
                        .map(str::trim)
                        .filter(|x| Cors::matches(x, "Content-Type"))
                        .collect();
                    ok.sort();
                    if !ok.is_empty() {
                        h.push(("Access-Control-Allow-Headers".into(), ok.join(", ")));
                    }
                }
                h.push(("Access-Control-Allow-Methods".into(), METHODS.into()));
            }
        }
        if set != "*" && Cors::is_regex(&origin) {
            h.push(("Vary".into(), "Origin".into()));
        }
    }
}

// --------------------------------------------------------------- server

/// Run the app on `server` until it's unblocked: each request handled
/// here, its response written out on a thread of its own. Requests are
/// logged to stderr as Werkzeug does.
pub fn serve(app: &App, server: &tiny_http::Server) {
    serve_with(|req| app.handle(req), server);
}

/// [`serve`] with any request handler (the aura plugin's app).
pub fn serve_with(handle: impl Fn(&Request) -> Response, server: &tiny_http::Server) {
    for mut rq in server.incoming_requests() {
        let mut body = vec![];
        if rq.as_reader().read_to_end(&mut body).is_err() {
            continue;
        }
        let req = Request {
            method: rq.method().as_str().to_string(),
            url: rq.url().to_string(),
            headers: rq
                .headers()
                .iter()
                .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
                .collect(),
            body,
        };
        let resp = handle(&req);
        let remote = rq
            .remote_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_default();
        let v = rq.http_version();
        eprintln!(
            "{remote} - - [{}] \"{} {} HTTP/{}.{}\" {} -",
            chrono::Local::now().format("%d/%b/%Y %H:%M:%S"),
            req.method,
            req.url,
            v.0,
            v.1,
            resp.status
        );
        std::thread::spawn(move || {
            let _ = rq.respond(tiny_response(resp));
        });
    }
}

fn tiny_response(resp: Response) -> tiny_http::Response<Box<dyn Read + Send>> {
    let len = resp.len() as usize;
    let headers = resp
        .headers
        .iter()
        .filter_map(|(k, v)| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).ok())
        .collect();
    let data: Box<dyn Read + Send> = match resp.body {
        Body::Bytes(b) => Box::new(std::io::Cursor::new(b)),
        Body::File(f, n) => Box::new(f.take(n)),
    };
    tiny_http::Response::new(
        tiny_http::StatusCode(resp.status),
        headers,
        data,
        Some(len),
        None,
    )
}

/// `beet web [-d] [HOST] [PORT]`'s options.
#[derive(Debug, Default)]
pub struct CmdOpts {
    pub debug: bool,
    /// HOST and PORT (more are ignored, as beets does).
    pub args: Vec<String>,
}

/// `beet web`: serve the library on `web.host`/`web.port` (or HOST and
/// PORT) until interrupted.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let log = PluginLog::new("web");
    let host = match opts.args.first() {
        Some(h) => h.clone(),
        None => cfg.str("web.host"),
    };
    let port = match opts.args.get(1) {
        Some(p) => match p.trim().parse::<u16>() {
            Ok(p) => p,
            Err(_) => bail!("invalid literal for int() with base 10: '{p}'"),
        },
        None => match cfg.str("web.port").trim().parse::<u16>() {
            Ok(p) => p,
            Err(_) => bail!(
                "configuration error: web.port: must be a number, not {}",
                cfg.str("web.port")
            ),
        },
    };
    let settings = Settings::from_config(cfg, opts.debug);
    if let Some(c) = &settings.cors {
        log.info(&format!("Enabling CORS with origin: {}", c.origin));
    }
    let addr = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let server = match tiny_http::Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            let in_use = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
            if in_use {
                eprintln!(
                    "Address already in use\nPort {port} is in use by another program. Either \
                     identify and stop that program, or start the server with a different port."
                );
                std::process::exit(1);
            }
            bail!("{e}");
        }
    };
    let bound = server
        .server_addr()
        .to_ip()
        .map_or(addr.clone(), |a| a.to_string());
    println!(" * Debug mode: {}", if opts.debug { "on" } else { "off" });
    std::io::stdout().flush().ok();
    eprintln!(" * Running on http://{bound}\nPress CTRL+C to quit");
    let app = App { cfg, lib, settings };
    serve(&app, &server);
    Ok(())
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "web",
        order: 26,
        defaults: r#"web:
    host: 127.0.0.1
    port: 8337
    cors: ''
    cors_supports_credentials: no
    reverse_proxy: no
    include_paths: no
    readonly: yes
"#,
        config_keys: &["web"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                plugin: "web",
                build: build_web,
                run: run_web,
            },
        ],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(WebPlugin))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct WebArgs {
    /// Debug mode
    #[arg(short = 'd', long)]
    debug: bool,
    /// [HOST] [PORT]
    #[arg(value_name = "HOST PORT")]
    args: Vec<String>,
}

pub fn build_web() -> clap::Command {
    use clap::Args as _;
    WebArgs::augment_args(clap::Command::new("web").about("start a Web interface"))
}

pub fn run_web(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = WebArgs::from_arg_matches(m)?;
    crate::plugins::web::command(
        cfg,
        lib,
        &crate::plugins::web::CmdOpts {
            debug: a.debug,
            args: a.args,
        },
    )
}
