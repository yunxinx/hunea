use std::{fs, path::Path};

#[test]
fn workspace_declares_ai_runtime_crates_and_tools_domain() {
    let workspace = workspace_root();
    let root_manifest =
        fs::read_to_string(workspace.join("Cargo.toml")).expect("workspace manifest is readable");
    let tool_loop_manifest =
        fs::read_to_string(workspace.join("crates/tool-loop-runtime/Cargo.toml"))
            .expect("tool-loop-runtime manifest is readable");
    let app_manifest = fs::read_to_string(workspace.join("crates/terminal-app/Cargo.toml"))
        .expect("terminal-app manifest is readable");
    let conversation_manifest =
        fs::read_to_string(workspace.join("crates/conversation-runtime/Cargo.toml"))
            .expect("conversation-runtime manifest is readable");
    let tools_manifest = fs::read_to_string(workspace.join("crates/tool-runtime/Cargo.toml"))
        .expect("tool-runtime manifest is readable");

    assert!(
        root_manifest.contains("\"crates/tool-runtime\""),
        "workspace should declare crates/tool-runtime as the shared tool-domain crate"
    );
    assert!(
        root_manifest.contains("tool-runtime = { path = \"crates/tool-runtime\" }"),
        "workspace dependencies should expose tool-runtime for runtime crates"
    );
    assert!(
        root_manifest.contains("provider-protocol = { path = \"crates/provider-protocol\" }")
            && root_manifest
                .contains("openai-compat-provider = { path = \"crates/openai-compat-provider\" }")
            && root_manifest
                .contains("tool-loop-runtime = { path = \"crates/tool-loop-runtime\" }"),
        "workspace dependencies should expose provider runtime crates"
    );
    assert!(
        app_manifest.contains("tool-runtime.workspace = true"),
        "terminal-app should assemble tool registries without leaking them into runtime-domain"
    );
    assert!(
        !dependency_section(&app_manifest).contains("provider-protocol"),
        "terminal-app should not depend on provider-protocol outside tests"
    );
    assert!(
        conversation_manifest.contains("tool-loop-runtime.workspace = true")
            && conversation_manifest.contains("openai-compat-provider.workspace = true"),
        "conversation-runtime should depend on the tool loop and OpenAI-compatible adapter"
    );
    assert!(
        tool_loop_manifest.contains("tool-runtime.workspace = true"),
        "tool-loop-runtime should own the tool-loop connection to tool-runtime"
    );

    let dependencies = dependency_section(&tools_manifest);
    for forbidden in [
        "terminal-app",
        "runtime-domain",
        "conversation-runtime",
        "terminal-ui",
        "ratatui",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "tool-runtime should stay independent from {forbidden}"
        );
    }
}

#[test]
fn terminal_app_keeps_provider_protocol_test_only() {
    let workspace = workspace_root();
    let app_manifest = fs::read_to_string(workspace.join("crates/terminal-app/Cargo.toml"))
        .expect("terminal-app manifest is readable");
    let dependencies = dependency_section(&app_manifest);

    assert!(
        !dependencies.contains("provider-protocol"),
        "terminal-app should not depend on provider-protocol at runtime when it is only used by tests"
    );
}

#[test]
fn runtime_domain_has_no_frontend_or_runtime_crate_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = dependency_section(manifest);

    for crate_name in ["tool-runtime", "conversation-runtime", "terminal-ui"] {
        assert!(
            !dependencies.contains(crate_name),
            "runtime-domain should define shared DTOs without depending on {crate_name}"
        );
    }
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("runtime-domain should live under crates/runtime-domain")
}

fn dependency_section(manifest: &str) -> &str {
    manifest
        .split_once("[dependencies]")
        .and_then(|(_, rest)| rest.split_once("\n[").map(|(section, _)| section))
        .expect("Cargo.toml should contain a dependencies section")
}
