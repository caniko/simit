use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{HomebrewAction, HomebrewBumpArgs, HomebrewCommand, HomebrewRenderArgs};
use crate::commands::scaffold::{BumpFlow, WriteArtifact};
use crate::config::{HomebrewOverrides, ProjectConfig, ResolvedHomebrew};
use crate::registry::{self, FeatureStatus};
use crate::render::homebrew_formula::{self, Platform, Sha256Set};
use crate::sha256;

pub fn run(command: HomebrewCommand) -> Result<()> {
    match command.action {
        HomebrewAction::Render(args) => render(args),
        HomebrewAction::Bump(args) => bump(args),
    }
}

fn render(args: HomebrewRenderArgs) -> Result<()> {
    let (resolved, package_version) = resolve(args.homebrew.as_overrides())?;
    let version = args.version.as_deref().unwrap_or(&package_version);
    validate_version(version)?;
    let formula = homebrew_formula::render(&resolved, version, &Sha256Set::all_no_check());

    if let Some(output) = args.output {
        let path = output.as_std_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(path, formula).with_context(|| format!("writing {}", path.display()))?;
    } else {
        print!("{formula}");
    }

    Ok(())
}

fn bump(args: HomebrewBumpArgs) -> Result<()> {
    validate_version(&args.version)?;
    let (resolved, _) = resolve(args.homebrew.as_overrides())?;
    validate_download_repo(&resolved.download_repo)?;
    let archives = parse_archives(&args.archive)?;
    let sha256s = sha256_set(&resolved, &archives)?;
    let formula = homebrew_formula::render(&resolved, &args.version, &sha256s);
    let formula_path = args
        .tap
        .as_std_path()
        .join("Formula")
        .join(format!("{}.rb", resolved.name));
    let staged_path = format!("Formula/{}.rb", resolved.name);
    let flow = BumpFlow {
        repo: args.tap.as_std_path(),
        artifact: WriteArtifact {
            path: &formula_path,
            contents: &formula,
        },
        staged_path: &staged_path,
        working_tree_label: "tap",
        default_branch_hint: "tap",
    };
    flow.write()?;

    if args.push {
        let commit_message = args
            .commit_message
            .unwrap_or_else(|| format!("{} {}", resolved.name, args.version));
        flow.push(&commit_message)?;
    }

    registry::touch_current_project_or_warn([("homebrew", FeatureStatus::Managed)]);
    Ok(())
}

fn resolve(overrides: HomebrewOverrides<'_>) -> Result<(ResolvedHomebrew, String)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::select_packages(&metadata, &[], false)?
        .into_iter()
        .next()
        .expect("single package selected");
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_homebrew(overrides, &package)?;
    Ok((resolved, package.version))
}

fn parse_archives(values: &[String]) -> Result<BTreeMap<Platform, PathBuf>> {
    let mut archives = BTreeMap::new();
    for value in values {
        let (platform, path) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--archive must be PLATFORM=PATH"))?;
        let platform = Platform::parse(platform).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("archive path for {} must not be empty", platform.key());
        }
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("archive path does not exist: {}", path.display());
        }
        if archives.insert(platform, path).is_some() {
            bail!("duplicate archive for {}", platform.key());
        }
    }
    Ok(archives)
}

fn sha256_set(
    resolved: &ResolvedHomebrew,
    archives: &BTreeMap<Platform, PathBuf>,
) -> Result<Sha256Set> {
    let enabled = homebrew_formula::enabled_platforms(&resolved.platforms);
    let enabled_set = enabled.iter().copied().collect::<BTreeSet<_>>();
    for platform in archives.keys() {
        if !enabled_set.contains(platform) {
            bail!("archive provided for disabled platform {}", platform.key());
        }
    }

    let mut sha256s = Sha256Set::default();
    for platform in enabled {
        let path = archives
            .get(&platform)
            .ok_or_else(|| anyhow::anyhow!("missing --archive for {}", platform.key()))?;
        sha256s.set(platform, sha256::sha256_of_file(path)?);
    }
    Ok(sha256s)
}

fn validate_version(version: &str) -> Result<()> {
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

fn validate_download_repo(value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("homebrew.download_repo must be OWNER/REPO");
    }
    Ok(())
}
