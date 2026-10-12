//! Same-run dependency transport never substitutes qualification execution.
use super::*;
use crate::config::NixBuildConfig;

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
    workflow.push_str(&format!("      - name: {name}\n        shell: python\n        env:\n          SIMIT_DEPENDENCY_MODE: {mode}\n          SIMIT_DEPENDENCY_ROOT: ${{{{ runner.temp }}}}/simit-nix-dependencies\n          SIMIT_DEPENDENCY_MAX_BYTES: {}\n", options.dependency_max_mib.unwrap_or(2048) * 1024 * 1024));
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
        workflow.push_str(&format!("          {name}: {}\n", yaml_json_scalar(&value)));
    }
    workflow.push_str("        run: |\n");
    for line in include_str!("nix_dependencies.py").lines() {
        if line.is_empty() {
            workflow.push('\n');
        } else {
            workflow.push_str(&format!("          {line}\n"));
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
    workflow.push_str("      - name: Download same-run dependencies\n");
    push_action_uses(
        workflow,
        Platform::Github,
        Some(runner),
        "download-artifact",
        "v8.0.2",
        "v4.3.0",
    );
    workflow.push_str("\n        with:\n          name: simit-nix-dependencies-${{ github.run_id }}-${{ github.run_attempt }}\n          path: ${{ runner.temp }}/simit-nix-dependencies\n");
    program(workflow, "import", options, plan)
}

pub(super) fn job(
    workflow: &mut String,
    runner: &ResolvedRunner,
    extra_setup: &[String],
    substituters: &[String],
    keys: &[String],
    options: &NixBuildConfig,
    plan: &[String],
) -> Result<()> {
    if options.dependency_installables.is_empty() {
        return Ok(());
    }
    workflow.push_str(&format!("\n  dependencies:\n    runs-on: {}\n    timeout-minutes: {}\n    env:\n      NIX_CONFIG: \"experimental-features = nix-command flakes\"\n", runs_on(runner), options.timeout_minutes.unwrap_or(40)));
    for (name, value) in &options.extra_env {
        if name != "NIX_CONFIG" {
            workflow.push_str(&format!("      {name}: \"{}\"\n", yaml_double_quote(value)));
        }
    }
    workflow.push_str("    steps:\n");
    push_checkout_step(workflow, Platform::Github, runner);
    workflow.push_str("        with:\n          repository: ${{ github.event.pull_request.head.repo.full_name || github.repository }}\n          ref: ${{ github.event.pull_request.head.sha || github.sha }}\n          persist-credentials: false\n");
    push_install_nix_step_with_cache(workflow, Platform::Github, substituters, keys);
    push_required_env_step(workflow, &options.required_secrets);
    push_extra_setup_steps(workflow, extra_setup);
    push_extra_setup_steps(workflow, &options.extra_setup);
    push_required_env_step(workflow, &options.required_env);
    program(workflow, "export", options, plan)?;
    workflow
        .push_str("      - name: Upload dependency evidence and closure\n        if: always()\n");
    push_action_uses(
        workflow,
        Platform::Github,
        Some(runner),
        "upload-artifact",
        "v7.0.2",
        "v4.6.2",
    );
    workflow.push_str(&format!("\n        with:\n          name: simit-nix-dependencies-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}\n          retention-days: {}\n          compression-level: 0\n          if-no-files-found: error\n          path: ${{{{ runner.temp }}}}/simit-nix-dependencies\n", options.artifact_retention_days.unwrap_or(14)));
    Ok(())
}
