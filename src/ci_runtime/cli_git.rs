//! Read-only source probes use protected platform Git, never the job's PATH.
use anyhow::{Result, bail};
use std::path::PathBuf;

pub(super) fn executable() -> Result<PathBuf> {
    // This is baked into the artifact by its packaging derivation, never read
    // from the job environment. Nix sandboxes have no system Git installation.
    if let Some(path) = option_env!("SIMIT_TRUSTED_GIT") {
        let path = PathBuf::from(path);
        anyhow::ensure!(path.is_absolute(), "packaged trusted Git must be absolute");
        if path.is_file() {
            return Ok(path);
        }
        // Standalone release binaries can run outside their build's Nix store;
        // only the protected platform locations below can provide a fallback.
    }
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
            if super::super::trusted_platform::protected_file(&candidate) {
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
