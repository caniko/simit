//! Project-level simit configuration.
//!
//! Homebrew setting precedence is intentionally centralized here. For each
//! setting, the value comes from, in order:
//!
//! 1. The CLI flag, when provided.
//! 2. The `[homebrew]` field in `simit.toml`, when the file exists and the
//!    field is present.
//! 3. The Cargo package metadata fallback, where one exists.
//! 4. An error.
//!
//! Settings that have no Cargo fallback, such as `tap_url` and
//! `download_repo`, error if neither a CLI flag nor config value provides them.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub homebrew: Option<HomebrewConfig>,
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

impl ProjectConfig {
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let path = workspace_root.join("simit.toml");
        if !path.exists() {
            return Ok(Self::default());
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml_edit::de::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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
            bail!("simit.toml: [homebrew].tap_url is required");
        }
        if homebrew.download_repo.is_empty() {
            bail!("simit.toml: [homebrew].download_repo is required");
        }
        if let Some(desc) = &homebrew.description
            && desc.chars().count() > 80
        {
            bail!("simit.toml: [homebrew].description must be 80 characters or fewer");
        }
        if let Some(home) = &homebrew.homepage
            && !home.starts_with("https://")
        {
            bail!("simit.toml: [homebrew].homepage must start with https://");
        }
        if !homebrew.platforms.any_enabled() {
            bail!("simit.toml: [homebrew].platforms has all platforms disabled");
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

fn merge<T>(cli: Option<T>, cfg: Option<T>, metadata: Option<T>, name: &str) -> Result<T> {
    cli.or(cfg)
        .or(metadata)
        .ok_or_else(|| anyhow!("{}", missing_message(name)))
}

fn missing_message(name: &str) -> String {
    match name {
        "name" => {
            "homebrew.name not set: provide it via --homebrew-name, simit.toml [homebrew].name, or Cargo.toml package.name"
        }
        "tap_url" => {
            "homebrew.tap_url not set: provide it via --homebrew-tap or simit.toml [homebrew].tap_url"
        }
        "description" => {
            "homebrew.description not set: provide it via --homebrew-description, simit.toml [homebrew].description, or Cargo.toml package.description"
        }
        "homepage" => {
            "homebrew.homepage not set: provide it via --homebrew-homepage, simit.toml [homebrew].homepage, or Cargo.toml package.homepage"
        }
        "license" => {
            "homebrew.license not set: provide it via --homebrew-license, simit.toml [homebrew].license, or Cargo.toml package.license"
        }
        "download_repo" => {
            "homebrew.download_repo not set: provide it via --homebrew-download-repo or simit.toml [homebrew].download_repo"
        }
        _ => "homebrew setting not set",
    }
    .to_owned()
}
