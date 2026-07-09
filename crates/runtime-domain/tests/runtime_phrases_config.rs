use std::fs;

use runtime_domain::paths::DataDirResolution;
use runtime_domain::phrases::{
    DEFAULT_STATUS_PHRASES, StatusPhraseMode, StatusPhraseOrder, load_from_paths,
    load_with_resolution,
};

#[cfg(unix)]
fn process_euid_is_root() -> bool {
    // SAFETY: geteuid 无参数、无内存副作用；仅测试用。
    unsafe { libc::geteuid() == 0 }
}

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
        working_dir.join(".hunea").join("phrases.toml"),
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
        working_dir.join(".hunea").join("phrases.toml"),
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

#[test]
fn load_with_resolution_global_merges_global_and_workspace() {
    let working_dir = temp_test_dir("phrases-resolution-global-merge");
    let global_dir = temp_test_dir("phrases-resolution-global-config");
    fs::write(
        global_dir.join("phrases.toml"),
        r#"mode = "override"
phrases = ["Global phrase"]
"#,
    )
    .expect("global phrases should be written");
    fs::write(
        working_dir.join(".hunea").join("phrases.toml"),
        r#"mode = "override"
phrases = ["Workspace phrase"]
"#,
    )
    .expect("workspace phrases should be written");

    let resolution = DataDirResolution::Global(global_dir);
    let (loaded, _warnings) = load_with_resolution(Some(&working_dir), &resolution)
        .expect("global resolution should load");

    // 工作区后加载，override 模式覆盖全局
    assert_eq!(loaded.phrases, vec!["Workspace phrase"]);
}

#[test]
fn load_with_resolution_portable_skips_global() {
    let working_dir = temp_test_dir("phrases-resolution-portable-skip");
    let global_dir = temp_test_dir("phrases-resolution-portable-config");
    fs::write(
        global_dir.join("phrases.toml"),
        r#"mode = "override"
phrases = ["Global phrase"]
"#,
    )
    .expect("global phrases should be written");
    fs::write(
        working_dir.join(".hunea").join("phrases.toml"),
        r#"mode = "override"
phrases = ["Portable phrase"]
"#,
    )
    .expect("portable phrases should be written");

    let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
    let (loaded, _warnings) = load_with_resolution(Some(&working_dir), &resolution)
        .expect("portable resolution should load");

    assert_eq!(loaded.phrases, vec!["Portable phrase"]);
}

#[test]
fn load_with_resolution_ignores_workspace_root_phrases_toml() {
    let working_dir = temp_test_dir("phrases-resolution-ignore-root");
    fs::write(
        working_dir.join("phrases.toml"),
        r#"mode = "override"
phrases = ["Root phrase"]
"#,
    )
    .expect("workspace-root phrases should be written");
    fs::write(
        working_dir.join(".hunea").join("phrases.toml"),
        r#"mode = "override"
phrases = ["Project phrase"]
"#,
    )
    .expect("project phrases should be written");

    let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
    let (loaded, _warnings) = load_with_resolution(Some(&working_dir), &resolution)
        .expect("portable resolution should load");

    assert_eq!(loaded.phrases, vec!["Project phrase"]);
    assert_eq!(
        loaded.source_path.as_deref(),
        Some(working_dir.join(".hunea").join("phrases.toml").as_path())
    );
}

#[cfg(unix)]
#[test]
fn load_with_resolution_skips_unreadable_global_and_uses_workspace() {
    if process_euid_is_root() {
        eprintln!("skipping permission test under root");
        return;
    }

    let working_dir = temp_test_dir("phrases-resolution-skip-read");
    let global_dir = temp_test_dir("phrases-resolution-skip-config");
    fs::write(
        global_dir.join("phrases.toml"),
        r#"mode = "override"
phrases = ["Global phrase"]
"#,
    )
    .expect("global phrases should be written");
    fs::write(
        working_dir.join(".hunea").join("phrases.toml"),
        r#"mode = "override"
phrases = ["Workspace phrase"]
"#,
    )
    .expect("workspace phrases should be written");

    use std::os::unix::fs::PermissionsExt;
    let unreadable_path = global_dir.join("phrases.toml");
    fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o000))
        .expect("chmod should work");

    let resolution = DataDirResolution::Global(global_dir);
    let (loaded, warnings) = load_with_resolution(Some(&working_dir), &resolution)
        .expect("should skip unreadable global and load workspace");

    // 恢复权限以便 tempdir 清理
    let _ = fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o644));

    assert_eq!(loaded.phrases, vec!["Workspace phrase"]);
    assert_eq!(
        warnings.len(),
        1,
        "unreadable global should surface as warning"
    );
}

#[cfg(unix)]
#[test]
fn load_with_resolution_all_sources_unreadable_uses_defaults_with_warnings() {
    if process_euid_is_root() {
        eprintln!("skipping permission test under root");
        return;
    }

    let working_dir = temp_test_dir("phrases-resolution-all-unreadable");
    let global_dir = temp_test_dir("phrases-resolution-all-unreadable-global");
    fs::write(
        global_dir.join("phrases.toml"),
        r#"mode = "override"
phrases = ["Global phrase"]
"#,
    )
    .expect("global phrases should be written");
    fs::write(
        working_dir.join(".hunea").join("phrases.toml"),
        r#"mode = "override"
phrases = ["Workspace phrase"]
"#,
    )
    .expect("workspace phrases should be written");

    use std::os::unix::fs::PermissionsExt;
    let global_path = global_dir.join("phrases.toml");
    let workspace_path = working_dir.join(".hunea").join("phrases.toml");
    fs::set_permissions(&global_path, fs::Permissions::from_mode(0o000))
        .expect("chmod should work");
    fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o000))
        .expect("chmod should work");

    let resolution = DataDirResolution::Global(global_dir);
    let (loaded, warnings) = load_with_resolution(Some(&working_dir), &resolution)
        .expect("unreadable files should fall back to defaults");

    let _ = fs::set_permissions(&global_path, fs::Permissions::from_mode(0o644));
    let _ = fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o644));

    assert_eq!(loaded.phrases, DEFAULT_STATUS_PHRASES);
    assert_eq!(warnings.len(), 2, "expected two warnings: {warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| matches!(w, runtime_domain::phrases::PhrasesConfigError::Read { .. })),
        "expected Read warnings, got: {warnings:?}"
    );
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
        .join("hunea-runtime-phrases-config-tests")
        .join(unique);
    fs::create_dir_all(path.join(".hunea")).expect("temp dir should be created");
    path
}
