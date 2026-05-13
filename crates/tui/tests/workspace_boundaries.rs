#[test]
fn tui_has_no_runtime_crate_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .and_then(|(_, rest)| rest.split_once("\n[").map(|(section, _)| section))
        .expect("tui Cargo.toml should contain a dependencies section");

    for runtime_crate in ["agent-client-protocol", "mo-acp", "mo-native-agent"] {
        assert!(
            !dependencies.contains(runtime_crate),
            "mo-tui should consume runtime events through mo-core, not depend on {runtime_crate}"
        );
    }
}
