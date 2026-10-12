use std::{fs, process::Command};

#[test]
fn generated_native_publisher_preserves_exact_artifact_and_registry_contracts() {
    let source = tempfile::TempDir::new().unwrap();
    let path = source.path().join("publisher.py");
    fs::write(&path, include_str!("../src/monorepo/native_release.py")).unwrap();
    let output = Command::new("python3")
        .args([
            "-I",
            "-c",
            include_str!("fixtures/native-release-contract.py"),
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}
