//! Execute the same project-owned gate locally and in generated qualification.
use std::{
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
    // Bash normalizes IFS/PWD on startup and may source BASH_ENV. Stage declared
    // data under inert names, then assign it after startup without interpolation.
    let mut prefix = "SIMIT_NATIVE_GATE_VALUE_".to_owned();
    while environment
        .iter()
        .any(|(name, _)| name.starts_with(&prefix))
    {
        prefix.push('_');
    }
    let mut script = String::new();
    for (index, (name, value)) in environment.iter().enumerate() {
        let alias = format!("{prefix}{index}");
        process.env_remove(name).env(&alias, value);
        script.push_str(&format!("export {name}=\"${alias}\"\nunset {alias}\n"));
    }
    script.push_str(&gate.run);
    process
        .args(["-euo", "pipefail", "-c", &script])
        .current_dir(&root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    let seconds = command
        .deadline
        .map_or(gate.timeout_minutes * 60, |deadline| {
            deadline
                .saturating_sub(since)
                .min(gate.timeout_minutes * 60)
        });
    let cancelled = Arc::new(AtomicBool::new(false));
    // This CLI exits after one command. As with the native CI CLI, retain the
    // process-global handlers for its lifetime; Drop is not run on default
    // signal termination and Bash lives in a separate process group.
    #[cfg(unix)]
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, cancelled.clone())?;
    }
    let status = if seconds == 0 {
        None
    } else {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        if cancelled.load(Ordering::SeqCst) {
            bail!("component check cancelled before execution");
        }
        let mut child = GateProcess {
            child: process.spawn().context("run declared component check")?,
            cleanup: true,
        };
        let status = loop {
            if cancelled.load(Ordering::SeqCst) {
                break None;
            }
            if let Some(status) = child
                .child
                .try_wait()
                .context("wait for declared component check")?
            {
                // Successful setup gates may leave services for later steps.
                // Bash is already reaped by try_wait; preserve its descendants.
                if status.success() {
                    child.cleanup = false;
                }
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        // Reap Bash and stop its descendants before collecting failure evidence.
        drop(child);
        status
    };
    if status.is_some_and(|status| status.success()) {
        return Ok(());
    }
    if command.nix_diagnostics {
        diagnostics(since);
    }
    if cancelled.load(Ordering::SeqCst) {
        bail!(
            "component {} check {} cancelled",
            command.component,
            command.check
        );
    }
    let Some(status) = status else {
        bail!(
            "component {} check {} timed out (configured limit: {} minutes)",
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

struct GateProcess {
    child: Child,
    cleanup: bool,
}

impl Drop for GateProcess {
    fn drop(&mut self) {
        if !self.cleanup {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = rustix::process::Pid::from_raw(self.child.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) const NIX_DIAGNOSTICS: &str = include_str!("nix_diagnostics.sh");

fn diagnostics(since: u64) {
    // The caller uses this same bounded script if provisioning fails before
    // native Simit starts. Neither path needs another Nix evaluation.
    if let Err(error) = Command::new("bash")
        .args([
            "-euo",
            "pipefail",
            "-c",
            NIX_DIAGNOSTICS,
            "simit-diagnostics",
            &since.to_string(),
        ])
        .status()
    {
        eprintln!("diagnostics unavailable: {error}");
    }
}
