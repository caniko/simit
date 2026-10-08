//! One project-owned component graph shared by local planning and CI generation.

pub(crate) mod ci;
mod plan;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component as PathComponent, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::{ProjectConfig, RequiredGate};

pub use plan::{Plan, changed_paths, select};

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
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub systems: Vec<String>,
    #[serde(default)]
    pub checks: Vec<RequiredGate>,
}

#[derive(Debug)]
pub struct Graph {
    pub components: BTreeMap<String, Component>,
    pub shared_paths: Vec<String>,
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
        for component in &self.components {
            validate_id(&component.id)?;
            if !ids.insert(&component.id) {
                bail!("duplicate monorepo component {}", component.id);
            }
            for path in &component.paths {
                validate_relative_path(path)?;
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
                }
            }
        }
        for path in self.shared_paths.iter().chain(&self.formatter_modules) {
            validate_relative_path(path)?;
        }
        self.graph(None)?.validate_ownership()
    }

    /// Cargo metadata supplies path ownership and all local dependency edges.
    pub fn resolve(&self, root: &Path) -> Result<Graph> {
        self.validate()?;
        let metadata = if root.join("Cargo.toml").is_file() {
            Some(crate::cargo::cargo_metadata(&root.join("Cargo.toml"))?)
        } else {
            if self
                .components
                .iter()
                .any(|component| !component.cargo_packages.is_empty())
            {
                bail!("monorepo Cargo ownership requires Cargo.toml at the repository root");
            }
            None
        };
        let graph = self.graph(metadata.as_ref())?;
        graph.validate_ownership()?;
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

    fn graph(&self, metadata: Option<&crate::cargo::Metadata>) -> Result<Graph> {
        let mut components: BTreeMap<_, _> = self
            .components
            .iter()
            .map(|c| (c.id.clone(), c.clone()))
            .collect();
        if let Some(metadata) = metadata {
            let mut owners = BTreeMap::new();
            let packages: BTreeMap<_, _> = metadata
                .packages
                .iter()
                .filter(|p| metadata.workspace_members.contains(&p.id))
                .map(|p| (p.name.as_str(), p))
                .collect();
            for component in components.values_mut() {
                for name in &component.cargo_packages {
                    let package = packages.get(name.as_str()).with_context(|| {
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
                    validate_relative_path(directory.as_str())?;
                    component.paths.push(directory.to_string());
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
        crate::cli::MonorepoAction::Plan(command) => {
            let (root, project) = load(&std::env::current_dir()?)?;
            let graph = project
                .monorepo
                .as_ref()
                .context("missing monorepo config")?
                .resolve(&root)?;
            let paths = if let Some(base) = command.base.as_deref() {
                changed_paths(&root, base)?
            } else {
                command.changed_paths
            };
            let plan = select(&graph, &paths, paths.is_empty() && command.base.is_none())?;
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
