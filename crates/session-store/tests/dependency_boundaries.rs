use std::path::Path;

#[test]
fn session_store_keeps_persistence_dependencies_isolated() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = dependency_section(manifest);

    for crate_name in ["app-config", "terminal-app", "terminal-ui"] {
        assert!(
            !dependencies.contains(crate_name),
            "session-store should not depend on {crate_name}"
        );
    }

    for dependency in [
        "provider-protocol",
        "rusqlite",
        "serde",
        "serde_json",
        "thiserror",
        "tokio",
        "tracing",
        "uuid",
    ] {
        assert!(
            dependencies.contains(dependency),
            "session-store should declare {dependency}"
        );
    }
}

#[test]
fn conversation_runtime_depends_on_session_store_without_frontend_leak() {
    let workspace_root = workspace_root();
    let manifest =
        std::fs::read_to_string(workspace_root.join("crates/conversation-runtime/Cargo.toml"))
            .expect("conversation-runtime manifest should be readable");
    let dependencies = dependency_section(&manifest);

    assert!(
        dependencies.contains("session-store.workspace = true"),
        "conversation-runtime should depend on session-store"
    );
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("session-store should live under crates/session-store")
}

fn dependency_section(manifest: &str) -> &str {
    manifest
        .split_once("[dependencies]")
        .map(|(_, rest)| {
            rest.split_once("\n[")
                .map(|(section, _)| section)
                .unwrap_or(rest)
        })
        .expect("Cargo.toml should contain a dependencies section")
}
