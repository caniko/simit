//! Explicit static version owners for non-Cargo monorepo packages.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use toml_edit::{DocumentMut, value};

use super::{Component, Graph};
use crate::cli::ReleaseCommand;
use crate::{cargo, changelog, git, release_identity::ReleaseTag};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestRelease {
    pub manifest: String,
    pub namespace: String,
    #[serde(default)]
    pub publish: bool,
}

impl ManifestRelease {
    pub(super) fn validate(&self, component: &Component) -> Result<()> {
        super::validate_relative_path(&self.manifest)?;
        ReleaseTag::for_package(&self.namespace, Version::new(0, 0, 0))?;
        ensure!(
            matches!(
                Path::new(&self.manifest)
                    .file_name()
                    .and_then(|s| s.to_str()),
                Some("pyproject.toml" | "package.json")
            ),
            "non-Cargo releases require pyproject.toml or package.json"
        );
        ensure!(
            component
                .paths
                .iter()
                .any(|path| Path::new(&self.manifest).starts_with(path)),
            "release manifest {} must be owned by component {}",
            self.manifest,
            component.id
        );
        Ok(())
    }

    pub(crate) fn load(&self, root: &Path) -> Result<Package> {
        let mut path = root.to_path_buf();
        for part in Path::new(&self.manifest).components() {
            path.push(part);
            ensure!(
                !fs::symlink_metadata(&path)?.file_type().is_symlink(),
                "release manifest must not traverse a symlink: {}",
                path.display()
            );
        }
        let content = fs::read_to_string(&path)?;
        self.parse(&content)
    }

    pub(crate) fn parse(&self, content: &str) -> Result<Package> {
        let registry = super::native_registry::Registry::for_manifest(&self.manifest);
        let (name, version, private) =
            if matches!(registry, super::native_registry::Registry::Python) {
                let doc: DocumentMut = content.parse()?;
                let project = doc
                    .get("project")
                    .context("Python releases require static [project] metadata")?;
                if let Some(dynamic) = project.get("dynamic").and_then(toml_edit::Item::as_array) {
                    ensure!(
                        !dynamic
                            .iter()
                            .any(|field| field.as_str() == Some("version")),
                        "dynamic Python versions cannot be independently mutated"
                    );
                }
                (
                    project
                        .get("name")
                        .and_then(toml_edit::Item::as_str)
                        .context("missing project.name")?
                        .to_owned(),
                    project
                        .get("version")
                        .and_then(toml_edit::Item::as_str)
                        .context("Python releases require static project.version")?
                        .to_owned(),
                    false,
                )
            } else {
                let doc: Value = serde_json::from_str(content)?;
                (
                    doc["name"]
                        .as_str()
                        .context("missing npm package name")?
                        .to_owned(),
                    doc["version"]
                        .as_str()
                        .context("npm releases require a static version")?
                        .to_owned(),
                    doc["private"].as_bool().unwrap_or(false),
                )
            };
        ensure!(!name.is_empty(), "release package name cannot be empty");
        let version = Version::parse(&version)?;
        if self.publish && !private {
            registry.version(&version)?;
            if matches!(registry, super::native_registry::Registry::Npm) {
                let doc: Value = serde_json::from_str(content)?;
                ensure!(
                    doc["publishConfig"]["registry"].is_null()
                        || doc["publishConfig"]["registry"]
                            .as_str()
                            .is_some_and(
                                |url| url.trim_end_matches('/') == "https://registry.npmjs.org"
                            ),
                    "custom npm registry requires its own publication backend"
                );
                ensure!(
                    version.pre.is_empty()
                        || doc["publishConfig"]["tag"]
                            .as_str()
                            .is_some_and(|tag| !tag.is_empty()),
                    "npm prereleases require an explicit publishConfig.tag"
                );
            }
        }
        Ok(Package {
            config: self.clone(),
            name,
            version,
            publish: self.publish && !private,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Package {
    pub config: ManifestRelease,
    pub name: String,
    pub version: Version,
    pub publish: bool,
}

pub(super) fn validate(root: &Path, graph: &Graph) -> Result<()> {
    let mut namespaces: BTreeSet<_> = graph
        .components
        .values()
        .flat_map(|c| c.cargo_packages.iter().cloned())
        .collect();
    // Native package names belong to separate registries. Git release tags use
    // the globally unique namespace above, even when npm and PyPI names match.
    let mut names: BTreeSet<_> = namespaces
        .iter()
        .map(|name| ("cargo", name.clone()))
        .collect();
    let mut manifests = BTreeSet::new();
    for release in graph.components.values().flat_map(|c| &c.releases) {
        ensure!(
            namespaces.insert(release.namespace.clone()),
            "duplicate release namespace {}",
            release.namespace
        );
        ensure!(
            manifests.insert(&release.manifest),
            "duplicate release manifest {}",
            release.manifest
        );
        let package = release.load(root)?;
        let registry = super::native_registry::Registry::for_manifest(&release.manifest);
        ensure!(
            names.insert((registry.name(), registry.package_key(&package.name))),
            "duplicate {} release package name {}",
            registry.name(),
            package.name
        );
    }
    Ok(())
}

pub(crate) struct Selection {
    pub root: PathBuf,
    pub project: crate::config::ProjectConfig,
    pub graph: Graph,
    pub component: String,
    pub package: Package,
}

pub(crate) fn select(command: &ReleaseCommand) -> Result<Option<Selection>> {
    let Some(id) = &command.component else {
        return Ok(None);
    };
    let (root, project) = super::load(&std::env::current_dir()?)?;
    // Preserve the Cargo fast path without re-running metadata or native tools.
    let component = project
        .monorepo
        .as_ref()
        .context("missing monorepo config")?
        .components
        .iter()
        .find(|c| &c.id == id)
        .with_context(|| format!("unknown monorepo component {id}"))?;
    if component.releases.is_empty() {
        return Ok(None);
    }
    ensure!(
        command.packages.len() <= 1,
        "independent releases select exactly one package"
    );
    if command
        .packages
        .first()
        .is_some_and(|name| component.cargo_packages.contains(name))
    {
        return Ok(None);
    }
    if command.packages.is_empty() {
        ensure!(
            component.cargo_packages.is_empty() && component.releases.len() == 1,
            "component {id} has multiple release packages; select --package explicitly"
        );
    }
    let packages = component
        .releases
        .iter()
        .map(|release| release.load(&root))
        .collect::<Result<Vec<_>>>()?;
    // An explicit release namespace is authoritative. Native names remain a
    // convenient selector only when they identify one owner in this component.
    let index = if let Some(requested) = command.packages.first() {
        if let Some(index) = packages
            .iter()
            .position(|package| &package.config.namespace == requested)
        {
            index
        } else {
            let matches: Vec<_> = packages
                .iter()
                .enumerate()
                .filter(|(_, package)| &package.name == requested)
                .collect();
            ensure!(
                matches.len() <= 1,
                "ambiguous package {requested}; select --package <release namespace>"
            );
            matches
                .first()
                .with_context(|| format!("--package must belong to component {id}"))?
                .0
        }
    } else {
        0
    };
    let package = packages
        .into_iter()
        .nth(index)
        .context("missing release owner")?;
    let graph = project
        .monorepo
        .as_ref()
        .context("missing monorepo config")?
        .resolve(&root)?;
    Ok(Some(Selection {
        root,
        project,
        graph,
        component: id.clone(),
        package,
    }))
}

pub(crate) fn print_plan(selection: &Selection, command: &ReleaseCommand) -> Result<()> {
    ensure!(
        !command.dry_run_package,
        "non-Cargo package probes use component checks, not cargo package"
    );
    let package = &selection.package;
    let tag = ReleaseTag::for_package(&package.config.namespace, package.version.clone())?;
    if command.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schemaVersion": 1, "component": selection.component, "entries": [{"component": selection.component, "name": package.name, "version": package.version.to_string(), "tag": tag.to_string(), "manifest_path": selection.root.join(&package.config.manifest), "publish": package.publish, "depends_on": selection.graph.components[&selection.component].depends_on}], "skippedMembers": []})
            )?
        );
    } else {
        println!(
            "{} {} ({tag}; publish = {})",
            package.name, package.version, package.publish
        );
    }
    Ok(())
}

pub(crate) fn bump(selection: &Selection, command: &ReleaseCommand) -> Result<()> {
    let spec = cargo::BumpSpec::new(
        command
            .action
            .bump_kind()
            .context("expected version bump")?,
        command.pre.clone(),
    )?;
    let package = &selection.package;
    let version = cargo::bump_version(package.version.clone(), &spec);
    let tag = ReleaseTag::for_package(&package.config.namespace, version.clone())?;
    let message = command
        .message
        .as_ref()
        .context("release commit message is required; pass -m <message>")?;
    if command.dry_run {
        println!(
            "simit release dry-run\npackage {}: {} -> {version}\nwould qualify component {}\nwould create tag {tag}",
            package.name, package.version, selection.component
        );
        return Ok(());
    }
    git::ensure_worktree_clean(&selection.root)?;
    let head = git::head_commit(&selection.root)?;
    git::release_preflight(&selection.root, !command.no_tag, !command.no_sign, &tag)?;
    let mut updates = package.version_updates(&selection.root, &version)?;
    let directory = selection
        .root
        .join(&package.config.manifest)
        .parent()
        .context("manifest has no parent")?
        .to_path_buf();
    let notes_path = directory.join(changelog::DEFAULT_PATH);
    let notes_enabled = !command.no_changelog
        && selection.project.release.notes_source(
            selection
                .project
                .ci
                .platform
                .unwrap_or(crate::cli::Platform::Forgejo),
        ) == crate::config::ReleaseNotesSource::Changelog;
    if notes_enabled && notes_path.exists() {
        ensure!(
            !fs::symlink_metadata(&notes_path)?.file_type().is_symlink(),
            "release changelog must not be a symlink: {}",
            notes_path.display()
        );
        let mut content = fs::read_to_string(&notes_path)?;
        if selection.project.release.changelog.auto_draft {
            content = changelog::draft_component_content(
                &content,
                &selection.root,
                &package.config.namespace,
                &selection.graph.components[&selection.component].paths,
            )?;
        }
        let notes = changelog::release_content_with_namespace(
            &content,
            &version,
            changelog::today_utc()?,
            None,
            &notes_path,
            Some(&selection.root),
            Some(&package.config.namespace),
        )?;
        let before = fs::read(&notes_path)?;
        updates.push(Update {
            path: notes_path,
            before,
            after: notes.into_bytes(),
        });
    }
    qualify(selection)?;
    git::ensure_worktree_clean(&selection.root)?;
    ensure!(
        git::head_commit(&selection.root)? == head,
        "release HEAD changed during qualification"
    );
    for update in &updates {
        ensure!(
            fs::read(&update.path)? == update.before,
            "release path changed during qualification: {}",
            update.path.display()
        );
    }
    for update in &updates {
        fs::write(&update.path, &update.after)?;
    }
    git::stage_paths(
        &selection.root,
        &updates
            .into_iter()
            .map(|update| update.path)
            .collect::<Vec<_>>(),
    )?;
    git::commit(&selection.root, &["-m".into(), message.into()])?;
    if !command.no_tag {
        git::tag(&selection.root, &tag, !command.no_sign)?;
    }
    crate::registry::refresh_current_project_or_warn();
    Ok(())
}

struct Update {
    path: PathBuf,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl Package {
    fn version_updates(&self, root: &Path, version: &Version) -> Result<Vec<Update>> {
        if self.publish {
            super::native_registry::Registry::for_manifest(&self.config.manifest)
                .version(version)?;
        }
        let path = root.join(&self.config.manifest);
        let before = fs::read(&path)?;
        let content = std::str::from_utf8(&before)?;
        let mut updates = Vec::new();
        if matches!(
            super::native_registry::Registry::for_manifest(&self.config.manifest),
            super::native_registry::Registry::Python
        ) {
            let mut doc: DocumentMut = content.parse()?;
            doc["project"]["version"] = value(version.to_string());
            updates.push(Update {
                path: path.clone(),
                before,
                after: doc.to_string().into_bytes(),
            });
            for directory in path
                .parent()
                .context("manifest has no parent")?
                .ancestors()
                .take_while(|directory| directory.starts_with(root))
            {
                let lock = directory.join("uv.lock");
                if !lock.exists() {
                    continue;
                }
                ensure!(
                    !fs::symlink_metadata(&lock)?.file_type().is_symlink(),
                    "release lock must not be a symlink: {}",
                    lock.display()
                );
                let before = fs::read(&lock)?;
                let mut doc: DocumentMut = std::str::from_utf8(&before)?.parse()?;
                let mut found = false;
                if let Some(packages) = doc
                    .get_mut("package")
                    .and_then(toml_edit::Item::as_array_of_tables_mut)
                {
                    for package in packages.iter_mut() {
                        let registry = super::native_registry::Registry::Python;
                        if package
                            .get("name")
                            .and_then(toml_edit::Item::as_str)
                            .is_some_and(|name| {
                                registry.package_key(name) == registry.package_key(&self.name)
                            })
                            && package.get("source").is_some_and(|source| {
                                source.get("editable").is_some() || source.get("virtual").is_some()
                            })
                        {
                            let current = registry
                                .version(&self.version)
                                .unwrap_or_else(|_| self.version.to_string());
                            ensure!(
                                package
                                    .get("version")
                                    .and_then(toml_edit::Item::as_str)
                                    .is_some_and(|version| version == current
                                        || version == self.version.to_string()),
                                "uv.lock package version disagrees with manifest"
                            );
                            package["version"] = value(
                                registry
                                    .version(version)
                                    .unwrap_or_else(|_| version.to_string()),
                            );
                            found = true;
                        }
                    }
                }
                ensure!(
                    found,
                    "uv.lock has no local version record for {}; regenerate the lock before releasing",
                    self.name
                );
                if found {
                    updates.push(Update {
                        path: lock,
                        before,
                        after: doc.to_string().into_bytes(),
                    });
                    break;
                }
            }
        } else {
            let mut doc: Value = serde_json::from_str(content)?;
            doc["version"] = json!(version.to_string());
            updates.push(Update {
                path: path.clone(),
                before,
                after: json_bytes(&doc)?,
            });
            for name in ["package-lock.json", "npm-shrinkwrap.json"] {
                let lock = path.parent().context("manifest has no parent")?.join(name);
                if !lock.exists() {
                    continue;
                }
                ensure!(
                    !fs::symlink_metadata(&lock)?.file_type().is_symlink(),
                    "release lock must not be a symlink: {}",
                    lock.display()
                );
                let before = fs::read(&lock)?;
                let mut doc: Value = serde_json::from_slice(&before)?;
                ensure!(
                    doc["name"] == self.name && doc["version"] == self.version.to_string(),
                    "npm lock identity/version disagrees with manifest"
                );
                doc["version"] = json!(version.to_string());
                if let Some(package) = doc
                    .get_mut("packages")
                    .and_then(|packages| packages.get_mut(""))
                {
                    ensure!(
                        package["version"] == self.version.to_string(),
                        "npm root lock version disagrees with manifest"
                    );
                    package["version"] = json!(version.to_string());
                }
                updates.push(Update {
                    path: lock,
                    before,
                    after: json_bytes(&doc)?,
                });
            }
        }
        self.config.parse(std::str::from_utf8(&updates[0].after)?)?;
        Ok(updates)
    }
}

fn json_bytes(doc: &Value) -> Result<Vec<u8>> {
    Ok(format!("{}\n", serde_json::to_string_pretty(doc)?).into_bytes())
}

pub(crate) fn qualify(selection: &Selection) -> Result<()> {
    let plan = super::select(
        &selection.graph,
        std::slice::from_ref(&selection.package.config.manifest),
        false,
    )?;
    for id in &plan.selected {
        let component = &selection.graph.components[id];
        ensure!(
            !component.checks.is_empty(),
            "component {} requires explicit release checks",
            component.id
        );
        for gate in &component.checks {
            println!("qualifying component {} check {}", component.id, gate.id);
            let status = Command::new("timeout")
                .current_dir(&selection.root)
                .envs(&gate.env)
                .args([
                    "--kill-after=5s",
                    &format!("{}m", gate.timeout_minutes),
                    "bash",
                    "-euo",
                    "pipefail",
                    "-c",
                    &gate.run,
                ])
                .status()
                .with_context(|| format!("running component {} check {}", component.id, gate.id))?;
            ensure!(
                status.success(),
                "component {} check {} failed: {status}",
                component.id,
                gate.id
            );
        }
    }
    Ok(())
}
