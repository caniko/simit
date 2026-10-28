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
fn submit_dry_run_describes_the_api_submission_without_secret() {
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
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Example.DemoApp 1.2.3"));
    assert!(stdout.contains("through the GitHub API with $WINGET_PAT"));
    assert!(stdout.contains("https://codeberg.org/example/demo-app/releases/download/1.2.3/demo-app-1.2.3-x86_64-windows.zip"));
    assert!(!stdout.contains("secret-token"));
    for gone in ["wine", "wingetcreate"] {
        assert!(!stdout.contains(gone));
    }
}

#[test]
fn submit_rejects_the_removed_wine_flags() {
    let temp = init_package();
    for flag in ["--wine", "--wingetcreate", "--wingetcreate-url"] {
        let output = simit()
            .current_dir(temp.path())
            .args(["dist", "winget", "submit", "--version", "1.2.3", flag, "x"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flag}");
    }
}
