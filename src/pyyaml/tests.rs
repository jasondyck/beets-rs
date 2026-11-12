//! Against PyYAML's own output (tests/fixtures/pyyaml/cases.json, made by
//! gen.py there with the reference venv).

use super::*;
use serde_json::Value as J;

fn dec(v: &J) -> PyVal {
    let (k, x) = v.as_object().unwrap().iter().next().unwrap();
    match k.as_str() {
        "none" => PyVal::None,
        "bool" => PyVal::Bool(x.as_bool().unwrap()),
        "int" => PyVal::Int(x.as_str().unwrap().parse().unwrap()),
        "float" => PyVal::Float(match x.as_str().unwrap() {
            "inf" => f64::INFINITY,
            "-inf" => f64::NEG_INFINITY,
            "nan" => f64::NAN,
            f => f.parse().unwrap(),
        }),
        "str" => PyVal::Str(x.as_str().unwrap().to_string()),
        "ts" => PyVal::Timestamp(x.as_str().unwrap().to_string()),
        "bytes" => PyVal::Bytes(
            (0..x.as_str().unwrap().len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&x.as_str().unwrap()[i..i + 2], 16).unwrap())
                .collect(),
        ),
        "list" => PyVal::List(x.as_array().unwrap().iter().map(dec).collect()),
        "map" => PyVal::Map(
            x.as_array()
                .unwrap()
                .iter()
                .map(|p| (dec(&p[0]), dec(&p[1])))
                .collect(),
        ),
        other => panic!("{other}"),
    }
}

fn cases() -> J {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/pyyaml/cases.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn dump_like_pyyaml() {
    let mut bad = vec![];
    for c in cases()["dump"].as_array().unwrap() {
        let docs: Vec<PyVal> = c[0].as_array().unwrap().iter().map(dec).collect();
        // Known: PyYAML's emitter takes U+0085 for a line break (single
        // quotes), libyaml's escapes it ("\N").
        if c[0].to_string().contains('\u{85}') {
            continue;
        }
        let want = c[1].as_str().unwrap();
        let got = dump_all(&docs);
        if got != want {
            bad.push(format!("{docs:?}\n want {want:?}\n  got {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn load_like_pyyaml() {
    let mut bad = vec![];
    for c in cases()["load"].as_array().unwrap() {
        let input = c[0].as_str().unwrap();
        // Known: PyYAML's scanner rejects a tab inside a plain scalar,
        // libyaml's takes it.
        if input.contains('\t') {
            continue;
        }
        let got = load_all(input);
        let ok = match (&got, c[1].get("docs"), c[1].get("error")) {
            (Ok(docs), Some(want), _) => {
                let want: Vec<PyVal> = want.as_array().unwrap().iter().map(dec).collect();
                docs.len() == want.len()
                    && docs
                        .iter()
                        .zip(&want)
                        .all(|(a, b)| a == b || a.py_repr() == b.py_repr())
            }
            (Err(e), _, Some(want)) => e == want.as_str().unwrap(),
            _ => false,
        };
        if !ok {
            bad.push(format!("{input:?}\n want {}\n  got {got:?}", c[1]));
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}
