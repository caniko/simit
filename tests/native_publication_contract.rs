use std::process::Command;

#[test]
fn protected_native_upload_admits_data_only_and_reuses_original_partial_archives() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("python3")
        .args(["-I"])
        .arg(root.join("tests/fixtures/native-publisher.py"))
        .arg(root.join("src/monorepo/native_release.py"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}
