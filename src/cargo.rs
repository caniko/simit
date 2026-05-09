use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use camino::Utf8PathBuf;
use semver::{BuildMetadata, Prerelease, Version};
use serde::Deserialize;
use toml_edit::DocumentMut;

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
    pub rust_version: Option<String>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
    pub manifest_path: Utf8PathBuf,
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
