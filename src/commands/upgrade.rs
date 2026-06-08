use std::error::Error;
use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;

use crate::cargo;
use crate::cli::UpgradeCommand;
use crate::readme_badges;
use crate::registry::{self, FeatureStatus, Registry};
use crate::render::diff::unified_diff;

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
        let outcome = upgrade_one(&target, mode, command.diff, fleet);
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
) -> ProjectOutcome {
    match readme_badges::plan(path.as_std_path()) {
        Ok(upgrade) => {
            if upgrade.changed() {
                if show_diff {
                    if print_diff_header {
                        println!("==> {path}");
                    }
                    print!(
                        "{}",
                        unified_diff(
                            &path.join("README.md").to_string(),
                            &upgrade.current,
                            &upgrade.upgraded,
                        )
                    );
                }
                if mode == Mode::Write {
                    if let Err(err) = readme_badges::write(path.as_std_path(), &upgrade) {
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
                changed: upgrade.changed(),
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
