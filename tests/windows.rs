use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn init_package() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        r#"[package]
name = "demo-app"
version = "0.1.0"
edition = "2024"
license = "MIT"
authors = ["Demo Author"]
description = "Demo app"
homepage = "https://example.com"
"#,
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        r#"[chocolatey]
name = "demo-app"
download_repo = "example/demo-app"

[scoop]
name = "demo-app"
bucket_url = "https://codeberg.org/example/scoop-demo-app.git"
download_repo = "example/demo-app"
binaries = ["demo-app"]

[scoop.architectures]
arm64 = false

[winget]
package_id = "Example.DemoApp"
download_repo = "example/demo-app"
zip_archive = "demo-app-{version}-x86_64-windows.zip"
"#,
    )
    .unwrap();
    temp
}

fn archive(root: &Path) -> String {
    let path = root.join("x64.zip");
    fs::write(&path, b"x64\n").unwrap();
    path.display().to_string()
}

#[test]
fn windows_publish_dry_run_plans_configured_channels() {
    let temp = init_package();
    let x64 = archive(temp.path());

    let output = simit()
        .current_dir(temp.path())
        .env("WINGET_PAT", "secret-token")
        .args([
            "dist",
            "windows",
            "publish",
            "--version",
            "1.2.3",
            "--archive",
            &format!("x64={x64}"),
            "--work-dir",
            temp.path().join("work").to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("would write Chocolatey package demo-app 1.2.3"));
    assert!(stdout.contains("would write Scoop manifest demo-app 1.2.3"));
    assert!(stdout.contains("wine "));
    assert!(stdout.contains("wingetcreate.exe update Example.DemoApp"));
    assert!(!stdout.contains("secret-token"));
}
