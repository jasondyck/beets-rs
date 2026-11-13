//! A port of beets' `beetsplug/_utils/requests.py`: an HTTP session with a
//! timeout, retries on transient server errors, a minimum interval between
//! requests and beets' error types (`HTTPNotFoundError`), plus the URL
//! helpers (`urllib.parse.quote`, `urlencode`) its users need.

use super::USER_AGENT;
use serde_json::Value as J;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `TimeoutAndRetrySession`'s default timeout.
const TIMEOUT: Duration = Duration::from_secs(10);
/// urllib3 `Retry(total=6, backoff_factor=0.5, status_forcelist=...)`.
const RETRIES: u32 = 6;
const RETRY_STATUSES: &[u16] = &[500, 502, 503, 504, 429];
#[cfg(not(test))]
const BACKOFF_FACTOR: f64 = 0.5;
#[cfg(test)]
const BACKOFF_FACTOR: f64 = 0.0;
/// `RateLimitAdapter(rate_limit=0.25)`: shared by every request, as beets'
/// session is a singleton.
#[cfg(not(test))]
const RATE_LIMIT: Duration = Duration::from_millis(250);
#[cfg(test)]
const RATE_LIMIT: Duration = Duration::from_millis(0);

static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// A request failure, as beets' handlers see it.
#[derive(Debug)]
pub enum HttpError {
    /// An HTTP error status. `HTTPNotFoundError` (and the like) for the
    /// codes beets has its own exception for, else requests' `HTTPError`.
    Status {
        code: u16,
        url: String,
        body: Vec<u8>,
        /// The response's headers (a 401's `WWW-Authenticate` challenge).
        headers: Vec<(String, String)>,
    },
    /// Transport failures and exhausted retries (`ConnectionError`,
    /// `RetryError`), with requests' message.
    Request(String),
    /// The response wasn't JSON (`requests.JSONDecodeError`).
    Json,
    /// Any other error a backend raises (`GeniusHTTPError`, ...).
    Other(String),
}

impl HttpError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, HttpError::Status { code: 404, .. })
    }
}

/// `http.HTTPStatus(code).phrase`.
pub fn reason(code: u16) -> &'static str {
    match code {
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Request Entity Too Large",
        414 => "Request-URI Too Long",
        415 => "Unsupported Media Type",
        418 => "I'm a Teapot",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // beets' `BeetsHTTPError`.
            HttpError::Status { code: 404, .. } => write!(f, "HTTP Error: 404 Not Found."),
            // requests' `raise_for_status`.
            HttpError::Status { code, url, .. } => {
                let kind = if *code < 500 { "Client" } else { "Server" };
                write!(f, "{code} {kind} Error: {} for url: {url}", reason(*code))
            }
            HttpError::Request(m) | HttpError::Other(m) => f.write_str(m),
            HttpError::Json => f.write_str("Could not decode response JSON data"),
        }
    }
}

impl std::error::Error for HttpError {}

#[derive(Debug)]
pub struct Response {
    content_type: String,
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl Response {
    /// A header's value, case-insensitively (the last one sent).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    /// The `Content-Type` header, if the server sent one.
    pub fn content_type(&self) -> Option<&str> {
        (!self.content_type.is_empty()).then_some(self.content_type.as_str())
    }

    /// The body as text: requests' `r.text`, decoded with the charset the
    /// response declares, else (`force_utf8: false`, requests guessing)
    /// UTF-8 with Latin-1 as the fallback for invalid UTF-8.
    pub fn text(&self, force_utf8: bool) -> String {
        let charset = self
            .content_type
            .split(';')
            .skip(1)
            .filter_map(|p| p.trim().split_once('='))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("charset"))
            .map(|(_, v)| v.trim().trim_matches('"').to_ascii_lowercase());
        let latin1 = |b: &[u8]| b.iter().map(|&c| c as char).collect::<String>();
        match charset.as_deref() {
            Some("iso-8859-1" | "latin-1" | "latin1" | "windows-1252" | "cp1252") => {
                latin1(&self.body)
            }
            Some(_) => String::from_utf8_lossy(&self.body).into_owned(),
            None if force_utf8 => String::from_utf8_lossy(&self.body).into_owned(),
            None => match std::str::from_utf8(&self.body) {
                Ok(s) => s.to_string(),
                Err(_) => latin1(&self.body),
            },
        }
    }

    pub fn json(&self) -> Result<J, HttpError> {
        serde_json::from_slice(&self.body).map_err(|_| HttpError::Json)
    }
}

/// beets' `TimeoutAndRetrySession` + `RequestHandler`.
pub struct Session {
    agent: ureq::Agent,
    /// For `allow_redirects=False`.
    no_redirects: ureq::Agent,
    /// Retries and the shared rate limit (off for [`Session::plain`]).
    retries: bool,
    /// Retry 429 like the other transient statuses (off for tidal, which
    /// waits for the response's `Retry-After` itself).
    retry_429: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// What to send.
#[derive(Default)]
pub struct Request<'a> {
    pub headers: &'a [(&'a str, &'a str)],
    pub json: Option<&'a J>,
    pub allow_redirects: bool,
    /// An `application/x-www-form-urlencoded` body (see [`urlencode`]).
    pub form: Option<&'a str>,
}

impl Session {
    pub fn new() -> Self {
        Self::build(USER_AGENT, true)
    }

    /// A session that retries 5xx and transport failures but hands a 429
    /// back to the caller (beets' tidal session, which honours
    /// `Retry-After`).
    pub fn new_without_429_retry() -> Self {
        Session {
            retry_429: false,
            ..Self::build(USER_AGENT, true)
        }
    }

    /// A plain `requests.Session` as fetchart's `_logged_get` uses it: the
    /// given User-Agent, a 10 s timeout, no retries, no rate limit, and no
    /// `raise_for_status` (error responses come back as responses).
    pub fn plain(user_agent: &str) -> Self {
        Self::build(user_agent, false)
    }

    fn build(user_agent: &str, retries: bool) -> Self {
        let build = |redirects| {
            ureq::AgentBuilder::new()
                .user_agent(user_agent)
                .timeout(TIMEOUT)
                .redirects(redirects)
                .build()
        };
        Session {
            agent: build(5),
            no_redirects: build(0),
            retries,
            retry_429: true,
        }
    }

    /// GET without `raise_for_status`: any response the server sends is
    /// returned (see `Response::status`); only transport failures (and
    /// exhausted retries) are errors.
    pub fn get_any(&self, url: &str, req: Request) -> Result<Response, HttpError> {
        match self.request("GET", url, req) {
            Ok(r) => Ok(r),
            Err(HttpError::Status {
                code,
                body,
                headers,
                ..
            }) => Ok(Response {
                content_type: String::new(),
                status: code,
                body,
                headers,
            }),
            Err(e) => Err(e),
        }
    }

    fn wait_for_rate_limit() {
        let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < RATE_LIMIT {
                std::thread::sleep(RATE_LIMIT - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    /// Send a request, retrying transient failures as urllib3's `Retry`
    /// does; an error status is an error (`raise_for_status`).
    pub fn request(&self, method: &str, url: &str, req: Request) -> Result<Response, HttpError> {
        let agent = if req.allow_redirects {
            &self.agent
        } else {
            &self.no_redirects
        };
        let mut errors = 0u32;
        loop {
            if self.retries {
                Self::wait_for_rate_limit();
            }
            let mut r = agent.request(method, url);
            for (k, v) in req.headers {
                r = r.set(k, v);
            }
            let result = match (req.json, req.form) {
                (Some(body), _) => r.send_json(body),
                (None, Some(form)) => r
                    .set("Content-Type", "application/x-www-form-urlencoded")
                    .send_string(form),
                (None, None) => r.call(),
            };
            let retry_cause = match result {
                Ok(resp) => return read(resp),
                // A plain session hands every response back, as `requests`.
                Err(ureq::Error::Status(_, resp)) if !self.retries => return read(resp),
                Err(ureq::Error::Status(code, resp)) => {
                    if !RETRY_STATUSES.contains(&code) || (code == 429 && !self.retry_429) {
                        let resp = read(resp)?;
                        return Err(HttpError::Status {
                            code,
                            url: url.to_string(),
                            headers: resp.headers.clone(),
                            body: resp.body,
                        });
                    }
                    format!("ResponseError('too many {code} error responses')")
                }
                Err(ureq::Error::Transport(t)) if !self.retries => {
                    return Err(HttpError::Request(t.to_string()));
                }
                Err(ureq::Error::Transport(t)) => format!("{t}"),
            };
            errors += 1;
            if errors > RETRIES {
                return Err(HttpError::Request(max_retries_message(url, &retry_cause)));
            }
            // urllib3's backoff: none before the first retry, then doubling.
            if errors > 1 {
                let secs = (BACKOFF_FACTOR * 2f64.powi(errors as i32 - 1)).min(120.0);
                std::thread::sleep(Duration::from_secs_f64(secs));
            }
        }
    }

    pub fn get(&self, url: &str, req: Request) -> Result<Response, HttpError> {
        self.request("GET", url, req)
    }
}

/// Read a response's body (requests raises on a read error, e.g. a
/// truncated chunked body).
fn read(resp: ureq::Response) -> Result<Response, HttpError> {
    use std::io::Read;
    let content_type = resp.header("content-type").unwrap_or_default().to_string();
    let status = resp.status();
    let mut headers = vec![];
    for name in resp.headers_names() {
        for v in resp.all(&name) {
            headers.push((name.clone(), v.to_string()));
        }
    }
    let mut body = vec![];
    resp.into_reader()
        .take(64 << 20)
        .read_to_end(&mut body)
        .map_err(|e| HttpError::Request(e.to_string()))?;
    Ok(Response {
        content_type,
        status,
        body,
        headers,
    })
}

/// urllib3's `MaxRetryError` message, as requests reports it.
fn max_retries_message(url: &str, cause: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) => (h, p.to_string()),
        _ => (
            hostport,
            if scheme == "https" { "443" } else { "80" }.to_string(),
        ),
    };
    let pool = if scheme == "https" {
        "HTTPSConnectionPool"
    } else {
        "HTTPConnectionPool"
    };
    format!(
        "{pool}(host='{host}', port={port}): Max retries exceeded with url: {path} (Caused by {cause})"
    )
}

/// `urllib.parse.quote(s, safe)`.
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || safe.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `urllib.parse.quote_plus(s)`.
pub fn quote_plus(s: &str) -> String {
    quote(s, " ").replace(' ', "+")
}

/// `urllib.parse.urlencode(params)`.
pub fn urlencode(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// beets' `LyricsRequestHandler.format_url`.
pub fn format_url(url: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        url.to_string()
    } else {
        format!("{url}?{}", urlencode(params))
    }
}

/// `urlparse(url).netloc`.
pub fn netloc(url: &str) -> &str {
    let Some((_, rest)) = url.split_once("://") else {
        return "";
    };
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

#[cfg(test)]
pub mod testserver {
    //! A tiny HTTP server on 127.0.0.1, port 0, standing in for the web
    //! services in tests (as beets' tests use `requests_mock`).

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    pub struct Seen {
        pub method: String,
        /// Path and query, as sent.
        pub target: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Seen {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    pub struct Reply {
        pub status: u16,
        pub content_type: &'static str,
        pub body: Vec<u8>,
    }

    impl Reply {
        pub fn json(v: serde_json::Value) -> Reply {
            Reply {
                status: 200,
                content_type: "application/json",
                body: v.to_string().into_bytes(),
            }
        }
        pub fn text(s: &str) -> Reply {
            Reply {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: s.as_bytes().to_vec(),
            }
        }
        pub fn status(status: u16) -> Reply {
            Reply {
                status,
                content_type: "text/plain",
                body: vec![],
            }
        }
    }

    type Handler = dyn Fn(&Seen) -> Reply + Send + Sync;

    pub struct Server {
        pub base: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        pub fn requests(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    pub fn serve(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let seen2 = seen.clone();
        let handler: Arc<Handler> = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let mut headers = vec![];
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let len = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; len];
                // A test client that hangs up is not an error here.
                let _ = reader.read_exact(&mut body);
                let req = Seen {
                    method,
                    target,
                    headers,
                    body,
                };
                seen2.lock().unwrap().push(req.clone());
                let reply = handler(&req);
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    super::reason(reply.status),
                    reply.content_type,
                    reply.body.len()
                );
                // The test client may have gone away; nothing to log.
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&reply.body);
            }
        });
        Server { base, seen }
    }
}

#[cfg(test)]
mod tests {
    use super::testserver::{Reply, serve};
    use super::*;

    #[test]
    fn quoting_as_python() {
        assert_eq!(quote("a b/c(d)~", "/"), "a%20b/c%28d%29~");
        assert_eq!(quote_plus("a b/c&é"), "a+b%2Fc%26%C3%A9");
        assert_eq!(
            urlencode(&[("artist_name", "A & B".into()), ("duration", "3".into())]),
            "artist_name=A+%26+B&duration=3"
        );
        assert_eq!(netloc("https://www.x.com:80/a?b"), "www.x.com:80");
    }

    #[test]
    fn errors_and_retries() {
        let s = serve(|r| match r.target.as_str() {
            "/missing" => Reply::status(404),
            "/forbidden" => Reply::status(403),
            "/bad" => Reply::status(502),
            _ => Reply::text("ok"),
        });
        let session = Session::new();
        let get = |p: &str| session.get(&format!("{}{p}", s.base), Request::default());
        assert_eq!(get("/fine").unwrap().text(false), "ok");
        assert_eq!(
            get("/missing").unwrap_err().to_string(),
            "HTTP Error: 404 Not Found."
        );
        assert_eq!(
            get("/forbidden").unwrap_err().to_string(),
            format!("403 Client Error: Forbidden for url: {}/forbidden", s.base)
        );
        let err = get("/bad").unwrap_err().to_string();
        assert!(
            err.ends_with(
                "Max retries exceeded with url: /bad (Caused by ResponseError('too many 502 error responses'))"
            ),
            "{err}"
        );
        // Six retries after the first attempt.
        let bad = s.requests().iter().filter(|r| r.target == "/bad").count();
        assert_eq!(bad, 7);
    }

    #[test]
    fn a_truncated_body_is_a_request_error() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            // Promise 100 bytes and close after five.
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort")
                .unwrap();
        });
        // requests raises on a body read error; so does `read`.
        let err = Session::plain("beets-test")
            .get_any(&format!("http://{addr}/"), Request::default())
            .unwrap_err();
        assert!(matches!(err, HttpError::Request(_)), "{err:?}");
    }
}
