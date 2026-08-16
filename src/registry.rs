//! Per-user project registry.
//!
//! The registry is best-effort write-side state: mutating commands update it
//! after their primary work succeeds, but registry failures must not make those
//! commands fail. Direct callers can use `load`, `save`, and `touch` when they
//! need strict error reporting.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use directories_next::ProjectDirs;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::cargo::{self, Package};
use crate::ci_resolution::{CiCliOverrides, CiInference, WorkflowSnapshot};
use crate::cli::{CiProvider, CrowWorkflowFormat, Platform};
use crate::config::{FlakeScope, HomebrewOverrides, ProjectConfig};
use crate::project;
use crate::python;
use crate::render::ci;
use crate::render::flake;
use crate::user_config::{ResolvedCiRunners, ResolvedRunner};

const HOOK_TYPES: &[&str] = &["pre-commit", "pre-push", "commit-msg"];

pub const SCHEMA_VERSION: u32 = 1;
pub const KNOWN_FEATURES: &[&str] = &[
    "flake",
    "ci",
    "homebrew",
    "chocolatey",
    "scoop",
    "aur",
    "copr",
    "apt",
    "flatpak",
    "winget",
    "changelog",
    "hooks",
];
const DEFAULT_SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    ".direnv",
    ".jj",
    "vendor",
    "result",
    "result-bin",
];
const DEFAULT_MAX_DEPTH: usize = 8;

/// Options controlling recursive Rust workspace discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverOptions {
    /// Maximum directory depth to descend from the root. The root itself is depth 0.
    pub max_depth: usize,
    /// Whether symlinked directories should be followed during discovery.
    pub follow_symlinks: bool,
    /// Whether candidates with no detected simit-managed features should be registered.
    pub include_empty: bool,
    /// Additional directory basenames to skip during traversal.
    pub extra_skip_dirs: Vec<String>,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            follow_symlinks: false,
            include_empty: false,
            extra_skip_dirs: Vec::new(),
        }
    }
}

/// Summary of a discovery run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiscoverReport {
    /// Workspaces newly added to the registry.
    pub registered: Vec<Utf8PathBuf>,
    /// Existing registry workspaces whose feature status was refreshed.
    pub refreshed: Vec<Utf8PathBuf>,
    /// Candidate workspaces skipped because all detected features were absent.
    pub skipped_empty: Vec<Utf8PathBuf>,
    /// Candidate-local errors that did not abort the full discovery run.
    pub errors: Vec<(Utf8PathBuf, String)>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Registry {
    pub schema_version: u32,
    #[serde(default, rename = "project", with = "project_entries")]
    pub projects: BTreeMap<Utf8PathBuf, ProjectEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProjectEntry {
    pub name: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub features: BTreeMap<String, FeatureStatus>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum FeatureStatus {
    Managed,
    #[serde(rename = "managed+extra")]
    ManagedExtra,
    Drift,
    #[serde(rename = "hand-rolled")]
    HandRolled,
    Configured,
    Conflicted,
    Installed,
    Absent,
}

/// Live comparison of generated CI files in one checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiAudit {
    pub status: FeatureStatus,
    pub platform: Option<String>,
    pub changed_files: Vec<PathBuf>,
    pub missing_files: Vec<PathBuf>,
    pub extra_generated_files: Vec<PathBuf>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            projects: BTreeMap::new(),
        }
    }
}

pub fn registry_path() -> Result<Utf8PathBuf> {
    let dirs =
        ProjectDirs::from("", "", "simit").context("resolving per-user simit data directory")?;
    Utf8PathBuf::from_path_buf(dirs.data_dir().join("projects.toml")).map_err(|path| {
        anyhow::anyhow!("simit registry path is not valid UTF-8: {}", path.display())
    })
}

pub fn load() -> Result<Registry> {
    let path = registry_path()?;
    load_from_path(&path)
}

pub fn save(registry: &Registry) -> Result<()> {
    let path = registry_path()?;
    let _lock = lock_for_path(&path)?;
    save_unlocked(&path, registry)
}

/// Discover Rust workspaces under `root` and update the per-user project registry.
///
/// The registry is loaded once, all discovered updates are applied in memory, and
/// the registry is saved once at the end. Candidate-local failures, such as a
/// broken manifest or failed `cargo metadata`, are recorded in the returned
/// report instead of aborting the whole walk.
pub fn discover_under(root: &Path, opts: &DiscoverOptions) -> Result<DiscoverReport> {
    if disabled() {
        return discover_under_dry_run(root, opts);
    }

    let mut registry = load()?;
    let mut report = discover_with_registry(root, opts, &mut registry)?;
    let path = registry_path()?;
    let _lock = lock_for_path(&path)?;
    save_unlocked(&path, &registry)?;
    sort_report(&mut report);
    Ok(report)
}

/// Discover Rust workspaces under `root` without writing the project registry.
///
/// The walk and per-candidate handling match [`discover_under`], but the
/// registry file is never locked or saved.
pub fn discover_under_dry_run(root: &Path, opts: &DiscoverOptions) -> Result<DiscoverReport> {
    let mut registry = load()?;
    let mut report = discover_with_registry(root, opts, &mut registry)?;
    sort_report(&mut report);
    Ok(report)
}

pub fn touch(
    workspace_root: &Path,
    package_name: &str,
    feature_updates: impl IntoIterator<Item = (&'static str, FeatureStatus)>,
) -> Result<()> {
    if disabled() {
        return Ok(());
    }

    let path = registry_path()?;
    let _lock = lock_for_path(&path)?;
    let mut registry = load_from_path(&path)?;
    touch_loaded(&mut registry, workspace_root, package_name, feature_updates)?;
    save_unlocked(&path, &registry)
}

pub fn refresh(workspace_root: &Path, package_name: &str) -> Result<()> {
    touch(workspace_root, package_name, std::iter::empty())
}

pub fn touch_current_project_or_warn(
    feature_updates: impl IntoIterator<Item = (&'static str, FeatureStatus)>,
) {
    warn_on_error(touch_current_project(feature_updates));
}

pub fn refresh_current_project_or_warn() {
    warn_on_error(refresh_current_project());
}

fn touch_current_project(
    feature_updates: impl IntoIterator<Item = (&'static str, FeatureStatus)>,
) -> Result<()> {
    if disabled() {
        return Ok(());
    }

    let Some(metadata) = metadata_for_registry()? else {
        return Ok(());
    };
    let package_name = registry_package_name(&metadata)?;
    touch(
        metadata.workspace_root.as_std_path(),
        &package_name,
        feature_updates,
    )
}

fn refresh_current_project() -> Result<()> {
    if disabled() {
        return Ok(());
    }

    let Some(metadata) = metadata_for_registry()? else {
        return Ok(());
    };
    let package_name = registry_package_name(&metadata)?;
    refresh(metadata.workspace_root.as_std_path(), &package_name)
}

fn metadata_for_registry() -> Result<Option<cargo::Metadata>> {
    match cargo::metadata_for_current_dir() {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if format!("{err:#}").contains("could not find Cargo.toml") => Ok(None),
        Err(err) => Err(err),
    }
}

fn registry_package_name(metadata: &cargo::Metadata) -> Result<String> {
    let mut names = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .map(|package| package.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .next()
        .context("workspace has no packages")
}

pub fn package_name_for_workspace(workspace_root: &Path) -> Result<String> {
    let manifest = workspace_root.join("Cargo.toml");
    if let Some(name) = package_name_from_manifest(&manifest)? {
        return Ok(name);
    }
    let metadata = cargo::cargo_metadata(&manifest)?;
    registry_package_name(&metadata)
}

fn package_name_from_manifest(manifest: &Path) -> Result<Option<String>> {
    let text =
        fs::read_to_string(manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let document = text
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parsing {}", manifest.display()))?;
    let Some(package) = document.get("package") else {
        return Ok(None);
    };
    let Some(name) = package.get("name") else {
        bail!(
            "package manifest is missing package.name: {}",
            manifest.display()
        );
    };
    let Some(name) = name.as_str() else {
        bail!("package.name must be a string: {}", manifest.display());
    };
    Ok(Some(name.to_owned()))
}

fn discover_with_registry(
    root: &Path,
    opts: &DiscoverOptions,
    registry: &mut Registry,
) -> Result<DiscoverReport> {
    validate_schema(registry)?;
    let root = canonical_utf8_path(root)?;
    let mut report = DiscoverReport::default();
    let mut queue = VecDeque::from([(root, 0_usize)]);
    let mut visited = BTreeSet::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if opts.follow_symlinks && !visited.insert(dir.clone()) {
            continue;
        }

        match workspace_candidate(&dir) {
            Ok(true) => {
                apply_discovered_candidate(&dir, opts, registry, &mut report);
                continue;
            }
            Ok(false) => {}
            Err(err) => {
                report.errors.push((dir.clone(), format!("{err:#}")));
                continue;
            }
        }

        if depth >= opts.max_depth {
            continue;
        }

        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                report
                    .errors
                    .push((dir.clone(), format!("reading directory {}: {err}", dir)));
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    report.errors.push((
                        dir.clone(),
                        format!("reading directory entry in {}: {err}", dir),
                    ));
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if should_skip_dir(&name, opts) {
                continue;
            }

            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(err) => {
                    report.errors.push((
                        dir.clone(),
                        format!("reading file type for {}: {err}", entry.path().display()),
                    ));
                    continue;
                }
            };

            if file_type.is_symlink() && !opts.follow_symlinks {
                continue;
            }
            if !(file_type.is_dir() || file_type.is_symlink()) {
                continue;
            }

            let path = entry.path();
            let metadata = if opts.follow_symlinks {
                fs::metadata(&path)
            } else {
                fs::symlink_metadata(&path)
            };
            match metadata {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => continue,
                Err(err) => {
                    report.errors.push((
                        dir.clone(),
                        format!("reading metadata for {}: {err}", path.display()),
                    ));
                    continue;
                }
            }

            match canonical_utf8_path(&path) {
                Ok(path) => queue.push_back((path, depth + 1)),
                Err(err) => report.errors.push((
                    dir.clone(),
                    format!("canonicalizing {}: {err:#}", path.display()),
                )),
            }
        }
    }

    Ok(report)
}

fn apply_discovered_candidate(
    workspace_root: &Utf8PathBuf,
    opts: &DiscoverOptions,
    registry: &mut Registry,
    report: &mut DiscoverReport,
) {
    let features = detect_feature_status(workspace_root.as_std_path());
    if !opts.include_empty && !uses_simit_features(&features) {
        report.skipped_empty.push(workspace_root.clone());
        return;
    }

    let package_name = match package_name_for_workspace(workspace_root.as_std_path()) {
        Ok(name) => name,
        Err(err) => {
            report
                .errors
                .push((workspace_root.clone(), format!("{err:#}")));
            return;
        }
    };

    let now = Utc::now();
    if let Some(entry) = registry.projects.get_mut(workspace_root) {
        entry.name = package_name;
        entry.last_seen = now;
        entry.features = features;
        report.refreshed.push(workspace_root.clone());
    } else {
        registry.projects.insert(
            workspace_root.clone(),
            ProjectEntry {
                name: package_name,
                first_seen: now,
                last_seen: now,
                features,
            },
        );
        report.registered.push(workspace_root.clone());
    }
}

fn workspace_candidate(dir: &Utf8PathBuf) -> Result<bool> {
    let manifest = dir.join("Cargo.toml");
    if !manifest.exists() {
        return Ok(false);
    }

    let text = fs::read_to_string(&manifest).with_context(|| format!("reading {}", manifest))?;
    let document = text
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parsing {}", manifest))?;
    Ok(document.get("workspace").is_some() || document.get("package").is_some())
}

fn should_skip_dir(name: &str, opts: &DiscoverOptions) -> bool {
    name.starts_with('.')
        || DEFAULT_SKIP_DIRS.contains(&name)
        || opts.extra_skip_dirs.iter().any(|skip| skip == name)
}

fn sort_report(report: &mut DiscoverReport) {
    report.registered.sort();
    report.refreshed.sort();
    report.skipped_empty.sort();
    report.errors.sort_by(|left, right| left.0.cmp(&right.0));
}

pub fn detect_feature_status(workspace_root: &Path) -> BTreeMap<String, FeatureStatus> {
    let mut features = BTreeMap::new();
    features.insert("flake".to_owned(), detect_flake_status(workspace_root));
    features.insert("ci".to_owned(), detect_ci_status(workspace_root));
    features.insert("homebrew".to_owned(), FeatureStatus::Absent);
    features.insert("chocolatey".to_owned(), FeatureStatus::Absent);
    features.insert("scoop".to_owned(), FeatureStatus::Absent);
    features.insert("aur".to_owned(), FeatureStatus::Absent);
    features.insert("copr".to_owned(), FeatureStatus::Absent);
    features.insert("apt".to_owned(), FeatureStatus::Absent);
    features.insert("flatpak".to_owned(), FeatureStatus::Absent);
    features.insert("winget".to_owned(), FeatureStatus::Absent);
    features.insert(
        "changelog".to_owned(),
        detect_file_status(workspace_root, "CHANGELOG.md"),
    );
    features.insert("hooks".to_owned(), detect_hooks_status(workspace_root));

    if let Ok(config) = ProjectConfig::load(workspace_root) {
        if config.homebrew.is_some() && features["homebrew"] == FeatureStatus::Absent {
            features.insert("homebrew".to_owned(), FeatureStatus::Configured);
        }
        if config.chocolatey.is_some() && features["chocolatey"] == FeatureStatus::Absent {
            features.insert("chocolatey".to_owned(), FeatureStatus::Configured);
        }
        if config.scoop.is_some() && features["scoop"] == FeatureStatus::Absent {
            features.insert("scoop".to_owned(), FeatureStatus::Configured);
        }
        if config.aur.is_some() && features["aur"] == FeatureStatus::Absent {
            features.insert("aur".to_owned(), FeatureStatus::Configured);
        }
        if config.copr.is_some() && features["copr"] == FeatureStatus::Absent {
            features.insert("copr".to_owned(), FeatureStatus::Configured);
        }
        if config.apt.is_some() && features["apt"] == FeatureStatus::Absent {
            features.insert("apt".to_owned(), FeatureStatus::Configured);
        }
        if config.flatpak.is_some() && features["flatpak"] == FeatureStatus::Absent {
            features.insert("flatpak".to_owned(), FeatureStatus::Configured);
        }
        if config.winget.is_some() && features["winget"] == FeatureStatus::Absent {
            features.insert("winget".to_owned(), FeatureStatus::Configured);
        }
    }

    features
}

/// Compare the checkout's marked CI workflows with the files Simit would
/// generate today. This deliberately bypasses the per-user project registry.
pub fn audit_ci(workspace_root: &Path) -> Result<CiAudit> {
    let workflows = collect_workflow_files(workspace_root)?
        .into_iter()
        .filter(|file| !is_release_workflow(file))
        .collect::<Vec<_>>();
    let (marked, unmarked): (Vec<_>, Vec<_>) = workflows.into_iter().partition(|file| file.marked);
    let unmarked = unmarked
        .into_iter()
        .filter(|file| !is_supplementary_workflow(file))
        .collect::<Vec<_>>();

    if marked.is_empty() {
        return Ok(CiAudit {
            status: if unmarked.is_empty() {
                FeatureStatus::Absent
            } else {
                FeatureStatus::HandRolled
            },
            platform: None,
            changed_files: Vec::new(),
            missing_files: Vec::new(),
            extra_generated_files: Vec::new(),
        });
    }

    let (provider, platform) = infer_ci_target(&marked)?;
    let expected = infer_expected_ci_files(workspace_root, &marked)?
        .into_iter()
        .map(|file| (file.relative_path, file.content))
        .collect::<BTreeMap<_, _>>();
    let actual = marked
        .iter()
        .map(|file| (file.relative_path.clone(), file.content.as_str()))
        .collect::<BTreeMap<_, _>>();

    let mut changed_files = Vec::new();
    let mut missing_files = Vec::new();
    for (path, content) in &expected {
        match actual.get(path) {
            Some(actual) if *actual == content.as_str() => {}
            Some(_) => changed_files.push(path.clone()),
            None => missing_files.push(path.clone()),
        }
    }
    let mut extra_generated_files = actual
        .keys()
        .filter(|path| !expected.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    changed_files.sort();
    missing_files.sort();
    extra_generated_files.sort();

    let status = if !changed_files.is_empty()
        || !missing_files.is_empty()
        || !extra_generated_files.is_empty()
    {
        FeatureStatus::Drift
    } else if !unmarked.is_empty() {
        FeatureStatus::ManagedExtra
    } else {
        FeatureStatus::Managed
    };

    Ok(CiAudit {
        status,
        platform: Some(match provider {
            CiProvider::Actions => platform.as_str().to_owned(),
            CiProvider::Crow => "crow".to_owned(),
        }),
        changed_files,
        missing_files,
        extra_generated_files,
    })
}

pub fn uses_simit_features(features: &BTreeMap<String, FeatureStatus>) -> bool {
    features.iter().any(|(feature, status)| {
        *status != FeatureStatus::Absent && feature.as_str() != "changelog"
    })
}

pub fn canonical_project_path(path: &Path) -> Result<Utf8PathBuf> {
    canonical_utf8_path(path)
}

fn detect_flake_status(workspace_root: &Path) -> FeatureStatus {
    if resolved_flake_scope_for_status(workspace_root) == FlakeScope::HooksOnly {
        return detect_hooks_only_flake_status(workspace_root);
    }

    let flake_path = workspace_root.join("flake.nix");
    let Ok(content) = fs::read_to_string(&flake_path) else {
        return if flake_path.exists() {
            FeatureStatus::Drift
        } else {
            FeatureStatus::Absent
        };
    };

    if !flake::has_required_wiring(&content) {
        return FeatureStatus::Absent;
    }

    let Ok(languages) = project::detect_languages(workspace_root) else {
        return FeatureStatus::Managed;
    };
    let treefmt = fs::read_to_string(workspace_root.join("nix/treefmt.nix")).ok();
    let pre_commit = fs::read_to_string(workspace_root.join("nix/pre-commit.nix")).ok();
    if treefmt
        .as_deref()
        .is_some_and(|content| flake::has_required_treefmt(content, &languages, "2024"))
        && pre_commit.as_deref().is_some_and(|content| {
            flake::has_required_pre_commit(
                content,
                &languages,
                None,
                flake::AuditTools {
                    audit: languages.rust,
                    deny: false,
                    pyo3: false,
                },
            )
        })
    {
        FeatureStatus::Managed
    } else {
        FeatureStatus::Drift
    }
}

fn detect_hooks_only_flake_status(workspace_root: &Path) -> FeatureStatus {
    let path = workspace_root.join("nix/pre-commit.nix");
    let Ok(content) = fs::read_to_string(&path) else {
        return if path.exists() {
            FeatureStatus::Drift
        } else {
            FeatureStatus::Absent
        };
    };

    let Ok(mut languages) = project::detect_languages(workspace_root) else {
        return FeatureStatus::Managed;
    };
    languages.nix = true;

    if flake::has_required_pre_commit(
        &content,
        &languages,
        None,
        flake::AuditTools {
            audit: languages.rust,
            deny: false,
            pyo3: false,
        },
    ) {
        FeatureStatus::Managed
    } else {
        FeatureStatus::Drift
    }
}

fn resolved_flake_scope_for_status(workspace_root: &Path) -> FlakeScope {
    explicit_flake_scope(workspace_root).unwrap_or_else(|| {
        if has_existing_full_scope_adoption(workspace_root) {
            FlakeScope::Full
        } else {
            FlakeScope::HooksOnly
        }
    })
}

fn explicit_flake_scope(workspace_root: &Path) -> Option<FlakeScope> {
    explicit_flake_scope_from_simit_toml(&workspace_root.join("simit.toml"))
        .or_else(|| explicit_flake_scope_from_cargo_toml(&workspace_root.join("Cargo.toml")))
}

fn explicit_flake_scope_from_simit_toml(path: &Path) -> Option<FlakeScope> {
    let text = fs::read_to_string(path).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    parse_scope(doc.get("flake")?.get("scope")?.as_str()?)
}

fn explicit_flake_scope_from_cargo_toml(path: &Path) -> Option<FlakeScope> {
    let text = fs::read_to_string(path).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let workspace_scope = doc
        .get("workspace")
        .and_then(|item| item.get("metadata"))
        .and_then(|item| item.get("simit"))
        .and_then(|item| item.get("flake"))
        .and_then(|item| item.get("scope"))
        .and_then(|item| item.as_str())
        .and_then(parse_scope);
    if workspace_scope.is_some() {
        return workspace_scope;
    }

    doc.get("package")
        .and_then(|item| item.get("metadata"))
        .and_then(|item| item.get("simit"))
        .and_then(|item| item.get("flake"))
        .and_then(|item| item.get("scope"))
        .and_then(|item| item.as_str())
        .and_then(parse_scope)
}

fn parse_scope(value: &str) -> Option<FlakeScope> {
    match value {
        "hooks-only" => Some(FlakeScope::HooksOnly),
        "full" => Some(FlakeScope::Full),
        _ => None,
    }
}

fn has_existing_full_scope_adoption(workspace_root: &Path) -> bool {
    let Ok(content) = fs::read_to_string(workspace_root.join("flake.nix")) else {
        return false;
    };

    workspace_root.join("nix/treefmt.nix").exists()
        && content.contains("crane.url = \"github:ipetkov/crane\"")
        && content.contains("rustToolchain = pkgs.rust-bin.stable.latest.default.override")
        && content.contains("package = craneLib.buildPackage")
}

fn detect_ci_status(workspace_root: &Path) -> FeatureStatus {
    let workflows = match collect_workflow_files(workspace_root) {
        Ok(workflows) => workflows,
        Err(_) => return FeatureStatus::Drift,
    };
    // The release workflow is generated by `init release`, but is not part of
    // the CI renderer's expected file set.  Treating its simit marker as a CI
    // workflow incorrectly reports CI drift whenever release automation is
    // enabled.
    let workflows = workflows
        .into_iter()
        .filter(|file| !is_release_workflow(file))
        .collect::<Vec<_>>();
    let (marked, unmarked): (Vec<_>, Vec<_>) = workflows.into_iter().partition(|file| file.marked);
    let unmarked = unmarked
        .into_iter()
        .filter(|file| !is_supplementary_workflow(file))
        .collect::<Vec<_>>();

    if marked.is_empty() && unmarked.is_empty() {
        return FeatureStatus::Absent;
    }
    if marked.is_empty() {
        return FeatureStatus::HandRolled;
    }
    if marked_workflows_drift(workspace_root, &marked) {
        return FeatureStatus::Drift;
    }
    if unmarked.is_empty() {
        FeatureStatus::Managed
    } else {
        FeatureStatus::ManagedExtra
    }
}

fn is_release_workflow(file: &WorkflowFile) -> bool {
    matches!(
        file.relative_path
            .file_name()
            .and_then(|name| name.to_str()),
        Some(
            "release.yml"
                | "release.yaml"
                | "release.jsonnet"
                | "publish-vscode-extension.yaml"
                | "publish-jetbrains-plugin.yaml",
        )
    )
}

fn is_supplementary_workflow(file: &WorkflowFile) -> bool {
    if file
        .relative_path
        .file_name()
        .and_then(|name| name.to_str())
        == Some("credential-visibility.yml")
    {
        return true;
    }
    file.content.contains("Native GitHub ")
        && file.content.contains("equivalent of the generated Forgejo")
}

#[derive(Debug, Clone)]
struct WorkflowFile {
    relative_path: PathBuf,
    content: String,
    marked: bool,
}

fn collect_workflow_files(workspace_root: &Path) -> Result<Vec<WorkflowFile>> {
    let mut workflows = Vec::new();

    for relative_dir in [".forgejo/workflows", ".github/workflows", ".crow"] {
        let dir = workspace_root.join(relative_dir);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
        };

        for entry in entries {
            let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
            if !entry
                .file_type()
                .with_context(|| format!("reading file type for {}", entry.path().display()))?
                .is_file()
            {
                continue;
            }
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !matches!(extension, "yaml" | "yml" | "jsonnet") {
                continue;
            }
            let content =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            workflows.push(WorkflowFile {
                relative_path: PathBuf::from(relative_dir).join(entry.file_name()),
                marked: generated_workflow_marker_present(&content),
                content,
            });
        }
    }

    let gitlab = workspace_root.join(".gitlab-ci.yml");
    if let Ok(content) = fs::read_to_string(&gitlab) {
        workflows.push(WorkflowFile {
            relative_path: PathBuf::from(".gitlab-ci.yml"),
            marked: generated_workflow_marker_present(&content),
            content,
        });
    }

    workflows.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(workflows)
}

fn marked_workflows_drift(workspace_root: &Path, marked: &[WorkflowFile]) -> bool {
    let expected = match infer_expected_ci_files(workspace_root, marked) {
        Ok(expected) => expected,
        Err(_) => return true,
    };
    let expected = expected
        .into_iter()
        .map(|file| (file.relative_path, file.content))
        .collect::<BTreeMap<_, _>>();

    expected.len() != marked.len()
        || marked.iter().any(|workflow| {
            expected
                .get(&workflow.relative_path)
                .is_none_or(|content| content != &workflow.content)
        })
}

fn infer_expected_ci_files(
    workspace_root: &Path,
    marked: &[WorkflowFile],
) -> Result<Vec<project::GeneratedFile>> {
    let config = ProjectConfig::load(workspace_root).unwrap_or_default();
    let (provider, platform) = infer_ci_target(marked)?;
    let snapshots = workflow_snapshots(marked);
    if provider == CiProvider::Crow {
        return infer_expected_crow_files(workspace_root, marked, &snapshots);
    }
    if platform == Platform::Gitlab && workspace_root.join("flake.nix").is_file() {
        return Ok(vec![project::GeneratedFile {
            relative_path: PathBuf::from(".gitlab-ci.yml"),
            content: crate::render::ci::gitlab_nix_flake_workflow(),
        }]);
    }
    if !workspace_root.join("Cargo.toml").is_file() && python::is_python_uv_project(workspace_root)
    {
        if platform == Platform::Gitlab {
            return Ok(vec![project::GeneratedFile {
                relative_path: PathBuf::from(".gitlab-ci.yml"),
                content: crate::render::ci::gitlab_nix_flake_workflow(),
            }]);
        }
        let inference = CiInference::from_workflows(&snapshots)?;
        let resolved = crate::ci_resolution::ResolvedCiInputs::resolve(
            workspace_root,
            &config,
            &CiCliOverrides::default(),
            Some(&inference),
        )?;
        let runner = infer_primary_runner(marked, "ci")?;
        let options = resolved.ci_options(&config, false, resolved.omnix_ref.clone());
        let mut files = vec![crate::render::ci::python_ci_file(
            platform,
            resolved.runtime,
            &runner,
            &options,
            &config.flake.expected_outputs.checks,
            &config.ci.components,
        )?];
        if provider == CiProvider::Actions
            && config.prebuild.is_none()
            && !options.nix_builds.is_empty()
        {
            files.push(crate::render::ci::nix_build_matrix_file(
                platform,
                &runner,
                &options.nix_builds,
                &options.extra_setup,
            )?);
        }
        if resolved.with_pypi_publish {
            files.push(crate::render::ci::python_publish_file(
                platform,
                resolved.runtime,
                &runner,
                &options,
            )?);
        }
        if let Some(pages) = config_pages_or_inferred(&config, marked)? {
            files.push(crate::render::ci::codeberg_pages_file(
                platform, &runner, &pages,
            )?);
        }
        push_prebuild_file(&mut files, &config, platform, provider)?;
        return Ok(files);
    }
    if !workspace_root.join("Cargo.toml").is_file() && workspace_root.join("flake.nix").is_file() {
        if platform == Platform::Gitlab {
            return Ok(vec![project::GeneratedFile {
                relative_path: PathBuf::from(".gitlab-ci.yml"),
                content: crate::render::ci::gitlab_nix_flake_workflow(),
            }]);
        }
        let runner = config
            .ci
            .runner
            .as_deref()
            .or_else(|| {
                config
                    .ci
                    .nix_system_runners
                    .values()
                    .next()
                    .map(String::as_str)
            })
            .or_else(|| {
                marked.iter().find_map(|workflow| {
                    workflow.content.lines().find_map(|line| {
                        let runner = line.trim().strip_prefix("runs-on: ")?;
                        (!runner.contains("${{")).then_some(runner)
                    })
                })
            })
            .unwrap_or("ubuntu-latest");
        let runner = ResolvedRunner::literal(runner)?;
        let mut files = vec![crate::render::ci::nix_flake_ci_file_with_system_runners(
            platform,
            &runner,
            &config.ci.nix_system_runners,
            &config.release.artifacts,
        )?];
        if provider == CiProvider::Actions
            && config.prebuild.is_none()
            && !config.ci.nix_builds.is_empty()
        {
            files.push(crate::render::ci::nix_build_matrix_file(
                platform,
                &runner,
                &config.ci.nix_builds,
                &config.ci.extra_setup,
            )?);
        }
        if let Some(pages) = config_pages_or_inferred(&config, marked)? {
            files.push(crate::render::ci::codeberg_pages_file(
                platform, &runner, &pages,
            )?);
        }
        push_prebuild_file(&mut files, &config, platform, provider)?;
        return Ok(files);
    }
    let metadata = cargo::cargo_metadata(&cargo::find_manifest(workspace_root)?)?;
    let inference = CiInference::from_workflows(&snapshots)?;
    if !workspace_root.join("simit.toml").exists() {
        eprintln!(
            "note: inferring CI options for {} from generated workflows because simit.toml [ci] is absent",
            workspace_root.display()
        );
    }
    let has_granular_jobs = marked
        .iter()
        .any(|wf| wf.content.contains("  codeberg-") || wf.content.contains("  codeberg_"));
    let mut cli = CiCliOverrides::default();
    if has_granular_jobs {
        cli.granular = true;
    }
    let mut resolved = crate::ci_resolution::ResolvedCiInputs::resolve(
        workspace_root,
        &config,
        &cli,
        Some(&inference),
    )?;
    if metadata.workspace_members.len() > 1 && resolved.packages.is_empty() {
        resolved.workspace = true;
    }
    if has_granular_jobs {
        crate::commands::init_ci::apply_granular_step_runners(
            &mut resolved.step_runners,
            resolved.runtime,
        );
    }
    let step_runners: BTreeMap<String, ResolvedRunner> = if platform == Platform::Github {
        BTreeMap::new()
    } else {
        resolved
            .step_runners
            .iter()
            .map(|(step, label)| {
                Ok((
                    step.clone(),
                    ResolvedRunner::literal(label).map_err(|e| {
                        anyhow::anyhow!("invalid step runner label for '{step}': {e}")
                    })?,
                ))
            })
            .collect::<Result<_>>()?
    };
    let packages = cargo::select_packages(&metadata, &resolved.packages, resolved.workspace)?;
    let package_scoped = metadata.workspace_members.len() > 1
        && resolved.workspace_strategy == crate::cli::WorkspaceStrategy::Members;
    let windows_runner = infer_windows_runner(marked);
    let inferred_ci_runner = infer_primary_runner(marked, "ci")?;
    let inferred_release_runner = infer_primary_runner(marked, "publish-crate")
        .unwrap_or_else(|_| inferred_ci_runner.clone());
    let runners = ResolvedCiRunners {
        ci: resolved_runner_override(resolved.runner.as_deref()).unwrap_or(inferred_ci_runner),
        release: resolved_runner_override(resolved.runner.as_deref())
            .unwrap_or(inferred_release_runner),
        windows: resolved_runner_override(resolved.windows_runner.as_deref()).or(windows_runner),
    };
    let options = resolved.ci_options(&config, resolved.with_artifacts, resolved.omnix_ref.clone());
    let self_check_runner_override = single_runner_label(&runners.ci);
    let self_check_windows_runner_override = runners.windows.as_ref().and_then(single_runner_label);
    let self_check = ci::SelfCheckOptions {
        enabled: marked
            .iter()
            .any(|workflow| workflow.content.contains("Check generated CI")),
        runner_override: self_check_runner_override,
        windows_runner_override: self_check_windows_runner_override,
        packages: &resolved.packages,
        workspace: resolved.workspace,
    };

    let generation_packages =
        if resolved.workspace_strategy == crate::cli::WorkspaceStrategy::Aggregate {
            packages.first().into_iter().collect::<Vec<_>>()
        } else {
            packages.iter().collect::<Vec<_>>()
        };
    let mut files = Vec::new();
    for package in generation_packages {
        let package_options = ci::CiOptions {
            package_scoped,
            publish_crates: resolved.workspace_strategy != crate::cli::WorkspaceStrategy::Aggregate
                && options.publish_crates,
            homebrew: infer_homebrew_options(&config, package, marked)?,
            chocolatey: infer_chocolatey_options(&config, package, marked)?,
            scoop: infer_scoop_options(&config, package, marked)?,
            ..options.clone()
        };
        files.extend(ci::files(ci::FilesRequest {
            provider,
            platform,
            crow: &config.ci.crow,
            runtime: resolved.runtime,
            package,
            file_suffix: package_scoped.then_some(package.name.as_str()),
            self_check,
            runners: &runners,
            options: package_options,
            step_runners: &step_runners,
        })?);
    }
    if resolved.workspace_strategy == crate::cli::WorkspaceStrategy::Aggregate
        && options.publish_crates
    {
        for package in &packages {
            if package.is_publishable() {
                files.push(ci::publish_file(
                    platform,
                    resolved.runtime,
                    package,
                    &runners.release,
                    options.clone(),
                ));
            }
        }
    }
    if provider == CiProvider::Actions
        && config.prebuild.is_none()
        && !options.nix_builds.is_empty()
    {
        files.push(crate::render::ci::nix_build_matrix_file(
            platform,
            &runners.ci,
            &options.nix_builds,
            &options.extra_setup,
        )?);
    }
    if resolved.with_pypi_publish && cargo::has_pyo3_dep(&metadata.packages) {
        files.push(ci::maturin_publish_file(
            platform,
            resolved.runtime,
            &runners.ci,
            &options,
        )?);
    }
    if let Some(pages) = config_pages_or_inferred(&config, marked)? {
        let pages_runner = config
            .ci
            .runner
            .as_deref()
            .map(ResolvedRunner::literal)
            .transpose()?
            .or_else(|| infer_primary_runner(marked, "pages").ok())
            .unwrap_or_else(|| ResolvedRunner::literal("ubuntu-latest").expect("literal runner"));
        files.push(ci::codeberg_pages_file(platform, &pages_runner, &pages)?);
    }
    push_prebuild_file(&mut files, &config, platform, provider)?;

    Ok(files
        .into_iter()
        .filter(|file| is_workflow_path(&file.relative_path))
        .collect())
}

fn push_prebuild_file(
    files: &mut Vec<project::GeneratedFile>,
    config: &ProjectConfig,
    platform: Platform,
    provider: CiProvider,
) -> Result<()> {
    let Some(prebuild) = &config.prebuild else {
        return Ok(());
    };
    if platform != Platform::Github || provider != CiProvider::Actions {
        bail!("[prebuild] requires GitHub Actions");
    }
    files.push(crate::render::ci::github_prebuild_file(
        &config.ci.nix_system_runners,
        &config.ci.nix_builds,
        prebuild,
        &config.release.artifacts,
        config.release.attic.as_ref(),
    )?);
    Ok(())
}

fn workflow_snapshots(marked: &[WorkflowFile]) -> Vec<WorkflowSnapshot> {
    marked
        .iter()
        .map(|workflow| WorkflowSnapshot {
            relative_path: workflow.relative_path.clone(),
            content: workflow.content.clone(),
        })
        .collect()
}

fn resolved_runner_override(label: Option<&str>) -> Option<ResolvedRunner> {
    label.map(|label| ResolvedRunner {
        name: None,
        labels: vec![label.to_owned()],
    })
}

fn single_runner_label(runner: &ResolvedRunner) -> Option<&str> {
    if runner.labels.len() == 1 {
        Some(runner.labels[0].as_str())
    } else {
        None
    }
}

fn infer_ci_target(marked: &[WorkflowFile]) -> Result<(CiProvider, Platform)> {
    let has_crow = marked
        .iter()
        .any(|workflow| workflow.relative_path.starts_with(".crow"));
    if has_crow {
        if marked
            .iter()
            .any(|workflow| !workflow.relative_path.starts_with(".crow"))
        {
            bail!("mixed CI providers in workflow tree");
        }
        return Ok((CiProvider::Crow, Platform::Forgejo));
    }

    let mut platform = None;
    for workflow in marked {
        let current = if workflow.relative_path.starts_with(".forgejo/workflows") {
            Platform::Forgejo
        } else if workflow.relative_path.starts_with(".github/workflows") {
            Platform::Github
        } else if workflow.relative_path == Path::new(".gitlab-ci.yml") {
            Platform::Gitlab
        } else {
            bail!("unknown workflow root {}", workflow.relative_path.display());
        };
        match platform {
            Some(existing) if existing != current => {
                bail!("mixed CI platforms in workflow tree")
            }
            Some(_) => {}
            None => platform = Some(current),
        }
    }
    Ok((
        CiProvider::Actions,
        platform.context("no marked CI workflows found")?,
    ))
}

fn infer_expected_crow_files(
    workspace_root: &Path,
    marked: &[WorkflowFile],
    snapshots: &[WorkflowSnapshot],
) -> Result<Vec<project::GeneratedFile>> {
    let metadata = cargo::cargo_metadata(&cargo::find_manifest(workspace_root)?)?;
    let config = ProjectConfig::load(workspace_root).unwrap_or_default();
    let inference = CiInference::from_workflows(snapshots)?;
    let resolved = crate::ci_resolution::ResolvedCiInputs::resolve(
        workspace_root,
        &config,
        &CiCliOverrides::default(),
        Some(&inference),
    )?;
    let packages = cargo::select_packages(&metadata, &resolved.packages, resolved.workspace)?;
    let format = if marked.iter().any(|workflow| {
        workflow
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "jsonnet")
    }) {
        CrowWorkflowFormat::Jsonnet
    } else {
        CrowWorkflowFormat::Yaml
    };
    let crow = config.ci.crow.clone();
    let runner = resolved_runner_override(resolved.runner.as_deref()).unwrap_or(ResolvedRunner {
        name: None,
        labels: vec!["crow-default".to_owned()],
    });
    let runners = ResolvedCiRunners {
        ci: runner.clone(),
        release: runner.clone(),
        windows: None,
    };
    let mut options =
        resolved.ci_options(&config, resolved.with_artifacts, resolved.omnix_ref.clone());
    options.publish_crates = resolved.publish_crates || resolved.with_artifacts;
    let step_runners = resolved
        .step_runners
        .iter()
        .map(|(step, label)| {
            (
                step.clone(),
                ResolvedRunner {
                    name: None,
                    labels: vec![label.clone()],
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let package_scoped = metadata.workspace_members.len() > 1;
    let self_check = metadata
        .packages
        .iter()
        .any(|package| package.name == "simit");
    let mut files = Vec::new();
    for package in &packages {
        files.extend(crate::render::crow::files(
            crate::render::crow::FilesRequest {
                format,
                crow: &crow,
                runtime: resolved.runtime,
                package,
                file_suffix: package_scoped.then_some(package.name.as_str()),
                self_check: ci::SelfCheckOptions {
                    enabled: self_check,
                    runner_override: None,
                    windows_runner_override: None,
                    packages: &resolved.packages,
                    workspace: resolved.workspace,
                },
                runners: &runners,
                options: ci::CiOptions {
                    package_scoped,
                    ..options.clone()
                },
                step_runners: &step_runners,
            },
        )?);
    }
    if marked
        .iter()
        .any(|workflow| workflow_name(&workflow.relative_path) == Some("pages"))
    {
        if let Some(pages) = config.resolve_codeberg_pages()? {
            files.push(crate::render::crow::codeberg_pages_file(
                format,
                &crow,
                &runners.ci,
                &ci::CodebergPagesOptions {
                    repo: pages.repo,
                    owner: pages.owner,
                    canonical_domain: pages.canonical_domain,
                    site_output: pages.site_output,
                    token_secret: pages.token_secret,
                    source_branch: pages.source_branch,
                    deploy_app: pages.deploy_app,
                },
            )?);
        }
    }
    if marked
        .iter()
        .any(|workflow| workflow_name(&workflow.relative_path) == Some("publish-vscode-extension"))
    {
        if let Some(vscode) = config.resolve_vscode()? {
            files.push(crate::render::crow::vscode_extension_file(
                format,
                &crow,
                &runners.release,
                &vscode,
            )?);
        }
    }
    if marked
        .iter()
        .any(|workflow| workflow_name(&workflow.relative_path) == Some("publish-jetbrains-plugin"))
    {
        if let Some(jetbrains) = config.resolve_jetbrains()? {
            files.push(crate::render::crow::jetbrains_plugin_file(
                format,
                &crow,
                &runners.release,
                &jetbrains,
            )?);
        }
    }
    if marked
        .iter()
        .any(|workflow| workflow_name(&workflow.relative_path) == Some("publish-pypi"))
        && cargo::has_pyo3_dep(&metadata.packages)
    {
        files.push(crate::render::crow::maturin_publish_file(
            format,
            &crow,
            &runners.release,
            &options,
        )?);
    }
    Ok(files
        .into_iter()
        .filter(|file| is_workflow_path(&file.relative_path))
        .collect())
}

fn generated_workflow_marker_present(content: &str) -> bool {
    content.contains(ci::GENERATED_WORKFLOW_MARKER)
        || content.contains("Generated by simit. Manual edits will be reported as ci=drift.")
}

fn infer_homebrew_options(
    config: &ProjectConfig,
    package: &Package,
    marked: &[WorkflowFile],
) -> Result<Option<ci::HomebrewOptions>> {
    if !marked
        .iter()
        .any(|workflow| workflow.content.contains("name: Publish Homebrew tap"))
    {
        return Ok(None);
    }

    let workflow = marked
        .iter()
        .find(|workflow| {
            workflow_name(&workflow.relative_path) == Some("release-artifacts")
                && workflow.content.contains("name: Publish Homebrew tap")
                && workflow_suffix(&workflow.relative_path)
                    .as_deref()
                    .is_none_or(|suffix| suffix == package.name)
        })
        .or_else(|| {
            marked
                .iter()
                .find(|workflow| workflow.content.contains("name: Publish Homebrew tap"))
        });

    let inferred_name =
        workflow.and_then(|workflow| infer_homebrew_flag(&workflow.content, "--name"));
    let inferred_binaries = workflow.map_or_else(Vec::new, |workflow| {
        infer_homebrew_flags(&workflow.content, "--binary")
    });
    let inferred_tap_url =
        workflow.and_then(|workflow| infer_ci_env_value(&workflow.content, "HOMEBREW_TAP_URL"));
    let inferred_description =
        workflow.and_then(|workflow| infer_homebrew_flag(&workflow.content, "--description"));
    let inferred_homepage =
        workflow.and_then(|workflow| infer_homebrew_flag(&workflow.content, "--homepage"));
    let inferred_license =
        workflow.and_then(|workflow| infer_homebrew_flag(&workflow.content, "--license"));
    let inferred_download_repo =
        workflow.and_then(|workflow| infer_homebrew_download_repo(&workflow.content));

    let overrides = HomebrewOverrides {
        name: inferred_name.as_deref(),
        binaries: (!inferred_binaries.is_empty()).then_some(inferred_binaries.as_slice()),
        tap_url: inferred_tap_url.as_deref(),
        description: inferred_description.as_deref(),
        homepage: inferred_homepage.as_deref(),
        license: inferred_license.as_deref(),
        download_repo: inferred_download_repo.as_deref(),
        ..Default::default()
    };

    let resolved = config.resolve_homebrew(overrides, package)?;
    Ok(Some(ci::HomebrewOptions {
        name: resolved.name,
        binaries: resolved.binaries,
        tap_url: resolved.tap_url,
        tap_token_secret: resolved.tap_token_secret,
        description: resolved.description,
        homepage: resolved.homepage,
        license: resolved.license,
        archive_pattern: resolved.archive_pattern,
        download_repo: resolved.download_repo,
        platforms: ci::HomebrewPlatformSet {
            darwin_arm: resolved.platforms.darwin_arm,
            darwin_intel: resolved.platforms.darwin_intel,
            linux_arm: resolved.platforms.linux_arm,
            linux_intel: resolved.platforms.linux_intel,
        },
    }))
}

fn infer_ci_env_value(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    content
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn infer_homebrew_flag(content: &str, flag: &str) -> Option<String> {
    infer_homebrew_flags(content, flag).into_iter().next()
}

fn infer_homebrew_flags(content: &str, flag: &str) -> Vec<String> {
    let prefix = format!("{flag} ");
    content
        .lines()
        .filter_map(|line| {
            let value = line.trim_start().strip_prefix(&prefix)?;
            let value = value.trim_end_matches('\\').trim();
            (!value.is_empty()).then(|| shell_unquote(value))
        })
        .collect()
}

fn infer_homebrew_download_repo(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (_, rest) = line.split_once("https://codeberg.org/")?;
        let (repo, _) = rest.split_once("/releases/download/")?;
        (!repo.is_empty()).then(|| repo.to_owned())
    })
}

fn infer_chocolatey_options(
    config: &ProjectConfig,
    package: &Package,
    marked: &[WorkflowFile],
) -> Result<Option<ci::ChocolateyOptions>> {
    if !marked.iter().any(|workflow| {
        workflow
            .content
            .contains("name: Publish Chocolatey package")
    }) {
        return Ok(None);
    }

    let resolved = config.resolve_chocolatey(Default::default(), package)?;
    Ok(Some(ci::ChocolateyOptions {
        name: resolved.name,
        id: resolved.id,
        title: resolved.title,
        authors: resolved.authors,
        description: resolved.description,
        summary: resolved.summary,
        project_url: resolved.project_url,
        license_url: resolved.license_url,
        icon_url: resolved.icon_url,
        package_source_url: resolved.package_source_url,
        docs_url: resolved.docs_url,
        bug_tracker_url: resolved.bug_tracker_url,
        project_source_url: resolved.project_source_url,
        tags: resolved.tags,
        release_notes_url: resolved.release_notes_url,
        download_repo: resolved.download_repo,
        archive_pattern: resolved.archive_pattern,
        push_source: resolved.push.source,
    }))
}

fn infer_scoop_options(
    config: &ProjectConfig,
    package: &Package,
    marked: &[WorkflowFile],
) -> Result<Option<ci::ScoopOptions>> {
    if !marked
        .iter()
        .any(|workflow| workflow.content.contains("name: Publish Scoop bucket"))
    {
        return Ok(None);
    }

    let resolved = config.resolve_scoop(Default::default(), package)?;
    Ok(Some(ci::ScoopOptions {
        name: resolved.name,
        bucket_url: resolved.bucket_url,
        bucket_token_secret: resolved.bucket_token_secret,
        description: resolved.description,
        homepage: resolved.homepage,
        license: resolved.license,
        download_repo: resolved.download_repo,
        archive_pattern: resolved.archive_pattern,
        binaries: resolved.binaries,
        x64: resolved.architectures.x64,
        arm64: resolved.architectures.arm64,
    }))
}

fn config_pages_or_inferred(
    config: &ProjectConfig,
    marked: &[WorkflowFile],
) -> Result<Option<ci::CodebergPagesOptions>> {
    if let Some(pages) = config.resolve_codeberg_pages()? {
        return Ok(Some(ci::CodebergPagesOptions {
            repo: pages.repo,
            owner: pages.owner,
            canonical_domain: pages.canonical_domain,
            site_output: pages.site_output,
            token_secret: pages.token_secret,
            source_branch: pages.source_branch,
            deploy_app: pages.deploy_app,
        }));
    }
    infer_codeberg_pages_options(marked)
}

fn infer_codeberg_pages_options(
    marked: &[WorkflowFile],
) -> Result<Option<ci::CodebergPagesOptions>> {
    let Some(workflow) = marked
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some("pages"))
    else {
        return Ok(None);
    };
    let repo = infer_pages_repo(&workflow.content).with_context(|| {
        format!(
            "inferring Codeberg Pages repo from {}",
            workflow.relative_path.display()
        )
    })?;
    let owner = repo
        .split_once('/')
        .map(|(owner, _)| owner.to_owned())
        .context("inferring Codeberg Pages repo owner")?;
    Ok(Some(ci::CodebergPagesOptions {
        repo,
        owner,
        canonical_domain: infer_pages_canonical_domain(&workflow.content),
        site_output: infer_pages_site_output(&workflow.content)
            .unwrap_or_else(|| ".#site".to_owned()),
        token_secret: infer_pages_token_secret(&workflow.content)
            .unwrap_or_else(|| "codeberg_token".to_owned()),
        source_branch: infer_pages_source_branch(&workflow.content)
            .unwrap_or_else(|| "trunk".to_owned()),
        deploy_app: infer_pages_deploy_app(&workflow.content)
            .unwrap_or_else(|| ".#deploy-pages".to_owned()),
    }))
}

fn infer_pages_repo(content: &str) -> Option<String> {
    let marker = "@codeberg.org/";
    let line = content.lines().find(|line| line.contains(marker))?;
    let repo_start = line.find(marker)? + marker.len();
    let repo_tail = &line[repo_start..];
    let repo_end = repo_tail.find(".git").unwrap_or(repo_tail.len());
    Some(repo_tail[..repo_end].trim_matches('"').to_owned())
}

fn infer_pages_token_secret(content: &str) -> Option<String> {
    let marker = "CODEBERG_TOKEN: ${{ secrets.";
    let line = content.lines().find(|line| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(" }}")?;
    Some(tail[..end].to_owned())
}

fn infer_pages_source_branch(content: &str) -> Option<String> {
    let mut lines = content.lines();
    while !lines.next()?.trim_start().starts_with("branches:") {}
    lines
        .find_map(|line| line.trim_start().strip_prefix("- "))
        .map(str::to_owned)
}

fn infer_pages_deploy_app(content: &str) -> Option<String> {
    let marker = "DEPLOY_REMOTE=pages-origin nix run ";
    let line = content.lines().find(|line| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    Some(line[start..].trim().to_owned())
}

fn infer_pages_canonical_domain(content: &str) -> Option<String> {
    let marker = "grep -qx ";
    let suffix = " result-pages-site/.domains";
    let line = content
        .lines()
        .find(|line| line.contains(marker) && line.contains(suffix))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(suffix)?;
    Some(shell_unquote(tail[..end].trim()))
}

fn infer_pages_site_output(content: &str) -> Option<String> {
    let marker = "nix build ";
    let line = content.lines().find(|line| {
        line.contains(marker)
            && (line.contains(" --out-link result-pages-site")
                || line.contains(" --no-link --out-link result-pages-site"))
    })?;
    let suffix = if line.contains(" --no-link --out-link result-pages-site") {
        " --no-link --out-link result-pages-site"
    } else {
        " --out-link result-pages-site"
    };
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(suffix)?;
    Some(shell_unquote(tail[..end].trim()))
}

fn infer_primary_runner(marked: &[WorkflowFile], workflow_kind: &str) -> Result<ResolvedRunner> {
    let workflow = marked
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some(workflow_kind))
        .or_else(|| {
            (workflow_kind == "publish-crate")
                .then(|| {
                    marked.iter().find(|workflow| {
                        workflow_name(&workflow.relative_path) == Some("release-artifacts")
                    })
                })
                .flatten()
        })
        .or_else(|| {
            marked
                .iter()
                .find(|workflow| workflow_name(&workflow.relative_path) == Some("pages"))
        })
        .context("missing primary workflow for runner inference")?;
    let labels = parse_runs_on_labels(&workflow.content, 0)
        .with_context(|| format!("parsing runs-on from {}", workflow.relative_path.display()))?;

    Ok(ResolvedRunner { name: None, labels })
}

fn infer_windows_runner(marked: &[WorkflowFile]) -> Option<ResolvedRunner> {
    let workflow = marked
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some("release-artifacts"))?;
    let labels = parse_runs_on_labels(&workflow.content, 1).ok()?;
    Some(ResolvedRunner { name: None, labels })
}

fn parse_runs_on_labels(content: &str, occurrence: usize) -> Result<Vec<String>> {
    let line = content
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("runs-on: "))
        .nth(occurrence)
        .context("runs-on not found")?;
    let line = line.trim();
    if let Some(values) = line
        .strip_prefix('[')
        .and_then(|line| line.strip_suffix(']'))
    {
        Ok(values
            .split(',')
            .map(|value| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
            .collect())
    } else {
        Ok(vec![line.trim_matches('"').to_owned()])
    }
}

fn workflow_name(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    if stem == "ci" || stem == "build" || stem.starts_with("ci-") || stem.starts_with("build-") {
        Some("ci")
    } else if stem == "publish-crate" || stem.starts_with("publish-crate-") {
        Some("publish-crate")
    } else if stem == "release-artifacts" || stem.starts_with("release-artifacts-") {
        Some("release-artifacts")
    } else if stem == "pages" {
        Some("pages")
    } else if stem == "publish-vscode-extension" {
        Some("publish-vscode-extension")
    } else if stem == "publish-jetbrains-plugin" {
        Some("publish-jetbrains-plugin")
    } else if stem == "publish-pypi" {
        Some("publish-pypi")
    } else {
        None
    }
}

fn workflow_suffix(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_prefix("ci-")
        .or_else(|| stem.strip_prefix("build-"))
        .or_else(|| stem.strip_prefix("publish-crate-"))
        .or_else(|| stem.strip_prefix("release-artifacts-"))
        .map(str::to_owned)
}

fn is_workflow_path(path: &Path) -> bool {
    path.starts_with(".forgejo/workflows")
        || path.starts_with(".github/workflows")
        || path.starts_with(".crow")
        || path == Path::new(".gitlab-ci.yml")
}

fn shell_unquote(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else {
        value.to_owned()
    }
}

fn detect_hooks_status(workspace_root: &Path) -> FeatureStatus {
    if !has_hooks_config(workspace_root) {
        return FeatureStatus::Absent;
    }

    let git_common_dir = match git_output(
        workspace_root,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        Ok(Some(path)) => resolve_path(workspace_root, &path),
        Ok(None) => return detect_hooks_status_without_git(workspace_root),
        Err(err) => {
            debug_git_failure("resolving git hooks directory", &err);
            return FeatureStatus::Configured;
        }
    };
    classify_hooks_route(workspace_root, &git_common_dir)
}

fn detect_hooks_status_without_git(workspace_root: &Path) -> FeatureStatus {
    if required_hooks_installed(&workspace_root.join(".git/hooks")) {
        FeatureStatus::Installed
    } else {
        FeatureStatus::Configured
    }
}

fn has_hooks_config(workspace_root: &Path) -> bool {
    workspace_root.join("nix/pre-commit.nix").exists()
        || workspace_root.join(".pre-commit-config.yaml").exists()
        || workspace_root.join(".pre-commit-config.yml").exists()
}

fn classify_hooks_route(workspace_root: &Path, git_common_dir: &Path) -> FeatureStatus {
    let hooks_dir = git_common_dir.join("hooks");
    let local_hooks_path = hooks_path_config(workspace_root, "--local");
    let global_hooks_path = hooks_path_config(workspace_root, "--global");
    let global_canix_dispatcher = global_hooks_path
        .as_deref()
        .is_some_and(is_canix_dispatcher);
    let effective_hooks_path =
        match git_output(workspace_root, ["config", "--get", "core.hooksPath"]) {
            Ok(Some(path)) => resolve_path(workspace_root, &expand_home(&path)),
            Ok(None) => hooks_dir.clone(),
            Err(err) => {
                debug_git_failure("resolving core.hooksPath", &err);
                return FeatureStatus::Configured;
            }
        };

    if let Some(local) = local_hooks_path.as_deref() {
        if is_canix_dispatcher(local) {
            return status_for_project_hooks(&hooks_dir);
        }
        if global_canix_dispatcher {
            return FeatureStatus::Conflicted;
        }
        if local != hooks_dir {
            return FeatureStatus::Conflicted;
        }
        return status_for_project_hooks(local);
    }

    if is_canix_dispatcher(&effective_hooks_path) {
        return status_for_project_hooks(&hooks_dir);
    }
    if effective_hooks_path == hooks_dir || effective_hooks_path.starts_with(git_common_dir) {
        return status_for_project_hooks(&effective_hooks_path);
    }
    FeatureStatus::Conflicted
}

fn hooks_path_config(workspace_root: &Path, scope: &str) -> Option<PathBuf> {
    git_output(workspace_root, ["config", scope, "--get", "core.hooksPath"])
        .ok()
        .flatten()
        .map(|path| resolve_path(workspace_root, &expand_home(&path)))
}

fn status_for_project_hooks(hooks_dir: &Path) -> FeatureStatus {
    if required_hooks_installed(hooks_dir) {
        FeatureStatus::Installed
    } else {
        FeatureStatus::Configured
    }
}

fn required_hooks_installed(hooks_dir: &Path) -> bool {
    HOOK_TYPES
        .iter()
        .all(|hook| is_executable_hook(&hooks_dir.join(hook)))
}

#[cfg(unix)]
fn is_executable_hook(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_hook(path: &Path) -> bool {
    path.is_file()
}

fn is_canix_dispatcher(path: &Path) -> bool {
    path.join("dispatched-by-canix").exists()
}

fn git_output<const N: usize>(
    workspace_root: &Path,
    args: [&str; N],
) -> std::io::Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

fn resolve_path(workspace_root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}

fn expand_home(path: &str) -> String {
    if path == "~" {
        return env::var("HOME").unwrap_or_else(|_| path.to_owned());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

fn debug_git_failure(_context: &str, _err: &std::io::Error) {
    #[cfg(debug_assertions)]
    eprintln!("debug: could not {_context}: {_err}");
}

fn detect_file_status(workspace_root: &Path, relative: &str) -> FeatureStatus {
    if workspace_root.join(relative).exists() {
        FeatureStatus::Managed
    } else {
        FeatureStatus::Absent
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::Mutex;

    use tempfile::TempDir;

    use crate::config::{HomebrewConfig, HomebrewPlatformsConfig};

    use super::*;

    static GIT_CONFIG_LOCK: Mutex<()> = Mutex::new(());

    struct GitConfigGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
        _global_config: TempDir,
        old_git_config_global: Option<OsString>,
        old_git_config_nosystem: Option<OsString>,
        old_home: Option<OsString>,
        old_xdg_config_home: Option<OsString>,
    }

    impl GitConfigGuard {
        fn new() -> Self {
            let guard = GIT_CONFIG_LOCK.lock().unwrap();
            let global_config = TempDir::new().unwrap();
            let old_git_config_global = env::var_os("GIT_CONFIG_GLOBAL");
            let old_git_config_nosystem = env::var_os("GIT_CONFIG_NOSYSTEM");
            let old_home = env::var_os("HOME");
            let old_xdg_config_home = env::var_os("XDG_CONFIG_HOME");
            // SAFETY: tests in this module serialize Git configuration
            // environment changes with GIT_CONFIG_LOCK and restore them in Drop.
            unsafe {
                env::set_var(
                    "GIT_CONFIG_GLOBAL",
                    global_config.path().join("global.gitconfig"),
                );
                env::set_var("GIT_CONFIG_NOSYSTEM", "1");
                env::set_var("HOME", global_config.path());
                env::set_var("XDG_CONFIG_HOME", global_config.path().join("xdg"));
            }
            Self {
                _guard: guard,
                _global_config: global_config,
                old_git_config_global,
                old_git_config_nosystem,
                old_home,
                old_xdg_config_home,
            }
        }
    }

    impl Drop for GitConfigGuard {
        fn drop(&mut self) {
            restore_env("GIT_CONFIG_GLOBAL", self.old_git_config_global.as_ref());
            restore_env("GIT_CONFIG_NOSYSTEM", self.old_git_config_nosystem.as_ref());
            restore_env("HOME", self.old_home.as_ref());
            restore_env("XDG_CONFIG_HOME", self.old_xdg_config_home.as_ref());
        }
    }

    fn restore_env(key: &str, value: Option<&OsString>) {
        // SAFETY: callers hold GIT_CONFIG_LOCK through GitConfigGuard while
        // restoring process environment variables for this test module.
        unsafe {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    fn git_repo() -> (GitConfigGuard, TempDir) {
        let guard = GitConfigGuard::new();
        let repo = TempDir::new().unwrap();
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
        (guard, repo)
    }

    fn add_hooks_config(repo: &Path) {
        fs::create_dir_all(repo.join("nix")).unwrap();
        fs::write(repo.join("nix/pre-commit.nix"), "{ }").unwrap();
    }

    fn write_executable(path: &Path) {
        fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn add_default_hooks(repo: &Path) {
        let hooks_dir = repo.join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        for hook in HOOK_TYPES {
            write_executable(&hooks_dir.join(hook));
        }
    }

    fn add_canix_dispatcher(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("dispatched-by-canix"), "").unwrap();
        for hook in HOOK_TYPES {
            write_executable(&path.join(hook));
        }
    }

    #[test]
    fn simit_repo_recognizes_managed_pages_and_supplementary_github_workflows() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workflows = collect_workflow_files(root)
            .unwrap()
            .into_iter()
            .filter(|file| !is_release_workflow(file))
            .collect::<Vec<_>>();
        let (marked, unmarked): (Vec<_>, Vec<_>) =
            workflows.into_iter().partition(|file| file.marked);

        assert!(
            unmarked
                .iter()
                .all(|file| file.relative_path.starts_with(".github/workflows"))
        );
        assert!(
            marked
                .iter()
                .any(|file| file.relative_path == Path::new(".github/workflows/pages.yaml"))
        );
        assert!(unmarked.iter().all(is_supplementary_workflow));
        assert!(!marked.is_empty());
    }

    #[test]
    fn live_ci_audit_reports_absent_without_registry_state() {
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"audit-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let audit = audit_ci(root.path()).unwrap();

        assert_eq!(audit.status, FeatureStatus::Absent);
        assert!(audit.platform.is_none());
        assert!(audit.changed_files.is_empty());
        assert!(audit.missing_files.is_empty());
        assert!(audit.extra_generated_files.is_empty());
    }

    #[test]
    fn ci_detector_excludes_generated_release_workflow() {
        let release = WorkflowFile {
            relative_path: PathBuf::from(".forgejo/workflows/release.yml"),
            content: String::new(),
            marked: true,
        };
        let release_yaml = WorkflowFile {
            relative_path: PathBuf::from(".github/workflows/release.yaml"),
            content: String::new(),
            marked: true,
        };
        let ci = WorkflowFile {
            relative_path: PathBuf::from(".forgejo/workflows/ci.yaml"),
            content: String::new(),
            marked: true,
        };

        assert!(is_release_workflow(&release));
        assert!(is_release_workflow(&release_yaml));
        assert!(!is_release_workflow(&ci));
    }

    #[test]
    fn ci_detector_infers_homebrew_name_override_from_release_workflow() {
        let config = ProjectConfig {
            homebrew: Some(HomebrewConfig {
                name: None,
                binaries: vec!["modde".to_owned(), "modde-ui".to_owned()],
                tap_url: "https://codeberg.org/caniko/homebrew-modde.git".to_owned(),
                tap_token_secret: "homebrew_tap_token".to_owned(),
                description: Some("Cross-platform game mod manager".to_owned()),
                homepage: Some("https://modde.tartanoglu.com".to_owned()),
                license: Some("GPL-3.0-only".to_owned()),
                download_repo: "caniko/rs-modde".to_owned(),
                archive_pattern: "modde-{version}-{arch}-{os}.tar.gz".to_owned(),
                platforms: HomebrewPlatformsConfig::default(),
            }),
            ..Default::default()
        };
        let package = Package {
            id: "path+file:///workspace#modde-cli@0.2.0".to_owned(),
            name: "modde-cli".to_owned(),
            version: "0.2.0".to_owned(),
            edition: Some("2024".to_owned()),
            authors: Vec::new(),
            license: Some("GPL-3.0-only".to_owned()),
            description: Some("CLI interface for modde".to_owned()),
            homepage: Some("https://modde.tartanoglu.com".to_owned()),
            rust_version: Some("1.85".to_owned()),
            publish: None,
            features: BTreeMap::new(),
            dependencies: Vec::new(),
            manifest_path: Utf8PathBuf::from("/workspace/crates/modde-cli/Cargo.toml"),
        };
        let marked = vec![WorkflowFile {
            relative_path: PathBuf::from(".forgejo/workflows/release-artifacts-modde-cli.yaml"),
            marked: true,
            content: r#"
# Generated by simit. Manual edits will be reported as ci=drift.
      - name: Publish Homebrew tap
        env:
          HOMEBREW_TAP_URL: https://codeberg.org/caniko/homebrew-modde.git
        run: |
          nix run '.#rs-harbor' -- brew bump \
            --name modde \
            --version "$VERSION" \
            --description 'Cross-platform game mod manager' \
            --homepage https://modde.tartanoglu.com \
            --license GPL-3.0-only \
            --archive "darwin_arm=https://codeberg.org/caniko/rs-modde/releases/download/${VERSION}/modde-${VERSION}-aarch64-darwin.tar.gz,release/modde-${VERSION}-aarch64-darwin.tar.gz" \
            --binary modde \
            --binary modde-ui \
            --tap "$PWD/tap"
"#
            .to_owned(),
        }];

        let homebrew = infer_homebrew_options(&config, &package, &marked)
            .unwrap()
            .unwrap();

        assert_eq!(homebrew.name, "modde");
        assert_eq!(homebrew.binaries, ["modde", "modde-ui"]);
    }

    fn set_local_hooks_path(repo: &Path, hooks_path: &Path) {
        let status = Command::new("git")
            .args(["config", "--local", "core.hooksPath"])
            .arg(hooks_path)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn feature_status_conflicted_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&FeatureStatus::Conflicted).unwrap(),
            "\"conflicted\""
        );
        assert_eq!(
            serde_json::from_str::<FeatureStatus>("\"conflicted\"").unwrap(),
            FeatureStatus::Conflicted
        );
    }

    #[test]
    fn detects_hooks_absent_without_pre_commit_config() {
        let (_guard, repo) = git_repo();

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Absent);
    }

    #[test]
    fn detects_hooks_configured_without_installed_default_hook() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Configured);
    }

    #[test]
    fn detects_hooks_installed_in_default_git_hooks_dir() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());
        add_default_hooks(repo.path());

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Installed);
    }

    #[test]
    fn detects_hooks_conflicted_in_custom_hooks_path() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());
        let hooks_path = repo.path().join("custom-hooks");
        fs::create_dir_all(&hooks_path).unwrap();
        for hook in HOOK_TYPES {
            write_executable(&hooks_path.join(hook));
        }
        set_local_hooks_path(repo.path(), &hooks_path);

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Conflicted);
    }

    #[test]
    fn detects_hooks_conflicted_when_custom_hooks_path_lacks_pre_commit() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());
        let hooks_path = repo.path().join("custom-hooks");
        fs::create_dir_all(&hooks_path).unwrap();
        set_local_hooks_path(repo.path(), &hooks_path);

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Conflicted);
    }

    #[test]
    fn detects_hooks_installed_through_canix_dispatcher() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());
        add_default_hooks(repo.path());
        let dispatcher = repo.path().join("global-hooks");
        add_canix_dispatcher(&dispatcher);
        let status = Command::new("git")
            .args(["config", "--global", "core.hooksPath"])
            .arg(&dispatcher)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Installed);
    }

    #[test]
    fn detects_hooks_configured_when_dispatcher_chain_lacks_project_hooks() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());
        let dispatcher = repo.path().join("global-hooks");
        add_canix_dispatcher(&dispatcher);
        let status = Command::new("git")
            .args(["config", "--global", "core.hooksPath"])
            .arg(&dispatcher)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Configured);
    }

    #[test]
    fn detects_hooks_conflicted_when_local_path_bypasses_global_dispatcher() {
        let (_guard, repo) = git_repo();
        add_hooks_config(repo.path());
        add_default_hooks(repo.path());
        let dispatcher = repo.path().join("global-hooks");
        add_canix_dispatcher(&dispatcher);
        let status = Command::new("git")
            .args(["config", "--global", "core.hooksPath"])
            .arg(&dispatcher)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
        set_local_hooks_path(repo.path(), &repo.path().join(".git/hooks"));

        assert_eq!(detect_hooks_status(repo.path()), FeatureStatus::Conflicted);
    }
}

fn touch_loaded(
    registry: &mut Registry,
    workspace_root: &Path,
    package_name: &str,
    feature_updates: impl IntoIterator<Item = (&'static str, FeatureStatus)>,
) -> Result<()> {
    validate_schema(registry)?;
    let now = Utc::now();
    let path = canonical_utf8_path(workspace_root)?;
    let detected = detect_feature_status(workspace_root);
    let entry = registry
        .projects
        .entry(path)
        .or_insert_with(|| ProjectEntry {
            name: package_name.to_owned(),
            first_seen: now,
            last_seen: now,
            features: BTreeMap::new(),
        });

    entry.name = package_name.to_owned();
    entry.last_seen = now;
    entry.features.extend(detected);
    entry.features.extend(
        feature_updates
            .into_iter()
            .map(|(feature, status)| (feature.to_owned(), status)),
    );
    Ok(())
}

fn canonical_utf8_path(path: &Path) -> Result<Utf8PathBuf> {
    let canonical =
        fs::canonicalize(path).with_context(|| format!("canonicalizing {}", path.display()))?;
    Utf8PathBuf::from_path_buf(canonical).map_err(|path| {
        anyhow::anyhow!(
            "workspace path is not valid UTF-8 after canonicalization: {}",
            path.display()
        )
    })
}

fn load_from_path(path: &Utf8PathBuf) -> Result<Registry> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Registry::default()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path)),
    };
    let registry: Registry =
        toml_edit::de::from_str(&text).with_context(|| format!("parsing {}", path))?;
    validate_schema(&registry)?;
    Ok(registry)
}

fn save_unlocked(path: &Utf8PathBuf, registry: &Registry) -> Result<()> {
    validate_schema(registry)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("registry path has no parent: {}", path))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent))?;
    let tmp_path = path.with_extension("toml.tmp");
    match fs::remove_file(&tmp_path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("removing stale {}", tmp_path)),
    }

    let text = toml_edit::ser::to_string_pretty(registry).context("serializing registry")?;
    fs::write(&tmp_path, text).with_context(|| format!("writing {}", tmp_path))?;

    #[cfg(windows)]
    {
        // Windows does not reliably replace an existing destination with
        // `rename`, so this is best-effort rather than strictly atomic there.
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {}", path)),
        }
    }

    fs::rename(&tmp_path, path).with_context(|| format!("renaming {} to {}", tmp_path, path))?;
    Ok(())
}

fn validate_schema(registry: &Registry) -> Result<()> {
    if registry.schema_version != SCHEMA_VERSION {
        bail!(
            "registry schema_version {} is unsupported; required version {}",
            registry.schema_version,
            SCHEMA_VERSION
        );
    }
    Ok(())
}

fn lock_for_path(path: &Utf8PathBuf) -> Result<RegistryLock> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("registry path has no parent: {}", path))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent))?;
    let lock_path = parent.join("projects.toml.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path))?;

    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(RegistryLock { file }),
            Err(err) if would_block(&err) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) if would_block(&err) => {
                bail!(
                    "timed out waiting for simit project registry lock at {}",
                    lock_path
                );
            }
            Err(err) => return Err(err).with_context(|| format!("locking {}", lock_path)),
        }
    }
}

fn would_block(err: &std::io::Error) -> bool {
    matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted)
}

struct RegistryLock {
    file: File,
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn disabled() -> bool {
    env::var_os("SIMIT_NO_REGISTRY").is_some_and(|value| value == "1")
}

fn warn_on_error(result: Result<()>) {
    if let Err(err) = result {
        eprintln!("warning: could not update simit project registry: {err:#}");
    }
}

mod project_entries {
    use std::collections::BTreeMap;

    use camino::Utf8PathBuf;
    use chrono::{DateTime, Utc};
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{FeatureStatus, ProjectEntry};

    #[derive(Deserialize, Serialize)]
    struct ProjectRecord {
        path: Utf8PathBuf,
        name: String,
        first_seen: DateTime<Utc>,
        last_seen: DateTime<Utc>,
        #[serde(default)]
        features: BTreeMap<String, FeatureStatus>,
    }

    pub fn serialize<S>(
        projects: &BTreeMap<Utf8PathBuf, ProjectEntry>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let records = projects
            .iter()
            .map(|(path, entry)| ProjectRecord {
                path: path.clone(),
                name: entry.name.clone(),
                first_seen: entry.first_seen,
                last_seen: entry.last_seen,
                features: entry.features.clone(),
            })
            .collect::<Vec<_>>();
        records.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<Utf8PathBuf, ProjectEntry>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let records = Vec::<ProjectRecord>::deserialize(deserializer)?;
        let mut projects = BTreeMap::new();
        for record in records {
            if projects
                .insert(
                    record.path.clone(),
                    ProjectEntry {
                        name: record.name,
                        first_seen: record.first_seen,
                        last_seen: record.last_seen,
                        features: record.features,
                    },
                )
                .is_some()
            {
                return Err(D::Error::custom(format!(
                    "duplicate project path in registry: {}",
                    record.path
                )));
            }
        }
        Ok(projects)
    }
}
