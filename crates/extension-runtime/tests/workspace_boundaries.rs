use std::{collections::BTreeSet, fs, path::Path};

#[test]
fn extension_runtime_stays_transport_neutral() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .map(|(_, rest)| rest.split_once("\n[").map_or(rest, |(section, _)| section))
        .expect("extension-runtime should declare dependencies");
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
    let allowed = BTreeSet::from([
        "extension-hook-runtime".to_string(),
        "extension-protocol".to_string(),
        "tool-runtime".to_string(),
        "serde".to_string(),
        "serde_json".to_string(),
        "thiserror".to_string(),
        "tokio".to_string(),
        "tokio-util".to_string(),
    ]);
    assert_eq!(dependency_names, allowed);

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("extension runtime source should be readable");
    for forbidden in [
        "terminal-app",
        "terminal-ui",
        "conversation-runtime",
        "session-store",
        "Command::new",
        "std::process",
    ] {
        assert!(!source.contains(forbidden), "source contains {forbidden}");
    }
}
