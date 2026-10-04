use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

mod common;

fn project(config: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"player\"\nversion = \"0.2.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("simit.toml"), config).unwrap();
    dir
}

fn render(root: &Path) -> String {
    let output = common::simit()
        .current_dir(root)
        .args(["init", "release", "--print"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

const HOSTED: &str = "[release.codeberg]\nrepo = \"example/player\"\n\n[release.artifacts]\nrunner = \"atlas-nix-trusted\"\nprebuild_binaries = true\nsign = false\n";

#[test]
fn prefixed_tags_keep_manifest_versions_unprefixed() {
    let dir = project(&format!("[release]\ntag_prefix = \"v\"\n\n{HOSTED}"));
    let workflow = render(dir.path());
    assert!(workflow.contains("\"v[0-9]*\""));
    assert!(workflow.contains("git verify-tag \"$TAG\""));
    assert!(workflow.contains("--arg tag \"$TAG\""));
    assert!(workflow.contains("(.version == $version)"));
    assert!(workflow.contains("VERSION=\"${TAG#v}\""));
}

#[test]
fn disabling_changelog_body_removes_the_changelog_precondition() {
    let dir = project(&HOSTED.replace(
        "repo = \"example/player\"",
        "repo = \"example/player\"\nbody_from_changelog = false",
    ));
    let workflow = render(dir.path());
    assert!(!workflow.contains("CHANGELOG.md"));
    assert!(workflow.contains("git verify-tag"));
}

#[test]
fn required_gates_block_publication_on_the_validated_revision() {
    let dir = project(&format!(
        "[ci]\nrequired_gates = [{{ id = \"player-tests\", run = \"nix run .#player-tests\", timeout_minutes = 90, env = {{ WOW_DATA = \"\" }} }}]\n\n{HOSTED}"
    ));
    let workflow = render(dir.path());
    let checkout = workflow.find("git checkout --detach").unwrap();
    let gate = workflow.find("nix run .#player-tests").unwrap();
    let upload = workflow.find("Publish Forgejo release").unwrap();
    assert!(checkout < gate && gate < upload);
    assert!(workflow.contains("timeout-minutes: 90"));
    assert!(workflow.contains("WOW_DATA:"));
}

#[test]
fn unsafe_tag_prefix_is_rejected() {
    let dir = project(&format!("[release]\ntag_prefix = \"../\"\n\n{HOSTED}"));
    let output = common::simit()
        .current_dir(dir.path())
        .args(["init", "release", "--print"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tag_prefix"));
}

#[test]
fn binary_verification_does_not_request_crate_publication() {
    let dir = project(&format!("[release.notes]\nsource = \"git\"\n\n{HOSTED}"));
    let output = common::simit()
        .current_dir(dir.path())
        .args(["release", "verify", "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["results"].as_array().unwrap();
    assert!(
        !checks
            .iter()
            .any(|c| c["check"].as_str().unwrap().starts_with("crates.io"))
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CRATES_IO_API_TOKEN"));
    assert!(
        !checks
            .iter()
            .any(|c| c["check"] == "CHANGELOG entry exists")
    );
}

#[test]
fn git_notes_are_bounded_by_the_release_tag() {
    let dir = project(&format!(
        "[release]\ntag_prefix = \"v\"\n\n[release.notes]\nsource = \"git\"\n\n{HOSTED}"
    ));
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "core.hooksPath", "/dev/null"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-qm",
        "Initial player",
    ]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.1.0"]);
    fs::write(dir.path().join("change"), "release").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-qm",
        "Improve lighting",
    ]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.2.0"]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.2.1"]);
    fs::write(dir.path().join("change"), "later").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-qm",
        "Unreleased work",
    ]);
    git(&["-c", "tag.gpgSign=false", "tag", "v0.1.9"]);
    let workflow = render(dir.path());
    let start = workflow
        .find("      - name: Generate release notes\n")
        .unwrap();
    let step = &workflow[start..];
    let end = step[1..].find("\n      - ").map_or(step.len(), |i| i + 1);
    let script = step[..end]
        .split("        run: |\n")
        .nth(1)
        .unwrap()
        .lines()
        .map(|l| l.strip_prefix("          ").unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        dir.path().join("release-env"),
        "TAG=v0.2.0\nVERSION=0.2.0\nIS_PRERELEASE=false\n",
    )
    .unwrap();
    let output = Command::new("bash")
        .current_dir(dir.path())
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let notes = fs::read_to_string(dir.path().join("release-notes.md")).unwrap();
    assert!(notes.contains("Improve lighting"));
    assert!(!notes.contains("Initial player"));
    assert!(!notes.contains("Unreleased work"));
    fs::write(
        dir.path().join("release-env"),
        "TAG=v0.1.0\nVERSION=0.1.0\nIS_PRERELEASE=false\n",
    )
    .unwrap();
    let output = Command::new("bash")
        .current_dir(dir.path())
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(output.status.success());
    let notes = fs::read_to_string(dir.path().join("release-notes.md")).unwrap();
    assert!(notes.contains("Initial player"));
    assert!(!notes.contains("Improve lighting"));
}

#[test]
fn package_downloads_use_tags_but_archive_names_use_versions() {
    let dir = project(
        "[release]\ntag_prefix = \"v\"\n\n[homebrew]\ndescription = \"Player\"\nhomepage = \"https://example.com/player\"\nlicense = \"MIT\"\ndownload_repo = \"example/player\"\ntap_url = \"https://example.com/tap.git\"\n",
    );
    let output = common::simit()
        .current_dir(dir.path())
        .args(["dist", "homebrew", "render", "--version", "0.2.0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let formula = String::from_utf8(output.stdout).unwrap();
    assert!(formula.contains("/releases/download/v0.2.0/player-0.2.0-"));
}
