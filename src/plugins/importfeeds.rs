//! beets' `importfeeds` plugin: writes the paths of imported items to m3u
//! files (or symlinks/echo) when an album or item is imported.

use crate::config::Config;
use crate::events::{Event, Listener};
use crate::model::Record;
use crate::plugins::{Plugin, PluginLog};
use anyhow::{Context, Result};
use chrono::Local;
use std::cell::RefCell;
use std::path::{Component, Path, PathBuf};

pub struct ImportFeeds {
    cfg: Config,
    formats: Vec<String>,
    m3u_name: String,
    dir: Option<PathBuf>,
    relative_to: Option<PathBuf>,
    absolute: bool,
    /// The `m3u_session` file, made at `import_begin`.
    session: RefCell<Option<PathBuf>>,
    log: PluginLog,
}

impl ImportFeeds {
    pub fn new(cfg: &Config) -> Self {
        let dir = cfg
            .str("importfeeds.dir")
            .is_empty()
            .then_some(())
            .map_or_else(
                || {
                    let d = cfg.str("importfeeds.dir");
                    Some(crate::config::expand_tilde(&d))
                },
                |_| None,
            );
        let relative = cfg.str("importfeeds.relative_to");
        let relative_to = if relative.is_empty() {
            None
        } else {
            Some(crate::config::expand_tilde(&relative))
        };
        let log = PluginLog::new("importfeeds");
        // beets raises a plugin listener's logger to WARNING (INFO with -v).
        log.set_base_level(crate::plugins::WARNING);
        ImportFeeds {
            cfg: cfg.clone(),
            formats: cfg.list("importfeeds.formats"),
            m3u_name: cfg.str("importfeeds.m3u_name"),
            dir,
            relative_to,
            absolute: cfg.bool("importfeeds.absolute_path"),
            session: RefCell::new(None),
            log,
        }
    }

    fn feeds_dir(&self) -> PathBuf {
        self.dir.clone().unwrap_or_else(|| self.cfg.directory())
    }

    fn base(&self) -> PathBuf {
        self.relative_to.clone().unwrap_or_else(|| self.feeds_dir())
    }

    pub(crate) fn record(&self, basename: &str, items: &[Record]) -> Result<()> {
        let feeds = self.feeds_dir();
        let base = self.base();
        let mut paths = vec![];
        for item in items {
            let p = item.path();
            paths.push(if self.absolute {
                p.to_path_buf()
            } else {
                relative_path(p, &base)
            });
        }
        if self.formats.iter().any(|f| f == "m3u") {
            write_m3u(&feeds.join(&self.m3u_name), &paths)?;
        }
        if self.formats.iter().any(|f| f == "m3u_session")
            && let Some(session) = self.session.borrow().as_ref()
        {
            write_m3u(session, &paths)?;
        }
        if self.formats.iter().any(|f| f == "m3u_multi") {
            let date = Local::now().format("%Y%m%d_%Hh%M").to_string();
            let name = basename
                .chars()
                .map(|c| {
                    if c.is_whitespace() || ",/\\'\"".contains(c) {
                        '_'
                    } else {
                        c
                    }
                })
                .collect::<String>();
            write_m3u(&feeds.join(format!("{date}_{name}.m3u")), &paths)?;
        }
        if self.formats.iter().any(|f| f == "link") {
            for p in &paths {
                let dest = feeds.join(p.file_name().unwrap_or_default());
                if !dest.exists()
                    && let Err(e) = std::os::unix::fs::symlink(p, &dest)
                {
                    eprintln!(
                        "warning: importfeeds: could not create symlink for {}: {e}",
                        p.display()
                    );
                }
            }
        }
        if self.formats.iter().any(|f| f == "echo") {
            // Only shown with -v: listeners log at WARNING (see `new`).
            self.log.info("Location of imported music:");
            for p in &paths {
                self.log.info(&format!("  {}", p.display()));
            }
        }
        Ok(())
    }

    fn begin(&self) {
        if !self.formats.iter().any(|f| f == "m3u_session") {
            return;
        }
        let date = Local::now().format("%Y%m%d_%Hh%M").to_string();
        let base = self.m3u_name.clone();
        let base = base
            .strip_suffix(".m3u")
            .or_else(|| base.strip_suffix(".M3U"))
            .unwrap_or(&base);
        *self.session.borrow_mut() = Some(self.feeds_dir().join(format!("{base}_{date}.m3u")));
    }
}

fn write_m3u(path: &Path, paths: &[PathBuf]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    for p in paths {
        writeln!(f, "{}", p.display())?;
    }
    Ok(())
}

/// Python's `os.path.relpath(path, base)`, as far as it can be done
/// lexically.
fn relative_path(path: &Path, base: &Path) -> PathBuf {
    let abs = |p: &Path| {
        let mut v = vec![];
        for c in p.components() {
            match c {
                Component::RootDir => {}
                Component::CurDir => {}
                Component::ParentDir => {
                    v.pop();
                }
                Component::Normal(x) => v.push(x.to_os_string()),
                Component::Prefix(_) => v.push(c.as_os_str().to_os_string()),
            }
        }
        v
    };
    let (p, b) = (abs(path), abs(base));
    let mut common = 0;
    while common < p.len() && common < b.len() && p[common] == b[common] {
        common += 1;
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

impl Plugin for ImportFeeds {
    fn name(&self) -> &'static str {
        "importfeeds"
    }
}

impl Listener for ImportFeeds {
    fn wants(&self, event: &str) -> bool {
        matches!(event, "import_begin" | "album_imported" | "item_imported")
    }

    fn on(&self, event: &Event) {
        // `import_begin` carries only the session.
        if event.name == "import_begin" {
            self.begin();
            return;
        }
        let Some(lib) = event.lib() else { return };
        match event.name {
            "album_imported" => {
                if let Some(album) = event.album()
                    && let Ok(items) = lib.album_items(album.id.unwrap_or(0))
                    && let Err(e) = self.record(album.str("album"), &items)
                {
                    eprintln!("warning: importfeeds: {e:#}");
                }
            }
            "item_imported" => {
                if let Some(item) = event.item()
                    && let Err(e) = self.record(item.str("title"), std::slice::from_ref(item))
                {
                    eprintln!("warning: importfeeds: {e:#}");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "importfeeds_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "importfeeds",
        order: 40,
        defaults: r#"importfeeds:
    formats: []
    m3u_name: imported.m3u
    dir:
    relative_to:
    absolute_path: no
"#,
        config_keys: &["importfeeds"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    // beets registers the listeners in the plugin's `__init__`.
    crate::events::register(std::rc::Rc::new(ImportFeeds::new(cfg)));
    Ok(Box::new(ImportFeeds::new(cfg)))
}
