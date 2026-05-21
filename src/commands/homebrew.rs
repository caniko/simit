use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::{HomebrewAction, HomebrewBumpArgs, HomebrewCommand, HomebrewRenderArgs};
use crate::config::{HomebrewOverrides, ProjectConfig, ResolvedHomebrew};
use crate::git;
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
    if let Some(parent) = formula_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&formula_path, formula)
        .with_context(|| format!("writing {}", formula_path.display()))?;

    if args.push {
        let commit_message = args
            .commit_message
            .unwrap_or_else(|| format!("{} {}", resolved.name, args.version));
        push_formula(args.tap.as_std_path(), &resolved.name, &commit_message)?;
    }

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

fn push_formula(tap: &Path, name: &str, commit_message: &str) -> Result<()> {
    guard_clean_for_push(tap, name)?;
    let formula = format!("Formula/{name}.rb");
    run_git(tap, ["add", "--", formula.as_str()])?;

    let status = git::output(tap, &["status", "--porcelain", "--", &formula])?;
    if status.trim().is_empty() {
        if last_commit_subject(tap).as_deref() == Some(commit_message) {
            return Ok(());
        }
        let default_branch = origin_default_branch(tap)?;
        run_git(tap, ["commit", "--allow-empty", "-m", commit_message])?;
        run_git(tap, ["push", "origin", &format!("HEAD:{default_branch}")])
    } else {
        let default_branch = origin_default_branch(tap)?;
        run_git(tap, ["commit", "-m", commit_message])?;
        run_git(tap, ["push", "origin", &format!("HEAD:{default_branch}")])
    }
}

fn guard_clean_for_push(tap: &Path, name: &str) -> Result<()> {
    let dirty = git::output(tap, &["status", "--porcelain"])?;
    let allowed = format!("Formula/{name}.rb");
    let unrelated = dirty
        .lines()
        .filter(|line| !status_line_is_for_path(line, &allowed))
        .collect::<Vec<_>>();
    if !unrelated.is_empty() {
        bail!(
            "tap working tree has unrelated changes:\n{}",
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

fn origin_default_branch(tap: &Path) -> Result<String> {
    let _ = git::output(tap, &["remote", "set-head", "origin", "-a"]);
    let symbolic = git::output(
        tap,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .context(
        "detecting origin default branch; run `git -C <tap> remote set-head origin -a` locally and retry",
    )?;
    symbolic
        .trim()
        .strip_prefix("origin/")
        .map(str::to_string)
        .filter(|branch| !branch.is_empty())
        .context("origin/HEAD did not resolve to origin/<branch>")
}

fn last_commit_subject(tap: &Path) -> Option<String> {
    git::output(tap, &["log", "-1", "--pretty=%s"])
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
        bail!("homebrew.download_repo must be OWNER/REPO");
    }
    Ok(())
}
