use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use camino::Utf8PathBuf;
use semver::{BuildMetadata, Prerelease, Version};
use serde::Deserialize;
use toml_edit::{DocumentMut, Item};

use crate::cli::BumpKind;

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_members: Vec<String>,
    pub workspace_root: Utf8PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub rust_version: Option<String>,
    #[serde(default)]
    pub publish: Option<Vec<String>>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub manifest_path: Utf8PathBuf,
}

impl Package {
    pub fn is_publishable(&self) -> bool {
        self.publish
            .as_ref()
            .is_none_or(|registries| !registries.is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub path: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone)]
pub struct VersionPlan {
    pub package: Package,
    pub old_version: Version,
    pub new_version: Version,
}

#[derive(Debug, Clone)]
pub struct BumpSpec {
    pub kind: BumpKind,
    pub prerelease: Option<Prerelease>,
}

impl BumpSpec {
    pub fn new(kind: BumpKind, pre: Option<String>) -> Result<Self> {
        let prerelease = match pre {
            Some(value) => Some(
                Prerelease::new(&value)
                    .with_context(|| format!("parsing prerelease identifier `{value}`"))?,
            ),
            None => None,
        };

        if kind == BumpKind::Prerelease && prerelease.is_none() {
            bail!("prerelease bumps require --pre <id>");
        }

        Ok(Self { kind, prerelease })
    }
}

pub fn metadata_for_current_dir() -> Result<Metadata> {
    let start = std::env::current_dir().context("reading current directory")?;
    let manifest = find_manifest(&start)?;
    cargo_metadata(&manifest)
}

pub fn find_manifest(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not find Cargo.toml in {} or its parents",
        start.display()
    );
}

pub fn cargo_metadata(manifest: &Path) -> Result<Metadata> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .context("running cargo metadata")?;

    if !output.status.success() {
        bail!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout).context("parsing cargo metadata")
}

pub fn select_packages(
    metadata: &Metadata,
    requested: &[String],
    workspace: bool,
) -> Result<Vec<Package>> {
    let workspace_packages = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .cloned()
        .collect::<Vec<_>>();

    if workspace && !requested.is_empty() {
        bail!("use either --workspace or --package, not both");
    }

    if workspace {
        if workspace_packages.is_empty() {
            bail!("workspace has no packages");
        }
        return Ok(workspace_packages);
    }

    if !requested.is_empty() {
        let mut selected = Vec::new();
        for name in requested {
            let package = workspace_packages
                .iter()
                .find(|package| package.name == *name)
                .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"))?;
            selected.push(package.clone());
        }
        selected.sort_by(|left, right| left.name.cmp(&right.name));
        selected.dedup_by(|left, right| left.name == right.name);
        return Ok(selected);
    }

    match workspace_packages.as_slice() {
        [package] => Ok(vec![package.clone()]),
        [] => bail!("workspace has no packages"),
        _ => bail!("workspace has multiple packages; rerun with --package <name> or --workspace"),
    }
}

/// Select one package to supply metadata fallbacks for a workspace-level
/// packaging channel (aur/copr/apt).
///
/// Unlike [`select_packages`], this never errors on multi-package workspaces:
/// with an explicit `requested` name it selects that package, otherwise it
/// returns the alphabetically-first workspace member. Channel config is
/// expected to provide the substantive fields; the package only fills in
/// `description`/`license`/`homepage`/`name` fallbacks.
pub fn representative_package(metadata: &Metadata, requested: Option<&str>) -> Result<Package> {
    if let Some(name) = requested {
        return select_packages(metadata, std::slice::from_ref(&name.to_owned()), false)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"));
    }

    let mut members = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .cloned()
        .collect::<Vec<_>>();
    members.sort_by(|left, right| left.name.cmp(&right.name));
    members
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("workspace has no packages"))
}

pub fn plan_versions(packages: Vec<Package>, bump: &BumpSpec) -> Result<Vec<VersionPlan>> {
    packages
        .into_iter()
        .map(|package| {
            let old_version = Version::parse(&package.version)
                .with_context(|| format!("parsing version {}", package.version))?;
            let new_version = bump_version(old_version.clone(), bump);
            Ok(VersionPlan {
                package,
                old_version,
                new_version,
            })
        })
        .collect()
}

pub fn common_new_version(plans: &[VersionPlan]) -> Result<Version> {
    let Some(first) = plans.first() else {
        bail!("no packages selected");
    };
    if plans
        .iter()
        .any(|plan| plan.new_version != first.new_version)
    {
        let details = plans
            .iter()
            .map(|plan| format!("{} -> {}", plan.package.name, plan.new_version))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("selected packages do not resolve to one release version:\n{details}");
    }
    Ok(first.new_version.clone())
}

pub fn bump_version(mut version: Version, bump: &BumpSpec) -> Version {
    version.pre = Prerelease::EMPTY;
    version.build = BuildMetadata::EMPTY;

    match bump.kind {
        BumpKind::Patch => version.patch += 1,
        BumpKind::Minor => {
            version.minor += 1;
            version.patch = 0;
        }
        BumpKind::Major => {
            version.major += 1;
            version.minor = 0;
            version.patch = 0;
        }
        BumpKind::Prerelease => {}
    }

    if let Some(pre) = &bump.prerelease {
        version.pre = pre.clone();
    }

    version
}

pub fn update_manifest_version(manifest_path: &Path, version: &Version) -> Result<()> {
    let original = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut document = original
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let package = document
        .get_mut("package")
        .and_then(|item| item.as_table_mut())
        .ok_or_else(|| anyhow!("{} has no [package] table", manifest_path.display()))?;

    let version_item = package
        .get_mut("version")
        .ok_or_else(|| anyhow!("{} has no package.version", manifest_path.display()))?;

    if version_item.as_str().is_none() {
        // `version.workspace = true`: the version is inherited from the workspace
        // root and is bumped there by `update_workspace_version`, not per-crate.
        if version_inherits_workspace(version_item) {
            return Ok(());
        }
        bail!(
            "{} package.version must be a literal string for simit to update it",
            manifest_path.display()
        );
    }

    *version_item = toml_edit::value(version.to_string());
    fs::write(manifest_path, document.to_string())
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    Ok(())
}

/// Whether a `[package].version` item is `version.workspace = true`.
fn version_inherits_workspace(version_item: &Item) -> bool {
    version_item
        .as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(Item::as_bool)
        == Some(true)
}

/// Names of all workspace-member packages.
pub fn workspace_member_names(metadata: &Metadata) -> Vec<String> {
    metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .map(|package| package.name.clone())
        .collect()
}

/// Bump the workspace root's `[workspace.package].version` and the matching
/// `[workspace.dependencies]` version requirements for workspace members.
///
/// This is the workspace-inheritance counterpart to [`update_manifest_version`]:
/// when crates use `version.workspace = true`, the canonical version lives only
/// in the root `[workspace.package].version`, so a release bumps it there (and
/// keeps intra-workspace dependency requirements in lockstep) rather than in
/// each crate manifest. Returns `Ok(true)` when the root `Cargo.toml` was
/// modified, `Ok(false)` when there is no `[workspace.package].version` to bump.
pub fn update_workspace_version(
    workspace_root: &Path,
    version: &Version,
    member_names: &[String],
) -> Result<bool> {
    let path = workspace_root.join("Cargo.toml");
    let original =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut document = original
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;

    let Some(workspace) = document
        .get_mut("workspace")
        .and_then(|item| item.as_table_mut())
    else {
        return Ok(false);
    };

    let new_version = version.to_string();
    let mut changed = false;

    if let Some(version_item) = workspace
        .get_mut("package")
        .and_then(|item| item.as_table_mut())
        .and_then(|package| package.get_mut("version"))
    {
        if version_item.as_str() != Some(new_version.as_str()) {
            *version_item = toml_edit::value(new_version.clone());
            changed = true;
        }
    }

    if let Some(dependencies) = workspace
        .get_mut("dependencies")
        .and_then(|item| item.as_table_mut())
    {
        for member in member_names {
            let Some(dependency) = dependencies.get_mut(member) else {
                continue;
            };
            if let Some(table) = dependency.as_table_like_mut() {
                // `member = { version = "x", path = "..." }`
                if table
                    .get("version")
                    .and_then(Item::as_str)
                    .is_some_and(|current| current != new_version)
                {
                    table.insert("version", toml_edit::value(new_version.clone()));
                    changed = true;
                }
            } else if dependency
                .as_str()
                .is_some_and(|current| current != new_version)
            {
                // `member = "x"`
                *dependency = toml_edit::value(new_version.clone());
                changed = true;
            }
        }
    }

    if changed {
        fs::write(&path, document.to_string())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(changed)
}

pub fn update_lockfile(workspace_root: &Path, plans: &[VersionPlan]) -> Result<()> {
    for plan in plans {
        let status = Command::new("cargo")
            .current_dir(workspace_root)
            .args(["update", "-p", &plan.package.name, "--precise"])
            .arg(plan.new_version.to_string())
            .status()
            .context("running cargo update")?;

        if !status.success() {
            bail!("cargo update failed while updating Cargo.lock");
        }
    }

    Ok(())
}

#[cfg(test)]
mod workspace_version_tests {
    use super::*;

    #[test]
    fn detects_dotted_workspace_inheritance() {
        let doc: DocumentMut = "[package]\nversion.workspace = true\n".parse().unwrap();
        assert!(version_inherits_workspace(&doc["package"]["version"]));
        let literal: DocumentMut = "[package]\nversion = \"1.0.0\"\n".parse().unwrap();
        assert!(!version_inherits_workspace(&literal["package"]["version"]));
    }

    #[test]
    fn bumps_workspace_package_and_member_dep_reqs() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n\n\
             [workspace.package]\nversion = \"0.2.0\"\nedition = \"2024\"\n\n\
             [workspace.dependencies]\n\
             a = { version = \"0.2.0\", path = \"crates/a\" }\n\
             serde = \"1\"\n",
        )
        .unwrap();
        let version = Version::parse("0.2.1").unwrap();
        let members = vec!["a".to_owned(), "b".to_owned()];

        let changed = update_workspace_version(dir.path(), &version, &members).unwrap();
        assert!(changed);
        let out = std::fs::read_to_string(&cargo_toml).unwrap();
        assert!(out.contains("version = \"0.2.1\"")); // [workspace.package]
        assert!(out.contains("path = \"crates/a\"")); // member dep preserved
        assert!(!out.contains("0.2.0")); // member dep req bumped too
        assert!(out.contains("serde = \"1\"")); // non-member dep untouched

        // idempotent: a second bump to the same version writes nothing new.
        assert!(!update_workspace_version(dir.path(), &version, &members).unwrap());
    }

    #[test]
    fn noop_when_no_workspace_package_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let version = Version::parse("0.2.0").unwrap();
        assert!(!update_workspace_version(dir.path(), &version, &[]).unwrap());
    }
}
