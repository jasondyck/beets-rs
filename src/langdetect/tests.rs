//! The langdetect port against expected values from langdetect 1.0.9 itself
//! (`tools/gen-langdetect-corpus.py`, with `DetectorFactory.seed = 0`).

use super::detector::{Detector, SEED, detect};
use super::python_random::PyRandom;
use serde_json::Value;

fn corpus() -> Value {
    let path = format!(
        "{}/tests/fixtures/langdetect/corpus.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let data = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&data).unwrap()
}

#[test]
fn python_random_stream() {
    let data = corpus();
    let rng = &data["rng"];

    let mut random = PyRandom::new();
    random.seed(SEED);
    for expected in rng["random"].as_array().unwrap() {
        assert_eq!(random.random(), expected.as_f64().unwrap());
    }

    let mut random = PyRandom::new();
    random.seed(SEED);
    for expected in rng["gauss"].as_array().unwrap() {
        assert_eq!(random.gauss(), expected.as_f64().unwrap());
    }

    let mut random = PyRandom::new();
    random.seed(SEED);
    for (i, n) in [1usize, 2, 3, 5, 8, 13, 21]
        .iter()
        .cycle()
        .take(rng["choice"].as_array().unwrap().len())
        .enumerate()
    {
        let seq: Vec<usize> = (0..*n).collect();
        let expected = rng["choice"][i].as_u64().unwrap() as usize;
        assert_eq!(*random.choice(&seq), expected, "choice {i} of {n}");
    }
}

#[test]
fn detect_matches_langdetect() {
    let data = corpus();
    let texts = data["texts"].as_array().unwrap();
    assert!(!texts.is_empty());
    for case in texts {
        let name = case["name"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let expected = case["language"].as_str();
        assert_eq!(detect(text).as_deref(), expected, "language of {name}");

        let expected_probs = case["probs"].as_array();
        let mut detector = Detector::new();
        detector.append(text);
        let probs = detector.get_probabilities();
        match expected_probs {
            None => assert!(probs.is_none(), "probabilities of {name}"),
            Some(expected) => {
                let probs = probs.unwrap_or_else(|| panic!("probabilities of {name}"));
                assert_eq!(probs.len(), expected.len(), "probability count of {name}");
                for (got, want) in probs.iter().zip(expected) {
                    assert_eq!(got.lang, want[0].as_str().unwrap(), "language of {name}");
                    let want = want[1].as_f64().unwrap();
                    assert!(
                        (got.prob - want).abs() < 1e-9,
                        "probability of {name}: {} vs {want}",
                        got.prob
                    );
                }
            }
        }
    }
}
