//! Same-run dependency transport never substitutes qualification execution.
use super::{
    NixSetupActionRef, Platform, ResolvedRunner, Result, StepCondition, nix_groups,
    push_action_uses, push_checkout_step, push_docs_gate_keys, push_extra_setup_steps,
    push_hosted_kvm_step, push_install_nix_step_with_cache, push_required_env_step, runs_on,
    yaml_double_quote, yaml_json_scalar,
};
use crate::config::NixBuildConfig;
use std::fmt::Write as _;

fn program(
    workflow: &mut String,
    mode: &str,
    options: &NixBuildConfig,
    plan: &[String],
) -> Result<()> {
    let name = if mode == "export" {
        "Build and export dependency closure"
    } else {
        "Verify and import dependency closure"
    };
    write!(
        workflow,
        "      - name: {name}\n        shell: python\n        env:\n          SIMIT_DEPENDENCY_MODE: {mode}\n          SIMIT_DEPENDENCY_ROOT: ${{{{ runner.temp }}}}/simit-nix-dependencies\n          SIMIT_DEPENDENCY_MAX_BYTES: {}\n",
        options.dependency_max_mib.unwrap_or(2048) * 1024 * 1024
    ).expect("write");
    nix_groups::push_producer_env_lines(workflow, options);
    let mut limits = Vec::new();
    if let Some(value) = options.max_jobs {
        limits.extend(["--max-jobs".to_owned(), value.to_string()]);
    }
    if let Some(value) = options.cores {
        limits.extend(["--cores".to_owned(), value.to_string()]);
    }
    for (name, value) in [
        (
            "SIMIT_NIX_DEPENDENCIES",
            serde_json::to_string(&options.dependency_installables)?,
        ),
        ("SIMIT_NIX_PLAN", serde_json::to_string(plan)?),
        (
            "SIMIT_NIX_CHECK_SYSTEMS",
            serde_json::to_string(&options.check_systems)?,
        ),
        ("SIMIT_NIX_LIMITS", serde_json::to_string(&limits)?),
    ] {
        writeln!(workflow, "          {name}: {}", yaml_json_scalar(&value)).expect("write");
    }
    workflow.push_str("        run: |\n");
    for line in include_str!("nix_dependencies.py").lines() {
        if line.is_empty() {
            workflow.push('\n');
        } else {
            writeln!(workflow, "          {line}").expect("write");
        }
    }
    Ok(())
}

pub(super) fn import(
    workflow: &mut String,
    runner: &ResolvedRunner,
    options: &NixBuildConfig,
    plan: &[String],
) -> Result<()> {
    if options.dependency_installables.is_empty() {
        return Ok(());
    }
    workflow.push_str(
        "      - name: Download same-run dependencies\n        id: simit-dependency-download\n",
    );
    push_action_uses(
        workflow,
        Platform::Github,
        Some(runner),
        "download-artifact",
        "v8.0.2",
        "v4.3.0",
    );
    workflow.push_str("\n        with:\n          name: simit-nix-dependencies-${{ github.run_id }}-${{ github.run_attempt }}\n          path: ${{ runner.temp }}/simit-nix-dependencies\n");
    write!(
        workflow,
        "      - name: Explain missing attempt-bound dependencies\n        if: {}\n",
        StepCondition::DependencyDownloadFailed.wrapped()
    )
    .expect("write");
    workflow.push_str("        run: |\n          echo 'Dependency transport requires a producer from this exact run attempt. Use Re-run all jobs; partial reruns cannot reuse a previous-attempt artifact.' >&2\n          exit 1\n");
    provision_python(workflow, runner);
    program(workflow, "import", options, plan)
}

fn provision_python(workflow: &mut String, runner: &ResolvedRunner) {
    workflow.push_str("      - name: Provision dependency-transport Python\n");
    push_action_uses(
        workflow,
        Platform::Github,
        Some(runner),
        "setup-python",
        "v6.3.0",
        "v5.6.0",
    );
    workflow.push_str("\n        with:\n          python-version: '3.12'\n");
}

#[allow(
    clippy::too_many_arguments,
    reason = "Cache settings, dependency plan, and setup policy are resolved by the caller"
)]
pub(super) fn job(
    workflow: &mut String,
    runner: &ResolvedRunner,
    extra_setup: &[String],
    substituters: &[String],
    keys: &[String],
    options: &NixBuildConfig,
    plan: &[String],
    nix_setup: Option<&NixSetupActionRef>,
) -> Result<()> {
    if options.dependency_installables.is_empty() {
        return Ok(());
    }
    write!(
        workflow,
        "\n  dependencies:\n    runs-on: {}\n",
        runs_on(runner)
    )
    .expect("write");
    push_docs_gate_keys(workflow, Platform::Github, &[]);
    if let Some(timeout) = options.timeout_minutes {
        writeln!(workflow, "    timeout-minutes: {timeout}").expect("write");
    }
    workflow.push_str("    env:\n");
    if !options.extra_env.contains_key("NIX_CONFIG") {
        workflow.push_str("      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n");
    }
    for (name, value) in &options.extra_env {
        writeln!(workflow, "      {name}: \"{}\"", yaml_double_quote(value)).expect("write");
    }
    workflow.push_str("    steps:\n");
    push_checkout_step(workflow, Platform::Github, runner);
    workflow.push_str("        with:\n          repository: ${{ github.event.pull_request.head.repo.full_name || github.repository }}\n          ref: ${{ github.event.pull_request.head.sha || github.sha }}\n          persist-credentials: false\n");
    push_install_nix_step_with_cache(workflow, Platform::Github, substituters, keys, nix_setup);
    push_required_env_step(workflow, &options.required_secrets);
    provision_python(workflow, runner);
    if options.kvm {
        push_hosted_kvm_step(workflow, false);
    }
    push_extra_setup_steps(workflow, extra_setup);
    push_extra_setup_steps(workflow, &options.extra_setup);
    push_required_env_step(workflow, &options.required_env);
    program(workflow, "export", options, plan)?;
    write!(
        workflow,
        "      - name: Upload dependency evidence and closure\n        if: {}\n",
        StepCondition::Always.expression()
    )
    .expect("write");
    push_action_uses(
        workflow,
        Platform::Github,
        Some(runner),
        "upload-artifact",
        "v7.0.2",
        "v4.6.2",
    );
    write!(
        workflow,
        "\n        with:\n          name: simit-nix-dependencies-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}\n          retention-days: {}\n          compression-level: 0\n          if-no-files-found: error\n          path: ${{{{ runner.temp }}}}/simit-nix-dependencies\n",
        options.artifact_retention_days.unwrap_or(14)
    ).expect("write");
    Ok(())
}
