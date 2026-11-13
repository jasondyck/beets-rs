//! beets' `test/plugins/test_art.py` (and `test_fetchart.py`), with the web
//! services served by a fake server on 127.0.0.1 (port 0) and test images
//! generated with the `image` crate.

use super::*;
use crate::artresizer::tests::{picture, write_image};
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use serde_json::json;
use serde_yaml::Value as Y;
use std::sync::Arc;

/// A library in a temp dir, with fetchart settings from `yaml`.
struct Env {
    dir: tempfile::TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new(yaml: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::defaults();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        cfg.set("directory", Y::String(music.display().to_string()));
        set_deep(&mut cfg, "", serde_yaml::from_str(yaml).unwrap());
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env { dir, cfg, lib }
    }

    fn ctx(&self) -> PluginCtx<'_> {
        PluginCtx {
            cfg: &self.cfg,
            lib: &self.lib,
            write: true,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// An album with one item whose (not necessarily existing) file is in
    /// `music/<dir>/`.
    fn album(&self, dir: &str, fields: &[(&str, &str)]) -> Album {
        let mut item = Item::new_item();
        item.set_path(self.path(&format!("music/{dir}/01.mp3")));
        item.set_str("albumartist", "Some Artist");
        item.set_str("album", "Some Album");
        let mut items = vec![item];
        let mut album = self.lib.add_album(&mut items).unwrap();
        album.set_str("albumartist", "Some Artist");
        album.set_str("album", "Some Album");
        for (k, v) in fields {
            album.set_parsed(k, v).unwrap();
        }
        self.lib.store(&album).unwrap();
        album
    }

    fn plugin(&self, base: Option<&str>) -> FetchArt {
        FetchArt::with(Conf::new(&self.cfg).unwrap(), urls(base), &self.cfg).unwrap()
    }
}

/// Merge a YAML mapping into the config key by key (sections are merged,
/// not replaced).
fn set_deep(cfg: &mut Config, prefix: &str, y: Y) {
    match y {
        Y::Mapping(m) => {
            for (k, v) in m {
                let k = k.as_str().unwrap();
                let key = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{prefix}.{k}")
                };
                set_deep(cfg, &key, v);
            }
        }
        v => cfg.set(prefix, v),
    }
}

fn urls(base: Option<&str>) -> Urls {
    match base {
        None => Urls::default(),
        Some(b) => Urls {
            caa: b.to_string(),
            amazon: format!("{b}/amazon/"),
            albumart: format!("{b}/albumart"),
            google: format!("{b}/google"),
            fanarttv: format!("{b}/fanart/"),
            itunes: format!("{b}/itunes"),
            dbpedia: format!("{b}/sparql"),
            wikipedia: format!("{b}/wiki"),
            lastfm: format!("{b}/lastfm"),
            spotify: format!("{b}/spotify/"),
        },
    }
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    crate::artresizer::encode(&picture(w, h, 0), image::ImageFormat::Jpeg, 90).unwrap()
}

fn png(w: u32, h: u32) -> Vec<u8> {
    crate::artresizer::encode(&picture(w, h, 0), image::ImageFormat::Png, 0).unwrap()
}

fn image_reply(ct: &'static str, body: Vec<u8>) -> Reply {
    Reply {
        status: 200,
        content_type: ct,
        body,
    }
}

/// A fake server: `routes` maps a path prefix (or full target) to a reply;
/// anything else is a 404.
type Route = (&'static str, Arc<dyn Fn(&Seen) -> Reply + Send + Sync>);

fn server(routes: Vec<Route>) -> Server {
    serve(move |r| {
        for (prefix, f) in &routes {
            if r.target.starts_with(prefix) {
                return f(r);
            }
        }
        Reply::status(404)
    })
}

fn route(prefix: &'static str, f: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Route {
    (prefix, Arc::new(f))
}

fn targets(s: &Server) -> Vec<String> {
    s.requests().into_iter().map(|r| r.target).collect()
}

fn source(kind: SourceKind, c: &str) -> Source {
    Source {
        kind,
        match_by: vec![c.to_string()],
    }
}

fn urls_of(cands: &[Candidate]) -> Vec<String> {
    cands.iter().filter_map(|c| c.url.clone()).collect()
}

fn cleanup(c: &Option<Candidate>) {
    if let Some(p) = c.as_ref().and_then(|c| c.path.as_ref())
        && is_temp(p)
    {
        let _ = std::fs::remove_file(p);
    }
}

// ---------------------------------------------------------------- helpers

#[test]
fn python_reprs() {
    assert_eq!(py_float(5.0), "5.0");
    assert_eq!(py_float(2.25), "2.25");
    assert_eq!(py_repr("a b"), "'a b'");
    assert_eq!(py_repr("it's"), "\"it's\"");
    assert_eq!(py_title("the beatles"), "The Beatles");
    assert_eq!(py_title("AC/DC they're"), "Ac/Dc They'Re");
}

// ---------------------------------------------------------------- config

#[test]
fn sources_with_per_source_options() {
    let all: Vec<(String, String)> = [
        ("filesystem", "default"),
        ("coverart", "release"),
        ("coverart", "releasegroup"),
        ("itunes", "default"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let p = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    };
    assert_eq!(
        sanitize_pairs(
            &p(&[("coverart", "releasegroup release"), ("*", "*")]),
            &all
        )
        .unwrap(),
        p(&[
            ("coverart", "releasegroup"),
            ("coverart", "release"),
            ("filesystem", "default"),
            ("itunes", "default"),
        ])
    );
    assert_eq!(
        sanitize_pairs(&p(&[("itunes", "*"), ("coverart", "*")]), &all).unwrap(),
        p(&[
            ("itunes", "default"),
            ("coverart", "release"),
            ("coverart", "releasegroup"),
        ])
    );
    assert_eq!(
        sanitize_pairs(&p(&[("nope", "*")]), &all)
            .unwrap_err()
            .to_string(),
        "setting nope=* is not recognized"
    );
    assert_eq!(
        sanitize_pairs(&p(&[("coverart", "front")]), &all)
            .unwrap_err()
            .to_string(),
        "setting coverart=front is not recognized"
    );

    let env = Env::new(
        "fetchart: {sources: [filesystem, {coverart: releasegroup}, itunes], lastfm_key: k}",
    );
    let fa = env.plugin(None);
    let desc: Vec<String> = fa.sources.iter().map(Source::description).collect();
    assert_eq!(
        desc,
        [
            "filesystem[default]",
            "coverart[releasegroup]",
            "itunes[default]"
        ]
    );
    // The defaults: every criterion of coverart, in order.
    let fa = Env::new("{}").plugin(None);
    let desc: Vec<String> = fa.sources.iter().map(Source::description).collect();
    assert_eq!(
        desc,
        [
            "filesystem[default]",
            "coverart[release]",
            "coverart[releasegroup]",
            "itunes[default]",
            "amazon[default]",
            "albumart[default]",
            "cover_art_url[default]"
        ]
    );
}

#[test]
fn sources_need_their_keys_and_must_exist() {
    let env = Env::new("fetchart: {sources: [google]}");
    let e = FetchArt::with(Conf::new(&env.cfg).unwrap(), urls(None), &env.cfg)
        .err()
        .unwrap();
    assert_eq!(e.to_string(), "setting google=* is not recognized");
    let env = Env::new("fetchart: {sources: [google], google_key: k}");
    assert_eq!(env.plugin(None).sources.len(), 1);
    let env = Env::new("fetchart: {sources: []}");
    let e = FetchArt::with(Conf::new(&env.cfg).unwrap(), urls(None), &env.cfg)
        .err()
        .unwrap();
    assert_eq!(e.to_string(), "fetchart: no sources defined in config");
    // A single source may be a plain string.
    let env = Env::new("fetchart: {sources: amazon}");
    assert_eq!(env.plugin(None).sources[0].kind, SourceKind::Amazon);
}

/// `test_fetchart.py` `test_sources_is_*`.
#[test]
fn sources_config_shapes() {
    let n = |yaml: &str| Env::new(yaml).plugin(None).sources.len();
    assert_eq!(n("fetchart: {sources: filesystem}"), 1);
    // Everything available: 11 sources, coverart twice, minus google and
    // lastfm (no keys).
    assert_eq!(n("fetchart: {sources: '*'}"), 10);
    assert_eq!(n("fetchart: {sources: [filesystem, coverart]}"), 3);
    assert_eq!(
        n("fetchart: {sources: {filesystem: '*', coverart: '*'}}"),
        3
    );
    assert_eq!(n("fetchart: {sources: [{coverart: [releasegroup]}]}"), 1);
}

/// `test_fetchart.py` `test_set_art_from_folder` (a non-ASCII cover name
/// that only matches as the folder's fallback image).
#[test]
fn command_sets_art_from_folder() {
    let env = Env::new(
        "{fetchart: {sources: [filesystem], cover_names: 'cöver.jpg'}, art_filename: mycover}",
    );
    let album = env.album("a", &[]);
    let dir = env.path("music/a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c\u{f6}ver.jpg"), b"IMAGE").unwrap();
    env.plugin(None)
        .batch_fetch_art(&env.ctx(), vec![album.clone()], false, false)
        .unwrap();
    let stored = env.lib.get_album(album.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.path_field("artpath"), dir.join("mycover.jpg"));
    assert_eq!(std::fs::read(dir.join("mycover.jpg")).unwrap(), b"IMAGE");
}

/// `test_art.py` `TestDeprecatedConfig.test_moves_filesystem_to_end`.
#[test]
fn remote_priority_moves_filesystem_to_end() {
    let env = Env::new("fetchart: {remote_priority: yes}");
    let fa = env.plugin(None);
    assert_eq!(fa.sources.last().unwrap().kind, SourceKind::FileSystem);
    assert_eq!(fa.sources[0].kind, SourceKind::CoverArt);
}

/// `TestEnforceRatioConfig`.
#[test]
fn enforce_ratio_config() {
    let conf = |v: &str| Conf::new(&Env::new(&format!("fetchart: {{enforce_ratio: {v}}}")).cfg);
    let c = conf("'10px'").unwrap();
    assert!(c.enforce_ratio);
    assert_eq!(c.margin_px, Some(10));
    let c = conf("'0.5%'").unwrap();
    assert!(c.enforce_ratio);
    assert_eq!(c.margin_percent, Some(0.005));
    let c = conf("yes").unwrap();
    assert!(c.enforce_ratio && c.margin_px.is_none() && c.margin_percent.is_none());
    assert!(!conf("no").unwrap().enforce_ratio);
    for bad in ["'10'", "'px'", "'101%'", "'-1px'"] {
        assert!(conf(bad).is_err(), "{bad}");
    }
}

// ---------------------------------------------------------------- fetch_image

/// `TestFetchImage`.
#[test]
fn fetch_image_checks_the_data() {
    let s = server(vec![
        route("/html", |_| image_reply("text/html", b"<html>".repeat(10))),
        route("/jpeg", |_| image_reply("image/jpeg", jpeg(8, 8))),
        route("/png-as-jpeg", |_| image_reply("image/jpeg", png(8, 8))),
        route("/png", |_| image_reply("image/png", png(8, 8))),
        route("/short", |_| {
            image_reply("image/jpeg", vec![0xFF, 0xD8, 0xFF])
        }),
        route("/gif", |_| {
            image_reply("image/gif", {
                let mut v = b"GIF89a".to_vec();
                v.resize(64, 0);
                v
            })
        }),
    ]);
    let env = Env::new("{}");
    let fa = env.plugin(Some(&s.base));
    let fetch = |p: &str| {
        let mut c = Candidate::url("dummy", format!("{}{p}", s.base), None);
        let (_, logs) = crate::logging::capture(|| fa.fetch_image(&mut c));
        (c.path, logs)
    };
    assert_eq!(fetch("/html").0, None, "invalid type");
    assert_eq!(fetch("/short").0, None, "not enough data");
    assert_eq!(fetch("/gif").0, None, "unsupported type");
    let (p, logs) = fetch("/jpeg");
    let p = p.unwrap();
    assert_eq!(p.extension().unwrap(), "jpg");
    assert!(logs.is_empty(), "{logs:?}");
    std::fs::remove_file(p).unwrap();
    let (p, _) = fetch("/png");
    let p = p.unwrap();
    assert_eq!(p.extension().unwrap(), "png", "extension from the type");
    std::fs::remove_file(p).unwrap();
    // The magic bytes win over the server's Content-Type.
    let (p, logs) = fetch("/png-as-jpeg");
    let p = p.unwrap();
    assert_eq!(p.extension().unwrap(), "png");
    assert_eq!(
        logs,
        [
            "fetchart: Server specified image/jpeg, but returned a image/png image. \
          Correcting the extension to .png"
        ]
    );
    std::fs::remove_file(p).unwrap();
    // Every download is a plain GET with beets' User-Agent.
    assert!(
        s.requests()
            .iter()
            .all(|r| r.header("user-agent") == Some("beets"))
    );
}

// ---------------------------------------------------------------- filesystem

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

fn fs_paths(env: &Env, fa: &FetchArt, dir: &Path) -> Vec<PathBuf> {
    fa.filesystem(&env.ctx(), Some(&[dir.to_path_buf()]))
        .into_iter()
        .filter_map(|c| c.path)
        .collect()
}

/// `TestFSArt`.
#[test]
fn filesystem_source() {
    let env = Env::new("{}");
    let fa = env.plugin(None);
    let d = env.path("art");
    std::fs::create_dir_all(&d).unwrap();
    assert!(fs_paths(&env, &fa, &d).is_empty(), "empty dir");
    assert!(fa.filesystem(&env.ctx(), None).is_empty(), "no paths");

    touch(&d.join("a.jpg"));
    assert_eq!(fs_paths(&env, &fa, &d), [d.join("a.jpg")], "finds jpg");
    std::fs::remove_file(d.join("a.jpg")).unwrap();
    touch(&d.join("a.JPEG"));
    assert_eq!(fs_paths(&env, &fa, &d), [d.join("a.JPEG")], "uppercase ext");
    std::fs::remove_file(d.join("a.JPEG")).unwrap();

    // Non-images and art in subdirectories don't count.
    touch(&d.join("a.txt"));
    touch(&d.join("sub/cover.jpg"));
    assert!(fs_paths(&env, &fa, &d).is_empty());

    // A well-named file wins.
    touch(&d.join("a.jpg"));
    touch(&d.join("art.jpg"));
    let got = fs_paths(&env, &fa, &d);
    assert_eq!(got[0], d.join("art.jpg"));
    assert_eq!(got.last().unwrap(), &d.join("a.jpg"));
}

#[test]
fn filesystem_cover_name_matches_stem_not_extension() {
    let env = Env::new("fetchart: {cover_names: [jpg]}");
    let fa = env.plugin(None);
    let d = env.path("art");
    touch(&d.join("a.jpg"));
    touch(&d.join("b.jpg"));
    let got = fa.filesystem(&env.ctx(), Some(std::slice::from_ref(&d)));
    // Neither is well-named: only the first as a fallback.
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].match_, Some(MetadataMatch::Fallback));
}

#[test]
fn filesystem_precedence_amongst_correct_files() {
    let env = Env::new("fetchart: {cover_names: [cover, front, back]}");
    let fa = env.plugin(None);
    let d = env.path("art");
    for n in ["front-cover.jpg", "front.jpg", "back.jpg"] {
        touch(&d.join(n));
    }
    assert_eq!(
        fs_paths(&env, &fa, &d),
        [
            d.join("front-cover.jpg"),
            d.join("front.jpg"),
            d.join("back.jpg")
        ]
    );
}

#[test]
fn filesystem_cautious_and_fallback() {
    let env = Env::new("fetchart: {cautious: yes}");
    let fa = env.plugin(None);
    let d = env.path("art");
    touch(&d.join("a.jpg"));
    assert!(
        fs_paths(&env, &fa, &d).is_empty(),
        "cautious skips fallback"
    );

    let fallback = env.path("fb/fallback.jpg");
    touch(&fallback);
    let env2 = Env::new(&format!("fetchart: {{fallback: '{}'}}", fallback.display()));
    let fa2 = env2.plugin(None);
    let d2 = env2.path("art");
    touch(&d2.join("a.jpg"));
    assert_eq!(
        fs_paths(&env2, &fa2, &d2),
        [fallback.clone(), d2.join("a.jpg")],
        "configured fallback first"
    );
    // `_is_candidate_fallback` survives files that don't exist.
    let c = Candidate::file("filesystem", env2.path("nope.jpg"), MetadataMatch::Exact);
    assert!(!fa2.is_candidate_fallback(&c));
    let c = Candidate::file("filesystem", fallback, MetadataMatch::Fallback);
    assert!(fa2.is_candidate_fallback(&c));
}

#[test]
fn filesystem_respects_ignore() {
    let env = Env::new("ignore: ['skip*']");
    let fa = env.plugin(None);
    let d = env.path("art");
    touch(&d.join("skip-cover.jpg"));
    touch(&d.join(".hidden-cover.jpg"));
    assert!(fs_paths(&env, &fa, &d).is_empty());
}

// ---------------------------------------------------------------- combined

/// A server with one image at /image.jpg and the given extra routes.
fn image_server() -> Server {
    server(vec![
        route("/amazon/", |_| image_reply("image/jpeg", jpeg(10, 10))),
        route("/albumart", |r| {
            if r.target.starts_with("/albumart?asin=") {
                Reply::text(r#"<a href ="IMG_URL" title ="View larger image">"#.replace(
                    "IMG_URL",
                    "/img/aao.jpg",
                ).as_str())
            } else {
                image_reply("image/jpeg", jpeg(10, 10))
            }
        }),
        route("/img/", |_| image_reply("image/jpeg", jpeg(10, 10))),
        route("/release/", |_| {
            Reply::json(json!({"images": [{"types": ["Front"], "image": "IMG"}]}))
        }),
    ])
}

/// `TestCombined`.
#[test]
fn combined_main_interface() {
    let s = image_server();
    let env = Env::new("fetchart: {sources: [filesystem, coverart, amazon, albumart]}");
    let fa = env.plugin(Some(&s.base));
    let ctx = env.ctx();

    // Amazon art with only an ASIN.
    let album = env.album("a", &[("asin", "xxxx")]);
    let c = fa.art_for_album(&ctx, &album, None, false);
    assert!(c.as_ref().unwrap().path.is_some());
    assert_eq!(c.as_ref().unwrap().source_name, "amazon");
    cleanup(&c);
    assert_eq!(targets(&s), ["/amazon/xxxx.01.LZZZZZZZ.jpg"]);

    // Nothing without an ASIN or a path.
    let album = env.album("b", &[]);
    assert!(fa.art_for_album(&ctx, &album, None, false).is_none());

    // Filesystem art first.
    let d = env.path("src");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("cover.jpg"), jpeg(5, 5)).unwrap();
    let album = env.album("c", &[("asin", "xxxx")]);
    let n = s.requests().len();
    let c = fa
        .art_for_album(&ctx, &album, Some(std::slice::from_ref(&d)), false)
        .unwrap();
    assert_eq!(c.path, Some(d.join("cover.jpg")));
    assert_eq!(s.requests().len(), n, "no requests");

    // Local only: no network, but filesystem art is found.
    let n = s.requests().len();
    assert!(fa.art_for_album(&ctx, &album, None, true).is_none());
    let c = fa
        .art_for_album(&ctx, &album, Some(std::slice::from_ref(&d)), true)
        .unwrap();
    assert_eq!(c.path, Some(d.join("cover.jpg")));
    assert_eq!(s.requests().len(), n);
}

#[test]
fn combined_falls_back_in_source_order() {
    // Amazon has nothing: AlbumArt.org is tried next.
    let s = server(vec![
        route("/amazon/", |_| Reply::status(404)),
        route("/albumart?asin=", |r| {
            let host = r.header("host").unwrap();
            Reply::text(&format!(
                r#"<a href="http://{host}/img/aao.jpg" title="View larger image">"#
            ))
        }),
        route("/img/", |_| image_reply("image/jpeg", jpeg(10, 10))),
    ]);
    let env = Env::new("fetchart: {sources: [amazon, albumart]}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("asin", "xxxx")]);
    let c = fa.art_for_album(&env.ctx(), &album, None, false);
    assert_eq!(c.as_ref().unwrap().source_name, "albumart");
    cleanup(&c);
    assert_eq!(
        targets(&s),
        [
            "/amazon/xxxx.01.LZZZZZZZ.jpg",
            "/amazon/xxxx.02.LZZZZZZZ.jpg",
            "/albumart?asin=xxxx",
            "/img/aao.jpg"
        ]
    );
}

// ---------------------------------------------------------------- coverart

const CAA_RELEASE: &str = r#"{
    "images": [
    {"front": true, "image": "http://caa/release/rid/12345.gif",
     "thumbnails": {"1200": "http://caa/rid-1200.jpg", "250": "http://caa/rid-250.jpg",
       "500": "http://caa/rid-500.jpg", "large": "http://caa/rid-500.jpg",
       "small": "http://caa/rid-250.jpg"},
     "types": ["Front"]},
    {"front": false, "image": "http://caa/release/rid/12345.jpg",
     "thumbnails": {"1200": "http://caa/rid-1200.jpg", "250": "http://caa/rid-250.jpg",
       "500": "http://caa/rid-500.jpg", "large": "http://caa/rid-500.jpg",
       "small": "http://caa/rid-250.jpg"},
     "types": ["Front"]},
    {"image": "http://caa/back.jpg", "types": ["Back"]}
    ]}"#;
const CAA_RELEASE_WITHOUT_THUMBNAILS: &str = r#"{
    "images": [
    {"image": "http://caa/release/rid/12345.gif", "types": ["Front"]},
    {"image": "http://caa/release/rid/12345.jpg",
     "thumbnails": {"large": "http://caa/rgid-500.jpg", "small": "http://caa/rgid-250.jpg"},
     "types": ["Front"]}
    ]}"#;
const CAA_GROUP: &str = r#"{
    "images": [
    {"image": "http://caa/release/releaseid/12345.jpg",
     "thumbnails": {"1200": "http://caa/rgid-1200.jpg", "250": "http://caa/rgid-250.jpg",
       "500": "http://caa/rgid-500.jpg"},
     "types": ["Front"]}
    ]}"#;

fn caa_server(release: &'static str, group: &'static str) -> Server {
    server(vec![
        route("/release/rid", move |_| Reply {
            status: 200,
            content_type: "application/json",
            body: release.as_bytes().to_vec(),
        }),
        route("/release-group/rgid", move |_| Reply {
            status: 200,
            content_type: "application/json",
            body: group.as_bytes().to_vec(),
        }),
    ])
}

/// `TestCoverArtArchive`.
#[test]
fn caa_finds_images() {
    let s = caa_server(CAA_RELEASE, CAA_GROUP);
    let env = Env::new("fetchart: {maxwidth: 0}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("mb_albumid", "rid"), ("mb_releasegroupid", "rgid")]);
    let rel = fa.candidates(
        &env.ctx(),
        &source(SourceKind::CoverArt, "release"),
        &album,
        None,
    );
    assert_eq!(
        urls_of(&rel),
        [
            "http://caa/release/rid/12345.gif",
            "http://caa/release/rid/12345.jpg"
        ]
    );
    assert!(rel.iter().all(|c| c.match_ == Some(MetadataMatch::Exact)));
    let grp = fa.candidates(
        &env.ctx(),
        &source(SourceKind::CoverArt, "releasegroup"),
        &album,
        None,
    );
    assert_eq!(urls_of(&grp), ["http://caa/release/releaseid/12345.jpg"]);
    assert_eq!(grp[0].match_, Some(MetadataMatch::Fallback));
    assert_eq!(targets(&s), ["/release/rid", "/release-group/rgid"]);
    // No IDs, no requests.
    let bare = env.album("b", &[]);
    let n = s.requests().len();
    assert!(
        fa.candidates(
            &env.ctx(),
            &source(SourceKind::CoverArt, "release"),
            &bare,
            None
        )
        .is_empty()
    );
    assert_eq!(s.requests().len(), n);
}

#[test]
fn caa_uses_pre_sized_maxwidth_thumbs() {
    let s = caa_server(CAA_RELEASE, CAA_GROUP);
    let env = Env::new("fetchart: {maxwidth: 250}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("mb_albumid", "rid"), ("mb_releasegroupid", "rgid")]);
    let rel = fa.candidates(
        &env.ctx(),
        &source(SourceKind::CoverArt, "release"),
        &album,
        None,
    );
    assert_eq!(
        urls_of(&rel),
        ["http://caa/rid-250.jpg", "http://caa/rid-250.jpg"]
    );
    let grp = fa.candidates(
        &env.ctx(),
        &source(SourceKind::CoverArt, "releasegroup"),
        &album,
        None,
    );
    assert_eq!(urls_of(&grp), ["http://caa/rgid-250.jpg"]);
}

#[test]
fn caa_maxwidth_without_thumbnails() {
    let s = caa_server(CAA_RELEASE_WITHOUT_THUMBNAILS, CAA_GROUP);
    let env = Env::new("fetchart: {maxwidth: 250}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("mb_albumid", "rid")]);
    let rel = fa.candidates(
        &env.ctx(),
        &source(SourceKind::CoverArt, "release"),
        &album,
        None,
    );
    // The legacy "small" name stands in for 250.
    assert_eq!(
        urls_of(&rel),
        [
            "http://caa/release/rid/12345.gif",
            "http://caa/rgid-250.jpg"
        ]
    );
    // 1200 has no legacy name: the full image.
    let env = Env::new("fetchart: {maxwidth: 1200}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("mb_albumid", "rid")]);
    let rel = fa.candidates(
        &env.ctx(),
        &source(SourceKind::CoverArt, "release"),
        &album,
        None,
    );
    assert_eq!(
        urls_of(&rel),
        [
            "http://caa/release/rid/12345.gif",
            "http://caa/release/rid/12345.jpg"
        ]
    );
}

#[test]
fn caa_release_group_fallback_end_to_end() {
    let s = server(vec![
        route("/release/rid", |_| Reply::status(404)),
        route("/release-group/rgid", |r| {
            let base = format!("http://{}", r.header("host").unwrap());
            Reply {
                status: 200,
                content_type: "application/json",
                body:
                    json!({"images": [{"types": ["Front"], "image": format!("{base}/img/g.jpg")}]})
                        .to_string()
                        .into_bytes(),
            }
        }),
        route("/img/", |_| image_reply("image/jpeg", jpeg(20, 20))),
    ]);
    let env = Env::new("fetchart: {sources: [coverart]}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("mb_albumid", "rid"), ("mb_releasegroupid", "rgid")]);
    let c = fa.art_for_album(&env.ctx(), &album, None, false);
    assert_eq!(c.as_ref().unwrap().source_name, "coverart");
    assert_eq!(c.as_ref().unwrap().match_, Some(MetadataMatch::Fallback));
    cleanup(&c);
    assert_eq!(
        targets(&s),
        ["/release/rid", "/release-group/rgid", "/img/g.jpg"]
    );
}

// ---------------------------------------------------------------- albumart

/// `TestAAO`.
#[test]
fn aao_scraper() {
    let s = server(vec![route("/albumart", |r| {
        if r.target.contains("asin=found") {
            Reply::text(
                r#"<br />
                <a href="TARGET_URL" title="View larger image"
                   class="thickbox" style="color: #7E9DA2; text-decoration:none;">
                <img src="http://www.albumart.org/images/zoom-icon.jpg"
                     alt="View larger image" width="17" height="15"  border="0"/></a>"#,
            )
        } else {
            Reply::text("blah blah")
        }
    })]);
    let env = Env::new("{}");
    let fa = env.plugin(Some(&s.base));
    let src = source(SourceKind::AlbumArt, "default");
    let album = env.album("a", &[("asin", "found")]);
    assert_eq!(
        urls_of(&fa.candidates(&env.ctx(), &src, &album, None)),
        ["TARGET_URL"]
    );
    let album = env.album("b", &[("asin", "missing")]);
    assert!(fa.candidates(&env.ctx(), &src, &album, None).is_empty());
}

// ---------------------------------------------------------------- itunes

fn itunes_server(body: &'static str, status: u16) -> Server {
    server(vec![route("/itunes", move |_| Reply {
        status,
        content_type: "application/json",
        body: body.as_bytes().to_vec(),
    })])
}

fn itunes(env: &Env, s: &Server) -> Vec<Candidate> {
    let fa = env.plugin(Some(&s.base));
    let album = env.album(
        "a",
        &[("albumartist", "some artist"), ("album", "some album")],
    );
    fa.candidates(
        &env.ctx(),
        &source(SourceKind::ITunes, "default"),
        &album,
        None,
    )
}

/// `TestITunesStore`.
#[test]
fn itunes_finds_image_and_fallback() {
    let s = itunes_server(
        r#"{"results": [
            {"artistName": "some artist", "collectionName": "some album",
             "artworkUrl100": "url_to_the_image/100x100bb.jpg"},
            {"artistName": "other", "collectionName": "x",
             "artworkUrl100": "other/100x100bb.jpg"}
        ]}"#,
        200,
    );
    let env = Env::new("{}");
    let got = itunes(&env, &s);
    assert_eq!(
        urls_of(&got),
        [
            "url_to_the_image/1200x1200bb.jpg",
            "url_to_the_image/1200x1200bb.jpg"
        ]
    );
    assert_eq!(got[0].match_, Some(MetadataMatch::Exact));
    assert_eq!(got[1].match_, Some(MetadataMatch::Fallback));
    let t = &s.requests()[0].target;
    assert_eq!(
        t,
        "/itunes?term=some+artist+some+album&entity=album&media=music&limit=200"
    );
    // high_resolution
    let env = Env::new("fetchart: {high_resolution: yes}");
    assert_eq!(
        urls_of(&itunes(&env, &s))[0],
        "url_to_the_image/10000x10000-999.jpg"
    );
}

#[test]
fn itunes_fallback_match_only() {
    let s = itunes_server(
        r#"{"results": [{"collectionName": "some album", "artistName": "some other artist",
            "artworkUrl100": "url_to_the_image/100x100bb.jpg"}]}"#,
        200,
    );
    let got = itunes(&Env::new("{}"), &s);
    assert_eq!(urls_of(&got), ["url_to_the_image/1200x1200bb.jpg"]);
    assert_eq!(got[0].match_, Some(MetadataMatch::Fallback));
}

#[test]
fn itunes_no_results_errors_and_malformed() {
    let env = Env::new("{}");
    for (body, status) in [
        (r#"{"results": []}"#, 200),
        ("", 404),
        (r#"{"error": "not found"}"#, 200),
        ("[[[[malformed", 200),
        (
            r#"{"results": [{"artistName": "some artist", "collectionName": "some album"}]}"#,
            200,
        ),
    ] {
        let s = itunes_server(body, status);
        assert!(itunes(&env, &s).is_empty(), "{body}");
    }
    // A transport failure is just "no result".
    let fa = env.plugin(Some("http://127.0.0.1:1"));
    let album = env.album("x", &[]);
    assert!(
        fa.candidates(
            &env.ctx(),
            &source(SourceKind::ITunes, "default"),
            &album,
            None
        )
        .is_empty()
    );
}

// ---------------------------------------------------------------- google

/// `TestGoogleImage`.
#[test]
fn google_images() {
    let s = server(vec![route("/google", |r| {
        if r.target.contains("q=error") {
            Reply::json(json!({"error": {"errors": [{"reason": "some reason"}]}}))
        } else if r.target.contains("q=bad") {
            Reply::text("bla blup")
        } else {
            Reply::json(json!({"items": [{"link": "url_to_the_image"}]}))
        }
    })]);
    let env = Env::new("fetchart: {google_key: KEY}");
    let fa = env.plugin(Some(&s.base));
    let src = source(SourceKind::Google, "default");
    let album = env.album(
        "a",
        &[("albumartist", "some artist"), ("album", "some album")],
    );
    assert_eq!(
        urls_of(&fa.candidates(&env.ctx(), &src, &album, None)),
        ["url_to_the_image"]
    );
    assert_eq!(
        s.requests()[0].target,
        "/google?key=KEY&cx=001442825323518660753%3Ahrh5ch1gjzm\
         &q=some+artist%2Csome+album&searchType=image"
    );
    for which in ["error", "bad"] {
        let album = env.album(which, &[("albumartist", which), ("album", "x")]);
        assert!(fa.candidates(&env.ctx(), &src, &album, None).is_empty());
    }
}

// ---------------------------------------------------------------- fanart.tv

const FANART_MULTIPLE: &str = r#"{
    "name": "artistname", "mbid_id": "artistid",
    "albums": {"thereleasegroupid": {
        "albumcover": [
            {"id": "24", "url": "http://example.com/1.jpg", "likes": "0"},
            {"id": "42", "url": "http://example.com/2.jpg", "likes": "3"},
            {"id": "23", "url": "http://example.com/3.jpg", "likes": "0"}],
        "cdart": [{"id": "123", "url": "http://example.com/4.jpg", "likes": "0"}]}}}"#;
const FANART_NO_ART: &str = r#"{"albums": {"thereleasegroupid": {
    "cdart": [{"id": "123", "url": "http://example.com/4.jpg", "likes": "0"}]}}}"#;

/// `TestFanartTV`.
#[test]
fn fanarttv() {
    let s = server(vec![route("/fanart/", |r| {
        let body = match r.target.as_str() {
            "/fanart/thereleasegroupid" => FANART_MULTIPLE,
            "/fanart/noart" => FANART_NO_ART,
            "/fanart/error" => r#"{"status": "error", "error message": "the error message"}"#,
            _ => "bla blup",
        };
        Reply {
            status: 200,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
        }
    })]);
    let env = Env::new("fetchart: {fanarttv_key: CLIENT}");
    let fa = env.plugin(Some(&s.base));
    let src = source(SourceKind::FanartTV, "default");
    let album = env.album("a", &[("mb_releasegroupid", "thereleasegroupid")]);
    let got = fa.candidates(&env.ctx(), &src, &album, None);
    // Most liked first; cdart ignored.
    assert_eq!(
        urls_of(&got),
        [
            "http://example.com/2.jpg",
            "http://example.com/1.jpg",
            "http://example.com/3.jpg"
        ]
    );
    assert_eq!(got[0].size, Some((1000, 1000)));
    let r = &s.requests()[0];
    assert_eq!(r.header("api-key"), Some(FANARTTV_PROJECT_KEY));
    assert_eq!(r.header("client-key"), Some("CLIENT"));
    for id in ["noart", "error", "malformed"] {
        let album = env.album(id, &[("mb_releasegroupid", id)]);
        assert!(
            fa.candidates(&env.ctx(), &src, &album, None).is_empty(),
            "{id}"
        );
    }
}

// ---------------------------------------------------------------- lastfm

#[test]
fn lastfm() {
    let s = server(vec![route("/lastfm", |r| {
        if r.target.contains("mbid=missing") {
            Reply::json(json!({"error": 6, "message": "Album not found"}))
        } else if r.target.contains("mbid=bad") {
            Reply::json(json!({"error": 10, "message": "Invalid API key"}))
        } else {
            Reply::json(json!({"album": {"image": [
                {"size": "small", "#text": "http://lfm/s.jpg"},
                {"size": "large", "#text": "http://lfm/l.jpg"},
                {"size": "mega", "#text": "http://lfm/m.jpg"},
            ]}}))
        }
    })]);
    let env = Env::new("fetchart: {lastfm_key: LKEY}");
    let fa = env.plugin(Some(&s.base));
    let src = source(SourceKind::LastFM, "default");
    let album = env.album("a", &[("mb_albumid", "rid")]);
    let got = fa.candidates(&env.ctx(), &src, &album, None);
    assert_eq!(
        urls_of(&got),
        ["http://lfm/m.jpg", "http://lfm/l.jpg", "http://lfm/s.jpg"]
    );
    assert_eq!(got[1].size, Some((174, 174)));
    assert_eq!(got[0].match_, None);
    assert_eq!(
        s.requests()[0].target,
        "/lastfm?method=album.getinfo&api_key=LKEY&mbid=rid&format=json"
    );
    let album = env.album("m", &[("mb_albumid", "missing")]);
    assert!(fa.candidates(&env.ctx(), &src, &album, None).is_empty());
    // Other errors are logged as errors.
    let album = env.album("b", &[("mb_albumid", "bad")]);
    let (got, logs) = crate::logging::capture(|| fa.candidates(&env.ctx(), &src, &album, None));
    assert!(got.is_empty());
    assert_eq!(
        logs,
        ["fetchart: lastfm: failed to get album info: Invalid API key (10)"]
    );
}

// ---------------------------------------------------------------- wikipedia

#[test]
fn wikipedia() {
    let s = server(vec![
        route("/sparql", |r| {
            assert_eq!(r.header("content-type"), Some("application/json"));
            assert!(
                r.target.contains("foaf%3Aname+%22The+Artist%22%40en"),
                "{}",
                r.target
            );
            Reply::json(json!({"results": {"bindings": [
                {"pageId": {"value": "123"}, "coverFilename": {"value": "Cover .jpg"}}
            ]}}))
        }),
        route(
            "/wiki?format=json&action=query&continue=&prop=images",
            |_| {
                Reply::json(json!({"query": {"pages": {"123": {"images": [
                    {"title": "File:Other.png"}, {"title": "File:Cover art.jpg"}
                ]}}}}))
            },
        ),
        route(
            "/wiki?format=json&action=query&continue=&prop=imageinfo",
            |r| {
                assert!(
                    r.target
                        .ends_with("&iiprop=url&titles=File%3ACover+art.jpg"),
                    "{}",
                    r.target
                );
                Reply::json(json!({"query": {"pages": {"-1": {"imageinfo": [
                    {"url": "http://wiki/cover.jpg"}
                ]}}}}))
            },
        ),
    ]);
    let env = Env::new("{}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album(
        "a",
        &[("albumartist", "the artist"), ("album", "The Album")],
    );
    let got = fa.candidates(
        &env.ctx(),
        &source(SourceKind::Wikipedia, "default"),
        &album,
        None,
    );
    assert_eq!(urls_of(&got), ["http://wiki/cover.jpg"]);
    assert_eq!(s.requests().len(), 3);
}

// ---------------------------------------------------------------- spotify, cover_art_url

#[test]
fn spotify_and_cover_art_url() {
    let s = server(vec![route("/spotify/", |r| {
        if r.target == "/spotify/abc" {
            Reply::text(
                r#"<html><head><meta property="og:image" content="http://sp/i.jpg"></head></html>"#,
            )
        } else {
            Reply::text("<html></html>")
        }
    })]);
    let env = Env::new("{}");
    let fa = env.plugin(Some(&s.base));
    let src = source(SourceKind::Spotify, "default");
    let album = env.album("a", &[]);
    assert!(
        fa.candidates(&env.ctx(), &src, &album, None).is_empty(),
        "no id"
    );
    let mut item = env.lib.album_items(album.id.unwrap()).unwrap().remove(0);
    item.set_str("spotify_album_id", "abc");
    env.lib.store(&item).unwrap();
    assert_eq!(
        urls_of(&fa.candidates(&env.ctx(), &src, &album, None)),
        ["http://sp/i.jpg"]
    );
    item.set_str("spotify_album_id", "none");
    env.lib.store(&item).unwrap();
    assert!(
        fa.candidates(&env.ctx(), &src, &album, None).is_empty(),
        "no og:image"
    );

    let src = source(SourceKind::CoverArtUrl, "default");
    assert!(fa.candidates(&env.ctx(), &src, &album, None).is_empty());
    item.set_str("cover_art_url", "http://item/cover.jpg");
    env.lib.store(&item).unwrap();
    assert_eq!(
        urls_of(&fa.candidates(&env.ctx(), &src, &album, None)),
        ["http://item/cover.jpg"]
    );
    let mut album = album;
    album.set_str("cover_art_url", "http://album/cover.jpg");
    assert_eq!(
        urls_of(&fa.candidates(&env.ctx(), &src, &album, None)),
        ["http://album/cover.jpg"],
        "the album's own field first"
    );
}

// ---------------------------------------------------------------- validation

fn candidate_for(env: &Env, w: u32, h: u32) -> Candidate {
    let p = env.path(&format!("img-{w}x{h}.jpg"));
    write_image(&p, &picture(w, h, 0), 90);
    Candidate::file("filesystem", p, MetadataMatch::Exact)
}

fn check(env: &Env, yaml: &str, w: u32, h: u32) -> ImageAction {
    let mut cfg = env.cfg.clone();
    let y: Y = serde_yaml::from_str(yaml).unwrap();
    if let Y::Mapping(m) = y {
        for (k, v) in m {
            cfg.set(&format!("fetchart.{}", k.as_str().unwrap()), v);
        }
    }
    let conf = Conf::new(&cfg).unwrap();
    let mut c = candidate_for(env, w, h);
    c.check(&PluginLog::new("fetchart"), &conf, &[])
}

/// `TestAlbumArtOperationConfiguration`.
#[test]
fn validation_minwidth_and_ratio() {
    use ImageAction::*;
    let env = Env::new("{}");
    assert_eq!(check(&env, "{}", 100, 50), Exact, "no checks");
    assert_eq!(check(&env, "{minwidth: 100}", 99, 99), Bad);
    assert_eq!(check(&env, "{minwidth: 100}", 100, 100), Exact);
    assert_eq!(check(&env, "{enforce_ratio: yes}", 100, 100), Exact);
    assert_eq!(check(&env, "{enforce_ratio: yes}", 100, 99), Bad);
    assert_eq!(check(&env, "{enforce_ratio: '5px'}", 100, 95), Exact);
    assert_eq!(check(&env, "{enforce_ratio: '5px'}", 100, 94), Bad);
    assert_eq!(check(&env, "{enforce_ratio: '0px'}", 100, 99), Bad);
    assert_eq!(check(&env, "{enforce_ratio: '5%'}", 100, 95), Exact);
    assert_eq!(check(&env, "{enforce_ratio: '5%'}", 100, 94), Bad);
    assert_eq!(check(&env, "{enforce_ratio: '0%'}", 100, 99), Bad);
}

#[test]
fn validation_actions() {
    use ImageAction::*;
    let env = Env::new("{}");
    assert_eq!(check(&env, "{maxwidth: 50}", 100, 100), Downscale);
    assert_eq!(check(&env, "{maxwidth: 100}", 100, 100), Exact);
    assert_eq!(check(&env, "{max_filesize: 10}", 100, 100), Downsize);
    assert_eq!(check(&env, "{deinterlace: yes}", 10, 10), Deinterlace);
    assert_eq!(check(&env, "{cover_format: PNG}", 10, 10), Reformat);
    assert_eq!(check(&env, "{cover_format: JPEG}", 10, 10), Exact);
    // Order: downscale, reformat, deinterlace, downsize.
    assert_eq!(
        check(
            &env,
            "{maxwidth: 5, cover_format: PNG, deinterlace: yes, max_filesize: 1}",
            10,
            10
        ),
        Downscale
    );
    // Unreadable images pass (with a warning), as in beets without a backend.
    let p = env.path("junk.jpg");
    std::fs::write(&p, b"junk").unwrap();
    let conf = Conf::new(&Env::new("fetchart: {minwidth: 10}").cfg).unwrap();
    let mut c = Candidate::file("filesystem", p, MetadataMatch::Exact);
    let (r, logs) = crate::logging::capture(|| c.check(&PluginLog::new("fetchart"), &conf, &[]));
    assert_eq!(r, Exact);
    assert!(
        logs.iter()
            .any(|l| l.contains("Could not get size of image")),
        "{logs:?}"
    );
    // No path at all: bad.
    let mut c = Candidate::url("x", "u", None);
    assert_eq!(c.check(&PluginLog::new("fetchart"), &conf, &[]), Bad);
}

/// `TestAlbumArtPerformOperation`, with real image processing.
#[test]
fn processing() {
    let env = Env::new("{}");
    let conf = |yaml: &str| {
        let e = Env::new(&format!("fetchart: {yaml}"));
        Conf::new(&e.cfg).unwrap()
    };
    let log = PluginLog::new("fetchart");

    // Downscaled to maxwidth.
    let mut c = candidate_for(&env, 400, 200);
    c.resize(&log, &conf("{maxwidth: 100}"));
    let p = c.path.clone().unwrap();
    assert_eq!(artresizer::get_size(&p), Some((100, 50)));
    assert!(is_temp(&p));
    std::fs::remove_file(p).unwrap();

    // Size limited without scaling.
    let mut c = candidate_for(&env, 300, 300);
    let orig = std::fs::metadata(c.path.as_ref().unwrap()).unwrap().len();
    c.resize(&log, &conf(&format!("{{max_filesize: {}}}", orig / 2)));
    let p = c.path.clone().unwrap();
    assert_eq!(artresizer::get_size(&p), Some((300, 300)));
    assert!(std::fs::metadata(&p).unwrap().len() <= orig / 2);
    std::fs::remove_file(p).unwrap();

    // Not resized when it fits.
    let mut c = candidate_for(&env, 50, 50);
    let before = c.path.clone();
    c.resize(&log, &conf("{maxwidth: 100, max_filesize: 10000000}"));
    assert_eq!(c.path, before);

    // Deinterlaced (rewritten) and resized.
    let mut c = candidate_for(&env, 200, 200);
    c.resize(&log, &conf("{maxwidth: 100, deinterlace: yes}"));
    let p = c.path.clone().unwrap();
    assert_eq!(artresizer::get_size(&p), Some((100, 100)));
    std::fs::remove_file(p).unwrap();

    // Reformatted: the file gets the new type and extension.
    let mut c = candidate_for(&env, 20, 20);
    let src = c.path.clone().unwrap();
    let tmp = artresizer::temp_filename("test_", Some(&src));
    std::fs::copy(&src, &tmp).unwrap();
    c.path = Some(tmp.clone());
    c.resize(&log, &conf("{cover_format: PNG}"));
    let p = c.path.clone().unwrap();
    assert_eq!(p, tmp.with_extension("png"));
    assert_eq!(artresizer::get_format(&p).as_deref(), Some("PNG"));
    assert!(!tmp.exists());
    std::fs::remove_file(p).unwrap();
}

// ---------------------------------------------------------------- import

struct ImportEnv {
    env: Env,
    album: Album,
    src: PathBuf,
}

/// An imported album (copied into the library) whose source folder holds
/// `cover.jpg`. `TestArtImporter`'s setup.
fn import_env(yaml: &str) -> ImportEnv {
    let env = Env::new(yaml);
    let src = env.path("src/album");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("cover.jpg"), jpeg(30, 30)).unwrap();
    let album = env.album("Some Artist/Some Album", &[]);
    std::fs::create_dir_all(env.path("music/Some Artist/Some Album")).unwrap();
    ImportEnv { env, album, src }
}

fn run_import(ie: &mut ImportEnv, op: FileOp, asis: bool) {
    let fa = ie.env.plugin(None);
    let ctx = ie.env.ctx();
    let mut items = vec![];
    let paths = [ie.src.clone()];
    let mut task = ImportTask {
        album: Some(&mut ie.album),
        items: &mut items,
        paths: &paths,
        asis,
        op,
    };
    fa.import_stage(&ctx, &mut task).unwrap();
    fa.import_task_files(&ctx, &mut task).unwrap();
}

#[test]
fn import_fetches_art() {
    let mut ie = import_env("fetchart: {sources: [filesystem]}");
    run_import(&mut ie, FileOp::Copy, true);
    let art = ie.env.path("music/Some Artist/Some Album/cover.jpg");
    assert_eq!(ie.album.path_field("artpath"), art);
    assert!(art.is_file());
    assert!(ie.src.join("cover.jpg").exists(), "original left in place");
    let stored = ie.env.lib.get_album(ie.album.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.path_field("artpath"), art);
    assert_eq!(stored.get("art_source"), None, "store_source is off");
}

#[test]
fn import_moves_art_when_originals_go() {
    let mut ie = import_env("fetchart: {sources: [filesystem], store_source: yes}");
    run_import(&mut ie, FileOp::Move, false);
    assert!(!ie.src.join("cover.jpg").exists(), "moved");
    let stored = ie.env.lib.get_album(ie.album.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.str("art_source"), "filesystem");
    // With import.delete too.
    let mut ie = import_env("{fetchart: {sources: [filesystem]}, import: {delete: yes}}");
    run_import(&mut ie, FileOp::Copy, false);
    assert!(!ie.src.join("cover.jpg").exists());
}

#[test]
fn import_art_not_found_and_existing_art() {
    let mut ie = import_env("fetchart: {sources: [filesystem]}");
    std::fs::remove_file(ie.src.join("cover.jpg")).unwrap();
    run_import(&mut ie, FileOp::Copy, true);
    assert_eq!(ie.album.path_field("artpath"), Path::new(""));

    // An album that already has art (a re-import) is left alone.
    let mut ie = import_env("fetchart: {sources: [filesystem]}");
    let old = ie.env.path("music/Some Artist/Some Album/old.png");
    std::fs::write(&old, png(4, 4)).unwrap();
    ie.album.set("artpath", Value::Path(old.clone()));
    run_import(&mut ie, FileOp::Move, false);
    assert_eq!(ie.album.path_field("artpath"), old);
    assert!(ie.src.join("cover.jpg").exists());
}

#[test]
fn import_does_not_delete_art_already_in_place() {
    // Importing from the library folder itself (as `-L`): the art is the
    // destination already.
    let mut ie = import_env("fetchart: {sources: [filesystem]}");
    let dir = ie.env.path("music/Some Artist/Some Album");
    std::fs::write(dir.join("cover.jpg"), jpeg(30, 30)).unwrap();
    ie.src = dir.clone();
    run_import(&mut ie, FileOp::Move, false);
    assert!(dir.join("cover.jpg").exists());
    assert_eq!(ie.album.path_field("artpath"), dir.join("cover.jpg"));
}

#[test]
fn import_as_is_stays_local_unless_fetch_for_asis() {
    let s = image_server();
    let env = Env::new("fetchart: {sources: [amazon]}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("asin", "xxxx")]);
    assert!(fa.fetch_art(&env.ctx(), &album, &[], true).is_none());
    assert!(s.requests().is_empty());
    let c = fa.fetch_art(&env.ctx(), &album, &[], false);
    assert!(c.is_some());
    cleanup(&c);
    let env = Env::new("fetchart: {sources: [amazon], fetch_for_asis: yes}");
    let fa = env.plugin(Some(&s.base));
    let album = env.album("a", &[("asin", "xxxx")]);
    let c = fa.fetch_art(&env.ctx(), &album, &[], true);
    assert!(c.is_some());
    cleanup(&c);
}

#[test]
fn import_processes_and_removes_temporaries() {
    let mut ie = import_env("fetchart: {sources: [filesystem], maxwidth: 10, cover_format: PNG}");
    run_import(&mut ie, FileOp::Copy, true);
    let art = ie.env.path("music/Some Artist/Some Album/cover.png");
    assert_eq!(ie.album.path_field("artpath"), art);
    assert_eq!(artresizer::get_size(&art), Some((10, 10)));
    assert!(ie.src.join("cover.jpg").exists());
}

// ---------------------------------------------------------------- command

#[test]
fn command_output() {
    let env = Env::new("{fetchart: {sources: [filesystem]}, ui: {color: no}}");
    let album = env.album("a", &[]);
    let dir = env.path("music/a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("front.jpg"), jpeg(10, 10)).unwrap();
    let fa = env.plugin(None);
    fa.batch_fetch_art(&env.ctx(), vec![album.clone()], false, false)
        .unwrap();
    let stored = env.lib.get_album(album.id.unwrap()).unwrap().unwrap();
    // Moved into place under art_filename, as beets' `fetchart` does.
    assert_eq!(stored.path_field("artpath"), dir.join("cover.jpg"));
    assert!(!dir.join("front.jpg").exists());
    // -f: web sources only, so nothing is found.
    fa.batch_fetch_art(&env.ctx(), vec![stored.clone()], true, false)
        .unwrap();
}

/// The `store_source` and default `art_filename` with a `cover_art_url`
/// download, end to end.
#[test]
fn command_downloads_from_cover_art_url() {
    let s = server(vec![route("/img/", |_| {
        image_reply("image/png", png(16, 16))
    })]);
    let env = Env::new("{fetchart: {sources: [cover_art_url], store_source: yes}}");
    let mut album = env.album("a", &[]);
    album.set_str("cover_art_url", format!("{}/img/x.png", s.base));
    env.lib.store(&album).unwrap();
    std::fs::create_dir_all(env.path("music/a")).unwrap();
    let fa = env.plugin(Some(&s.base));
    fa.batch_fetch_art(&env.ctx(), vec![album.clone()], false, true)
        .unwrap();
    let stored = env.lib.get_album(album.id.unwrap()).unwrap().unwrap();
    assert_eq!(stored.path_field("artpath"), env.path("music/a/cover.png"));
    assert_eq!(stored.str("art_source"), "cover_art_url");
}
