use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Write};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use crate::cargo::{self, Metadata, Package};
use crate::cli::{ReleaseAction, ReleaseCommand};

#[derive(Debug, Clone)]
struct ReleasePlan {
    entries: Vec<ReleasePlanEntry>,
    skipped_members: Vec<String>,
}

#[derive(Debug, Clone)]
struct ReleasePlanEntry {
    package: Package,
    depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct JsonPlanEntry {
    name: String,
    version: String,
    manifest_path: Utf8PathBuf,
    publish: bool,
    depends_on: Vec<String>,
}

pub fn run(command: ReleaseCommand) -> Result<()> {
    reject_non_plan_flags(&command)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let plan = build_release_plan(&metadata, &command.packages)?;

    if command.json {
        let json = plan
            .entries
            .iter()
            .map(|entry| JsonPlanEntry {
                name: entry.package.name.clone(),
                version: entry.package.version.clone(),
                manifest_path: entry.package.manifest_path.clone(),
                publish: entry.package.is_publishable(),
                depends_on: entry.depends_on.clone(),
            })
            .collect::<Vec<_>>();
        serde_json::to_writer_pretty(io::stdout(), &json).context("writing release plan JSON")?;
        println!();
    } else {
        print_release_plan(&plan)?;
    }

    if command.dry_run_package {
        run_dry_run_package(&plan.entries)?;
    }

    Ok(())
}

fn reject_non_plan_flags(command: &ReleaseCommand) -> Result<()> {
    if command.action != ReleaseAction::Plan {
        bail!("internal error: release plan called with non-plan action");
    }
    if command.trust_action.is_some() || command.trust_key.is_some() || command.trust_root.is_some()
    {
        bail!("release trust arguments are only valid with `simit release trust`");
    }
    if command.no_tag {
        bail!("--no-tag is not valid with `simit release plan`");
    }
    if command.no_sign {
        bail!("--no-sign is not valid with `simit release plan`");
    }
    if command.dry_run {
        bail!("--dry-run is not valid with `simit release plan`; use --dry-run-package");
    }
    if command.pre.is_some() {
        bail!("--pre is not valid with `simit release plan`");
    }
    if command.message.is_some() {
        bail!("-m/--message is not valid with `simit release plan`");
    }
    if command.no_changelog {
        bail!("--no-changelog is not valid with `simit release plan`");
    }
    if command.push {
        bail!("--push is not valid with `simit release plan`");
    }
    if command.remote != "origin" {
        bail!("--remote is not valid with `simit release plan`");
    }
    if command.verify_version.is_some() {
        bail!("--version is only valid with `simit release verify`");
    }
    if command.push_target.is_some() {
        bail!("--push-target is only valid with `simit release verify`");
    }
    if command.json && command.dry_run_package {
        bail!("--json cannot be combined with --dry-run-package");
    }
    Ok(())
}

fn build_release_plan(metadata: &Metadata, requested: &[String]) -> Result<ReleasePlan> {
    let workspace_packages = workspace_packages(metadata);
    if workspace_packages.is_empty() {
        bail!("workspace has no packages");
    }

    let workspace_members = workspace_packages
        .iter()
        .map(|package| package.name.clone())
        .collect::<BTreeSet<_>>();
    let packages_by_name = workspace_packages
        .iter()
        .cloned()
        .map(|package| (package.name.clone(), package))
        .collect::<BTreeMap<_, _>>();
    let packages_by_dir = workspace_packages
        .iter()
        .map(|package| {
            (
                manifest_dir(&package.manifest_path)
                    .unwrap_or_else(|| package.manifest_path.clone()),
                package.name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let skipped_members = workspace_packages
        .iter()
        .filter(|package| !package.is_publishable())
        .map(|package| package.name.clone())
        .collect::<Vec<_>>();

    let selected_roots = if requested.is_empty() {
        workspace_packages
            .iter()
            .filter(|package| package.is_publishable())
            .map(|package| package.name.clone())
            .collect::<BTreeSet<_>>()
    } else {
        let mut selected = BTreeSet::new();
        for name in requested {
            if !workspace_members.contains(name) {
                bail!("package `{name}` is not a workspace member");
            }
            let package = packages_by_name
                .get(name)
                .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"))?;
            if !package.is_publishable() {
                bail!("package `{name}` is not publishable");
            }
            selected.insert(name.clone());
        }
        selected
    };

    if selected_roots.is_empty() {
        bail!("workspace has no publishable packages");
    }

    let mut selected = BTreeSet::new();
    let mut queue = VecDeque::from_iter(selected_roots.iter().cloned());
    while let Some(name) = queue.pop_front() {
        if !selected.insert(name.clone()) {
            continue;
        }
        let package = packages_by_name
            .get(&name)
            .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"))?;
        for dependency_name in local_dependency_names(package, &packages_by_dir)? {
            let dependency_package = packages_by_name.get(&dependency_name).ok_or_else(|| {
                anyhow!(
                    "package `{}` depends on local path `{dependency_name}` which is not a workspace member",
                    package.name
                )
            })?;
            if !dependency_package.is_publishable() {
                bail!(
                    "package `{}` depends on non-publishable workspace member `{dependency_name}`",
                    package.name
                );
            }
            queue.push_back(dependency_name);
        }
    }

    let mut adjacency = BTreeMap::<String, BTreeSet<String>>::new();
    let mut incoming = BTreeMap::<String, usize>::new();
    let mut depends_on = BTreeMap::<String, Vec<String>>::new();

    for name in &selected {
        adjacency.entry(name.clone()).or_default();
        incoming.entry(name.clone()).or_insert(0);
    }

    for name in &selected {
        let package = packages_by_name
            .get(name)
            .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"))?;
        let mut local_dependencies = local_dependency_names(package, &packages_by_dir)?
            .into_iter()
            .filter(|dependency_name| selected.contains(dependency_name))
            .collect::<Vec<_>>();
        local_dependencies.sort();
        local_dependencies.dedup();
        depends_on.insert(name.clone(), local_dependencies.clone());

        for dependency_name in local_dependencies {
            let edges = adjacency.entry(dependency_name.clone()).or_default();
            if edges.insert(name.clone()) {
                *incoming.entry(name.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut ready = incoming
        .iter()
        .filter_map(|(name, count)| (*count == 0).then_some(name.clone()))
        .collect::<Vec<_>>();
    ready.sort();

    let mut ordered = Vec::new();
    while let Some(name) = pop_first(&mut ready) {
        ordered.push(name.clone());
        let mut next = adjacency
            .get(&name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        next.sort();
        for dependent in next {
            let count = incoming
                .get_mut(&dependent)
                .ok_or_else(|| anyhow!("internal error: missing indegree for `{dependent}`"))?;
            *count -= 1;
            if *count == 0 {
                ready.push(dependent);
                ready.sort();
            }
        }
    }

    if ordered.len() != selected.len() {
        let unresolved = incoming
            .into_iter()
            .filter_map(|(name, count)| (count > 0).then_some(name))
            .collect::<BTreeSet<_>>();
        let cycle = find_cycle(&unresolved, &depends_on)
            .unwrap_or_else(|| unresolved.iter().cloned().collect::<Vec<_>>());
        bail!(
            "workspace publish graph has a local dependency cycle: {}",
            cycle.join(" -> ")
        );
    }

    let entries = ordered
        .into_iter()
        .map(|name| {
            let package = packages_by_name
                .get(&name)
                .ok_or_else(|| anyhow!("package `{name}` is not a workspace member"))?
                .clone();
            Ok(ReleasePlanEntry {
                package,
                depends_on: depends_on.remove(&name).unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ReleasePlan {
        entries,
        skipped_members,
    })
}

fn workspace_packages(metadata: &Metadata) -> Vec<Package> {
    metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .cloned()
        .collect()
}

fn local_dependency_names(
    package: &Package,
    packages_by_dir: &BTreeMap<Utf8PathBuf, String>,
) -> Result<Vec<String>> {
    let manifest_dir = manifest_dir(&package.manifest_path).ok_or_else(|| {
        anyhow!(
            "package `{}` has manifest path without a parent directory",
            package.name
        )
    })?;

    let mut names = Vec::new();
    for dependency in &package.dependencies {
        let Some(path) = dependency.path.as_ref() else {
            continue;
        };
        let dependency_dir = normalize_dependency_dir(&manifest_dir, path);
        if let Some(name) = packages_by_dir.get(&dependency_dir) {
            names.push(name.clone());
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn manifest_dir(manifest_path: &Utf8Path) -> Option<Utf8PathBuf> {
    manifest_path.parent().map(Utf8Path::to_path_buf)
}

fn normalize_dependency_dir(base_dir: &Utf8Path, dependency_path: &Utf8Path) -> Utf8PathBuf {
    let joined = if dependency_path.is_absolute() {
        dependency_path.to_path_buf()
    } else {
        base_dir.join(dependency_path)
    };
    clean_utf8_path(&joined)
}

fn clean_utf8_path(path: &Utf8Path) -> Utf8PathBuf {
    let mut cleaned = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir => {
                cleaned.pop();
            }
            _ => cleaned.push(component.as_str()),
        }
    }
    cleaned
}

fn pop_first(values: &mut Vec<String>) -> Option<String> {
    if values.is_empty() {
        None
    } else {
        Some(values.remove(0))
    }
}

fn find_cycle(
    unresolved: &BTreeSet<String>,
    depends_on: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    fn visit(
        node: &str,
        unresolved: &BTreeSet<String>,
        depends_on: &BTreeMap<String, Vec<String>>,
        stack: &mut Vec<String>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if !visiting.insert(node.to_owned()) {
            let start = stack.iter().position(|name| name == node)?;
            let mut cycle = stack[start..].to_vec();
            cycle.push(node.to_owned());
            return Some(cycle);
        }
        if !visited.insert(node.to_owned()) {
            visiting.remove(node);
            return None;
        }

        stack.push(node.to_owned());
        let mut dependencies = depends_on.get(node).cloned().unwrap_or_default();
        dependencies.sort();
        for dependency in dependencies {
            if !unresolved.contains(&dependency) {
                continue;
            }
            if let Some(cycle) = visit(
                &dependency,
                unresolved,
                depends_on,
                stack,
                visiting,
                visited,
            ) {
                return Some(cycle);
            }
        }
        stack.pop();
        visiting.remove(node);
        None
    }

    let mut stack = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for node in unresolved {
        if let Some(cycle) = visit(
            node,
            unresolved,
            depends_on,
            &mut stack,
            &mut visiting,
            &mut visited,
        ) {
            return Some(cycle);
        }
    }
    None
}

fn print_release_plan(plan: &ReleasePlan) -> Result<()> {
    println!("publish order ({} crates):", plan.entries.len());
    for (index, entry) in plan.entries.iter().enumerate() {
        println!(
            "  {}. {} {}",
            index + 1,
            entry.package.name,
            entry.package.version
        );
    }
    if plan.skipped_members.is_empty() {
        println!("non-publishable members skipped: (none)");
    } else {
        println!(
            "non-publishable members skipped: {}",
            plan.skipped_members.join(", ")
        );
    }
    io::stdout().flush().context("flushing release plan output")
}

fn run_dry_run_package(entries: &[ReleasePlanEntry]) -> Result<()> {
    for entry in entries {
        println!(
            "dry-run package: {} {}",
            entry.package.name, entry.package.version
        );
        let output = Command::new("cargo")
            .args([
                "package",
                "-p",
                &entry.package.name,
                "--allow-dirty",
                "--no-verify",
            ])
            .output()
            .with_context(|| format!("running cargo package for `{}`", entry.package.name))?;

        if !output.stdout.is_empty() {
            io::stdout()
                .write_all(&output.stdout)
                .context("writing cargo package stdout")?;
        }
        if !output.stderr.is_empty() {
            io::stderr()
                .write_all(&output.stderr)
                .context("writing cargo package stderr")?;
        }

        if output.status.success() {
            println!("  ok");
        } else {
            println!("  fail");
            bail!(
                "cargo package failed for `{}` with exit code {}",
                entry.package.name,
                output.status.code().unwrap_or(1)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo::Dependency;

    fn package(name: &str, publishable: bool, dependencies: &[&str]) -> Package {
        Package {
            id: format!("{name} 0.1.0 (path+file:///workspace/{name})"),
            name: name.to_owned(),
            version: "0.1.0".to_owned(),
            edition: Some("2024".to_owned()),
            authors: Vec::new(),
            license: None,
            description: None,
            homepage: None,
            rust_version: None,
            publish: if publishable { None } else { Some(Vec::new()) },
            features: BTreeMap::new(),
            dependencies: dependencies
                .iter()
                .map(|dependency| Dependency {
                    source: None,
                    path: Some(Utf8PathBuf::from(format!("../{dependency}"))),
                })
                .collect(),
            manifest_path: Utf8PathBuf::from(format!("/workspace/{name}/Cargo.toml")),
        }
    }

    fn metadata(packages: Vec<Package>) -> Metadata {
        let workspace_members = packages.iter().map(|package| package.id.clone()).collect();
        Metadata {
            packages,
            workspace_members,
            workspace_root: Utf8PathBuf::from("/workspace"),
        }
    }

    #[test]
    fn build_release_plan_orders_publishable_workspace_members() {
        let metadata = metadata(vec![
            package("c", true, &["b"]),
            package("b", true, &["a"]),
            package("a", true, &[]),
            package("xtask", false, &[]),
        ]);

        let plan = build_release_plan(&metadata, &[]).unwrap();
        let names = plan
            .entries
            .iter()
            .map(|entry| entry.package.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["a", "b", "c"]);
        assert_eq!(plan.skipped_members, vec!["xtask"]);
        assert_eq!(plan.entries[1].depends_on, vec!["a"]);
        assert_eq!(plan.entries[2].depends_on, vec!["b"]);
    }

    #[test]
    fn build_release_plan_reports_cycles() {
        let metadata = metadata(vec![package("a", true, &["b"]), package("b", true, &["a"])]);

        let error = build_release_plan(&metadata, &[]).unwrap_err().to_string();
        assert!(error.contains("workspace publish graph has a local dependency cycle"));
        assert!(error.contains("a -> b -> a") || error.contains("b -> a -> b"));
    }
}
