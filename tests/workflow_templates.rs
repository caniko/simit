use std::{fs, path::Path, process::Output};

use simit::registry::{FeatureStatus, audit_ci};
use tempfile::TempDir;

mod common;

fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("flake.nix"), "{ outputs = _: {}; }\n").unwrap();
    fs::create_dir_all(temp.path().join(".simit/templates")).unwrap();
    fs::write(temp.path().join(".simit/templates/tests.yml"),
        "name: Project tests\non: [push]\njobs:\n  test:\n    runs-on: '@simit(runner)@'\n    steps:\n      - run: echo '${{ github.sha }}'\n").unwrap();
    config(
        &temp,
        ".github/workflows/tests.yml",
        ".simit/templates/tests.yml",
        "ubuntu-24.04",
    );
    temp
}

fn config(temp: &TempDir, output: &str, source: &str, runner: &str) {
    fs::write(temp.path().join("simit.toml"), format!(
        "[ci]\nplatform='github'\nprovider='actions'\nruntime='nix'\nrunner='ubuntu-24.04'\nnix_builds=['.#default']\n[ci.nix_build]\nonly=true\n[ci.workflow_templates]\n'{output}'='{source}'\n[ci.workflow_variables]\nrunner='{runner}'\n")).unwrap();
}

fn generate(temp: &TempDir, args: &[&str]) -> Output {
    common::simit()
        .current_dir(temp.path())
        .args(["init", "ci"])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn project_templates_round_trip_and_share_generation_drift_and_registry_ownership() {
    let temp = project();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let path = temp.path().join(".github/workflows/tests.yml");
    let original = fs::read_to_string(&path).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&original).unwrap();
    assert_eq!(yaml["jobs"]["test"]["runs-on"], "ubuntu-24.04");
    assert_eq!(
        yaml["jobs"]["test"]["steps"][0]["run"],
        "echo '${{ github.sha }}'"
    );
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
    fs::write(
        &path,
        original.replace("ubuntu-24.04", "unavailable-runner"),
    )
    .unwrap();
    assert!(!generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(audit_ci(temp.path()).unwrap().status, FeatureStatus::Drift);
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(original, fs::read_to_string(&path).unwrap());
    config(
        &temp,
        ".github/workflows/tests.yml",
        ".simit/templates/tests.yml",
        "windows-11-arm",
    );
    assert!(generate(&temp, &[]).status.success());
    let updated: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(updated["jobs"]["test"]["runs-on"], "windows-11-arm");
    // Removing a declaration retires its marked output, preserving foreign files.
    let cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        cfg.split("[ci.workflow_templates]").next().unwrap(),
    )
    .unwrap();
    let foreign = temp.path().join(".github/workflows/foreign.yml");
    fs::write(&foreign, "name: foreign\n").unwrap();
    assert!(!generate(&temp, &["--check"]).status.success());
    assert!(generate(&temp, &[]).status.success());
    assert!(!path.exists());
    assert_eq!(fs::read_to_string(foreign).unwrap(), "name: foreign\n");
}

#[test]
fn template_encoding_bom_stays_outside_the_header_prefixed_yaml_payload() {
    let temp = project();
    let source = temp.path().join(".simit/templates/tests.yml");
    let payload = fs::read_to_string(&source).unwrap();
    let with_bom = format!("\u{feff}{payload}");
    fs::write(&source, &with_bom).unwrap();
    let original: serde_yaml::Value = serde_yaml::from_str(&payload).unwrap();
    assert_eq!(original["name"], "Project tests");
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let path = temp.path().join(".github/workflows/tests.yml");
    let generated = fs::read_to_string(&path).unwrap();
    assert!(
        !generated.contains('\u{feff}'),
        "the encoding prefix must not become a payload character: {generated:?}"
    );
    let yaml: serde_yaml::Value = serde_yaml::from_str(&generated).unwrap();
    assert_eq!(yaml["name"], original["name"]);
    assert_eq!(yaml["jobs"]["test"]["runs-on"], "ubuntu-24.04");
    assert_eq!(fs::read_to_string(&source).unwrap(), with_bom);
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
    assert!(generate(&temp, &[]).status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), generated);
    assert_eq!(fs::read_to_string(&source).unwrap(), with_bom);
}

#[test]
fn invalid_templates_fail_before_any_outputs_are_written() {
    for (output, source, template) in [
        ("../escape.yml", ".simit/templates/tests.yml", "name: bad\n"),
        (
            ".github/workflows/tests.yml",
            "../escape.yml",
            "name: bad\n",
        ),
        (
            ".github/workflows/nix-builds.yaml",
            ".simit/templates/tests.yml",
            "name: bad\n",
        ),
        (
            ".github/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: '@simit(missing)@'\n",
        ),
        (
            ".github/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: [\n",
        ),
        (
            ".forgejo/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: bad\n",
        ),
        (
            ".github/workflows/tests.yml",
            ".simit/templates/tests.yml",
            "name: '@simit(unterminated'\n",
        ),
    ] {
        let temp = project();
        config(&temp, output, source, "ubuntu-24.04");
        fs::write(temp.path().join(".simit/templates/tests.yml"), template).unwrap();
        let result = generate(&temp, &[]);
        assert!(!result.status.success(), "{result:?}");
        assert!(
            !temp
                .path()
                .join(".github/workflows/nix-builds.yaml")
                .exists()
        );
        assert!(!temp.path().join(".github/workflows/tests.yml").exists());
    }
}

#[test]
fn yaml_nonprintable_characters_in_template_paths_fail_before_any_writes() {
    let characters = (1..=0x1f)
        .chain(0x7f..=0x9f)
        .chain([0x2028, 0x2029, 0xfffe, 0xffff])
        .map(|value| char::from_u32(value).unwrap());
    for separator in characters {
        for source_path in [false, true] {
            let temp = project();
            assert!(generate(&temp, &[]).status.success());
            let cfg_path = temp.path().join("simit.toml");
            let original_cfg = fs::read_to_string(&cfg_path).unwrap();
            let builtin = temp.path().join(".github/workflows/nix-builds.yaml");
            let builtin_bytes = fs::read(&builtin).unwrap();
            let template_output = temp.path().join(".github/workflows/tests.yml");
            let output_bytes = fs::read(&template_output).unwrap();
            let original_source = temp.path().join(".simit/templates/tests.yml");
            let source_bytes = fs::read(&original_source).unwrap();
            let bad_path = if source_path {
                format!(".simit/templates/break{separator}in-source.yml")
            } else {
                format!(".github/workflows/break{separator}in-output.yml")
            };
            // Windows refuses control characters in filenames itself. The
            // mapping must still fail before any generated files are written.
            let bad_source_created = source_path && (!cfg!(windows) || !separator.is_control());
            if bad_source_created {
                fs::write(temp.path().join(&bad_path), &source_bytes).unwrap();
            }
            let (output, source) = if source_path {
                (".github/workflows/tests.yml", bad_path.as_str())
            } else {
                (bad_path.as_str(), ".simit/templates/tests.yml")
            };
            // Use TOML's escaped string representation so forbidden literal
            // TOML controls reach path validation as actual decoded characters.
            let mut cfg: toml_edit::DocumentMut = original_cfg.parse().unwrap();
            let mappings = cfg["ci"]["workflow_templates"].as_table_mut().unwrap();
            mappings.clear();
            mappings.insert(output, toml_edit::value(source));
            fs::write(&cfg_path, cfg.to_string()).unwrap();
            let candidate_cfg = fs::read_to_string(&cfg_path).unwrap();
            for args in [vec![], vec!["--check", "--diff"]] {
                let result = generate(&temp, &args);
                assert!(!result.status.success(), "{bad_path:?}: {result:?}");
                assert!(
                    String::from_utf8_lossy(&result.stderr)
                        .contains("invalid [ci.workflow_templates] mapping"),
                    "{result:?}"
                );
                assert_eq!(fs::read(&builtin).unwrap(), builtin_bytes);
                assert_eq!(fs::read(&template_output).unwrap(), output_bytes);
                assert_eq!(fs::read(&original_source).unwrap(), source_bytes);
                assert_eq!(fs::read_to_string(&cfg_path).unwrap(), candidate_cfg);
                if bad_source_created {
                    assert_eq!(fs::read(temp.path().join(&bad_path)).unwrap(), source_bytes);
                } else if !source_path {
                    assert!(!temp.path().join(&bad_path).exists());
                }
            }
            fs::write(&cfg_path, original_cfg).unwrap();
            assert!(generate(&temp, &["--check", "--diff"]).status.success());
        }
    }
}

#[test]
fn windows_invalid_template_components_fail_before_any_writes_on_every_host() {
    let mut components = vec![
        "CON".to_owned(),
        "prn".to_owned(),
        "AuX".to_owned(),
        "NUL".to_owned(),
        "bad<name".to_owned(),
        "bad>name".to_owned(),
        "bad:name".to_owned(),
        "bad\"name".to_owned(),
        "bad\\name".to_owned(),
        "bad|name".to_owned(),
        "bad?name".to_owned(),
        "bad*name".to_owned(),
    ];
    for prefix in ["COM", "lpt"] {
        for suffix in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"] {
            components.push(format!("{prefix}{suffix}"));
        }
    }
    for component in components {
        for source_path in [false, true] {
            let temp = project();
            assert!(generate(&temp, &[]).status.success());
            let cfg_path = temp.path().join("simit.toml");
            let original_cfg = fs::read_to_string(&cfg_path).unwrap();
            let paths = [
                ".github/workflows/nix-builds.yaml",
                ".github/workflows/tests.yml",
                ".simit/templates/tests.yml",
            ];
            let before = paths.map(|path| fs::read(temp.path().join(path)).unwrap());
            let bad_path = if source_path {
                format!(".simit/templates/{component}/tests.yml")
            } else {
                format!(".github/workflows/{component}.yml")
            };
            let (output, source) = if source_path {
                (".github/workflows/tests.yml", bad_path.as_str())
            } else {
                (bad_path.as_str(), ".simit/templates/tests.yml")
            };
            let mut cfg: toml_edit::DocumentMut = original_cfg.parse().unwrap();
            let mappings = cfg["ci"]["workflow_templates"].as_table_mut().unwrap();
            mappings.clear();
            mappings.insert(output, toml_edit::value(source));
            fs::write(&cfg_path, cfg.to_string()).unwrap();
            let candidate_cfg = fs::read_to_string(&cfg_path).unwrap();
            // Do not create these paths even on Unix: rejection must happen in
            // portable configuration validation, before filesystem resolution.
            for args in [vec![], vec!["--check", "--diff"]] {
                let result = generate(&temp, &args);
                assert!(!result.status.success(), "{bad_path:?}: {result:?}");
                assert!(
                    String::from_utf8_lossy(&result.stderr)
                        .contains("invalid [ci.workflow_templates] mapping"),
                    "{bad_path:?}: {result:?}"
                );
                for (path, bytes) in paths.iter().zip(&before) {
                    assert_eq!(fs::read(temp.path().join(path)).unwrap(), *bytes);
                }
                assert_eq!(fs::read_to_string(&cfg_path).unwrap(), candidate_cfg);
            }
            fs::write(&cfg_path, &original_cfg).unwrap();
            assert!(generate(&temp, &["--check", "--diff"]).status.success());
        }
    }
    for component in ["parent.", "parent ", "NUL.txt", "AUX.tar.gz"] {
        let temp = project();
        let mut cfg: toml_edit::DocumentMut = fs::read_to_string(temp.path().join("simit.toml"))
            .unwrap()
            .parse()
            .unwrap();
        cfg["ci"]["workflow_templates"][".github/workflows/tests.yml"] =
            toml_edit::value(format!(".simit/{component}/tests.yml"));
        fs::write(temp.path().join("simit.toml"), cfg.to_string()).unwrap();
        let result = generate(&temp, &[]);
        assert!(!result.status.success(), "{component:?}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("invalid [ci.workflow_templates] mapping"),
            "{component:?}: {result:?}"
        );
        assert!(!temp.path().join(".github/workflows").exists());
    }
}

#[test]
fn portable_template_names_near_windows_devices_remain_usable() {
    for component in [
        "CONifer",
        "auxiliary",
        "COM0",
        "com10",
        "LPT0",
        "lpt10",
        "naïve",
        ".hidden",
    ] {
        let temp = project();
        let source = format!(".simit/templates/{component}.yml");
        let output = format!(".github/workflows/{component}.yml");
        fs::copy(
            temp.path().join(".simit/templates/tests.yml"),
            temp.path().join(&source),
        )
        .unwrap();
        config(&temp, &output, &source, "ubuntu-24.04");
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{component:?}: {result:?}");
        assert!(temp.path().join(&output).is_file());
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
    }
}

#[test]
fn active_workflow_sources_are_rejected_without_rewriting_builtins() {
    for platform in ["github", "forgejo"] {
        for extension in ["yml", "yaml"] {
            let temp = project();
            assert!(generate(&temp, &[]).status.success());
            let builtin = temp.path().join(".github/workflows/nix-builds.yaml");
            let before = fs::read_to_string(&builtin).unwrap();
            let source = format!(".{platform}/workflows/tests-template.{extension}");
            let path = temp.path().join(&source);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let template =
                fs::read_to_string(temp.path().join(".simit/templates/tests.yml")).unwrap();
            fs::write(&path, &template).unwrap();
            config(
                &temp,
                ".github/workflows/tests.yml",
                &source,
                "windows-2022",
            );
            let cfg_path = temp.path().join("simit.toml");
            let cfg = fs::read_to_string(&cfg_path)
                .unwrap()
                .replace("nix_builds=['.#default']", "nix_builds=['.#changed']");
            fs::write(&cfg_path, &cfg).unwrap();
            for args in [vec![], vec!["--check", "--diff"]] {
                let result = generate(&temp, &args);
                assert!(!result.status.success(), "{source}: {result:?}");
                assert!(
                    String::from_utf8_lossy(&result.stderr).contains("active Actions workflow")
                );
                assert_eq!(fs::read_to_string(&builtin).unwrap(), before);
                assert_eq!(fs::read_to_string(&path).unwrap(), template);
                assert_eq!(fs::read_to_string(&cfg_path).unwrap(), cfg);
            }
        }
    }
}

#[test]
fn configured_template_sources_are_in_the_actual_cargo_package_file_set() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let manifest: toml_edit::DocumentMut = manifest.parse().unwrap();
    let templates = manifest["package"]["metadata"]["simit"]["ci"]["workflow_templates"]
        .as_table()
        .unwrap();
    assert!(!templates.is_empty());
    let output = std::process::Command::new("cargo")
        .current_dir(root)
        .args(["package", "--list", "--allow-dirty", "--offline"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let list = String::from_utf8(output.stdout).unwrap();
    for (_, source) in templates {
        let source = source.as_str().unwrap();
        assert!(
            list.lines()
                .any(|file| file.replace(std::path::MAIN_SEPARATOR, "/") == source),
            "package omits {source}: {list}"
        );
        assert!(
            root.join(source).is_file(),
            "Nix-filtered source omits {source}"
        );
    }
}

#[test]
fn hard_linked_active_workflow_sources_fail_before_any_writes() {
    for directory in [".github/workflows", ".forgejo/workflows"] {
        let temp = project();
        let source = temp.path().join(".simit/templates/tests.yml");
        let original = fs::read(&source).unwrap();
        let active = temp.path().join(directory).join("live.yml");
        fs::create_dir_all(active.parent().unwrap()).unwrap();
        fs::hard_link(&source, &active).unwrap();
        let config_path = temp.path().join("simit.toml");
        let config = fs::read(&config_path).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{directory}: {result:?}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("active Actions workflow"));
            assert_eq!(fs::read(&config_path).unwrap(), config);
            assert_eq!(fs::read(&source).unwrap(), original);
            assert_eq!(fs::read(&active).unwrap(), original);
            assert!(!temp.path().join(".github/workflows/tests.yml").exists());
            assert!(
                !temp
                    .path()
                    .join(".github/workflows/nix-builds.yaml")
                    .exists()
            );
        }
    }
}

#[test]
fn hard_linked_destinations_fail_without_mutating_sources_or_other_outputs() {
    for alias in [
        ".simit/templates/tests.yml",
        ".github/workflows/nix-builds.yaml",
        ".github/workflows/other.yml",
    ] {
        let temp = project();
        let cfg_path = temp.path().join("simit.toml");
        let cfg = fs::read_to_string(&cfg_path).unwrap().replace(
            "[ci.workflow_variables]",
            "'.github/workflows/other.yml'='.simit/templates/tests.yml'\n[ci.workflow_variables]",
        );
        fs::write(&cfg_path, &cfg).unwrap();
        assert!(generate(&temp, &[]).status.success());
        let output = temp.path().join(".github/workflows/tests.yml");
        let alias_path = temp.path().join(alias);
        fs::remove_file(&output).unwrap();
        fs::hard_link(&alias_path, &output).unwrap();
        let retained = [
            ".simit/templates/tests.yml",
            ".github/workflows/nix-builds.yaml",
            ".github/workflows/other.yml",
            ".github/workflows/tests.yml",
        ]
        .map(|path| (path, fs::read(temp.path().join(path)).unwrap()));
        let cfg = cfg
            .replace("ubuntu-24.04", "windows-2022")
            .replace("nix_builds=['.#default']", "nix_builds=['.#changed']");
        fs::write(&cfg_path, &cfg).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{alias}: {result:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("aliases another source or generated output")
            );
            for (path, content) in &retained {
                assert_eq!(fs::read(temp.path().join(path)).unwrap(), *content);
            }
            assert_eq!(fs::read_to_string(&cfg_path).unwrap(), cfg);
        }
    }
}

#[test]
fn active_workflow_destination_aliases_fail_before_any_writes() {
    for platform in ["github", "forgejo"] {
        for (existing, output) in [
            ("Tests.yml", "tests.yml"),
            ("Straße.yml", "STRASSE.yml"),
            ("É.yml", "e\u{301}.yml"),
        ] {
            for owned in [false, true] {
                let temp = project();
                let cfg_path = temp.path().join("simit.toml");
                let cfg = fs::read_to_string(&cfg_path)
                    .unwrap()
                    .replace("github", platform);
                let cfg = if platform == "forgejo" {
                    cfg.replace("[ci.nix_build]\nonly=true\n", "")
                } else {
                    cfg
                };
                fs::write(&cfg_path, cfg).unwrap();
                let result = generate(&temp, &[]);
                assert!(result.status.success(), "{platform}: {result:?}");
                let directory = format!(".{platform}/workflows");
                fs::remove_file(temp.path().join(&directory).join("tests.yml")).unwrap();
                let active = temp.path().join(&directory).join(existing);
                let content = if owned {
                    "# Simit workflow template: .simit/templates/old.yml\n# Generated by simit. Manual edits will be reported as ci=drift.\nname: Existing project policy\non: [push]\njobs: {}\n"
                } else {
                    "name: Existing project policy\non: [push]\njobs: {}\n"
                };
                fs::write(&active, content).unwrap();
                let cfg = fs::read_to_string(&cfg_path)
                    .unwrap()
                    .replace("workflows/tests.yml", &format!("workflows/{output}"))
                    .replace("ubuntu-24.04", "windows-2022")
                    .replace("nix_builds=['.#default']", "nix_builds=['.#changed']");
                fs::write(&cfg_path, &cfg).unwrap();
                let builtin = temp.path().join(&directory).join("nix-builds.yaml");
                let builtin_before = fs::read(&builtin).unwrap();
                let source = temp.path().join(".simit/templates/tests.yml");
                let source_before = fs::read(&source).unwrap();
                for args in [vec![], vec!["--check", "--diff"]] {
                    let result = generate(&temp, &args);
                    assert!(
                        !result.status.success(),
                        "{platform}/{existing}/{output}/{owned}: {result:?}"
                    );
                    assert!(
                        String::from_utf8_lossy(&result.stderr)
                            .contains("aliases an active Actions workflow")
                    );
                    assert_eq!(fs::read(&active).unwrap(), content.as_bytes());
                    assert_eq!(fs::read(&builtin).unwrap(), builtin_before);
                    assert_eq!(fs::read(&source).unwrap(), source_before);
                    assert_eq!(fs::read_to_string(&cfg_path).unwrap(), cfg);
                    assert_eq!(
                        fs::read_dir(temp.path().join(&directory)).unwrap().count(),
                        if platform == "forgejo" { 3 } else { 2 }
                    );
                }
                fs::remove_file(&active).unwrap();
                assert!(generate(&temp, &[]).status.success());
                assert!(generate(&temp, &["--check", "--diff"]).status.success());
                assert_eq!(
                    audit_ci(temp.path()).unwrap().status,
                    FeatureStatus::Managed
                );
            }
        }
    }
}

#[test]
fn active_workflow_hard_links_are_rejected_but_exact_destinations_remain_declared() {
    for platform in ["github", "forgejo"] {
        let temp = project();
        let cfg_path = temp.path().join("simit.toml");
        let cfg = fs::read_to_string(&cfg_path)
            .unwrap()
            .replace("github", platform);
        let cfg = if platform == "forgejo" {
            cfg.replace("[ci.nix_build]\nonly=true\n", "")
        } else {
            cfg
        };
        fs::write(&cfg_path, &cfg).unwrap();
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{platform}: {result:?}");
        let directory = temp.path().join(format!(".{platform}/workflows"));
        let output = directory.join("tests.yml");
        let active = directory.join("foreign.yml");
        let foreign = b"name: Foreign policy\non: [push]\njobs: {}\n";
        fs::write(&active, foreign).unwrap();
        fs::remove_file(&output).unwrap();
        fs::hard_link(&active, &output).unwrap();
        let builtin = directory.join("nix-builds.yaml");
        let builtin_before = fs::read(&builtin).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{platform}: {result:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("aliases an active Actions workflow")
            );
            assert_eq!(fs::read(&active).unwrap(), foreign);
            assert_eq!(fs::read(&output).unwrap(), foreign);
            assert_eq!(fs::read(&builtin).unwrap(), builtin_before);
            assert_eq!(fs::read_to_string(&cfg_path).unwrap(), cfg);
        }
        fs::remove_file(&output).unwrap();
        // Explicitly declaring the exact path retains existing adoption behavior.
        fs::write(&output, foreign).unwrap();
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{platform}: {result:?}");
        assert_eq!(fs::read(&active).unwrap(), foreign);
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
        let audit = audit_ci(temp.path()).unwrap();
        assert_eq!(audit.status, FeatureStatus::ManagedExtra);
        assert!(audit.changed_files.is_empty());
        assert!(audit.missing_files.is_empty());
        assert!(audit.extra_generated_files.is_empty());
        assert_eq!(fs::read(&active).unwrap(), foreign);
    }
}

#[test]
fn hard_links_to_unrelated_files_are_replaced_without_corrupting_the_other_link() {
    for destination in [
        ".github/workflows/tests.yml",
        ".github/workflows/nix-builds.yaml",
    ] {
        let temp = project();
        assert!(generate(&temp, &[]).status.success());
        let unrelated = temp.path().join("README.md");
        let original = b"Unrelated project documentation\n";
        fs::write(&unrelated, original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let output = temp.path().join(destination);
        fs::remove_file(&output).unwrap();
        fs::hard_link(&unrelated, &output).unwrap();
        let config_path = temp.path().join("simit.toml");
        let config = fs::read_to_string(&config_path)
            .unwrap()
            .replace("ubuntu-24.04", "windows-2022");
        fs::write(&config_path, &config).unwrap();

        assert!(!generate(&temp, &["--check", "--diff"]).status.success());
        assert_eq!(fs::read(&unrelated).unwrap(), original);
        assert_eq!(fs::read(&output).unwrap(), original);
        assert_eq!(fs::read_to_string(&config_path).unwrap(), config);
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{destination}: {result:?}");
        assert_eq!(fs::read(&unrelated).unwrap(), original);
        assert!(!same_file::is_same_file(&output, &unrelated).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o640
            );
            assert_eq!(
                fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
        assert!(yaml["jobs"].is_mapping());
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
        assert_eq!(
            audit_ci(temp.path()).unwrap().status,
            FeatureStatus::Managed
        );
        assert_eq!(fs::read(&unrelated).unwrap(), original);
    }
}

#[test]
fn non_file_destinations_fail_before_rewriting_builtins() {
    let temp = project();
    assert!(generate(&temp, &[]).status.success());
    let builtin = temp.path().join(".github/workflows/nix-builds.yaml");
    let before = fs::read_to_string(&builtin).unwrap();
    let output = temp.path().join(".github/workflows/tests.yml");
    fs::remove_file(&output).unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("foreign"), "foreign file\n").unwrap();
    config(
        &temp,
        ".github/workflows/tests.yml",
        ".simit/templates/tests.yml",
        "windows-2022",
    );
    let cfg_path = temp.path().join("simit.toml");
    let cfg = fs::read_to_string(&cfg_path)
        .unwrap()
        .replace("nix_builds=['.#default']", "nix_builds=['.#changed']");
    fs::write(&cfg_path, &cfg).unwrap();
    for args in [vec![], vec!["--check", "--diff"]] {
        let result = generate(&temp, &args);
        assert!(!result.status.success(), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("non-file destination"));
        assert_eq!(fs::read_to_string(&builtin).unwrap(), before);
        assert_eq!(fs::read_to_string(&cfg_path).unwrap(), cfg);
    }
    assert_eq!(
        fs::read_to_string(output.join("foreign")).unwrap(),
        "foreign file\n"
    );
}

#[test]
fn retired_templates_with_moved_headers_are_removed_but_foreign_markers_survive() {
    for preamble in [
        "\n# Project note\n",
        "--- # project note\n",
        "%YAML 1.2\n--- # project note\n",
        "%TAG !e! tag:example.com,2026:\n---\n",
        "\u{feff}%YAML 1.1\n--- # project note\n",
    ] {
        let temp = project();
        assert!(generate(&temp, &[]).status.success());
        let output = temp.path().join(".github/workflows/tests.yml");
        let original = fs::read_to_string(&output).unwrap();
        let edited = preamble.to_owned() + &original;
        // A BOM is an encoding prefix, not part of the decoded YAML document.
        // The string-input libyaml parser rejects it on Windows; keep it in the
        // actual workflow so generator ownership still exercises that prefix.
        let document = edited.strip_prefix('\u{feff}').unwrap_or(&edited);
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(document).unwrap_or_else(|error| panic!("{preamble:?}: {error}"));
        assert!(yaml["jobs"].is_mapping());
        fs::write(&output, &edited).unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), edited);
        let foreign = temp.path().join(".github/workflows/foreign.yml");
        let foreign_content = "# Simit workflow template: project-note\nname: Foreign\n";
        fs::write(&foreign, foreign_content).unwrap();
        let cfg_path = temp.path().join("simit.toml");
        let cfg = fs::read_to_string(&cfg_path).unwrap();
        fs::write(
            &cfg_path,
            cfg.split("[ci.workflow_templates]").next().unwrap(),
        )
        .unwrap();
        assert!(!generate(&temp, &["--check", "--diff"]).status.success());
        assert!(output.is_file());
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{result:?}");
        assert!(!output.exists());
        assert_eq!(fs::read_to_string(&foreign).unwrap(), foreign_content);
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
        // The preserved project workflow is reported as an unmanaged extra.
        assert_eq!(
            audit_ci(temp.path()).unwrap().status,
            FeatureStatus::ManagedExtra
        );
    }
}

#[test]
fn marker_strings_in_foreign_workflow_payloads_do_not_authorize_retirement() {
    for platform in ["github", "forgejo"] {
        let temp = project();
        let cfg_path = temp.path().join("simit.toml");
        let cfg = fs::read_to_string(&cfg_path)
            .unwrap()
            .replace("github", platform);
        let cfg = if platform == "forgejo" {
            cfg.replace("[ci.nix_build]\nonly=true\n", "")
        } else {
            cfg
        };
        fs::write(&cfg_path, cfg).unwrap();
        assert!(generate(&temp, &[]).status.success());
        let output = temp.path().join(format!(".{platform}/workflows/tests.yml"));
        let generated = fs::read_to_string(&output).unwrap();
        fs::write(&output, "---\n\n# Project note\n".to_owned() + &generated).unwrap();
        let mut foreign = Vec::new();
        for (name, content) in [
            (
                "foreign.yml",
                "name: Foreign\non: [push]\njobs:\n  inspect:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n          # Simit workflow template: project-note\n          # Generated by simit. Manual edits will be reported as ci=drift.\n          echo preserved\n",
            ),
            (
                "ci-foreign.yml",
                "name: Foreign\non: [push]\njobs:\n  inspect:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n          # Simit workflow template: project-note\n          # Generated by simit. Manual edits will be reported as ci=drift.\n          echo preserved\n",
            ),
            (
                "header-foreign.yml",
                "# Simit workflow template: project-note\nname: Foreign\non: [push]\njobs:\n  inspect:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n          # Generated by simit. Manual edits will be reported as ci=drift.\n          echo preserved\n",
            ),
            (
                "quoted-foreign.yml",
                "# A note mentioning # Simit workflow template: source and # Generated by simit. Manual edits will be reported as ci=drift.\nname: Foreign\non: [push]\njobs: {}\n",
            ),
        ] {
            let _: serde_yaml::Value = serde_yaml::from_str(content).unwrap();
            let path = temp.path().join(format!(".{platform}/workflows/{name}"));
            fs::write(&path, content).unwrap();
            foreign.push((path, content));
        }
        let cfg = fs::read_to_string(&cfg_path).unwrap();
        fs::write(
            &cfg_path,
            cfg.split("[ci.workflow_templates]").next().unwrap(),
        )
        .unwrap();
        assert!(!generate(&temp, &["--check", "--diff"]).status.success());
        assert!(output.exists());
        for (path, content) in &foreign {
            assert_eq!(fs::read_to_string(path).unwrap(), *content);
        }
        for _ in 0..2 {
            let result = generate(&temp, &[]);
            assert!(result.status.success(), "{result:?}");
            assert!(!output.exists());
            for (path, content) in &foreign {
                assert_eq!(fs::read_to_string(path).unwrap(), *content);
            }
            assert!(generate(&temp, &["--check", "--diff"]).status.success());
            assert_eq!(
                audit_ci(temp.path()).unwrap().status,
                FeatureStatus::ManagedExtra
            );
        }
    }
}

#[test]
fn case_only_template_renames_preserve_the_generated_output_and_audit() {
    for platform in ["github", "forgejo"] {
        for (old, new) in [
            ("Tests.yml", "tests.yml"),
            ("ci-custom.yaml", "CI-custom.yaml"),
            ("CI-custom.yaml", "ci-custom.yaml"),
            ("Ä.yml", "ä.yml"),
            ("ci-Ä.yaml", "ci-ä.yaml"),
            ("É.yml", "e\u{301}.yml"),
        ] {
            let temp = project();
            let cfg_path = temp.path().join("simit.toml");
            let cfg = fs::read_to_string(&cfg_path)
                .unwrap()
                .replace("github", platform)
                .replace("workflows/tests.yml", &format!("workflows/{old}"));
            let cfg = if platform == "forgejo" {
                cfg.replace("[ci.nix_build]\nonly=true\n", "")
            } else {
                cfg
            };
            fs::write(&cfg_path, &cfg).unwrap();
            let result = generate(&temp, &[]);
            assert!(result.status.success(), "{result:?}");
            let new_name = format!(".{platform}/workflows/{new}");
            let cfg = fs::read_to_string(&cfg_path)
                .unwrap()
                .replace(&format!("workflows/{old}"), &format!("workflows/{new}"))
                .replace("ubuntu-24.04", "windows-2022");
            fs::write(&cfg_path, cfg).unwrap();
            for _ in 0..2 {
                let result = generate(&temp, &[]);
                assert!(result.status.success(), "{result:?}");
                let output = fs::read_to_string(temp.path().join(&new_name)).unwrap();
                let parsed: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();
                assert_eq!(parsed["jobs"]["test"]["runs-on"], "windows-2022");
                assert!(generate(&temp, &["--check", "--diff"]).status.success());
                assert_eq!(
                    audit_ci(temp.path()).unwrap().status,
                    FeatureStatus::Managed
                );
                let template_count =
                    fs::read_dir(temp.path().join(format!(".{platform}/workflows")))
                        .unwrap()
                        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
                        .filter(|content| content.contains("# Simit workflow template: "))
                        .count();
                assert_eq!(
                    template_count, 1,
                    "{platform}: no stale case-only output remains"
                );
            }
        }
    }
}

#[test]
fn registry_audits_reject_configured_template_backend_mismatches_without_writes() {
    for platform in ["github", "forgejo"] {
        let temp = project();
        let cfg_path = temp.path().join("simit.toml");
        let cfg = fs::read_to_string(&cfg_path)
            .unwrap()
            .replace("github", platform)
            .replace("[ci.nix_build]\nonly=true\n", "");
        fs::write(&cfg_path, &cfg).unwrap();
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{platform}: {result:?}");
        assert_eq!(
            audit_ci(temp.path()).unwrap().status,
            FeatureStatus::Managed
        );
        let directory = temp.path().join(format!(".{platform}/workflows"));
        let retained = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let content = fs::read(&path).unwrap();
                (path, content)
            })
            .collect::<Vec<_>>();
        let other = if platform == "github" {
            "forgejo"
        } else {
            "github"
        };
        let invalid = fs::read_to_string(&cfg_path)
            .unwrap()
            .replace(
                &format!("platform = \"{platform}\""),
                &format!("platform = \"{other}\""),
            )
            .replace(
                &format!("platform='{platform}'"),
                &format!("platform='{other}'"),
            );
        fs::write(&cfg_path, &invalid).unwrap();
        let audit = audit_ci(temp.path());
        assert!(audit.is_err(), "{platform}: {audit:?}");
        let error = format!("{:#}", audit.unwrap_err());
        assert!(
            error.contains("selected Actions platform"),
            "{platform}: {error}"
        );
        assert_eq!(
            simit::registry::detect_feature_status(temp.path())["ci"],
            FeatureStatus::Drift
        );
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{platform}: {result:?}");
        }
        for (path, content) in retained {
            assert_eq!(fs::read(path).unwrap(), content);
        }
        assert_eq!(fs::read_to_string(&cfg_path).unwrap(), invalid);
        fs::write(&cfg_path, cfg).unwrap();
        assert!(generate(&temp, &[]).status.success());
        assert!(generate(&temp, &["--check", "--diff"]).status.success());
        assert_eq!(
            audit_ci(temp.path()).unwrap().status,
            FeatureStatus::Managed
        );
    }
}

#[test]
#[cfg(unix)]
fn qualifier_source_hashes_include_dash_and_option_like_tracked_filenames() {
    use sha2::Digest;
    use std::process::Command;

    let temp = TempDir::new().unwrap();
    let fixture = b"source fixture\n";
    fs::write(temp.path().join("--help"), fixture).unwrap();
    let dash_fixture = b"a tracked dash is not standard input\n";
    fs::write(temp.path().join("-"), dash_fixture).unwrap();
    fs::write(temp.path().join("source.rs"), b"fn main() {}\n").unwrap();
    for args in [vec!["init", "--quiet"], vec!["add", "--", "."]] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );
    }
    for path in [
        ".github/workflows/qualify-generator.yaml",
        ".github/workflows/review-compatibility.yml",
    ] {
        let template =
            fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                .unwrap();
        let command = template
            .lines()
            .find(|line| line.contains("git ls-files -z"))
            .unwrap()
            .split(" > ")
            .next()
            .unwrap()
            .trim();
        let output = Command::new("sh")
            .args(["-c", command])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{path}: {output:?}");
        let manifest = String::from_utf8(output.stdout).unwrap();
        assert_eq!(manifest.lines().count(), 3, "{path}: {manifest}");
        for (name, content) in [
            ("--help", fixture.as_slice()),
            ("-", dash_fixture.as_slice()),
        ] {
            let expected = format!("{}  ./{name}", hex::encode(sha2::Sha256::digest(content)));
            assert!(
                manifest.lines().any(|line| line == expected),
                "{path}: {manifest}"
            );
        }
    }
}

#[test]
fn template_retirement_preserves_the_opt_in_review_policy_and_builtin_outputs() {
    let temp = project();
    let policy = "\n[review_policy]\ntoolbelt_version='0.2.0'\napp_id_secret='APP_ID'\napp_private_key_secret='APP_KEY'\ncredential_environment='review-policy'\n";
    let cfg_path = temp.path().join("simit.toml");
    fs::write(&cfg_path, fs::read_to_string(&cfg_path).unwrap() + policy).unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let policy_path = temp.path().join(".github/workflows/review-policy.yaml");
    let builtin_path = temp.path().join(".github/workflows/nix-builds.yaml");
    let policy_output = fs::read_to_string(&policy_path).unwrap();
    let builtin_output = fs::read_to_string(&builtin_path).unwrap();
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );

    let config = fs::read_to_string(&cfg_path).unwrap();
    let without_template = config
        .split("[ci.workflow_templates]")
        .next()
        .unwrap()
        .to_owned()
        + policy;
    fs::write(cfg_path, without_template).unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    assert!(!temp.path().join(".github/workflows/tests.yml").exists());
    assert_eq!(fs::read_to_string(policy_path).unwrap(), policy_output);
    assert_eq!(fs::read_to_string(builtin_path).unwrap(), builtin_output);
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
}

#[test]
fn release_owned_workflows_cannot_be_claimed_by_project_templates() {
    for platform in ["github", "forgejo"] {
        for name in [
            "release.yaml",
            "release.yml",
            "publish-vscode-extension.yaml",
            "publish-jetbrains-plugin.yaml",
        ] {
            let temp = project();
            let output = format!(".{platform}/workflows/{name}");
            config(&temp, &output, ".simit/templates/tests.yml", "ubuntu-24.04");
            let cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
            fs::write(
                temp.path().join("simit.toml"),
                cfg.replace("platform='github'", &format!("platform='{platform}'")),
            )
            .unwrap();
            let path = temp.path().join(&output);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let release = "# Generated by simit init release\nname: Release\n";
            fs::write(&path, release).unwrap();
            let result = generate(&temp, &[]);
            assert!(!result.status.success(), "{result:?}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("release-owned"));
            assert_eq!(fs::read_to_string(&path).unwrap(), release);
            assert!(
                !temp
                    .path()
                    .join(format!(".{platform}/workflows/nix-builds.yaml"))
                    .exists()
            );
        }
    }
}

#[test]
fn project_templates_do_not_infer_builtin_checks_packages_or_runners() {
    for output in ["tests.yml", "ci-other.yaml", "release-artifacts.yaml"] {
        let temp = project();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname='template-inference'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let config = format!(
            "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\n[ci.workflow_templates]\n'.github/workflows/{output}'='.simit/templates/tests.yml'\n"
        );
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        fs::write(temp.path().join(".simit/templates/tests.yml"), "name: Project-only policy\non: [push]\njobs:\n  policy:\n    runs-on: windows-2022\n    steps:\n      - run: cargo deny check bans licenses sources\n").unwrap();
        let result = generate(&temp, &[]);
        assert!(result.status.success(), "{result:?}");
        // Keep optional settings absent so registry inference must use only built-ins.
        fs::write(temp.path().join("simit.toml"), &config).unwrap();
        let built_in = fs::read_to_string(temp.path().join(".github/workflows/ci.yaml")).unwrap();
        assert!(!built_in.contains("cargo deny check"));
        assert!(!built_in.contains("windows-2022"));
        let result = generate(&temp, &["--check", "--diff"]);
        assert!(result.status.success(), "{result:?}");
        let audit = audit_ci(temp.path()).unwrap();
        assert_eq!(audit.status, FeatureStatus::Managed, "{output}: {audit:?}");
    }
}

#[test]
fn builtin_workflows_cannot_be_sources_even_when_the_previous_output_exists() {
    let temp = project();
    assert!(generate(&temp, &[]).status.success());
    let source = ".github/workflows/nix-builds.yaml";
    let previous = fs::read_to_string(temp.path().join(source)).unwrap();
    config(&temp, ".github/workflows/tests.yml", source, "ubuntu-24.04");
    let cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    fs::write(
        temp.path().join("simit.toml"),
        cfg.replace("nix_builds=['.#default']", "nix_builds=['.#changed']"),
    )
    .unwrap();
    let output = generate(&temp, &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("built-in generated output"));
    assert_eq!(
        fs::read_to_string(temp.path().join(source)).unwrap(),
        previous
    );
}

#[test]
fn template_outputs_reject_portable_case_collisions_without_writes() {
    for output in [
        ".github/workflows/NIX-BUILDS.yaml",
        ".github/workflows/RELEASE.yml",
    ] {
        let temp = project();
        config(&temp, output, ".simit/templates/tests.yml", "ubuntu-24.04");
        let before = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{output}: {result:?}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("collides"));
            assert_eq!(
                fs::read_to_string(temp.path().join("simit.toml")).unwrap(),
                before
            );
            assert!(
                !temp
                    .path()
                    .join(".github/workflows/nix-builds.yaml")
                    .exists()
            );
        }
    }
    let temp = project();
    let mut cfg = fs::read_to_string(temp.path().join("simit.toml")).unwrap();
    cfg = cfg.replace(
        "[ci.workflow_variables]",
        "'.github/workflows/Tests.yml'='.simit/templates/tests.yml'\n[ci.workflow_variables]",
    );
    fs::write(temp.path().join("simit.toml"), cfg).unwrap();
    let result = generate(&temp, &[]);
    assert!(!result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stderr).contains("case-insensitive"));
}

#[test]
fn unicode_case_and_normalization_collisions_are_rejected_before_any_writes() {
    for (first, second) in [
        ("Ä.yml", "ä.yml"),
        ("Straße.yml", "STRASSE.yml"),
        ("Σ.yml", "ς.yml"),
        ("É.yml", "e\u{301}.yml"),
    ] {
        let temp = project();
        let config_path = temp.path().join("simit.toml");
        let config = fs::read_to_string(&config_path)
            .unwrap()
            .replace("workflows/tests.yml", &format!("workflows/{first}"))
            .replace(
                "[ci.workflow_variables]",
                &format!("'.github/workflows/{second}'='.simit/templates/tests.yml'\n[ci.workflow_variables]"),
            );
        fs::write(&config_path, &config).unwrap();
        for args in [vec![], vec!["--check", "--diff"]] {
            let result = generate(&temp, &args);
            assert!(!result.status.success(), "{first}/{second}: {result:?}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("case-insensitive"));
            assert_eq!(fs::read_to_string(&config_path).unwrap(), config);
            assert!(!temp.path().join(".github/workflows").exists());
        }
    }
}

#[test]
#[cfg(unix)]
fn symlink_targets_are_not_treated_as_generated_output_case_aliases() {
    use std::os::unix::fs::symlink;

    let temp = project();
    assert!(generate(&temp, &[]).status.success());
    let expected = temp.path().join(".github/workflows/nix-builds.yaml");
    let obsolete = temp.path().join(".github/workflows/nix-builds.yml");
    let config_path = temp.path().join("simit.toml");
    let config_before = fs::read(&config_path).unwrap();
    let template = temp.path().join(".github/workflows/tests.yml");
    let template_before = fs::read(&template).unwrap();
    let generated_before = fs::read(&expected).unwrap();
    fs::rename(&expected, &obsolete).unwrap();
    symlink("nix-builds.yml", &expected).unwrap();

    let checked = generate(&temp, &["--check", "--diff"]);
    assert!(
        !checked.status.success(),
        "a symlink cannot hide an obsolete generated target: {checked:?}"
    );
    assert_eq!(audit_ci(temp.path()).unwrap().status, FeatureStatus::Drift);
    let written = generate(&temp, &[]);
    assert!(!written.status.success(), "{written:?}");
    assert!(String::from_utf8_lossy(&written.stderr).contains("symlink"));
    assert_eq!(fs::read(&obsolete).unwrap(), generated_before);
    assert_eq!(fs::read(&template).unwrap(), template_before);
    assert_eq!(fs::read(&config_path).unwrap(), config_before);
    assert_eq!(
        fs::read_link(&expected).unwrap(),
        Path::new("nix-builds.yml")
    );

    fs::remove_file(&expected).unwrap();
    fs::rename(&obsolete, &expected).unwrap();
    assert!(generate(&temp, &["--check", "--diff"]).status.success());
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
}

#[test]
fn edited_template_headers_do_not_infer_or_persist_builtin_gates() {
    let temp = project();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='edited-template'\nversion='0.1.0'\nedition='2024'\n",
    )
    .unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\n[ci.workflow_templates]\n'.github/workflows/ci-other.yaml'='.simit/templates/tests.yml'\n").unwrap();
    fs::write(temp.path().join(".simit/templates/tests.yml"), "name: Custom\non: [push]\njobs:\n  project:\n    runs-on: windows-2022\n    steps:\n      - run: cargo deny check\n").unwrap();
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    let cfg_path = temp.path().join("simit.toml");
    let baseline = fs::read_to_string(&cfg_path).unwrap();
    // Leave the option omitted so a contaminated inference would persist true.
    fs::write(
        &cfg_path,
        baseline
            .lines()
            .filter(|line| !line.starts_with("with_deny = "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let output = temp.path().join(".github/workflows/ci-other.yaml");
    let original = fs::read_to_string(&output).unwrap();
    fs::write(
        &output,
        "\n# Project note before the generated header\n".to_owned()
            + &original.replace("# Simit workflow template:", "# Edited template header:"),
    )
    .unwrap();
    assert_eq!(audit_ci(temp.path()).unwrap().status, FeatureStatus::Drift);
    let result = generate(&temp, &[]);
    assert!(result.status.success(), "{result:?}");
    assert!(
        !fs::read_to_string(temp.path().join(".github/workflows/ci.yaml"))
            .unwrap()
            .contains("cargo deny check")
    );
    assert!(
        !simit::config::ProjectConfig::load(temp.path())
            .unwrap()
            .ci
            .with_deny
    );
    assert_eq!(fs::read_to_string(output).unwrap(), original);
    let result = generate(&temp, &["--check", "--diff"]);
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        audit_ci(temp.path()).unwrap().status,
        FeatureStatus::Managed
    );
}

#[test]
fn inactive_platform_generated_sources_are_rejected_before_reconciliation() {
    let temp = project();
    let source = ".forgejo/workflows/ci.yaml";
    let previous =
        "# Generated by simit. Manual edits will be reported as ci=drift.\nname: CI\non: [push]\n";
    fs::create_dir_all(temp.path().join(".forgejo/workflows")).unwrap();
    fs::write(temp.path().join(source), previous).unwrap();
    config(&temp, ".github/workflows/tests.yml", source, "ubuntu-24.04");
    for args in [vec![], vec!["--check", "--diff"]] {
        let output = generate(&temp, &args);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("built-in generated output"));
        assert_eq!(
            fs::read_to_string(temp.path().join(source)).unwrap(),
            previous
        );
        assert!(
            !temp
                .path()
                .join(".github/workflows/nix-builds.yaml")
                .exists()
        );
    }
}

#[test]
fn cli_gitlab_template_mismatch_rejects_both_check_and_write() {
    let temp = project();
    // Establish a matching built-in GitLab file before adding the Actions mapping.
    fs::write(temp.path().join("simit.toml"), "[ci]\nruntime='nix'\n").unwrap();
    let output = generate(&temp, &["--platform", "gitlab"]);
    assert!(output.status.success(), "{output:?}");
    let before = fs::read_to_string(temp.path().join(".gitlab-ci.yml")).unwrap();
    fs::write(temp.path().join("simit.toml"), "[ci]\nruntime='nix'\n[ci.workflow_templates]\n'.github/workflows/tests.yml'='.simit/templates/tests.yml'\n").unwrap();
    for args in [
        vec!["--platform", "gitlab"],
        vec!["--platform", "gitlab", "--check", "--diff"],
    ] {
        let output = generate(&temp, &args);
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("requires GitHub or Forgejo Actions")
        );
        assert_eq!(
            fs::read_to_string(temp.path().join(".gitlab-ci.yml")).unwrap(),
            before
        );
    }
}

#[test]
fn missing_builtins_report_drift_when_only_template_outputs_remain() {
    for rust in [false, true] {
        let temp = project();
        let builtin = if rust {
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname='missing-builtins'\nversion='0.1.0'\nedition='2024'\n",
            )
            .unwrap();
            fs::create_dir(temp.path().join("src")).unwrap();
            fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
            fs::write(temp.path().join("simit.toml"), "[ci]\nplatform='github'\nprovider='actions'\nruntime='cargo'\nrunner='ubuntu-24.04'\n[ci.workflow_templates]\n'.github/workflows/tests.yml'='.simit/templates/tests.yml'\n[ci.workflow_variables]\nrunner='windows-2022'\n").unwrap();
            ".github/workflows/ci.yaml"
        } else {
            ".github/workflows/nix-builds.yaml"
        };
        let output = generate(&temp, &[]);
        assert!(output.status.success(), "{output:?}");
        fs::remove_file(temp.path().join(builtin)).unwrap();
        let audit = audit_ci(temp.path()).unwrap();
        assert_eq!(audit.status, FeatureStatus::Drift, "{audit:?}");
        let output = generate(&temp, &["--check", "--diff"]);
        assert!(!output.status.success(), "{output:?}");
        let output = generate(&temp, &[]);
        assert!(output.status.success(), "{output:?}");
        assert!(temp.path().join(builtin).is_file());
        assert_eq!(
            audit_ci(temp.path()).unwrap().status,
            FeatureStatus::Managed
        );
    }
}

#[test]
fn project_audit_emits_an_executable_repair_when_only_templates_remain() {
    for rust in [false, true] {
        for platform in ["github", "forgejo"] {
            for configured_backend in [false, true] {
                let temp = project();
                let cfg_path = temp.path().join("simit.toml");
                let cfg = if rust {
                    fs::write(temp.path().join("Cargo.toml"), "[package]\nname='audit-template-repair'\nversion='0.1.0'\nedition='2024'\n").unwrap();
                    fs::create_dir(temp.path().join("src")).unwrap();
                    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
                    format!(
                        "[ci]\nplatform='{platform}'\nprovider='actions'\nruntime='cargo'\nrunner='ubuntu-24.04'\n[ci.workflow_templates]\n'.{platform}/workflows/tests.yml'='.simit/templates/tests.yml'\n[ci.workflow_variables]\nrunner='windows-2022'\n"
                    )
                } else {
                    let cfg = fs::read_to_string(&cfg_path)
                        .unwrap()
                        .replace("github", platform);
                    if platform == "forgejo" {
                        cfg.replace("[ci.nix_build]\nonly=true\n", "")
                    } else {
                        cfg
                    }
                };
                fs::write(&cfg_path, cfg).unwrap();
                assert!(generate(&temp, &[]).status.success());
                // A template body must never infer the builtin runner or gates.
                let template = temp.path().join(format!(".{platform}/workflows/tests.yml"));
                let original = fs::read_to_string(&template).unwrap();
                for entry in fs::read_dir(template.parent().unwrap()).unwrap() {
                    let path = entry.unwrap().path();
                    if path != template {
                        fs::remove_file(path).unwrap();
                    }
                }
                if !configured_backend {
                    let mut cfg: toml_edit::DocumentMut =
                        fs::read_to_string(&cfg_path).unwrap().parse().unwrap();
                    cfg["ci"].as_table_mut().unwrap().remove("platform");
                    cfg["ci"].as_table_mut().unwrap().remove("provider");
                    fs::write(&cfg_path, cfg.to_string()).unwrap();
                }
                let report = common::simit()
                    .current_dir(temp.path())
                    .args(["projects", "audit", ".", "--json"])
                    .output()
                    .unwrap();
                assert_eq!(report.status.code(), Some(1), "{report:?}");
                let report: serde_json::Value = serde_json::from_slice(&report.stdout)
                    .unwrap_or_else(|error| panic!("{report:?}: {error}"));
                let project = &report["projects"][0];
                assert_eq!(project["ciStatus"], "drift", "{project:?}");
                assert!(
                    project["errors"].as_array().unwrap().is_empty(),
                    "{project:?}"
                );
                assert!(
                    !project["missingFiles"].as_array().unwrap().is_empty(),
                    "{project:?}"
                );
                let command = project["regenerateCommand"]
                    .as_str()
                    .expect("template-only drift needs a repair command");
                assert!(
                    command.contains(&format!("--platform {platform}")),
                    "{command}"
                );
                assert!(!command.contains("windows-2022"), "{command}");
                let output = common::simit()
                    .current_dir(temp.path())
                    .args(command.split_ascii_whitespace().skip(1))
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{command}: {output:?}");
                assert_eq!(fs::read_to_string(&template).unwrap(), original);
                assert!(generate(&temp, &["--check", "--diff"]).status.success());
                assert_eq!(
                    audit_ci(temp.path()).unwrap().status,
                    FeatureStatus::Managed
                );
            }
        }
    }
}
