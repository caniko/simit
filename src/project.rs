use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::render::ci::is_generated_workflow_marker;
use crate::render::diff::unified_diff;

#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub relative_path: PathBuf,
    pub content: String,
}

/// A complete generated-workflow plan: the files to render plus a predicate
/// over workflow file names, used to sweep the shared workflow directories
/// for obsolete generator-owned files. Both CI ([`crate::commands::init_ci`])
/// and release ([`crate::commands::init_release`]) generation converge here so
/// the obsolete scan and reconcile path exist in exactly one place.
pub struct GeneratedPlan<'a> {
    pub files: Vec<GeneratedFile>,
    pub message: &'a str,
    pub owns_name: fn(&OsStr) -> bool,
}

impl GeneratedPlan<'_> {
    pub fn check(&self, workspace_root: &Path, show_diff: bool) -> Result<()> {
        let obsolete = obsolete_generated_workflows(workspace_root, &self.files, self.owns_name)?;
        reconcile_generated_files(
            workspace_root,
            &self.files,
            &obsolete,
            self.message,
            true,
            show_diff,
        )
    }

    pub fn write(&self, workspace_root: &Path) -> Result<()> {
        let obsolete = obsolete_generated_workflows(workspace_root, &self.files, self.owns_name)?;
        reconcile_generated_files(
            workspace_root,
            &self.files,
            &obsolete,
            self.message,
            false,
            false,
        )
    }
}

/// Scan the shared workflow directories ([`.forgejo/workflows`],
/// [`.github/workflows`], [`.crow`]) for generator-owned workflow files that
/// are not part of `files` and would be left behind by a write.
fn obsolete_generated_workflows(
    workspace_root: &Path,
    files: &[GeneratedFile],
    owns_name: fn(&OsStr) -> bool,
) -> Result<Vec<PathBuf>> {
    let expected = files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let mut obsolete = Vec::new();
    for relative_dir in [".forgejo/workflows", ".github/workflows", ".crow"] {
        let directory = workspace_root.join(relative_dir);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", directory.display()));
            }
        };
        for entry in entries {
            let entry =
                entry.with_context(|| format!("reading entry in {}", directory.display()))?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                continue;
            };
            if !matches!(extension, "yaml" | "yml" | "jsonnet") {
                continue;
            }
            let relative = PathBuf::from(relative_dir).join(entry.file_name());
            if expected.contains(&relative) || !owns_name(entry.file_name().as_os_str()) {
                continue;
            }
            let content =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            if is_generated_workflow_marker(&content) {
                obsolete.push(relative);
            }
        }
    }
    obsolete.sort();
    Ok(obsolete)
}

/// Apply a complete generated-file plan without leaving a half-applied
/// migration behind.  `obsolete` must contain only generator-owned paths.
pub fn reconcile_generated_files(
    workspace_root: &Path,
    files: &[GeneratedFile],
    obsolete: &[PathBuf],
    message: &str,
    check: bool,
    show_diff: bool,
) -> Result<()> {
    validate_generated_paths(files, obsolete)?;
    if check {
        check_generated_files(workspace_root, files, message, show_diff)?;
        if !obsolete.is_empty() {
            bail!(
                "{message}:\n{}",
                obsolete
                    .iter()
                    .map(|path| format!("{} is extra (obsolete)", path.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        return Ok(());
    }

    write_generated_files(workspace_root, files)?;
    remove_generated_files(workspace_root, obsolete)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Languages {
    pub rust: bool,
    pub nix: bool,
    pub uv_python: bool,
    pub toml: bool,
    pub yaml: bool,
    pub markdown: bool,
}

pub fn detect_languages(workspace_root: &Path) -> Result<Languages> {
    let mut languages = Languages {
        rust: workspace_root.join("Cargo.toml").exists(),
        uv_python: is_uv_python_project(workspace_root)?,
        ..Languages::default()
    };

    detect_languages_in_dir(workspace_root, workspace_root, &mut languages)?;
    Ok(languages)
}

pub fn write_generated_files(workspace_root: &Path, files: &[GeneratedFile]) -> Result<()> {
    validate_generated_paths(files, &[])?;
    for file in files {
        let path = workspace_root.join(&file.relative_path);
        refuse_symlink(&path)?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("generated path has no parent: {}", path.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        fs::write(&path, &file.content).with_context(|| format!("writing {}", path.display()))?;
    }

    Ok(())
}

pub fn remove_generated_files(workspace_root: &Path, paths: &[PathBuf]) -> Result<()> {
    for relative_path in paths {
        let path = workspace_root.join(relative_path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        if metadata.file_type().is_symlink() {
            bail!("refusing to remove symlink {}", path.display());
        }
        if !metadata.file_type().is_file() {
            bail!("refusing to remove non-file {}", path.display());
        }
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

pub fn check_generated_files(
    workspace_root: &Path,
    files: &[GeneratedFile],
    message: &str,
    show_diff: bool,
) -> Result<()> {
    let mut mismatches = Vec::new();
    let mut diffs = Vec::new();

    for file in files {
        let path = workspace_root.join(&file.relative_path);
        match fs::read_to_string(&path) {
            Ok(actual) if actual == file.content => {}
            Ok(actual) => {
                mismatches.push(format!("{} differs", file.relative_path.display()));
                if show_diff {
                    diffs.push(unified_diff(
                        &file.relative_path.display().to_string(),
                        &actual,
                        &file.content,
                    ));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                mismatches.push(format!("{} is missing", file.relative_path.display()));
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else if diffs.is_empty() {
        bail!("{message}:\n{}", mismatches.join("\n"));
    } else {
        bail!(
            "{message}:\n{}\n{}",
            mismatches.join("\n"),
            diffs.join("\n")
        );
    }
}

fn validate_generated_paths(files: &[GeneratedFile], obsolete: &[PathBuf]) -> Result<()> {
    let mut paths = std::collections::BTreeSet::new();
    for path in files
        .iter()
        .map(|file| &file.relative_path)
        .chain(obsolete.iter())
    {
        validate_relative_path(path)?;
        if !paths.insert(path.clone()) {
            bail!("duplicate generated path: {}", path.display());
        }
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "generated path must stay below the workspace root: {}",
            path.display()
        );
    }
    Ok(())
}

fn refuse_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing to overwrite symlink {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn is_uv_python_project(workspace_root: &Path) -> Result<bool> {
    if workspace_root.join("uv.lock").exists() {
        return Ok(true);
    }

    let pyproject = workspace_root.join("pyproject.toml");
    if !pyproject.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&pyproject)
        .with_context(|| format!("reading {}", pyproject.display()))?;
    Ok(content.contains("[tool.uv") || content.contains("[dependency-groups]"))
}

fn detect_languages_in_dir(root: &Path, dir: &Path, languages: &mut Languages) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading file type for {}", path.display()))?;

        if file_type.is_dir() {
            if should_skip_dir(root, &path) {
                continue;
            }
            detect_languages_in_dir(root, &path, languages)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        match path.extension().and_then(|extension| extension.to_str()) {
            Some("nix") => languages.nix = true,
            Some("toml") => languages.toml = true,
            Some("yaml" | "yml") => languages.yaml = true,
            Some("md" | "markdown") => languages.markdown = true,
            // Rust is a project component only when the repository root owns
            // a Cargo manifest. Source fixtures in Python or documentation
            // projects must not activate Rust tooling.
            Some("rs") => {}
            _ => {}
        }
    }

    Ok(())
}

fn should_skip_dir(root: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    matches!(
        name,
        ".git" | "target" | ".direnv" | "result" | "result-doc" | "node_modules"
    ) || path
        .strip_prefix(root)
        .map(|relative| relative.starts_with(".forgejo/workflows"))
        .unwrap_or(false)
}
