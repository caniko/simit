use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::ci_resolution::{CiCliOverrides, CiInference, ResolvedCiInputs, WorkflowSnapshot};
use crate::cli::{
    ChocolateyOverridesArgs, HomebrewOverridesArgs, InitCiCommand, Platform, Runtime,
    RuntimeChoice, ScoopOverridesArgs,
};
use crate::commands::upgrade;
use crate::config::{
    CodebergPagesConfig, ProjectConfig, ResolvedChocolatey, ResolvedCodebergPages,
    ResolvedHomebrew, ResolvedScoop,
};
use crate::project;
use crate::python;
use crate::registry::{self, FeatureStatus};
use crate::release_trust::{self, TrustOverrides};
use crate::render::ci::{
    self, ChocolateyOptions, CiOptions, CodebergPagesOptions, HomebrewOptions, HomebrewPlatformSet,
    OMNIX_REF_DEFAULT, OmCiMode, ScoopOptions, SelfCheckOptions,
};
use crate::user_config::{ResolvedRunner, UserConfig, validate_runner_label};

pub fn run(command: InitCiCommand) -> Result<()> {
    if cargo::find_manifest(&std::env::current_dir().context("reading current directory")?).is_err()
        && python::project_for_current_dir().is_ok()
    {
        return run_python(command);
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let cfg = ProjectConfig::load(workspace_root)?;
    let workflow_snapshots = workflow_snapshots_for_platform(workspace_root, command.platform)?;
    let inference = CiInference::from_workflows(&workflow_snapshots)?;
    let inferred_pages = infer_codeberg_pages_from_workflows(&workflow_snapshots)?;
    let cli_overrides = ci_cli_overrides(&command);
    let mut resolved =
        ResolvedCiInputs::resolve(workspace_root, &cfg, &cli_overrides, Some(&inference))?;
    if resolved.granular && command.platform == Platform::Forgejo {
        apply_granular_step_runners(&mut resolved.step_runners, resolved.runtime);
    }
    validate_runner(resolved.runner.as_deref())?;
    validate_runner(resolved.windows_runner.as_deref())?;
    let packages = cargo::select_packages(&metadata, &resolved.packages, resolved.workspace)?;
    if command.with_homebrew && command.platform != Platform::Forgejo {
        bail!("Homebrew tap publish is forgejo-only for now");
    }
    if command.with_homebrew && resolved.runtime != Runtime::Nix {
        bail!("Homebrew tap publish requires --runtime nix");
    }
    // Chocolatey and Scoop are allowed with both runtimes: phase 3 will build
    // Windows artifacts on Windows runners, independent of the Linux CI runtime.
    let self_check = metadata
        .packages
        .iter()
        .any(|package| package.name == "simit");
    let windows_packagers = command.with_chocolatey || command.with_scoop;
    let with_artifacts = resolved.with_artifacts || command.with_homebrew || windows_packagers;
    if command.with_homebrew && !resolved.with_artifacts {
        eprintln!("--with-homebrew implies --with-artifacts; enabling it.");
    }
    if command.with_chocolatey && !resolved.with_artifacts {
        eprintln!("--with-chocolatey implies --with-artifacts; enabling it.");
    }
    if command.with_scoop && !resolved.with_artifacts {
        eprintln!("--with-scoop implies --with-artifacts; enabling it.");
    }
    let wants_codeberg_pages =
        command.with_codeberg_pages || cfg.ci.pages.is_some() || inferred_pages.is_some();
    if wants_codeberg_pages && command.platform != Platform::Forgejo {
        bail!("Codeberg Pages workflow generation is forgejo-only");
    }
    if wants_codeberg_pages && resolved.runtime != Runtime::Nix {
        bail!("Codeberg Pages workflow generation requires --runtime nix");
    }
    let explicit_runners_cover_required =
        runner_overrides_cover_required_runners(&resolved, windows_packagers);
    if resolved.om_ci != OmCiMode::Off && resolved.runtime != Runtime::Nix {
        bail!("--with-om-ci requires --runtime nix");
    }
    if command.omnix_ref.is_some() && resolved.om_ci == OmCiMode::Off {
        eprintln!("--omnix-ref is ignored unless --with-om-ci or --om-ci-augment is enabled.");
    }
    let user_config = UserConfig::load().or_else(|err| {
        if command.platform == Platform::Github || explicit_runners_cover_required {
            Ok(UserConfig::default())
        } else {
            Err(err)
        }
    })?;
    let omnix_ref = command
        .omnix_ref
        .clone()
        .or_else(|| user_config.ci.tools.omnix.r#ref.clone())
        .unwrap_or_else(|| resolved.omnix_ref.clone());
    let options = resolved.ci_options(&cfg, with_artifacts, omnix_ref.clone());
    let runners = user_config.resolve_ci_runners(
        command.platform,
        resolved.runtime,
        resolved.runner.as_deref(),
        resolved.windows_runner.as_deref(),
        windows_packagers,
    )?;
    let step_runners: BTreeMap<String, ResolvedRunner> = resolved
        .step_runners
        .iter()
        .map(|(step, label)| {
            Ok((
                step.clone(),
                ResolvedRunner::literal(label)
                    .map_err(|e| anyhow::anyhow!("invalid step runner label for '{step}': {e}"))?,
            ))
        })
        .collect::<Result<_>>()?;
    let persisted_runner =
        self_check_runner_override(resolved.runner.as_deref(), &runners.ci).map(str::to_owned);
    let persisted_windows_runner = runners.windows.as_ref().and_then(|runner| {
        self_check_runner_override(resolved.windows_runner.as_deref(), runner).map(str::to_owned)
    });
    let multi_package_workspace = metadata.workspace_members.len() > 1;
    let mut files = Vec::new();
    for package in &packages {
        let homebrew = if command.with_homebrew {
            Some(homebrew_options(&cfg, &command.homebrew, package)?)
        } else {
            None
        };
        let chocolatey = if command.with_chocolatey {
            Some(chocolatey_options(&cfg, &command.chocolatey, package)?)
        } else {
            None
        };
        let scoop = if command.with_scoop {
            Some(scoop_options(&cfg, &command.scoop, package)?)
        } else {
            None
        };
        let package_options = CiOptions {
            homebrew,
            chocolatey,
            scoop,
            package_scoped: multi_package_workspace,
            ..options.clone()
        };
        let self_check_runner = self_check_runner_override(resolved.runner.as_deref(), &runners.ci);
        let self_check_windows_runner = runners.windows.as_ref().and_then(|runner| {
            self_check_runner_override(resolved.windows_runner.as_deref(), runner)
        });
        files.extend(ci::files(
            command.platform,
            resolved.runtime,
            package,
            multi_package_workspace.then_some(package.name.as_str()),
            SelfCheckOptions {
                enabled: self_check,
                runner_override: self_check_runner,
                windows_runner_override: self_check_windows_runner,
                packages: &resolved.packages,
                workspace: resolved.workspace,
            },
            &runners,
            package_options,
            &step_runners,
        )?);
    }
    if resolved.with_pypi_publish && cargo::has_pyo3_dep(&metadata.packages) {
        files.push(ci::maturin_publish_file(
            command.platform,
            resolved.runtime,
            &runners.ci,
            &options,
        )?);
    }
    let pages = codeberg_pages_options(&cfg, &command, inferred_pages.as_ref())?;
    if let Some(pages) = &pages {
        files.push(ci::codeberg_pages_file(
            command.platform,
            &runners.ci,
            pages,
        )?);
    }
    let persisted_ci = resolved.persisted_ci(
        &cfg,
        with_artifacts,
        &omnix_ref,
        persisted_runner,
        persisted_windows_runner,
    );
    let mut persisted_ci = persisted_ci;
    if command.with_codeberg_pages {
        persisted_ci.pages = Some(codeberg_pages_config(&pages)?);
    } else if let Some(pages) = inferred_pages {
        persisted_ci.pages = Some(pages);
    }
    let persisted_in_simit_toml =
        workspace_root.join("simit.toml").exists() && cfg.ci == persisted_ci;
    let check_message = format!(
        "CI workflows are not up to date; run `{}`",
        render_regeneration_command(
            &command,
            &resolved,
            with_artifacts,
            &omnix_ref,
            persisted_in_simit_toml
        )
    );
    let trust_overrides = TrustOverrides {
        key: command.maintainer_key,
        trust_root: command.maintainers_gpg,
    };
    maybe_push_deny_template(
        workspace_root,
        resolved.with_deny,
        command.check,
        &mut files,
    );
    files.push(release_trust::generated_file(
        workspace_root,
        &cfg,
        &trust_overrides,
        command.check,
    )?);

    if command.check {
        check_generated_ci_files(
            workspace_root,
            &files,
            command.platform,
            &check_message,
            command.diff,
        )?;
        upgrade::update_readme_badges_if_present(workspace_root, true, command.diff)
    } else {
        project::write_generated_files(workspace_root, &files)?;
        if ProjectConfig::can_persist_ci(workspace_root)? {
            ProjectConfig::write_ci(workspace_root, &persisted_ci)?;
        }
        upgrade::update_readme_badges_if_present(workspace_root, false, false)?;
        registry::touch_current_project_or_warn([("ci", FeatureStatus::Managed)]);
        Ok(())
    }
}

fn run_python(command: InitCiCommand) -> Result<()> {
    let project = python::project_for_current_dir()?;
    let workspace_root = project.workspace_root.as_std_path();
    let cfg = ProjectConfig::load(workspace_root)?;

    if command.workspace || !command.packages.is_empty() {
        bail!("Python uv CI does not support --workspace or --package");
    }
    if command.with_homebrew
        || command.with_chocolatey
        || command.with_scoop
        || command.with_artifacts == Some(true)
    {
        bail!("Python uv CI currently supports CI only, not release packaging workflows");
    }
    if command.with_nextest == Some(true)
        || command.with_msrv == Some(true)
        || command.with_audit == Some(true)
        || command.with_deny == Some(true)
        || command.with_docs == Some(true)
    {
        bail!("Rust-specific CI options are not supported for Python uv projects");
    }
    let workflow_snapshots = workflow_snapshots_for_platform(workspace_root, command.platform)?;
    let inferred_pages = infer_codeberg_pages_from_workflows(&workflow_snapshots)?;
    if command.with_codeberg_pages || cfg.ci.pages.is_some() || inferred_pages.is_some() {
        bail!("Codeberg Pages generation is not supported for Python uv CI yet");
    }
    let inference = CiInference::from_workflows(&workflow_snapshots)?;
    let mut cli_overrides = ci_cli_overrides(&command);
    if cli_overrides.runtime.is_none() && cfg.ci.runtime.is_none() {
        cli_overrides.runtime = Some(RuntimeChoice::Nix);
    }
    let resolved =
        ResolvedCiInputs::resolve(workspace_root, &cfg, &cli_overrides, Some(&inference))?;
    if resolved.runtime != Runtime::Nix {
        bail!("Python uv CI generation requires --runtime nix");
    }
    validate_runner(resolved.runner.as_deref())?;

    let explicit_runners_cover_required = runner_overrides_cover_required_runners(&resolved, false);
    let user_config = UserConfig::load().or_else(|err| {
        if command.platform == Platform::Github || explicit_runners_cover_required {
            Ok(UserConfig::default())
        } else {
            Err(err)
        }
    })?;
    let omnix_ref = command
        .omnix_ref
        .clone()
        .or_else(|| user_config.ci.tools.omnix.r#ref.clone())
        .unwrap_or_else(|| resolved.omnix_ref.clone());
    let options = resolved.ci_options(&cfg, false, omnix_ref.clone());
    let runners = user_config.resolve_ci_runners(
        command.platform,
        resolved.runtime,
        resolved.runner.as_deref(),
        resolved.windows_runner.as_deref(),
        false,
    )?;
    let persisted_runner =
        self_check_runner_override(resolved.runner.as_deref(), &runners.ci).map(str::to_owned);
    let with_pypi_publish = resolved.with_pypi_publish;
    let mut files = vec![ci::python_ci_file(
        command.platform,
        resolved.runtime,
        &runners.ci,
        &options,
        &cfg.flake.expected_outputs.checks,
    )?];
    if with_pypi_publish {
        files.push(ci::python_publish_file(
            command.platform,
            resolved.runtime,
            &runners.ci,
            &options,
        )?);
    }
    let persisted_ci = resolved.persisted_ci(&cfg, false, &omnix_ref, persisted_runner, None);
    let persisted_in_simit_toml =
        workspace_root.join("simit.toml").exists() && cfg.ci == persisted_ci;
    let check_message = format!(
        "CI workflows are not up to date; run `{}`",
        render_regeneration_command(
            &command,
            &resolved,
            false,
            &omnix_ref,
            persisted_in_simit_toml,
        )
    );

    if command.check {
        check_generated_ci_files(
            workspace_root,
            &files,
            command.platform,
            &check_message,
            command.diff,
        )?;
        upgrade::update_readme_badges_if_present(workspace_root, true, command.diff)
    } else {
        project::write_generated_files(workspace_root, &files)?;
        if ProjectConfig::can_persist_ci(workspace_root)? {
            ProjectConfig::write_ci(workspace_root, &persisted_ci)?;
        }
        upgrade::update_readme_badges_if_present(workspace_root, false, false)?;
        registry::touch_current_project_or_warn([("ci", FeatureStatus::Managed)]);
        Ok(())
    }
}

fn ci_cli_overrides(command: &InitCiCommand) -> CiCliOverrides {
    let step_runner = command
        .step_runner
        .iter()
        .filter_map(|arg| {
            let (step, runner) = arg.split_once('=')?;
            Some((step.to_string(), runner.to_string()))
        })
        .collect();
    CiCliOverrides {
        runtime: command.runtime,
        runner: command.runner.clone(),
        windows_runner: command.windows_runner.clone(),
        step_runner,
        granular: command.granular,
        workspace: command.workspace,
        packages: command.packages.clone(),
        with_nextest: command.with_nextest,
        with_msrv: command.with_msrv,
        with_audit: command.with_audit,
        with_deny: command.with_deny,
        with_docs: command.with_docs,
        with_artifacts: command.with_artifacts,
        with_pypi_publish: command.with_pypi_publish,
        with_om_ci: command.with_om_ci,
        om_ci_augment: command.om_ci_augment,
        omnix_ref: command.omnix_ref.clone(),
        release_smoke_command: command.release_smoke_command.clone(),
    }
}

pub(crate) fn workflow_snapshots_for_platform(
    workspace_root: &Path,
    platform: Platform,
) -> Result<Vec<WorkflowSnapshot>> {
    let workflow_dir = PathBuf::from(platform.workflow_dir());
    let absolute_dir = workspace_root.join(&workflow_dir);
    let entries = match fs::read_dir(&absolute_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", absolute_dir.display())),
    };

    let mut snapshots = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("reading entry in {}", absolute_dir.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("reading file type for {}", entry.path().display()))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if extension != "yaml" && extension != "yml" {
            continue;
        }
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        if content.contains(ci::GENERATED_WORKFLOW_MARKER)
            && is_ci_managed_workflow_name(&entry.file_name())
        {
            snapshots.push(WorkflowSnapshot {
                relative_path: workflow_dir.join(entry.file_name()),
                content,
            });
        }
    }

    snapshots.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(snapshots)
}

pub(crate) fn project_regeneration_command(workspace_root: &Path) -> Result<Option<String>> {
    let forgejo = workflow_snapshots_for_platform(workspace_root, Platform::Forgejo)?;
    let github = workflow_snapshots_for_platform(workspace_root, Platform::Github)?;
    let (platform, snapshots) = match (forgejo.is_empty(), github.is_empty()) {
        (true, true) => return Ok(None),
        (false, true) => (Platform::Forgejo, forgejo),
        (true, false) => (Platform::Github, github),
        (false, false) => bail!("mixed generated CI platforms in workflow tree"),
    };

    let cfg = ProjectConfig::load(workspace_root)?;
    let inference = CiInference::from_workflows(&snapshots)?;
    let resolved = ResolvedCiInputs::resolve(
        workspace_root,
        &cfg,
        &CiCliOverrides::default(),
        Some(&inference),
    )?;
    let with_homebrew = snapshots
        .iter()
        .any(|workflow| workflow.content.contains("name: Publish Homebrew tap"));
    let with_chocolatey = snapshots.iter().any(|workflow| {
        workflow
            .content
            .contains("name: Publish Chocolatey package")
    });
    let with_scoop = snapshots
        .iter()
        .any(|workflow| workflow.content.contains("name: Publish Scoop bucket"));
    let windows_packagers = with_chocolatey || with_scoop;
    let with_artifacts = resolved.with_artifacts || with_homebrew || windows_packagers;
    let inferred_pages = infer_codeberg_pages_from_workflows(&snapshots)?;
    let command = InitCiCommand {
        packages: Vec::new(),
        workspace: false,
        platform,
        runtime: None,
        runner: None,
        windows_runner: None,
        granular: resolved.granular,
        maintainer_key: None,
        maintainers_gpg: None,
        release_smoke_command: None,
        check: false,
        diff: false,
        with_nextest: None,
        with_msrv: None,
        with_audit: None,
        with_deny: None,
        with_docs: None,
        with_om_ci: None,
        om_ci_augment: None,
        omnix_ref: None,
        step_runner: Vec::new(),
        with_artifacts: None,
        with_homebrew,
        with_chocolatey,
        with_scoop,
        homebrew: HomebrewOverridesArgs {
            name: None,
            tap: None,
            binary: Vec::new(),
            description: None,
            homepage: None,
            license: None,
            download_repo: None,
            archive_pattern: None,
            no_platform: Vec::new(),
        },
        chocolatey: ChocolateyOverridesArgs {
            name: None,
            id: None,
            title: None,
            authors: None,
            description: None,
            project_url: None,
            license_url: None,
            tags: None,
            release_notes_url: None,
            download_repo: None,
            archive_pattern: None,
            push_source: None,
        },
        scoop: ScoopOverridesArgs {
            name: None,
            bucket: None,
            description: None,
            homepage: None,
            license: None,
            download_repo: None,
            archive_pattern: None,
            binary: Vec::new(),
            no_arch: Vec::new(),
        },
        with_pypi_publish: None,
        with_codeberg_pages: inferred_pages.is_some(),
        pages_repo: inferred_pages.as_ref().map(|pages| pages.repo.clone()),
        pages_canonical_domain: inferred_pages
            .as_ref()
            .and_then(|pages| pages.canonical_domain.clone()),
        pages_site_output: inferred_pages
            .as_ref()
            .map(|pages| pages.site_output.clone()),
        pages_token_secret: inferred_pages
            .as_ref()
            .map(|pages| pages.token_secret.clone()),
        pages_source_branch: inferred_pages
            .as_ref()
            .map(|pages| pages.source_branch.clone()),
        pages_deploy_app: inferred_pages
            .as_ref()
            .map(|pages| pages.deploy_app.clone()),
    };

    let persisted_in_simit_toml =
        persisted_ci_matches_simit_toml(&command, workspace_root, &cfg, &resolved, with_artifacts)
            .unwrap_or_default();
    let mut rendered = render_regeneration_command(
        &command,
        &resolved,
        with_artifacts,
        &resolved.omnix_ref,
        persisted_in_simit_toml,
    );
    let verify_flags = regeneration_verify_flags(&cfg, &inference, windows_packagers);
    if !verify_flags.is_empty() {
        rendered.push_str(" # verify ");
        rendered.push_str(&verify_flags.join(" "));
    }
    Ok(Some(rendered))
}

pub(crate) fn render_regeneration_command(
    command: &InitCiCommand,
    resolved: &ResolvedCiInputs,
    with_artifacts: bool,
    omnix_ref: &str,
    persisted_in_simit_toml: bool,
) -> String {
    if persisted_in_simit_toml {
        return format!("simit init ci --platform {}", command.platform.as_str());
    }

    let mut args = vec![
        "simit".to_owned(),
        "init".to_owned(),
        "ci".to_owned(),
        "--platform".to_owned(),
        command.platform.as_str().to_owned(),
    ];

    if resolved.runtime != Runtime::Cargo || command.runtime.is_some() {
        args.push("--runtime".to_owned());
        args.push(runtime_as_str(resolved.runtime).to_owned());
    }
    if resolved.granular {
        args.push("--granular".to_owned());
    }
    if let Some(runner) = &resolved.runner {
        args.push("--runner".to_owned());
        args.push(shell_word(runner));
    }
    if let Some(runner) = &resolved.windows_runner {
        args.push("--windows-runner".to_owned());
        args.push(shell_word(runner));
    }
    if resolved.workspace {
        args.push("--workspace".to_owned());
    }
    for package in &resolved.packages {
        args.push("--package".to_owned());
        args.push(shell_word(package));
    }
    if resolved.with_nextest {
        args.push("--with-nextest".to_owned());
    }
    if resolved.with_msrv {
        args.push("--with-msrv".to_owned());
    }
    if resolved.with_audit {
        args.push("--with-audit".to_owned());
    }
    if resolved.with_deny {
        args.push("--with-deny".to_owned());
    }
    if resolved.with_docs {
        args.push("--with-docs".to_owned());
    }
    if with_artifacts {
        args.push("--with-artifacts".to_owned());
    }
    if resolved.with_pypi_publish {
        args.push("--with-pypi-publish".to_owned());
    }
    match resolved.om_ci {
        OmCiMode::Off => {}
        OmCiMode::Replace => args.push("--with-om-ci".to_owned()),
        OmCiMode::Augment => args.push("--om-ci-augment".to_owned()),
    }
    if resolved.om_ci != OmCiMode::Off && omnix_ref != OMNIX_REF_DEFAULT {
        args.push("--omnix-ref".to_owned());
        args.push(shell_word(omnix_ref));
    }
    if command.with_homebrew {
        args.push("--with-homebrew".to_owned());
        push_optional_arg(
            &mut args,
            "--homebrew-name",
            command.homebrew.name.as_deref(),
        );
        push_optional_arg(&mut args, "--homebrew-tap", command.homebrew.tap.as_deref());
        for binary in &command.homebrew.binary {
            args.push("--homebrew-binary".to_owned());
            args.push(shell_word(binary));
        }
        push_optional_arg(
            &mut args,
            "--homebrew-description",
            command.homebrew.description.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--homebrew-homepage",
            command.homebrew.homepage.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--homebrew-license",
            command.homebrew.license.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--homebrew-download-repo",
            command.homebrew.download_repo.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--homebrew-archive-pattern",
            command.homebrew.archive_pattern.as_deref(),
        );
        for platform in &command.homebrew.no_platform {
            args.push("--homebrew-no-platform".to_owned());
            args.push(shell_word(platform));
        }
    }
    if command.with_chocolatey {
        args.push("--with-chocolatey".to_owned());
    }
    if command.with_scoop {
        args.push("--with-scoop".to_owned());
    }
    if command.with_codeberg_pages {
        args.push("--with-codeberg-pages".to_owned());
        push_optional_arg(&mut args, "--pages-repo", command.pages_repo.as_deref());
        push_optional_arg(
            &mut args,
            "--pages-canonical-domain",
            command.pages_canonical_domain.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--pages-site-output",
            command.pages_site_output.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--pages-token-secret",
            command.pages_token_secret.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--pages-source-branch",
            command.pages_source_branch.as_deref(),
        );
        push_optional_arg(
            &mut args,
            "--pages-deploy-app",
            command.pages_deploy_app.as_deref(),
        );
    }

    args.join(" ")
}

fn persisted_ci_matches_simit_toml(
    command: &InitCiCommand,
    workspace_root: &Path,
    cfg: &ProjectConfig,
    resolved: &ResolvedCiInputs,
    with_artifacts: bool,
) -> Result<bool> {
    if !workspace_root.join("simit.toml").exists() {
        return Ok(false);
    }

    let windows_packagers = command.with_chocolatey || command.with_scoop;
    let explicit_runners_cover_required =
        runner_overrides_cover_required_runners(resolved, windows_packagers);
    let user_config = UserConfig::load().or_else(|err| {
        if command.platform == Platform::Github || explicit_runners_cover_required {
            Ok(UserConfig::default())
        } else {
            Err(err)
        }
    })?;
    let runners = user_config.resolve_ci_runners(
        command.platform,
        resolved.runtime,
        resolved.runner.as_deref(),
        resolved.windows_runner.as_deref(),
        windows_packagers,
    )?;
    let persisted_runner =
        self_check_runner_override(resolved.runner.as_deref(), &runners.ci).map(str::to_owned);
    let persisted_windows_runner = runners.windows.as_ref().and_then(|runner| {
        self_check_runner_override(resolved.windows_runner.as_deref(), runner).map(str::to_owned)
    });
    let persisted_ci = resolved.persisted_ci(
        cfg,
        with_artifacts,
        &resolved.omnix_ref,
        persisted_runner,
        persisted_windows_runner,
    );
    let mut persisted_ci = persisted_ci;
    if command.with_codeberg_pages && cfg.ci.pages.is_some() {
        persisted_ci.pages = cfg.ci.pages.clone();
    }
    Ok(cfg.ci == persisted_ci)
}

fn regeneration_verify_flags(
    cfg: &ProjectConfig,
    inference: &CiInference,
    windows_packagers: bool,
) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if cfg.ci.runner.is_none() && inference.runner.is_none() {
        flags.push("--runner");
    }
    if windows_packagers && cfg.ci.windows_runner.is_none() && inference.windows_runner.is_none() {
        flags.push("--windows-runner");
    }
    flags
}

fn runtime_as_str(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Cargo => "cargo",
        Runtime::Nix => "nix",
    }
}

fn push_optional_arg(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(flag.to_owned());
        args.push(shell_word(value));
    }
}

fn shell_word(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '='))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn maybe_push_deny_template(
    workspace_root: &Path,
    with_deny: bool,
    check: bool,
    files: &mut Vec<project::GeneratedFile>,
) {
    if !with_deny || check || workspace_root.join("deny.toml").exists() {
        return;
    }

    files.push(project::GeneratedFile {
        relative_path: PathBuf::from("deny.toml"),
        content: ci::deny_toml(),
    });
}

fn check_generated_ci_files(
    workspace_root: &Path,
    files: &[project::GeneratedFile],
    platform: Platform,
    message: &str,
    show_diff: bool,
) -> Result<()> {
    project::check_generated_files(workspace_root, files, message, show_diff)?;

    let expected = files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let extras = extra_generated_workflows(workspace_root, platform, &expected)?;
    if extras.is_empty() {
        Ok(())
    } else {
        let details = extras
            .into_iter()
            .map(|path| format!("{} is extra", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("{message}:\n{details}");
    }
}

fn extra_generated_workflows(
    workspace_root: &Path,
    platform: Platform,
    expected: &BTreeSet<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let workflow_dir = PathBuf::from(platform.workflow_dir());
    let absolute_dir = workspace_root.join(&workflow_dir);
    let entries = match fs::read_dir(&absolute_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", absolute_dir.display())),
    };

    let mut extras = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("reading entry in {}", absolute_dir.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("reading file type for {}", entry.path().display()))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if extension != "yaml" && extension != "yml" {
            continue;
        }
        let relative_path = workflow_dir.join(entry.file_name());
        if expected.contains(&relative_path) {
            continue;
        }
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        if content.contains(ci::GENERATED_WORKFLOW_MARKER)
            && is_ci_managed_workflow_name(&entry.file_name())
        {
            extras.push(relative_path);
        }
    }
    extras.sort();
    Ok(extras)
}

fn is_ci_managed_workflow_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    matches!(
        name,
        "ci.yaml"
            | "ci.yml"
            | "publish-crate.yaml"
            | "publish-crate.yml"
            | "release-artifacts.yaml"
            | "release-artifacts.yml"
            | "pages.yaml"
            | "pages.yml"
    ) || name.starts_with("ci-")
        || name.starts_with("publish-crate-")
        || name.starts_with("release-artifacts-")
}

fn workflow_name(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

fn runner_overrides_cover_required_runners(
    resolved: &ResolvedCiInputs,
    windows_packagers: bool,
) -> bool {
    resolved.runner.is_some() && (!windows_packagers || resolved.windows_runner.is_some())
}

fn self_check_runner_override<'a>(
    explicit: Option<&'a str>,
    resolved: &'a ResolvedRunner,
) -> Option<&'a str> {
    explicit.or_else(|| {
        if resolved.labels.len() == 1 {
            Some(resolved.labels[0].as_str())
        } else {
            None
        }
    })
}

fn codeberg_pages_options(
    cfg: &ProjectConfig,
    command: &InitCiCommand,
    inferred: Option<&CodebergPagesConfig>,
) -> Result<Option<CodebergPagesOptions>> {
    if !command.with_codeberg_pages && cfg.ci.pages.is_none() && inferred.is_none() {
        return Ok(None);
    }

    let resolved = resolve_codeberg_pages(cfg, command, inferred)?;
    Ok(Some(CodebergPagesOptions {
        repo: resolved.repo,
        owner: resolved.owner,
        canonical_domain: resolved.canonical_domain,
        site_output: resolved.site_output,
        token_secret: resolved.token_secret,
        source_branch: resolved.source_branch,
        deploy_app: resolved.deploy_app,
    }))
}

fn resolve_codeberg_pages(
    cfg: &ProjectConfig,
    command: &InitCiCommand,
    inferred: Option<&CodebergPagesConfig>,
) -> Result<ResolvedCodebergPages> {
    let config = cfg.resolve_codeberg_pages()?;
    let repo = command
        .pages_repo
        .clone()
        .or_else(|| config.as_ref().map(|pages| pages.repo.clone()))
        .or_else(|| inferred.map(|pages| pages.repo.clone()))
        .context("--with-codeberg-pages requires --pages-repo or [ci.pages].repo")?;
    validate_download_repo_for("--pages-repo", &repo)?;
    let owner = repo
        .split_once('/')
        .expect("validated owner/repo")
        .0
        .to_owned();
    let token_secret = command
        .pages_token_secret
        .clone()
        .or_else(|| config.as_ref().map(|pages| pages.token_secret.clone()))
        .or_else(|| inferred.map(|pages| pages.token_secret.clone()))
        .unwrap_or_else(|| "codeberg_token".to_owned());
    let canonical_domain = command
        .pages_canonical_domain
        .clone()
        .or_else(|| {
            config
                .as_ref()
                .and_then(|pages| pages.canonical_domain.clone())
        })
        .or_else(|| inferred.and_then(|pages| pages.canonical_domain.clone()));
    let site_output = command
        .pages_site_output
        .clone()
        .or_else(|| config.as_ref().map(|pages| pages.site_output.clone()))
        .or_else(|| inferred.map(|pages| pages.site_output.clone()))
        .unwrap_or_else(|| ".#site".to_owned());
    let source_branch = command
        .pages_source_branch
        .clone()
        .or_else(|| config.as_ref().map(|pages| pages.source_branch.clone()))
        .or_else(|| inferred.map(|pages| pages.source_branch.clone()))
        .unwrap_or_else(|| "trunk".to_owned());
    let deploy_app = command
        .pages_deploy_app
        .clone()
        .or_else(|| config.as_ref().map(|pages| pages.deploy_app.clone()))
        .or_else(|| inferred.map(|pages| pages.deploy_app.clone()))
        .unwrap_or_else(|| ".#deploy-pages".to_owned());
    if token_secret.trim().is_empty() {
        bail!("--pages-token-secret must not be empty");
    }
    if canonical_domain
        .as_ref()
        .is_some_and(|domain| domain.trim().is_empty())
    {
        bail!("--pages-canonical-domain must not be empty");
    }
    if site_output.trim().is_empty() {
        bail!("--pages-site-output must not be empty");
    }
    if source_branch.trim().is_empty() {
        bail!("--pages-source-branch must not be empty");
    }
    if deploy_app.trim().is_empty() {
        bail!("--pages-deploy-app must not be empty");
    }

    Ok(ResolvedCodebergPages {
        repo,
        owner,
        canonical_domain,
        site_output,
        token_secret,
        source_branch,
        deploy_app,
    })
}

fn infer_codeberg_pages_from_workflows(
    snapshots: &[WorkflowSnapshot],
) -> Result<Option<CodebergPagesConfig>> {
    let Some(workflow) = snapshots
        .iter()
        .find(|workflow| workflow_name(&workflow.relative_path) == Some("pages"))
    else {
        return Ok(None);
    };

    let Some(repo) = infer_pages_repo(&workflow.content) else {
        return Ok(None);
    };
    validate_download_repo_for("--pages-repo", &repo)?;
    Ok(Some(CodebergPagesConfig {
        repo,
        canonical_domain: infer_pages_canonical_domain(&workflow.content),
        site_output: infer_pages_site_output(&workflow.content)
            .unwrap_or_else(|| ".#site".to_owned()),
        token_secret: infer_pages_token_secret(&workflow.content)
            .unwrap_or_else(|| "codeberg_token".to_owned()),
        source_branch: infer_pages_source_branch(&workflow.content)
            .unwrap_or_else(|| "trunk".to_owned()),
        deploy_app: infer_pages_deploy_app(&workflow.content)
            .unwrap_or_else(|| ".#deploy-pages".to_owned()),
    }))
}

fn infer_pages_repo(content: &str) -> Option<String> {
    let marker = "@codeberg.org/";
    let line = content.lines().find(|line| line.contains(marker))?;
    let repo_start = line.find(marker)? + marker.len();
    let repo_tail = &line[repo_start..];
    let repo_end = repo_tail.find(".git").unwrap_or(repo_tail.len());
    Some(repo_tail[..repo_end].trim_matches('"').to_owned())
}

fn infer_pages_token_secret(content: &str) -> Option<String> {
    let marker = "CODEBERG_TOKEN: ${{ secrets.";
    let line = content.lines().find(|line| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(" }}")?;
    Some(tail[..end].to_owned())
}

fn infer_pages_canonical_domain(content: &str) -> Option<String> {
    let marker = "grep -qx ";
    let suffix = " result-pages-site/.domains";
    let line = content
        .lines()
        .find(|line| line.contains(marker) && line.contains(suffix))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(suffix)?;
    Some(shell_unquote(tail[..end].trim()))
}

fn infer_pages_site_output(content: &str) -> Option<String> {
    let marker = "nix build ";
    let suffix = " --no-link --out-link result-pages-site";
    let line = content
        .lines()
        .find(|line| line.contains(marker) && line.contains(suffix))?;
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(suffix)?;
    Some(shell_unquote(tail[..end].trim()))
}

fn infer_pages_source_branch(content: &str) -> Option<String> {
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim_start().starts_with("branches:") {
            continue;
        }
        let trimmed = line.trim();
        if let Some(inline) = trimmed
            .strip_prefix("branches: [")
            .and_then(|value| value.strip_suffix(']'))
        {
            return Some(inline.trim_matches('"').to_owned());
        }
        while let Some(next) = lines.peek() {
            let trimmed = next.trim();
            if let Some(branch) = trimmed.strip_prefix("- ") {
                return Some(branch.trim_matches('"').to_owned());
            }
            if !next.starts_with(' ') {
                break;
            }
            lines.next();
        }
    }
    None
}

fn infer_pages_deploy_app(content: &str) -> Option<String> {
    let marker = "DEPLOY_REMOTE=pages-origin nix run ";
    let line = content.lines().find(|line| line.contains(marker))?;
    let start = line.find(marker)? + marker.len();
    Some(line[start..].trim().to_owned())
}

fn shell_unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("'\"'\"'", "'");
    }
    value.to_owned()
}

fn codeberg_pages_config(pages: &Option<CodebergPagesOptions>) -> Result<CodebergPagesConfig> {
    let pages = pages
        .as_ref()
        .context("--with-codeberg-pages did not resolve a Pages configuration")?;
    Ok(CodebergPagesConfig {
        repo: pages.repo.clone(),
        canonical_domain: pages.canonical_domain.clone(),
        site_output: pages.site_output.clone(),
        token_secret: pages.token_secret.clone(),
        source_branch: pages.source_branch.clone(),
        deploy_app: pages.deploy_app.clone(),
    })
}

fn chocolatey_options(
    cfg: &ProjectConfig,
    args: &ChocolateyOverridesArgs,
    package: &cargo::Package,
) -> Result<ChocolateyOptions> {
    let resolved = cfg.resolve_chocolatey(args.as_overrides(), package)?;
    validate_download_repo_for("--choco-download-repo", &resolved.download_repo)?;
    Ok(chocolatey_options_from_resolved(resolved))
}

fn chocolatey_options_from_resolved(resolved: ResolvedChocolatey) -> ChocolateyOptions {
    ChocolateyOptions {
        name: resolved.name,
        id: resolved.id,
        title: resolved.title,
        authors: resolved.authors,
        description: resolved.description,
        project_url: resolved.project_url,
        license_url: resolved.license_url,
        tags: resolved.tags,
        release_notes_url: resolved.release_notes_url,
        download_repo: resolved.download_repo,
        archive_pattern: resolved.archive_pattern,
        push_source: resolved.push.source,
    }
}

fn scoop_options(
    cfg: &ProjectConfig,
    args: &ScoopOverridesArgs,
    package: &cargo::Package,
) -> Result<ScoopOptions> {
    let resolved = cfg.resolve_scoop(args.as_overrides(), package)?;
    validate_download_repo_for("--scoop-download-repo", &resolved.download_repo)?;
    Ok(scoop_options_from_resolved(resolved))
}

fn scoop_options_from_resolved(resolved: ResolvedScoop) -> ScoopOptions {
    ScoopOptions {
        name: resolved.name,
        bucket_url: resolved.bucket_url,
        bucket_token_secret: resolved.bucket_token_secret,
        description: resolved.description,
        homepage: resolved.homepage,
        license: resolved.license,
        download_repo: resolved.download_repo,
        archive_pattern: resolved.archive_pattern,
        binaries: resolved.binaries,
        x64: resolved.architectures.x64,
        arm64: resolved.architectures.arm64,
    }
}

fn homebrew_options(
    cfg: &ProjectConfig,
    args: &HomebrewOverridesArgs,
    package: &cargo::Package,
) -> Result<HomebrewOptions> {
    let resolved = cfg.resolve_homebrew(args.as_overrides(), package)?;
    let tap_url = normalize_tap_url(&resolved.tap_url)?;
    validate_download_repo(&resolved.download_repo)?;
    let platforms = homebrew_platforms(&resolved);
    Ok(HomebrewOptions {
        name: resolved.name,
        binaries: resolved.binaries,
        tap_url,
        tap_token_secret: resolved.tap_token_secret,
        description: resolved.description,
        homepage: resolved.homepage,
        license: resolved.license,
        archive_pattern: resolved.archive_pattern,
        download_repo: resolved.download_repo,
        platforms,
    })
}

fn normalize_tap_url(value: &str) -> Result<String> {
    if value.is_empty() {
        bail!("--homebrew-tap cannot be empty");
    }
    let with_scheme = if value.starts_with("https://") || value.starts_with("http://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let (_, path) = with_scheme
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("--homebrew-tap must include a repository path"))?;
    if path.trim_matches('/').is_empty() {
        bail!("--homebrew-tap must include a repository path");
    }
    if with_scheme.ends_with(".git") {
        Ok(with_scheme)
    } else {
        Ok(format!("{with_scheme}.git"))
    }
}

fn validate_download_repo(value: &str) -> Result<()> {
    validate_download_repo_for("--homebrew-download-repo", value)
}

fn validate_download_repo_for(flag: &str, value: &str) -> Result<()> {
    if value.split('/').count() != 2 || value.split('/').any(str::is_empty) {
        bail!("{flag} must be OWNER/REPO");
    }
    Ok(())
}

fn homebrew_platforms(resolved: &ResolvedHomebrew) -> HomebrewPlatformSet {
    HomebrewPlatformSet {
        darwin_arm: resolved.platforms.darwin_arm,
        darwin_intel: resolved.platforms.darwin_intel,
        linux_arm: resolved.platforms.linux_arm,
        linux_intel: resolved.platforms.linux_intel,
    }
}

pub(crate) fn validate_runner(value: Option<&str>) -> Result<()> {
    value.map(validate_runner_label).transpose()?;
    Ok(())
}

pub(crate) fn apply_granular_step_runners(
    step_runners: &mut BTreeMap<String, String>,
    runtime: Runtime,
) {
    if runtime == Runtime::Nix {
        step_runners
            .entry(crate::render::ci::STEP_FLAKE_CHECK.to_string())
            .or_insert_with(|| "atlas-nix-trusted".to_string());
    }
    step_runners
        .entry(crate::render::ci::STEP_CARGO_FMT.to_string())
        .or_insert_with(|| "codeberg-tiny".to_string());
    step_runners
        .entry(crate::render::ci::STEP_CARGO_CLIPPY.to_string())
        .or_insert_with(|| "codeberg-small".to_string());
    step_runners
        .entry(crate::render::ci::STEP_CARGO_TEST.to_string())
        .or_insert_with(|| "codeberg-medium".to_string());
    step_runners
        .entry(crate::render::ci::STEP_CARGO_DOC.to_string())
        .or_insert_with(|| "codeberg-small".to_string());
    step_runners
        .entry(crate::render::ci::STEP_CARGO_PACKAGE.to_string())
        .or_insert_with(|| "codeberg-medium".to_string());
    step_runners
        .entry(crate::render::ci::STEP_QUALITY_TOOLS.to_string())
        .or_insert_with(|| "codeberg-small".to_string());
}
