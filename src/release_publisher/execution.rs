//! Independent publisher execution. Candidate Cargo configuration, wrappers,
//! hooks and build scripts are never run with the publishing credential.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::archive::{
    CrateMember, MAX_CRATE_BYTES, crate_from_provider_zip, crate_members,
    verify_reconstructed_crate,
};
use super::{
    CompletedRunEvent, PublisherContext, PublisherPolicy, ReleaseArtifact, ReleaseRun,
    bind_release_push, verify_release_artifact_archive,
};
use crate::cli::{PublisherStage, ReleaseCommand};

const MAX_PROVIDER_BYTES: u64 = 160 * 1024 * 1024;

fn env_value(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("trusted publisher requires {name}"))
}

fn package_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte)),
        "invalid publisher package name"
    );
    Ok(())
}

fn data_command(program: &str, state: &Path) -> Result<Command> {
    let mut command = Command::new(program);
    command
        .env_clear()
        .current_dir(state)
        .env("PATH", env_value("PATH")?)
        .env("HOME", state.join("home"))
        .env("CARGO_HOME", state.join("cargo-home"))
        .env("CARGO_TARGET_DIR", state.join("target"))
        .env(
            "RUSTUP_HOME",
            env::var_os("RUSTUP_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or(std::path::PathBuf::from(env_value("HOME")?).join(".rustup")),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GNUPGHOME", state.join("gpg"));
    Ok(command)
}

fn output(command: &mut Command) -> Result<Output> {
    let output = command
        .output()
        .context("execute trusted data-only release tool")?;
    // Do not echo arguments, HTTP headers, candidate output or credentials.
    ensure!(
        output.status.success(),
        "trusted release data tool failed ({})",
        output.status
    );
    Ok(output)
}

fn api_json(client: &Client, token: &str, path: &str) -> Result<Value> {
    let response = client
        .get(format!("https://api.github.com/{path}"))
        .bearer_auth(token)
        .send()?
        .error_for_status()?;
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "provider JSON exceeds byte limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn run(command: ReleaseCommand) -> Result<()> {
    ensure!(
        !command.dry_run,
        "--dry-run is not supported for the trusted publisher; use --publisher-stage prepare for credential-free verification"
    );
    let stage = command
        .publisher_stage
        .context("--publisher-stage is required")?;
    ensure!(
        command.packages.len() == 1 && !command.workspace,
        "trusted publication selects exactly one --package"
    );
    let name = &command.packages[0];
    package_name(name)?;
    let reconstruction = if command.reconstruct_packages.is_empty() {
        vec![name.clone()]
    } else {
        command.reconstruct_packages.clone()
    };
    ensure!(
        reconstruction.contains(name),
        "selected publisher crate is absent from reconstruction set"
    );
    for package in &reconstruction {
        package_name(package)?;
    }
    let repository = env_value("GITHUB_REPOSITORY")?;
    ensure!(
        command.secrets_repo.as_deref() == Some(repository.as_str()),
        "publisher --repo differs from native repository context"
    );
    let event: CompletedRunEvent =
        serde_json::from_slice(&fs::read(env_value("GITHUB_EVENT_PATH")?)?)?;
    let source_workflow = command
        .source_workflow
        .context("--source-workflow is required")?;
    let publisher_workflow = command
        .publisher_workflow
        .context("--publisher-workflow is required")?;
    let context = PublisherContext {
        event_name: env_value("GITHUB_EVENT_NAME")?,
        workflow_ref: env_value("GITHUB_WORKFLOW_REF")?,
        workflow_sha: env_value("GITHUB_WORKFLOW_SHA")?,
        git_ref: env_value("GITHUB_REF")?,
    };
    let token = env_value("GITHUB_TOKEN")?;
    // Fixed provider/registry hosts; no candidate-selected endpoint or redirect auth.
    let client = Client::builder()
        .user_agent("simit-trusted-publisher")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let native_repository = api_json(&client, &token, &format!("repos/{repository}"))?;
    let default_branch = native_repository["default_branch"]
        .as_str()
        .context("provider repository has no default branch")?
        .to_owned();
    ensure!(
        native_repository["id"].as_u64() == Some(event.repository.id),
        "provider repository identity differs from completion event"
    );
    let workflow_name = source_workflow
        .strip_prefix(".github/workflows/")
        .context("invalid source workflow path")?;
    ensure!(
        !workflow_name.contains(['/', '\\', '@']),
        "invalid source workflow filename"
    );
    let workflow = api_json(
        &client,
        &token,
        &format!("repos/{repository}/actions/workflows/{workflow_name}"),
    )?;
    ensure!(
        workflow["path"].as_str() == Some(source_workflow.as_str()),
        "native source workflow path differs from trusted policy"
    );
    let policy = PublisherPolicy {
        repository: repository.clone(),
        default_branch,
        source_workflow_path: source_workflow,
        source_workflow_id: workflow["id"].as_u64().context("missing workflow ID")?,
        publisher_workflow_path: publisher_workflow,
    };
    let provider: ReleaseRun = serde_json::from_value(api_json(
        &client,
        &token,
        &format!("repos/{repository}/actions/runs/{}", event.workflow_run.id),
    )?)?;
    let bound = bind_release_push(&policy, &context, &event, &provider)?;
    let state = TempDir::new()?;
    for directory in ["home", "cargo-home", "gpg"] {
        fs::create_dir(state.path().join(directory))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(state.path().join("gpg"), fs::Permissions::from_mode(0o700))?;
    }
    let trusted_root = env::current_dir()?;
    let trusted_head = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&trusted_root)
            .args(["rev-parse", "HEAD"]),
    )?;
    ensure!(
        String::from_utf8(trusted_head.stdout)?.trim() == context.workflow_sha,
        "publisher checkout is not the independent workflow SHA"
    );
    // The key is read only from the admitted default-branch checkout.
    let key = trusted_root.join("keys/maintainers.gpg");
    ensure!(
        fs::symlink_metadata(&key)?.file_type().is_file(),
        "trusted keyring must be a regular file"
    );
    output(
        data_command("gpg", state.path())?
            .args(["--batch", "--import"])
            .arg(key),
    )?;
    let source = state.path().join("source");
    output(data_command("git", state.path())?.arg("init").arg(&source))?;
    output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["remote", "add", "origin"])
            .arg(format!("https://github.com/{repository}.git")),
    )?;
    let mut fetch = data_command("git", state.path())?;
    // Read-only Actions authentication; never forward a crates.io token to Git.
    fetch
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
        .env(
            "GIT_CONFIG_VALUE_0",
            format!(
                "Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
            ),
        );
    output(
        fetch
            .arg("-C")
            .arg(&source)
            .args(["fetch", "--no-tags", "origin"])
            .arg(format!(
                "refs/tags/{}:refs/tags/{}",
                bound.source_head_label, bound.source_head_label
            )),
    )?;
    // A semver branch label alone is insufficient: require the independently
    // trusted maintainer signature and exact original-run commit before reading source.
    output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["verify-tag", &bound.source_head_label]),
    )?;
    let signed_sha = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["rev-parse", "--verify"])
            .arg(format!("refs/tags/{}^{{commit}}", bound.source_head_label)),
    )?;
    ensure!(
        String::from_utf8(signed_sha.stdout)?.trim() == bound.source_sha,
        "signed tag differs from original run source"
    );
    let signed_workflow = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["show"])
            .arg(format!(
                "{}:{}",
                bound.source_sha, policy.source_workflow_path
            )),
    )?;
    verify_tag_only_source_workflow(
        &signed_workflow.stdout,
        &fs::read(trusted_root.join(&policy.source_workflow_path))?,
    )?;
    output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["checkout", "--detach", &bound.source_sha]),
    )?;
    let tracked = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["ls-files", "--stage", "-z"]),
    )?;
    ensure!(
        tracked
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .all(|record| record.starts_with(b"100644 ") || record.starts_with(b"100755 ")),
        "signed packaging source contains symlinks or submodules"
    );
    validate_manifest_paths(&source)?;

    let artifact_prefix = format!("simit-crate-{name}-");
    let mut artifacts = Vec::new();
    for page in 1..=10 {
        let list = api_json(
            &client,
            &token,
            &format!(
                "repos/{repository}/actions/runs/{}/artifacts?per_page=100&page={page}",
                bound.source_run_id
            ),
        )?;
        let entries = list["artifacts"]
            .as_array()
            .context("missing native artifacts")?;
        for entry in entries {
            let artifact: ReleaseArtifact = serde_json::from_value(entry.clone())?;
            if !artifact.expired
                && artifact
                    .name
                    .strip_prefix(&artifact_prefix)
                    .and_then(|number| number.parse::<u64>().ok())
                    .is_some_and(|attempt| (1..=bound.source_run_attempt).contains(&attempt))
            {
                artifacts.push(artifact);
            }
        }
        if entries.len() < 100 {
            break;
        }
        ensure!(page < 10, "original run has too many artifacts");
    }
    // Keep the first immutable original-run archive for checksum-bound retries.
    // Artifact names do not attest attempt provenance; signed content is checked below.
    artifacts.sort_by_key(|artifact| artifact.id);
    let artifact = artifacts
        .first()
        .context("original run has no live crate artifact")?;
    let artifact: ReleaseArtifact = serde_json::from_value(api_json(
        &client,
        &token,
        &format!("repos/{repository}/actions/artifacts/{}", artifact.id),
    )?)?;
    let mut bytes = Vec::new();
    client
        .get(format!(
            "https://api.github.com/repos/{repository}/actions/artifacts/{}/zip",
            artifact.id
        ))
        .bearer_auth(&token)
        .send()?
        .error_for_status()?
        .take(MAX_PROVIDER_BYTES + 1)
        .read_to_end(&mut bytes)?;
    verify_release_artifact_archive(
        &bound,
        &artifact,
        &artifact.name,
        MAX_PROVIDER_BYTES,
        bytes.as_slice(),
    )?;
    let root = format!("{name}-{}", bound.source_head_label);
    let candidate = crate_from_provider_zip(&bytes, &format!("{root}.crate"))?;
    // Cargo configuration is discovered from cwd, not --manifest-path. The cwd
    // and HOME/CARGO_HOME are fresh; env_clear removes wrappers, credential
    // providers, aliases, tokens and injected compiler flags. --no-verify never
    // compiles or runs a candidate build script. No Nix expression is evaluated.
    output(&mut reconstruction_command(
        state.path(),
        &source,
        &reconstruction,
    )?)?;
    let reconstructed = fs::read(
        state
            .path()
            .join("target/package")
            .join(format!("{root}.crate")),
    )?;
    verify_reconstructed_crate(&candidate, &reconstructed, &root)?;
    let checksum = hex::encode(Sha256::digest(&candidate));
    let members = crate_members(&candidate, &root)?;
    let cargo_manifest = members
        .get(&format!("{root}/Cargo.toml"))
        .context("crate has no normalized manifest")?;
    let cargo_manifest: toml_edit::DocumentMut =
        std::str::from_utf8(&cargo_manifest.contents)?.parse()?;
    let metadata = upload_metadata(
        &cargo_manifest,
        &members,
        &root,
        name,
        &bound.source_head_label,
    )?;
    match stage {
        PublisherStage::Prepare => {
            ensure!(
                env::var_os("CRATES_IO_API_TOKEN").is_none(),
                "preparation must be credential-free"
            );
            let destination = command
                .publisher_output
                .context("prepare requires --publisher-output")?;
            let destination = destination.as_std_path();
            fs::create_dir(destination).context("publisher output must be a new directory")?;
            for (path, content) in members {
                let relative = path
                    .strip_prefix(&format!("{root}/"))
                    .context("invalid member root")?;
                let output = destination.join(relative);
                fs::create_dir_all(output.parent().context("member has no parent")?)?;
                write_verified_member(&output, &content)?;
            }
            println!(
                "{}",
                serde_json::to_string(
                    &json!({"sourceRun": bound, "artifact": artifact.id, "checksum": checksum, "manifest": destination.join("Cargo.toml")})
                )?
            );
        }
        PublisherStage::Publish => {
            ensure!(
                command.publisher_output.is_none(),
                "publish does not accept a candidate working directory"
            );
            let publish_token = env::var("CRATES_IO_API_TOKEN").ok();
            publish_archive(
                &client,
                publish_token.as_deref(),
                &metadata,
                &candidate,
                &checksum,
            )?;
            wait_for_registry(
                &client,
                state.path(),
                name,
                &bound.source_head_label,
                &checksum,
            )?;
            println!(
                "{}",
                serde_json::to_string(
                    &json!({"sourceRun": bound, "artifact": artifact.id, "checksum": checksum, "published": true})
                )?
            );
        }
    }
    Ok(())
}

fn write_verified_member(output: &Path, member: &CrateMember) -> Result<()> {
    fs::write(output, &member.contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(output, fs::Permissions::from_mode(member.mode))?;
    }
    Ok(())
}

fn reconstruction_command(state: &Path, source: &Path, packages: &[String]) -> Result<Command> {
    let mut cargo = data_command("cargo", state)?;
    cargo
        .args([
            "package",
            "--locked",
            "--no-verify",
            "--allow-dirty",
            "--manifest-path",
        ])
        .arg(source.join("Cargo.toml"));
    for package in packages {
        cargo.args(["--package", package]);
    }
    Ok(cargo)
}

fn string_array(item: Option<&toml_edit::Item>) -> Vec<String> {
    item.and_then(toml_edit::Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Native run head labels do not prove a tag push. The provider's exact push
/// source must contain the independently enrolled, exclusively tag-triggered
/// workflow. A candidate cannot add a semver branch trigger or dispatch route.
fn verify_tag_only_source_workflow(signed: &[u8], enrolled: &[u8]) -> Result<()> {
    let workflow: serde_yaml::Value = serde_yaml::from_slice(signed)?;
    let enrollment: serde_yaml::Value = serde_yaml::from_slice(enrolled)?;
    ensure!(
        workflow["on"] == enrollment["on"],
        "signed workflow trigger differs from independently enrolled tag-only trigger"
    );
    let triggers = workflow["on"]
        .as_mapping()
        .context("source workflow has no typed triggers")?;
    ensure!(
        triggers.len() == 1,
        "source publication must have only a tag-push trigger"
    );
    let push = triggers
        .get(serde_yaml::Value::String("push".into()))
        .and_then(serde_yaml::Value::as_mapping)
        .context("source workflow has no push trigger")?;
    ensure!(
        push.len() == 1
            && push
                .get(serde_yaml::Value::String("tags".into()))
                .and_then(serde_yaml::Value::as_sequence)
                .is_some_and(|tags| tags.len() == 1 && tags[0].as_str() == Some("[0-9]*")),
        "source publication must have only the enrolled semver tag filter"
    );
    Ok(())
}

fn manifest_string<'a>(table: &'a dyn toml_edit::TableLike, key: &str) -> Option<&'a str> {
    table.get(key).and_then(toml_edit::Item::as_str)
}

/// Cargo data-only packaging can read declared manifests, targets, readmes and
/// licenses. Keep all such reads inside the signed checkout before invoking it.
fn validate_manifest_paths(source: &Path) -> Result<()> {
    fn path_is_inside(source: &Path, parent: &Path, value: &str) -> bool {
        let path = Path::new(value);
        if path.is_absolute() || value.contains('\\') {
            return false;
        }
        let mut resolved = parent.to_path_buf();
        for component in path.components() {
            match component {
                std::path::Component::Normal(part) => resolved.push(part),
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    resolved.pop();
                }
                _ => return false,
            }
            if !resolved.starts_with(source) {
                return false;
            }
        }
        resolved.starts_with(source)
    }
    fn inspect(
        source: &Path,
        parent: &Path,
        item: &toml_edit::Item,
        section: Option<&str>,
    ) -> Result<()> {
        if let Some(table) = item.as_table_like() {
            for (key, value) in table.iter() {
                if ["path", "readme", "license-file", "build", "workspace"].contains(&key) {
                    if let Some(path) = value.as_str() {
                        ensure!(
                            path_is_inside(source, parent, path),
                            "Cargo manifest path escapes signed source"
                        );
                    }
                }
                if key == "members" || key == "exclude" || key == "include" {
                    for path in string_array(Some(value)) {
                        let pattern = path.trim_start_matches('!');
                        // Cargo package include/exclude uses gitignore-style
                        // rooted patterns, not absolute filesystem paths.
                        let pattern = if section == Some("package")
                            && (key == "include" || key == "exclude")
                        {
                            pattern.strip_prefix('/').unwrap_or(pattern)
                        } else {
                            pattern
                        };
                        ensure!(
                            path_is_inside(source, parent, pattern),
                            "Cargo manifest pattern escapes signed source"
                        );
                    }
                }
                inspect(source, parent, value, Some(key))?;
            }
        }
        if let Some(tables) = item.as_array_of_tables() {
            for table in tables {
                inspect(
                    source,
                    parent,
                    &toml_edit::Item::Table(table.clone()),
                    section,
                )?;
            }
        }
        Ok(())
    }
    let mut directories = vec![source.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() && entry.file_name() != ".git" {
                directories.push(entry.path());
            }
            if kind.is_file() && entry.file_name() == "Cargo.toml" {
                let manifest: toml_edit::DocumentMut = fs::read_to_string(entry.path())?.parse()?;
                inspect(
                    source,
                    &directory,
                    &toml_edit::Item::Table(manifest.as_table().clone()),
                    None,
                )?;
            }
        }
    }
    Ok(())
}

fn upload_metadata(
    manifest: &toml_edit::DocumentMut,
    members: &BTreeMap<String, CrateMember>,
    root: &str,
    name: &str,
    version: &str,
) -> Result<Value> {
    let package = manifest
        .get("package")
        .and_then(toml_edit::Item::as_table_like)
        .context("crate has no package table")?;
    ensure!(
        manifest_string(package, "name") == Some(name)
            && manifest_string(package, "version") == Some(version),
        "normalized crate name/version differs from trusted release selection"
    );
    let mut dependencies = Vec::new();
    let mut add_dependencies = |tables: &dyn toml_edit::TableLike,
                                target: Option<&str>|
     -> Result<()> {
        for (section, kind) in [
            ("dependencies", "normal"),
            ("build-dependencies", "build"),
            ("dev-dependencies", "dev"),
        ] {
            let Some(table) = tables.get(section).and_then(toml_edit::Item::as_table_like) else {
                continue;
            };
            for (dependency_name, item) in table.iter() {
                let dependency = item
                    .as_table_like()
                    .context("normalized dependency is not a table")?;
                let requirement = manifest_string(dependency, "version")
                    .context("normalized dependency has no version")?;
                semver::VersionReq::parse(requirement)?;
                let actual_name = manifest_string(dependency, "package").unwrap_or(dependency_name);
                let registry = manifest_string(dependency, "registry-index");
                ensure!(
                    registry.is_none()
                        || registry == Some("https://github.com/rust-lang/crates.io-index"),
                    "crate depends on an unsupported registry"
                );
                dependencies.push(json!({"name": actual_name, "version_req": requirement, "features": string_array(dependency.get("features")), "optional": dependency.get("optional").and_then(toml_edit::Item::as_bool).unwrap_or(false), "default_features": dependency.get("default-features").and_then(toml_edit::Item::as_bool).unwrap_or(true), "target": target, "kind": kind, "registry": null, "explicit_name_in_toml": (actual_name != dependency_name).then_some(dependency_name)}));
            }
        }
        Ok(())
    };
    add_dependencies(manifest.as_table(), None)?;
    if let Some(targets) = manifest
        .get("target")
        .and_then(toml_edit::Item::as_table_like)
    {
        for (target, table) in targets.iter() {
            add_dependencies(
                table.as_table_like().context("invalid target table")?,
                Some(target),
            )?;
        }
    }
    let features: BTreeMap<_, _> = manifest
        .get("features")
        .and_then(toml_edit::Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .map(|(key, value)| (key.to_owned(), string_array(Some(value))))
                .collect()
        })
        .unwrap_or_default();
    let readme_file = manifest_string(package, "readme");
    let readme = readme_file
        .map(|file| {
            members
                .get(&format!("{root}/{file}"))
                .context("declared readme is absent")
                .and_then(|member| Ok(std::str::from_utf8(&member.contents)?.to_owned()))
        })
        .transpose()?;
    Ok(json!({
        "name": name, "vers": version, "deps": dependencies, "features": features,
        "authors": string_array(package.get("authors")), "description": manifest_string(package, "description"),
        "documentation": manifest_string(package, "documentation"), "homepage": manifest_string(package, "homepage"),
        "readme": readme, "readme_file": readme_file, "keywords": string_array(package.get("keywords")),
        "categories": string_array(package.get("categories")), "license": manifest_string(package, "license"),
        "license_file": manifest_string(package, "license-file"), "repository": manifest_string(package, "repository"),
        "badges": {}, "links": manifest_string(package, "links"), "rust_version": manifest_string(package, "rust-version")
    }))
}

pub fn exact_registry_record(record: &Value, name: &str, version: &str, checksum: &str) -> bool {
    record["version"]["crate"] == name
        && record["version"]["num"] == version
        && record["version"]["yanked"] == false
        && record["version"]["checksum"] == checksum
}

fn registry_record(client: &Client, name: &str, version: &str) -> Result<Option<Value>> {
    let response = client
        .get(format!("https://crates.io/api/v1/crates/{name}/{version}"))
        .send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response.error_for_status()?;
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "registry response exceeds byte limit"
    );
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn upload_body(metadata: &Value, archive: &[u8]) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(metadata)?;
    ensure!(
        archive.len() as u64 <= MAX_CRATE_BYTES,
        "upload archive exceeds byte limit"
    );
    let mut body = Vec::with_capacity(8 + json.len() + archive.len());
    body.extend_from_slice(&u32::try_from(json.len())?.to_le_bytes());
    body.extend(json);
    body.extend_from_slice(&u32::try_from(archive.len())?.to_le_bytes());
    body.extend_from_slice(archive);
    Ok(body)
}

fn publish_archive(
    client: &Client,
    token: Option<&str>,
    metadata: &Value,
    archive: &[u8],
    checksum: &str,
) -> Result<()> {
    publish_archive_with(
        metadata,
        archive,
        checksum,
        token,
        |name, version| registry_record(client, name, version),
        |token, metadata, archive| upload_archive(client, token, metadata, archive),
    )
}

/// Shared single/member/coordinated publication decision with injectable I/O.
/// Source, signature and artifact admission precede this transport stage.
pub fn publish_archive_with(
    metadata: &Value,
    archive: &[u8],
    checksum: &str,
    token: Option<&str>,
    mut registry: impl FnMut(&str, &str) -> Result<Option<Value>>,
    mut upload: impl FnMut(&str, &Value, &[u8]) -> Result<()>,
) -> Result<()> {
    let name = metadata["name"].as_str().context("missing upload name")?;
    let version = metadata["vers"]
        .as_str()
        .context("missing upload version")?;
    ensure!(
        hex::encode(Sha256::digest(archive)) == checksum,
        "verified archive checksum changed before upload"
    );
    if let Some(record) = registry(name, version)? {
        ensure!(
            exact_registry_record(&record, name, version, checksum),
            "registry conflict, yanked crate or checksum mismatch"
        );
        return Ok(());
    }
    let token = token
        .filter(|token| !token.is_empty())
        .context("publishing credential is required for a missing registry version")?;
    upload(token, metadata, archive)
}

fn upload_archive(client: &Client, token: &str, metadata: &Value, archive: &[u8]) -> Result<()> {
    let response = client
        .put("https://crates.io/api/v1/crates/new")
        .header(reqwest::header::AUTHORIZATION, token)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(upload_body(metadata, archive)?)
        .send()?
        .error_for_status()?;
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "upload response exceeds byte limit"
    );
    let result: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        result.get("errors").is_none() && result["ok"] == true,
        "registry rejected verified archive upload"
    );
    Ok(())
}

fn wait_for_registry(
    client: &Client,
    state: &Path,
    name: &str,
    version: &str,
    checksum: &str,
) -> Result<()> {
    let probe = state.join("probe");
    fs::create_dir_all(probe.join("src"))?;
    fs::write(probe.join("src/lib.rs"), "")?;
    fs::write(
        probe.join("Cargo.toml"),
        format!(
            "[package]\nname='simit-registry-probe'\nversion='0.0.0'\nedition='2021'\n[workspace]\n[dependencies]\n{name}={{version='={version}',registry='crates-io',default-features=false}}\n"
        ),
    )?;
    wait_for_registry_with(
        name,
        version,
        checksum,
        || registry_record(client, name, version),
        || {
            let _ = fs::remove_file(probe.join("Cargo.lock"));
            let fetch = data_command("timeout", state)?
                .args([
                    "--kill-after=5s",
                    "60s",
                    "cargo",
                    "fetch",
                    "--manifest-path",
                ])
                .arg(probe.join("Cargo.toml"))
                .output()?;
            if fetch.status.success() {
                return registry_resolution_matches(
                    &fs::read_to_string(probe.join("Cargo.lock"))?,
                    name,
                    version,
                    checksum,
                );
            }
            Ok(false)
        },
        || std::thread::sleep(Duration::from_secs(30)),
    )
}

pub fn registry_resolution_matches(
    lock: &str,
    name: &str,
    version: &str,
    checksum: &str,
) -> Result<bool> {
    let lock: toml_edit::DocumentMut = lock.parse()?;
    Ok(lock["package"]
        .as_array_of_tables()
        .context("probe lock has no packages")?
        .iter()
        .any(|package| {
            package.get("name").and_then(toml_edit::Item::as_str) == Some(name)
                && package.get("version").and_then(toml_edit::Item::as_str) == Some(version)
                && package.get("source").and_then(toml_edit::Item::as_str)
                    == Some("registry+https://github.com/rust-lang/crates.io-index")
                && package.get("checksum").and_then(toml_edit::Item::as_str) == Some(checksum)
        }))
}

pub fn wait_for_registry_with(
    name: &str,
    version: &str,
    checksum: &str,
    mut registry: impl FnMut() -> Result<Option<Value>>,
    mut resolve: impl FnMut() -> Result<bool>,
    mut wait: impl FnMut(),
) -> Result<()> {
    for attempt in 0..20 {
        if let Some(record) = registry()? {
            ensure!(
                exact_registry_record(&record, name, version, checksum),
                "registry accepted a different archive or yanked version"
            );
            if resolve()? {
                return Ok(());
            }
        }
        if attempt < 19 {
            wait();
        }
    }
    bail!("registry did not resolve and fetch the exact verified archive within the bounded wait")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn credential_free_extraction_preserves_verified_modes() {
        use std::os::unix::fs::PermissionsExt;
        let destination = TempDir::new().unwrap();
        let path = destination.path().join("script");
        for mode in [0o644, 0o755, 0o600] {
            let member = CrateMember {
                contents: b"verified".to_vec(),
                mode,
            };
            write_verified_member(&path, &member).unwrap();
            assert_eq!(fs::read(&path).unwrap(), member.contents);
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
    }

    #[test]
    fn cargo_package_rooted_patterns_are_not_absolute_filesystem_reads() {
        let source = TempDir::new().unwrap();
        fs::write(
            source.path().join("Cargo.toml"),
            include_str!("../../Cargo.toml"),
        )
        .unwrap();
        validate_manifest_paths(source.path()).unwrap();
        for declaration in [
            "[package]\ninclude=['/src/**/*.rs', '!/src/generated.rs']\nexclude=['/target/**']",
            "[package]\ninclude=['/etc/passwd']",
        ] {
            fs::write(source.path().join("Cargo.toml"), declaration).unwrap();
            validate_manifest_paths(source.path()).unwrap();
        }
        for declaration in [
            "[package]\ninclude=['/../secret']",
            "[workspace]\nmembers=['/etc']",
            "[workspace]\nexclude=['/etc']",
            "[package]\nreadme='/etc/passwd'",
        ] {
            fs::write(source.path().join("Cargo.toml"), declaration).unwrap();
            assert!(
                validate_manifest_paths(source.path()).is_err(),
                "{declaration}"
            );
        }
    }

    #[test]
    fn manifest_reads_cannot_escape_the_signed_checkout() {
        let source = TempDir::new().unwrap();
        fs::create_dir(source.path().join("member")).unwrap();
        for declaration in [
            "[package]\nreadme='/etc/passwd'",
            "[package]\nlicense-file='../../secret'",
            "[dependencies]\nother={path='../../other'}",
            "[[bin]]\nname='escape'\npath='../../outside.rs'",
            "[workspace]\nmembers=['../../other']",
        ] {
            fs::write(source.path().join("member/Cargo.toml"), declaration).unwrap();
            assert!(
                validate_manifest_paths(source.path()).is_err(),
                "{declaration}"
            );
        }
        fs::write(
            source.path().join("member/Cargo.toml"),
            "[package]\nreadme='../README.md'\n[dependencies]\nother={path='../other'}",
        )
        .unwrap();
        validate_manifest_paths(source.path()).unwrap();
    }

    #[test]
    fn real_cargo_reconstruction_is_data_only_and_packages_unpublished_workspace_dependencies() {
        let state = TempDir::new().unwrap();
        for directory in [
            "home",
            "cargo-home",
            "source",
            "source/a/src",
            "source/b/src",
            "source/.cargo",
        ] {
            fs::create_dir_all(state.path().join(directory)).unwrap();
        }
        let source = state.path().join("source");
        fs::write(
            source.join("Cargo.toml"),
            "[workspace]\nmembers=['a','b']\nresolver='2'\n",
        )
        .unwrap();
        for name in ["a", "b"] {
            let dependencies = if name == "b" {
                "[dependencies]\nsimit-admission-unpublished-a={version='=0.1.0',path='../a'}\n"
            } else {
                ""
            };
            fs::write(source.join(format!("{name}/Cargo.toml")), format!("[package]\nname='simit-admission-unpublished-{name}'\nversion='0.1.0'\nedition='2021'\nlicense='MIT'\n{dependencies}")).unwrap();
            fs::write(
                source.join(format!("{name}/src/lib.rs")),
                "pub fn safe() {}\n",
            )
            .unwrap();
            fs::write(source.join(format!("{name}/build.rs")), "compile_error!(\"candidate build script must never compile during reconstruction\");\n").unwrap();
        }
        // Both declarations would fail or run candidate code if discovery used
        // the candidate cwd instead of the isolated publisher state.
        fs::write(source.join(".cargo/config.toml"), "[build]\nrustc='candidate-exfiltrator'\nrustc-wrapper='candidate-exfiltrator'\n[registry]\nglobal-credential-providers=['candidate-exfiltrator']\n").unwrap();
        fs::write(
            source.join("rust-toolchain.toml"),
            "[toolchain]\nchannel='candidate-selected-nonexistent-toolchain'\n",
        )
        .unwrap();
        validate_manifest_paths(&source).unwrap();
        output(
            data_command("cargo", state.path())
                .unwrap()
                .args(["generate-lockfile", "--offline", "--manifest-path"])
                .arg(source.join("Cargo.toml")),
        )
        .unwrap();
        let reconstruction = reconstruction_command(
            state.path(),
            &source,
            &[
                "simit-admission-unpublished-a".into(),
                "simit-admission-unpublished-b".into(),
            ],
        )
        .unwrap()
        .arg("--offline")
        .output()
        .unwrap();
        assert!(
            reconstruction.status.success(),
            "data-only packaging failed: {}",
            String::from_utf8_lossy(&reconstruction.stderr)
        );
        let root = "simit-admission-unpublished-b-0.1.0";
        let archive = fs::read(state.path().join(format!("target/package/{root}.crate"))).unwrap();
        let members = crate_members(&archive, root).unwrap();
        let lock = std::str::from_utf8(&members[&format!("{root}/Cargo.lock")].contents).unwrap();
        assert!(lock.contains("simit-admission-unpublished-a"));
        assert!(members.contains_key(&format!("{root}/build.rs")));
        let child = data_command("cargo", state.path()).unwrap();
        assert!(
            !child.get_envs().any(|(key, _)| key == "CRATES_IO_API_TOKEN"
                || key == "GITHUB_TOKEN"
                || key == "RUSTC_WRAPPER")
        );
    }

    #[test]
    fn signed_source_cannot_replace_tag_enrollment_with_a_semver_branch_or_dispatch() {
        let enrolled = b"on:\n  push:\n    tags: ['[0-9]*']\n";
        verify_tag_only_source_workflow(enrolled, enrolled).unwrap();
        for changed in [
            &b"on:\n  push:\n    branches: ['1.2.3']\n"[..],
            &b"on:\n  push:\n    tags: ['[0-9]*']\n  workflow_dispatch:\n"[..],
        ] {
            assert!(verify_tag_only_source_workflow(changed, enrolled).is_err());
            assert!(verify_tag_only_source_workflow(changed, changed).is_err());
        }
    }

    #[test]
    fn registry_acceptance_requires_exact_unyanked_checksum() {
        let original = json!({"version":{"crate":"demo","num":"1.0.0","yanked":false,"checksum":"a".repeat(64)}});
        assert!(exact_registry_record(
            &original,
            "demo",
            "1.0.0",
            &"a".repeat(64)
        ));
        for (key, value) in [
            ("crate", json!("other")),
            ("num", json!("1.0.1")),
            ("yanked", json!(true)),
            ("checksum", json!("b".repeat(64))),
        ] {
            let mut changed = original.clone();
            changed["version"][key] = value;
            assert!(!exact_registry_record(
                &changed,
                "demo",
                "1.0.0",
                &"a".repeat(64)
            ));
        }
    }

    #[test]
    fn registry_wire_body_contains_only_metadata_and_verified_crate_bytes() {
        let metadata = json!({"name":"demo","vers":"1.0.0"});
        let body = upload_body(&metadata, b"verified crate").unwrap();
        let metadata_len = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
        assert_eq!(
            serde_json::from_slice::<Value>(&body[4..4 + metadata_len]).unwrap(),
            metadata
        );
        let offset = 4 + metadata_len;
        assert_eq!(
            u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap()),
            14
        );
        assert_eq!(&body[offset + 4..], b"verified crate");
    }

    #[test]
    fn upload_metadata_preserves_renamed_optional_target_and_build_dependencies() {
        let manifest = r#"[package]
name='demo'
version='1.0.0'
license='MIT'
[dependencies.alias]
package='actual'
version='1'
optional=true
default-features=false
features=['x']
[target.'cfg(unix)'.build-dependencies.builder]
version='2'
[features]
default=['dep:alias']
"#
        .parse()
        .unwrap();
        let metadata =
            upload_metadata(&manifest, &BTreeMap::new(), "demo-1.0.0", "demo", "1.0.0").unwrap();
        assert_eq!(metadata["deps"][0]["name"], "actual");
        assert_eq!(metadata["deps"][0]["explicit_name_in_toml"], "alias");
        assert_eq!(metadata["deps"][0]["optional"], true);
        assert_eq!(metadata["deps"][0]["default_features"], false);
        assert_eq!(metadata["deps"][1]["kind"], "build");
        assert_eq!(metadata["deps"][1]["target"], "cfg(unix)");
        assert_eq!(metadata["features"]["default"], json!(["dep:alias"]));
    }
}
