use std::fs;
use std::path::Path;

mod common;

fn fixture(runtime: &str, split: bool, all_features: bool) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn example() {}\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "runtime-gates-fixture"
version = "0.1.0"
edition = "2021"
rust-version = "1.88"
license = "MIT"

[features]
default = []
cli = []

[[bin]]
name = "runtime-gates-fixture"
path = "src/main.rs"
required-features = ["cli"]
"#,
    )
    .unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    let runners = if split {
        "step_runners = { cargo-test = \"ubuntu-22.04\" }\n"
    } else {
        ""
    };
    // GitHub deliberately ignores Forgejo step-runner overrides. Use Forgejo
    // for the split-job fixture so these tests exercise the multi-job renderer.
    let platform = if split { "forgejo" } else { "github" };
    fs::write(
        root.join("simit.toml"),
        format!(
            "[ci]\nplatform = \"{platform}\"\nprovider = \"actions\"\nruntime = \"{runtime}\"\nrunner = \"ubuntu-24.04\"\nall_features = {all_features}\nwith_msrv = true\npublish_crates = true\n{runners}"
        ),
    )
    .unwrap();
    let output = common::simit()
        .current_dir(root)
        .args(["init", "ci", "--platform", platform])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if split {
        let yaml = fs::read_to_string(root.join(".forgejo/workflows/ci.yaml")).unwrap();
        let document: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert!(document["jobs"].as_mapping().unwrap().len() > 1);
    }
    directory
}

fn commands(root: &Path, name: &str) -> Vec<String> {
    let platform = if root.join(".forgejo").exists() {
        "forgejo"
    } else {
        "github"
    };
    let workflow =
        fs::read_to_string(root.join(format!(".{platform}/workflows/{name}.yaml"))).unwrap();
    let document: serde_yaml::Value = serde_yaml::from_str(&workflow).unwrap();
    document["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .flat_map(|job| job["steps"].as_sequence().unwrap())
        .filter_map(|step| step["run"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn nix_ci_and_publisher_cover_cli_library_and_the_declared_compiler() {
    for split in [false, true] {
        let directory = fixture("nix", split, true);
        for name in ["ci", "publish-crate"] {
            let runs = commands(directory.path(), name);
            for command in [
                "nix develop -c cargo test --all-features",
                "nix develop -c cargo test --no-default-features",
                "nix develop -c cargo clippy --all-targets --all-features -- --deny warnings",
                "nix develop -c cargo clippy --all-targets --no-default-features -- --deny warnings",
            ] {
                assert!(
                    runs.iter().any(|run| run == command),
                    "{name}, split={split}: missing {command}: {runs:?}"
                );
            }
            let msrv = runs
                .iter()
                .find(|run| run.contains("nix develop .#msrv"))
                .expect("MSRV must select its dedicated shell");
            assert!(
                msrv.contains("rustc --version"),
                "MSRV must verify the compiler identity: {msrv}"
            );
            assert!(
                msrv.contains("1.88.0"),
                "MSRV must use Cargo's minimum version: {msrv}"
            );
            assert!(
                msrv.contains("cargo check --all-targets --all-features"),
                "MSRV must compile the optional CLI: {msrv}"
            );
            assert!(
                !runs
                    .iter()
                    .any(|run| run == "nix develop -c cargo check --all-targets")
            );
        }
    }
}

#[test]
fn cargo_split_jobs_keep_feature_and_msrv_selection() {
    let directory = fixture("cargo", true, true);
    let runs = commands(directory.path(), "ci");
    assert!(runs.iter().any(|run| run == "cargo test --all-features"));
    assert!(
        runs.iter()
            .any(|run| run == "cargo test --no-default-features")
    );
    assert!(
        runs.iter()
            .any(|run| run.contains("cargo +1.88 check --all-targets --all-features"))
    );
    assert!(
        runs.iter()
            .any(|run| run.contains("rustup toolchain install 1.88 --profile minimal"))
    );
}

#[test]
fn explicit_default_feature_policy_is_respected_by_nix_jobs() {
    let directory = fixture("nix", false, false);
    for name in ["ci", "publish-crate"] {
        let runs = commands(directory.path(), name);
        assert!(runs.iter().any(|run| run == "nix develop -c cargo test"));
        assert!(
            runs.iter()
                .any(|run| run == "nix develop -c cargo test --no-default-features")
        );
        assert!(!runs.iter().any(|run| run.contains("--all-features")));
    }
}

#[test]
#[cfg(unix)]
fn nix_format_gate_supports_legacy_shells_and_propagates_formatter_failures() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;

    for split in [false, true] {
        let directory = fixture("nix", split, false);
        let root = directory.path();
        let gate = commands(root, "ci")
            .into_iter()
            .find(|run| run.contains("bin/treefmt\" --ci"))
            .unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        symlink("/bin/sh", bin.join("sh")).unwrap();
        let wrapper = root.join("formatter");
        fs::create_dir_all(wrapper.join("bin")).unwrap();
        let wrapper_bin = wrapper.join("bin/treefmt");
        fs::write(&wrapper_bin, "#!/bin/sh\nprintf 'project-treefmt %s\\n' \"$*\" > \"$FORMAT_LOG\"\nexit \"$FORMAT_STATUS\"\n").unwrap();
        fs::set_permissions(&wrapper_bin, fs::Permissions::from_mode(0o755)).unwrap();
        for (name, script) in [
            (
                "nix",
                "#!/bin/sh\ncase \"$1\" in\n eval) printf '%s' \"$FORMAT_WRAPPER\"; exit \"$EVAL_STATUS\" ;;\n build) printf '%s' \"$FORMAT_WRAPPER\"; exit 0 ;;\n esac\nshift 2\nPATH=\"$FORMAT_BIN\" exec \"$@\"\n",
            ),
            (
                "cargo",
                "#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" > \"$FORMAT_LOG\"\nexit \"$FORMAT_STATUS\"\n",
            ),
            (
                "treefmt",
                "#!/bin/sh\nprintf 'treefmt %s\\n' \"$*\" > \"$FORMAT_LOG\"\nexit \"$FORMAT_STATUS\"\n",
            ),
        ] {
            let file = bin.join(name);
            fs::write(&file, script).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        for (project_wrapper, treefmt, status, expected) in [
            (true, true, 0, "project-treefmt --ci\n"),
            (true, true, 17, "project-treefmt --ci\n"),
            (false, true, 0, "cargo fmt --all -- --check\n"),
            (false, true, 17, "cargo fmt --all -- --check\n"),
            (false, false, 0, "cargo fmt --all -- --check\n"),
            (false, false, 17, "cargo fmt --all -- --check\n"),
        ] {
            if !treefmt && bin.join("treefmt").exists() {
                fs::remove_file(bin.join("treefmt")).unwrap();
            }
            let log = root.join("format.log");
            let output = Command::new("sh")
                .current_dir(root)
                .args(["-c", &gate])
                .env("PATH", &bin)
                .env("FORMAT_BIN", &bin)
                .env("FORMAT_LOG", &log)
                .env("FORMAT_STATUS", status.to_string())
                .env("EVAL_STATUS", "0")
                .env(
                    "FORMAT_WRAPPER",
                    if project_wrapper {
                        wrapper.as_os_str()
                    } else {
                        std::ffi::OsStr::new("")
                    },
                )
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(status),
                "split={split}, treefmt={treefmt}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(fs::read_to_string(&log).unwrap(), expected);
        }
        let log = root.join("eval-failure.log");
        let output = Command::new("sh")
            .current_dir(root)
            .args(["-c", &gate])
            .env("PATH", &bin)
            .env("FORMAT_BIN", &bin)
            .env("FORMAT_WRAPPER", "")
            .env("FORMAT_LOG", &log)
            .env("FORMAT_STATUS", "0")
            .env("EVAL_STATUS", "23")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(23));
        assert!(
            !log.exists(),
            "evaluation errors must not silently select Cargo formatting"
        );
    }
}

#[test]
#[cfg(unix)]
fn github_nix_input_transport_works_without_a_runner_ssh_key() {
    use std::process::Command;

    let directory = fixture("nix", false, false);
    let config_path = directory.path().join("simit.toml");
    let mut project_config = fs::read_to_string(&config_path).unwrap();
    project_config.push_str("\n[release.github]\nrepo = \"example/runtime-gates-fixture\"\n");
    fs::write(&config_path, project_config).unwrap();
    let release = common::simit()
        .current_dir(directory.path())
        .args(["init", "release", "--platform", "github"])
        .output()
        .unwrap();
    assert!(
        release.status.success(),
        "{}",
        String::from_utf8_lossy(&release.stderr)
    );
    let release_yaml =
        fs::read_to_string(directory.path().join(".github/workflows/release.yml")).unwrap();
    let release_doc: serde_yaml::Value = serde_yaml::from_str(&release_yaml).unwrap();
    let release_transport = release_doc["jobs"]["release"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find_map(|step| step["run"].as_str().filter(|run| run.contains("insteadOf")))
        .expect("Comprehensive release jobs must also normalize GitHub SSH input transport");
    let setup = commands(directory.path(), "ci")
        .into_iter()
        .find(|run| run.contains("insteadOf"))
        .expect("GitHub Nix jobs must normalize GitHub SSH input transport");
    let config = directory.path().join("git-fixture-config");
    assert!(
        Command::new("sh")
            .args(["-c", &setup])
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("sh")
            .args(["-c", release_transport])
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success()
    );
    for (url, expected) in [
        (
            "ssh://git@github.com/caniko/harbor-rs.git",
            "https://github.com/caniko/harbor-rs.git",
        ),
        (
            "git@github.com:caniko/harbor-rs.git",
            "https://github.com/caniko/harbor-rs.git",
        ),
        (
            "ssh://git@codefloe.com/caniko/cotton.git",
            "ssh://git@codefloe.com/caniko/cotton.git",
        ),
    ] {
        let result = Command::new("git")
            .args(["ls-remote", "--get-url", url])
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), expected);
    }
}

#[test]
fn generated_flakes_supply_a_native_msrv_shell_for_both_build_modes() {
    for cross in [false, true] {
        let directory = fixture("nix", false, true);
        fs::remove_file(directory.path().join("flake.nix")).unwrap();
        let mut command = common::simit();
        command
            .current_dir(directory.path())
            .args(["init", "flake", "--scope", "full"]);
        if cross {
            command.args(["--cross", "--target", "native"]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let flake = fs::read_to_string(directory.path().join("flake.nix")).unwrap();
        assert!(flake.contains("msrv ="), "cross={cross}: {flake}");
        assert!(
            flake.contains("channel = \"1.88.0\""),
            "cross={cross}: {flake}"
        );
        assert!(flake.contains("CARGO_ENCODED_RUSTFLAGS = \"\""));
        assert!(flake.contains("RUSTFLAGS = \"\""));
    }
}

#[test]
#[cfg(unix)]
fn msrv_gate_rejects_a_wrong_compiler_before_running_cargo() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let directory = fixture("nix", false, true);
    let root = directory.path();
    let nix = root.join("nix");
    fs::write(&nix, "#!/bin/sh\ncase \"$4\" in\n  rustc) echo \"rustc $FAKE_VERSION (fixture)\" ;;\n  cargo) touch \"$CARGO_MARKER\" ;;\n  *) exit 2 ;;\nesac\n").unwrap();
    fs::set_permissions(&nix, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(root.to_owned())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for name in ["ci", "publish-crate"] {
        let runs = commands(root, name);
        let msrv = runs
            .iter()
            .find(|run| run.contains("nix develop .#msrv"))
            .unwrap();
        for (version, success) in [("1.96.0", false), ("1.88.0", true)] {
            let marker = root.join(format!("{name}-{version}"));
            let output = Command::new("sh")
                .args(["-c", msrv])
                .env("PATH", &path)
                .env("FAKE_VERSION", version)
                .env("CARGO_MARKER", &marker)
                .output()
                .unwrap();
            assert_eq!(
                output.status.success(),
                success,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                marker.exists(),
                success,
                "must only compile with the declared MSRV"
            );
        }
    }
}

#[test]
fn nix_nextest_gates_use_executable_tool_probes_and_feature_selection() {
    let directory = fixture("nix", true, true);
    let output = common::simit()
        .current_dir(directory.path())
        .args(["init", "ci", "--platform", "forgejo", "--with-nextest"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["ci", "publish-crate"] {
        let runs = commands(directory.path(), name);
        assert!(
            runs.iter()
                .any(|run| run == "nix develop -c cargo-nextest --version")
        );
        assert!(
            runs.iter()
                .any(|run| run == "nix develop -c cargo nextest run --all-features")
        );
        assert!(
            runs.iter()
                .any(|run| run == "nix develop -c cargo nextest run --no-default-features")
        );
    }
}
