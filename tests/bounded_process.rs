#![cfg(unix)]

use simit::{
    bounded_process::{Limits, ProcessError, execute},
    ci_runtime::cancellation::Cancellation,
};
use std::{process::Command, time::Duration};

#[test]
fn callers_select_deadlines_and_receive_typed_failures() {
    let root = tempfile::tempdir().unwrap();
    // Blocked-descriptor unit fixtures prove descendant cleanup. This public
    // boundary checks typed deadlines without a scheduling-sensitive side effect.
    let result = execute(
        Command::new("sh").args(["-c", "exec sleep 10"]),
        &root.path().join("stdout"),
        &root.path().join("stderr"),
        &Cancellation::default(),
        Limits {
            timeout: Duration::from_millis(50),
            stdout: 32,
            stderr: 128,
        },
    );
    assert!(matches!(result, Err(ProcessError::Timeout)));
}

#[test]
fn byte_overflow_is_terminal_and_never_inferred_from_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(
        Command::new("sh").args(["-c", "printf '%0100d' 0"]),
        &root.path().join("stdout"),
        &root.path().join("stderr"),
        &Cancellation::default(),
        Limits {
            timeout: Duration::from_secs(1),
            stdout: 8,
            stderr: 128,
        },
    );
    assert!(matches!(result, Err(ProcessError::OutputLimit)));
    assert_eq!(
        std::fs::metadata(root.path().join("stdout")).unwrap().len(),
        8
    );
}

#[test]
fn cancellation_before_dispatch_creates_no_evidence_or_process() {
    let root = tempfile::tempdir().unwrap();
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let result = execute(
        Command::new("sh").args(["-c", "exit 0"]),
        &root.path().join("stdout"),
        &root.path().join("stderr"),
        &cancellation,
        Limits {
            timeout: Duration::from_secs(1),
            stdout: 32,
            stderr: 128,
        },
    );
    assert!(matches!(result, Err(ProcessError::Cancelled)));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn invalid_limits_reject_before_evidence_or_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(
        Command::new("sh").args(["-c", "exit 0"]),
        &root.path().join("stdout"),
        &root.path().join("stderr"),
        &Cancellation::default(),
        Limits {
            timeout: Duration::ZERO,
            stdout: 8,
            stderr: 8,
        },
    );
    assert!(matches!(result, Err(ProcessError::InvalidLimits(_))));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
