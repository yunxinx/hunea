use std::collections::BTreeMap;

use serde::Deserialize;

/// `RegistryDocument` 表示 ACP registry 的顶层 JSON 文档。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RegistryDocument {
    pub version: String,
    pub agents: Vec<RegistryAgent>,
    #[serde(default)]
    pub extensions: Vec<serde_json::Value>,
}

/// `RegistryAgent` 表示 registry 中的单个 ACP Agent 条目。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub distribution: RegistryDistribution,
    pub repository: Option<String>,
    pub website: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub icon: Option<String>,
}

/// `RegistryDistribution` 表示 registry 支持的分发方式。
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RegistryDistribution {
    pub binary: Option<BTreeMap<String, RegistryBinaryTarget>>,
    pub npx: Option<RegistryPackageDistribution>,
    pub uvx: Option<RegistryPackageDistribution>,
}

/// `RegistryBinaryTarget` 表示某个平台上的 binary 分发目标。
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RegistryBinaryTarget {
    pub archive: String,
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// `RegistryPackageDistribution` 表示 package manager 分发；第一版只解析不执行。
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RegistryPackageDistribution {
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
