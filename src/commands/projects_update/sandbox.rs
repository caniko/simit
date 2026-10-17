//! Candidate configuration can execute tools: expose no updater authority to it.
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command};

pub fn generation(executable: &Path, checkout: &Path, nix_state: &Path) -> Result<Command> {
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
    let flake = checkout.join("flake.nix");
    if flake.is_file() && crate::config::flake_declares_simit_config(&flake)? {
        // SQLite's online snapshot includes committed WAL content without
        // exposing the live database, daemon socket or host fetcher cache.
        let source_state = std::env::var_os("NIX_STATE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/nix/var/nix".into());
        let destination = nix_state.join("db/db.sqlite");
        std::fs::create_dir_all(destination.parent().context("missing database parent")?)?;
        let mut snapshot = Command::new(tool("sqlite3")?);
        snapshot
            .env_clear()
            .stdin(std::process::Stdio::null())
            .arg("-readonly")
            .arg(source_state.join("db/db.sqlite"))
            .arg(format!(
                "VACUUM INTO '{}';",
                destination
                    .to_str()
                    .context("non-UTF-8 snapshot path")?
                    .replace('\'', "''")
            ));
        super::engine::run_tool(snapshot, 120).context("snapshot offline Nix registrations")?;
    }
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
            "--overlay-src",
            "/nix/store",
            "--tmp-overlay",
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
    command.arg("--bind").arg(nix_state).arg("/nix/var/nix");
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
        // Registrations are a consistent private snapshot. Store writes go to
        // Bubblewrap's disposable overlay, never the host store or its daemon.
        ("NIX_REMOTE", "local"),
        ("NIX_STATE_DIR", "/nix/var/nix"),
        ("NIX_LOG_DIR", "/tmp/nix-log"),
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
