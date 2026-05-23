use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::git;
use crate::render::diff::unified_diff;

pub fn run_git(target: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .current_dir(target)
        .args(args)
        .status()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

pub fn shell_word(value: &str) -> String {
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

pub fn redact_url(url: &str) -> String {
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

pub fn prepare_target(target: &Path, no_git: bool) -> Result<()> {
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

pub struct ArtifactCheck<'a> {
    pub label: &'a str,
    pub path: &'a Path,
    pub expected: &'a str,
    pub remediation: &'a str,
}

impl ArtifactCheck<'_> {
    pub fn verify(&self, show_diff: bool) -> Result<()> {
        match fs::read_to_string(self.path) {
            Ok(actual) if actual == self.expected => Ok(()),
            Ok(actual) if show_diff => bail!(
                "{} is not up to date; {}:\n{} differs\n{}",
                self.label,
                self.remediation,
                self.path.display(),
                unified_diff(&self.path.display().to_string(), &actual, self.expected)
            ),
            Ok(_) => bail!(
                "{} is not up to date; {}:\n{} differs",
                self.label,
                self.remediation,
                self.path.display()
            ),
            Err(err) if err.kind() == ErrorKind::NotFound => bail!(
                "{} is not up to date; {}:\n{} is missing",
                self.label,
                self.remediation,
                self.path.display()
            ),
            Err(err) => Err(err).with_context(|| format!("reading {}", self.path.display())),
        }
    }
}

pub struct WriteArtifact<'a> {
    pub path: &'a Path,
    pub contents: &'a str,
}

impl WriteArtifact<'_> {
    pub fn commit(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(self.path, self.contents)
            .with_context(|| format!("writing {}", self.path.display()))
    }
}

pub fn bootstrap_repo(
    target: &Path,
    remote_url: &str,
    initial_branch: &str,
    file_to_stage: &str,
) -> Result<()> {
    let git_dir = target.join(".git");
    if !git_dir.exists() {
        fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
        run_git(target, &["init"])?;
        run_git(target, &["checkout", "-B", initial_branch])?;
    } else if !git_dir.is_dir() {
        bail!(
            ".git in {} is not a directory; refusing to proceed",
            target.display()
        );
    }

    match git::output(target, &["remote", "get-url", "origin"]) {
        Ok(url) if url.trim() == remote_url => {}
        Ok(url) if url.trim().is_empty() => {
            run_git(target, &["remote", "add", "origin", remote_url])?;
        }
        Ok(url) => {
            eprintln!(
                "warning: origin already configured to {}; leaving as-is. To switch: git -C {} remote set-url origin {}",
                redact_url(url.trim()),
                shell_word(&target.display().to_string()),
                shell_word(&redact_url(remote_url))
            );
        }
        Err(_) => {
            run_git(target, &["remote", "add", "origin", remote_url])?;
        }
    }

    run_git(target, &["add", "--", file_to_stage])
}

pub fn print_next_steps(target: &Path, headline: &str, lines: &[String]) {
    println!("{headline} at {}.", target.display());
    println!();
    println!("Next steps:");
    for line in lines {
        println!("  {line}");
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CheckPrintMode {
    Write,
    Check { diff: bool },
    Print,
}

impl CheckPrintMode {
    pub fn parse(command_name: &str, check: bool, print: bool, diff: bool) -> Result<Self> {
        if check && print {
            bail!("{command_name} accepts only one of --check or --print");
        }
        if diff && !check {
            bail!("{command_name} --diff requires --check");
        }
        if print {
            Ok(Self::Print)
        } else if check {
            Ok(Self::Check { diff })
        } else {
            Ok(Self::Write)
        }
    }
}

pub struct BumpFlow<'a> {
    pub repo: &'a Path,
    pub artifact: WriteArtifact<'a>,
    pub staged_path: &'a str,
    pub working_tree_label: &'a str,
    pub default_branch_hint: &'a str,
}

impl BumpFlow<'_> {
    pub fn write(&self) -> Result<()> {
        self.artifact.commit()
    }

    pub fn push(&self, commit_message: &str) -> Result<()> {
        self.guard_clean_for_push()?;
        run_git(self.repo, &["add", "--", self.staged_path])?;

        let status = git::output(
            self.repo,
            &["status", "--porcelain", "--", self.staged_path],
        )?;
        let default_branch = if status.trim().is_empty()
            && last_commit_subject(self.repo).as_deref() == Some(commit_message)
        {
            return Ok(());
        } else {
            origin_default_branch(self.repo, self.default_branch_hint)?
        };

        if status.trim().is_empty() {
            run_git(
                self.repo,
                &["commit", "--allow-empty", "-m", commit_message],
            )?;
        } else {
            run_git(self.repo, &["commit", "-m", commit_message])?;
        }
        run_git(
            self.repo,
            &["push", "origin", &format!("HEAD:{default_branch}")],
        )
    }

    fn guard_clean_for_push(&self) -> Result<()> {
        let dirty = git::output(self.repo, &["status", "--porcelain"])?;
        let unrelated = dirty
            .lines()
            .filter(|line| !status_line_is_for_path(line, self.staged_path))
            .collect::<Vec<_>>();
        if !unrelated.is_empty() {
            bail!(
                "{} working tree has unrelated changes:\n{}",
                self.working_tree_label,
                unrelated.join("\n")
            );
        }
        Ok(())
    }
}

fn status_line_is_for_path(line: &str, path: &str) -> bool {
    line.get(3..) == Some(path)
        || line
            .split_once(" -> ")
            .is_some_and(|(_, target)| target == path)
}

fn origin_default_branch(repo: &Path, hint: &str) -> Result<String> {
    let _ = git::output(repo, &["remote", "set-head", "origin", "-a"]);
    let symbolic = git::output(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .with_context(|| {
        format!(
            "detecting origin default branch; run `git -C <{hint}> remote set-head origin -a` locally and retry"
        )
    })?;
    symbolic
        .trim()
        .strip_prefix("origin/")
        .map(str::to_string)
        .filter(|branch| !branch.is_empty())
        .context("origin/HEAD did not resolve to origin/<branch>")
}

fn last_commit_subject(repo: &Path) -> Option<String> {
    git::output(repo, &["log", "-1", "--pretty=%s"])
        .ok()
        .map(|subject| subject.trim().to_owned())
}
