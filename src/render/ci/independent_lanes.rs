//! Parallel lanes separate Rust checks from optional full-flake checks.
use super::{
    CHECKOUT_COMPAT_VERSION, CHECKOUT_VERSION, CiComponent, CiOptions, NIX_FORMAT_COMMAND, Package,
    Platform, ResolvedCiRunners, Runtime, SelfCheckOptions, github_action_version,
    push_cargo_ci_steps, push_ci_branches, push_ci_workflow_name, push_docs_gate_job,
    push_docs_gate_keys, push_extra_setup_steps, push_generated_workflow_header,
    push_github_read_permissions, push_independent_source_concurrency,
    push_install_nix_step_with_cache, push_job_env, push_member_job_name,
    push_nix_cargo_bin_path_step, push_nix_cargo_cache_steps, push_nix_ci_legacy_steps,
    push_required_env_step, push_required_gate_jobs, push_required_secrets_header,
    push_sccache_stats_step, push_source_head_checkout, runs_on, trim_trailing_blank_lines,
};
use std::fmt::Write as _;

pub(super) const EVALUATION_STEP: &str = "      - name: Evaluate complete flake\n        run: nix flake check --no-build --all-systems --no-update-lock-file\n\n";
pub(super) const REALIZATION_STEP: &str = "      - name: Realize complete flake checks\n        run: nix flake check --no-update-lock-file\n\n";

pub(super) fn evaluation_enabled(runtime: Runtime, options: &CiOptions) -> bool {
    // Nix projects retain evaluation even when realization is retired.
    // Cargo projects must opt into flake checks explicitly.
    options.repository_checks
        && (runtime == Runtime::Nix || options.nix_flake_check)
        && (options.components.is_empty()
            || options.components.contains(&CiComponent::FlakeEvaluation)
            || options.components.contains(&CiComponent::Checks))
}

pub(super) fn realization_enabled(options: &CiOptions) -> bool {
    options.flake_check_command() == Some("nix flake check")
}

pub(super) fn render(
    platform: Platform,
    runtime: Runtime,
    package: &Package,
    self_check: SelfCheckOptions<'_>,
    runners: &ResolvedCiRunners,
    options: &CiOptions,
) -> String {
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    push_required_secrets_header(&mut workflow, &options.required_secrets);
    push_ci_workflow_name(&mut workflow, package, options);
    workflow.push_str("on:\n  push:\n");
    push_ci_branches(&mut workflow, &options.push_branches);
    workflow.push_str("    tags-ignore: [\"**\"]\n  pull_request:\n\n");
    push_github_read_permissions(&mut workflow, platform);
    push_independent_source_concurrency(&mut workflow);
    workflow.push_str("jobs:\n");
    let mut lanes = vec!["rust"];
    if evaluation_enabled(runtime, options) {
        lanes.push("evaluation");
    }
    if realization_enabled(options) {
        lanes.push("realization");
    }
    for lane in lanes {
        let lane_runtime = if lane == "rust" {
            runtime
        } else {
            Runtime::Nix
        };
        writeln!(workflow, "  {lane}:").expect("write");
        push_member_job_name(&mut workflow, package, options, lane);
        push_docs_gate_keys(&mut workflow, platform, &[]);
        writeln!(workflow, "    runs-on: {}", runs_on(&runners.ci)).expect("write");
        push_job_env(
            &mut workflow,
            platform,
            lane_runtime,
            &options.extra_env,
            true,
        );
        workflow.push_str("    steps:\n");
        push_source_head_checkout(&mut workflow, &runners.ci);
        push_required_env_step(&mut workflow, &options.required_env);
        if lane_runtime == Runtime::Cargo {
            push_cargo_ci_steps(
                &mut workflow,
                platform,
                package,
                self_check,
                &runners.ci,
                options,
            );
            trim_trailing_blank_lines(&mut workflow);
            workflow.push_str("\n\n");
            continue;
        }
        push_install_nix_step_with_cache(
            &mut workflow,
            platform,
            &options.ci_nix_substituters,
            &options.ci_nix_trusted_public_keys,
            options.nix_setup_action_ref.as_ref(),
        );
        if lane == "rust" {
            push_nix_cargo_cache_steps(&mut workflow, platform, &runners.ci, options, lane);
        }
        push_nix_cargo_bin_path_step(&mut workflow);
        push_extra_setup_steps(&mut workflow, &options.extra_setup);
        match lane {
            "evaluation" => workflow.push_str(EVALUATION_STEP),
            "realization" => workflow.push_str(REALIZATION_STEP),
            _ => {
                let mut rust_options = options.clone();
                rust_options.nix_flake_check = false;
                if options.repository_checks && options.check_command.is_none() {
                    write!(
                        workflow,
                        "      - name: Format check\n        run: |\n          nix develop -c {NIX_FORMAT_COMMAND}\n\n"
                    ).expect("write");
                }
                push_nix_ci_legacy_steps(
                    &mut workflow,
                    platform,
                    package,
                    self_check,
                    &rust_options,
                );
                push_sccache_stats_step(&mut workflow, platform, Runtime::Nix);
            }
        }
        trim_trailing_blank_lines(&mut workflow);
        workflow.push_str("\n\n");
    }
    push_required_gate_jobs(
        &mut workflow,
        platform,
        runtime,
        package,
        runners,
        options,
        true,
    );
    trim_trailing_blank_lines(&mut workflow);
    push_docs_gate_job(
        &mut workflow,
        platform,
        github_action_version(Some(&runners.ci), CHECKOUT_VERSION, CHECKOUT_COMPAT_VERSION),
        options.package_scoped.then_some(package.name.as_str()),
    );
    workflow
}
