//! Crow CI for native Cargo workspaces and flake-only projects.

use std::fs;

use super::{init_package, simit};

#[test]
fn native_manual_gate_does_not_affect_inference_and_cargo_runtime_survives_without_workflows() {
    let temp = init_package(true);
    let root = temp.path();
    fs::write(
        root.join("flake.nix"),
        "{ outputs = _: { checks = {}; }; }\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        simit()
            .current_dir(root)
            .args(["init", "ci", "--ci-provider", "crow"])
            .args(args)
            .output()
            .unwrap()
    };
    fs::write(
        root.join("simit.toml"),
        r#"[ci]
all_features = false

[[ci.manual_gates]]
id = "security"
runtime = "cargo"
run = "cargo audit && cargo test --all-features && cargo deny check"

[ci.crow]
cargo_home = "/work/cargo"
"#,
    )
    .unwrap();
    let generated = run(&["--runtime", "cargo"]);
    assert!(generated.status.success(), "{generated:?}");
    let build = fs::read_to_string(root.join(".crow/build.yaml")).unwrap();
    for command in ["cargo audit", "cargo deny check", "--all-features"] {
        assert!(!build.contains(command), "{build}");
    }
    let config = fs::read_to_string(root.join("simit.toml")).unwrap();
    let config: toml_edit::DocumentMut = config.parse().unwrap();
    assert_eq!(config["ci"]["runtime"].as_str(), Some("cargo"));
    assert_eq!(config["ci"]["all_features"].as_bool(), Some(false));
    for key in ["with_audit", "with_deny"] {
        assert_ne!(
            config["ci"].get(key).and_then(|item| item.as_bool()),
            Some(true)
        );
    }
    let gate: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(root.join(".crow/manual-security.yaml")).unwrap())
            .unwrap();
    assert_eq!(gate["steps"][0]["environment"]["CARGO_HOME"], "/work/cargo");
    assert!(
        gate["steps"][0]["commands"][0]
            .as_str()
            .unwrap()
            .contains("$${RUSTUP_TOOLCHAIN:-}")
    );
    let check = run(&["--check"]);
    assert!(check.status.success(), "{check:?}");
    fs::remove_file(root.join(".crow/build.yaml")).unwrap();
    fs::remove_file(root.join(".crow/manual-security.yaml")).unwrap();
    let regenerated = run(&[]);
    assert!(regenerated.status.success(), "{regenerated:?}");
    assert_eq!(
        fs::read_to_string(root.join(".crow/build.yaml")).unwrap(),
        build
    );
}

#[test]
fn invalid_crow_native_options_are_rejected() {
    for (ci, crow, diagnostic) in [
        (
            "",
            "[[ci.manual_gates]]\nid = 'gate'\nrun = 'true'\nartifacts = { name = 'result', paths = ['result/'] }",
            "artifacts require",
        ),
        (
            "step_runners = { cargo-fmt = 'agent' }",
            "[ci.crow]\ncombine_steps = true",
            "per-step runners",
        ),
        (
            "runtime = 'nix'",
            "[ci.crow]\nshallow_clone = true",
            "shallow_clone",
        ),
        ("", "[ci.crow]\nshallow_clone = true", "shallow_clone"),
        (
            "",
            "[ci.crow.sccache]\nenv = { RUSTC_WRAPPER = 'other' }",
            "RUSTC_WRAPPER",
        ),
        (
            "",
            "[ci.crow.sccache]\nsecret = 'CACHE_TOKEN'\nenv = { CACHE_TOKEN = 'plaintext' }",
            "CACHE_TOKEN",
        ),
        (
            "required_secrets = ['REPO_TOKEN']",
            "[ci.crow.sccache]\nenv = { REPO_TOKEN = 'plaintext' }",
            "REPO_TOKEN",
        ),
        ("format_all_rust_files = true", "", "format_all_rust_files"),
    ] {
        let temp = init_package(true);
        fs::write(
            temp.path().join("flake.nix"),
            "{ outputs = _: { checks = {}; }; }\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("simit.toml"),
            format!("[ci]\n{ci}\n{crow}\n"),
        )
        .unwrap();
        let output = simit()
            .current_dir(temp.path())
            .args(["init", "ci", "--ci-provider", "crow"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{ci}\n{crow}\n{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{output:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn crow_pins_native_steps_without_overriding_msrv_or_local_toolchains() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let temp = init_package(false);
    let root = temp.path();
    for (name, body) in [
        (
            "rustup",
            "case \"$1\" in default) printf '%s\\n' '1.94.1-test (default)';; toolchain) test \"$3\" = '1.85';; *) exit 1;; esac",
        ),
        (
            "cargo",
            "test \"$1\" = '+1.85' && test \"$2\" = 'check' && test \"$3\" = '--locked'",
        ),
    ] {
        let path = root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for combine in [false, true] {
        fs::write(root.join("simit.toml"), format!("[ci]\nprovider = 'crow'\nruntime = 'cargo'\nwith_msrv = true\nwith_docs = true\n[ci.crow]\ncombine_steps = {combine}\n")).unwrap();
        let output = simit()
            .current_dir(root)
            .args(["init", "ci"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let workflow: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(root.join(".crow/build.yaml")).unwrap())
                .unwrap();
        for step in workflow["steps"].as_sequence().unwrap() {
            let name = step["name"].as_str().unwrap();
            let commands: Vec<_> = step["commands"]
                .as_sequence()
                .unwrap()
                .iter()
                .map(|command| command.as_str().unwrap())
                .collect();
            let pins: Vec<_> = commands
                .iter()
                .filter(|command| command.contains("$$(rustup default)"))
                .collect();
            assert_eq!(
                pins.len(),
                usize::from(name != "cargo-msrv"),
                "{name}: {commands:?}"
            );
            if name != "cargo-msrv" {
                assert_eq!(**pins.first().unwrap(), commands[0]);
                let pin = commands[0].replace("$$", "$");
                for (ci, explicit, expected) in [
                    (None, None, "unset"),
                    (None, Some("custom"), "custom"),
                    (Some("true"), Some("custom"), "custom"),
                    (Some("true"), None, "1.94.1-test"),
                ] {
                    let mut sh = Command::new("/bin/sh");
                    sh.env("PATH", root)
                        .env_remove("CI")
                        .env_remove("RUSTUP_TOOLCHAIN");
                    if let Some(ci) = ci {
                        sh.env("CI", ci);
                    }
                    if let Some(toolchain) = explicit {
                        sh.env("RUSTUP_TOOLCHAIN", toolchain);
                    }
                    let output = sh
                        .args([
                            "-ec",
                            &format!("{pin}\nprintf '%s' \"${{RUSTUP_TOOLCHAIN-unset}}\""),
                        ])
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    assert_eq!(output.stdout, expected.as_bytes());
                }
            }
            if matches!(name, "cargo-msrv" | "cargo-ci") {
                let msrv = commands
                    .iter()
                    .find(|command| command.contains("cargo +1.85 check"))
                    .unwrap();
                assert!(!msrv.contains("RUSTUP_TOOLCHAIN"), "{msrv}");
                let output = Command::new("/bin/sh")
                    .env("PATH", root)
                    .env_remove("SIMIT_TEST_UNSET")
                    .args(["-ec", &format!("{msrv}\nprintf '%s' \"$SIMIT_TEST_UNSET\"")])
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
            }
        }
    }
}

#[test]
fn combined_native_workspace_round_trips_crow_options_and_runs_valid_cargo_commands() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = '2'\nmembers = ['lib', 'cli', 'excluded']\n",
    )
    .unwrap();
    for (package, target) in [("lib", "lib"), ("cli", "main"), ("excluded", "main")] {
        fs::create_dir_all(root.join(package).join("src")).unwrap();
        fs::write(
            root.join(package).join("Cargo.toml"),
            format!("[package]\nname = '{package}'\nversion = '0.1.0'\nedition = '2024'\nrust-version = '1.93'\n[features]\nopt = []\n"),
        )
        .unwrap();
        fs::write(
            root.join(package).join("src").join(format!("{target}.rs")),
            if target == "main" {
                "fn main() {}\n"
            } else {
                "pub fn demo() {}\n"
            },
        )
        .unwrap();
    }
    fs::write(
        root.join("simit.toml"),
        r#"[ci]
provider = "crow"
platform = "forgejo"
runtime = "cargo"
workspace = true
workspace_strategy = "aggregate"
exclude_packages = ["excluded"]
package_features = { lib = ["opt"], cli = ["opt"] }
all_features = false
unit_tests_only = true
clippy_all_targets = false
format_all_rust_files = true
format_excludes = ["vendored/**"]
extra_setup = ["printf 'hello %s\\n' \"$${CI_REPO:-local}\""]
extra_env = { CARGO_PROFILE_TEST_DEBUG = "1" }
with_docs = true
local_gate = true
local_gate_steps = [{ name = "Extra local step", run = "true" }]

[[ci.manual_gates]]
id = "push_cache"
runtime = "nix"
run = "nix run .#cache"
env = { CACHE_NAME = "test" }
secret = "ATTIC_TOKEN"

[ci.crow]
combine_steps = true
cargo_home = ".crow-cargo"
shallow_clone = true
paths_include = ["**/*.rs"]
paths_exclude = ["docs/**"]
strip_ci_debug = true
build_jobs = 2
labels = { agent = "native-rust" }
nix_image = "example.org/ci/nix:stable"

[ci.crow.sccache]
enabled = true
env = { SCCACHE_CACHE_SIZE = "2G" }
secret = "SCCACHE_TOKEN"
"#,
    )
    .unwrap();

    let run = |args: &[&str]| {
        let output = simit().current_dir(root).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    };
    run(&["init", "ci"]);
    run(&["init", "ci", "--ci-provider", "crow", "--check", "--diff"]);
    let generated: Vec<_> = fs::read_dir(root.join(".crow"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(generated.len(), 2, "{generated:?}");
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(root.join(".crow/build.yaml")).unwrap()).unwrap();
    assert_eq!(workflow["clone"][0]["settings"]["depth"], 1);
    assert_eq!(workflow["clone"][0]["settings"]["lfs"], false);
    for trigger in workflow["when"].as_sequence().unwrap() {
        assert_eq!(trigger["path"]["include"][0], "**/*.rs");
        assert_eq!(trigger["path"]["exclude"][0], "docs/**");
    }
    let steps = workflow["steps"].as_sequence().unwrap();
    assert_eq!(steps.len(), 1);
    let step = &steps[0];
    assert_eq!(step["name"], "cargo-ci");
    assert_eq!(step["image"], "rust:1.93-trixie");
    for (key, value) in [
        ("CARGO_HOME", ".crow-cargo"),
        ("CARGO_BUILD_JOBS", "2"),
        ("CARGO_PROFILE_DEV_DEBUG", "0"),
        ("CARGO_PROFILE_TEST_DEBUG", "1"),
        ("CARGO_INCREMENTAL", "0"),
        ("RUSTC_WRAPPER", "sccache"),
        ("SCCACHE_CACHE_SIZE", "2G"),
    ] {
        assert_eq!(step["environment"][key], value, "{key}");
    }
    assert_eq!(
        step["environment"]["SCCACHE_TOKEN"]["from_secret"],
        "SCCACHE_TOKEN"
    );
    let commands: Vec<_> = step["commands"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|command| command.as_str().unwrap())
        .collect();
    assert!(commands[0].contains("$${RUSTUP_TOOLCHAIN:-}"));
    assert!(commands[1].starts_with("command -v sccache"));
    assert!(commands[2].starts_with("printf"));
    assert!(commands[3].contains("':!vendored/**' > \"$$tmp\""));
    assert_eq!(
        commands[4].lines().last().unwrap(),
        "cargo clippy --locked --lib --bins --features 'cli/opt,lib/opt' --workspace --exclude excluded -- --deny warnings"
    );
    assert_eq!(
        commands[5],
        "cargo test --locked --features lib/opt -p lib --lib"
    );
    assert_eq!(
        commands[6],
        "cargo test --locked --features cli/opt -p cli --bins"
    );
    assert!(commands[7].starts_with("cargo doc --locked --workspace --exclude excluded"));
    assert_eq!(commands[8], "sccache --show-stats || true");
    let gate: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(root.join(".crow/manual-push-cache.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(gate["when"][0]["event"], "manual");
    assert_eq!(gate["labels"]["agent"], "native-rust");
    assert_eq!(gate["steps"][0]["image"], "example.org/ci/nix:stable");
    assert_eq!(
        gate["steps"][0]["environment"]["ATTIC_TOKEN"]["from_secret"],
        "ATTIC_TOKEN"
    );
    assert_eq!(gate["steps"][0]["environment"]["CACHE_NAME"], "test");
    assert!(gate["steps"][0]["environment"]["NIX_CONFIG"].is_string());
    let script = fs::read_to_string(root.join("scripts/ci-local.sh")).unwrap();
    assert!(script.contains("${CI_REPO:-local}"), "{script}");
    assert!(!script.contains("$${CI_REPO:-local}"), "{script}");
    assert!(!script.contains("nix run .#cache"), "{script}");
    assert!(!script.contains("sccache"), "{script}");
    assert!(!script.contains("RUSTUP_TOOLCHAIN"), "{script}");
    let format_command = commands[3].replace("$$", "$");
    assert!(
        script.contains(&format_command.replace('\'', r"'\''")),
        "{script}"
    );
    assert!(
        script.ends_with("step 'Extra local step' 'true'\n"),
        "{script}"
    );

    // Prove Cargo accepts --lib --bins for mixed workspaces and each unit
    // test command applies only the features of the packages it selects.
    let output = std::process::Command::new("cargo")
        .current_dir(root)
        .args(["generate-lockfile", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new("bash")
        .current_dir(root)
        .args(["-ec", &commands[4..7].join("\n")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Rustfmt checks tracked fragments that Cargo fmt does not see, but
    // leaves excluded tracked files alone.
    fs::create_dir(root.join("lib_parts")).unwrap();
    fs::create_dir(root.join("vendored")).unwrap();
    fs::write(root.join("lib_parts/piece.rs"), "fn  piece(){}\n").unwrap();
    fs::write(root.join("vendored/ignored.rs"), "fn  ignored(){}\n").unwrap();
    let output = std::process::Command::new("bash")
        .current_dir(root)
        .args(["-ec", "git init -q && git add -- '*.rs'"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let format = || {
        std::process::Command::new("/bin/sh")
            .current_dir(root)
            .args(["-ec", &format_command])
            .output()
            .unwrap()
    };
    let tmp = root.join("tmp");
    fs::create_dir(&tmp).unwrap();
    let output = std::process::Command::new("/bin/sh")
        .current_dir(root)
        .env("TMPDIR", &tmp)
        .env("GIT_DIR", root.join("missing.git"))
        .args(["-ec", &format_command])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(128), "{output:?}");
    assert_eq!(fs::read_dir(tmp).unwrap().count(), 0);
    let output = format();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("lib_parts/piece.rs"));
    fs::write(root.join("lib_parts/piece.rs"), "fn piece() {}\n").unwrap();
    let output = format();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

const CONFIG: &str = r#"[ci]
provider = "crow"
platform = "forgejo"
runner = "codefloe-global"
runtime = "nix"
nix_builds = [
  ".#checks.x86_64-linux.wardrobe",
  ".#checks.x86_64-linux.measure-selftest",
]
"#;

#[test]
fn crow_flake_only_ci_builds_declared_installables_without_lfs() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    fs::write(root.join("simit.toml"), CONFIG).unwrap();
    let run = |args: &[&str]| {
        let output = simit().current_dir(root).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };

    run(&["init", "ci"]);
    let ci = fs::read_to_string(root.join(".crow/ci.yaml")).unwrap();
    let workflow: serde_yaml::Value = serde_yaml::from_str(&ci).unwrap();
    assert_eq!(workflow["labels"]["agent"], "codefloe-global");
    let clone = &workflow["clone"][0];
    assert_eq!(clone["settings"]["lfs"], false);
    assert_eq!(clone["settings"]["depth"], 0);
    let commands = workflow["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|step| step["commands"][0].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        commands,
        [
            "nix flake check --no-update-lock-file --no-build",
            "nix build '.#checks.x86_64-linux.wardrobe' --no-link --print-build-logs --no-update-lock-file",
            "nix build '.#checks.x86_64-linux.measure-selftest' --no-link --print-build-logs --no-update-lock-file",
        ]
    );
    assert!(!root.join(".github").exists());
    assert_eq!(fs::read_to_string(root.join("simit.toml")).unwrap(), CONFIG);

    run(&["init", "ci", "--check"]);
    let audit = run(&["projects", "audit", "--json", "."]);
    let audit: serde_json::Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert_eq!(audit["projects"][0]["ciStatus"], "managed", "{audit}");

    // Options only the Actions renderer honours fail instead of vanishing.
    fs::write(
        root.join("simit.toml"),
        format!("{CONFIG}extra_setup = [\"true\"]\n"),
    )
    .unwrap();
    let output = simit()
        .current_dir(root)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("require the Actions provider"));
}

#[test]
fn crow_flake_only_ci_rejects_the_crow_check_command() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        format!("{CONFIG}\n[ci.crow]\ncheck_command = \"just check\"\n"),
    )
    .unwrap();
    let output = simit()
        .current_dir(root)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not supported on the Crow Nix-only CI path"),
        "{stderr}"
    );
    assert!(!root.join(".crow").exists());
}

#[test]
fn crow_flake_only_ci_keeps_nix_builds_beside_the_crow_prebuild() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        format!("{CONFIG}\n[prebuild]\nprovider = \"crow\"\nnix_builds = [\".#server\"]\n"),
    )
    .unwrap();
    let output = simit()
        .current_dir(root)
        .args(["init", "ci"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ci = fs::read_to_string(root.join(".crow/ci.yaml")).unwrap();
    assert!(ci.contains("checks.x86_64-linux.wardrobe"), "{ci}");
    assert!(root.join(".crow/prebuild.yaml").is_file());
}

#[test]
fn crow_flake_only_audit_infers_an_unpersisted_runner() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    let config = CONFIG.replace("runner = \"codefloe-global\"\n", "");
    fs::write(root.join("simit.toml"), &config).unwrap();
    let run = |args: &[&str]| {
        let output = simit().current_dir(root).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };

    run(&["init", "ci", "--runner", "codefloe-global"]);
    // Flake-backed configuration cannot persist the override; model that.
    fs::write(root.join("simit.toml"), &config).unwrap();
    let ci = fs::read_to_string(root.join(".crow/ci.yaml")).unwrap();
    assert!(ci.contains("agent: codefloe-global"), "{ci}");
    let audit = run(&["projects", "audit", "--json", "."]);
    let audit: serde_json::Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert_eq!(audit["projects"][0]["ciStatus"], "managed", "{audit}");
}

#[test]
fn crow_flake_only_ci_builds_checks_when_selected_and_replaces_jsonnet() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    fs::write(
        root.join("simit.toml"),
        format!("{CONFIG}components = [\"checks\"]\n"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = simit().current_dir(root).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };

    run(&["init", "ci", "--crow-format", "jsonnet"]);
    assert!(root.join(".crow/ci.jsonnet").is_file());
    run(&["init", "ci", "--crow-format", "yaml"]);
    assert!(!root.join(".crow/ci.jsonnet").exists());
    let ci = fs::read_to_string(root.join(".crow/ci.yaml")).unwrap();
    assert!(
        ci.contains("nix flake check --no-update-lock-file\n"),
        "{ci}"
    );
    assert!(!ci.contains("--no-build"), "{ci}");
    let audit = run(&["projects", "audit", "--json", "."]);
    let audit: serde_json::Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert_eq!(audit["projects"][0]["ciStatus"], "managed", "{audit}");
}
