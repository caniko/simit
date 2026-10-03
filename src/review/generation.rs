//! Opt-in GitHub controller and client workflows; separate from ordinary release CI.
use super::contract::{Request, repository, sha};
use crate::{
    project::{self, GeneratedFile},
    render::ci::GENERATED_WORKFLOW_MARKER,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Controller,
    Client,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub role: Role,
    pub controller: Option<String>,
    pub revision: Option<String>,
    pub request: Option<String>,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        match self.role {
            Role::Controller => ensure!(
                self.controller.is_none() && self.revision.is_none() && self.request.is_none(),
                "review controller identity comes from GitHub OIDC, not project inputs"
            ),
            Role::Client => {
                repository(
                    self.controller
                        .as_deref()
                        .context("review client needs controller")?,
                )?;
                sha(self
                    .revision
                    .as_deref()
                    .context("review client needs exact revision")?)?;
                let request = self
                    .request
                    .as_deref()
                    .context("review client needs request JSON")?;
                ensure!(request.len() <= 65536, "review request too large");
                serde_json::from_str::<Request>(request)?.validate()?;
            }
        }
        Ok(())
    }
}

pub const PATHS: [&str; 4] = [
    ".github/workflows/review-repository.yml",
    ".github/workflows/publish-review.yml",
    ".github/workflows/review-client.yml",
    ".github/actions/setup-nix/action.yml",
];

pub fn is_review_path(path: &Path) -> bool {
    PATHS.iter().any(|p| path == Path::new(p))
}

pub fn files(config: &Config) -> Result<Vec<GeneratedFile>> {
    config.validate()?;
    let header = format!("{GENERATED_WORKFLOW_MARKER}\n");
    Ok(match config.role {
        Role::Controller => [
            (PATHS[0], include_str!("assets/review-repository.yml")),
            (PATHS[1], include_str!("assets/publish-review.yml")),
            (PATHS[3], include_str!("assets/setup-nix.yml")),
        ]
        .into_iter()
        .map(|(p, content)| GeneratedFile {
            relative_path: p.into(),
            content: format!("{header}{content}"),
        })
        .collect(),
        Role::Client => vec![GeneratedFile {
            relative_path: PATHS[2].into(),
            content: format!(
                "{header}name: Repository review client\non:\n  workflow_dispatch:\npermissions:\n  contents: read\n  id-token: write\njobs:\n  review:\n    uses: {}/.github/workflows/review-repository.yml@{}\n    with:\n      request: {}\n",
                config.controller.as_deref().unwrap(),
                config.revision.as_deref().unwrap(),
                serde_json::to_string(config.request.as_deref().unwrap())?
            ),
        }],
    })
}

pub fn run(root: &Path, config: &Config, check: bool, diff: bool) -> Result<()> {
    if config.role == Role::Controller {
        ensure!(
            root.join("flake.nix").is_file(),
            "review controller requires a pinned flake exposing repo-review"
        );
    }
    let files = files(config)?;
    let obsolete: Vec<PathBuf> = PATHS
        .iter()
        .map(PathBuf::from)
        .filter(|path| !files.iter().any(|file| file.relative_path == *path))
        .filter(|path| {
            fs::read_to_string(root.join(path))
                .is_ok_and(|s| s.starts_with(GENERATED_WORKFLOW_MARKER))
        })
        .collect();
    if check {
        project::check_generated_files(
            root,
            &files,
            "Review workflows drift; run `simit init ci --review-only`",
            diff,
        )?;
        ensure!(
            obsolete.is_empty(),
            "obsolete generated review workflows: {obsolete:?}"
        );
    } else {
        project::write_generated_files(root, &files)?;
        project::remove_generated_files(root, &obsolete)?;
    }
    Ok(())
}
