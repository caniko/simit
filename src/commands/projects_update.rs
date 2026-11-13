//! Workstation-declared CI updates. Discovery supplies observations; this
//! operator-owned configuration supplies deployment authority.
use anyhow::{Context, Result, ensure};
use clap::Args as ClapArgs;
use std::path::PathBuf;

mod checkpoint;
mod engine;
mod forgejo;
mod github;
mod pin;
mod sandbox;
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
    /// Explicit flake-input pin work items written by fleetix.
    #[arg(long, value_name = "PATH", conflicts_with = "repo")]
    pub pin_file: Option<PathBuf>,
    #[arg(long, conflicts_with_all = ["push", "status"])]
    pub dry_run: bool,
    #[arg(long, conflicts_with = "status")]
    pub push: bool,
    #[arg(long)]
    pub status: bool,
    /// Explain every enrolled coordinate without forge requests or candidate generation.
    #[arg(long, conflicts_with_all = ["push", "status", "dry_run", "limit"])]
    pub plan: bool,
    #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(0..=21600))]
    pub wait_seconds: u64,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    #[arg(long)]
    pub json: bool,
}

/// Native implementation of the versioned adapter protocol.
#[derive(Debug, ClapArgs)]
pub struct AdapterArgs {
    #[arg(long)]
    pub request: PathBuf,
}

pub fn adapter(args: AdapterArgs) -> Result<()> {
    respond(args, github::execute)
}

pub fn forgejo_adapter(args: AdapterArgs) -> Result<()> {
    respond(args, forgejo::execute)
}

fn respond(
    args: AdapterArgs,
    execute: fn(&AdapterRequest) -> Result<AdapterResponse>,
) -> Result<()> {
    let request: AdapterRequest = crate::review::read_json(&args.request)?;
    request.validate()?;
    let response = execute(&request)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let config: Config = crate::review::read_json(
        &args
            .config
            .canonicalize()
            .context("resolve updater policy file")?,
    )?;
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
    // The work list is a one-shot input, not policy: validate it before any
    // state or inventory work, and bind it into the rollout key.
    let pins = match &args.pin_file {
        Some(path) => {
            let file: PinFile =
                crate::review::read_json(path).context("read pin work-item file")?;
            file.validate()?;
            Some(file.items)
        }
        None => None,
    };
    let selection_policy = engine::policy_digest(&config, profile, workspace, None, None, None)?;
    let selection_key = match &pins {
        Some(pins) => {
            crate::review::canonical_digest(&(&args.profile, &selection_policy, &args.repo, pins))?
        }
        None => crate::review::canonical_digest(&(&args.profile, &selection_policy, &args.repo))?,
    };
    let frozen_path = engine::selection_path(&state_directory, &selection_key);
    let prior: Option<engine::FrozenSelection> = if args.push && frozen_path.exists() {
        Some(crate::review::read_json(&frozen_path)?)
    } else {
        None
    };
    let mut skipped_reports = Vec::new();
    let mut pin_selection_empty = false;
    let mut frozen = if let Some(prior) = prior.filter(|p| !p.completed) {
        prior
    } else {
        let inventory: Inventory = if let Some(command) = &workspace.inventory.command {
            let output = checkpoint::protect(&state_directory, &workspace.root, || {
                sandbox::inventory(command)
            })
            .context("read-only inventory discovery failed")?;
            serde_json::from_slice(&output)?
        } else {
            crate::review::read_json(
                workspace
                    .inventory
                    .file
                    .as_ref()
                    .context("missing inventory file")?,
            )?
        };
        let (selection, skipped) = if let Some(pins) = &pins {
            let (selection, skipped) = select_pins(&config, profile, workspace, &inventory, pins)?;
            pin_selection_empty = selection.is_empty();
            (selection, skipped)
        } else {
            (
                select(&config, profile, workspace, &inventory, &args.repo)?,
                Vec::new(),
            )
        };
        skipped_reports = skipped;
        if args.plan {
            // ponytail: scan enrollment × observations; index by coordinate if
            // large inventories make selection planning materially expensive.
            let enrollment = profile
                .select
                .coordinates
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|coordinate| {
                    let observations = inventory
                        .projects
                        .iter()
                        .filter(|project| &project.coordinate == coordinate)
                        .map(|project| -> Result<_> {
                            let reason = if config.forges.contains_key(&project.forge) {
                                profile.select.exclusion_reason(project)?
                            } else {
                                Some("unknown-forge")
                            };
                            Ok(serde_json::json!({"project": project, "reason": reason}))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let disabled = profile
                        .project_overrides
                        .get(coordinate)
                        .filter(|override_| !override_.enable);
                    let status = if disabled.is_some() {
                        "disabled"
                    } else if selection
                        .iter()
                        .any(|project| &project.coordinate == coordinate)
                    {
                        "selected"
                    } else if observations.is_empty() {
                        "missing"
                    } else if !args.repo.is_empty() && !args.repo.contains(coordinate) {
                        "narrowed"
                    } else {
                        "filtered"
                    };
                    Ok(serde_json::json!({
                        "coordinate": coordinate, "status": status,
                        "reason": disabled.and_then(|override_| override_.reason.as_deref()),
                        "blocker": disabled.and_then(|override_| override_.blocker.as_deref()),
                        "observations": observations,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema": "simit.updater-selection-plan.v1",
                        "profile": args.profile, "workspace": profile.workspace,
                        "root": workspace.root, "qualification": "not-run",
                        "inventoryDigest": crate::review::canonical_digest(&inventory)?,
                        "policyDigest": selection_policy, "selected": selection,
                        "enrollment": enrollment,
                    }))?
                );
            } else {
                for entry in &enrollment {
                    println!(
                        "{}: {}",
                        entry["coordinate"].as_str().unwrap_or("?"),
                        entry["status"].as_str().unwrap_or("?")
                    );
                    for observation in entry["observations"].as_array().into_iter().flatten() {
                        if let Some(reason) = observation["reason"].as_str() {
                            println!(
                                "  {}: {reason}",
                                observation["project"]["relativePath"]
                                    .as_str()
                                    .unwrap_or("?")
                            );
                        }
                    }
                    if let Some(reason) = entry["reason"].as_str() {
                        println!("  {reason}");
                    }
                }
            }
            return Ok(());
        }
        for canary in &profile.rollout.canaries {
            ensure!(
                args.repo.is_empty() || args.repo.contains(canary),
                "CLI narrowing must include configured canaries"
            );
            ensure!(
                pins.is_some()
                    || selection
                        .iter()
                        .any(|project| &project.coordinate == canary),
                "configured canary is outside eligible selection: {canary}"
            );
        }
        ensure!(
            !selection.is_empty() || pins.is_some(),
            "profile selects no eligible repositories"
        );
        engine::FrozenSelection {
            projects: selection,
            pins: pins.clone(),
            inventory_digest: crate::review::canonical_digest(&inventory)?,
            completed: false,
        }
    };
    if args.push {
        engine::save(&frozen_path, &frozen)?;
    }
    let mut reports = skipped_reports;
    let mut failed = false;
    let count = args
        .limit
        .unwrap_or(profile.rollout.batch_size)
        .min(profile.rollout.batch_size) as usize;
    // ponytail: serial transactions stay below maxConcurrent; add bounded workers
    // only after independent per-repository state locking is introduced.
    for project in frozen.projects.iter().take(count) {
        let item = frozen.pins.as_ref().and_then(|items| {
            items
                .iter()
                .find(|item| item.coordinate == project.coordinate)
        });
        match engine::update(
            &config,
            &args.profile,
            profile,
            workspace,
            project,
            args.push,
            args.wait_seconds,
            item,
        ) {
            Ok(receipt) => {
                // A pull request awaiting approval parks; only a canary blocks on it.
                let complete = matches!(
                    receipt.phase.as_str(),
                    "unchanged" | "preview" | "qualified"
                ) || (receipt.phase == "proposed"
                    && !profile.rollout.canaries.contains(&project.coordinate));
                let mut report = serde_json::to_value(receipt)?;
                if let Some(item) = item {
                    report["input"] = item.input.clone().into();
                    report["target"] = item.target.clone().into();
                }
                reports.push(report);
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
                // An unrewritable declaration drops this item without failing
                // the rollout; the report carries the reason.
                if let Some(item) = item
                    && let Some(skip) = error.downcast_ref::<pin::PinSkip>()
                {
                    reports.push(serde_json::json!({
                        "coordinate": project.coordinate,
                        "input": item.input, "target": item.target,
                        "phase": "skipped", "reason": skip.reason(),
                    }));
                    continue;
                }
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
                    && matches!(
                        r["phase"].as_str(),
                        Some("qualified" | "unchanged" | "skipped")
                    )
            })
        });
        let retried = reports
            .iter()
            .filter(|r| {
                !profile.rollout.stop_on_failure || matches!(r["phase"].as_str(), Some("proposed"))
            })
            .filter_map(|r| r["coordinate"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        // Keep incomplete canaries at the front as barriers; retry ordinary
        // failures and parked pull requests after unattempted repositories.
        frozen.projects.sort_by_key(|p| {
            !profile.rollout.canaries.contains(&p.coordinate)
                && retried.contains(p.coordinate.as_str())
        });
        frozen.completed = frozen.projects.is_empty();
        engine::save(&frozen_path, &frozen)?;
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in &reports {
            print!(
                "{}: {}",
                report["coordinate"].as_str().unwrap_or("?"),
                report["phase"].as_str().unwrap_or("?")
            );
            if let Some(reason) = report["reason"].as_str() {
                print!(" ({reason})");
            }
            println!();
            if let Some(diff) = report["diff"].as_str() {
                print!("{diff}");
            }
            if let Some(error) = report["error"].as_str() {
                eprintln!("{error}");
            }
        }
    }
    ensure!(
        !pin_selection_empty,
        "no pin work item matches this profile's eligible selection"
    );
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

/// Pin items narrow selection like `--repo`, but an item outside the eligible
/// selection is reported and skipped instead of failing the rollout.
fn select_pins(
    config: &Config,
    profile: &Profile,
    workspace: &Workspace,
    inventory: &Inventory,
    items: &[PinItem],
) -> Result<(Vec<Project>, Vec<serde_json::Value>)> {
    let eligible = select(config, profile, workspace, inventory, &[])?;
    let wanted = items
        .iter()
        .map(|item| item.coordinate.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    // `select` already orders canaries first; keep that order among the items.
    let projects = eligible
        .into_iter()
        .filter(|project| wanted.contains(project.coordinate.as_str()))
        .collect::<Vec<_>>();
    let selected = projects
        .iter()
        .map(|project| project.coordinate.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut skipped = Vec::new();
    for item in items {
        if selected.contains(item.coordinate.as_str()) {
            continue;
        }
        skipped.push(serde_json::json!({
            "coordinate": item.coordinate, "input": item.input,
            "target": item.target, "phase": "skipped",
            "reason": pin_skip_reason(config, profile, inventory, &item.coordinate)?,
        }));
    }
    Ok((projects, skipped))
}

fn pin_skip_reason(
    config: &Config,
    profile: &Profile,
    inventory: &Inventory,
    coordinate: &str,
) -> Result<String> {
    let observations = inventory
        .projects
        .iter()
        .filter(|project| project.coordinate == coordinate)
        .collect::<Vec<_>>();
    if observations.is_empty() {
        return Ok("missing".into());
    }
    if profile
        .project_overrides
        .get(coordinate)
        .is_some_and(|override_| !override_.enable)
    {
        return Ok("disabled".into());
    }
    for project in observations {
        if !config.forges.contains_key(&project.forge) {
            return Ok("unknown-forge".into());
        }
        if let Some(reason) = profile.select.exclusion_reason(project)? {
            return Ok(reason.into());
        }
    }
    Ok("not-eligible".into())
}
