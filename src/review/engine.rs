//! The immutable packaged engine manifest must agree with the controller's frozen lock.
use super::{
    contract::{Plan, repository, sha},
    read_json,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineManifest {
    pub schema_version: u32,
    pub repository: String,
    pub revision: String,
    pub nixpkgs_revision: String,
}

pub fn manifest() -> Result<EngineManifest> {
    let path = std::env::var_os("SIMIT_REVIEW_ENGINE_MANIFEST")
        .context("review execution requires the controller's pinned Nix review-tools package")?;
    let manifest: EngineManifest = read_json(Path::new(&path))?;
    ensure!(manifest.schema_version == 1, "unsupported engine manifest");
    repository(&manifest.repository)?;
    sha(&manifest.revision)?;
    sha(&manifest.nixpkgs_revision)?;
    Ok(manifest)
}

pub fn verify_lock(lock: &Value, manifest: &EngineManifest) -> Result<()> {
    ensure!(manifest.schema_version == 1, "unsupported engine manifest");
    repository(&manifest.repository)?;
    sha(&manifest.revision)?;
    sha(&manifest.nixpkgs_revision)?;
    ensure!(lock["version"] == 7, "unsupported controller lock");
    let root = lock["root"]
        .as_str()
        .context("missing controller lock root")?;
    let locked_input = |input: &str| -> Result<&Value> {
        let node = lock["nodes"][root]["inputs"][input]
            .as_str()
            .context("review controller requires direct locked simit and nixpkgs inputs")?;
        Ok(&lock["nodes"][node]["locked"])
    };
    let engine = locked_input("simit")?;
    ensure!(
        engine["type"] == "github"
            && engine["rev"] == manifest.revision
            && format!(
                "{}/{}",
                engine["owner"].as_str().unwrap_or(""),
                engine["repo"].as_str().unwrap_or("")
            ) == manifest.repository,
        "running engine differs from controller's locked Simit identity"
    );
    let nixpkgs = locked_input("nixpkgs")?;
    ensure!(
        nixpkgs["type"] == "github"
            && nixpkgs["owner"] == "NixOS"
            && nixpkgs["repo"] == "nixpkgs"
            && nixpkgs["rev"] == manifest.nixpkgs_revision,
        "running tool packages differ from controller's locked Nixpkgs"
    );
    Ok(())
}

pub fn verify_plan(plan: &Plan) -> Result<()> {
    plan.validate()?;
    verify_lock(&plan.tool_lock, &manifest()?)
}
