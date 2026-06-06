use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod common;

fn simit() -> Command {
    common::simit()
}

fn write_executable(path: &Path, body: &str) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    fs::write(path, format!("#!{shell}\n{body}")).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fake_tools() -> TempDir {
    let temp = TempDir::new().unwrap();
    write_executable(
        &temp.path().join("minisign"),
        r#"set -euo pipefail
mode=""
secret=""
pub=""
message=""
sig=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -G|-S|-V) mode="$1"; shift ;;
    -s) secret="$2"; shift 2 ;;
    -p) pub="$2"; shift 2 ;;
    -m) message="$2"; shift 2 ;;
    -x) sig="$2"; shift 2 ;;
    *) shift ;;
  esac
done
case "$mode" in
  -G)
    cat >/dev/null
    printf 'secret generated\n' > "$secret"
    printf 'public generated\n' > "$pub"
    ;;
  -S)
    cat >/dev/null
    test -s "$secret"
    test -s "$message"
    printf 'signature\n' > "$sig"
    ;;
  -V)
    test -s "$pub"
    test -s "$message"
    test -s "$sig"
    ;;
  *) exit 2 ;;
esac
"#,
    );
    write_executable(
        &temp.path().join("curl"),
        r#"set -euo pipefail
method="GET"
url=""
payload=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --config|-H) shift 2 ;;
    -fsS) shift ;;
    -X) method="$2"; shift 2 ;;
    --data-binary) payload="${2#@}"; shift 2 ;;
    http*) url="$1"; shift ;;
    *) shift ;;
  esac
done
if [ "$method" = "PUT" ]; then
  secret="${url##*/}"
  test -s "$payload"
  printf '%s\n' "$secret" >> "${FAKE_CURL_LOG:?}"
  exit 0
fi
printf '[{"name":"MINISIGN_SECRET_KEY"},{"name":"MINISIGN_PASSWORD"},{"name":"codeberg_token"}]\n'
"#,
    );
    temp
}

fn init_project() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2024"
"#,
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::create_dir(temp.path().join("keys")).unwrap();
    fs::write(temp.path().join("keys/minisign.pub"), "public existing\n").unwrap();
    temp
}

#[test]
fn imports_minisign_secrets_without_printing_values() {
    let project = init_project();
    let tools = fake_tools();
    let secret_key = project.path().join("minisign.sec");
    let password = project.path().join("minisign.password");
    let token = project.path().join("token");
    let log = project.path().join("curl.log");
    fs::write(&secret_key, "PRIVATE-SECRET-VALUE\n").unwrap();
    fs::write(&password, "PASSWORD-SECRET-VALUE\n").unwrap();
    fs::write(&token, "TOKEN-SECRET-VALUE\n").unwrap();

    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = simit()
        .current_dir(project.path())
        .env("PATH", path)
        .env("FAKE_CURL_LOG", &log)
        .args([
            "release",
            "secrets",
            "init",
            "--repo",
            "example/demo",
            "--token-file",
        ])
        .arg(&token)
        .arg("--minisign-secret-key-file")
        .arg(&secret_key)
        .arg("--minisign-password-file")
        .arg(&password)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("uploaded release secrets for example/demo"));
    assert!(!stdout.contains("PRIVATE-SECRET-VALUE"));
    assert!(!stdout.contains("PASSWORD-SECRET-VALUE"));
    assert!(!stdout.contains("TOKEN-SECRET-VALUE"));
    assert!(!stderr.contains("PRIVATE-SECRET-VALUE"));
    assert!(!stderr.contains("PASSWORD-SECRET-VALUE"));
    assert!(!stderr.contains("TOKEN-SECRET-VALUE"));
    let log = fs::read_to_string(log).unwrap();
    assert!(log.contains("MINISIGN_SECRET_KEY"));
    assert!(log.contains("MINISIGN_PASSWORD"));
}

#[test]
fn rotate_minisign_updates_public_key_only_with_flag() {
    let project = init_project();
    let tools = fake_tools();
    let token = project.path().join("token");
    let log = project.path().join("curl.log");
    fs::write(&token, "TOKEN-SECRET-VALUE\n").unwrap();
    let before = fs::read_to_string(project.path().join("keys/minisign.pub")).unwrap();
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let failed = simit()
        .current_dir(project.path())
        .env("PATH", &path)
        .env("FAKE_CURL_LOG", &log)
        .args([
            "release",
            "secrets",
            "init",
            "--repo",
            "example/demo",
            "--token-file",
        ])
        .arg(&token)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(
        fs::read_to_string(project.path().join("keys/minisign.pub")).unwrap(),
        before
    );

    let rotated = simit()
        .current_dir(project.path())
        .env("PATH", path)
        .env("FAKE_CURL_LOG", &log)
        .args([
            "release",
            "secrets",
            "init",
            "--repo",
            "example/demo",
            "--token-file",
        ])
        .arg(&token)
        .arg("--rotate-minisign")
        .output()
        .unwrap();
    assert!(
        rotated.status.success(),
        "{}",
        String::from_utf8_lossy(&rotated.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.path().join("keys/minisign.pub")).unwrap(),
        "public generated\n"
    );
}

#[test]
fn check_accepts_account_level_codeberg_token() {
    let project = init_project();
    let tools = fake_tools();
    let token = project.path().join("token");
    fs::write(&token, "TOKEN-SECRET-VALUE\n").unwrap();
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = simit()
        .current_dir(project.path())
        .env("PATH", path)
        .args([
            "release",
            "secrets",
            "check",
            "--repo",
            "example/demo",
            "--token-file",
        ])
        .arg(&token)
        .arg("--assume-account-secret")
        .arg("codeberg_token")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
