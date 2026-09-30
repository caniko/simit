use anyhow::{Context, Result, bail};
use semver::Version;

use crate::cargo::Package;
use crate::cli::Platform;
use crate::config::{
    AptOverrides, AurOverrides, ChocolateyOverrides, CoprOverrides, HomebrewOverrides,
    ProjectConfig, ResolvedApt, ResolvedAur, ResolvedChocolatey, ResolvedCopr, ResolvedHomebrew,
    ResolvedScoop, ScoopOverrides,
};

/// Shared helper for `release` + `release verify`: parse the selected
/// packages and return the first version, its `name -> version` label, plus
/// any divergent `name -> version` entries. Callers keep their own error
/// wording.
pub fn divergent_versions(packages: &[Package]) -> Result<(Version, String, Vec<String>)> {
    let Some(first) = packages.first() else {
        bail!("no packages selected");
    };
    let first_version = Version::parse(&first.version)
        .with_context(|| format!("parsing version {}", first.version))?;
    let first_label = format!("{} -> {}", first.name, first.version);

    let mut divergent = Vec::new();
    for package in packages.iter().skip(1) {
        let version = Version::parse(&package.version)
            .with_context(|| format!("parsing version {}", package.version))?;
        if version != first_version {
            divergent.push(format!("{} -> {}", package.name, package.version));
        }
    }
    Ok((first_version, first_label, divergent))
}

/// Package version shared by apt/homebrew/scoop/chocolatey/winget/windows:
/// `[0-9A-Za-z.+~_-]+` without a leading `v`.
pub fn validate_version(version: &str) -> Result<()> {
    if version.starts_with('v')
        || version.is_empty()
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '~' | '_' | '-'))
    {
        bail!("version must match [0-9A-Za-z.+~_-]+ without a leading v, got: {version}");
    }
    Ok(())
}

/// `OWNER/REPO` check shared by all packaging commands. `field` is the
/// config key or CLI flag shown in the error (e.g. `"scoop.download_repo"`
/// or `"--pages-repo"`).
pub fn validate_download_repo(field: &str, value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("{field} must be OWNER/REPO");
    }
    Ok(())
}

/// Write rendered packaging output to `PATH` (creating parents) or print to
/// stdout. Shared by `homebrew render` / `scoop render`.
pub fn write_or_print(output: Option<&camino::Utf8PathBuf>, contents: &str) -> Result<()> {
    if let Some(output) = output {
        let path = output.as_std_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    } else {
        print!("{contents}");
    }
    Ok(())
}

/// Owned per-platform packaging targets for release-workflow rendering.
/// Canonical home for the 6-way `resolve_*` block copy-pasted between
/// `init release` and `release secrets contract`.
pub struct ReleasePlatformTargets {
    pub aur: Option<ResolvedAur>,
    pub copr: Option<ResolvedCopr>,
    pub apt: Option<ResolvedApt>,
    pub homebrew: Option<ResolvedHomebrew>,
    pub scoop: Option<ResolvedScoop>,
    pub chocolatey: Option<ResolvedChocolatey>,
}

pub fn resolve_release_platform_targets(
    cfg: &ProjectConfig,
    package: &Package,
    platform: Platform,
) -> Result<ReleasePlatformTargets> {
    let aur = cfg
        .aur
        .as_ref()
        .map(|_| cfg.resolve_aur_for_platform(AurOverrides::default(), package, platform))
        .transpose()?;
    let copr = cfg
        .copr
        .as_ref()
        .map(|_| cfg.resolve_copr_for_platform(CoprOverrides::default(), package, platform))
        .transpose()?;
    let apt = cfg
        .apt
        .as_ref()
        .map(|_| cfg.resolve_apt(AptOverrides::default(), package))
        .transpose()?;
    let homebrew = cfg
        .homebrew
        .as_ref()
        .map(|_| cfg.resolve_homebrew(HomebrewOverrides::default(), package))
        .transpose()?;
    let scoop = cfg
        .scoop
        .as_ref()
        .map(|_| cfg.resolve_scoop(ScoopOverrides::default(), package))
        .transpose()?;
    let chocolatey = cfg
        .chocolatey
        .as_ref()
        .map(|_| cfg.resolve_chocolatey(ChocolateyOverrides::default(), package))
        .transpose()?;
    Ok(ReleasePlatformTargets {
        aur,
        copr,
        apt,
        homebrew,
        scoop,
        chocolatey,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, version: &str) -> Package {
        serde_json::from_value(serde_json::json!({
            "id": name,
            "name": name,
            "version": version,
            "manifest_path": "Cargo.toml",
        }))
        .unwrap()
    }

    #[test]
    fn version_rejects_leading_v_and_empty() {
        assert!(validate_version("1.2.3").is_ok());
        assert!(validate_version("1.0.0-alpha+001~x_-").is_ok());
        assert!(validate_version("v1.2.3").is_err());
        assert!(validate_version("").is_err());
    }

    #[test]
    fn download_repo_requires_owner_slash_repo() {
        assert!(validate_download_repo("scoop.download_repo", "owner/repo").is_ok());
        let err = validate_download_repo("f", "owner").unwrap_err();
        assert_eq!(err.to_string(), "f must be OWNER/REPO");
        assert!(validate_download_repo("f", "a/b/c").is_err());
    }

    #[test]
    fn divergent_versions_reports_name_version_pairs() {
        assert!(divergent_versions(&[]).is_err());
        let solo = vec![package("a", "1.0.0")];
        let (version, label, divergent) = divergent_versions(&solo).unwrap();
        assert_eq!(version.to_string(), "1.0.0");
        assert_eq!(label, "a -> 1.0.0");
        assert!(divergent.is_empty());

        let same = vec![package("a", "1.0.0"), package("b", "1.0.0")];
        assert!(divergent_versions(&same).unwrap().2.is_empty());

        let mixed = vec![package("a", "1.0.0"), package("b", "2.0.0")];
        assert_eq!(
            divergent_versions(&mixed).unwrap().2,
            vec!["b -> 2.0.0".to_owned()]
        );
    }
}
