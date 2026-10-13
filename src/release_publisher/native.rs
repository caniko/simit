//! Native archives cross the same protected provider/source boundary as Cargo.
//! Only engine-owned data parsing and HTTP upload run with registry credentials.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{BoundReleasePush, ReleaseArtifact};
use crate::cli::{PublisherStage, ReleaseCommand};

// Engine-owned signing using npm's Sigstore bundle contract. This program reads
// a provider-bound statement on stdin, never package archives/configuration.
const PROVENANCE_SIGNER: &str = r#"
const fs = require('node:fs');
const path = require('node:path');
const root = process.argv[2];
if (!path.isAbsolute(root)) throw new Error('npm module root must be absolute');
const sigstore = require(path.join(root, 'npm/node_modules/sigstore'));
const statement = JSON.parse(fs.readFileSync(0, 'utf8'));
sigstore.attest(Buffer.from(JSON.stringify(statement)), 'application/vnd.in-toto+json')
  .then(bundle => process.stdout.write(JSON.stringify(bundle)))
  .catch(error => { console.error(error.message); process.exitCode = 1; });
"#;

fn provider_members(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(
        (2..=3).contains(&zip.len()),
        "native artifact requires receipt and exact package inventory"
    );
    let mut members = BTreeMap::new();
    let mut total = 0;
    for index in 0..zip.len() {
        let member = zip.by_index(index)?;
        let name = member.name().to_owned();
        ensure!(
            Path::new(&name).file_name().and_then(|n| n.to_str()) == Some(name.as_str())
                && !name.contains(['/', '\\'])
                && !name.bytes().any(|b| b.is_ascii_control())
                && member.is_file()
                && member
                    .unix_mode()
                    .is_none_or(|mode| mode & 0o170000 == 0o100000),
            "invalid native provider member"
        );
        let limit = if name == "receipt.json" {
            64 * 1024
        } else {
            128 * 1024 * 1024
        };
        let mut data = Vec::new();
        member.take(limit + 1).read_to_end(&mut data)?;
        total += data.len();
        ensure!(
            data.len() as u64 <= limit
                && total <= 256 * 1024 * 1024
                && members.insert(name, data).is_none(),
            "oversized or duplicate native artifact member"
        );
    }
    ensure!(
        members.contains_key("receipt.json"),
        "native artifact has no receipt"
    );
    Ok(members)
}

pub(super) fn run(
    command: &ReleaseCommand,
    bound: &BoundReleasePush,
    source: &Path,
    links: &BTreeSet<PathBuf>,
    bytes: &[u8],
    artifact: &ReleaseArtifact,
    state: &Path,
) -> Result<()> {
    ensure!(
        command.publisher_stage == Some(PublisherStage::Publish)
            && command.publisher_output.is_none()
            && command.reconstruct_packages.is_empty(),
        "native publisher uploads only re-admitted original archives"
    );
    let manifest = command
        .native_manifest
        .as_deref()
        .context("missing native manifest")?;
    let registry = command
        .native_registry
        .as_deref()
        .context("missing native registry")?;
    let namespace = command
        .tag_namespace
        .as_deref()
        .context("missing native namespace")?;
    ensure!(
        Path::new(manifest)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
            && !manifest.contains('\\'),
        "invalid enrolled native manifest path"
    );
    let package = enrolled_package(source, links, manifest, namespace)?;
    ensure!(
        package.publish
            && package.name == command.packages[0]
            && package.version.to_string() == bound.release_version(),
        "signed native manifest differs from protected package/version enrollment"
    );
    let data: Value = if registry == "python" {
        ensure!(
            manifest.ends_with("/pyproject.toml") || manifest == "pyproject.toml",
            "native registry/manifest mismatch"
        );
        let document: Value = toml_edit::de::from_str(&fs::read_to_string(source.join(manifest))?)?;
        document["project"].clone()
    } else {
        ensure!(
            registry == "npm"
                && (manifest.ends_with("/package.json") || manifest == "package.json"),
            "native registry/manifest mismatch"
        );
        serde_json::from_slice(&fs::read(source.join(manifest))?)?
    };
    let artifact_version = crate::monorepo::native_registry::Registry::for_manifest(manifest)
        .version(&package.version)?;
    let identity = json!({"schemaVersion": 2, "registry": registry, "name": package.name, "version": artifact_version, "namespace": namespace, "tag": bound.source_head_label(), "source": bound.source_sha(), "repository": bound.repository(), "workflow": bound.source_workflow_path(), "workflowId": bound.source_workflow_id(), "runId": bound.source_run_id()});
    ensure!(
        data["publishConfig"]["provenance"].is_null()
            || data["publishConfig"]["provenance"].is_boolean(),
        "signed npm provenance policy must be boolean"
    );
    ensure!(
        data["publishConfig"]["provenance"]
            .as_bool()
            .unwrap_or(false)
            == command.native_provenance,
        "signed npm provenance policy differs from protected enrollment"
    );
    let members = provider_members(bytes)?;
    let dist = state.join("native-dist");
    fs::create_dir(&dist)?;
    for (name, data) in members {
        fs::write(dist.join(name), data)?;
    }
    let script = state.join("native-upload.py");
    fs::write(&script, include_str!("../monorepo/native_release.py"))?;
    fs::write(state.join("native-provenance.cjs"), PROVENANCE_SIGNER)?;
    let request = state.join("native-admission.json");
    fs::write(
        &request,
        serde_json::to_vec(
            &json!({"identity": identity, "manifest": data, "maxAttempt": bound.source_run_attempt(), "artifactAttempt": artifact.name.rsplit('-').next().context("missing artifact attempt")?.parse::<u64>()?}),
        )?,
    )?;
    let token_name = if registry == "python" {
        "UV_PUBLISH_TOKEN"
    } else {
        "NODE_AUTH_TOKEN"
    };
    let mut upload = Command::new("python3");
    upload
        .env_clear()
        .current_dir(state)
        .env(
            "PATH",
            std::env::var_os("PATH").context("missing trusted PATH")?,
        )
        .env("HOME", state.join("home"))
        .args(["-I"])
        .arg(&script)
        .arg("admitted-publish")
        .arg(&request)
        .arg(&dist)
        .stdin(Stdio::null());
    if let Some(token) = std::env::var_os(token_name) {
        upload.env(token_name, token);
    }
    if command.native_provenance {
        for name in [
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "GITHUB_WORKFLOW_REF",
            "GITHUB_WORKFLOW_SHA",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_ATTEMPT",
            "GITHUB_REPOSITORY_ID",
            "GITHUB_REPOSITORY_OWNER_ID",
        ] {
            upload.env(
                name,
                std::env::var_os(name)
                    .with_context(|| format!("protected provenance requires {name}"))?,
            );
        }
    }
    ensure!(
        upload.status()?.success(),
        "native upload failed; retain original run and immutable archive"
    );
    Ok(())
}

fn enrolled_package(
    source: &Path,
    links: &BTreeSet<PathBuf>,
    manifest: &str,
    namespace: &str,
) -> Result<crate::monorepo::releases::Package> {
    ensure!(
        !links.contains(&source.join(manifest)),
        "native identity cannot depend on candidate symlinks"
    );
    // Do not use ProjectConfig::load here: flake-backed configuration evaluates
    // candidate Nix with the ambient publisher credential. The protected argv
    // already supplies the enrollment. Parse only its exact signed manifest.
    crate::monorepo::releases::ManifestRelease {
        manifest: manifest.to_owned(),
        namespace: namespace.to_owned(),
        publish: true,
    }
    .load(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enrolled_manifest_admission_never_loads_candidate_config_keys_scripts_or_flake() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("python")).unwrap();
        fs::write(
            root.join("python/pyproject.toml"),
            "[project]\nname = 'python-owner'\nversion = '1.2.3'\n",
        )
        .unwrap();
        fs::write(
            root.join("simit.toml"),
            "invalid candidate config must never be parsed",
        )
        .unwrap();
        fs::write(
            root.join("flake.nix"),
            "{ outputs = _: { simitConfig = builtins.getEnv \"UV_PUBLISH_TOKEN\"; }; }",
        )
        .unwrap();
        fs::create_dir(root.join("keys")).unwrap();
        fs::write(root.join("keys/maintainers.gpg"), "candidate keyring").unwrap();
        fs::create_dir(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/publish.py"),
            "raise RuntimeError('candidate script')",
        )
        .unwrap();
        let package = enrolled_package(
            root,
            &BTreeSet::new(),
            "python/pyproject.toml",
            "python-owner",
        )
        .unwrap();
        assert_eq!(package.name, "python-owner");
        assert_eq!(package.version.to_string(), "1.2.3");
        assert!(
            enrolled_package(
                root,
                &BTreeSet::from([root.join("python/pyproject.toml")]),
                "python/pyproject.toml",
                "python-owner"
            )
            .is_err()
        );
    }
    #[test]
    fn provider_native_inventory_rejects_paths_links_duplicates_and_extra_members() {
        use std::io::Write;
        for name in ["../escape", "/escape", "nested/entry", "receipt.json"] {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("receipt.json", options).unwrap();
            zip.write_all(b"{}").unwrap();
            // Duplicate names are rejected by zip itself, so prove a missing payload instead.
            if name != "receipt.json" {
                zip.start_file(name, options).unwrap();
                zip.write_all(b"data").unwrap();
            }
            assert!(provider_members(&zip.finish().unwrap().into_inner()).is_err());
        }
    }
}
