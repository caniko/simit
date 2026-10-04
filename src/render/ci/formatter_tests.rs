use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use tempfile::TempDir;

use super::NIX_FORMAT_COMMAND;

fn executable(path: &std::path::Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn managed_formatter_failures_never_fall_back_to_cargo_or_ambient_treefmt() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bin = root.join("bin");
    let wrapper = root.join("wrapper");
    fs::create_dir(&bin).unwrap();
    fs::create_dir_all(wrapper.join("bin")).unwrap();
    fs::create_dir(root.join("nix")).unwrap();
    fs::write(root.join("nix/treefmt.nix"), "{}").unwrap();
    executable(
        &bin.join("nix"),
        "#!/bin/sh\ncase $1 in eval) test \"$MODE\" != eval-failed || exit 7; test \"$MODE\" = missing || echo /fixture.drv;; build) echo \"$WRAPPER\";; esac\n",
    );
    executable(&bin.join("cargo"), "#!/bin/sh\necho cargo-fallback\n");
    executable(&bin.join("treefmt"), "#!/bin/sh\necho ambient-treefmt\n");
    let run = |mode: &str| {
        Command::new("sh")
            .current_dir(root)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("MODE", mode)
            .env("WRAPPER", &wrapper)
            .args(["-c", NIX_FORMAT_COMMAND])
            .output()
            .unwrap()
    };
    for (mode, expected) in [("missing", 1), ("eval-failed", 7), ("no-executable", 1)] {
        let output = run(mode);
        assert_eq!(output.status.code(), Some(expected));
        assert!(output.stdout.is_empty());
    }
    executable(
        &wrapper.join("bin/treefmt"),
        "#!/bin/sh\ntest \"$1\" = --ci || exit 99\nexit 42\n",
    );
    assert_eq!(run("configured").status.code(), Some(42));
    executable(
        &wrapper.join("bin/treefmt"),
        "#!/bin/sh\necho project-treefmt\n",
    );
    let output = run("configured");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"project-treefmt\n");

    fs::remove_file(root.join("nix/treefmt.nix")).unwrap();
    let output = run("missing");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"cargo-fallback\n");
}
