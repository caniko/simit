//! Parallel lanes preserve the legacy Rust checks and full-flake realization.
use super::*;

pub(super) fn render(
    platform: Platform,
    package: &Package,
    self_check: SelfCheckOptions<'_>,
    runners: &ResolvedCiRunners,
    options: CiOptions,
) -> String {
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    push_required_secrets_header(&mut workflow, &options.required_secrets);
    push_ci_workflow_name(&mut workflow, package, &options);
    workflow.push_str(
        "on:\n  push:\n    branches: [\"**\"]\n    tags-ignore: [\"**\"]\n  pull_request:\n\n",
    );
    push_github_read_permissions(&mut workflow, platform);
    push_independent_source_concurrency(&mut workflow);
    workflow.push_str("jobs:\n");
    let mut lanes = vec!["rust", "evaluation"];
    if options.nix_flake_check {
        lanes.push("realization");
    }
    for lane in lanes {
        workflow.push_str(&format!("  {lane}:\n"));
        push_member_job_name(&mut workflow, package, &options, lane);
        workflow.push_str(&format!("    runs-on: {}\n", runs_on(&runners.ci)));
        push_job_env(
            &mut workflow,
            platform,
            Runtime::Nix,
            &options.extra_env,
            true,
        );
        workflow.push_str("    steps:\n");
        push_source_head_checkout(&mut workflow, &runners.ci);
        push_required_env_step(&mut workflow, &options.required_env);
        push_install_nix_step_with_cache(
            &mut workflow,
            platform,
            &options.nix_substituters,
            &options.nix_trusted_public_keys,
        );
        if lane == "rust" {
            push_nix_cargo_cache_steps(&mut workflow, platform, &runners.ci, &options, lane);
            push_nix_cargo_bin_path_step(&mut workflow);
        }
        push_extra_setup_steps(&mut workflow, &options.extra_setup);
        match lane {
            "evaluation" => workflow.push_str("      - name: Evaluate complete flake\n        run: nix flake check --no-build --no-update-lock-file\n\n"),
            "realization" => workflow.push_str("      - name: Realize complete flake checks\n        run: nix flake check --no-update-lock-file\n\n"),
            _ => {
                let mut rust_options = options.clone();
                rust_options.nix_flake_check = false;
                if options.check_command.is_none() {
                    workflow.push_str(&format!("      - name: Format check\n        run: |\n          nix develop -c {NIX_FORMAT_COMMAND}\n\n"));
                }
                push_nix_ci_legacy_steps(&mut workflow, platform, package, self_check, &rust_options);
                push_sccache_stats_step(&mut workflow, platform, Runtime::Nix);
            }
        }
        trim_trailing_blank_lines(&mut workflow);
        workflow.push_str("\n\n");
    }
    push_required_gate_jobs(
        &mut workflow,
        platform,
        Runtime::Nix,
        package,
        runners,
        &options,
    );
    trim_trailing_blank_lines(&mut workflow);
    workflow
}
