//! Consumer regressions for the two retained Toolbelt observer failures.
use anyhow::Result;
use serde_json::{Value, json};
use simit::ci_runtime::{cancellation::Cancellation, observer::*};
use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    io::{Cursor, Write},
    path::Path,
    rc::Rc,
    time::Duration,
};

struct TestClock(Rc<Cell<Duration>>);

impl Clock for TestClock {
    fn elapsed(&self) -> Duration {
        self.0.get()
    }

    fn sleep(&mut self, duration: Duration, cancellation: &Cancellation) -> Result<()> {
        cancellation.check()?;
        self.0.set(self.0.get() + duration);
        Ok(())
    }
}

struct Provider {
    elapsed: Rc<Cell<Duration>>,
    ready_after: Vec<u64>,
    faults: BTreeMap<String, VecDeque<u16>>,
    run_reads: BTreeMap<u64, usize>,
    drift_after_capture: bool,
    branch: String,
    event: String,
    head_repository_id: u64,
    pull_requests: Vec<Value>,
    workflow_reference: Option<String>,
    jobs: Option<Vec<Value>>,
    logs: Vec<u8>,
    other_runs: Vec<Value>,
    download_seconds: u64,
}

impl Provider {
    fn workflow(index: usize) -> &'static str {
        [
            ".github/workflows/ci.yaml",
            ".github/workflows/nix-builds.yaml",
        ][index]
    }

    fn run(&self, index: usize) -> Value {
        let completed = self.elapsed.get().as_secs() >= self.ready_after[index];
        json!({
            "id": 12 + index as u64,
            "repository": {"id": 100, "full_name": "owner/repo"},
            "head_repository": {"id": self.head_repository_id, "full_name": "owner/repo"},
            "head_sha": "a".repeat(40),
            "head_branch": self.branch,
            "run_attempt": 1,
            "event": self.event,
            "pull_requests": self.pull_requests,
            "path": self.workflow_reference.as_ref().map_or_else(
                || Self::workflow(index).to_owned(),
                |reference| format!("{}@{reference}", Self::workflow(index))),
            "status": if completed { "completed" } else { "in_progress" },
            "conclusion": if completed { Some("success") } else { None },
            "html_url": format!("https://github.com/owner/repo/actions/runs/{}", 12 + index),
        })
    }
}

impl Transport for Provider {
    fn get(
        &mut self,
        endpoint: &str,
        maximum: u64,
        timeout: Duration,
        cancellation: &Cancellation,
    ) -> Result<Response> {
        cancellation.check()?;
        assert!(!timeout.is_zero());
        assert!(!endpoint.contains("rerun"));
        for (suffix, statuses) in &mut self.faults {
            if endpoint.contains(suffix.as_str())
                && let Some(status) = statuses.pop_front()
            {
                return Ok(Response {
                    status,
                    body: b"retained transient provider response".to_vec(),
                    retry_after: Some(2),
                    transport_error: None,
                });
            }
        }
        let body = if endpoint.contains("actions/runs?") {
            let mut runs = self.other_runs.clone();
            runs.extend((0..self.ready_after.len()).map(|i| self.run(i)));
            serde_json::to_vec(&json!({
                "total_count": runs.len(),
                "workflow_runs": runs
            }))?
        } else if endpoint.contains("actions/jobs/") && endpoint.ends_with("/logs") {
            b"original successful execution\n".to_vec()
        } else {
            let id: u64 = endpoint
                .split("actions/runs/")
                .nth(1)
                .unwrap()
                .split('/')
                .next()
                .unwrap()
                .parse()?;
            if endpoint.contains("/jobs?") {
                let jobs = self.jobs.clone().unwrap_or_else(|| vec![json!({
                    "id": id + 100,
                    "run_id": id,
                    "run_attempt": 1,
                    "head_sha": "a".repeat(40),
                    "name": "test",
                    "status": "completed",
                    "conclusion": "success",
                    "steps": [{"number": 1, "name": "Test", "status": "completed", "conclusion": "success"}]
                })]);
                let page: usize = endpoint.split("&page=").nth(1).unwrap().parse()?;
                let start = (page - 1) * 100;
                let end = (start + 100).min(jobs.len());
                serde_json::to_vec(&json!({"total_count": jobs.len(), "jobs": &jobs[start..end]}))?
            } else if endpoint.ends_with("/logs") {
                anyhow::ensure!(
                    timeout > Duration::from_secs(self.download_seconds),
                    "download request timeout too short"
                );
                self.elapsed
                    .set(self.elapsed.get() + Duration::from_secs(self.download_seconds));
                self.logs.clone()
            } else {
                let reads = self.run_reads.entry(id).or_default();
                *reads += 1;
                let drift = self.drift_after_capture && *reads >= 3;
                let mut run = self.run((id - 12) as usize);
                if drift {
                    run["run_attempt"] = json!(2);
                }
                serde_json::to_vec(&run)?
            }
        };
        assert!(body.len() as u64 <= maximum);
        Ok(Response {
            status: 200,
            body,
            retry_after: None,
            transport_error: None,
        })
    }
}

fn fixture(ready_after: Vec<u64>, timeout_seconds: u64) -> (Spec, Identity, Provider, TestClock) {
    let elapsed = Rc::new(Cell::new(Duration::ZERO));
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["0_test.txt", "test/1_Test.txt"] {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(b"original successful execution\n")
            .unwrap();
    }
    let workflows = (0..ready_after.len())
        .map(|i| (Provider::workflow(i).to_owned(), vec!["test".to_owned()]))
        .collect();
    let spec = Spec {
        schema: "simit.original-observer-spec.v1".into(),
        repository: "owner/repo".into(),
        workflows,
        timeout_seconds,
        poll_seconds: 60,
    };
    let identity = Identity {
        repository_id:100,
        head: "a".repeat(40),
        event: "push".into(),
        run_id: 99,
        run_attempt: 1,
        workflow_ref: "owner/repo/.github/workflows/custody.yaml@refs/heads/feature".into(),
        workflow_sha: "a".repeat(40),
        event_json: json!({"after": "a".repeat(40), "ref": "refs/heads/feature", "deleted": false,"repository":{"id":100,"full_name":"owner/repo"}})
            .to_string(),
    };
    let provider = Provider {
        elapsed: Rc::clone(&elapsed),
        ready_after,
        faults: BTreeMap::new(),
        run_reads: BTreeMap::new(),
        drift_after_capture: false,
        branch: "feature".into(),
        event: "push".into(),
        head_repository_id: 100,
        pull_requests: Vec::new(),
        workflow_reference: None,
        jobs: None,
        logs: archive.finish().unwrap().into_inner(),
        other_runs: Vec::new(),
        download_seconds: 0,
    };
    (spec, identity, provider, TestClock(elapsed))
}

fn retained(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("receipt.json")).unwrap()).unwrap()
}

fn pr_fixture() -> (Spec, Identity, Provider, TestClock) {
    let (spec, mut identity, mut provider, clock) = fixture(vec![0], 120);
    let association = json!({
        "number": 71,
        "head": {"ref": "feature", "sha": "a".repeat(40), "repo": {"id": 100, "full_name": "owner/repo"}},
        "base": {"ref": "trunk", "sha": "b".repeat(40), "repo": {"id": 100, "full_name": "owner/repo"}}
    });
    identity.event = "pull_request".into();
    identity.workflow_sha = "c".repeat(40);
    identity.workflow_ref = "owner/repo/.github/workflows/custody.yaml@refs/pull/71/merge".into();
    identity.event_json = json!({"number": 71, "pull_request": association,"repository":{"id":100,"full_name":"owner/repo"}}).to_string();
    provider.event = "pull_request".into();
    provider.pull_requests = vec![association];
    (spec, identity, provider, clock)
}

#[test]
fn queue_and_four_hour_producer_fit_the_finite_capture_budget() {
    // Original PR custody started at 22:45; its CI finished at 03:23.
    let (spec, identity, mut provider, mut clock) = fixture(vec![300, 16_680], 20_700);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    let receipt = observe_with(
        &spec,
        &identity,
        &root,
        &mut provider,
        &mut clock,
        &Cancellation::default(),
    )
    .unwrap();
    assert!(receipt.capture_complete);
    assert!(!receipt.qualified);
    assert_eq!(receipt.producers.len(), 2);
    assert_eq!(clock.elapsed(), Duration::from_secs(16_680));
    assert!(clock.elapsed() < Duration::from_secs(spec.timeout_seconds));
}

#[test]
fn four_hour_expiry_preserves_completed_sibling_and_explicit_partial_receipt() {
    let (spec, identity, mut provider, mut clock) = fixture(vec![300, 16_680], 14_400);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut clock,
            &Cancellation::default(),
        )
        .is_err()
    );
    let receipt = retained(&root);
    assert_eq!(receipt["capture_complete"], false);
    assert_eq!(receipt["qualified"], false);
    assert_eq!(receipt["producers"].as_array().unwrap().len(), 1);
    assert!(receipt["error"].as_str().unwrap().contains("deadline"));
    assert_eq!(
        std::fs::read(root.join("12/provider-logs.zip")).unwrap(),
        provider.logs
    );
    assert_eq!(clock.elapsed(), Duration::from_secs(14_400));
}

#[test]
fn transient_jobs_and_logs_failures_preserve_responses_then_capture_original_bytes() {
    let (spec, identity, mut provider, mut clock) = fixture(vec![0], 120);
    provider
        .faults
        .insert("/jobs?".into(), VecDeque::from([500, 502]));
    provider
        .faults
        .insert("/logs".into(), VecDeque::from([503]));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    let receipt = observe_with(
        &spec,
        &identity,
        &root,
        &mut provider,
        &mut clock,
        &Cancellation::default(),
    )
    .unwrap();
    assert!(receipt.capture_complete);
    assert_eq!(
        std::fs::read(root.join("12/provider-logs.zip")).unwrap(),
        provider.logs
    );
    let responses = std::fs::read_dir(root.join("requests"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "body"))
        .filter(|path| std::fs::read(path).unwrap() == b"retained transient provider response")
        .count();
    assert_eq!(responses, 3);
}

#[test]
fn rerun_between_capture_and_final_revalidation_invalidates_the_packet() {
    let (spec, identity, mut provider, mut clock) = fixture(vec![0], 120);
    provider.drift_after_capture = true;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    assert!(
        observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut clock,
            &Cancellation::default(),
        )
        .is_err()
    );
    let receipt = retained(&root);
    assert_eq!(receipt["capture_complete"], false);
    assert_eq!(receipt["qualified"], false);
    assert_eq!(
        std::fs::read(root.join("12/provider-logs.zip")).unwrap(),
        provider.logs
    );
    assert!(receipt["error"].as_str().unwrap().contains("identity"));
}

#[test]
fn identical_sha_on_a_different_branch_cannot_supply_original_custody() {
    for branch in ["other-feature", "Feature", ""] {
        let (spec, identity, mut provider, mut clock) = fixture(vec![0], 120);
        provider.branch = branch.into();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        let result = observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut clock,
            &Cancellation::default(),
        );
        assert!(
            result.is_err(),
            "wrong-branch original was admitted: {result:?}"
        );
        assert_eq!(retained(&root)["capture_complete"], false);
    }
}

#[test]
fn exact_pr_target_captures_originals_independently_of_the_merge_workflow_sha() {
    let (spec, identity, mut provider, mut clock) = pr_fixture();
    let temp = tempfile::tempdir().unwrap();
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
    assert!(!receipt.qualified);
    assert_ne!(receipt.identity.head, receipt.identity.workflow_sha);
}

#[test]
fn same_head_on_a_different_pr_ref_repository_or_base_cannot_supply_custody() {
    for (pointer, value) in [
        ("/number", json!(72)),
        ("/head/ref", json!("other-feature")),
        ("/head/sha", json!("d".repeat(40))),
        ("/head/repo/id", json!(101)),
        ("/base/ref", json!("other-base")),
        ("/base/sha", json!("d".repeat(40))),
        ("/base/repo/id", json!(101)),
    ] {
        let (spec, identity, mut provider, mut clock) = pr_fixture();
        *provider.pull_requests[0].pointer_mut(pointer).unwrap() = value;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        let result = observe_with(
            &spec,
            &identity,
            &root,
            &mut provider,
            &mut clock,
            &Cancellation::default(),
        );
        assert!(
            result.is_err(),
            "substituted {pointer} was admitted: {result:?}"
        );
        assert_eq!(retained(&root)["capture_complete"], false);
    }
}

#[test]
fn missing_or_ambiguous_pr_association_cannot_become_branch_only_success() {
    for associations in [0, 2] {
        let (spec, identity, mut provider, mut clock) = pr_fixture();
        provider.pull_requests = vec![provider.pull_requests[0].clone(); associations];
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("packet");
        assert!(
            observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut clock,
                &Cancellation::default(),
            )
            .is_err()
        );
        assert_eq!(retained(&root)["capture_complete"], false);
    }
}

#[test]
fn paginated_jobs_must_fit_the_aggregate_retention_limit_before_capture_completes() {
    const JSON_LIMIT: usize = 16 * 1024 * 1024;
    let (mut spec, identity, mut provider, mut clock) = fixture(vec![0], 120);
    let steps: Vec<Value> = (1..=120)
        .map(|number| {
            json!({
                "number": number,
                "name": "x".repeat(700),
                "status": "completed",
                "conclusion": "success"
            })
        })
        .collect();
    let jobs: Vec<Value> = (0..200)
        .map(|index| {
            json!({
                "id": 1000 + index,
                "run_id": 12,
                "run_attempt": 1,
                "head_sha": "a".repeat(40),
                "name": format!("matrix-{index}"),
                "status": "completed",
                "conclusion": "success",
                "steps": steps
            })
        })
        .collect();
    for page in jobs.chunks(100) {
        assert!(
            serde_json::to_vec(&json!({"total_count": 200, "jobs": page}))
                .unwrap()
                .len()
                <= JSON_LIMIT
        );
    }
    assert!(serde_json::to_vec(&jobs).unwrap().len() > JSON_LIMIT);
    spec.workflows.insert(
        ".github/workflows/ci.yaml".into(),
        (0..200).map(|index| format!("matrix-{index}")).collect(),
    );
    provider.jobs = Some(jobs);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("packet");
    let result = observe_with(
        &spec,
        &identity,
        &root,
        &mut provider,
        &mut clock,
        &Cancellation::default(),
    );
    assert!(
        result.is_err(),
        "unverifiable aggregated jobs were admitted"
    );
    let receipt = retained(&root);
    assert_eq!(receipt["capture_complete"], false);
    assert!(
        receipt["error"]
            .as_str()
            .unwrap()
            .contains("aggregate jobs")
    );
    let partial_bytes = std::fs::metadata(root.join("12/jobs.json")).unwrap().len();
    assert!(partial_bytes > 0 && partial_bytes <= JSON_LIMIT as u64);
    let retained_job_pages = std::fs::read_dir(root.join("requests"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap())
        .filter(|metadata| metadata["endpoint"].as_str().unwrap().contains("/jobs?"))
        .count();
    assert_eq!(retained_job_pages, 2);
}

#[test]
fn workflow_reference_syntax_is_independent_of_the_bound_event_target() {
    for pull_request in [false, true] {
        let mut accepted = if pull_request {
            vec![
                "feature",
                "refs/heads/feature",
                "refs/pull/71/head",
                "refs/pull/71/merge",
            ]
        } else {
            vec!["feature", "refs/heads/feature"]
        };
        accepted.extend([
            "main",
            "refs/heads/other-feature",
            "refs/pull/29/merge",
            "feature@foo",
            "refs/tags/v1",
        ]);
        for reference in accepted {
            let (spec, identity, mut provider, mut clock) = if pull_request {
                pr_fixture()
            } else {
                fixture(vec![0], 120)
            };
            provider.workflow_reference = Some(reference.into());
            let temp = tempfile::tempdir().unwrap();
            observe_with(
                &spec,
                &identity,
                &temp.path().join("packet"),
                &mut provider,
                &mut clock,
                &Cancellation::default(),
            )
            .unwrap();
        }
        for reference in [
            "",
            "refs/heads/bad..ref",
            "refs//main",
            "refs/heads/.hidden",
            "refs/heads/main.lock",
            "refs/heads/bad ref",
            "refs/heads/main@{1}",
            "refs/heads/trailing.",
            "@",
        ] {
            let (spec, identity, mut provider, mut clock) = if pull_request {
                pr_fixture()
            } else {
                fixture(vec![0], 120)
            };
            provider.workflow_reference = Some(reference.into());
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("packet");
            let result = observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut clock,
                &Cancellation::default(),
            );
            assert!(
                result.is_err(),
                "malformed workflow reference {reference} was admitted (PR={pull_request})"
            );
            assert_eq!(retained(&root)["capture_complete"], false);
        }
    }
}

#[test]
fn same_sha_other_targets_do_not_hide_the_exact_original_or_its_duplicates() {
    for pull_request in [false, true] {
        for duplicate in [false, true] {
            let (spec, identity, mut provider, mut clock) = if pull_request {
                pr_fixture()
            } else {
                fixture(vec![0], 120)
            };
            let mut other = provider.run(0);
            other["id"] = json!(20);
            // Even an irrelevant workflow suffix must not abort exact-target discovery.
            other["path"] = json!(".github/workflows/ci.yaml@bad..ref");
            if pull_request {
                other["pull_requests"][0]["number"] = json!(72);
            } else {
                other["head_branch"] = json!("other-feature");
            }
            provider.other_runs.push(other);
            if duplicate {
                let mut exact_duplicate = provider.run(0);
                exact_duplicate["id"] = json!(21);
                provider.other_runs.push(exact_duplicate);
            }
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("packet");
            let outcome = observe_with(
                &spec,
                &identity,
                &root,
                &mut provider,
                &mut clock,
                &Cancellation::default(),
            );
            if duplicate {
                assert!(
                    outcome
                        .unwrap_err()
                        .to_string()
                        .contains("duplicate original producer")
                );
                assert_eq!(retained(&root)["capture_complete"], false);
            } else {
                let receipt = outcome.unwrap();
                assert!(receipt.capture_complete);
                assert_eq!(receipt.producers[0].run_id, 12);
                assert!(!provider.run_reads.contains_key(&20));
            }
        }
    }
}

#[test]
fn slow_healthy_log_download_can_use_the_remaining_aggregate_deadline() {
    let (spec, identity, mut provider, mut clock) = fixture(vec![0], 300);
    provider.download_seconds = 120;
    let temp = tempfile::tempdir().unwrap();
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
    assert_eq!(clock.elapsed(), Duration::from_secs(120));
}
