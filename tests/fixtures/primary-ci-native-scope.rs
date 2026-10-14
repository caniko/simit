//! Copy to tests/primary_ci_native_scope.rs in the pinned #48 integration tree.
use std::fs;
mod common;

#[test]
fn primary_only_preserves_every_native_publisher_and_helper() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    let policy = "[ci]\nplatform = 'github'\nprovider = 'actions'\nruntime = 'nix'\n[monorepo]\nschema_version = 1\n[[monorepo.components]]\nid = 'python'\npaths = ['python']\nchecks = [{id = 'test', run = 'true'}]\nreleases = [{manifest = 'python/pyproject.toml', namespace = 'python-owner', publish = true}]\n[[monorepo.components]]\nid = 'node'\npaths = ['node']\nchecks = [{id = 'test', run = 'true'}]\nreleases = [{manifest = 'node/package.json', namespace = 'node-owner', publish = true}]\n";
    fs::write(root.join("simit.toml"), policy).unwrap();
    fs::create_dir(root.join("python")).unwrap();
    fs::create_dir(root.join("node")).unwrap();
    fs::write(
        root.join("python/pyproject.toml"),
        "[project]\nname = 'python-owner'\nversion = '1.2.3'\n",
    )
    .unwrap();
    fs::write(
        root.join("node/package.json"),
        r#"{"name":"@example/node-owner","version":"1.2.3"}"#,
    )
    .unwrap();
    let paths = [
        ".github/workflows/publish-python-python-owner.yaml",
        ".github/workflows/trusted-publish-python-python-owner.yaml",
        ".github/workflows/publish-npm-node-owner.yaml",
        ".github/workflows/trusted-publish-npm-node-owner.yaml",
        ".github/scripts/simit-native-release.py",
    ];
    let retained = "# retained publication enrollment with deliberate drift\n";
    for path in paths {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, retained).unwrap();
    }
    for tail in [&[][..], &["--check", "--diff"][..]] {
        let output = common::simit()
            .env("SIMIT_NO_REGISTRY", "1")
            .current_dir(root)
            .args(["init", "ci", "--primary-only"])
            .args(tail)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(root.join(".github/workflows/ci.yaml").is_file());
        assert_eq!(fs::read_to_string(root.join("simit.toml")).unwrap(), policy);
        assert!(!root.join("keys").exists());
        for path in paths {
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), retained);
        }
    }
}
