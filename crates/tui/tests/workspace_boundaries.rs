#[test]
fn tui_has_no_runtime_crate_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .and_then(|(_, rest)| rest.split_once("\n[").map(|(section, _)| section))
        .expect("tui Cargo.toml should contain a dependencies section");

    assert!(
        !dependencies.contains("mo-native-agent"),
        "mo-tui should consume runtime events through mo-core, not depend on native runtime implementation crates"
    );
}

#[test]
fn tui_runner_consumes_runtime_events() {
    let runner = include_str!("../src/runner.rs");

    assert!(
        runner.contains("RuntimeEvent"),
        "mo-tui runner should consume runtime events through the shared mo-core session DTOs"
    );
}
