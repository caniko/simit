use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::cargo::{self, Package};
use crate::changelog;
use crate::cli::ReleaseCommand;
use crate::config::ProjectConfig;
use crate::git;
use crate::registry::{self, FeatureStatus};
use crate::release_trust::{self, TrustOverrides};

const CRATES_IO_BASE_URL: &str = "https://crates.io/api/v1";

#[derive(Debug)]
pub struct VerifyExit {
    code: i32,
}

impl VerifyExit {
    pub fn code(&self) -> i32 {
        self.code
    }
}

impl fmt::Display for VerifyExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "release verify reported exit code {}", self.code)
    }
}

impl Error for VerifyExit {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CheckStatus {
    Pass,
    Fail,
    Blocked,
}

impl CheckStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CheckResult {
    check: String,
    status: CheckStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<String>,
}

impl CheckResult {
    fn pass(check: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Pass,
            message: message.into(),
            remediation: None,
        }
    }

    fn fail(
        check: impl Into<String>,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Fail,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }

    fn blocked(
        check: impl Into<String>,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Blocked,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct VerifyReport {
    command: &'static str,
    results: Vec<CheckResult>,
    summary: VerifySummary,
}

#[derive(Debug, Clone, Serialize)]
struct VerifySummary {
    pass: usize,
    fail: usize,
    blocked: usize,
    exit_code: i32,
}

pub fn run(command: ReleaseCommand) -> Result<()> {
    reject_non_verify_flags(&command)?;

    let metadata = cargo::metadata_for_current_dir()?;
    let workspace_root = metadata.workspace_root.as_std_path();
    let packages = cargo::select_packages(&metadata, &command.packages, command.workspace)?;
    let version = match command.verify_version.clone() {
        Some(version) => version,
        None => common_current_version(&packages)?,
    };
    let config = ProjectConfig::load(workspace_root)?;
    let mut results = vec![
        check_worktree_clean(workspace_root),
        check_ci_managed(workspace_root),
        check_flake_managed(workspace_root),
        check_release_trust(workspace_root, &config, &command),
        check_changelog(workspace_root, &version),
    ];
    results.extend(check_crates_io(&packages, &version));
    results.push(check_tag(
        workspace_root,
        &version,
        command.push_target.as_deref(),
    ));
    results.push(CheckResult::blocked(
        "remote secrets",
        "remote secrets: CRATES_IO_API_TOKEN presence not verifiable locally",
        "run `simit release secrets` once available; until then verify the remote secret in Forgejo/GitHub settings",
    ));

    let report = report(results);
    if command.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text_report(&report);
    }

    if report.summary.exit_code == 0 {
        Ok(())
    } else {
        Err(VerifyExit {
            code: report.summary.exit_code,
        }
        .into())
    }
}

fn reject_non_verify_flags(command: &ReleaseCommand) -> Result<()> {
    if command.no_tag {
        bail!("--no-tag is not valid with `simit release verify`");
    }
    if command.no_sign {
        bail!("--no-sign is not valid with `simit release verify`");
    }
    if command.dry_run {
        bail!("--dry-run is not valid with `simit release verify`");
    }
    if command.pre.is_some() {
        bail!("--pre is not valid with `simit release verify`");
    }
    if command.message.is_some() {
        bail!("-m/--message is not valid with `simit release verify`");
    }
    if command.no_changelog {
        bail!("--no-changelog is not valid with `simit release verify`");
    }
    if command.push {
        bail!("--push is not valid with `simit release verify`; use --push-target <remote>");
    }
    if command.remote != "origin" {
        bail!("--remote is not valid with `simit release verify`; use --push-target <remote>");
    }
    if command.trust_action.is_some() || command.trust_key.is_some() || command.trust_root.is_some()
    {
        bail!("release trust arguments are only valid with `simit release trust`");
    }
    Ok(())
}

fn check_worktree_clean(workspace_root: &Path) -> CheckResult {
    match Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace_root)
        .output()
    {
        Ok(output) if output.status.success() && output.stdout.is_empty() => {
            CheckResult::pass("worktree clean", "worktree clean")
        }
        Ok(output) if output.status.success() => CheckResult::fail(
            "worktree clean",
            "worktree clean: uncommitted changes present",
            "commit, stash, or discard local changes before release verification",
        ),
        Ok(output) => CheckResult::blocked(
            "worktree clean",
            format!(
                "worktree clean: git status failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            "run `git status --porcelain` and fix the reported git problem",
        ),
        Err(err) => CheckResult::blocked(
            "worktree clean",
            format!("worktree clean: could not run git: {err}"),
            "install git and rerun `git status --porcelain`",
        ),
    }
}

fn check_ci_managed(workspace_root: &Path) -> CheckResult {
    match registry::detect_feature_status(workspace_root)
        .get("ci")
        .copied()
        .unwrap_or(FeatureStatus::Absent)
    {
        FeatureStatus::Managed | FeatureStatus::ManagedExtra => {
            CheckResult::pass("simit ci managed", "simit ci managed (no drift)")
        }
        FeatureStatus::Drift => CheckResult::fail(
            "simit ci managed",
            "simit ci managed: generated workflows have drift",
            "run `simit init ci --check --diff`, then regenerate with the printed command",
        ),
        FeatureStatus::Absent => CheckResult::fail(
            "simit ci managed",
            "simit ci managed: no generated CI workflows found",
            "run `simit init ci --platform forgejo` or the platform appropriate for this repo",
        ),
        FeatureStatus::HandRolled => CheckResult::fail(
            "simit ci managed",
            "simit ci managed: workflows are hand-rolled",
            "adopt simit-managed workflows with `simit init ci --platform <platform>`",
        ),
        other => CheckResult::fail(
            "simit ci managed",
            format!("simit ci managed: unexpected CI state {other:?}"),
            "run `simit projects scan --path .` for feature-state details",
        ),
    }
}

fn check_flake_managed(workspace_root: &Path) -> CheckResult {
    match registry::detect_feature_status(workspace_root)
        .get("flake")
        .copied()
        .unwrap_or(FeatureStatus::Absent)
    {
        FeatureStatus::Managed => {
            CheckResult::pass("simit flake managed", "simit flake managed (no drift)")
        }
        FeatureStatus::Drift => CheckResult::fail(
            "simit flake managed",
            "simit flake managed: generated flake or hook files have drift",
            "run `simit init flake --check --diff`, then regenerate if appropriate",
        ),
        FeatureStatus::Absent => CheckResult::fail(
            "simit flake managed",
            "simit flake managed: generated flake support is missing",
            "run `simit init flake`",
        ),
        other => CheckResult::fail(
            "simit flake managed",
            format!("simit flake managed: unexpected flake state {other:?}"),
            "run `simit projects scan --path .` for feature-state details",
        ),
    }
}

fn check_release_trust(
    workspace_root: &Path,
    config: &ProjectConfig,
    command: &ReleaseCommand,
) -> CheckResult {
    let overrides = TrustOverrides {
        key: command.trust_key.clone(),
        trust_root: command.trust_root.clone(),
    };
    match release_trust::check_quiet(workspace_root, config, &overrides) {
        Ok(()) => CheckResult::pass(
            "release trust root present",
            "release trust root present (keys/maintainers.gpg)",
        ),
        Err(err) => CheckResult::fail(
            "release trust root present",
            format!("release trust root present: {err:#}"),
            "run `simit release trust init` with the correct signing key",
        ),
    }
}

fn check_changelog(workspace_root: &Path, version: &Version) -> CheckResult {
    let path = workspace_root.join(changelog::DEFAULT_PATH);
    match fs::read_to_string(&path) {
        Ok(content) if changelog_has_version_entry(&content, version) => CheckResult::pass(
            "CHANGELOG entry exists",
            format!("CHANGELOG entry exists for {version}"),
        ),
        Ok(_) => CheckResult::fail(
            "CHANGELOG entry exists",
            format!("CHANGELOG entry exists: no released entry for {version}"),
            format!("promote CHANGELOG.md [Unreleased] with `simit changelog release {version}`"),
        ),
        Err(err) => CheckResult::fail(
            "CHANGELOG entry exists",
            format!("CHANGELOG entry exists: could not read CHANGELOG.md: {err}"),
            "create or restore CHANGELOG.md before release verification",
        ),
    }
}

fn changelog_has_version_entry(content: &str, version: &Version) -> bool {
    let bracketed = format!("## [{version}]");
    let bare = format!("## {version}");
    content.lines().any(|line| {
        let trimmed = line.trim();
        (trimmed.starts_with(&bracketed) || trimmed.starts_with(&bare))
            && !trimmed.contains("[Unreleased]")
    })
}

fn check_crates_io(packages: &[Package], version: &Version) -> Vec<CheckResult> {
    packages
        .iter()
        .filter(|package| package.is_publishable())
        .map(|package| check_crate_version(package, version))
        .collect()
}

fn check_crate_version(package: &Package, version: &Version) -> CheckResult {
    match fetch_crate_versions(&package.name, Duration::from_secs(5)) {
        Ok(versions)
            if versions
                .iter()
                .any(|published| published == &version.to_string()) =>
        {
            CheckResult::pass(
                format!("crates.io {}", package.name),
                format!("crates.io: {} {version} already live", package.name),
            )
        }
        Ok(_) => CheckResult::fail(
            format!("crates.io {}", package.name),
            format!("crates.io: {} {version} not yet published", package.name),
            "publish the crate or verify that this pre-release check is being run before publication",
        ),
        Err(CratesIoError::NotFound) => CheckResult::fail(
            format!("crates.io {}", package.name),
            format!("crates.io: {} not found on crates.io", package.name),
            "publish the crate or set `publish = false` if it should not be released",
        ),
        Err(err) => CheckResult::blocked(
            format!("crates.io {}", package.name),
            format!("crates.io: {} reachability blocked: {err}", package.name),
            "check network access to crates.io and rerun `simit release verify`",
        ),
    }
}

fn check_tag(workspace_root: &Path, version: &Version, push_target: Option<&str>) -> CheckResult {
    let tag = version.to_string();
    let local = Command::new("git")
        .args(["tag", "--list", &tag])
        .current_dir(workspace_root)
        .output();
    match local {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => {}
        Ok(output) if output.status.success() => {
            return CheckResult::fail(
                "tag presence",
                format!("tag presence: {tag} not present locally"),
                format!(
                    "create the release tag with `git tag {tag}` or run `simit release sync-up`"
                ),
            );
        }
        Ok(output) => {
            return CheckResult::blocked(
                "tag presence",
                format!(
                    "tag presence: git tag failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                "run `git tag --list <version>` and fix the reported git problem",
            );
        }
        Err(err) => {
            return CheckResult::blocked(
                "tag presence",
                format!("tag presence: could not run git: {err}"),
                "install git and rerun `git tag --list <version>`",
            );
        }
    }

    if let Some(remote) = push_target {
        match git::remote_tag_ref_object(workspace_root, remote, version) {
            Ok(_) => CheckResult::pass(
                "tag presence",
                format!("tag presence: {tag} present locally and on {remote}"),
            ),
            Err(err) => CheckResult::fail(
                "tag presence",
                format!("tag presence: {tag} not found on {remote}: {err:#}"),
                format!("push the tag with `git push {remote} refs/tags/{tag}`"),
            ),
        }
    } else {
        CheckResult::pass(
            "tag presence",
            format!("tag presence: {tag} present locally"),
        )
    }
}

#[derive(Debug)]
enum CratesIoError {
    NotFound,
    Http(u16),
    Command(String),
    Parse(String),
}

impl fmt::Display for CratesIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("crate not found"),
            Self::Http(status) => write!(f, "HTTP {status}"),
            Self::Command(message) => f.write_str(message),
            Self::Parse(message) => f.write_str(message),
        }
    }
}

fn fetch_crate_versions(
    crate_name: &str,
    timeout: Duration,
) -> std::result::Result<Vec<String>, CratesIoError> {
    let url = format!("{CRATES_IO_BASE_URL}/crates/{crate_name}");
    let timeout = timeout.as_secs().max(1).to_string();
    let output = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            &timeout,
            "-A",
            "simit release verify",
            "-w",
            "\n%{http_code}",
            &url,
        ])
        .output()
        .map_err(|err| CratesIoError::Command(format!("could not run curl: {err}")))?;

    if !output.status.success() {
        return Err(CratesIoError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, status) = stdout
        .rsplit_once('\n')
        .ok_or_else(|| CratesIoError::Parse("missing HTTP status from curl".to_owned()))?;
    match status.parse::<u16>() {
        Ok(200) => parse_crates_io_versions(body),
        Ok(404) => Err(CratesIoError::NotFound),
        Ok(status) => Err(CratesIoError::Http(status)),
        Err(err) => Err(CratesIoError::Parse(format!("invalid HTTP status: {err}"))),
    }
}

#[derive(Deserialize)]
struct CratesIoResponse {
    versions: Vec<CratesIoVersion>,
}

#[derive(Deserialize)]
struct CratesIoVersion {
    num: String,
}

fn parse_crates_io_versions(body: &str) -> std::result::Result<Vec<String>, CratesIoError> {
    serde_json::from_str::<CratesIoResponse>(body)
        .map(|response| {
            response
                .versions
                .into_iter()
                .map(|version| version.num)
                .collect()
        })
        .map_err(|err| CratesIoError::Parse(format!("parsing crates.io response: {err}")))
}

fn common_current_version(packages: &[Package]) -> Result<Version> {
    let Some(first) = packages.first() else {
        bail!("no packages selected");
    };
    let first_version = Version::parse(&first.version)
        .with_context(|| format!("parsing version {}", first.version))?;

    for package in packages.iter().skip(1) {
        let version = Version::parse(&package.version)
            .with_context(|| format!("parsing version {}", package.version))?;
        if version != first_version {
            bail!(
                "selected packages do not have one current release version; pass --version to verify a specific version"
            );
        }
    }

    Ok(first_version)
}

fn report(results: Vec<CheckResult>) -> VerifyReport {
    let pass = results
        .iter()
        .filter(|result| result.status == CheckStatus::Pass)
        .count();
    let fail = results
        .iter()
        .filter(|result| result.status == CheckStatus::Fail)
        .count();
    let blocked = results
        .iter()
        .filter(|result| result.status == CheckStatus::Blocked)
        .count();
    let exit_code = if fail > 0 {
        1
    } else if blocked > 0 {
        2
    } else {
        0
    };

    VerifyReport {
        command: "simit release verify",
        results,
        summary: VerifySummary {
            pass,
            fail,
            blocked,
            exit_code,
        },
    }
}

fn print_text_report(report: &VerifyReport) {
    println!("{}", report.command);
    for result in &report.results {
        println!("  [{:<7}] {}", result.status.label(), result.message);
        if let Some(remediation) = &result.remediation {
            println!("            remediation: {remediation}");
        }
    }
    println!(
        "summary: {} fail, {} blocked",
        report.summary.fail, report.summary.blocked
    );
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::{CheckResult, changelog_has_version_entry, parse_crates_io_versions, report};

    #[test]
    fn changelog_entry_must_be_released_version() {
        let version = Version::parse("0.5.1").unwrap();
        assert!(changelog_has_version_entry(
            "## [Unreleased]\n\n## [0.5.1] - 2026-05-25\n",
            &version
        ));
        assert!(!changelog_has_version_entry(
            "## [Unreleased]\n\n- prepare 0.5.1\n",
            &version
        ));
    }

    #[test]
    fn crates_io_response_versions_are_parsed() {
        let versions = parse_crates_io_versions(
            r#"{"crate":{"id":"demo"},"versions":[{"num":"0.5.1"},{"num":"0.5.0"}]}"#,
        )
        .unwrap();
        assert_eq!(versions, ["0.5.1", "0.5.0"]);
    }

    #[test]
    fn exit_code_prefers_fail_over_blocked() {
        let report = report(vec![
            CheckResult::pass("one", "one"),
            CheckResult::blocked("two", "two", "fix two"),
            CheckResult::fail("three", "three", "fix three"),
        ]);
        assert_eq!(report.summary.exit_code, 1);
    }
}
