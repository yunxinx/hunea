use std::{fs, path::Path};

#[test]
fn native_agent_uses_lumos_ai_crates() {
    let workspace = workspace_root();
    let root_manifest = fs::read_to_string(workspace.join("Cargo.toml"))
        .expect("workspace manifest should be readable");
    let native_manifest = fs::read_to_string(workspace.join("crates/native-agent/Cargo.toml"))
        .expect("native-agent manifest should be readable");

    assert!(
        root_manifest.contains("mo-ai-core")
            && root_manifest.contains("mo-ai-openai")
            && root_manifest.contains("mo-agent-runtime"),
        "workspace should expose Lumos-owned AI crates"
    );
    assert!(
        native_manifest.contains("mo-agent-runtime.workspace = true"),
        "native-agent should delegate orchestration to mo-agent-runtime"
    );
    assert!(
        native_manifest.contains("mo-ai-openai.workspace = true"),
        "native-agent should use the OpenAI-compatible adapter"
    );
}

#[test]
fn native_runtime_crates_keep_ai_boundaries_explicit() {
    let workspace = workspace_root();
    let agent_runtime_manifest =
        fs::read_to_string(workspace.join("crates/agent-runtime/Cargo.toml"))
            .expect("agent-runtime manifest should be readable");
    let ai_core_manifest = fs::read_to_string(workspace.join("crates/ai-core/Cargo.toml"))
        .expect("ai-core manifest should be readable");
    let ai_openai_manifest = fs::read_to_string(workspace.join("crates/ai-openai/Cargo.toml"))
        .expect("ai-openai manifest should be readable");

    assert!(
        agent_runtime_manifest.contains("mo-ai-core.workspace = true")
            && agent_runtime_manifest.contains("mo-tools.workspace = true"),
        "agent-runtime should depend on provider-neutral AI and tool-domain crates"
    );
    assert!(
        !ai_core_manifest.contains("reqwest")
            && !ai_core_manifest.contains("mo-tools")
            && !ai_core_manifest.contains("mo-native-agent"),
        "ai-core should stay provider-neutral and runtime-independent"
    );
    assert!(
        ai_openai_manifest.contains("mo-ai-core.workspace = true"),
        "ai-openai should project OpenAI-compatible protocol into ai-core types"
    );
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("native-agent should live under crates/native-agent")
}
