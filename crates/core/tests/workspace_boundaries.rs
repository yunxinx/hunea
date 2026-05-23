use std::{fs, path::Path};

#[test]
fn workspace_declares_ai_runtime_crates_and_tools_domain() {
    let workspace = workspace_root();
    let root_manifest =
        fs::read_to_string(workspace.join("Cargo.toml")).expect("workspace manifest is readable");
    let agent_runtime_manifest =
        fs::read_to_string(workspace.join("crates/agent-runtime/Cargo.toml"))
            .expect("agent-runtime manifest is readable");
    let app_manifest = fs::read_to_string(workspace.join("crates/app/Cargo.toml"))
        .expect("app manifest is readable");
    let native_manifest = fs::read_to_string(workspace.join("crates/native-agent/Cargo.toml"))
        .expect("native-agent manifest is readable");
    let tools_manifest = fs::read_to_string(workspace.join("crates/tools/Cargo.toml"))
        .expect("tools manifest is readable");

    assert!(
        root_manifest.contains("\"crates/tools\""),
        "workspace should declare crates/tools as the shared tool-domain crate"
    );
    assert!(
        root_manifest.contains("mo-tools = { path = \"crates/tools\" }"),
        "workspace dependencies should expose mo-tools for runtime crates"
    );
    assert!(
        root_manifest.contains("mo-ai-core = { path = \"crates/ai-core\" }")
            && root_manifest.contains("mo-ai-openai = { path = \"crates/ai-openai\" }")
            && root_manifest.contains("mo-agent-runtime = { path = \"crates/agent-runtime\" }"),
        "workspace dependencies should expose Lumos-owned AI crates"
    );
    assert!(
        app_manifest.contains("mo-tools.workspace = true"),
        "mo-app should assemble Lumos tool registries without leaking them into mo-core"
    );
    assert!(
        native_manifest.contains("mo-agent-runtime.workspace = true")
            && native_manifest.contains("mo-ai-openai.workspace = true"),
        "mo-native-agent should call the Lumos agent runtime and OpenAI-compatible adapter"
    );
    assert!(
        agent_runtime_manifest.contains("mo-tools.workspace = true"),
        "mo-agent-runtime should own the tool-loop connection to mo-tools"
    );

    let dependencies = dependency_section(&tools_manifest);
    for forbidden in ["mo-app", "mo-core", "mo-native-agent", "mo-tui", "ratatui"] {
        assert!(
            !dependencies.contains(forbidden),
            "mo-tools should stay independent from {forbidden}"
        );
    }
}

#[test]
fn core_has_no_frontend_or_runtime_crate_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = dependency_section(manifest);

    for crate_name in ["mo-tools", "mo-native-agent", "mo-tui"] {
        assert!(
            !dependencies.contains(crate_name),
            "mo-core should define shared DTOs without depending on {crate_name}"
        );
    }
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core should live under crates/core")
}

fn dependency_section(manifest: &str) -> &str {
    manifest
        .split_once("[dependencies]")
        .and_then(|(_, rest)| rest.split_once("\n[").map(|(section, _)| section))
        .expect("Cargo.toml should contain a dependencies section")
}
