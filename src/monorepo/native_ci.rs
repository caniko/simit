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
            let artifact_name = format!("native-release-{}", owner.namespace);
            let action = |name, version| {
                crate::render::ci::github_action_ref(name, version)
                    .split(" #")
                    .next()
                    .unwrap()
                    .to_owned()
            };
            let mut validate_steps = vec![
                super::ci::checkout(),
                super::ci::install_nix(),
                super::ci::input_transport(),
                tag_check.clone(),
                run("pack"),
            ];
            validate_steps.push(json!({"uses": action("actions/upload-artifact", "v4.6.2"), "with": {"name": artifact_name, "path": "${{ runner.temp }}/native-release-dist", "if-no-files-found": "error", "retention-days": 7}}));
            let mut publish_steps = vec![
                super::ci::checkout(),
                super::ci::install_nix(),
                super::ci::input_transport(),
                tag_check,
            ];
            publish_steps.push(json!({"uses": action("actions/download-artifact", "v4.3.0"), "with": {"name": artifact_name, "path": "${{ runner.temp }}/native-release-dist"}}));
            let mut publish = run("publish");
            publish["env"] = match registry {
                Registry::Python => json!({"UV_PUBLISH_TOKEN": "${{ secrets.PYPI_API_TOKEN }}"}),
                Registry::Npm => json!({"NODE_AUTH_TOKEN": "${{ secrets.NPM_TOKEN }}"}),
            };
            publish_steps.push(publish);
            let runner = config.ci.runner.as_deref().unwrap_or("ubuntu-latest");
            let mut workflow = json!({"name": format!("Publish {} {}", registry.name(), owner.namespace), "on": {"push": {"tags": [format!("{}/v[0-9]*", owner.namespace)]}}, "permissions": {"contents": "read"}, "concurrency": {"group": format!("publish-native-{}-${{{{ github.ref }}}}", owner.namespace), "cancel-in-progress": false}, "jobs": {"validate": {"needs": ["qualified"], "runs-on": runner, "timeout-minutes": 30, "env": env, "steps": validate_steps}, "publish": {"needs": ["validate"], "runs-on": runner, "timeout-minutes": 20, "env": env, "steps": publish_steps}}});
            workflow["jobs"].as_object_mut().unwrap().extend(
                super::ci::full_qualification(qualification)?
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            if matches!(registry, Registry::Npm) {
                let manifest: Value =
                    serde_json::from_str(&std::fs::read_to_string(root.join(&owner.manifest))?)?;
                if manifest["publishConfig"]["provenance"] == true {
                    workflow["jobs"]["publish"]["permissions"] =
                        json!({"contents": "read", "id-token": "write"});
                }
            }
            files.push(GeneratedFile {
                relative_path: PathBuf::from(format!(
                    ".github/workflows/publish-{}-{}.yaml",
                    registry.name(),
                    owner.namespace
                )),
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
