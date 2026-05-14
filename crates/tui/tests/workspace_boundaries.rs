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

#[test]
fn tui_runner_consumes_runtime_events_not_acp_session_events() {
    let runner = include_str!("../src/runner.rs");

    assert!(
        !runner.contains("AcpSessionEvent"),
        "mo-tui runner should consume ACP activity through RuntimeEvent, not AcpSessionEvent"
    );
    assert!(
        !runner.contains("drain_acp_events"),
        "mo-tui runner should not expose a separate ACP event drain path"
    );
}
