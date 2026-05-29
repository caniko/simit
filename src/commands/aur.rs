use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cargo;
use crate::cli::{AurAction, AurCommand, AurRenderArgs};
use crate::config::{AurOverrides, ProjectConfig, ResolvedAur};
use crate::render::pkgbuild::{self, RenderedPkgbuild};

pub fn run(command: AurCommand) -> Result<()> {
    match command.action {
        AurAction::Render(args) => render(args),
    }
}

fn render(args: AurRenderArgs) -> Result<()> {
    let (resolved, package_version) = resolve(args.aur.as_overrides(), args.package.as_deref())?;
    let version = args.version.as_deref().unwrap_or(&package_version);
    for flavor in pkgbuild::render(&resolved, version) {
        println!("==> dist/aur/{}/PKGBUILD", flavor.pkgname);
        print!("{}", flavor.pkgbuild);
    }
    Ok(())
}

pub(crate) fn resolve(
    overrides: AurOverrides<'_>,
    package: Option<&str>,
) -> Result<(ResolvedAur, String)> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let pkg = cargo::representative_package(&metadata, package)?;
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_aur(overrides, &pkg)?;
    Ok((resolved, pkg.version))
}

/// PKGBUILD path under the workspace root for a rendered flavor.
pub(crate) fn pkgbuild_path(workspace_root: &Path, rendered: &RenderedPkgbuild) -> PathBuf {
    workspace_root
        .join("dist")
        .join("aur")
        .join(&rendered.pkgname)
        .join("PKGBUILD")
}

pub(crate) fn write_flavors(workspace_root: &Path, flavors: &[RenderedPkgbuild]) -> Result<()> {
    for flavor in flavors {
        let path = pkgbuild_path(workspace_root, flavor);
        let parent = path.parent().expect("PKGBUILD path has a parent");
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        fs::write(&path, &flavor.pkgbuild)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
