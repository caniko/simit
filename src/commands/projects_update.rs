//! Workstation-declared CI updates. Discovery supplies observations; this
//! operator-owned configuration supplies deployment authority.
use anyhow::{Context, Result, ensure};
use clap::Args as ClapArgs;
use std::path::PathBuf;

mod checkpoint;
mod engine;
mod github;
pub mod schema;
use schema::*;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[arg(long, default_value = "/etc/simit/updates.json")]
    pub config: PathBuf,
    #[arg(long)]
    pub profile: String,
    /// Narrow the configured selection to exact canonical coordinates.
    #[arg(long)]
    pub repo: Vec<String>,
    #[arg(long, conflicts_with_all = ["push", "status"])]
    pub dry_run: bool,
    #[arg(long, conflicts_with = "status")]
    pub push: bool,
    #[arg(long)]
    pub status: bool,
    #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(0..=21600))]
    pub wait_seconds: u64,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    #[arg(long)]
    pub json: bool,
}

/// Native GitHub Actions implementation of the versioned adapter protocol.
#[derive(Debug, ClapArgs)]
pub struct AdapterArgs {
    #[arg(long)]
    pub request: PathBuf,
}

pub fn adapter(args: AdapterArgs) -> Result<()> {
    let request: AdapterRequest = crate::review::read_json(&args.request)?;
    request.validate()?;
    let response = github::execute(&request)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let config: Config = crate::review::read_json(&args.config)?;
    config.validate()?;
    let profile = config
        .profiles
        .get(&args.profile)
        .context("unknown update profile")?;
    let workspace = &config.workspaces[&profile.workspace];
    if args.status {
        let receipts = engine::status(&engine::profile_directory(workspace, &args.profile))?;
        println!("{}", serde_json::to_string_pretty(&receipts)?);
        return Ok(());
    }
    let state_directory = config.state_directory(workspace)?;
    let lock = engine::lock(&state_directory)?;
    checkpoint::ensure_idle(&state_directory)?;
    let selection_policy = engine::policy_digest(&config, profile, workspace, None)?;
    let selection_key =
        crate::review::canonical_digest(&(&args.profile, &selection_policy, &args.repo))?;
    let frozen_path = engine::selection_path(&state_directory, &selection_key);
    let prior: Option<engine::FrozenSelection> = if args.push && frozen_path.exists() {
        Some(crate::review::read_json(&frozen_path)?)
    } else {
        None
    };
    let mut frozen = if let Some(prior) = prior.filter(|p| !p.completed) {
        prior
    } else {
        let inventory: Inventory = if let Some(command) = &workspace.inventory.command {
            serde_json::from_slice(&engine::command_output(command, &[], None)?)?
        } else {
            crate::review::read_json(
                workspace
                    .inventory
                    .file
                    .as_ref()
                    .context("missing inventory file")?,
            )?
        };
        let selection = select(&config, profile, workspace, &inventory, &args.repo)?;
        ensure!(
            !selection.is_empty(),
            "profile selects no eligible repositories"
        );
        engine::FrozenSelection {
            projects: selection,
            inventory_digest: crate::review::canonical_digest(&inventory)?,
            completed: false,
        }
    };
    if args.push {
        engine::save(&frozen_path, &frozen)?;
    }
    let mut reports = Vec::new();
    let mut failed = false;
    let count = args
        .limit
        .unwrap_or(profile.rollout.batch_size)
        .min(profile.rollout.batch_size) as usize;
    // ponytail: serial transactions stay below maxConcurrent; add bounded workers
    // only after independent per-repository state locking is introduced.
    for project in frozen.projects.iter().take(count) {
        match engine::update(
            &config,
            &args.profile,
            profile,
            workspace,
            project,
            args.push,
            args.wait_seconds,
        ) {
            Ok(report) => {
                let complete =
                    matches!(report.phase.as_str(), "unchanged" | "preview" | "qualified");
                reports.push(serde_json::to_value(report)?);
                if !complete {
                    failed = true;
                    if profile.rollout.stop_on_failure
                        || profile.rollout.canaries.contains(&project.coordinate)
                    {
                        break;
                    }
                }
            }
            Err(error) => {
                reports.push(serde_json::json!({"coordinate":project.coordinate,"phase":"failed","error":format!("{error:#}")}));
                failed = true;
                if profile.rollout.stop_on_failure
                    || profile.rollout.canaries.contains(&project.coordinate)
                {
                    break;
                }
            }
        }
    }
    // Successful repositories are removed only from this frozen rollout; receipts
    // retain their identity and baseline for subsequent generator revisions.
    engine::verify_lock(&state_directory, &lock)?;
    if args.push {
        frozen.projects.retain(|p| {
            !reports.iter().any(|r| {
                r["coordinate"] == p.coordinate
                    && matches!(r["phase"].as_str(), Some("qualified" | "unchanged"))
            })
        });
        if !profile.rollout.stop_on_failure {
            let attempted = reports
                .iter()
                .filter_map(|r| r["coordinate"].as_str())
                .collect::<std::collections::BTreeSet<_>>();
            // Keep incomplete canaries at the front as barriers; retry ordinary
            // failures only after repositories this rollout has not attempted.
            frozen.projects.sort_by_key(|p| {
                !profile.rollout.canaries.contains(&p.coordinate)
                    && attempted.contains(p.coordinate.as_str())
            });
        }
        frozen.completed = frozen.projects.is_empty();
        engine::save(&frozen_path, &frozen)?;
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in &reports {
            println!(
                "{}: {}",
                report["coordinate"].as_str().unwrap_or("?"),
                report["phase"].as_str().unwrap_or("?")
            );
            if let Some(diff) = report["diff"].as_str() {
                print!("{diff}");
            }
            if let Some(error) = report["error"].as_str() {
                eprintln!("{error}");
            }
        }
    }
    ensure!(
        !failed,
        "update incomplete; retained receipts identify failed or pending repositories"
    );
    Ok(())
}

fn select(
    config: &Config,
    profile: &Profile,
    workspace: &Workspace,
    inventory: &Inventory,
    narrowed: &[String],
) -> Result<Vec<Project>> {
    inventory.validate(workspace)?;
    let mut selected = std::collections::BTreeMap::new();
    for project in &inventory.projects {
        let Some(forge) = config.forges.get(&project.forge) else {
            continue;
        };
        if !project.coordinate.starts_with(&format!("{}/", forge.host)) {
            anyhow::bail!("inventory forge/coordinate mismatch");
        }
        if !profile.select.matches(project)?
            || profile
                .project_overrides
                .get(&project.coordinate)
                .is_some_and(|o| !o.enable)
        {
            continue;
        }
        ensure!(
            selected
                .insert(project.coordinate.clone(), project.clone())
                .is_none(),
            "multiple canonical checkouts for {}; repair inventory identity",
            project.coordinate
        );
    }
    for coordinate in narrowed {
        ensure!(
            selected.contains_key(coordinate),
            "repository is outside this profile's eligible selection: {coordinate}"
        );
    }
    for canary in &profile.rollout.canaries {
        ensure!(
            selected.contains_key(canary),
            "configured canary is outside eligible selection: {canary}"
        );
        ensure!(
            narrowed.is_empty() || narrowed.contains(canary),
            "CLI narrowing must include configured canaries"
        );
    }
    let mut projects = selected
        .into_values()
        .filter(|p| narrowed.is_empty() || narrowed.contains(&p.coordinate))
        .collect::<Vec<_>>();
    projects.sort_by_key(|p| {
        (
            !profile.rollout.canaries.contains(&p.coordinate),
            p.coordinate.clone(),
        )
    });
    Ok(projects)
}
