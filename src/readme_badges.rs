use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cargo::{self, Metadata, Package};
use crate::config::ProjectConfig;
use crate::registry::{self, FeatureStatus};

pub const START_MARKER: &str = "<!-- simit:badges:start -->";
pub const END_MARKER: &str = "<!-- simit:badges:end -->";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadmeUpgrade {
    pub current: String,
    pub upgraded: String,
}

impl ReadmeUpgrade {
    pub fn changed(&self) -> bool {
        self.current != self.upgraded
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Badge {
    label: String,
    url: String,
    link: Option<String>,
}

pub fn plan(workspace_root: &Path) -> Result<ReadmeUpgrade> {
    let readme_path = workspace_root.join("README.md");
    let current = match fs::read_to_string(&readme_path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            bail!(
                "README.md is required for simit upgrade: create README.md with a top-level `#` heading, then rerun `simit upgrade`"
            );
        }
        Err(err) if err.kind() == ErrorKind::InvalidData => {
            bail!("README.md must be valid UTF-8 before simit can manage badges");
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", readme_path.display())),
    };
    let metadata = cargo::cargo_metadata(&workspace_root.join("Cargo.toml"))
        .with_context(|| format!("loading cargo metadata for {}", workspace_root.display()))?;
    let config = ProjectConfig::load(workspace_root).with_context(|| {
        format!(
            "loading simit project config for {}",
            workspace_root.display()
        )
    })?;
    let features = registry::detect_feature_status(workspace_root);
    let badges = badges(workspace_root, &metadata, &config, &features)?;
    let block = render_block(&badges);
    let upgraded = upsert_block(&current, &block)?;
    Ok(ReadmeUpgrade { current, upgraded })
}

pub fn write(workspace_root: &Path, upgrade: &ReadmeUpgrade) -> Result<()> {
    fs::write(workspace_root.join("README.md"), &upgrade.upgraded)
        .with_context(|| format!("writing {}", workspace_root.join("README.md").display()))
}

fn badges(
    workspace_root: &Path,
    metadata: &Metadata,
    config: &ProjectConfig,
    features: &std::collections::BTreeMap<String, FeatureStatus>,
) -> Result<Vec<Badge>> {
    let package = cargo::representative_package(metadata, None)?;
    let mut badges = Vec::new();

    if has_simit_feature(features, "ci") {
        let target = workflow_link(
            workspace_root,
            &[".forgejo/workflows/ci.yaml", ".github/workflows/ci.yaml"],
        );
        badges.push(static_badge(
            "CI",
            status_word(features, "ci"),
            "2088ff",
            target,
        ));
    }
    if has_simit_feature(features, "flake") {
        badges.push(static_badge(
            "Nix",
            status_word(features, "flake"),
            "5277c3",
            Some("flake.nix".to_owned()),
        ));
    }
    if config.ci.with_docs || workspace_root.join("docs/book.toml").exists() {
        badges.push(static_badge(
            "docs",
            "enabled",
            "6f42c1",
            docs_link(workspace_root, &package),
        ));
    }
    if package.is_publishable() {
        badges.push(static_badge(
            "crates.io",
            "ready",
            "f46623",
            Some(format!("https://crates.io/crates/{}", package.name)),
        ));
    }
    if config.release.codeberg.is_some()
        || workspace_root
            .join(".forgejo/workflows/release.yml")
            .exists()
    {
        badges.push(static_badge(
            "release",
            "configured",
            "2ea44f",
            workflow_link(
                workspace_root,
                &[
                    ".forgejo/workflows/release.yml",
                    ".github/workflows/release.yml",
                ],
            ),
        ));
    }
    if !config.release.artifacts.build_commands.is_empty()
        || !config.release.artifacts.checksum_globs.is_empty()
        || config.release.attic.is_some()
    {
        badges.push(static_badge(
            "artifacts",
            "configured",
            "2ea44f",
            workflow_link(
                workspace_root,
                &[
                    ".forgejo/workflows/release.yml",
                    ".github/workflows/release.yml",
                ],
            ),
        ));
    }
    if config.homebrew.is_some() {
        badges.push(static_badge(
            "Homebrew",
            "configured",
            "2ea44f",
            config
                .homebrew
                .as_ref()
                .map(|homebrew| homebrew.tap_url.clone()),
        ));
    }
    if config.chocolatey.is_some() {
        badges.push(static_badge(
            "Chocolatey",
            "configured",
            "7b3f99",
            Some("https://community.chocolatey.org/".to_owned()),
        ));
    }
    if config.scoop.is_some() {
        badges.push(static_badge(
            "Scoop",
            "configured",
            "2ea44f",
            config.scoop.as_ref().map(|scoop| scoop.bucket_url.clone()),
        ));
    }
    if config.aur.is_some() {
        badges.push(static_badge(
            "AUR",
            "configured",
            "1793d1",
            Some("dist/aur".to_owned()),
        ));
    }
    if config.copr.is_some() {
        badges.push(static_badge(
            "COPR",
            "configured",
            "3f51b5",
            Some(".copr/Makefile".to_owned()),
        ));
    }
    if config.apt.is_some() {
        badges.push(static_badge(
            "apt",
            "configured",
            "a81d33",
            Some("dist/apt/conf/distributions".to_owned()),
        ));
    }
    if config.flatpak.is_some() {
        badges.push(static_badge(
            "Flatpak",
            "configured",
            "4a86cf",
            config
                .flatpak
                .as_ref()
                .map(|flatpak| format!("https://github.com/{}", flatpak.repo)),
        ));
    }
    if config.winget.is_some() {
        badges.push(static_badge(
            "winget",
            "configured",
            "0078d4",
            config.winget.as_ref().map(|winget| {
                format!(
                    "https://github.com/microsoft/winget-pkgs/tree/master/manifests/{}/{}",
                    winget
                        .package_id
                        .chars()
                        .next()
                        .unwrap_or('A')
                        .to_ascii_lowercase(),
                    winget.package_id.replace('.', "/")
                )
            }),
        ));
    }

    Ok(badges)
}

fn has_simit_feature(
    features: &std::collections::BTreeMap<String, FeatureStatus>,
    name: &str,
) -> bool {
    features
        .get(name)
        .is_some_and(|status| !matches!(status, FeatureStatus::Absent | FeatureStatus::HandRolled))
}

fn status_word(
    features: &std::collections::BTreeMap<String, FeatureStatus>,
    name: &str,
) -> &'static str {
    match features.get(name).copied().unwrap_or(FeatureStatus::Absent) {
        FeatureStatus::Managed => "managed",
        FeatureStatus::ManagedExtra => "managed+extra",
        FeatureStatus::Drift => "drift",
        FeatureStatus::HandRolled => "hand-rolled",
        FeatureStatus::Configured => "configured",
        FeatureStatus::Conflicted => "conflicted",
        FeatureStatus::Installed => "installed",
        FeatureStatus::Absent => "absent",
    }
}

fn workflow_link(workspace_root: &Path, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| workspace_root.join(candidate).exists())
        .map(|candidate| (*candidate).to_owned())
}

fn docs_link(workspace_root: &Path, package: &Package) -> Option<String> {
    if workspace_root.join("docs/book.toml").exists() {
        Some("docs".to_owned())
    } else {
        Some(format!("https://docs.rs/{}", package.name))
    }
}

fn static_badge(label: &str, message: &str, color: &str, link: Option<String>) -> Badge {
    Badge {
        label: label.to_owned(),
        url: format!(
            "https://img.shields.io/badge/{}-{}-{}",
            encode_shields_part(label),
            encode_shields_part(message),
            color
        ),
        link,
    }
}

fn encode_shields_part(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            ' ' => "_".chars().collect::<Vec<_>>(),
            '-' => "--".chars().collect::<Vec<_>>(),
            '_' => "__".chars().collect::<Vec<_>>(),
            _ if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+') => vec![ch],
            _ => "_".chars().collect::<Vec<_>>(),
        })
        .collect()
}

fn render_block(badges: &[Badge]) -> String {
    let mut block = String::new();
    block.push_str(START_MARKER);
    block.push('\n');
    if !badges.is_empty() {
        block.push_str(
            &badges
                .iter()
                .map(render_badge)
                .collect::<Vec<_>>()
                .join(" "),
        );
        block.push('\n');
    }
    block.push_str(END_MARKER);
    block.push('\n');
    block
}

fn render_badge(badge: &Badge) -> String {
    let image = format!("![{}]({})", badge.label, badge.url);
    match &badge.link {
        Some(link) => format!("[{image}]({link})"),
        None => image,
    }
}

fn upsert_block(readme: &str, block: &str) -> Result<String> {
    if let Some(start) = readme.find(START_MARKER) {
        let after_start = start + START_MARKER.len();
        let Some(relative_end) = readme[after_start..].find(END_MARKER) else {
            bail!("README.md contains `{START_MARKER}` without `{END_MARKER}`");
        };
        let end = after_start + relative_end + END_MARKER.len();
        let mut replacement_end = end;
        if readme[replacement_end..].starts_with("\r\n") {
            replacement_end += 2;
        } else if readme[replacement_end..].starts_with('\n') {
            replacement_end += 1;
        }
        let mut out = String::new();
        out.push_str(&readme[..start]);
        out.push_str(block);
        out.push_str(&readme[replacement_end..]);
        return Ok(out);
    }

    let Some((heading_end, _)) = top_level_heading_end(readme) else {
        bail!("README.md must contain a top-level `#` heading before simit can insert badges");
    };

    let mut out = String::new();
    out.push_str(&readme[..heading_end]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(block);
    out.push('\n');
    out.push_str(readme[heading_end..].trim_start_matches(['\r', '\n']));
    Ok(out)
}

fn top_level_heading_end(readme: &str) -> Option<(usize, &str)> {
    let mut offset = 0;
    for line in readme.split_inclusive('\n') {
        let line_without_newline = line.trim_end_matches(['\r', '\n']);
        if line_without_newline.starts_with("# ") {
            return Some((offset + line.len(), line_without_newline));
        }
        offset += line.len();
    }
    if readme.starts_with("# ") {
        return Some((readme.len(), readme));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_block_after_h1() {
        let out = upsert_block("# Demo\n\nbody\n", "BLOCK\n").unwrap();
        assert_eq!(out, "# Demo\n\nBLOCK\n\nbody\n");
    }

    #[test]
    fn replaces_existing_block_only() {
        let input =
            "# Demo\n\n<!-- simit:badges:start -->\nold\n<!-- simit:badges:end -->\n\nbody\n";
        let out = upsert_block(input, "NEW\n").unwrap();
        assert_eq!(out, "# Demo\n\nNEW\n\nbody\n");
    }

    #[test]
    fn rejects_missing_h1() {
        let err = upsert_block("body\n", "BLOCK\n").unwrap_err();
        assert!(err.to_string().contains("top-level"));
    }

    #[test]
    fn renders_badges_in_input_order() {
        let rendered = render_block(&[
            static_badge(
                "CI",
                "managed",
                "2088ff",
                Some(".forgejo/workflows/ci.yaml".to_owned()),
            ),
            static_badge("Nix", "managed", "5277c3", Some("flake.nix".to_owned())),
        ]);
        assert!(rendered.contains("[![CI](https://img.shields.io/badge/CI-managed-2088ff)](.forgejo/workflows/ci.yaml) [![Nix](https://img.shields.io/badge/Nix-managed-5277c3)](flake.nix)"));
    }
}
