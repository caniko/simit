//! Qualification runs candidate files even when Actions reads a merge workflow.
use std::{collections::BTreeSet, fs, path::Path, process::Command};

#[test]
fn qualification_entrypoints_exist_in_the_candidate_and_packaged_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let package = Command::new("cargo")
        .current_dir(root)
        .args(["package", "--list", "--allow-dirty", "--offline"])
        .output()
        .unwrap();
    assert!(package.status.success(), "{package:?}");
    let files: BTreeSet<_> = String::from_utf8(package.stdout)
        .unwrap()
        .lines()
        .map(|line| line.replace(std::path::MAIN_SEPARATOR, "/"))
        .collect();
    let mut entrypoints = BTreeSet::new();
    assert!(
        files.contains("src/monorepo/nix_diagnostics.sh"),
        "package omits the embedded provisioning/native diagnostics asset"
    );
    assert!(
        files.contains("src/monorepo/provision.sh"),
        "package omits the embedded provisioning watchdog"
    );
    for directory in [".github/workflows", ".simit/workflows"] {
        for entry in fs::read_dir(root.join(directory)).unwrap() {
            let path = entry.unwrap().path();
            // Observer enrollment JSON lives beside templates, but is data,
            // not a workflow with executable job steps to inventory.
            if ![".yaml", ".yml", ".yaml.in", ".yml.in"]
                .iter()
                .any(|suffix| path.to_string_lossy().ends_with(suffix))
            {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let yaml: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            for (_, job) in yaml["jobs"].as_mapping().unwrap() {
                let Some(steps) = job["steps"].as_sequence() else {
                    continue;
                };
                for step in steps {
                    let Some(run) = step["run"].as_str() else {
                        continue;
                    };
                    // These are literal repository entrypoints, not generated
                    // temporary paths or remote engine/store paths.
                    for word in run.split_whitespace() {
                        let word = word.trim_end_matches(';').trim_matches(['\'', '"']);
                        if !word.ends_with('/')
                            && ["scripts/", "tests/fixtures/", "src/"]
                                .iter()
                                .any(|prefix| word.starts_with(prefix))
                        {
                            assert!(
                                root.join(word).is_file(),
                                "{} references missing candidate entrypoint {word}",
                                path.display()
                            );
                            assert!(
                                files.contains(word),
                                "package omits {word} referenced by {}",
                                path.display()
                            );
                            entrypoints.insert(word.to_owned());
                        }
                    }
                }
            }
        }
    }
    for required in [
        "scripts/qualify-nix-transport.py",
        "scripts/qualify-native-groups.py",
        "scripts/qualify-updater-sandbox-host.sh",
        "scripts/qualify-updater-generator.py",
    ] {
        assert!(
            entrypoints.contains(required),
            "qualification no longer exercises {required}"
        );
    }
}

#[test]
fn generator_qualification_checks_out_the_exact_candidate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in [
        ".github/workflows/qualify-generator.yaml",
        ".simit/workflows/qualify-generator.yaml.in",
    ] {
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(root.join(name)).unwrap()).unwrap();
        let checkout = yaml["jobs"]["qualify"]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|action| action.starts_with("actions/checkout@"))
            })
            .unwrap();
        assert_eq!(
            checkout["with"]["ref"].as_str(),
            Some("${{ github.event.pull_request.head.sha || github.sha }}"),
            "{name}"
        );
        assert_eq!(
            checkout["with"]["repository"].as_str(),
            Some("${{ github.event.pull_request.head.repo.full_name || github.repository }}"),
            "{name}"
        );
        assert_eq!(
            checkout["with"]["persist-credentials"].as_bool(),
            Some(false)
        );
    }
}
