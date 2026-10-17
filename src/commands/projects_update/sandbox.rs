//! Candidate configuration can execute tools: expose no updater authority to it.
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command};

pub fn generation(executable: &Path, checkout: &Path) -> Result<Command> {
    ensure!(
        cfg!(target_os = "linux"),
        "updater generation requires Linux namespaces"
    );
    // Only operator-provisioned immutable tools enter the child PATH. In
    // particular, never expose system/profile directories or a user home.
    let directories = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter_map(|path| path.canonicalize().ok())
        .filter(|path| path.starts_with("/nix/store") && path.is_dir())
        .collect::<Vec<_>>();
    let tool = |name: &str| -> Result<std::path::PathBuf> {
        directories
            .iter()
            .find_map(|directory| {
                let path = directory.join(name).canonicalize().ok()?;
                (path.starts_with("/nix/store") && path.is_file()).then_some(path)
            })
            .with_context(|| format!("generation requires an immutable Nix-store {name} on PATH"))
    };
    let path = std::env::join_paths(&directories)?;
    let mut command = Command::new(tool("bwrap")?);
    command
        .env_clear()
        .stdin(std::process::Stdio::null())
        .args([
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--as-pid-1",
            "--cap-drop",
            "ALL",
            "--disable-userns",
            "--ro-bind",
            "/nix/store",
            "/nix/store",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/tmp/home",
            "--clearenv",
        ]);
    command.args(["--symlink"]).arg(tool("sh")?).arg("/bin/sh");
    command.arg("--ro-bind").arg(checkout).arg("/source");
    command.arg("--ro-bind").arg(executable).arg("/generator");
    for (key, value) in [
        ("PATH", path.to_str().context("non-UTF-8 tool PATH")?),
        ("HOME", "/tmp/home"),
        ("USER", "nobody"),
        ("LOGNAME", "nobody"),
        ("TMPDIR", "/tmp"),
        ("XDG_CACHE_HOME", "/tmp/cache"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("PYTHONNOUSERSITE", "1"),
        // A private local store has no daemon socket, host DB, fetcher cache or
        // credentials. Missing inputs fail closed in the network namespace.
        (
            "NIX_REMOTE",
            "local?real=/tmp/nix-store&state=/tmp/nix-state&log=/tmp/nix-log",
        ),
        ("NIX_CONF_DIR", "/tmp/nix-conf"),
        ("NIX_USER_CONF_FILES", "/dev/null"),
        (
            "NIX_CONFIG",
            "experimental-features = nix-command flakes\naccept-flake-config = false\nallow-import-from-derivation = false\nsubstituters =\nbuilders =\nmax-jobs = 0\nflake-registry =\n",
        ),
    ] {
        command.args(["--setenv", key, value]);
    }
    // Making the generator PID 1 ties every descendant's lifetime to its exit;
    // the kernel kills and reaps that namespace before Bubblewrap completes.
    command.args([
        "--chdir",
        "/source",
        "--",
        "/generator",
        "init",
        "ci",
        "--primary-only",
        "--plan-json",
    ]);
    Ok(command)
}
