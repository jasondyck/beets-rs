//! A port of beets 2.14's `plexupdate` plugin (`beetsplug/plexupdate.py`):
//! after a command that changed the library, ask the Plex server to
//! refresh its music library. `beet plexupdate --auth` logs in to plex.tv
//! with its PIN flow and stores the token next to the configuration.
//!
//! Settings: `plex.host`, `port`, `library_name`, `secure`,
//! `ignore_cert_errors`, `tokenfile` and the deprecated `token`.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::plugins::PluginLog;
use crate::plugins::requests::{HttpError, Request, Session};
use serde_json::{Map, Value as J};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// plex.tv's API, used by the interactive PIN authentication.
pub const PLEX_API: &str = "https://plex.tv/api/v2";
/// The page the user visits to authorize a pin.
pub const PLEX_AUTH_URL: &str = "https://app.plex.tv/auth#?";

/// The `plexupdate` command's options.
pub struct CmdOpts {
    pub auth: bool,
}

/// `PlexSession`: requests with the plex.tv identifying headers, the auth
/// token from the token file (or `plex.token`) and PIN authentication.
pub struct PlexSession {
    pub token_path: PathBuf,
    pub token_override: Option<String>,
    pub api_root: String,
    /// `time.sleep(5)` between pin polls.
    pub poll_interval: std::cell::Cell<std::time::Duration>,
    session: Session,
}

impl PlexSession {
    pub fn new(token_path: PathBuf, token_override: Option<String>, api_root: &str) -> Self {
        PlexSession {
            token_path,
            token_override,
            api_root: api_root.to_string(),
            poll_interval: std::cell::Cell::new(std::time::Duration::from_secs(5)),
            session: Session::new(),
        }
    }

    /// `load_token`: the token file's JSON (empty when it is missing or
    /// unreadable).
    pub fn load_token(&self) -> Map<String, J> {
        std::fs::read_to_string(&self.token_path)
            .ok()
            .and_then(|s| serde_json::from_str::<J>(&s).ok())
            .and_then(|v| match v {
                J::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// `save_token`: merge into the token file.
    pub fn save_token(&self, data: &[(&str, J)]) {
        let mut token = self.load_token();
        for (k, v) in data {
            token.insert(k.to_string(), v.clone());
        }
        let _ = std::fs::write(
            &self.token_path,
            serde_json::to_string_pretty(&J::Object(token)).unwrap_or_default(),
        );
    }

    pub fn client_id(&self) -> String {
        if let Some(id) = self
            .load_token()
            .get("client_identifier")
            .and_then(J::as_str)
        {
            return id.to_string();
        }
        let id = uuid_hex();
        self.save_token(&[("client_identifier", J::String(id.clone()))]);
        id
    }

    pub fn token(&self) -> Option<String> {
        self.token_override.clone().or_else(|| {
            self.load_token()
                .get("X-Plex-Token")
                .and_then(J::as_str)
                .map(String::from)
        })
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            ("accept", "application/json".to_string()),
            ("X-Plex-Product", "beets".to_string()),
            (
                "X-Plex-Product-Version",
                crate::plugins::mbapi::BEETS_VERSION.to_string(),
            ),
            ("X-Plex-Client-Identifier", self.client_id()),
        ];
        if let Some(token) = self.token() {
            headers.push(("X-Plex-Token", token));
        }
        headers
    }

    /// A request with the session's headers.
    pub fn request(
        &self,
        method: &str,
        url: &str,
    ) -> Result<crate::plugins::requests::Response, HttpError> {
        let headers = self.headers();
        let refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.session.request(
            method,
            url,
            Request {
                headers: &refs,
                ..Default::default()
            },
        )
    }

    pub fn get(&self, url: &str) -> Result<crate::plugins::requests::Response, HttpError> {
        self.request("GET", url)
    }

    /// `create_pin`.
    pub fn create_pin(&self) -> Result<J, HttpError> {
        let url = format!("{}/pins?strong=True", self.api_root);
        match self.request("POST", &url) {
            Ok(resp) => resp.json(),
            Err(HttpError::Status { code, body, .. }) => {
                // `raise_for_status` on a non-JSON error body.
                let _ = body;
                Err(HttpError::Status {
                    code,
                    url,
                    body: vec![],
                    headers: vec![],
                })
            }
            Err(e) => Err(e),
        }
    }

    /// `wait_for_login`: poll the pin until it has an `authToken`.
    pub fn wait_for_login(&self, pin_id: i64, timeout: std::time::Duration) -> Option<String> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let mut data = J::Null;
            if let Ok(resp) = self.get(&format!("{}/pins/{pin_id}", self.api_root))
                && let Ok(json) = resp.json()
            {
                data = json;
            }
            if let Some(token) = data.get("authToken").and_then(J::as_str) {
                return Some(token.to_string());
            }
            // 5 s polling, as beets.
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            std::thread::sleep(left.min(self.poll_interval.get()));
        }
        None
    }
}

fn uuid_hex() -> String {
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
        .is_err()
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (t >> (i % 16 * 8)) as u8;
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `PlexAPI`: the local server's endpoints and plex.tv authentication.
pub struct PlexAPI {
    pub token_path: PathBuf,
    pub token_override: Option<String>,
    pub library_name: String,
    pub base_url: String,
    pub api_root: String,
    /// `ignore_cert_errors` (see the module's note on ureq's TLS).
    #[allow(dead_code)]
    pub verify: bool,
    /// Open the authorization URL in a browser (off in tests).
    pub open_browser: bool,
    session: RefCell<Option<Rc<PlexSession>>>,
}

impl PlexAPI {
    pub fn new(
        token_path: PathBuf,
        host: &str,
        port: u16,
        library_name: &str,
        secure: bool,
        verify: bool,
        token_override: Option<String>,
    ) -> Self {
        PlexAPI {
            token_path,
            token_override,
            library_name: library_name.to_string(),
            base_url: format!("{}://{host}:{port}/", if secure { "https" } else { "http" }),
            api_root: PLEX_API.to_string(),
            verify,
            open_browser: true,
            session: RefCell::new(None),
        }
    }

    /// The lazy `session` property.
    pub fn session(&self) -> Rc<PlexSession> {
        if let Some(s) = self.session.borrow().as_ref() {
            return s.clone();
        }
        let session = Rc::new(PlexSession::new(
            self.token_path.clone(),
            self.token_override.clone(),
            &self.api_root,
        ));
        *self.session.borrow_mut() = Some(session.clone());
        session
    }

    fn get_json(&self, url: &str) -> Result<J, HttpError> {
        let resp = self.session().get(url).map_err(plex_http_error)?;
        resp.json()
    }

    /// `ui_authenticate_flow`: the plex.tv PIN login.
    pub fn ui_authenticate_flow(&self, log: &PluginLog) -> anyhow::Result<()> {
        let session = self.session();
        let pin = session
            .create_pin()
            .map_err(|e| anyhow::anyhow!("Plex login flow failed: {e}"))?;
        let code = pin
            .get("code")
            .and_then(J::as_str)
            .unwrap_or_default()
            .to_string();
        let auth_url = format!(
            "{PLEX_AUTH_URL}{}",
            crate::plugins::requests::urlencode(&[
                ("clientID", session.client_id()),
                ("code", code),
                ("context[device][product]", "beets".to_string()),
            ])
        );
        println!("Please visit: {auth_url}");
        println!("Waiting for authorization in your browser...");
        if self.open_browser {
            let _ = open_browser(&auth_url);
        }

        let pin_id = pin.get("id").and_then(J::as_i64).unwrap_or(0);
        let Some(account_token) =
            session.wait_for_login(pin_id, std::time::Duration::from_secs(300))
        else {
            anyhow::bail!(
                "Plex authentication timed out. Please run `beet plexupdate --auth` again."
            );
        };
        session.save_token(&[("X-Plex-Token", J::String(account_token))]);
        println!(
            "Authentication successful! Token saved to {}.",
            session.token_path.display()
        );
        log.debug("Plex authentication finished");
        Ok(())
    }

    /// `get_music_section`: the key of the section named after
    /// `plex.library_name`.
    pub fn get_music_section(&self) -> Result<Option<String>, HttpError> {
        let data = self.get_json(&join(&self.base_url, "library/sections"))?;
        for directory in data
            .get("MediaContainer")
            .and_then(|m| m.get("Directory"))
            .and_then(|d| d.as_array())
            .into_iter()
            .flatten()
        {
            if directory.get("title").and_then(J::as_str) == Some(self.library_name.as_str()) {
                let key = directory.get("key").cloned().unwrap_or(J::Null);
                return Ok(Some(match key {
                    J::String(s) => s,
                    other => other.to_string(),
                }));
            }
        }
        Ok(None)
    }

    /// `update_library`: refresh the section.
    pub fn update_library(&self) -> anyhow::Result<()> {
        let Some(key) = self
            .get_music_section()
            .map_err(|e| anyhow::anyhow!("{e}"))?
        else {
            anyhow::bail!(
                "No music library named '{}' found on the Plex server. \
                 Check the `plex.library_name` setting.",
                self.library_name
            );
        };
        self.session()
            .get(&join(
                &self.base_url,
                &format!("library/sections/{key}/refresh"),
            ))
            .map_err(plex_error)?;
        Ok(())
    }
}

/// `urllib.parse.urljoin` for a base ending in `/`.
fn join(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/').to_string() + "/")
}

/// `UnauthorizedPlexError`'s message for a 401.
fn plex_http_error(e: HttpError) -> HttpError {
    match e {
        HttpError::Status { code: 401, .. } => HttpError::Other(
            "HTTP Error: 401 Unauthorized. You may need to reauthenticate \
             using `beet plexupdate --auth`."
                .into(),
        ),
        other => other,
    }
}

fn plex_error(e: HttpError) -> anyhow::Error {
    anyhow::anyhow!("{}", plex_http_error(e))
}

/// `webbrowser.open`, with its errors suppressed.
fn open_browser(url: &str) -> bool {
    if std::env::var_os("BEETSRS_NO_BROWSER").is_some() {
        return false;
    }
    std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

pub struct PlexUpdate {
    cfg: Config,
    log: PluginLog,
}

impl crate::plugins::Plugin for PlexUpdate {
    fn name(&self) -> &'static str {
        "plex"
    }
}

impl PlexUpdate {
    pub fn new(cfg: &Config) -> Self {
        PlexUpdate {
            cfg: cfg.clone(),
            log: PluginLog::new("plexupdate"),
        }
    }

    fn tokenfile(&self) -> PathBuf {
        let name = self.cfg.str("plex.tokenfile");
        let p = Path::new(&name);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cfg.config_dir.join(p)
        }
    }

    /// The `api` property.
    pub fn api(&self) -> PlexAPI {
        PlexAPI::new(
            self.tokenfile(),
            &self.cfg.str("plex.host"),
            self.cfg.str("plex.port").parse().unwrap_or(32400),
            &self.cfg.str("plex.library_name"),
            self.cfg.bool("plex.secure"),
            !self.cfg.bool("plex.ignore_cert_errors"),
            Some(self.cfg.str("plex.token")).filter(|t| !t.is_empty()),
        )
    }

    /// `update`: refresh the library when the command exits.
    pub fn update(&self) {
        self.log.info("Updating Plex library...");
        match self.api().update_library() {
            Ok(()) => self.log.info("Started library update successfully."),
            Err(e) => {
                let msg = format!("{e}");
                if msg.starts_with("HTTP Error: 401") {
                    self.log.error(
                        "Plex library update failed: the server rejected the request (401). \
                         If it requires authentication, run `beet plexupdate --auth`.",
                    );
                } else if msg.starts_with("No music library") {
                    self.log
                        .error(&format!("Plex library update failed: {msg}"));
                } else {
                    self.log.error(&format!("Library update failed!\n{msg}"));
                }
            }
        }
    }
}

struct ListenerState {
    plugin: Rc<PlexUpdate>,
    registered: Cell<bool>,
}

impl Listener for ListenerState {
    fn wants(&self, event: &str) -> bool {
        event == "database_change"
    }

    fn on(&self, _ev: &Event) {
        if !self.registered.replace(true) {
            crate::events::register(Rc::new(ExitListener {
                plugin: self.plugin.clone(),
            }));
        }
    }
}

struct ExitListener {
    plugin: Rc<PlexUpdate>,
}

impl Listener for ExitListener {
    fn wants(&self, event: &str) -> bool {
        event == "cli_exit"
    }

    fn on(&self, _ev: &Event) {
        self.plugin.update();
    }
}

/// The plugin's `__init__`: listen for changes (the exit-time refresh is
/// registered on the first one, so read-only commands skip it).
pub fn load(cfg: &Config) -> PlexUpdate {
    crate::events::register(Rc::new(ListenerState {
        plugin: Rc::new(PlexUpdate::new(cfg)),
        registered: Cell::new(false),
    }));
    PlexUpdate::new(cfg)
}

/// `beet plexupdate [-a]`: `--auth` logs in; without it, the command's
/// help is shown.
pub fn command(cfg: &Config, opts: CmdOpts) -> anyhow::Result<()> {
    let plugin = PlexUpdate::new(cfg);
    if opts.auth {
        plugin.api().ui_authenticate_flow(&plugin.log)?;
    } else {
        build_plexupdate().print_help()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "plexupdate_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "plexupdate",
        order: 43,
        defaults: r#"plex:
    host: localhost
    port: 32400
    library_name: Music
    secure: no
    ignore_cert_errors: no
    tokenfile: plex_token.json
    token: ''
"#,
        config_keys: &["plex"],
        redacted: &["plex.token"],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 440,
                plugin: "plexupdate",
                build: build_plexupdate,
                run: run_plexupdate,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct PlexupdateArgs {
    /// Authenticate and login to Plex
    #[arg(short = 'a', long)]
    auth: bool,
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

pub fn build_plexupdate() -> clap::Command {
    use clap::Args as _;
    PlexupdateArgs::augment_args(
        clap::Command::new("plexupdate")
            .bin_name("beet plexupdate")
            .about("Update Plex library when beets library changes (plugin: plexupdate)"),
    )
}

pub fn run_plexupdate(
    cfg: &crate::config::Config,
    _lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = PlexupdateArgs::from_arg_matches(m)?;
    command(cfg, CmdOpts { auth: a.auth })
}
