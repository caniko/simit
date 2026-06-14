use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;

use crate::cargo;
use crate::cli::UpgradeCommand;
use crate::readme_badges;
use crate::registry::{self, FeatureStatus, Registry};
use crate::render::ci::{self, CodebergPagesOptions};
use crate::render::diff::unified_diff;
use crate::user_config::ResolvedRunner;

#[derive(Debug)]
pub struct UpgradeExit {
    code: i32,
    message: String,
}

impl UpgradeExit {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> i32 {
        self.code
    }
}

impl fmt::Display for UpgradeExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for UpgradeExit {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Write,
    DryRun,
    Check,
}

#[derive(Debug)]
struct ProjectOutcome {
    path: Utf8PathBuf,
    changed: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct FileUpgrade {
    relative_path: PathBuf,
    current: String,
    upgraded: String,
}

pub fn run(command: UpgradeCommand) -> Result<()> {
    if command.diff && !(command.dry_run || command.check) {
        bail!("simit upgrade --diff requires --dry-run or --check");
    }
    let mode = if command.check {
        Mode::Check
    } else if command.dry_run {
        Mode::DryRun
    } else {
        Mode::Write
    };

    let targets = targets(&command)?;
    if targets.is_empty() {
        println!("no registered simit-managed projects to upgrade");
        return Ok(());
    }

    let fleet = command.all || targets.len() > 1;
    let mut outcomes = Vec::new();
    for target in targets {
        let outcome = upgrade_one(&target, mode, command.diff, fleet, command.pages_only);
        if !fleet {
            match outcome.error {
                Some(error) => bail!("{error}"),
                None => {
                    print_single_outcome(&outcome, mode);
                    return check_exit(&[outcome], mode);
                }
            }
        }
        outcomes.push(outcome);
    }

    print_fleet_outcomes(&outcomes, mode);
    if mode == Mode::Write {
        refresh_successful_registry_entries(&outcomes)?;
    }
    check_exit(&outcomes, mode)
}

pub fn update_readme_badges_if_present(
    workspace_root: &Path,
    check: bool,
    show_diff: bool,
) -> Result<()> {
    if !workspace_root.join("README.md").exists() {
        return Ok(());
    }
    if !workspace_root.join("Cargo.toml").exists() {
        return Ok(());
    }
    let upgrade = readme_badges::plan(workspace_root)?;
    if !upgrade.changed() {
        return Ok(());
    }
    if show_diff {
        print!(
            "{}",
            unified_diff(
                &workspace_root.join("README.md").display().to_string(),
                &upgrade.current,
                &upgrade.upgraded,
            )
        );
    }
    if check {
        bail!("README badges are not up to date; run `simit upgrade`:\nREADME.md differs");
    }
    readme_badges::write(workspace_root, &upgrade)
}

fn targets(command: &UpgradeCommand) -> Result<Vec<Utf8PathBuf>> {
    if command.all {
        return all_registry_targets();
    }

    if !command.paths.is_empty() {
        return command
            .paths
            .iter()
            .map(|path| normalize_project_path(path))
            .collect();
    }

    let metadata = cargo::metadata_for_current_dir()?;
    Ok(vec![registry::canonical_project_path(
        metadata.workspace_root.as_std_path(),
    )?])
}

fn all_registry_targets() -> Result<Vec<Utf8PathBuf>> {
    let registry = registry::load()?;
    let mut targets = registry
        .projects
        .into_iter()
        .filter(|(path, entry)| {
            !is_ephemeral(path) && path.as_std_path().exists() && is_simit_managed(&entry.features)
        })
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    Ok(targets)
}

fn is_simit_managed(features: &std::collections::BTreeMap<String, FeatureStatus>) -> bool {
    features.iter().any(|(feature, status)| {
        feature != "changelog"
            && !matches!(status, FeatureStatus::Absent | FeatureStatus::HandRolled)
    })
}

fn is_ephemeral(path: &camino::Utf8Path) -> bool {
    let path = path.as_str();
    path == "/tmp" || path.starts_with("/tmp/")
}

fn normalize_project_path(path: &camino::Utf8Path) -> Result<Utf8PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        let current = std::env::current_dir().context("reading current directory")?;
        Utf8PathBuf::from_path_buf(current.join(path)).map_err(|path| {
            anyhow::anyhow!("project path is not valid UTF-8: {}", path.display())
        })?
    };
    if !absolute.as_std_path().exists() {
        bail!("project path does not exist: {absolute}");
    }
    registry::canonical_project_path(absolute.as_std_path())
}

fn upgrade_one(
    path: &Utf8PathBuf,
    mode: Mode,
    show_diff: bool,
    print_diff_header: bool,
    pages_only: bool,
) -> ProjectOutcome {
    match plan_project_upgrade(path.as_std_path(), pages_only) {
        Ok(upgrades) => {
            if show_diff && !upgrades.is_empty() {
                if print_diff_header {
                    println!("==> {path}");
                }
                for upgrade in &upgrades {
                    let diff_path = path.join(
                        upgrade
                            .relative_path
                            .to_str()
                            .unwrap_or("<non-utf8-generated-path>"),
                    );
                    print!(
                        "{}",
                        unified_diff(
                            diff_path.as_ref(),
                            &upgrade.current,
                            &upgrade.upgraded,
                        )
                    );
                }
            }
            if mode == Mode::Write {
                for upgrade in &upgrades {
                    if let Err(err) = write_file_upgrade(path.as_std_path(), upgrade) {
                        return ProjectOutcome {
                            path: path.clone(),
                            changed: true,
                            error: Some(format!("{err:#}")),
                        };
                    }
                }
            }
            ProjectOutcome {
                path: path.clone(),
                changed: !upgrades.is_empty(),
                error: None,
            }
        }
        Err(err) => ProjectOutcome {
            path: path.clone(),
            changed: false,
            error: Some(format!("{err:#}")),
        },
    }
}

fn plan_project_upgrade(workspace_root: &Path, pages_only: bool) -> Result<Vec<FileUpgrade>> {
    let mut upgrades = Vec::new();

    if let Some(pages) = plan_codeberg_pages_upgrade(workspace_root)? {
        upgrades.push(pages);
    }

    if pages_only {
        return Ok(upgrades);
    }

    match readme_badges::plan(workspace_root) {
        Ok(readme) if readme.changed() => upgrades.push(FileUpgrade {
            relative_path: PathBuf::from("README.md"),
            current: readme.current,
            upgraded: readme.upgraded,
        }),
        Ok(_) => {}
        Err(err) if upgrades.is_empty() => return Err(err),
        Err(err) => {
            eprintln!("simit: skipping README badge upgrade for {workspace_root:?}: {err:#}")
        }
    }

    Ok(upgrades)
}

fn write_file_upgrade(workspace_root: &Path, upgrade: &FileUpgrade) -> Result<()> {
    let path = workspace_root.join(&upgrade.relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, &upgrade.upgraded).with_context(|| format!("writing {}", path.display()))
}

fn plan_codeberg_pages_upgrade(workspace_root: &Path) -> Result<Option<FileUpgrade>> {
    let relative_path = PathBuf::from(".forgejo/workflows/pages.yaml");
    let path = workspace_root.join(&relative_path);
    let current = match fs::read_to_string(&path) {
        Ok(current) => current,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    if !workspace_root.join("flake.nix").exists() || !flake_has_deploy_pages(workspace_root)? {
        return Ok(None);
    }
    if !current.contains("nix run .#deploy-pages")
        && !current.contains("DEPLOY_REMOTE=pages-origin nix run .#deploy-pages")
    {
        return Ok(None);
    }

    let Some(repo) = infer_codeberg_repo(workspace_root)? else {
        return Ok(None);
    };
    let owner = repo
        .split_once('/')
        .map(|(owner, _)| owner.to_owned())
        .ok_or_else(|| anyhow::anyhow!("inferred Codeberg repo must be OWNER/REPO"))?;
    let runner = infer_ci_runner(workspace_root)?
        .or_else(|| infer_pages_runner(&current))
        .unwrap_or_else(|| "atlas-nix-trusted".to_owned());
    let source_branch = infer_pages_source_branch(&current).unwrap_or_else(|| "trunk".to_owned());
    let generated = ci::codeberg_pages_file(
        crate::cli::Platform::Forgejo,
        &ResolvedRunner {
            name: None,
            labels: vec![runner],
        },
        &CodebergPagesOptions {
            repo,
            owner,
            token_secret: "codeberg_token".to_owned(),
            source_branch,
            deploy_app: ".#deploy-pages".to_owned(),
        },
    )?;

    if current == generated.content {
        return Ok(None);
    }

    Ok(Some(FileUpgrade {
        relative_path,
        current,
        upgraded: generated.content,
    }))
}

fn flake_has_deploy_pages(workspace_root: &Path) -> Result<bool> {
    let flake = workspace_root.join("flake.nix");
    let content =
        fs::read_to_string(&flake).with_context(|| format!("reading {}", flake.display()))?;
    Ok(content.contains("apps.deploy-pages") || content.contains("deploy-pages ="))
}

fn infer_codeberg_repo(workspace_root: &Path) -> Result<Option<String>> {
    if let Some(repo) = infer_codeberg_repo_from_git(workspace_root)? {
        return Ok(Some(repo));
    }
    infer_codeberg_repo_from_cargo(workspace_root)
}

fn infer_codeberg_repo_from_git(workspace_root: &Path) -> Result<Option<String>> {
    let top_level = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("reading git top-level")?;
    if !top_level.status.success() {
        return Ok(None);
    }
    let top_level = String::from_utf8_lossy(&top_level.stdout);
    let top_level = Path::new(top_level.trim());
    if top_level != workspace_root {
        return Ok(None);
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .context("reading git remote.origin.url")?;
    if !output.status.success() {
        return Ok(None);
    }
    let remote = String::from_utf8_lossy(&output.stdout);
    Ok(parse_codeberg_repo(remote.trim()))
}

fn infer_codeberg_repo_from_cargo(workspace_root: &Path) -> Result<Option<String>> {
    let manifest = workspace_root.join("Cargo.toml");
    let content =
        fs::read_to_string(&manifest).with_context(|| format!("reading {}", manifest.display()))?;
    for line in content.lines() {
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix("repository") else {
            continue;
        };
        let Some((_, value)) = value.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if let Some(repo) = parse_codeberg_repo(value) {
            return Ok(Some(repo));
        }
    }
    Ok(None)
}

fn parse_codeberg_repo(value: &str) -> Option<String> {
    let path = value
        .strip_prefix("ssh://git@codeberg.org/")
        .or_else(|| value.strip_prefix("git@codeberg.org:"))
        .or_else(|| value.strip_prefix("https://codeberg.org/"))
        .or_else(|| value.strip_prefix("http://codeberg.org/"))?;
    let repo = path.strip_suffix(".git").unwrap_or(path);
    if repo.split('/').count() == 2 && repo.split('/').all(|part| !part.is_empty()) {
        Some(repo.to_owned())
    } else {
        None
    }
}

fn infer_pages_runner(content: &str) -> Option<String> {
    let line = content
        .lines()
        .find(|line| line.trim_start().starts_with("runs-on:"))?;
    Some(
        line.trim()
            .strip_prefix("runs-on:")?
            .trim()
            .trim_matches('"')
            .to_owned(),
    )
}

fn infer_ci_runner(workspace_root: &Path) -> Result<Option<String>> {
    let path = workspace_root.join(".forgejo/workflows/ci.yaml");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    if !content.contains(ci::GENERATED_WORKFLOW_MARKER) {
        return Ok(None);
    }
    Ok(infer_pages_runner(&content))
}

fn infer_pages_source_branch(content: &str) -> Option<String> {
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim_start().starts_with("branches:") {
            continue;
        }
        let trimmed = line.trim();
        if let Some(inline) = trimmed
            .strip_prefix("branches: [")
            .and_then(|value| value.strip_suffix(']'))
        {
            return Some(inline.trim_matches('"').to_owned());
        }
        while let Some(next) = lines.peek() {
            let trimmed = next.trim();
            if let Some(branch) = trimmed.strip_prefix("- ") {
                return Some(branch.trim_matches('"').to_owned());
            }
            if !next.starts_with(' ') {
                break;
            }
            lines.next();
        }
    }
    None
}

fn print_single_outcome(outcome: &ProjectOutcome, mode: Mode) {
    if outcome.changed {
        match mode {
            Mode::Write => println!("upgraded {}", outcome.path),
            Mode::DryRun => println!("would upgrade {}", outcome.path),
            Mode::Check => println!("{} needs upgrade", outcome.path),
        }
    } else {
        println!("{} is up to date", outcome.path);
    }
}

fn print_fleet_outcomes(outcomes: &[ProjectOutcome], mode: Mode) {
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut failed = 0usize;
    for outcome in outcomes {
        if let Some(error) = &outcome.error {
            failed += 1;
            println!("failed {}: {error}", outcome.path);
        } else if outcome.changed {
            changed += 1;
            match mode {
                Mode::Write => println!("upgraded {}", outcome.path),
                Mode::DryRun => println!("would upgrade {}", outcome.path),
                Mode::Check => println!("needs upgrade {}", outcome.path),
            }
        } else {
            unchanged += 1;
            println!("up to date {}", outcome.path);
        }
    }
    println!("upgrade summary: {changed} changed, {unchanged} up to date, {failed} failed");
}

fn refresh_successful_registry_entries(outcomes: &[ProjectOutcome]) -> Result<()> {
    let mut registry = registry::load()?;
    let mut changed = false;
    for outcome in outcomes {
        if outcome.error.is_some() {
            continue;
        }
        if let Some(entry) = registry.projects.get_mut(&outcome.path) {
            entry.features = registry::detect_feature_status(outcome.path.as_std_path());
            entry.last_seen = chrono::Utc::now();
            changed = true;
        }
    }
    if changed {
        registry::save(&registry)?;
    }
    Ok(())
}

fn check_exit(outcomes: &[ProjectOutcome], mode: Mode) -> Result<()> {
    let failures = outcomes
        .iter()
        .filter(|outcome| outcome.error.is_some())
        .count();
    let changes = outcomes
        .iter()
        .filter(|outcome| outcome.error.is_none() && outcome.changed)
        .count();
    if failures > 0 {
        return Err(
            UpgradeExit::new(1, format!("upgrade failed for {failures} project(s)")).into(),
        );
    }
    if mode == Mode::Check && changes > 0 {
        return Err(
            UpgradeExit::new(1, format!("{changes} project(s) need `simit upgrade`")).into(),
        );
    }
    Ok(())
}

#[allow(dead_code)]
fn _assert_registry_send_sync(_: &Registry, _: &Path) {}
