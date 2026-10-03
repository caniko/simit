use serde_json::json;
use simit::review::engine::{EngineManifest, verify_lock};

#[test]
fn controller_lock_binds_engine_revision_and_tool_nixpkgs() {
    let manifest = EngineManifest {
        schema_version: 1,
        repository: "caniko/simit".into(),
        revision: "a".repeat(40),
        nixpkgs_revision: "b".repeat(40),
    };
    let lock = json!({"version":7,"root":"root","nodes":{"root":{"inputs":{"simit":"simit","nixpkgs":"nixpkgs"}},"simit":{"locked":{"type":"github","owner":"caniko","repo":"simit","rev":manifest.revision}},"nixpkgs":{"locked":{"type":"github","owner":"NixOS","repo":"nixpkgs","rev":manifest.nixpkgs_revision}}}});
    verify_lock(&lock, &manifest).unwrap();
    for (node, key, value) in [
        ("simit", "rev", json!("c".repeat(40))),
        ("simit", "owner", json!("attacker")),
        ("nixpkgs", "rev", json!("c".repeat(40))),
    ] {
        let mut bad = lock.clone();
        bad["nodes"][node]["locked"][key] = value;
        assert!(verify_lock(&bad, &manifest).is_err());
    }
    assert!(verify_lock(&json!({}), &manifest).is_err());
    let mut bad = lock;
    bad["nodes"]["root"]["inputs"]["simit"] = json!(["other", "simit"]);
    assert!(verify_lock(&bad, &manifest).is_err());
}
