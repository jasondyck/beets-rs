//! beets' `test/plugins/test_export.py`, plus the format options.

use super::*;
use crate::model::{Item, Kind, Value};
use crate::query::Query;
use serde_yaml::Value as Y;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    cfg: Config,
    lib: Library,
}

/// A library holding one album of real MP3s (needs ffmpeg; None without
/// it).
fn env() -> Option<Env> {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    let album_dir = music.join("Artist/Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    let mut items = vec![];
    for (n, title) in ["Alpha", "Beta"].iter().enumerate() {
        let path = album_dir.join(format!("{}.mp3", n + 1));
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=0.5"])
            .args(["-metadata", "artist=xartist", "-metadata", "album=xalbum"])
            .args(["-metadata", &format!("title={title}")])
            .args(["-metadata", &format!("track={}", n + 1)])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return None;
        }
        items.push(crate::media::read(&path).unwrap());
    }
    let mut cfg = Config::defaults();
    cfg.set("directory", Y::String(music.display().to_string()));
    cfg.set("plugins", serde_yaml::from_str("[export]").unwrap());
    crate::formatted::configure(&cfg).unwrap();
    let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
    lib.add_album(&mut items).unwrap();
    Some(Env {
        _dir: dir,
        cfg,
        lib,
    })
}

impl Env {
    fn run(&self, opts: CmdOpts) -> String {
        let mut out = vec![];
        run(&self.cfg, &self.lib, &opts, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn keys_opts(&self, format: &str) -> CmdOpts {
        CmdOpts {
            format: Some(format.into()),
            included_keys: vec!["title,album".into()],
            query: vec!["xartist".into()],
            ..Default::default()
        }
    }
}

fn items(lib: &Library) -> Vec<Item> {
    lib.items(&Query::parse(Kind::Item, &["artist:xartist"][..]).unwrap())
        .unwrap()
}

#[test]
fn json_output() {
    let Some(env) = env() else { return };
    let out = env.run(env.keys_opts("json"));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let first = &v[0];
    assert_eq!(first["title"], "Alpha");
    assert_eq!(first["album"], "xalbum");
    assert_eq!(v.as_array().unwrap().len(), 2);
    // The default layout: indent 4, sorted keys, a trailing newline.
    assert!(out.starts_with("[\n    {\n        \"album\": \"xalbum\""));
    assert!(out.ends_with("}\n]\n"));
}

#[test]
fn jsonlines_output() {
    let Some(env) = env() else { return };
    let out = env.run(env.keys_opts("jsonlines"));
    assert_eq!(out.lines().count(), 2);
    let v: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(v["title"], "Alpha");
    assert_eq!(v["album"], "xalbum");
}

#[test]
fn csv_output() {
    let Some(env) = env() else { return };
    let out = env.run(env.keys_opts("csv"));
    let lines: Vec<&str> = out.split("\r\n").collect();
    let head: Vec<&str> = lines[0].split(',').collect();
    let vals: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(head.len(), 2);
    for (i, col) in head.iter().enumerate() {
        match *col {
            "title" => assert_eq!(vals[i], "Alpha"),
            "album" => assert_eq!(vals[i], "xalbum"),
            other => panic!("unexpected column {other}"),
        }
    }
    assert!(out.ends_with("\r\n"));
}

#[test]
fn xml_output() {
    let Some(env) = env() else { return };
    let out = env.run(env.keys_opts("xml"));
    assert!(out.starts_with("<library><tracks><track>"));
    assert!(out.contains("<album>xalbum</album>"));
    assert!(out.contains("<title>Beta</title>"));
    assert!(out.ends_with("</track></tracks></library>"));
}

#[test]
fn library_fields() {
    let Some(env) = env() else { return };
    let opts = CmdOpts {
        library: true,
        format: Some("json".into()),
        included_keys: vec!["title,album,path,id".into()],
        query: vec!["xartist".into()],
        ..Default::default()
    };
    let out = env.run(opts);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v[0]["title"], "Alpha");
    assert_eq!(v[0]["album"], "xalbum");
    assert!(v[0]["path"].as_str().unwrap().ends_with(".mp3"));
    assert!(!v[0]["id"].as_str().unwrap().is_empty());
}

#[test]
fn albums_and_limits() {
    let Some(env) = env() else { return };
    let opts = CmdOpts {
        album: true,
        format: Some("json".into()),
        included_keys: vec!["album".into()],
        limit: Some(1),
        query: vec!["xalbum".into()],
        ..Default::default()
    };
    let out = env.run(opts);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["album"], "xalbum");
}

#[test]
fn output_file_and_append() {
    let Some(env) = env() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.json");
    let mut opts = env.keys_opts("json");
    opts.output = Some(path.display().to_string());
    command(&env.cfg, &env.lib, opts.clone()).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.starts_with('['));
    opts.append = true;
    opts.format = Some("jsonlines".into());
    command(&env.cfg, &env.lib, opts.clone()).unwrap();
    let all = std::fs::read_to_string(&path).unwrap();
    assert_eq!(all, format!("{first}{}", env.run(opts)));
}

#[test]
fn default_format_checks_out() {
    let mut cfg = Config::defaults();
    cfg.set("plugins", serde_yaml::from_str("[export]").unwrap());
    assert!(ExportPlugin::new(&cfg).is_ok());
    cfg.set("export.default_format", Y::String("xspf".into()));
    let e = ExportPlugin::new(&cfg).unwrap_err().to_string();
    assert_eq!(
        e,
        "export.default_format: must be one of ['json', 'jsonlines', 'csv', 'xml'], not 'xspf'"
    );
}

#[test]
fn json_formatting_options() {
    let Some(env) = env() else { return };
    let mut env = env;
    env.cfg.set("export.json.formatting.indent", Y::Null);
    env.cfg.set(
        "export.json.formatting.separators",
        serde_yaml::from_str("[',', ':']").unwrap(),
    );
    env.cfg
        .set("export.json.formatting.sort_keys", Y::Bool(false));
    let out = env.run(env.keys_opts("json"));
    // With sort_keys off, the tag fields keep the `-i` order.
    assert_eq!(
        out,
        "[{\"title\":\"Alpha\",\"album\":\"xalbum\"},{\"title\":\"Beta\",\"album\":\"xalbum\"}]\n"
    );
    // ensure_ascii escapes non-ASCII.
    let lib = &env.lib;
    lib.transaction(|lib| {
        for mut i in items(lib) {
            i.set("title", Value::Str("Tëst".into()));
            lib.store(&i)?;
        }
        Ok(())
    })
    .unwrap();
    env.cfg
        .set("export.json.formatting.ensure_ascii", Y::Bool(true));
    let mut opts = env.keys_opts("json");
    opts.library = true;
    let out = env.run(opts);
    assert!(out.contains(r#""title":"T\u00ebst""#), "{out}");
}

#[test]
fn unreadable_files_are_reported() {
    let Some(env) = env() else { return };
    let bad = env._dir.path().join("bad.mp3");
    std::fs::write(&bad, b"not audio").unwrap();
    let opts = CmdOpts {
        format: Some("json".into()),
        included_keys: vec!["title".into()],
        query: vec![bad.display().to_string()],
        ..Default::default()
    };
    let out = env.run(opts);
    assert_eq!(out, "[]\n");
}
