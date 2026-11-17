//! Crow CI for flake-only projects driven by `[ci].nix_builds`.

use std::fs;

use super::simit;

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
