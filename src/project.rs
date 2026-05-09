use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::render::diff::unified_diff;

#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub relative_path: PathBuf,
    pub content: String,
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
    for file in files {
        let path = workspace_root.join(&file.relative_path);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("generated path has no parent: {}", path.display()))?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        fs::write(&path, &file.content).with_context(|| format!("writing {}", path.display()))?;
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
            Some("rs") => languages.rust = true,
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
