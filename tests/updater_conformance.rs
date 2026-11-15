//! Generator/adapter conformance: every workflow shape the generator emits
//! must pass the GitHub adapter's source admission, for the default branch
//! and for an updater maintenance branch. The two used to be hand-synced
//! lists, so a mismatch first appeared as a refused fleet landing.
use std::{fs, path::Path};

use tempfile::TempDir;

mod common;

const CRATE: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
const NIX: &str =
    "platform = \"github\"\nprovider = \"actions\"\nruntime = \"nix\"\nrunner = \"ubuntu-24.04\"\n";
const BUILDS: &str =
    "nix_builds = [\".#packages.x86_64-linux.demo\", \".#checks.x86_64-linux.demo\"]\n";
const GROUPS: &str = "[ci.nix_build.groups]\nall = [\".#packages.x86_64-linux.demo\", \".#checks.x86_64-linux.demo\"]\n";
const PREBUILD: &str = "[prebuild]\nnix_builds = [\".#packages.x86_64-linux.demo\"]\n[prebuild.system_runners]\nx86_64-linux = \"ubuntu-24.04\"\n";

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A single crate, or a two-member workspace, with a flake beside it.
fn project(workspace: bool, config: &str) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    if workspace {
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/alpha\", \"crates/beta\"]\nresolver = \"3\"\n",
        );
        for member in ["alpha", "beta"] {
            write(
                root,
                &format!("crates/{member}/Cargo.toml"),
                &CRATE.replace("demo", member),
            );
            write(root, &format!("crates/{member}/src/lib.rs"), "");
        }
    } else {
        write(root, "Cargo.toml", CRATE);
        write(root, "src/lib.rs", "");
    }
    write(root, "flake.nix", "{ outputs = _: {}; }\n");
    write(root, "simit.toml", config);
    temp
}

/// The plan a fleet update commits: `init ci --primary-only --plan-json`.
fn plan(root: &Path) -> simit::project::CiUpdatePlan {
    let output = common::simit()
        .current_dir(root)
        .env("SIMIT_NO_REGISTRY", "1")
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .args(["init", "ci", "--primary-only", "--plan-json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn every_generated_workflow_shape_passes_updater_source_admission() {
    let ci = |options: &str| format!("[ci]\n{NIX}{options}");
    let trunk = "push_branches = [\"trunk\"]\n";
    // (name, workspace, simit.toml, refusal expected until the generator and
    // adapter share one declaration of that shape)
    let cases: Vec<(&str, bool, String, Option<&str>)> = vec![
        ("crate, cargo runtime", false, "[ci]\nplatform = \"github\"\nprovider = \"actions\"\nruntime = \"cargo\"\n".into(), None),
        ("crate", false, ci(""), None),
        ("crate, branch filter", false, ci(trunk), None),
        ("crate, required gate", false, ci("required_gates = [{ id = \"harbor-ci\", run = \"nix develop -c harbor-ci\" }]\n"), None),
        ("aggregate workspace", true, ci("workspace = true\nworkspace_strategy = \"aggregate\"\n"), None),
        ("aggregate workspace, branch filter", true, ci(&format!("workspace = true\nworkspace_strategy = \"aggregate\"\n{trunk}")), None),
        ("package-scoped workspace", true, ci("workspace = true\n"), None),
        ("package-scoped workspace, branch filter", true, ci(&format!("workspace = true\n{trunk}")), None),
        ("nix builds", false, ci(BUILDS), None),
        ("nix builds, branch filter", false, ci(&format!("{BUILDS}{trunk}")), None),
        ("nix builds, evidence", false, ci(&format!("{BUILDS}[ci.nix_build]\ncapture_results = true\ntimeout_minutes = 60\nmax_jobs = 1\ncores = 2\n")), None),
        ("nix builds, evidence, branch filter", false, ci(&format!("{BUILDS}[ci.nix_build]\ncapture_results = true\npush_branches = [\"trunk\"]\n")), None),
        ("nix builds, separate events", false, ci(&format!("{BUILDS}[ci.nix_build]\ncapture_results = true\nseparate_events = true\n")), None),
        // A project whose only workflow is the installable matrix has no
        // `test` job, so a profile that requires `test` cannot enroll it.
        ("nix builds only", false, ci(&format!("{BUILDS}[ci.nix_build]\nonly = true\ncapture_results = true\n")), Some("required check test is not provably produced")),
        // TODO(optional steps): the grouped-build engine cache steps, native
        // KVM and diagnostics conditions are not source-provable yet, and an
        // unfiltered grouped workflow names its concurrency group by a literal.
        ("nix build groups", false, ci(&format!("{BUILDS}[ci.nix_build]\ncapture_results = true\n{GROUPS}")), Some("concurrency is not provably isolated")),
        ("nix build groups, branch filter", false, ci(&format!("{BUILDS}[ci.nix_build]\ncapture_results = true\npush_branches = [\"trunk\"]\n{GROUPS}")), Some("conditional")),
        ("nix builds, native KVM", false, ci(&format!("{BUILDS}[ci.nix_build]\nkvm = true\nsystem_runners = {{ x86_64-linux = \"ubuntu-24.04\" }}\n")), Some("conditional")),
        ("nix builds, diagnostics", false, ci(&format!("{BUILDS}[ci.nix_build]\ncapture_results = true\npost_build_always = [\"true\"]\n")), Some("conditional")),
        ("prebuild", false, format!("{}{PREBUILD}", ci("")), None),
        ("prebuild, branch filter", false, format!("{}{PREBUILD}", ci("")).replace("[prebuild]\n", "[prebuild]\npush_branches = [\"trunk\"]\n"), None),
        ("prebuild, package-scoped workspace", true, format!("{}{PREBUILD}", ci("workspace = true\n")), None),
        ("prebuild, Attic", false, format!("{}{PREBUILD}[release.attic]\ncache = \"demo\"\nurl = \"https://attic.example\"\ntoken_name = \"demo\"\ntoken_secret = \"ATTIC_TOKEN\"\n", ci("")).replace("[prebuild]\n", "[prebuild]\npublish_attic = true\nattic_app = \".#push-flake-inputs\"\n"), None),
        ("prebuild, release entry", false, format!("{}{PREBUILD}[release.artifacts]\nprebuild_binaries = true\n", ci("")).replace("[prebuild]\n", "[prebuild]\nrelease_archives = true\n"), None),
    ];
    let mut failures = Vec::new();
    for (name, workspace, config, refusal) in cases {
        let temp = project(workspace, &config);
        let plan = plan(temp.path());
        for branch in ["trunk", "simit/update/0123456789abcdef"] {
            let admitted =
                simit::commands::projects_update::admit_candidate_source(&plan, branch, &["test"]);
            match (admitted, refusal) {
                (Ok(()), None) => {}
                (Err(error), Some(expected)) if format!("{error:#}").contains(expected) => {}
                (Ok(()), Some(_)) => failures.push(format!(
                    "{name} on {branch}: admitted; drop its expected refusal"
                )),
                (Err(error), _) => failures.push(format!("{name} on {branch}: {error:#}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
