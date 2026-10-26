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
    // checkout or silently resolve a moving branch when the pin is absent. A
    // committed fallback serves runs without repository variables (fork PRs),
    // which already execute their own head's workflow file.
    let revision = match &options.engine_revision {
        Some(revision) => format!("(vars.SIMIT_CI_ENGINE_REVISION || '{revision}')"),
        None => "vars.SIMIT_CI_ENGINE_REVISION".to_owned(),
    };
    push_engine_cache_restore(&mut workflow, &revision);
    workflow.push_str(&format!(
        "      - name: Provision grouped-build Rust engine\n        shell: bash\n        env:\n          SIMIT_CI_ENGINE_REVISION: ${{{{ {revision} }}}}\n"
    ));
    workflow.push_str(r#"          BASH_ENV: ''
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
    push_engine_cache_save(&mut workflow, &revision);
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
    push_input_cache_restore(&mut workflow, options);
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
    push_input_cache_save(&mut workflow, options);
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

/// GitHub keeps a 10 GB repository cache; one input closure must not evict the rest.
// ponytail: fixed 4 GiB per-input ceiling; make it configurable if a real input needs more.
const INPUT_CACHE_MAX_KIB: u64 = 4 << 20;

fn engine_cache_key(revision: &str) -> String {
    format!("simit-engine-v1-${{{{ {revision} }}}}-${{{{ runner.os }}}}-${{{{ runner.arch }}}}")
}

/// Reuse the pinned engine's closure across runs instead of compiling it in
/// every group job. Provisioning still evaluates the same pin and records its
/// digest; a miss or unreadable entry is an ordinary build. Like cached inputs,
/// entries carry the trust of the repository's default-branch runs.
fn push_engine_cache_restore(workflow: &mut String, revision: &str) {
    workflow.push_str(&format!("      - name: Restore grouped-build engine\n        id: simit-engine-cache\n        if: ${{{{ {revision} != '' }}}}\n        uses: {}\n        with:\n          path: ${{{{ runner.temp }}}}/simit-engine-cache\n          key: {}\n", input_cache_action("restore"), engine_cache_key(revision)));
    workflow.push_str(r#"      - name: Import grouped-build engine
        if: ${{ steps.simit-engine-cache.outputs.cache-hit == 'true' }}
        shell: bash
        env:
          BASH_ENV: ''
          ENV: ''
          NIX_CONFIG: 'experimental-features = nix-command flakes'
          NIX_USER_CONF_FILES: /dev/null
          NIX_PATH: ''
        run: |
          set -euo pipefail
          cache_root="$RUNNER_TEMP/simit-engine-cache"
          env -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_OAUTH_TOKEN -u ATTIC_TOKEN -u CACHIX_AUTH_TOKEN -u CACHIX_SIGNING_KEY nix copy --no-check-sigs --all --from "file://$cache_root" || echo '::warning::Cached grouped-build engine failed to import; building it normally'
          rm -rf "$cache_root"
"#);
}

fn push_engine_cache_save(workflow: &mut String, revision: &str) {
    workflow.push_str(&format!("      - name: Export grouped-build engine\n        id: simit-engine-cache-save\n        if: ${{{{ {revision} != '' && steps.simit-engine-cache.outputs.cache-hit != 'true' }}}}\n"));
    workflow.push_str(r#"        shell: bash
        env:
          BASH_ENV: ''
          ENV: ''
          NIX_CONFIG: 'experimental-features = nix-command flakes'
          NIX_USER_CONF_FILES: /dev/null
          NIX_PATH: ''
        run: |
          set -euo pipefail
          cache_root="$RUNNER_TEMP/simit-engine-cache"
          rm -rf "$cache_root"
          if nix copy --to "file://$cache_root?compression=zstd" "${SIMIT_CI_ENGINE%/bin/simit}"; then
            printf 'save=true\n' >> "$GITHUB_OUTPUT"
          else
            echo '::warning::Grouped-build engine failed to export; not saving it'
          fi
"#);
    workflow.push_str(&format!("      - name: Save grouped-build engine\n        if: ${{{{ steps.simit-engine-cache-save.outputs.save == 'true' }}}}\n        uses: {}\n        with:\n          path: ${{{{ runner.temp }}}}/simit-engine-cache\n          key: {}\n", input_cache_action("save"), engine_cache_key(revision)));
}

fn input_cache_action(action: &str) -> String {
    // Restore and save are subdirectories of the pinned actions/cache commit.
    github_action_ref("actions/cache", "v6.1.0").replacen(
        "actions/cache@",
        &format!("actions/cache/{action}@"),
        1,
    )
}

fn push_input_cache_hits(workflow: &mut String, options: &NixBuildConfig) {
    for index in 0..options.cache_installables.len() {
        workflow.push_str(&format!("          SIMIT_NIX_CACHE_HIT_{index}: ${{{{ steps.simit-nix-cache-{index}.outputs.cache-hit }}}}\n"));
    }
}

/// Cross-run reuse of declared heavy inputs, keyed by their exact derivation.
/// A miss or unreadable entry is an ordinary build; a cached closure can never
/// satisfy a qualification output of the job.
fn push_input_cache_restore(workflow: &mut String, options: &NixBuildConfig) {
    if options.cache_installables.is_empty() {
        return;
    }
    workflow.push_str("      - name: Resolve cached Nix inputs\n        id: simit-nix-cache\n        shell: bash\n");
    push_producer_env(workflow, options);
    workflow.push_str(r#"        run: |
          set -euo pipefail
          cache_root="$RUNNER_TEMP/simit-nix-cache"
          mkdir -p "$cache_root"
          resolve() {
            local drv
            drv=$(nix path-info --derivation --no-update-lock-file "$2")
            [[ "$drv" =~ ^/nix/store/([0-9a-z]{32})-[^/]+\.drv$ ]] || { echo "Cached Nix input $2 must select exactly one local derivation" >&2; exit 1; }
            printf '%s\n' "$drv" > "$cache_root/$1.drv"
            printf 'key-%s=simit-nix-input-v1-%s\n' "$1" "${BASH_REMATCH[1]}" >> "$GITHUB_OUTPUT"
          }
"#);
    for (index, installable) in options.cache_installables.iter().enumerate() {
        workflow.push_str(&format!(
            "          resolve {index} {}\n",
            shell_word(installable)
        ));
    }
    for index in 0..options.cache_installables.len() {
        workflow.push_str(&format!("      - name: Restore cached Nix input {index}\n        id: simit-nix-cache-{index}\n        uses: {}\n        with:\n          path: ${{{{ runner.temp }}}}/simit-nix-cache/{index}\n          key: ${{{{ steps.simit-nix-cache.outputs.key-{index} }}}}\n", input_cache_action("restore")));
    }
    workflow.push_str("      - name: Import cached Nix inputs\n        shell: bash\n        env:\n          SIMIT_NIX_GROUP: ${{ matrix.group }}\n");
    push_input_cache_hits(workflow, options);
    push_producer_env_lines(workflow, options);
    workflow.push_str("        run: |\n          set -euo pipefail\n          cache_root=\"$RUNNER_TEMP/simit-nix-cache\"\n          case \"$SIMIT_NIX_GROUP\" in\n");
    for (name, members) in &options.groups {
        let attrs: Vec<_> = members
            .iter()
            .map(|member| shell_word(member.split_once('^').map_or(member, |(path, _)| path)))
            .collect();
        workflow.push_str(&format!(
            "            {}) members=({}) ;;\n",
            shell_word(name),
            attrs.join(" ")
        ));
    }
    workflow.push_str(r#"            *) echo 'Unknown Nix execution group' >&2; exit 1 ;;
          esac
          gate_drvs=$(nix path-info --derivation --no-update-lock-file "${members[@]}")
          gates=$(nix-store --query --outputs $gate_drvs)
          test -n "$gates"
          import_input() {
            local hit="SIMIT_NIX_CACHE_HIT_$1" outputs closure
            [ "${!hit}" = true ] || return 0
            outputs=$(nix-store --query --outputs "$(cat "$cache_root/$1.drv")")
            if ! closure=$(nix path-info --store "file://$cache_root/$1" --recursive $outputs); then
              echo "::warning::Cached Nix input $1 is unreadable; building it normally"
              rm -rf "$cache_root/$1"
              return 0
            fi
            if grep -xF -- "$gates" <<< "$closure"; then
              echo "Cached Nix input $1 contains the qualification outputs above" >&2
              exit 1
            fi
            nix copy --no-check-sigs --from "file://$cache_root/$1" $outputs || echo "::warning::Cached Nix input $1 failed to import; building it normally"
            rm -rf "$cache_root/$1"
          }
"#);
    for index in 0..options.cache_installables.len() {
        workflow.push_str(&format!("          import_input {index}\n"));
    }
}

/// Publish only inputs this job realized, including when a later gate failed.
fn push_input_cache_save(workflow: &mut String, options: &NixBuildConfig) {
    if options.cache_installables.is_empty() {
        return;
    }
    workflow.push_str("      - name: Export cached Nix inputs\n        id: simit-nix-cache-save\n        if: ${{ !cancelled() }}\n        shell: bash\n        env:\n");
    push_input_cache_hits(workflow, options);
    push_producer_env_lines(workflow, options);
    workflow.push_str(&r#"        run: |
          set -euo pipefail
          cache_root="$RUNNER_TEMP/simit-nix-cache"
          export_input() {
            local hit="SIMIT_NIX_CACHE_HIT_$1" outputs kib
            [ "${!hit}" != true ] && test -s "$cache_root/$1.drv" || return 0
            outputs=$(nix-store --query --outputs "$(cat "$cache_root/$1.drv")")
            nix-store --check-validity $outputs 2>/dev/null || return 0
            if ! nix copy --to "file://$cache_root/$1?compression=zstd" $outputs; then
              echo "::warning::Cached Nix input $1 failed to export; not saving it"
              rm -rf "$cache_root/$1"
              return 0
            fi
            # POSIX du: Darwin has no byte-granular -b.
            kib=$(du -sk "$cache_root/$1" | cut -f1)
            if [ "$kib" -gt MAX_KIB ]; then
              echo "::warning::Cached Nix input $1 closure is $kib KiB; not saving above MAX_KIB KiB"
              rm -rf "$cache_root/$1"
              return 0
            fi
            printf 'save-%s=true\n' "$1" >> "$GITHUB_OUTPUT"
          }
"#.replace("MAX_KIB", &INPUT_CACHE_MAX_KIB.to_string()));
    for index in 0..options.cache_installables.len() {
        workflow.push_str(&format!("          export_input {index}\n"));
    }
    for index in 0..options.cache_installables.len() {
        workflow.push_str(&format!("      - name: Save cached Nix input {index}\n        if: ${{{{ !cancelled() && steps.simit-nix-cache-save.outputs.save-{index} == 'true' }}}}\n        uses: {}\n        with:\n          path: ${{{{ runner.temp }}}}/simit-nix-cache/{index}\n          key: ${{{{ steps.simit-nix-cache.outputs.key-{index} }}}}\n", input_cache_action("save")));
    }
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
