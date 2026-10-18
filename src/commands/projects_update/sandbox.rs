//! Filesystem and process boundaries for discovery, adapters and generation.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

fn directories() -> Vec<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter_map(|path| path.canonicalize().ok())
        .filter(|path| path.starts_with("/nix/store") && path.is_dir())
        .collect()
}

pub(super) fn tool(name: &str) -> Result<PathBuf> {
    directories()
        .iter()
        .find_map(|directory| {
            let path = directory.join(name);
            let resolved = path.canonicalize().ok()?;
            (resolved.starts_with("/nix/store") && resolved.is_file()).then_some(path)
        })
        .with_context(|| format!("sandbox requires an immutable Nix-store {name} on PATH"))
}

pub(super) fn adapter(
    executable: &super::schema::Command,
    inputs: &BTreeMap<PathBuf, PathBuf>,
    request: &Path,
) -> Result<Command> {
    ensure!(
        cfg!(target_os = "linux"),
        "updater adapters require Linux namespaces"
    );
    executable.validate()?;
    let path = std::env::join_paths(directories())?;
    let mut command = Command::new(tool("bwrap")?);
    command
        .env_clear()
        .stdin(std::process::Stdio::null())
        .args([
            "--unshare-all",
            "--share-net",
            "--unshare-user",
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
    command.arg("--symlink").arg(tool("sh")?).arg("/bin/sh");
    // Only network plumbing is inherited from the host. No host home, source,
    // updater state, daemon sockets or ambient executable directories are bound.
    for file in ["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf"] {
        if Path::new(file).is_file() {
            command.args(["--ro-bind", file, file]);
        }
    }
    if let Some(bundle) = option_env!("SIMIT_TRUSTED_CA_BUNDLE")
        .map(PathBuf::from)
        .or_else(|| {
            Path::new("/etc/ssl/certs/ca-certificates.crt")
                .canonicalize()
                .ok()
        })
        .filter(|path| path.starts_with("/nix/store") && path.is_file())
    {
        command.args(["--setenv", "SSL_CERT_FILE"]).arg(&bundle);
        command
            .arg("--ro-bind")
            .arg(bundle)
            .arg("/etc/ssl/certs/ca-certificates.crt");
    }
    for key in ["SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE"] {
        if let Some(source) = std::env::var_os(key)
            && let Ok(source) = Path::new(&source).canonicalize()
            && source.starts_with("/nix/store")
        {
            command.args(["--setenv", key]).arg(source);
        }
    }
    for (destination, source) in inputs {
        command.arg("--ro-bind").arg(source).arg(destination);
    }
    command
        .arg("--ro-bind")
        .arg(request)
        .arg("/simit/request.json");
    for (key, value) in [
        ("PATH", path.to_str().context("non-UTF-8 adapter PATH")?),
        ("HOME", "/tmp/home"),
        ("TMPDIR", "/tmp"),
        ("LANG", "C.UTF-8"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("PYTHONNOUSERSITE", "1"),
    ] {
        command.args(["--setenv", key, value]);
    }
    command
        .args(["--chdir", "/tmp/home", "--"])
        .arg(&executable.program)
        .args(&executable.arguments)
        .args(["--request", "/simit/request.json"]);
    Ok(command)
}

pub fn inventory(command: &super::schema::Command) -> Result<Vec<u8>> {
    ensure!(
        cfg!(target_os = "linux"),
        "updater discovery requires Linux namespaces"
    );
    let temporary = tempfile::Builder::new()
        .prefix("simit-inventory-")
        .tempdir()?;
    let temporary_path = temporary
        .path()
        .to_str()
        .context("non-UTF-8 inventory temporary path")?;
    let mut arguments = [
        "--unshare-all",
        "--unshare-user",
        "--die-with-parent",
        "--new-session",
        "--as-pid-1",
        "--cap-drop",
        "ALL",
        "--disable-userns",
        "--ro-bind",
        "/",
        "/",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/run",
    ]
    .map(str::to_owned)
    .to_vec();
    if Path::new("/run/current-system").is_dir() {
        arguments
            .extend(["--ro-bind", "/run/current-system", "/run/current-system"].map(str::to_owned));
    }
    if Path::new("/nix/var/nix/daemon-socket").is_dir() {
        arguments.extend(["--tmpfs", "/nix/var/nix/daemon-socket"].map(str::to_owned));
    }
    arguments.extend(
        [
            "--tmpfs",
            temporary_path,
            "--setenv",
            "TMPDIR",
            temporary_path,
            "--",
        ]
        .map(str::to_owned),
    );
    arguments.push(
        command
            .program
            .to_str()
            .context("non-UTF-8 inventory executable")?
            .into(),
    );
    arguments.extend(command.arguments.iter().cloned());
    super::engine::command_output(
        &super::schema::Command {
            program: tool("bwrap")?,
            arguments,
            timeout_seconds: command.timeout_seconds,
        },
        &[],
        None,
    )
}

fn snapshot_registrations(source_state: &Path, nix_state: &Path) -> Result<()> {
    let destination = nix_state.join("db/db.sqlite");
    std::fs::create_dir_all(destination.parent().context("missing database parent")?)?;
    let schema_path = source_state.join("db/schema");
    let schema = std::fs::read(&schema_path)?;
    let mut snapshot = Command::new(tool("sqlite3")?);
    snapshot
        .env_clear()
        .stdin(std::process::Stdio::null())
        .args(["-cmd", ".timeout 10000"])
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
    ensure!(
        std::fs::read(schema_path)? == schema,
        "Nix store schema changed during snapshot"
    );
    std::fs::write(nix_state.join("db/schema"), schema)?;
    Ok(())
}

pub fn generation(executable: &Path, checkout: &Path, nix_state: &Path) -> Result<Command> {
    ensure!(
        cfg!(target_os = "linux"),
        "updater generation requires Linux namespaces"
    );
    // Only operator-provisioned immutable tools enter the child PATH. In
    // particular, never expose system/profile directories or a user home.
    let directories = directories();
    let flake = checkout.join("flake.nix");
    if flake.is_file() && crate::config::flake_declares_simit_config(&flake)? {
        // SQLite's online snapshot includes committed WAL content without
        // exposing the live database, daemon socket or host fetcher cache.
        let source_state = std::env::var_os("NIX_STATE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/nix/var/nix".into());
        snapshot_registrations(&source_state, nix_state)?;
    }
    let path = std::env::join_paths(&directories)?;
    let mut command = Command::new(tool("bwrap")?);
    command
        .env_clear()
        .stdin(std::process::Stdio::null())
        .args([
            "--unshare-all",
            "--unshare-user",
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
    // Store wrappers and $ORIGIN loaders must retain their installation path.
    // Mutable, digest-enrolled scripts have already been privately snapshotted.
    let program = if executable.starts_with("/nix/store") {
        executable
    } else {
        command.arg("--ro-bind").arg(executable).arg("/generator");
        Path::new("/generator")
    };
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
    command
        .args(["--chdir", "/source", "--"])
        .arg(program)
        .args(["init", "ci", "--primary-only", "--plan-json"]);
    Ok(command)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        process::Stdio,
        time::Duration,
    };

    #[test]
    fn registration_snapshot_waits_for_a_writer_and_preserves_committed_data() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("snapshot");
        std::fs::create_dir_all(source.join("db")).unwrap();
        std::fs::write(source.join("db/schema"), "10").unwrap();
        let sqlite = tool("sqlite3").unwrap();
        let mut writer = Command::new(&sqlite)
            .env_clear()
            .arg("-batch")
            .arg(source.join("db/db.sqlite"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = writer.stdin.take().unwrap();
        input.write_all(b"CREATE TABLE registrations(value); BEGIN EXCLUSIVE; INSERT INTO registrations VALUES ('committed');\n.print locked\n").unwrap();
        let mut ready = String::new();
        BufReader::new(writer.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "locked\n");
        std::thread::scope(|scope| {
            let release = scope.spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                input.write_all(b"COMMIT;\n").unwrap();
                drop(input);
                assert!(writer.wait().unwrap().success());
            });
            let result = snapshot_registrations(&source, &destination);
            release.join().unwrap();
            result.unwrap();
        });
        let snapshot = Command::new(sqlite)
            .env_clear()
            .arg("-readonly")
            .arg(destination.join("db/db.sqlite"))
            .arg("SELECT value FROM registrations;")
            .output()
            .unwrap();
        assert!(snapshot.status.success(), "{snapshot:?}");
        assert_eq!(snapshot.stdout, b"committed\n");
        assert_eq!(std::fs::read(destination.join("db/schema")).unwrap(), b"10");
    }
}
