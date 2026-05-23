use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{ScoopAction, ScoopBumpArgs, ScoopCommand, ScoopRenderArgs};
use crate::commands::scaffold::{BumpFlow, WriteArtifact};
use crate::config::{ProjectConfig, ResolvedScoop, ScoopOverrides};
use crate::registry::{self, FeatureStatus};
use crate::render::scoop_manifest::{self, Architecture, ScoopChecksums};
use crate::sha256;

pub fn run(command: ScoopCommand) -> Result<()> {
    match command.action {
        ScoopAction::Render(args) => render(args),
        ScoopAction::Bump(args) => bump(args),
    }
}

fn render(args: ScoopRenderArgs) -> Result<()> {
    let (resolved, package_version) = resolve(args.scoop.as_overrides())?;
    let version = args.version.as_deref().unwrap_or(&package_version);
    validate_version(version)?;
    let manifest = scoop_manifest::render(&resolved, version, &ScoopChecksums::all_placeholder());

    if let Some(output) = args.output {
        let path = output.as_std_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(path, manifest).with_context(|| format!("writing {}", path.display()))?;
    } else {
        print!("{manifest}");
    }

    Ok(())
}

fn bump(args: ScoopBumpArgs) -> Result<()> {
    validate_version(&args.version)?;
    let (resolved, _) = resolve(args.scoop.as_overrides())?;
    validate_download_repo(&resolved.download_repo)?;
    let archives = parse_archives(&args.archive)?;
    let checksums = checksum_set(&resolved, &archives)?;
    let manifest = scoop_manifest::render(&resolved, &args.version, &checksums);
    let manifest_path = manifest_path(args.bucket.as_std_path(), &resolved.name);
    let staged_path = format!("bucket/{}.json", resolved.name);
    let flow = BumpFlow {
        repo: args.bucket.as_std_path(),
        artifact: WriteArtifact {
            path: &manifest_path,
            contents: &manifest,
        },
        staged_path: &staged_path,
        working_tree_label: "bucket",
        default_branch_hint: "bucket",
    };
    flow.write()?;

    if args.push {
        let commit_message = args
            .commit_message
            .unwrap_or_else(|| format!("{} {}", resolved.name, args.version));
        flow.push(&commit_message)?;
    }

    registry::touch_current_project_or_warn([("scoop", FeatureStatus::Managed)]);
    Ok(())
}

fn resolve(overrides: ScoopOverrides<'_>) -> Result<(ResolvedScoop, String)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::select_packages(&metadata, &[], false)?
        .into_iter()
        .next()
        .expect("single package selected");
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_scoop(overrides, &package)?;
    Ok((resolved, package.version))
}

fn manifest_path(bucket: &Path, name: &str) -> PathBuf {
    bucket.join("bucket").join(format!("{name}.json"))
}

fn parse_archives(values: &[String]) -> Result<BTreeMap<Architecture, PathBuf>> {
    let mut archives = BTreeMap::new();
    for value in values {
        let (architecture, path) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--archive must be ARCH=PATH"))?;
        let architecture = Architecture::parse(architecture).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("archive path for {} must not be empty", architecture.key());
        }
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("archive path does not exist: {}", path.display());
        }
        if archives.insert(architecture, path).is_some() {
            bail!("duplicate archive for {}", architecture.key());
        }
    }
    Ok(archives)
}

fn checksum_set(
    resolved: &ResolvedScoop,
    archives: &BTreeMap<Architecture, PathBuf>,
) -> Result<ScoopChecksums> {
    let enabled = scoop_manifest::enabled_architectures(&resolved.architectures);
    let enabled_set = enabled.iter().copied().collect::<BTreeSet<_>>();
    for architecture in archives.keys() {
        if !enabled_set.contains(architecture) {
            bail!(
                "archive provided for disabled architecture {}",
                architecture.key()
            );
        }
    }

    let mut checksums = ScoopChecksums::default();
    for architecture in enabled {
        let path = archives
            .get(&architecture)
            .ok_or_else(|| anyhow::anyhow!("missing --archive for {}", architecture.key()))?;
        checksums.set(architecture, sha256::sha256_of_file(path)?);
    }
    Ok(checksums)
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
        bail!("scoop.download_repo must be OWNER/REPO");
    }
    Ok(())
}
