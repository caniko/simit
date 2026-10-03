#![cfg(unix)]

use serde_json::json;
use simit::review::{contract::*, read_json, write_json};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fork_heads_resolve_and_checkout_from_the_fork_while_merge_and_revision_use_base() {
    let controller = "c".repeat(40);
    let head = "a".repeat(40);
    let base = "b".repeat(40);
    let merge = "d".repeat(40);
    let tree = "e".repeat(40);
    for mode in ["head", "merge", "revision"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let tools = root.join("tools");
        fs::create_dir(&tools).unwrap();
        let commit = match mode {
            "head" => &head,
            "merge" => &merge,
            _ => &base,
        };
        let source = if mode == "head" {
            "contributor/project"
        } else {
            "OWNER/REPOSITORY"
        };
        let mut request = example();
        request.mode = if mode == "merge" {
            Mode::Merge
        } else {
            Mode::Head
        };
        request.expected_head = Some(if mode == "revision" {
            base.clone()
        } else {
            head.clone()
        });
        request.expected_base = if mode == "revision" {
            None
        } else {
            Some(base.clone())
        };
        if mode == "revision" {
            request.pr = None;
            request.revision = Some(base.clone());
        }
        write_json(&root.join("request.json"), &request).unwrap();
        let pr = json!({"number":1,"head":{"sha":head,"repo":{"full_name":"contributor/project"}},"base":{"sha":base},"mergeable":true,"merged":false,"merge_commit_sha":merge});
        let metadata = |sha: &str| json!({"sha":sha,"commit":{"tree":{"sha":tree}},"parents":[{"sha":base},{"sha":head}]});
        let lock = json!({"version":7,"root":"root","nodes":{"root":{"inputs":{"simit":"simit","nixpkgs":"nixpkgs"}},"simit":{"locked":{"type":"github","owner":"caniko","repo":"simit","rev":controller}},"nixpkgs":{"locked":{"type":"github","owner":"NixOS","repo":"nixpkgs","rev":base}}}});
        write_json(&root.join("flake.lock"), &lock).unwrap();
        write_json(&root.join("engine.json"), &json!({"schema_version":1,"repository":"caniko/simit","revision":controller,"nixpkgs_revision":base})).unwrap();
        for (name, script) in [
            ("gh", format!(r#"#!/bin/sh
case "$4" in
repos/caniko/controller) printf '%s' '{{"id":3,"full_name":"caniko/controller"}}';;
repos/caniko/controller/commits/{controller}) printf '%s' '{controller_metadata}';;
repos/OWNER/REPOSITORY) printf '%s' '{{"id":1,"full_name":"OWNER/REPOSITORY"}}';;
repos/OWNER/REPOSITORY/commits/HEAD|repos/OWNER/REPOSITORY/commits/{base}) printf '%s' '{base_metadata}';;
repos/OWNER/REPOSITORY/pulls/1) printf '%s' '{pr}';;
repos/OWNER/REPOSITORY/commits/{merge}) printf '%s' '{merge_metadata}';;
repos/contributor/project) printf '%s' '{{"id":2,"full_name":"contributor/project"}}';;
repos/contributor/project/commits/{head}) printf '%s' '{head_metadata}';;
*) printf 'unexpected API endpoint: %s' "$4" >&2; exit 1;;
esac
"#, controller_metadata=metadata(&controller), base_metadata=metadata(&base), merge_metadata=metadata(&merge), head_metadata=metadata(&head))),
            ("git", format!(r#"#!/bin/sh
case "$1" in
init) printf '{{}}' > flake.nix;;
rev-parse)
  if [ "$PWD" = '{root}' ]; then printf '%s' '{controller}';
  elif [ "$2" = 'HEAD^{{tree}}' ]; then printf '%s' '{tree}';
  else printf '%s' '{commit}'; fi;;
-c)
  if [ "$3" = fetch ]; then
    printf '%s %s\n' "$6" "$7" >> "$FETCH_LOG"
    [ "$6" = 'https://github.com/{source}.git' ] && [ "$7" = '{commit}' ] || exit 1
  fi;;
*) exit 1;;
esac
"#, root=root.display())),
            ("uname", "#!/bin/sh\nprintf x86_64".into()),
            ("nix", "#!/bin/sh\ncase \"$1 $2 $3\" in\n'config show system') printf x86_64-linux;;\n'config show sandbox') printf true;;\n--version*) printf 'Nix fixture';;\n'flake metadata --json') printf 'checkout verified' > \"$EVAL_LOG\"; exit 1;;\n*) exit 1;;\nesac\n".into()),
        ] {
            let path = tools.join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let run = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_repo-review"))
                .args(args)
                .current_dir(root)
                .env("PATH", &tools)
                .env("FETCH_LOG", root.join("fetch.log"))
                .env("EVAL_LOG", root.join("evaluation.log"))
                .env("SIMIT_REVIEW_ENGINE_MANIFEST", root.join("engine.json"))
                .env("GITHUB_REPOSITORY", "caniko/controller")
                .env("GITHUB_RUN_ID", "1")
                .env("GITHUB_RUN_ATTEMPT", "1")
                .output()
                .unwrap()
        };
        let result = run(&[
            "plan",
            "request.json",
            "--controller",
            "caniko/controller",
            "--revision",
            &controller,
            "--output",
            "plan.json",
        ]);
        assert!(
            result.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let plan: Plan = read_json(&root.join("plan.json")).unwrap();
        plan.validate().unwrap();
        assert_eq!(plan.request.repository, "OWNER/REPOSITORY");
        assert_eq!(plan.target.repository, "OWNER/REPOSITORY");
        assert_eq!(plan.target.id, 1);
        assert_eq!(&plan.target.commit, commit);
        assert_eq!(plan.target.tree, tree);
        let result = run(&[
            "build",
            "--plan",
            "plan.json",
            "--system",
            "x86_64-linux",
            "--output",
            "bundle",
        ]);
        assert!(!result.status.success());
        let report: PlatformResult = read_json(&root.join("bundle/review-result.json")).unwrap();
        assert!(
            report
                .error
                .as_deref()
                .unwrap_or("")
                .starts_with("nix failed"),
            "{mode}: {:?}",
            report.error
        );
        assert_eq!(
            fs::read_to_string(root.join("evaluation.log")).unwrap(),
            "checkout verified"
        );
        assert_eq!(
            fs::read_to_string(root.join("fetch.log")).unwrap(),
            format!("https://github.com/{source}.git {commit}\n")
        );
    }
}
