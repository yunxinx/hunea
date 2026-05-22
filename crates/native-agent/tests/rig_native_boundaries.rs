use std::{fs, path::Path};

#[test]
fn native_agent_declares_rig_core_runtime_dependency() {
    let workspace = workspace_root();
    let root_manifest = fs::read_to_string(workspace.join("Cargo.toml"))
        .expect("workspace manifest should be readable");
    let native_manifest = fs::read_to_string(workspace.join("crates/native-agent/Cargo.toml"))
        .expect("native-agent manifest should be readable");
    let tools_manifest = fs::read_to_string(workspace.join("crates/tools/Cargo.toml"))
        .expect("tools manifest should be readable");

    assert!(
        root_manifest.contains("rig-core"),
        "workspace dependencies should expose rig-core for native-agent"
    );
    assert!(
        native_manifest.contains("rig-core.workspace = true"),
        "native-agent should depend on Rig directly"
    );
    assert!(
        tools_manifest.contains("rig-core.workspace = true"),
        "tools crate should own the Rig tool-server adapter"
    );
}

#[test]
fn rig_types_do_not_leak_outside_tools_and_native_agent() {
    let workspace = workspace_root();
    let mut offenders = Vec::new();
    for crate_name in ["app", "core", "tui", "acp", "cli", "config"] {
        let src = workspace.join("crates").join(crate_name).join("src");
        collect_matching_rs_files(&src, "rig_core", &mut offenders);
        collect_matching_rs_files(&src, "ToolDyn", &mut offenders);
    }

    assert!(
        offenders.is_empty(),
        "Rig implementation details should stay inside tools/native-agent: {offenders:?}"
    );
}

#[test]
fn native_agent_tool_loop_uses_rig_tool_server_surface() {
    let workspace = workspace_root();
    let llm_dir = workspace.join("crates/native-agent/src/llm");
    let stream =
        fs::read_to_string(llm_dir.join("stream.rs")).expect("stream.rs should be readable");
    let tools = fs::read_to_string(llm_dir.join("tools.rs")).expect("tools.rs should be readable");
    let combined = format!("{stream}\n{tools}");

    assert!(
        combined.contains("RigToolServer::from_executor"),
        "native-agent should get the Rig tool server from mo-tools"
    );
    assert!(
        combined.contains(".tool_server_handle("),
        "native-agent should attach tools through AgentBuilder::tool_server_handle"
    );
    assert!(
        combined.contains(".multi_turn(max_turns)"),
        "native-agent should use Rig multi_turn with runtime policy"
    );
    for legacy in [
        "ToolServer::new().run()",
        "impl ToolDyn",
        "MAX_AGENT_TOOL_ROUNDS",
        "RigToolContext",
        "build_rig_tools_for_request",
        "build_rig_tool_context",
        "RigToolExecutionState",
        "pending_calls",
        "completed_results",
        "fallback_call_counter",
    ] {
        assert!(
            !combined.contains(legacy),
            "native-agent still contains legacy tool orchestration artifact {legacy}"
        );
    }
}

#[test]
fn tools_crate_owns_rig_tool_server_adapter() {
    let workspace = workspace_root();
    let rig_tools = fs::read_to_string(workspace.join("crates/tools/src/rig.rs"))
        .expect("tools Rig adapter should be readable");

    assert!(
        rig_tools.contains("ToolServer::new().run()"),
        "mo-tools should create Rig ToolServerHandle"
    );
    assert!(
        rig_tools.contains("impl ToolDyn for RigToolAdapter"),
        "mo-tools should adapt Lumos tools into Rig ToolDyn"
    );
    assert!(
        rig_tools.contains("add_tool") && rig_tools.contains("remove_tool"),
        "mo-tools should expose Rig dynamic tool lifecycle"
    );
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("native-agent should live under crates/native-agent")
}

fn collect_matching_rs_files(root: &Path, needle: &str, offenders: &mut Vec<String>) {
    for entry in fs::read_dir(root).expect("source directory should be readable") {
        let entry = entry.expect("source entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_matching_rs_files(&path, needle, offenders);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).expect("source file should be readable");
        if content.contains(needle) {
            offenders.push(path.display().to_string());
        }
    }
}
