use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use camino::Utf8PathBuf;
use semver::Version;
use serde::Deserialize;
use toml_edit::DocumentMut;

fn main() {
    if let Err(err) = run(env::args_os().skip(1).collect()) {
        eprintln!("simit: {err:#}");
        std::process::exit(1);
    }
}

fn run(args: Vec<OsString>) -> Result<()> {
    let command = parse_args(args)?;
    match command {
        CommandSpec::Commit(commit) => commit.run(),
    }
}

#[derive(Debug)]
enum CommandSpec {
    Commit(CommitCommand),
}

#[derive(Debug)]
struct CommitCommand {
    bump: Bump,
    package: Option<String>,
    create_tag: bool,
    sign_tag: bool,
    git_args: Vec<OsString>,
}

impl CommitCommand {
    fn run(self) -> Result<()> {
        let start = env::current_dir().context("reading current directory")?;
        let manifest = find_manifest(&start)?;
        let metadata = cargo_metadata(&manifest)?;
        let package = select_package(&metadata, self.package.as_deref())?;
        let old_version = Version::parse(&package.version)
            .with_context(|| format!("parsing version {}", package.version))?;
        let new_version = bump_version(old_version, self.bump);
        let workspace_root = metadata.workspace_root.as_std_path();
        let manifest_path = package.manifest_path.as_std_path();

        if self.create_tag {
            ensure_tag_absent(workspace_root, &new_version)?;
        }

        update_manifest_version(manifest_path, &new_version)?;
        let lock_path = metadata.workspace_root.join("Cargo.lock");
        if lock_path.exists() {
            cargo_update(workspace_root, &package.name, &new_version)?;
        }

        stage_version_files(workspace_root, manifest_path, lock_path.exists())?;
        git_commit(workspace_root, &self.git_args)?;

        if self.create_tag {
            git_tag(workspace_root, &new_version, self.sign_tag)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bump {
    Patch,
    Minor,
    Major,
}

fn parse_args(args: Vec<OsString>) -> Result<CommandSpec> {
    let mut iter = args.into_iter();
    let Some(command) = iter.next() else {
        bail!("expected command: simit commit patch|minor|major <git commit args>");
    };

    if command != "commit" {
        bail!("unknown command {:?}; expected `commit`", command);
    }

    let mut package = None;
    let mut create_tag = true;
    let mut sign_tag = true;
    let mut bump = None;
    let mut git_args = Vec::new();

    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--no-tag") if bump.is_none() => create_tag = false,
            Some("--no-sign") if bump.is_none() => sign_tag = false,
            Some("--package") if bump.is_none() => {
                let Some(name) = iter.next() else {
                    bail!("--package requires a package name");
                };
                package = Some(
                    name.into_string()
                        .map_err(|_| anyhow!("--package value must be UTF-8"))?,
                );
            }
            Some("patch") if bump.is_none() => {
                bump = Some(Bump::Patch);
                git_args.extend(iter);
                break;
            }
            Some("minor") if bump.is_none() => {
                bump = Some(Bump::Minor);
                git_args.extend(iter);
                break;
            }
            Some("major") if bump.is_none() => {
                bump = Some(Bump::Major);
                git_args.extend(iter);
                break;
            }
            _ if bump.is_none() => {
                bail!("expected patch, minor, or major before git commit arguments");
            }
            _ => unreachable!("remaining git arguments are collected after bump"),
        }
    }

    let Some(bump) = bump else {
        bail!("expected patch, minor, or major");
    };

    Ok(CommandSpec::Commit(CommitCommand {
        bump,
        package,
        create_tag,
        sign_tag,
        git_args,
    }))
}

fn find_manifest(start: &Path) -> Result<PathBuf> {
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

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: Utf8PathBuf,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    manifest_path: Utf8PathBuf,
}

fn cargo_metadata(manifest: &Path) -> Result<Metadata> {
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

fn select_package<'a>(metadata: &'a Metadata, requested: Option<&str>) -> Result<&'a Package> {
    let workspace_packages = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .collect::<Vec<_>>();

    if let Some(name) = requested {
        return workspace_packages
            .into_iter()
            .find(|package| package.name == name)
            .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"));
    }

    match workspace_packages.as_slice() {
        [package] => Ok(package),
        [] => bail!("workspace has no packages"),
        _ => bail!("workspace has multiple packages; rerun with --package <name>"),
    }
}

fn bump_version(mut version: Version, bump: Bump) -> Version {
    version.pre = semver::Prerelease::EMPTY;
    version.build = semver::BuildMetadata::EMPTY;

    match bump {
        Bump::Patch => version.patch += 1,
        Bump::Minor => {
            version.minor += 1;
            version.patch = 0;
        }
        Bump::Major => {
            version.major += 1;
            version.minor = 0;
            version.patch = 0;
        }
    }

    version
}

fn update_manifest_version(manifest_path: &Path, version: &Version) -> Result<()> {
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

fn cargo_update(workspace_root: &Path, package: &str, version: &Version) -> Result<()> {
    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args(["update", "-p", package, "--precise"])
        .arg(version.to_string())
        .status()
        .context("running cargo update")?;

    if !status.success() {
        bail!("cargo update failed while updating Cargo.lock");
    }

    Ok(())
}

fn stage_version_files(workspace_root: &Path, manifest_path: &Path, has_lock: bool) -> Result<()> {
    let mut command = Command::new("git");
    command.current_dir(workspace_root).args(["add", "--"]);
    command.arg(manifest_path);
    if has_lock {
        command.arg(workspace_root.join("Cargo.lock"));
    }

    let status = command.status().context("staging version files")?;
    if !status.success() {
        bail!("git add failed while staging version files");
    }

    Ok(())
}

fn git_commit(workspace_root: &Path, git_args: &[OsString]) -> Result<()> {
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

fn ensure_tag_absent(workspace_root: &Path, version: &Version) -> Result<()> {
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

fn git_tag(workspace_root: &Path, version: &Version, sign_tag: bool) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_patch() {
        assert_eq!(
            bump_version(Version::parse("1.2.3").unwrap(), Bump::Patch).to_string(),
            "1.2.4"
        );
    }

    #[test]
    fn bumps_minor_and_resets_patch() {
        assert_eq!(
            bump_version(Version::parse("1.2.3").unwrap(), Bump::Minor).to_string(),
            "1.3.0"
        );
    }

    #[test]
    fn bumps_major_and_resets_minor_patch() {
        assert_eq!(
            bump_version(Version::parse("1.2.3").unwrap(), Bump::Major).to_string(),
            "2.0.0"
        );
    }

    #[test]
    fn clears_pre_release_and_build_metadata() {
        assert_eq!(
            bump_version(
                Version::parse("1.2.3-alpha.1+build.7").unwrap(),
                Bump::Patch
            )
            .to_string(),
            "1.2.4"
        );
    }
}
