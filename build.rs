//! Records the compiler version for `beet version`, and discovers the
//! plugin modules: adding a file under `src/plugins/` (or a directory with a
//! `mod.rs`) is enough, no central list to edit.

use std::path::Path;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().nth(1).map(String::from))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=BEETS_RS_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");

    // Discover the plugin modules. A module is a file `src/plugins/X.rs`
    // (not a `*_tests.rs`, not `mod.rs`) or a package `src/plugins/X/mod.rs`.
    // The list goes in OUT_DIR as `#[path = "<absolute>"] pub mod X;` lines
    // (an `include!`d file resolves plain `mod X;` relative to OUT_DIR, and
    // relative child module paths are then relative to `src/plugins/`).
    let dir = Path::new("src/plugins");
    println!("cargo:rerun-if-changed={}", dir.display());
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut names: Vec<(String, String)> = vec![];
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let stem = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if path.join("mod.rs").is_file() {
                    names.push((
                        stem.clone(),
                        format!("{manifest}/src/plugins/{stem}/mod.rs"),
                    ));
                }
            } else if let Some(name) = stem.strip_suffix(".rs") {
                if name == "mod" || name.ends_with("_tests") {
                    continue;
                }
                names.push((name.to_string(), format!("{manifest}/src/plugins/{stem}")));
            }
        }
    }
    names.sort();
    let mut out = String::new();
    for (name, file) in &names {
        out.push_str(&format!("#[path = {file:?}]\npub mod {name};\n"));
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("plugin_modules.rs");
    if std::fs::read_to_string(&dest).ok().as_deref() != Some(out.as_str()) {
        std::fs::write(&dest, out).unwrap();
    }
}
