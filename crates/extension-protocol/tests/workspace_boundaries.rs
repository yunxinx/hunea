use std::{collections::BTreeSet, fs, path::Path};

#[test]
fn extension_protocol_stays_below_runtime_implementation_layers() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .map(|(_, rest)| rest.split_once("\n[").map_or(rest, |(section, _)| section))
        .expect("extension-protocol should declare dependencies");

    let dependency_names: BTreeSet<String> = dependencies
        .lines()
        .filter_map(|line| {
            let line = line.split_once('#').map_or(line, |(line, _)| line).trim();
            if line.is_empty() {
                return None;
            }
            let name = line.split_once('=').map_or(line, |(name, _)| name).trim();
            Some(name.strip_suffix(".workspace").unwrap_or(name).to_string())
        })
        .collect();
    let allowed = BTreeSet::from([
        "provider-protocol".to_string(),
        "serde".to_string(),
        "serde_json".to_string(),
        "thiserror".to_string(),
    ]);
    assert_eq!(dependency_names, allowed);

    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("extension protocol source should be readable");
    assert!(!source.contains("Command::new"));
    assert!(!source.contains("std::process"));
}
