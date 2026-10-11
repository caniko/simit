//! Paired credential-free source builds and independent default-branch publishers.

use super::*;

// Candidate-controlled build scripts/setup run only on fresh hosted workers,
// independently of configured CI/release runners and their mounted credentials.
pub(super) const CANDIDATE_RUNNER: &str = "ubuntu-24.04";

pub(super) fn validate_credential_free_source(workflow: &str, options: &CiOptions) -> Result<()> {
    // Required gates also run against candidate source in the fresh trusted
    // verify job, including when the tag workflow does not render those gates.
    let inputs = std::iter::once(workflow).chain(options.required_gates.iter().flat_map(|gate| {
        std::iter::once(gate.run.as_str()).chain(gate.env.values().map(String::as_str))
    }));
    for input in inputs {
        for body in input.split("${{").skip(1) {
            let Some((expression, _)) = body.split_once("}}") else {
                continue;
            };
            if expression
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|word| word.eq_ignore_ascii_case("secrets"))
            {
                bail!(
                    "release source workflows must not reference GitHub secrets; keep credentials in independently protected jobs"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn crate_build_display(name: &str) -> String {
    format!("Build Crate {name}")
}

pub(super) fn archive_steps(name: &str, runtime: Runtime) -> String {
    archive_steps_with_packaging(name, runtime, true)
}

fn archive_steps_with_packaging(name: &str, runtime: Runtime, package: bool) -> String {
    let prefix = command_prefix(runtime);
    let quoted = shell_word(name);
    let packaging = if package {
        format!(
            "          {prefix}cargo package --locked --allow-dirty --no-verify -p {quoted} --target-dir \"$RUNNER_TEMP/simit-package-target\"\n"
        )
    } else {
        String::new()
    };
    let mut w = format!(
        "      - name: Build immutable archive {name}\n        run: |\n          set -euo pipefail\n          version=\"${{GITHUB_REF_NAME}}\"\n          test \"$({prefix}cargo pkgid --locked -p {quoted} | awk -F'[#@]' 'NF > 1 {{print $NF}}' | tail -n 1)\" = \"$version\"\n{packaging}          mkdir -p \"$RUNNER_TEMP/simit-crates/{name}\"\n          cp \"$RUNNER_TEMP/simit-package-target/package/{name}-$version.crate\" \"$RUNNER_TEMP/simit-crates/{name}/\"\n"
    );
    w.push_str(&format!(
        "      - name: Retain original-run archive {name}\n"
    ));
    push_action_uses(&mut w, Platform::Github, "upload-artifact", "v4.6.2");
    w.push_str(&format!("        with:\n          name: simit-crate-{name}-${{{{ github.run_attempt }}}}\n          path: ${{{{ runner.temp }}}}/simit-crates/{name}/*.crate\n          if-no-files-found: error\n          retention-days: 30\n\n"));
    w
}

pub(super) fn source_workflow(
    runtime: Runtime,
    options: &CiOptions,
    names: &[String],
    workspace: bool,
) -> String {
    let mut w = String::new();
    push_generated_workflow_header(&mut w);
    w.push_str(
        "# Credential-free signed-tag build. Rerun the original signed-tag run for recovery.\n",
    );
    w.push_str(if workspace {
        "name: Build Workspace Crates\n\n"
    } else {
        "name: Build Crate\n\n"
    });
    w.push_str(github_crate_publish_triggers());
    w.push_str("permissions:\n  contents: read\n\nconcurrency:\n  group: ${{ github.workflow_ref }}-${{ github.ref }}\n  cancel-in-progress: false\n\njobs:\n  build:\n    runs-on: ");
    w.push_str(CANDIDATE_RUNNER);
    w.push_str("\n    timeout-minutes: 60\n");
    push_job_env(&mut w, Platform::Github, runtime, &options.extra_env, true);
    w.push_str("    steps:\n");
    push_event_checkout_step(&mut w, Platform::Github);
    if runtime == Runtime::Nix {
        push_install_nix_step_with_cache(
            &mut w,
            Platform::Github,
            &options.nix_substituters,
            &options.nix_trusted_public_keys,
        );
    } else {
        push_rust_setup_step(&mut w, Platform::Github);
    }
    w.push_str(&validate_release_tag_step(None, None));
    push_required_env_step(&mut w, &options.required_env);
    push_extra_setup_steps(&mut w, &options.extra_setup);
    if options.om_ci != OmCiMode::Off {
        push_om_ci_step(&mut w, options);
    }
    push_quality_tool_install_steps(&mut w, runtime, options);
    if options.with_audit {
        push_audit_database_step(&mut w);
        push_audit_step(&mut w, runtime);
    }
    if options.with_deny {
        push_deny_step(&mut w, command_prefix(runtime));
    }
    for gate in &options.required_gates {
        w.push_str(&format!(
            "      - name: Required gate {}\n        timeout-minutes: {}\n        env:\n",
            gate.id, gate.timeout_minutes
        ));
        if gate.env.is_empty() {
            w.push_str("          SIMIT_REQUIRED_GATE: \"1\"\n");
        }
        for (key, value) in &gate.env {
            w.push_str(&format!(
                "          {key}: \"{}\"\n",
                yaml_double_quote(value)
            ));
        }
        w.push_str(&format!("        run: {}\n", gate.run));
    }
    for name in names {
        let prefix = command_prefix(runtime);
        let quoted = shell_word(name);
        let features = if options.all_features {
            " --all-features"
        } else {
            ""
        };
        w.push_str(&format!("      - name: Test {name}\n        run: {prefix}cargo test --locked -p {quoted} --all-targets{features}\n"));
        w.push_str(&format!("      - name: Clippy {name}\n        run: {prefix}cargo clippy --locked -p {quoted} --all-targets{features} -- -D warnings\n"));
        if options.with_docs {
            w.push_str(&format!("      - name: Document {name}\n        run: {prefix}cargo doc --locked -p {quoted} --no-deps --all-features\n"));
        }
    }
    // Cargo resolves not-yet-published interdependent members through its
    // temporary packaging registry when the complete release set is selected.
    let selection = names
        .iter()
        .map(|name| format!(" -p {}", shell_word(name)))
        .collect::<String>();
    w.push_str(&format!("      - name: Package coordinated release set\n        run: {}cargo package --locked --allow-dirty --no-verify{selection} --target-dir \"$RUNNER_TEMP/simit-package-target\"\n", command_prefix(runtime)));
    for name in names {
        w.push_str(&archive_steps_with_packaging(name, runtime, false));
    }
    trim_trailing_blank_lines(&mut w);
    w
}

fn bootstrap(w: &mut String) {
    w.push_str("      - name: Require independently qualified publisher engine\n        env:\n          PUBLISHER_REVISION: ${{ vars.SIMIT_PUBLISHER_REVISION }}\n        run: |\n          set -euo pipefail\n          [[ \"$PUBLISHER_REVISION\" =~ ^[0-9a-f]{40}$ ]] || { echo 'Enroll a qualified immutable Simit publisher revision' >&2; exit 1; }\n");
    w.push_str("      - name: Checkout independent publisher source\n");
    push_action_uses(w, Platform::Github, "checkout", "v4.3.1");
    w.push_str("        with:\n          ref: ${{ github.workflow_sha }}\n          persist-credentials: false\n      - name: Checkout qualified publisher engine\n");
    push_action_uses(w, Platform::Github, "checkout", "v4.3.1");
    w.push_str("        with:\n          repository: caniko/simit\n          ref: ${{ vars.SIMIT_PUBLISHER_REVISION }}\n          path: .simit-publisher-engine\n          persist-credentials: false\n");
    push_rust_setup_step(w, Platform::Github);
    w.push_str("      - name: Build trusted publisher engine\n        working-directory: .simit-publisher-engine\n        run: cargo build --release --locked\n");
}

pub(super) fn publisher_file(
    source_path: &Path,
    runtime: Runtime,
    options: &CiOptions,
    plan: &[(String, Vec<String>)],
    workspace: bool,
) -> GeneratedFile {
    let source_name = source_path.file_name().unwrap().to_str().unwrap();
    let publisher_name = format!("trusted-{source_name}");
    let publisher_path = PathBuf::from(".github/workflows").join(&publisher_name);
    let source_display = if workspace {
        "Build Workspace Crates".to_owned()
    } else {
        crate_build_display(&plan[0].0)
    };
    let mut w = String::new();
    push_generated_workflow_header(&mut w);
    w.push_str("# Enroll only after moving CRATES_IO_API_TOKEN into the literal environment below,\n# restricting it to the exact protected default branch, and qualifying the engine revision.\n# Tag-controlled workflows must have no repository/org publishing credential.\n# Recovery: rerun the original signed-tag run; matching immutable archives remain reusable.\nname: Trusted Crate Publisher\n\non:\n  workflow_run:\n    workflows: [");
    w.push_str(&format!("\"{}\"", yaml_double_quote(&source_display)));
    w.push_str("]\n    types: [completed]\n\npermissions:\n  contents: read\n  actions: read\n\nconcurrency:\n  group: simit-crates-${{ github.event.workflow_run.id }}\n  cancel-in-progress: false\n\njobs:\n");
    let mut previous: Option<String> = None;
    let reconstruction = plan
        .iter()
        .map(|(name, _)| format!(" --reconstruct-package {}", shell_word(name)))
        .collect::<String>();
    for (name, dependencies) in plan {
        // Cargo names already use GitHub's permitted job-ID alphabet. Keep
        // their spelling: collapsing '-'/'_' or case aliases distinct crates.
        let id = name;
        let verify = format!("verify-{id}");
        let publish = format!("publish-{id}");
        w.push_str(&format!("  {verify}:\n"));
        let mut needs: Vec<String> = dependencies
            .iter()
            .map(|dependency| format!("publish-{dependency}"))
            .collect();
        if let Some(previous) = &previous {
            needs.push(previous.clone());
        }
        needs.sort();
        needs.dedup();
        if !needs.is_empty() {
            w.push_str(&format!("    needs: [{}]\n", needs.join(", ")));
        }
        // Path is also checked against its native workflow ID by the engine.
        w.push_str("    if: github.event.workflow_run.conclusion == 'success' && github.event.workflow_run.event == 'push'\n");
        w.push_str("    runs-on: ");
        w.push_str(CANDIDATE_RUNNER);
        w.push_str("\n    timeout-minutes: 60\n    steps:\n");
        bootstrap(&mut w);
        if runtime == Runtime::Nix {
            push_install_nix_step_with_cache(
                &mut w,
                Platform::Github,
                &options.nix_substituters,
                &options.nix_trusted_public_keys,
            );
        }
        w.push_str(&format!("      - name: Admit signed source and provider-bound archive {name}\n        env:\n          GITHUB_TOKEN: ${{{{ github.token }}}}\n        run: |\n          set -euo pipefail\n          .simit-publisher-engine/target/release/simit release publisher --publisher-stage prepare --repo \"$GITHUB_REPOSITORY\" --package {} --source-workflow {} --publisher-workflow {}{reconstruction} --publisher-output \"$RUNNER_TEMP/verified-{id}\" > \"$RUNNER_TEMP/verified-{id}.json\"\n", shell_word(name), shell_word(&source_path.to_string_lossy()), shell_word(&publisher_path.to_string_lossy())));
        // Fresh credential-free runner: candidate build scripts may execute here.
        // Publishing always re-admits on a separate hosted runner, never trusts
        // this job's filesystem, exports, output paths or candidate binaries.
        w.push_str("      - name: Checkout original signed source for verification\n");
        push_action_uses(&mut w, Platform::Github, "checkout", "v4.3.1");
        w.push_str("        with:\n          ref: ${{ github.event.workflow_run.head_sha }}\n          path: .simit-release-source\n          persist-credentials: false\n");
        let directory = ".simit-release-source";
        let candidate_context = |w: &mut String| {
            push_candidate_context(w, runtime, &options.extra_env, directory);
        };
        // Build configuration belongs only to the admitted candidate phase,
        // never independent engine bootstrap, admission or upload.
        push_required_env_step_with_context(&mut w, &options.required_env, candidate_context);
        push_extra_setup_steps_with_context(&mut w, &options.extra_setup, candidate_context);
        let prefix = command_prefix(runtime);
        w.push_str(&format!("      - name: Verify packaged contents {name}\n"));
        candidate_context(&mut w);
        w.push_str(&format!("        run: {prefix}cargo publish --dry-run --locked --manifest-path \"$RUNNER_TEMP/verified-{id}/Cargo.toml\"\n"));
        if !options.required_gates.is_empty() {
            for gate in &options.required_gates {
                w.push_str(&format!(
                    "      - name: Required gate {}\n        timeout-minutes: {}\n",
                    gate.id, gate.timeout_minutes
                ));
                let mut environment: BTreeMap<String, String> =
                    options.extra_env.iter().cloned().collect();
                environment.extend(gate.env.clone());
                push_candidate_context(
                    &mut w,
                    runtime,
                    &environment.into_iter().collect::<Vec<_>>(),
                    directory,
                );
                w.push_str(&format!("        run: {}\n", gate.run));
            }
        }
        w.push_str(&format!("  {publish}:\n    needs: [{verify}]\n    runs-on: ubuntu-24.04\n    timeout-minutes: 60\n    environment:\n      name: simit-crates-io\n    steps:\n"));
        bootstrap(&mut w);
        w.push_str(&format!("      - name: Upload independently verified archive {name}\n        env:\n          GITHUB_TOKEN: ${{{{ github.token }}}}\n          CRATES_IO_API_TOKEN: ${{{{ secrets.CRATES_IO_API_TOKEN }}}}\n        run: .simit-publisher-engine/target/release/simit release publisher --publisher-stage publish --repo \"$GITHUB_REPOSITORY\" --package {} --source-workflow {} --publisher-workflow {}{reconstruction}\n", shell_word(name), shell_word(&source_path.to_string_lossy()), shell_word(&publisher_path.to_string_lossy())));
        previous = Some(publish);
    }
    let jobs = plan
        .iter()
        .map(|(name, _)| format!("publish-{name}"))
        .collect::<Vec<_>>();
    w.push_str(&format!("  publish-report:\n    needs: [{}]\n    if: always()\n    runs-on: ubuntu-24.04\n    steps:\n      - name: Release result\n        env:\n          RESULTS: ${{{{ toJSON(needs) }}}}\n        run: |\n          printf '%s\\n' \"$RESULTS\"\n          echo 'resume: rerun the original tag-push run; already-published crates with matching checksums exit 0 after exact registry resolution'\n          echo 'see per-crate job statuses for the auditable result'\n", jobs.join(", ")));
    trim_trailing_blank_lines(&mut w);
    GeneratedFile {
        relative_path: publisher_path,
        content: w,
    }
}

fn push_candidate_context(
    w: &mut String,
    runtime: Runtime,
    environment: &[(String, String)],
    directory: &str,
) {
    w.push_str(&format!("        working-directory: {directory}\n"));
    let mut env = String::new();
    push_job_env(&mut env, Platform::Github, runtime, environment, true);
    for line in env.lines() {
        w.push_str("    ");
        w.push_str(line);
        w.push('\n');
    }
}
