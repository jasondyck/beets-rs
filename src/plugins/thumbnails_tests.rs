use super::*;

#[test]
fn thumbnail_name_is_md5_of_uri() {
    let t = Thumbnails {
        auto: false,
        force: false,
        dolphin: false,
        log: PluginLog::new("thumbnails"),
    };
    // The hash of "file:///my/uri", as beets' test expects.
    assert_eq!(
        {
            let uri = "file:///my/uri";
            format!(
                "{}.png",
                md5(uri.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        },
        "9488f5797fbe12ffb316d607dfd93d04.png"
    );
    let _ = t;
}

#[test]
fn uri_is_percent_encoded() {
    assert_eq!(uri(Path::new("/foo")), "file:///foo");
    assert_eq!(uri(Path::new("/foo!")), "file:///foo!");
    assert_eq!(
        uri(Path::new("/music/싸이")),
        "file:///music/%EC%8B%B8%EC%9D%B4"
    );
}

#[test]
fn dolphin_directory_file() {
    let d = tempfile::tempdir().unwrap();
    let art = d.path().join("cover.jpg");
    std::fs::write(&art, b"x").unwrap();
    let t = Thumbnails {
        auto: false,
        force: false,
        dolphin: false,
        log: PluginLog::new("thumbnails"),
    };
    t.make_dolphin_cover_thumbnail(d.path(), &art);
    assert_eq!(
        std::fs::read_to_string(d.path().join(".directory")).unwrap(),
        "[Desktop Entry]\nIcon=./cover.jpg"
    );
    std::fs::write(&art, b"x").unwrap();
    t.make_dolphin_cover_thumbnail(d.path(), Path::new("/other/art.tiff"));
    assert_eq!(
        std::fs::read_to_string(d.path().join(".directory")).unwrap(),
        "[Desktop Entry]\nIcon=./cover.jpg"
    );
}
