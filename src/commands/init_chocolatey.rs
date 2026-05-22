use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::InitChocolateyCommand;
use crate::commands::chocolatey;
use crate::render::chocolatey_nuspec::Sha256Set;
use crate::render::diff::unified_diff;

pub fn run(command: InitChocolateyCommand) -> Result<()> {
    if command.check && command.print {
        bail!("init-chocolatey accepts only one of --check or --print");
    }
    if command.diff && !command.check {
        bail!("init-chocolatey --diff requires --check");
    }

    let (resolved, package_version) = chocolatey::resolve(command.chocolatey.as_overrides())?;
    chocolatey::validate_resolved(&resolved)?;
    let package = chocolatey::render_package(
        &resolved,
        &package_version,
        false,
        &Sha256Set::all_no_check(),
    );

    if command.print {
        chocolatey::print_package(&package);
        return Ok(());
    }

    let target = command.target.as_std_path();
    if command.check {
        return check_package_matches(target, &package, command.diff);
    }

    chocolatey::write_package(target, &package)?;
    print_next_steps(target, &resolved.id);
    Ok(())
}

fn check_package_matches(
    target: &Path,
    package: &crate::render::chocolatey_nuspec::RenderedPackage,
    show_diff: bool,
) -> Result<()> {
    let expected = [
        (target.join(&package.nuspec_name), package.nuspec.as_str()),
        (
            target.join("tools").join("chocolateyInstall.ps1"),
            package.install_script.as_str(),
        ),
        (
            target.join("tools").join("chocolateyUninstall.ps1"),
            package.uninstall_script.as_str(),
        ),
    ];

    for (path, text) in expected {
        check_file_matches(&path, text, show_diff)?;
    }
    Ok(())
}

fn check_file_matches(path: &Path, expected: &str, show_diff: bool) -> Result<()> {
    match fs::read_to_string(path) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(actual) if show_diff => bail!(
            "Chocolatey package is not up to date; run `simit init-chocolatey --target {}`:\n{} differs\n{}",
            package_root(path).display(),
            path.display(),
            unified_diff(&path.display().to_string(), &actual, expected)
        ),
        Ok(_) => bail!(
            "Chocolatey package is not up to date; run `simit init-chocolatey --target {}`:\n{} differs",
            package_root(path).display(),
            path.display()
        ),
        Err(err) if err.kind() == ErrorKind::NotFound => bail!(
            "Chocolatey package is not up to date; run `simit init-chocolatey --target {}`:\n{} is missing",
            package_root(path).display(),
            path.display()
        ),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

fn package_root(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some("chocolateyInstall.ps1")
        || path.file_name().and_then(|name| name.to_str()) == Some("chocolateyUninstall.ps1")
    {
        path.parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

fn print_next_steps(target: &Path, id: &str) {
    println!("Initialised Chocolatey package at {}.", target.display());
    println!();
    println!("Next steps:");
    println!("  choco pack {}", shell_word(&target.display().to_string()));
    println!(
        "  simit chocolatey bump --version <version> --package-dir {} --archive x64=<archive.zip>",
        shell_word(&target.display().to_string())
    );
    println!();
    println!("Package id: {id}");
}

fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        value.to_owned()
    } else {
        let mut quoted = String::from("'");
        quoted.push_str(&value.replace('\'', "'\\''"));
        quoted.push('\'');
        quoted
    }
}
