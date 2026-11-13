use super::*;

#[test]
fn bare_ascii_matches_and_is_case_insensitive() {
    assert!(matches("cafe", "Café"));
    assert!(matches("Cafe", "Café"));
    assert!(matches("bjork", "Björk"));
    assert!(!matches("zzz", "Björk"));
}
