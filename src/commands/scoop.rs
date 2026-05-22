use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{ScoopAction, ScoopBumpArgs, ScoopCommand, ScoopRenderArgs};
use crate::config::{ProjectConfig, ResolvedScoop, ScoopOverrides};
use crate::git;
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
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&manifest_path, manifest)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    if args.push {
        let commit_message = args
            .commit_message
            .unwrap_or_else(|| format!("{} {}", resolved.name, args.version));
        push_manifest(args.bucket.as_std_path(), &resolved.name, &commit_message)?;
    }

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

fn push_manifest(bucket: &Path, name: &str, commit_message: &str) -> Result<()> {
    guard_clean_for_push(bucket, name)?;
    let manifest = format!("bucket/{name}.json");
    run_git(bucket, ["add", "--", manifest.as_str()])?;

    let status = git::output(bucket, &["status", "--porcelain", "--", &manifest])?;
    if status.trim().is_empty() {
        if last_commit_subject(bucket).as_deref() == Some(commit_message) {
            return Ok(());
        }
        let default_branch = origin_default_branch(bucket)?;
        run_git(bucket, ["commit", "--allow-empty", "-m", commit_message])?;
        run_git(
            bucket,
            ["push", "origin", &format!("HEAD:{default_branch}")],
        )
    } else {
        let default_branch = origin_default_branch(bucket)?;
        run_git(bucket, ["commit", "-m", commit_message])?;
        run_git(
            bucket,
            ["push", "origin", &format!("HEAD:{default_branch}")],
        )
    }
}

fn guard_clean_for_push(bucket: &Path, name: &str) -> Result<()> {
    let dirty = git::output(bucket, &["status", "--porcelain"])?;
    let allowed = format!("bucket/{name}.json");
    let unrelated = dirty
        .lines()
        .filter(|line| !status_line_is_for_path(line, &allowed))
        .collect::<Vec<_>>();
    if !unrelated.is_empty() {
        bail!(
            "bucket working tree has unrelated changes:\n{}",
            unrelated.join("\n")
        );
    }
    Ok(())
}

fn status_line_is_for_path(line: &str, path: &str) -> bool {
    line.get(3..) == Some(path)
        || line
            .split_once(" -> ")
            .is_some_and(|(_, target)| target == path)
}

fn run_git<'a, I>(target: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    let status = Command::new("git")
        .current_dir(target)
        .args(&args)
        .status()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

fn origin_default_branch(bucket: &Path) -> Result<String> {
    let _ = git::output(bucket, &["remote", "set-head", "origin", "-a"]);
    let symbolic = git::output(
        bucket,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .context(
        "detecting origin default branch; run `git -C <bucket> remote set-head origin -a` locally and retry",
    )?;
    symbolic
        .trim()
        .strip_prefix("origin/")
        .map(str::to_string)
        .filter(|branch| !branch.is_empty())
        .context("origin/HEAD did not resolve to origin/<branch>")
}

fn last_commit_subject(bucket: &Path) -> Option<String> {
    git::output(bucket, &["log", "-1", "--pretty=%s"])
        .ok()
        .map(|subject| subject.trim().to_owned())
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
