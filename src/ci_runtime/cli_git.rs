//! Read-only source probes use protected platform Git, never the job's PATH.
use anyhow::{Result, bail};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(unix)]
fn protected(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    let Ok(executable) = std::fs::metadata(&canonical) else {
        return false;
    };
    if !executable.is_file() || executable.mode() & 0o111 == 0 {
        return false;
    }
    // Check both the installed profile/symlink chain and its resolved binary.
    // A job-owned executable or writable parent cannot become source authority.
    [path, canonical.as_path()].into_iter().all(|path| {
        path.ancestors().all(|ancestor| {
            std::fs::symlink_metadata(ancestor).is_ok_and(|metadata| {
                metadata.uid() == 0
                    && (metadata.file_type().is_symlink() || metadata.mode() & 0o022 == 0
                // Nix's root-owned sticky store directory can be group-writable;
                // the root-owned descendants checked here cannot be replaced.
                || (metadata.is_dir() && metadata.mode() & 0o1000 != 0))
            })
        })
    })
}

pub(super) fn executable() -> Result<PathBuf> {
    #[cfg(unix)]
    {
        let mut candidates = [
            "/usr/bin/git",
            "/bin/git",
            "/run/current-system/sw/bin/git",
            "/nix/var/nix/profiles/default/bin/git",
        ]
        .map(PathBuf::from)
        .to_vec();
        // NixOS may expose Git only through administrator-installed user
        // profiles. Enumerate that fixed protected directory, not environment.
        if let Ok(entries) = std::fs::read_dir("/etc/profiles/per-user") {
            let mut profiles = entries
                .take(64)
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path().join("bin/git"))
                .collect::<Vec<_>>();
            profiles.sort();
            candidates.extend(profiles);
        }
        for candidate in candidates {
            if protected(&candidate) {
                return Ok(std::fs::canonicalize(candidate)?);
            }
        }
    }
    #[cfg(windows)]
    {
        // Supported GitHub-hosted Windows images install Git under the protected
        // Program Files directory on C:. No mutable ProgramFiles/PATH lookup.
        for candidate in [
            r"C:\Program Files\Git\cmd\git.exe",
            r"C:\Program Files\Git\bin\git.exe",
        ] {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return Ok(std::fs::canonicalize(path)?);
            }
        }
    }
    bail!("trusted platform Git executable unavailable for source-owned observer spec")
}
