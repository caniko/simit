//! Crow CI workflow rendering.
//!
//! This module deliberately owns only project-side workflow templates. It
//! does not invoke Crow or talk to a Crow server.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cargo::Package;
use crate::cli::{CrowWorkflowFormat, Runtime};
use crate::config::{CrowCiConfig, CrowVariable};
use crate::render::release_workflow::ReleaseWorkflowInputs;
use crate::project::GeneratedFile;
use crate::render::ci::{
    CiOptions, OmCiMode, SelfCheckOptions, STEP_CARGO_CLIPPY, STEP_CARGO_DOC,
    STEP_CARGO_FMT, STEP_CARGO_PACKAGE, STEP_CARGO_TEST, STEP_SELF_CHECK,
};
use crate::user_config::{ResolvedCiRunners, ResolvedRunner};

use super::ci;

const DEFAULT_CROW_NIX_IMAGE: &str = "ghcr.io/cachix/devenv:latest";

pub struct FilesRequest<'a> {
    pub format: CrowWorkflowFormat,
    pub crow: &'a CrowCiConfig,
    pub runtime: Runtime,
    pub package: &'a Package,
    pub file_suffix: Option<&'a str>,
    pub self_check: SelfCheckOptions<'a>,
    pub runners: &'a ResolvedCiRunners,
    pub options: CiOptions,
    pub step_runners: &'a BTreeMap<String, ResolvedRunner>,
}

#[derive(Debug, Serialize)]
struct Workflow {
    name: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    labels: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
    when: Vec<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_clone: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    variables: BTreeMap<String, CrowVariable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<Workspace>,
    steps: Vec<Step>,
}

#[derive(Debug, Serialize)]
struct Workspace {
    base: String,
    path: String,
}

#[derive(Debug, Serialize)]
struct Step {
    name: String,
    image: String,
    commands: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    temp_volumes: Vec<String>,
}

impl Step {
    fn new(name: impl Into<String>, image: &str) -> Self {
        Self {
            name: name.into(),
            image: image.to_owned(),
            commands: Vec::new(),
            environment: BTreeMap::new(),
            depends_on: Vec::new(),
            directory: None,
            entrypoint: None,
            failure: None,
            temp_volumes: Vec::new(),
        }
    }

    fn command(mut self, command: impl Into<String>) -> Self {
        self.commands.push(command.into());
        self
    }

    fn commands(mut self, commands: impl IntoIterator<Item = String>) -> Self {
        self.commands.extend(commands);
        self
    }

    fn secret(mut self, name: &str) -> Self {
        self.environment
            .insert(name.to_owned(), json!({ "from_secret": name }));
        self
    }
}

pub fn files(request: FilesRequest<'_>) -> Result<Vec<GeneratedFile>> {
    if request.options.with_msrv && request.package.rust_version.is_none() {
        bail!("--with-msrv requires package.rust-version in Cargo.toml");
    }

    let image = image_for(&request)?;
    let suffix = request.file_suffix;
    let mut files = vec![GeneratedFile {
        relative_path: crow_path("build", suffix, request.format),
        content: render_workflow(
            build_workflow(
                request.crow,
                request.runtime,
                request.package,
                &image,
                request.self_check,
                request.runners,
                &request.options,
                request.step_runners,
            ),
            request.format,
        )?,
    }];

    if request.options.publish_crates && request.package.is_publishable() {
        files.push(GeneratedFile {
            relative_path: crow_path("publish-crate", suffix, request.format),
            content: render_workflow(
                publish_workflow(
                    request.crow,
                    request.runtime,
                    request.package,
                    &image,
                    &request.options,
                ),
                request.format,
            )?,
        });
    }

    if request.options.with_artifacts {
        files.push(GeneratedFile {
            relative_path: crow_path("release-artifacts", suffix, request.format),
            content: render_workflow(
                artifacts_workflow(
                    request.crow,
                    request.runtime,
                    request.package,
                    &image,
                    request.runners,
                    &request.options,
                ),
                request.format,
            )?,
        });
    }

    Ok(files)
}

#[allow(clippy::too_many_arguments)]
fn build_workflow(
    config: &CrowCiConfig,
    runtime: Runtime,
    package: &Package,
    image: &str,
    self_check: SelfCheckOptions<'_>,
    runners: &ResolvedCiRunners,
    options: &CiOptions,
    step_runners: &BTreeMap<String, ResolvedRunner>,
) -> Workflow {
    let mut steps = Vec::new();
    let prefix = command_prefix(runtime);
    let mut setup = Step::new("project-setup", image);
    setup = setup.commands(options.extra_setup.clone());
    if !options.extra_setup.is_empty() {
        steps.push(setup);
    }

    if runtime == Runtime::Nix {
        steps.push(
            Step::new("nix-config", image)
                .command("nix --version")
                .command("export NIX_CONFIG=\"experimental-features = nix-command flakes\""),
        );
    }

    if runtime == Runtime::Nix && options.om_ci != OmCiMode::Replace {
        steps.push(step("nix-check", image, format!("{prefix}nix flake check")));
    }
    steps.push(step(
        STEP_CARGO_FMT,
        image,
        format!("{prefix}cargo fmt --all -- --check"),
    ));
    steps.push(step(
        STEP_CARGO_TEST,
        image,
        format!("{prefix}cargo test{}", package_selector(package, options)),
    ));
    if options.with_docs {
        steps.push(step(
            STEP_CARGO_DOC,
            image,
            format!("{prefix}cargo doc --no-deps --all-features"),
        ));
    }
    if options.with_nextest {
        steps.push(step(
            "cargo-nextest",
            image,
            format!("{prefix}cargo nextest run --all-features{}", package_selector(package, options)),
        ));
    }
    if options.with_audit {
        steps.push(step(
            "cargo-audit",
            image,
            format!("{prefix}cargo audit --no-fetch --stale"),
        ));
    }
    if options.with_deny {
        steps.push(step(
            "cargo-deny",
            image,
            format!("{prefix}cargo deny check bans licenses sources"),
        ));
    }
    if options.om_ci != OmCiMode::Off {
        steps.push(
            Step::new("om-ci", image)
                .command(format!("nix run \"{}\" -- ci run", options.omnix_ref)),
        );
    }
    steps.push(step(
        STEP_CARGO_CLIPPY,
        image,
        format!("{prefix}cargo clippy --all-targets -- --deny warnings"),
    ));
    if package.is_publishable() {
        steps.push(step(
            STEP_CARGO_PACKAGE,
            image,
            format!("{prefix}cargo package --allow-dirty --list{}", package_selector(package, options)),
        ));
    }
    if self_check.enabled {
        let mut command = String::from("cargo run -- init ci --ci-provider crow");
        if self_check.workspace {
            command.push_str(" --workspace");
        }
        for package in self_check.packages {
            command.push_str(" --package ");
            command.push_str(package);
        }
        command.push_str(" --check");
        steps.push(step(STEP_SELF_CHECK, image, command));
    }

    apply_step_runner_labels(&mut steps, step_runners);
    add_environment(&mut steps, options);
    if runtime == Runtime::Nix {
        add_nix_environment(&mut steps);
    }
    for secret in &options.required_secrets {
        for step in &mut steps {
            step.environment
                .insert(secret.clone(), json!({ "from_secret": secret }));
        }
    }

    Workflow {
        name: "build".to_owned(),
        labels: labels(config, &runners.ci),
        platform: config.platform.clone(),
        when: vec![condition("event", "push"), condition("event", "pull_request")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: config.workspace_base.as_ref().map(|base| Workspace {
            base: base.clone(),
            path: "src/${CI_REPO}".to_owned(),
        }),
        steps,
    }
}

fn publish_workflow(
    config: &CrowCiConfig,
    runtime: Runtime,
    package: &Package,
    image: &str,
    options: &CiOptions,
) -> Workflow {
    let prefix = command_prefix(runtime);
    let mut publish = Step::new("publish", image)
        .command(format!("{prefix}cargo publish --dry-run{}", package_selector(package, options)))
        .command(format!("{prefix}cargo publish{}", package_selector(package, options)));
    for secret in &options.required_secrets {
        publish = publish.secret(secret);
    }
    Workflow {
        name: "publish-crate".to_owned(),
        labels: labels(config, &ResolvedRunner::literal("crow-default").expect("literal label")),
        platform: config.platform.clone(),
        when: vec![condition("event", "manual"), condition("event", "tag")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![publish],
    }
}

fn artifacts_workflow(
    config: &CrowCiConfig,
    runtime: Runtime,
    package: &Package,
    image: &str,
    runners: &ResolvedCiRunners,
    options: &CiOptions,
) -> Workflow {
    let prefix = command_prefix(runtime);
    let build = if runtime == Runtime::Nix {
        format!("{prefix}nix build")
    } else {
        format!("{prefix}cargo build --release --locked{}", package_selector(package, options))
    };
    let mut step = Step::new("build-release-artifacts", image)
        .command("test -n \"$${CI_COMMIT_TAG:-}\"")
        .command(build);
    for secret in &options.required_secrets {
        step = step.secret(secret);
    }
    Workflow {
        name: "release-artifacts".to_owned(),
        labels: labels(config, &runners.release),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![step],
    }
}

fn image_for(request: &FilesRequest<'_>) -> Result<String> {
    match request.runtime {
        Runtime::Cargo => Ok(request
            .crow
            .image
            .clone()
            .unwrap_or_else(|| rust_container(request.package))),
        Runtime::Nix => Ok(request
            .crow
            .nix_image
            .clone()
            .or_else(|| request.crow.image.clone())
            .unwrap_or_else(|| DEFAULT_CROW_NIX_IMAGE.to_owned())),
    }
}

fn rust_container(package: &Package) -> String {
    let Some(version) = package.rust_version.as_deref() else {
        return "rust:bookworm".to_owned();
    };
    format!("rust:{version}-bookworm")
}

fn command_prefix(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Cargo => "",
        Runtime::Nix => "nix develop -c ",
    }
}

fn package_selector(package: &Package, options: &CiOptions) -> String {
    if options.package_scoped {
        format!(" -p {}", package.name)
    } else {
        String::new()
    }
}

fn step(name: impl Into<String>, image: &str, command: String) -> Step {
    Step::new(name, image).command(command)
}

fn condition(key: &str, value: &str) -> BTreeMap<String, Value> {
    let mut condition = BTreeMap::new();
    condition.insert(key.to_owned(), Value::String(value.to_owned()));
    condition
}

fn conditions(entries: &[(&str, &str)]) -> BTreeMap<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

fn labels(config: &CrowCiConfig, runner: &ResolvedRunner) -> BTreeMap<String, String> {
    let mut labels = config.labels.clone();
    if labels.is_empty() {
        labels.insert("platform".to_owned(), "linux/amd64".to_owned());
    }
    for label in &runner.labels {
        if let Some((key, value)) = label.split_once('=') {
            labels.insert(key.to_owned(), value.to_owned());
        } else if !labels.contains_key("agent") && label != "crow-default" {
            labels.insert("agent".to_owned(), label.to_owned());
        }
    }
    labels
}

fn add_environment(steps: &mut [Step], options: &CiOptions) {
    for step in steps {
        for (key, value) in &options.extra_env {
            step.environment
                .insert(key.clone(), Value::String(value.clone()));
        }
        step.environment
            .entry("CARGO_HOME".to_owned())
            .or_insert_with(|| Value::String("/tmp/.cargo".to_owned()));
    }
}

fn add_nix_environment(steps: &mut [Step]) {
    for step in steps {
        step.environment.insert(
            "NIX_CONFIG".to_owned(),
            Value::String("experimental-features = nix-command flakes".to_owned()),
        );
    }
}

fn apply_step_runner_labels(steps: &mut [Step], runners: &BTreeMap<String, ResolvedRunner>) {
    for step in steps {
        let Some(runner) = runners.get(&step.name) else {
            continue;
        };
        for label in &runner.labels {
            if let Some((key, value)) = label.split_once('=') {
                step.environment.insert(
                    format!("SIMIT_RUNNER_{}", key.to_ascii_uppercase()),
                    Value::String(value.to_owned()),
                );
            }
        }
    }
}

fn crow_path(stem: &str, suffix: Option<&str>, format: CrowWorkflowFormat) -> PathBuf {
    let name = match suffix {
        Some(suffix) => format!("{stem}-{suffix}"),
        None => stem.to_owned(),
    };
    let extension = match format {
        CrowWorkflowFormat::Yaml => "yaml",
        CrowWorkflowFormat::Jsonnet => "jsonnet",
    };
    PathBuf::from(".crow").join(format!("{name}.{extension}"))
}

fn render_workflow(workflow: Workflow, format: CrowWorkflowFormat) -> Result<String> {
    let yaml = serde_yaml::to_string(&workflow).context("serializing Crow workflow")?;
    match format {
        CrowWorkflowFormat::Yaml => Ok(format!("{}\n{}", ci::GENERATED_WORKFLOW_MARKER, yaml)),
        CrowWorkflowFormat::Jsonnet => {
            let value: Value = serde_yaml::from_str(&yaml).context("converting Crow workflow to Jsonnet")?;
            Ok(format!(
                "// {}\n{}\n",
                ci::GENERATED_WORKFLOW_MARKER.trim_start_matches('#').trim(),
                serde_json::to_string_pretty(&value).context("serializing Crow Jsonnet")?
            ))
        }
    }
}

pub fn codeberg_pages_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    pages: &ci::CodebergPagesOptions,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut deploy = Step::new("deploy-pages", &image)
        .command(format!("nix build .#{} --no-link", pages.site_output))
        .command("test -d result".to_owned())
        .command(format!("nix run .#{}", pages.deploy_app))
        .secret(&pages.token_secret);
    if let Some(domain) = &pages.canonical_domain {
        deploy = deploy.command(format!(
            "test \"$(cat result/.domains 2>/dev/null || true)\" = {}",
            shell_quote(domain)
        ));
    }
    let workflow = Workflow {
        name: "pages".to_owned(),
        labels: labels(config, runner),
        platform: config.platform.clone(),
        when: vec![conditions(&[("event", "push"), ("branch", &pages.source_branch)])],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![deploy],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("pages", None, format),
        content: render_workflow(workflow, format)?,
    })
}

pub fn vscode_extension_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    vscode: &crate::config::ResolvedVscode,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut publish = Step::new("publish-vscode-extension", &image)
        .commands(vscode.prepublish_commands.clone())
        .command(vscode.package_command.clone())
        .secret(&vscode.vsce_pat_secret)
        .secret(&vscode.ovsx_pat_secret);
    if let Some(package) = &vscode.cargo_package {
        publish = publish.command(format!("cargo build -p {package}"));
    }
    let workflow = Workflow {
        name: "publish-vscode-extension".to_owned(),
        labels: labels(config, runner),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![publish],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("publish-vscode-extension", None, format),
        content: render_workflow(workflow, format)?,
    })
}

pub fn jetbrains_plugin_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    jetbrains: &crate::config::ResolvedJetbrains,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut publish = Step::new("publish-jetbrains-plugin", &image)
        .commands(jetbrains.prepublish_commands.clone())
        .command(format!("test -f {}", shell_quote(&jetbrains.package_installable)))
        .secret(&jetbrains.marketplace_token_secret)
        .secret(&jetbrains.certificate_chain_secret)
        .secret(&jetbrains.private_key_secret)
        .secret(&jetbrains.private_key_password_secret);
    if let Some(package) = &jetbrains.cargo_package {
        publish = publish.command(format!("cargo build -p {package}"));
    }
    let workflow = Workflow {
        name: "publish-jetbrains-plugin".to_owned(),
        labels: labels(config, runner),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![publish],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("publish-jetbrains-plugin", None, format),
        content: render_workflow(workflow, format)?,
    })
}

pub fn python_ci_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    options: &CiOptions,
    check_outputs: &[String],
    components: &[crate::config::CiComponent],
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let selected = |component| components.is_empty() || components.contains(&component);
    let mut steps = Vec::new();
    if !options.extra_setup.is_empty() {
        steps.push(Step::new("project-setup", &image).commands(options.extra_setup.clone()));
    }
    if selected(crate::config::CiComponent::FlakeWiring) {
        steps.push(step(
            "flake-wiring",
            &image,
            "nix run --no-write-lock-file git+https://codeberg.org/caniko/simit.git -- init flake --check --diff".to_owned(),
        ));
    }
    if selected(crate::config::CiComponent::FlakeEvaluation) {
        steps.push(step("flake-check", &image, "nix flake check --no-build".to_owned()));
    }
    if selected(crate::config::CiComponent::Checks) {
        let checks = if check_outputs.is_empty() {
            vec!["offline-tests".to_owned(), "typecheck".to_owned(), "uv-format".to_owned()]
        } else {
            check_outputs.to_vec()
        };
        for check in checks {
            steps.push(step(
                format!("check-{check}"),
                &image,
                format!("nix build .#checks.x86_64-linux.{check}"),
            ));
        }
    }
    apply_common_options(&mut steps, options);
    add_nix_environment(&mut steps);
    let workflow = Workflow {
        name: "ci".to_owned(),
        labels: labels(config, runner),
        platform: config.platform.clone(),
        when: vec![condition("event", "push"), condition("event", "pull_request")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: config.workspace_base.as_ref().map(|base| Workspace {
            base: base.clone(),
            path: "src/${CI_REPO}".to_owned(),
        }),
        steps,
    };
    Ok(GeneratedFile {
        relative_path: crow_path("ci", None, format),
        content: render_workflow(workflow, format)?,
    })
}

pub fn python_publish_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    options: &CiOptions,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut publish = Step::new("publish-pypi", &image)
        .command("nix develop -c uv build".to_owned())
        .command("nix develop -c uv publish".to_owned())
        .secret("PYPI_TOKEN");
    apply_common_options(std::slice::from_mut(&mut publish), options);
    add_nix_environment(std::slice::from_mut(&mut publish));
    let workflow = Workflow {
        name: "publish-pypi".to_owned(),
        labels: labels(config, runner),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![publish],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("publish-pypi", None, format),
        content: render_workflow(workflow, format)?,
    })
}

pub fn maturin_publish_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    options: &CiOptions,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut publish = Step::new("publish-pypi", &image)
        .command("nix develop -c maturin build --release --sdist".to_owned())
        .command("nix develop -c maturin publish --skip-existing".to_owned())
        .secret("PYPI_TOKEN");
    apply_common_options(std::slice::from_mut(&mut publish), options);
    add_nix_environment(std::slice::from_mut(&mut publish));
    let workflow = Workflow {
        name: "publish-pypi".to_owned(),
        labels: labels(config, runner),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![publish],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("publish-pypi", None, format),
        content: render_workflow(workflow, format)?,
    })
}

fn nix_image(config: &CrowCiConfig) -> Result<String> {
    Ok(config
        .nix_image
        .clone()
        .or_else(|| config.image.clone())
        .unwrap_or_else(|| DEFAULT_CROW_NIX_IMAGE.to_owned()))
}

fn apply_common_options(steps: &mut [Step], options: &CiOptions) {
    for step in steps {
        for (key, value) in &options.extra_env {
            step.environment
                .insert(key.clone(), Value::String(value.clone()));
        }
        for secret in &options.required_secrets {
            step.environment
                .insert(secret.clone(), json!({ "from_secret": secret }));
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn release_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    inputs: &ReleaseWorkflowInputs<'_>,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut build = Step::new("release", &image)
        .command("test -n \"$${CI_COMMIT_TAG:-}\"".to_owned())
        .command("test \"$${CI_COMMIT_TAG}\" = \"$${CI_COMMIT_TAG#v}\" || true".to_owned())
        .command("mkdir -p release".to_owned());
    if let Some(command) = &inputs.artifacts.supply_chain_command {
        build = build.command(command.clone());
    }
    for command in &inputs.artifacts.sbom_commands {
        build = build.command(command.clone());
    }
    for command in &inputs.artifacts.build_commands {
        build = build.command(command.clone());
    }
    for attr in inputs.artifacts.effective_nix_bundle_attrs() {
        build = build.command(format!("nix build .#{} --out-link release/{}", attr, attr));
    }
    if !inputs.artifacts.checksum_globs.is_empty() {
        build = build.command(format!(
            "(cd release && sha256sum {}) > release/SHA256SUMS.txt",
            inputs.artifacts.checksum_globs.join(" ")
        ));
    }
    if inputs.artifacts.sign {
        build = build.command(
            "minisign -Sm release/SHA256SUMS.txt -s \"$${MINISIGN_SECRET_KEY}\" -t \"$${CI_COMMIT_TAG}\""
                .to_owned(),
        );
        build = build.secret("MINISIGN_SECRET_KEY");
    }
    if let Some(release) = inputs.release {
        build = build
            .command(format!(
                "curl --fail --silent --show-error -X POST -H 'Authorization: token $${{{}}}' -H 'Content-Type: application/json' --data '{{\"tag_name\":\"$${{CI_COMMIT_TAG}}\",\"target_commitish\":\"{}\"}}' {}/repos/{}/releases",
                release.token_secret, release.target_branch, release.api_base, release.repo
            ))
            .secret(&release.token_secret);
    }
    add_nix_environment(std::slice::from_mut(&mut build));
    let workflow = Workflow {
        name: "release".to_owned(),
        labels: labels(config, &ResolvedRunner { name: None, labels: vec![inputs.runner.to_owned()] }),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag"), condition("event", "manual")],
        skip_clone: config.skip_clone.then_some(true),
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![build],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("release", None, format),
        content: render_workflow(workflow, format)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_documented_default_image_for_nix_workflows() {
        assert_eq!(nix_image(&CrowCiConfig::default()).unwrap(), DEFAULT_CROW_NIX_IMAGE);
    }

    #[test]
    fn renders_native_secret_and_jsonnet_markers() {
        let workflow = Workflow {
            name: "test".to_owned(),
            labels: BTreeMap::from([(String::from("agent"), String::from("crow"))]),
            platform: Some("linux/amd64".to_owned()),
            when: vec![condition("event", "push")],
            skip_clone: None,
            variables: BTreeMap::new(),
            workspace: None,
            steps: vec![Step::new("publish", "rust:bookworm")
                .secret("TOKEN")
                .command("echo $${TOKEN}".to_owned())],
        };
        let yaml = render_workflow(workflow, CrowWorkflowFormat::Yaml).unwrap();
        assert!(yaml.starts_with(ci::GENERATED_WORKFLOW_MARKER));
        assert!(yaml.contains("from_secret: TOKEN"));
        assert!(yaml.contains("$${TOKEN}"));

        let jsonnet = render_workflow(
            Workflow {
                name: "test".to_owned(),
                labels: BTreeMap::new(),
                platform: None,
                when: Vec::new(),
                skip_clone: None,
                variables: BTreeMap::new(),
                workspace: None,
                steps: Vec::new(),
            },
            CrowWorkflowFormat::Jsonnet,
        )
        .unwrap();
        assert!(jsonnet.starts_with("// Generated by simit."));
    }
}
