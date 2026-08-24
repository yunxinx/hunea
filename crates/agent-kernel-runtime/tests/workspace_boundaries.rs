use std::{collections::BTreeSet, fs, path::Path};

#[test]
fn agent_kernel_runtime_stays_below_terminal_and_concrete_host_layers() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .map(|(_, rest)| rest.split_once("\n[").map_or(rest, |(section, _)| section))
        .expect("agent-kernel-runtime should declare dependencies");
    let dependency_names = dependencies
        .lines()
        .filter_map(|line| {
            let line = line.split_once('#').map_or(line, |(line, _)| line).trim();
            if line.is_empty() {
                return None;
            }
            let name = line.split_once('=').map_or(line, |(name, _)| name).trim();
            Some(name.strip_suffix(".workspace").unwrap_or(name).to_string())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        dependency_names,
        BTreeSet::from([
            "agent-kernel-protocol".to_string(),
            "provider-protocol".to_string(),
            "runtime-domain".to_string(),
            "serde_json".to_string(),
            "stdio-framing".to_string(),
            "thiserror".to_string(),
        ])
    );

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("runtime source should be readable");
    for forbidden in [
        "terminal-app",
        "terminal-ui",
        "conversation-runtime",
        "session-store",
        "openai-compat-provider",
        "extension-protocol",
    ] {
        assert!(!source.contains(forbidden), "source contains {forbidden}");
    }
}
