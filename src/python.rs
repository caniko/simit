use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub version: String,
    pub requires_python: Option<String>,
    pub scripts: Vec<String>,
    pub optional_extras: Vec<String>,
    pub dependency_groups: Vec<String>,
    pub workspace_root: Utf8PathBuf,
}

#[derive(Debug, Deserialize)]
struct Pyproject {
    #[serde(default)]
    project: Option<ProjectTable>,
    #[serde(default)]
    tool: Option<ToolTable>,
    #[serde(default, rename = "dependency-groups")]
    dependency_groups: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ToolTable {
    #[serde(default)]
    poetry: Option<PoetryProjectTable>,
}

#[derive(Debug, Deserialize)]
struct ProjectTable {
    name: String,
    version: String,
    #[serde(default, rename = "requires-python")]
    requires_python: Option<String>,
    #[serde(default)]
    scripts: BTreeMap<String, String>,
    #[serde(default, rename = "optional-dependencies")]
    optional_dependencies: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct PoetryProjectTable {
    name: String,
    version: String,
    #[serde(default)]
    python: Option<String>,
    #[serde(default)]
    scripts: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    extras: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    group: BTreeMap<String, serde_json::Value>,
}

pub fn project_for_current_dir() -> Result<Project> {
    let start = std::env::current_dir().context("reading current directory")?;
    let root = find_project_root(&start)?;
    load_project(&root)
}

pub fn find_project_root(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        if dir.join("pyproject.toml").exists() && dir.join("uv.lock").exists() {
            return Ok(dir.to_path_buf());
        }
    }

    bail!(
        "could not find Python uv project (pyproject.toml and uv.lock) in {} or its parents",
        start.display()
    );
}

pub fn load_project(root: &Path) -> Result<Project> {
    let pyproject_path = root.join("pyproject.toml");
    let content = fs::read_to_string(&pyproject_path)
        .with_context(|| format!("reading {}", pyproject_path.display()))?;
    let pyproject = toml_edit::de::from_str::<Pyproject>(&content)
        .with_context(|| format!("parsing {}", pyproject_path.display()))?;

    let (name, version, requires_python, mut scripts, mut optional_extras, mut dependency_groups) =
        if let Some(project) = pyproject.project {
            (
                project.name,
                project.version,
                project.requires_python,
                project.scripts.keys().cloned().collect::<Vec<_>>(),
                project
                    .optional_dependencies
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
                pyproject
                    .dependency_groups
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else if let Some(poetry) = pyproject.tool.and_then(|tool| tool.poetry) {
            (
                poetry.name,
                poetry.version,
                poetry.python,
                poetry.scripts.keys().cloned().collect::<Vec<_>>(),
                poetry.extras.keys().cloned().collect::<Vec<_>>(),
                poetry.group.keys().cloned().collect::<Vec<_>>(),
            )
        } else {
            bail!("pyproject.toml has neither [project] nor [tool.poetry]");
        };
    scripts.sort();
    optional_extras.sort();
    dependency_groups.sort();

    Ok(Project {
        name,
        version,
        requires_python,
        scripts,
        optional_extras,
        dependency_groups,
        workspace_root: Utf8PathBuf::from_path_buf(root.to_path_buf())
            .map_err(|path| anyhow::anyhow!("workspace root is not UTF-8: {}", path.display()))?,
    })
}

pub fn is_python_uv_project(root: &Path) -> bool {
    root.join("pyproject.toml").exists() && root.join("uv.lock").exists()
}
