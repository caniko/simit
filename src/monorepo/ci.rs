//! Component qualification uses the same changed-path closure as local planning.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::cli::{CiProvider, InitCiCommand, Platform, Runtime, RuntimeChoice};
use crate::config::ProjectConfig;
use crate::project::GeneratedFile;
use crate::render::ci::{GENERATED_WORKFLOW_MARKER, github_action_ref, immutable_action_ref};

fn checkout() -> Value {
    json!({"uses": github_action_ref("actions/checkout", "v4.3.1").split(" #").next().unwrap(), "with": {"fetch-depth": 0, "persist-credentials": false}})
}

fn install_nix() -> Value {
    json!({"uses": immutable_action_ref("https://github.com/cachix/install-nix-action", "v31").trim_start_matches("https://github.com/").split(" #").next().unwrap()})
}

pub(crate) fn files(root: &Path, config: &ProjectConfig) -> Result<Vec<GeneratedFile>> {
    if config.ci.platform != Some(Platform::Github)
        || config.ci.provider.is_some_and(|p| p != CiProvider::Actions)
        || config.ci.runtime.is_some_and(|r| r != Runtime::Nix)
    {
        bail!("monorepo qualification requires GitHub Actions and runtime nix");
    }
    if config.ci.publish_crates
        || config.ci.with_artifacts
        || config.ci.with_pypi_publish
        || config.ci.pages.is_some()
        || config.prebuild.is_some()
        || !config.ci.nix_builds.is_empty()
        || !config.ci.required_gates.is_empty()
        || !config.ci.components.is_empty()
        || config.ci.check_command.is_some()
        || !config.ci.extra_setup.is_empty()
        || !config.ci.extra_env.is_empty()
        || config.ci.with_nextest
        || config.ci.with_nix_cargo_cache
        || config.ci.with_msrv
        || config.ci.with_audit
        || config.ci.with_deny
        || config.ci.with_docs
    {
        bail!(
            "monorepo qualification requires builds and integration gates in component checks; publication workflows must be configured independently"
        );
    }
    let graph = config
        .monorepo
        .as_ref()
        .context("missing monorepo config")?
        .resolve(root)?;
    let runner = config.ci.runner.as_deref().unwrap_or("ubuntu-latest");
    crate::user_config::validate_runner_label(runner)?;
    let mut jobs = serde_json::Map::new();
    jobs.insert("plan".into(), json!({
        "runs-on": runner,
        "timeout-minutes": 20,
        "env": {"NIX_CONFIG": "experimental-features = nix-command flakes"},
        "outputs": {"selected": "${{ steps.plan.outputs.selected }}"},
        "steps": [checkout(), install_nix(), {
            "id": "plan", "name": "Select affected components", "shell": "bash",
            "env": {"BASE_REVISION": "${{ github.event.pull_request.base.sha || github.event.before }}"},
            "run": "set -euo pipefail\nif [ -n \"$BASE_REVISION\" ] && git cat-file -e \"$BASE_REVISION^{commit}\" 2>/dev/null; then\n  nix develop .#ci --command simit monorepo plan --base \"$BASE_REVISION\" --json > plan.json\nelse\n  nix develop .#ci --command simit monorepo plan --json > plan.json\nfi\nselected=$(jq -c '.selected' plan.json)\nprintf 'selected=%s\\n' \"$selected\" >> \"$GITHUB_OUTPUT\"\ncat plan.json\n"
        }]
    }));
    let mut needs = vec!["plan".to_owned()];
    for component in graph.components.values() {
        if component.checks.is_empty() {
            bail!(
                "component {} requires at least one qualification check",
                component.id
            );
        }
        let id = format!("component-{}", component.id);
        needs.push(id.clone());
        let mut job = json!({"needs": "plan", "if": format!("${{{{ contains(fromJSON(needs.plan.outputs.selected), '{}') }}}}", component.id), "runs-on": runner, "timeout-minutes": 360, "env": {"NIX_CONFIG": "experimental-features = nix-command flakes"}});
        if !component.systems.is_empty() {
            let rows = component
                .systems
                .iter()
                .map(|system| {
                    let runner = config.ci.nix_system_runners.get(system).with_context(|| {
                        format!(
                            "component {} system {system} has no native runner",
                            component.id
                        )
                    })?;
                    crate::user_config::validate_runner_label(runner)?;
                    Ok(json!({"system": system, "runner": runner}))
                })
                .collect::<Result<Vec<_>>>()?;
            job["strategy"] =
                json!({"fail-fast": false, "max-parallel": 2, "matrix": {"include": rows}});
            job["runs-on"] = json!("${{ matrix.runner }}");
        }
        let mut steps = vec![checkout(), install_nix()];
        steps.push(json!({"name": "Verify generated workflows", "run": "nix develop .#ci --command simit init ci --check"}));
        for gate in &component.checks {
            // The project-owned command is one argument to sh, not an interpolated
            // expression; GitHub event data never enters a shell command.
            let quoted = format!("'{}'", gate.run.replace('\'', "'\\''"));
            steps.push(json!({"name": gate.id, "timeout-minutes": gate.timeout_minutes, "env": gate.env, "run": format!("nix develop .#ci --command sh -ec {quoted}")}));
        }
        job["steps"] = json!(steps);
        jobs.insert(id, job);
    }
    jobs.insert("qualified".into(), json!({
        "needs": needs, "if": "${{ always() }}", "runs-on": runner, "timeout-minutes": 5,
        "steps": [{"name": "Require every selected component", "env": {"RESULTS": "${{ toJSON(needs) }}", "SELECTED": "${{ needs.plan.outputs.selected }}"}, "run": "printf '%s' \"$RESULTS\" | jq -e --argjson selected \"$SELECTED\" '. as $jobs | $jobs.plan.result == \"success\" and all($selected[]; $jobs[\"component-\" + .].result == \"success\")'"}]
    }));
    let workflow = json!({"name": "Monorepo qualification", "on": {"push": {}, "pull_request": {}, "workflow_dispatch": {}}, "permissions": {"contents": "read"}, "concurrency": {"group": "monorepo-${{ github.event.pull_request.number || github.ref }}", "cancel-in-progress": true}, "jobs": jobs});
    Ok(vec![GeneratedFile {
        relative_path: PathBuf::from(".github/workflows/ci.yaml"),
        content: format!(
            "{GENERATED_WORKFLOW_MARKER}\n{}",
            serde_yaml::to_string(&workflow)?
        ),
    }])
}

pub(crate) fn resolve_config(root: &Path, command: &InitCiCommand) -> Result<ProjectConfig> {
    if command.diff && !command.check {
        bail!("--diff requires --check");
    }
    if command.runtime == Some(RuntimeChoice::Cargo)
        || !command.packages.is_empty()
        || command.workspace
        || command.workspace_strategy.is_some()
        || command.crow_format.is_some()
        || command.windows_runner.is_some()
        || !command.step_runner.is_empty()
        || command.granular
        || command.maintainer_key.is_some()
        || command.maintainers_gpg.is_some()
        || command.release_smoke_command.is_some()
        || command.with_nextest.is_some()
        || command.with_nix_cargo_cache.is_some()
        || command.nix_flake_check.is_some()
        || command.with_msrv.is_some()
        || command.with_audit.is_some()
        || command.with_deny.is_some()
        || command.with_docs.is_some()
        || command.with_om_ci.is_some()
        || command.om_ci_augment.is_some()
        || command.omnix_ref.is_some()
        || command.with_artifacts.is_some()
        || command.with_pypi_publish.is_some()
        || command.publish_crates.is_some()
        || command.coordinated_publish.is_some()
        || command.with_homebrew
        || command.with_chocolatey
        || command.with_scoop
        || command.with_codeberg_pages
        || command.with_vscode
        || command.with_jetbrains
    {
        bail!(
            "monorepo CI uses component checks in a root Nix CI shell; language and release overrides belong in component configuration"
        );
    }
    let mut config = ProjectConfig::load(root)?;
    if let Some(platform) = command.platform {
        config.ci.platform = Some(platform);
    }
    if let Some(provider) = command.ci_provider {
        config.ci.provider = Some(provider);
    }
    if let Some(runner) = &command.runner {
        config.ci.runner = Some(runner.clone());
    }
    if command.runtime == Some(RuntimeChoice::Nix) {
        config.ci.runtime = Some(Runtime::Nix);
    }
    Ok(config)
}
