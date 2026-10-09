//! Bootstrap the worker-pinned native transport independently of the consumer.
//! This does not start a service or establish socket/sandbox readiness.
use super::{
    artifact,
    contract::{CompilerCache, Plan, runner},
    digest,
    engine::{self, EngineManifest},
    process::{args, nix_args, run},
    write_json,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSource {
    pub engine_repository: String,
    pub engine_revision: String,
    pub nixpkgs_revision: String,
    pub system: String,
    pub installable: String,
    pub version: String,
    pub derivation: String,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapReceipt {
    pub source: NativeSource,
    pub nar_hash: String,
    pub nar_size: u64,
    pub binary_sha256: String,
}

/// The direct controller tool pin owns this package, never the target's lock.
pub fn tool_reference(manifest: &EngineManifest, system: &str) -> Result<String> {
    ensure!(manifest.schema_version == 1, "unsupported engine manifest");
    super::contract::repository(&manifest.repository)?;
    super::contract::sha(&manifest.revision)?;
    super::contract::sha(&manifest.nixpkgs_revision)?;
    runner(system)?;
    ensure!(system.ends_with("-linux"), "Kvrocks requires native Linux");
    Ok(format!(
        "github:NixOS/nixpkgs/{}#legacyPackages.{system}.kvrocks",
        manifest.nixpkgs_revision
    ))
}

/// Freeze the evaluated native derivation before realization. This is also the
/// validation boundary for retained package discovery in later lifecycle receipts.
pub fn freeze_source(
    manifest: &EngineManifest,
    system: &str,
    package: &Value,
) -> Result<NativeSource> {
    let installable = tool_reference(manifest, system)?;
    ensure!(
        package["pname"] == "kvrocks"
            && package["system"] == system
            && package["outputs"] == serde_json::json!(["out"]),
        "Kvrocks bootstrap package/platform/output mismatch"
    );
    let version = package["version"]
        .as_str()
        .context("missing tool version")?;
    ensure!(
        !version.is_empty()
            && version.len() <= 64
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_+".contains(&b)),
        "invalid tool version"
    );
    let derivation = package["drvPath"]
        .as_str()
        .context("missing tool derivation")?;
    let output = package["outPath"].as_str().context("missing tool output")?;
    artifact::store_path(derivation)?;
    artifact::store_path(output)?;
    ensure!(
        derivation.ends_with(".drv") && !output.ends_with(".drv"),
        "Kvrocks bootstrap requires frozen derivation and runtime output"
    );
    Ok(NativeSource {
        engine_repository: manifest.repository.clone(),
        engine_revision: manifest.revision.clone(),
        nixpkgs_revision: manifest.nixpkgs_revision.clone(),
        system: system.into(),
        installable,
        version: version.into(),
        derivation: derivation.into(),
        output: output.into(),
    })
}

/// Realize only the native worker tool, with the same bounded no-IFD/no-builder
/// policy as the engine. The lifecycle owner calls this before target evaluation.
pub fn bootstrap(plan: &Plan, system: &str, out: &Path) -> Result<BootstrapReceipt> {
    plan.validate()?;
    ensure!(
        plan.request.compiler_cache == Some(CompilerCache::KvrocksV1)
            && plan
                .request
                .systems
                .iter()
                .any(|requested| requested == system),
        "Kvrocks bootstrap requires an explicit requested platform/cache profile"
    );
    let manifest = engine::manifest()?;
    engine::verify_lock(&plan.tool_lock, &manifest)?;
    let reference = tool_reference(&manifest, system)?;
    ensure!(!out.exists(), "bootstrap evidence directory must be new");
    let (_, architecture, _) = runner(system)?;
    ensure!(
        run("uname", &args(&["-m"]), None, None)?.trim() == architecture
            && run("nix", &args(&["config", "show", "system"]), None, None)?.trim() == system,
        "Kvrocks bootstrap requires the requested native worker architecture"
    );
    fs::create_dir(out)?;
    let package: Value = serde_json::from_str(&run(
        "nix",
        &nix_args(&[
            "eval",
            "--json",
            "--no-update-lock-file",
            &reference,
            "--apply",
            "p: { inherit (p) pname version system drvPath outPath outputs; }",
        ]),
        None,
        Some(&out.join("discovery.log")),
    )?)?;
    let source = freeze_source(&manifest, system, &package)?;
    write_json(&out.join("source.json"), &source)?;
    let selected = format!("{}^out", source.derivation);
    let built: Value = serde_json::from_str(&run(
        "nix",
        &nix_args(&["build", "--no-link", "--json", "-L", &selected]),
        None,
        Some(&out.join("build.log")),
    )?)?;
    ensure!(
        built.as_array().is_some_and(|items| items.len() == 1)
            && built[0]["drvPath"] == source.derivation
            && built[0]["outputs"] == serde_json::json!({"out": source.output}),
        "realized Kvrocks tool differs from frozen native derivation"
    );
    let infos: Value = serde_json::from_str(&run(
        "nix",
        &nix_args(&["path-info", "--json", &source.output]),
        None,
        Some(&out.join("path-info.log")),
    )?)?;
    let info = if let Some(info) = infos.get(&source.output) {
        info
    } else {
        ensure!(
            infos.as_array().is_some_and(|items| items.len() == 1)
                && infos[0]["path"] == source.output,
            "tool NAR report differs from frozen output"
        );
        &infos[0]
    };
    let nar_hash = info["narHash"].as_str().context("missing tool NAR hash")?;
    let nar_size = info["narSize"].as_u64().context("missing tool NAR size")?;
    ensure!(
        (nar_hash.starts_with("sha256:") || nar_hash.starts_with("sha256-"))
            && nar_hash.len() <= 80
            && nar_size > 0,
        "invalid tool NAR identity"
    );
    let output = Path::new(&source.output).canonicalize()?;
    let binary = output.join("bin/kvrocks").canonicalize()?;
    ensure!(
        binary.starts_with(&output),
        "tool binary escapes its output"
    );
    let metadata = fs::metadata(&binary)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 256 * 1024 * 1024,
        "invalid native tool binary"
    );
    let receipt = BootstrapReceipt {
        source,
        nar_hash: nar_hash.into(),
        nar_size,
        binary_sha256: digest(&fs::read(binary)?),
    };
    write_json(&out.join("bootstrap.json"), &receipt)?;
    Ok(receipt)
}
