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
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use directories_next::ProjectDirs;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::cargo;
use crate::config::ProjectConfig;
use crate::project;
use crate::render::ci;
use crate::render::flake;

pub const SCHEMA_VERSION: u32 = 1;
pub const KNOWN_FEATURES: &[&str] = &[
    "flake",
    "ci",
    "homebrew",
    "chocolatey",
    "scoop",
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
    Drift,
    #[serde(rename = "hand-rolled")]
    HandRolled,
    Configured,
    Installed,
    Absent,
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
    }

    features
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
        && pre_commit
            .as_deref()
            .is_some_and(|content| flake::has_required_pre_commit(content, &languages, None))
    {
        FeatureStatus::Managed
    } else {
        FeatureStatus::Drift
    }
}

fn detect_ci_status(workspace_root: &Path) -> FeatureStatus {
    let mut workflow_count = 0usize;
    let mut managed_count = 0usize;

    for relative_dir in [".forgejo/workflows", ".github/workflows"] {
        let dir = workspace_root.join(relative_dir);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return FeatureStatus::Drift,
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => return FeatureStatus::Drift,
            };
            if !file_type.is_file() {
                continue;
            }
            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if extension != "yaml" && extension != "yml" {
                continue;
            }

            workflow_count += 1;
            let Ok(content) = fs::read_to_string(&path) else {
                return FeatureStatus::Drift;
            };
            if content.contains(ci::GENERATED_WORKFLOW_MARKER) {
                managed_count += 1;
            }
        }
    }

    match (workflow_count, managed_count) {
        (0, _) => FeatureStatus::Absent,
        (total, managed) if managed == total => FeatureStatus::Managed,
        (_, 0) => FeatureStatus::HandRolled,
        _ => FeatureStatus::Drift,
    }
}

fn detect_hooks_status(workspace_root: &Path) -> FeatureStatus {
    if workspace_root.join("nix/pre-commit.nix").exists() {
        FeatureStatus::Installed
    } else {
        FeatureStatus::Absent
    }
}

fn detect_file_status(workspace_root: &Path, relative: &str) -> FeatureStatus {
    if workspace_root.join(relative).exists() {
        FeatureStatus::Managed
    } else {
        FeatureStatus::Absent
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
