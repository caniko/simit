mod common;

#[test]
fn aggregate_github_ci_coalesces_push_and_pr_without_fork_or_member_collisions() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"aggregate-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n[workspace]\n").unwrap();
    let result = common::simit()
        .current_dir(root)
        .args([
            "init",
            "ci",
            "--platform",
            "github",
            "--runtime",
            "cargo",
            "--workspace",
            "--workspace-strategy",
            "aggregate",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let raw = std::fs::read_to_string(root.join(".github/workflows/ci.yaml")).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    let group = yaml["concurrency"]["group"].as_str().unwrap();
    assert!(group.contains(".github/workflows/ci.yaml"), "{group}");
    assert!(
        group.contains("github.event.pull_request.head.repo.full_name || github.repository"),
        "{group}"
    );
    assert!(
        group.contains("github.event.pull_request.head.ref || github.ref_name"),
        "{group}"
    );
    assert!(
        !group.contains("github.event_name") && !group.contains("github.workflow_ref"),
        "{group}"
    );
    assert_eq!(
        yaml["concurrency"]["cancel-in-progress"].as_bool(),
        Some(true)
    );
}
