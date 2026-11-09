//! GitHub Actions REST collection.
//!
//! One pathway row is one job execution; job duration is
//! `completed_at - started_at`. Run-level billable timing is not required.

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::ci_usage::{
    Forge, RepoSpec, Sample, UnitKind, Window,
    http::JsonApi,
};

const PER_PAGE: u32 = 100;
const MAX_RUN_PAGES: u32 = 50;
const MAX_JOB_PAGES: u32 = 10;
/// GitHub returns at most this many runs for one filtered listing.
const GITHUB_RUN_CAP: u64 = 1000;

pub(super) fn collect(spec: &RepoSpec, window: &Window) -> Result<(Vec<Sample>, Vec<String>)> {
    let token = token()?;
    let base = std::env::var("GITHUB_API_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https://api.github.com".into());
    let api = JsonApi::new(
        &base,
        token,
        "GitHub",
        "application/vnd.github+json",
        true,
    )?;
    let repo = format!("{}/{}", spec.owner, spec.name);
    let mut warnings = Vec::new();
    let mut samples = Vec::new();
    for run in runs_in_range(&api, &repo, window.since, window.until, &mut warnings)? {
        samples.extend(collect_jobs(&api, &repo, &run)?);
    }
    Ok((samples, warnings))
}

/// GitHub silently caps a filtered run listing at 1000 results, newest first,
/// so a range over the cap is split in half until each part fits.
fn runs_in_range(
    api: &JsonApi,
    repo: &str,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    warnings: &mut Vec<String>,
) -> Result<Vec<RunRef>> {
    let range = format!(
        "{}..{}",
        since.to_rfc3339_opts(SecondsFormat::Secs, true),
        until.to_rfc3339_opts(SecondsFormat::Secs, true)
    );
    let mut runs = Vec::new();
    let mut fetched = 0_u64;
    let mut total = 0_u64;
    for page in 1..=MAX_RUN_PAGES {
        let path = format!(
            "repos/{repo}/actions/runs?per_page={PER_PAGE}&page={page}&created={}",
            encode(&range)
        );
        let (reported, page_runs) = parse_runs_page(&api.get(&path)?)?;
        // The page past the cap can report total_count = 0; keep the largest count.
        total = total.max(reported);
        if page == 1 && total > GITHUB_RUN_CAP && until - since > chrono::Duration::hours(1) {
            let middle = since + (until - since) / 2;
            let mut runs = runs_in_range(api, repo, since, middle, warnings)?;
            runs.extend(runs_in_range(api, repo, middle, until, warnings)?);
            return Ok(runs);
        }
        let count = page_runs.len() as u64;
        fetched += count;
        // `created=a..b` is inclusive at both ends; keep [since, until) so split
        // halves never count a boundary run twice.
        runs.extend(
            page_runs
                .into_iter()
                .filter(|run| run.created >= since && run.created < until),
        );
        if count < u64::from(PER_PAGE) {
            break;
        }
        ensure!(
            page < MAX_RUN_PAGES,
            "GitHub run pagination cap ({MAX_RUN_PAGES} pages) reached for {repo}; narrow --since/--until"
        );
    }
    if total > fetched {
        warnings.push(format!(
            "github:{repo}: GitHub returned {fetched} of {total} workflow runs created in {range}; the remainder are not counted"
        ));
    }
    Ok(runs)
}

fn collect_jobs(api: &JsonApi, repo: &str, run: &RunRef) -> Result<Vec<Sample>> {
    let mut samples = Vec::new();
    let mut page = 1;
    loop {
        ensure!(
            page <= MAX_JOB_PAGES,
            "GitHub job pagination cap ({MAX_JOB_PAGES} pages) reached for {repo} run {}; narrow --since/--until",
            run.id
        );
        let path = format!(
            "repos/{repo}/actions/runs/{}/jobs?filter=all&per_page={PER_PAGE}&page={page}",
            run.id
        );
        let value = api.get(&path)?;
        let jobs = value["jobs"]
            .as_array()
            .with_context(|| format!("GitHub jobs response for run {} is missing jobs", run.id))?;
        samples.extend(parse_jobs(repo, &run.name, jobs)?);
        if jobs.len() < PER_PAGE as usize {
            break;
        }
        if page == MAX_JOB_PAGES {
            bail!(
                "GitHub job pagination cap ({MAX_JOB_PAGES} pages) reached for {repo} run {}; narrow --since/--until",
                run.id
            );
        }
        page += 1;
    }
    Ok(samples)
}

#[derive(Debug)]
struct RunRef {
    id: u64,
    name: String,
    created: DateTime<Utc>,
}

fn parse_runs_page(value: &Value) -> Result<(u64, Vec<RunRef>)> {
    let total = value["total_count"]
        .as_u64()
        .context("GitHub runs response is missing total_count")?;
    let runs = value["workflow_runs"]
        .as_array()
        .context("GitHub runs response is missing workflow_runs")?;
    let mut parsed = Vec::with_capacity(runs.len());
    for run in runs {
        let id = run["id"]
            .as_u64()
            .context("GitHub workflow run is missing id")?;
        let name = run["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .or_else(|| run["path"].as_str())
            .unwrap_or("(unknown workflow)")
            .to_owned();
        let created = parse_time(run["created_at"].as_str())
            .context("GitHub workflow run is missing created_at")?;
        parsed.push(RunRef { id, name, created });
    }
    Ok((total, parsed))
}

fn parse_jobs(repo: &str, workflow: &str, jobs: &[Value]) -> Result<Vec<Sample>> {
    let mut samples = Vec::new();
    for job in jobs {
        let (Some(started), Some(completed)) = (
            parse_time(job["started_at"].as_str()),
            parse_time(job["completed_at"].as_str()),
        ) else {
            // Queued, in-progress or cancelled jobs have no usable duration.
            continue;
        };
        let seconds = (completed - started).num_milliseconds() as f64 / 1000.0;
        if seconds < 0.0 {
            continue;
        }
        let unit = job["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or("(unnamed job)")
            .to_owned();
        let conclusion = job["conclusion"].as_str().unwrap_or_default();
        let labels = job["labels"]
            .as_array()
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|labels| !labels.is_empty());
        samples.push(Sample {
            forge: Forge::Github,
            repo: repo.to_owned(),
            workflow: workflow.to_owned(),
            unit,
            unit_kind: UnitKind::Job,
            seconds: Some(seconds),
            failed: matches!(conclusion, "failure" | "timed_out" | "startup_failure"),
            oom_killed: false,
            quota_denied: false,
            runner: labels,
        });
    }
    Ok(samples)
}

fn parse_time(raw: Option<&str>) -> Option<DateTime<Utc>> {
    raw.and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|time| time.with_timezone(&Utc))
}

fn token() -> Result<String> {
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(token) = std::env::var(name) {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(token.to_owned());
            }
        }
    }
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .context("reading GitHub token from `gh auth token`")?;
    if output.status.success() {
        let token = String::from_utf8(output.stdout).context("GitHub token was not UTF-8")?;
        let token = token.trim();
        if !token.is_empty() {
            return Ok(token.to_owned());
        }
    }
    bail!(
        "GitHub token is unavailable; set GITHUB_TOKEN or GH_TOKEN, or authenticate with `gh auth login`"
    )
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNS: &str = include_str!("../../tests/fixtures/ci-usage/github-runs.json");
    const JOBS: &str = include_str!("../../tests/fixtures/ci-usage/github-jobs.json");

    #[test]
    fn runs_page_parses_and_window_filters_by_created_at() {
        let value: Value = serde_json::from_str(RUNS).unwrap();
        let (total, runs) = parse_runs_page(&value).unwrap();
        assert_eq!(total, 3);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].id, 1001);
        assert_eq!(runs[0].name, "CI");

        let window = Window {
            since: DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            until: DateTime::parse_from_rfc3339("2026-10-08T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let in_window = runs
            .iter()
            .filter(|run| run.created >= window.since && run.created < window.until)
            .count();
        assert_eq!(in_window, 2);
    }

    #[test]
    fn jobs_parse_duration_labels_and_failure() {
        let value: Value = serde_json::from_str(JOBS).unwrap();
        let jobs = value["jobs"].as_array().unwrap();
        let samples = parse_jobs("caniko/simit", "CI", jobs).unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].workflow, "CI");
        assert_eq!(samples[0].unit, "test (ubuntu-24.04)");
        assert_eq!(samples[0].seconds, Some(3600.0));
        assert_eq!(samples[0].runner.as_deref(), Some("ubuntu-24.04"));
        assert!(!samples[0].failed);
        assert_eq!(samples[1].unit, "clippy");
        assert_eq!(samples[1].seconds, Some(1200.0));
        assert!(samples[1].failed);
        assert_eq!(samples[1].runner.as_deref(), Some("ubuntu-24.04, x64"));
    }
}
