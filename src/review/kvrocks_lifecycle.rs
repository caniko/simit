//! Owned hosted Kvrocks lifetime. Privileged operations only use the worker's
//! immutable tool; consumer evaluation and compilation stay unprivileged.

use super::kvrocks::ServicePlan;
pub use super::kvrocks::UnitIdentity;
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

fn properties(value: &str) -> Result<BTreeMap<&str, &str>> {
    ensure!(value.len() <= 64 * 1024, "unit observation is too large");
    let mut result = BTreeMap::new();
    for line in value.lines() {
        let (key, value) = line.split_once('=').context("invalid unit property")?;
        ensure!(
            !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "invalid unit property name"
        );
        ensure!(
            result.insert(key, value).is_none(),
            "duplicate unit property"
        );
    }
    Ok(result)
}

/// A service name is not ownership: bind the live invocation, worker account,
/// cgroup and termination policy before either admission or a later stop.
pub fn unit_identity(plan: &ServicePlan, observation: &str) -> Result<UnitIdentity> {
    let properties = properties(observation)?;
    let get = |key| properties.get(key).copied().unwrap_or("");
    let description = format!("Simit Kvrocks {}", plan.run_identity);
    let cgroup = format!("/system.slice/{}", plan.unit);
    ensure!(
        get("Id") == plan.unit
            && get("Description") == description
            && get("LoadState") == "loaded"
            && get("ActiveState") == "active"
            && get("SubState") == "running"
            && get("User") == plan.identity.user
            && get("Group") == plan.identity.group
            && get("ControlGroup") == cgroup,
        "Kvrocks unit/worker/cgroup differs from the owned service"
    );
    ensure!(
        get("KillMode") == "control-group"
            && get("TimeoutStopUSec") == format!("{}s", plan.shutdown_seconds)
            && get("SendSIGKILL") == "yes"
            && get("KillSignal") == "15"
            && get("FinalKillSignal") == "9"
            && get("LimitNOFILE") == "8192"
            && get("RestrictAddressFamilies") == "AF_UNIX",
        "Kvrocks effective shutdown/address-family policy differs from its plan"
    );
    let invocation_id = get("InvocationID");
    ensure!(
        invocation_id.len() == 32
            && invocation_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid live unit invocation identity"
    );
    let pid: u32 = get("MainPID").parse().context("invalid live service PID")?;
    ensure!(pid > 1, "Kvrocks has no live service process");
    Ok(UnitIdentity {
        invocation_id: invocation_id.into(),
        pid,
        control_group: cgroup,
    })
}

/// A cleanup-eligible failed unit is not evidence of a successful cache lifetime.
/// Require the admitted live invocation both during execution and at normal EOF.
pub fn validate_live_invocation(
    plan: &ServicePlan,
    admitted: &UnitIdentity,
    observation: &str,
) -> Result<()> {
    ensure!(
        unit_identity(plan, observation)? == *admitted,
        "owned cache invocation changed during execution"
    );
    Ok(())
}

/// Observe the kernel's cgroup-v2 limits, rather than treating requested
/// systemd properties as proof of actual resource enforcement.
pub fn validate_limits(plan: &ServicePlan, memory: &str, cpu: &str, pids: &str) -> Result<()> {
    ensure!(
        memory.trim() == plan.memory_max_bytes.to_string()
            && cpu.trim() == format!("{} 100000", u64::from(plan.cpu_quota_percent) * 1000)
            && pids.trim() == "64",
        "Kvrocks effective cgroup resource limits differ from its plan"
    );
    Ok(())
}

/// Shutdown accepts an owned unit that failed during startup, but never a new
/// invocation replacing the one admitted to the consumer build.
pub fn validate_shutdown(
    plan: &ServicePlan,
    prior: Option<&UnitIdentity>,
    observation: &str,
) -> Result<()> {
    let properties = properties(observation)?;
    let get = |key| properties.get(key).copied().unwrap_or("");
    ensure!(
        get("Id") == plan.unit
            && get("Description") == format!("Simit Kvrocks {}", plan.run_identity)
            && get("User") == plan.identity.user
            && get("Group") == plan.identity.group
            && get("KillMode") == "control-group"
            && get("SendSIGKILL") == "yes"
            && get("KillSignal") == "15"
            && get("FinalKillSignal") == "9"
            && get("TimeoutStopUSec") == format!("{}s", plan.shutdown_seconds),
        "unit ownership or graceful termination policy changed; preserve it"
    );
    if let Some(prior) = prior {
        ensure!(
            get("InvocationID") == prior.invocation_id,
            "unit invocation changed; refuse foreign shutdown/cleanup"
        );
    }
    Ok(())
}

/// A launch rejected by systemd, or an exited unit that systemd has collected,
/// leaves no unit to stop. The caller must still prove the retained cgroup empty
/// before removing any allocated state. Loaded/replaced units retain all checks.
pub fn shutdown_required(
    plan: &ServicePlan,
    prior: Option<&UnitIdentity>,
    observation: &str,
) -> Result<bool> {
    let values = properties(observation)?;
    if values.get("LoadState") == Some(&"not-found") {
        ensure!(
            values.get("Id") == Some(&plan.unit.as_str())
                && values.get("MainPID") == Some(&"0")
                && values.get("ControlGroup") == Some(&""),
            "absent unit still has an execution identity; preserve it"
        );
        return Ok(false);
    }
    validate_shutdown(plan, prior, observation)?;
    Ok(true)
}

pub fn supervisor_identity(
    plan: &ServicePlan,
    pid: u32,
    invocation: &str,
    observation: &str,
) -> Result<UnitIdentity> {
    let values = properties(observation)?;
    let get = |key| values.get(key).copied().unwrap_or("");
    let unit = super::kvrocks::supervisor_unit(plan);
    let cgroup = format!("/system.slice/{unit}");
    ensure!(
        pid > 1
            && super::kvrocks::invocation_id(invocation)
            && get("InvocationID") == invocation
            && get("Id") == unit
            && get("MainPID") == pid.to_string()
            && get("ControlGroup") == cgroup
            && get("Description") == format!("Simit Kvrocks supervisor {}", plan.run_identity)
            && get("LoadState") == "loaded"
            && get("ActiveState") == "active"
            && get("SubState") == "running"
            && get("User") == "root"
            && get("Group") == "root"
            && get("KillMode") == "control-group"
            && get("TimeoutStopUSec") == "45s"
            && get("SendSIGKILL") == "yes"
            && get("KillSignal") == "15"
            && get("FinalKillSignal") == "9",
        "cache supervisor is not the owned PID-1-managed root invocation"
    );
    Ok(UnitIdentity {
        invocation_id: invocation.into(),
        pid,
        control_group: cgroup,
    })
}

pub fn validate_process_identity(
    status: &str,
    identity: &super::kvrocks::WorkerIdentity,
) -> Result<()> {
    let mut ids = BTreeMap::new();
    for line in status.lines() {
        if let Some((key, values)) = line.split_once(':')
            && (key == "Uid" || key == "Gid")
        {
            ensure!(
                ids.insert(key, values).is_none(),
                "duplicate process identity observation"
            );
        }
    }
    for (key, expected) in [("Uid", identity.uid), ("Gid", identity.gid)] {
        let values = ids
            .get(key)
            .context("missing process identity observation")?
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            values == vec![expected; 4],
            "service process credentials differ from the actual worker/build group"
        );
    }
    Ok(())
}

/// Every process-owned socket must be in the Unix table. Exactly one may be a
/// listener, at the canonical path. An unknown inode could be an Internet socket
/// and is rejected even when systemd reports RestrictAddressFamilies=AF_UNIX.
pub fn validate_unix_listeners(table: &str, owned: &[u64], socket: &str) -> Result<()> {
    ensure!(table.len() <= 1024 * 1024, "Unix socket table is too large");
    let wanted = owned.iter().copied().collect::<BTreeSet<_>>();
    ensure!(!wanted.is_empty(), "service has no socket descriptors");
    let mut found = BTreeSet::new();
    let mut listeners = 0;
    for line in table.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        ensure!(fields.len() >= 7, "invalid Unix socket table row");
        let inode: u64 = fields[6].parse().context("invalid Unix socket inode")?;
        if !wanted.contains(&inode) {
            continue;
        }
        ensure!(found.insert(inode), "duplicate process socket inode");
        let flags = u32::from_str_radix(fields[3], 16).context("invalid Unix socket flags")?;
        ensure!(fields[4] == "0001", "unexpected process-owned socket type");
        if flags & 0x10000 != 0 {
            ensure!(
                fields.len() == 8 && fields[7] == socket,
                "foreign Kvrocks listener"
            );
            listeners += 1;
        }
    }
    ensure!(
        found == wanted && listeners == 1,
        "Unix-only listener ownership is unproven"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::engine::EngineManifest;
    use crate::review::kvrocks::{self, BootstrapReceipt, WorkerIdentity};
    use std::path::Path;

    fn plan() -> kvrocks::ServicePlan {
        let manifest = EngineManifest {
            schema_version: 1,
            repository: "caniko/simit".into(),
            revision: "a".repeat(40),
            nixpkgs_revision: "b".repeat(40),
        };
        let bootstrap = BootstrapReceipt {
            source: kvrocks::freeze_source(
                &manifest,
                "x86_64-linux",
                &serde_json::json!({
                    "pname": "kvrocks", "version": "2.14.0", "system": "x86_64-linux",
                    "outputs": ["out"],
                    "drvPath": format!("/nix/store/{}-kvrocks.drv", "0".repeat(32)),
                    "outPath": format!("/nix/store/{}-kvrocks", "1".repeat(32)),
                }),
            )
            .unwrap(),
            nar_hash: "sha256-test".into(),
            nar_size: 1,
            binary_sha256: "c".repeat(64),
        };
        kvrocks::service_plan(
            &bootstrap,
            &WorkerIdentity {
                user: "runner".into(),
                uid: 1001,
                group: "nixbld".into(),
                gid: 30000,
            },
            Path::new("/var/lib"),
            &"d".repeat(64),
        )
        .unwrap()
    }

    fn properties(plan: &kvrocks::ServicePlan) -> String {
        format!(
            "Id={}\nDescription=Simit Kvrocks {}\nLoadState=loaded\nActiveState=active\nSubState=running\nInvocationID={}\nMainPID=12345\nControlGroup=/system.slice/{}\nUser=runner\nGroup=nixbld\nKillMode=control-group\nTimeoutStopUSec=30s\nSendSIGKILL=yes\nKillSignal=15\nFinalKillSignal=9\nRestrictAddressFamilies=AF_UNIX\nLimitNOFILE=8192\n",
            plan.unit,
            plan.run_identity,
            "e".repeat(32),
            plan.unit,
        )
    }

    #[test]
    fn final_completion_rejects_failed_owned_units_while_preserving_cleanup() {
        let plan = plan();
        let live = properties(&plan);
        let admitted = unit_identity(&plan, &live).unwrap();
        validate_live_invocation(&plan, &admitted, &live).unwrap();
        for ended in [
            live.replace("ActiveState=active", "ActiveState=failed")
                .replace("SubState=running", "SubState=failed")
                .replace("MainPID=12345", "MainPID=0"),
            live.replace("ActiveState=active", "ActiveState=inactive")
                .replace("SubState=running", "SubState=dead")
                .replace("MainPID=12345", "MainPID=0"),
        ] {
            // An own-unit exit after the last poll still permits graceful cleanup,
            // but EOF cannot acknowledge a successful cache-enabled execution.
            assert!(shutdown_required(&plan, Some(&admitted), &ended).unwrap());
            assert!(validate_live_invocation(&plan, &admitted, &ended).is_err());
        }
        for replaced in [
            live.replace(&admitted.invocation_id, &"f".repeat(32)),
            live.replace("MainPID=12345", "MainPID=54321"),
        ] {
            assert!(validate_live_invocation(&plan, &admitted, &replaced).is_err());
        }
    }

    #[test]
    fn service_readiness_requires_the_owned_unit_invocation_and_effective_limits() {
        let plan = plan();
        assert_eq!(
            plan.control_directory,
            format!("/run/simit-kvrocks-control-{}", plan.run_identity)
        );
        assert!(
            plan.systemd_args
                .contains(&"--property=CPUQuotaPeriodSec=100ms".to_owned())
        );
        assert!(
            plan.systemd_args
                .contains(&"--property=RuntimeMaxSec=21660s".to_owned())
        );
        assert_eq!(
            plan.systemd_args.last().unwrap(),
            &format!("{}/kvrocks.conf", plan.control_directory)
        );
        let identity = unit_identity(&plan, &properties(&plan)).unwrap();
        assert_eq!(identity.pid, 12345);
        assert_eq!(identity.invocation_id, "e".repeat(32));
        validate_limits(&plan, "2147483648\n", "200000 100000\n", "64\n").unwrap();
        for (memory, cpu, pids) in [
            ("max", "200000 100000", "64"),
            ("2147483649", "200000 100000", "64"),
            ("2147483648", "max 100000", "64"),
            ("2147483648", "200000 50000", "64"),
            ("2147483648", "200000 100000", "max"),
            ("2147483648", "200000 100000", "65"),
        ] {
            assert!(validate_limits(&plan, memory, cpu, pids).is_err());
        }
    }

    #[test]
    fn service_readiness_rejects_foreign_units_and_ambiguous_observations() {
        let plan = plan();
        let original = properties(&plan);
        for (old, new) in [
            ("Description=Simit Kvrocks", "Description=foreign"),
            ("ActiveState=active", "ActiveState=inactive"),
            ("MainPID=12345", "MainPID=0"),
            ("User=runner", "User=root"),
            ("Group=nixbld", "Group=root"),
            ("KillMode=control-group", "KillMode=process"),
            ("TimeoutStopUSec=30s", "TimeoutStopUSec=infinity"),
            ("SendSIGKILL=yes", "SendSIGKILL=no"),
            ("LimitNOFILE=8192", "LimitNOFILE=1024"),
            (
                "RestrictAddressFamilies=AF_UNIX",
                "RestrictAddressFamilies=AF_UNIX AF_INET",
            ),
            ("ControlGroup=/system.slice/", "ControlGroup=/../../"),
            ("InvocationID=", "InvocationID=invalid-"),
        ] {
            assert!(
                unit_identity(&plan, &original.replace(old, new)).is_err(),
                "{old}"
            );
        }
        assert!(unit_identity(&plan, &(original.clone() + "MainPID=12345\n")).is_err());
        assert!(unit_identity(&plan, &(original + "bad line\n")).is_err());
    }

    #[test]
    fn shutdown_preserves_a_replaced_invocation_and_accepts_own_failed_startup() {
        let plan = plan();
        let original = properties(&plan);
        let identity = unit_identity(&plan, &original).unwrap();
        let failed = original
            .replace("ActiveState=active", "ActiveState=failed")
            .replace("SubState=running", "SubState=failed")
            .replace("MainPID=12345", "MainPID=0");
        validate_shutdown(&plan, Some(&identity), &failed).unwrap();
        validate_shutdown(&plan, None, &failed).unwrap();
        assert!(
            validate_shutdown(
                &plan,
                Some(&identity),
                &failed.replace(&"e".repeat(32), &"f".repeat(32))
            )
            .is_err()
        );
        let absent = format!(
            "Id={}\nLoadState=not-found\nMainPID=0\nControlGroup=\n",
            plan.unit
        );
        assert!(!shutdown_required(&plan, Some(&identity), &absent).unwrap());
        assert!(
            shutdown_required(
                &plan,
                Some(&identity),
                &absent.replace("MainPID=0", "MainPID=99")
            )
            .is_err()
        );
        assert!(shutdown_required(&plan, Some(&identity), &failed).unwrap());
        assert!(
            validate_shutdown(&plan, None, &failed.replace("User=runner", "User=root")).is_err()
        );
        assert!(
            validate_shutdown(
                &plan,
                Some(&identity),
                &failed.replace("KillMode=control-group", "KillMode=process")
            )
            .is_err()
        );
    }

    #[test]
    fn listener_proof_is_bound_to_process_socket_inodes_and_the_canonical_path() {
        let unix = "Num RefCount Protocol Flags Type St Inode Path\n00000000: 2 0 00010000 0001 01 123 /run/redis-sccache/redis.sock\n00000001: 2 0 00000000 0001 03 456\n";
        validate_unix_listeners(unix, &[123, 456], "/run/redis-sccache/redis.sock").unwrap();
        for (table, inodes) in [
            (unix.to_owned(), vec![123, 999]),
            (unix.to_owned(), vec![456]),
            (unix.replace("redis.sock", "foreign.sock"), vec![123, 456]),
            (
                unix.replace("00000000: 2 0 00010000", "00000000: 2 0 00000000"),
                vec![123, 456],
            ),
            (
                unix.to_owned() + "00000002: 2 0 00010000 0001 01 789 /run/foreign.sock\n",
                vec![123, 456, 789],
            ),
        ] {
            assert!(
                validate_unix_listeners(&table, &inodes, "/run/redis-sccache/redis.sock").is_err()
            );
        }
    }

    #[test]
    fn process_credentials_bind_real_effective_saved_and_filesystem_ids() {
        let identity = plan().identity;
        let status =
            "Name:\tkvrocks\nUid:\t1001\t1001\t1001\t1001\nGid:\t30000\t30000\t30000\t30000\n";
        validate_process_identity(status, &identity).unwrap();
        assert!(validate_process_identity(&status.replace("1001", "0"), &identity).is_err());
        assert!(validate_process_identity(&status.replace("30000", "0"), &identity).is_err());
        assert!(
            validate_process_identity(
                &status.replace("1001\t1001\t1001\t1001", "1001\t0\t1001\t1001"),
                &identity
            )
            .is_err()
        );
        assert!(
            validate_process_identity(
                &(status.to_owned() + "Uid:\t1001\t1001\t1001\t1001\n"),
                &identity
            )
            .is_err()
        );
    }

    #[test]
    fn cancellation_supervisor_is_an_independent_managed_root_invocation() {
        let plan = plan();
        let unit = super::super::kvrocks::supervisor_unit(&plan);
        let original = properties(&plan)
            .replace(&plan.unit, &unit)
            .replace(
                "Description=Simit Kvrocks ",
                "Description=Simit Kvrocks supervisor ",
            )
            .replace("User=runner", "User=root")
            .replace("Group=nixbld", "Group=root")
            .replace("TimeoutStopUSec=30s", "TimeoutStopUSec=45s");
        let invocation = "e".repeat(32);
        supervisor_identity(&plan, 12345, &invocation, &original).unwrap();
        for (old, new) in [
            ("TimeoutStopUSec=45s", "TimeoutStopUSec=10s"),
            ("MainPID=12345", "MainPID=12346"),
            ("User=root", "User=runner"),
            ("KillMode=control-group", "KillMode=process"),
        ] {
            assert!(
                supervisor_identity(&plan, 12345, &invocation, &original.replace(old, new))
                    .is_err()
            );
        }
        let wrapper = format!(
            "/nix/store/{}-simit-review-tools/bin/repo-review",
            "2".repeat(32)
        );
        let args = super::super::kvrocks::supervisor_arguments(
            &plan,
            Path::new(&wrapper),
            plan.identity.uid,
        )
        .unwrap();
        assert!(args.contains(&"--pipe".into()) && args.contains(&"--wait".into()));
        assert!(args.contains(&"--property=TimeoutStopSec=45s".into()));
        assert!(args.contains(&format!("--setenv=SUDO_UID={}", plan.identity.uid)));
        assert!(!args.iter().any(|arg| arg.contains("--replace")));
        assert!(
            super::super::kvrocks::supervisor_arguments(&plan, Path::new(&wrapper), 0).is_err()
        );
        assert!(
            super::super::kvrocks::supervisor_arguments(
                &plan,
                Path::new("/tmp/mutable/bin/repo-review"),
                plan.identity.uid
            )
            .is_err()
        );
    }
}
