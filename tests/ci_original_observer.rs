use serde_json::{Value, json};
use simit::ci_runtime::{cancellation::Cancellation, observer::*};
use std::{
    collections::VecDeque,
    io::{Cursor, Write},
    time::Duration,
};

#[derive(Default)]
struct FakeClock(Duration);
impl Clock for FakeClock {
    fn elapsed(&self) -> Duration {
        self.0
    }
    fn sleep(&mut self, duration: Duration, cancellation: &Cancellation) -> anyhow::Result<()> {
        cancellation.check()?;
        self.0 += duration;
        Ok(())
    }
}

struct Provider {
    runs: VecDeque<Value>,
    failures: VecDeque<u16>,
    logs: Vec<u8>,
    jobs: Option<Value>,
    discovery: Option<Value>,
}
fn run() -> Value {
    json!({"id":12,"repository":{"id":101,"full_name":"owner/repo"},"head_sha":"a".repeat(40),
        "head_branch":"feature","head_repository":{"id":101,"full_name":"owner/repo"},
        "run_attempt":1,"event":"push","path":".github/workflows/ci.yaml",
        "status":"completed","conclusion":"success","html_url":"https://github.com/owner/repo/actions/runs/12"})
}
impl Transport for Provider {
    fn get(
        &mut self,
        endpoint: &str,
        _maximum: u64,
        _timeout: Duration,
        cancellation: &Cancellation,
    ) -> anyhow::Result<Response> {
        cancellation.check()?;
        assert!(!endpoint.contains("rerun"));
        if let Some(status) = self.failures.pop_front() {
            return Ok(Response {
                status,
                body: b"original interrupted response".to_vec(),
                retry_after: None,
                transport_error: None,
            });
        }
        let body = if endpoint.contains("/jobs?") {
            serde_json::to_vec(&self.jobs.clone().unwrap_or_else(||json!({"total_count":1,"jobs":[{"id":22,"run_id":12,"run_attempt":1,"head_sha":"a".repeat(40),"name":"test","status":"completed","conclusion":"success","steps":[{"number":1,"name":"Test","status":"completed","conclusion":"success"}]}]})))?
        } else if endpoint.ends_with("/logs") {
            self.logs.clone()
        } else if endpoint.contains("actions/runs?") {
            serde_json::to_vec(
                &self
                    .discovery
                    .clone()
                    .unwrap_or_else(|| json!({"total_count":1,"workflow_runs":[run()]})),
            )?
        } else {
            serde_json::to_vec(&self.runs.pop_front().unwrap_or_else(run))?
        };
        Ok(Response {
            status: 200,
            body,
            retry_after: None,
            transport_error: None,
        })
    }
}
fn fixture() -> (Spec, Identity, Provider) {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("test/1_Test.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"original execution log\n").unwrap();
    let provider = Provider {
        runs: VecDeque::new(),
        failures: VecDeque::new(),
        logs: zip.finish().unwrap().into_inner(),
        jobs: None,
        discovery: None,
    };
    let spec = Spec {
        schema: "simit.original-observer-spec.v1".into(),
        repository: "owner/repo".into(),
        workflows: [(".github/workflows/ci.yaml".into(), vec!["test".into()])].into(),
        timeout_seconds: 120,
        poll_seconds: 10,
    };
    let identity = Identity {
        repository_id:101,
        head: "a".repeat(40),
        event: "push".into(),
        run_id: 99,
        run_attempt: 1,
        workflow_ref: "owner/repo/.github/workflows/custody.yaml@refs/heads/feature".into(),
        workflow_sha: "a".repeat(40),
        event_json: serde_json::to_string_pretty(
            &json!({"after":"a".repeat(40),"ref":"refs/heads/feature","deleted":false,"repository":{"id":101,"full_name":"owner/repo"}}),
        )
        .unwrap()
            + "\n",
    };
    (spec, identity, provider)
}
fn receipt(root: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("receipt.json")).unwrap()).unwrap()
}

#[test]
fn transient_500_and_502_are_retained_then_terminal_polling_completes() {
    let (spec, identity, mut provider) = fixture();
    provider.failures.extend([500, 502]);
    let mut queued = run();
    queued["status"] = json!("in_progress");
    queued["conclusion"] = Value::Null;
    provider.runs.extend([queued, run()]);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    let result = observe_with(
        &spec,
        &identity,
        &root,
        &mut provider,
        &mut FakeClock::default(),
        &Cancellation::default(),
    )
    .unwrap();
    assert!(result.capture_complete);
    assert!(!result.qualified);
    assert_eq!(result.producers.len(), 1);
    assert_eq!(
        std::fs::read(root.join("requests/0001.body")).unwrap(),
        b"original interrupted response"
    );
    assert_eq!(
        std::fs::read(root.join("requests/0002.body")).unwrap(),
        b"original interrupted response"
    );
    assert_eq!(receipt(&root)["capture_complete"], true);
    assert_eq!(
        std::fs::read(root.join("event.json")).unwrap(),
        identity.event_json.as_bytes()
    );
}

#[test]
fn persistent_provider_error_retains_an_explicit_failed_receipt() {
    let (spec, identity, mut provider) = fixture();
    provider.failures.extend([502; 10]);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default()
        )
        .is_err()
    );
    let r = receipt(&root);
    assert_eq!(r["capture_complete"], false);
    assert_eq!(r["qualified"], false);
    assert!(!r["error"].as_str().unwrap().is_empty());
    assert!(!r["members"].as_object().unwrap().is_empty());
}

#[test]
fn authorization_failure_is_not_retried_or_promoted() {
    let (spec, identity, mut provider) = fixture();
    provider.failures.extend([403, 200]);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default()
        )
        .is_err()
    );
    assert_eq!(provider.failures.len(), 1);
    assert_eq!(receipt(&root)["capture_complete"], false);
}

#[test]
fn run_head_event_attempt_workflow_and_repository_drift_fail_closed() {
    for (pointer, value) in [
        ("/id", json!(13)),
        ("/head_sha", json!("b".repeat(40))),
        ("/event", json!("pull_request")),
        ("/run_attempt", json!(2)),
        ("/path", json!(".github/workflows/other.yaml")),
        ("/repository/full_name", json!("other/repo")),
        ("/repository/id", json!(102)),
    ] {
        let (spec, identity, mut provider) = fixture();
        let mut changed = run();
        *changed.pointer_mut(pointer).unwrap() = value;
        provider.runs.push_back(changed);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        assert!(
            observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut FakeClock::default(),
                &Cancellation::default()
            )
            .is_err(),
            "{pointer}"
        );
        assert_eq!(receipt(&root)["capture_complete"], false);
    }
}

#[test]
fn aggregate_deadline_retains_incomplete_evidence_even_with_nonterminal_producer() {
    let (mut spec, identity, mut provider) = fixture();
    spec.timeout_seconds = 30;
    let mut queued = run();
    queued["status"] = json!("in_progress");
    queued["conclusion"] = Value::Null;
    provider.runs.extend(std::iter::repeat_n(queued, 20));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    let mut clock = FakeClock::default();
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut clock,
            &Cancellation::default()
        )
        .is_err()
    );
    assert!(clock.elapsed() <= Duration::from_secs(30));
    assert_eq!(receipt(&root)["capture_complete"], false);
}

#[test]
fn cancelled_failed_skipped_and_empty_success_cannot_establish_acceptance() {
    for conclusion in ["cancelled", "failure", "skipped", "success"] {
        let (spec, identity, mut provider) = fixture();
        let mut terminal = run();
        terminal["conclusion"] = json!(conclusion);
        provider.runs.push_back(terminal);
        if conclusion == "success" {
            provider.logs = zip::ZipWriter::new(Cursor::new(Vec::new()))
                .finish()
                .unwrap()
                .into_inner();
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        assert!(
            observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut FakeClock::default(),
                &Cancellation::default()
            )
            .is_err()
        );
        assert_eq!(receipt(&root)["capture_complete"], false);
    }
}

#[test]
fn pre_cancelled_capture_preserves_a_nonaccepting_receipt_and_existing_packets() {
    let (spec, identity, mut provider) = fixture();
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &cancellation
        )
        .is_err()
    );
    let bytes = std::fs::read(root.join("receipt.json")).unwrap();
    assert_eq!(receipt(&root)["capture_complete"], false);
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default()
        )
        .is_err()
    );
    assert_eq!(std::fs::read(root.join("receipt.json")).unwrap(), bytes);
}

#[test]
fn unsuccessful_original_retains_provider_archive_and_jobs_before_rejection() {
    for conclusion in ["cancelled", "failure", "skipped"] {
        let (spec, identity, mut provider) = fixture();
        let mut failed = run();
        failed["conclusion"] = json!(conclusion);
        provider.runs.push_back(failed);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        assert!(
            observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut FakeClock::default(),
                &Cancellation::default()
            )
            .is_err()
        );
        assert_eq!(receipt(&root)["capture_complete"], false);
        assert_eq!(
            std::fs::read(root.join("12/provider-logs.zip")).unwrap(),
            provider.logs
        );
        let jobs: Vec<Value> =
            serde_json::from_slice(&std::fs::read(root.join("12/jobs.json")).unwrap()).unwrap();
        assert_eq!(jobs.len(), 1);
    }
}

#[test]
fn missing_native_steps_duplicate_runs_and_duplicate_zip_names_are_rejected() {
    for kind in ["steps", "runs", "zip", "crc"] {
        let (spec, identity, mut provider) = fixture();
        match kind {
            "steps" => {
                provider.jobs = Some(
                    json!({"total_count":1,"jobs":[{"id":22,"run_id":12,"run_attempt":1,"head_sha":"a".repeat(40),"name":"test","status":"completed","conclusion":"success","steps":[]}]}),
                )
            }
            "runs" => {
                provider.discovery = Some(json!({"total_count":2,"workflow_runs":[run(),run()]}))
            }
            "zip" => {
                let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                for name in ["a.txt", "b.txt"] {
                    zip.start_file(name, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    zip.write_all(b"original").unwrap();
                }
                let mut raw = zip.finish().unwrap().into_inner();
                let offsets: Vec<_> = raw
                    .windows(5)
                    .enumerate()
                    .filter(|(_, b)| *b == b"b.txt")
                    .map(|(i, _)| i)
                    .collect();
                for offset in offsets {
                    raw[offset] = b'a';
                }
                provider.logs = raw;
            }
            "crc" => {
                let raw = &provider.logs;
                let offset = 30
                    + u16::from_le_bytes([raw[26], raw[27]]) as usize
                    + u16::from_le_bytes([raw[28], raw[29]]) as usize;
                provider.logs[offset] ^= 0xff;
            }
            _ => unreachable!(),
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        assert!(
            observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut FakeClock::default(),
                &Cancellation::default()
            )
            .is_err(),
            "{kind}"
        );
        assert_eq!(receipt(&root)["capture_complete"], false);
    }
}

struct Uploaded {
    archive: Vec<u8>,
    metadata: Value,
}
impl Transport for Uploaded {
    fn get(
        &mut self,
        endpoint: &str,
        _maximum: u64,
        _timeout: Duration,
        cancellation: &Cancellation,
    ) -> anyhow::Result<Response> {
        cancellation.check()?;
        let body = if endpoint.ends_with("/zip") {
            self.archive.clone()
        } else if endpoint.contains("/artifacts/") {
            serde_json::to_vec(&self.metadata)?
        } else if endpoint.ends_with("/99") {
            let mut observer = run();
            observer["id"] = json!(99);
            observer["path"] = json!(".github/workflows/custody.yaml");
            serde_json::to_vec(&observer)?
        } else {
            serde_json::to_vec(&run())?
        };
        Ok(Response {
            status: 200,
            body,
            retry_after: None,
            transport_error: None,
        })
    }
}
fn uploaded(root: &std::path::Path) -> (Uploaded, String) {
    use sha2::{Digest, Sha256};
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut dirs = vec![root.to_owned()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            zip.start_file(
                path.strip_prefix(root).unwrap().to_str().unwrap(),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&std::fs::read(path).unwrap()).unwrap();
        }
    }
    let archive = zip.finish().unwrap().into_inner();
    let digest = hex::encode(Sha256::digest(&archive));
    let created = chrono::Utc::now();
    let expires = created + chrono::Duration::days(32);
    let metadata = json!({"id":42,"expired":false,"digest":format!("sha256:{digest}"),"workflow_run":{"id":99,"head_sha":"a".repeat(40)},"created_at":created.to_rfc3339(),"expires_at":expires.to_rfc3339()});
    (Uploaded { archive, metadata }, digest)
}

#[test]
fn native_retention_binds_actual_provider_zip_to_local_members_and_independent_source() {
    let (spec, identity, mut provider) = fixture();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    observe_with(
        &spec,
        &identity,
        &root,
        &mut provider,
        &mut FakeClock::default(),
        &Cancellation::default(),
    )
    .unwrap();
    let original = std::fs::read(root.join("receipt.json")).unwrap();
    let (mut upload, digest) = uploaded(&root);
    let admitted = verify_retention(
        &spec,
        &identity,
        &root,
        42,
        &digest,
        &mut upload,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(admitted["capture_complete"], true);
    assert_eq!(admitted["qualified"], false);
    assert!(
        root.with_extension("retention")
            .join("retention.json")
            .is_file()
    );
    assert_eq!(std::fs::read(root.join("receipt.json")).unwrap(), original);
}

#[test]
fn native_retention_rejects_substituted_upload_identity_digest_expiry_and_local_bytes() {
    for kind in ["identity", "digest", "expiry", "bytes", "zip"] {
        let (spec, identity, mut provider) = fixture();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default(),
        )
        .unwrap();
        let (mut upload, digest) = uploaded(&root);
        match kind {
            "identity" => upload.metadata["workflow_run"]["id"] = json!(100),
            "digest" => upload.metadata["digest"] = json!(format!("sha256:{}", "b".repeat(64))),
            "expiry" => upload.metadata["expired"] = json!(true),
            "bytes" => std::fs::write(root.join("event.json"), b"changed").unwrap(),
            "zip" => upload.archive.push(0),
            _ => unreachable!(),
        }
        assert!(
            verify_retention(
                &spec,
                &identity,
                &root,
                42,
                &digest,
                &mut upload,
                &Cancellation::default()
            )
            .is_err(),
            "{kind}"
        );
    }
}
