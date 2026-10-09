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

#[test]
fn kvrocks_worker_identity_requires_the_actual_runner_and_nix_build_group() {
    let identity = kvrocks::parse_worker_identity(
        "runner",
        "nixbld",
        "runner:x:1001:1001:Runner:/home/runner:/bin/bash\n",
        "nixbld:x:30000:nixbld1,nixbld2\n",
    )
    .unwrap();
    assert_eq!(identity.user, "runner");
    assert_eq!(identity.uid, 1001);
    assert_eq!(identity.group, "nixbld");
    assert_eq!(identity.gid, 30000);
    for (passwd, group) in [
        (
            "other:x:1001:1001:Runner:/home/runner:/bin/bash",
            "nixbld:x:30000:nixbld1",
        ),
        (
            "runner:x:0:0:root:/root:/bin/bash",
            "nixbld:x:30000:nixbld1",
        ),
        (
            "runner:x:invalid:1001:Runner:/home/runner:/bin/bash",
            "nixbld:x:30000:nixbld1",
        ),
        (
            "runner:x:1001:1001:Runner:/home/runner:/bin/bash",
            "other:x:30000:nixbld1",
        ),
        (
            "runner:x:1001:1001:Runner:/home/runner:/bin/bash",
            "nixbld:x:0:nixbld1",
        ),
        (
            "runner:x:1001:1001:Runner:/home/runner:/bin/bash\nother:x:1002:1002::/:/bin/sh",
            "nixbld:x:30000:nixbld1",
        ),
    ] {
        assert!(kvrocks::parse_worker_identity("runner", "nixbld", passwd, group).is_err());
    }
}

#[cfg(unix)]
#[test]
fn kvrocks_runtime_preflight_preserves_foreign_state_and_checks_socket_identity() {
    use std::os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        net::UnixListener,
    };
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("redis-sccache");
    kvrocks::refuse_existing_runtime(&runtime).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let sentinel = runtime.join("foreign-state");
    std::fs::write(&sentinel, "preserve foreign service bytes").unwrap();
    assert!(kvrocks::refuse_existing_runtime(&runtime).is_err());
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "preserve foreign service bytes"
    );
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o750)).unwrap();
    let socket = runtime.join("redis.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660)).unwrap();
    let metadata = std::fs::metadata(&runtime).unwrap();
    let identity = kvrocks::WorkerIdentity {
        user: "fixture".into(),
        uid: metadata.uid(),
        group: "fixture".into(),
        gid: metadata.gid(),
    };
    kvrocks::verify_socket(&socket, &identity).unwrap();
    for mode in [0o600, 0o666, 0o777] {
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(kvrocks::verify_socket(&socket, &identity).is_err());
    }
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660)).unwrap();
    let mut foreign = identity.clone();
    foreign.uid = foreign.uid.checked_add(1).unwrap();
    assert!(kvrocks::verify_socket(&socket, &foreign).is_err());
    foreign = identity.clone();
    foreign.gid = foreign.gid.checked_add(1).unwrap();
    assert!(kvrocks::verify_socket(&socket, &foreign).is_err());
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(kvrocks::verify_socket(&socket, &identity).is_err());
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o750)).unwrap();
    let alias = runtime.join("alias.sock");
    symlink(&socket, &alias).unwrap();
    assert!(kvrocks::verify_socket(&alias, &identity).is_err());
    let dangling = root.path().join("dangling-runtime");
    symlink(root.path().join("missing"), &dangling).unwrap();
    assert!(kvrocks::refuse_existing_runtime(&dangling).is_err());
    assert!(dangling.is_symlink());
    assert!(kvrocks::verify_socket(&sentinel, &identity).is_err());
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "preserve foreign service bytes"
    );
}

#[cfg(unix)]
#[test]
fn kvrocks_runtime_allocation_is_exclusive_and_only_cleans_its_own_empty_directory() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let metadata = std::fs::metadata(root.path()).unwrap();
    let identity = kvrocks::WorkerIdentity {
        user: "fixture".into(),
        uid: metadata.uid(),
        group: "fixture".into(),
        gid: metadata.gid(),
    };
    let runtime = root.path().join("redis-sccache");
    let owned = kvrocks::OwnedRuntime::create(&runtime, &identity).unwrap();
    let metadata = std::fs::symlink_metadata(&runtime).unwrap();
    assert!(metadata.is_dir());
    assert_eq!(metadata.mode() & 0o7777, 0o750);
    assert_eq!(metadata.uid(), identity.uid);
    assert_eq!(metadata.gid(), identity.gid);
    assert!(kvrocks::OwnedRuntime::create(&runtime, &identity).is_err());
    owned.cleanup().unwrap();
    assert!(!runtime.exists());

    let owned = kvrocks::OwnedRuntime::create(&runtime, &identity).unwrap();
    let retained = root.path().join("retained-owned-directory");
    std::fs::rename(&runtime, &retained).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let sentinel = runtime.join("foreign");
    std::fs::write(&sentinel, "preserve replacement").unwrap();
    assert!(owned.cleanup().is_err());
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "preserve replacement"
    );
    assert!(retained.is_dir());
    std::fs::remove_file(&sentinel).unwrap();
    std::fs::remove_dir(&runtime).unwrap();

    let owned = kvrocks::OwnedRuntime::create(&runtime, &identity).unwrap();
    let sentinel = runtime.join("unexpected-state");
    std::fs::write(&sentinel, "preserve unclaimed state").unwrap();
    assert!(owned.cleanup().is_err());
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "preserve unclaimed state"
    );
    std::fs::remove_file(&sentinel).unwrap();
    std::fs::remove_dir(&runtime).unwrap();

    symlink(root.path().join("missing"), &runtime).unwrap();
    assert!(kvrocks::OwnedRuntime::create(&runtime, &identity).is_err());
    assert!(runtime.is_symlink());
    let alias = root.path().join("runtime-parent-alias");
    symlink(&retained, &alias).unwrap();
    assert!(kvrocks::OwnedRuntime::create(&alias.join("new-runtime"), &identity).is_err());
    assert!(!retained.join("new-runtime").exists());
    assert!(alias.is_symlink());
}

#[test]
fn kvrocks_transient_service_plan_binds_native_tool_run_identity_and_bounded_resources() {
    let manifest = EngineManifest {
        schema_version: 1,
        repository: "caniko/simit".into(),
        revision: "c".repeat(40),
        nixpkgs_revision: "d".repeat(40),
    };
    let package = json!({"pname":"kvrocks", "version":"2.14.0", "system":"x86_64-linux",
        "outputs":["out"], "drvPath":format!("/nix/store/{}-kvrocks-2.14.0.drv", "0".repeat(32)),
        "outPath":format!("/nix/store/{}-kvrocks-2.14.0", "1".repeat(32))});
    let bootstrap = kvrocks::BootstrapReceipt {
        source: kvrocks::freeze_source(&manifest, "x86_64-linux", &package).unwrap(),
        nar_hash: format!("sha256:{}", "a".repeat(64)),
        nar_size: 123456,
        binary_sha256: "b".repeat(64),
    };
    let identity = kvrocks::parse_worker_identity(
        "runner",
        "nixbld",
        "runner:x:1001:1001:Runner:/home/runner:/bin/bash\n",
        "nixbld:x:30000:nixbld1,nixbld2\n",
    )
    .unwrap();
    let run_identity = "e".repeat(64);
    let plan = kvrocks::service_plan(
        &bootstrap,
        &identity,
        std::path::Path::new("/home/runner/work/_temp"),
        &run_identity,
    )
    .unwrap();
    assert_eq!(plan.bootstrap, bootstrap);
    assert_eq!(plan.identity, identity);
    assert_eq!(plan.run_identity, run_identity);
    assert_eq!(plan.unit, format!("simit-kvrocks-{run_identity}.service"));
    assert_eq!(plan.socket, "/run/redis-sccache/redis.sock");
    assert_eq!(plan.startup_seconds, 30);
    assert_eq!(plan.shutdown_seconds, 30);
    assert_eq!(plan.memory_max_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(plan.cpu_quota_percent, 200);
    assert_eq!(plan.max_db_gib, 4);
    assert_eq!(plan.sandbox_paths, [plan.socket.clone()]);
    assert_eq!(
        plan.state_directory,
        format!("/home/runner/work/_temp/simit-kvrocks-{run_identity}")
    );
    let properties = &plan.systemd_args;
    for required in [
        "--no-block",
        "--service-type=exec",
        "--property=User=runner",
        "--property=Group=nixbld",
        "--property=RuntimeDirectory=redis-sccache",
        "--property=RuntimeDirectoryMode=0750",
        "--property=RuntimeDirectoryPreserve=yes",
        "--property=MemoryMax=2147483648",
        "--property=CPUQuota=200%",
        "--property=KillMode=control-group",
        "--property=TimeoutStopSec=30s",
        "--property=SendSIGKILL=no",
        "--property=Restart=no",
        "--property=RestrictAddressFamilies=AF_UNIX",
        "--property=NoNewPrivileges=yes",
    ] {
        assert!(properties.iter().any(|arg| arg == required), "{required}");
    }
    let tail = &properties[properties.len() - 4..];
    assert_eq!(tail[0], "--");
    assert_eq!(tail[1], format!("{}/bin/kvrocks", bootstrap.source.output));
    assert_eq!(tail[2], "-c");
    assert_eq!(tail[3], format!("{}/kvrocks.conf", plan.state_directory));
    for required in [
        "port 16666",
        "unixsocket /run/redis-sccache/redis.sock",
        "unixsocketperm 660",
        "daemonize no",
        "supervised no",
        "workers 2",
        "max-db-size 4",
        "rocksdb.block_cache_size 512",
        "rocksdb.max_background_jobs 2",
        "rocksdb.max_write_buffer_number 2",
        "rocksdb.write_buffer_size 64",
    ] {
        assert!(
            plan.config.lines().any(|line| line == required),
            "{required}"
        );
    }
    assert!(!plan.config.lines().any(|line| line.starts_with("bind ")));
    assert!(
        serde_json::to_value(&plan)
            .unwrap()
            .get("bootstrap")
            .is_some()
    );
    for invalid_run in ["", "latest", "../foreign", "-other", "a\nport 6379"] {
        assert!(
            kvrocks::service_plan(
                &bootstrap,
                &identity,
                std::path::Path::new("/tmp/runner"),
                invalid_run
            )
            .is_err()
        );
    }
    for invalid_temp in [
        "relative",
        "/",
        "/tmp/../foreign",
        "/tmp/other state",
        "/tmp/evil\nbind 0.0.0.0",
    ] {
        assert!(
            kvrocks::service_plan(
                &bootstrap,
                &identity,
                std::path::Path::new(invalid_temp),
                &run_identity
            )
            .is_err()
        );
    }
    for field in ["uid", "gid"] {
        let mut invalid_identity = identity.clone();
        if field == "uid" {
            invalid_identity.uid = 0;
        } else {
            invalid_identity.gid = 0;
        }
        assert!(
            kvrocks::service_plan(
                &bootstrap,
                &invalid_identity,
                std::path::Path::new("/tmp/runner"),
                &run_identity
            )
            .is_err()
        );
    }
    let mut invalid_tool = bootstrap.clone();
    invalid_tool.source.output = "/tmp/candidate-tool".into();
    assert!(
        kvrocks::service_plan(
            &invalid_tool,
            &identity,
            std::path::Path::new("/tmp/runner"),
            &run_identity
        )
        .is_err()
    );
    invalid_tool = bootstrap.clone();
    invalid_tool.source.system = "x86_64-darwin".into();
    assert!(
        kvrocks::service_plan(
            &invalid_tool,
            &identity,
            std::path::Path::new("/tmp/runner"),
            &run_identity
        )
        .is_err()
    );
    invalid_tool = bootstrap;
    invalid_tool.binary_sha256 = "latest".into();
    assert!(
        kvrocks::service_plan(
            &invalid_tool,
            &identity,
            std::path::Path::new("/tmp/runner"),
            &run_identity
        )
        .is_err()
    );
}
