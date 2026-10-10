//! Bind a trusted default-branch publisher to one successful original release run.
//!
//! The policy and publisher context must come from the independently trusted
//! workflow, and `provider_run` from the authenticated GitHub run API. Candidate
//! artifacts, names, successful checks, or event data cannot supply that policy.
//! This is admission metadata, not proof of a signed tag or safe credentials;
//! signature verification and protected environment enrollment remain required.

use std::io::Read;

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct PublisherPolicy {
    pub repository: String,
    pub default_branch: String,
    pub source_workflow_path: String,
    pub source_workflow_id: u64,
    pub publisher_workflow_path: String,
}

/// GitHub-provided context of the independent publisher, never candidate output.
#[derive(Debug, Clone)]
pub struct PublisherContext {
    pub event_name: String,
    pub workflow_ref: String,
    pub workflow_sha: String,
    pub git_ref: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunRepository {
    pub id: u64,
    pub full_name: String,
}

/// The identity subset shared by the webhook and native Actions run response.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseRun {
    pub id: u64,
    pub run_attempt: u64,
    pub workflow_id: u64,
    pub path: String,
    pub event: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub head_branch: String,
    pub head_sha: String,
    pub repository: RunRepository,
    pub head_repository: RunRepository,
}

#[derive(Debug, Deserialize)]
pub struct CompletedRunEvent {
    pub action: String,
    pub repository: EventRepository,
    pub workflow_run: ReleaseRun,
}

#[derive(Debug, Deserialize)]
pub struct EventRepository {
    pub id: u64,
    pub full_name: String,
    pub default_branch: String,
}

/// Preserve the original run across retries, with a distinct attempt identity.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AdmittedReleaseRun {
    pub source_run_id: u64,
    pub source_run_attempt: u64,
    pub source_workflow_id: u64,
    pub source_workflow_path: String,
    pub repository: String,
    pub source_repository_id: u64,
    pub tag: String,
    pub source_sha: String,
    pub publisher_workflow_ref: String,
    pub publisher_workflow_sha: String,
}

/// Native artifact metadata fetched by trusted code from the repository API.
/// Candidate manifests cannot supply this record or its digest.
#[derive(Debug, Deserialize, Serialize)]
pub struct ReleaseArtifact {
    pub id: u64,
    pub name: String,
    pub expired: bool,
    pub digest: Option<String>,
    pub workflow_run: Option<ArtifactRun>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtifactRun {
    pub id: u64,
    pub repository_id: u64,
    pub head_repository_id: u64,
    pub head_branch: String,
    pub head_sha: String,
}

/// Provider-bound download bytes, not signed-tag or archive-member acceptance.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ProviderBoundArtifact {
    pub artifact_id: u64,
    pub source_run_id: u64,
    pub name: String,
    pub archive_bytes: u64,
    pub archive_sha256: String,
}

/// Verify the original run/repository and native SHA-256 digest before parsing
/// any downloaded archive. Expected name and byte limit come from trusted policy.
/// GitHub artifact metadata does not expose run_attempt: recovery may reuse an
/// immutable artifact from the original run with the same source SHA. This is not
/// proof that a particular attempt produced it. Signed-tag verification and safe
/// member admission are still required before credential-bearing publication.
/// Download endpoints must be constructed from the admitted repository/artifact
/// identity by trusted code, never selected from a candidate-provided URL.
pub fn verify_release_artifact_archive(
    run: &AdmittedReleaseRun,
    artifact: &ReleaseArtifact,
    expected_name: &str,
    max_archive_bytes: u64,
    archive: impl Read,
) -> Result<ProviderBoundArtifact> {
    ensure!(
        (1..=512 * 1024 * 1024).contains(&max_archive_bytes),
        "release archive byte limit must be between 1 byte and 512 MiB"
    );
    ensure!(
        artifact.id > 0
            && !artifact.expired
            && !expected_name.is_empty()
            && artifact.name == expected_name,
        "release artifact identity, name or expiry is invalid"
    );
    let source = artifact
        .workflow_run
        .as_ref()
        .context("release artifact has no native workflow-run identity")?;
    ensure!(
        run.source_run_id > 0
            && run.source_repository_id > 0
            && is_sha(&run.source_sha)
            && source.id == run.source_run_id
            && source.repository_id == run.source_repository_id
            && source.head_repository_id == run.source_repository_id
            && source.head_branch == run.tag
            && source.head_sha == run.source_sha,
        "release artifact does not match the admitted original run and source"
    );
    let digest = artifact
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .context("release artifact has no native SHA-256 digest")?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "release artifact SHA-256 digest is invalid"
    );

    let mut reader = archive.take(max_archive_bytes + 1);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .context("read release artifact archive")?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        ensure!(
            bytes <= max_archive_bytes,
            "release artifact archive exceeds its trusted byte limit"
        );
        hasher.update(&buffer[..read]);
    }
    ensure!(bytes > 0, "release artifact archive is empty");
    let archive_sha256 = hex::encode(hasher.finalize());
    ensure!(
        archive_sha256 == digest,
        "release artifact archive differs from its native provider digest"
    );
    Ok(ProviderBoundArtifact {
        artifact_id: artifact.id,
        source_run_id: run.source_run_id,
        name: artifact.name.clone(),
        archive_bytes: bytes,
        archive_sha256,
    })
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_workflow_path(value: &str) -> bool {
    let Some(name) = value.strip_prefix(".github/workflows/") else {
        return false;
    };
    !name.is_empty()
        && !name.contains(['/', '\\', '@'])
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        && (name.ends_with(".yaml") || name.ends_with(".yml"))
}

/// Reject failures, fork runs, branch dispatches, stale attempts and source drift
/// before allowing any candidate checkout or credential-bearing publication.
pub fn admit_release_run(
    policy: &PublisherPolicy,
    context: &PublisherContext,
    event: &CompletedRunEvent,
    provider_run: &ReleaseRun,
) -> Result<AdmittedReleaseRun> {
    let components: Vec<_> = policy.repository.split('/').collect();
    ensure!(
        components.len() == 2
            && components.iter().all(|part| !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))),
        "publisher repository policy must be an exact owner/repository"
    );
    ensure!(
        !policy.default_branch.is_empty()
            && !policy.default_branch.contains(['@', '\\'])
            && !policy
                .default_branch
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace()),
        "publisher default branch policy is invalid"
    );
    ensure!(
        policy.source_workflow_id > 0
            && is_workflow_path(&policy.source_workflow_path)
            && is_workflow_path(&policy.publisher_workflow_path),
        "publisher workflow policy must name exact native workflow identities"
    );
    let trusted_ref = format!(
        "{}/{}@refs/heads/{}",
        policy.repository, policy.publisher_workflow_path, policy.default_branch
    );
    ensure!(
        context.event_name == "workflow_run"
            && context.workflow_ref == trusted_ref
            && context.git_ref == format!("refs/heads/{}", policy.default_branch)
            && is_sha(&context.workflow_sha),
        "publisher must execute its independent default-branch workflow"
    );
    ensure!(
        event.action == "completed"
            && event.repository.id > 0
            && event.repository.full_name == policy.repository
            && event.repository.default_branch == policy.default_branch,
        "release completion event does not match trusted repository policy"
    );
    let run = &event.workflow_run;
    ensure!(
        run == provider_run,
        "release event and native provider run identity differ"
    );
    ensure!(
        run.id > 0
            && run.run_attempt > 0
            && run.workflow_id == policy.source_workflow_id
            && run.path == policy.source_workflow_path
            && run.repository.id == event.repository.id
            && run.head_repository.id == event.repository.id
            && run.repository.full_name == policy.repository
            && run.head_repository.full_name == policy.repository,
        "release run is not the approved repository workflow"
    );
    ensure!(
        run.event == "push"
            && run.status == "completed"
            && run.conclusion.as_deref() == Some("success")
            && is_sha(&run.head_sha),
        "release run must be a successful completed push with an exact source SHA"
    );
    let version = Version::parse(&run.head_branch)?;
    ensure!(
        version.pre.is_empty()
            && version.build.is_empty()
            && version.to_string() == run.head_branch,
        "release run must identify an exact numeric semver tag"
    );
    Ok(AdmittedReleaseRun {
        source_run_id: run.id,
        source_run_attempt: run.run_attempt,
        source_workflow_id: run.workflow_id,
        source_workflow_path: run.path.clone(),
        repository: policy.repository.clone(),
        source_repository_id: run.repository.id,
        tag: run.head_branch.clone(),
        source_sha: run.head_sha.clone(),
        publisher_workflow_ref: context.workflow_ref.clone(),
        publisher_workflow_sha: context.workflow_sha.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PublisherPolicy, PublisherContext, CompletedRunEvent) {
        let policy = PublisherPolicy {
            repository: "caniko/demo".into(),
            default_branch: "trunk".into(),
            source_workflow_path: ".github/workflows/publish-crate.yaml".into(),
            source_workflow_id: 17,
            publisher_workflow_path: ".github/workflows/trusted-publish-crate.yaml".into(),
        };
        let context = PublisherContext {
            event_name: "workflow_run".into(),
            workflow_ref:
                "caniko/demo/.github/workflows/trusted-publish-crate.yaml@refs/heads/trunk".into(),
            workflow_sha: "a".repeat(40),
            git_ref: "refs/heads/trunk".into(),
        };
        let event = serde_json::from_value(serde_json::json!({
            "action": "completed",
            "repository": { "id": 101, "full_name": "caniko/demo", "default_branch": "trunk" },
            "workflow_run": {
                "id": 42, "run_attempt": 1, "workflow_id": 17,
                "path": ".github/workflows/publish-crate.yaml", "event": "push",
                "status": "completed", "conclusion": "success", "head_branch": "1.2.3",
                "head_sha": "b".repeat(40),
                "repository": { "id": 101, "full_name": "caniko/demo" },
                "head_repository": { "id": 101, "full_name": "caniko/demo" }
            }
        }))
        .unwrap();
        (policy, context, event)
    }

    #[test]
    fn retries_preserve_original_run_and_distinguish_attempts() {
        let (policy, context, mut event) = fixture();
        let original = admit_release_run(&policy, &context, &event, &event.workflow_run).unwrap();
        event.workflow_run.run_attempt = 2;
        let retry = admit_release_run(&policy, &context, &event, &event.workflow_run).unwrap();
        assert_eq!(retry.source_run_id, original.source_run_id);
        assert_eq!(retry.source_sha, original.source_sha);
        assert_eq!(retry.source_run_attempt, 2);
    }

    #[test]
    fn rejects_candidate_selected_publisher_context() {
        let (policy, context, event) = fixture();
        for (event_name, workflow_ref, git_ref) in [
            ("push", context.workflow_ref.as_str(), "refs/heads/trunk"),
            (
                "workflow_dispatch",
                context.workflow_ref.as_str(),
                "refs/heads/trunk",
            ),
            (
                "workflow_run",
                "caniko/demo/.github/workflows/trusted-publish-crate.yaml@refs/tags/1.2.3",
                "refs/tags/1.2.3",
            ),
            (
                "workflow_run",
                "attacker/demo/.github/workflows/trusted-publish-crate.yaml@refs/heads/trunk",
                "refs/heads/trunk",
            ),
            (
                "workflow_run",
                "caniko/demo/.github/workflows/other.yaml@refs/heads/trunk",
                "refs/heads/trunk",
            ),
        ] {
            let changed = PublisherContext {
                event_name: event_name.into(),
                workflow_ref: workflow_ref.into(),
                git_ref: git_ref.into(),
                ..context.clone()
            };
            assert!(admit_release_run(&policy, &changed, &event, &event.workflow_run).is_err());
        }
    }

    #[test]
    fn rejects_unsuccessful_fork_dispatch_and_wrong_workflow_runs() {
        let (policy, context, event) = fixture();
        let original = serde_json::to_value(&event.workflow_run).unwrap();
        for (key, value) in [
            ("id", serde_json::json!(0)),
            ("run_attempt", serde_json::json!(0)),
            ("workflow_id", serde_json::json!(18)),
            ("path", serde_json::json!(".github/workflows/other.yaml")),
            ("event", serde_json::json!("workflow_dispatch")),
            ("event", serde_json::json!("pull_request")),
            ("status", serde_json::json!("in_progress")),
            ("conclusion", serde_json::json!("failure")),
            ("conclusion", serde_json::json!("cancelled")),
            ("conclusion", serde_json::json!("skipped")),
            ("conclusion", serde_json::Value::Null),
            ("head_sha", serde_json::json!("b".repeat(39))),
            ("head_branch", serde_json::json!("trunk")),
            ("head_branch", serde_json::json!("v1.2.3")),
            ("head_branch", serde_json::json!("1.2.3-rc.1")),
            ("head_branch", serde_json::json!("1.2.3+build")),
            ("head_branch", serde_json::json!("01.2.3")),
            (
                "head_repository",
                serde_json::json!({ "id": 102, "full_name": "attacker/demo" }),
            ),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            let mut changed_event = fixture().2;
            changed_event.workflow_run = serde_json::from_value(changed).unwrap();
            assert!(
                admit_release_run(
                    &policy,
                    &context,
                    &changed_event,
                    &changed_event.workflow_run
                )
                .is_err(),
                "accepted {key}"
            );
        }
    }

    #[test]
    fn rejects_provider_drift_even_when_both_runs_individually_succeed() {
        let (policy, context, event) = fixture();
        for changed in [
            ReleaseRun {
                id: 43,
                ..event.workflow_run.clone()
            },
            ReleaseRun {
                run_attempt: 2,
                ..event.workflow_run.clone()
            },
            ReleaseRun {
                head_sha: "c".repeat(40),
                ..event.workflow_run.clone()
            },
            ReleaseRun {
                head_branch: "1.2.4".into(),
                ..event.workflow_run.clone()
            },
        ] {
            assert!(admit_release_run(&policy, &context, &event, &changed).is_err());
        }
    }

    fn artifact_fixture() -> (AdmittedReleaseRun, ReleaseArtifact, Vec<u8>) {
        use sha2::{Digest, Sha256};

        let (policy, context, event) = fixture();
        let run = admit_release_run(&policy, &context, &event, &event.workflow_run).unwrap();
        // This layer binds opaque download bytes; ZIP members are checked later.
        let bytes = b"opaque provider archive bytes".to_vec();
        let artifact = serde_json::from_value(serde_json::json!({
            "id": 73, "name": "release-crates", "expired": false,
            "digest": format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
            "workflow_run": {
                "id": run.source_run_id, "repository_id": run.source_repository_id,
                "head_repository_id": run.source_repository_id,
                "head_branch": run.tag, "head_sha": run.source_sha
            }
        }))
        .unwrap();
        (run, artifact, bytes)
    }

    #[test]
    fn archive_admission_preserves_original_run_and_provider_digest() {
        let (mut run, artifact, bytes) = artifact_fixture();
        run.source_run_attempt = 2;
        let admitted = verify_release_artifact_archive(
            &run,
            &artifact,
            "release-crates",
            1024,
            bytes.as_slice(),
        )
        .unwrap();
        assert_eq!(admitted.artifact_id, 73);
        assert_eq!(admitted.source_run_id, 42);
        assert_eq!(admitted.archive_bytes, bytes.len() as u64);
        assert_eq!(
            format!("sha256:{}", admitted.archive_sha256),
            artifact.digest.unwrap()
        );
    }

    #[test]
    fn archive_admission_rejects_missing_digest_identity_drift_and_expiry() {
        let (run, artifact, bytes) = artifact_fixture();
        let original = serde_json::to_value(&artifact).unwrap();
        for (key, value) in [
            ("id", serde_json::json!(0)),
            ("name", serde_json::json!("other")),
            ("expired", serde_json::json!(true)),
            ("digest", serde_json::Value::Null),
            ("digest", serde_json::json!("sha256:bad")),
            (
                "digest",
                serde_json::json!(format!("sha256:{}", "g".repeat(64))),
            ),
            (
                "digest",
                serde_json::json!(format!("sha256:{}", "0".repeat(64))),
            ),
            ("workflow_run", serde_json::Value::Null),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            let changed: ReleaseArtifact = serde_json::from_value(changed).unwrap();
            assert!(
                verify_release_artifact_archive(
                    &run,
                    &changed,
                    "release-crates",
                    1024,
                    bytes.as_slice()
                )
                .is_err(),
                "accepted {key}"
            );
        }
        for (key, value) in [
            ("id", serde_json::json!(43)),
            ("repository_id", serde_json::json!(102)),
            ("head_repository_id", serde_json::json!(102)),
            ("head_branch", serde_json::json!("1.2.4")),
            ("head_sha", serde_json::json!("c".repeat(40))),
        ] {
            let mut changed = original.clone();
            changed["workflow_run"][key] = value;
            let changed: ReleaseArtifact = serde_json::from_value(changed).unwrap();
            assert!(
                verify_release_artifact_archive(
                    &run,
                    &changed,
                    "release-crates",
                    1024,
                    bytes.as_slice()
                )
                .is_err(),
                "accepted run {key}"
            );
        }
    }

    #[test]
    fn archive_admission_rejects_changed_truncated_empty_and_oversized_bytes() {
        let (run, artifact, bytes) = artifact_fixture();
        for changed in [
            Vec::new(),
            bytes[..bytes.len() - 1].to_vec(),
            [bytes.as_slice(), b"extra"].concat(),
        ] {
            assert!(
                verify_release_artifact_archive(
                    &run,
                    &artifact,
                    "release-crates",
                    1024,
                    changed.as_slice()
                )
                .is_err()
            );
        }
        assert!(
            verify_release_artifact_archive(
                &run,
                &artifact,
                "release-crates",
                bytes.len() as u64 - 1,
                bytes.as_slice()
            )
            .is_err()
        );
        assert!(
            verify_release_artifact_archive(&run, &artifact, "release-crates", 0, bytes.as_slice())
                .is_err()
        );
        assert!(
            verify_release_artifact_archive(
                &run,
                &artifact,
                "release-crates",
                u64::MAX,
                bytes.as_slice()
            )
            .is_err()
        );
    }

    #[test]
    fn archive_admission_propagates_download_read_failures() {
        struct BrokenDownload;
        impl Read for BrokenDownload {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture download failed"))
            }
        }
        let (run, artifact, _) = artifact_fixture();
        let error = verify_release_artifact_archive(
            &run,
            &artifact,
            "release-crates",
            1024,
            BrokenDownload,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("fixture download failed"));
    }

    #[test]
    fn run_admission_rejects_same_name_with_different_repository_ids() {
        let (policy, context, mut event) = fixture();
        event.workflow_run.head_repository.id += 1;
        assert!(admit_release_run(&policy, &context, &event, &event.workflow_run).is_err());
        let (policy, context, mut event) = fixture();
        event.repository.id += 1;
        assert!(admit_release_run(&policy, &context, &event, &event.workflow_run).is_err());
    }
}
