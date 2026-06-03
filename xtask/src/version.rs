use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use semver::{Prerelease, Version};
use toml_edit::{DocumentMut, Item, value};

/// 支持的版本递增类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionBump {
    Major,
    Minor,
    Patch,
    Alpha,
}

impl VersionBump {
    /// 从 CLI 参数解析版本递增类型。
    pub fn parse(input: &str) -> Result<Self> {
        match input {
            "major" => Ok(Self::Major),
            "minor" => Ok(Self::Minor),
            "patch" => Ok(Self::Patch),
            "alpha" => Ok(Self::Alpha),
            _ => bail!(
                "unsupported version bump `{input}`; expected one of: major, minor, patch, alpha"
            ),
        }
    }
}

/// 根据当前 SemVer 和递增类型计算下一版本。
pub fn bump_version(current: &str, bump: VersionBump) -> Result<String> {
    let mut version = parse_release_version(current)?;
    match bump {
        VersionBump::Major => {
            if version.pre.is_empty() {
                version.major += 1;
                version.minor = 0;
                version.patch = 0;
            }
            version.pre = Prerelease::EMPTY;
        }
        VersionBump::Minor => {
            if version.pre.is_empty() {
                version.minor += 1;
                version.patch = 0;
            }
            version.pre = Prerelease::EMPTY;
        }
        VersionBump::Patch => {
            if version.pre.is_empty() {
                version.patch += 1;
            }
            version.pre = Prerelease::EMPTY;
        }
        VersionBump::Alpha => {
            if version.pre.is_empty() {
                version.patch += 1;
                version.pre = Prerelease::new("alpha.1")?;
            } else {
                version.pre = next_alpha_prerelease(&version.pre)?;
            }
        }
    }
    Ok(version.to_string())
}

/// 更新根 `Cargo.toml` 文本中的 `[workspace.package].version`。
pub fn set_workspace_version_text(manifest_text: &str, version: &str) -> Result<String> {
    let version = parse_release_version(version)?;
    let mut document = manifest_text
        .parse::<DocumentMut>()
        .context("Cargo.toml is not valid TOML")?;
    let package = workspace_package_table_mut(&mut document)?;
    if !package.contains_key("version") {
        bail!("Cargo.toml is missing [workspace.package].version");
    }
    package.insert("version", value(version.to_string()));
    Ok(document.to_string())
}

/// 读取根 `Cargo.toml` 文本中的 `[workspace.package].version`。
pub fn current_workspace_version(manifest_text: &str) -> Result<String> {
    let document = manifest_text
        .parse::<DocumentMut>()
        .context("Cargo.toml is not valid TOML")?;
    document
        .get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("package"))
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"))
        .and_then(Item::as_str)
        .map(str::to_string)
        .context("Cargo.toml is missing [workspace.package].version")
}

/// 更新根 `Cargo.toml` 文件中的 workspace 版本，并返回旧版本。
pub fn set_workspace_version(manifest_path: &Path, version: &str) -> Result<String> {
    let manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let old_version = current_workspace_version(&manifest_text)?;
    let updated_manifest = set_workspace_version_text(&manifest_text, version)?;
    fs::write(manifest_path, updated_manifest)
        .with_context(|| format!("write {}", manifest_path.display()))?;
    Ok(old_version)
}

/// 只更新 `Cargo.lock` 文本中的本 workspace package 版本。
pub fn set_lockfile_workspace_versions_text(
    lockfile_text: &str,
    old_version: &str,
    new_version: &str,
) -> Result<String> {
    parse_release_version(old_version)?;
    let new_version = parse_release_version(new_version)?;
    let mut document = lockfile_text
        .parse::<DocumentMut>()
        .context("Cargo.lock is not valid TOML")?;
    let packages = document
        .get_mut("package")
        .and_then(Item::as_array_of_tables_mut)
        .context("Cargo.lock is missing [[package]] entries")?;

    let mut updated_count = 0usize;
    for package in packages.iter_mut() {
        let is_workspace_package = !package.contains_key("source");
        let package_version = package.get("version").and_then(Item::as_str);
        if is_workspace_package && package_version == Some(old_version) {
            package.insert("version", value(new_version.to_string()));
            updated_count += 1;
        }
    }
    if updated_count == 0 {
        bail!("Cargo.lock has no workspace package entries at version {old_version}");
    }
    Ok(document.to_string())
}

/// 只更新 `Cargo.lock` 文件中的本 workspace package 版本。
pub fn set_lockfile_workspace_versions(
    lockfile_path: &Path,
    old_version: &str,
    new_version: &str,
) -> Result<()> {
    let lockfile_text = fs::read_to_string(lockfile_path)
        .with_context(|| format!("read {}", lockfile_path.display()))?;
    let updated_lockfile =
        set_lockfile_workspace_versions_text(&lockfile_text, old_version, new_version)?;
    fs::write(lockfile_path, updated_lockfile)
        .with_context(|| format!("write {}", lockfile_path.display()))?;
    Ok(())
}

/// 返回当前 `xtask` 所属的 workspace 根目录。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask crate should live below the workspace root")
        .to_path_buf()
}

fn parse_release_version(input: &str) -> Result<Version> {
    let version = Version::parse(input).with_context(|| format!("invalid SemVer `{input}`"))?;
    if !version.build.is_empty() {
        bail!("build metadata is not supported for release versions: `{input}`");
    }
    Ok(version)
}

fn next_alpha_prerelease(current: &Prerelease) -> Result<Prerelease> {
    let text = current.as_str();
    let suffix = text
        .strip_prefix("alpha.")
        .with_context(|| format!("cannot alpha-bump non-alpha prerelease `{text}`"))?;
    let number = suffix
        .parse::<u64>()
        .with_context(|| format!("alpha prerelease suffix is not numeric: `{text}`"))?;
    Prerelease::new(&format!("alpha.{}", number + 1)).context("build alpha prerelease")
}

fn workspace_package_table_mut(
    document: &mut DocumentMut,
) -> Result<&mut dyn toml_edit::TableLike> {
    document
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .and_then(|workspace| workspace.get_mut("package"))
        .and_then(Item::as_table_like_mut)
        .context("Cargo.toml is missing [workspace.package]")
}

#[cfg(test)]
mod tests {
    use super::{
        VersionBump, bump_version, current_workspace_version, set_lockfile_workspace_versions_text,
        set_workspace_version_text,
    };

    #[test]
    fn bump_version_updates_stable_semver_components() {
        assert_eq!(bump_version("0.4.0", VersionBump::Patch).unwrap(), "0.4.1");
        assert_eq!(bump_version("0.4.1", VersionBump::Minor).unwrap(), "0.5.0");
        assert_eq!(bump_version("0.5.0", VersionBump::Major).unwrap(), "1.0.0");
    }

    #[test]
    fn bump_version_starts_or_advances_alpha_prerelease() {
        assert_eq!(
            bump_version("0.4.0", VersionBump::Alpha).unwrap(),
            "0.4.1-alpha.1"
        );
        assert_eq!(
            bump_version("0.4.1-alpha.1", VersionBump::Alpha).unwrap(),
            "0.4.1-alpha.2"
        );
    }

    #[test]
    fn stable_bumps_promote_alpha_prerelease_to_target_release() {
        assert_eq!(
            bump_version("0.4.1-alpha.2", VersionBump::Patch).unwrap(),
            "0.4.1"
        );
        assert_eq!(
            bump_version("0.5.0-alpha.2", VersionBump::Minor).unwrap(),
            "0.5.0"
        );
        assert_eq!(
            bump_version("0.5.0-alpha.2", VersionBump::Patch).unwrap(),
            "0.5.0"
        );
        assert_eq!(
            bump_version("1.0.0-alpha.2", VersionBump::Major).unwrap(),
            "1.0.0"
        );
    }

    #[test]
    fn bump_version_rejects_build_metadata_for_release_versions() {
        assert!(bump_version("0.4.0+local", VersionBump::Patch).is_err());
    }

    #[test]
    fn set_workspace_version_text_updates_only_workspace_package_version() {
        let manifest = r#"[workspace]
members = ["crates/app"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
demo = { version = "1" }
"#;

        let updated = set_workspace_version_text(manifest, "0.4.0").unwrap();

        assert!(updated.contains("[workspace.package]\nversion = \"0.4.0\""));
        assert!(updated.contains("demo = { version = \"1\" }"));
    }

    #[test]
    fn set_workspace_version_text_rejects_invalid_semver() {
        assert!(
            set_workspace_version_text("[workspace.package]\nversion = \"0.1.0\"\n", "v0.4.0")
                .is_err()
        );
    }

    #[test]
    fn current_workspace_version_reads_workspace_package_version() {
        let manifest = "[workspace.package]\nversion = \"0.4.0\"\n";

        assert_eq!(current_workspace_version(manifest).unwrap(), "0.4.0");
    }

    #[test]
    fn set_lockfile_workspace_versions_text_updates_only_workspace_packages() {
        let lockfile = r#"[[package]]
name = "hunea"
version = "0.1.0"
dependencies = ["dep"]

[[package]]
name = "dep"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"
"#;

        let updated = set_lockfile_workspace_versions_text(lockfile, "0.1.0", "0.4.0").unwrap();

        assert!(updated.contains("name = \"hunea\"\nversion = \"0.4.0\""));
        assert!(updated.contains("name = \"dep\"\nversion = \"0.1.0\""));
    }
}
