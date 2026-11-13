//! Tests for `loadext` and `Library::load_extension`, with a tiny SQLite
//! extension compiled by the system C compiler (no SQLite headers needed:
//! the extension's `init` only has to exist).

use super::*;
use crate::events::Arg;

/// Compile `lib<name>.so` implementing SQLite's entry point for it
/// (SQLite derives `sqlite3_<name without non-alphanumerics>_init`).
/// None when there is no C compiler.
fn build_extension(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let entry = format!(
        "sqlite3_{}_init",
        name.trim_start_matches("lib")
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
    );
    let source = format!(
        "typedef struct sqlite3 sqlite3;\n\
         typedef struct sqlite3_api_routines sqlite3_api_routines;\n\
         int {entry}(sqlite3 *db, char **pzErrMsg, \
         const sqlite3_api_routines *pApi) {{\n\
             (void)db; (void)pzErrMsg; (void)pApi; return 0;\n\
         }}\n"
    );
    let c = dir.join(format!("{name}.c"));
    std::fs::write(&c, source).unwrap();
    let so = dir.join(format!("{name}.so"));
    let ok = std::process::Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&c)
        .arg("-o")
        .arg(&so)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok.then_some(so)
}

fn config(dir: &std::path::Path, extensions: &[&std::path::Path]) -> Config {
    let mut cfg = Config::defaults();
    cfg.config_dir = dir.to_path_buf();
    cfg.set(
        "loadext",
        serde_yaml::Value::Sequence(
            extensions
                .iter()
                .map(|p| {
                    serde_yaml::Value::String(p.to_string_lossy().into_owned())
                })
                .collect(),
        ),
    );
    cfg
}

#[test]
fn loads_an_extension_when_the_library_opens() {
    let dir = tempfile::tempdir().unwrap();
    let Some(so) = build_extension(dir.path(), "libloadext_test") else {
        eprintln!("no C compiler: skipping");
        return;
    };
    let cfg = config(dir.path(), &[&so]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ((), lines) = crate::logging::capture(|| {
        let _plugin = LoadExt::new(&cfg);
        crate::events::send("library_opened", &[("lib", Arg::Lib(&lib))]);
    });
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn a_path_without_the_suffix_is_resolved_by_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let Some(so) = build_extension(dir.path(), "libloadext_test") else {
        eprintln!("no C compiler: skipping");
        return;
    };
    // beets' docs: no file extension means the platform's, which SQLite
    // adds itself (it appends ".so" on unix when the exact path fails).
    let without = so.with_extension("");
    let cfg = config(dir.path(), &[&without]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ((), lines) = crate::logging::capture(|| {
        let _plugin = LoadExt::new(&cfg);
        crate::events::send("library_opened", &[("lib", Arg::Lib(&lib))]);
    });
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn a_relative_entry_is_relative_to_the_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    let Some(so) = build_extension(dir.path(), "libloadext_test") else {
        eprintln!("no C compiler: skipping");
        return;
    };
    let cfg = config(dir.path(), &[std::path::Path::new(&so.file_name().unwrap())]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ((), lines) = crate::logging::capture(|| {
        let _plugin = LoadExt::new(&cfg);
        crate::events::send("library_opened", &[("lib", Arg::Lib(&lib))]);
    });
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn a_missing_extension_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("libmissing_ext.so");
    let cfg = config(dir.path(), &[&missing]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ((), lines) = crate::logging::capture(|| {
        let _plugin = LoadExt::new(&cfg);
        crate::events::send("library_opened", &[("lib", Arg::Lib(&lib))]);
    });
    let expected = format!(
        "loadext: failed to load extension {}:",
        missing.display()
    );
    assert!(
        lines.iter().any(|l| l.starts_with(&expected)),
        "{lines:?}"
    );
}

#[test]
fn no_extensions_configured_loads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), &[]);
    let lib = crate::library::Library::in_memory().unwrap();
    let ((), lines) = crate::logging::capture(|| {
        let _plugin = LoadExt::new(&cfg);
        crate::events::send("library_opened", &[("lib", Arg::Lib(&lib))]);
    });
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn the_plugin_registry_entry_loads() {
    let cfg = Config::defaults();
    let plugin = registry_make(&cfg).unwrap();
    assert_eq!(plugin.name(), "loadext");
    assert!(crate::events::listening("library_opened"));
}

#[test]
fn sql_load_extension_is_not_authorized_after_loading() {
    let dir = tempfile::tempdir().unwrap();
    let Some(so) = build_extension(dir.path(), "libloadext_test") else {
        eprintln!("no C compiler: skipping");
        return;
    };
    let lib = crate::library::Library::in_memory().unwrap();
    // Extension loading is enabled only for the load: a second load works
    // (the guard restores the previous, disabled state), but SQL's
    // `load_extension()` is not available to later queries.
    lib.load_extension(&so).unwrap();
    lib.load_extension(&so).unwrap();
    let err = lib.query_column("SELECT load_extension('x')").unwrap_err();
    assert!(
        err.to_string().contains("not authorized"),
        "{err}"
    );
}
