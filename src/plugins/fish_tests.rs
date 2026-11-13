use super::*;
use crate::library::Library;

#[test]
fn writes_completions() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("completions/beet.fish");
    let cfg = Config::defaults();
    let lib = Library::in_memory().unwrap();
    command(
        &cfg,
        &lib,
        CmdOpts {
            no_fields: false,
            extra_values: vec![],
            output: out.to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("__fish_beet_needs_command"));
    assert!(text.contains("set CMDS "));
    assert!(text.contains("set FIELDS "));
    assert!(text.contains("completions for ls"));
}
