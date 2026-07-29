//! Shared CI option resolution for generation and drift detection.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::{Platform, Runtime, RuntimeChoice, WorkspaceStrategy};
use crate::config::{CiConfig, ProjectConfig};
use crate::render::ci::{CiOptions, OMNIX_REF_DEFAULT, OmCiMode};

#[derive(Debug, Clone, Default)]
pub struct CiCliOverrides {
    pub runtime: Option<RuntimeChoice>,
    pub runner: Option<String>,
    pub windows_runner: Option<String>,
    pub step_runner: Vec<(String, String)>,
    pub granular: bool,
    pub workspace: bool,
    pub workspace_strategy: Option<WorkspaceStrategy>,
    pub packages: Vec<String>,
    pub with_nextest: Option<bool>,
    pub with_msrv: Option<bool>,
    pub with_audit: Option<bool>,
    pub with_deny: Option<bool>,
    pub with_docs: Option<bool>,
    pub with_artifacts: Option<bool>,
    pub with_pypi_publish: Option<bool>,
    pub publish_crates: Option<bool>,
    pub with_om_ci: Option<bool>,
    pub om_ci_augment: Option<bool>,
    pub omnix_ref: Option<String>,
    pub release_smoke_command: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowSnapshot {
    pub relative_path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct CiInference {
    pub runtime: Option<Runtime>,
    pub runner: Option<String>,
    pub windows_runner: Option<String>,
    pub workspace: Option<bool>,
    pub workspace_strategy: Option<WorkspaceStrategy>,
    pub packages: Option<Vec<String>>,
    pub with_nextest: Option<bool>,
    pub with_msrv: Option<bool>,
    pub with_audit: Option<bool>,
    pub with_deny: Option<bool>,
    pub with_docs: Option<bool>,
    pub with_artifacts: Option<bool>,
    pub with_pypi_publish: Option<bool>,
    pub publish_crates: Option<bool>,
    pub om_ci: Option<OmCiMode>,
    pub omnix_ref: Option<String>,
    pub release_smoke_command: Option<String>,
}

impl CiInference {
    pub fn from_workflows(marked: &[WorkflowSnapshot]) -> Result<Self> {
        if marked.is_empty() {
            return Ok(Self::default());
        }

        let all_content = marked
            .iter()
            .map(|workflow| workflow.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let self_check = marked
            .iter()
            .find(|workflow| workflow.content.contains("cargo run -- init ci"));
        let self_check_workspace = self_check
            .map(|workflow| workflow.content.contains(" --workspace"))
            .unwrap_or(false);
        let self_check_packages = self_check.map_or_else(Vec::new, |workflow| {
            infer_self_check_packages(&workflow.content)
        });
        let suffix_packages = marked
            .iter()
            .filter_map(|workflow| workflow_suffix(&workflow.relative_path))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let (workspace, packages) = if self_check_workspace {
            (Some(true), Some(Vec::new()))
        } else if !self_check_packages.is_empty() {
            (Some(false), Some(self_check_packages))
        } else if !suffix_packages.is_empty() {
            (Some(false), Some(suffix_packages))
        } else {
            (Some(false), Some(Vec::new()))
        };
        let workspace_strategy = all_content
            .contains("--workspace-strategy aggregate")
            .then_some(WorkspaceStrategy::Aggregate)
            .or_else(|| workspace.map(|_| WorkspaceStrategy::Members));

        Ok(Self {
            runtime: Some(infer_ci_runtime(marked)?),
            runner: infer_primary_runner_label(marked, "ci"),
            windows_runner: infer_windows_runner_label(marked),
            workspace,
            workspace_strategy,
            packages,
            // Keep these markers in sync with the emitting sites in
            // src/render/ci.rs: optional tool steps, om-ci, and artifact files.
            with_nextest: Some(all_content.contains("cargo nextest run")),
            with_msrv: Some(all_content.contains("      - name: Check MSRV\n")),
            with_audit: Some(all_content.contains("cargo audit")),
            with_deny: Some(all_content.contains("cargo deny check")),
            with_docs: Some(all_content.contains("cargo doc --no-deps --all-features")),
            with_artifacts: Some(marked.iter().any(|workflow| {
                workflow_name(&workflow.relative_path) == Some("release-artifacts")
            })),
            with_pypi_publish: Some(
                marked
                    .iter()
                    .any(|workflow| workflow_name(&workflow.relative_path) == Some("publish-pypi")),
            ),
            publish_crates: Some(
                marked.iter().any(|workflow| {
                    workflow_name(&workflow.relative_path) == Some("publish-crate")
                }),
            ),
            om_ci: Some(infer_om_ci_mode(&all_content)),
            omnix_ref: infer_omnix_ref(&all_content),
            release_smoke_command: infer_release_smoke_command(&all_content),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedCiInputs {
    pub runtime: Runtime,
    pub runner: Option<String>,
    pub windows_runner: Option<String>,
    pub step_runners: BTreeMap<String, String>,
    pub granular: bool,
    pub workspace: bool,
    pub workspace_strategy: WorkspaceStrategy,
    pub packages: Vec<String>,
    pub with_nextest: bool,
    pub with_msrv: bool,
    pub with_audit: bool,
    pub with_deny: bool,
    pub with_docs: bool,
    pub with_artifacts: bool,
    pub with_pypi_publish: bool,
    pub publish_crates: bool,
    pub om_ci: OmCiMode,
    pub omnix_ref: String,
    pub release_smoke_command: Option<String>,
}

impl ResolvedCiInputs {
    pub fn resolve(
        workspace_root: &Path,
        cfg: &ProjectConfig,
        cli: &CiCliOverrides,
        inference: Option<&CiInference>,
    ) -> Result<Self> {
        let config = CiConfigLayer::load(workspace_root, cfg)?;
        let inference = inference.cloned().unwrap_or_default();
        let runtime = if let Some(choice) = cli.runtime {
            resolve_runtime_choice(choice, workspace_root)?
        } else if let Some(runtime) = config.runtime {
            validate_config_runtime(runtime, workspace_root)?;
            runtime
        } else if let Some(runtime) = inference.runtime {
            runtime
        } else {
            auto_runtime(workspace_root)?
        };

        let replace_om_ci = cli
            .with_om_ci
            .or(config.om_ci)
            .or(inference.om_ci.map(|mode| mode != OmCiMode::Off))
            .unwrap_or(false);
        let augment_om_ci = cli
            .om_ci_augment
            .or(config.om_ci_augment)
            .or(inference.om_ci.map(|mode| mode == OmCiMode::Augment))
            .unwrap_or(false);
        let om_ci = match (replace_om_ci || augment_om_ci, augment_om_ci) {
            (false, _) => OmCiMode::Off,
            (true, true) => OmCiMode::Augment,
            (true, false) => OmCiMode::Replace,
        };

        let (workspace, packages) = if !cli.packages.is_empty() {
            (false, cli.packages.clone())
        } else if cli.workspace || config.workspace == Some(true) {
            (true, Vec::new())
        } else if let Some(packages) = config.packages {
            (false, packages)
        } else if let Some(workspace) = config.workspace {
            (workspace, Vec::new())
        } else if inference.workspace == Some(true) {
            (true, Vec::new())
        } else if let Some(packages) = inference.packages {
            (false, packages)
        } else {
            (inference.workspace.unwrap_or(false), Vec::new())
        };
        let workspace_strategy = cli
            .workspace_strategy
            .or_else(|| {
                (cfg.ci.workspace_strategy != WorkspaceStrategy::Members)
                    .then_some(cfg.ci.workspace_strategy)
            })
            .or(inference.workspace_strategy)
            .unwrap_or_default();
        if workspace_strategy == WorkspaceStrategy::Aggregate
            && (!workspace || !packages.is_empty())
        {
            bail!("aggregate workspace CI requires --workspace and no --package selectors");
        }

        Ok(Self {
            runtime,
            runner: cli.runner.clone().or(config.runner).or(inference.runner),
            granular: cli.granular,
            windows_runner: cli
                .windows_runner
                .clone()
                .or(config.windows_runner)
                .or(inference.windows_runner),
            step_runners: {
                let mut m = config.step_runners.unwrap_or_default();
                for (step, runner) in &cli.step_runner {
                    m.insert(step.clone(), runner.clone());
                }
                m
            },
            workspace,
            workspace_strategy,
            packages,
            with_nextest: cli
                .with_nextest
                .or(config.with_nextest)
                .or(inference.with_nextest)
                .unwrap_or(false),
            with_msrv: cli
                .with_msrv
                .or(config.with_msrv)
                .or(inference.with_msrv)
                .unwrap_or(false),
            with_audit: cli
                .with_audit
                .or(config.with_audit)
                .or(inference.with_audit)
                .unwrap_or(false),
            with_deny: cli
                .with_deny
                .or(config.with_deny)
                .or(inference.with_deny)
                .unwrap_or(false),
            with_docs: cli
                .with_docs
                .or(config.with_docs)
                .or(inference.with_docs)
                .unwrap_or(false),
            with_artifacts: cli
                .with_artifacts
                .or(config.with_artifacts)
                .or(inference.with_artifacts)
                .unwrap_or(false),
            with_pypi_publish: cli
                .with_pypi_publish
                .or(config.with_pypi_publish)
                .or(inference.with_pypi_publish)
                .unwrap_or(false),
            publish_crates: cli
                .publish_crates
                .or(config.publish_crates)
                .or(inference.publish_crates)
                .unwrap_or(false),
            om_ci,
            omnix_ref: cli
                .omnix_ref
                .clone()
                .or(config.omnix_ref)
                .or(inference.omnix_ref)
                .unwrap_or_else(|| OMNIX_REF_DEFAULT.to_owned()),
            release_smoke_command: cli
                .release_smoke_command
                .clone()
                .or_else(|| cfg.release.smoke.command.clone())
                .or(inference.release_smoke_command),
        })
    }

    pub fn ci_options(
        &self,
        cfg: &ProjectConfig,
        with_artifacts: bool,
        omnix_ref: String,
    ) -> CiOptions {
        CiOptions {
            with_nextest: self.with_nextest,
            with_msrv: self.with_msrv,
            with_audit: self.with_audit,
            with_deny: self.with_deny,
            with_docs: self.with_docs,
            with_artifacts,
            with_pypi_publish: self.with_pypi_publish,
            publish_crates: self.publish_crates,
            om_ci: self.om_ci,
            omnix_ref,
            release_smoke_command: self.release_smoke_command.clone(),
            extra_setup: cfg.ci.extra_setup.clone(),
            extra_env: cfg
                .ci
                .extra_env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            required_secrets: cfg.ci.required_secrets.clone(),
            required_env: cfg.ci.required_env.clone(),
            workspace_strategy: self.workspace_strategy,
            package_scoped: false,
            homebrew: None,
            chocolatey: None,
            scoop: None,
        }
    }

    pub fn persisted_ci(
        &self,
        cfg: &ProjectConfig,
        platform: Platform,
        with_artifacts: bool,
        omnix_ref: &str,
        runner: Option<String>,
        windows_runner: Option<String>,
    ) -> CiConfig {
        let mut ci = cfg.ci.clone();
        ci.platform = Some(platform);
        ci.runtime = (self.runtime != Runtime::Cargo).then_some(self.runtime);
        ci.runner = runner;
        ci.windows_runner = windows_runner;
        ci.workspace = self.workspace;
        ci.workspace_strategy = self.workspace_strategy;
        ci.packages = self.packages.clone();
        ci.with_nextest = self.with_nextest;
        ci.with_msrv = self.with_msrv;
        ci.with_audit = self.with_audit;
        ci.with_deny = self.with_deny;
        ci.with_docs = self.with_docs;
        ci.with_artifacts = with_artifacts;
        ci.with_pypi_publish = self.with_pypi_publish;
        ci.publish_crates = self.publish_crates;
        ci.step_runners = self.step_runners.clone();
        ci.om_ci = self.om_ci != OmCiMode::Off;
        ci.om_ci_augment = self.om_ci == OmCiMode::Augment;
        ci.omnix_ref = (self.om_ci != OmCiMode::Off && omnix_ref != OMNIX_REF_DEFAULT)
            .then(|| omnix_ref.to_owned());
        ci
    }
}

#[derive(Debug, Clone, Default)]
struct CiConfigLayer {
    runtime: Option<Runtime>,
    runner: Option<String>,
    windows_runner: Option<String>,
    step_runners: Option<BTreeMap<String, String>>,
    workspace: Option<bool>,
    packages: Option<Vec<String>>,
    with_nextest: Option<bool>,
    with_msrv: Option<bool>,
    with_audit: Option<bool>,
    with_deny: Option<bool>,
    with_docs: Option<bool>,
    with_artifacts: Option<bool>,
    with_pypi_publish: Option<bool>,
    publish_crates: Option<bool>,
    om_ci: Option<bool>,
    om_ci_augment: Option<bool>,
    omnix_ref: Option<String>,
}

impl CiConfigLayer {
    fn load(workspace_root: &Path, cfg: &ProjectConfig) -> Result<Self> {
        let simit_toml = workspace_root.join("simit.toml");
        if simit_toml.exists() {
            return Self::from_simit_toml(&simit_toml, cfg);
        }
        Ok(Self::from_non_simit_config(cfg))
    }

    fn from_simit_toml(path: &Path, cfg: &ProjectConfig) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let document = text
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("parsing {}", path.display()))?;
        let ci_table = document.get("ci").and_then(|item| item.as_table());

        let step_runners = present(ci_table, "step_runners").and_then(|_| {
            let m = &cfg.ci.step_runners;
            if m.is_empty() { None } else { Some(m.clone()) }
        });

        Ok(Self {
            runtime: present(ci_table, "runtime").and(cfg.ci.runtime),
            runner: present(ci_table, "runner").and_then(|_| cfg.ci.runner.clone()),
            windows_runner: present(ci_table, "windows_runner")
                .and_then(|_| cfg.ci.windows_runner.clone()),
            workspace: present(ci_table, "workspace").map(|_| cfg.ci.workspace),
            step_runners,
            packages: present(ci_table, "packages").map(|_| cfg.ci.packages.clone()),
            with_nextest: present(ci_table, "with_nextest").map(|_| cfg.ci.with_nextest),
            with_msrv: present(ci_table, "with_msrv").map(|_| cfg.ci.with_msrv),
            with_audit: present(ci_table, "with_audit").map(|_| cfg.ci.with_audit),
            with_deny: present(ci_table, "with_deny").map(|_| cfg.ci.with_deny),
            with_docs: present(ci_table, "with_docs").map(|_| cfg.ci.with_docs),
            with_artifacts: present(ci_table, "with_artifacts").map(|_| cfg.ci.with_artifacts),
            with_pypi_publish: present(ci_table, "with_pypi_publish")
                .map(|_| cfg.ci.with_pypi_publish),
            publish_crates: present(ci_table, "publish_crates").map(|_| cfg.ci.publish_crates),
            om_ci: present(ci_table, "om_ci").map(|_| cfg.ci.om_ci),
            om_ci_augment: present(ci_table, "om_ci_augment").map(|_| cfg.ci.om_ci_augment),
            omnix_ref: present(ci_table, "omnix_ref").and_then(|_| cfg.ci.omnix_ref.clone()),
        })
    }

    fn from_non_simit_config(cfg: &ProjectConfig) -> Self {
        let step_runners = if cfg.ci.step_runners.is_empty() {
            None
        } else {
            Some(cfg.ci.step_runners.clone())
        };
        Self {
            runtime: cfg.ci.runtime,
            runner: cfg.ci.runner.clone(),
            windows_runner: cfg.ci.windows_runner.clone(),
            step_runners,
            workspace: cfg.ci.workspace.then_some(true),
            packages: (!cfg.ci.packages.is_empty()).then(|| cfg.ci.packages.clone()),
            with_nextest: cfg.ci.with_nextest.then_some(true),
            with_msrv: cfg.ci.with_msrv.then_some(true),
            with_audit: cfg.ci.with_audit.then_some(true),
            with_deny: cfg.ci.with_deny.then_some(true),
            with_docs: cfg.ci.with_docs.then_some(true),
            with_artifacts: cfg.ci.with_artifacts.then_some(true),
            with_pypi_publish: cfg.ci.with_pypi_publish.then_some(true),
            publish_crates: cfg.ci.publish_crates.then_some(true),
            om_ci: cfg.ci.om_ci.then_some(true),
            om_ci_augment: cfg.ci.om_ci_augment.then_some(true),
            omnix_ref: cfg.ci.omnix_ref.clone(),
        }
    }
}

fn present(table: Option<&toml_edit::Table>, key: &str) -> Option<()> {
    table.and_then(|table| table.contains_key(key).then_some(()))
}

fn resolve_runtime_choice(choice: RuntimeChoice, workspace_root: &Path) -> Result<Runtime> {
    match choice {
        RuntimeChoice::Auto => auto_runtime(workspace_root),
        RuntimeChoice::Cargo => Ok(Runtime::Cargo),
        RuntimeChoice::Nix => {
            if !workspace_root.join("flake.nix").exists() {
                bail!("--runtime nix requires flake.nix at the workspace root");
            }
            Ok(Runtime::Nix)
        }
    }
}

fn auto_runtime(workspace_root: &Path) -> Result<Runtime> {
    if meaningful_flake_outputs(workspace_root)? {
        Ok(Runtime::Nix)
    } else {
        Ok(Runtime::Cargo)
    }
}

fn meaningful_flake_outputs(workspace_root: &Path) -> Result<bool> {
    let path = workspace_root.join("flake.nix");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    Ok([
        "packages",
        "apps",
        "checks",
        "devShells",
        "nixosModules",
        "nixosModule",
        "homeModules",
        "homeModule",
        "lib =",
        "lib.",
    ]
    .iter()
    .any(|needle| content.contains(needle)))
}

fn validate_config_runtime(runtime: Runtime, workspace_root: &Path) -> Result<()> {
    if runtime == Runtime::Nix && !workspace_root.join("flake.nix").exists() {
        bail!("[ci].runtime = \"nix\" requires flake.nix at the workspace root");
    }
    Ok(())
}

fn infer_ci_runtime(marked: &[WorkflowSnapshot]) -> Result<Runtime> {
    let Some(ci_workflow) = marked
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some("ci"))
        .or_else(|| {
            marked
                .iter()
                .find(|workflow| workflow_name(&workflow.relative_path) == Some("publish-crate"))
        })
        .or_else(|| {
            marked
                .iter()
                .find(|workflow| workflow_name(&workflow.relative_path) == Some("publish-pypi"))
        })
    else {
        return Ok(Runtime::Cargo);
    };

    if ci_workflow.content.contains("      - name: Install Nix\n")
        || ci_workflow.content.contains("run: nix flake check")
        || ci_workflow.content.contains("run: nix develop -c cargo")
        || ci_workflow
            .content
            .contains("      - name: Build package\n")
    {
        Ok(Runtime::Nix)
    } else {
        Ok(Runtime::Cargo)
    }
}

fn infer_primary_runner_label(marked: &[WorkflowSnapshot], workflow_kind: &str) -> Option<String> {
    let workflow = marked
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some(workflow_kind))
        .or_else(|| {
            (workflow_kind == "publish-crate")
                .then(|| {
                    marked.iter().find(|workflow| {
                        workflow_name(&workflow.relative_path) == Some("release-artifacts")
                    })
                })
                .flatten()
        })?;
    single_runs_on_label(&workflow.content, 0)
}

fn infer_windows_runner_label(marked: &[WorkflowSnapshot]) -> Option<String> {
    let workflow = marked
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some("release-artifacts"))?;
    single_runs_on_label(&workflow.content, 1)
}

fn single_runs_on_label(content: &str, occurrence: usize) -> Option<String> {
    let line = content
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("runs-on: "))
        .nth(occurrence)?
        .trim();
    if line.starts_with('[') || line.contains("${{") {
        return None;
    }
    Some(line.trim_matches('"').to_owned())
}

fn infer_om_ci_mode(content: &str) -> OmCiMode {
    if !content.contains("      - name: Run om ci\n") {
        return OmCiMode::Off;
    }
    if content.contains("run: nix flake check") {
        OmCiMode::Augment
    } else {
        OmCiMode::Replace
    }
}

fn infer_omnix_ref(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("OMNIX_REF: "))
        .map(shell_unquote)
}

fn infer_release_smoke_command(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(
                "VERSION=\"${GITHUB_REF_NAME:-${FORGE_REF_NAME:-${CODEBERG_REF_NAME:-}}}\"",
            )
            .map(|_| ())
    })?;

    content.lines().find_map(|line| {
        let trimmed = line.trim_start();
        trimmed
            .strip_suffix(" \"$VERSION\" release")
            .and_then(|value| value.strip_prefix(""))
            .filter(|value| !value.is_empty())
            .filter(|value| !value.starts_with("if [ -z \"$VERSION\" ]"))
            .filter(|value| !value.starts_with("test -n \"$VERSION\""))
            .filter(|value| !value.starts_with("mkdir -p release"))
            .filter(|value| !value.starts_with("set -euo pipefail"))
            .map(shell_unquote)
    })
}

fn infer_self_check_packages(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|line| line.contains("cargo run -- init ci"))
        .flat_map(|line| {
            let mut packages = Vec::new();
            let mut parts = line.split_whitespace();
            while let Some(part) = parts.next() {
                if part == "--package" {
                    if let Some(package) = parts.next() {
                        packages.push(shell_unquote(package));
                    }
                }
            }
            packages
        })
        .collect()
}

fn workflow_name(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    if stem == "ci" || stem == "build" || stem.starts_with("ci-") || stem.starts_with("build-") {
        Some("ci")
    } else if stem == "publish-crate" || stem.starts_with("publish-crate-") {
        Some("publish-crate")
    } else if stem == "release-artifacts" || stem.starts_with("release-artifacts-") {
        Some("release-artifacts")
    } else if stem == "publish-pypi" {
        Some("publish-pypi")
    } else {
        None
    }
}

fn workflow_suffix(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    stem.strip_prefix("ci-")
        .or_else(|| stem.strip_prefix("publish-crate-"))
        .or_else(|| stem.strip_prefix("release-artifacts-"))
        .or_else(|| stem.strip_prefix("publish-pypi-"))
        .map(str::to_owned)
}

fn shell_unquote(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else {
        value.to_owned()
    }
}
