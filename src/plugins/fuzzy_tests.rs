use super::*;

#[test]
fn fuzzy_matches_like_beets() {
    assert!(matches("foo", "seafood"));
    assert!(matches("seafood", "seafood"));
    assert!(!matches("zzz", "seafood"));
    assert!(matches("foo", "SEAFOOD"));
    assert!(!matches("Foo", "seafood"));
}
