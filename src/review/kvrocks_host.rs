//! Privileged, source-owned service lifetime. Only the packaged worker tool is
//! evaluated here; consumer evaluation and compilation remain unprivileged.
use super::{
    artifact, canonical_digest,
    contract::{CompilerCache, Plan},
    engine, kvrocks,
    kvrocks_lifecycle::{self, UnitIdentity},
    process,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequest {
    pub plan: Plan,
    pub system: String,
    pub bootstrap: kvrocks::BootstrapReceipt,
    pub identity: kvrocks::WorkerIdentity,
}

impl HostRequest {
    pub fn service(&self) -> Result<kvrocks::ServicePlan> {
        self.plan.validate()?;
        ensure!(
            self.plan.request.compiler_cache == Some(CompilerCache::KvrocksV1)
                && self.plan.request.systems.contains(&self.system)
                && self.bootstrap.source.system == self.system
                && self.plan.run_id > 0
                && self.plan.run_attempt > 0,
            "invalid immutable native cache lifecycle request"
        );
        kvrocks::service_plan(
            &self.bootstrap,
            &self.identity,
            Path::new("/var/lib"),
            &canonical_digest(&(&self.plan.digest, &self.system))?,
        )
    }
}

pub use super::kvrocks::ReadyReceipt;

fn bounded_file(path: &Path, maximum: u64) -> Result<String> {
    let mut value = String::new();
    File::open(path)?
        .take(maximum + 1)
        .read_to_string(&mut value)?;
    ensure!(
        value.len() as u64 <= maximum,
        "host observation is too large"
    );
    Ok(value)
}

fn root_tool(program: &str, arguments: &[String], seconds: u64) -> Result<String> {
    root_tool_with_timeout(program, arguments, Duration::from_secs(seconds))
}

fn root_tool_with_timeout(
    program: &str,
    arguments: &[String],
    timeout: Duration,
) -> Result<String> {
    process::run_command_with_limits(
        process::command(program, arguments, None),
        None,
        process::CommandLimits {
            timeout,
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 1024 * 1024,
        },
    )
}

struct StartupBudget {
    deadline: Instant,
}

impl StartupBudget {
    fn remaining(&self, phase_seconds: u64) -> Result<Duration> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .context("shared cache startup deadline exceeded")?;
        ensure!(
            !remaining.is_zero(),
            "shared cache startup deadline exceeded"
        );
        Ok(remaining.min(Duration::from_secs(phase_seconds)))
    }

    fn run(&self, program: &str, arguments: &[String], seconds: u64) -> Result<String> {
        root_tool_with_timeout(program, arguments, self.remaining(seconds)?)
    }
}

const UNIT_PROPERTIES: &str = "Id,Description,LoadState,ActiveState,SubState,InvocationID,MainPID,ControlGroup,User,Group,KillMode,TimeoutStopUSec,SendSIGKILL,KillSignal,FinalKillSignal,RestrictAddressFamilies,LimitNOFILE";

fn unit_properties(plan: &kvrocks::ServicePlan) -> Result<String> {
    show_unit(&plan.unit)
}

fn show_unit(unit: &str) -> Result<String> {
    root_tool(
        "/usr/bin/systemctl",
        &process::args(&["show", "--no-pager", "--property", UNIT_PROPERTIES, unit]),
        5,
    )
}

fn unit_properties_until(plan: &kvrocks::ServicePlan, deadline: Instant) -> Result<String> {
    StartupBudget { deadline }.run(
        "/usr/bin/systemctl",
        &process::args(&[
            "show",
            "--no-pager",
            "--property",
            UNIT_PROPERTIES,
            &plan.unit,
        ]),
        5,
    )
}

fn trusted_parent(path: &Path) -> Result<()> {
    for parent in path.ancestors() {
        let metadata = fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
            "privileged cache allocation needs root-owned, non-writable real parents"
        );
    }
    Ok(())
}

fn startup_frame(
    input: impl Read + Send + 'static,
    cancelled: Arc<AtomicBool>,
) -> Result<HostRequest> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = BufReader::new(input.take(kvrocks::HOST_REQUEST_MAX_BYTES + 1));
        let frame = (|| -> Result<HostRequest> {
            let mut frame = String::new();
            input.read_line(&mut frame)?;
            ensure!(
                frame.len() as u64 <= kvrocks::HOST_REQUEST_MAX_BYTES && frame.ends_with('\n'),
                "invalid bounded cache host frame"
            );
            Ok(serde_json::from_str(&frame)?)
        })();
        if sender.send(frame).is_ok() {
            let mut byte = [0];
            // EOF, extra input and read failure all revoke the lifetime.
            let _ = input.read(&mut byte);
        }
        cancelled.store(true, Ordering::SeqCst);
    });
    receiver
        .recv_timeout(Duration::from_secs(kvrocks::STARTUP_FRAME_SECONDS))
        .context("bounded cache startup frame deadline exceeded")?
}

fn cleanup_socket(
    runtime: Option<&kvrocks::OwnedRuntime>,
    socket: &Path,
    identity: &kvrocks::WorkerIdentity,
    pinned: Option<(u64, u64)>,
) -> Result<()> {
    let Some(runtime) = runtime else {
        return Ok(());
    };
    runtime.verify_owned_path()?;
    match fs::symlink_metadata(socket) {
        Ok(metadata) => {
            kvrocks::verify_socket(socket, identity)?;
            if let Some(pinned) = pinned {
                ensure!(
                    pinned == (metadata.dev(), metadata.ino()) && metadata.file_type().is_socket(),
                    "owned service socket was replaced; preserve it"
                );
            }
            fs::remove_file(socket)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_native_tool(request: &HostRequest, budget: &StartupBudget) -> Result<()> {
    engine::verify_plan(&request.plan)?;
    let manifest = engine::manifest()?;
    let reference = kvrocks::tool_reference(&manifest, &request.system)?;
    let package: serde_json::Value = serde_json::from_str(&budget.run(
        "nix",
        &process::nix_args(&[
            "eval",
            "--json",
            "--no-update-lock-file",
            &reference,
            "--apply",
            "p: { inherit (p) pname version system drvPath outPath outputs; }",
        ]),
        120,
    )?)?;
    ensure!(
        kvrocks::freeze_source(&manifest, &request.system, &package)? == request.bootstrap.source,
        "privileged bootstrap differs from the packaged worker's native pin"
    );
    let output = Path::new(&request.bootstrap.source.output).canonicalize()?;
    let binary = output.join("bin/kvrocks").canonicalize()?;
    ensure!(
        binary.starts_with(&output),
        "native tool binary escapes its output"
    );
    let metadata = fs::metadata(&binary)?;
    ensure!(
        metadata.is_file()
            && metadata.len() <= 256 * 1024 * 1024
            && artifact::file_digest(&binary)? == request.bootstrap.binary_sha256,
        "native tool binary differs from its bootstrap receipt"
    );
    let info: serde_json::Value = serde_json::from_str(&budget.run(
        "nix",
        &process::nix_args(&["path-info", "--json", &request.bootstrap.source.output]),
        30,
    )?)?;
    let item = info
        .get(&request.bootstrap.source.output)
        .or_else(|| {
            info.as_array()
                .filter(|items| {
                    items.len() == 1 && items[0]["path"] == request.bootstrap.source.output
                })
                .map(|items| &items[0])
        })
        .context("native NAR report differs from frozen output")?;
    ensure!(
        item["narHash"] == request.bootstrap.nar_hash
            && item["narSize"] == request.bootstrap.nar_size,
        "native NAR identity differs from the bootstrap receipt"
    );
    Ok(())
}

struct HostService {
    plan: kvrocks::ServicePlan,
    control: File,
    state: Option<kvrocks::OwnedRuntime>,
    runtime: Option<kvrocks::OwnedRuntime>,
    invocation: Option<UnitIdentity>,
    socket_identity: Option<(u64, u64)>,
    config_identity: Option<(u64, u64)>,
    supervisor: UnitIdentity,
    started: bool,
    finished: bool,
}

impl HostService {
    fn allocate(
        plan: kvrocks::ServicePlan,
        supervisor: UnitIdentity,
        budget: &StartupBudget,
    ) -> Result<Self> {
        budget.remaining(5)?;
        for parent in [Path::new("/run"), Path::new("/var/lib")] {
            trusted_parent(parent)?;
        }
        for path in [
            &plan.control_directory,
            &plan.state_directory,
            "/run/redis-sccache",
        ] {
            kvrocks::refuse_existing_runtime(Path::new(path))?;
        }
        let existing = budget.run(
            "/usr/bin/systemctl",
            &process::args(&["list-units", "--all", "--plain", "--no-legend", &plan.unit]),
            5,
        )?;
        ensure!(
            existing.trim().is_empty(),
            "Kvrocks unit already exists; preserve its owner"
        );
        fs::DirBuilder::new()
            .mode(0o750)
            .create(&plan.control_directory)?;
        let allocated = fs::symlink_metadata(&plan.control_directory)?;
        let control = match File::open(&plan.control_directory) {
            Ok(file) => file,
            Err(error) => {
                if fs::symlink_metadata(&plan.control_directory).is_ok_and(|current| {
                    current.is_dir()
                        && current.dev() == allocated.dev()
                        && current.ino() == allocated.ino()
                }) {
                    let _ = fs::remove_dir(&plan.control_directory);
                }
                return Err(error).context("retaining privileged control directory inode");
            }
        };
        let mut owned = Self {
            plan,
            control,
            state: None,
            runtime: None,
            invocation: None,
            socket_identity: None,
            config_identity: None,
            supervisor,
            started: false,
            finished: false,
        };
        std::os::unix::fs::chown(
            &owned.plan.control_directory,
            Some(0),
            Some(owned.plan.identity.gid),
        )?;
        owned.state = Some(kvrocks::OwnedRuntime::create(
            Path::new(&owned.plan.state_directory),
            &owned.plan.identity,
        )?);
        owned.runtime = Some(kvrocks::OwnedRuntime::create(
            Path::new("/run/redis-sccache"),
            &owned.plan.identity,
        )?);
        let config = Path::new(&owned.plan.control_directory).join("kvrocks.conf");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o640)
            .open(&config)?;
        let allocated = file.metadata()?;
        owned.config_identity = Some((allocated.dev(), allocated.ino()));
        file.write_all(owned.plan.config.as_bytes())?;
        file.sync_all()?;
        std::os::unix::fs::chown(config, Some(0), Some(owned.plan.identity.gid))?;
        budget.remaining(5)?;
        Ok(owned)
    }

    fn cgroup_path(&self) -> PathBuf {
        Path::new("/sys/fs/cgroup/system.slice").join(&self.plan.unit)
    }

    fn observe_ready(&mut self, deadline: Instant) -> Result<ReadyReceipt> {
        let invocation = kvrocks_lifecycle::unit_identity(
            &self.plan,
            &unit_properties_until(&self.plan, deadline)?,
        )?;
        if let Some(prior) = &self.invocation {
            ensure!(
                prior == &invocation,
                "owned unit invocation/process was replaced"
            );
        }
        self.invocation = Some(invocation.clone());
        let cgroup = self.cgroup_path();
        kvrocks_lifecycle::validate_limits(
            &self.plan,
            &bounded_file(&cgroup.join("memory.max"), 64)?,
            &bounded_file(&cgroup.join("cpu.max"), 64)?,
            &bounded_file(&cgroup.join("pids.max"), 64)?,
        )?;
        let process_root = Path::new("/proc").join(invocation.pid.to_string());
        kvrocks_lifecycle::validate_process_identity(
            &bounded_file(&process_root.join("status"), 65536)?,
            &self.plan.identity,
        )?;
        let members = bounded_file(&cgroup.join("cgroup.procs"), 65536)?;
        ensure!(
            members.lines().collect::<Vec<_>>() == [invocation.pid.to_string()],
            "unexpected process in owned Kvrocks cgroup"
        );
        ensure!(
            fs::read_link(process_root.join("exe"))?
                == Path::new(&self.plan.bootstrap.source.output)
                    .join("bin/kvrocks")
                    .canonicalize()?,
            "owned unit executes a different native binary"
        );
        let mut inodes = Vec::new();
        for entry in fs::read_dir(process_root.join("fd"))? {
            let target = fs::read_link(entry?.path())?;
            if let Some(inode) = target
                .to_str()
                .and_then(|value| value.strip_prefix("socket:["))
                .and_then(|value| value.strip_suffix(']'))
            {
                inodes.push(inode.parse().context("invalid process socket inode")?);
            }
        }
        kvrocks_lifecycle::validate_unix_listeners(
            &bounded_file(&process_root.join("net/unix"), 1024 * 1024)?,
            &inodes,
            &self.plan.socket,
        )?;
        kvrocks::verify_socket(Path::new(&self.plan.socket), &self.plan.identity)?;
        super::kvrocks_probe::ping_until(
            Path::new(&self.plan.socket),
            deadline.min(Instant::now() + Duration::from_secs(2)),
        )?;
        let metadata = fs::symlink_metadata(&self.plan.socket)?;
        let socket_identity = (metadata.dev(), metadata.ino());
        if let Some(prior) = self.socket_identity {
            ensure!(prior == socket_identity, "owned cache socket was replaced");
        }
        self.socket_identity = Some(socket_identity);
        let current = kvrocks_lifecycle::unit_identity(
            &self.plan,
            &unit_properties_until(&self.plan, deadline)?,
        )?;
        ensure!(
            Instant::now() < deadline && current == invocation,
            "readiness expired or live unit changed during observation"
        );
        Ok(ReadyReceipt {
            service: self.plan.clone(),
            invocation,
            supervisor: self.supervisor.clone(),
            socket_device: metadata.dev(),
            socket_inode: metadata.ino(),
        })
    }

    fn start(&mut self, cancelled: &AtomicBool, budget: &StartupBudget) -> Result<ReadyReceipt> {
        ensure!(!cancelled.load(Ordering::SeqCst), "cache startup cancelled");
        // Arm before launch: an ambiguous failure still needs ownership
        // reconciliation and graceful own-unit cleanup.
        let deadline = budget
            .deadline
            .min(Instant::now() + Duration::from_secs(u64::from(self.plan.startup_seconds)));
        self.started = true;
        if let Some(state) = &mut self.state {
            state.preserve_on_drop();
        }
        if let Some(runtime) = &mut self.runtime {
            runtime.preserve_on_drop();
        }
        StartupBudget { deadline }.run("/usr/bin/systemd-run", &self.plan.systemd_args, 5)?;
        loop {
            ensure!(!cancelled.load(Ordering::SeqCst), "cache startup cancelled");
            match self.observe_ready(deadline) {
                Ok(receipt) => return Ok(receipt),
                Err(error) if Instant::now() >= deadline => {
                    return Err(error).context("Kvrocks startup deadline exceeded");
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    fn stop(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if self.started {
            let observation = unit_properties(&self.plan)?;
            let stop = kvrocks_lifecycle::shutdown_required(
                &self.plan,
                self.invocation.as_ref(),
                &observation,
            )?;
            // systemd implements the retained thirty-second SIGTERM grace and
            // final SIGKILL only in this verified unit's own control group.
            if stop {
                root_tool(
                    "/usr/bin/systemctl",
                    &process::args(&["stop", &self.plan.unit]),
                    u64::from(self.plan.shutdown_seconds) + 5,
                )?;
            }
            if self.cgroup_path().exists() {
                ensure!(
                    bounded_file(&self.cgroup_path().join("cgroup.procs"), 65536)?
                        .trim()
                        .is_empty(),
                    "owned service cgroup still has processes; preserve its state"
                );
            }
        }
        cleanup_socket(
            self.runtime.as_ref(),
            Path::new(&self.plan.socket),
            &self.plan.identity,
            self.socket_identity,
        )?;
        if let Some(runtime) = self.runtime.take() {
            runtime.cleanup()?;
        }
        if let Some(mut state) = self.state.take() {
            state.cleanup_service_data()?;
        }
        let current = fs::symlink_metadata(&self.plan.control_directory)?;
        let pinned = self.control.metadata()?;
        ensure!(
            current.is_dir()
                && current.uid() == 0
                && current.dev() == pinned.dev()
                && current.ino() == pinned.ino(),
            "privileged control directory was replaced; preserve it"
        );
        let config = Path::new(&self.plan.control_directory).join("kvrocks.conf");
        if let Some(identity) = self.config_identity {
            let metadata = fs::symlink_metadata(&config)?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == 0
                    && metadata.nlink() == 1
                    && (metadata.dev(), metadata.ino()) == identity,
                "control configuration was replaced; preserve it"
            );
            fs::remove_file(config)?;
        } else {
            // No successful exclusive open means no ownership of this path.
            kvrocks::refuse_existing_runtime(&config)?;
        }
        fs::remove_dir(&self.plan.control_directory)?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        if !self.finished {
            if let Err(error) = self.stop() {
                eprintln!("owned Kvrocks cleanup failed: {error:#}");
            }
        }
    }
}

/// A root supervisor holds directory inodes for the whole unprivileged build.
/// EOF on the control pipe (including worker death/cancellation), or a termination
/// signal, triggers verified own-unit graceful shutdown.
pub fn host() -> Result<serde_json::Value> {
    let budget = StartupBudget {
        deadline: Instant::now() + Duration::from_secs(kvrocks::HOST_STARTUP_SECONDS),
    };
    // A mutable development binary is never a privileged execution route.
    super::kvrocks_worker::wrapper()?;
    ensure!(
        budget.run("id", &process::args(&["-u"]), 5)?.trim() == "0",
        "cache host helper requires sudo"
    );
    let caller: u32 = std::env::var("SUDO_UID")
        .context("cache helper needs original worker identity")?
        .parse()?;
    ensure!(
        caller != 0 && caller != u32::MAX,
        "invalid original worker UID"
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(signal, cancelled.clone())?;
    }
    let request = startup_frame(std::io::stdin(), cancelled.clone())?;
    ensure!(
        request.identity.uid == caller,
        "privileged cache identity differs from original worker"
    );
    let passwd = budget.run(
        "getent",
        &process::args(&["passwd", &request.identity.user]),
        5,
    )?;
    let group = budget.run("getent", &process::args(&["group", "nixbld"]), 5)?;
    ensure!(
        kvrocks::parse_worker_identity(&request.identity.user, "nixbld", &passwd, &group)?
            == request.identity,
        "privileged account lookup differs from original worker identity"
    );
    validate_native_tool(&request, &budget)?;
    ensure!(!cancelled.load(Ordering::SeqCst), "cache startup cancelled");
    let service = request.service()?;
    let supervisor = kvrocks_lifecycle::supervisor_identity(
        &service,
        std::process::id(),
        &std::env::var("INVOCATION_ID")
            .context("cache supervisor requires a managed invocation")?,
        &budget.run(
            "/usr/bin/systemctl",
            &process::args(&[
                "show",
                "--no-pager",
                "--property",
                UNIT_PROPERTIES,
                &kvrocks::supervisor_unit(&service),
            ]),
            5,
        )?,
    )?;
    let mut owned = HostService::allocate(service, supervisor, &budget)?;
    let receipt = owned.start(&cancelled, &budget)?;
    println!("{}", serde_json::to_string(&receipt)?);
    std::io::stdout().flush()?;
    let lifetime = Instant::now() + Duration::from_secs(u64::from(owned.plan.lifetime_seconds));
    while !cancelled.load(Ordering::SeqCst) {
        ensure!(Instant::now() < lifetime, "owned cache lifetime exceeded");
        let current =
            kvrocks_lifecycle::unit_identity(&owned.plan, &unit_properties(&owned.plan)?)?;
        ensure!(
            owned.invocation.as_ref() == Some(&current),
            "owned cache unit changed during compilation"
        );
        kvrocks::verify_socket(Path::new(&owned.plan.socket), &owned.plan.identity)?;
        let socket = fs::symlink_metadata(&owned.plan.socket)?;
        ensure!(
            owned.socket_identity == Some((socket.dev(), socket.ino())),
            "owned cache socket was replaced during compilation"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    owned.stop()?;
    Ok(serde_json::json!({"stopped": true, "run_identity": owned.plan.run_identity}))
}

/// A short root launcher authenticates sudo's real caller before asking PID 1 to
/// own the supervisor. Its pipe writer closes on launcher/worker death; the
/// independent supervisor can then finish the full graceful cleanup budget.
pub fn launch() -> Result<serde_json::Value> {
    let wrapper = super::kvrocks_worker::wrapper()?;
    ensure!(
        root_tool("id", &process::args(&["-u"]), 5)?.trim() == "0",
        "cache launcher requires sudo"
    );
    let caller: u32 = std::env::var("SUDO_UID")
        .context("cache launcher needs original sudo caller")?
        .parse()?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let request = startup_frame(std::io::stdin(), cancelled.clone())?;
    let service = request.service()?;
    ensure!(
        request.identity.uid == caller && !cancelled.load(Ordering::SeqCst),
        "cache launcher caller/lifetime mismatch"
    );
    let mut command = process::command(
        "/usr/bin/systemd-run",
        &kvrocks::supervisor_arguments(&service, &wrapper, caller)?,
        None,
    );
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let mut child = command
        .spawn()
        .context("launching PID-1-managed cache supervisor")?;
    let mut input = child
        .stdin
        .take()
        .context("missing managed supervisor control writer")?;
    input.write_all(&serde_json::to_vec(&request)?)?;
    input.write_all(b"\n")?;
    input.flush()?;
    std::thread::spawn(move || {
        while !cancelled.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(25));
        }
        drop(input);
    });
    let status = child.wait()?;
    // The managed helper already emitted the ready and stopped JSON frames.
    // Preserve that exact stream and status without appending a third frame.
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn startup_phases_share_a_deadline_with_a_larger_worker_handoff_budget() {
        let budget = StartupBudget {
            deadline: Instant::now() + Duration::from_secs(10),
        };
        assert!(budget.remaining(120).unwrap() <= Duration::from_secs(10));
        assert_eq!(budget.remaining(5).unwrap(), Duration::from_secs(5));
        let expired = StartupBudget {
            deadline: Instant::now() - Duration::from_secs(1),
        };
        // A depleted budget refuses a new process before spawning it.
        assert!(
            expired
                .run("missing-startup-fixture-executable", &[], 120)
                .unwrap_err()
                .to_string()
                .contains("shared cache startup deadline")
        );
        const {
            assert!(
                kvrocks::WORKER_STARTUP_SECONDS
                    >= kvrocks::HOST_STARTUP_SECONDS + 3 * kvrocks::STARTUP_FRAME_SECONDS
            );
        }
    }

    #[test]
    fn startup_frame_accepts_a_valid_plan_with_a_large_controller_lock() {
        use super::super::contract::{VERSION, WORKFLOW, example};
        let mut request = example();
        let head = "a".repeat(40);
        request.pr = None;
        request.revision = Some(head.clone());
        request.expected_head = Some(head.clone());
        request.test_profile = "checks-rebuild-v1".into();
        request.compiler_cache = Some(CompilerCache::KvrocksV1);
        let identity = serde_json::json!({"id": 1, "repository": request.repository, "commit": head, "tree": head});
        let lock = serde_json::json!({"large_controller_lock": "x".repeat(128 * 1024)});
        let mut plan: Plan = serde_json::from_value(serde_json::json!({
            "schema_version": VERSION, "request_id": canonical_digest(&request).unwrap(), "request": request,
            "target": identity, "pr": null, "recipe": null, "controller": identity, "workflow": WORKFLOW,
            "tool_lock": lock, "tool_lock_digest": canonical_digest(&lock).unwrap(),
            "run_repository": "OWNER/REPOSITORY", "run_id": 1, "run_attempt": 1, "digest": "",
        })).unwrap();
        plan.seal().unwrap();
        plan.validate().unwrap();
        let host = HostRequest {
            plan,
            system: "x86_64-linux".into(),
            bootstrap: kvrocks::BootstrapReceipt {
                source: kvrocks::NativeSource {
                    engine_repository: "caniko/simit".into(),
                    engine_revision: "b".repeat(40),
                    nixpkgs_revision: "c".repeat(40),
                    system: "x86_64-linux".into(),
                    version: "2.14.0".into(),
                    installable: format!(
                        "github:NixOS/nixpkgs/{}#legacyPackages.x86_64-linux.kvrocks",
                        "c".repeat(40)
                    ),
                    derivation: format!("/nix/store/{}-kvrocks.drv", "0".repeat(32)),
                    output: format!("/nix/store/{}-kvrocks", "1".repeat(32)),
                },
                nar_hash: format!("sha256:{}", "d".repeat(64)),
                nar_size: 1,
                binary_sha256: "e".repeat(64),
            },
            identity: kvrocks::WorkerIdentity {
                user: "runner".into(),
                uid: 1001,
                group: "nixbld".into(),
                gid: 30000,
            },
        };
        host.service().unwrap();
        let mut frame = serde_json::to_vec(&host).unwrap();
        frame.push(b'\n');
        assert!(frame.len() > 65536);
        let decoded = startup_frame(
            std::io::Cursor::new(frame),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(decoded.plan.digest, host.plan.digest);
        assert_eq!(decoded.service().unwrap(), host.service().unwrap());
    }

    #[test]
    fn failed_allocation_and_replaced_runtime_preserve_foreign_socket_bytes() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        fs::create_dir(&runtime).unwrap();
        let socket = runtime.join("redis.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let metadata = fs::metadata(&runtime).unwrap();
        let identity = kvrocks::WorkerIdentity {
            user: "fixture".into(),
            uid: metadata.uid(),
            group: "fixture".into(),
            gid: metadata.gid(),
        };
        cleanup_socket(None, &socket, &identity, None).unwrap();
        assert!(
            socket.exists(),
            "preflight did not grant foreign socket ownership"
        );
        let owned_path = root.path().join("owned");
        let mut owned = kvrocks::OwnedRuntime::create(&owned_path, &identity).unwrap();
        owned.preserve_on_drop();
        fs::rename(&owned_path, root.path().join("retained")).unwrap();
        fs::rename(&runtime, &owned_path).unwrap();
        assert!(
            cleanup_socket(
                Some(&owned),
                &owned_path.join("redis.sock"),
                &identity,
                None
            )
            .is_err()
        );
        assert!(owned_path.join("redis.sock").exists());
    }

    #[test]
    fn malformed_or_oversized_startup_frames_revoke_the_host_lifetime() {
        for input in [
            b"{}".to_vec(),
            b"{}\n".to_vec(),
            vec![b'x'; kvrocks::HOST_REQUEST_MAX_BYTES as usize + 1],
        ] {
            let cancelled = Arc::new(AtomicBool::new(false));
            assert!(startup_frame(std::io::Cursor::new(input), cancelled.clone()).is_err());
        }
    }
}
