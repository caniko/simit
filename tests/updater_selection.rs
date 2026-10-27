use std::{fs, process::Command};

use serde_json::json;

#[test]
fn plan_accounts_for_every_enrolled_coordinate_without_executing_adapters_or_generators() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let coordinate = |name: &str| format!("git.example/team/{name}");
    let observation = |name: &str, role: &str, lifecycle: &str, ci: &str| {
        json!({
            "coordinate": coordinate(name), "forge": "example",
            "relativePath": format!("custom/layout/{name}/{role}"),
            "kind": "owned-repo", "lifecycle": lifecycle, "access": "normal",
            "checkoutRole": role, "simitCi": ci
        })
    };
    let inventory = root.join("inventory.json");
    fs::write(
        &inventory,
        serde_json::to_vec(&json!({
            "schemaVersion": 1, "root": root.join("projects"),
            "observedAt": chrono::Utc::now(), "producer": "selection-test",
            "policyDigest": "b".repeat(64),
            "projects": [
                observation("demo", "canonical", "active", "auto"),
                observation("demo", "mirror", "active", "auto"),
                observation("next", "canonical", "active", "auto"),
                observation("mirror", "mirror", "active", "auto"),
                observation("observed", "canonical", "observed", "auto"),
                observation("custom", "canonical", "active", "custom"),
                observation("disabled", "canonical", "active", "auto")
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let config = root.join("updates.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "workspaces": {"projects": {
                "root": root.join("projects"), "stateDirectory": root.join("state"),
                "inventory": {"file": inventory}
            }},
            "forges": {"example": {
                "host": "git.example", "apiUrl": "https://git.example/api",
                "credentialFile": root.join("absent-credential"), "checkAuthority": "777",
                "adapter": {"program": "/nix/store/absent-adapter/bin/adapter"}
            }},
            "profiles": {"fleet": {
                "workspace": "projects", "scopes": ["ci"],
                "select": {"coordinates": [
                    coordinate("demo"), coordinate("next"), coordinate("mirror"), coordinate("observed"),
                    coordinate("custom"), coordinate("disabled"), coordinate("missing")
                ]},
                "generator": {
                    "executable": "/nix/store/absent-generator/bin/simit",
                    "sourceRevision": "a".repeat(40),
                    "sourceCoordinate": coordinate("generator"), "forge": "example",
                    "requiredChecks": [{"name": "test", "authority": "777"}]
                },
                "projectOverrides": {(coordinate("disabled")): {
                    "enable": false, "reason": "Project-owned CI", "blocker": "provider-mismatch"
                }}
            }}
        }))
        .unwrap(),
    )
    .unwrap();
    let plan = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_simit"))
            .args(["projects", "update", "--config"])
            .arg(&config)
            .args(["--profile", "fleet", "--plan", "--json"])
            .args(extra)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let report = plan(&[]);
    assert_eq!(report["schema"], "simit.updater-selection-plan.v1");
    assert_eq!(report["qualification"], "not-run");
    assert_eq!(report["selected"].as_array().unwrap().len(), 2);
    assert_eq!(report["enrollment"].as_array().unwrap().len(), 7);
    let entry = |name: &str| {
        report["enrollment"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["coordinate"] == coordinate(name))
            .unwrap()
    };
    assert_eq!(entry("demo")["status"], "selected");
    assert_eq!(entry("demo")["observations"].as_array().unwrap().len(), 2);
    assert_eq!(
        entry("mirror")["observations"][0]["reason"],
        "checkout-role"
    );
    assert_eq!(entry("observed")["observations"][0]["reason"], "lifecycle");
    assert_eq!(entry("custom")["observations"][0]["reason"], "ci-policy");
    assert_eq!(entry("disabled")["status"], "disabled");
    assert_eq!(entry("disabled")["reason"], "Project-owned CI");
    assert_eq!(entry("disabled")["blocker"], "provider-mismatch");
    assert!(entry("demo")["blocker"].is_null());
    assert_eq!(entry("missing")["status"], "missing");
    let narrowed = plan(&["--repo", &coordinate("demo")]);
    assert_eq!(narrowed["selected"].as_array().unwrap().len(), 1);
    assert_eq!(
        narrowed["enrollment"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["coordinate"] == coordinate("next"))
            .unwrap()["status"],
        "narrowed"
    );
    let mut policy: serde_json::Value =
        serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    policy["profiles"]["fleet"]["select"]["lifecycles"] = json!(["retired"]);
    policy["profiles"]["fleet"]["rollout"] = json!({"canaries": [coordinate("demo")]});
    fs::write(&config, serde_json::to_vec(&policy).unwrap()).unwrap();
    let empty = plan(&[]);
    assert!(empty["selected"].as_array().unwrap().is_empty());
    assert_eq!(empty["enrollment"].as_array().unwrap().len(), 7);
    let preview = Command::new(env!("CARGO_BIN_EXE_simit"))
        .args(["projects", "update", "--config"])
        .arg(&config)
        .args(["--profile", "fleet", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(!preview.status.success(), "{preview:?}");
    assert!(
        String::from_utf8_lossy(&preview.stderr)
            .contains("configured canary is outside eligible selection"),
        "{preview:?}"
    );
    policy["profiles"]["fleet"]["select"]["lifecycles"] = json!(["active"]);
    policy["profiles"]["fleet"]["rollout"]["canaries"] = json!([]);
    fs::write(&config, serde_json::to_vec(&policy).unwrap()).unwrap();
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(&inventory).unwrap()).unwrap();
    let mut stale = original.clone();
    stale["observedAt"] = json!(chrono::Utc::now() - chrono::Duration::days(1));
    let mut wrong_root = original.clone();
    wrong_root["root"] = json!(root.join("other-workspace"));
    let mut duplicate = original.clone();
    duplicate["projects"]
        .as_array_mut()
        .unwrap()
        .push(observation("demo", "canonical", "active", "auto"));
    for (document, expected) in [
        (stale, "inventory is stale"),
        (wrong_root, "inventory schema/root mismatch"),
        (duplicate, "multiple canonical checkouts"),
    ] {
        fs::write(&inventory, serde_json::to_vec(&document).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_simit"))
            .args(["projects", "update", "--config"])
            .arg(&config)
            .args(["--profile", "fleet", "--plan", "--json"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{output:?}"
        );
    }
    assert!(
        fs::read_dir(root.join("state")).unwrap().all(|entry| !entry
            .unwrap()
            .file_type()
            .unwrap()
            .is_dir())
    );
    assert!(!root.join("projects").exists());
}

#[test]
fn selection_plans_cannot_accidentally_execute_or_limit_the_report() {
    for mode in [
        vec!["--push"],
        vec!["--status"],
        vec!["--dry-run"],
        vec!["--limit", "1"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_simit"))
            .args(["projects", "update", "--profile", "fleet", "--plan"])
            .args(&mode)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{mode:?}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}
