use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    appconfig::AcpInstallRoot,
    runtime::acp::install::{
        manifest::{InstallManifest, read_install_manifest, write_install_manifest},
        paths::{InstallPathInputs, resolve_install_paths},
    },
};

#[test]
fn resolve_install_root_config() {
    let inputs = install_inputs();

    let paths = resolve_install_paths(&inputs, AcpInstallRoot::Config, "kimi", "1.39.0")
        .expect("config install root should resolve");

    assert_eq!(
        paths.agent_version_dir,
        PathBuf::from("/tmp/lumos-config/.acpclient/installs/kimi/1.39.0")
    );
    assert_eq!(
        paths.manifest_path,
        PathBuf::from("/tmp/lumos-config/.acpclient/installs/kimi/1.39.0/manifest.toml")
    );
}

#[test]
fn resolve_install_root_project() {
    let inputs = install_inputs();

    let paths = resolve_install_paths(&inputs, AcpInstallRoot::Project, "kimi", "1.39.0")
        .expect("project install root should resolve");

    assert_eq!(
        paths.agent_version_dir,
        PathBuf::from("/tmp/lumos-project/.lumos/.acpclient/installs/kimi/1.39.0")
    );
}

#[test]
fn manifest_roundtrip() {
    let temp_dir = temp_test_dir("install-manifest");
    let manifest_path = temp_dir.join("manifest.toml");
    let mut env = BTreeMap::new();
    env.insert("KIMI_TEST".to_string(), "1".to_string());
    let manifest = InstallManifest {
        agent_id: "kimi".to_string(),
        agent_version: "1.39.0".to_string(),
        archive_url: "https://example.test/kimi.tar.gz".to_string(),
        command: "./kimi".to_string(),
        args: vec!["acp".to_string()],
        env,
        checksum_verified: false,
        installed_at: "2026-04-25T00:00:00Z".to_string(),
    };

    write_install_manifest(&manifest_path, &manifest).expect("manifest should write");
    let loaded = read_install_manifest(&manifest_path).expect("manifest should read");

    assert_eq!(loaded, manifest);
}

fn install_inputs() -> InstallPathInputs {
    InstallPathInputs {
        user_config_dir: PathBuf::from("/tmp/lumos-config"),
        user_data_dir: Some(PathBuf::from("/tmp/lumos-data")),
        user_cache_dir: Some(PathBuf::from("/tmp/lumos-cache")),
        project_dir: Some(PathBuf::from("/tmp/lumos-project")),
        custom_install_dir: Some(PathBuf::from("/tmp/lumos-custom")),
    }
}

fn temp_test_dir(prefix: &str) -> PathBuf {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    );
    let path = std::env::temp_dir().join(format!("lumos-rust-{prefix}-{unique}"));
    std::fs::create_dir_all(&path).expect("temp test dir should be created");
    path
}
