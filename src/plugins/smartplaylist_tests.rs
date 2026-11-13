//! beets' `test/plugins/test_smartplaylist.py` (on a real library where
//! beets mocks items), and the plugin's Python-compatible helpers.
//!
//! Not ported: `test_matches` and `test_db_changes`, which test the
//! `database_change` listener behind `auto`; in beets 2.14 it never
//! fires (see the module docs), and it isn't wired up here.

use super::*;
use crate::model::Value;
use tempfile::TempDir;

struct Env {
    dir: TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new(playlists: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let mut cfg = Config::defaults();
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str("[smartplaylist]").unwrap());
        cfg.set(
            "smartplaylist.playlists",
            serde_yaml::from_str(playlists).unwrap(),
        );
        cfg.set(
            "smartplaylist.playlist_dir",
            Y::String(dir.path().join("playlists").display().to_string()),
        );
        crate::formatted::configure(&cfg).unwrap();
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env { dir, cfg, lib }
    }

    fn set(&mut self, key: &str, yaml: &str) {
        self.cfg.set(
            &format!("smartplaylist.{key}"),
            serde_yaml::from_str(yaml).unwrap(),
        );
    }

    fn add(&self, path: &str, pairs: &[(&str, &str)]) -> Item {
        let mut it = Item::new_item();
        it.set_path(path);
        for (k, v) in pairs {
            it.set_parsed(k, v).unwrap();
        }
        self.lib.add(&mut it).unwrap();
        it
    }

    /// `beet splupdate`, its result and log.
    fn run(&self, opts: &CmdOpts) -> (Result<()>, Vec<String>) {
        crate::logging::capture(|| command(&self.cfg, &self.lib, opts))
    }

    fn names(names: &[&str]) -> CmdOpts {
        CmdOpts {
            names: names.iter().map(std::string::ToString::to_string).collect(),
            ..Default::default()
        }
    }

    fn playlist(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.path().join("playlists").join(name)).ok()
    }
}

#[test]
fn build_queries_test() {
    let env = Env::new(
        "[{name: foo, query: 'FOO foo'},
          {name: bar, album_query: ['BAR bar1', 'BAR bar2']},
          {name: baz, query: 'BAZ baz', album_query: 'BAZ baz'},
          {query: nameless},
          {name: bad, query: 'year:1..x'}]",
    );
    let log = PluginLog::new("smartplaylist");
    let (pls, logged) = crate::logging::capture(|| build_queries(&env.cfg, &log));
    let shape = |q: &PlQuery| match q {
        PlQuery::None => "none".to_string(),
        PlQuery::One(_) => "one".to_string(),
        PlQuery::Many(qs) => format!("many({})", qs.len()),
    };
    let got: Vec<(String, String, String)> = pls
        .iter()
        .map(|p| (p.name.clone(), shape(&p.query), shape(&p.album_query)))
        .collect();
    let want = [
        ("foo", "one", "none"),
        ("bar", "none", "many(2)"),
        ("baz", "one", "one"),
    ]
    .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()));
    assert_eq!(got, want);
    assert_eq!(
        logged,
        [
            "smartplaylist: playlist configuration is missing name",
            "smartplaylist: invalid query in playlist bad: 'x' is not an int or a float",
        ]
    );
}

#[test]
fn build_queries_with_sorts() {
    let env = Env::new(
        "[{name: no_sort, query: foo},
          {name: one_sort, query: 'foo year+'},
          {name: only_empty_sorts, query: [foo, bar]},
          {name: one_non_empty_sort, query: ['foo year+', bar]},
          {name: multiple_sorts, query: ['foo year+', 'bar genres-']},
          {name: mixed, query: ['foo year+', bar, 'baz genres+ id-']}]",
    );
    let pls = build_queries(&env.cfg, &PluginLog::new("smartplaylist"));
    let sorts = |p: &Playlist| -> Vec<Vec<(String, bool)>> {
        p.query
            .queries()
            .iter()
            .map(|q| {
                q.sort
                    .0
                    .iter()
                    .map(|s| (s.field.clone(), s.ascending))
                    .collect()
            })
            .collect()
    };
    let s = |f: &str, a: bool| (f.to_string(), a);
    assert_eq!(sorts(&pls[0]), [vec![]]);
    assert_eq!(sorts(&pls[1]), [vec![s("year", true)]]);
    assert_eq!(sorts(&pls[2]), [vec![], vec![]]);
    assert_eq!(sorts(&pls[3]), [vec![s("year", true)], vec![]]);
    assert_eq!(
        sorts(&pls[4]),
        [vec![s("year", true)], vec![s("genres", false)]]
    );
    assert_eq!(
        sorts(&pls[5]),
        [
            vec![s("year", true)],
            vec![],
            vec![s("genres", true), s("id", false)]
        ]
    );
}

#[test]
fn playlist_update() {
    let env = Env::new("[{name: '$title-my<playlist>.m3u', query: ''}]");
    env.add("/tagada.mp3", &[("title", "ta:ga:da")]);
    let (r, _) = env.run(&CmdOpts::default());
    r.unwrap();
    assert_eq!(
        env.playlist("ta_ga_da-my_playlist_.m3u").unwrap(),
        b"/tagada.mp3\n"
    );
}

#[test]
fn playlist_update_output_extm3u() {
    let mut env = Env::new("[{name: '$title-my<playlist>.m3u', query: ''}]");
    env.set("output", "extm3u");
    env.set("prefix", "http://beets:8337/files");
    let mut it = env.add(
        "/tagada.mp3",
        &[("artist", "fake artist"), ("title", "ta:ga:da")],
    );
    it.set("length", Value::Float(300.123));
    env.lib.store(&it).unwrap();
    env.run(&CmdOpts::default()).0.unwrap();
    assert_eq!(
        env.playlist("ta_ga_da-my_playlist_.m3u").unwrap(),
        b"#EXTM3U\n#EXTINF:300,fake artist - ta:ga:da\nhttp://beets:8337/files/tagada.mp3\n"
    );
}

#[test]
fn playlist_update_output_extm3u_fields() {
    let mut env = Env::new("[{name: 'my.m3u', query: ''}]");
    env.set("output", "extm3u");
    env.set("fields", "[id, genres]");
    let mut it = env.add(
        "/tagada.mp3",
        &[
            ("artist", "Fake Artist"),
            ("title", "fake Title"),
            ("genres", "Rock; Pop"),
        ],
    );
    it.set("length", Value::Float(300.123));
    env.lib.store(&it).unwrap();
    env.run(&CmdOpts::default()).0.unwrap();
    assert_eq!(
        env.playlist("my.m3u").unwrap(),
        b"#EXTM3U\n#EXTINF:300 id=\"1\" genres=\"Rock%3B%20Pop\",Fake Artist - fake Title\n/tagada.mp3\n"
    );
}

#[test]
fn get_playlist_items_test() {
    let env = Env::new("[]");
    for n in 1..=3 {
        env.add(&format!("/item{n}.mp3"), &[]);
    }
    let pl = Playlist {
        name: "x".into(),
        query: PlQuery::Many(vec![
            parse_query_string(Kind::Item, "path::item id-").unwrap(),
            parse_query_string(Kind::Item, "path::item3").unwrap(),
        ]),
        album_query: PlQuery::None,
    };
    let ids: Vec<i64> = playlist_items(&env.lib, &pl)
        .unwrap()
        .iter()
        .map(|i| i.id.unwrap())
        .collect();
    assert_eq!(ids, [3, 2, 1]);
}

#[test]
fn get_item_uri() {
    let mut env = Env::new("[]");
    env.set("prefix", "http://beets:8337/files");
    let it = env.add(
        "/imported/path/with/dont/move/tagada.mp3",
        &[("artist", "fake artist"), ("title", "fake title")],
    );
    let uri = |env: &Env| {
        let s = Settings::new(&env.cfg, &CmdOpts::default());
        String::from_utf8(item_uri(&env.cfg, &env.lib, &s, &it).unwrap()).unwrap()
    };
    assert_eq!(
        uri(&env),
        "http://beets:8337/files/imported/path/with/dont/move/tagada.mp3"
    );
    env.set("dest_regen", "yes");
    let dest = format!(
        "http://beets:8337/files{}/Non-Album/fake artist/fake title.mp3",
        env.dir.path().join("music").display()
    );
    assert_eq!(uri(&env), dest);
    env.set("uri_format", "'http://beets:8337/item/$id/file'");
    assert_eq!(uri(&env), "http://beets:8337/item/1/file");
}

/// `SmartPlaylistCLITest`'s setup.
fn cli_env() -> (Env, Item) {
    let env = Env::new("[{name: my_playlist.m3u, query: 'the title'}, {name: all.m3u, query: ''}]");
    let it = env.add(
        "/tagada.mp3",
        &[("artist", "the ärtist"), ("title", "the title")],
    );
    (env, it)
}

#[test]
fn splupdate() {
    let (env, it) = cli_env();
    let path = format!("{}\n", it.path().display()).into_bytes();
    assert!(env.run(&Env::names(&["tagada"])).0.is_err());
    env.run(&Env::names(&["my_playlist"])).0.unwrap();
    assert_eq!(env.playlist("my_playlist.m3u").unwrap(), path);
    assert_eq!(env.playlist("all.m3u"), None);
    env.run(&Env::names(&["my_playlist.m3u"])).0.unwrap();
    assert_eq!(env.playlist("my_playlist.m3u").unwrap(), path);
    env.run(&CmdOpts::default()).0.unwrap();
    for name in ["my_playlist.m3u", "all.m3u"] {
        assert_eq!(env.playlist(name).unwrap(), path);
    }
}

#[test]
fn splupdate_unknown_playlist_error_is_sorted_and_quoted() {
    let env = Env::new(
        "[{name: z last.m3u, query: x}, {name: \"rock'n roll.m3u\", query: x},
          {name: a one.m3u, query: x}]",
    );
    let err = env.run(&Env::names(&["tagada"])).0.unwrap_err();
    assert_eq!(
        err.to_string(),
        "No playlist matching any of 'a one.m3u' 'rock'\"'\"'n roll.m3u' 'z last.m3u' found"
    );
}

#[test]
fn splupdate_log_output() {
    let (env, _) = cli_env();
    let (r, log) = env.run(&Env::names(&["my_playlist"]));
    r.unwrap();
    assert_eq!(
        log,
        [
            "smartplaylist: Updating 1 smart playlists...",
            "smartplaylist: Creating playlist my_playlist.m3u: 1 tracks.",
            "smartplaylist: 1 playlists updated",
        ]
    );
}

#[test]
fn splupdate_verbose_log_output() {
    let (env, _) = cli_env();
    // `-v`: the plugin's log at DEBUG (without touching the global level).
    let log = PluginLog::new("smartplaylist");
    log.set_base_level(crate::plugins::DEBUG);
    let pls = build_queries(&env.cfg, &log);
    let s = Settings::new(&env.cfg, &CmdOpts::default());
    let (r, log) =
        crate::logging::capture(|| update_playlists(&env.cfg, &env.lib, &s, &pls[..1], &log));
    r.unwrap();
    assert!(
        log.contains(&"smartplaylist: the ärtist - the title".to_string()),
        "{log:?}"
    );
}

#[test]
fn splupdate_pretend_log_output() {
    let (env, _) = cli_env();
    let (r, log) = env.run(&CmdOpts {
        pretend: true,
        ..Env::names(&["my_playlist"])
    });
    r.unwrap();
    assert_eq!(
        log.last().unwrap(),
        "smartplaylist: 1 playlists would be updated"
    );
    assert_eq!(env.playlist("my_playlist.m3u"), None);
}

// ------------------------------------------------------------ beyond beets'

#[test]
fn python_helpers() {
    assert_eq!(shell_quote(""), "''");
    assert_eq!(shell_quote("a.m3u"), "a.m3u");
    assert_eq!(shell_quote("$x.m3u"), "'$x.m3u'");
    assert_eq!(py_bytes_repr(b"/a/\xc3\xa9'x"), "b\"/a/\\xc3\\xa9'x\"");
    assert_eq!(py_bytes_repr(b"a\"b'c\\"), "b'a\"b\\'c\\\\'");
    assert_eq!(
        quote("/a b/é?#%&+~".as_bytes(), "/"),
        "/a%20b/%C3%A9%3F%23%25%26%2B~"
    );
    assert_eq!(quote(b"Rock; Pop", "/:"), "Rock%3B%20Pop");
    assert_eq!(
        relpath(Path::new("/a/b/c.mp3"), Path::new("/a/x")),
        Path::new("../b/c.mp3")
    );
    assert_eq!(py_str(&PyVal::None), "None");
    assert_eq!(py_str(&PyVal::Float(1.0)), "1.0");
}

#[test]
fn a_missing_field_stops_the_file_where_beets_does() {
    let mut env = Env::new("[{name: all.m3u, query: 'id+'}]");
    env.set("output", "extm3u");
    env.set("fields", "[rating]");
    env.add("/a.mp3", &[("title", "A"), ("rating", "5")]);
    env.add("/b.mp3", &[("title", "B")]);
    let err = env.run(&CmdOpts::default()).0.unwrap_err();
    assert_eq!(err.to_string(), "KeyError: 'rating'");
    assert_eq!(
        env.playlist("all.m3u").unwrap(),
        b"#EXTM3U\n#EXTINF:0 rating=\"5\", - A\n/a.mp3\n"
    );
}

#[test]
fn urlencode_and_relative_to() {
    let mut env = Env::new("[{name: all.m3u, query: ''}]");
    env.set("urlencode", "yes");
    env.set("relative_to", "'/music'");
    env.add("/music/A b/é?.mp3", &[]);
    env.add("//net/x.mp3", &[]);
    env.run(&CmdOpts::default()).0.unwrap();
    assert_eq!(
        String::from_utf8(env.playlist("all.m3u").unwrap()).unwrap(),
        "A%20b/%C3%A9%3F.mp3\n../net/x.mp3\n"
    );
}
