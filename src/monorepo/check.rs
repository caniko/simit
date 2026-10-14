//! Execute the same project-owned gate locally and in generated qualification.
use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::cli::MonorepoCheckCommand;

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
    let since = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    // The command comes from the validated project config, never event data.
    // Match the existing generated sh -ec contract and component environment.
    let status = Command::new("sh")
        .args(["-ec", &gate.run])
        .envs(&gate.env)
        .current_dir(&root)
        .status()
        .context("run declared component check")?;
    if status.success() {
        return Ok(());
    }
    if command.nix_diagnostics {
        diagnostics(since);
    }
    bail!(
        "component {} check {} failed: {status}",
        command.component,
        command.check
    )
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
