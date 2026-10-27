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
use crate::config::{AtticConfig, CrowCiConfig, CrowVariable, PrebuildConfig};
use crate::project::GeneratedFile;
use crate::render::ci::{
    CiOptions, OmCiMode, STEP_CARGO_CLIPPY, STEP_CARGO_DOC, STEP_CARGO_FMT, STEP_CARGO_PACKAGE,
    STEP_CARGO_TEST, STEP_SELF_CHECK, SelfCheckOptions,
};
use crate::render::release_workflow::ReleaseWorkflowInputs;
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    when: Vec<BTreeMap<String, Value>>,
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
            when: Vec::new(),
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
    files_for_scope(request, false)
}

pub(crate) fn primary_files(request: FilesRequest<'_>) -> Result<Vec<GeneratedFile>> {
    files_for_scope(request, true)
}

fn files_for_scope(request: FilesRequest<'_>, primary_only: bool) -> Result<Vec<GeneratedFile>> {
    if request.options.independent_lanes {
        bail!("independent_lanes requires GitHub Actions Rust/Nix CI");
    }
    if request.options.with_nix_cargo_cache {
        bail!("with_nix_cargo_cache requires the Actions provider");
    }
    if request.options.with_msrv && request.package.rust_version.is_none() {
        bail!("--with-msrv requires package.rust-version in Cargo.toml");
    }

    let image = image_for(&request)?;
    let suffix = request.file_suffix;
    let mut files = if !request.options.repository_checks
        && (request.options.check_command.is_some()
            || request.crow.check_command.is_some()
            || (request.options.om_ci == OmCiMode::Replace
                && !request.options.with_docs
                && !request.options.with_msrv))
    {
        Vec::new()
    } else {
        vec![GeneratedFile {
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
        }]
    };

    if !primary_only && request.options.publish_crates && request.package.is_publishable() {
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

    if !primary_only && request.options.with_artifacts {
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

    if runtime == Runtime::Nix
        && options.repository_checks
        && options
            .components
            .contains(&crate::config::CiComponent::FlakeWiring)
    {
        steps.push(step("flake-wiring", image, super::flake_wiring_check()));
    }

    if let Some(command) = &config.check_command {
        if let Some(secret) = &config.check_push_secret {
            let mut push = step("project-check-push", image, command.clone()).secret(secret);
            push.when.push(condition("event", "push"));
            steps.push(push);
            let mut pr = step("project-check-pr", image, command.clone());
            pr.when.push(condition("event", "pull_request"));
            steps.push(pr);
        } else if let Some(secret) = &config.check_secret {
            steps.push(step("project-check", image, command.clone()).secret(secret));
        } else {
            steps.push(step("project-check", image, command.clone()));
        }
    } else if let Some(command) = options.check_command.as_deref() {
        if runtime == Runtime::Nix
            && let Some(command) = options.flake_check_command()
        {
            steps.push(step("nix-check", image, command.to_owned()));
        }
        let command = if runtime == Runtime::Cargo {
            format!("rustup component add clippy rustfmt && {command}")
        } else {
            crate::render::ci::project_check_command(runtime, command)
        };
        steps.push(step(crate::render::ci::STEP_PROJECT_CHECK, image, command));
    } else {
        if runtime == Runtime::Nix
            && options.om_ci != OmCiMode::Replace
            && let Some(command) = options.flake_check_command()
        {
            steps.push(step("nix-check", image, command.to_owned()));
        }
        if options.repository_checks {
            steps.push(step(
                if runtime == Runtime::Nix {
                    crate::render::ci::STEP_FORMAT
                } else {
                    STEP_CARGO_FMT
                },
                image,
                if runtime == Runtime::Nix {
                    // Crow preprocesses dollars before invoking the shell. Escape
                    // this generated shell/Nix command, including \${...}, so its
                    // runtime expressions survive that separate expansion layer.
                    format!(
                        "{prefix}{}",
                        crate::render::ci::NIX_FORMAT_COMMAND.replace('$', "$$")
                    )
                } else {
                    format!("{prefix}cargo fmt --all -- --check")
                },
            ));
        }
        if options.om_ci != OmCiMode::Replace {
            if options.with_nextest {
                let command = if runtime == Runtime::Cargo {
                    format!(
                        "command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked --version {}",
                        ci::CARGO_NEXTEST_VERSION
                    )
                } else {
                    format!("{prefix}cargo-nextest --version")
                };
                steps.push(step("nextest-tool", image, command));
            }
            let selector = package_selector(package, options);
            let mut modes = vec![if options.all_features {
                " --all-features"
            } else {
                ""
            }];
            if !package.features.is_empty() {
                modes.push(" --no-default-features");
            }
            for (index, features) in modes.into_iter().enumerate() {
                let harness = if options.with_nextest {
                    "cargo nextest run"
                } else {
                    "cargo test"
                };
                let name = if options.with_nextest {
                    "cargo-nextest"
                } else {
                    STEP_CARGO_TEST
                };
                let name = if index == 0 {
                    name.to_owned()
                } else {
                    format!("{name}-no-default-features")
                };
                let targets = ci::unit_test_target_args(package, options);
                steps.push(step(
                    &name,
                    image,
                    format!("{prefix}{harness}{features}{selector}{targets}"),
                ));
                if options.with_nextest && !options.unit_tests_only && package.has_doctests() {
                    steps.push(step(
                        format!("{name}-doctests"),
                        image,
                        format!("{prefix}cargo test --doc{features}{selector}"),
                    ));
                }
                let name = if index == 0 {
                    STEP_CARGO_CLIPPY.to_owned()
                } else {
                    format!("{STEP_CARGO_CLIPPY}-no-default-features")
                };
                steps.push(step(
                    name,
                    image,
                    format!(
                        "{prefix}cargo clippy --all-targets{features}{selector} -- --deny warnings"
                    ),
                ));
            }
        }
        if options.with_docs {
            steps.push(step(
                STEP_CARGO_DOC,
                image,
                format!(
                    "{prefix}cargo doc{} --no-deps --all-features",
                    package_selector(package, options)
                ),
            ));
        }
        if options.with_msrv {
            steps.push(step(
                "cargo-msrv",
                image,
                ci::msrv_check_command(runtime, package, options).replace('$', "$$"),
            ));
        }
        if options.repository_checks && options.with_audit {
            steps.push(step(
                "cargo-audit",
                image,
                format!("{prefix}cargo audit --no-fetch --stale"),
            ));
        }
        if options.repository_checks && options.with_deny {
            steps.push(step(
                "cargo-deny",
                image,
                format!("{prefix}cargo deny check bans licenses sources"),
            ));
        }
        if options.repository_checks && options.om_ci != OmCiMode::Off {
            steps.push(
                Step::new("om-ci", image)
                    .command(format!("nix run \"{}\" -- ci run", options.omnix_ref)),
            );
        }
        if options.om_ci != OmCiMode::Replace && package.is_publishable() {
            steps.push(step(
                STEP_CARGO_PACKAGE,
                image,
                format!(
                    "{prefix}cargo package --allow-dirty --list{}",
                    package_selector(package, options)
                ),
            ));
        }
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
        if self_check.release {
            steps.push(step(
                "release-self-check",
                image,
                "cargo run -- init release --ci-provider crow --check --diff".to_owned(),
            ));
        }
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
        when: vec![
            push_to_maintained_branches(),
            condition("event", "pull_request"),
        ],
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
        .command(format!(
            "{prefix}cargo publish --dry-run{}",
            package_selector(package, options)
        ))
        .command(format!(
            "{prefix}cargo publish{}",
            package_selector(package, options)
        ));
    for secret in &options.required_secrets {
        publish = publish.secret(secret);
    }
    Workflow {
        name: "publish-crate".to_owned(),
        labels: labels(
            config,
            &ResolvedRunner::literal("crow-default").expect("literal label"),
        ),
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
        format!(
            "{prefix}cargo build --release --locked{}",
            package_selector(package, options)
        )
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

/// Push pipelines run only for the default branch and the fleet updater's
/// maintenance refs; pull request pipelines gate every other branch, so a
/// branch push would only duplicate them.
// ponytail: matches the default `rollout.maintenanceRefPrefix`; a profile with
// another prefix gets PR pipelines only on its maintenance refs.
fn push_to_maintained_branches() -> BTreeMap<String, Value> {
    let mut when = condition("event", "push");
    let maintenance = format!(
        "{}**",
        crate::commands::projects_update::schema::maintenance_prefix()
    );
    when.insert(
        "branch".to_owned(),
        json!(["${CI_REPO_DEFAULT_BRANCH}", maintenance]),
    );
    when
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
        let Some(runner) = crate::render::ci::step_runner_override(runners, &step.name) else {
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
            let value: Value =
                serde_yaml::from_str(&yaml).context("converting Crow workflow to Jsonnet")?;
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
    let mut deploy = Step::new("deploy-pages", &image);
    for setup in &pages.extra_setup {
        deploy = deploy.command(setup.clone());
    }
    deploy = deploy
        .command(format!(
            "nix build .#{} --out-link result",
            pages.site_output
        ))
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
        when: vec![conditions(&[
            ("event", "push"),
            ("branch", &pages.source_branch),
        ])],
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
        .command(format!(
            "test -f {}",
            shell_quote(&jetbrains.package_installable)
        ))
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
    if options.independent_lanes {
        bail!("independent_lanes requires GitHub Actions Rust/Nix CI");
    }
    if options.check_command.is_some() || options.with_nix_cargo_cache || !options.nix_flake_check {
        bail!(
            "check_command, with_nix_cargo_cache and nix_flake_check=false require Rust language CI"
        );
    }
    let image = nix_image(config)?;
    let selected = |component| components.is_empty() || components.contains(&component);
    let mut steps = Vec::new();
    if !options.extra_setup.is_empty() {
        steps.push(Step::new("project-setup", &image).commands(options.extra_setup.clone()));
    }
    if selected(crate::config::CiComponent::FlakeWiring) {
        steps.push(step("flake-wiring", &image, super::flake_wiring_check()));
    }
    if selected(crate::config::CiComponent::FlakeEvaluation) {
        steps.push(step(
            "flake-check",
            &image,
            "nix flake check --no-build".to_owned(),
        ));
    }
    if selected(crate::config::CiComponent::Checks) {
        let checks = if check_outputs.is_empty() {
            vec![
                "offline-tests".to_owned(),
                "typecheck".to_owned(),
                "formatting".to_owned(),
            ]
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
        when: vec![
            push_to_maintained_branches(),
            condition("event", "pull_request"),
        ],
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
    pypi_publish_file(
        format,
        config,
        runner,
        options,
        &[
            "nix develop -c uv build".to_owned(),
            "nix develop -c uv publish".to_owned(),
        ],
    )
}

pub fn maturin_publish_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    options: &CiOptions,
) -> Result<GeneratedFile> {
    pypi_publish_file(
        format,
        config,
        runner,
        options,
        &[
            "nix develop -c maturin build --release --sdist".to_owned(),
            "nix develop -c maturin publish --skip-existing".to_owned(),
        ],
    )
}

fn pypi_publish_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    runner: &ResolvedRunner,
    options: &CiOptions,
    build_commands: &[String],
) -> Result<GeneratedFile> {
    if options.pypi_trusted_publishing {
        bail!("PyPI trusted publishing requires GitHub Actions");
    }
    let image = nix_image(config)?;
    let mut publish = Step::new("publish-pypi", &image);
    for command in build_commands {
        publish = publish.command(command.clone());
    }
    let mut publish = publish.secret(&options.pypi_token_secret);
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

/// Default-branch push pipeline that realizes `nix_builds` and, with
/// `publish_attic`, runs the project's Attic push app with the token secret.
pub fn prebuild_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    nix_builds: &[String],
    prebuild: &PrebuildConfig,
    attic: Option<&AtticConfig>,
) -> Result<GeneratedFile> {
    let image = nix_image(config)?;
    let mut step = Step::new("prebuild", &image).command("mkdir -p .simit-prebuild");
    for (index, installable) in nix_builds.iter().enumerate() {
        step = step.command(format!(
            "nix build {} --out-link '.simit-prebuild/ci-{index}' --print-build-logs",
            shell_quote(installable)
        ));
    }
    if prebuild.publish_attic {
        let token_secret = attic
            .and_then(|attic| attic.token_secret.as_deref())
            .context("Crow prebuild Attic publication requires [release.attic].token_secret")?;
        let app = prebuild
            .attic_app
            .as_deref()
            .context("Crow prebuild Attic publication requires [prebuild].attic_app")?;
        // harbor-rs `mkAtticPush` reads ATTIC_TOKEN, whatever the Crow secret is named.
        step.environment.insert(
            "ATTIC_TOKEN".to_owned(),
            json!({ "from_secret": token_secret }),
        );
        step = step
            .command("test -n \"$${ATTIC_TOKEN:-}\"")
            .command(format!("nix run {}", shell_quote(app)));
    }
    add_nix_environment(std::slice::from_mut(&mut step));
    let workflow = Workflow {
        name: "prebuild".to_owned(),
        labels: labels(
            config,
            &ResolvedRunner {
                name: None,
                labels: Vec::new(),
            },
        ),
        platform: config.platform.clone(),
        when: vec![conditions(&[
            ("event", "push"),
            ("branch", "${CI_REPO_DEFAULT_BRANCH}"),
        ])],
        // The prebuild builds the checked-out flake, so it never skips the clone.
        skip_clone: None,
        variables: config.variables.clone(),
        workspace: None,
        steps: vec![step],
    };
    Ok(GeneratedFile {
        relative_path: crow_path("prebuild", None, format),
        content: render_workflow(workflow, format)?,
    })
}

/// Secret-free workflows that qualify the flake, `[prebuild].qualify` and the
/// prebuild installables on pull requests and default-branch pushes. Each
/// command is its own workflow, `.crow/prebuild-qualify-<name>.*`, so every one
/// gets the repository timeout and they run in parallel.
pub fn qualify_files(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    prebuild: &PrebuildConfig,
) -> Result<Vec<GeneratedFile>> {
    let image = nix_image(config)?;
    let installables = prebuild
        .qualify
        .iter()
        .chain(&prebuild.nix_builds)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut jobs = vec![(
        "flake-check".to_owned(),
        "nix flake check --no-build".to_owned(),
    )];
    for (installable, job) in installables.iter().zip(qualify_job_names(&installables)) {
        jobs.push((
            job,
            format!(
                "nix build {} --no-link --print-build-logs",
                shell_quote(installable)
            ),
        ));
    }
    let mut files = Vec::new();
    for (job, command) in jobs {
        let mut step = Step::new(job.clone(), &image).command(command);
        add_nix_environment(std::slice::from_mut(&mut step));
        let workflow = Workflow {
            name: format!("qualify-{job}"),
            labels: labels(
                config,
                &ResolvedRunner {
                    name: None,
                    labels: Vec::new(),
                },
            ),
            platform: config.platform.clone(),
            when: vec![
                condition("event", "pull_request"),
                conditions(&[("event", "push"), ("branch", "${CI_REPO_DEFAULT_BRANCH}")]),
            ],
            skip_clone: None,
            variables: config.variables.clone(),
            workspace: None,
            steps: vec![step],
        };
        files.push(GeneratedFile {
            relative_path: crow_path(&format!("prebuild-qualify-{job}"), None, format),
            content: render_workflow(workflow, format)?,
        });
    }
    Ok(files)
}

/// Workflow names for the installables: the final attribute, e.g.
/// `.#checks.x86_64-linux.clippy` -> `clippy`, or the whole attribute path when
/// that is ambiguous, then a numeric suffix for repeated installables.
fn qualify_job_names(installables: &[&str]) -> Vec<String> {
    let slug = |text: &str| {
        let name = text
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let name = name.trim_matches('-');
        if name.is_empty() {
            "installable".to_owned()
        } else {
            name.to_owned()
        }
    };
    let attribute = |installable: &str| -> String {
        installable
            .rsplit_once('#')
            .map_or(installable, |(_, attribute)| attribute)
            .to_owned()
    };
    let short = installables
        .iter()
        .map(|installable| {
            let attribute = attribute(installable);
            slug(attribute.rsplit('.').next().unwrap_or(&attribute))
        })
        .collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::from(["flake-check".to_owned()]);
    installables
        .iter()
        .zip(&short)
        .map(|(installable, name)| {
            let ambiguous =
                name == "flake-check" || short.iter().filter(|other| *other == name).count() > 1;
            let base = if ambiguous {
                slug(&attribute(installable))
            } else {
                name.clone()
            };
            let mut job = base.clone();
            let mut suffix = 2;
            while !seen.insert(job.clone()) {
                job = format!("{base}-{suffix}");
                suffix += 1;
            }
            job
        })
        .collect()
}

pub fn release_file(
    format: CrowWorkflowFormat,
    config: &CrowCiConfig,
    inputs: &ReleaseWorkflowInputs<'_>,
) -> Result<GeneratedFile> {
    if !inputs.required_gates.is_empty() {
        bail!(
            "Crow release workflows do not support required gates; use an Actions release provider to enforce them"
        );
    }
    if inputs.tag_prefix != crate::release_identity::TagPrefix::None {
        bail!(
            "Crow release workflows do not support prefixed tags; use an Actions release provider to enforce the configured prefix"
        );
    }
    if matches!(
        inputs.notes_source,
        Some(crate::config::ReleaseNotesSource::Git | crate::config::ReleaseNotesSource::Changelog)
    ) {
        bail!(
            "Crow release workflows do not support release notes; use an Actions release provider or explicitly select notes source none"
        );
    }
    if inputs.pypi.is_some()
        && inputs.publish.policy("pypi") != Some(crate::config::ReleasePublisherPolicy::Disabled)
    {
        bail!(
            "Crow release workflows do not support [release.pypi]; use an Actions release provider to publish the wheels"
        );
    }
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
    if let Some(attic) = inputs.attic {
        let token_secret = attic
            .token_secret
            .as_deref()
            .context("Crow Attic publication requires [release.attic].token_secret")?;
        if attic.result_links.is_empty() {
            bail!("Crow Attic publication requires [release.attic].result_links");
        }
        let paths = attic
            .result_links
            .iter()
            .map(|link| {
                let path =
                    if link.starts_with('/') || link.starts_with("./") || link.starts_with("../") {
                        link.clone()
                    } else {
                        format!("./{link}")
                    };
                shell_quote(&path)
            })
            .collect::<Vec<_>>()
            .join(" ");
        build = build
            .secret(token_secret)
            .command(format!("test -n \"$${{{token_secret}:-}}\""))
            .command(format!("nix path-info -r {paths} > attic-paths.txt"))
            .command("test -s attic-paths.txt".to_owned())
            .command("attic_config_root=\"$(mktemp -d)\"".to_owned())
            .command("trap 'rm -rf \"$attic_config_root\"' EXIT".to_owned())
            .command("install -d -m 0700 \"$attic_config_root/attic\"".to_owned())
            .command("attic_config=\"$attic_config_root/attic/config.toml\"".to_owned())
            .command("install -m 0600 /dev/null \"$attic_config\"".to_owned())
            .command(format!(
                "{{ printf '%s\\n' 'default-server = \"{}\"'; printf '%s\\n' '[servers.{}]'; printf '%s\\n' 'endpoint = \"{}\"'; printf 'token = \"%s\"\\n' \"$${{{token_secret}}}\"; }} > \"$attic_config\"",
                attic.cache, attic.cache, attic.url
            ))
            .command("test \"$(stat -c '%a' \"$attic_config\")\" = 600".to_owned())
            .command("attic=\"$(nix build --inputs-from . --no-link --print-out-paths nixpkgs#attic-client)\"".to_owned())
            .command("test -x \"$attic/bin/attic\"".to_owned())
            .command(format!(
                "XDG_CONFIG_HOME=\"$attic_config_root\" \"$attic/bin/attic\" push --stdin --no-closure --ignore-upstream-cache-filter {} < attic-paths.txt",
                attic.cache
            ));
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
        labels: labels(
            config,
            &ResolvedRunner {
                name: None,
                labels: vec![inputs.runner.to_owned()],
            },
        ),
        platform: config.platform.clone(),
        when: vec![condition("event", "tag")],
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
    #[test]
    fn format_runner_preserves_legacy_override_and_prefers_new_key() {
        use super::*;
        let legacy = ResolvedRunner {
            name: Some("legacy".into()),
            labels: vec!["agent=legacy".into()],
        };
        let current = ResolvedRunner {
            name: Some("current".into()),
            labels: vec!["agent=current".into()],
        };
        let mut runners = BTreeMap::from([(STEP_CARGO_FMT.to_owned(), legacy.clone())]);
        assert_eq!(
            crate::render::ci::step_runner_override(&runners, "format"),
            Some(&legacy)
        );
        let mut steps = vec![Step::new("format", "nix")];
        apply_step_runner_labels(&mut steps, &runners);
        assert_eq!(steps[0].environment["SIMIT_RUNNER_AGENT"], "legacy");
        runners.insert("format".into(), current.clone());
        assert_eq!(
            crate::render::ci::step_runner_override(&runners, "format"),
            Some(&current)
        );
        assert_eq!(
            crate::render::ci::step_runner_override(&runners, STEP_CARGO_FMT),
            Some(&legacy)
        );
    }

    use super::*;

    #[test]
    fn uses_documented_default_image_for_nix_workflows() {
        assert_eq!(
            nix_image(&CrowCiConfig::default()).unwrap(),
            DEFAULT_CROW_NIX_IMAGE
        );
    }

    #[test]
    fn pages_build_creates_the_result_link_it_checks() {
        let runner = crate::user_config::ResolvedRunner::literal("crow").unwrap();
        let pages = ci::CodebergPagesOptions {
            repo: "caniko/site".to_owned(),
            owner: "caniko".to_owned(),
            canonical_domain: None,
            site_output: "site".to_owned(),
            token_secret: "CODEBERG_TOKEN".to_owned(),
            source_branch: "trunk".to_owned(),
            deploy_app: "deploy-pages".to_owned(),
            extra_setup: Vec::new(),
        };
        let file = codeberg_pages_file(
            CrowWorkflowFormat::Yaml,
            &CrowCiConfig::default(),
            &runner,
            &pages,
        )
        .unwrap();
        assert!(file.content.contains("nix build .#site --out-link result"));
        assert!(!file.content.contains("nix build .#site --no-link\n"));
    }

    #[test]
    fn qualify_job_names_disambiguate_shared_final_attributes() {
        let names = super::qualify_job_names(&[
            ".#checks.x86_64-linux.test",
            ".#checks.aarch64-linux.test",
            ".#packages.x86_64-linux.server",
            ".#flake-check",
            ".#packages.x86_64-linux.server",
        ]);
        assert_eq!(
            names,
            [
                "checks-x86-64-linux-test",
                "checks-aarch64-linux-test",
                "packages-x86-64-linux-server",
                "flake-check-2",
                "packages-x86-64-linux-server-2",
            ]
        );
        assert_eq!(super::qualify_job_names(&[".#docs"]), ["docs"]);
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
            steps: vec![
                Step::new("publish", "rust:bookworm")
                    .secret("TOKEN")
                    .command("echo $${TOKEN}".to_owned()),
            ],
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
