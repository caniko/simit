//! Additive project templates share the built-in CI generation and drift gates.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    cli::{CiProvider, Platform},
    config::CiConfig,
    project::GeneratedFile,
};

pub(crate) const TEMPLATE_MARKER: &str = "# Simit workflow template: ";

fn portable_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn same_output(root: &Path, left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if portable_path(left) != portable_path(right) {
        return false;
    }
    // A case-only rename aliases an existing file on insensitive filesystems,
    // but identifies two distinct outputs on sensitive filesystems.
    match (
        root.join(left).canonicalize(),
        root.join(right).canonicalize(),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn is_actions_workflow(path: &Path) -> bool {
    let normalized = portable_path(path);
    let path = Path::new(&normalized);
    matches!(
        path.parent().and_then(Path::to_str),
        Some(".github/workflows" | ".forgejo/workflows")
    ) && matches!(
        path.extension().and_then(|part| part.to_str()),
        Some("yaml" | "yml")
    )
}

pub(crate) fn is_template_output(ci: &CiConfig, path: &Path, content: &str) -> bool {
    content.contains(TEMPLATE_MARKER)
        || ci
            .workflow_templates
            .keys()
            .any(|output| portable_path(Path::new(output)) == portable_path(path))
}

fn relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\n', '\r', '\0'])
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn variable_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn validate_config(ci: &CiConfig) -> Result<()> {
    if ci.workflow_templates.is_empty() {
        if !ci.workflow_variables.is_empty() {
            bail!("[ci.workflow_variables] requires [ci.workflow_templates]");
        }
        return Ok(());
    }
    if ci.provider == Some(CiProvider::Crow) || ci.platform == Some(Platform::Gitlab) {
        bail!("[ci.workflow_templates] requires GitHub or Forgejo Actions");
    }
    let outputs = ci
        .workflow_templates
        .keys()
        .map(|output| portable_path(Path::new(output)))
        .collect::<BTreeSet<_>>();
    if outputs.len() != ci.workflow_templates.len() {
        bail!("workflow templates collide on case-insensitive filesystems");
    }
    for (output, source) in &ci.workflow_templates {
        let path = Path::new(output);
        if !relative_path(output)
            || !relative_path(source)
            || !matches!(
                path.parent().and_then(Path::to_str),
                Some(".github/workflows" | ".forgejo/workflows")
            )
            || !matches!(
                path.extension().and_then(|part| part.to_str()),
                Some("yaml" | "yml")
            )
            || outputs.contains(&portable_path(Path::new(source)))
        {
            bail!(
                "invalid [ci.workflow_templates] mapping {output:?} = {source:?}; use distinct repository-relative template and Actions workflow paths"
            );
        }
        if crate::registry::is_release_workflow_path(Path::new(&portable_path(path))) {
            bail!("workflow template {output} collides with a release-owned workflow");
        }
    }
    for name in ci.workflow_variables.keys() {
        if !variable_name(name) {
            bail!("invalid [ci.workflow_variables] name {name:?}");
        }
    }
    Ok(())
}

fn substitute(template: &str, ci: &CiConfig) -> Result<String> {
    let mut remaining = template;
    let mut output = String::new();
    while let Some((prefix, rest)) = remaining.split_once("@simit(") {
        output.push_str(prefix);
        let (name, tail) = rest
            .split_once(")@")
            .context("unterminated @simit(name)@ placeholder")?;
        let value = ci
            .workflow_variables
            .get(name)
            .with_context(|| format!("undefined workflow template variable {name:?}"))?;
        // Values are inserted once; GitHub expressions and nested tokens remain literal.
        output.push_str(value);
        remaining = tail;
    }
    output.push_str(remaining);
    Ok(output)
}

pub(crate) fn validate_backend(
    ci: &CiConfig,
    provider: CiProvider,
    platform: Platform,
) -> Result<()> {
    validate_config(ci)?;
    if !ci.workflow_templates.is_empty() {
        if provider != CiProvider::Actions || platform == Platform::Gitlab {
            bail!("[ci.workflow_templates] requires GitHub or Forgejo Actions");
        }
        for output in ci.workflow_templates.keys() {
            if Path::new(output).parent() != Some(Path::new(platform.workflow_dir())) {
                bail!("workflow template {output} does not match the selected Actions platform");
            }
        }
    }
    Ok(())
}

pub(crate) fn append(root: &Path, ci: &CiConfig, files: &mut Vec<GeneratedFile>) -> Result<()> {
    validate_config(ci)?;
    if ci.workflow_templates.is_empty() {
        return Ok(());
    }
    let canonical_root = root
        .canonicalize()
        .context("resolving workflow template root")?;
    let builtin_paths = files
        .iter()
        .map(|file| portable_path(&file.relative_path))
        .collect::<BTreeSet<_>>();
    let mut templates = Vec::new();
    for (output, source) in &ci.workflow_templates {
        let relative = PathBuf::from(output);
        for ancestor in relative
            .ancestors()
            .filter(|path| !path.as_os_str().is_empty())
        {
            let path = root.join(ancestor);
            if path.is_symlink() {
                bail!(
                    "workflow template output {output} crosses a symlink at {}",
                    path.display()
                );
            }
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    let valid = if ancestor == relative {
                        metadata.is_file()
                    } else {
                        metadata.is_dir()
                    };
                    if !valid {
                        bail!(
                            "workflow template output {output} crosses a non-file destination or non-directory parent at {}",
                            path.display()
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("inspecting workflow template destination");
                }
            }
        }
        let destination = root.join(&relative);
        if destination.try_exists()? {
            let sources = ci.workflow_templates.values().map(Path::new);
            let outputs = ci.workflow_templates.keys().map(Path::new);
            let builtins = files.iter().map(|file| file.relative_path.as_path());
            for candidate in sources.chain(outputs).chain(builtins) {
                if candidate == relative {
                    continue;
                }
                let candidate = root.join(candidate);
                if candidate.try_exists()?
                    && same_file::is_same_file(&destination, &candidate)
                        .context("comparing workflow template file identities")?
                {
                    bail!(
                        "workflow template output {output} aliases another source or generated output at {}; use distinct files",
                        candidate.display()
                    );
                }
            }
        }
        if builtin_paths.contains(&portable_path(&relative)) {
            bail!("workflow template {output} collides with a built-in generated workflow");
        }
        if builtin_paths.contains(&portable_path(Path::new(source))) {
            bail!(
                "workflow template source {source} is a built-in generated output; use a project-owned template"
            );
        }
        if !builtin_paths.iter().any(|path| {
            Path::new(path).parent().map(portable_path) == relative.parent().map(portable_path)
        }) {
            bail!("workflow template {output} does not match the selected Actions platform");
        }
        let path = root
            .join(source)
            .canonicalize()
            .with_context(|| format!("resolving workflow template {source}"))?;
        if !path.starts_with(&canonical_root) {
            bail!("workflow template {source} escapes the repository");
        }
        if builtin_paths
            .iter()
            .any(|builtin| portable_path(&path) == portable_path(&canonical_root.join(builtin)))
        {
            bail!(
                "workflow template source {source} resolves to a built-in generated output; use a project-owned template"
            );
        }
        let source_text = fs::read_to_string(&path)
            .with_context(|| format!("reading workflow template {source}"))?;
        if super::ci::is_generated_workflow_marker(&source_text) {
            bail!(
                "workflow template source {source} is a built-in generated output; use a project-owned template"
            );
        }
        if is_actions_workflow(Path::new(source))
            || is_actions_workflow(path.strip_prefix(&canonical_root)?)
        {
            bail!(
                "workflow template source {source} is an active Actions workflow; use a project-owned template with a non-workflow extension or directory"
            );
        }
        let rendered = substitute(&source_text, ci)
            .with_context(|| format!("rendering workflow template {source}"))?;
        let _: serde_yaml::Value = serde_yaml::from_str(&rendered)
            .with_context(|| format!("parsing rendered workflow {output}"))?;
        templates.push(GeneratedFile {
            relative_path: relative,
            content: format!(
                "{TEMPLATE_MARKER}{source}\n{}\n{rendered}",
                super::ci::GENERATED_WORKFLOW_MARKER
            ),
        });
    }
    // Complete all template validation before mutating the generation plan.
    files.extend(templates);
    Ok(())
}

pub(crate) fn obsolete(root: &Path, files: &[GeneratedFile]) -> Result<Vec<PathBuf>> {
    let expected = files
        .iter()
        .map(|file| &file.relative_path)
        .collect::<BTreeSet<_>>();
    let mut obsolete = Vec::new();
    for directory in [".github/workflows", ".forgejo/workflows"] {
        let path = root.join(directory);
        if !path.exists() {
            continue;
        }
        if !path.canonicalize()?.starts_with(root.canonicalize()?) {
            bail!("workflow template directory {directory} escapes the repository");
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let relative = PathBuf::from(directory).join(entry.file_name());
            if !matches!(
                relative.extension().and_then(|part| part.to_str()),
                Some("yaml" | "yml")
            ) {
                continue;
            }
            if expected
                .iter()
                .any(|expected| same_output(root, expected, &relative))
            {
                continue;
            }
            let content = fs::read_to_string(entry.path())?;
            if content.contains(TEMPLATE_MARKER)
                && super::ci::is_generated_workflow_marker(&content)
            {
                obsolete.push(relative);
            }
        }
    }
    Ok(obsolete)
}
