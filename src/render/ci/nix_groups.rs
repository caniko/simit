//! Opt-in Nix groups keep shared dependencies on one runner and retain each gate.
use super::*;
use crate::config::NixBuildConfig;

pub(super) fn render(
    runner: &ResolvedRunner,
    installables: &[String],
    extra_setup: &[String],
    substituters: &[String],
    trusted_public_keys: &[String],
    options: &NixBuildConfig,
) -> Result<GeneratedFile> {
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    push_required_secrets_header(&mut workflow, &options.required_secrets);
    workflow.push_str("name: Nix installable builds\n\non:\n  push:\n    branches: [\"**\"]\n");
    if !options.push_tags.is_empty() {
        workflow.push_str("    tags:\n");
        for pattern in &options.push_tags {
            workflow.push_str(&format!("      - \"{}\"\n", yaml_double_quote(pattern)));
        }
    }
    workflow
        .push_str("  pull_request:\n  workflow_dispatch:\n\npermissions:\n  contents: read\n\n");
    if options.separate_events {
        push_independent_source_concurrency(&mut workflow);
    } else {
        workflow.push_str("concurrency:\n  group: nix-groups-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.ref_type == 'tag' && github.ref || format('refs/heads/{0}', github.event.pull_request.head.ref || github.ref_name) }}\n  cancel-in-progress: true\n\n");
    }
    workflow.push_str(&format!("jobs:\n  build:\n    name: build (${{{{ matrix.group }}}})\n    runs-on: {}\n    timeout-minutes: {}\n    strategy:\n      fail-fast: false\n      max-parallel: {}\n      matrix:\n        include:\n", runs_on(runner), options.timeout_minutes.unwrap_or(40), options.max_parallel.unwrap_or(2)));
    for (name, members) in &options.groups {
        workflow.push_str(&format!(
            "          - group: \"{}\"\n            installables: {}\n",
            yaml_double_quote(name),
            yaml_json_scalar(&serde_json::to_string(members)?)
        ));
    }
    if !options.dependency_installables.is_empty() {
        // The build job consumes only a complete same-run dependency producer.
        workflow.push_str("    needs: [dependencies]\n");
    }
    workflow.push_str("    env:\n");
    if !options.extra_env.contains_key("NIX_CONFIG") {
        workflow.push_str("      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n");
    }
    for (name, value) in &options.extra_env {
        workflow.push_str(&format!("      {name}: \"{}\"\n", yaml_double_quote(value)));
    }
    workflow.push_str("    steps:\n");
    push_source_head_checkout(&mut workflow, runner);
    workflow.push_str("      - name: Prepare Nix build evidence\n        run: |\n          SIMIT_NIX_BUILD_RESULTS=\"$RUNNER_TEMP/simit-nix-build-${{ strategy.job-index }}\"\n          mkdir -p \"$SIMIT_NIX_BUILD_RESULTS\"\n          printf '%s\\n' \"SIMIT_NIX_BUILD_RESULTS=$SIMIT_NIX_BUILD_RESULTS\" >> \"$GITHUB_ENV\"\n          git rev-parse HEAD > \"$SIMIT_NIX_BUILD_RESULTS/revision\"\n");
    push_install_nix_step_with_cache(
        &mut workflow,
        Platform::Github,
        substituters,
        trusted_public_keys,
    );
    push_required_env_step(&mut workflow, &options.required_secrets);
    if options.kvm {
        workflow.push_str("      - name: Enable hosted KVM\n        run: |\n          test \"${RUNNER_ENVIRONMENT:-}\" = github-hosted\n          test -c /dev/kvm\n          sudo chmod a+rw /dev/kvm\n          test -r /dev/kvm && test -w /dev/kvm\n");
    }
    push_extra_setup_steps(&mut workflow, extra_setup);
    push_extra_setup_steps(&mut workflow, &options.extra_setup);
    push_required_env_step(&mut workflow, &options.required_env);
    nix_dependencies::import(&mut workflow, runner, options, installables)?;
    let mut limits = Vec::new();
    if let Some(value) = options.max_jobs {
        limits.extend(["--max-jobs".to_owned(), value.to_string()]);
    }
    if let Some(value) = options.cores {
        limits.extend(["--cores".to_owned(), value.to_string()]);
    }
    workflow.push_str("      - name: Build Nix group\n        shell: python\n        env:\n          INSTALLABLES: ${{ matrix.installables }}\n");
    for (key, value) in [
        ("SIMIT_NIX_PLAN", serde_json::to_string(installables)?),
        (
            "SIMIT_NIX_CHECK_SYSTEMS",
            serde_json::to_string(&options.check_systems)?,
        ),
        ("SIMIT_NIX_LIMITS", serde_json::to_string(&limits)?),
        (
            "SIMIT_NIX_FRESH",
            serde_json::to_string(&options.fresh_installables)?,
        ),
    ] {
        workflow.push_str(&format!("          {key}: {}\n", yaml_json_scalar(&value)));
    }
    workflow.push_str("        run: |\n");
    for line in include_str!("nix_group.py").lines() {
        if line.is_empty() {
            workflow.push('\n');
        } else {
            workflow.push_str(&format!("          {line}\n"));
        }
    }
    for (index, script) in options.post_build.iter().enumerate() {
        workflow.push_str(&format!(
            "      - name: Retain Nix results {}\n        run: |\n",
            index + 1
        ));
        for line in script.lines() {
            workflow.push_str(&format!("          {line}\n"));
        }
    }
    for (index, script) in options.post_build_always.iter().enumerate() {
        workflow.push_str(&format!("      - name: Retain Nix diagnostics {}\n        if: ${{{{ !cancelled() }}}}\n        run: |\n", index + 1));
        for line in script.lines() {
            workflow.push_str(&format!("          {line}\n"));
        }
    }
    workflow.push_str("      - name: Upload Nix build evidence\n        if: always()\n");
    push_action_uses(
        &mut workflow,
        Platform::Github,
        Some(runner),
        "upload-artifact",
        "v7.0.2",
        "v4.6.2",
    );
    workflow.push_str(&format!("\n        with:\n          name: nix-build-${{{{ strategy.job-index }}}}\n          retention-days: {}\n          if-no-files-found: error\n          path: |\n            ${{{{ runner.temp }}}}/simit-nix-build-${{{{ strategy.job-index }}}}\n", options.artifact_retention_days.unwrap_or(14)));
    for path in &options.artifact_paths {
        workflow.push_str(&format!("            {path}\n"));
    }
    nix_dependencies::job(
        &mut workflow,
        runner,
        extra_setup,
        substituters,
        trusted_public_keys,
        options,
        installables,
    )?;
    Ok(GeneratedFile {
        relative_path: PathBuf::from(".github/workflows/nix-builds.yaml"),
        content: workflow,
    })
}
