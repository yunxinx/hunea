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
    assert!(
        !dependencies.contains("session-store"),
        "terminal-ui should consume message-history DTOs through runtime-domain, not depend on persistence crates"
    );
    assert!(
        !dependencies.contains("terminal-app"),
        "terminal-ui should expose a consumer-owned runtime port, not depend on its concrete app adapter"
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

#[test]
fn tui_runtime_port_hides_concrete_runtime_implementations() {
    let runtime_port = include_str!("../src/runner/runtime_port.rs");
    let crate_root = include_str!("../src/lib.rs");

    assert!(
        runtime_port.contains("pub trait UiRuntimePort"),
        "terminal-ui should own the runtime port consumed by its runner"
    );
    assert!(
        runtime_port.contains("pub use runtime_domain::runtime_wake::RuntimeWake"),
        "terminal-ui should use the domain-owned wake port"
    );
    assert!(
        !runtime_port.contains("pub struct RuntimeWake"),
        "terminal-ui must not define a second wake port"
    );
    for concrete_type in ["ConversationWorker", "ProviderClient", "SessionStore"] {
        assert!(
            !runtime_port.contains(concrete_type),
            "UiRuntimePort must not expose concrete runtime type {concrete_type}"
        );
    }
    assert!(
        !crate_root.contains("LoopEventWaker"),
        "terminal-ui must keep its event-pump waker private and export only RuntimeWake"
    );
}
