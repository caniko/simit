use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::InitHomebrewTapCommand;
use crate::config::{ProjectConfig, ResolvedHomebrew};
use crate::git;
use crate::render::diff::unified_diff;
use crate::render::homebrew_formula::{self, Sha256Set};

pub fn run(command: InitHomebrewTapCommand) -> Result<()> {
    if command.check && command.print {
        bail!("init-homebrew-tap accepts only one of --check or --print");
    }
    if command.diff && !command.check {
        bail!("init-homebrew-tap --diff requires --check");
    }

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let package = cargo::select_packages(&metadata, &[], false)?
        .into_iter()
        .next()
        .expect("single package selected");
    let cfg = ProjectConfig::load(workspace_root)?;
    let resolved = cfg.resolve_homebrew(command.homebrew.as_overrides(), &package)?;
    let formula_text =
        homebrew_formula::render(&resolved, &package.version, &Sha256Set::all_no_check());

    if command.print {
        print!("{formula_text}");
        return Ok(());
    }

    let target = command.target.as_std_path();
    let formula_path = formula_path(target, &resolved.name);

    if command.check {
        return check_formula_matches(&formula_path, &formula_text, command.diff);
    }

    prepare_target(target, command.no_git)?;
    let formula_dir = formula_path
        .parent()
        .expect("formula path always has a parent directory");
    fs::create_dir_all(formula_dir)
        .with_context(|| format!("creating {}", formula_dir.display()))?;
    fs::write(&formula_path, formula_text)
        .with_context(|| format!("writing {}", formula_path.display()))?;

    if !command.no_git {
        bootstrap_git(target, &resolved.tap_url, &resolved.name)?;
    }

    print_next_steps(target, &resolved);
    Ok(())
}

fn formula_path(target: &Path, name: &str) -> PathBuf {
    target.join("Formula").join(format!("{name}.rb"))
}

fn prepare_target(target: &Path, no_git: bool) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    if !target.is_dir() {
        bail!("target {} exists and is not a directory", target.display());
    }
    if no_git || target.join(".git").is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(target)
        .with_context(|| format!("reading target directory {}", target.display()))?;
    if entries.next().transpose()?.is_some() {
        bail!(
            "target {} exists and is not a git repo; refusing to overwrite",
            target.display()
        );
    }
    Ok(())
}

fn check_formula_matches(formula_path: &Path, expected: &str, show_diff: bool) -> Result<()> {
    match fs::read_to_string(formula_path) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(actual) if show_diff => bail!(
            "Homebrew formula is not up to date; run `simit init-homebrew-tap --target {}`:\n{} differs\n{}",
            formula_path
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new("."))
                .display(),
            formula_path.display(),
            unified_diff(&formula_path.display().to_string(), &actual, expected)
        ),
        Ok(_) => bail!(
            "Homebrew formula is not up to date; run `simit init-homebrew-tap --target {}`:\n{} differs",
            formula_path
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new("."))
                .display(),
            formula_path.display()
        ),
        Err(err) if err.kind() == ErrorKind::NotFound => bail!(
            "Homebrew formula is not up to date; run `simit init-homebrew-tap --target {}`:\n{} is missing",
            formula_path
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new("."))
                .display(),
            formula_path.display()
        ),
        Err(err) => Err(err).with_context(|| format!("reading {}", formula_path.display())),
    }
}

fn bootstrap_git(target: &Path, tap_url: &str, formula_name: &str) -> Result<()> {
    let git_dir = target.join(".git");
    if !git_dir.exists() {
        fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
        run_git(target, ["init"])?;
        run_git(target, ["checkout", "-B", "trunk"])?;
    } else if !git_dir.is_dir() {
        bail!(
            ".git in {} is not a directory; refusing to proceed",
            target.display()
        );
    }

    match git::output(target, &["remote", "get-url", "origin"]) {
        Ok(url) if url.trim() == tap_url => {}
        Ok(url) if url.trim().is_empty() => {
            run_git(target, ["remote", "add", "origin", tap_url])?;
        }
        Ok(url) => {
            eprintln!(
                "warning: origin already configured to {}; leaving as-is. To switch: git -C {} remote set-url origin {}",
                redact_url(url.trim()),
                shell_word(&target.display().to_string()),
                shell_word(&redact_url(tap_url))
            );
        }
        Err(_) => {
            run_git(target, ["remote", "add", "origin", tap_url])?;
        }
    }

    let formula = format!("Formula/{formula_name}.rb");
    run_git(target, ["add", "--", formula.as_str()])
}

fn run_git<'a, I>(target: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    let status = Command::new("git")
        .current_dir(target)
        .args(&args)
        .status()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

fn print_next_steps(target: &Path, resolved: &ResolvedHomebrew) {
    println!("Initialised tap at {}.", target.display());
    println!();
    println!("Next steps:");
    println!(
        "  git -C {} commit -m {}",
        shell_word(&target.display().to_string()),
        shell_word(&format!("Initial {} formula", resolved.name))
    );
    println!(
        "  git -C {} push -u origin trunk",
        shell_word(&target.display().to_string())
    );
    println!();
    println!(
        "Use your tap repo's default branch instead of trunk if it already uses another convention."
    );
}

fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };
    let auth_start = scheme_end + 3;
    let Some(path_start) = url[auth_start..].find('/') else {
        return url.to_owned();
    };
    let path_start = auth_start + path_start;
    let auth = &url[auth_start..path_start];
    if let Some(at) = auth.rfind('@') {
        format!("{}://{}", &url[..scheme_end], &url[auth_start + at + 1..])
    } else {
        url.to_owned()
    }
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
