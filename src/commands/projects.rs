use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::io::{self, IsTerminal};

use anyhow::{Context, Result, bail};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cargo;
use crate::cli::{
    ProjectsAction, ProjectsClearStateArgs, ProjectsCommand, ProjectsDiscoverArgs,
    ProjectsForgetArgs, ProjectsListArgs, ProjectsPruneArgs, ProjectsScanArgs, ProjectsShowArgs,
    ProjectsSort,
};
use crate::registry::{
    self, DiscoverOptions, DiscoverReport, FeatureStatus, ProjectEntry, Registry,
};

#[derive(Debug)]
pub struct CommandExit {
    code: i32,
    message: String,
}

impl CommandExit {
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

impl fmt::Display for CommandExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CommandExit {}

pub fn run(command: ProjectsCommand) -> Result<()> {
    match command.action {
        ProjectsAction::List(args) => list(args),
        ProjectsAction::Show(args) => show(args),
        ProjectsAction::Scan(args) => scan(args),
        ProjectsAction::Discover(args) => discover(args),
        ProjectsAction::Forget(args) => forget(args),
        ProjectsAction::Prune(args) => prune(args),
        ProjectsAction::ClearState(args) => clear_state(args),
    }
}

#[derive(Debug)]
struct FeatureFilter {
    name: String,
    status: Option<FeatureStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionItem {
    feature: String,
    status: FeatureStatus,
}

#[derive(Serialize)]
struct ProjectRecord<'a> {
    path: &'a Utf8Path,
    name: &'a str,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    features: &'a BTreeMap<String, FeatureStatus>,
}

fn list(args: ProjectsListArgs) -> Result<()> {
    let registry = registry::load()?;
    let filters = args
        .features
        .iter()
        .map(|value| parse_feature_filter(value))
        .collect::<Result<Vec<_>>>()?;
    let mut projects = registry.projects.iter().collect::<Vec<_>>();
    projects.retain(|(_, entry)| filters.iter().all(|filter| matches_filter(entry, filter)));
    sort_projects(&mut projects, args.sort);

    if args.json {
        let records = projects
            .iter()
            .map(|(path, entry)| record(path, entry))
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        let show_issues = args.issues || !args.no_issues;
        let mut attention = Vec::new();
        for (path, entry) in projects {
            let items = attention_items_for_project(path, entry);
            println!("{}", format_list_line(path, entry, show_issues, &items));
            if show_issues && !items.is_empty() {
                attention.push((path, items));
            }
        }
        if show_issues {
            print_attention_footer(&attention);
        }
    }
    Ok(())
}

fn show(args: ProjectsShowArgs) -> Result<()> {
    let registry = registry::load()?;
    let path = match args.path {
        Some(path) => normalize_registry_path(&path)?,
        None => current_workspace_root()?,
    };

    let Some(entry) = registry.projects.get(&path) else {
        if args.json || path.as_std_path().exists() {
            bail!("project is not registered: {path}");
        }
        bail!("project path does not exist and is not registered: {path}");
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&record(&path, entry))?);
    } else {
        let show_issues = args.issues || !args.no_issues;
        let attention = attention_items_for_project(&path, entry);
        if show_issues && !attention.is_empty() {
            println!(
                "{} project has {} issue{}: {}",
                attention_glyph(),
                attention.len(),
                if attention.len() == 1 { "" } else { "s" },
                format_attention_items(&attention)
            );
            println!();
        }
        print_project(&path, entry);
    }
    Ok(())
}

fn scan(args: ProjectsScanArgs) -> Result<()> {
    let mut registry = registry::load()?;
    let mut touched = 0usize;
    let mut pruned = Vec::new();
    let now = Utc::now();

    for (path, entry) in &mut registry.projects {
        if !path.as_std_path().exists() {
            if args.prune {
                pruned.push(path.clone());
            }
            continue;
        }
        entry.features = registry::detect_feature_status(path.as_std_path());
        entry.last_seen = now;
        touched += 1;
    }

    for path in &pruned {
        registry.projects.remove(path);
    }

    if args.dry_run {
        println!(
            "would scan {touched} project{}",
            if touched == 1 { "" } else { "s" }
        );
        if args.prune {
            println!(
                "would prune {} stale project{}",
                pruned.len(),
                if pruned.len() == 1 { "" } else { "s" }
            );
            for path in &pruned {
                println!("  {path}");
            }
        }
    } else {
        registry::save(&registry)?;
        println!(
            "scanned {touched} project{}",
            if touched == 1 { "" } else { "s" }
        );
        if args.prune {
            println!(
                "pruned {} stale project{}",
                pruned.len(),
                if pruned.len() == 1 { "" } else { "s" }
            );
        }
    }
    Ok(())
}

fn discover(args: ProjectsDiscoverArgs) -> Result<()> {
    let root = match args.root {
        Some(root) => root,
        None => {
            let current = std::env::current_dir().context("reading current directory")?;
            Utf8PathBuf::from_path_buf(current).map_err(|path| {
                anyhow::anyhow!("current directory is not valid UTF-8: {}", path.display())
            })?
        }
    };
    let root = match registry::canonical_project_path(root.as_std_path()) {
        Ok(root) if root.as_std_path().is_dir() => root,
        Ok(root) => {
            return Err(
                CommandExit::new(2, format!("discovery root is not a directory: {root}")).into(),
            );
        }
        Err(err) => {
            return Err(CommandExit::new(
                2,
                format!("could not start discovery from {root}: {err:#}"),
            )
            .into());
        }
    };

    let opts = DiscoverOptions {
        max_depth: args
            .max_depth
            .unwrap_or_else(|| DiscoverOptions::default().max_depth),
        follow_symlinks: args.follow_symlinks,
        include_empty: args.include_empty,
        extra_skip_dirs: args.skip,
    };

    let report = if args.dry_run {
        registry::discover_under_dry_run(root.as_std_path(), &opts)?
    } else {
        registry::discover_under(root.as_std_path(), &opts)?
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_discover_report(&root, &report)?;
    }
    Ok(())
}

fn forget(args: ProjectsForgetArgs) -> Result<()> {
    let path = normalize_registry_path(&args.path)?;
    let mut registry = registry::load()?;
    if registry.projects.remove(&path).is_none() {
        bail!("project is not registered: {path}");
    }
    registry::save(&registry)?;
    println!("forgot {path}");
    Ok(())
}

fn print_discover_report(root: &Utf8Path, report: &DiscoverReport) -> Result<()> {
    print_discover_section("Registered:", &report.registered)?;
    print_discover_section("Refreshed:", &report.refreshed)?;
    if !report.skipped_empty.is_empty() {
        println!(
            "Skipped (no simit features): {} project{}",
            report.skipped_empty.len(),
            if report.skipped_empty.len() == 1 {
                ""
            } else {
                "s"
            }
        );
    }
    if !report.errors.is_empty() {
        println!("Errors:");
        for (path, message) in &report.errors {
            println!("  {}  {path}", path.file_name().unwrap_or("<root>"));
            println!("    {message}");
        }
    }
    println!(
        "discovered {} projects under {} ({} new, {} refreshed, {} skipped, {} errors)",
        report.registered.len() + report.refreshed.len(),
        root,
        report.registered.len(),
        report.refreshed.len(),
        report.skipped_empty.len(),
        report.errors.len()
    );
    Ok(())
}

fn print_discover_section(label: &str, paths: &[Utf8PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    println!("{label}");
    for path in paths {
        let features = registry::detect_feature_status(path.as_std_path());
        let marker = if registry::uses_simit_features(&features) {
            ""
        } else {
            "  [no simit features]"
        };
        println!("  {}  {path}{marker}", discover_display_name(path)?);
    }
    Ok(())
}

fn discover_display_name(path: &Utf8Path) -> Result<String> {
    registry::package_name_for_workspace(path.as_std_path())
        .with_context(|| format!("resolving package name for {path}"))
}

fn prune(args: ProjectsPruneArgs) -> Result<()> {
    let mut registry = registry::load()?;
    let stale = registry
        .projects
        .keys()
        .filter(|path| !path.as_std_path().exists())
        .cloned()
        .collect::<Vec<_>>();

    if args.dry_run {
        println!(
            "would prune {} stale project{}",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
        for path in &stale {
            println!("  {path}");
        }
    } else {
        for path in &stale {
            registry.projects.remove(path);
        }
        registry::save(&registry)?;
        println!(
            "pruned {} stale project{}",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

fn clear_state(args: ProjectsClearStateArgs) -> Result<()> {
    let path = registry::registry_path()?;
    if args.dry_run {
        println!("would clear project registry state at {path}");
    } else {
        registry::save(&Registry::default())?;
        println!("cleared project registry state at {path}");
    }
    Ok(())
}

fn parse_feature_filter(value: &str) -> Result<FeatureFilter> {
    let (name, status) = match value.split_once('=') {
        Some((name, status)) => (name, Some(parse_feature_status(status)?)),
        None => (value, None),
    };
    if !registry::KNOWN_FEATURES.contains(&name) {
        bail!(
            "unknown feature `{name}`; valid features: {}",
            registry::KNOWN_FEATURES.join(", ")
        );
    }
    Ok(FeatureFilter {
        name: name.to_owned(),
        status,
    })
}

fn parse_feature_status(value: &str) -> Result<FeatureStatus> {
    match value {
        "managed" => Ok(FeatureStatus::Managed),
        "managed+extra" => Ok(FeatureStatus::ManagedExtra),
        "drift" => Ok(FeatureStatus::Drift),
        "hand-rolled" => Ok(FeatureStatus::HandRolled),
        "configured" => Ok(FeatureStatus::Configured),
        "conflicted" => Ok(FeatureStatus::Conflicted),
        "installed" => Ok(FeatureStatus::Installed),
        "absent" => Ok(FeatureStatus::Absent),
        _ => bail!(
            "unknown feature status `{value}`; valid statuses: managed, managed+extra, drift, hand-rolled, configured, conflicted, installed, absent"
        ),
    }
}

fn matches_filter(entry: &ProjectEntry, filter: &FeatureFilter) -> bool {
    let status = entry
        .features
        .get(&filter.name)
        .copied()
        .unwrap_or(FeatureStatus::Absent);
    match filter.status {
        Some(expected) => status == expected,
        None => status != FeatureStatus::Absent,
    }
}

fn sort_projects(projects: &mut Vec<(&Utf8PathBuf, &ProjectEntry)>, sort: ProjectsSort) {
    projects.sort_by(|(left_path, left), (right_path, right)| {
        let primary = match sort {
            ProjectsSort::Name => left.name.cmp(&right.name),
            ProjectsSort::Path => left_path.cmp(right_path),
            ProjectsSort::LastSeen => left.last_seen.cmp(&right.last_seen).reverse(),
            ProjectsSort::FirstSeen => left.first_seen.cmp(&right.first_seen).reverse(),
        };
        primary.then_with(|| left_path.cmp(right_path))
    });
}

fn format_list_line(
    path: &Utf8Path,
    entry: &ProjectEntry,
    show_issues: bool,
    attention: &[AttentionItem],
) -> String {
    let indicator = if registry::uses_simit_features(&entry.features) {
        "*"
    } else {
        "-"
    };
    let attention_marker = if show_issues && !attention.is_empty() {
        format!("{} ", attention_glyph())
    } else {
        String::new()
    };
    let features = registry::KNOWN_FEATURES
        .iter()
        .filter(|feature| {
            entry
                .features
                .get(**feature)
                .is_some_and(|status| *status != FeatureStatus::Absent)
        })
        .map(|feature| format_feature_token(feature, entry.features.get(*feature).copied()))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{attention_marker}{indicator} {} {}  [{features}]",
        entry.name, path
    )
}

fn format_feature_token(feature: &str, status: Option<FeatureStatus>) -> String {
    match status.unwrap_or(FeatureStatus::Absent) {
        FeatureStatus::Managed | FeatureStatus::Configured | FeatureStatus::Installed => {
            feature.to_owned()
        }
        FeatureStatus::ManagedExtra
        | FeatureStatus::Drift
        | FeatureStatus::HandRolled
        | FeatureStatus::Conflicted => {
            format!("{feature}({})", status_label(status.unwrap()))
        }
        FeatureStatus::Absent => feature.to_owned(),
    }
}

fn print_project(path: &Utf8Path, entry: &ProjectEntry) {
    println!("name: {}", entry.name);
    println!("path: {path}");
    println!("first_seen: {}", entry.first_seen.to_rfc3339());
    println!("last_seen: {}", entry.last_seen.to_rfc3339());
    println!();
    println!("feature     status");
    println!("-------     ------");
    for feature in registry::KNOWN_FEATURES {
        let status = entry
            .features
            .get(*feature)
            .copied()
            .unwrap_or(FeatureStatus::Absent);
        println!("{feature:<11} {}", status_label(status));
    }
}

fn status_label(status: FeatureStatus) -> &'static str {
    match status {
        FeatureStatus::Managed => "managed",
        FeatureStatus::ManagedExtra => "managed+extra",
        FeatureStatus::Drift => "drift",
        FeatureStatus::HandRolled => "hand-rolled",
        FeatureStatus::Configured => "configured",
        FeatureStatus::Conflicted => "conflicted",
        FeatureStatus::Installed => "installed",
        FeatureStatus::Absent => "absent",
    }
}

pub fn attention_items(features: &BTreeMap<String, FeatureStatus>) -> Vec<AttentionItem> {
    let mut items = Vec::new();
    for feature in ["hooks", "ci", "flake", "changelog"] {
        let Some(status) = features.get(feature).copied() else {
            continue;
        };
        let needs_attention = matches!(
            (feature, status),
            ("hooks", FeatureStatus::Conflicted)
                | ("ci" | "flake" | "changelog", FeatureStatus::Drift)
        );
        if needs_attention {
            items.push(AttentionItem {
                feature: feature.to_owned(),
                status,
            });
        }
    }
    items
}

fn attention_items_for_project(path: &Utf8Path, entry: &ProjectEntry) -> Vec<AttentionItem> {
    if is_ephemeral_project_path(path) {
        Vec::new()
    } else {
        attention_items(&entry.features)
    }
}

fn is_ephemeral_project_path(path: &Utf8Path) -> bool {
    let path = path.as_str();
    path == "/tmp" || path.starts_with("/tmp/")
}

fn print_attention_footer(attention: &[(&Utf8PathBuf, Vec<AttentionItem>)]) {
    if attention.is_empty() {
        return;
    }
    println!();
    println!("{} project(s) need attention:", attention.len());
    for (path, items) in attention {
        println!("  {path}   {}", format_attention_items(items));
    }
}

fn format_attention_items(items: &[AttentionItem]) -> String {
    items
        .iter()
        .map(|item| format!("{}={}", item.feature, status_label(item.status)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn attention_glyph() -> String {
    if io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none() {
        "\x1b[33m!\x1b[0m".to_owned()
    } else {
        "!".to_owned()
    }
}

fn record<'a>(path: &'a Utf8Path, entry: &'a ProjectEntry) -> ProjectRecord<'a> {
    ProjectRecord {
        path,
        name: &entry.name,
        first_seen: entry.first_seen,
        last_seen: entry.last_seen,
        features: &entry.features,
    }
}

fn current_workspace_root() -> Result<Utf8PathBuf> {
    match cargo::metadata_for_current_dir() {
        Ok(metadata) => registry::canonical_project_path(metadata.workspace_root.as_std_path()),
        Err(err) if format!("{err:#}").contains("could not find Cargo.toml") => {
            bail!("no Rust workspace detected; pass a project PATH")
        }
        Err(err) => Err(err).context("detecting current workspace root"),
    }
}

fn normalize_registry_path(path: &Utf8Path) -> Result<Utf8PathBuf> {
    if path.as_std_path().exists() {
        return registry::canonical_project_path(path.as_std_path());
    }
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    let current = std::env::current_dir().context("reading current directory")?;
    let joined = current.join(path);
    Utf8PathBuf::from_path_buf(joined)
        .map_err(|path| anyhow::anyhow!("project path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(items: &[(&str, FeatureStatus)]) -> BTreeMap<String, FeatureStatus> {
        items
            .iter()
            .map(|(feature, status)| ((*feature).to_owned(), *status))
            .collect()
    }

    #[test]
    fn attention_classifier_flags_hooks_conflicted() {
        let items = attention_items(&features(&[("hooks", FeatureStatus::Conflicted)]));

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].feature, "hooks");
        assert_eq!(items[0].status, FeatureStatus::Conflicted);
    }

    #[test]
    fn attention_classifier_flags_flake_drift() {
        let items = attention_items(&features(&[("flake", FeatureStatus::Drift)]));

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].feature, "flake");
        assert_eq!(items[0].status, FeatureStatus::Drift);
    }

    #[test]
    fn attention_classifier_ignores_non_issue_hook_states() {
        assert!(attention_items(&features(&[("hooks", FeatureStatus::Configured)])).is_empty());
        assert!(attention_items(&features(&[("hooks", FeatureStatus::Installed)])).is_empty());
    }

    #[test]
    fn attention_classifier_ignores_managed_and_installed_features() {
        let items = attention_items(&features(&[
            ("flake", FeatureStatus::Managed),
            ("ci", FeatureStatus::Managed),
            ("changelog", FeatureStatus::Managed),
            ("hooks", FeatureStatus::Installed),
        ]));

        assert!(items.is_empty());
    }
}
