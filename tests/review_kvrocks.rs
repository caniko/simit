//! Contract-first regression for the opt-in hosted compiler-cache route.
//! Actual service/socket and each pinned wrapper's sandbox receipts are separate.
use serde_json::{Value, json};
use simit::review::{canonical_digest, contract::Request};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn request() -> Value {
    let mut request = serde_json::to_value(simit::review::contract::example()).unwrap();
    request["expected_head"] = json!(HEAD);
    request["expected_base"] = json!(BASE);
    request["test_profile"] = json!("checks-rebuild-v1");
    request
}

fn accepts(value: Value) -> bool {
    serde_json::from_value::<Request>(value).is_ok_and(|request| request.validate().is_ok())
}

#[test]
fn kvrocks_is_an_explicit_immutable_linux_request_without_legacy_identity_drift() {
    let legacy = request();
    assert!(legacy.get("compiler_cache").is_none());
    let legacy_request: Request = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&legacy_request).unwrap(), legacy);

    let mut enabled = legacy.clone();
    enabled["compiler_cache"] = json!("kvrocks-v1");
    let opted_in: Request = serde_json::from_value(enabled.clone())
        .expect("the typed Kvrocks profile must be accepted without free-form worker commands");
    opted_in.validate().unwrap();
    assert_eq!(serde_json::to_value(&opted_in).unwrap(), enabled);
    assert_ne!(
        canonical_digest(&legacy_request).unwrap(),
        canonical_digest(&opted_in).unwrap()
    );

    let mut revision = enabled;
    revision["pr"] = Value::Null;
    revision["revision"] = json!(HEAD);
    revision["expected_base"] = Value::Null;
    assert!(accepts(revision.clone()));
    revision["revision"] = json!(BASE);
    assert!(
        !accepts(revision.clone()),
        "the immutable revision must match the expected head"
    );
    revision["revision"] = json!("trunk");
    assert!(
        !accepts(revision),
        "cache requests must name an immutable revision"
    );
}

#[test]
fn kvrocks_rejects_mutable_identity_unsupported_platforms_and_free_form_profiles() {
    let mut enabled = request();
    enabled["compiler_cache"] = json!("kvrocks-v1");
    assert!(
        accepts(enabled.clone()),
        "the positive control must be valid"
    );
    for (field, value) in [
        ("expected_head", Value::Null),
        ("expected_base", Value::Null),
        ("mode", json!("merge")),
        ("systems", json!(["aarch64-darwin"])),
        ("systems", json!(["x86_64-linux", "aarch64-darwin"])),
        ("test_profile", json!("checks-v1")),
        ("compiler_cache", json!("redis-v1")),
        ("compiler_cache", json!("disk")),
        ("compiler_cache", json!("kvrocks-v1;touch /tmp/foreign")),
        ("compiler_cache", json!({"command": "kvrocks"})),
        ("extra_nix_config", json!("sandbox=false")),
    ] {
        let mut invalid = enabled.clone();
        invalid[field] = value;
        assert!(!accepts(invalid), "must reject {field}");
    }
    let mut nixpkgs = enabled.clone();
    nixpkgs["backend"] = json!("nixpkgs");
    nixpkgs["repository"] = json!("NixOS/nixpkgs");
    nixpkgs["packages"] = json!([]);
    nixpkgs["checks"] = json!([]);
    assert!(
        !accepts(nixpkgs),
        "this cache profile is a flake consumer route"
    );

    enabled["systems"] = json!(["x86_64-linux", "aarch64-linux"]);
    assert!(
        accepts(enabled),
        "native Linux systems remain independently selected"
    );
}
