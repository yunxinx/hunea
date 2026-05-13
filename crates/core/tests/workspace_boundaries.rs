#[test]
fn core_has_no_frontend_or_runtime_crate_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .and_then(|(_, rest)| rest.split_once("\n[").map(|(section, _)| section))
        .expect("core Cargo.toml should contain a dependencies section");

    for crate_name in [
        "agent-client-protocol",
        "mo-acp",
        "mo-native-agent",
        "mo-tui",
    ] {
        assert!(
            !dependencies.contains(crate_name),
            "mo-core should define shared DTOs without depending on {crate_name}"
        );
    }
}
