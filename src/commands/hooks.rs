use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::cargo;
use crate::cli::HooksInstallCommand;
use crate::render::diff::unified_diff;

const HOOK_TYPES: &[&str] = &["pre-commit", "pre-push", "commit-msg"];

pub fn install(command: HooksInstallCommand) -> Result<()> {
    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    install_in_workspace(workspace_root, command)
}

fn install_in_workspace(workspace_root: &Path, command: HooksInstallCommand) -> Result<()> {
    ensure_pre_commit_config(workspace_root)?;
    let git_common_dir = git_common_dir(workspace_root)?;
    let hooks_dir = git_common_dir.join("hooks");
    let effective_hooks_path = effective_hooks_path(workspace_root, &hooks_dir)?;
    warn_if_git_will_not_execute_installed_hooks(
        &git_common_dir,
        &hooks_dir,
        &effective_hooks_path,
    );

    if command.check || command.diff {
        check_hooks(workspace_root, &hooks_dir, command.diff)?;
        return check_hook_route(workspace_root, &git_common_dir, &hooks_dir);
    }

    run_pre_commit_install(
        workspace_root,
        None,
        install_hooks_envs(workspace_root),
        false,
    )?;
    repair_local_hooks_path_override(workspace_root)?;
    check_hooks(workspace_root, &hooks_dir, false)?;
    check_hook_route(workspace_root, &git_common_dir, &hooks_dir)?;
    Ok(())
}

fn ensure_pre_commit_config(workspace_root: &Path) -> Result<()> {
    if workspace_root.join("nix/pre-commit.nix").exists()
        || workspace_root.join(".pre-commit-config.yaml").exists()
        || workspace_root.join(".pre-commit-config.yml").exists()
    {
        Ok(())
    } else {
        bail!("no pre-commit configuration found; run `simit init flake` first");
    }
}

fn install_hooks_envs(workspace_root: &Path) -> bool {
    workspace_root.join(".pre-commit-config.yaml").exists()
        || workspace_root.join(".pre-commit-config.yml").exists()
}

fn git_common_dir(workspace_root: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .with_context(|| {
            format!(
                "resolving git hooks directory for {}",
                workspace_root.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "could not resolve git hooks directory for {}; is this a git repository?",
            workspace_root.display()
        );
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(resolve_path(workspace_root, &path))
}

fn effective_hooks_path(workspace_root: &Path, hooks_dir: &Path) -> Result<PathBuf> {
    Ok(
        hooks_path_config(workspace_root, &["--get", "core.hooksPath"])?
            .unwrap_or_else(|| hooks_dir.to_path_buf()),
    )
}

fn local_hooks_path(workspace_root: &Path) -> Result<Option<PathBuf>> {
    hooks_path_config(workspace_root, &["--local", "--get", "core.hooksPath"])
}

fn global_hooks_path(workspace_root: &Path) -> Result<Option<PathBuf>> {
    hooks_path_config(workspace_root, &["--global", "--get", "core.hooksPath"])
}

fn hooks_path_config(workspace_root: &Path, args: &[&str]) -> Result<Option<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("config")
        .args(args)
        .output()
        .with_context(|| format!("resolving core.hooksPath for {}", workspace_root.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!path.is_empty()).then(|| resolve_path(workspace_root, &expand_home(&path))))
}

fn warn_if_git_will_not_execute_installed_hooks(
    git_common_dir: &Path,
    hooks_dir: &Path,
    effective_hooks_path: &Path,
) {
    if effective_hooks_path == hooks_dir
        || effective_hooks_path.starts_with(git_common_dir)
        || effective_hooks_path.join("dispatched-by-canix").exists()
    {
        return;
    }
    eprintln!(
        "warning: installing hooks into {}, but this repository's effective core.hooksPath is {}; git will execute that directory instead unless a dispatcher chains project hooks",
        hooks_dir.display(),
        effective_hooks_path.display()
    );
}

fn run_pre_commit_install(
    workspace_root: &Path,
    hooks_dir: Option<&Path>,
    install_hooks: bool,
    quiet: bool,
) -> Result<()> {
    let mut command = Command::new("pre-commit");
    command
        .current_dir(workspace_root)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env(
            "GIT_CONFIG_VALUE_0",
            hooks_dir.unwrap_or_else(|| Path::new("")),
        )
        .args([
            "install",
            "--allow-missing-config",
            "--hook-type",
            "pre-commit",
            "--hook-type",
            "pre-push",
            "--hook-type",
            "commit-msg",
            "--overwrite",
        ]);
    if install_hooks {
        command.arg("--install-hooks");
    }
    if quiet {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }

    let status = command
        .status()
        .with_context(|| "running pre-commit install; ensure `pre-commit` is installed")?;
    if !status.success() {
        bail!("pre-commit install failed");
    }
    Ok(())
}

fn check_hooks(workspace_root: &Path, hooks_dir: &Path, show_diff: bool) -> Result<()> {
    let desired_dir = desired_hooks(workspace_root)?;
    let mut mismatches = Vec::new();
    let mut diffs = Vec::new();

    for hook_type in HOOK_TYPES {
        let path = hooks_dir.join(hook_type);
        let expected_path = desired_dir.path().join(".git/hooks").join(hook_type);
        let expected = fs::read_to_string(&expected_path)
            .with_context(|| format!("reading generated {}", expected_path.display()))?;
        match fs::read_to_string(&path) {
            Ok(actual) if actual == expected && is_executable(&path) => {}
            Ok(actual) => {
                if actual != expected {
                    mismatches.push(format!("{} differs", path.display()));
                    if show_diff {
                        diffs.push(unified_diff(
                            &path.display().to_string(),
                            &actual,
                            &expected,
                        ));
                    }
                } else {
                    mismatches.push(format!("{} is not executable", path.display()));
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                mismatches.push(format!("{} is missing", path.display()));
                if show_diff {
                    diffs.push(unified_diff(&path.display().to_string(), "", &expected));
                }
            }
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else if diffs.is_empty() {
        bail!(
            "installed hooks are not up to date; run `simit hooks install`:\n{}",
            mismatches.join("\n")
        );
    } else {
        bail!(
            "installed hooks are not up to date; run `simit hooks install`:\n{}\n{}",
            mismatches.join("\n"),
            diffs.join("\n")
        );
    }
}

fn repair_local_hooks_path_override(workspace_root: &Path) -> Result<()> {
    let Some(local) = local_hooks_path(workspace_root)? else {
        return Ok(());
    };
    if is_canix_dispatcher(&local) {
        return Ok(());
    }
    let Some(global) = global_hooks_path(workspace_root)? else {
        return Ok(());
    };
    if !is_canix_dispatcher(&global) {
        return Ok(());
    }

    let status = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(["config", "--local", "--unset-all", "core.hooksPath"])
        .status()
        .with_context(|| {
            format!(
                "unsetting local core.hooksPath for {}",
                workspace_root.display()
            )
        })?;
    if !status.success() {
        bail!(
            "could not unset local core.hooksPath for {}; git exited with {status}",
            workspace_root.display()
        );
    }
    eprintln!(
        "repaired local core.hooksPath override: unset {} so git uses canix dispatcher {}",
        local.display(),
        global.display()
    );
    Ok(())
}

fn check_hook_route(workspace_root: &Path, git_common_dir: &Path, hooks_dir: &Path) -> Result<()> {
    let local = local_hooks_path(workspace_root)?;
    let global = global_hooks_path(workspace_root)?;
    let global_canix_dispatcher = global.as_deref().is_some_and(is_canix_dispatcher);
    let effective = effective_hooks_path(workspace_root, hooks_dir)?;

    if let Some(local) = local.as_deref() {
        if is_canix_dispatcher(local) {
            return Ok(());
        }
        if global_canix_dispatcher {
            bail!(
                "local core.hooksPath {} bypasses canix dispatcher {}; run `simit hooks install` to repair it",
                local.display(),
                global.as_ref().unwrap().display()
            );
        }
        if local != hooks_dir {
            bail!(
                "core.hooksPath {} does not use this repository's .git/hooks and is not a canix dispatcher",
                local.display()
            );
        }
        return Ok(());
    }

    if is_canix_dispatcher(&effective)
        || effective == hooks_dir
        || effective.starts_with(git_common_dir)
    {
        Ok(())
    } else {
        bail!(
            "effective core.hooksPath {} does not use this repository's .git/hooks and is not a canix dispatcher",
            effective.display()
        )
    }
}

fn is_canix_dispatcher(path: &Path) -> bool {
    path.join("dispatched-by-canix").exists()
}

fn desired_hooks(workspace_root: &Path) -> Result<TempWorkspace> {
    let desired = TempWorkspace::new().context("creating temporary hooks workspace")?;
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(desired.path())
        .stdout(Stdio::null())
        .status()
        .context("initializing temporary hooks workspace")?;
    if !status.success() {
        bail!("git init failed while preparing expected hooks");
    }

    for config_name in [".pre-commit-config.yaml", ".pre-commit-config.yml"] {
        let source = workspace_root.join(config_name);
        if source.exists() {
            fs::copy(&source, desired.path().join(config_name))
                .with_context(|| format!("copying {}", source.display()))?;
            break;
        }
    }

    run_pre_commit_install(desired.path(), None, false, true)?;
    Ok(desired)
}

fn resolve_path(workspace_root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}

fn expand_home(path: &str) -> String {
    if path == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| path.to_owned());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new() -> Result<Self> {
        let mut path = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before UNIX epoch")?
            .as_nanos();
        path.push(format!("simit-hooks-{nonce}-{}", std::process::id()));
        fs::create_dir(&path).with_context(|| format!("creating {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
