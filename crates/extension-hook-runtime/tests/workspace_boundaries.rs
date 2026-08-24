use std::collections::BTreeSet;

#[test]
fn extension_hook_runtime_stays_pipeline_neutral() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .map(|(_, rest)| rest.split_once("\n[").map_or(rest, |(section, _)| section))
        .expect("extension-hook-runtime should declare dependencies");
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
        "provider-protocol".to_string(),
        "tool-runtime".to_string(),
        "thiserror".to_string(),
        "tokio".to_string(),
        "tokio-util".to_string(),
    ]);
    assert_eq!(dependency_names, allowed);

    let source = [
        include_str!("../src/lib.rs"),
        include_str!("../src/error.rs"),
        include_str!("../src/identity.rs"),
        include_str!("../src/payload.rs"),
        include_str!("../src/registry.rs"),
    ]
    .join("\n");
    for forbidden in [
        "terminal_app",
        "terminal_ui",
        "conversation_runtime",
        "session_store",
        "extension_runtime",
        "openai_compat_provider",
    ] {
        assert!(!source.contains(forbidden), "source contains {forbidden}");
    }
}
