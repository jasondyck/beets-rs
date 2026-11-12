//! Against CPython (tests/fixtures/inline/cases.json, made by gen.py there
//! with the reference venv), and ports of beets' test/plugins/test_inline.py.

use super::*;
use serde_json::Value as J;

fn to_v<'a>(k: &str, j: &J) -> V<'a> {
    match j {
        J::Null => V::None,
        J::Bool(b) => V::Bool(*b),
        J::Number(n) if n.is_i64() => V::Int(n.as_i64().unwrap()),
        J::Number(n) => V::Float(n.as_f64().unwrap()),
        J::String(s) if k == "path" => V::Bytes(Rc::new(s.as_bytes().to_vec())),
        J::String(s) => V::str(s.as_str()),
        J::Array(a) => V::list(a.iter().map(|x| to_v("", x)).collect()),
        J::Object(_) => unreachable!(),
    }
}

fn cases() -> J {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/inline/cases.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn globals<'a>(c: &J) -> HashMap<String, V<'a>> {
    c["globals"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), to_v(k, v)))
        .collect()
}

fn outcome(r: PyResult<V>) -> J {
    match r {
        Ok(v) => serde_json::json!({ "repr": v.repr() }),
        Err(e) => serde_json::json!({ "error": [e.kind, e.msg] }),
    }
}

#[test]
fn expressions_like_cpython() {
    let c = cases();
    let mut bad = vec![];
    for case in c["exprs"].as_array().unwrap() {
        let src = case[0].as_str().unwrap();
        let g = globals(&c);
        let got = match parser::parse(src) {
            Ok(Code::Expr(e)) => outcome(eval::Interp::new(&g).eval(&e)),
            other => serde_json::json!({ "parse": format!("{other:?}") }),
        };
        if got != case[1] {
            bad.push(format!("{src}\n  want {}\n   got {got}", case[1]));
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn bodies_like_cpython() {
    let c = cases();
    let mut bad = vec![];
    for case in c["bodies"].as_array().unwrap() {
        let src = case[0].as_str().unwrap();
        let g = globals(&c);
        let got = match parser::parse(src) {
            Ok(Code::Body(b)) => outcome(eval::Interp::new(&g).run_body(&b, V::None)),
            Ok(Code::Expr(e)) => outcome(eval::Interp::new(&g).eval(&e)),
            // Starred assignment targets are valid Python beyond the subset.
            Err(ParseError::Unsupported(u)) if src.contains('*') => {
                assert!(u.starts_with("starred"), "{u}");
                continue;
            }
            Err(e) => serde_json::json!({ "parse": format!("{e:?}") }),
        };
        if got != case[1] {
            bad.push(format!("{src:?}\n  want {}\n   got {got}", case[1]));
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

// ---------------------------------------------- beets' test_inline.py

struct Env {
    lib: Library,
    cfg: Config,
}

impl Env {
    fn new(yaml: &str) -> Env {
        let mut cfg = Config::defaults();
        let y: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        if let serde_yaml::Value::Mapping(m) = y {
            for (k, v) in m {
                cfg.set(k.as_str().unwrap(), v);
            }
        }
        Env {
            lib: Library::in_memory().unwrap(),
            cfg,
        }
    }

    fn env(&self) -> Rc<FormatEnv> {
        Rc::new(FormatEnv::new(&self.cfg).unwrap())
    }

    fn item(&self, fields: &[(&str, Value)]) -> Record {
        let mut it = Record::new_item();
        let n = self
            .lib
            .items(&crate::query::Query::parse::<&str>(Kind::Item, &[]).unwrap())
            .unwrap()
            .len();
        it.set_path(format!("/m/{n}.mp3"));
        it.set_str("title", "Title");
        for (k, v) in fields {
            it.set(k, v.clone());
        }
        self.lib.add(&mut it).unwrap();
        it
    }

    fn album(&self, n: usize) -> Record {
        let mut items: Vec<Record> = (0..n)
            .map(|i| {
                let mut it = Record::new_item();
                it.set_path(format!("/m/a{i}.mp3"));
                it.set_str("album", "Album");
                it.set_int("track", i as i64 + 1);
                it
            })
            .collect();
        self.lib.add_album(&mut items).unwrap()
    }

    fn render(&self, rec: &Record, t: &str) -> String {
        let f = Formatted::with_env(rec, Some(&self.lib), self.env());
        crate::template::Template::parse(t).render(&f)
    }

    /// `compile_inline(code, album, field_name)(obj)`.
    fn call<'s>(&'s self, code: &str, album: bool, name: &str, rec: &Record) -> V<'s> {
        let kind = if album { "album_fields" } else { "item_fields" };
        let mut cfg = self.cfg.clone();
        cfg.set(
            kind,
            serde_yaml::from_str(&format!(
                "{{{name}: {}}}",
                serde_json::to_string(code).unwrap()
            ))
            .unwrap(),
        );
        cfg.set("plugins", serde_yaml::from_str("[inline]").unwrap());
        let env = Rc::new(FormatEnv::new(&cfg).unwrap());
        let inline = env.inline.clone().unwrap();
        let k = if album { Kind::Album } else { Kind::Item };
        let f = inline.field(k, name).unwrap();
        evaluate(f, rec, None, Some(&self.lib), &env).unwrap()
    }
}

fn v(i: i64) -> Value {
    Value::Int(i)
}

#[test]
fn no_recursion_when_inline_shadows_fixed_field() {
    let e = Env::new(
        "{plugins: [inline], item_fields: {track_no: \"f'{disc:02d}-{track:02d}' if disctotal > 1 else f'{track:02d}'\"}}",
    );
    let it = e.item(&[("track", v(1)), ("disc", v(1)), ("disctotal", v(1))]);
    assert_eq!(e.render(&it, "$track_no"), "01");
}

#[test]
fn inline_function_body_item_field() {
    let e = Env::new("{}");
    let it = e.item(&[("track", v(3))]);
    assert_eq!(
        e.call("return track + 1", false, "next_track", &it).repr(),
        "4"
    );
}

#[test]
fn inline_album_expression_uses_items() {
    let e = Env::new("{}");
    let a = e.album(3);
    assert_eq!(e.call("len(items)", true, "item_count", &a).repr(), "3");
}

#[test]
fn inline_album_expression_uses_items_via_obj() {
    let e = Env::new("{}");
    let a = e.album(3);
    assert_eq!(
        e.call("len(db_obj.items())", true, "item_count", &a).repr(),
        "3"
    );
}

#[test]
fn inline_function_body_item_field_via_obj() {
    let e = Env::new("{}");
    let it = e.item(&[("track", v(3))]);
    assert_eq!(
        e.call("return db_obj.track + 1", false, "next_track", &it)
            .repr(),
        "4"
    );
}

#[test]
fn inline_obj_missing() {
    let e = Env::new(
        "{plugins: [inline, missing], album_fields: {has_missing: 'bool(db_obj.missing)'}}",
    );
    let mut a = e.album(1);
    a.set_int("tracktotal", 3);
    e.lib.store(&a).unwrap();
    for mut it in e.lib.album_items(a.id.unwrap()).unwrap() {
        it.set_int("tracktotal", 3);
        e.lib.store(&it).unwrap();
    }
    assert_eq!(e.render(&a, "$has_missing"), "True");
}

#[test]
fn inline_function_body_obj() {
    let e = Env::new("{}");
    let it = e.item(&[("track", v(3))]);
    assert!(
        e.call("return db_obj.title", false, "title_value", &it)
            .truthy()
    );
}

// ---------------------------------------------------- beyond beets' tests

#[test]
fn doc_examples_and_formatting() {
    let e = Env::new(
        "{plugins: [inline], item_fields: {
           initial: \"albumartist[0].upper() + u'.'\",
           disc_and_track: 'f\"{disc:02d}.{track:02d}\" if disctotal > 1 else f\"{track:02d}\"',
           nothing: 'None', listed: 'genres', p: 'path', flag: 'comp', rest: 'db_obj.singleton'},
         album_fields: {bitrate: \"total = 0\\nfor item in items:\\n    total += item.bitrate\\nreturn total / len(items)\"}}",
    );
    let it = e.item(&[
        ("albumartist", Value::Str("beatles".into())),
        ("track", v(7)),
        ("disc", v(2)),
        ("disctotal", v(2)),
    ]);
    assert_eq!(
        e.render(
            &it,
            "$initial|$disc_and_track|$nothing|$listed|$p|$flag|$rest"
        ),
        "B.|02.07||[]|/m/0.mp3|0|True"
    );
    let a = e.album(2);
    for mut it in e.lib.album_items(a.id.unwrap()).unwrap() {
        it.set_int("bitrate", 100_000 * it.int("track"));
        e.lib.store(&it).unwrap();
    }
    assert_eq!(e.render(&a, "$bitrate"), "150000.0");
    // An item's own field comes first (the album's in paths only), as in
    // beets' `FormattedItemMapping`.
    let it = &e.lib.album_items(a.id.unwrap()).unwrap()[0];
    assert_eq!(e.render(it, "$bitrate"), "100kbps");
}

#[test]
fn syntax_errors_and_unsupported_python() {
    let e = Env::new(
        "{plugins: [inline], item_fields: {bad: 'def (', imp: \"import os\\nreturn os.sep\", lam: '(lambda: 1)()'}}",
    );
    let inline = e.env().inline.clone().unwrap();
    // Not Python: left out.
    assert!(inline.field(Kind::Item, "bad").is_none());
    let it = e.item(&[]);
    for (name, what) in [("imp", "import statements"), ("lam", "lambda expressions")] {
        let f = inline.field(Kind::Item, name).unwrap();
        let err = evaluate(f, &it, None, Some(&e.lib), &e.env()).unwrap_err();
        assert_eq!(err.kind, "NotImplementedError");
        assert_eq!(err.msg, format!("beets-rs' inline doesn't support {what}"));
        assert_eq!(
            inline_error(&f.code, &err).lines().next().unwrap(),
            "error in inline path field code:"
        );
    }
}

#[test]
fn the_field_itself_is_not_a_variable() {
    let e = Env::new("{}");
    let it = e.item(&[("track", v(3))]);
    let mut cfg = e.cfg.clone();
    cfg.set("plugins", serde_yaml::from_str("[inline]").unwrap());
    cfg.set(
        "item_fields",
        serde_yaml::from_str("{track: track}").unwrap(),
    );
    let env = Rc::new(FormatEnv::new(&cfg).unwrap());
    let f = env
        .inline
        .as_ref()
        .unwrap()
        .field(Kind::Item, "track")
        .unwrap();
    let err = evaluate(f, &it, None, Some(&e.lib), &env).unwrap_err();
    assert_eq!(
        (err.kind, err.msg.as_str()),
        ("NameError", "name 'track' is not defined")
    );
    // Recursion through db_obj ends.
    cfg.set(
        "item_fields",
        serde_yaml::from_str("{loop: db_obj.loop}").unwrap(),
    );
    let env = Rc::new(FormatEnv::new(&cfg).unwrap());
    let f = env
        .inline
        .as_ref()
        .unwrap()
        .field(Kind::Item, "loop")
        .unwrap();
    assert_eq!(
        evaluate(f, &it, None, Some(&e.lib), &env).unwrap_err().kind,
        "RecursionError"
    );
}
