#![cfg(unix)]

use simit::runtime_admission::{AdmissionOutcome, AdmissionSpec, Cancellation, Probe, admit};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

fn spec(script: &str) -> AdmissionSpec {
    let shell = std::process::Command::new("sh")
        .args(["-c", "command -v sh"])
        .output()
        .unwrap();
    AdmissionSpec {
        schema_version: 1,
        probe: Probe {
            executable: PathBuf::from(String::from_utf8(shell.stdout).unwrap().trim()),
            args: vec!["-c".into(), script.into()],
            environment: BTreeMap::new(),
            working_directory: PathBuf::from("/"),
        },
        retryable_statuses: vec![7],
        max_attempts: 3,
        backoff_ms: 10,
        attempt_timeout_ms: 200,
        overall_timeout_ms: 1000,
        stdout_bytes: 1024,
        stderr_bytes: 1024,
    }
}

#[test]
fn restoration_within_budget_admits_but_success_is_never_cached() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("observations");
    let mut input = spec("printf x >> \"$MARKER\"; [ $(wc -c < \"$MARKER\") -ge 2 ] || exit 7");
    input
        .probe
        .environment
        .insert("MARKER".into(), marker.display().to_string());
    // Explicit environment; the shell's utilities need an explicit PATH.
    input
        .probe
        .environment
        .insert("PATH".into(), std::env::var("PATH").unwrap());
    let validated = input.validate().unwrap();
    let first = admit(
        &validated,
        &root.path().join("first"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(first.outcome, AdmissionOutcome::Admitted);
    assert_eq!(first.attempts.len(), 2);
    let second = admit(
        &validated,
        &root.path().join("second"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(second.outcome, AdmissionOutcome::Admitted);
    assert_eq!(std::fs::read(marker).unwrap(), b"xxx");
    assert!(first.finished_unix_ms >= first.started_unix_ms);
}

#[test]
fn exhausted_denial_is_terminal_and_a_new_invocation_has_a_fresh_budget() {
    let root = tempfile::tempdir().unwrap();
    let validated = spec("exit 7").validate().unwrap();
    for name in ["first", "new-request"] {
        let receipt = admit(
            &validated,
            &root.path().join(name),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(receipt.outcome, AdmissionOutcome::AttemptsExhausted);
        assert_eq!(receipt.attempts.len(), 3);
    }
    assert!(
        admit(
            &validated,
            &root.path().join("first"),
            &Cancellation::default()
        )
        .is_err()
    );
}

#[test]
fn only_enrolled_exit_statuses_retry() {
    let root = tempfile::tempdir().unwrap();
    let validated = spec("printf 'group process exceeded time limit' >&2; exit 9")
        .validate()
        .unwrap();
    let receipt = admit(
        &validated,
        &root.path().join("gate"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::Denied { status: 9 });
    assert_eq!(receipt.attempts.len(), 1);
}

#[test]
fn aggregate_byte_budget_cannot_reset_on_retries() {
    let root = tempfile::tempdir().unwrap();
    let mut input = spec("printf 12345; exit 7");
    input.stdout_bytes = 8;
    let receipt = admit(
        &input.validate().unwrap(),
        &root.path().join("gate"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::OutputExhausted);
    assert_eq!(receipt.attempts.len(), 2);
    let retained: u64 = receipt
        .attempts
        .iter()
        .map(|attempt| attempt.stdout_bytes)
        .sum();
    assert_eq!(retained, 8);
}

#[test]
fn cancellation_interrupts_backoff_without_another_probe() {
    let root = tempfile::tempdir().unwrap();
    let mut input = spec("exit 7");
    input.backoff_ms = 800;
    let token = Cancellation::default();
    let validated = input.validate().unwrap();
    let gate = root.path().join("gate");
    std::thread::scope(|scope| {
        let start = Instant::now();
        let worker = scope.spawn(|| admit(&validated, &gate, &token).unwrap());
        while !gate.join("attempt-1.stderr").exists() {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(60));
        token.cancel();
        let receipt = worker.join().unwrap();
        assert_eq!(receipt.outcome, AdmissionOutcome::Cancelled);
        assert_eq!(receipt.attempts.len(), 1);
        assert!(start.elapsed() < Duration::from_millis(600));
    });
}

#[test]
fn hanging_and_missing_probes_are_terminal_without_retries() {
    let root = tempfile::tempdir().unwrap();
    let mut input = spec("while :; do :; done");
    input.attempt_timeout_ms = 50;
    let receipt = admit(
        &input.clone().validate().unwrap(),
        &root.path().join("hang"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::ProbeTimedOut);
    assert_eq!(receipt.attempts.len(), 1);
    input.probe.executable = root.path().join("missing");
    let receipt = admit(
        &input.validate().unwrap(),
        &root.path().join("missing-probe"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::ProcessFailed);
    assert_eq!(receipt.attempts.len(), 1);
}

#[test]
fn invalid_specs_reject_before_effects_and_inherited_env_is_scrubbed() {
    let mut input = spec("[ -z \"${HOME+x}\" ]");
    input.retryable_statuses = vec![0];
    assert!(input.clone().validate().is_err());
    input.retryable_statuses = vec![7];
    input.probe.args.push("x".repeat(65536));
    assert!(input.validate().is_err());
    let root = tempfile::tempdir().unwrap();
    let receipt = admit(
        &spec("[ -z \"${HOME+x}\" ]").validate().unwrap(),
        &root.path().join("gate"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::Admitted);
}

#[test]
fn overall_deadline_includes_backoff_and_clamps_the_last_probe() {
    let root = tempfile::tempdir().unwrap();
    let mut input = spec("exit 7");
    input.overall_timeout_ms = 100;
    input.attempt_timeout_ms = 100;
    input.backoff_ms = 100;
    let receipt = admit(
        &input.validate().unwrap(),
        &root.path().join("backoff"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::DeadlineExhausted);
    assert_eq!(receipt.attempts.len(), 1);
    let mut input = spec("while :; do :; done");
    input.overall_timeout_ms = 60;
    input.attempt_timeout_ms = 60;
    let receipt = admit(
        &input.validate().unwrap(),
        &root.path().join("last-probe"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::DeadlineExhausted);
    assert_eq!(receipt.attempts.len(), 1);
    assert!(receipt.elapsed_ms < 600);
}

#[test]
fn versioned_reader_rejects_unknown_fields_and_oversized_input() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("spec.json");
    let mut json = serde_json::to_value(spec("exit 0")).unwrap();
    json["cached_success"] = true.into();
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(AdmissionSpec::read(&path).is_err());
    std::fs::write(&path, vec![b' '; 65537]).unwrap();
    assert!(AdmissionSpec::read(&path).is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
#[cfg(feature = "runtime-admission-cli")]
fn independent_cli_uses_the_same_spec_receipt_and_failure_contract() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("spec.json");
    for (name, script, expected) in [
        ("allowed", "exit 0", AdmissionOutcome::Admitted),
        ("denied", "exit 9", AdmissionOutcome::Denied { status: 9 }),
    ] {
        std::fs::write(&path, serde_json::to_vec(&spec(script)).unwrap()).unwrap();
        let evidence = root.path().join(name);
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_simit"))
            .args(["runtime", "admit", "--spec"])
            .arg(&path)
            .arg("--evidence")
            .arg(&evidence)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            expected == AdmissionOutcome::Admitted
        );
        let receipt: simit::runtime_admission::AdmissionReceipt =
            serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt.outcome, expected);
        let persisted: simit::runtime_admission::AdmissionReceipt =
            serde_json::from_slice(&std::fs::read(evidence.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(persisted.spec_sha256, receipt.spec_sha256);
        assert_eq!(persisted.outcome, expected);
    }
}

#[test]
#[cfg(feature = "runtime-admission-cli")]
fn sigterm_cancels_the_cli_and_retains_a_terminal_receipt() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("spec.json");
    let mut input = spec("while :; do :; done");
    input.attempt_timeout_ms = 1000;
    let evidence = root.path().join("gate");
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["runtime", "admit", "--spec"])
        .arg(&path)
        .arg("--evidence")
        .arg(&evidence)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !evidence.join("attempt-1.stderr").exists() {
        if start.elapsed() >= Duration::from_secs(2) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI did not reach the probe");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() >= Duration::from_secs(2) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI did not terminate after cancellation");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let receipt: simit::runtime_admission::AdmissionReceipt =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::Cancelled);
    assert_eq!(receipt.attempts.len(), 1);
}

#[test]
fn signal_exit_is_terminal_and_not_a_retryable_positive_exit_code() {
    let root = tempfile::tempdir().unwrap();
    let receipt = admit(
        &spec("kill -TERM $$").validate().unwrap(),
        &root.path().join("gate"),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, AdmissionOutcome::Denied { status: -15 });
    assert_eq!(receipt.attempts.len(), 1);
}

#[test]
fn late_success_and_retryable_exit_never_cross_the_per_probe_deadline() {
    let root = tempfile::tempdir().unwrap();
    let sleep = std::process::Command::new("sh")
        .args(["-c", "command -v sleep"])
        .output()
        .unwrap();
    let sleep = String::from_utf8(sleep.stdout).unwrap();
    for status in [0, 7] {
        let mut input = spec("\"$SLEEP\" 0.025; exit \"$STATUS\"");
        input
            .probe
            .environment
            .insert("SLEEP".into(), sleep.trim().into());
        input
            .probe
            .environment
            .insert("STATUS".into(), status.to_string());
        input.attempt_timeout_ms = 21;
        let receipt = admit(
            &input.validate().unwrap(),
            &root.path().join(format!("late-{status}")),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(receipt.outcome, AdmissionOutcome::ProbeTimedOut);
        assert_eq!(receipt.attempts.len(), 1);
    }
}
