//! Repository-review engine migrated from caniko/nixpkgs-review-gha at
//! f7bad6b95787f752cedbf50e09c5bf4ae0f39a07. Original fork attribution is retained
//! in assets/LICENSE; the controller deployment remains independently pinned.
pub mod artifact;
pub mod backend;
pub mod cli;
pub mod contract;
pub mod engine;
pub mod generation;
pub mod github;
pub mod process;
pub mod service;

use anyhow::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// serde_json's default sorted maps give one canonical representation recursively.
pub fn canonical_digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&serde_json::to_value(value)?)?))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && meta.len() <= 16 * 1024 * 1024,
        "invalid JSON file"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}
