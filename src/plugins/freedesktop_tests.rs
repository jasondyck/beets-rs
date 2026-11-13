//! beets' `freedesktop` plugin has no tests: its command just prints the
//! deprecation message.

#[test]
fn message_points_at_thumbnails() {
    assert!(super::MESSAGE[0].contains("superseded by the 'thumbnails' plugin"));
    assert!(super::MESSAGE[1].starts_with("'thumbnails --dolphin' replaces freedesktop"));
}
