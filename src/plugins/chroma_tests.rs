//! beets' `test/plugins/test_chroma.py`, plus tests of what pyacoustid does
//! for the plugin, with a fake AcoustID (and MusicBrainz) on 127.0.0.1 (a
//! free port) and real `fpcalc` runs on generated audio.

use super::*;
use crate::plugins::requests::testserver::{Reply, Seen, Server, serve};
use serde_json::json;
use serde_yaml::Value as Y;
use tempfile::TempDir;

/// Tests that point the plugin at a fake AcoustID run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn use_api(server: &Server) {
    *TEST_API_BASE.lock().unwrap() = Some(format!("{}/v2/", server.base));
}

fn have_tools() -> bool {
    let ok = |c: &str| {
        std::process::Command::new(c)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    ok("fpcalc") && ok("ffmpeg")
}

/// `seconds` of generated audio (a lavfi source) as an MP3.
fn audio(dir: &Path, name: &str, source: &str, tags: &[(&str, &str)]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-loglevel", "error", "-f", "lavfi", "-i", source]);
    for (k, v) in tags {
        cmd.args(["-metadata", &format!("{k}={v}")]);
    }
    assert!(cmd.arg(&path).status().unwrap().success());
    path
}

fn form(seen: &Seen) -> Vec<(String, String)> {
    let body = String::from_utf8(seen.body.clone()).unwrap();
    body.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap();
            let dec = |s: &str| {
                let s = s.replace('+', " ");
                let b = s.as_bytes();
                let mut out = vec![];
                let mut i = 0;
                while i < b.len() {
                    if b[i] == b'%' {
                        out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                        i += 3;
                    } else {
                        out.push(b[i]);
                        i += 1;
                    }
                }
                String::from_utf8(out).unwrap()
            };
            (dec(k), dec(v))
        })
        .collect()
}

struct Env {
    dir: TempDir,
    cfg: Config,
    lib: Library,
}

impl Env {
    fn new(plugins: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        let mut cfg = Config::defaults();
        cfg.config_dir = dir.path().join("cfg");
        cfg.set("directory", Y::String(music.display().to_string()));
        cfg.set("plugins", serde_yaml::from_str(plugins).unwrap());
        cfg.set("musicbrainz.ratelimit", Y::from(1000));
        cfg.set("import.write", Y::Bool(false));
        let lib = Library::open_with(&dir.path().join("l.db"), Some(&music), false).unwrap();
        Env { dir, cfg, lib }
    }

    fn mb(&mut self, server: &Server) {
        let host = server.base.trim_start_matches("http://").to_string();
        self.cfg.set("musicbrainz.host", Y::String(host));
    }

    fn add(&self, path: &Path, fields: &[(&str, &str)]) -> Item {
        let mut it = Item::new_item();
        it.set_path(path);
        for (k, v) in fields {
            it.set_parsed(k, v).unwrap();
        }
        self.lib.add(&mut it).unwrap();
        it
    }

    fn items(&self) -> Vec<Item> {
        query_items(&self.lib, &[], None).unwrap()
    }
}

// ------------------------------------------------- beets' TestChroma

const FINGERPRINT_1: &str = "FP_1";
const FINGERPRINT_1_CLOSE: &str = "FP_1_CLOSE";
const FINGERPRINT_2: &str = "FP_2";

/// beets' mocked `compare_fingerprints`.
fn fake_compare(a: &str, b: &str) -> Result<f64> {
    Ok(if a == b {
        1.0
    } else if a == FINGERPRINT_1_CLOSE && b == FINGERPRINT_1 {
        0.9
    } else {
        0.1
    })
}

fn search_env() -> Env {
    let env = Env::new("[chroma]");
    for (title, fp) in [
        ("TEST_TITLE_1", FINGERPRINT_1),
        ("TEST_TITLE_2", FINGERPRINT_2),
    ] {
        env.add(
            Path::new("/file"),
            &[
                ("length", "30"),
                ("title", title),
                ("acoustid_fingerprint", fp),
            ],
        );
    }
    env
}

fn run_search(env: &Env, fp: &str) -> Vec<String> {
    let opts = SearchOpts {
        search: Some(fp.into()),
        count: 5,
        format: Some("$title".into()),
        ..Default::default()
    };
    chromasearch(&env.cfg, &env.lib, opts, &fake_compare).unwrap()
}

#[test]
fn chroma_search_exact() {
    let env = search_env();
    let out = run_search(&env, FINGERPRINT_2);
    assert_eq!(out.len(), 1);
    assert!(out[0].contains("TEST_TITLE_2"), "{out:?}");
    assert_eq!(out[0], "Found exact match: TEST_TITLE_2");
    let out = run_search(&env, FINGERPRINT_1);
    assert_eq!(out.len(), 1);
    assert!(out[0].contains("TEST_TITLE_1"), "{out:?}");
}

#[test]
fn chroma_search_close() {
    let env = search_env();
    let out = run_search(&env, FINGERPRINT_1_CLOSE);
    assert_eq!(out, ["[ 90.0%] TEST_TITLE_1", "[ 10.0%] TEST_TITLE_2"]);
}

#[test]
fn chroma_search_options() {
    let env = search_env();
    let run = |opts: SearchOpts| chromasearch(&env.cfg, &env.lib, opts, &fake_compare);
    let err = run(SearchOpts {
        count: 5,
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(err.to_string(), "no --search provided");
    let err = run(SearchOpts {
        search: Some("x".into()),
        count: 0,
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(err.to_string(), "--count must be > 0");
    // --full keeps going past an exact match; -c caps the list.
    let out = run(SearchOpts {
        search: Some(FINGERPRINT_1.into()),
        count: 1,
        full: true,
        path: true,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(out, ["[100.0%] /file"]);
}

#[test]
fn top_n_keeps_the_best_in_heapq_order() {
    let mut top = TopN::new(3);
    for (s, n) in [
        (0.2, "a"),
        (0.5, "b"),
        (0.2, "c"),
        (0.9, "d"),
        (0.5, "e"),
        (0.1, "f"),
    ] {
        top.add(s, n.into());
    }
    let got: Vec<(f64, String)> = top.sorted();
    assert_eq!(
        got,
        [(0.9, "d".into()), (0.5, "b".into()), (0.5, "e".into())]
    );
}

#[test]
fn scored_items_show_rounded_percentages() {
    assert_eq!(scored_item(0.123456, "x"), "[12.35%] x");
    assert_eq!(scored_item(1.0, "x"), "[100.0%] x");
    assert_eq!(scored_item(0.05, "x"), "[  5.0%] x");
}

// ------------------------------------------------------ fingerprints

/// `fpcalc`'s fingerprint for a file, compressed and raw.
fn fpcalc(path: &Path, raw: bool) -> String {
    let mut cmd = std::process::Command::new("fpcalc");
    if raw {
        cmd.arg("-raw");
    }
    let out = cmd.arg(path).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    text.lines()
        .find_map(|l| l.strip_prefix("FINGERPRINT="))
        .unwrap()
        .to_string()
}

#[test]
fn fingerprints_decode_as_chromaprint() {
    if !have_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let a = audio(dir.path(), "a.mp3", "sine=f=440:d=12", &[]);
    let b = audio(dir.path(), "b.mp3", "anoisesrc=d=12:seed=7", &[]);
    for p in [&a, &b] {
        let raw: Vec<u32> = fpcalc(p, true)
            .split(',')
            .map(|v| v.parse::<i64>().unwrap() as u32)
            .collect();
        let decoded = decode_fingerprint(&fpcalc(p, false)).unwrap();
        assert!(!raw.is_empty());
        assert_eq!(decoded, raw);
    }
    // pyacoustid's fingerprint_file (fpcalc -length 120).
    let (dur, fp_a) = fingerprint_file(&a).unwrap();
    assert!((dur - 12.0).abs() < 0.5, "{dur}");
    assert_eq!(fp_a, fpcalc(&a, false));
    let fp_b = fingerprint_file(&b).unwrap().1;
    assert_eq!(compare_fingerprints(&fp_a, &fp_a).unwrap(), 1.0);
    let other = compare_fingerprints(&fp_a, &fp_b).unwrap();
    assert!(other < 0.5, "{other}");
    assert!(compare_fingerprints("not a fingerprint!", &fp_a).is_err());
    // Errors, as pyacoustid reports them.
    let err = fingerprint_file(&dir.path().join("missing.mp3")).unwrap_err();
    assert!(err.starts_with("fpcalc exited with status "), "{err}");
}

#[test]
fn match_fingerprints_counts_aligned_frames() {
    let a = [0b1111u32, 0, 0xffff_0000, 7];
    // Same frames shifted by one, with up to two bit errors.
    let b = [0x1234_5678u32, 0b1011, 0b11, 0xffff_0001, 7];
    assert_eq!(match_fingerprints(&a, &b), 1.0);
    assert_eq!(match_fingerprints(&a, &[0x5555_5555]), 0.0);
}

#[test]
fn bytes_repr_as_python() {
    assert_eq!(bytes_repr(Path::new("/a/b.mp3")), "b'/a/b.mp3'");
    assert_eq!(bytes_repr(Path::new("/a/it's.mp3")), "b\"/a/it's.mp3\"");
    assert_eq!(bytes_repr(Path::new("/a/é")), "b'/a/\\xc3\\xa9'");
}

#[test]
fn fingerprint_command_stores_and_reuses_fingerprints() {
    if !have_tools() {
        return;
    }
    let env = Env::new("[chroma]");
    let path = audio(env.dir.path(), "a.mp3", "sine=f=440:d=8", &[]);
    env.add(&path, &[("length", "8.0"), ("title", "t")]);
    env.add(Path::new("/nolength.mp3"), &[("title", "u")]);
    let ((), log) = crate::logging::capture(|| {
        fingerprint_command(&env.cfg, &env.lib, None, &[]).unwrap();
    });
    let p = path.display();
    assert_eq!(
        log,
        [
            format!("chroma: {p}: fingerprinting"),
            "chroma: /nolength.mp3: no duration available".to_string(),
        ]
    );
    let stored = env.items()[0].str("acoustid_fingerprint").to_string();
    assert_eq!(stored, fpcalc(&path, false));
    // Not written: import.write is off.
    let ((), log) = crate::logging::capture(|| {
        fingerprint_command(&env.cfg, &env.lib, Some(1), &[]).unwrap();
    });
    assert_eq!(log, [format!("chroma: {p}: using existing fingerprint")]);
}

// ------------------------------------------------------- lookups

fn release(id: &str, year: Option<i64>, country: Option<&str>) -> J {
    let mut r = json!({"id": id});
    if let Some(y) = year {
        r["date"] = json!({"year": y, "month": 1});
    }
    if let Some(c) = country {
        r["country"] = json!(c);
    }
    r
}

#[test]
fn acoustid_match_records_sorted_releases() {
    if !have_tools() {
        return;
    }
    let _s = serial();
    let server = serve(|_| {
        Reply::json(json!({"status": "ok", "results": [{
            "id": "acoustid-1", "score": 0.9,
            "recordings": [
                {"id": "rec-1", "releases": [
                    release("rel-late", Some(2010), Some("US")),
                    release("rel-nodate", None, Some("GB")),
                ]},
                {"id": "rec-2", "releases": [
                    release("rel-early", Some(1999), Some("DE")),
                    release("rel-late-gb", Some(2010), Some("GB")),
                ]},
                {"id": "rec-3"},
            ],
        }]}))
    });
    use_api(&server);
    let mut env = Env::new("[chroma, musicbrainz]");
    env.cfg.set(
        "match.preferred.countries",
        serde_yaml::from_str("[GB, US]").unwrap(),
    );
    env.cfg.set("match.preferred.original_year", Y::Bool(true));
    let path = audio(env.dir.path(), "a.mp3", "sine=f=330:d=6", &[]);
    let log = PluginLog::new("chroma");
    acoustid_match(&env.cfg, &log, &path);
    let st = state();
    let (recs, rels) = st.matches.get(&path).unwrap();
    assert_eq!(recs, &["rec-1", "rec-2", "rec-3"]);
    assert_eq!(
        rels,
        &["rel-early", "rel-late-gb", "rel-late", "rel-nodate"]
    );
    assert_eq!(st.acoustids[&path], "acoustid-1");
    assert_eq!(st.fingerprints[&path], fpcalc(&path, false));
    drop(st);
    // pyacoustid's lookup request.
    let seen = &server.requests()[0];
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.target, "/v2/lookup");
    let f = form(seen);
    let keys: Vec<&str> = f.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        ["format", "client", "duration", "fingerprint", "meta"]
    );
    assert_eq!(f[1].1, API_KEY);
    assert_eq!(f[2].1, "6");
    assert_eq!(f[4].1, "recordings releases");
}

#[test]
fn acoustid_match_without_a_usable_result() {
    if !have_tools() {
        return;
    }
    let _s = serial();
    let server = serve(|seen| {
        let fp = form(seen);
        let dur = &fp.iter().find(|(k, _)| k == "duration").unwrap().1;
        match dur.as_str() {
            // A low score: the fingerprint is kept, nothing else.
            "3" => Reply::json(json!({"status": "ok", "results": [{"id": "x", "score": 0.2}]})),
            // A matched AcoustID without recordings.
            "4" => Reply::json(json!({"status": "ok", "results": [{"id": "y", "score": 0.8}]})),
            _ => Reply::json(
                json!({"status": "error", "error": {"code": 4, "message": "invalid API key"}}),
            ),
        }
    });
    use_api(&server);
    let env = Env::new("[chroma]");
    let log = PluginLog::new("chroma");
    for d in [3, 4, 5] {
        let path = audio(
            env.dir.path(),
            &format!("{d}.mp3"),
            &format!("sine=f=500:d={d}"),
            &[],
        );
        acoustid_match(&env.cfg, &log, &path);
        let st = state();
        assert!(st.fingerprints.contains_key(&path));
        assert!(!st.matches.contains_key(&path));
        assert_eq!(
            st.acoustids.get(&path).map(String::as_str),
            (d == 4).then_some("y")
        );
    }
    // Fingerprinting failures are errors.
    let missing = env.dir.path().join("missing.mp3");
    let ((), lines) = crate::logging::capture(|| acoustid_match(&env.cfg, &log, &missing));
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].starts_with(&format!(
            "chroma: fingerprinting of b'{}' failed: fpcalc exited with status",
            missing.display()
        )),
        "{lines:?}"
    );
}

/// Seed the match cache as if a lookup had run (beets'
/// `_seed_acoustid_match`).
fn seed(path: &Path) -> Item {
    state().matches.insert(
        path.to_path_buf(),
        (
            vec!["00000000-0000-0000-0001-000000000002".into()],
            vec!["11111111-1111-1111-1111-111111111111".into(); 3],
        ),
    );
    let mut it = Item::new_item();
    it.set_path(path);
    it
}

fn mb_server() -> Server {
    serve(|seen: &Seen| {
        let path = seen.target.split('?').next().unwrap_or_default();
        if let Some(id) = path.strip_prefix("/ws/2/release/") {
            let file = format!(
                "{}/tests/fixtures/mb/release-{id}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            return match std::fs::read_to_string(file) {
                Ok(body) => Reply::json(serde_json::from_str(&body).unwrap()),
                Err(_) => Reply::status(404),
            };
        }
        if let Some(id) = path.strip_prefix("/ws/2/recording/") {
            return Reply::json(json!({"id": id, "title": "Beta", "length": 180000}));
        }
        match path {
            "/ws/2/release" | "/ws/2/release/" => Reply::json(json!({"releases": [], "count": 0})),
            "/ws/2/recording" | "/ws/2/recording/" => {
                Reply::json(json!({"recordings": [], "count": 0}))
            }
            _ => Reply::status(404),
        }
    })
}

#[test]
fn candidates_need_the_musicbrainz_plugin() {
    let env = Env::new("[chroma]");
    let mb = MusicBrainz::new(&env.cfg);
    let item = seed(&env.dir.path().join("x.mp3"));
    assert!(album_candidates(&env.cfg, &mb, std::slice::from_ref(&item)).is_empty());
    assert!(item_candidates(&env.cfg, &mb, &item).is_empty());
}

#[test]
fn candidates_come_from_musicbrainz() {
    let mut env = Env::new("[chroma, musicbrainz]");
    let server = mb_server();
    env.mb(&server);
    let mb = MusicBrainz::new(&env.cfg);
    let item = seed(&env.dir.path().join("y.mp3"));
    let albums = album_candidates(&env.cfg, &mb, std::slice::from_ref(&item));
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].album_id, "11111111-1111-1111-1111-111111111111");
    let tracks = item_candidates(&env.cfg, &mb, &item);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].track_id, "00000000-0000-0000-0001-000000000002");
    // An item without a match has no candidates.
    let mut other = Item::new_item();
    other.set_path(env.dir.path().join("z.mp3"));
    assert!(item_candidates(&env.cfg, &mb, &other).is_empty());
    // A release counts when its hits are more than 60% of the items; as in
    // beets, a release listed three times for one item is three hits.
    let mut items = vec![item, other.clone()];
    assert_eq!(album_candidates(&env.cfg, &mb, &items).len(), 1);
    for n in 0..3 {
        let mut o = other.clone();
        o.set_path(env.dir.path().join(format!("o{n}.mp3")));
        items.push(o);
    }
    assert!(album_candidates(&env.cfg, &mb, &items).is_empty());
}

// --------------------------------------------------------- submit

#[test]
fn submit_sends_chunks_of_fingerprints() {
    let _s = serial();
    let server = serve(|seen| {
        if form(seen).iter().any(|(k, _)| k == "mbid.0") {
            Reply::json(
                json!({"status": "error", "error": {"code": 6, "message": "invalid user API key"}}),
            )
        } else {
            Reply::json(json!({"status": "ok", "submissions": []}))
        }
    });
    use_api(&server);
    let env = Env::new("[chroma]");
    let items = vec![
        env.add(
            Path::new("/a.mp3"),
            &[
                ("length", "61.9"),
                ("acoustid_fingerprint", "FPA"),
                ("title", "T & U"),
                ("artist", "A"),
                ("year", "2001"),
                ("track", "3"),
            ],
        ),
        env.add(
            Path::new("/b.mp3"),
            &[
                ("length", "30"),
                ("acoustid_fingerprint", "FPB"),
                ("mb_trackid", "mbid-b"),
            ],
        ),
    ];
    let log = PluginLog::new("chroma");
    let ((), lines) = crate::logging::capture(|| {
        submit_items(&log, &env.cfg, Some(&env.lib), "userkey", items, 1);
    });
    assert_eq!(
        lines,
        [
            "chroma: /a.mp3: using existing fingerprint",
            "chroma: submitting 1 fingerprints",
            "chroma: /b.mp3: using existing fingerprint",
            "chroma: submitting 1 fingerprints",
            "chroma: acoustid submission error: error 6: invalid user API key",
        ]
    );
    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].target, "/v2/submit");
    let pairs = |i: usize| -> Vec<(String, String)> { form(&reqs[i]) };
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        pairs(0),
        [
            s("format", "json"),
            s("client", API_KEY),
            s("user", "userkey"),
            s("duration.0", "61"),
            s("fingerprint.0", "FPA"),
            s("track.0", "T & U"),
            s("artist.0", "A"),
            s("album.0", ""),
            s("albumartist.0", ""),
            s("year.0", "2001"),
            s("trackno.0", "3"),
            s("discno.0", "0"),
        ]
    );
    assert_eq!(
        pairs(1)[3..],
        [
            s("duration.0", "30"),
            s("fingerprint.0", "FPB"),
            s("mbid.0", "mbid-b")
        ]
    );
}

#[test]
fn submit_needs_a_user_api_key() {
    let env = Env::new("[chroma]");
    let err = submit_command(&env.cfg, &env.lib, None, &[]).unwrap_err();
    assert_eq!(err.to_string(), "no Acoustid user API key provided");
}

// --------------------------------------------------------- import

/// With `auto`, importing fingerprints the files and looks them up; the
/// releases AcoustID finds are candidates, and applying one stores the
/// fingerprints and AcoustIDs.
#[test]
fn import_finds_acoustid_candidates_and_stores_ids() {
    if !have_tools() {
        return;
    }
    let _s = serial();
    let acoustid = serve(|seen| {
        let dur = form(seen)
            .into_iter()
            .find(|(k, _)| k == "duration")
            .unwrap()
            .1;
        Reply::json(json!({"status": "ok", "results": [{
            "id": format!("acoustid-{dur}"), "score": 0.95,
            "recordings": [{"id": "rec", "releases": [{"id": "11111111-1111-1111-1111-111111111111"}]}],
        }]}))
    });
    use_api(&acoustid);
    let mb = mb_server();
    let mut env = Env::new("[chroma, musicbrainz]");
    env.mb(&mb);
    // The generated files are much shorter than the release's tracks:
    // accept any match.
    env.cfg.set("match.strong_rec_thresh", Y::from(1.0));
    env.cfg.set(
        "match.max_rec",
        serde_yaml::from_str("{missing_tracks: strong, unmatched_tracks: strong}").unwrap(),
    );
    env.cfg.set("import.quiet", Y::Bool(true));
    env.cfg.set("import.copy", Y::Bool(true));
    env.cfg.set("import.write", Y::Bool(false));
    let src = env.dir.path().join("src");
    for (n, (title, d)) in [("Alpha", 4), ("Beta", 5), ("Gamma", 6)].iter().enumerate() {
        let track = (n + 1).to_string();
        audio(
            &src.join("album"),
            &format!("{track}.mp3"),
            &format!("sine=f={}:d={d}", 300 + 100 * n),
            &[("title", title), ("track", &track)],
        );
    }
    let plugins = super::super::load(&env.cfg).unwrap();
    let opts = crate::importer::ImportOptions::from_config(&env.cfg);
    let mut imp = crate::importer::Importer::new(&env.cfg, &env.lib, opts, &plugins).unwrap();
    imp.run(std::slice::from_ref(&src)).unwrap();
    let items = env.items();
    assert_eq!(items.len(), 3);
    for it in &items {
        assert_eq!(it.str("album"), "Fixture Album");
        assert_eq!(it.str("mb_albumid"), "11111111-1111-1111-1111-111111111111");
        // Each file's own lookup (by its duration: Alpha 4 s, Beta 5 s...).
        let dur = match it.str("title") {
            "Alpha" => 4,
            "Beta" => 5,
            _ => 6,
        };
        assert_eq!(it.str("acoustid_id"), format!("acoustid-{dur}"));
        assert!(!it.str("acoustid_fingerprint").is_empty());
    }
    assert_eq!(acoustid.requests().len(), 3);
}
