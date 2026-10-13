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
    bind_independent_release_push, bind_native_release_push, bind_prefixed_release_push,
    verify_release_artifact_archive,
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
        // Recreate only the fixed trusted transport policy. Never inherit
        // candidate/global Git config, SSH agents or provider credentials.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "url.https://github.com/.insteadOf")
        .env("GIT_CONFIG_VALUE_0", "ssh://git@github.com/")
        .env("GIT_CONFIG_KEY_1", "url.https://github.com/.insteadOf")
        .env("GIT_CONFIG_VALUE_1", "git@github.com:")
        .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true")
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
    let native = command.native_manifest.is_some();
    if !native {
        package_name(name)?;
    }
    let namespace = command.tag_namespace.as_deref();
    ensure!(
        native || namespace.is_none_or(|package| package == name),
        "publisher tag namespace must equal the selected crate"
    );
    let reconstruction = if command.reconstruct_packages.is_empty() {
        vec![name.clone()]
    } else {
        command.reconstruct_packages.clone()
    };
    ensure!(
        reconstruction.contains(name),
        "selected publisher crate is absent from reconstruction set"
    );
    if !native {
        for package in &reconstruction {
            package_name(package)?;
        }
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
        .clone()
        .context("--source-workflow is required")?;
    let publisher_workflow = command
        .publisher_workflow
        .clone()
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
    let tag_prefix = if command.publisher_tag_prefix == "v" {
        crate::release_identity::TagPrefix::V
    } else {
        crate::release_identity::TagPrefix::None
    };
    let bound = if native {
        bind_native_release_push(
            &policy,
            &context,
            &event,
            &provider,
            namespace.context("native publisher requires namespace")?,
        )?
    } else if let Some(package) = namespace {
        bind_independent_release_push(&policy, &context, &event, &provider, package)?
    } else {
        bind_prefixed_release_push(&policy, &context, &event, &provider, tag_prefix)?
    };
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
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_CONFIG_KEY_2", "http.https://github.com/.extraheader")
        .env(
            "GIT_CONFIG_VALUE_2",
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
    let enrolled_workflow = fs::read(trusted_root.join(&policy.source_workflow_path))?;
    if native {
        verify_native_builder(&enrolled_workflow)?;
    }
    if namespace.is_some() || tag_prefix == crate::release_identity::TagPrefix::V {
        verify_tag_only_source_workflow_with_prefix(
            &signed_workflow.stdout,
            &enrolled_workflow,
            namespace,
            tag_prefix,
        )?;
    } else {
        verify_tag_only_source_workflow(&signed_workflow.stdout, &enrolled_workflow)?;
    }
    checkout_signed_source(state.path(), &source, &bound.source_sha)?;
    let tracked = output(
        data_command("git", state.path())?
            .arg("-C")
            .arg(&source)
            .args(["ls-files", "--stage", "-z"]),
    )?;
    let inert_links = super::source::validate_tree(&source, &tracked.stdout)?;
    let artifact_prefix = if native {
        format!(
            "native-release-{}-",
            namespace.context("native publication requires --tag-namespace")?
        )
    } else {
        validate_manifest_paths_with_links(&source, &inert_links)?;
        format!("simit-crate-{name}-")
    };
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
    ensure!(
        artifact
            .name
            .strip_prefix(&artifact_prefix)
            .and_then(|attempt| attempt.parse::<u64>().ok())
            .is_some_and(|attempt| (1..=bound.source_run_attempt).contains(&attempt)),
        "refetched artifact differs from enrolled original-run identity"
    );
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
    if native {
        return super::native::run(
            &command,
            &bound,
            &source,
            &inert_links,
            &bytes,
            &artifact,
            state.path(),
        );
    }
    let root = format!("{name}-{}", bound.release_version);
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
    super::source::reject_link_members(&members, &root, &source, &inert_links)?;
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
        &bound.release_version,
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
                &bound.release_version,
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

fn checkout_signed_source(state: &Path, source: &Path, revision: &str) -> Result<()> {
    // Represent signed symlink blobs as inert target-text files. Cargo may not
    // consume them as manifests, declared inputs or archive members below.
    output(data_command("git", state)?.arg("-C").arg(source).args([
        "config",
        "core.symlinks",
        "false",
    ]))?;
    output(
        data_command("git", state)?
            .arg("-C")
            .arg(source)
            .args(["checkout", "--detach", revision]),
    )?;
    Ok(())
}

fn reconstruction_command(state: &Path, source: &Path, packages: &[String]) -> Result<Command> {
    ensure!(
        state.is_absolute() && source.is_absolute(),
        "reconstruction requires absolute isolated paths"
    );
    let mut cargo = data_command("cargo", state)?;
    // Cargo searches .cargo/config in cwd and every ancestor, even with an
    // explicit manifest path. A Nix /build ancestor can replace crates.io with
    // a vendor directory, hiding coordinated unpublished workspace packages.
    // Use a neutral filesystem-root cwd; all source/state paths stay explicit
    // and HOME/CARGO_HOME remain private. Candidate configuration stays unread.
    cargo.current_dir(
        state
            .ancestors()
            .last()
            .context("publisher state has no root")?,
    );
    cargo
        .args([
            "package",
            "--locked",
            "--no-verify",
            "--allow-dirty",
            "--registry",
            "crates-io",
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
/// workflow definition. Binding only triggers would let the signed source add
/// credential authority, private runners or alternate jobs under those triggers.
fn verify_tag_only_source_workflow(signed: &[u8], enrolled: &[u8]) -> Result<()> {
    verify_tag_only_source_workflow_with_namespace(signed, enrolled, None)
}

fn verify_tag_only_source_workflow_with_namespace(
    signed: &[u8],
    enrolled: &[u8],
    namespace: Option<&str>,
) -> Result<()> {
    verify_tag_only_source_workflow_with_prefix(signed, enrolled, namespace, Default::default())
}

fn verify_tag_only_source_workflow_with_prefix(
    signed: &[u8],
    enrolled: &[u8],
    namespace: Option<&str>,
    prefix: crate::release_identity::TagPrefix,
) -> Result<()> {
    let tag_pattern = if let Some(package) = namespace {
        package_name(package)?;
        format!("{package}/v[0-9]*")
    } else {
        format!("{}[0-9]*", prefix.as_str())
    };
    let workflow: serde_yaml::Value = serde_yaml::from_slice(signed)?;
    let enrollment: serde_yaml::Value = serde_yaml::from_slice(enrolled)?;
    ensure!(
        workflow["on"] == enrollment["on"],
        "signed workflow trigger differs from independently enrolled tag-only trigger"
    );
    ensure!(
        workflow == enrollment,
        "signed workflow definition differs from independently enrolled credential-free workflow"
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
                .is_some_and(
                    |tags| tags.len() == 1 && tags[0].as_str() == Some(tag_pattern.as_str())
                ),
        "source publication must have only the enrolled semver tag filter"
    );
    Ok(())
}

fn verify_native_builder(enrolled: &[u8]) -> Result<()> {
    let workflow: serde_yaml::Value = serde_yaml::from_slice(enrolled)?;
    ensure!(
        workflow["jobs"]["validate"]["runs-on"].as_str() == Some("ubuntu-24.04"),
        "native archives require the fixed hosted ubuntu-24.04 builder"
    );
    Ok(())
}

fn manifest_string<'a>(table: &'a dyn toml_edit::TableLike, key: &str) -> Option<&'a str> {
    table.get(key).and_then(toml_edit::Item::as_str)
}

/// Cargo data-only packaging can read declared manifests, targets, readmes and
/// licenses. Keep all such reads inside the signed checkout before invoking it.
#[cfg(test)]
fn validate_manifest_paths(source: &Path) -> Result<()> {
    validate_manifest_paths_with_links(source, &std::collections::BTreeSet::new())
}

fn validate_manifest_paths_with_links(
    source: &Path,
    links: &std::collections::BTreeSet<std::path::PathBuf>,
) -> Result<()> {
    fn path_is_inside(
        source: &Path,
        parent: &Path,
        value: &str,
        links: &std::collections::BTreeSet<std::path::PathBuf>,
    ) -> bool {
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
        resolved.starts_with(source) && !links.contains(&resolved)
    }
    fn inspect(
        source: &Path,
        parent: &Path,
        item: &toml_edit::Item,
        section: Option<&str>,
        links: &std::collections::BTreeSet<std::path::PathBuf>,
    ) -> Result<()> {
        if let Some(table) = item.as_table_like() {
            for (key, value) in table.iter() {
                // Cargo explicitly ignores free-form package/workspace
                // metadata. Dependency crates named metadata still retain
                // their real filesystem-path checks.
                if key == "metadata" && matches!(section, Some("package" | "workspace")) {
                    continue;
                }
                if ["path", "readme", "license-file", "build", "workspace"].contains(&key) {
                    if let Some(path) = value.as_str() {
                        ensure!(
                            path_is_inside(source, parent, path, links),
                            "Cargo manifest path escapes signed source or depends on a Git symlink"
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
                            path_is_inside(source, parent, pattern, links),
                            "Cargo manifest pattern escapes signed source or depends on a Git symlink"
                        );
                    }
                }
                inspect(source, parent, value, Some(key), links)?;
            }
        }
        if let Some(tables) = item.as_array_of_tables() {
            for table in tables {
                inspect(
                    source,
                    parent,
                    &toml_edit::Item::Table(table.clone()),
                    section,
                    links,
                )?;
            }
        }
        Ok(())
    }
    fn dependency_manifests(
        table: &dyn toml_edit::TableLike,
        parent: &Path,
        pending: &mut Vec<std::path::PathBuf>,
    ) {
        for section in [
            "dependencies",
            "build-dependencies",
            "dev-dependencies",
            "replace",
        ] {
            if let Some(dependencies) = table.get(section).and_then(toml_edit::Item::as_table_like)
            {
                for (_, dependency) in dependencies.iter() {
                    if let Some(path) = dependency
                        .as_table_like()
                        .and_then(|dependency| manifest_string(dependency, "path"))
                    {
                        pending.push(parent.join(path).join("Cargo.toml"));
                    }
                }
            }
        }
        for section in ["target", "patch"] {
            if let Some(groups) = table.get(section).and_then(toml_edit::Item::as_table_like) {
                for (_, group) in groups.iter() {
                    if let Some(group) = group.as_table_like() {
                        if section == "target" {
                            dependency_manifests(group, parent, pending);
                        } else {
                            for (_, dependency) in group.iter() {
                                if let Some(path) = dependency
                                    .as_table_like()
                                    .and_then(|dependency| manifest_string(dependency, "path"))
                                {
                                    pending.push(parent.join(path).join("Cargo.toml"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut pending = vec![source.join("Cargo.toml")];
    ensure!(
        !links.contains(&source.join("Cargo.lock")),
        "Cargo lock depends on a signed Git symlink"
    );
    let mut visited = std::collections::BTreeSet::new();
    while let Some(path) = pending.pop() {
        // Missing references remain Cargo's packaging error. Never parse an
        // unrelated fixture just because it has a Cargo.toml filename.
        if !path.is_file() {
            continue;
        }
        let path = fs::canonicalize(path)?;
        ensure!(
            path.starts_with(source),
            "Cargo manifest path escapes signed source"
        );
        ensure!(
            !links.contains(&path),
            "Cargo manifest depends on a signed Git symlink"
        );
        if !visited.insert(path.clone()) {
            continue;
        }
        let parent = path.parent().context("manifest has no parent")?;
        let manifest: toml_edit::DocumentMut = fs::read_to_string(&path)?.parse()?;
        inspect(
            source,
            parent,
            &toml_edit::Item::Table(manifest.as_table().clone()),
            None,
            links,
        )?;
        dependency_manifests(manifest.as_table(), parent, &mut pending);
        if let Some(package) = manifest
            .get("package")
            .and_then(toml_edit::Item::as_table_like)
        {
            if let Some(workspace) = manifest_string(package, "workspace") {
                pending.push(parent.join(workspace).join("Cargo.toml"));
            }
        }
        if let Some(workspace) = manifest
            .get("workspace")
            .and_then(toml_edit::Item::as_table_like)
        {
            dependency_manifests(workspace, parent, &mut pending);
            let excluded = string_array(workspace.get("exclude"))
                .into_iter()
                .map(|pattern| glob::Pattern::new(&pattern))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for member in string_array(workspace.get("members")) {
                let pattern = format!(
                    "{}/{}",
                    glob::Pattern::escape(&parent.to_string_lossy()),
                    member
                );
                for member in glob::glob(&pattern)? {
                    let member = member?;
                    let relative = member
                        .strip_prefix(parent)
                        .context("workspace member escapes root")?;
                    if !excluded
                        .iter()
                        .any(|pattern| pattern.matches_path(relative))
                    {
                        pending.push(member.join("Cargo.toml"));
                    }
                }
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
    let authorized = package.get("publish").is_none_or(|policy| {
        policy.as_bool() == Some(true)
            || policy.as_array().is_some_and(|registries| {
                registries
                    .iter()
                    .all(|registry| registry.as_str().is_some())
                    && registries
                        .iter()
                        .any(|registry| registry.as_str() == Some("crates-io"))
            })
    });
    ensure!(
        authorized,
        "signed package.publish does not authorize publication to crates.io"
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
    registry: impl FnMut(&str, &str) -> Result<Option<Value>>,
    upload: impl FnMut(&str, &Value, &[u8]) -> Result<()>,
) -> Result<()> {
    publish_archive_with_reconciliation(
        metadata,
        archive,
        checksum,
        token,
        registry,
        upload,
        || std::thread::sleep(Duration::from_secs(30)),
    )
}

fn publish_archive_with_reconciliation(
    metadata: &Value,
    archive: &[u8],
    checksum: &str,
    token: Option<&str>,
    mut registry: impl FnMut(&str, &str) -> Result<Option<Value>>,
    mut upload: impl FnMut(&str, &Value, &[u8]) -> Result<()>,
    mut wait: impl FnMut(),
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
    if let Err(upload_error) = upload(token, metadata, archive) {
        // An upload may have committed even when its response is lost or
        // unreadable. Never repeat it in this invocation: reconcile only the
        // exact immutable registry identity, then let the caller's independent
        // Cargo-resolution gate prove that the archive is actually usable.
        for attempt in 0..20 {
            if let Ok(Some(record)) = registry(name, version) {
                ensure!(
                    exact_registry_record(&record, name, version, checksum),
                    "registry conflict, yanked crate or checksum mismatch after uncertain upload"
                );
                return Ok(());
            }
            if attempt < 19 {
                wait();
            }
        }
        return Err(upload_error.context("uncertain upload did not reconcile within the bounded wait; retain the original run and archive"));
    }
    Ok(())
}

fn upload_archive(client: &Client, token: &str, metadata: &Value, archive: &[u8]) -> Result<()> {
    upload_archive_at(
        client,
        "https://crates.io/api/v1/crates/new",
        token,
        metadata,
        archive,
    )
}

fn upload_archive_at(
    client: &Client,
    endpoint: &str,
    token: &str,
    metadata: &Value,
    archive: &[u8],
) -> Result<()> {
    let response = client
        .put(endpoint)
        .header(reqwest::header::AUTHORIZATION, token)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::ACCEPT, "application/json")
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
        result.is_object()
            && (result.get("errors").is_none_or(Value::is_null)
                || result
                    .get("errors")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)),
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
    #[test]
    fn native_archive_builder_is_independently_hosted() {
        for runner in [
            "ubuntu-24.04",
            "self-hosted",
            "private-builder",
            "${{ inputs.runner }}",
        ] {
            let workflow = format!("jobs:\n  validate:\n    runs-on: '{runner}'\n");
            assert_eq!(
                super::verify_native_builder(workflow.as_bytes()).is_ok(),
                runner == "ubuntu-24.04"
            );
        }
        assert!(
            super::verify_native_builder(
                b"jobs: {validate: {runs-on: [self-hosted, ubuntu-24.04]}}"
            )
            .is_err()
        );
    }
    use super::*;

    #[test]
    #[cfg(unix)]
    fn data_only_checkout_keeps_hook_links_inert_and_git_source_clean() {
        let state = TempDir::new().unwrap();
        let fixture = state.path().join("fixture");
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("init")
                .arg(&fixture),
        )
        .unwrap();
        fs::write(fixture.join("README.md"), "regular source").unwrap();
        fs::create_dir(fixture.join("src")).unwrap();
        fs::write(fixture.join("src/lib.rs"), "pub fn safe() {}\n").unwrap();
        fs::write(fixture.join("Cargo.toml"), "[package]\nname='simit-inert-source-fixture'\nversion='1.0.0'\nedition='2021'\nlicense='MIT'\ninclude=['src/**','README.md']\n[workspace]\n").unwrap();
        output(
            data_command("cargo", state.path())
                .unwrap()
                .args(["generate-lockfile", "--offline", "--manifest-path"])
                .arg(fixture.join("Cargo.toml")),
        )
        .unwrap();
        let secret = state.path().join("outside-secret");
        fs::write(&secret, "must never appear in the checkout").unwrap();
        std::os::unix::fs::symlink(&secret, fixture.join(".pre-commit-config.yaml")).unwrap();
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("-C")
                .arg(&fixture)
                .args(["add", "."]),
        )
        .unwrap();
        // Operator-authorized disposable fixture identity; never release metadata.
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("-C")
                .arg(&fixture)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    "fixture",
                ]),
        )
        .unwrap();
        let revision = output(
            data_command("git", state.path())
                .unwrap()
                .arg("-C")
                .arg(&fixture)
                .args(["rev-parse", "HEAD"]),
        )
        .unwrap();
        let revision = String::from_utf8(revision.stdout).unwrap();
        let packages = ["simit-inert-source-fixture".to_owned()];
        let archive_root = "simit-inert-source-fixture-1.0.0";
        let archive = state
            .path()
            .join(format!("target/package/{archive_root}.crate"));
        output(
            reconstruction_command(state.path(), &fixture, &packages)
                .unwrap()
                .arg("--offline"),
        )
        .unwrap();
        let candidate = fs::read(&archive).unwrap();
        let source = state.path().join("source");
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("init")
                .arg(&source),
        )
        .unwrap();
        output(
            data_command("git", state.path())
                .unwrap()
                .arg("-C")
                .arg(&source)
                .arg("fetch")
                .arg(&fixture)
                .arg(revision.trim()),
        )
        .unwrap();
        checkout_signed_source(state.path(), &source, revision.trim()).unwrap();
        let tracked = output(
            data_command("git", state.path())
                .unwrap()
                .arg("-C")
                .arg(&source)
                .args(["ls-files", "--stage", "-z"]),
        )
        .unwrap();
        let links = super::super::source::validate_tree(&source, &tracked.stdout).unwrap();
        validate_manifest_paths_with_links(&source, &links).unwrap();
        output(
            reconstruction_command(state.path(), &source, &packages)
                .unwrap()
                .arg("--offline"),
        )
        .unwrap();
        let reconstructed = fs::read(archive).unwrap();
        verify_reconstructed_crate(&candidate, &reconstructed, archive_root).unwrap();
        let members = crate_members(&candidate, archive_root).unwrap();
        super::super::source::reject_link_members(&members, archive_root, &source, &links).unwrap();
        assert!(!members.contains_key(&format!("{archive_root}/.pre-commit-config.yaml")));
        let link = source.join(".pre-commit-config.yaml");
        assert_eq!(links, std::collections::BTreeSet::from([link.clone()]));
        assert!(fs::symlink_metadata(&link).unwrap().is_file());
        assert_eq!(fs::read_to_string(&link).unwrap(), secret.to_str().unwrap());
        assert_eq!(
            fs::read_to_string(secret).unwrap(),
            "must never appear in the checkout"
        );
        let status = output(
            data_command("git", state.path())
                .unwrap()
                .arg("-C")
                .arg(&source)
                .args(["status", "--porcelain"]),
        )
        .unwrap();
        assert!(status.stdout.is_empty());
    }

    #[test]
    fn manifest_and_lock_inputs_cannot_depend_on_inert_git_links() {
        let source = TempDir::new().unwrap();
        let root = source.path();
        for (declaration, path) in [
            ("[package]\nname='demo'\nversion='1.0.0'\n", "Cargo.toml"),
            ("[package]\nname='demo'\nversion='1.0.0'\n", "Cargo.lock"),
            (
                "[package]\nname='demo'\nversion='1.0.0'\nreadme='README.md'\n",
                "README.md",
            ),
            (
                "[package]\nname='demo'\nversion='1.0.0'\nlicense-file='LICENSE'\n",
                "LICENSE",
            ),
            (
                "[package]\nname='demo'\nversion='1.0.0'\nbuild='build.rs'\n",
                "build.rs",
            ),
            (
                "[package]\nname='demo'\nversion='1.0.0'\n[lib]\npath='src/lib.rs'\n",
                "src/lib.rs",
            ),
            ("[dependencies.other]\npath='other'\n", "other"),
        ] {
            fs::write(root.join("Cargo.toml"), declaration).unwrap();
            let links = std::collections::BTreeSet::from([root.join(path)]);
            assert!(
                validate_manifest_paths_with_links(root, &links).is_err(),
                "{path}"
            );
        }
        let child = root.join("member");
        fs::create_dir(&child).unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=['member']\n").unwrap();
        fs::write(
            child.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='1.0.0'\nreadme='../README.md'\n",
        )
        .unwrap();
        assert!(
            validate_manifest_paths_with_links(
                root,
                &std::collections::BTreeSet::from([root.join("README.md")])
            )
            .is_err()
        );
    }

    #[test]
    fn isolated_reconstruction_rewrites_public_github_ssh_inputs_without_credentials() {
        let state = TempDir::new().unwrap();
        fs::create_dir(state.path().join("home")).unwrap();
        let repository = state.path().join("transport.git");
        output(
            data_command("git", state.path())
                .unwrap()
                .args(["init", "--bare"])
                .arg(&repository),
        )
        .unwrap();
        for original in [
            "ssh://git@github.com/owner/dependency.git",
            "git@github.com:owner/dependency.git",
        ] {
            output(
                data_command("git", state.path())
                    .unwrap()
                    .arg("--git-dir")
                    .arg(&repository)
                    .args(["config", "remote.origin.url", original]),
            )
            .unwrap();
            let resolved = output(
                data_command("git", state.path())
                    .unwrap()
                    .arg("--git-dir")
                    .arg(&repository)
                    .args(["remote", "get-url", "origin"]),
            )
            .unwrap();
            assert_eq!(
                String::from_utf8(resolved.stdout).unwrap().trim(),
                "https://github.com/owner/dependency.git"
            );
        }
        let cargo = data_command("cargo", state.path()).unwrap();
        assert!(
            cargo
                .get_envs()
                .any(|(key, value)| key == "CARGO_NET_GIT_FETCH_WITH_CLI"
                    && value == Some(std::ffi::OsStr::new("true")))
        );
        assert!(
            !cargo.get_envs().any(|(key, _)| key == "CRATES_IO_API_TOKEN"
                || key == "GITHUB_TOKEN"
                || key == "SSH_AUTH_SOCK"
                || key == "GIT_CONFIG_KEY_2")
        );
    }

    #[test]
    fn upload_metadata_enforces_the_signed_crates_io_publication_policy() {
        for (policy, allowed) in [
            ("", true),
            ("publish=false", false),
            ("publish=[]", false),
            ("publish=['private']", false),
            ("publish=['crates-io']", true),
            ("publish=['private','crates-io']", true),
            ("publish='crates-io'", false),
            ("publish=['crates-io',1]", false),
        ] {
            let manifest: toml_edit::DocumentMut =
                format!("[package]\nname='demo'\nversion='1.0.0'\n{policy}\n")
                    .parse()
                    .unwrap();
            assert_eq!(
                upload_metadata(&manifest, &BTreeMap::new(), "demo-1.0.0", "demo", "1.0.0").is_ok(),
                allowed,
                "{policy}"
            );
        }
    }

    #[test]
    fn metadata_named_dependencies_keep_their_real_escape_checks() {
        let source = TempDir::new().unwrap();
        for declaration in [
            "[dependencies.metadata]\npath='/outside'",
            "[workspace.dependencies.metadata]\npath='/outside'",
            "[target.'cfg(unix)'.dependencies.metadata]\npath='../outside'",
            "[patch.crates-io.metadata]\npath='/outside'",
        ] {
            fs::write(source.path().join("Cargo.toml"), declaration).unwrap();
            assert!(
                validate_manifest_paths(source.path()).is_err(),
                "{declaration}"
            );
        }
    }

    #[test]
    fn documented_publish_responses_are_accepted_by_the_real_http_transport() {
        use std::io::Write;
        use std::net::TcpListener;
        for (status, body, accepted) in [
            (
                200,
                r#"{"warnings":{"invalid_categories":[],"invalid_badges":[],"other":[]}}"#,
                true,
            ),
            (
                201,
                r#"{"warnings":{"other":["advisory"]},"errors":[]}"#,
                true,
            ),
            (200, r#"{"errors":[{"detail":"rejected"}]}"#, false),
            (403, r#"{"warnings":{}}"#, false),
            (200, "not json", false),
            (200, "null", false),
            (200, r#"{"errors":{"detail":"malformed"}}"#, false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                assert!(socket.read(&mut request).unwrap() > 0);
                write!(socket, "HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let client = Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()
                .unwrap();
            let result = upload_archive_at(
                &client,
                &format!("http://{address}/api/v1/crates/new"),
                "disposable-fixture-token",
                &json!({"name":"fixture","vers":"0.1.0"}),
                b"verified archive",
            );
            server.join().unwrap();
            assert_eq!(result.is_ok(), accepted, "{status}: {body}: {result:?}");
        }
    }

    #[test]
    fn uncertain_uploads_reconcile_exact_registry_identity_without_reupload() {
        let archive = b"verified archive";
        let checksum = hex::encode(Sha256::digest(archive));
        let metadata = json!({"name":"fixture","vers":"0.1.0"});
        for final_record in [
            Some(
                json!({"version":{"crate":"fixture","num":"0.1.0","yanked":false,"checksum":checksum}}),
            ),
            Some(
                json!({"version":{"crate":"other","num":"0.1.0","yanked":false,"checksum":checksum}}),
            ),
            Some(
                json!({"version":{"crate":"fixture","num":"0.1.0","yanked":true,"checksum":checksum}}),
            ),
            Some(
                json!({"version":{"crate":"fixture","num":"0.1.0","yanked":false,"checksum":"foreign"}}),
            ),
            None,
        ] {
            let mut reads = 0;
            let mut uploads = 0;
            let mut waits = 0;
            let result = publish_archive_with_reconciliation(
                &metadata,
                archive,
                &checksum,
                Some("disposable-fixture-token"),
                |_, _| {
                    reads += 1;
                    if reads <= 2 {
                        Ok(None)
                    } else {
                        Ok(final_record.clone())
                    }
                },
                |_, _, _| {
                    uploads += 1;
                    bail!("lost upload reply")
                },
                || {
                    waits += 1;
                },
            );
            assert_eq!(
                result.is_ok(),
                final_record
                    .as_ref()
                    .is_some_and(|record| exact_registry_record(
                        record, "fixture", "0.1.0", &checksum
                    )),
                "{result:?}"
            );
            assert_eq!(uploads, 1);
            assert!(reads <= 21 && waits <= 19);
        }
    }

    #[test]
    fn free_form_metadata_does_not_authorize_or_reject_filesystem_reads() {
        let source = TempDir::new().unwrap();
        for section in ["package.metadata.tool", "workspace.metadata.tool"] {
            fs::write(source.path().join("Cargo.toml"), format!("[{section}]\npath='/api'\nbuild='external'\nworkspace='/outside'\nreadme='/etc/passwd'\nmembers=['/outside']\ninclude=['/outside']\nexclude=['/outside']\n")).unwrap();
            validate_manifest_paths(source.path()).unwrap();
        }
    }

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
        fs::write(
            source.path().join("Cargo.toml"),
            "[workspace]\nmembers=['member']\n",
        )
        .unwrap();
        fs::create_dir(source.path().join("member")).unwrap();
        for declaration in [
            "[package]\nreadme='/etc/passwd'",
            "[package]\nlicense-file='../../secret'",
            "[dependencies]\nother={path='../../other'}",
            "[[bin]]\nname='escape'\npath='../../outside.rs'",
            "[workspace]\nmembers=['../../other']",
            "[dependencies.metadata]\npath='../../other'",
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
    fn manifest_validation_ignores_unreachable_cargo_fixtures_but_checks_the_graph() {
        let source = TempDir::new().unwrap();
        for directory in [
            "crates/selected",
            "crates/excluded",
            "fixtures/invalid",
            "local-dependency",
        ] {
            fs::create_dir_all(source.path().join(directory)).unwrap();
        }
        fs::write(
            source.path().join("Cargo.toml"),
            "[workspace]\nmembers=['crates/*']\nexclude=['crates/excluded']\n",
        )
        .unwrap();
        fs::write(source.path().join("crates/selected/Cargo.toml"), "[package]\nname='selected'\nversion='1.0.0'\n[dependencies]\nlocal={path='../../local-dependency'}\n").unwrap();
        fs::write(
            source.path().join("local-dependency/Cargo.toml"),
            "[package]\nname='local'\nversion='1.0.0'\n",
        )
        .unwrap();
        fs::write(
            source.path().join("fixtures/invalid/Cargo.toml"),
            "intentionally invalid toml [[",
        )
        .unwrap();
        fs::write(
            source.path().join("crates/excluded/Cargo.toml"),
            "[package]\nreadme='/outside'\n",
        )
        .unwrap();
        validate_manifest_paths(source.path()).unwrap();
        fs::write(
            source.path().join("local-dependency/Cargo.toml"),
            "[dependencies]\nescape={path='../../outside'}\n",
        )
        .unwrap();
        assert!(validate_manifest_paths(source.path()).is_err());
    }

    #[test]
    fn real_cargo_reconstruction_is_data_only_and_packages_unpublished_workspace_dependencies() {
        assert_data_only_workspace_reconstruction(false);
    }

    #[test]
    fn real_cargo_reconstruction_supports_directory_source_replacement_in_packaged_gates() {
        assert_data_only_workspace_reconstruction(true);
    }

    fn assert_data_only_workspace_reconstruction(directory_source: bool) {
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
        if directory_source {
            // buildRustPackage supplies this trusted source shape. It must not
            // change the reconstruction registry or execute candidate config.
            fs::create_dir(state.path().join(".cargo")).unwrap();
            fs::create_dir(state.path().join("vendor")).unwrap();
            fs::write(state.path().join(".cargo/config.toml"), format!("[source.crates-io]\nreplace-with='qualification-vendor'\n[source.qualification-vendor]\ndirectory={:?}\n", state.path().join("vendor"))).unwrap();
        }
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
    fn signed_workflow_cannot_change_enrolled_authority_with_identical_triggers() {
        let enrolled = b"on:\n  push:\n    tags: ['[0-9]*']\npermissions:\n  contents: read\njobs:\n  build:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: cargo package\n";
        let original: serde_yaml::Value = serde_yaml::from_slice(enrolled).unwrap();
        for mutation in [
            "permissions: {id-token: write}",
            "permissions: write-all",
            "jobs: {build: {permissions: {id-token: write}}}",
            "jobs: {build: {environment: simit-crates-io}}",
            "jobs: {build: {env: {TOKEN: '${{ secrets.TOKEN }}'}}}",
            "jobs: {build: {runs-on: atlas-nix-trusted}}",
            "jobs: {build: {steps: [{run: 'echo ${{ secrets.TOKEN }}'}]}}",
            "jobs: {extra: {runs-on: ubuntu-24.04, steps: [{run: 'echo bypass'}]}}",
        ] {
            let mut changed = original.clone();
            let patch: serde_yaml::Value = serde_yaml::from_str(mutation).unwrap();
            for (key, value) in patch.as_mapping().unwrap() {
                changed
                    .as_mapping_mut()
                    .unwrap()
                    .insert(key.clone(), value.clone());
            }
            assert_eq!(changed["on"], original["on"]);
            let bytes = serde_yaml::to_string(&changed).unwrap();
            assert!(
                verify_tag_only_source_workflow(bytes.as_bytes(), enrolled).is_err(),
                "accepted {mutation}"
            );
        }
        // YAML key ordering, comments and formatting carry no execution authority.
        let equivalent = b"# equivalent enrollment\njobs: {build: {steps: [{run: cargo package}], runs-on: ubuntu-24.04}}\npermissions: {contents: read}\non: {push: {tags: ['[0-9]*']}}\n";
        verify_tag_only_source_workflow(equivalent, enrolled).unwrap();
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
    fn independent_source_enrollment_keeps_exact_namespace_and_tag_only_authority() {
        let enrolled = b"on:\n  push:\n    tags: ['harbor-cache/v[0-9]*']\n";
        verify_tag_only_source_workflow_with_namespace(enrolled, enrolled, Some("harbor-cache"))
            .unwrap();
        assert!(verify_tag_only_source_workflow(enrolled, enrolled).is_err());
        assert!(
            verify_tag_only_source_workflow_with_namespace(enrolled, enrolled, Some("harbor-sdk"))
                .is_err()
        );
        for changed in [
            &b"on:\n  push:\n    tags: ['[0-9]*']\n"[..],
            &b"on:\n  push:\n    branches: ['harbor-cache/v1.2.3']\n"[..],
            &b"on:\n  push:\n    tags: ['harbor-cache/v[0-9]*']\n  workflow_dispatch:\n"[..],
            &b"on:\n  push:\n    tags: ['harbor-cache/v[0-9]*', 'harbor-sdk/v[0-9]*']\n"[..],
        ] {
            assert!(
                verify_tag_only_source_workflow_with_namespace(
                    changed,
                    changed,
                    Some("harbor-cache")
                )
                .is_err()
            );
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
