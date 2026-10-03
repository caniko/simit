use std::process::Command;

#[test]
fn review_example_and_validation_work_without_project_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let example = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["review", "example"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        example.status.success(),
        "{}",
        String::from_utf8_lossy(&example.stderr)
    );
    let request = temp.path().join("request.json");
    std::fs::write(&request, &example.stdout).unwrap();
    let validation = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["review", "validate"])
        .arg(&request)
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(validation.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&example.stdout).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&validation.stdout).unwrap()
    );
}
