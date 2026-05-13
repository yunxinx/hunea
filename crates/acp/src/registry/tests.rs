use std::collections::BTreeMap;

use crate::{
    AcpAgentServerConfig, AcpAgentServerType,
    registry::{
        model::RegistryDocument,
        resolve::{
            current_platform_target, resolve_binary_command, resolve_binary_command_for_server,
        },
    },
};

const REGISTRY_JSON: &str = r#"
{
  "version": "1.0.0",
  "agents": [
    {
      "id": "kimi",
      "name": "Kimi CLI",
      "version": "1.39.0",
      "description": "Moonshot AI's coding assistant",
      "repository": "https://github.com/MoonshotAI/kimi-cli",
      "license": "MIT",
      "distribution": {
        "binary": {
          "linux-x86_64": {
            "archive": "https://example.test/kimi.tar.gz",
            "cmd": "./kimi",
            "args": ["acp"],
            "env": { "KIMI_TEST": "1" }
          }
        }
      }
    },
    {
      "id": "package-only",
      "name": "Package Only",
      "version": "0.1.0",
      "description": "No binary distribution",
      "distribution": {
        "npx": {
          "package": "package-only@0.1.0",
          "args": ["--acp"]
        }
      }
    }
  ],
  "extensions": []
}
"#;

#[test]
fn parse_binary_registry_entry() {
    let registry: RegistryDocument = toml_json(REGISTRY_JSON);
    let kimi = registry
        .agents
        .iter()
        .find(|agent| agent.id == "kimi")
        .expect("kimi agent should exist");
    let linux = kimi
        .distribution
        .binary
        .as_ref()
        .and_then(|targets| targets.get("linux-x86_64"))
        .expect("linux x86_64 binary should exist");

    assert_eq!(registry.version, "1.0.0");
    assert_eq!(kimi.name, "Kimi CLI");
    assert_eq!(linux.archive, "https://example.test/kimi.tar.gz");
    assert_eq!(linux.cmd, "./kimi");
    assert_eq!(linux.args, vec!["acp"]);
    assert_eq!(linux.env.get("KIMI_TEST"), Some(&"1".to_string()));
}

#[test]
fn resolve_binary_target_for_current_platform() {
    let registry: RegistryDocument = toml_json(REGISTRY_JSON);
    let resolved = resolve_binary_command(&registry, "kimi").expect("kimi should resolve");

    assert_eq!(current_platform_target(), Some("linux-x86_64"));
    assert_eq!(resolved.agent_id, "kimi");
    assert_eq!(resolved.agent_version, "1.39.0");
    assert_eq!(resolved.archive_url, "https://example.test/kimi.tar.gz");
    assert_eq!(resolved.command, "./kimi");
    assert_eq!(resolved.args, vec!["acp"]);
    assert_eq!(resolved.env.get("KIMI_TEST"), Some(&"1".to_string()));
}

#[test]
fn resolve_reports_missing_agent() {
    let registry: RegistryDocument = toml_json(REGISTRY_JSON);
    let error =
        resolve_binary_command(&registry, "missing").expect_err("missing agent should fail");

    assert!(error.to_string().contains("missing"));
}

#[test]
fn resolve_rejects_package_only_distribution() {
    let registry: RegistryDocument = toml_json(REGISTRY_JSON);
    let error = resolve_binary_command(&registry, "package-only")
        .expect_err("package-only agent should not resolve in binary-only mode");

    assert!(error.to_string().contains("binary"));
}

#[test]
fn agent_server_command_fields_override_registry_target() {
    let registry: RegistryDocument = toml_json(REGISTRY_JSON);
    let mut env = BTreeMap::new();
    env.insert("KIMI_TEST".to_string(), "override".to_string());
    env.insert("EXTRA".to_string(), "yes".to_string());
    let server = AcpAgentServerConfig {
        server_type: AcpAgentServerType::Registry,
        agent: "kimi".to_string(),
        command: "kimi-dev".to_string(),
        args: vec!["acp".to_string(), "--debug".to_string()],
        env,
        default_model: None,
        default_mode: None,
    };

    let resolved = resolve_binary_command_for_server(&registry, "kimi", &server)
        .expect("kimi should resolve with override");

    assert_eq!(resolved.command, "kimi-dev");
    assert_eq!(resolved.args, vec!["acp", "--debug"]);
    assert_eq!(resolved.env.get("KIMI_TEST"), Some(&"override".to_string()));
    assert_eq!(resolved.env.get("EXTRA"), Some(&"yes".to_string()));
}

fn toml_json<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).expect("fixture json should deserialize")
}
