//! One project-owned component graph shared by local planning and CI generation.

mod check;
pub(crate) mod ci;
mod native_ci;
pub(crate) mod native_registry;
mod plan;
pub(crate) mod releases;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component as PathComponent, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::config::{ProjectConfig, RequiredGate};

pub use check::GateExit;
pub use plan::{ChangedPaths, Plan, changed_paths, select};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub components: Vec<Component>,
    #[serde(default = "default_shared_paths")]
    pub shared_paths: Vec<String>,
    /// Root-relative, project-owned treefmt modules composed by init flake.
    #[serde(default)]
    pub formatter_modules: Vec<String>,
    /// Components are Git submodules; generated checkouts fetch them recursively.
    #[serde(default)]
    pub submodules: bool,
    /// Paths whose changes select no component (documentation, migration notes).
    #[serde(default)]
    pub ignore_paths: Vec<String>,
}

fn default_shared_paths() -> Vec<String> {
    [
        "simit.toml",
        "flake.nix",
        "flake.lock",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "deny.toml",
        ".github",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub id: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub cargo_packages: Vec<String>,
    /// Root-relative manifest of this component's own Cargo workspace (or
    /// standalone package); `cargo_packages` then resolve there, not at the root.
    #[serde(default)]
    pub cargo_manifest: Option<String>,
    /// Static Python/npm version owners; registry publication is separate.
    #[serde(default)]
    pub releases: Vec<releases::ManifestRelease>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub systems: Vec<String>,
    #[serde(default)]
    pub checks: Vec<RequiredGate>,
    /// Components naming the same job share one CI job (checkout, Nix store,
    /// provisioning) and run their checks in dependency order; default `id`.
    #[serde(default)]
    pub job: Option<String>,
}

impl Component {
    pub fn job_id(&self) -> &str {
        self.job.as_deref().unwrap_or(&self.id)
    }
}

/// Merged Cargo metadata plus each member's owning manifest (`None` for the root).
type CargoWorkspaces = (crate::cargo::Metadata, BTreeMap<String, Option<String>>);

#[derive(Debug)]
pub struct Graph {
    pub components: BTreeMap<String, Component>,
    pub shared_paths: Vec<String>,
    pub ignore_paths: Vec<String>,
    pub order: Vec<String>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!(
                "unsupported monorepo schema_version {}; expected 1",
                self.schema_version
            );
        }
        if self.components.is_empty() {
            bail!("monorepo requires at least one component");
        }
        let mut ids = BTreeSet::new();
        let mut job_systems: BTreeMap<&str, &Vec<String>> = BTreeMap::new();
        for component in &self.components {
            validate_id(&component.id)?;
            validate_id(component.job_id())?;
            if let Some(systems) = job_systems.insert(component.job_id(), &component.systems)
                && systems != &component.systems
            {
                bail!(
                    "components sharing job {} must declare the same systems",
                    component.job_id()
                );
            }
            if !ids.insert(&component.id) {
                bail!("duplicate monorepo component {}", component.id);
            }
            for path in &component.paths {
                validate_relative_path(path)?;
            }
            if let Some(manifest) = &component.cargo_manifest {
                validate_relative_path(manifest)?;
                ensure!(
                    Path::new(manifest).file_name() == Some("Cargo.toml".as_ref())
                        && component.paths.iter().any(|path| covers(path, manifest)),
                    "component {} cargo_manifest must be a Cargo.toml inside its paths",
                    component.id
                );
            }
            for release in &component.releases {
                release.validate()?;
            }
            let mut gates = BTreeSet::new();
            for gate in &component.checks {
                validate_id(&gate.id)?;
                if !gates.insert(&gate.id) {
                    bail!("duplicate check {} in {}", gate.id, component.id);
                }
                if gate.run.trim().is_empty()
                    || gate.run.contains(['\n', '\r'])
                    || gate.run.contains("${{")
                {
                    bail!(
                        "component {} check {} requires a single-line command",
                        component.id,
                        gate.id
                    );
                }
                if !(1..=360).contains(&gate.timeout_minutes) {
                    bail!(
                        "component {} check timeout must be 1..=360 minutes",
                        component.id
                    );
                }
                for name in gate.env.keys() {
                    if name.is_empty()
                        || !name.bytes().enumerate().all(|(i, c)| {
                            c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                        })
                    {
                        bail!("invalid component check environment name {name:?}");
                    }
                    if matches!(
                        name.as_str(),
                        "UID" | "EUID" | "PPID" | "BASHOPTS" | "SHELLOPTS" | "BASH_VERSINFO"
                    ) {
                        bail!("unsupported read-only Bash environment name {name}");
                    }
                    // Bash's integer, computed, startup/control and internal-array variables are
                    // not scalar data: assignment may evaluate arithmetic, be
                    // ignored, or be replaced by the next command. Reject them
                    // before either native execution or generated CI writes.
                    if matches!(
                        name.as_str(),
                        "RANDOM"
                            | "SRANDOM"
                            | "OPTIND"
                            | "HISTCMD"
                            | "SECONDS"
                            | "LINENO"
                            | "BASHPID"
                            | "EPOCHSECONDS"
                            | "EPOCHREALTIME"
                            | "BASH_MONOSECONDS"
                            | "BASH_SUBSHELL"
                            | "BASH_COMPAT"
                            | "BASH_XTRACEFD"
                            | "BASH_ALIASES"
                            | "BASH_ARGC"
                            | "BASH_ARGV"
                            | "BASH_ARGV0"
                            | "BASH_ENV"
                            | "BASH_CMDS"
                            | "BASH_COMMAND"
                            | "BASH_LINENO"
                            | "BASH_SOURCE"
                            | "DIRSTACK"
                            | "FUNCNAME"
                            | "GROUPS"
                            | "PIPESTATUS"
                            | "_"
                    ) {
                        bail!("unsupported managed Bash environment name {name}");
                    }
                }
            }
        }
        for path in self
            .shared_paths
            .iter()
            .chain(&self.formatter_modules)
            .chain(&self.ignore_paths)
        {
            validate_relative_path(path)?;
        }
        for ignored in &self.ignore_paths {
            for path in self
                .shared_paths
                .iter()
                .chain(self.components.iter().flat_map(|c| &c.paths))
            {
                ensure!(
                    !covers(ignored, path) && !covers(path, ignored),
                    "ignore path {ignored} overlaps owned or shared path {path}"
                );
            }
        }
        self.graph(None)?.validate_ownership()
    }

    /// Cargo metadata supplies path ownership and all local dependency edges.
    pub fn resolve(&self, root: &Path) -> Result<Graph> {
        self.validate()?;
        let metadata = self.cargo_workspaces(root)?;
        let graph = self.graph(metadata.as_ref())?;
        graph.validate_ownership()?;
        releases::validate(
            root,
            &graph,
            metadata.as_ref().map(|(metadata, _)| metadata),
        )?;
        for path in graph
            .components
            .values()
            .flat_map(|c| &c.paths)
            .chain(&self.formatter_modules)
        {
            let mut current = root.to_path_buf();
            for part in Path::new(path).components() {
                current.push(part);
                if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
                    bail!(
                        "component path must not traverse a symlink: {}",
                        current.display()
                    );
                }
            }
        }
        Ok(graph)
    }

    /// Root and component Cargo workspaces merged into one metadata view.
    fn cargo_workspaces(&self, root: &Path) -> Result<Option<CargoWorkspaces>> {
        let mut merged: Option<crate::cargo::Metadata> = None;
        let mut scopes = BTreeMap::new();
        if root.join("Cargo.toml").is_file() {
            let metadata = crate::cargo::cargo_metadata(&root.join("Cargo.toml"))?;
            for member in &metadata.workspace_members {
                scopes.insert(member.clone(), None);
            }
            merged = Some(metadata);
        } else if self
            .components
            .iter()
            .any(|c| c.cargo_manifest.is_none() && !c.cargo_packages.is_empty())
        {
            bail!("monorepo Cargo ownership requires Cargo.toml at the repository root");
        }
        let manifests: BTreeSet<_> = self
            .components
            .iter()
            .filter_map(|c| c.cargo_manifest.as_ref())
            .collect();
        for manifest in manifests {
            let path = root.join(manifest);
            ensure!(
                std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()),
                "component cargo_manifest {manifest} must be an existing regular file"
            );
            let metadata = crate::cargo::cargo_metadata(&path)?;
            ensure!(
                std::fs::canonicalize(metadata.workspace_root.join("Cargo.toml"))?
                    == std::fs::canonicalize(&path)?,
                "component cargo_manifest {manifest} must be a workspace root or standalone package"
            );
            for member in &metadata.workspace_members {
                ensure!(
                    scopes
                        .insert(member.clone(), Some(manifest.clone()))
                        .is_none(),
                    "Cargo package {member} belongs to both the root and component workspace {manifest}; exclude it from the root workspace"
                );
            }
            let merged = merged.get_or_insert_with(|| crate::cargo::Metadata {
                packages: Vec::new(),
                workspace_members: Vec::new(),
                workspace_root: camino::Utf8PathBuf::default(),
            });
            for package in metadata.packages {
                if !merged.packages.iter().any(|known| known.id == package.id) {
                    merged.packages.push(package);
                }
            }
            merged.workspace_members.extend(metadata.workspace_members);
        }
        if let Some(merged) = &mut merged
            && merged.workspace_root.as_str().is_empty()
        {
            merged.workspace_root =
                camino::Utf8PathBuf::from_path_buf(std::fs::canonicalize(root)?)
                    .ok()
                    .context("monorepo root is not UTF-8")?;
        }
        Ok(merged.map(|metadata| (metadata, scopes)))
    }

    fn graph(&self, metadata: Option<&CargoWorkspaces>) -> Result<Graph> {
        let mut components: BTreeMap<_, _> = self
            .components
            .iter()
            .map(|c| (c.id.clone(), c.clone()))
            .collect();
        if let Some((metadata, scopes)) = metadata {
            let mut owners = BTreeMap::new();
            let mut packages = BTreeMap::new();
            for package in &metadata.packages {
                if metadata.workspace_members.contains(&package.id) {
                    ensure!(
                        packages.insert(package.name.as_str(), package).is_none(),
                        "Cargo package name {} is declared by more than one workspace",
                        package.name
                    );
                }
            }
            for component in components.values_mut() {
                for name in &component.cargo_packages {
                    let package = packages
                        .get(name.as_str())
                        .filter(|package| scopes[&package.id] == component.cargo_manifest)
                        .with_context(|| {
                            format!(
                                "component {} owns unknown Cargo package {name}",
                                component.id
                            )
                        })?;
                    if owners.insert(name.clone(), component.id.clone()).is_some() {
                        bail!("duplicate Cargo ownership for {name}");
                    }
                    let directory = package
                        .manifest_path
                        .parent()
                        .context("Cargo manifest has no parent")?
                        .strip_prefix(&metadata.workspace_root)
                        .context("Cargo member is outside the monorepo")?;
                    if directory.as_str().is_empty() {
                        ensure!(
                            !component.paths.is_empty(),
                            "root Cargo package {name} requires explicit component source paths"
                        );
                        component.paths.push("Cargo.toml".to_owned());
                    } else {
                        let directory = directory
                            .components()
                            .map(|part| part.as_str())
                            .collect::<Vec<_>>()
                            .join("/");
                        validate_relative_path(&directory)?;
                        component.paths.push(directory);
                    }
                }
            }
            for name in packages.keys() {
                if !owners.contains_key(*name) {
                    bail!("Cargo package {name} has no monorepo component ownership");
                }
            }
            for component in components.values_mut() {
                let mut dependencies: BTreeSet<_> = component.depends_on.iter().cloned().collect();
                for name in &component.cargo_packages {
                    for dependency in &packages[name.as_str()].dependencies {
                        if let Some(path) = &dependency.path {
                            for (name, package) in &packages {
                                if package.manifest_path.parent() == Some(path.as_path()) {
                                    let owner = &owners[*name];
                                    if owner != &component.id {
                                        dependencies.insert(owner.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                component.depends_on = dependencies.into_iter().collect();
            }
        }
        let mut pending: BTreeMap<_, BTreeSet<_>> = components
            .iter()
            .map(|(id, c)| (id.clone(), c.depends_on.iter().cloned().collect()))
            .collect();
        for (id, dependencies) in &pending {
            for dependency in dependencies {
                if !components.contains_key(dependency) {
                    bail!("component {id} depends on unknown component {dependency}");
                }
            }
        }
        let mut order = Vec::new();
        while !pending.is_empty() {
            let next = pending
                .iter()
                .find(|(_, deps)| deps.is_empty())
                .map(|(id, _)| id.clone())
                .with_context(|| {
                    format!(
                        "monorepo dependency cycle among {}",
                        pending.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                })?;
            pending.remove(&next);
            for deps in pending.values_mut() {
                deps.remove(&next);
            }
            order.push(next);
        }
        Ok(Graph {
            components,
            shared_paths: self.shared_paths.clone(),
            ignore_paths: self.ignore_paths.clone(),
            order,
        })
    }
}

impl Graph {
    fn validate_ownership(&self) -> Result<()> {
        let mut paths: Vec<(&str, &str)> = Vec::new();
        for component in self.components.values() {
            for path in &component.paths {
                for (owner, previous) in &paths {
                    if *owner != component.id && (covers(previous, path) || covers(path, previous))
                    {
                        bail!(
                            "overlapping component ownership: {owner}:{previous} and {}:{path}",
                            component.id
                        );
                    }
                }
                paths.push((&component.id, path));
            }
        }
        Ok(())
    }
}

pub(crate) fn covers(prefix: &str, path: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || !id.as_bytes()[0].is_ascii_lowercase()
    {
        bail!("component/check id must be lowercase kebab-case: {id:?}");
    }
    Ok(())
}

pub(crate) fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains(['\\', '\n', '\r', '\0'])
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, PathComponent::Normal(_)))
        || path.ends_with('/')
        || path.contains("//")
        || path.split('/').any(|part| part == "." || part == "..")
    {
        bail!("component path must be a normalized relative path: {path:?}");
    }
    Ok(())
}

/// Stop at a Git repository boundary so a nested checkout cannot inherit fleet policy.
pub fn find_root(start: &Path) -> Result<Option<PathBuf>> {
    for directory in start.ancestors() {
        let file = directory.join("simit.toml");
        if file.is_file() {
            let text = std::fs::read_to_string(&file)?;
            let document = text.parse::<toml_edit::DocumentMut>()?;
            if document.contains_key("monorepo") {
                return Ok(Some(directory.to_path_buf()));
            }
        }
        let manifest = directory.join("Cargo.toml");
        if manifest.is_file() {
            let text = std::fs::read_to_string(&manifest)?;
            let document = text.parse::<toml_edit::DocumentMut>()?;
            if ["package", "workspace"].iter().any(|kind| {
                document
                    .get(kind)
                    .and_then(|item| item.get("metadata"))
                    .and_then(|item| item.get("simit"))
                    .and_then(|item| item.get("monorepo"))
                    .is_some()
            }) {
                return Ok(Some(directory.to_path_buf()));
            }
        }
        let flake = directory.join("flake.nix");
        if flake.is_file()
            && crate::config::flake_declares_simit_config(&flake)?
            && ProjectConfig::load(directory)?.monorepo.is_some()
        {
            return Ok(Some(directory.to_path_buf()));
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    Ok(None)
}

pub fn load(start: &Path) -> Result<(PathBuf, ProjectConfig)> {
    let root = find_root(start)?.context("no [monorepo] configuration found in this repository")?;
    let config = ProjectConfig::load(&root)?;
    Ok((root, config))
}

/// Root-relative manifest of the Cargo workspace that owns a component's packages.
pub(crate) fn component_cargo_manifest(project: &ProjectConfig, id: &str) -> Result<String> {
    let component = project
        .monorepo
        .as_ref()
        .context("missing monorepo config")?
        .components
        .iter()
        .find(|component| component.id == id)
        .with_context(|| format!("unknown monorepo component {id}"))?;
    Ok(component
        .cargo_manifest
        .clone()
        .unwrap_or_else(|| "Cargo.toml".to_owned()))
}

pub(crate) fn component_metadata(
    root: &Path,
    project: &ProjectConfig,
    id: &str,
) -> Result<crate::cargo::Metadata> {
    crate::cargo::cargo_metadata(&root.join(component_cargo_manifest(project, id)?))
}

pub(crate) fn release_package(
    root: &Path,
    project: &ProjectConfig,
    metadata: &crate::cargo::Metadata,
    id: &str,
    requested: &[String],
) -> Result<crate::cargo::Package> {
    let graph = project
        .monorepo
        .as_ref()
        .context("missing monorepo config")?
        .resolve(root)?;
    let component = graph
        .components
        .get(id)
        .with_context(|| format!("unknown monorepo component {id}"))?;
    let requested = if requested.is_empty() {
        &component.cargo_packages
    } else {
        requested
    };
    if requested.len() != 1
        || requested
            .iter()
            .any(|name| !component.cargo_packages.contains(name))
    {
        bail!(
            "independent component releases require exactly one owned Cargo package; pass --package <name>"
        );
    }
    let mut packages = crate::cargo::select_packages(metadata, requested, false)?;
    let package = packages.remove(0);
    let manifest = std::fs::read_to_string(package.manifest_path.as_std_path())?
        .parse::<toml_edit::DocumentMut>()?;
    if manifest["package"]["version"].as_str().is_none() {
        bail!(
            "independent release package.version must be literal; shared workspace versions would change other components"
        );
    }
    Ok(package)
}

pub(crate) fn compose_formatter_modules(
    project: &ProjectConfig,
    root: &Path,
    files: &mut [crate::project::GeneratedFile],
) -> Result<()> {
    let Some(config) = &project.monorepo else {
        return Ok(());
    };
    config.resolve(root)?;
    if config.formatter_modules.is_empty() {
        return Ok(());
    }
    let imports = config
        .formatter_modules
        .iter()
        .map(|path| {
            format!(
                "    (../. + {})\n",
                serde_json::to_string(&format!("/{path}"))
                    .expect("path string serialization")
                    .replace("${", "\\${")
            )
        })
        .collect::<String>();
    if let Some(file) = files
        .iter_mut()
        .find(|file| file.relative_path == Path::new("nix/treefmt.nix"))
    {
        file.content = file.content.replacen(
            "  projectRootFile",
            &format!("  imports = [\n{imports}  ];\n  projectRootFile"),
            1,
        );
    }
    Ok(())
}

pub fn run(command: crate::cli::MonorepoCommand) -> Result<()> {
    match command.action {
        crate::cli::MonorepoAction::Check(command) => check::run(command),
        crate::cli::MonorepoAction::Plan(command) => {
            let (root, project) = load(&std::env::current_dir()?)?;
            let graph = project
                .monorepo
                .as_ref()
                .context("missing monorepo config")?
                .resolve(&root)?;
            let changed = if let Some(base) = command.base.as_deref() {
                changed_paths(&root, base)?
            } else {
                ChangedPaths {
                    full: command.changed_paths.is_empty(),
                    paths: command.changed_paths,
                }
            };
            let plan = select(&graph, &changed.paths, changed.full)?;
            if command.json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            } else {
                for id in &plan.selected {
                    println!("{id}: {}", plan.reasons[id].join(", "));
                }
            }
            Ok(())
        }
    }
}
