//! A port of beets 2.14's `hook` plugin (`beetsplug/hook.py`): run a
//! command when an event is sent. Each `hook.hooks` entry names an event
//! (one of beets' `ALL_EVENTS`) and a command, shell-split, each piece
//! formatted with Python's `str.format` and the event's arguments
//! (`{event}`, `{item.title}`, `{album}`, `{source}`, ...; `bytes` values,
//! such as paths, as text).

use crate::config::Config;
use crate::events::{ALL_EVENTS, Arg, Event, Listener};
use crate::model::{FieldType, Kind, Record};
use crate::plugins::PluginLog;
use crate::query::{Obj, PyVal};
use crate::template::Template;
use anyhow::{Result, bail};
use serde_yaml::Value as Y;
use std::os::unix::ffi::OsStrExt;

/// The plugin; its hooks listen on the event bus.
pub struct HookPlugin;

impl crate::plugins::Plugin for HookPlugin {
    fn name(&self) -> &'static str {
        "hook"
    }
}

struct Hooks {
    hooks: Vec<(String, String)>,
    format_item: String,
    format_album: String,
    log: PluginLog,
}

/// The plugin's `__init__`: each hook's event must be one of beets'
/// events; the hooks are registered as listeners.
pub fn load(cfg: &Config) -> Result<HookPlugin> {
    let mut hooks = vec![];
    if let Some(Y::Sequence(hs)) = cfg.get("hook.hooks") {
        for (i, h) in hs.iter().enumerate() {
            let get = |k: &str| h.as_mapping().and_then(|m| m.get(Y::String(k.into())));
            let event = match get("event") {
                Some(Y::String(e)) if ALL_EVENTS.contains(&e.as_str()) => e.clone(),
                Some(other) => {
                    let choices: Vec<String> = ALL_EVENTS
                        .iter()
                        .map(|e| crate::hooks::py_repr(e))
                        .collect();
                    bail!(
                        "hook.hooks#{i}.event: must be one of [{}], not {}",
                        choices.join(", "),
                        match other {
                            Y::String(s) => crate::hooks::py_repr(s),
                            o => serde_yaml::to_string(o)
                                .unwrap_or_default()
                                .trim()
                                .to_string(),
                        }
                    );
                }
                None => bail!("hook.hooks#{i}.event not found"),
            };
            let command = match get("command") {
                Some(Y::String(c)) => c.clone(),
                Some(Y::Null) | None => bail!("hook.hooks#{i}.command not found"),
                Some(other) => serde_yaml::to_string(other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
            hooks.push((event, command));
        }
    }
    let log = PluginLog::new("hook");
    // Listeners log at WARNING (each -v a level lower), as beets' do.
    log.set_base_level(crate::plugins::WARNING);
    crate::events::register(std::sync::Arc::new(Hooks {
        hooks,
        format_item: cfg.str("format_item"),
        format_album: cfg.str("format_album"),
        log,
    }));
    Ok(HookPlugin)
}

impl Listener for Hooks {
    fn wants(&self, event: &str) -> bool {
        self.hooks.iter().any(|(e, _)| e == event)
    }

    fn on(&self, ev: &Event) {
        for (event, command) in &self.hooks {
            if event == ev.name {
                self.run(ev, command);
            }
        }
    }
}

impl Hooks {
    /// `hook_function`.
    fn run(&self, ev: &Event, command: &str) {
        if command.is_empty() {
            self.log.error(&format!("invalid command \"{command}\""));
            return;
        }
        let pieces = match crate::plugins::duplicates::shlex_split(command).and_then(|ps| {
            ps.iter()
                .map(|p| self.format(p, ev))
                .collect::<Result<Vec<_>>>()
        }) {
            Ok(p) => p,
            Err(e) => {
                // beets fails here (an exception from the listener).
                crate::logging::emit(format!("error: {e:#}"));
                return;
            }
        };
        self.log.debug(&format!(
            "running command \"{}\" for event {}",
            pieces.join(" "),
            ev.name
        ));
        let Some((prog, args)) = pieces.split_first() else {
            return;
        };
        match std::process::Command::new(prog).args(args).status() {
            Ok(st) if st.success() => {}
            Ok(st) => {
                use std::os::unix::process::ExitStatusExt;
                let code = st.code().unwrap_or_else(|| -st.signal().unwrap_or(0));
                self.log
                    .error(&format!("hook for {} exited with status {code}", ev.name));
            }
            Err(e) => self.log.error(&format!(
                "hook for {} failed: {}",
                ev.name,
                python_oserror(&e, prog)
            )),
        }
    }

    /// Python's `str.format(piece, event=..., **kwargs)`, bytes as text.
    fn format(&self, piece: &str, ev: &Event) -> Result<String> {
        let mut out = String::new();
        let mut chars = piece.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push('}');
                }
                '}' => bail!("ValueError: Single '}}' encountered in format string"),
                '{' => {
                    let mut field = String::new();
                    let mut depth = 0;
                    loop {
                        match chars.next() {
                            Some('}') if depth == 0 => break,
                            Some('[') => {
                                depth += 1;
                                field.push('[');
                            }
                            Some(']') => {
                                depth -= 1;
                                field.push(']');
                            }
                            Some(c) => field.push(c),
                            None => bail!("ValueError: expected '}}' before end of string"),
                        }
                    }
                    out.push_str(&self.field(&field, ev)?);
                }
                c => out.push(c),
            }
        }
        Ok(out)
    }

    /// One `{field!conversion:spec}`.
    fn field(&self, field: &str, ev: &Event) -> Result<String> {
        let (field, spec) = match field.split_once(':') {
            Some((f, s)) => (f, s),
            None => (field, ""),
        };
        let (field, conv) = match field.rsplit_once('!') {
            Some((f, c)) => (f, Some(c)),
            None => (field, None),
        };
        // name, then .attr / [key] accessors.
        let name_end = field.find(['.', '[']).unwrap_or(field.len());
        let name = &field[..name_end];
        if name.is_empty() {
            bail!("IndexError: Replacement index 0 out of range for positional args tuple");
        }
        let mut accessors = vec![];
        let mut rest = &field[name_end..];
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix('.') {
                let end = r.find(['.', '[']).unwrap_or(r.len());
                accessors.push(&r[..end]);
                rest = &r[end..];
            } else if let Some(r) = rest.strip_prefix('[') {
                let end = r.find(']').unwrap_or(r.len());
                accessors.push(&r[..end]);
                rest = r.get(end + 1..).unwrap_or("");
            } else {
                break;
            }
        }
        let value = if name == "event" {
            Value::Str(ev.name.to_string())
        } else {
            match ev.arg(name) {
                Some(a) => Value::Arg(a),
                None => bail!("KeyError: {}", crate::hooks::py_repr(name)),
            }
        };
        let value = accessors
            .iter()
            .try_fold(value, |v, a| Self::attr(&v, a, ev))?;
        let s = match conv {
            Some("r") | Some("a") => self.repr(&value, ev),
            None | Some("s") => self.str(&value, ev),
            Some(c) => bail!("ValueError: Unknown conversion specifier {c}"),
        };
        Ok(apply_spec(&s, spec))
    }

    /// `getattr(value, attr)` / `value[attr]`.
    fn attr<'a>(v: &Value<'a>, attr: &str, ev: &Event<'a>) -> Result<Value<'a>> {
        match v {
            Value::Arg(Arg::Item(r) | Arg::Album(r) | Arg::Model(r)) => {
                let album = match (r.kind, ev.lib()) {
                    (Kind::Item, Some(lib)) => lib.get_album(r.int("album_id")).ok().flatten(),
                    _ => None,
                };
                let v = Obj {
                    rec: r,
                    album: album.as_ref(),
                    album_dir: None,
                    lib: ev.lib(),
                    album_counts: None,
                }
                .get(attr);
                match v {
                    // A fixed boolean field reads back from SQLite as an int.
                    Some(PyVal::Bool(b))
                        if crate::model::field_type(r.kind, attr) == Some(FieldType::Bool) =>
                    {
                        Ok(Value::Py(PyVal::Int(i64::from(b))))
                    }
                    Some(v) => Ok(Value::Py(v)),
                    None => bail!(
                        "AttributeError: '{}' object has no attribute {}",
                        if r.kind == Kind::Item {
                            "Item"
                        } else {
                            "Album"
                        },
                        crate::hooks::py_repr(attr)
                    ),
                }
            }
            Value::Arg(Arg::Task(t)) => match attr {
                "paths" => Ok(Value::Arg(Arg::Paths(&t.paths))),
                "singleton" => Ok(Value::Py(PyVal::Bool(t.singleton))),
                "album" => Ok(match &t.album {
                    Some(a) => Value::Arg(Arg::Album(a)),
                    None => Value::Py(PyVal::None),
                }),
                _ => bail!(
                    "AttributeError: 'ImportTask' object has no attribute {}",
                    crate::hooks::py_repr(attr)
                ),
            },
            Value::Arg(Arg::Lib(lib)) => match attr {
                "directory" => Ok(Value::Py(PyVal::Str(
                    lib.music_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                ))),
                _ => bail!(
                    "AttributeError: 'Library' object has no attribute {}",
                    crate::hooks::py_repr(attr)
                ),
            },
            _ => bail!(
                "AttributeError: object has no attribute {}",
                crate::hooks::py_repr(attr)
            ),
        }
    }

    /// `str(value)` (bytes decoded, as beets' `BytesToStrFormatter`).
    fn str(&self, v: &Value, ev: &Event) -> String {
        match v {
            Value::Str(s) => s.clone(),
            Value::Py(p) => match p {
                PyVal::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                p => py_str(p),
            },
            Value::Arg(a) => match a {
                Arg::Item(r) | Arg::Album(r) | Arg::Model(r) => self.model_str(r, ev),
                Arg::Path(p) => p.display().to_string(),
                Arg::Paths(ps) => format!(
                    "[{}]",
                    ps.iter()
                        .map(|p| bytes_repr(p.as_os_str().as_bytes()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Arg::Lib(_) => "<beets.library.library.Library object at 0x7f0000000000>".into(),
                Arg::Task(t) => format!(
                    "<beets.importer.tasks.{} object at 0x7f0000000000>",
                    if t.singleton {
                        "SingletonImportTask"
                    } else {
                        "ImportTask"
                    }
                ),
                Arg::Tags(tags) => format!(
                    "{{{}}}",
                    tags.iter()
                        .map(|(k, v)| format!(
                            "{}: {}",
                            crate::hooks::py_repr(k),
                            crate::hooks::py_repr(v)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Arg::Bool(b) => if *b { "True" } else { "False" }.into(),
                Arg::Opaque(class) => format!("<{class} object at 0x7f0000000000>"),
                Arg::Match(_) => "<beets.autotag.match.AlbumMatch object at 0x7f0000000000>".into(),
            },
        }
    }

    /// `repr(value)`.
    fn repr(&self, v: &Value, ev: &Event) -> String {
        match v {
            Value::Str(s) => crate::hooks::py_repr(s),
            Value::Py(PyVal::Str(s)) => crate::hooks::py_repr(s),
            Value::Py(PyVal::Bytes(b)) => bytes_repr(b),
            Value::Py(p) => py_str(p),
            Value::Arg(Arg::Path(p)) => bytes_repr(p.as_os_str().as_bytes()),
            Value::Arg(Arg::Item(r) | Arg::Album(r) | Arg::Model(r)) => {
                let class = if r.kind == Kind::Item {
                    "Item"
                } else {
                    "Album"
                };
                format!("{class}({})", self.model_str(r, ev))
            }
            v => self.str(v, ev),
        }
    }

    /// `str(model)`: `format_item` / `format_album`.
    fn model_str(&self, r: &Record, ev: &Event) -> String {
        let fmt = if r.kind == Kind::Item {
            &self.format_item
        } else {
            &self.format_album
        };
        crate::commands::format_record(&Template::parse(fmt), r, ev.lib())
    }
}

/// A formatted value: a kwarg, an attribute's value, or text.
enum Value<'a> {
    Arg(Arg<'a>),
    Py(PyVal),
    Str(String),
}

/// Python's `str()` of a model value.
fn py_str(v: &PyVal) -> String {
    match v {
        PyVal::None => "None".into(),
        PyVal::Bool(b) => if *b { "True" } else { "False" }.into(),
        PyVal::Int(i) => i.to_string(),
        PyVal::Float(f) => crate::hooks::py_float(*f),
        PyVal::Str(s) => s.clone(),
        PyVal::Bytes(b) => bytes_repr(b),
        PyVal::List(l) => format!(
            "[{}]",
            l.iter()
                .map(|s| crate::hooks::py_repr(s))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `repr()` of `bytes`.
fn bytes_repr(b: &[u8]) -> String {
    let q = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{q}");
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if c == q as u8 => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(q);
    out
}

/// A format spec's `[[fill]align][width]` (and `.Nf` for numbers).
fn apply_spec(s: &str, spec: &str) -> String {
    if spec.is_empty() {
        return s.to_string();
    }
    if let Some(p) = spec.strip_prefix('.').and_then(|p| p.strip_suffix('f'))
        && let (Ok(p), Ok(x)) = (p.parse::<usize>(), s.parse::<f64>())
    {
        return format!("{x:.p$}");
    }
    let chars: Vec<char> = spec.chars().collect();
    let is_align = |c: char| matches!(c, '<' | '>' | '^');
    let (fill, align, rest) = match chars.as_slice() {
        [f, a, rest @ ..] if is_align(*a) => (*f, *a, rest.iter().collect::<String>()),
        [a, rest @ ..] if is_align(*a) => (' ', *a, rest.iter().collect::<String>()),
        _ => (' ', '<', spec.to_string()),
    };
    let Ok(width) = rest.parse::<usize>() else {
        return s.to_string();
    };
    let len = s.chars().count();
    if len >= width {
        return s.to_string();
    }
    let pad = width - len;
    let fill_n = |n: usize| std::iter::repeat_n(fill, n).collect::<String>();
    match align {
        '>' => format!("{}{s}", fill_n(pad)),
        '^' => format!("{}{s}{}", fill_n(pad / 2), fill_n(pad - pad / 2)),
        _ => format!("{s}{}", fill_n(pad)),
    }
}

/// Python's `str(OSError)` for a failed `subprocess` start:
/// `[Errno 2] No such file or directory: 'cmd'`.
fn python_oserror(e: &std::io::Error, prog: &str) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let msg = std::io::Error::from_raw_os_error(n).to_string();
            let msg = msg.split(" (os error").next().unwrap_or(&msg).to_string();
            format!("[Errno {n}] {msg}: {}", crate::hooks::py_repr(prog))
        }
        None => e.to_string(),
    }
}

#[cfg(test)]
#[path = "hook_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "hook",
        order: 8,
        defaults: r#"hook:
    hooks: []
"#,
        config_keys: &["hook"],
        item_types: &[],
        album_types: &[],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(load(cfg)?))
}
