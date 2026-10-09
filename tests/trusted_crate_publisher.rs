use std::fs;

use serde_yaml::Value;
use tempfile::TempDir;

mod common;

fn publisher_project(mode: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let workspace = mode != "single";
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            "[package]\nname='publisher-demo'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n{}",
            if workspace {
                "[workspace]\nmembers=['dependent']\nresolver='3'\n"
            } else {
                ""
            }
        ),
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/lib.rs"), "").unwrap();
    if workspace {
        fs::create_dir_all(temp.path().join("dependent/src")).unwrap();
        fs::write(
            temp.path().join("dependent/Cargo.toml"),
            "[package]\nname='publisher-dependent'\nversion='0.1.0'\nedition='2024'\nlicense='MIT'\n[dependencies]\npublisher-demo={path='..',version='0.1.0'}\n",
        )
        .unwrap();
        fs::write(temp.path().join("dependent/src/lib.rs"), "").unwrap();
    }
    fs::write(
        temp.path().join("simit.toml"),
        format!(
            "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\nrunner='ubuntu-latest'\npublish_crates=true\nworkspace={workspace}\npublish_strategy='{}'\n",
            if mode == "coordinated" {
                "coordinated"
            } else {
                "members"
            }
        ),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{mode}: {output:?}");
    let check = common::simit()
        .current_dir(temp.path())
        .args(["init", "ci", "--check", "--diff"])
        .output()
        .unwrap();
    assert!(check.status.success(), "{mode}: {check:?}");
    temp
}

fn has_publication_credential(value: &Value) -> bool {
    match value {
        Value::String(value) => {
            value.contains("secrets.CARGO_REGISTRY_TOKEN")
                || value.contains("secrets.CRATES_IO_API_TOKEN")
        }
        Value::Sequence(values) => values.iter().any(has_publication_credential),
        Value::Mapping(values) => values.values().any(has_publication_credential),
        _ => false,
    }
}

fn assert_trusted_credential_boundary(mode: &str) {
    let project = publisher_project(mode);
    let mut credential_jobs = 0;
    let mut signed_tag_routes = 0;
    for entry in fs::read_dir(project.path().join(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        let workflow: Value = serde_yaml::from_str(&text).unwrap();
        if workflow["on"]["push"]["tags"].is_sequence() {
            signed_tag_routes += 1;
            // GitHub loads a tag-push workflow from that tag. A writer can
            // remove every signature check from that definition; credentials
            // therefore belong to the independently trusted default-branch
            // publisher, never this candidate-controlled execution route.
            assert!(
                !has_publication_credential(&workflow),
                "{mode}: tag-controlled {} still requests publication credentials",
                path.display()
            );
        }
        for job in workflow["jobs"].as_mapping().unwrap().values() {
            if !has_publication_credential(job) {
                continue;
            }
            credential_jobs += 1;
            assert!(workflow["on"]["push"].is_null(), "{path:?}");
            assert!(workflow["on"]["pull_request"].is_null(), "{path:?}");
            assert_eq!(
                workflow["on"]["workflow_run"]["types"],
                serde_yaml::to_value(["completed"]).unwrap(),
                "{mode}: {path:?} must execute its independently trusted default-branch definition"
            );
            let environment = job["environment"]
                .as_str()
                .or_else(|| job["environment"]["name"].as_str())
                .expect("publication credentials require a protected environment");
            assert!(!environment.is_empty(), "{path:?}");
            assert!(
                !environment.contains("${{"),
                "candidate input must not select the credential environment: {path:?}"
            );
        }
    }
    assert!(signed_tag_routes > 0, "retain signed original-run recovery");
    assert!(credential_jobs > 0, "retain a supported publishing route");
    // The actual environment/ref restrictions and credential enrollment are
    // operator-owned acceptance gates, not properties this YAML test can prove.
}

#[test]
fn single_crate_credentials_are_outside_the_tag_controlled_workflow() {
    assert_trusted_credential_boundary("single");
}

#[test]
fn member_publish_credentials_are_outside_the_tag_controlled_workflow() {
    assert_trusted_credential_boundary("members");
}

#[test]
fn coordinated_credentials_are_outside_the_tag_controlled_workflow() {
    assert_trusted_credential_boundary("coordinated");
}
