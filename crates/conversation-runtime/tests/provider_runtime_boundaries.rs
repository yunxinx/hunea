use std::{fs, path::Path};

#[test]
fn conversation_runtime_uses_provider_runtime_crates() {
    let workspace = workspace_root();
    let root_manifest = fs::read_to_string(workspace.join("Cargo.toml"))
        .expect("workspace manifest should be readable");
    let conversation_manifest =
        fs::read_to_string(workspace.join("crates/conversation-runtime/Cargo.toml"))
            .expect("conversation-runtime manifest should be readable");

    assert!(
        root_manifest.contains("provider-protocol")
            && root_manifest.contains("openai-compat-provider")
            && root_manifest.contains("tool-loop-runtime"),
        "workspace should expose provider runtime crates"
    );
    assert!(
        conversation_manifest.contains("tool-loop-runtime.workspace = true"),
        "conversation-runtime should delegate orchestration to tool-loop-runtime"
    );
    assert!(
        conversation_manifest.contains("openai-compat-provider.workspace = true"),
        "conversation-runtime should use the OpenAI-compatible adapter"
    );
}

#[test]
fn provider_runtime_crates_keep_boundaries_explicit() {
    let workspace = workspace_root();
    let tool_loop_manifest =
        fs::read_to_string(workspace.join("crates/tool-loop-runtime/Cargo.toml"))
            .expect("tool-loop-runtime manifest should be readable");
    let provider_protocol_manifest =
        fs::read_to_string(workspace.join("crates/provider-protocol/Cargo.toml"))
            .expect("provider-protocol manifest should be readable");
    let openai_provider_manifest =
        fs::read_to_string(workspace.join("crates/openai-compat-provider/Cargo.toml"))
            .expect("openai-compat-provider manifest should be readable");

    assert!(
        tool_loop_manifest.contains("provider-protocol.workspace = true")
            && tool_loop_manifest.contains("tool-runtime.workspace = true"),
        "tool-loop-runtime should depend on provider-neutral protocol and tool crates"
    );
    assert!(
        !provider_protocol_manifest.contains("reqwest")
            && !provider_protocol_manifest.contains("tool-runtime")
            && !provider_protocol_manifest.contains("conversation-runtime"),
        "provider-protocol should stay provider-neutral and runtime-independent"
    );
    assert!(
        openai_provider_manifest.contains("provider-protocol.workspace = true"),
        "openai-compat-provider should project the provider protocol into OpenAI-compatible types"
    );
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("conversation-runtime should live under crates/conversation-runtime")
}
