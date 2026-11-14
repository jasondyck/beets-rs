//! beets' plugin events (`beets.events`, `plugins.send`): the core sends
//! each event by name with beets' keyword arguments, and listeners
//! (plugins, such as hook, importadded and permissions) registered when
//! the plugins load get them in registration order.
//!
//! As in beets, the registry is global: convert's parallel transcoders send
//! `write`/`after_convert` from worker threads and reach the listeners the
//! plugins registered. Tests keep a registry per test thread (so parallel
//! tests can't see each other's listeners); a thread spawned for a test
//! inherits it with [`inherit`]/[`install`].

use crate::library::Library;
use crate::model::{Album, Item, Record};
#[cfg(test)]
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// beets' `ALL_EVENTS`, in its order (hook's choices).
pub const ALL_EVENTS: &[&str] = &[
    "after_write",
    "before_item_moved",
    "item_copied",
    "item_hardlinked",
    "item_linked",
    "item_moved",
    "item_reflinked",
    "before_choose_candidate",
    "import_task_before_choice",
    "import_task_created",
    "import_task_apply",
    "import_task_choice",
    "import_task_files",
    "import_task_start",
    "import",
    "album_imported",
    "album_removed",
    "art_set",
    "albuminfo_received",
    "trackinfo_received",
    "album_matched",
    "cli_exit",
    "library_opened",
    "database_change",
    "import_begin",
    "item_imported",
    "item_removed",
    "write",
    "mb_album_extract",
    "mb_track_extract",
    "pluginload",
    "smartplaylist_update",
    "after_convert",
    "alternatives.item_updated",
];

/// What an import task event carries (beets' `ImportTask`, as far as
/// listeners here use it).
#[derive(Debug, Default, Clone)]
pub struct TaskInfo {
    /// The task's items (`task.items`; one for a singleton).
    pub items: Vec<Item>,
    /// `task.paths`.
    pub paths: Vec<PathBuf>,
    /// A singleton task.
    pub singleton: bool,
    /// The task's album (`task.album`), once it is in the library.
    pub album: Option<Album>,
    /// The files stay where they are (no copy, move or link).
    pub in_place: bool,
    /// Ids of the new items that replaced library items (reimports).
    pub reimported_item_ids: Vec<i64>,
    /// Directories of the albums the task replaced.
    pub replaced_album_paths: Vec<PathBuf>,
    /// The task was skipped at the match prompt.
    pub skip: bool,
}

/// One keyword argument of an event.
#[derive(Clone, Copy)]
pub enum Arg<'a> {
    Lib(&'a Library),
    Item(&'a Item),
    Album(&'a Album),
    /// `database_change`'s `model`: an item or an album.
    Model(&'a Record),
    /// A path (`bytes` in beets).
    Path(&'a Path),
    /// The import command's paths (a list of `bytes`).
    Paths(&'a [PathBuf]),
    Task(&'a TaskInfo),
    /// The tags of a `write` event (`{name: value}` as Python's `str()`).
    Tags(&'a [(String, String)]),
    /// A boolean argument (`after_convert`'s `keepnew`).
    Bool(bool),
    /// `album_matched`'s `match`: mutable, as beets' listeners adjust it
    /// (mbpseudo switches its data source).
    Match(&'a std::cell::RefCell<crate::autotag::AlbumMatch>),
    /// An object listeners here only see as Python would print it
    /// (`<beets.importer.session.ImportSession object at 0x...>`).
    Opaque(&'static str),
}

/// An event being sent.
pub struct Event<'a> {
    pub name: &'static str,
    pub args: &'a [(&'static str, Arg<'a>)],
}

impl<'a> Event<'a> {
    pub fn arg(&self, key: &str) -> Option<Arg<'a>> {
        self.args.iter().find(|(k, _)| *k == key).map(|(_, a)| *a)
    }

    pub fn item(&self) -> Option<&'a Item> {
        match self.arg("item")? {
            Arg::Item(i) => Some(i),
            _ => None,
        }
    }

    pub fn album(&self) -> Option<&'a Album> {
        match self.arg("album")? {
            Arg::Album(a) => Some(a),
            _ => None,
        }
    }

    pub fn lib(&self) -> Option<&'a Library> {
        match self.arg("lib")? {
            Arg::Lib(l) => Some(l),
            _ => None,
        }
    }

    pub fn path(&self, key: &str) -> Option<&'a Path> {
        match self.arg(key)? {
            Arg::Path(p) => Some(p),
            _ => None,
        }
    }

    pub fn match_album(&self) -> Option<&'a std::cell::RefCell<crate::autotag::AlbumMatch>> {
        match self.arg("match")? {
            Arg::Match(m) => Some(m),
            _ => None,
        }
    }

    pub fn task(&self) -> Option<&'a TaskInfo> {
        match self.arg("task")? {
            Arg::Task(t) => Some(t),
            _ => None,
        }
    }
}

/// A plugin's listener. As in beets' global registry, the same listener
/// serves every thread (convert's parallel transcoders send `write` and
/// `after_convert` from worker threads).
pub trait Listener: Send + Sync {
    /// Whether it listens for this event.
    fn wants(&self, event: &str) -> bool;
    fn on(&self, event: &Event);
}

type Listeners = Vec<Arc<dyn Listener>>;

// As beets, one registry for the process; the listeners keep their state
// behind atomics and locks. Tests run in parallel, so there each test
// thread has its own (see `inherit`).
#[cfg(not(test))]
static LISTENERS: std::sync::RwLock<Listeners> = std::sync::RwLock::new(Vec::new());
#[cfg(test)]
thread_local! {
    static LISTENERS: RefCell<Listeners> = const { RefCell::new(Vec::new()) };
}

/// Register a listener (a plugin, when it loads).
pub fn register(l: Arc<dyn Listener>) {
    #[cfg(not(test))]
    LISTENERS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(l);
    #[cfg(test)]
    LISTENERS.with(|v| v.borrow_mut().push(l));
}

/// The current registry (tests: this thread's), for a worker thread that
/// must send events to the listeners its spawner knows.
pub struct Inherited(#[cfg(test)] Arc<Listeners>);

/// Capture the listeners for a thread spawned from here. In a normal build
/// the registry is shared anyway, so this is a marker.
pub fn inherit() -> Inherited {
    #[cfg(test)]
    {
        Inherited(LISTENERS.with(|v| Arc::new(v.borrow().clone())))
    }
    #[cfg(not(test))]
    {
        Inherited()
    }
}

/// Give a spawned thread the captured listeners (test builds only: the
/// registry is global otherwise).
pub fn install(_inherited: &Inherited) {
    #[cfg(test)]
    LISTENERS.with(|v| *v.borrow_mut() = (*_inherited.0).clone());
}

fn listeners() -> Listeners {
    #[cfg(not(test))]
    return LISTENERS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    #[cfg(test)]
    LISTENERS.with(|v| v.borrow().clone())
}

/// Whether anything listens for `event` (to skip building its arguments).
pub fn listening(event: &str) -> bool {
    listeners().iter().any(|l| l.wants(event))
}

/// beets' `plugins.send(event, **kwargs)`.
pub fn send(name: &'static str, args: &[(&'static str, Arg)]) {
    let ls: Listeners = listeners().into_iter().filter(|l| l.wants(name)).collect();
    if ls.is_empty() {
        return;
    }
    let ev = Event { name, args };
    for l in ls {
        l.on(&ev);
    }
}

/// The item events of a file operation: `before_item_moved` before a
/// move, `item_copied` (etc.) after the others.
pub fn file_op_event(op: crate::fileops::FileOp, before: bool) -> Option<&'static str> {
    use crate::fileops::FileOp;
    match (op, before) {
        (FileOp::Move, true) => Some("before_item_moved"),
        (FileOp::Move, false) => Some("item_moved"),
        (FileOp::Copy, false) => Some("item_copied"),
        (FileOp::Link, false) => Some("item_linked"),
        (FileOp::Hardlink, false) => Some("item_hardlinked"),
        (FileOp::Reflink | FileOp::ReflinkAuto, false) => Some("item_reflinked"),
        _ => None,
    }
}

/// Send a file operation's event for `item` going from `source` to
/// `destination`.
pub fn send_file_op(
    op: crate::fileops::FileOp,
    before: bool,
    item: &Item,
    source: &Path,
    dest: &Path,
) {
    if let Some(name) = file_op_event(op, before)
        && listening(name)
    {
        send(
            name,
            &[
                ("item", Arg::Item(item)),
                ("source", Arg::Path(source)),
                ("destination", Arg::Path(dest)),
            ],
        );
    }
}
