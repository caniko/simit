use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cargo;
use crate::cli::{CoprAction, CoprCommand, CoprRenderArgs};
use crate::config::{CoprOverrides, ProjectConfig, ResolvedCopr};
use crate::render::{copr_makefile, rpm_spec};

pub const MAKEFILE_PATH: &str = ".copr/Makefile";

pub fn run(command: CoprCommand) -> Result<()> {
    match command.action {
        CoprAction::Render(args) => render(args),
    }
}

fn render(args: CoprRenderArgs) -> Result<()> {
    let (resolved, package_version) = resolve(args.copr.as_overrides(), args.package.as_deref())?;
    let version = args.version.as_deref().unwrap_or(&package_version);
    let rendered = render_files(&resolved, version);
    println!("==> {}", resolved.spec_path);
    print!("{}", rendered.spec);
    println!("==> {MAKEFILE_PATH}");
    print!("{}", rendered.makefile);
    Ok(())
}

pub(crate) fn resolve(
    overrides: CoprOverrides<'_>,
    package: Option<&str>,
) -> Result<(ResolvedCopr, String)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let pkg = cargo::representative_package(&metadata, package)?;
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_copr(overrides, &pkg)?;
    Ok((resolved, pkg.version))
}

pub(crate) struct RenderedCopr {
    pub spec: String,
    pub makefile: String,
}

pub(crate) fn render_files(resolved: &ResolvedCopr, version: &str) -> RenderedCopr {
    RenderedCopr {
        spec: rpm_spec::render_spec(resolved, version),
        makefile: copr_makefile::render_makefile(resolved),
    }
}

pub(crate) fn write_files(
    workspace_root: &Path,
    resolved: &ResolvedCopr,
    rendered: &RenderedCopr,
) -> Result<()> {
    let spec_path = workspace_root.join(&resolved.spec_path);
    if let Some(parent) = spec_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&spec_path, &rendered.spec)
        .with_context(|| format!("writing {}", spec_path.display()))?;

    let makefile_path = workspace_root.join(MAKEFILE_PATH);
    if let Some(parent) = makefile_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&makefile_path, &rendered.makefile)
        .with_context(|| format!("writing {}", makefile_path.display()))?;
    Ok(())
}
