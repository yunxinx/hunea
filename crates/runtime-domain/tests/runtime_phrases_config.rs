use std::fs;

use runtime_domain::phrases::{
    DEFAULT_STATUS_PHRASES, StatusPhraseMode, StatusPhraseOrder, load_from_paths,
};

#[test]
fn phrases_config_defaults_to_builtin_random_phrases() {
    let working_dir = temp_test_dir("missing-phrases-config");

    let loaded = load_from_paths(Some(&working_dir), None).expect("missing config should load");

    assert_eq!(loaded.phrases, DEFAULT_STATUS_PHRASES);
    assert_eq!(loaded.order, StatusPhraseOrder::Random);
    assert_eq!(loaded.source_path, None);
}

#[test]
fn phrases_config_appends_project_phrases_and_uses_cycle_order() {
    let working_dir = temp_test_dir("append-cycle-phrases");
    fs::write(
        working_dir.join("phrases.toml"),
        r#"
mode = "append"
order = "cycle"
phrases = ["Polishing", "Checking"]
"#,
    )
    .expect("phrases config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("phrases config should load");

    assert_eq!(loaded.mode, StatusPhraseMode::Append);
    assert_eq!(loaded.order, StatusPhraseOrder::Cycle);
    assert_eq!(
        loaded.phrases,
        DEFAULT_STATUS_PHRASES
            .iter()
            .copied()
            .chain(["Polishing", "Checking"])
            .map(str::to_string)
            .collect::<Vec<_>>()
    );
}

#[test]
fn phrases_config_overrides_builtin_phrases() {
    let working_dir = temp_test_dir("override-phrases");
    fs::write(
        working_dir.join(".lumos").join("phrases.toml"),
        r#"
mode = "override"
phrases = ["Thinking locally", "", "Reading context"]
"#,
    )
    .expect("phrases config should be written");

    let loaded = load_from_paths(Some(&working_dir), None).expect("phrases config should load");

    assert_eq!(loaded.mode, StatusPhraseMode::Override);
    assert_eq!(loaded.order, StatusPhraseOrder::Random);
    assert_eq!(loaded.phrases, vec!["Thinking locally", "Reading context"]);
}

fn temp_test_dir(name: &str) -> std::path::PathBuf {
    let unique = format!(
        "{}-{}",
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos()
    );
    let path = std::env::temp_dir()
        .join("lumos-runtime-phrases-config-tests")
        .join(unique);
    fs::create_dir_all(path.join(".lumos")).expect("temp dir should be created");
    path
}
