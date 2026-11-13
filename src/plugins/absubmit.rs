//! A port of beets 2.14's `absubmit` plugin (`beetsplug/absubmit.py`):
//! `beet absubmit` runs the AcousticBrainz `streaming_extractor_music`
//! binary on each item and submits its JSON analysis to an AcousticBrainz
//! server (`absubmit.base_url`; the plugin is deprecated since the public
//! server stopped accepting submissions).
//!
//! beets submits the analyses with `util.par_map` (one process per core);
//! this port does them one at a time.

use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Kind, Record};
use crate::plugins::PluginLog;
use crate::plugins::requests::{Request, Session};
use crate::query::Query;
use anyhow::{Result, bail};
use serde_json::Value as J;
use std::path::{Path, PathBuf};

/// We use this field to check whether AcousticBrainz info is present.
pub const PROBE_FIELD: &str = "mood_acoustic";

pub struct ABSubmit {
    /// The extractor binary's path.
    extractor: PathBuf,
    /// The SHA-1 of the extractor binary, sent as `essentia_build_sha`.
    extractor_sha: String,
    /// `base_url` + `{mbid}/low-level`; empty when no base_url is set.
    url: String,
    force: bool,
    pretend: bool,
    log: PluginLog,
}

impl ABSubmit {
    /// The plugin's `__init__`: find and hash the extractor, build the
    /// submission URL.
    pub fn new(cfg: &Config) -> Result<ABSubmit> {
        let log = PluginLog::new("absubmit");
        log.warning("This plugin is deprecated.");
        let extractor = Self::find_extractor(cfg)?;
        let bytes = std::fs::read(&extractor)?;
        let extractor_sha = sha1_hex(&bytes);
        let base_url = cfg.str("absubmit.base_url");
        let url = if base_url.is_empty() {
            String::new()
        } else {
            if !base_url.starts_with("http") {
                bail!("AcousticBrainz server base URL must start with an HTTP scheme");
            }
            let base_url = if base_url.ends_with('/') {
                base_url
            } else {
                format!("{base_url}/")
            };
            format!("{base_url}{{mbid}}/low-level")
        };
        Ok(ABSubmit {
            extractor,
            extractor_sha,
            url,
            force: cfg.bool("absubmit.force"),
            pretend: cfg.bool("absubmit.pretend"),
            log,
        })
    }

    /// beets' extractor lookup: the configured path, or the binary named
    /// `streaming_extractor_music` on `$PATH`.
    fn find_extractor(cfg: &Config) -> Result<PathBuf> {
        let configured = cfg.str("absubmit.extractor");
        if !configured.is_empty() {
            let extractor = crate::query::normpath(Path::new(&configured));
            if !extractor.is_file() {
                bail!("Extractor command does not exist: {}.", extractor.display());
            }
            return Ok(extractor);
        }
        let name = "streaming_extractor_music";
        match std::process::Command::new(name).output() {
            // Found: it exits with an error without the right number of
            // arguments, which is fine.
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "No extractor command found: please install the extractor binary from \
                 https://essentia.upf.edu/"
            ),
            Err(e) => bail!("{e}"),
        }
        match which(name) {
            Some(p) => Ok(p),
            None => bail!("Path to extractor command {name} not found"),
        }
    }

    /// beets' `command`: refuse to run without a `base_url`.
    pub fn check_configured(&self) -> Result<()> {
        if self.url.is_empty() {
            bail!(
                "This plugin is deprecated since AcousticBrainz no longer accepts new \
                 submissions. See the base_url configuration option."
            );
        }
        Ok(())
    }

    /// beets' `analyze_submit`: extract the analysis and submit it.
    pub fn analyze_submit(
        &self,
        cfg: &Config,
        lib: &Library,
        item: &Item,
        force_refetch: bool,
        pretend_fetch: bool,
    ) -> Result<()> {
        let Some(analysis) = self.get_analysis(cfg, lib, item, force_refetch, pretend_fetch)? else {
            return Ok(());
        };
        self.submit_data(cfg, lib, item, &analysis)
    }

    /// beets' `_get_analysis`: run the extractor, unless the analysis is
    /// there already or the item can't be submitted.
    pub fn get_analysis(
        &self,
        cfg: &Config,
        lib: &Library,
        item: &Item,
        force_refetch: bool,
        pretend_fetch: bool,
    ) -> Result<Option<J>> {
        let shown = crate::commands::format_record(
            &crate::template::Template::parse(&cfg.str("format_item")),
            item,
            Some(lib),
        );
        let mbid = item.str("mb_trackid");
        if !force_refetch && !self.force && probe_set(item) {
            return Ok(None);
        }
        if mbid.is_empty() {
            self.log
                .info(&format!("Not analysing {shown}, missing musicbrainz track id."));
            return Ok(None);
        }
        if pretend_fetch || self.pretend {
            self.log
                .info(&format!("pretend action - extract item: {shown}"));
            return Ok(None);
        }
        // The extractor writes its output to the file it's given.
        let file = temp_json_path();
        let result = (|| -> Result<Option<J>> {
            let path = item.path();
            if let Err(e) = call(&[&self.extractor, path, &file]) {
                self.log.warning(&format!(
                    "Failed to analyse {shown} for AcousticBrainz: {e}"
                ));
                return Ok(None);
            }
            let raw = std::fs::read(&file)?;
            let text = String::from_utf8_lossy(&raw);
            let mut analysis: J = match serde_json::from_str(&text) {
                Ok(a) => a,
                Err(e) => {
                    self.log.warning(&format!(
                        "Failed to parse analysis output for {shown}: {e}"
                    ));
                    return Ok(None);
                }
            };
            // Add the hash of the extractor to the output.
            let metadata = analysis
                .as_object_mut()
                .map(|o| o.entry("metadata").or_insert(J::Object(Default::default())))
                .and_then(|m| m.as_object_mut())
                .map(|o| o.entry("version").or_insert(J::Object(Default::default())))
                .and_then(|v| v.as_object_mut());
            match metadata {
                Some(m) => {
                    m.insert(
                        "essentia_build_sha".into(),
                        J::String(self.extractor_sha.clone()),
                    );
                }
                None => bail!("analysis output is not a JSON object"),
            }
            Ok(Some(analysis))
        })();
        let _ = std::fs::remove_file(&file);
        result
    }

    /// beets' `_submit_data`: POST the analysis to `base_url`.
    pub fn submit_data(
        &self,
        cfg: &Config,
        lib: &Library,
        item: &Item,
        data: &J,
    ) -> Result<()> {
        let url = self.url.replace("{mbid}", item.str("mb_trackid"));
        let headers = [("Content-Type", "application/json")];
        let session = Session::plain(crate::plugins::USER_AGENT);
        let response = match session.request(
            "POST",
            &url,
            Request {
                headers: &headers,
                json: Some(data),
                ..Default::default()
            },
        ) {
            Ok(r) => r,
            // `requests.post` raises on a transport error; beets doesn't
            // catch it.
            Err(e) => return Err(anyhow::anyhow!("{e}")),
        };
        if response.status != 200 {
            let message = match response.json() {
                Ok(j) => match j.get("message").and_then(|m| m.as_str()) {
                    Some(m) => m.to_string(),
                    None => "unable to get error message: 'message'".to_string(),
                },
                Err(e) => format!("unable to get error message: {e}"),
            };
            let shown = crate::commands::format_record(
                &crate::template::Template::parse(&cfg.str("format_item")),
                item,
                Some(lib),
            );
            self.log.error(&format!(
                "Failed to submit AcousticBrainz analysis for {shown}: {message}."
            ));
            return Ok(());
        }
        Ok(())
    }
}

impl crate::plugins::Plugin for ABSubmit {
    fn name(&self) -> &'static str {
        "absubmit"
    }
}

/// `util.command_output`: the command's exit status is checked.
fn call(args: &[&Path]) -> std::result::Result<(), String> {
    let mut cmd = std::process::Command::new(args[0]);
    cmd.args(&args[1..]);
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "{} exited with status {}",
            args[0].display(),
            out.status.code().unwrap_or(-1)
        )),
        Err(e) => Err(format!("{e}")),
    }
}

/// `shutil.which`: the executable on `$PATH`.
fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if let Ok(meta) = std::fs::metadata(&candidate)
            && meta.is_file()
            && meta.permissions().mode() & 0o111 != 0
        {
            return Some(candidate);
        }
    }
    None
}

/// `tempfile.mkstemp(suffix=".json")`.
fn temp_json_path() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{}{:08x}.json", std::process::id(), nanos ^ n as u32))
}

/// `beet absubmit`'s options.
#[derive(Debug, Default, Clone)]
pub struct CmdOpts {
    /// `-f/--force`: re-download data when already present.
    pub force_refetch: bool,
    /// `-p/--pretend`: only show what would be processed.
    pub pretend_fetch: bool,
    pub limit: Option<usize>,
    pub query: Vec<String>,
}

/// `beet absubmit`: analyse and submit the matching items.
pub fn command(cfg: &Config, lib: &Library, opts: &CmdOpts) -> Result<()> {
    let plugin = ABSubmit::new(cfg)?;
    plugin.check_configured()?;
    let mut items = lib.items(&Query::parse(Kind::Item, &opts.query)?)?;
    items.truncate(opts.limit.unwrap_or(usize::MAX));
    for item in &items {
        plugin.analyze_submit(cfg, lib, item, opts.force_refetch, opts.pretend_fetch)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ SHA-1

/// SHA-1 (FIPS 180-4) of `data`, as hex: `hashlib.sha1(data).hexdigest()`.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// Whether the analysis's probe field is present and true (`item.get(...)`
/// as Python sees it: a float in AcousticBrainz's output).
fn probe_set(item: &Record) -> bool {
    item.get(PROBE_FIELD)
        .map(|v| !v.raw().trim().is_empty() && v.raw().trim() != "0")
        .unwrap_or(false)
}


inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "absubmit",
        order: 38,
        defaults: r#"absubmit:
    extractor:
    force: no
    pretend: no
    base_url:
"#,
        config_keys: &["absubmit"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[
            crate::plugin_registry::PluginCommand {
                order: 380,
                plugin: "absubmit",
                build: build_absubmit,
                run: run_absubmit,
            },
        ],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(ABSubmit::new(cfg)?))
}

// ---------------------------------------------------------------- command

#[derive(clap::Args)]
pub struct AbsubmitArgs {
    /// Re-download data when already present
    #[arg(short = 'f', long)]
    force: bool,
    /// Pretend to perform action, but show only files which would be processed
    #[arg(short = 'p', long)]
    pretend: bool,
    /// Limit query results
    #[arg(short = 'l', long)]
    limit: Option<usize>,
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,
}

pub fn build_absubmit() -> clap::Command {
    use clap::Args as _;
    AbsubmitArgs::augment_args(clap::Command::new("absubmit").about(
        "Calculate and submit AcousticBrainz analysis (plugin: absubmit)",
    ))
}

pub fn run_absubmit(
    cfg: &crate::config::Config,
    lib: &crate::library::Library,
    _plugins: &[Box<dyn crate::plugins::Plugin>],
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    use clap::FromArgMatches as _;
    let a = AbsubmitArgs::from_arg_matches(m)?;
    crate::plugins::absubmit::command(
        cfg,
        lib,
        &crate::plugins::absubmit::CmdOpts {
            force_refetch: a.force,
            pretend_fetch: a.pretend,
            limit: a.limit,
            query: a.query,
        },
    )
}

#[cfg(test)]
#[path = "absubmit_tests.rs"]
mod tests;
