//! Unprivileged worker-side supervisor protocol and ordinary Nix sandbox probe.
use super::{
    artifact,
    contract::Plan,
    kvrocks::{self, CacheReceipt, ReadyReceipt},
    kvrocks_host::HostRequest,
    process,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

enum Frame {
    Ready(Box<ReadyReceipt>),
    Stopped(Value),
    Failed(String),
}

pub fn wrapper() -> Result<PathBuf> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let bin = executable.parent().context("engine binary has no parent")?;
    let output = bin.parent().context("engine binary has no store output")?;
    artifact::store_path(output.to_str().context("non-UTF8 engine output")?)?;
    let wrapper = bin.join("repo-review");
    ensure!(
        wrapper.is_file(),
        "requires the immutable packaged review-tools wrapper"
    );
    Ok(wrapper)
}

pub struct Supervisor {
    child: Child,
    input: Option<ChildStdin>,
    frames: Receiver<Frame>,
    pub ready: ReadyReceipt,
    roots: tempfile::TempDir,
    stopped: bool,
}

impl Supervisor {
    pub fn start(plan: &Plan, system: &str, out: &Path) -> Result<Self> {
        let identity = kvrocks::lookup_worker_identity()?;
        let bootstrap = kvrocks::bootstrap(plan, system, &out.join("compiler-cache-bootstrap"))?;
        let roots = tempfile::tempdir()?;
        let native_root = roots.path().join("native-tool");
        let native: Value = serde_json::from_str(&process::run(
            "nix",
            &process::nix_args(&[
                "build",
                "--json",
                "--out-link",
                native_root.to_str().context("non-UTF8 native GC root")?,
                &format!("{}^out", bootstrap.source.derivation),
            ]),
            None,
            Some(&out.join("compiler-cache-root.log")),
        )?)?;
        ensure!(
            native.as_array().is_some_and(|items| items.len() == 1)
                && native[0]["drvPath"] == bootstrap.source.derivation
                && native[0]["outputs"] == serde_json::json!({"out": bootstrap.source.output}),
            "native GC root differs from frozen bootstrap"
        );
        let request = HostRequest {
            plan: plan.clone(),
            system: system.into(),
            bootstrap,
            identity,
        };
        let expected = request.service()?;
        let wrapper = wrapper()?;
        let mut command = process::command(
            "sudo",
            &process::args(&[
                "-n",
                wrapper.to_str().context("non-UTF8 engine wrapper")?,
                "kvrocks-launch",
            ]),
            None,
        );
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(File::create(
                out.join("compiler-cache-host.log"),
            )?));
        let mut child = command
            .spawn()
            .context("starting source-owned Kvrocks supervisor")?;
        let input = child
            .stdin
            .take()
            .context("missing supervisor control pipe")?;
        let stdout = child
            .stdout
            .take()
            .context("missing supervisor response pipe")?;
        let (sender, frames) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut reader = BufReader::new(stdout.take(1024 * 1024));
                let mut first = String::new();
                reader.read_line(&mut first)?;
                ensure!(
                    first.len() <= 65536 && first.ends_with('\n'),
                    "invalid bounded ready frame"
                );
                let ready: ReadyReceipt = serde_json::from_str(&first)?;
                sender.send(Frame::Ready(Box::new(ready)))?;
                let mut remainder = Vec::new();
                reader.read_to_end(&mut remainder)?;
                ensure!(remainder.len() <= 65536, "oversized stop receipt");
                sender.send(Frame::Stopped(serde_json::from_slice(&remainder)?))?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = sender.send(Frame::Failed(error.to_string()));
            }
        });
        // Construct the guard before writing or waiting can fail. Closing its
        // sole control writer revokes the privileged lifetime on every error.
        let mut owned = Self {
            child,
            input: Some(input),
            frames,
            roots,
            stopped: false,
            ready: ReadyReceipt {
                service: expected.clone(),
                invocation: kvrocks::UnitIdentity {
                    invocation_id: String::new(),
                    pid: 0,
                    control_group: String::new(),
                },
                supervisor: kvrocks::UnitIdentity {
                    invocation_id: String::new(),
                    pid: 0,
                    control_group: String::new(),
                },
                socket_device: 0,
                socket_inode: 0,
            },
        };
        let frame = serde_json::to_vec(&request)?;
        ensure!(
            (frame.len() as u64) < kvrocks::HOST_REQUEST_MAX_BYTES,
            "startup frame exceeds its bounded protocol"
        );
        let deadline = Instant::now() + Duration::from_secs(kvrocks::WORKER_STARTUP_SECONDS);
        let input = owned.input.as_mut().context("missing control writer")?;
        input.write_all(&frame)?;
        input.write_all(b"\n")?;
        input.flush()?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("worker cache startup deadline exceeded")?;
        owned.ready = match owned.frames.recv_timeout(remaining)? {
            Frame::Ready(ready) => *ready,
            Frame::Failed(error) => anyhow::bail!("cache supervisor rejected startup: {error}"),
            Frame::Stopped(_) => anyhow::bail!("cache supervisor stopped before readiness"),
        };
        ensure!(
            owned.ready.service == expected
                && owned.ready.invocation.pid > 1
                && owned.ready.invocation.control_group
                    == format!("/system.slice/{}", expected.unit)
                && owned.ready.socket_inode > 0,
            "supervisor ready identity differs from owned immutable plan"
        );
        super::write_json(&out.join("compiler-cache-ready.json"), &owned.ready)?;
        Ok(owned)
    }

    pub fn ensure_alive(&mut self) -> Result<()> {
        ensure!(
            self.child.try_wait()?.is_none(),
            "cache supervisor exited during consumer execution"
        );
        Ok(())
    }

    pub fn admit_sandbox(&mut self, out: &Path) -> Result<CacheReceipt> {
        self.ensure_alive()?;
        let source = &self.ready.service.bootstrap.source;
        let wrapper = wrapper()?;
        let output = wrapper
            .parent()
            .and_then(Path::parent)
            .context("missing engine output")?
            .to_str()
            .context("non-UTF8 engine output")?;
        ensure!(
            output
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._+-".contains(&byte)),
            "unsafe engine store output"
        );
        let reference = format!(
            "github:NixOS/nixpkgs/{}#legacyPackages.{}",
            source.nixpkgs_revision, source.system
        );
        let expression = format!(
            "pkgs: (pkgs.runCommandNoCC \"simit-kvrocks-sandbox-admission\" {{}} '' ${{{output}}}/bin/repo-review kvrocks-probe > \"$out\" '').drvPath"
        );
        let derivation = process::run(
            "nix",
            &process::nix_args(&[
                "eval",
                "--raw",
                "--no-update-lock-file",
                &reference,
                "--apply",
                &expression,
            ]),
            None,
            Some(&out.join("compiler-cache-sandbox-eval.log")),
        )?;
        artifact::store_path(&derivation)?;
        ensure!(
            derivation.ends_with(".drv"),
            "unfrozen sandbox admission derivation"
        );
        let root = self.roots.path().join("sandbox-admission");
        let mut build = process::nix_args(&[
            "build",
            "--json",
            "--out-link",
            root.to_str().context("non-UTF8 sandbox GC root")?,
            &format!("{derivation}^out"),
        ]);
        build.extend(kvrocks::sandbox_arguments());
        let realized: Value = serde_json::from_str(&process::run(
            "nix",
            &build,
            None,
            Some(&out.join("compiler-cache-sandbox.log")),
        )?)?;
        ensure!(
            realized.as_array().is_some_and(|items| items.len() == 1)
                && realized[0]["drvPath"] == derivation,
            "sandbox probe derivation changed"
        );
        let sandbox_output = realized[0]["outputs"]["out"]
            .as_str()
            .context("missing sandbox probe output")?;
        artifact::store_path(sandbox_output)?;
        // --rebuild needs a valid initial output. Execute the same frozen probe
        // again even when the first realization was substituted.
        let mut rebuild = process::nix_args(&[
            "build",
            "--no-link",
            "--rebuild",
            &format!("{derivation}^out"),
        ]);
        rebuild.extend(kvrocks::sandbox_arguments());
        process::run(
            "nix",
            &rebuild,
            None,
            Some(&out.join("compiler-cache-sandbox-rebuild.log")),
        )?;
        let metadata = fs::metadata(sandbox_output)?;
        ensure!(
            metadata.is_file() && metadata.len() <= 65536,
            "invalid sandbox admission output"
        );
        let bytes = fs::read(sandbox_output)?;
        ensure!(
            serde_json::from_slice::<Value>(&bytes)?
                == serde_json::json!({"socket": "/run/redis-sccache/redis.sock", "resp2_ping": true}),
            "ordinary sandbox did not establish canonical socket admission"
        );
        self.ensure_alive()?;
        let receipt = CacheReceipt {
            ready: self.ready.clone(),
            sandbox_derivation: derivation,
            sandbox_output: sandbox_output.into(),
            sandbox_receipt_sha256: super::digest(&bytes),
        };
        fs::write(out.join("compiler-cache-probe.json"), &bytes)?;
        super::write_json(&out.join("compiler-cache-sandbox.json"), &receipt)?;
        Ok(receipt)
    }

    pub fn stop(&mut self) -> Result<Value> {
        ensure!(!self.stopped, "cache supervisor already stopped");
        drop(self.input.take());
        let deadline = Instant::now()
            + Duration::from_secs(u64::from(self.ready.service.shutdown_seconds) + 15);
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            ensure!(
                Instant::now() < deadline,
                "cache supervisor graceful shutdown deadline exceeded"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        ensure!(
            status.success(),
            "owned cache supervisor failed shutdown/cleanup"
        );
        let receipt = match self.frames.recv_timeout(Duration::from_secs(1))? {
            Frame::Stopped(receipt) => receipt,
            Frame::Failed(error) => anyhow::bail!("cache cleanup receipt rejected: {error}"),
            Frame::Ready(_) => anyhow::bail!("duplicate cache ready frame"),
        };
        ensure!(
            receipt
                == serde_json::json!({"stopped": true, "run_identity": self.ready.service.run_identity}),
            "cleanup receipt differs from the owned service lifetime"
        );
        self.stopped = true;
        Ok(receipt)
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if !self.stopped {
            if let Err(error) = self.stop() {
                eprintln!("owned cache worker cleanup failed: {error:#}");
            }
        }
    }
}
