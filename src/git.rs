use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use semver::Version;

use crate::cargo::VersionPlan;

pub fn commit_preflight(
    workspace_root: &Path,
    plans: &[VersionPlan],
    create_tag: bool,
    sign_tag: bool,
    version: &Version,
) -> Result<()> {
    ensure_git_identity(workspace_root)?;
    ensure_attached_head(workspace_root)?;
    ensure_paths_clean(workspace_root, &version_paths(workspace_root, plans))?;
    if create_tag {
        ensure_tag_absent(workspace_root, version)?;
        if sign_tag {
            ensure_signing_available(workspace_root)?;
        }
    }
    Ok(())
}

pub fn release_preflight(
    workspace_root: &Path,
    create_tag: bool,
    sign_tag: bool,
    version: &Version,
) -> Result<()> {
    ensure_git_identity(workspace_root)?;
    ensure_attached_head(workspace_root)?;
    if create_tag {
        ensure_tag_absent(workspace_root, version)?;
        if sign_tag {
            ensure_signing_available(workspace_root)?;
        }
    }
    Ok(())
}

pub fn version_paths(workspace_root: &Path, plans: &[VersionPlan]) -> Vec<PathBuf> {
    let mut paths = plans
        .iter()
        .map(|plan| plan.package.manifest_path.as_std_path().to_path_buf())
        .collect::<Vec<_>>();
    let lock_path = workspace_root.join("Cargo.lock");
    if lock_path.exists() {
        paths.push(lock_path);
    }
    paths.sort();
    paths.dedup();
    paths
}

pub fn stage_paths(workspace_root: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(workspace_root).args(["add", "--"]);
    for path in paths {
        command.arg(path);
    }

    let status = command.status().context("staging files")?;
    if !status.success() {
        bail!("git add failed while staging files");
    }
    Ok(())
}

pub fn commit(workspace_root: &Path, git_args: &[OsString]) -> Result<()> {
    let status = Command::new("git")
        .current_dir(workspace_root)
        .arg("commit")
        .args(git_args)
        .status()
        .context("running git commit")?;

    if !status.success() {
        bail!("git commit failed");
    }
    Ok(())
}

pub fn tag(workspace_root: &Path, version: &Version, sign_tag: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(workspace_root).arg("tag");

    if sign_tag {
        command
            .arg("-s")
            .arg("-m")
            .arg(format!("Release {version}"));
    } else {
        command.arg("--no-sign");
    }

    let status = command
        .arg(version.to_string())
        .status()
        .context("creating git tag")?;

    if !status.success() {
        bail!("git tag failed for {version}");
    }
    Ok(())
}

pub fn output(workspace_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;

    if !output.status.success() {
        bail!(
            "git {} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    String::from_utf8(output.stdout).context("git output was not valid UTF-8")
}

pub fn ensure_tag_absent(workspace_root: &Path, version: &Version) -> Result<()> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/tags/{version}"))
        .output()
        .context("checking existing git tag")?;

    if output.status.success() {
        bail!("tag {version} already exists");
    }
    Ok(())
}

pub fn run_project_checks(workspace_root: &Path) -> Result<()> {
    run(workspace_root, "cargo", &["test"])?;
    run(
        workspace_root,
        "cargo",
        &[
            "clippy",
            "--all-targets",
            "--all-features",
            "--",
            "--deny",
            "warnings",
        ],
    )
}

fn ensure_git_identity(workspace_root: &Path) -> Result<()> {
    ensure_git_config(workspace_root, "user.name")?;
    ensure_git_config(workspace_root, "user.email")
}

fn ensure_git_config(workspace_root: &Path, key: &str) -> Result<()> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["config", "--get", key])
        .output()
        .with_context(|| format!("checking git config {key}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!("git config {key} is required before releasing");
    }
    Ok(())
}

fn ensure_attached_head(workspace_root: &Path) -> Result<()> {
    let status = Command::new("git")
        .current_dir(workspace_root)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .status()
        .context("checking git HEAD state")?;
    if !status.success() {
        bail!("git HEAD is detached; checkout a branch before releasing");
    }
    Ok(())
}

fn ensure_paths_clean(workspace_root: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut command = Command::new("git");
    command
        .current_dir(workspace_root)
        .args(["status", "--porcelain", "--"]);
    for path in paths {
        command.arg(path);
    }
    let output = command.output().context("checking version file status")?;
    if !output.status.success() {
        bail!("git status failed while checking version files");
    }
    if !output.stdout.is_empty() {
        bail!(
            "version files have uncommitted changes:\n{}",
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

fn ensure_signing_available(workspace_root: &Path) -> Result<()> {
    let key = Command::new("git")
        .current_dir(workspace_root)
        .args(["config", "--get", "user.signingkey"])
        .output()
        .context("checking git signing key")?;
    if !key.status.success() || key.stdout.is_empty() {
        bail!("signed tags require git config user.signingkey or use --no-sign");
    }
    Ok(())
}

fn run(workspace_root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .current_dir(workspace_root)
        .args(args)
        .status()
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} failed", args.join(" "));
    }
    Ok(())
}
