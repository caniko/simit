//! Package-qualified publication for explicit Python/npm release owners.
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use crate::{config::ProjectConfig, project::GeneratedFile, render::ci::GENERATED_WORKFLOW_MARKER};

use super::native_registry::Registry;

pub(super) fn files(
    root: &Path,
    config: &ProjectConfig,
    qualification: &Value,
) -> Result<Vec<GeneratedFile>> {
    let mut files = Vec::new();
    for component in &config
        .monorepo
        .as_ref()
        .expect("validated monorepo config")
        .components
    {
        for owner in &component.releases {
            let package = owner.load(root)?;
            if !package.publish {
                continue;
            }
            ensure!(
                config.release.signing.trust_root.as_str() == "keys/maintainers.gpg",
                "native publication requires keys/maintainers.gpg"
            );
            let registry = Registry::for_manifest(&owner.manifest);
            let env = json!({"NIX_CONFIG": "experimental-features = nix-command flakes", "SIMIT_NATIVE_MANIFEST": owner.manifest, "SIMIT_NATIVE_NAMESPACE": owner.namespace, "SIMIT_NATIVE_PACKAGE": package.name, "SIMIT_NATIVE_REGISTRY": registry.name()});
            let tag_check = json!({"name": "Verify signed exact-source tag", "shell": "bash", "run": "set -euo pipefail\ntag=${GITHUB_REF_NAME:?missing tag}\ncase \"$tag\" in \"$SIMIT_NATIVE_NAMESPACE\"/v[0-9]*) ;; *) echo 'unexpected release namespace' >&2; exit 1;; esac\ntest \"$(git rev-list -n 1 \"refs/tags/$tag\")\" = \"$(git rev-parse HEAD)\"\ntest \"$(git rev-parse HEAD)\" = \"${GITHUB_SHA:?missing event revision}\"\nexport GNUPGHOME=$(mktemp -d)\ntrap 'rm -rf \"$GNUPGHOME\"' EXIT\nchmod 700 \"$GNUPGHOME\"\ngpg --batch --import keys/maintainers.gpg\ngit verify-tag \"$tag\"\n"});
            let mut tag_check = tag_check;
            let script = tag_check["run"].as_str().unwrap().replace(
                "export GNUPGHOME=$(mktemp -d)",
                "GNUPGHOME=$(mktemp -d)\nexport GNUPGHOME",
            );
            tag_check["run"] = json!(format!(
                "nix develop --no-update-lock-file --max-jobs 1 --cores 2 --print-build-logs .#ci --command bash -euo pipefail <<'SIMIT_TAG_CHECK'\n{script}SIMIT_TAG_CHECK\n"
            ));
            let run = |phase| json!({"name": format!("{phase} native release"), "run": format!("nix develop --no-update-lock-file --max-jobs 1 --cores 2 --print-build-logs .#ci --command python3 -I .github/scripts/simit-native-release.py {phase}")});
            let artifact_name = format!(
                "native-release-{}-${{{{ github.run_attempt }}}}",
                owner.namespace
            );
            let action = |name, version| {
                crate::render::ci::github_action_ref(name, version)
                    .split(" #")
                    .next()
                    .unwrap()
                    .to_owned()
            };
            // Receipts attest runner-produced bytes, so archive construction is
            // part of the publication trust boundary, not configurable capacity.
            let runner = "ubuntu-24.04";
            let node24 = crate::render::ci::github_runner_supports_node24(runner);
            let mut validate_steps = vec![
                super::ci::checkout(node24),
                super::ci::install_nix(node24),
                super::ci::input_transport(),
                tag_check.clone(),
                run("pack"),
            ];
            validate_steps.push(json!({"id": "native-upload", "name": "Upload native release archive", "uses": action("actions/upload-artifact", if node24 { "v7.0.2" } else { "v4.6.2" }), "with": {"name": artifact_name, "path": "${{ runner.temp }}/native-release-dist", "if-no-files-found": "error", "retention-days": 30}}));
            let display = format!("Build {} {}", registry.name(), owner.namespace);
            let mut workflow = json!({"name": display, "on": {"push": {"tags": [format!("{}/v[0-9]*", owner.namespace)]}}, "permissions": {"contents": "read"}, "concurrency": {"group": format!("publish-native-{}-${{{{ github.ref }}}}", owner.namespace), "cancel-in-progress": false}, "jobs": {"validate": {"needs": ["qualified"], "runs-on": runner, "timeout-minutes": 30, "env": env, "steps": validate_steps}}});
            workflow["jobs"].as_object_mut().unwrap().extend(
                super::ci::full_qualification(qualification)?
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            let source_path = format!(
                ".github/workflows/publish-{}-{}.yaml",
                registry.name(),
                owner.namespace
            );
            let publisher_path = format!(
                ".github/workflows/trusted-publish-{}-{}.yaml",
                registry.name(),
                owner.namespace
            );
            crate::render::ci::validate_credential_free_workflow(
                &serde_yaml::to_string(&workflow)?,
                &Default::default(),
            )?;
            let bootstrap: Value = serde_yaml::from_str(&format!(
                "steps:\n{}",
                crate::render::ci::publisher_bootstrap()
            ))?;
            let mut publish_steps = bootstrap["steps"].as_array().unwrap().clone();
            let mut upload_env = json!({"GITHUB_TOKEN": "${{ github.token }}"});
            let environment = match registry {
                Registry::Python => {
                    upload_env["UV_PUBLISH_TOKEN"] = json!("${{ secrets.PYPI_API_TOKEN }}");
                    "simit-pypi"
                }
                Registry::Npm => {
                    upload_env["NODE_AUTH_TOKEN"] = json!("${{ secrets.NPM_TOKEN }}");
                    "simit-npm"
                }
            };
            let word = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
            let provenance = if matches!(registry, Registry::Npm) {
                let manifest: Value =
                    serde_json::from_slice(&std::fs::read(root.join(&owner.manifest))?)?;
                manifest["publishConfig"]["provenance"] == true
            } else {
                false
            };
            let provenance_arg = if provenance {
                " --native-provenance"
            } else {
                ""
            };
            publish_steps.push(json!({"name": "Re-admit original signed provider archive and upload", "env": upload_env, "run": format!(".simit-publisher-engine/target/release/simit release publisher --publisher-stage publish --repo \"$GITHUB_REPOSITORY\" --package {} --tag-namespace {} --native-manifest {} --native-registry {} --source-workflow {} --publisher-workflow {}{provenance_arg}", word(&package.name), word(&owner.namespace), word(&owner.manifest), word(registry.name()), word(&source_path), word(&publisher_path))}));
            let mut publisher = json!({"name": format!("Trusted {} Publisher {}", registry.name(), owner.namespace), "on": {"workflow_run": {"workflows": [display], "types": ["completed"]}}, "permissions": {"contents": "read", "actions": "read"}, "concurrency": {"group": format!("native-{}-${{{{ github.event.workflow_run.id }}}}", owner.namespace), "cancel-in-progress": false}, "jobs": {"publish": {"if": "github.event.workflow_run.conclusion == 'success' && github.event.workflow_run.event == 'push'", "runs-on": "ubuntu-24.04", "timeout-minutes": 30, "environment": {"name": environment}, "steps": publish_steps}}});
            if provenance {
                publisher["jobs"]["publish"]["permissions"] =
                    json!({"contents": "read", "actions": "read", "id-token": "write"});
            }
            files.push(GeneratedFile {
                relative_path: publisher_path.into(),
                content: format!("{GENERATED_WORKFLOW_MARKER}\n# Enroll the registry token only in the protected default-branch environment.\n# Recovery reuses the first immutable archive of the original signed-tag run.\n{}", serde_yaml::to_string(&publisher)?),
            });
            files.push(GeneratedFile {
                relative_path: PathBuf::from(source_path),
                content: format!(
                    "{GENERATED_WORKFLOW_MARKER}\n{}",
                    serde_yaml::to_string(&workflow)?
                ),
            });
        }
    }
    if !files.is_empty() {
        files.push(GeneratedFile {
            relative_path: ".github/scripts/simit-native-release.py".into(),
            content: include_str!("native_release.py").to_owned(),
        });
    }
    Ok(files)
}
