//! beets' `beetsplug/_utils/vfs.py`: a filesystem-like tree of a library's
//! items, which BPD's path-addressed commands (`lsinfo`, `listall`, `add`)
//! browse.

use crate::library::Library;
use crate::model::Item;
use std::collections::BTreeMap;

/// A directory: file names → item ids, directory names → child nodes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Node {
    pub files: BTreeMap<String, i64>,
    pub dirs: BTreeMap<String, Node>,
}

/// `util.components` of a "/"-separated path: its non-empty components.
pub fn components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect()
}

/// `item_components`: the item's path relative to the library directory,
/// as components.
pub fn item_components(lib: &Library, item: &Item) -> Vec<String> {
    use std::os::unix::ffi::OsStrExt;
    let path = item.path();
    let stored = crate::library::to_db_path(path.as_os_str().as_bytes(), lib.music_dir());
    components(&String::from_utf8_lossy(&stored))
}

/// `item_path`: the item's location in the tree as a "/"-separated path.
pub fn item_path(lib: &Library, item: &Item) -> String {
    item_components(lib, item).join("/")
}

fn insert(node: &mut Node, path: &[String], itemid: i64) {
    match path {
        [] => {}
        [file] => {
            node.files.insert(file.clone(), itemid);
        }
        [dir, rest @ ..] => {
            let child = node.dirs.entry(dir.clone()).or_default();
            insert(child, rest, itemid);
        }
    }
}

/// `libtree`: the tree of every item in the library.
pub fn libtree(lib: &Library) -> anyhow::Result<Node> {
    let mut root = Node::default();
    let items = lib.items(&crate::query::Query::parse::<&str>(
        crate::model::Kind::Item,
        &[],
    )?)?;
    for item in items {
        let Some(id) = item.id else { continue };
        let path = item_components(lib, &item);
        insert(&mut root, &path, id);
    }
    Ok(root)
}

/// `_path_join`: smash two BPD paths together.
pub fn path_join(p1: &str, p2: &str) -> String {
    let out = format!("{p1}/{p2}");
    out.replace("//", "/").replace("//", "/")
}
