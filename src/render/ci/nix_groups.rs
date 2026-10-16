//! Opt-in Nix groups keep shared dependencies on one runner and retain each gate.
use super::*;
use crate::config::NixBuildConfig;
use anyhow::ensure;

pub(super) fn render(
    runner: &ResolvedRunner,
    installables: &[String],
    extra_setup: &[String],
    substituters: &[String],
    trusted_public_keys: &[String],
    options: &NixBuildConfig,
) -> Result<GeneratedFile> {
    let native_runners = !options.system_runners.is_empty();
    if native_runners {
        for members in options.groups.values() {
            let system = options.group_system(members)?;
            ensure!(
                matches!(system, "x86_64-linux" | "aarch64-linux" | "aarch64-darwin"),
                "unsupported grouped engine runner: {system} has no immutable engine package"
            );
        }
    } else {
        let macos = runner
            .labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("macos"));
        ensure!(
            !runner.labels.iter().any(|label| {
                label.starts_with("windows-")
                    || label == "macos-13"
                    || label.starts_with("macos-")
                        && (label.ends_with("-intel") || label.ends_with("-large"))
                    || macos && (label.eq_ignore_ascii_case("x64") || label == "x86_64")
            }),
            "unsupported grouped engine runner: only x86_64-linux, aarch64-linux and aarch64-darwin have an immutable engine package"
        );
    }
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    push_required_secrets_header(&mut workflow, &options.required_secrets);
    workflow.push_str("name: Nix installable builds\n\non:\n  push:\n");
    push_ci_branches(&mut workflow, &options.push_branches);
    if !options.push_tags.is_empty() {
        workflow.push_str("    tags:\n");
        for pattern in &options.push_tags {
            workflow.push_str(&format!("      - \"{}\"\n", yaml_double_quote(pattern)));
        }
    }
    workflow
        .push_str("  pull_request:\n  workflow_dispatch:\n\npermissions:\n  contents: read\n\n");
    if options.separate_events || !options.push_branches.is_empty() {
        push_independent_source_concurrency(&mut workflow);
    } else {
        workflow.push_str("concurrency:\n  group: nix-groups-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.ref_type == 'tag' && github.ref || format('refs/heads/{0}', github.event.pull_request.head.ref || github.ref_name) }}\n  cancel-in-progress: true\n\n");
    }
    workflow.push_str(&format!(
        "jobs:\n  build:\n    name: build (${{{{ matrix.group }}}})\n    runs-on: {}\n",
        if native_runners {
            "${{ matrix.runner }}".to_owned()
        } else {
            runs_on(runner)
        }
    ));
    if let Some(timeout) = options.timeout_minutes {
        workflow.push_str(&format!("    timeout-minutes: {timeout}\n"));
    }
    workflow.push_str(&format!("    strategy:\n      fail-fast: false\n      max-parallel: {}\n      matrix:\n        include:\n", options.max_parallel.unwrap_or(2)));
    for (name, members) in &options.groups {
        workflow.push_str(&format!(
            "          - group: \"{}\"\n            installables: {}\n",
            yaml_double_quote(name),
            yaml_json_scalar(&serde_json::to_string(members)?)
        ));
        if native_runners {
            let system = options.group_system(members)?;
            workflow.push_str(&format!(
                "            system: \"{}\"\n            runner: \"{}\"\n",
                yaml_double_quote(system),
                yaml_double_quote(&options.system_runners[system]),
            ));
        }
    }
    if !options.dependency_installables.is_empty() {
        // The build job consumes only a complete same-run dependency producer.
        workflow.push_str("    needs: [dependencies]\n");
    }
    // Producer shell startup files/configuration cannot participate in bootstrap.
    workflow
        .push_str("    env:\n      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n");
    workflow.push_str("    steps:\n");
    push_source_head_checkout_with_version(
        &mut workflow,
        nix_build_action_version(runner, options, "v7.0.1", "v4.3.1"),
    );
    workflow.push_str("      - name: Prepare Nix build evidence\n        run: |\n          SIMIT_NIX_BUILD_RESULTS=\"$RUNNER_TEMP/simit-nix-build-${{ strategy.job-index }}\"\n          mkdir -p \"$SIMIT_NIX_BUILD_RESULTS\"\n          printf '%s\\n' \"SIMIT_NIX_BUILD_RESULTS=$SIMIT_NIX_BUILD_RESULTS\" >> \"$GITHUB_ENV\"\n          git rev-parse HEAD > \"$SIMIT_NIX_BUILD_RESULTS/revision\"\n");
    push_install_nix_step_with_cache(
        &mut workflow,
        Platform::Github,
        substituters,
        trusted_public_keys,
    );
    if native_runners {
        push_native_nix_runner_step_with_context(&mut workflow, |w| {
            w.push_str("          BASH_ENV: ''\n          ENV: ''\n");
            for key in ["NIX_CONFIG", "NIX_USER_CONF_FILES", "NIX_PATH"] {
                if let Some(value) = options.extra_env.get(key) {
                    w.push_str(&format!("          {key}: {}\n", yaml_json_scalar(value)));
                }
            }
        });
    }
    // Enrollment is administrator-owned. Never build the engine from the producer
    // checkout or silently resolve a moving branch when the pin is absent.
    workflow.push_str(r#"      - name: Provision grouped-build Rust engine
        shell: bash
        env:
          SIMIT_CI_ENGINE_REVISION: ${{ vars.SIMIT_CI_ENGINE_REVISION }}
          BASH_ENV: ''
          ENV: ''
          NIX_CONFIG: 'experimental-features = nix-command flakes'
          NIX_USER_CONF_FILES: /dev/null
          NIX_PATH: ''
        run: |
          [[ "$SIMIT_CI_ENGINE_REVISION" =~ ^[0-9a-f]{40}$ ]] || { echo 'Enroll an immutable SIMIT_CI_ENGINE_REVISION containing ci nix-group' >&2; exit 1; }
          case "$(uname -s):$(uname -m)" in
            Linux:x86_64|Linux:aarch64|Linux:arm64|Darwin:arm64|Darwin:aarch64) ;;
            *) echo 'unsupported grouped engine runner system' >&2; exit 1 ;;
          esac
          engine_path=$(env -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_OAUTH_TOKEN -u ATTIC_TOKEN -u CACHIX_AUTH_TOKEN -u CACHIX_SIGNING_KEY nix build --no-link --no-update-lock-file --print-out-paths --option accept-flake-config false --option access-tokens '' --option substituters https://cache.nixos.org "github:caniko/simit/$SIMIT_CI_ENGINE_REVISION#default")
          [[ "$engine_path" =~ ^/nix/store/[0-9a-z]{32}-[A-Za-z0-9+._?=-]+$ ]] || { echo 'Invalid engine output inventory' >&2; exit 1; }
          test -x "$engine_path/bin/simit"
          "$engine_path/bin/simit" ci nix-group --help > /dev/null
          printf '%s\n' "$SIMIT_CI_ENGINE_REVISION" > "$SIMIT_NIX_BUILD_RESULTS/engine-source-revision"
          engine_digest=$(nix hash file --type sha256 --base16 "$engine_path/bin/simit")
          printf '%s  %s\n' "$engine_digest" "$engine_path/bin/simit" > "$SIMIT_NIX_BUILD_RESULTS/engine-executable.sha256"
          printf '%s\n' "SIMIT_CI_ENGINE=$engine_path/bin/simit" >> "$GITHUB_ENV"
"#);
    push_required_env_step_with_context(&mut workflow, &options.required_secrets, |w| {
        push_producer_env(w, options)
    });
    if options.kvm {
        workflow.push_str("      - name: Enable hosted KVM\n");
        if native_runners {
            workflow.push_str("        if: ${{ matrix.system == 'x86_64-linux' }}\n");
        }
        workflow.push_str("        run: |\n          test \"${RUNNER_ENVIRONMENT:-}\" = github-hosted\n          test -c /dev/kvm\n          sudo chmod a+rw /dev/kvm\n          test -r /dev/kvm && test -w /dev/kvm\n");
    }
    push_extra_setup_steps_with_context(&mut workflow, extra_setup, |w| {
        push_producer_env(w, options)
    });
    push_extra_setup_steps_with_context(&mut workflow, &options.extra_setup, |w| {
        push_producer_env(w, options)
    });
    push_required_env_step_with_context(&mut workflow, &options.required_env, |w| {
        push_producer_env(w, options)
    });
    nix_dependencies::import(&mut workflow, runner, options, installables)?;
    let mut limits = Vec::new();
    if let Some(value) = options.max_jobs {
        limits.extend(["--max-jobs".to_owned(), value.to_string()]);
    }
    if let Some(value) = options.cores {
        limits.extend(["--cores".to_owned(), value.to_string()]);
    }
    workflow.push_str("      - name: Build Nix group\n        shell: bash\n        env:\n          SIMIT_NIX_GROUP: ${{ matrix.group }}\n");
    push_producer_env_lines(&mut workflow, options);
    // GitHub runs this step from a file. Keep accepted specs in that file and
    // use Bash's builtin printf, never argv/env. The full workflow has its own cap.
    workflow.push_str("        run: |\n          case \"$SIMIT_NIX_GROUP\" in\n");
    for (name, members) in &options.groups {
        let spec = crate::ci_runtime::group::GroupSpec {
            schema: "simit.nix-group-spec.v1".into(),
            members: members.clone(),
            plan: installables.to_vec(),
            check_systems: options.check_systems.clone(),
            limits: limits.clone(),
            post_build: options.post_build.clone(),
            post_build_always: options.post_build_always.clone(),
            fresh_installables: options.fresh_installables.clone(),
        };
        spec.validate()?;
        let quoted = shell_word(&serde_json::to_string(&spec)?);
        workflow.push_str(&format!("            {})\n              (umask 077; set -C; printf '%s' {quoted} > \"$SIMIT_NIX_BUILD_RESULTS/group-spec.json\")\n              ;;\n", shell_word(name)));
    }
    workflow.push_str("            *) echo 'Unknown Nix execution group' >&2; exit 1 ;;\n          esac\n          \"$SIMIT_CI_ENGINE\" ci nix-group --spec \"$SIMIT_NIX_BUILD_RESULTS/group-spec.json\"\n");
    push_diagnostics_fallback(&mut workflow, options);
    workflow.push_str("      - name: Upload Nix build evidence\n        if: always()\n");
    push_action_uses(
        &mut workflow,
        Platform::Github,
        None,
        "upload-artifact",
        nix_build_action_version(runner, options, "v7.0.2", "v4.6.2"),
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
    // Include every group spec, repeated fallback hook, YAML escape and transport
    // step in GitHub's decimal 500 KB workflow-file budget before any writes.
    ensure!(
        workflow.len() <= 500_000,
        "grouped Nix build exceeds GitHub's 500000-byte workflow limit (rendered {} bytes)",
        workflow.len()
    );
    Ok(GeneratedFile {
        relative_path: PathBuf::from(".github/workflows/nix-builds.yaml"),
        content: workflow,
    })
}

fn push_diagnostics_fallback(workflow: &mut String, options: &NixBuildConfig) {
    if options.post_build_always.is_empty() {
        return;
    }
    // The collectors must survive failed engine provisioning or admission.
    // Render static member arms so the fallback needs only the job's shell.
    workflow.push_str("      - name: Retain uncollected group diagnostics\n        if: ${{ !cancelled() }}\n        env:\n          SIMIT_NIX_GROUP: ${{ matrix.group }}\n          SIMIT_NIX_BUILD_RESULTS: ${{ runner.temp }}/simit-nix-build-${{ strategy.job-index }}\n");
    push_producer_env_lines(workflow, options);
    workflow.push_str("        run: |\n          group_root=\"$SIMIT_NIX_BUILD_RESULTS\"\n          collector_failed=0\n          case \"$SIMIT_NIX_GROUP\" in\n");
    for (name, members) in &options.groups {
        workflow.push_str(&format!("            {})\n", shell_word(name)));
        for (index, installable) in members.iter().enumerate() {
            workflow.push_str(&format!("              export INSTALLABLE={}\n              export SIMIT_NIX_BUILD_RESULTS=\"$group_root/{index}\"\n              mkdir -p \"$SIMIT_NIX_BUILD_RESULTS\"\n              if [ ! -e \"$SIMIT_NIX_BUILD_RESULTS/diagnostics-complete\" ]; then\n", shell_word(installable)));
            for (hook, script) in options.post_build_always.iter().enumerate() {
                let quoted = shell_word(script).replace('\n', "\n          ");
                let wrapper = shell_word(crate::ci_runtime::group::HOOK_FILE_COMMAND)
                    .replace('\n', "\n          ");
                workflow.push_str(&format!("                (umask 077; set -C; printf '%s' {quoted} > \"$SIMIT_NIX_BUILD_RESULTS/fallback-diagnostic-{hook}.script\")\n                status=0\n                SIMIT_NIX_HOOK_FILE=\"$SIMIT_NIX_BUILD_RESULTS/fallback-diagnostic-{hook}.script\" bash -e -o pipefail -c {wrapper} || status=$?\n                printf '%s\\n' \"$status\" > \"$SIMIT_NIX_BUILD_RESULTS/fallback-diagnostic-{hook}.status\"\n                if [ \"$status\" -ne 0 ]; then collector_failed=1; fi\n"));
            }
            workflow.push_str("                printf 'completed\\n' > \"$SIMIT_NIX_BUILD_RESULTS/diagnostics-complete\"\n              fi\n");
        }
        workflow.push_str("              ;;\n");
    }
    workflow.push_str("            *) echo 'Unknown Nix diagnostic group' >&2; exit 1 ;;\n          esac\n          exit \"$collector_failed\"\n");
}

fn push_producer_env(workflow: &mut String, options: &NixBuildConfig) {
    if !options.extra_env.is_empty() {
        workflow.push_str("        env:\n");
        push_producer_env_lines(workflow, options);
    }
}

pub(super) fn push_producer_env_lines(workflow: &mut String, options: &NixBuildConfig) {
    for (name, value) in &options.extra_env {
        workflow.push_str(&format!(
            "          {name}: \"{}\"\n",
            yaml_double_quote(value)
        ));
    }
}
