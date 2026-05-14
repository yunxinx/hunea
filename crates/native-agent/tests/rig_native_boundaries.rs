use std::{fs, path::Path};

#[test]
fn native_agent_declares_rig_core_runtime_dependency() {
    let workspace = workspace_root();
    let root_manifest = fs::read_to_string(workspace.join("Cargo.toml"))
        .expect("workspace manifest should be readable");
    let native_manifest = fs::read_to_string(workspace.join("crates/native-agent/Cargo.toml"))
        .expect("native-agent manifest should be readable");

    assert!(
        root_manifest.contains("rig-core"),
        "workspace dependencies should expose rig-core for native-agent"
    );
    assert!(
        native_manifest.contains("rig-core.workspace = true"),
        "native-agent should depend on Rig directly"
    );
}

#[test]
fn rig_types_do_not_leak_outside_native_agent() {
    let workspace = workspace_root();
    let mut offenders = Vec::new();
    for crate_name in ["app", "core", "tui", "acp", "cli", "config"] {
        let src = workspace.join("crates").join(crate_name).join("src");
        collect_matching_rs_files(&src, "rig_core", &mut offenders);
        collect_matching_rs_files(&src, "ToolDyn", &mut offenders);
    }

    assert!(
        offenders.is_empty(),
        "Rig implementation details should stay inside native-agent: {offenders:?}"
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
