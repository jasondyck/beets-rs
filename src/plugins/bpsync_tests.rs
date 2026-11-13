//! beets' `bpsync` plugin has no tests; it logs its deprecation at load and
//! its command can't work without the (unported) Beatport source.

use super::*;

#[test]
fn load_logs_the_deprecation() {
    let cfg = crate::config::Config::defaults();
    let (_r, lines) = crate::logging::capture(|| load(&cfg));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("The 'bpsync' plugin is deprecated")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("The 'beatport' plugin is deprecated")),
        "{lines:?}"
    );
}

#[test]
fn command_reports_the_missing_source() {
    let cfg = crate::config::Config::defaults();
    let lib = crate::library::Library::in_memory().unwrap();
    let err = command(
        &cfg,
        &lib,
        CmdOpts {
            pretend: false,
            mv: None,
            nowrite: false,
            format: None,
            limit: None,
            query: vec![],
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("beatport"));
}
