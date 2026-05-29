use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cargo;
use crate::cli::{AptAction, AptCommand, AptRenderArgs};
use crate::config::{AptOverrides, ProjectConfig, ResolvedApt};
use crate::render::apt_conf;

pub const DISTRIBUTIONS_PATH: &str = "dist/apt/conf/distributions";

pub fn run(command: AptCommand) -> Result<()> {
    match command.action {
        AptAction::Render(args) => render(args),
    }
}

fn render(args: AptRenderArgs) -> Result<()> {
    let resolved = resolve(args.apt.as_overrides(), args.package.as_deref())?;
    print!("{}", apt_conf::render_distributions(&resolved));
    Ok(())
}

pub(crate) fn resolve(overrides: AptOverrides<'_>, package: Option<&str>) -> Result<ResolvedApt> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let pkg = cargo::representative_package(&metadata, package)?;
    let cfg = ProjectConfig::load(workspace_root)?;
    cfg.resolve_apt(overrides, &pkg)
}

pub(crate) fn write_distributions(workspace_root: &Path, resolved: &ResolvedApt) -> Result<()> {
    let path = workspace_root.join(DISTRIBUTIONS_PATH);
    let parent = path.parent().expect("distributions path has a parent");
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    fs::write(&path, apt_conf::render_distributions(resolved))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
