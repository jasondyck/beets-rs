use super::*;
use crate::config::Config;

fn cfg(yaml: &str) -> Config {
    let mut c = Config::defaults();
    for (k, v) in serde_yaml::from_str::<serde_yaml::Mapping>(yaml).unwrap() {
        let key = k.as_str().unwrap().to_string();
        c.set(&format!("titlecase.{key}"), v);
    }
    c
}

#[test]
fn basic_titlecase() {
    for (given, want) in [
        ("a", "A"),
        ("PENDULUM", "Pendulum"),
        ("Aaron-carl", "Aaron-Carl"),
        ("LTJ bukem", "LTJ Bukem"),
        ("(original mix)", "(Original Mix)"),
        ("ALL CAPS TITLE", "All Caps Title"),
    ] {
        assert_eq!(Titlecase::new(&cfg("{}")).titlecase(given, ""), want, "{given}");
    }
}

#[test]
fn small_first_last() {
    let t = Titlecase::new(&cfg("small_first_last: yes"));
    assert_eq!(t.titlecase("In a Silent Way", ""), "In a Silent Way");
    let t = Titlecase::new(&cfg("small_first_last: no"));
    assert_eq!(t.titlecase("In a Silent Way", ""), "in a Silent Way");
}

#[test]
fn preserve() {
    let t = Titlecase::new(&cfg(
        "{preserve: [easyFun, A.D.O.R, D'Angelo, ABBA, LaTeX, O.R.B] }",
    ));
    for w in ["easyFun", "A.D.O.R", "D'Angelo", "ABBA", "LaTeX", "O.R.B"] {
        assert_eq!(t.titlecase(&w.to_uppercase(), ""), w);
        assert_eq!(t.titlecase(&w.to_lowercase(), ""), w);
    }
}

#[test]
fn separators() {
    let c = cfg("{separators: ['/', ';', '|']}");
    let t = Titlecase::new(&c);
    assert_eq!(t.titlecase("it ; a / in | of / to | the", ""), "It ; A / In | Of / To | The");
}

#[test]
fn flags() {
    let t = Titlecase::new(&cfg("all_caps: yes"));
    assert_eq!(t.titlecase("P A R I S!", ""), "P A R I S!");
    assert_eq!(t.titlecase("RBMK1000", ""), "RBMK1000");
    let t = Titlecase::new(&cfg("all_lowercase: yes"));
    assert_eq!(t.titlecase("pillow dub...", ""), "pillow dub...");
    let t = Titlecase::new(&cfg("{all_lowercase: yes, force_lowercase: yes}"));
    assert_eq!(t.titlecase("OPHIDIAN", ""), "Ophidian");
}

/// Cases where a naive titlecaser differs from python-titlecase: names
/// with apostrophes, slash- and hyphen-separated words, punctuation.
#[test]
fn punctuation_and_names() {
    let t = Titlecase::new(&cfg("{}"));
    for (given, want) in [
        ("d'angelo and the vanguard", "d'Angelo and the Vanguard"),
        ("l'arc", "l'Arc"),
        ("o'reily", "O'Reily"),
        ("a/b/c", "A/B/C"),
        ("AC//DC", "Ac//dc"),
        ("What Goes Around…/…Comes Around (Interlude)", "What Goes Around…/…Comes Around (Interlude)"),
        ("McCartney", "McCartney"),
        ("mc hammer", "Mc Hammer"),
        ("MACBETH", "Macbeth"),
        ("Dr. Dre", "Dr. Dre"),
        ("mr. brightside", "Mr. Brightside"),
        ("U.S.A.", "U.S.A."),
        ("RBMK-1000", "RBMK-1000"),
        ("don't stop me now", "Don't Stop Me Now"),
    ] {
        assert_eq!(t.titlecase(given, ""), want, "{given}");
    }
}

#[test]
fn replace_pairs() {
    // The two-element list form and the flat (pairs of strings) form.
    let t = Titlecase::new(&cfg(r#"replace: [["…", "..."]]"#));
    assert_eq!(t.titlecase("a…b", ""), "A...b");
    let t = Titlecase::new(&cfg(r#"replace: [["’", "'"]]"#));
    assert_eq!(t.titlecase("don’t", ""), "Don't");
    // A plain string replaces it with the empty string (confuse's Pairs).
    let t = Titlecase::new(&cfg(r#"replace: ["’"]"#));
    assert_eq!(t.titlecase("don’t", ""), "Dont");
}
