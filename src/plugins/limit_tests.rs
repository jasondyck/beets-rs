use super::*;
use crate::library::Library;
use crate::model::{Item, Value};
use crate::query::{Context, Query, set_context};

fn lib() -> Library {
    let lib = Library::in_memory().unwrap();
    for n in 1..=10 {
        let mut it = Item::new_item();
        it.set_int("track", n);
        it.set_str("title", format!("t{n}"));
        it.set("path", Value::Path(format!("/m/{n}.mp3").into()));
        lib.add(&mut it).unwrap();
    }
    lib
}

fn titles(lib: &Library, parts: &[&str]) -> Vec<String> {
    let q = Query::parse(crate::model::Kind::Item, parts).unwrap();
    lib.items(&q).unwrap().iter().map(|i| i.str("title").to_string()).collect()
}

#[test]
fn prefix_and_command_limit() {
    set_context(Context {
        limit: Some("<".into()),
        ..Context::default()
    });
    let lib = lib();
    assert_eq!(titles(&lib, &["<5"]).len(), 5);
    // The prefix must come after the filter to count only filtered rows.
    assert_eq!(titles(&lib, &["track:6..", "<5"]).len(), 5);
    assert_eq!(titles(&lib, &["<5", "track:6.."]).len(), 0);
}
