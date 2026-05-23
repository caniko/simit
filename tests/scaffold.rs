use std::fs;

use simit::commands::scaffold::{ArtifactCheck, redact_url, shell_word};
use tempfile::TempDir;

#[test]
fn redact_url_strips_userinfo() {
    assert_eq!(
        redact_url("https://user:pass@example.com/owner/repo.git"),
        "https://example.com/owner/repo.git"
    );
    assert_eq!(
        redact_url("https://token@example.com/owner/repo.git"),
        "https://example.com/owner/repo.git"
    );
    assert_eq!(
        redact_url("git@example.com:owner/repo.git"),
        "git@example.com:owner/repo.git"
    );
}

#[test]
fn shell_word_leaves_safe_values_bare_and_quotes_funky_values() {
    assert_eq!(shell_word("/tmp/simit-1.2.3_ok"), "/tmp/simit-1.2.3_ok");
    assert_eq!(shell_word("two words"), "'two words'");
    assert_eq!(shell_word("can't"), "'can'\\''t'");
    assert_eq!(shell_word(""), "''");
}

#[test]
fn artifact_check_verify_succeeds_on_match() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("artifact.txt");
    fs::write(&path, "expected\n").unwrap();

    ArtifactCheck {
        label: "Test artifact",
        path: &path,
        expected: "expected\n",
        remediation: "run `simit test`",
    }
    .verify(false)
    .unwrap();
}

#[test]
fn artifact_check_verify_errors_on_mismatch() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("artifact.txt");
    fs::write(&path, "actual\n").unwrap();

    let err = ArtifactCheck {
        label: "Test artifact",
        path: &path,
        expected: "expected\n",
        remediation: "run `simit test`",
    }
    .verify(true)
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("Test artifact is not up to date"));
    assert!(message.contains("artifact.txt differs"));
    assert!(message.contains("--- "));
}

#[test]
fn artifact_check_verify_errors_on_missing_file() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("missing.txt");

    let err = ArtifactCheck {
        label: "Test artifact",
        path: &path,
        expected: "expected\n",
        remediation: "run `simit test`",
    }
    .verify(false)
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("Test artifact is not up to date"));
    assert!(message.contains("missing.txt is missing"));
}
