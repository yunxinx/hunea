use std::{collections::BTreeSet, fs, path::Path};

#[test]
fn agent_kernel_protocol_stays_below_runtime_implementation_layers() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .map(|(_, rest)| rest.split_once("\n[").map_or(rest, |(section, _)| section))
        .expect("agent-kernel-protocol should declare dependencies");
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
            "provider-protocol".to_string(),
            "serde".to_string(),
            "serde_json".to_string(),
            "thiserror".to_string(),
        ])
    );

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("protocol source should be readable");
    for forbidden in [
        "std::process",
        "tokio",
        "runtime_domain",
        "terminal_app",
        "extension_protocol",
        "FrameCodec",
    ] {
        assert!(!source.contains(forbidden), "source contains {forbidden}");
    }
}
