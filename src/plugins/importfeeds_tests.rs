use super::*;
use crate::config::Config;
use crate::library::Library;
use crate::model::{Item, Value};

#[test]
fn m3u_multi_uses_the_album_name() {
    let d = tempfile::tempdir().unwrap();
    let feeds = d.path().join("feeds");
    let mut c = Config::defaults();
    c.set("importfeeds.dir", feeds.to_string_lossy().into());
    c.set("importfeeds.formats", serde_yaml::from_str("[m3u_multi]").unwrap());
    let p = ImportFeeds::new(&c);
    let lib = Library::in_memory().unwrap();
    let mut item = Item::new_item();
    item.set("path", Value::Path(d.path().join("music/a.mp3")));
    lib.add(&mut item).unwrap();
    p.record("album/name", &[item]).unwrap();
    let file = std::fs::read_dir(&feeds).unwrap().next().unwrap().unwrap();
    let name = file.file_name().to_string_lossy().into_owned();
    assert!(name.ends_with("_album_name.m3u"), "{name}");
    let text = std::fs::read_to_string(file.path()).unwrap().replace('\\', "/");
    assert!(text.contains("music/a.mp3"), "{text}");
}

#[test]
fn relative_and_absolute_paths() {
    let d = tempfile::tempdir().unwrap();
    let feeds = d.path().join("feeds");
    let mut c = Config::defaults();
    c.set("importfeeds.dir", feeds.to_string_lossy().into());
    c.set("importfeeds.formats", serde_yaml::from_str("[m3u]").unwrap());
    let p = ImportFeeds::new(&c);
    let mut item = Item::new_item();
    item.set("path", Value::Path(feeds.join("sub/a.mp3")));
    p.record("x", std::slice::from_ref(&item)).unwrap();
    let text = std::fs::read_to_string(feeds.join("imported.m3u")).unwrap();
    assert_eq!(text.trim().replace('\\', "/"), "sub/a.mp3");
}

#[test]
fn m3u_session_and_link() {
    let d = tempfile::tempdir().unwrap();
    let feeds = d.path().join("feeds");
    let music = d.path().join("music");
    std::fs::create_dir_all(&music).unwrap();
    let file = music.join("a.mp3");
    std::fs::write(&file, b"x").unwrap();
    let mut c = Config::defaults();
    c.set("importfeeds.dir", feeds.to_string_lossy().into());
    c.set("importfeeds.m3u_name", "session.m3u".into());
    c.set(
        "importfeeds.formats",
        serde_yaml::from_str("[m3u_session, link]").unwrap(),
    );
    let p = ImportFeeds::new(&c);
    p.begin();
    let mut item = Item::new_item();
    item.set("path", Value::Path(file.clone()));
    p.record("album name", std::slice::from_ref(&item)).unwrap();
    let names: Vec<String> = std::fs::read_dir(&feeds)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("session_") && n.ends_with(".m3u")),
        "{names:?}"
    );
    // `link` creates a symlink named after the file.
    assert!(feeds.join("a.mp3").is_symlink(), "{names:?}");
}

/// The listener side: `import_begin` (which carries only the session) and
/// `item_imported` write the session playlist.
#[test]
fn import_events_write_the_session_playlist() {
    let d = tempfile::tempdir().unwrap();
    let feeds = d.path().join("feeds");
    let music = d.path().join("music");
    std::fs::create_dir_all(&music).unwrap();
    let mut c = Config::defaults();
    c.set("directory", music.to_string_lossy().into());
    c.set("importfeeds.dir", feeds.to_string_lossy().into());
    c.set(
        "importfeeds.formats",
        serde_yaml::from_str("[m3u_session]").unwrap(),
    );
    let lib = Library::open_with(&d.path().join("l.db"), Some(&music), false).unwrap();
    crate::events::register(std::sync::Arc::new(ImportFeeds::new(&c)));
    crate::events::send(
        "import_begin",
        &[("session", crate::events::Arg::Opaque("<session>"))],
    );
    let mut item = Item::new_item();
    item.set("path", Value::Path(music.join("a.mp3")));
    lib.add(&mut item).unwrap();
    crate::events::send(
        "item_imported",
        &[
            ("lib", crate::events::Arg::Lib(&lib)),
            ("item", crate::events::Arg::Item(&item)),
        ],
    );
    let names: Vec<String> = std::fs::read_dir(&feeds)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with("imported_") && names[0].ends_with(".m3u"));
    assert!(std::fs::read_to_string(feeds.join(&names[0]))
        .unwrap()
        .contains("a.mp3"));
}
