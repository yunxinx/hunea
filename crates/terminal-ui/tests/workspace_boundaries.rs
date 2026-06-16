#[test]
fn tui_has_no_runtime_crate_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .and_then(|(_, rest)| rest.split_once("\n[").map(|(section, _)| section))
        .expect("tui Cargo.toml should contain a dependencies section");

    assert!(
        !dependencies.contains("conversation-runtime"),
        "terminal-ui should consume runtime events through runtime-domain, not depend on conversation runtime implementation crates"
    );
    assert!(
        !dependencies.contains("provider-protocol"),
        "terminal-ui should not depend on provider protocol types for render or composer state"
    );
}

#[test]
fn tui_runner_consumes_runtime_events() {
    let runner = include_str!("../src/runner/mod.rs");

    assert!(
        runner.contains("RuntimeEvent"),
        "terminal-ui runner should consume runtime events through the shared runtime-domain session DTOs"
    );
}
