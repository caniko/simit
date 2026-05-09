use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use semver::Version;
use time::OffsetDateTime;

pub fn planned_update(workspace_root: &Path, version: &Version, message: &str) -> Result<String> {
    let path = workspace_root.join("CHANGELOG.md");
    let content = fs::read_to_string(&path).with_context(|| {
        format!(
            "reading {}; strict changelog support requires a Keep a Changelog file",
            path.display()
        )
    })?;
    update_content(&content, version, message)
}

pub fn write_update(workspace_root: &Path, updated: &str) -> Result<()> {
    let path = workspace_root.join("CHANGELOG.md");
    fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))
}

pub fn update_content(content: &str, version: &Version, message: &str) -> Result<String> {
    let marker = "## [Unreleased]";
    let Some(marker_index) = content.find(marker) else {
        bail!("CHANGELOG.md must contain `## [Unreleased]`");
    };

    let after_marker = marker_index + marker.len();
    let remaining = &content[after_marker..];
    let next_section = remaining
        .find("\n## ")
        .map(|index| after_marker + index)
        .unwrap_or(content.len());
    let unreleased_body = content[after_marker..next_section].trim();
    let date = OffsetDateTime::now_utc().date().to_string();

    let mut updated = String::new();
    updated.push_str(&content[..after_marker]);
    updated.push_str("\n\n");
    updated.push_str("## [");
    updated.push_str(&version.to_string());
    updated.push_str("] - ");
    updated.push_str(&date);
    updated.push_str("\n\n");
    if unreleased_body.is_empty() {
        updated.push_str("- ");
        updated.push_str(message);
        updated.push('\n');
    } else {
        updated.push_str(unreleased_body);
        updated.push('\n');
    }
    updated.push_str(&content[next_section..]);

    Ok(updated)
}
