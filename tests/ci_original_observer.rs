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
    workflow_path: Option<String>,
    rate_limited: bool,
    capped_error_body: bool,
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
                retry_after: if self.rate_limited && status == 403 {
                    Some(3)
                } else {
                    None
                },
                transport_error: self.capped_error_body.then(|| "byte-limit".into()),
            });
        }
        let body = if endpoint.contains("/jobs?") {
            let mut jobs=self.jobs.clone().unwrap_or_else(||json!({"total_count":1,"jobs":[{"id":22,"run_id":12,"run_attempt":1,"head_sha":"a".repeat(40),"name":"test","status":"completed","conclusion":"success","steps":[{"number":1,"name":"Test","status":"completed","conclusion":"success"}]}]}));
            let page = endpoint
                .rsplit("&page=")
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let all = jobs["jobs"].as_array().unwrap();
            jobs["jobs"] = json!(
                all.iter()
                    .skip((page - 1) * 100)
                    .take(100)
                    .cloned()
                    .collect::<Vec<_>>()
            );
            serde_json::to_vec(&jobs)?
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
            let mut snapshot = self.runs.pop_front().unwrap_or_else(run);
            if let Some(path) = &self.workflow_path {
                snapshot["path"] = json!(path);
            }
            serde_json::to_vec(&snapshot)?
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
        workflow_path: None,
        rate_limited: false,
        capped_error_body: false,
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
fn attempt_scoped_jobs_admit_absent_attempt_but_reject_explicit_wrong_or_null_attempt() {
    for attempt in [None, Some(json!(1)), Some(json!(2)), Some(Value::Null)] {
        let (spec, identity, mut provider) = fixture();
        let mut job = json!({"id":22,"run_id":12,"head_sha":"a".repeat(40),"name":"test","status":"completed","conclusion":"success","steps":[{"number":1,"name":"Test","status":"completed","conclusion":"success"}]});
        if let Some(value) = &attempt {
            job["run_attempt"] = value.clone();
        }
        provider.jobs = Some(json!({"total_count":1,"jobs":[job]}));
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        let result = observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default(),
        );
        let admitted = attempt.is_none() || attempt == Some(json!(1));
        assert_eq!(result.is_ok(), admitted, "attempt={attempt:?}: {result:?}");
        assert_eq!(receipt(&root)["capture_complete"], admitted);
    }
}

#[test]
fn active_observer_enrollment_fails_before_packet_or_provider_effects() {
    for event in ["push", "pull_request"] {
        let (mut spec, mut identity, mut provider) = fixture();
        spec.workflows.insert(
            ".github/workflows/custody.yaml".into(),
            vec!["retain".into()],
        );
        identity.event = event.into();
        if event == "pull_request" {
            identity.workflow_ref =
                "owner/repo/.github/workflows/custody.yaml@refs/pull/66/merge".into();
            identity.event_json = json!({"repository":{"id":101,"full_name":"owner/repo"},
                "pull_request":{"head":{"sha":identity.head}}})
            .to_string();
        }
        provider.failures.push_back(401);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        let error = observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("active observer workflow"),
            "{error}"
        );
        assert!(!root.exists(), "invalid enrollment created a packet");
        assert_eq!(
            provider.failures.len(),
            1,
            "invalid enrollment contacted provider"
        );
    }
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
fn secondary_rate_limited_403_retries_inside_the_existing_deadline() {
    let (spec, identity, mut provider) = fixture();
    provider.rate_limited = true;
    provider.failures.push_back(403);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    let mut clock = FakeClock::default();
    observe_with(
        &spec,
        &identity,
        &root,
        &mut provider,
        &mut clock,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(clock.elapsed(), Duration::from_secs(3));
    let retained: Value =
        serde_json::from_slice(&std::fs::read(root.join("requests/0001.json")).unwrap()).unwrap();
    assert_eq!(retained["status"], 403);
    assert_eq!(retained["retry_after"], 3);
}

#[test]
fn headerless_secondary_429_waits_at_least_a_minute_with_bounded_backoff() {
    let (mut spec, identity, mut provider) = fixture();
    spec.timeout_seconds = 300;
    provider.failures.extend([429, 429]);
    let temp = tempfile::tempdir().unwrap();
    let mut clock = FakeClock::default();
    let receipt = observe_with(
        &spec,
        &identity,
        &temp.path().join("packet"),
        &mut provider,
        &mut clock,
        &Cancellation::default(),
    )
    .unwrap();
    assert!(receipt.capture_complete);
    assert_eq!(clock.elapsed(), Duration::from_secs(180));
}

#[test]
fn request_protocol_errors_retry_but_locally_invalid_requests_remain_permanent() {
    struct RequestFailure {
        provider: Provider,
        first: bool,
        kind: &'static str,
    }
    impl Transport for RequestFailure {
        fn get(
            &mut self,
            endpoint: &str,
            maximum: u64,
            timeout: Duration,
            cancellation: &Cancellation,
        ) -> anyhow::Result<Response> {
            if self.first {
                self.first = false;
                return Ok(Response {
                    status: 0,
                    body: vec![],
                    retry_after: None,
                    transport_error: Some(self.kind.into()),
                });
            }
            self.provider.get(endpoint, maximum, timeout, cancellation)
        }
    }
    for kind in ["request", "permanent-request"] {
        let (spec, identity, provider) = fixture();
        let mut provider = RequestFailure {
            provider,
            first: true,
            kind,
        };
        let temp = tempfile::tempdir().unwrap();
        let packet = temp.path().join("packet");
        let mut clock = FakeClock::default();
        let result = observe_with(
            &spec,
            &identity,
            &packet,
            &mut provider,
            &mut clock,
            &Cancellation::default(),
        );
        assert_eq!(result.is_ok(), kind == "request");
        assert_eq!(clock.elapsed().as_secs(), u64::from(kind == "request"));
        assert_eq!(receipt(&packet)["capture_complete"], kind == "request");
    }
}

#[test]
fn capped_transient_error_bodies_retry_but_success_and_authorization_limits_fail_closed() {
    for status in [408, 429, 500, 502, 503, 504, 403, 200, 401] {
        let (spec, identity, mut provider) = fixture();
        provider.capped_error_body = true;
        provider.rate_limited = true;
        provider.failures.push_back(status);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        let outcome = observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default(),
        );
        assert_eq!(
            outcome.is_ok(),
            !matches!(status, 200 | 401),
            "status={status}: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(root.join("requests/0001.body")).unwrap(),
            b"original interrupted response"
        );
        assert_eq!(
            receipt(&root)["capture_complete"],
            !matches!(status, 200 | 401)
        );
    }
}

#[test]
fn exact_branch_with_at_sign_remains_a_valid_ref_qualified_workflow_target() {
    let (spec, mut identity, mut provider) = fixture();
    let mut event: Value = serde_json::from_str(&identity.event_json).unwrap();
    event["ref"] = json!("refs/heads/feature@foo");
    identity.event_json = event.to_string();
    identity.workflow_ref =
        "owner/repo/.github/workflows/custody.yaml@refs/heads/feature@foo".into();
    let mut exact = run();
    exact["head_branch"] = json!("feature@foo");
    exact["path"] = json!(".github/workflows/ci.yaml@refs/heads/feature@foo");
    provider.discovery = Some(json!({"total_count":1,"workflow_runs":[exact]}));
    provider.runs.extend(std::iter::repeat_n(exact, 3));
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
        .is_ok()
    );
}

#[test]
fn git_valid_unicode_whitespace_and_non_ascii_controls_remain_valid_ref_targets() {
    for separator in ['\u{a0}', '\u{2003}', '\u{85}'] {
        let branch = format!("feature{separator}foo");
        let reference = format!("refs/heads/{branch}");
        assert!(
            std::process::Command::new("git")
                .args(["check-ref-format", &reference])
                .status()
                .unwrap()
                .success()
        );
        let (spec, mut identity, mut provider) = fixture();
        let mut event: Value = serde_json::from_str(&identity.event_json).unwrap();
        event["ref"] = json!(reference);
        identity.event_json = event.to_string();
        let mut snapshot = run();
        snapshot["head_branch"] = json!(branch);
        provider.runs = VecDeque::from(vec![snapshot.clone(); 32]);
        provider.discovery = Some(json!({"total_count":1,"workflow_runs":[snapshot]}));
        provider.workflow_path = Some(format!(".github/workflows/ci.yaml@{reference}"));
        let root = tempfile::tempdir().unwrap();
        let receipt = observe_with(
            &spec,
            &identity,
            &root.path().join("packet"),
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default(),
        )
        .unwrap();
        assert!(receipt.capture_complete);
    }
}

#[test]
fn cli_reads_every_maximal_enrolled_job_inventory_before_environment_admission() {
    let (mut spec, _, _) = fixture();
    for i in 0..10 {
        spec.workflows.insert(
            format!(".github/workflows/workflow-{i}.yaml"),
            (0..256)
                .map(|job| format!("{job:03}{}", "x".repeat(509)))
                .collect(),
        );
    }
    spec.workflows.remove(".github/workflows/ci.yaml");
    spec.timeout_seconds = 20700;
    spec.poll_seconds = 60;
    spec.validate().unwrap();
    let raw = serde_json::to_vec_pretty(&spec).unwrap();
    assert!(raw.len() > 1024 * 1024);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("spec.json");
    std::fs::write(&path, raw).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["ci", "observe-original", "--spec"])
        .arg(path)
        .arg("--results")
        .arg(temp.path().join("packet"))
        .env_remove("RUNNER_ENVIRONMENT")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("environment variable"), "{error}");
    assert!(!error.contains("byte limit"), "{error}");
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
fn ref_qualified_workflow_paths_are_admitted_and_malformed_suffixes_rejected() {
    for suffix in [
        "@feature",
        "@refs/heads/feature",
        "@",
        "@refs/heads/feature\n",
        "@feature@other",
        "@refs/heads/bad..ref",
    ] {
        let (spec, identity, mut provider) = fixture();
        let path = format!(".github/workflows/ci.yaml{suffix}");
        let mut discovered = run();
        discovered["path"] = json!(path);
        provider.discovery = Some(json!({"total_count":1,"workflow_runs":[discovered]}));
        provider.workflow_path = Some(path);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        let result = observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut FakeClock::default(),
            &Cancellation::default(),
        );
        assert_eq!(
            result.is_ok(),
            matches!(
                suffix,
                "@feature" | "@refs/heads/feature" | "@feature@other"
            ),
            "{suffix}"
        );
    }
}

#[test]
fn polling_inventory_must_fit_the_aggregate_request_allowance() {
    let (mut spec, _, _) = fixture();
    spec.timeout_seconds = 20700;
    spec.poll_seconds = 1;
    assert!(spec.validate().is_err());
    spec.poll_seconds = 60;
    assert!(spec.validate().is_ok());
    for n in 1..=10 {
        let mut candidate = spec.clone();
        for i in 1..n {
            candidate
                .workflows
                .insert(format!(".github/workflows/{i}.yaml"), vec!["test".into()]);
        }
        assert!(candidate.validate().is_ok());
        candidate.poll_seconds = 1;
        assert!(candidate.validate().is_err());
    }
}

#[test]
fn oversized_aggregate_jobs_cannot_publish_a_complete_packet() {
    let (mut spec, identity, mut provider) = fixture();
    let mut steps = Vec::new();
    for number in 1..=900 {
        steps.push(
            json!({"number":number,"name":"Test","status":"completed","conclusion":"success"}),
        );
    }
    let jobs=(1..=180).map(|id|json!({"id":id,"run_id":12,"run_attempt":1,"head_sha":"a".repeat(40),"name":format!("job-{id}"),"status":"completed","conclusion":"success","steps":steps})).collect::<Vec<_>>();
    spec.workflows.insert(
        ".github/workflows/ci.yaml".into(),
        (1..=180).map(|id| format!("job-{id}")).collect(),
    );
    provider.jobs = Some(json!({"total_count":180,"jobs":jobs}));
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
    discovery: Option<Value>,
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
        } else if endpoint.contains("actions/runs?") {
            serde_json::to_vec(
                &self
                    .discovery
                    .clone()
                    .unwrap_or_else(|| json!({"total_count":1,"workflow_runs":[run()]})),
            )?
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
                path.strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|component| component.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/"),
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
    (
        Uploaded {
            archive,
            metadata,
            discovery: None,
        },
        digest,
    )
}

#[test]
fn retention_reselects_originals_and_rejects_new_exact_target_duplicates() {
    for kind in ["duplicate", "missing", "replacement", "other-target"] {
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
        let mut extra = run();
        extra["id"] = json!(13);
        let runs = match kind {
            "duplicate" => vec![run(), extra],
            "missing" => vec![],
            "replacement" => vec![extra],
            "other-target" => {
                extra["head_branch"] = json!("other-branch");
                vec![run(), extra]
            }
            _ => unreachable!(),
        };
        upload.discovery = Some(json!({"total_count":runs.len(),"workflow_runs":runs}));
        let result = verify_retention(
            &spec,
            &identity,
            &root,
            42,
            &digest,
            &mut upload,
            &Cancellation::default(),
        );
        assert_eq!(result.is_ok(), kind == "other-target", "{kind}: {result:?}");
        assert_eq!(std::fs::read(root.join("receipt.json")).unwrap(), original);
    }
}

#[test]
fn native_retention_binds_actual_provider_zip_to_local_members_and_independent_source() {
    let temp = tempfile::tempdir().unwrap();
    let foreign = temp.path().join("packet.retention");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("sentinel"), b"unrelated diagnostics").unwrap();
    for packet in ["packet.v1", "packet.v2"] {
        let (spec, identity, mut provider) = fixture();
        let root = temp.path().join(packet);
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
        let captured = receipt(&root);
        assert!(
            captured["members"]
                .as_object()
                .unwrap()
                .keys()
                .all(|name| !name.contains('\\'))
        );
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
            temp.path()
                .join(format!("{packet}.retention"))
                .join("retention.json")
                .is_file()
        );
        assert_eq!(std::fs::read(root.join("receipt.json")).unwrap(), original);
        assert_eq!(
            std::fs::read(foreign.join("sentinel")).unwrap(),
            b"unrelated diagnostics"
        );
    }
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
