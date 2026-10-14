//! Execute the same project-owned gate locally and in generated qualification.
use std::{
    fs,
    process::{Child, Command},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::cli::{MonorepoCheckCommand, MonorepoGateEnv};

pub(super) fn run(command: MonorepoCheckCommand) -> Result<()> {
    let (root, project) = super::load(&std::env::current_dir()?)?;
    let graph = project
        .monorepo
        .as_ref()
        .context("missing monorepo config")?
        .resolve(&root)?;
    let component = graph
        .components
        .get(&command.component)
        .with_context(|| format!("unknown component {}", command.component))?;
    let gate = component
        .checks
        .iter()
        .find(|gate| gate.id == command.check)
        .with_context(|| format!("unknown check {} in {}", command.check, command.component))?;
    // Only generated CI opts into provider resolution. Local literals retain
    // precedence over unrelated inherited values; never echo either value.
    let mut environment = Vec::new();
    for (name, value) in &gate.env {
        let value = match command.gate_env {
            MonorepoGateEnv::Local => {
                if value.contains("${{") {
                    bail!(
                        "gate environment variable {name}: provider expression requires --gate-env provider-resolved"
                    );
                }
                value.into()
            }
            MonorepoGateEnv::ProviderResolved => std::env::var_os(name).with_context(|| {
                format!("missing provider-resolved gate environment variable {name}")
            })?,
        };
        environment.push((name, value));
    }
    let since = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    // The command comes from the validated project config, never event data.
    // Preserve trunk's Bash errexit, nounset and pipefail contract.
    let mut process = Command::new("bash");
    process
        .args(["-euo", "pipefail", "-c", &gate.run])
        .envs(environment)
        .current_dir(&root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    let deadline = Instant::now() + Duration::from_secs(gate.timeout_minutes * 60);
    let mut child = GateProcess(process.spawn().context("run declared component check")?);
    let status = loop {
        if let Some(status) = child
            .0
            .try_wait()
            .context("wait for declared component check")?
        {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Reap Bash and stop its descendants before collecting failure evidence.
    drop(child);
    if status.is_some_and(|status| status.success()) {
        return Ok(());
    }
    if command.nix_diagnostics {
        diagnostics(since);
    }
    let Some(status) = status else {
        bail!(
            "component {} check {} timed out after {} minutes",
            command.component,
            command.check,
            gate.timeout_minutes
        );
    };
    bail!(
        "component {} check {} failed: {status}",
        command.component,
        command.check
    )
}

struct GateProcess(Child);

impl Drop for GateProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn diagnostics(since: u64) {
    eprintln!("=== Nix failure diagnostics (check start @{since}) ===");
    if !cfg!(target_os = "linux") {
        eprintln!("Linux kernel/systemd diagnostics unavailable on this platform");
        return;
    }
    // Fixed paths and bounded, non-interactive commands; never print the
    // inherited environment, restart services, or retry a failed check.
    for path in ["/proc/meminfo", "/proc/pressure/memory"] {
        match fs::read_to_string(path) {
            Ok(text) => eprintln!("--- {path} ---\n{text}"),
            Err(error) => eprintln!("{path}: {error}"),
        }
    }
    let since = format!("@{since}");
    for (label, arguments) in [
        (
            "Nix daemon status",
            vec![
                "systemctl",
                "show",
                "nix-daemon.service",
                "--property=ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,MemoryCurrent,MemoryPeak,OOMPolicy",
            ],
        ),
        (
            "Nix daemon journal",
            vec![
                "journalctl",
                "--unit=nix-daemon.service",
                "--since",
                &since,
                "--lines=150",
                "--no-pager",
                "--output=short-iso",
            ],
        ),
        (
            "Kernel journal",
            vec![
                "journalctl",
                "--dmesg",
                "--since",
                &since,
                "--lines=150",
                "--no-pager",
                "--output=short-iso",
            ],
        ),
    ] {
        eprintln!("--- {label} ---");
        match Command::new("timeout")
            .args(["--kill-after=5s", "15s", "sudo", "-n"])
            .args(arguments)
            .output()
        {
            Ok(output) => {
                eprintln!(
                    "status: {}\n{}{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(error) => eprintln!("diagnostics unavailable: {error}"),
        }
    }
}
