//! Component qualification uses the same changed-path closure as local planning.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::cli::{CiProvider, InitCiCommand, Platform, Runtime, RuntimeChoice};
use crate::config::ProjectConfig;
use crate::project::GeneratedFile;
use crate::render::ci::{GENERATED_WORKFLOW_MARKER, github_action_ref, immutable_action_ref};

pub(super) fn checkout(node24: bool) -> Value {
    let version = if node24 { "v7.0.1" } else { "v4.3.1" };
    json!({"uses": github_action_ref("actions/checkout", version).split(" #").next().unwrap(), "with": {"fetch-depth": 0, "persist-credentials": false}})
}

pub(super) fn install_nix(node24: bool) -> Value {
    let version = if node24 { "v31.11.1" } else { "v31" };
    json!({"uses": immutable_action_ref("https://github.com/cachix/install-nix-action", version).trim_start_matches("https://github.com/").split(" #").next().unwrap()})
}

pub(super) fn input_transport() -> Value {
    // Imported public component locks may retain their original operator SSH
    // transport. Reuse exact revisions over HTTPS without deployment keys.
    let run = format!(
        "{}git config --global --add url.https://codeberg.org/.insteadOf ssh://git@codeberg.org/\ngit config --global --add url.https://codeberg.org/.insteadOf git@codeberg.org:\n",
        crate::render::ci::GITHUB_INPUT_TRANSPORT
    );
    json!({"name": "Configure public input transport", "run": run})
}

pub(crate) fn files(
    root: &Path,
    config: &ProjectConfig,
    include_publishers: bool,
) -> Result<Vec<GeneratedFile>> {
    if config.ci.platform != Some(Platform::Github)
        || config.ci.provider.is_some_and(|p| p != CiProvider::Actions)
        || config.ci.runtime.is_some_and(|r| r != Runtime::Nix)
    {
        bail!("monorepo qualification requires GitHub Actions and runtime nix");
    }
    if include_publishers && (config.vscode.is_some() || config.jetbrains.is_some()) {
        bail!(
            "monorepo extension publication is not supported; manage extension publishers independently and use --primary-only for qualification regeneration"
        );
    }
    if config.ci.with_artifacts
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
    let node24 = crate::render::ci::github_runner_supports_node24(runner);
    let scope = if include_publishers {
        ""
    } else {
        " --primary-only"
    };
    let drift_check = json!({"name": "Verify generated workflows", "run": format!("nix develop --print-build-logs .#ci --command simit init ci --check{scope}")});
    let flake_path = root.join("flake.nix");
    // Evaluated policy retains native worker validation; static policy has one owner.
    let evaluated_policy =
        flake_path.is_file() && crate::config::flake_declares_simit_config(&flake_path)?;
    let mut plan_steps = vec![checkout(node24), install_nix(node24), input_transport()];
    if !evaluated_policy {
        plan_steps.push(drift_check.clone());
    }
    plan_steps.push(json!({
        "id": "plan", "name": "Select affected components", "shell": "bash",
        "env": {"BASE_REVISION": "${{ github.event.pull_request.base.sha || github.event.before }}"},
        "run": "set -euo pipefail\nif [ -n \"$BASE_REVISION\" ] && git cat-file -e \"$BASE_REVISION^{commit}\" 2>/dev/null; then\n  nix develop --print-build-logs .#ci --command simit monorepo plan --base \"$BASE_REVISION\" --json > plan.json\nelse\n  nix develop --print-build-logs .#ci --command simit monorepo plan --json > plan.json\nfi\nselected=$(jq -c '.selected' plan.json)\nprintf 'selected=%s\\n' \"$selected\" >> \"$GITHUB_OUTPUT\"\ncat plan.json\n"
    }));
    let mut jobs = serde_json::Map::new();
    jobs.insert(
        "plan".into(),
        json!({
            "runs-on": runner,
            "timeout-minutes": 20,
            "env": {"NIX_CONFIG": "experimental-features = nix-command flakes"},
            "outputs": {"selected": "${{ steps.plan.outputs.selected }}"},
            "steps": plan_steps
        }),
    );
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
        let node24 = if component.systems.is_empty() {
            node24
        } else {
            component.systems.iter().all(|system| {
                config
                    .ci
                    .nix_system_runners
                    .get(system)
                    .is_some_and(|runner| crate::render::ci::github_runner_supports_node24(runner))
            })
        };
        let mut steps = vec![checkout(node24), install_nix(node24), input_transport()];
        if !component.systems.is_empty() {
            steps.push(json!({"name": "Verify runner system", "shell":"bash", "env":{"EXPECTED_NIX_SYSTEM":"${{ matrix.system }}"}, "run":"set -euo pipefail\nactual_system=$(nix eval --impure --raw --expr builtins.currentSystem)\nif [ \"$actual_system\" != \"$EXPECTED_NIX_SYSTEM\" ]; then\n  echo \"Native Nix runner mismatch: expected $EXPECTED_NIX_SYSTEM, got $actual_system\" >&2\n  exit 1\nfi\n"}));
        }
        if evaluated_policy {
            steps.push(drift_check.clone());
        }
        for gate in &component.checks {
            // The project-owned command is one argument to Bash, not an interpolated
            // expression; GitHub event data never enters a shell command.
            let quoted = format!("'{}'", gate.run.replace('\'', "'\\''"));
            let assignments = gate
                .env
                .iter()
                .map(|(key, value)| {
                    format!("'{}'", format!("{key}={value}").replace('\'', "'\\''"))
                })
                .collect::<Vec<_>>()
                .join(" ");
            let env = if assignments.is_empty() {
                String::new()
            } else {
                format!("env {assignments} ")
            };
            let mut step = json!({"name": gate.id, "timeout-minutes": gate.timeout_minutes, "run": format!("nix develop --print-build-logs .#ci --command {env}bash -euo pipefail -c {quoted}")});
            if !gate.env.is_empty() {
                step["env"] = json!(gate.env);
            }
            steps.push(step);
        }
        job["steps"] = json!(steps);
        jobs.insert(id, job);
    }
    jobs.insert("qualified".into(), json!({
        "needs": needs, "if": "${{ always() }}", "runs-on": runner, "timeout-minutes": 5,
        "steps": [{"name": "Require every selected component", "env": {"RESULTS": "${{ toJSON(needs) }}", "SELECTED": "${{ needs.plan.outputs.selected }}"}, "run": "printf '%s' \"$RESULTS\" | jq -e --argjson selected \"$SELECTED\" '. as $jobs | $jobs.plan.result == \"success\" and all($selected[]; $jobs[\"component-\" + .].result == \"success\")'"}]
    }));
    let workflow = json!({"name": "Monorepo qualification", "on": {"push": {"branches": ["**"]}, "pull_request": {}, "workflow_dispatch": {}}, "permissions": {"contents": "read"}, "concurrency": {"group": "monorepo-${{ github.event.pull_request.head.repo.full_name || github.repository }}-${{ github.event.pull_request.head.ref || github.ref_name }}", "cancel-in-progress": true}, "jobs": jobs});
    let mut files = vec![GeneratedFile {
        relative_path: PathBuf::from(".github/workflows/ci.yaml"),
        content: format!(
            "{GENERATED_WORKFLOW_MARKER}\n{}",
            serde_yaml::to_string(&workflow)?
        ),
    }];
    if !include_publishers {
        return Ok(files);
    }
    let native = super::native_ci::files(root, config, &workflow)?;
    let has_native = !native.is_empty();
    files.extend(native);
    if config.ci.publish_crates {
        let metadata = crate::cargo::cargo_metadata(&root.join("Cargo.toml"))?;
        if config.release.signing.trust_root.as_str() != "keys/maintainers.gpg" {
            bail!(
                "independent Cargo publication currently requires the keys/maintainers.gpg trust root"
            );
        }
        for package in metadata.packages.iter().filter(|p| {
            metadata.workspace_members.contains(&p.id) && p.is_publishable_to_crates_io()
        }) {
            let plan = crate::commands::release_plan::build_release_plan(
                &metadata,
                std::slice::from_ref(&package.name),
            )?;
            if let Some(dependency) = plan
                .entries
                .iter()
                .find(|entry| !entry.package.is_publishable_to_crates_io())
            {
                bail!(
                    "package {} has a local dependency {} not authorized for crates.io",
                    package.name,
                    dependency.package.name
                );
            }
            let options = crate::render::ci::CiOptions {
                publish_crates: true,
                ..Default::default()
            };
            let mut generated = crate::render::ci::publish_independent_files(package, &options)?;
            let file = generated
                .first_mut()
                .context("independent publication requires a source workflow")?;
            let mut release: Value = serde_yaml::from_str(&file.content)?;
            release["permissions"] = json!({"contents": "read"});
            // Qualify the entire graph, including non-Cargo checks, before
            // constructing the archive for the independent publisher.
            let qualification = full_qualification(&workflow)?;
            require_hosted_qualification(&qualification)?;
            release["jobs"]
                .as_object_mut()
                .context("release jobs must be a mapping")?
                .extend(
                    qualification
                        .as_object()
                        .context("qualification jobs must be a mapping")?
                        .clone(),
                );
            release["jobs"]["build"]["needs"] = json!(["qualified"]);
            file.content = format!(
                "{GENERATED_WORKFLOW_MARKER}\n{}",
                serde_yaml::to_string(&release)?
            );
            let validation_options = crate::render::ci::CiOptions {
                required_gates: graph
                    .components
                    .values()
                    .flat_map(|component| component.checks.iter().cloned())
                    .collect(),
                ..Default::default()
            };
            crate::render::ci::validate_credential_free_workflow(
                &file.content,
                &validation_options,
            )?;
            files.extend(generated);
        }
    }
    if config.ci.publish_crates || has_native {
        files.push(crate::release_trust::generated_file(
            root,
            config,
            &Default::default(),
            true,
        )?);
    }
    Ok(files)
}

pub(super) fn full_qualification(workflow: &Value) -> Result<Value> {
    let mut jobs = workflow["jobs"].clone();
    let plan = jobs["plan"]["steps"]
        .as_array_mut()
        .context("qualification plan requires steps")?
        .iter_mut()
        .find(|step| step["id"] == "plan")
        .context("qualification plan step is missing")?;
    plan["env"]["BASE_REVISION"] = json!("");
    Ok(jobs)
}

fn require_hosted_qualification(jobs: &Value) -> Result<()> {
    for job in jobs
        .as_object()
        .context("qualification jobs must be a mapping")?
        .values()
    {
        let runner = job["runs-on"].as_str().context(
            "release qualification requires a literal hosted runner or fixed runner matrix",
        )?;
        if runner == "${{ matrix.runner }}" {
            let matrix = job["strategy"]["matrix"]["include"]
                .as_array()
                .context("release qualification requires an explicit runner matrix")?;
            if matrix.is_empty()
                || !matrix.iter().all(|entry| {
                    entry["runner"]
                        .as_str()
                        .is_some_and(crate::render::ci::github_runner_supports_node24)
                })
            {
                bail!("release qualification requires recognized GitHub-hosted runners");
            }
        } else if !crate::render::ci::github_runner_supports_node24(runner) {
            bail!("release qualification requires recognized GitHub-hosted runners");
        }
    }
    Ok(())
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
    if let Some(publish) = command.publish_crates {
        config.ci.publish_crates = publish;
    }
    Ok(config)
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn primary_scope_does_not_generate_or_enroll_native_publication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("python")).unwrap();
        std::fs::write(
            root.join("python/pyproject.toml"),
            "[project]\nname = 'python-owner'\nversion = '1.2.3'\n",
        )
        .unwrap();
        std::fs::write(root.join("simit.toml"), "[ci]\nplatform = 'github'\nruntime = 'nix'\n[monorepo]\nschema_version = 1\n[[monorepo.components]]\nid = 'python'\npaths = ['python']\nchecks = [{id = 'test', run = 'true'}]\nreleases = [{manifest = 'python/pyproject.toml', namespace = 'python-owner', publish = true}]\n").unwrap();
        let config = ProjectConfig::load(root).unwrap();
        let primary = files(root, &config, false).unwrap();
        assert_eq!(primary.len(), 1);
        assert_eq!(
            primary[0].relative_path,
            PathBuf::from(".github/workflows/ci.yaml")
        );
        assert!(!primary[0].content.contains("secrets."));
        assert!(!root.join("keys").exists());
    }
}
