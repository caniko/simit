//! Project-level simit configuration.
//!
//! Packager setting precedence is intentionally centralized here. For each
//! setting, the value comes from, in order:
//!
//! 1. The CLI flag, when provided.
//! 2. The corresponding simit project config field, when a project config
//!    source exists and the field is present.
//! 3. The Cargo package metadata fallback, where one exists.
//! 4. An error.
//!
//! Settings that have no Cargo fallback, such as `tap_url` and
//! `download_repo`, error if neither a CLI flag nor config value provides them.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub homebrew: Option<HomebrewConfig>,
    #[serde(default)]
    pub chocolatey: Option<ChocolateyConfig>,
    #[serde(default)]
    pub scoop: Option<ScoopConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HomebrewConfig {
    /// Formula name; if omitted, derived from Cargo package name.
    pub name: Option<String>,

    /// Binaries to install. If omitted, defaults to `[name]`.
    #[serde(default)]
    pub binaries: Vec<String>,

    /// Tap repo URL. Required when `[homebrew]` is present.
    pub tap_url: String,

    /// Description for the formula. If omitted, derived from Cargo metadata.
    pub description: Option<String>,

    /// Homepage URL. If omitted, derived from Cargo metadata.
    pub homepage: Option<String>,

    /// SPDX license identifier. If omitted, derived from Cargo metadata.
    pub license: Option<String>,

    /// Codeberg/GitHub `<owner>/<repo>` for release downloads.
    pub download_repo: String,

    /// Archive filename pattern with `{name}`, `{version}`, `{arch}`, `{os}`.
    #[serde(default = "default_archive_pattern")]
    pub archive_pattern: String,

    /// Per-platform enable flags. Each defaults to true.
    #[serde(default)]
    pub platforms: HomebrewPlatformsConfig,
}

fn default_archive_pattern() -> String {
    "{name}-{version}-{arch}-{os}.tar.gz".to_owned()
}

fn default_windows_archive_pattern() -> String {
    "{name}-{version}-{arch}-windows.zip".to_owned()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HomebrewPlatformsConfig {
    #[serde(default = "default_true")]
    pub darwin_arm: bool,
    #[serde(default = "default_true")]
    pub darwin_intel: bool,
    #[serde(default = "default_true")]
    pub linux_arm: bool,
    #[serde(default = "default_true")]
    pub linux_intel: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChocolateyConfig {
    /// Chocolatey package display name; if omitted, derived from Cargo package name.
    pub name: Option<String>,

    /// Nuspec package identifier; if omitted, derived from the resolved name.
    pub id: Option<String>,

    /// Nuspec title; if omitted, derived from the resolved name.
    pub title: Option<String>,

    /// Nuspec authors. If omitted, derived from Cargo package authors when available.
    pub authors: Option<String>,

    /// Nuspec description. If omitted, derived from Cargo metadata.
    pub description: Option<String>,

    /// Project URL. If omitted, derived from Cargo package homepage.
    pub project_url: Option<String>,

    /// License URL.
    pub license_url: Option<String>,

    /// Space-separated Chocolatey tags.
    pub tags: Option<String>,

    /// Release notes URL.
    pub release_notes_url: Option<String>,

    /// Codeberg/GitHub `<owner>/<repo>` for release downloads.
    pub download_repo: String,

    /// Archive filename pattern with `{name}`, `{version}`, `{arch}`.
    #[serde(default = "default_windows_archive_pattern")]
    pub archive_pattern: String,

    /// Chocolatey push settings.
    #[serde(default)]
    pub push: ChocolateyPushConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChocolateyPushConfig {
    #[serde(default = "default_chocolatey_push_source")]
    pub source: String,
}

impl Default for ChocolateyPushConfig {
    fn default() -> Self {
        Self {
            source: default_chocolatey_push_source(),
        }
    }
}

fn default_chocolatey_push_source() -> String {
    "https://push.chocolatey.org/".to_owned()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScoopConfig {
    /// Scoop manifest name; if omitted, derived from Cargo package name.
    pub name: Option<String>,

    /// Bucket repo URL. Required when `[scoop]` is present.
    pub bucket_url: String,

    /// Manifest description. If omitted, derived from Cargo metadata.
    pub description: Option<String>,

    /// Manifest homepage. If omitted, derived from Cargo metadata.
    pub homepage: Option<String>,

    /// Manifest license. If omitted, derived from Cargo metadata.
    pub license: Option<String>,

    /// Codeberg/GitHub `<owner>/<repo>` for release downloads.
    pub download_repo: String,

    /// Archive filename pattern with `{name}`, `{version}`, `{arch}`.
    #[serde(default = "default_windows_archive_pattern")]
    pub archive_pattern: String,

    /// Binaries to expose. If omitted, defaults to `[name]`.
    #[serde(default)]
    pub binaries: Vec<String>,

    /// Per-architecture enable flags. Each defaults to true.
    #[serde(default)]
    pub architectures: ScoopArchSet,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScoopArchSet {
    #[serde(default = "default_true")]
    pub x64: bool,
    #[serde(default = "default_true")]
    pub arm64: bool,
}

impl ScoopArchSet {
    pub fn any_enabled(&self) -> bool {
        self.x64 || self.arm64
    }
}

impl Default for ScoopArchSet {
    fn default() -> Self {
        Self {
            x64: true,
            arm64: true,
        }
    }
}

impl HomebrewPlatformsConfig {
    pub fn any_enabled(&self) -> bool {
        self.darwin_arm || self.darwin_intel || self.linux_arm || self.linux_intel
    }
}

impl Default for HomebrewPlatformsConfig {
    fn default() -> Self {
        Self {
            darwin_arm: true,
            darwin_intel: true,
            linux_arm: true,
            linux_intel: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHomebrew {
    pub name: String,
    pub binaries: Vec<String>,
    pub tap_url: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
    pub download_repo: String,
    pub archive_pattern: String,
    pub platforms: HomebrewPlatformsConfig,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HomebrewOverrides<'a> {
    pub name: Option<&'a str>,
    pub binaries: Option<&'a [String]>,
    pub tap_url: Option<&'a str>,
    pub description: Option<&'a str>,
    pub homepage: Option<&'a str>,
    pub license: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub archive_pattern: Option<&'a str>,
    pub disabled_platforms: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChocolatey {
    pub name: String,
    pub id: String,
    pub title: String,
    pub authors: Option<String>,
    pub description: String,
    pub project_url: String,
    pub license_url: Option<String>,
    pub tags: Option<String>,
    pub release_notes_url: Option<String>,
    pub download_repo: String,
    pub archive_pattern: String,
    pub push: ChocolateyPushConfig,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChocolateyOverrides<'a> {
    pub name: Option<&'a str>,
    pub id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub authors: Option<&'a str>,
    pub description: Option<&'a str>,
    pub project_url: Option<&'a str>,
    pub license_url: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub release_notes_url: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub archive_pattern: Option<&'a str>,
    pub push_source: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedScoop {
    pub name: String,
    pub bucket_url: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
    pub download_repo: String,
    pub archive_pattern: String,
    pub binaries: Vec<String>,
    pub architectures: ScoopArchSet,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScoopOverrides<'a> {
    pub name: Option<&'a str>,
    pub bucket_url: Option<&'a str>,
    pub description: Option<&'a str>,
    pub homepage: Option<&'a str>,
    pub license: Option<&'a str>,
    pub download_repo: Option<&'a str>,
    pub archive_pattern: Option<&'a str>,
    pub binaries: Option<&'a [String]>,
    pub disabled_architectures: &'a [String],
}

impl ProjectConfig {
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let sources = Self::load_sources(workspace_root)?;
        match sources.as_slice() {
            [] => Ok(Self::default()),
            [source] => Ok(source.config.clone()),
            _ => {
                let labels = sources
                    .iter()
                    .map(|source| source.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "multiple simit project config sources found ({labels}); keep exactly one of simit.toml, Cargo.toml [workspace.metadata.simit], Cargo.toml [package.metadata.simit], or flake outputs.simitConfig"
                )
            }
        }
    }

    fn load_sources(workspace_root: &Path) -> Result<Vec<ProjectConfigSource>> {
        let mut sources = Vec::new();

        if let Some(source) = load_simit_toml(workspace_root)? {
            sources.push(source);
        }
        sources.extend(load_cargo_metadata_config(workspace_root)?);
        if let Some(source) = load_flake_config(workspace_root)? {
            sources.push(source);
        }

        Ok(sources
            .into_iter()
            .filter(|source| !source.config.is_empty())
            .collect())
    }

    /// Validate the optional `[homebrew]` section.
    ///
    /// Validation is intentionally lazy: `load` only parses the file, and
    /// callers that consume Homebrew settings decide when malformed Homebrew
    /// config should become fatal.
    pub fn validate_homebrew(&self) -> Result<()> {
        let Some(homebrew) = &self.homebrew else {
            return Ok(());
        };

        if homebrew.tap_url.is_empty() {
            bail!("simit project config: [homebrew].tap_url is required");
        }
        reject_basic_auth_url(
            "simit project config: [homebrew].tap_url",
            &homebrew.tap_url,
        )?;
        if homebrew.download_repo.is_empty() {
            bail!("simit project config: [homebrew].download_repo is required");
        }
        if let Some(desc) = &homebrew.description {
            if desc.chars().count() > 80 {
                bail!(
                    "simit project config: [homebrew].description must be 80 characters or fewer"
                );
            }
        }
        if let Some(home) = &homebrew.homepage {
            if !home.starts_with("https://") {
                bail!("simit project config: [homebrew].homepage must start with https://");
            }
        }
        if !homebrew.platforms.any_enabled() {
            bail!("simit project config: [homebrew].platforms has all platforms disabled");
        }

        Ok(())
    }

    /// Validate the optional `[chocolatey]` section.
    pub fn validate_chocolatey(&self) -> Result<()> {
        let Some(chocolatey) = &self.chocolatey else {
            return Ok(());
        };

        if chocolatey.download_repo.is_empty() {
            bail!("simit project config: [chocolatey].download_repo is required");
        }
        if let Some(desc) = &chocolatey.description {
            if desc.chars().count() > 4000 {
                bail!(
                    "simit project config: [chocolatey].description must be 4000 characters or fewer"
                );
            }
        }
        if let Some(tags) = &chocolatey.tags {
            if tags.chars().count() > 4000 {
                bail!("simit project config: [chocolatey].tags must be 4000 characters or fewer");
            }
        }
        reject_basic_auth_url(
            "simit project config: [chocolatey].push.source",
            &chocolatey.push.source,
        )?;

        Ok(())
    }

    /// Validate the optional `[scoop]` section.
    pub fn validate_scoop(&self) -> Result<()> {
        let Some(scoop) = &self.scoop else {
            return Ok(());
        };

        if scoop.bucket_url.is_empty() {
            bail!("simit project config: [scoop].bucket_url is required");
        }
        reject_basic_auth_url(
            "simit project config: [scoop].bucket_url",
            &scoop.bucket_url,
        )?;
        if scoop.download_repo.is_empty() {
            bail!("simit project config: [scoop].download_repo is required");
        }
        if !scoop.architectures.any_enabled() {
            bail!("simit project config: [scoop].architectures has all architectures disabled");
        }

        Ok(())
    }

    /// Resolve Homebrew settings for one Cargo package.
    ///
    /// Workspaces may contain multiple packages; the caller is responsible for
    /// selecting the package whose metadata should provide fallbacks.
    pub fn resolve_homebrew(
        &self,
        overrides: HomebrewOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedHomebrew> {
        self.validate_homebrew()?;

        let cfg = self.homebrew.as_ref();
        let name = merge(
            overrides.name.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.name.clone()),
            Some(package.name.clone()),
            "name",
        )?;
        let tap_url = merge(
            overrides.tap_url.map(str::to_owned),
            cfg.map(|homebrew| homebrew.tap_url.clone()),
            None,
            "tap_url",
        )?;
        reject_basic_auth_url("homebrew.tap_url", &tap_url)?;
        let description = merge(
            overrides.description.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.description.clone()),
            package.description.clone(),
            "description",
        )?;
        let homepage = merge(
            overrides.homepage.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.homepage.clone()),
            package.homepage.clone(),
            "homepage",
        )?;
        let license = merge(
            overrides.license.map(str::to_owned),
            cfg.and_then(|homebrew| homebrew.license.clone()),
            package.license.clone(),
            "license",
        )?;
        let download_repo = merge(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|homebrew| homebrew.download_repo.clone()),
            None,
            "download_repo",
        )?;
        let archive_pattern = overrides
            .archive_pattern
            .map(str::to_owned)
            .or_else(|| cfg.map(|homebrew| homebrew.archive_pattern.clone()))
            .unwrap_or_else(default_archive_pattern);
        let binaries = resolve_binaries(overrides.binaries, cfg, &name);
        let mut platforms = cfg
            .map(|homebrew| homebrew.platforms.clone())
            .unwrap_or_default();
        apply_disabled_platforms(&mut platforms, overrides.disabled_platforms)?;

        if !platforms.any_enabled() {
            bail!("homebrew.platforms has all platforms disabled");
        }

        Ok(ResolvedHomebrew {
            name,
            binaries,
            tap_url,
            description,
            homepage,
            license,
            download_repo,
            archive_pattern,
            platforms,
        })
    }

    /// Resolve Chocolatey settings for one Cargo package.
    pub fn resolve_chocolatey(
        &self,
        overrides: ChocolateyOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedChocolatey> {
        self.validate_chocolatey()?;

        let cfg = self.chocolatey.as_ref();
        let name = merge_packager(
            overrides.name.map(str::to_owned),
            cfg.and_then(|chocolatey| chocolatey.name.clone()),
            Some(package.name.clone()),
            missing_chocolatey_message("name"),
        )?;
        let id = overrides
            .id
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.id.clone()))
            .unwrap_or_else(|| name.clone());
        let title = overrides
            .title
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.title.clone()))
            .unwrap_or_else(|| name.clone());
        let authors = overrides
            .authors
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.authors.clone()));
        let authors =
            authors.or_else(|| (!package.authors.is_empty()).then(|| package.authors.join(", ")));
        let download_repo = merge_packager(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|chocolatey| chocolatey.download_repo.clone()),
            None,
            missing_chocolatey_message("download_repo"),
        )?;
        let description = merge_packager(
            overrides.description.map(str::to_owned),
            cfg.and_then(|chocolatey| chocolatey.description.clone()),
            package.description.clone(),
            missing_chocolatey_message("description"),
        )?;
        if description.chars().count() > 4000 {
            bail!("chocolatey.description must be 4000 characters or fewer");
        }
        let project_url = merge_packager(
            overrides.project_url.map(str::to_owned),
            cfg.and_then(|chocolatey| chocolatey.project_url.clone()),
            package.homepage.clone(),
            missing_chocolatey_message("project_url"),
        )?;
        let license_url = overrides
            .license_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.license_url.clone()));
        let tags = overrides
            .tags
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.tags.clone()));
        if let Some(tags) = &tags {
            if tags.chars().count() > 4000 {
                bail!("chocolatey.tags must be 4000 characters or fewer");
            }
        }
        let release_notes_url = overrides
            .release_notes_url
            .map(str::to_owned)
            .or_else(|| cfg.and_then(|chocolatey| chocolatey.release_notes_url.clone()));
        let archive_pattern = overrides
            .archive_pattern
            .map(str::to_owned)
            .or_else(|| cfg.map(|chocolatey| chocolatey.archive_pattern.clone()))
            .unwrap_or_else(default_windows_archive_pattern);
        let push_source = overrides
            .push_source
            .map(str::to_owned)
            .or_else(|| cfg.map(|chocolatey| chocolatey.push.source.clone()))
            .unwrap_or_else(default_chocolatey_push_source);
        reject_basic_auth_url("chocolatey.push.source", &push_source)?;

        Ok(ResolvedChocolatey {
            name,
            id,
            title,
            authors,
            description,
            project_url,
            license_url,
            tags,
            release_notes_url,
            download_repo,
            archive_pattern,
            push: ChocolateyPushConfig {
                source: push_source,
            },
        })
    }

    /// Resolve Scoop settings for one Cargo package.
    pub fn resolve_scoop(
        &self,
        overrides: ScoopOverrides<'_>,
        package: &crate::cargo::Package,
    ) -> Result<ResolvedScoop> {
        self.validate_scoop()?;

        let cfg = self.scoop.as_ref();
        let name = merge_packager(
            overrides.name.map(str::to_owned),
            cfg.and_then(|scoop| scoop.name.clone()),
            Some(package.name.clone()),
            missing_scoop_message("name"),
        )?;
        let bucket_url = merge_packager(
            overrides.bucket_url.map(str::to_owned),
            cfg.map(|scoop| scoop.bucket_url.clone()),
            None,
            missing_scoop_message("bucket_url"),
        )?;
        reject_basic_auth_url("scoop.bucket_url", &bucket_url)?;
        let description = merge_packager(
            overrides.description.map(str::to_owned),
            cfg.and_then(|scoop| scoop.description.clone()),
            package.description.clone(),
            missing_scoop_message("description"),
        )?;
        let homepage = merge_packager(
            overrides.homepage.map(str::to_owned),
            cfg.and_then(|scoop| scoop.homepage.clone()),
            package.homepage.clone(),
            missing_scoop_message("homepage"),
        )?;
        let license = merge_packager(
            overrides.license.map(str::to_owned),
            cfg.and_then(|scoop| scoop.license.clone()),
            package.license.clone(),
            missing_scoop_message("license"),
        )?;
        let download_repo = merge_packager(
            overrides.download_repo.map(str::to_owned),
            cfg.map(|scoop| scoop.download_repo.clone()),
            None,
            missing_scoop_message("download_repo"),
        )?;
        let archive_pattern = overrides
            .archive_pattern
            .map(str::to_owned)
            .or_else(|| cfg.map(|scoop| scoop.archive_pattern.clone()))
            .unwrap_or_else(default_windows_archive_pattern);
        let binaries = resolve_scoop_binaries(overrides.binaries, cfg, &name);
        let mut architectures = cfg
            .map(|scoop| scoop.architectures.clone())
            .unwrap_or_default();
        apply_disabled_architectures(&mut architectures, overrides.disabled_architectures)?;

        if !architectures.any_enabled() {
            bail!("scoop.architectures has all architectures disabled");
        }

        Ok(ResolvedScoop {
            name,
            bucket_url,
            description,
            homepage,
            license,
            download_repo,
            archive_pattern,
            binaries,
            architectures,
        })
    }

    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug)]
struct ProjectConfigSource {
    label: String,
    config: ProjectConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CargoManifestConfig {
    #[serde(default)]
    package: Option<CargoPackageConfig>,
    #[serde(default)]
    workspace: Option<CargoWorkspaceConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackageConfig {
    #[serde(default)]
    metadata: CargoMetadataConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CargoWorkspaceConfig {
    #[serde(default)]
    metadata: CargoMetadataConfig,
}

#[derive(Debug, Default, Deserialize)]
struct CargoMetadataConfig {
    #[serde(default)]
    simit: Option<ProjectConfig>,
}

fn load_simit_toml(workspace_root: &Path) -> Result<Option<ProjectConfigSource>> {
    let path = workspace_root.join("simit.toml");
    if !path.exists() {
        return Ok(None);
    }

    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let config =
        toml_edit::de::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(ProjectConfigSource {
        label: "simit.toml".to_owned(),
        config,
    }))
}

fn load_cargo_metadata_config(workspace_root: &Path) -> Result<Vec<ProjectConfigSource>> {
    let path = workspace_root.join("Cargo.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: CargoManifestConfig =
        toml_edit::de::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let mut sources = Vec::new();
    if let Some(config) = manifest
        .workspace
        .and_then(|workspace| workspace.metadata.simit)
    {
        sources.push(ProjectConfigSource {
            label: "Cargo.toml [workspace.metadata.simit]".to_owned(),
            config,
        });
    }
    if let Some(config) = manifest.package.and_then(|package| package.metadata.simit) {
        sources.push(ProjectConfigSource {
            label: "Cargo.toml [package.metadata.simit]".to_owned(),
            config,
        });
    }

    Ok(sources)
}

fn load_flake_config(workspace_root: &Path) -> Result<Option<ProjectConfigSource>> {
    let path = workspace_root.join("flake.nix");
    if !path.exists() || !flake_declares_simit_config(&path)? {
        return Ok(None);
    }

    let command_text = "nix --extra-experimental-features 'nix-command flakes' eval --json --no-write-lock-file .#simitConfig";
    let output = Command::new("nix")
        .current_dir(workspace_root)
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--json",
            "--no-write-lock-file",
            ".#simitConfig",
        ])
        .output()
        .with_context(|| {
            format!(
                "running `{command_text}` to read flake outputs.simitConfig; ensure Nix is installed and flakes are enabled"
            )
        })?;

    if !output.status.success() {
        bail!(
            "flake outputs.simitConfig could not be evaluated; fix the flake and rerun `{command_text}` to validate it:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let config =
        serde_json::from_slice(&output.stdout).context("parsing flake outputs.simitConfig JSON")?;
    Ok(Some(ProjectConfigSource {
        label: "flake outputs.simitConfig".to_owned(),
        config,
    }))
}

fn flake_declares_simit_config(path: &Path) -> Result<bool> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .map(str::trim_start)
        .any(|line| !line.starts_with('#') && line.contains("simitConfig") && line.contains('=')))
}

fn reject_basic_auth_url(name: &str, value: &str) -> Result<()> {
    let Some(scheme_end) = value.find("://") else {
        return Ok(());
    };
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map_or(value.len(), |offset| authority_start + offset);
    if value[authority_start..authority_end].contains('@') {
        bail!("{name} must not include embedded credentials");
    }
    Ok(())
}

fn resolve_binaries(
    cli: Option<&[String]>,
    cfg: Option<&HomebrewConfig>,
    name: &str,
) -> Vec<String> {
    cli.filter(|values| !values.is_empty())
        .map(|values| values.to_vec())
        .or_else(|| {
            cfg.and_then(|homebrew| {
                (!homebrew.binaries.is_empty()).then(|| homebrew.binaries.clone())
            })
        })
        .unwrap_or_else(|| vec![name.to_owned()])
}

fn resolve_scoop_binaries(
    cli: Option<&[String]>,
    cfg: Option<&ScoopConfig>,
    name: &str,
) -> Vec<String> {
    cli.filter(|values| !values.is_empty())
        .map(|values| values.to_vec())
        .or_else(|| scoop_config_binaries(cfg))
        .unwrap_or_else(|| vec![name.to_owned()])
}

fn scoop_config_binaries(cfg: Option<&ScoopConfig>) -> Option<Vec<String>> {
    cfg.and_then(|scoop| (!scoop.binaries.is_empty()).then(|| scoop.binaries.clone()))
}

fn apply_disabled_platforms(
    platforms: &mut HomebrewPlatformsConfig,
    disabled: &[String],
) -> Result<()> {
    for key in disabled {
        match key.as_str() {
            "darwin_arm" => platforms.darwin_arm = false,
            "darwin_intel" => platforms.darwin_intel = false,
            "linux_arm" => platforms.linux_arm = false,
            "linux_intel" => platforms.linux_intel = false,
            _ => bail!(
                "homebrew disabled platform must be one of darwin_arm, darwin_intel, linux_arm, linux_intel"
            ),
        }
    }

    Ok(())
}

fn apply_disabled_architectures(
    architectures: &mut ScoopArchSet,
    disabled: &[String],
) -> Result<()> {
    for key in disabled {
        match key.as_str() {
            "x64" => architectures.x64 = false,
            "arm64" => architectures.arm64 = false,
            _ => bail!("scoop disabled architecture must be one of x64, arm64"),
        }
    }

    Ok(())
}

fn merge<T>(cli: Option<T>, cfg: Option<T>, metadata: Option<T>, name: &str) -> Result<T> {
    cli.or(cfg)
        .or(metadata)
        .ok_or_else(|| anyhow!("{}", missing_message(name)))
}

fn merge_packager<T>(
    cli: Option<T>,
    cfg: Option<T>,
    metadata: Option<T>,
    missing: String,
) -> Result<T> {
    cli.or(cfg)
        .or(metadata)
        .ok_or_else(|| anyhow!("{}", missing))
}

fn missing_message(name: &str) -> String {
    let config_hint = config_hint();
    match name {
        "name" => {
            format!(
                "homebrew.name not set: provide it via --homebrew-name, {config_hint} [homebrew].name, or Cargo.toml package.name"
            )
        }
        "tap_url" => {
            format!(
                "homebrew.tap_url not set: provide it via --homebrew-tap or {config_hint} [homebrew].tap_url"
            )
        }
        "description" => {
            format!(
                "homebrew.description not set: provide it via --homebrew-description, {config_hint} [homebrew].description, or Cargo.toml package.description"
            )
        }
        "homepage" => {
            format!(
                "homebrew.homepage not set: provide it via --homebrew-homepage, {config_hint} [homebrew].homepage, or Cargo.toml package.homepage"
            )
        }
        "license" => {
            format!(
                "homebrew.license not set: provide it via --homebrew-license, {config_hint} [homebrew].license, or Cargo.toml package.license"
            )
        }
        "download_repo" => {
            format!(
                "homebrew.download_repo not set: provide it via --homebrew-download-repo or {config_hint} [homebrew].download_repo"
            )
        }
        _ => "homebrew setting not set".to_owned(),
    }
}

fn missing_chocolatey_message(name: &str) -> String {
    let config_hint = config_hint();
    match name {
        "name" => {
            format!(
                "chocolatey.name not set: provide it via --choco-name, {config_hint} [chocolatey].name, or Cargo.toml package.name"
            )
        }
        "description" => {
            format!(
                "chocolatey.description not set: provide it via --choco-description, {config_hint} [chocolatey].description, or Cargo.toml package.description"
            )
        }
        "project_url" => {
            format!(
                "chocolatey.project_url not set: provide it via --choco-project-url, {config_hint} [chocolatey].project_url, or Cargo.toml package.homepage"
            )
        }
        "download_repo" => {
            format!(
                "chocolatey.download_repo not set: provide it via --choco-download-repo or {config_hint} [chocolatey].download_repo"
            )
        }
        _ => "chocolatey setting not set".to_owned(),
    }
}

fn missing_scoop_message(name: &str) -> String {
    let config_hint = config_hint();
    match name {
        "name" => {
            format!(
                "scoop.name not set: provide it via --scoop-name, {config_hint} [scoop].name, or Cargo.toml package.name"
            )
        }
        "bucket_url" => {
            format!(
                "scoop.bucket_url not set: provide it via --scoop-bucket or {config_hint} [scoop].bucket_url"
            )
        }
        "description" => {
            format!(
                "scoop.description not set: provide it via --scoop-description, {config_hint} [scoop].description, or Cargo.toml package.description"
            )
        }
        "homepage" => {
            format!(
                "scoop.homepage not set: provide it via --scoop-homepage, {config_hint} [scoop].homepage, or Cargo.toml package.homepage"
            )
        }
        "license" => {
            format!(
                "scoop.license not set: provide it via --scoop-license, {config_hint} [scoop].license, or Cargo.toml package.license"
            )
        }
        "download_repo" => {
            format!(
                "scoop.download_repo not set: provide it via --scoop-download-repo or {config_hint} [scoop].download_repo"
            )
        }
        _ => "scoop setting not set".to_owned(),
    }
}

fn config_hint() -> &'static str {
    "simit.toml, Cargo.toml [workspace.metadata.simit]/[package.metadata.simit], or flake outputs.simitConfig"
}
