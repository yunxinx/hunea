use anyhow::{Context, Result, bail};
use xtask::version::{
    VersionBump, bump_version, current_workspace_version, repo_root,
    set_lockfile_workspace_versions, set_workspace_version,
};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("version") => run_version_command(args.collect()),
        _ => {
            bail!(
                "usage: cargo xtask version set <semver>\n       cargo xtask version bump <major|minor|patch|alpha>"
            )
        }
    }
}

fn run_version_command(args: Vec<String>) -> Result<()> {
    let repo_root = repo_root();
    let manifest_path = repo_root.join("Cargo.toml");
    let lockfile_path = repo_root.join("Cargo.lock");
    match args.as_slice() {
        [command, version] if command == "set" => {
            let old_version = set_workspace_version(&manifest_path, version)?;
            set_lockfile_workspace_versions(&lockfile_path, &old_version, version)?;
            println!("workspace version: {old_version} -> {version}");
            Ok(())
        }
        [command, bump] if command == "bump" => {
            let manifest_text = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("read {}", manifest_path.display()))?;
            let old_version = current_workspace_version(&manifest_text)?;
            let next_version = bump_version(&old_version, VersionBump::parse(bump)?)?;
            set_workspace_version(&manifest_path, &next_version)?;
            set_lockfile_workspace_versions(&lockfile_path, &old_version, &next_version)?;
            println!("workspace version: {old_version} -> {next_version}");
            Ok(())
        }
        _ => {
            bail!(
                "usage: cargo xtask version set <semver>\n       cargo xtask version bump <major|minor|patch|alpha>"
            )
        }
    }
}
