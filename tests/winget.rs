use std::fs;
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

[package.metadata.simit.winget]
package_id = "Example.DemoApp"
download_repo = "example/demo-app"
zip_archive = "demo-app-{version}-x86_64-windows.zip"
"#,
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    temp
}

#[test]
fn submit_dry_run_prints_wingetcreate_command_without_secret() {
    let temp = init_package();

    let output = simit()
        .current_dir(temp.path())
        .env("WINGET_PAT", "secret-token")
        .args([
            "dist",
            "winget",
            "submit",
            "--version",
            "1.2.3",
            "--dry-run",
            "--wine",
            "fake-wine",
            "--wingetcreate",
            temp.path().join("fake-wingetcreate.exe").to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fake-wine "));
    assert!(stdout.contains("fake-wingetcreate.exe update Example.DemoApp"));
    assert!(stdout.contains("--version 1.2.3"));
    assert!(stdout.contains("https://codeberg.org/example/demo-app/releases/download/1.2.3/demo-app-1.2.3-x86_64-windows.zip|x64"));
    assert!(stdout.contains("--token $WINGET_PAT --submit"));
    assert!(!stdout.contains("secret-token"));
}
