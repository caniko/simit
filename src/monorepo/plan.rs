use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{Graph, covers, validate_relative_path};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub schema_version: u32,
    pub changed_paths: Vec<String>,
    pub full: bool,
    pub selected: Vec<String>,
    pub reasons: BTreeMap<String, Vec<String>>,
}

pub fn select(graph: &Graph, paths: &[String], all: bool) -> Result<Plan> {
    let mut reasons: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut full = all;
    let paths: BTreeSet<_> = paths.iter().cloned().collect();
    for path in &paths {
        validate_relative_path(path)?;
        let owners: Vec<_> = graph
            .components
            .values()
            .filter(|c| c.paths.iter().any(|p| covers(p, path)))
            .collect();
        if graph.shared_paths.iter().any(|p| covers(p, path)) || owners.is_empty() {
            full = true;
            let reason = format!(
                "{}:{path}",
                if owners.is_empty() {
                    "unowned"
                } else {
                    "shared"
                }
            );
            for id in graph.components.keys() {
                reasons
                    .entry(id.clone())
                    .or_default()
                    .insert(reason.clone());
            }
        } else {
            for component in owners {
                reasons
                    .entry(component.id.clone())
                    .or_default()
                    .insert(format!("changed:{path}"));
            }
        }
    }
    if all {
        for id in graph.components.keys() {
            reasons
                .entry(id.clone())
                .or_default()
                .insert("full".to_owned());
        }
    }
    let mut queue: VecDeque<_> = reasons.keys().cloned().collect();
    let mut visited = BTreeSet::new();
    while let Some(provider) = queue.pop_front() {
        if !visited.insert(provider.clone()) {
            continue;
        }
        for component in graph.components.values() {
            if component.depends_on.contains(&provider) {
                reasons
                    .entry(component.id.clone())
                    .or_default()
                    .insert(format!("dependent:{provider}"));
                queue.push_back(component.id.clone());
            }
        }
    }
    queue = reasons.keys().cloned().collect();
    visited.clear();
    while let Some(consumer) = queue.pop_front() {
        if !visited.insert(consumer.clone()) {
            continue;
        }
        for prerequisite in &graph.components[&consumer].depends_on {
            if !reasons.contains_key(prerequisite) {
                reasons
                    .entry(prerequisite.clone())
                    .or_default()
                    .insert(format!("prerequisite:{consumer}"));
            }
            queue.push_back(prerequisite.clone());
        }
    }
    Ok(Plan {
        schema_version: 1,
        changed_paths: paths.into_iter().collect(),
        full,
        selected: graph
            .order
            .iter()
            .filter(|id| reasons.contains_key(*id))
            .cloned()
            .collect(),
        reasons: reasons
            .into_iter()
            .map(|(id, why)| (id, why.into_iter().collect()))
            .collect(),
    })
}

/// Disabling rename detection deliberately retains old and new ownership paths.
pub fn changed_paths(root: &Path, base: &str) -> Result<Vec<String>> {
    let revision = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )?;
    let revision = std::str::from_utf8(&revision)?.trim();
    let mut paths = git(
        root,
        &["diff", "--name-only", "-z", "--no-renames", revision, "--"],
    )?;
    paths.extend(git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?);
    paths
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            Ok(std::str::from_utf8(p)
                .context("changed path is not UTF-8; full qualification is required")?
                .to_owned())
        })
        .collect()
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .context("reading monorepo Git changes")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}
