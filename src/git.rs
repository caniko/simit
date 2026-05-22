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

pub fn sync_up_preflight(workspace_root: &Path, sign_tag: bool) -> Result<()> {
    ensure_git_identity(workspace_root)?;
    ensure_attached_head(workspace_root)?;
    ensure_worktree_clean(workspace_root)?;
    if sign_tag {
        ensure_signing_available(workspace_root)?;
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

pub fn move_tag(workspace_root: &Path, version: &Version, sign_tag: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(workspace_root).args(["tag", "-f"]);

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
        .context("moving git tag")?;

    if !status.success() {
        bail!("git tag -f failed for {version}");
    }
    Ok(())
}

pub fn head_commit(workspace_root: &Path) -> Result<String> {
    rev_parse(workspace_root, "HEAD")
}

pub fn tag_ref_object(workspace_root: &Path, version: &Version) -> Result<String> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["rev-parse", "--verify"])
        .arg(format!("refs/tags/{version}"))
        .output()
        .context("checking existing git tag")?;

    if !output.status.success() {
        bail!("tag {version} does not exist");
    }

    String::from_utf8(output.stdout)
        .context("git tag object was not valid UTF-8")
        .map(|value| value.trim().to_owned())
}

pub fn tag_target_commit(workspace_root: &Path, version: &Version) -> Result<String> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["rev-parse", "--verify"])
        .arg(format!("refs/tags/{version}^{{commit}}"))
        .output()
        .context("checking existing git tag target")?;

    if !output.status.success() {
        bail!("tag {version} does not point to a commit");
    }

    String::from_utf8(output.stdout)
        .context("git tag target was not valid UTF-8")
        .map(|value| value.trim().to_owned())
}

pub fn remote_tag_ref_object(
    workspace_root: &Path,
    remote: &str,
    version: &Version,
) -> Result<String> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["ls-remote", "--tags", remote])
        .arg(format!("refs/tags/{version}"))
        .output()
        .with_context(|| format!("checking remote tag {remote}/{version}"))?;

    if !output.status.success() {
        bail!(
            "git ls-remote failed for {remote}:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout =
        String::from_utf8(output.stdout).context("git ls-remote output was not valid UTF-8")?;
    stdout
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find_map(|(object, refname)| {
            (refname == format!("refs/tags/{version}")).then(|| object.to_owned())
        })
        .ok_or_else(|| anyhow::anyhow!("remote {remote} does not have tag {version}"))
}

pub fn push_moved_tag(
    workspace_root: &Path,
    remote: &str,
    version: &Version,
    expected_remote_object: &str,
) -> Result<()> {
    let refname = format!("refs/tags/{version}");
    let lease = format!("--force-with-lease={refname}:{expected_remote_object}");
    let refspec = format!("{refname}:{refname}");
    let status = Command::new("git")
        .current_dir(workspace_root)
        .args(["push", remote, &lease, &refspec])
        .status()
        .with_context(|| format!("pushing moved tag {version} to {remote}"))?;

    if !status.success() {
        bail!("git push failed while updating {remote} tag {version}");
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

fn ensure_worktree_clean(workspace_root: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .args(["status", "--porcelain"])
        .output()
        .context("checking worktree status")?;
    if !output.status.success() {
        bail!("git status failed while checking worktree status");
    }
    if !output.stdout.is_empty() {
        bail!(
            "worktree has uncommitted changes:\n{}",
            String::from_utf8_lossy(&output.stdout).trim()
        );
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

fn rev_parse(workspace_root: &Path, revision: &str) -> Result<String> {
    output(workspace_root, &["rev-parse", revision]).map(|value| value.trim().to_owned())
}
