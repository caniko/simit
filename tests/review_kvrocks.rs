//! Contract-first regression for the opt-in hosted compiler-cache route.
//! Actual service/socket and each pinned wrapper's sandbox receipts are separate.
use serde_json::{Value, json};
use simit::review::{canonical_digest, contract::Request};
use simit::review::{engine::EngineManifest, kvrocks};

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

#[test]
fn kvrocks_bootstrap_uses_only_the_direct_worker_native_pin() {
    let mut manifest = EngineManifest {
        schema_version: 1,
        repository: "caniko/simit".into(),
        revision: "c".repeat(40),
        nixpkgs_revision: "d".repeat(40),
    };
    for system in ["x86_64-linux", "aarch64-linux"] {
        let reference = kvrocks::tool_reference(&manifest, system).unwrap();
        assert_eq!(
            reference,
            format!(
                "github:NixOS/nixpkgs/{}#legacyPackages.{system}.kvrocks",
                "d".repeat(40)
            )
        );
        assert!(!reference.contains(HEAD));
        assert!(!reference.contains(BASE));
        let package = json!({"pname":"kvrocks", "version":"2.14.0", "system":system,
            "outputs":["out"], "drvPath":format!("/nix/store/{}-kvrocks-2.14.0.drv", "0".repeat(32)),
            "outPath":format!("/nix/store/{}-kvrocks-2.14.0", "1".repeat(32))});
        let source = kvrocks::freeze_source(&manifest, system, &package).unwrap();
        assert_eq!(source.system, system);
        assert_eq!(source.installable, reference);
        assert_eq!(source.engine_revision, "c".repeat(40));
        assert_eq!(source.nixpkgs_revision, "d".repeat(40));
        assert_eq!(source.version, "2.14.0");
        for (field, value) in [
            ("pname", json!("redis")),
            ("system", json!("x86_64-darwin")),
            ("outputs", json!(["out", "dev"])),
            ("version", json!("2.14.0\nother-setting")),
            ("version", json!("")),
            ("drvPath", json!("/tmp/foreign.drv")),
            ("drvPath", package["outPath"].clone()),
            ("outPath", package["drvPath"].clone()),
            ("outPath", json!("/nix/store/../foreign")),
        ] {
            let mut invalid = package.clone();
            invalid[field] = value;
            assert!(
                kvrocks::freeze_source(&manifest, system, &invalid).is_err(),
                "must reject {field}"
            );
        }
    }
    for system in [
        "x86_64-darwin",
        "aarch64-darwin",
        "x86_64-windows",
        "x86_64-linux;echo foreign",
    ] {
        assert!(kvrocks::tool_reference(&manifest, system).is_err());
    }
    manifest.nixpkgs_revision = "trunk".into();
    assert!(kvrocks::tool_reference(&manifest, "x86_64-linux").is_err());
    manifest.nixpkgs_revision = "d".repeat(40);
    manifest.revision = "latest".into();
    assert!(kvrocks::tool_reference(&manifest, "x86_64-linux").is_err());
}
