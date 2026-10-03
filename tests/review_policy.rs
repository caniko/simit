use simit::{config::ProjectConfig, render::review_policy};
mod common;

#[test]
fn generated_policy_is_owned_and_drift_checked_with_ordinary_ci() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.path().join("simit.toml"), "[ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"cargo\"\n[review_policy]\ntoolbelt_version = \"0.2.0\"\napp_id_secret = \"APP_ID\"\napp_private_key_secret = \"APP_KEY\"\n").unwrap();
    for args in [vec!["init", "ci"], vec!["init", "ci", "--check", "--diff"]] {
        let output = common::simit()
            .current_dir(root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let workflow = root.path().join(review_policy::PATH);
    let contents = std::fs::read_to_string(&workflow).unwrap();
    std::fs::write(
        workflow,
        contents.replace("--version =0.2.0", "--version =0.3.0"),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(root.path())
        .args(["init", "ci", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("review-policy.yaml")
            || String::from_utf8_lossy(&output.stderr).contains("review-policy.yaml")
    );
}

#[test]
fn review_policy_uses_trusted_source_and_a_dedicated_app() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("simit.toml"),
        r#"
[ci]
platform = "github"
provider = "actions"
[review_policy]
toolbelt_version = "0.2.0"
app_id_secret = "REVIEW_POLICY_APP_ID"
app_private_key_secret = "REVIEW_POLICY_APP_PRIVATE_KEY"
"#,
    )
    .unwrap();
    let config = ProjectConfig::load(root.path()).unwrap();
    let file = review_policy::file(config.review_policy.as_ref().unwrap()).unwrap();
    assert_eq!(
        file.relative_path.to_str(),
        Some(".github/workflows/review-policy.yaml")
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(&file.content).unwrap();
    assert!(yaml["on"]["pull_request_target"].is_mapping());
    assert!(yaml["on"]["schedule"].is_sequence());
    assert!(file.content.contains(
        "github.ref == format('refs/heads/{0}', github.event.repository.default_branch)"
    ));
    assert!(file.content.contains("ref: ${{ github.sha }}"));
    assert!(
        file.content
            .contains("cancel-in-progress: ${{ github.event_name != 'schedule' }}")
    );
    assert!(!file.content.contains("items.length >= 100"));
    assert!(file.content.contains("permission-checks: write"));
    assert!(file.content.contains("review gate"));
    assert!(file.content.contains("--version =0.2.0 --locked"));
    assert!(
        !file
            .content
            .contains("ref: ${{ github.event.pull_request.head")
    );
    assert!(!file.content.contains("permissions:\n  checks: write"));
}

#[test]
fn review_policy_rejects_unpinned_or_injected_configuration() {
    let root = tempfile::tempdir().unwrap();
    for (version, secret) in [("latest", "APP_ID"), ("0.2.0", "APP_ID }}"), ("0.2.0", "")] {
        std::fs::write(root.path().join("simit.toml"), format!("[review_policy]\ntoolbelt_version = {version:?}\napp_id_secret = {secret:?}\napp_private_key_secret = \"APP_PRIVATE_KEY\"\n")).unwrap();
        assert!(ProjectConfig::load(root.path()).is_err());
    }
}

#[test]
fn policy_generation_rejects_an_effective_non_github_platform() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.path().join("simit.toml"), "[review_policy]\ntoolbelt_version = \"0.2.0\"\napp_id_secret = \"APP_ID\"\napp_private_key_secret = \"APP_KEY\"\n").unwrap();
    for args in [
        vec!["init", "ci"],
        vec!["init", "ci", "--platform", "forgejo"],
    ] {
        let output = common::simit()
            .current_dir(root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("GitHub Actions"));
        assert!(!root.path().join(review_policy::PATH).exists());
    }
    let output = common::simit()
        .current_dir(root.path())
        .args(["init", "ci", "--platform", "github", "--runtime", "cargo"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_full_sweep_page_rotates_to_later_prs_without_abandoning_evaluation() {
    let config = review_policy::Config {
        toolbelt_version: "0.2.0".into(),
        app_id_secret: "APP_ID".into(),
        app_private_key_secret: "APP_KEY".into(),
        policy_path: None,
    };
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&review_policy::file(&config).unwrap().content).unwrap();
    let shell = yaml["jobs"]["resolve"]["steps"][0]["run"].as_str().unwrap();
    let script = shell
        .strip_prefix("node <<'NODE'\n")
        .unwrap()
        .trim_end()
        .strip_suffix("NODE")
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let event = root.path().join("event.json");
    let output = root.path().join("outputs");
    std::fs::write(&event, "{}").unwrap();
    let fixture = r#"
Date.now = () => 600000;
let calls = 0;
global.fetch = async url => {
  calls++;
  if (calls === 2 && !url.endsWith('&page=2')) throw new Error('did not rotate to the later page');
  return {ok:true, headers:{get:()=>'<https://api.github.com/repos/example/project/pulls?page=3>; rel="last"'},
    json:async()=>Array.from({length:100},(_,i)=>({number:(calls===1?1:101)+i}))};
};
"#;
    let result = std::process::Command::new("node")
        .arg("-e")
        .arg(format!("{fixture}\n{script}"))
        .env("GITHUB_EVENT_PATH", event)
        .env("GITHUB_OUTPUT", &output)
        .env("GITHUB_EVENT_NAME", "schedule")
        .env("GITHUB_REPOSITORY", "example/project")
        .env("GH_TOKEN", "fixture")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let outputs = std::fs::read_to_string(output).unwrap();
    let prs: Vec<u64> = serde_json::from_str(outputs.trim().strip_prefix("prs=").unwrap()).unwrap();
    assert_eq!(prs, (101..=200).collect::<Vec<_>>());
}
